# Database IDE

A lightweight SQL IDE written in Rust, for SQL Server, PostgreSQL and MySQL: the fast replacement for SSMS a DBA uses every day. Open source (MIT OR Apache-2.0). Windows first, with nothing that rules out macOS or Linux later.

Read before any change:
- [docs/spec.md](docs/spec.md): what v1 is and how it is put together.
- [CODE_STANDARDS.md](CODE_STANDARDS.md): versions, structure, errors, the checks and the E2E harness.
- [CONTEXT.md](CONTEXT.md): the vocabulary. Use its terms in code and prose.

## State

The engine is built (Linear project "Engine v1", all done). Building the UI (gpui) on the engine's interface from Linear project "UI v1": Claude writes the UI tickets and Codex reviews them; engine additions the UI needs go to Codex as engine tickets, reviewed by Claude. The planning map stays on GitHub as the issue labelled `wayfinder:map`.

## Public repository

Keep this PC's paths, drive letters, user names, machine names and other private projects out of code, tests, commits, issues and pull requests.

## Factory

Each ticket runs the factory loop (start, plan, build, verify, review, record, merge and close) with these parameters:

| Parameter | Value |
|---|---|
| Tracker | Linear, team "Database-ide" (key `DAT`), projects "Engine v1" and "UI v1" (implementation tickets). GitHub Issues hold the planning map and a public mirror of every open ticket, for contributors. |
| Public mirror | Each open Linear ticket has a GitHub issue titled `[DAT-<n>] <title>`, linked from the Linear ticket. File the mirror when the ticket is filed, with no Linear links or writer/reviewer lines in its body; mark small, fully specified ones `good first issue`. Before starting a ticket, read its mirror: if a contributor has claimed it or opened a pull request, leave the ticket to them. On starting, comment on the mirror that it is taken. The merge message carries `Closes #<mirror>` beside the Linear ID. A contributor's pull request gets the same checks and review as an agent's. |
| Merge | Claude, the orchestrator, merges once the checks, the E2E harness and review pass. |
| Writer and reviewer | Engine tickets: Codex writes, Claude reviews. UI tickets: Claude writes, Codex reviews. Each review is of the branch against `main`. |
| Checks | The commands in [CODE_STANDARDS.md](CODE_STANDARDS.md#verification). |
| E2E harness | Named in [CODE_STANDARDS.md](CODE_STANDARDS.md#e2e-harness); the UI's in [CODE_STANDARDS.md](CODE_STANDARDS.md#ui-e2e-harness). |
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
