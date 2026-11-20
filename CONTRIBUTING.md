# Contributing

Database IDE is a Rust SQL IDE for SQL Server, PostgreSQL and MySQL, meant to replace SSMS for everyday DBA work. The engine (connections, runs, streaming results, plans, exports, the danger check, the formatter) is built. The gpui UI is being built ticket by ticket on top of it.

## Read first

- [docs/spec.md](docs/spec.md): what v1 does and how it is put together. A change that goes against the spec starts as an issue, not a pull request.
- [CODE_STANDARDS.md](CODE_STANDARDS.md): pinned versions, crate layout, error handling, the checks and the test harness.
- [CONTEXT.md](CONTEXT.md): the project's vocabulary. Code and issues use these terms ("run", "spill store", "editable result", "danger check").

## Picking something to work on

Open work is in [GitHub Issues](https://github.com/noctelvirei/database-ide/issues). Issues labelled [`good first issue`](https://github.com/noctelvirei/database-ide/labels/good%20first%20issue) are small and fully specified. Each issue has a "Done when" section; a pull request is finished when that list is.

Comment on the issue before you start, so two people don't build the same thing. Issue titles carry an ID like `DAT-46`: that is the maintainer's internal tracker, and commit messages refer to it.

## Building and running

You need Windows 10 or 11 and [rustup](https://rustup.rs). `rust-toolchain.toml` pins Rust 1.100.0, and rustup installs it on the first build.

```
cargo run -p dbide-app --release
```

builds and opens the app (`database-ide.exe`). `cargo run -p dbide -- --help` lists the command-line harness's commands. macOS and Linux aren't supported yet, but nothing in the code should rule them out: keep Windows-only APIs behind a module that names them.

## Checks

Run all four from the repo root before opening a pull request:

```
cargo fmt --all --check
cargo clippy --workspace --all-targets --locked -- -D warnings
cargo test --workspace --locked
node scripts/check-rust-layout.mjs
```

The layout script needs Node.js. It enforces the layout rules in CODE_STANDARDS.md that Clippy can't: no file over 1000 lines, no `#[path]`, no `include!`, no re-exports, and no plain comments beyond doc comments and `// SAFETY:` (see CODE_STANDARDS.md, Comments, for where rules and workarounds go instead).

## Test servers

Tests run against real database servers, never mocks. Each real-server test's name starts with its engine (`mssql_`, `pg_`, `mysql_`), and fails with a message naming the variable it needs when that variable is unset:

| Variable | What it points at |
|---|---|
| `DBIDE_PG_BIN` | The `bin` folder of a PostgreSQL 18 install or zip. The tests start a throwaway server on a free port and delete it afterwards. |
| `DBIDE_MYSQL_BIN` | The `bin` folder of a MySQL 8.4 zip, used the same way. |
| `DBIDE_MSSQL` | A SQL Server 2025 instance as `host,port` (for example `localhost,1433`) that your Windows account can sign in to and create databases on. Each test makes a `dbide_test_<random>` database and drops it. |

Skip an engine you don't have with `--skip`, and say in the pull request which you skipped:

```
cargo test --workspace --locked -- --skip mssql_ --skip pg_kerberos_
```

`pg_kerberos_` tests need a Kerberos realm; almost everyone skips them. `ui_` tests open the app's window for a few seconds each and need a signed-in desktop. The UI scripts and measurements in `scripts/` run on PowerShell 7 (`winget install Microsoft.PowerShell`).

## Pull requests

- One issue per pull request, with `Closes #<number>` in the description.
- Say which checks you ran and which test prefixes you skipped.
- A UI change includes the output of `pwsh -File scripts/measure-app.ps1 -Exe target/release/database-ide.exe`: the app must still open in under 1 second and idle under 100 MB.
- Adding a dependency or changing a pinned version needs the maintainer's agreement first. Ask on the issue.
- Keep your machine's paths, user name and host names out of code, tests and commits. Tests read their configuration from the variables above.

AI coding agents are welcome. `AGENTS.md` and `CLAUDE.md` hold the instructions they read.

## Licence

Database IDE is dual-licensed under [MIT](LICENSE-MIT) or [Apache 2.0](LICENSE-APACHE). Unless you say otherwise, any contribution you submit for inclusion is dual-licensed the same way, with no additional terms or conditions.
