---
name: verify
description: Build Database IDE, run the engine, CLI and app against the real SQL Server, PostgreSQL and MySQL servers, drive the app's changed path through a UI script, and save the evidence. Run whenever the factory's testing rules call for a verify run.
---

# Verify Database IDE

Agents run this skill unasked; Codex reads this file directly. It is a judge file: changes to it, and to the scripts beside it, go in their own pull request that the owner merges. CODE_STANDARDS.md ("E2E harness", "UI E2E harness") explains the harness this skill drives.

## Start

From the worktree root, in PowerShell 7.6 (`pwsh`):

1. Set the build folder and the servers. Keep build output and artifacts off the repo's drive.
   - `CARGO_TARGET_DIR`: a folder on the data drive used only by this worktree, so parallel runs don't share a build lock.
   - `DBIDE_PG_BIN` and `DBIDE_MYSQL_BIN`: the PostgreSQL 18 and MySQL 8.4 `bin` folders. The tests start throwaway servers from them on free ports; nothing needs starting by hand.
   - `DBIDE_MSSQL`: the SQL Server 2025 Developer instance as `host,port`, signed in as the current Windows account. It is set to Manual start: check it is running with `Get-Service MSSQLSERVER`. If it is stopped, ask the owner to start it; don't change the service.
   - `DBIDE_ARTIFACTS`: a folder on the data drive for this run's evidence.
2. Build: `cargo build --workspace --locked`. For a measurement, also `cargo build -p dbide-app --release --locked`.

It is ready when the build finishes. The tests wait on their own servers' ports.

## Drive

Run these in order. Each line names what a pass looks like.

1. **The checks.** `cargo fmt --all --check`, `cargo clippy --workspace --all-targets --locked -- -D warnings` and `node scripts/check-rust-layout.mjs` print no errors.
2. **Engine and CLI against the real servers.** `cargo test --workspace --locked --no-fail-fast -- --skip pg_kerberos_` passes every test except the `ui_` ones (step 3). The CLI's tests drive `dbide-cli` through its real arguments.
3. **The app.** `cargo test -p dbide-app --locked` runs the `ui_` tests: each opens the app's window, plays a script of keystrokes through gpui and asserts the state lines a user would see. Every test passes. They need a signed-in desktop session.
4. **The changed path, as a user sees it.** Write or reuse a script in `crates/app/tests/scripts/` that walks the change (the keys and text a user types, with `shot` steps where it shows), then:
   `pwsh -File .claude/skills/verify/ui-run.ps1 -Exe "$env:CARGO_TARGET_DIR/debug/database-ide.exe" -Script <steps.json> -OutDir "$env:DBIDE_ARTIFACTS/<ticket>"`
   It prints one state line per step and saves a PNG per `shot`. Open the PNGs and check that each shows what the step's name says.
5. **Startup, memory and size**, for a change to the app. Check the PC is quiet first (overall CPU load under about 10%; another busy program skews the first start), then:
   `pwsh -File .claude/skills/verify/measure-app.ps1 -Exe "$env:CARGO_TARGET_DIR/release/database-ide.exe"`
   Every line of its summary ends in `pass`: first start and warm median under 1,000 ms, private working set under 100 MB, zip under 20 MB. Record the load beside the result.

A test that fails and then passes with nothing changed is flaky; the pass doesn't count. Follow the testing rules for flaky tests.

## Scenarios

None yet. The owner writes them in `scenarios/`; until then each record says "no scenarios".

## Evidence

In `$env:DBIDE_ARTIFACTS/<ticket>`: `state.jsonl` and the PNGs from step 4, and the measurement summary from step 5. Engine and CLI tests leave their result and export files in the folders CODE_STANDARDS.md names. A rerun of the same script reproduces the same state lines. Test passwords never appear in any of it.

## Stop

The tests stop the PostgreSQL and MySQL servers they started and drop their SQL Server databases. `ui-run.ps1` and `measure-app.ps1` close the app they started. Leave SQL Server running, and stop only processes this run started.

## Proof that it catches failures

2026-11-17: F6 changed to move two panes instead of one (`cycle(2, …)` in `workbench.rs`). Step 3 failed `ui_f6_f8_and_ctrl_r_move_focus_through_the_visible_panes`: the state line read "Results" where the user should land on "Messages".
