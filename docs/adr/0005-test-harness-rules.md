# Test harness rules

This record holds the non-obvious rules behind the real-server test servers in `crates/test-servers` and the engine and UI tests that drive them: choices, invariants and server behaviour that the code alone doesn't show. Until 2026-11-20 each sat as a comment beside its code; CODE_STANDARDS.md now keeps such rules here, one place per rule.

A change to the code a rule names either keeps the rule or edits it here in the same commit. Rules are grouped by the file that holds the code.

## `crates/app/tests/lab/mod.rs`

- Lab connections saved by crates/app/tests/lab/mod.rs (Server::save_as_lab) turn encryption off and trust the server certificate, because the test servers present self-signed certificates.

## `crates/app/tests/million/mod.rs`

- The MySQL million-row query in crates/app/tests/million/mod.rs (query) cross-joins six copies of the digits 0-9, because MySQL has no generate_series and stops recursive CTEs at cte_max_recursion_depth (1000 by default).

## `crates/app/tests/ui_connections.rs`

- Lab connections use encryption Off and still tick Trust server certificate, because SQL Server encrypts the sign-in packet even with encryption off and the lab servers' certificates are self-signed (crates/app/tests/ui_connections.rs add_and_connect, crates/app/tests/lab/mod.rs save_as_lab).
- Wrong-password tests sign in as the per-test SQL login that dbide-test-servers creates (Mssql::user in crates/test-servers/src/mssql.rs), never sa.
- The first step of add_and_connect in crates/app/tests/ui_connections.rs ends on the Trust server certificate checkbox and then Enter; ui_pg_a_test_result_clears_when_a_detail_changes drops that final Enter to test the connection instead of saving.
- ui_a_folder_that_cant_be_saved_keeps_its_dialog_open_and_adds_no_row makes both connections.toml and its folder read-only, because Windows ignores the read-only flag on folders while other systems block writes through the folder's permissions.

## `crates/app/tests/ui_explorer.rs`

- Each Mssql test handle owns its own database and SQL login, and the app signs in with the source handle's login, so a SQL Server test that reaches a second handle's database must first create a user for that login there (crates/app/tests/ui_explorer.rs, crates/app/tests/ui_search.rs).

## `crates/app/tests/ui_exports.rs`

- Mid-run samples in UI tests (a_million_rows_to_file in crates/app/tests/ui_exports.rs, drive_a_million_rows in crates/app/tests/ui_grid.rs) accept a run that has already finished and only log progress, because a fast machine can finish before the sample is taken.

## `crates/app/tests/ui_format.rs`

- The query editor closes a quote as it is typed, so formats_and_undoes in crates/app/tests/ui_format.rs presses Delete after typing UNCLOSED to keep the string unclosed.

## `crates/app/tests/ui_plans.rs`

- Plan operator names differ in case between engines (MySQL writes "Nested loop inner join", SQL Server and PostgreSQL "Nested Loop"), so assert_join in crates/app/tests/ui_plans.rs matches them case-insensitively.
- SQL Server's actual plan arrives with the query's result set, whose grid comes first in the results pane, so its plan test needs Ctrl+Down to reach the plan; PostgreSQL and MySQL actual plans come without rows (show_plans in crates/app/tests/ui_plans.rs).

## `crates/app/tests/ui_search.rs`

- The explorer filter matches the "Search" row as well as the hit for far_away_quinces, so search_and_script in crates/app/tests/ui_search.rs presses Enter then Down to reach the hit.
- Each Mssql test handle owns its own database and SQL login; ui_mssql_search_finds_a_table_in_an_unopened_database leaves one database without a user for the source login so the search must report it as refused.

## `crates/engine/tests/autocomplete.rs`

- The SQL Server autocomplete test in crates/engine/tests/autocomplete.rs names its schema `many` rather than `bulk`, because BULK is reserved in T-SQL.

## `crates/engine/tests/export.rs`

- crates/engine/tests/export.rs reads XLSX parts through Windows PowerShell's built-in ZIP support (workbook_part) so the tests check real workbook cells without adding a dependency, which needs owner sign-off.

## `crates/engine/tests/mssql.rs`

- SQL Server reports a CAST expression's column as nullable, so tests in crates/engine/tests/mssql.rs expect `nullable: Some(true)` for such columns.

## `crates/engine/tests/mssql_grid_edit.rs`

- Tests that hold a lock from a second session release it before asserting on the blocked operation, so a failing assertion cannot leave a blocked worker that hangs teardown (crates/engine/tests/*_grid_edit.rs).

## `crates/engine/tests/mysql_grid_edit.rs`

- The MySQL text-key save test in crates/engine/tests/mysql_grid_edit.rs locks the first index entry, because an InnoDB scan (rather than a key lookup) for a later row would block on it; this proves Save locates rows by key.

## `crates/engine/tests/objects.rs`

- The PostgreSQL search-all test in crates/engine/tests/objects.rs makes a database refuse connections with ALLOW_CONNECTIONS false, since the harness user is a superuser and a revoked privilege would not produce a real refusal.

## `crates/engine/tests/objects_mysql.rs`

- MySQL 8.4 ignores BINARY on a routine's string parameter that has no charset, so objects::script omits it and the scripted routine keeps the database collation (crates/engine/tests/objects_mysql.rs).

## `crates/engine/tests/objects_mysql_catalog.rs`

- MySQL catalog tests in crates/engine/tests/objects_mysql_catalog.rs tolerate other databases being skipped, because concurrent tests on the shared server and user can drop a database between enumeration and opening it.

## `crates/engine/tests/postgres.rs`

- PostgreSQL names a session's temporary schema pg_temp_<n> with n chosen at connection time, so tests match only the pg_temp_ prefix (crates/engine/tests/postgres.rs).

## `crates/engine/tests/postgres_estimated_transactions.rs`

- In crates/engine/tests/postgres_estimated_transactions.rs the blocking lock stays held while the session is reused, so a dropped estimate that left its plan waiting would make the COMMIT time out.

## `crates/engine/tests/switch_mssql.rs`

- SQL Server switch tests in crates/engine/tests/switch_mssql.rs use fixed database names, so they serialize across test processes with the master-scoped applock dbide_switch_tests.
- create_database in crates/engine/tests/switch_mssql.rs drops any leftover database of the same fixed name first, so a run that died before cleanup cannot break the next one.

## `crates/test-servers/src/mssql.rs`

- The startup sweep in crates/test-servers/src/mssql.rs runs on its own short-lived connection, so its session-owned applocks are released when it closes and never stay attached to a test handle's admin session.
- Mssql::start in crates/test-servers/src/mssql.rs takes the DDL lock and the new database's own lock before creating the login and database, so another process's sweep never sees the objects unprotected.
- The SQL Server harness drains a failed batch with close_query before returning its error, so a retry or a session-owned lock release is not sent on a connection that still has pending results (crates/test-servers/src/mssql.rs).
- sweep in crates/test-servers/src/mssql.rs treats a database as in use when any other session holds locks in it, even from master with three-part names, and ignores sessions of the database's own login, which can linger after a dead owner's lock is released.
- sweep in crates/test-servers/src/mssql.rs skips a database when DROP fails with 3702, because an external session can connect between the in-use check and the drop.
- Forced cleanup in crates/test-servers/src/mssql.rs retries DROP DATABASE on 3702 because KILL can return before the killed session's rollback finishes; the last error is kept for the timeout report.
- Sweep's cleanup in crates/test-servers/src/mssql.rs does not retry DROP DATABASE on 3702, because a three-part-name user can hold locks from master; returning at once lets sweep skip that database without holding the DDL lock.

## `crates/test-servers/src/mssql/wait.rs`

- wait::diagnostics in crates/test-servers/src/mssql/wait.rs applies DIAGNOSTIC_TIMEOUT to the DMV query only; connecting has its own CONNECT_TIMEOUT_SECS budget.
- On SQL Server error 1222 the harness reports wait diagnostics (crates/test-servers/src/mssql/wait.rs) because the waiting statement has ended but the blocking transaction still holds its database locks.

## `crates/test-servers/src/mysql.rs`

- The MySQL harness lowercases test database names in crates/test-servers/src/mysql.rs because MySQL on Windows runs with lower_case_table_names=1 and stores names in lower case.
- The MySQL harness does not capture mysqld's pipes, which stay open for the daemon's lifetime; diagnostics come from its --log-error file (crates/test-servers/src/mysql.rs).
- The MySQL harness sends SQL to the mysql client through stdin so the generated password never appears in process arguments (crates/test-servers/src/mysql.rs).
- The harness's mysql client uses --get-server-public-key because caching_sha2_password over a non-TLS connection needs the server's RSA key (crates/test-servers/src/mysql.rs).

## `crates/test-servers/src/postgres.rs`

- The PostgreSQL harness does not capture pg_ctl's output, because the server inherits and keeps those handles and capturing would wait for the server to exit; the -l log file holds diagnostics (crates/test-servers/src/postgres.rs).

## `crates/test-servers/tests/mssql.rs`

- The test deadline in crates/test-servers/tests/mssql.rs writes straight to stderr because process::exit skips flushing libtest's captured output.
- The test deadline in crates/test-servers/tests/mssql.rs exits the process rather than panicking, because a panic in the watchdog would leave blocked test threads and child waits running.
- In crates/test-servers/tests/mssql.rs the parent locks the crashed child's database before the child exits, so a concurrent sweep cannot delete the fixture before the regression check.
