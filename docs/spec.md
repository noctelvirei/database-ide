# Database IDE: first-version spec

The decisions behind each section are closed tickets linked from the [wayfinder map](https://github.com/noctelvirei/database-ide/issues/1); this file gathers them into one buildable whole. Vocabulary is in [CONTEXT.md](../CONTEXT.md). This file is authoritative; the tickets record why.

## What it is

A Windows-first desktop SQL IDE in Rust for a DBA's daily work on SQL Server (first), PostgreSQL and MySQL. Open source under MIT OR Apache-2.0.

Targets, each a pass/fail check:
- Opens in under 1 second.
- Under 100 MB private working set (summed over its processes) for the idle app with every feature loaded, before any database work.
- Installer under 20 MB.

Out of scope for v1: monitoring (activity, Agent jobs, Query Store), schema designer, admin, Entra ID sign-in, read-only mode, full context-aware autocomplete, SSH tunnels, Windows sign-in as another account, macOS and Linux builds.

## Build order

1. **Engine** (this spec's main part): a UI-free Rust library plus a small command-line harness, built by Codex from Linear tickets, reviewed and merged by Claude. Testable with no window.
2. **UI**: gpui on top of the engine's interface, built by Claude from Linear tickets (project "UI v1"). Engine additions the UI needs, such as the formatter, go to Codex as engine tickets.

## Workspace layout

- `crates/engine`: the library. Owns connections, drivers, runs, the spill store, plans as data, exports, danger checks, grid-edit scripts, object metadata, Script as and formatting. No UI dependencies.
- `crates/dbide-cli`: the E2E harness. Drives the engine through its public API the way the UI will, against real servers, and writes artifacts (result files, exports, plan trees as JSON) a rerun reproduces.
- `crates/app`: the gpui UI. Holds no database logic: everything it shows comes through the engine's public API.

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

- Estimated and actual plans for SQL Server, PostgreSQL and MySQL ([plan viewer research](https://github.com/noctelvirei/database-ide/issues/6)).
- The engine returns, per statement, the raw plan payload (showplan XML or EXPLAIN JSON) plus one parsed operator tree shared by all three engines: operator name and kind, estimated and actual rows, cost and cost %, timings, I/O when reported, warnings, children, and every raw property for the details panel.
- Parse showplan XML leniently: unknown elements and attributes are kept as raw properties, never errors (Microsoft's published XSD stops at SQL Server 2022).
- PostgreSQL actual plans use `EXPLAIN (ANALYZE, BUFFERS, FORMAT JSON)`; for statements that change data, wrap in `BEGIN … ROLLBACK`.
- MySQL 8.4 plans use `EXPLAIN FORMAT=JSON` and `EXPLAIN ANALYZE FORMAT=JSON` with session `explain_json_format_version=2`, restored afterwards ([iterator JSON format](https://dev.mysql.com/blog-archive/new-json-format-for-explain/)). Actual plans run in a transaction rolled back on completion, error or cancellation. They refuse DDL and other non-explainable statements before execution, and refuse an existing transaction or disabled autocommit. Nontransactional effects (such as writes to MyISAM tables or external effects from stored functions) cannot be rolled back. Rows and elapsed times are totals over actual loops; the raw properties retain the per-loop counters and timings.
- MySQL 8.4 measures joined UPDATE inputs but [suppresses the UPDATE writes](https://github.com/mysql/mysql-server/blob/8.4/sql/sql_update.cc); a single-table UPDATE can return `<not executable by iterator executor>`. The transaction guard still protects transactional side effects from stored functions invoked by an actual plan.
- Open and save SQL Server `.sqlplan` files only.

## Exports

Clipboard text with headers, CSV, `.xlsx` (constant-memory writing), INSERT script. All read from the spill store. Run to file streams a run's rows straight into an export file without a grid.

- Ctrl+S on a grid saves its result set; the file's extension picks the format: `.csv` (UTF-8 with a byte-order mark, so Excel reads it), `.xlsx`, or `.sql` for INSERT statements in the connection's dialect. The INSERTs target the result's base table, or the file's name when the server reports none (SQL Server, for a plain SELECT). Saving over a file replaces it only once the new one is complete.
- Ctrl+Shift+F turns results to file on for the tab (and include actual plan off). F5 then asks for a `.csv` or `.xlsx` file; `name.csv` gives `name-1.csv`, `name-2.csv`, one per result set. The run bar shows the rows written so far; Stop or Alt+Break cancels, leaving an unfinished file as `.partial`. The messages strip says what each file holds.

## Connections

[Decision](https://github.com/noctelvirei/database-ide/issues/11): a password-free `connections.toml` in the user's app-data folder; passwords in Windows Credential Manager via `keyring`, or ask every time; nestable connection folders carrying connection colour and the danger check, inherited with per-connection override; import SSMS Registered Servers (`RegSrvr.xml` and `.regsrvr`); paste ADO.NET, libpq or JDBC connection strings. Fields: engine, host, port or instance, sign-in method, user, default database, colour, TLS mode, trust server certificate, application name, connect and query timeouts.

## Danger check

[Decision](https://github.com/noctelvirei/database-ide/issues/12): parse with `sqlparser-rs` in the engine's dialect; fall back to keyword rules over a comment- and string-aware tokenizer when parsing fails, and say the batch wasn't fully checked. Flag UPDATE/DELETE with no or trivially true WHERE, DROP, TRUNCATE, ALTER TABLE … DROP COLUMN, MERGE with a delete-by-source branch, RESTORE DATABASE, ALTER DATABASE … SET OFFLINE / SINGLE_USER / EMERGENCY, DBCC … REPAIR_ALLOW_DATA_LOSS, SHUTDOWN. The engine returns flagged statements, reasons, whether the batch parsed fully, and an estimated row count for UPDATE/DELETE from the estimated plan.

## Grid editing

[Decision](https://github.com/noctelvirei/database-ide/issues/10): the engine decides whether a result is an editable result (one base table, a primary or unique key present; base-table lookup per engine as in the drivers decision), turns pending changes into one UPDATE/INSERT/DELETE script with optimistic WHERE clauses, runs it in one transaction on the tab's session, and reports per-row edit conflicts (any statement not affecting exactly one row rolls the whole Save back).

## Object explorer and Script as

Per engine: databases, schemas, tables, views, procedures, functions, indexes; a table's columns, keys and indexes; filter and search across databases; Script as CREATE / ALTER / SELECT from the system catalogs.

## Formatter

The engine formats SQL text for one engine's dialect. It changes layout and the case of structural keywords only; identifiers, literals and comments are copied byte for byte.

- Tokens come from `sqlparser`'s tokenizer in the engine's dialect (brackets, backticks, `$tag$` quotes, `N'…'` literals and comments included). No formatter crate: `sqlformat` corrupts strings with backslashes and has no MySQL dialect; `sqlparser`'s printer drops comments.
- Keyword case: only a fixed list of structural keywords reserved in all three engines is upper-cased (`SELECT`, `FROM`, `WHERE`, `JOIN`, `ON`, `AND`, `OR`, `GROUP`, `ORDER`, `BY`, `HAVING`, `INSERT`, `INTO`, `VALUES`, `UPDATE`, `SET`, `DELETE`, `CREATE`, `ALTER`, `DROP` and the like). `sqlparser` marks many non-reserved words as keywords (`name`, `type`, `status`, `user`), and changing their case can change meaning under a case-sensitive collation, so every word not on the list keeps its bytes.
- Style: 4-space indent; `SELECT`, `FROM`, `WHERE`, `GROUP BY`, `HAVING`, `ORDER BY` each start a line with their items indented beneath, one per line, trailing commas; `JOIN … ON` on one line with further `AND`/`OR` conditions indented beneath; subqueries indented one level; one blank line between statements; a statement that fits in 80 characters stays on one line. Batch separators (`GO`) stay on their own line.
- A `--` line comment always ends its line; the one-line rule never joins code after it.
- Two string literals separated only by whitespace that holds a line break stay on separate lines: PostgreSQL reads `'a'` newline `'b'` as one string `'ab'` but rejects `'a' 'b'`. The formatter never joins them and never splits adjacent literals that share a line.
- Guard: the output is tokenized again and compared with the input, ignoring spaces and line breaks and the case of the listed keywords, except that whether a line break separates two adjacent string literals must match. Comments are compared byte for byte. Any other difference, or input that doesn't tokenize (an unclosed string), returns an error and the original text.
- Formats any fragment, so a selection formats on its own.

## UI

gpui + gpui-component pinned exactly, with our public fork for the find panel's Replace keys ([toolkit decision](https://github.com/noctelvirei/database-ide/issues/8)). Layout C "Workbench" ([layout decision](https://github.com/noctelvirei/database-ide/issues/13)): vertical query tabs grouped by connection with the object explorer below them, the editor with an always-visible messages strip, results or plan below, and a permanent inspector on the right. Keyboard-complete: every action has a shortcut or is reachable from the keyboard, and every workbench action is listed in the menu bar.

### Editor and autocomplete

gpui-component's code editor with tree-sitter SQL highlighting, find/replace and multi-cursor. Build with only the SQL grammar (all bundled grammars made the toolkit test's executable 57.5 MB). Autocomplete offers keywords plus the tab's current database's schemas, tables, views, routines and columns; after `name.` it offers the columns of the table called `name` or the objects of the schema called `name`. Aliases are not resolved (full context-aware autocomplete is out of scope). The object list loads from the engine's object metadata on first use, is cached per connection and database, and is refreshed with Ctrl+Shift+R.

### Runs and results

- A run sends the selection, or the whole tab when nothing is selected.
- Result sets stack vertically in one scrolling results pane, each grid with its own header and row count; any one can be maximised to fill the pane. Grids page rows from the spill store.
- The messages strip shows a run's messages in order: PRINT/RAISERROR/notices, row counts ("(12 rows affected)"), and errors in red. An error's line number maps back to the editor line, across `GO` batches and a selection's offset; clicking it moves the cursor there.
- Cancel with Alt+Break or the Stop button; rows already received stay visible and the run is marked cancelled.
- No query timeout by default (SSMS's 0); the connection's query timeout applies when set.
- Dangerous statements open a confirm dialog listing each flagged statement, its reason and the estimated rows, and saying when the batch wasn't fully checked. No "don't ask again".

### Plans

The Plan tab opens in a tree table (operator, object, estimated and actual rows, estimate error, cost %, time, reads; misestimates of 10× or more flagged) ([decision](https://github.com/noctelvirei/database-ide/issues/14)). F9 switches to an SSMS-style graph (vscode-mssql icons, right-to-left, arrow width by actual rows). Estimated rows are multiplied by executions or loops before comparing with actual rows. Selecting an operator fills the inspector with every raw property.

### Keyboard

SSMS's shortcuts, not remappable in v1:

| Keys | Action |
|---|---|
| F5, Ctrl+E | Run |
| Alt+Break | Cancel the run |
| Ctrl+L | Estimated plan |
| Ctrl+M | Toggle include actual plan |
| Ctrl+Shift+F | Toggle results to file |
| F9 | Plan tree ↔ graph |
| Ctrl+R | Hide or show results |
| Ctrl+N | New query tab on the current connection |
| Ctrl+Tab | Next query tab |
| Ctrl+K Ctrl+C / Ctrl+K Ctrl+U | Comment / uncomment lines |
| Ctrl+K Ctrl+F | Format the selection, or the whole tab |
| Ctrl+F / Ctrl+H | Find / replace |
| Ctrl+Shift+R | Refresh autocomplete |
| Ctrl+U | Pick the current database |
| F8 | Focus the object explorer |
| Ctrl+O / Ctrl+S | Open / save a `.sql` file |
| Ctrl+S in a grid / plan | Save the result set / the plan to a file |

### Menu bar

A Windows menu bar across the top of the window, so every action can be found without knowing its key. It is a row of menu titles of our own, each opening gpui-component's `PopupMenu` built from the same actions the keymap binds; each item shows its shortcut, and an item that can't act now (Cancel with no run going) is greyed out. gpui-component's `AppMenuBar` was the first choice, but it cannot open a given menu from Alt+its letter, so the bar is ours; a fork exposing that would let us switch back.

- **File:** New query, New connection, Open, Exit.
- **Edit:** Undo, Redo, Cut, Copy, Paste, Select all, Find, Replace, Format. In a grid, Copy and Select all act on the grid.
- **View:** Object explorer, Results (a tick shows when shown), Next pane, Previous pane.
- **Query:** Run, Cancel, Estimated plan, Include actual plan and Results to file (a tick shows when on), Change database.
- **Help:** Keyboard shortcuts (a panel listing every menu item's keys, read from the keymap), About (version, licences, the repository link).

Alt+the underlined letter opens a menu, as in SSMS, and F10 opens File; Up, Down, Enter and Escape move through it, and Left and Right move to the next menu. Plain Alt alone does not focus the bar. No Tools menu in v1, since there are no settings to show. Each item lands with the feature behind it: Save, Comment and Uncomment, Plan tree ↔ graph and Refresh autocomplete join their menus when those are built, and later Save as, recent files and query history (File and View), plan files, grid edit mode, and Check for updates (Help).

### Formatting

Ctrl+K Ctrl+F formats the selection, or the whole tab, through the engine's formatter, as one undo step. A refusal leaves the text unchanged and says why in the messages strip. Never formats on save.

### Files, restored tabs and history

- `.sql` files open as UTF-8 or UTF-16, detected from the byte-order mark, and save in the encoding they were opened in. New files are UTF-8 without a BOM.
- Every tab's text is saved to the app-data folder a second after typing stops and restored on start with its connection and database. No "save changes?" prompt on exit; a tab with a file shows a dot while it differs from the file.
- Query history: each run's text, connection, database, start time, duration and outcome (rows or error) is appended to a history file in the app-data folder. A searchable history panel reopens an entry in a new tab. Entries older than 90 days are dropped.

### Packaging and updates

- A per-user MSI (no admin prompt) and a portable zip, built with LTO, stripped, SQL grammar only; checked against the 20 MB installer target.
- Unsigned in v1; SmartScreen warns on first install.
- Once a day the app checks GitHub Releases; a newer version shows a notice with a link. Nothing downloads or installs on its own.

## Testing

[Decision](https://github.com/noctelvirei/database-ide/issues/16): real servers are the test. SQL Server 2025 Developer (Manual start; a fresh `dbide_test_<random>` database per test, dropped after, with a leftover sweep); PostgreSQL 18 and MySQL 8.4 as throwaway servers per test run in a temp folder on a free port, from binaries the harness finds through `DBIDE_PG_BIN` and `DBIDE_MYSQL_BIN`; invented data from scripts in the repo; `mssql-mock-tds` only for injected failures. Kerberos: the Pi lab ([lab decision](https://github.com/noctelvirei/database-ide/issues/18)) for PostgreSQL, a manual pre-release checklist for SQL Server.
