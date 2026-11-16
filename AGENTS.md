# Database IDE

A lightweight SQL IDE written in Rust, for SQL Server, PostgreSQL and MySQL: the fast replacement for SSMS a DBA uses every day. Personal tool first; an open-source release stays possible. Windows first, with nothing that rules out macOS or Linux later.

## State

Planning. The route to a first-version spec is the GitHub issue labelled `wayfinder:map`. There is no code yet. `CODE_STANDARDS.md` and the `## Factory` section get written once the UI toolkit is chosen; until then the repo is not fully set up for implementation.

Once the spec is done, the implementation plan moves to Linear as tickets. Codex builds the engine first (everything below the UI, testable with no window), each ticket orchestrated, reviewed and merged by Claude. The UI is then built in a Claude session on the engine's interface. GitHub Issues keep the planning map.

## Agent skills

### Issue tracker

Issues, the wayfinder map and its tickets live in GitHub Issues on the public repo noctelvirei/database-ide (open source since 2026-11-16, MIT OR Apache-2.0; keep this PC's paths and other private projects out of issues, comments and files), through the `gh` CLI. See `docs/agents/issue-tracker.md`.

### Triage labels

The five standard labels: needs-triage, needs-info, ready-for-agent, ready-for-human, wontfix. See `docs/agents/triage-labels.md`.

### Domain docs

Single-context: `CONTEXT.md` and `docs/adr/` at the repo root. See `docs/agents/domain.md`.
