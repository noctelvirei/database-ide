//! Statement boundaries, bounded row delivery and independent attention cleanup.

use std::num::NonZeroU64;

use mssql_tds::connection::tds_client::{
    BatchErrorMode, ExecuteOptions, ResultSet, StatementResult, TdsClient,
};
use mssql_tds::core::CancelHandle;

use super::{Request, batches, values};
use crate::session::{Event, PlanFormat, PlanMode, Severity};

/// Processes a run; failure means the transport cannot safely be reused.
pub(super) async fn run(client: &mut TdsClient, request: &Request, timeout: u32) -> Result<(), ()> {
    let batches = match batches::split(&request.batch) {
        Ok(batches) => batches,
        Err(error) => {
            send(
                request,
                Event::Error {
                    text: error.to_string(),
                    line: None,
                    code: None,
                },
            )
            .await;
            return Ok(());
        }
    };
    let setting = request.mode.map(|mode| match mode {
        PlanMode::Estimated => "SHOWPLAN_XML",
        PlanMode::Actual => "STATISTICS XML",
    });
    if let Some(setting) = setting
        && let Err(error) = idle(client, &format!("SET {setting} ON")).await
    {
        errors(request, error, 0).await;
        return if client.is_connection_dead() {
            Err(())
        } else {
            Ok(())
        };
    }
    let mut healthy = true;
    for batch in batches {
        for _ in 0..batch.repeat {
            if request.cancel.is_cancelled() {
                break;
            }
            if query(client, request, batch.sql, batch.line_offset, timeout)
                .await
                .is_err()
            {
                healthy = false;
                break;
            }
        }
        if !healthy || request.cancel.is_cancelled() {
            break;
        }
    }
    if healthy
        && let Some(setting) = setting
        && let Err(error) = idle(client, &format!("SET {setting} OFF")).await
    {
        errors(request, error, 0).await;
        healthy = false;
    }
    if healthy { Ok(()) } else { Err(()) }
}

async fn query(
    client: &mut TdsClient,
    request: &Request,
    sql: &str,
    offset: u64,
    timeout: u32,
) -> Result<(), ()> {
    let mut cancel = CancelHandle::from(request.cancel.clone());
    cancel.set_cancel_timeout_secs(3);
    let options = ExecuteOptions::new()
        .cancel(&cancel)
        .timeout_secs(timeout)
        .on_error(BatchErrorMode::Continue);
    let mut boundary = client.execute(sql.into(), options).await;
    loop {
        messages(client, request, offset).await;
        match boundary {
            Ok(StatementResult::End) => return Ok(()),
            Ok(StatementResult::NoRows { rows_affected }) => {
                if let Some(count) = rows_affected {
                    send(request, Event::RowCount(count)).await;
                }
            }
            Ok(StatementResult::Rows) => {
                let columns = client.get_metadata().clone();
                let plan = request.mode.is_some()
                    && columns.len() == 1
                    && columns[0].column_name == "Microsoft SQL Server 2005 XML Showplan";
                if !plan {
                    send(
                        request,
                        Event::ResultSetStarted {
                            columns: columns.iter().map(values::column).collect(),
                        },
                    )
                    .await;
                }
                let mut rows = Vec::with_capacity(256);
                loop {
                    let next = client.next_row().await;
                    if !client.info_messages().is_empty() {
                        flush(request, &mut rows).await;
                        messages(client, request, offset).await;
                    }
                    match next {
                        Ok(Some(row)) => {
                            let row: Vec<_> = row
                                .into_iter()
                                .zip(&columns)
                                .map(|(value, column)| values::value(value, column.data_type))
                                .collect();
                            if plan {
                                if let Some(crate::session::Value::Text(payload)) =
                                    row.into_iter().next()
                                {
                                    send(
                                        request,
                                        Event::Plan {
                                            payload,
                                            format: PlanFormat::ShowplanXml,
                                        },
                                    )
                                    .await;
                                }
                            } else {
                                rows.push(row);
                                if rows.len() == 256 {
                                    flush(request, &mut rows).await;
                                }
                            }
                        }
                        Ok(None) => {
                            flush(request, &mut rows).await;
                            break;
                        }
                        Err(error) => {
                            flush(request, &mut rows).await;
                            let recoverable = recoverable(client, &error);
                            errors(request, error, offset).await;
                            if !recoverable {
                                return Err(());
                            }
                            break;
                        }
                    }
                }
                if !plan && let Some(count) = client.last_result_row_count() {
                    send(request, Event::RowCount(count)).await;
                }
            }
            Err(error) => {
                let recoverable = recoverable(client, &error);
                errors(request, error, offset).await;
                if !recoverable {
                    return Err(());
                }
            }
        }
        if request.cancel.is_cancelled() {
            if client.has_open_batch() && client.cancel_query(3).await.is_err() {
                return Err(());
            }
            return if client.is_connection_dead() {
                Err(())
            } else {
                Ok(())
            };
        }
        boundary = client.advance().await;
    }
}

fn recoverable(client: &TdsClient, error: &mssql_tds::error::Error) -> bool {
    !client.is_connection_dead()
        && matches!(
            error,
            mssql_tds::error::Error::SqlServerError { .. }
                | mssql_tds::error::Error::OperationCancelledError(_)
                | mssql_tds::error::Error::TimeoutError(_)
        )
}

async fn flush(request: &Request, rows: &mut Vec<Vec<crate::session::Value>>) {
    if !rows.is_empty() {
        send(
            request,
            Event::Rows(std::mem::replace(rows, Vec::with_capacity(256))),
        )
        .await;
    }
}

/// Sends until cancellation, then drops events while the worker drains the protocol.
pub(super) async fn send(request: &Request, event: Event) {
    tokio::select! { biased; _ = request.cancel.cancelled() => {}, _ = request.events.send(event) => {} }
}

fn line(number: Option<i32>, offset: u64) -> Option<NonZeroU64> {
    number
        .and_then(|line| u64::try_from(line).ok())
        .filter(|line| *line > 0)
        .and_then(|line| NonZeroU64::new(line + offset))
}

async fn messages(client: &mut TdsClient, request: &Request, offset: u64) {
    for message in client.take_info_messages() {
        send(
            request,
            Event::Message {
                text: message.message,
                severity: Severity::Server(message.class.clamp(0, 255) as u8),
                line: line(message.line_number, offset),
            },
        )
        .await;
    }
}

async fn errors(request: &Request, error: mssql_tds::error::Error, offset: u64) {
    match error {
        mssql_tds::error::Error::SqlServerError { diagnostics } => {
            for error in diagnostics.errors {
                send(
                    request,
                    Event::Error {
                        text: error.message,
                        code: Some(error.number.to_string()),
                        line: line(error.line_number, offset),
                    },
                )
                .await;
            }
        }
        error => {
            send(
                request,
                Event::Error {
                    text: error.to_string(),
                    code: None,
                    line: None,
                },
            )
            .await
        }
    }
}

/// Drains housekeeping without producing user results or inheriting run cancellation.
pub(super) async fn idle(client: &mut TdsClient, sql: &str) -> mssql_tds::core::TdsResult<()> {
    client
        .execute(sql.into(), ExecuteOptions::new().timeout_secs(5))
        .await?;
    client.close_query().await?;
    client.take_info_messages();
    Ok(())
}

#[cfg(test)]
mod tests {
    #[test]
    fn error_line_adds_batch_offset_once() {
        assert_eq!(super::line(Some(4), 5).unwrap().get(), 9);
        assert_eq!(super::line(Some(0), 5), None);
        assert_eq!(super::line(None, 5), None);
    }
}
