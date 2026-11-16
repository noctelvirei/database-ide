# PostgreSQL and MySQL drivers

Research for [#5](https://github.com/noctelvirei/database-ide/issues/5), done 2026-11-16 against crate source downloaded from crates.io and the PostgreSQL 18 manual.

## Recommendation

Use **tokio-postgres 0.7.18** for PostgreSQL and **mysql_async 0.37.1** for MySQL. Do not use sqlx.

- tokio-postgres is the only PostgreSQL crate here with a cancel API and a notice API. Its gap is that multi-statement (simple-query) runs return column names but no type OIDs. Close it with a `[patch.crates-io]` git dependency on a fork carrying [PR #1358](https://github.com/rust-postgres/rust-postgres/pull/1358) (85 lines, mergeable) until upstream merges one of the three open PRs. Values still arrive as text, which is what the results grid shows, so the gap costs type hints (alignment, sort, editor), not data.
- mysql_async returns dynamic `Value`s with column types, walks multiple result sets, reports warning counts and exposes the connection id that `KILL QUERY` needs. sqlx-mysql drops the warning count.
- sqlx's one advantage, type OIDs in multi-statement PostgreSQL runs, does not outweigh having no cancel, notices only in tracing logs, and an engine abstraction the IDE cannot use: SQL Server needs its own driver anyway, so the engine has a per-engine layer whatever happens, and every IDE feature here (plans, notices, cancel, warnings) is engine-specific.

## Versions checked

| Crate | Version | Released | Repository | Licence |
|---|---|---|---|---|
| tokio-postgres | 0.7.18 | 2026-06-12 | [rust-postgres/rust-postgres](https://github.com/rust-postgres/rust-postgres) | MIT OR Apache-2.0 |
| postgres-types | 0.2.14 | 2026-06-12 | same | MIT OR Apache-2.0 |
| sqlx (sqlx-core, sqlx-postgres, sqlx-mysql) | 0.9.0 | 2026-05-21 | [transact-rs/sqlx](https://github.com/transact-rs/sqlx) (moved from launchbadge/sqlx) | MIT OR Apache-2.0 |
| mysql_async | 0.37.1 | 2026-09-01 | [blackbeam/mysql_async](https://github.com/blackbeam/mysql_async) | MIT OR Apache-2.0 |
| mysql_common | 0.38.2 | 2026-07-24 | [blackbeam/rust_mysql_common](https://github.com/blackbeam/rust_mysql_common) | MIT OR Apache-2.0 |

Source links below point at docs.rs source views for these exact versions.

## Verified facts

### Protocol background (PostgreSQL)

From [Message Flow](https://www.postgresql.org/docs/current/protocol-flow.html) (PostgreSQL 18):

- A Parse message (extended protocol) holds one statement only: "The query string contained in a Parse message cannot include more than one SQL statement; else a syntax error is reported." A batch the user types with several statements must go through the simple-query protocol, or be split client-side.
- Simple-query results are text, except a `FETCH` from a binary cursor.
- Several statements in one simple query run in one implicit transaction unless the batch has its own `BEGIN`/`COMMIT`; the first error stops the batch and rolls back the implicit block.
- Cancel is a `CancelRequest` sent on a *new* connection with the key from `BackendKeyData`. It "might or might not have any effect", and the server sends no reply to it.

From [EXPLAIN](https://www.postgresql.org/docs/current/sql-explain.html):

- `FORMAT` takes `TEXT`, `XML`, `JSON` or `YAML`. `ANALYZE` executes the statement; for DML the manual's advice is `BEGIN; EXPLAIN ANALYZE ...; ROLLBACK;`.
- "Buffers information is automatically included when `ANALYZE` is used" (PostgreSQL 18), so `BUFFERS` matters only for older servers.
- The result is ordinary rows in a column headed `QUERY PLAN`. With `FORMAT JSON` that is a single row holding one JSON document.

### tokio-postgres 0.7.18

- **Streaming.** [`Client::query_raw`](https://docs.rs/tokio-postgres/0.7.18/src/tokio_postgres/client.rs.html) and `query_typed_raw` return a `RowStream`; [`simple_query_raw`](https://docs.rs/tokio-postgres/0.7.18/src/tokio_postgres/client.rs.html) (added 0.7.15) returns a `SimpleQueryStream`. Responses travel over a bounded channel (`mpsc::channel(1)` in `client.rs`); when it is full, [`Connection::poll_read`](https://docs.rs/tokio-postgres/0.7.18/src/tokio_postgres/connection.rs.html) stops reading the socket ("waiting on sender"), so a slow consumer applies back-pressure to the server.
- **Dropping a stream does not stop the query.** When the receiver hangs up, the connection keeps reading and discarding the rest ("we need to keep paging through the rest of the messages even if the receiver's hung up"). Stopping a large result therefore needs cancel, not just drop.
- **Cancel.** `Client::cancel_token()` returns a `CancelToken` with `cancel_query(tls)` ([cancel_token.rs](https://docs.rs/tokio-postgres/0.7.18/src/tokio_postgres/cancel_token.rs.html)), usable from another task while the query runs.
- **Dynamic types, extended protocol.** `Row::columns()` gives each `Column`'s name, `Type`, table OID, column number and type modifier ([statement.rs](https://docs.rs/tokio-postgres/0.7.18/src/tokio_postgres/statement.rs.html)). Bind always asks for binary results (`Some(1)` in [query.rs](https://docs.rs/tokio-postgres/0.7.18/src/tokio_postgres/query.rs.html)). `FromSql` has an `accepts(&Type) -> bool` method ([postgres-types](https://docs.rs/postgres-types/0.2.14/src/postgres_types/lib.rs.html)), so a custom type that accepts every type can read the raw bytes of any column; built-in impls cover the common scalars, and chrono, jiff, serde_json, uuid, cidr and others come behind `with-*` features.
- **Dynamic types, simple-query protocol.** `SimpleQueryMessage` is `Row`, `CommandComplete(u64)` or `RowDescription(Arc<[SimpleColumn]>)` ([lib.rs](https://docs.rs/tokio-postgres/0.7.18/src/tokio_postgres/lib.rs.html)). `SimpleColumn` stores only the name: the RowDescription parser keeps `f.name()` and drops the type OID ([simple_query.rs](https://docs.rs/tokio-postgres/0.7.18/src/tokio_postgres/simple_query.rs.html)). `SimpleQueryRow::get` returns `Option<&str>`, so NULL and text values are intact. Open fixes: [issue #1329](https://github.com/rust-postgres/rust-postgres/issues/1329), PRs [#1338](https://github.com/rust-postgres/rust-postgres/pull/1338), [#1358](https://github.com/rust-postgres/rust-postgres/pull/1358) and [#1391](https://github.com/rust-postgres/rust-postgres/pull/1391) (the last also exposes command tags), all open and mergeable on 2026-11-16.
- **Multiple result sets and row counts.** The simple-query stream yields `RowDescription`, rows and `CommandComplete(rows)` per statement, so result-set boundaries and row counts survive. The command tag text (`INSERT 0 5`, `CREATE TABLE`) is not exposed.
- **Notices.** Notices arrive as `AsyncMessage::Notice(DbError)` from `Connection::poll_message` ([connection.rs](https://docs.rs/tokio-postgres/0.7.18/src/tokio_postgres/connection.rs.html)), on the connection task, not inside the row stream.
- **TLS.** `SslMode` is `Disable`, `Prefer` or `Require`; certificate checking belongs to the TLS connector passed in. `SslNegotiation::Direct` supports PostgreSQL 17+ direct TLS ([config.rs](https://docs.rs/tokio-postgres/0.7.18/src/tokio_postgres/config.rs.html)). Connectors: [postgres-native-tls](https://crates.io/crates/postgres-native-tls) 0.5.3 (same repo; SChannel on Windows) and [tokio-postgres-rustls](https://crates.io/crates/tokio-postgres-rustls) 0.14.0 (third party).
- **Sign-in.** Password, MD5 and SCRAM-SHA-256 (with `-PLUS` channel binding). Kerberos, GSS and SSPI return "unsupported authentication method" ([connect_raw.rs](https://docs.rs/tokio-postgres/0.7.18/src/tokio_postgres/connect_raw.rs.html)).
- **Maintenance.** Four releases between 2025-09-25 and 2026-06-12 ([CHANGELOG](https://github.com/rust-postgres/rust-postgres/blob/master/tokio-postgres/CHANGELOG.md)); last commit 2026-10-03; 205 open issues and PRs.

### sqlx 0.9.0 (PostgreSQL and MySQL)

- **Streaming.** `Executor::fetch` and `fetch_many` return lazy streams built with `try_stream!` that read the socket as they are polled ([sqlx-postgres executor.rs](https://docs.rs/sqlx-postgres/0.9.0/src/sqlx_postgres/connection/executor.rs.html)). `fetch_many` yields `Either<QueryResult, Row>` per step ([sqlx-core executor.rs](https://docs.rs/sqlx-core/0.9.0/src/sqlx_core/executor.rs.html)).
- **Multi-statement runs.** `raw_sql` sends the string unprepared: on PostgreSQL that is the simple-query protocol with `PgValueFormat::Text`, and each new `RowDescription` resets the column metadata, so every row carries `PgTypeInfo` (OID, kind) for its own result set. `PgValueRef::as_str` and `as_bytes` are public ([value.rs](https://docs.rs/sqlx-postgres/0.9.0/src/sqlx_postgres/value.rs.html)). This is the one place sqlx beats tokio-postgres.
- **No PostgreSQL cancel.** `PgConnectionInner` stores `process_id` and `secret_key` under `#[allow(dead_code)]` with the comment "used to send cancel requests" ([connection/mod.rs](https://docs.rs/sqlx-postgres/0.9.0/src/sqlx_postgres/connection/mod.rs.html)); nothing reads them, and no public accessor exists.
- **Notices go to logging only.** `NoticeResponse` is decoded and written to `tracing` under target `sqlx::postgres::notice` ([stream.rs](https://docs.rs/sqlx-postgres/0.9.0/src/sqlx_postgres/connection/stream.rs.html)). An API is requested in [issue #3621](https://github.com/transact-rs/sqlx/issues/3621) (open).
- **Row counts only.** `PgQueryResult` has `rows_affected`; `MySqlQueryResult` has `rows_affected` and `last_insert_id`, no warning count ([query_result.rs](https://docs.rs/sqlx-mysql/0.9.0/src/sqlx_mysql/query_result.rs.html)).
- **MySQL multi-results.** `MULTI_STATEMENTS`, `MULTI_RESULTS` and `PS_MULTI_RESULTS` are negotiated and `SERVER_MORE_RESULTS_EXISTS` is followed.
- **Dropped futures.** Open issues report a cancelled future leaving MySQL protocol state behind: [#4427](https://github.com/transact-rs/sqlx/issues/4427), [#4467](https://github.com/transact-rs/sqlx/issues/4467), [#563](https://github.com/transact-rs/sqlx/issues/563).
- **TLS.** Features `_tls-native-tls`, `_tls-rustls` (ring or aws-lc-rs) in sqlx-core.
- **Maintenance.** Last commit 2026-11-10; 783 open issues and PRs; repository now under the transact-rs organisation.

### mysql_async 0.37.1

- **Streaming.** `conn.query_iter(sql)` returns a `QueryResult`; `QueryResult::stream::<Row>()` returns a `ResultSetStream` for the current result set, and `next()` reads one row at a time from the socket ([query_result/mod.rs](https://docs.rs/mysql_async/0.37.1/src/mysql_async/queryable/query_result/mod.rs.html), [result_set_stream.rs](https://docs.rs/mysql_async/0.37.1/src/mysql_async/queryable/query_result/result_set_stream.rs.html)).
- **Multiple result sets.** `CLIENT_MULTI_STATEMENTS`, `CLIENT_MULTI_RESULTS` and `CLIENT_PS_MULTI_RESULTS` are set by default ([opts/mod.rs](https://docs.rs/mysql_async/0.37.1/src/mysql_async/opts/mod.rs.html)). The docs say to call `collect` or `stream` once per result set and use `is_empty()` to detect the end; an unconsumed set is drained on the next call.
- **Dynamic types.** `Row` holds `mysql_common::Value` (`NULL`, `Bytes`, `Int`, `UInt`, `Float`, `Double`, `Date`, `Time`) and each `Column` exposes `column_type()`, `flags()` and `character_set()` ([value/mod.rs](https://docs.rs/mysql_common/0.38.2/src/mysql_common/value/mod.rs.html), [packets/mod.rs](https://docs.rs/mysql_common/0.38.2/src/mysql_common/packets/mod.rs.html)). Text-protocol rows come back as `Bytes`, with the column type alongside.
- **Warnings, row counts, info.** `QueryResult` has `affected_rows()`, `last_insert_id()`, `info()` and `warnings()` (a count; the messages need `SHOW WARNINGS`).
- **Cancel.** No protocol-level cancel exists in MySQL; a query is stopped with `KILL QUERY <id>` from another connection ([MySQL KILL](https://dev.mysql.com/doc/refman/8.4/en/kill.html)). `Conn::id()` returns the server's connection id ([conn/mod.rs](https://docs.rs/mysql_async/0.37.1/src/mysql_async/conn/mod.rs.html)).
- **TLS.** `native-tls-tls` (SChannel on Windows), `rustls-tls` with `ring` or `aws-lc-rs` (`default-rustls` picks aws-lc-rs). On x86-64 MSVC, aws-lc-rs needs NASM unless its prebuilt NASM objects apply; CMake only for FIPS ([aws-lc-rs Windows requirements](https://aws.github.io/aws-lc-rs/requirements/windows.html)). The default feature set uses `flate2/zlib`; `minimal-rust` switches to the pure-Rust backend.
- **Sign-in.** `mysql_native_password`, `caching_sha2_password`, clear password, old password; `client_ed25519` and `client_parsec` features for MariaDB.
- **Maintenance.** Last commit 2026-11-08; 36 open issues and PRs; single main maintainer.

## Capability summary

| | tokio-postgres | sqlx-postgres | mysql_async | sqlx-mysql |
|---|---|---|---|---|
| Streaming with back-pressure | Yes | Yes | Yes | Yes |
| Cancel | `CancelToken` | None (use `pg_cancel_backend` from a second connection) | `KILL QUERY` via `Conn::id()` | `KILL QUERY`; id via `SELECT CONNECTION_ID()` |
| Dynamic types, single statement | Binary + `Type`; raw bytes via custom `FromSql` | Binary + `PgTypeInfo` | `Value` + `ColumnType` | Yes |
| Dynamic types, multi-statement batch | Text, names only (OIDs: open PRs) | Text + type OID | `Value` + `ColumnType` | Yes |
| Result-set boundaries and row counts | Yes; no command tag | Yes; no command tag | Yes, plus `info()` | Yes |
| Notices / warnings | `Connection::poll_message` | tracing log only | Warning count | None |
| EXPLAIN JSON | Plain query, one text row | Same | n/a (ticket scope) | n/a |
| TLS on Windows without installs | native-tls (SChannel) or rustls | native-tls or rustls | native-tls or rustls-ring | native-tls or rustls |
| Kerberos / SSPI | No | No | n/a | n/a |
| Licence | MIT OR Apache-2.0 | MIT OR Apache-2.0 | MIT OR Apache-2.0 | MIT OR Apache-2.0 |

## Judgement

- **Run path for PostgreSQL.** Send the user's text through `simple_query_raw`: it accepts any number of statements, keeps result-set boundaries, and returns values as text, which is how psql shows them and what the grid displays. Plans come back the same way: `EXPLAIN (FORMAT JSON) ...` is one text cell to parse with serde_json; for an actual plan of DML, wrap it in `BEGIN; ... ROLLBACK;` per the manual. Keep the binary extended path for the IDE's own catalogue queries, where the statements are known.
- **Closing the OID gap.** Three ways, cheapest first: (1) `[patch.crates-io]` to a fork with PR #1358 and drop it when upstream merges; (2) split statements client-side and use `query_raw` per statement, which gives binary values and full `Type` but needs a SQL splitter that understands dollar quoting and puts each statement in its own implicit transaction; (3) drive `postgres-protocol` directly, whose `Field` already parses the OID. Option 1 is a few lines of manifest; option 2 changes transaction semantics the user expects from a batch.
- **Stop button.** For both engines, stopping is cancel (or `KILL QUERY`) plus letting the driver drain what the server already sent. The engine needs a second connection per server for MySQL kills and for cancel on sqlx-style drivers; tokio-postgres opens its own.
- **Notices.** tokio-postgres hands notices to whoever polls the `Connection`, separate from the rows. The engine must tag them to the run in progress; they cannot be matched to a statement inside a batch by order alone.
- **TLS build.** Pick `native-tls` (SChannel, Windows certificate store) for both crates on Windows. It needs no NASM, and the DBA's corporate CA certificates already sit in the Windows store.
- **Windows domain sign-in.** Neither PostgreSQL crate does Kerberos/SSPI. If any of the DBA's PostgreSQL servers use `sspi` or `gss` in `pg_hba.conf`, that is a separate gap to raise; SQL Server integrated sign-in is the SQL Server driver's question.
- **Testing on Windows.** All crates here are pure Rust over TCP, so the engine's tests need only native PostgreSQL 18 and MySQL servers on the PC, no Docker.

## Not verified

- Behaviour against live servers: everything above is read from source and docs, not run.
- MySQL plan output (`EXPLAIN FORMAT=JSON`, `EXPLAIN ANALYZE`) per server version; the ticket asked only for PostgreSQL plans.
- Whether PR #1358 applies cleanly to 0.7.18 and builds on Windows.
- PostgreSQL 18 protocol 3.2 (longer cancel keys): neither PostgreSQL crate was checked for it.
- sqlx-mysql's dynamic type coverage was not read in detail; it is marked "Yes" on the strength of its `MySqlTypeInfo` per column, not a full check.
