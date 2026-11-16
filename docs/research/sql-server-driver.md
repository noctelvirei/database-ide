# SQL Server driver: tiberius, mssql-tds or ODBC

Research for issue #4, 2026-11-16. Question: which SQL Server driver should Database IDE use, checked against Windows sign-in, actual and estimated plans, multiple result sets with messages, streaming, cancel, TLS, named instances, types, maintenance and licence.

## Recommendation

Use **`mssql-tds`** (Microsoft's pure-Rust TDS client, [microsoft/mssql-rs](https://github.com/microsoft/mssql-rs)), pinned to an exact version behind a thin adapter module in the engine.

It is the only option where every requirement on the list is met by public API: Windows sign-in through the SSPI `Negotiate` package (Kerberos with NTLM fallback), statement-by-statement navigation with row counts, PRINT/RAISERROR messages, an SSMS-style "continue after error" mode, a cancel handle that another task can fire, `TrustServerCertificate`, named instances via SQL Browser, and LocalDB. It ships a mock TDS server crate that lets Codex test the UI-free engine natively on Windows without a SQL Server.

tiberius fails two requirements outright in 0.13.0: Windows sign-in is NTLM only, and the query stream discards PRINT/RAISERROR messages and per-statement row counts. ODBC through `odbc-api` can do everything, but only by dropping to `unsafe` raw-handle code for messages, statement-wise navigation and cancel, and it needs Microsoft's closed-source driver installed on every machine.

The main risk with `mssql-tds` is churn: four breaking minor releases between 2026-09-10 (0.1.0) and 2026-11-07 (0.4.0). That is why the version is pinned and wrapped.

## Options compared

| | tiberius 0.13.0 | mssql-tds 0.4.0 | odbc-api 30.1.0 + ODBC Driver 18 |
|---|---|---|---|
| Windows integrated sign-in | NTLM only on Windows (source) | SSPI `Negotiate`, so Kerberos or NTLM (source) | `Trusted_Connection=yes`, SSPI in the driver (docs) |
| Actual plan (`SET STATISTICS XML ON`) | Arrives as an extra result set; readable (inferred from multi-result support) | Same; each statement is its own navigable result (inferred) | Same, via `SQLMoreResults` (inferred) |
| Estimated plan (`SET SHOWPLAN_XML ON`) | Readable; send `SET` as its own batch with `simple_query` (inferred) | Readable; `execute` sends a SQL batch (source, inferred) | Readable (inferred) |
| Multiple result sets | Yes, `QueryItem::Metadata` per set (source) | Yes, `advance()` / `advance_to_rows()` (source) | Yes, but high-level API returns no cursor when the first statement has no columns (source) |
| PRINT / RAISERROR messages | Not exposed; INFO tokens dropped (source) | `take_info_messages()` (source) | Only logged through `log`; raw `Diagnostics` trait is public (source, docs) |
| Per-statement row counts | Not in the query stream; `execute` only, which returns no rows (source) | `StatementResult::NoRows { rows_affected }` (source) | `row_count()` (source) |
| Errors mid-batch | First error only, at end of stream; later errors lost (source) | `BatchErrorMode::Continue` reaches statements after the error (source) | Per `SQLMoreResults` diagnostics (docs) |
| Streaming rows | Yes, `Stream` of rows decoded token by token (source) | Yes, `next_row()`; PLP chunk reads for large values (source) | Yes, block cursors (source) |
| Cancel (attention) | `cancel_query(&mut self)`: the row stream must be dropped first; open bug #459 desyncs the connection (source, issue) | `CancelHandle` in `ExecuteOptions`, `cancel(&self)` from another task (source, docs) | `SQLCancel` from another thread is in the ODBC spec; `odbc-api` does not wrap it, #962 open (docs, issue) |
| TLS, `TrustServerCertificate` | Yes, `trust_cert()`; TDS 8.0 `Strict` too (source) | Yes in `On`/`Required`; ignored in `Strict`, default is `Strict` (source, docs) | Yes; `Encrypt` defaults to `yes` in Driver 18 (docs) |
| Named instances | `SqlBrowser::connect_named` with `sql-browser-tokio` feature (source) | SSRP built in; LocalDB and named pipes too (source) | `Server=host\instance` (docs) |
| datetimeoffset, money, xml | Yes (source) | Yes (source) | Yes, via driver (not checked in detail) |
| sql_variant | Yes, decoded (source) | Yes, decoded to base type (source) | Driver type `SQL_SS_VARIANT` (not checked in detail) |
| geography / CLR UDT | Raw bytes (source) | Raw bytes (source) | Raw bytes (not checked in detail) |
| Large varbinary | Whole value per row (source) | Whole value, or chunked PLP reads (source) | `SQLGetData` in parts (not checked in detail) |
| Builds on this PC (rustc 1.93, MSVC) | Yes (run) | Yes (run) | Yes (run) |
| Testing without SQL Server | None shipped | `mssql-mock-tds` (MIT) mock server (crate) | None; needs driver and server |
| Maintenance | Dormant 2024-07 to 2026-08; revived by one outside contributor; RustSec asked about status (repo, issue) | Microsoft team, 100 commits in the last 3 months; 0.x with breaking releases (repo, crates.io) | One maintainer, active; stable API at major 30 (repo) |
| Licence | MIT or Apache-2.0 | MIT | `odbc-api` MIT; ODBC Driver 18 is proprietary, installed separately |

"source" means read in the crate source at that version; "docs" means Microsoft Learn or crate docs; "run" means checked on this PC; "inferred" means it follows from verified facts but was not run against a server.

## Verified facts

### Windows integrated sign-in

- **tiberius.** `AuthMethod::Integrated` on Windows builds a `winauth::NtlmSspiBuilder` ([connection.rs L589-L593](https://docs.rs/crate/tiberius/0.13.0/source/src/client/connection.rs#589)). winauth 0.0.5 passes the SSPI package name `NTLM` to `AcquireCredentialsHandleA` ([windows.rs L19](https://docs.rs/crate/winauth/0.0.5/source/src/windows.rs#19), [L107-L109](https://docs.rs/crate/winauth/0.0.5/source/src/windows.rs#107)); the comment beside that call says "negotiate", but the constant is `b"NTLM\0"`. So there is no Kerberos on Windows: no delegation to linked servers, and no sign-in where a domain has NTLM restricted. On Unix, Kerberos needs the `integrated-auth-gssapi` feature ([auth.rs L68-L90](https://docs.rs/crate/tiberius/0.13.0/source/src/client/auth.rs#68)).
- **mssql-tds.** `TdsAuthenticationMethod::SSPI` ([client_context.rs L166-L170](https://docs.rs/crate/mssql-tds/0.4.0/source/src/connection/client_context.rs#166)). The default `SecurityPackage` is `Negotiate` ([security_context.rs L46-L52](https://docs.rs/crate/mssql-tds/0.4.0/source/src/security/security_context.rs#46)), passed to `acquire_credentials` ([sspi_context.rs L104-L107](https://docs.rs/crate/mssql-tds/0.4.0/source/src/security/windows/sspi_context.rs#104)). The SPN is built from server and port unless one is given. The `integrated-auth` feature is on by default ([README](https://docs.rs/crate/mssql-tds/0.4.0/source/README.md)).
- **ODBC.** `Trusted_Connection` / `SQL_COPT_SS_INTEGRATED_SECURITY` is listed for Windows, Linux and macOS in [Microsoft's keyword table](https://learn.microsoft.com/en-us/sql/connect/odbc/dsn-connection-string-attribute).

### Plans

- `SET STATISTICS XML ON` makes the server execute each statement and then return one XML document per statement, as `nvarchar(max)`; it "need not be the only statement in a batch" ([Microsoft Learn](https://learn.microsoft.com/en-us/sql/t-sql/statements/set-statistics-xml-transact-sql)). It also needs SHOWPLAN permission, or the statement is aborted.
- `SET SHOWPLAN_XML ON` returns one document per batch as `nvarchar(max)`, does not execute the statements, "cannot be specified inside a stored procedure" and "must be the only statement in a batch" ([Microsoft Learn](https://learn.microsoft.com/en-us/sql/t-sql/statements/set-showplan-xml-transact-sql)).
- So plans need no driver feature beyond reading every result set of a batch and keeping session `SET` state between batches. Two consequences for the engine: send `SET … ON` as its own SQL batch on the same session, and do not wrap user SQL in `sp_executesql`, because `SET` options set inside a procedure revert when it returns. tiberius `simple_query` ([client.rs L275](https://docs.rs/crate/tiberius/0.13.0/source/src/client.rs#275)) and mssql-tds `execute` ([tds_client.rs L1875](https://docs.rs/crate/mssql-tds/0.4.0/source/src/connection/tds_client.rs#1875), `SqlBatch::new`) both send a SQL batch packet. tiberius `query` uses `sp_executesql`.

### Multiple result sets, messages and row counts

- **tiberius.** `QueryStream` yields only `Metadata` and `Row`; every other token, including INFO, DONE and ERROR, hits `_ => continue` ([query.rs L426](https://docs.rs/crate/tiberius/0.13.0/source/src/tds/stream/query.rs#426)). The INFO token is marked "TODO(surface): expose INFO messages to callers" ([token.rs L147-L150](https://docs.rs/crate/tiberius/0.13.0/source/src/tds/stream/token.rs#147)). The first server error is kept and returned only at end of stream ([token.rs L470-L476](https://docs.rs/crate/tiberius/0.13.0/source/src/tds/stream/token.rs#470)). Row counts exist only on `execute`'s `ExecuteResult` ([result.rs L74](https://docs.rs/crate/tiberius/0.13.0/source/src/result.rs#74)), which returns no rows. A Messages pane needs a fork.
- **mssql-tds.** `execute` positions on the first "navigable result"; `advance()` moves statement by statement, "matching msodbcsql's `SQLMoreResults`" ([tds_client.rs L1803-L1830](https://docs.rs/crate/mssql-tds/0.4.0/source/src/connection/tds_client.rs#1803), [L6385-L6398](https://docs.rs/crate/mssql-tds/0.4.0/source/src/connection/tds_client.rs#6385)). Each step returns `StatementResult::Rows`, `NoRows { rows_affected }` or `End` ([L10045](https://docs.rs/crate/mssql-tds/0.4.0/source/src/connection/tds_client.rs#10045)). PRINT and low-severity RAISERROR text comes from `info_messages()` / `take_info_messages()`, kept even when the command errored ([L9019-L9045](https://docs.rs/crate/mssql-tds/0.4.0/source/src/connection/tds_client.rs#9019)). `ExecuteOptions::on_error = BatchErrorMode::Continue` returns a statement's error and leaves the batch open for the next statement ([L9938-L9970](https://docs.rs/crate/mssql-tds/0.4.0/source/src/connection/tds_client.rs#9938)), which is what SSMS shows.
- **odbc-api.** `Connection::execute` returns `None` when the first statement has no result columns ([execute.rs L128-L131](https://docs.rs/crate/odbc-api/30.1.0/source/src/execute.rs#128)), and `more_results` lives on the cursor ([cursor.rs L99-L103](https://docs.rs/crate/odbc-api/30.1.0/source/src/cursor.rs#99)), so `UPDATE …; SELECT …` loses the `SELECT` through the high-level API. `SQL_SUCCESS_WITH_INFO` diagnostics are logged and dropped ([error.rs L193-L201](https://docs.rs/crate/odbc-api/30.1.0/source/src/error.rs#193)). The `handles` module, the `Diagnostics` trait and `odbc_sys` are public ([lib.rs L32, L71](https://docs.rs/crate/odbc-api/30.1.0/source/src/lib.rs#32)), so the engine could drive `SQLExecDirect` / `SQLMoreResults` / `SQLGetDiagRec` itself in `unsafe` code. The SQL Server driver returns PRINT as `SQL_SUCCESS_WITH_INFO` with SQLSTATE 01000 and RAISERROR severity 11+ as `SQL_ERROR`, surfaced when `SQLMoreResults` reaches that statement ([Microsoft Learn, Processing Statements That Generate Messages](https://learn.microsoft.com/en-us/sql/relational-databases/native-client-odbc-error-messages/processing-statements-that-generate-messages)).

### Streaming

- **tiberius.** `QueryStream` implements `Stream`, decoding one token per poll ([query.rs L376-L430](https://docs.rs/crate/tiberius/0.13.0/source/src/tds/stream/query.rs#376)); `into_results` buffers only if called.
- **mssql-tds.** `ResultSet::next_row()` returns one row ([tds_client.rs L10109-L10111](https://docs.rs/crate/mssql-tds/0.4.0/source/src/connection/tds_client.rs#10109)); `next_row_into` decodes straight into a `RowWriter`, and `read_active_plp_chunk` reads a `varbinary(max)` / `nvarchar(max)` value in pieces ([L7310](https://docs.rs/crate/mssql-tds/0.4.0/source/src/connection/tds_client.rs#7310)), which suits run to file.
- **odbc-api.** Block cursors fetch a fixed-size row set at a time.

### Cancel

- **tiberius.** `Client::cancel_query(&mut self)` sends an Attention packet ([client.rs L708](https://docs.rs/crate/tiberius/0.13.0/source/src/client.rs#708), [connection.rs L426-L460](https://docs.rs/crate/tiberius/0.13.0/source/src/client/connection.rs#426)). Because it borrows the client mutably, the row stream must be dropped before it can be called; its own docs say so. Open issue [prisma/tiberius#459](https://github.com/prisma/tiberius/issues/459): against SQL Server 2022, cancelling before any result arrives returns an error and leaves the acknowledgement on the wire, so the next query reads the wrong response.
- **mssql-tds.** `ExecuteOptions::cancel: Option<&CancelHandle>`; "cancelling aborts the request without tearing down the connection" ([tds_client.rs L9929-L9932](https://docs.rs/crate/mssql-tds/0.4.0/source/src/connection/tds_client.rs#9929)). `CancelHandle::cancel(&self)` is backed by a tokio `CancellationToken` ([core.rs L22-L49](https://docs.rs/crate/mssql-tds/0.4.0/source/src/core.rs#22)); it is not `Clone`, so the engine shares it in an `Arc`. `cancel_query(timeout_secs)` waits for the attention acknowledgement and a failed settlement retires the connection ([client guide](https://github.com/microsoft/mssql-rs/blob/main/mssql-tds/docs/client-guide.md#response-lifecycle)).
- **ODBC.** `SQLCancel` on the same statement handle from another thread cancels the running function, which then returns SQLSTATE HY008 ([Microsoft Learn](https://learn.microsoft.com/en-us/sql/odbc/reference/syntax/sqlcancel-function)). `odbc-api` has no wrapper; the maintainer is undecided on a safe design ([pacman82/odbc-api#962](https://github.com/pacman82/odbc-api/issues/962)).

### TLS and named instances

- **tiberius.** `EncryptionLevel` has `Off`, `On`, `NotSupported`, `Required` and `Strict` (TDS 8.0) ([tds.rs L19-L31](https://docs.rs/crate/tiberius/0.13.0/source/src/tds.rs#19)); `Config::trust_cert()` skips validation ([config.rs L401](https://docs.rs/crate/tiberius/0.13.0/source/src/client/config.rs#401)). Named instances need `SqlBrowser::connect_named` and the `sql-browser-tokio` feature ([sql_browser.rs](https://docs.rs/crate/tiberius/0.13.0/source/src/sql_browser.rs)).
- **mssql-tds.** The default mode is `Strict`, which needs a TDS 8.0 server and a trusted certificate ([connection-behavior.md](https://github.com/microsoft/mssql-rs/blob/main/mssql-tds/docs/connection-behavior.md#encryption-defaults)). `trust_server_certificate` is honoured when the mode is not `Strict` ([ssl_handler.rs L49-L56](https://docs.rs/crate/mssql-tds/0.4.0/source/src/connection/transport/ssl_handler.rs#49)), the same rule as Driver 18. Most DBA servers with self-signed certificates therefore need `mode = On` plus `trust_server_certificate = true`. SQL Browser (SSRP) resolution is built in with a configurable timeout ([client_context.rs L325-L327](https://docs.rs/crate/mssql-tds/0.4.0/source/src/connection/client_context.rs#325)); LocalDB and named pipes have their own transports.
- **ODBC Driver 18.** `Encrypt` defaults to `yes`; `strict` ignores `TrustServerCertificate` ([Microsoft Learn](https://learn.microsoft.com/en-us/sql/connect/odbc/dsn-connection-string-attribute#encrypt)).

### Types

- **tiberius.** `ColumnType` covers `Money`, `Money4`, `DatetimeOffsetn`, `Xml`, `Guid` and the rest ([row.rs L36](https://docs.rs/crate/tiberius/0.13.0/source/src/row.rs#36)); `sql_variant` is fully decoded and CLR UDTs (geography, geometry, hierarchyid) come back as raw bytes ([type_info.rs L44, L206-L212](https://docs.rs/crate/tiberius/0.13.0/source/src/tds/codec/type_info.rs#44)).
- **mssql-tds.** `ColumnValues` has `DateTimeOffset`, `Money`, `SmallMoney`, `Xml`, `Bytes`, `Json` and `Vector` ([column_values.rs](https://docs.rs/crate/mssql-tds/0.4.0/source/src/datatypes/column_values.rs)); `sql_variant` decodes to its base type ([decoder.rs L1736, L1786](https://docs.rs/crate/mssql-tds/0.4.0/source/src/datatypes/decoder.rs#1736)); UDT columns decode as bytes ([decoder.rs L3086](https://docs.rs/crate/mssql-tds/0.4.0/source/src/datatypes/decoder.rs#3086)).
- Raw bytes for geography match what SSMS shows in its grid (`0xE6100000…`), so it is enough for the results grid.

### Builds and testing on this PC

- `cargo check` of a scratch crate depending on `mssql-tds =0.4.0`, `tiberius =0.13.0` (with `sql-browser-tokio`) and `odbc-api =30.1.0` passed on Windows 11 with rustc 1.93.0 and MSVC, no OpenSSL. mssql-tds uses Schannel on Windows by default.
- `mssql-mock-tds` 0.4.0 (MIT) is a mock TDS server library and CLI: login, TLS 7.4 and 8.0, registered result sets, INFO messages, errors and delays ([README](https://docs.rs/crate/mssql-mock-tds/0.4.0/source/README.md)). It does not run SQL, so it tests the engine's protocol handling, not query results. Its non-Windows builds need OpenSSL; on Windows it uses `native-tls`.
- No SQL Server service is installed on this PC. ODBC Driver 17 and 18 are installed (registry `ODBCINST.INI`).

### Maintenance and licence

- **tiberius** ([prisma/tiberius](https://github.com/prisma/tiberius)). 0.12.3 shipped 2024-07-19; no commits between 2025-07-29 and 2026-08; then 100 commits in August-September 2026, 97 by one contributor who first forked the crate, and 0.13.0 published by a Prisma owner on 2026-09-25. In [#427](https://github.com/prisma/tiberius/issues/427) the RustSec working group asked about its maintenance status after DoS reports went unanswered, and another contributor forked it as the `mssql` crate. MIT or Apache-2.0.
- **mssql-tds** ([microsoft/mssql-rs](https://github.com/microsoft/mssql-rs)). Published by `microsoft-oss-releases`; 100 commits since 2026-08-16 from Microsoft staff including the ODBC driver team; it also underpins `mssql-python`, an alpha ODBC driver and sqlcmd. Releases 0.1.0 (2026-09-10), 0.2.0 (2026-10-06), 0.3.0 (2026-10-25), 0.4.0 (2026-11-07). `edition = "2024"`. MIT. A placeholder crate `mssql-rs` 0.0.1-alpha.1 (2026-11-10) is described as "a future Microsoft SQL Server Driver".
- **odbc-api** ([pacman82/odbc-api](https://github.com/pacman82/odbc-api)). Active, one main maintainer, 9 open issues. MIT. Microsoft ODBC Driver 18 is closed source, free to redistribute, and needs an MSI install (admin) on each machine.

## Judgement

- **Why not tiberius.** NTLM-only Windows sign-in rules out Kerberos-only domains and linked-server delegation, and the dropped INFO, DONE and ERROR tokens rule out an SSMS-style Messages pane without forking the token loop. Its maintenance has only just restarted, and that work rests on one outside contributor.
- **Why not ODBC.** It reaches every requirement, but the three that matter most to this IDE (messages, statement-wise results and cancel) all need `unsafe` raw-handle code that `odbc-api` exists to hide. It adds a native install that the user must make on every machine, and it gives the engine nothing to test with except a real server.
- **Why mssql-tds despite its age.** Its API was designed around the same statement-wise model msodbcsql uses, by the team that maintains msodbcsql, and the IDE's needs (messages, row counts, continue after error, cancel from another task, TLS trust, named instances) are public, documented features, not workarounds. Churn is the cost: pin `=0.4.x`, keep every `mssql_tds` import in one engine module, and expect to follow Microsoft's later `mssql-rs` driver if it replaces this one.
- **Testing plan for Codex.** Unit-level protocol tests against `mssql-mock-tds` run in `cargo test` with no server. End-to-end tests that check plans, messages and cancel need a real SQL Server; SQL Server Developer or Express installs natively on Windows (no WSL, no Docker), but that is a system-wide install the owner has to choose. LocalDB is the lighter variant and mssql-tds can connect to it.

## Not verified

- Nothing was run against a SQL Server: plan capture, message ordering, cancel behaviour and Kerberos were read from source and docs, not observed.
- The column name of the showplan result set, used to tell plan XML apart from user result sets, is not given on the Microsoft pages read here; check it on the first real run.
- That msodbcsql sends a parameterless `SQLExecDirect` as a SQL batch rather than an RPC was not checked; it matters only if ODBC is chosen.
- Whether mssql-tds's `next_row()` returns promptly when the cancel handle fires mid-row was not exercised.
- ODBC type handling for sql_variant, datetimeoffset and UDTs through `odbc-api` was not checked in detail.
