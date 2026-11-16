# Database IDE

A lightweight SQL IDE written in Rust, for SQL Server, PostgreSQL and MySQL: the fast replacement for SSMS a DBA uses every day. Open source (MIT OR Apache-2.0). Windows first, with nothing that rules out macOS or Linux later.

Read before any change:
- [docs/spec.md](docs/spec.md): what v1 is and how it is put together.
- [CODE_STANDARDS.md](CODE_STANDARDS.md): versions, structure, errors, the checks and the E2E harness.
- [CONTEXT.md](CONTEXT.md): the vocabulary. Use its terms in code and prose.

## State

Building the engine. Codex builds it from Linear tickets; Claude orchestrates, reviews and merges. The UI (gpui) follows in a Claude session on the engine's interface. The planning map stays on GitHub as the issue labelled `wayfinder:map`.

## Public repository

Keep this PC's paths, drive letters, user names, machine names and other private projects out of code, tests, commits, issues and pull requests.

## Factory

Each ticket runs the factory loop (start, plan, build, verify, review, record, merge and close) with these parameters:

| Parameter | Value |
|---|---|
| Tracker | Linear, team "Database-ide" (key `DAT`), project "Engine v1" (implementation tickets). GitHub Issues hold the planning map only. |
| Merge | Claude, the orchestrator, merges once the checks, the E2E harness and review pass. |
| Writer and reviewer | Codex writes each engine ticket; Claude reviews the branch against `main`. |
| Checks | The commands in [CODE_STANDARDS.md](CODE_STANDARDS.md#verification). |
| E2E harness | Named in [CODE_STANDARDS.md](CODE_STANDARDS.md#e2e-harness). |
| Scenarios | `scenarios/`. None written yet; each record says "no scenarios". |
| CI | None. |
| Extra escalations | Adding a dependency or changing a pinned version; changing the engine's run/event interface once a second engine uses it; anything that writes to a server outside a test database. |

Root checkout: stays on a clean `main` matching `origin/main`. Ticket worktrees live on the data drive, one per ticket, from `origin/main`.

## Agent skills

### Issue tracker

The planning map and its tickets live in GitHub Issues on noctelvirei/database-ide through the `gh` CLI. See `docs/agents/issue-tracker.md`.

### Triage labels

The five standard labels: needs-triage, needs-info, ready-for-agent, ready-for-human, wontfix. See `docs/agents/triage-labels.md`.

### Domain docs

Single-context: `CONTEXT.md` and `docs/adr/` at the repo root. See `docs/agents/domain.md`.
