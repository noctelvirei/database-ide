# Database IDE: first-version spec

The decisions behind each section are closed tickets linked from the [wayfinder map](https://github.com/noctelvirei/database-ide/issues/1); this file gathers them into one buildable whole. Vocabulary is in [CONTEXT.md](../CONTEXT.md). This file is authoritative; the tickets record why.

## What it is

A Windows-first desktop SQL IDE in Rust for a DBA's daily work on SQL Server (first), PostgreSQL and MySQL. Open source under MIT OR Apache-2.0.

Targets, each a pass/fail check:
- Opens in under 1 second.
- Under 100 MB private working set (summed over its processes) for the idle app with every feature loaded, before any database work.
- Installer under 20 MB.

Out of scope for v1: monitoring (activity, Agent jobs, Query Store), schema designer, admin, MySQL plans, Entra ID sign-in, read-only mode, full context-aware autocomplete, SSH tunnels, Windows sign-in as another account, macOS and Linux builds.

## Build order

1. **Engine** (this spec's main part): a UI-free Rust library plus a small command-line harness, built by Codex from Linear tickets, reviewed and merged by Claude. Testable with no window.
2. **UI**: gpui on top of the engine's interface, built afterwards in a Claude session.

## Workspace layout

- `crates/engine`: the library. Owns connections, drivers, runs, the spill store, plans as data, exports, danger checks, grid-edit scripts, object metadata and Script as. No UI dependencies.
- `crates/dbide-cli`: the E2E harness. Drives the engine through its public API the way the UI will, against real servers, and writes artifacts (result files, exports, plan trees as JSON) a rerun reproduces.
- `crates/app` (later): the gpui UI.

Two crates because the engine must build and test with no UI dependencies; the CLI is a separate binary that the tests run.

## Engine interface

One shape for every engine ([drivers decision](https://github.com/noctelvirei/database-ide/issues/9)):

- `Session`: one open connection to one server, with a current database. Created from a saved connection.
- `Session::run(batch) -> Run`: sends SQL and returns a stream of events plus a cancel handle.
- Events, in order: result set started (columns: name, type, nullability, base table and base column when known), row batches (written to the spill store as they arrive), message (PRINT/RAISERROR/notice/warning, with severity and line), row count, plan, error (with line number), done (elapsed time).
- A cancel handle that works from another task while rows are streaming.
- Engine-specific features (`.sqlplan` files, Script as) are methods of the engine's own module, not forced into the common shape.

Each engine is one module behind that interface:

| Engine | Driver | Notes |
|---|---|---|
| SQL Server | `mssql-tds`, pinned exactly | SSPI Negotiate for Windows sign-in; `GO` batch splitting is the engine's job; `SET SHOWPLAN_XML` / `SET STATISTICS XML` sent as batches of their own; never wrap user SQL in `sp_executesql`; TLS `mode = On` plus trust server certificate for self-signed servers |
| PostgreSQL | our fork of `tokio-postgres` ([ADR 0001](adr/0001-own-tokio-postgres-fork.md)) | simple-query protocol for user batches (type OIDs via PR #1358); notices from the connection; `CancelToken`; domain sign-in via PR #1379 switched to Negotiate |
| MySQL | `mysql_async` | multi-statement results, warnings, affected rows; cancel is `KILL QUERY <id>` from a second short connection |

TLS for PostgreSQL and MySQL uses `native-tls` (Windows certificate store). Runtime: Tokio.

## Results: spill to disk

Every row of a run streams into a compact temp file as it arrives, with a row-offset index. The UI's grid and every export read rows by index from that file. Memory stays flat whatever the row count, and the query finishes and releases its locks without waiting for the user. Never fetch-as-you-scroll from the server. The temp file is deleted when its result is closed.

## Plans

- Estimated and actual plans for SQL Server and PostgreSQL ([plan viewer research](https://github.com/noctelvirei/database-ide/issues/6)).
- The engine returns, per statement, the raw plan payload (showplan XML or EXPLAIN JSON) plus one parsed operator tree shared by both engines: operator name and kind, estimated and actual rows, cost and cost %, timings, I/O, warnings, children, and every raw property for the details panel.
- Parse showplan XML leniently: unknown elements and attributes are kept as raw properties, never errors (Microsoft's published XSD stops at SQL Server 2022).
- PostgreSQL actual plans use `EXPLAIN (ANALYZE, BUFFERS, FORMAT JSON)`; for statements that change data, wrap in `BEGIN … ROLLBACK`.
- Open and save `.sqlplan` files.

## Exports

Clipboard text with headers, CSV, `.xlsx` (constant-memory writing), INSERT script. All read from the spill store. Run to file streams a run's rows straight into an export file without a grid.

## Connections

[Decision](https://github.com/noctelvirei/database-ide/issues/11): a password-free `connections.toml` in the user's app-data folder; passwords in Windows Credential Manager via `keyring`, or ask every time; nestable connection folders carrying connection colour and the danger check, inherited with per-connection override; import SSMS Registered Servers (`RegSrvr.xml` and `.regsrvr`); paste ADO.NET, libpq or JDBC connection strings. Fields: engine, host, port or instance, sign-in method, user, default database, colour, TLS mode, trust server certificate, application name, connect and query timeouts.

## Danger check

[Decision](https://github.com/noctelvirei/database-ide/issues/12): parse with `sqlparser-rs` in the engine's dialect; fall back to keyword rules over a comment- and string-aware tokenizer when parsing fails, and say the batch wasn't fully checked. Flag UPDATE/DELETE with no or trivially true WHERE, DROP, TRUNCATE, ALTER TABLE … DROP COLUMN, MERGE with a delete-by-source branch, RESTORE DATABASE, ALTER DATABASE … SET OFFLINE / SINGLE_USER / EMERGENCY, DBCC … REPAIR_ALLOW_DATA_LOSS, SHUTDOWN. The engine returns flagged statements, reasons, whether the batch parsed fully, and an estimated row count for UPDATE/DELETE from the estimated plan.

## Grid editing

[Decision](https://github.com/noctelvirei/database-ide/issues/10): the engine decides whether a result is an editable result (one base table, a primary or unique key present; base-table lookup per engine as in the drivers decision), turns pending changes into one UPDATE/INSERT/DELETE script with optimistic WHERE clauses, runs it in one transaction on the tab's session, and reports per-row edit conflicts (any statement not affecting exactly one row rolls the whole Save back).

## Object explorer and Script as

Per engine: databases, schemas, tables, views, procedures, functions, indexes; a table's columns, keys and indexes; filter and search across databases; Script as CREATE / ALTER / SELECT from the system catalogs.

## UI (built later)

gpui + gpui-component pinned exactly, with our public fork for the find panel's Replace keys ([toolkit decision](https://github.com/noctelvirei/database-ide/issues/8)). Layout C "Workbench" ([layout decision](https://github.com/noctelvirei/database-ide/issues/13)). Plan view: [plan view prototype](https://github.com/noctelvirei/database-ide/issues/14). Autocomplete: keywords plus table and column names.

## Testing

[Decision](https://github.com/noctelvirei/database-ide/issues/16): real servers are the test. SQL Server 2025 Developer (Manual start; a fresh `dbide_test_<random>` database per test, dropped after, with a leftover sweep); PostgreSQL 18 and MySQL 8.4 as throwaway servers per test run in a temp folder on a free port, from binaries the harness finds through `DBIDE_PG_BIN` and `DBIDE_MYSQL_BIN`; invented data from scripts in the repo; `mssql-mock-tds` only for injected failures. Kerberos: the Pi lab ([lab decision](https://github.com/noctelvirei/database-ide/issues/18)) for PostgreSQL, a manual pre-release checklist for SQL Server.
