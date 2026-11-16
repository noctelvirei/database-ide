# Main-window layout prototype (throwaway)

**This is a throwaway prototype.** It answers one question: what should Database IDE's main window look like? None of this code goes into the app as it is. It builds on the [gpui spike](../gpui-spike/README.md) and uses the same pins: gpui-component `=0.10.0`, gpui-pre `=0.3.12` and Rust 1.100.0.

The window has three structurally different layouts. All three show the same fake content: PROD-SQL01 (Prod folder, red) and dev-pg (Dev folder, green), three query tabs, a multi-statement batch, the spike's 1M-row grid paged from a file, messages ending in an error on line 15, and the spike's plan tree.

| Variant | Structure |
|---|---|
| **A Classic** | The SSMS shape. The object explorer is docked on the left and the query tabs run across the top. The editor sits above a bottom panel with Results, Messages and Plan tabs, and the status bar is painted in the connection colour. |
| **B Focused** | No permanent explorer: a command palette (Ctrl+K) jumps to tabs, connections and objects. The editor takes the full width under a full-width stripe in the connection colour. Results and Messages are in a bottom drawer (Ctrl+J), and the plan or a jumped-to object opens in a right-hand side panel. |
| **C Workbench** | Three columns. Query tabs are listed vertically, grouped under their connection folder's colour, with the explorer below them. The middle column holds the editor, a messages strip that is always visible, and Results or Plan. On the right, a permanent inspector shows the session facts and whatever was selected last: a plan operator, an object or a results column. |

Screenshots (1600 × 1000, the app window only) are in [`screenshots/`](screenshots): `variant-a.png`, `variant-b.png`, `variant-b-palette.png`, `variant-c.png`, and `variant-c-plan-node.png`, which shows C with the Key Lookup operator selected.

## Run

From this folder:

```
cargo run --release -- --variant a
```

Use `b` or `c` to start in another variant. The first run writes the spike's 118 MB data file to the system temp folder, or to `PROTOTYPE_DATA_DIR` if that is set. Set `CARGO_TARGET_DIR` to keep the build output off the source drive.

## Keys

| Key | Action |
|---|---|
| **F9 / Shift+F9** | Next / previous variant. The black and yellow pill at the bottom centre shows which one is active; it is not part of any design. |
| F6 / Shift+F6 | Next / previous pane in the current variant |
| Ctrl+1 … Ctrl+6 | Editor, results, messages, plan, explorer, inspector (C only). Ctrl+5 opens the palette in B. Each key first reveals its pane (bottom tab, drawer, side panel) |
| Ctrl+PageDown / Ctrl+PageUp, Ctrl+Tab | Next / previous query tab |
| Ctrl+K | Command palette (B) |
| Ctrl+J | Show or hide the results drawer (B) |
| Ctrl+E | Turn edit mode on or off (state only) |
| Up / Down, Enter | Move through C's tab list once it has focus; Enter goes to the editor |
| Arrows | Explorer tree; plan operators; grid cells |

F5, Run and the database dropdowns only draw; nothing runs.

## Flags for checking

- `--palette` opens B's palette at start.
- `--plan-node N` selects plan operator N and shows the plan.
- `--keys f9,ctrl-k,...` replays keys through gpui's own dispatch inside the app, 1.5 s apart. No key goes through the OS, so none can reach another window.

`shots.ps1 -Exe <path to the release exe> -OutDir screenshots` remakes the five screenshots. It starts the app once per shot and copies the client area with PrintWindow.

## Built from

- gpui-component's `Editor`, `DataTable`, `tree` (object explorer) and `command::Command` (B's palette)
- The spike's `grid.rs`, `completion.rs` and `plan.rs`; `plan.rs` gains a scale factor so the plan fits B's side panel (0.5) and C's lower pane (0.85)
- Tab strips, the drawer, the stripe and the inspector are plain `div`s
