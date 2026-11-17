//! Bounded text results; cleanup remains owned by the worker when a Run disappears.

use futures::StreamExt;
use mysql_async::{Conn, Row, prelude::Queryable};

use super::{Request, diagnostics, values};
use crate::session::{Event, Severity};

/// Consumer backpressure never prevents cancellation or protocol cleanup.
pub(super) async fn send(request: &Request, event: Event) {
    tokio::select! {
        biased;
        _ = request.cancel.cancelled() => {}
        _ = request.stopped.cancelled() => {}
        _ = request.events.send(event) => {}
    }
}

/// Streams one unchanged batch and drains recoverable errors; Err discards the connection.
pub(super) async fn run(conn: &mut Conn, request: &Request) -> Result<(), ()> {
    let statements = &request.database_changes.statements;
    let password = conn.opts().pass().map(str::to_owned);
    let mut statement = 0;
    if let Err(error) = results(conn, request, statements, &mut statement).await {
        send(
            request,
            diagnostics::error(&error, statements.get(statement), password.as_deref()),
        )
        .await;
        if error.is_fatal() {
            return Err(());
        }
    }
    Ok(())
}

async fn results(
    conn: &mut Conn,
    request: &Request,
    statements: &[diagnostics::Statement],
    index: &mut usize,
) -> mysql_async::Result<()> {
    let password = conn.opts().pass().map(str::to_owned);
    let mut result = conn.query_iter(&request.batch).await?;
    loop {
        let Some(mut stream) = result.stream::<Row>().await? else {
            break;
        };
        let columns = stream.columns();
        let has_rows = !columns.is_empty();
        let affected = stream.affected_rows();
        let warnings = stream.get_warnings();
        // USE's OK packet is already known; the next stream poll may wait on another query.
        if !has_rows
            && let Some(database) = statements
                .get(*index)
                .and_then(|statement| statement.database.as_ref())
        {
            let _ = request.database_changes.names[*index].set(database.clone());
        }
        if has_rows {
            send(
                request,
                Event::ResultSetStarted {
                    columns: columns.iter().map(values::column).collect(),
                },
            )
            .await;
        }
        let mut rows = Vec::with_capacity(256);
        let mut count = 0;
        while let Some(row) = stream.next().await {
            match row {
                Ok(row) if !request.stopped.is_cancelled() && !request.cancel.is_cancelled() => {
                    count += 1;
                    rows.push(
                        columns
                            .iter()
                            .zip(row.unwrap())
                            .map(|(column, cell)| values::value(column, cell))
                            .collect(),
                    );
                    if rows.len() == 256 {
                        send(
                            request,
                            Event::Rows(std::mem::replace(&mut rows, Vec::with_capacity(256))),
                        )
                        .await;
                    }
                }
                Ok(_) => {}
                Err(error) => {
                    if !rows.is_empty() {
                        send(request, Event::Rows(std::mem::take(&mut rows))).await;
                    }
                    return Err(error);
                }
            }
        }
        drop(stream);
        if !rows.is_empty() {
            send(request, Event::Rows(rows)).await;
        }
        if has_rows {
            send(request, Event::RowCount(count)).await;
        } else if statements
            .get(*index)
            .is_some_and(|statement| statement.changes_rows)
            || affected != 0
        {
            send(request, Event::RowCount(affected)).await;
        }
        let next_columns = result.columns();
        // The driver prefetches the next header. Its OK packet replaces a SELECT's
        // terminal warnings; a following row-set header leaves those warnings intact.
        let warnings = if has_rows {
            if next_columns
                .as_ref()
                .is_none_or(|columns| !columns.is_empty())
            {
                result.warnings()
            } else {
                0
            }
        } else {
            warnings
        };
        if warnings != 0 && (next_columns.is_some() || *index + 1 < statements.len()) {
            // shortcut: the server overwrites intermediate diagnostics in an unsplit batch;
            // retrieve their text if MySQL adds per-statement diagnostic history to its protocol.
            send(request, Event::Message {
                text: format!("{warnings} MySQL warning(s); diagnostic text is unavailable for an intermediate statement in a multi-statement batch"),
                severity: Severity::Warning, line: None,
            }).await;
        }
        if !has_rows
            || !statements
                .get(*index)
                .is_some_and(|statement| statement.multiple_results)
        {
            *index += 1;
        }
    }
    drop(result);
    if conn.get_warnings() != 0 && !request.stopped.is_cancelled() && !request.cancel.is_cancelled()
    {
        let warnings: Vec<(String, u16, String)> = conn.query("SHOW WARNINGS").await?;
        for (level, code, text) in warnings {
            send(
                request,
                Event::Message {
                    text: diagnostics::redact(&format!("{code}: {text}"), password.as_deref()),
                    severity: if level == "Note" {
                        Severity::Info
                    } else {
                        Severity::Warning
                    },
                    line: None,
                },
            )
            .await;
        }
    }
    Ok(())
}
