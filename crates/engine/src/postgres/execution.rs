//! Bounded simple-query events and cancellation cleanup owned independently of a Run.

use std::num::NonZeroU64;
use std::time::Duration;

use futures::StreamExt;
use tokio::sync::mpsc;
use tokio_postgres::{Client, SimpleQueryMessage};

use super::{Notice, Request, cancel::Cancellation, metadata::Metadata, values};
use crate::session::{Event, PlanFormat, Severity};

/// Delivers one event with backpressure, stopping delivery once its Run is cancelled.
pub(super) async fn send(request: &Request, event: Event) {
    tokio::select! {
        biased;
        _ = request.cancel.cancelled() => {}
        _ = request.events.send(event) => {}
    }
}

/// Keeps server SQLSTATE and converts character positions to one-based batch lines.
pub(super) fn error(error: &tokio_postgres::Error, sql: &str) -> Event {
    let db = error.as_db_error();
    Event::Error {
        text: db.map_or_else(|| error.to_string(), |db| db.message().into()),
        line: db.and_then(|db| line(db.position(), sql)),
        code: db.map(|db| db.code().code().into()),
    }
}

fn line(position: Option<&tokio_postgres::error::ErrorPosition>, sql: &str) -> Option<NonZeroU64> {
    if let Some(tokio_postgres::error::ErrorPosition::Original(position)) = position {
        NonZeroU64::new(
            1 + sql
                .chars()
                .take(position.saturating_sub(1) as usize)
                .filter(|ch| *ch == '\n')
                .count() as u64,
        )
    } else {
        None
    }
}

/// Drains through ReadyForQuery even after server errors; Err requires discarding the socket.
pub(super) async fn run(
    client: &Client,
    notices: &mut mpsc::Receiver<Notice>,
    cancellation: &Cancellation,
    request: &Request,
    sql: &str,
    mut metadata: Option<&mut Metadata>,
) -> Result<bool, ()> {
    let plan = metadata.is_none();
    let stream = match client.simple_query_raw(sql).await {
        Ok(stream) => stream,
        Err(failure) => {
            send(request, error(&failure, sql)).await;
            return Ok(false);
        }
    };
    tokio::pin!(stream);
    let mut cancelled = false;
    let mut success = true;
    let mut rows = Vec::with_capacity(256);
    let mut cancel_deadline = tokio::time::Instant::now();
    let mut retry_at = cancel_deadline;
    loop {
        tokio::select! {
            biased;
            _ = request.cancel.cancelled(), if !cancelled => {
                cancelled = true;
                cancel_deadline = tokio::time::Instant::now() + Duration::from_secs(3);
                cancellation.send().await?;
                retry_at = tokio::time::Instant::now() + Duration::from_millis(100);
            }
            _ = tokio::time::sleep_until(cancel_deadline), if cancelled => return Err(()),
            message = stream.next() => {
                match message {
                    Some(Ok(SimpleQueryMessage::RowDescription(columns))) if !plan => {
                        let metadata = metadata.as_mut().expect("ordinary runs have metadata");
                        let columns = tokio::select! {
                            biased;
                            _ = request.cancel.cancelled() => columns.iter().map(values::column).collect(),
                            columns = metadata.columns(&columns) => columns,
                        };
                        send(request, Event::ResultSetStarted { columns }).await;
                    }
                    Some(Ok(SimpleQueryMessage::Row(row))) => {
                        if cancelled { continue; }
                        if plan {
                            if let Some(payload) = row.get(0) {
                                send(request, Event::Plan { payload: payload.into(), format: PlanFormat::ExplainJson }).await;
                            }
                        } else {
                            let values = row.columns().iter().enumerate().map(|(index, column)| {
                                values::value(column.type_oid(), row.get(index))
                            }).collect();
                            rows.push(values);
                            if rows.len() == 256 {
                                send(request, Event::Rows(std::mem::replace(&mut rows, Vec::with_capacity(256)))).await;
                            }
                        }
                    }
                    Some(Ok(SimpleQueryMessage::CommandComplete(_))) if !plan => {
                        if !rows.is_empty() { send(request, Event::Rows(std::mem::take(&mut rows))).await; }
                    }
                    Some(Ok(SimpleQueryMessage::CommandTag(tag))) if !plan => {
                        let mut fields = tag.split_whitespace();
                        if matches!(fields.next(), Some("INSERT" | "UPDATE" | "DELETE" | "MERGE" | "SELECT" | "MOVE" | "FETCH" | "COPY"))
                            && let Some(count) = fields.next_back().and_then(|count| count.parse().ok()) {
                            send(request, Event::RowCount(count)).await;
                        }
                    }
                    Some(Err(failure)) => {
                        if !rows.is_empty() { send(request, Event::Rows(std::mem::take(&mut rows))).await; }
                        success = false;
                        send(request, error(&failure, sql)).await;
                        if failure.as_db_error().is_none() { return Err(()); }
                    }
                    None => break,
                    _ => {}
                }
            }
            Some(notice) = notices.recv() => {
                if !rows.is_empty() { send(request, Event::Rows(std::mem::take(&mut rows))).await; }
                let event = Event::Message {
                    text: notice.error.message().into(),
                    severity: if notice.error.severity() == "WARNING" { Severity::Warning } else { Severity::Info },
                    line: line(notice.error.position(), sql),
                };
                send(request, event).await;
                let _ = notice.acknowledged.send(());
            }
            _ = tokio::time::sleep_until(retry_at), if cancelled => {
                cancellation.send().await?;
                retry_at = tokio::time::Instant::now() + Duration::from_millis(100);
            }
        }
    }
    Ok(success && !cancelled)
}
