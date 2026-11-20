//! Worker-owned grid Save; DONE counts and trigger results never determine conflicts.

use mssql_tds::connection::tds_client::{ExecuteOptions, ResultSet, StatementResult, TdsClient};
use mssql_tds::core::CancelHandle;
use tokio::sync::oneshot;

use crate::grid_edit::{Conflict, Counts, Error, Kind, SaveReport, Script};
use crate::session::Value;

/// Diagnostic for a worker that cannot accept or finish Save.
pub(super) fn closed() -> Error {
    Error::Server {
        text: "SQL Server connection is closed".into(),
        code: None,
        change: None,
    }
}

fn server(error: mssql_tds::error::Error) -> Error {
    let code = match &error {
        mssql_tds::error::Error::SqlServerError { diagnostics } => diagnostics
            .errors
            .first()
            .map(|error| error.number.to_string()),
        _ => None,
    };
    Error::Server {
        text: error.to_string(),
        code,
        change: None,
    }
}

/// Serializes Save and its recovery even after the caller drops the reply receiver.
pub(super) async fn save(
    client: &mut TdsClient,
    script: &Script,
    reply: &mut oneshot::Sender<Result<SaveReport, Error>>,
    timeout: u32,
) -> Result<SaveReport, Error> {
    let marker = format!("dbide_{:016x}", rand::random::<u64>());
    let guard = format!("dbide_{:016x}", rand::random::<u64>());
    let user_transaction_open = scalar(
        client,
        &format!("SELECT @@TRANCOUNT AS [{marker}]"),
        &marker,
        timeout,
        reply,
    )
    .await?
        > 0;
    let begin = if user_transaction_open {
        format!("SAVE TRANSACTION {guard}")
    } else {
        "BEGIN TRANSACTION; IF @@TRANCOUNT = 2 COMMIT TRANSACTION;".into()
    };
    command(client, &begin).await?;
    let result = statements(
        client,
        script,
        reply,
        &marker,
        timeout,
        user_transaction_open,
    )
    .await;
    let rollback = !result
        .as_ref()
        .is_ok_and(|report| report.conflict.is_none());
    let finish = if rollback && user_transaction_open {
        format!("ROLLBACK TRANSACTION {guard}")
    } else if rollback {
        "IF @@TRANCOUNT > 0 ROLLBACK TRANSACTION".into()
    } else if user_transaction_open {
        return result;
    } else {
        "COMMIT TRANSACTION".into()
    };
    if let Err(recovery) = command(client, &finish).await {
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
    client: &mut TdsClient,
    script: &Script,
    reply: &mut oneshot::Sender<Result<SaveReport, Error>>,
    marker: &str,
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
        let sql = format!(
            "{}\nSELECT CONVERT(bigint, @@ROWCOUNT) AS [{marker}];",
            statement.sql()
        );
        let affected = scalar(client, &sql, marker, timeout, reply)
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

async fn command(client: &mut TdsClient, sql: &str) -> Result<(), Error> {
    super::execution::idle(client, sql).await.map_err(server)
}

async fn scalar(
    client: &mut TdsClient,
    sql: &str,
    marker: &str,
    timeout: u32,
    reply: &mut oneshot::Sender<Result<SaveReport, Error>>,
) -> Result<u64, Error> {
    let mut cancel = CancelHandle::new();
    cancel.set_cancel_timeout_secs(3);
    let result = {
        let read = read_scalar(client, sql, marker, timeout, &cancel);
        tokio::pin!(read);
        tokio::select! {
            result = &mut read => result,
            _ = reply.closed() => {
                cancel.cancel();
                let _ = read.await;
                Err(Error::Cancelled)
            }
        }
    };
    let cleanup = client.close_query().await.map_err(server);
    client.take_info_messages();
    match cleanup {
        Ok(()) => result,
        Err(recovery) => Err(Error::Recovery {
            original: Box::new(
                result
                    .err()
                    .unwrap_or_else(|| Error::Catalog("Save result could not finish".into())),
            ),
            recovery: recovery.to_string(),
        }),
    }
}

async fn read_scalar(
    client: &mut TdsClient,
    sql: &str,
    marker: &str,
    timeout: u32,
    cancel: &CancelHandle,
) -> Result<u64, Error> {
    let mut boundary = client
        .execute(
            sql.into(),
            ExecuteOptions::new().cancel(cancel).timeout_secs(timeout),
        )
        .await
        .map_err(server)?;
    let mut found = None;
    loop {
        match boundary {
            StatementResult::End => break,
            StatementResult::Rows => {
                let columns = client.get_metadata().clone();
                let matches = columns.len() == 1 && columns[0].column_name == marker;
                while let Some(row) = client.next_row().await.map_err(server)? {
                    if matches {
                        let value = row
                            .into_iter()
                            .next()
                            .ok_or_else(|| Error::Catalog("missing Save count".into()))?;
                        let Value::Integer(count) =
                            super::values::value(value, columns[0].data_type)
                        else {
                            return Err(Error::Catalog("invalid Save count".into()));
                        };
                        if found.is_some() {
                            return Err(Error::Catalog("duplicate Save count".into()));
                        }
                        found = Some(
                            u64::try_from(count)
                                .map_err(|_| Error::Catalog("negative Save count".into()))?,
                        );
                    }
                }
            }
            _ => {}
        }
        boundary = client.advance().await.map_err(server)?;
    }
    found.ok_or_else(|| Error::Catalog("missing Save count".into()))
}
