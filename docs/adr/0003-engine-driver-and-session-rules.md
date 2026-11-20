# Engine driver and session rules

This record holds the non-obvious rules behind the engine modules that talk to SQL Server, PostgreSQL and MySQL, the session interface and the spill store: choices, invariants and server behaviour that the code alone doesn't show. Until 2026-11-20 each sat as a comment beside its code; CODE_STANDARDS.md now keeps such rules here, one place per rule.

A change to the code a rule names either keeps the rule or edits it here in the same commit. Rules are grouped by the file that holds the code.

## `crates/engine/src/mssql.rs`

- In mssql.rs connect_error, database-unavailable numbers are checked before login failures because SQL Server can report a missing default database together with error 18456; checking login first would misreport it as bad credentials.
- mssql-tds parallel connects turn io::Error into a string, so mssql.rs connect_error recovers the OS error number from the '(os error N)' suffix and maps it through ConnectError::io.
- mssql-tds reports SQL Server Browser (SSRP) no-answer and timeout only as strings, so mssql.rs connect_error matches their text; because the Browser lookup uses its own short limit rather than the saved connect timeout, it is classed as an unreachable instance, not a timeout.

## `crates/engine/src/mssql/execution.rs`

- Plan runs in mssql/execution.rs turn the showplan setting OFF with a separate, uncancelled command after errors, cancellation or a dropped event consumer, so the session does not keep returning plans for later runs.
- In mssql/execution.rs query, info messages that arrive with a DONE token belong after every row already read, so buffered rows are flushed before the messages are sent to keep event order matching the server.
- mssql/execution.rs query checks for cancellation after each result and sends ATTENTION if a batch is still open, because cancellation can arrive while a bounded event send was blocked and nothing else would send it.

## `crates/engine/src/mssql/grid_edit.rs`

- SQL Server cannot roll back a doomed (uncommittable) user transaction to a savepoint, so when the finishing command fails mssql/grid_edit.rs save returns Error::Recovery and the session is discarded rather than reused.
- When the Save caller goes away, mssql/grid_edit.rs keeps polling the driver read after requesting cancel so the ATTENTION exchange completes before rollback and session reuse; dropping the read would leave the TDS stream mid-response.

## `crates/engine/src/mssql/values.rs`

- SQL Server datetime counts 1/300-second ticks; mssql/values.rs rounds them to the millisecond precision SQL Server displays.

## `crates/engine/src/mysql.rs`

- mysql.rs request applies database changes only from batches that completed or were never sent; a batch whose worker still holds its Arc may yet record a USE, so it stays queued.
- On a MySQL run timeout, mysql.rs sets timed_out and lets the event stream report it at exhaustion instead of sending an event, so a full event channel cannot delay KILL QUERY.
- mysql_async must not have an in-flight protocol read dropped, so after cancel or timeout mysql.rs keeps polling the original future while repeating KILL QUERY, and discards the session if it does not finish within 3 seconds.

## `crates/engine/src/mysql/diagnostics.rs`

- mysql/diagnostics.rs statements skips empty statements because MySQL returns no result for them; counting them would misalign statement indexes with server results.
- MySQL strips whitespace after a statement delimiter but keeps comments as part of the next statement, so mysql/diagnostics.rs next_line skips only whitespace when computing the next statement's line.

## `crates/engine/src/mysql/execution.rs`

- mysql/execution.rs records a USE statement's database as soon as its OK packet is read, because the next stream poll may block on a following query and the database change must be visible meanwhile.
- mysql_async prefetches the next result header: an OK packet replaces the warning count of a preceding SELECT, while a following row-set header leaves it intact, so mysql/execution.rs reads warnings from the result only in the latter case.

## `crates/engine/src/mysql/grid_edit.rs`

- mysql/grid_edit.rs save runs DO 0 before reading transaction status flags because a server error clears mysql_async's cached OK packet, which would otherwise give stale or missing flags.
- KILL QUERY can reach the server before the query it targets, so mysql/grid_edit.rs keeps polling the original read and repeats KILL until the read drains or 3 seconds pass.

## `crates/engine/src/mysql/plans.rs`

- MySQL actual plans in mysql/plans.rs run only SELECT, UPDATE, DELETE and INSERT; the allowlist also keeps out transaction control, CALL and administrative SQL that commits implicitly, which the rollback could not undo.
- mysql/plans.rs uses explain_json_format_version 2 because version 1 cannot carry iterator runtime counters for EXPLAIN ANALYZE; version 2 serves both estimated and actual plans.
- explain_json_format_version 2 gives no estimates for single-table UPDATE or DELETE without an iterator plan, so mysql/plans.rs explain switches to version 1 for those estimated plans.
- mysql/plans.rs sends EXPLAIN through a server-side prepared statement so the server enforces a single statement, even if sql_mode changes quoting so the parser's statement split differs from the server's.
- The ROLLBACK after an actual plan in mysql/plans.rs explain is reached after KILL QUERY or a dropped event consumer because the mysql.rs worker keeps polling the plan future to completion.

## `crates/engine/src/postgres.rs`

- postgres.rs connect resolves addresses itself and sets hostaddr on each attempt, keeping host unchanged: connect_raw would discard the host, and hostaddr pins the cancel request to the same server while the host name still drives the SPN for Windows sign-in.
- The postgres.rs connection task stops reading after each notice until the session acknowledges it, so notices cannot overtake query messages that preceded them on the wire.
- In postgres.rs connect_error, SQLSTATE 28P01 means a wrong password, but 28000 also covers pg_hba.conf rejecting the client whatever its password, so for password sign-in 28000 keeps the server's own text instead of claiming bad credentials.
- tokio-postgres has no public accessor for its timeout and authentication error kinds, so postgres.rs connect_error classifies them by their Display text.
- tokio-postgres exposes no typed missing-password cause (SCRAM, MD5 and cleartext all give only the source text 'password missing'), so postgres.rs connect_error matches that text.
- tokio-postgres keeps its authentication error kind private, so postgres.rs connect_error recognises a Windows sign-in failure by the Display text 'authentication error'.
- A PostgreSQL run in postgres.rs reports closed() only if the worker never marked it handled; the flag is read after the event channel closes, which is safe because the worker sets it before dropping the sender.
- postgres.rs switch_database passes the database name unquoted because dbname is a startup protocol value, not SQL text.
- When the postgres.rs worker exits, requests still queued are dropped without being marked handled, so each of their runs ends with the closed() error.

## `crates/engine/src/postgres/cancel.rs`

- The tokio-postgres fork shuts down the cancel socket's write half without waiting for EOF, so postgres/cancel.rs keeps the read half and reads until the server closes it; otherwise the cancel could land on the next statement.
- PostgreSQL on Windows closes the cancel socket with a reset instead of a FIN, so after the request is written postgres/cancel.rs treats a reset or abort as the server closing it.

## `crates/engine/src/postgres/execution.rs`

- postgres/execution.rs run polls query messages before the notice channel so that, together with the connection task's notice barrier, events keep wire order.
- A PostgreSQL cancel can reach the backend before the queued query starts and be ignored, so postgres/execution.rs run resends it every 100 ms until the run ends or the 3-second deadline passes.

## `crates/engine/src/postgres/grid_edit.rs`

- postgres/grid_edit.rs save and postgres/plans.rs detect a user transaction by trying SAVEPOINT: SQLSTATE 25P01 means no transaction is open; a Save returns any other error rather than guessing.
- postgres/grid_edit.rs save releases its savepoint after ROLLBACK TO as well as after success, because ROLLBACK TO keeps the savepoint; releasing it leaves the user's earlier work in their transaction intact.
- A failed COMMIT (for example a serialization failure) can leave a PostgreSQL transaction open, so postgres/grid_edit.rs save sends ROLLBACK and reports a recovery failure if that fails too.

## `crates/engine/src/postgres/metadata.rs`

- postgres/metadata.rs drops its metadata connection when a lookup times out or fails, so later lookups do not queue behind a query still running on it.

## `crates/engine/src/postgres/plans.rs`

- Actual plans in postgres/plans.rs check for a user transaction with SAVEPOINT, which fails harmlessly with 25P01 outside one, and refuse to run inside one so the plan never issues BEGIN within the user's transaction.
- postgres/plans.rs sends each statement's original text, padded with newlines to its batch line, inside EXPLAIN instead of re-rendering the parsed AST, so literals keep their spelling and server error positions map to batch lines.
- Actual plans in postgres/plans.rs always send ROLLBACK after EXPLAIN ANALYZE, on errors and cancellation too, and this runs even if the event consumer is gone because the postgres.rs worker awaits plans::run to completion rather than the consumer.

## `crates/engine/src/session.rs`

- Connection errors in crates/engine/src/session.rs redact the password from the final message, not only the diagnostic, because a supplied name or address may itself contain the password.
- certificate_failure in crates/engine/src/session.rs matches the formatted "(os error ...)" text because native-tls exposes those Schannel statuses only in the io::Error display.

## `crates/engine/src/spill.rs`

- Store completion in crates/engine/src/spill.rs is published with Release ordering; a reader that observes complete with Acquire also sees every row count published before it.
- Reader::rows in crates/engine/src/spill.rs reopens the spill file for each read so the read has its own cursor; a cloned File would share one cursor between concurrent reads.
