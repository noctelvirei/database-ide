# Database IDE engineering standards

<!-- agent-standards: {"version":1,"technologies":["rust","tokio","mssql-tds","tokio-postgres","mysql_async"]} -->

The product decisions are in [docs/spec.md](docs/spec.md) and the vocabulary in [CONTEXT.md](CONTEXT.md). Codex and Claude read this file before any change, on top of their own global coding, design and testing rules.

## Stack and versions

- Rust 1.100.0, pinned in `rust-toolchain.toml` at the repo root. `edition = "2024"`, `resolver = "3"`.
- Async: Tokio (multi-thread runtime). All three drivers run on it.
- Drivers, each pinned to an exact version (`=x.y.z`) or a git `rev`:
  - `mssql-tds` (SQL Server)
  - our fork of `tokio-postgres` at https://github.com/noctelvirei/rust-postgres, through `[patch.crates-io]` ([ADR 0001](docs/adr/0001-own-tokio-postgres-fork.md))
  - `mysql_async`
- TLS for PostgreSQL and MySQL: `native-tls` (SChannel on Windows).
- Other crates to use before writing anything by hand: `sqlparser` (danger check), `quick-xml` (showplan), `serde` / `serde_json` (EXPLAIN JSON, plan trees), `toml` (connections file), `keyring` (passwords), `csv`, `rust_xlsxwriter` (constant-memory mode), `thiserror` (error types), `tracing` (logging), `tempfile` (spill files and test servers), `clap` (CLI), `futures`, `tokio-util`, `bytes`, `postgres-native-tls`, `rand` (test database names), `memmap2` (reading the spill file). Adding any other dependency needs the owner's yes. Overnight, a ticket that needs one is parked with the question on it, and work moves to the next independent ticket.
- Pinned versions upgrade only on purpose, in a change of their own.

## Structure

- `crates/engine`: the library; everything below the UI. No UI dependency, ever.
- `crates/dbide-cli`: the E2E harness binary; uses only the engine's public API.
- `crates/app`: the gpui UI. It calls only the engine's public API and holds no SQL or driver code.
- Inside the engine, one module per piece of knowledge, split by feature, not by layer: `session` (the common run/event/cancel interface), `spill` (the row file and its index), `mssql`, `postgres`, `mysql` (one per engine, each the only place its driver is used), `plan` (the shared operator tree; showplan and EXPLAIN parsing live in their engine modules or `plan`'s submodules), `export`, `connections`, `danger`, `grid_edit`, `objects`.
- A driver type never appears in another module's signature. Engine modules convert to the engine's own types at their boundary.
- Module files sit as `foo.rs` beside `foo/`, never `foo/mod.rs`. No source file above 1000 lines. No re-exports (`pub use`), no `#[path]`, no `include!`.

## Style and errors

- rustfmt defaults; `rustfmt.toml` holds only `style_edition = "2024"`.
- API Guidelines naming; public types implement `Debug`.
- Each module returns its own `thiserror` error enum. Errors the user can act on (bad credentials, server unreachable, syntax error with a line number) carry what the UI needs to say so. `unwrap`/`expect` only in tests, or with an `expect` message stating the invariant.
- `unsafe` is denied workspace-wide. FFI modules (Windows SSPI in the forks) allow it locally with a `// SAFETY:` comment on each block.
- Every public item gets an interface comment saying what callers can't see from the signature: units, bounds, ordering, side effects, errors.
- Root `Cargo.toml` lints: `[workspace.lints.rust] unsafe_code = "deny"`; `[workspace.lints.clippy] all = { level = "warn", priority = -1 }`, `pub_use = "deny"`, `mod_module_files = "deny"`, `undocumented_unsafe_blocks = "deny"`. Each member sets `edition.workspace = true` and `[lints] workspace = true`.

## Public repository

The repo is public. No local paths, drive letters, user names, machine names, host names or private project names in code, tests, fixtures, commits, issues or pull requests. Test configuration comes from environment variables.

## Verification

Run from the repo root, all of them, before a change is offered for review:

```
cargo fmt --all --check
cargo clippy --workspace --all-targets --locked -- -D warnings
cargo test --workspace --locked
node scripts/check-rust-layout.mjs
```

Set `CARGO_TARGET_DIR` to a folder on the data drive; build output never goes into the repo's drive.

### E2E harness

Real servers are the test ([testing decision](https://github.com/noctelvirei/database-ide/issues/16)). Integration tests in `crates/engine/tests/` and `crates/dbide-cli/tests/` drive the public API (the CLI through its real arguments) against real servers and leave artifacts (result and export files, plan trees as JSON) in a folder a rerun reproduces.

- **PostgreSQL 18**: the harness starts a throwaway server per test run: `initdb` into a temp folder, a free port, deleted afterwards. Binaries from `DBIDE_PG_BIN`.
- **MySQL 8.4**: the same, with `mysqld --initialize-insecure`, binaries from `DBIDE_MYSQL_BIN`.
- **SQL Server 2025 Developer**: a fresh `dbide_test_<random>` database per test, dropped afterwards, plus a sweep for leftovers. Server from `DBIDE_MSSQL` (host, port, sign-in).
- Every real-server test's name starts with its engine: `mssql_`, `pg_`, `mysql_`. A test whose server variable is unset fails with a message naming the variable. While a server isn't available, run `cargo test --workspace --locked -- --skip mssql_` (and so on), and the record states exactly which prefixes were skipped. A skipped engine is never reported as passing.
- **PostgreSQL Kerberos**: `pg_kerberos_` tests sign in to a PostgreSQL server that accepts only `gss`, named by `DBIDE_PG_KERBEROS_HOST`, as the role `DBIDE_PG_KERBEROS_USER`. Run them in a process that holds that realm's credentials (`runas /netonly` or `CreateProcessWithLogonW` with `LOGON_NETCREDENTIALS_ONLY`), and skip them in the main run with `--skip pg_kerberos_`. The main run's SQL Server tests sign in as the current Windows account, so they can't share that process.
- Tests that touch Windows Credential Manager use the service name `database-ide-test` and delete their entries afterwards.
- `mssql-mock-tds` only for failures a real server won't produce on demand.
- Test data is invented, from scripts in the repo.

### UI E2E harness

The app is tested as a user drives it, in a real window, against the same real servers. The gpui spike ([prototype/gpui-spike](https://github.com/noctelvirei/database-ide/tree/prototype/gpui-spike/prototypes/gpui-spike)) proved the method; U1 builds it into `crates/app`:

- A test mode, `--ui-script <file>`, runs a script of steps inside the app. Keys go through gpui's `Window::dispatch_keystroke`, the path a real key takes after Windows' message translation: the keymap, the focused element's actions and text input. Synthetic OS input (SendInput) is not used, because it lands in whatever window has focus.
- After each step the app appends its visible state as one JSON line (focused pane, tab titles, editor text and selection, messages, grid row counts, dialog contents). Tests assert on those lines.
- A step can name a screenshot; a PowerShell script captures the app's window into the test's artifact folder. Screenshots are the proof a rerun reproduces, not the assertion.
- Startup time, idle private working set and the zipped release size are measured on every UI ticket, against the targets in the spec.
- Windows UI Automation can't drive the app: gpui's Windows accessibility is incomplete.

## Sources

Reviewed 2026-11-16:
- Rust API Guidelines: https://rust-lang.github.io/api-guidelines/
- Rust style guide: https://doc.rust-lang.org/style-guide/
- Driver research: [SQL Server](https://github.com/noctelvirei/database-ide/issues/4), [PostgreSQL and MySQL](https://github.com/noctelvirei/database-ide/issues/5), [PostgreSQL domain sign-in](https://github.com/noctelvirei/database-ide/issues/15)
- Toolkit and plan viewer research: [#3](https://github.com/noctelvirei/database-ide/issues/3), [#6](https://github.com/noctelvirei/database-ide/issues/6)

No code exists yet, so there are no known violations.
