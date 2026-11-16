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
- Other crates to use before writing anything by hand: `sqlparser` (danger check), `quick-xml` (showplan), `serde` / `serde_json` (EXPLAIN JSON, plan trees), `toml` (connections file), `keyring` (passwords), `csv`, `rust_xlsxwriter` (constant-memory mode), `thiserror` (error types), `tracing` (logging), `tempfile` (spill files and test servers), `clap` (CLI). Adding any other dependency needs the owner's yes.
- Pinned versions upgrade only on purpose, in a change of their own.

## Structure

- `crates/engine`: the library; everything below the UI. No UI dependency, ever.
- `crates/dbide-cli`: the E2E harness binary; uses only the engine's public API.
- `crates/app`: the gpui UI, added later.
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
- A test whose server variable is unset fails with a message naming the variable, unless it is run through the matching feature-gated skip (`--skip mssql`), which the record must state. A skipped engine is never reported as passing.
- `mssql-mock-tds` only for failures a real server won't produce on demand.
- Test data is invented, from scripts in the repo.

## Sources

Reviewed 2026-11-16:
- Rust API Guidelines: https://rust-lang.github.io/api-guidelines/
- Rust style guide: https://doc.rust-lang.org/style-guide/
- Driver research: [SQL Server](https://github.com/noctelvirei/database-ide/issues/4), [PostgreSQL and MySQL](https://github.com/noctelvirei/database-ide/issues/5), [PostgreSQL domain sign-in](https://github.com/noctelvirei/database-ide/issues/15)
- Toolkit and plan viewer research: [#3](https://github.com/noctelvirei/database-ide/issues/3), [#6](https://github.com/noctelvirei/database-ide/issues/6)

No code exists yet, so there are no known violations.
