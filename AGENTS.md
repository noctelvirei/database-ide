# Database IDE

A lightweight SQL IDE written in Rust, for SQL Server, PostgreSQL and MySQL: the fast replacement for SSMS a DBA uses every day. Personal tool first; an open-source release stays possible. Windows first, with nothing that rules out macOS or Linux later.

## State

Planning. The route to a first-version spec is the GitHub issue labelled `wayfinder:map`. There is no code yet. `CODE_STANDARDS.md` and the `## Factory` section get written once the UI toolkit is chosen; until then the repo is not fully set up for implementation.

## Agent skills

### Issue tracker

Issues, the wayfinder map and its tickets live in GitHub Issues on the private repo noctelvirei/database-ide, through the `gh` CLI. See `docs/agents/issue-tracker.md`.

### Triage labels

The five standard labels: needs-triage, needs-info, ready-for-agent, ready-for-human, wontfix. See `docs/agents/triage-labels.md`.

### Domain docs

Single-context: `CONTEXT.md` and `docs/adr/` at the repo root. See `docs/agents/domain.md`.
