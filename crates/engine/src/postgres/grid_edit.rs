//! Worker-owned Save transactions; cleanup outlives a dropped caller future.

use futures::StreamExt;
use tokio::sync::{mpsc, oneshot};
use tokio_postgres::{Client, SimpleQueryMessage};

use super::{Notice, idle_query};
use crate::grid_edit::{Conflict, Counts, Error, Kind, SaveReport, Script};

/// Diagnostic returned when a worker cannot accept or finish a Save.
pub(super) fn closed() -> Error {
    Error::Server {
        text: "PostgreSQL connection is closed".into(),
        code: None,
        change: None,
    }
}

fn server(error: tokio_postgres::Error, change: Option<usize>) -> Error {
    Error::Server {
        text: error
            .as_db_error()
            .map_or_else(|| error.to_string(), |db| db.message().into()),
        code: error.code().map(|code| code.code().into()),
        change,
    }
}

/// Owns the transaction through recovery even when the reply receiver disappears.
pub(super) async fn save(
    client: &Client,
    notices: &mut mpsc::Receiver<Notice>,
    script: &Script,
    reply: &oneshot::Sender<Result<SaveReport, Error>>,
) -> Result<SaveReport, Error> {
    let guard = format!("dbide_save_{:016x}", rand::random::<u64>());
    let user_transaction_open =
        match idle_query(client, notices, &format!("SAVEPOINT {guard}")).await {
            Ok(()) => true,
            Err(error) if error.code().is_some_and(|code| code.code() == "25P01") => {
                idle_query(client, notices, "BEGIN")
                    .await
                    .map_err(|error| server(error, None))?;
                false
            }
            Err(error) => return Err(server(error, None)),
        };
    let result = statements(client, notices, script, reply, user_transaction_open).await;
    let must_rollback = !result
        .as_ref()
        .is_ok_and(|report| report.conflict.is_none());
    let cleanup = if user_transaction_open {
        if must_rollback {
            idle_query(client, notices, &format!("ROLLBACK TO SAVEPOINT {guard}")).await
        } else {
            Ok(())
        }
    } else if must_rollback {
        idle_query(client, notices, "ROLLBACK").await
    } else {
        idle_query(client, notices, "COMMIT").await
    };
    let cleanup = match cleanup {
        Ok(()) if user_transaction_open => {
            idle_query(client, notices, &format!("RELEASE SAVEPOINT {guard}")).await
        }
        cleanup => cleanup,
    };
    if let Err(error) = cleanup {
        if !user_transaction_open && !must_rollback {
            let original = server(error, None);
            return match idle_query(client, notices, "ROLLBACK").await {
                Ok(()) => Err(original),
                Err(recovery) => Err(Error::Recovery {
                    original: Box::new(original),
                    recovery: recovery.to_string(),
                }),
            };
        }
        return Err(Error::Recovery {
            original: Box::new(
                result
                    .err()
                    .unwrap_or_else(|| Error::InvalidChange("Save could not finish".into())),
            ),
            recovery: error.to_string(),
        });
    }
    result
}

async fn statements(
    client: &Client,
    notices: &mut mpsc::Receiver<Notice>,
    script: &Script,
    reply: &oneshot::Sender<Result<SaveReport, Error>>,
    user_transaction_open: bool,
) -> Result<SaveReport, Error> {
    let mut report = SaveReport {
        user_transaction_open,
        ..SaveReport::default()
    };
    for (index, statement) in script.statements().iter().enumerate() {
        if reply.is_closed() {
            return Err(Error::Cancelled);
        }
        let affected = affected(client, notices, statement.sql())
            .await
            .map_err(|error| server(error, Some(index)))?;
        if affected != 1 {
            report.rows = Counts::default();
            report.conflict = Some(Conflict {
                change: index,
                affected,
            });
            return Ok(report);
        }
        match statement.kind() {
            Kind::Update => report.rows.updated += 1,
            Kind::Insert => report.rows.inserted += 1,
            Kind::Delete => report.rows.deleted += 1,
        }
    }
    if reply.is_closed() {
        return Err(Error::Cancelled);
    }
    Ok(report)
}

async fn affected(
    client: &Client,
    notices: &mut mpsc::Receiver<Notice>,
    sql: &str,
) -> Result<u64, tokio_postgres::Error> {
    let stream = client.simple_query_raw(sql).await?;
    tokio::pin!(stream);
    let mut count = 0;
    let mut failure = None;
    loop {
        tokio::select! {
            biased;
            message = stream.next() => match message {
                Some(Ok(SimpleQueryMessage::CommandComplete(affected))) => count += affected,
                Some(Err(error)) => failure = Some(error),
                None => break,
                _ => {}
            },
            Some(notice) = notices.recv() => { let _ = notice.acknowledged.send(()); }
        }
    }
    match failure {
        Some(error) => Err(error),
        None => Ok(count),
    }
}
