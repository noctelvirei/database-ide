//! Worker-owned MySQL Save, including cancellation, drain and transaction recovery.

use std::time::Duration;

use mysql_async::{Conn, Opts, consts::StatusFlags, prelude::Queryable};
use tokio::sync::oneshot;

use crate::grid_edit::{Conflict, Counts, Error, Kind, SaveReport, Script};

/// Returned when the session worker cannot accept or finish Save.
pub(super) fn closed() -> Error {
    Error::Server {
        text: "MySQL connection is closed".into(),
        code: None,
        change: None,
    }
}

fn server(error: mysql_async::Error, password: Option<&str>) -> Error {
    let code = match &error {
        mysql_async::Error::Server(error) => Some(error.code.to_string()),
        _ => None,
    };
    Error::Server {
        text: super::diagnostics::redact(&error.to_string(), password),
        code,
        change: None,
    }
}

/// Owns Save until commit or recovery, even if the caller drops its reply receiver.
pub(super) async fn save(
    conn: &mut Conn,
    options: &Opts,
    script: &Script,
    reply: &mut oneshot::Sender<Result<SaveReport, Error>>,
    timeout: u32,
) -> Result<SaveReport, Error> {
    command(conn, "DO 0").await?;
    let status = conn
        .last_ok_packet()
        .ok_or_else(|| Error::Catalog("missing session status".into()))?
        .status_flags();
    let user_transaction_open = status.contains(StatusFlags::SERVER_STATUS_IN_TRANS)
        || !status.contains(StatusFlags::SERVER_STATUS_AUTOCOMMIT);
    let guard = format!("dbide_save_{:016x}", rand::random::<u64>());
    let begin = if user_transaction_open {
        format!("SAVEPOINT {guard}")
    } else {
        "START TRANSACTION".into()
    };
    command(conn, &begin).await?;
    let result = statements(conn, options, script, reply, timeout, user_transaction_open).await;
    if matches!(&result, Err(Error::Recovery { .. })) {
        return result;
    }
    let rollback = !result
        .as_ref()
        .is_ok_and(|report| report.conflict.is_none());
    let finish = if user_transaction_open {
        if rollback {
            format!("ROLLBACK TO SAVEPOINT {guard}; RELEASE SAVEPOINT {guard}")
        } else {
            format!("RELEASE SAVEPOINT {guard}")
        }
    } else if rollback {
        "ROLLBACK".into()
    } else {
        "COMMIT".into()
    };
    if let Err(recovery) = command(conn, &finish).await {
        if !user_transaction_open && !rollback {
            return match command(conn, "ROLLBACK").await {
                Ok(()) => Err(recovery),
                Err(error) => Err(Error::Recovery {
                    original: Box::new(recovery),
                    recovery: error.to_string(),
                }),
            };
        }
        return Err(Error::Recovery {
            original: Box::new(
                result
                    .err()
                    .unwrap_or_else(|| Error::InvalidChange("Save could not finish".into())),
            ),
            recovery: recovery.to_string(),
        });
    }
    result
}

async fn statements(
    conn: &mut Conn,
    options: &Opts,
    script: &Script,
    reply: &mut oneshot::Sender<Result<SaveReport, Error>>,
    timeout: u32,
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
        let affected = affected(conn, options, statement.sql(), reply, timeout)
            .await
            .map_err(|error| match error {
                Error::Server { text, code, .. } => Error::Server {
                    text,
                    code,
                    change: Some(index),
                },
                error => error,
            })?;
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

async fn command(conn: &mut Conn, sql: &str) -> Result<(), Error> {
    let password = conn.opts().pass().map(str::to_owned);
    tokio::time::timeout(Duration::from_secs(3), conn.query_drop(sql))
        .await
        .map_err(|_| Error::Recovery {
            original: Box::new(Error::Cancelled),
            recovery: "MySQL transaction command timed out".into(),
        })?
        .map_err(|error| server(error, password.as_deref()))
}

async fn affected(
    conn: &mut Conn,
    options: &Opts,
    sql: &str,
    reply: &mut oneshot::Sender<Result<SaveReport, Error>>,
    timeout: u32,
) -> Result<u64, Error> {
    let id = conn.id();
    let password = conn.opts().pass().map(str::to_owned);
    let operation = async {
        conn.query_drop(sql).await?;
        Ok::<_, mysql_async::Error>(conn.affected_rows())
    };
    tokio::pin!(operation);
    let deadline = async {
        if timeout == 0 {
            futures::future::pending::<()>().await;
        }
        tokio::time::sleep(Duration::from_secs(timeout.into())).await;
    };
    let reason = tokio::select! {
        biased;
        result = &mut operation => return result.map_err(|error| server(error, password.as_deref())),
        _ = reply.closed() => Error::Cancelled,
        _ = deadline => Error::Server { text: "MySQL query timed out".into(), code: None, change: None },
    };
    let cleanup = async {
        loop {
            super::kill(options, id).await?;
            tokio::select! {
                biased;
                result = &mut operation => return match result {
                    Err(error) if error.is_fatal() => Err(()),
                    _ => Ok(()),
                },
                _ = tokio::time::sleep(Duration::from_millis(100)) => {}
            }
        }
    };
    match tokio::time::timeout(Duration::from_secs(3), cleanup).await {
        Ok(Ok(())) => Err(reason),
        _ => Err(Error::Recovery {
            original: Box::new(reason),
            recovery: "MySQL Save cancellation could not drain the connection".into(),
        }),
    }
}
