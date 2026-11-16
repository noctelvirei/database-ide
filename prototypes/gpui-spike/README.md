# gpui spike (throwaway prototype)

**This is a throwaway prototype**, built in one day to answer one question for [issue #8](https://github.com/noctelvirei/database-ide/issues/8): do gpui and gpui-component, current versions on Rust 1.100, meet the toolkit pass conditions? It is not product code and nothing here should be copied into the app without being rewritten. The answer is in [RESULTS.md](RESULTS.md).

One window opens with every feature loaded:

- a SQL editor (gpui-component `Editor`, tree-sitter SQL) with a completion popup fed by a fixed keyword, table and column list
- a results grid (gpui-component `DataTable`) over 1,000,000 × 10 rows paged from a file on disk
- a custom-drawn plan tree of 15 operators, and a details panel for the selected operator

## Run

From this folder:

```
cargo run --release
```

`rust-toolchain.toml` pins Rust 1.100.0. The first run writes the 1M-row data file (about 140 MB) to a folder in the system temp directory, or to `SPIKE_DATA_DIR` if set; later runs reuse it. Set `CARGO_TARGET_DIR` to keep build output off the source drive.

## Keys

| Key | Action |
|---|---|
| F6 / Shift+F6 | Next / previous pane |
| Ctrl+1 … Ctrl+4 | Editor, grid, plan, details |
| Ctrl+Alt+Up / Down, Alt+click | Add a cursor (editor) |
| Ctrl+F / Ctrl+H | Find / replace (editor) |
| Ctrl+Space | Completion (editor; it also opens while typing a word) |
| Arrows, PageUp/Down, Home/End, Ctrl+Home/End | Move in the grid |
| Arrows | Plan: Up/Down previous/next operator, Left parent, Right first child |
| F12 | Write app state to `SPIKE_DUMP` (measurement only) |

Ctrl+Space, Ctrl+Home/End in the grid, the pane keys and the plan's arrow keys are bound by the spike, not by gpui-component; see RESULTS.md.

## Measurement scripts

- `measure.ps1`: startup and memory, the same method as the toolkit research.
- `scroll.ps1 -Mode jump|wheel|drag`: frame times while the grid scrolls itself end to end, then memory.
- `keys.ps1`: runs the keyboard script in `src/keyscript.rs` and saves screenshots; the app records its state after each step.

Each script takes `-Exe` with the path of the release exe. Their outputs from the spike run are in `evidence/`.
