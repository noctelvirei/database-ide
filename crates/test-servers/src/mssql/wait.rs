//! Bounds harness I/O and captures server waits before abandoning a timed-out session.

use super::{ADMIN_DATABASE, Error, connect, query_rows, server_address, tds_message};
use mssql_tds::connection::tds_client::TdsClient;
use mssql_tds::error::Error as TdsError;
use std::time::Duration;

/// DDL can exceed 30 seconds under concurrent load; allow two minutes per query and drain.
pub const QUERY_TIMEOUT: Duration = Duration::from_secs(120);
// Give the independent observer time to read DMVs even while DDL is busy.
const DIAGNOSTIC_TIMEOUT: Duration = Duration::from_secs(15);
// Allow a loaded TLS session to close without retaining its locks indefinitely.
const CLOSE_TIMEOUT: Duration = Duration::from_secs(15);

/// Captures request blockers and application/database lock owners without SQL text or credentials.
pub async fn diagnostics() -> String {
    let snapshot = async {
        let (host, port) = server_address()?;
        let mut observer = connect(&host, port, ADMIN_DATABASE, None).await?;
        // Connection establishment has its own budget, leaving the full DMV budget for reading.
        let sql = "SELECT CONCAT('session=', r.session_id, ' blocker=', r.blocking_session_id, ' wait=', r.wait_type, ' wait_ms=', r.wait_time, ' resource=', r.wait_resource, ' database=', DB_NAME(r.database_id)) FROM sys.dm_exec_requests r WHERE r.session_id <> @@SPID AND (r.blocking_session_id <> 0 OR r.session_id IN (SELECT session_id FROM sys.dm_exec_sessions WHERE program_name = N'Database IDE test harness')); SELECT CONCAT('lock session=', request_session_id, ' type=', resource_type, ' database=', DB_NAME(resource_database_id), ' resource=', resource_description, ' mode=', request_mode, ' status=', request_status) FROM sys.dm_tran_locks WHERE resource_type IN ('APPLICATION', 'DATABASE');";
        tokio::time::timeout(DIAGNOSTIC_TIMEOUT, query_rows(&mut observer, sql))
            .await
            .map_err(|_| {
                Error::Wait(format!(
                    "wait diagnostics query exceeded {} seconds",
                    DIAGNOSTIC_TIMEOUT.as_secs()
                ))
            })?
            .map_err(Error::from)
    };
    match snapshot.await {
        Ok(output) => output,
        Err(error) => format!("wait diagnostics unavailable: {error}"),
    }
}

/// Includes draining in the deadline; a timed-out TDS stream is closed, never reused.
pub(super) async fn query(
    client: &mut TdsClient,
    sql: &str,
    operation: &str,
) -> Result<String, Error> {
    query_with_timeout(client, sql, operation, QUERY_TIMEOUT).await
}

/// Application lock waits have a separate server budget; allow that result to arrive first.
pub(super) async fn query_with_timeout(
    client: &mut TdsClient,
    sql: &str,
    operation: &str,
    timeout: Duration,
) -> Result<String, Error> {
    match tokio::time::timeout(timeout, query_rows(client, sql)).await {
        Ok(Err(TdsError::SqlServerError {
            diagnostics: server,
        })) if server.errors.iter().any(|error| error.number == 1222) => {
            // The waiter has ended, but the blocking transaction still owns its database locks.
            let error = TdsError::SqlServerError {
                diagnostics: server,
            };
            Err(Error::Wait(format!(
                "{operation}: {}; {}",
                tds_message(&error),
                diagnostics().await
            )))
        }
        Ok(result) => result.map_err(Error::from),
        Err(_) => {
            let diagnostics = diagnostics().await;
            let _ = close(client).await;
            Err(Error::Wait(format!(
                "{operation} exceeded {} seconds; {diagnostics}",
                timeout.as_secs()
            )))
        }
    }
}

/// Closing TLS must not retain the session-owned DDL lock indefinitely.
pub(super) async fn close(client: &mut TdsClient) -> Result<(), Error> {
    tokio::time::timeout(CLOSE_TIMEOUT, client.close_connection())
        .await
        .map_err(|_| {
            Error::Wait(format!(
                "closing TDS session exceeded {} seconds",
                CLOSE_TIMEOUT.as_secs()
            ))
        })?
        .map_err(Error::from)
}
