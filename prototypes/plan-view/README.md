# Plan view prototype (throwaway)

**This is a throwaway prototype.** It answers one question for issue #14: what should Database IDE's execution plan view look like? None of this code goes into the app as it is. It starts from the [main-window layout prototype](../main-window-layout/README.md), keeps only layout C "Workbench" (chosen in issue #13), and puts three plan views into its Plan tab and right-hand inspector. Pins are unchanged: gpui-component `=0.10.0`, gpui-pre `=0.3.12`, Rust 1.100.0.

All three views draw the same real actual plans, parsed into one operator tree:

| Plan | Engine | What it shows |
|---|---|---|
| `row-count-spool-slow` | SQL Server | Parallelism, two hash matches (one spills to tempdb, with a warning), nested loops anti semi joins, a row count spool whose estimate is off by 24 million times |
| `serially-parallel` | SQL Server | Parallelism, nested loops, a key lookup, estimates off by 150× to 1,000× |
| `postgres-shop` | PostgreSQL 18 | A correlated-filter misestimate (128 customers expected, 2,500 found) that every join above it inherits; nested loops, index scans, a hash join, a hash aggregate and a sort |

Sources and licences are in [`plans/NOTICE.md`](plans/NOTICE.md). Estimated rows are multiplied by estimated executions (SQL Server) or loops (PostgreSQL) so they compare with actual rows; without that, every nested-loops inner side shows as a false misestimate.

| Variant | Structure |
|---|---|
| **A SSMS graph** | Right-to-left operator graph with vscode-mssql's operator icons, arrows as thick as the actual rows they carry, cost %, time and "actual of estimated" under each node, warning and parallelism badges. Scrolls both ways. |
| **B Tree table** | One indented, collapsible row per operator with columns for object, estimated rows, actual rows, est/actual factor (red at 10× or more), cost %, own time and logical reads. Rows are tinted by their cost share, time cells by their time share. |
| **C Icicle** | The statement as a full-width bar on top, each operator's inputs below it sharing its width by inclusive cost (T switches to time), largest first. Colour shows the operator's own share. A strip at the bottom lists the five hottest operators. |

Selecting an operator in any view fills the inspector: warnings and the misestimate first, then rows, executions, own and inclusive cost and time, reads, predicates and engine-specific details.

Screenshots (1600 × 1000, the app window only) are in [`screenshots/`](screenshots): each view with the first SQL Server plan (`a-graph-sqlserver.png`, `b-table-sqlserver.png`, `c-icicle-sqlserver.png`) and with the PostgreSQL plan (`*-postgres.png`), plus three with a misestimated operator selected: `misestimate-a-graph-postgres.png` (Seq Scan on customers, 128 estimated, 2,500 actual), `misestimate-b-table-sqlserver-keylookup.png` (Clustered Index Seek, 22,749 estimated, 21 actual) and `misestimate-c-icicle-sqlserver.png` (Nested Loops, 156× too many rows estimated). `warning-a-graph-sqlserver.png` shows A's warning badge on the spilling Hash Match with the spill in the inspector. `keys-check.png` is the result of replaying F9, F8, F8, Down, Down from A: view B, the PostgreSQL plan, Sort selected.

## What the screenshots show

- The Plan tab in layout C gets about 960 × 400 px. A's graph of 16 operators is wider and taller than that, so it scrolls in both directions and never shows the whole plan at once; B and C fit both plans without scrolling sideways.
- B makes the misestimates and the expensive rows readable at a glance (the key lookup at 96 % cost and 41.2M reads, the 1,083× seek), but loses the shape of the plan below a few levels of indentation.
- C shows where the cost goes in one look, but long single-input chains (Compute Scalar, Parallelism, Stream Aggregate) each take a full-width row, and cheap operators are a few pixels wide or not drawn at all. The worst misestimate in `serially-parallel` (the 1,083× seek) costs 0.2 %, so C can't show it: sizing by cost hides exactly the cheap operator whose bad estimate caused the expensive one.
- SQL Server doesn't time some operators (Compute Scalar); they show their input's rows and time, as SSMS does.
- PostgreSQL costs a node per loop. The inner side of a nested loop is multiplied by the outer side's estimated rows so cost percentages add up; without that, C hides the inner index scans entirely.

## Run

From this folder:

```
cargo run --release -- --variant a
```

Use `b` or `c` for the other views. Set `CARGO_TARGET_DIR` to keep build output off the source drive. The Results tab reuses the spike's 1M-row grid, so the first run writes its 118 MB data file to the system temp folder, or to `PROTOTYPE_DATA_DIR` if set.

## Keys

| Key | Action |
|---|---|
| **F9 / Shift+F9** | Next / previous plan view. The black and yellow pill at the bottom shows the view and the plan; it is not part of any design. |
| **F8 / Shift+F8** | Next / previous sample plan (each is its own query tab, so the header and session follow) |
| Arrows (Plan focused) | A: Left toward the result, Right into the first input, Up/Down step through operators. B: Up/Down rows, Left/Right collapse/expand or parent/child. C: Up parent, Down hottest input, Left/Right siblings |
| Space | B: collapse or expand the selected row |
| T | C: size bars by estimated cost or actual time |
| Ctrl+4 / Ctrl+2 | Plan / Results tab |
| F6 / Shift+F6, Ctrl+1 … Ctrl+6 | Panes, as in the layout prototype |

## Flags for checking

- `--plan N` starts on plan N (0 and 1 SQL Server, 2 PostgreSQL).
- `--select N` selects operator N (pre-order, 0 is the statement).
- `--metric time` starts C sized by time.
- `--keys f9,down,...` replays keys through gpui's own dispatch, 1.5 s apart.

`shots.ps1 -Exe <path to the release exe> -OutDir screenshots` remakes the screenshots with PrintWindow; it sends no keys.

## Built from

- Layout C from `../main-window-layout` (editor, explorer, grid, inspector)
- `showplan.rs` (roxmltree) and `pg_json.rs` (serde_json) parse into `model.rs`
- Operator icons from vscode-mssql (MIT), see [`icons/NOTICE.md`](icons/NOTICE.md), drawn with gpui's `img()` from embedded SVG
- Each view is plain `div`s; A paints its arrows on a `canvas` under positioned nodes
