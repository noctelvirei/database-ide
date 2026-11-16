# gpui spike results

Run on 2026-11-16, Windows 11 Pro 10.0.26300, 120 Hz display, Rust 1.100.0, release build with `lto = true`, `codegen-units = 1`, `strip = true`.

**Versions:** gpui-component `=0.10.0` (crates.io, published 2026-11-16, the newest) with only the `tree-sitter-sql` feature; gpui as `gpui-pre =0.3.12` and its platform crate `gpui-pre-platform =0.3.12`, the versions gpui-component 0.10.0 pins.

**Verdict: four of the five conditions pass as shipped. "Fully usable by keyboard" is not met out of the box: the find/replace panel's Replace and Replace All have no key.** The likely fix is a patch of about 15 lines to gpui-component's search panel, carried in our own fork; it was read in the source, not built. Everything else the keyboard needed was a one-line key binding in the app.

| Condition (issue #8) | Result | Evidence |
|---|---|---|
| Editor: multi-cursor, find/replace, completion popup | **Pass** | Script steps 13–22, screenshots |
| 1M rows paged from a file scroll with no stutter | **Pass**: p50 8.3 ms at 120 Hz; one frame over 50 ms in 8 end-to-end runs | `evidence/scroll.txt`, `evidence/final-runs.txt` |
| Custom plan tree renders and responds to clicks | **Pass** | Steps 28–32, `plan-click.png` |
| Idle app opens in under 1 s | **Pass**: 796 ms first run after the final build, 125 ms warm median | `evidence/final-runs.txt` |
| Idle app under 100 MB private working set | **Pass**: 70.9 MB | `evidence/final-runs.txt` |
| Fully usable by keyboard | **Not met as shipped**: Replace / Replace All are mouse-only. Fix: a small patch to the search panel (below). Everything else works by keyboard, given one-line app bindings | Steps 1–31 |

## What was built

One 1500 × 1000 window opens with every feature loaded at startup:

- **SQL editor.** gpui-component's `Editor` with tree-sitter SQL highlighting and a `CompletionProvider` over 44 keywords and 5 tables with their columns.
- **Results grid.** gpui-component's `DataTable` over 1,000,000 × 10 rows. The rows live in a 118 MB text file plus an 8 MB index of row offsets. The delegate reads 128-row pages from disk on demand and keeps at most 32 pages (4,096 rows) in memory. Writing the two files takes 0.9 s and happens before the window opens, only when they are missing.
- **Plan tree.** 15 operators laid out right to left as SSMS does, painted on one `canvas()`: boxes, a placeholder icon shape per operator kind, cost % labels (red at 15 % and above), and elbow arrows whose thickness scales with the log of the row count (1–14 px). Clicks are hit-tested against the boxes from the last paint. The details panel shows the selected operator.
- **Keyboard.** F6 / Shift+F6 cycle the four panes; Ctrl+1 … Ctrl+4 jump to one. The focused pane gets a blue border.

The code is in `src/`: `main.rs` (window, panes, keys), `grid.rs` (data file and paging), `completion.rs`, `plan.rs`, `keyscript.rs` and `autoscroll.rs` (measurement only).

## Startup and memory

Measured with the research's `measure.ps1`, unchanged: first frame is the first `render()` call of the root view; memory is read 5 s after it and summed over the app's processes (there is one).

| Build | Startup run 1, straight after the build (ms) | Warm startup, median of runs 2–6 (ms) | Private WS (MB) | Summed WS (MB) | Commit (MB) |
|---|---|---|---|---|---|
| Final (the committed code) | 796 | 125 | 70.9 | 117.0 | 98.9 |
| Earlier (before the scrollbar and measurement changes; `evidence/measure-earlier-build.csv`) | 735 | 148 | 70.9 | 119.5 | 99.1 |

- **Against the research.** The research's loaded gpui app (0.5.1, a 100k-row grid held in a `Vec`) used 98.5 MB private, of which 59 MB was its data. This app holds no rows in memory, so 70.9 MB is the UI with every feature loaded.
- **The 100 MB target** passes on private working set, the measure issue #8 chose, with 29 MB to spare. Summed working set (119 MB) would fail, as the research predicted for gpui.
- **First run after a build** was not a cold start after a reboot; no reboot test was made.
- **Warm runs vary.** Single warm runs across all measurements ranged 124–236 ms.

## Scrolling the 1M-row grid

`scroll.ps1` makes the app scroll itself from row 0 to row 999,999, one step per frame, and records the interval between frames. At 120 Hz a frame is 8.33 ms; a stutter shows as an interval of 25 ms or more (three missed frames).

Each row is one run of 1,004 frames (526 for the drag). Times in ms.

| How it was driven | Run | p50 | p95 | p99 | Max | Frames > 25 ms | Frames > 50 ms |
|---|---|---|---|---|---|---|---|
| `scroll_to_row`, 997 rows per frame | 1 | 8.33 | 8.98 | 20.2 | 44.9 | 7 | 0 |
| | 2 | 8.32 | 8.75 | 9.4 | 18.9 | 0 | 0 |
| | 3 | 8.32 | 8.72 | 9.8 | 16.4 | 0 | 0 |
| | 4 | 8.32 | 8.68 | 9.0 | 13.9 | 0 | 0 |
| Wheel events over the grid, 997 rows per frame | 1 | 8.32 | 9.11 | 15.4 | 121.5 | 5 | 1 |
| | 2 | 8.32 | 8.82 | 20.2 | 43.4 | 4 | 0 |
| | 3 | 8.32 | 8.73 | 9.3 | 15.1 | 0 | 0 |
| | 4 | 8.31 | 8.76 | 11.3 | 19.4 | 0 | 0 |
| Scrollbar thumb dragged 1 px per frame | 1 | 8.34 | 9.28 | 10.7 | 16.9 | 0 | 0 |

- **Worst case by design.** At 997 rows per frame every frame lands on a page that is not cached, so every frame reads from disk and parses 128 rows. Dragging the thumb 1 px moves about 1,900 rows a frame, also a cache miss every frame.
- **The slow frames are single frames in mid-scroll, not warm-up.** Wheel run 2 logged its four at frames 162, 281, 801 and 877 (43, 26, 30 and 35 ms). Across the 8 jump and wheel runs, 16 of 8,032 frames went over 25 ms and one over 50 ms (121 ms, in the first wheel run; it did not come back in three more). Judgement: this passes "no stutter". The 121 ms frame is worth watching when the real grid is built.
- **A second drag run did not scroll.** The synthesized press missed the thumb, so the run only shows idle frames (max 14 ms); it is left out of the table. The successful run went end to end.
- **Memory stays flat.** Private working set before and after scrolling the whole grid stayed within ±2.2 MB in all nine runs, for example 75.4 → 73.8 MB (jump), 75.5 → 75.2 MB (wheel) and 72.1 → 74.3 MB (drag). ("Before" is read 1 s after the first frame, so it sits above the settled 70.9 MB.)
- **The last rows draw correctly.** `grid-end.png` shows rows 999,991–999,999 aligned with their cells, so the roughly 32M px of content height causes no visible float error.
- **Paging was served from a warm file cache.** The data file was written minutes earlier and read many times, so Windows held it in memory. Paging from a cold disk was not measured.
- **Scrollbar mode.** The grid's scrollbar is set to `Always`. gpui-component follows the Windows "hide scroll bars" setting by default, and a hidden scrollbar cannot be grabbed until it appears.

## Keyboard script

`keys.ps1` runs the 33-step script in `src/keyscript.rs` and saves screenshots; `evidence/keyscript.jsonl` holds the app state recorded after every step. Every step gave the expected state.

| Steps | What happened |
|---|---|
| 1–10 | F6 ×4 visits grid, plan, details, editor in turn; Shift+F6 goes back; Ctrl+2/3/4/1 jump straight to each pane |
| 11–12 | Typed a comment line at the end of the SQL, deleted 6 characters with Backspace, typed a replacement |
| 13 | Ctrl+Home, Ctrl+Alt+Down twice, typed `#`: it appeared at the start of lines 1, 2 and 3 (`multi-cursor.png`) |
| 14 | Esc returned to one cursor; Ctrl+Z removed the three `#` |
| 15–16 | Ctrl+F opened find, typed `created`: 3 matches; Enter steps through them (`find.png`) |
| 17 | Ctrl+H opened the replace field; typed `made` (`replace.png`) |
| 17b | Clicked Replace All with the mouse: all three `created` became `made` (`replaced.png`) |
| 19–20 | Typing `SEL` on a new line opened the completion popup with `SELECT`; Enter inserted it (`completion.png`) |
| 21–22 | Typed ` cust`, Esc closed the popup, Ctrl+Space reopened it with `customer_id`, `customers`; Down, Enter inserted `customers` |
| 23–27 | In the grid: Down ×3, Right ×2 → cell (3, 2); PageDown ×3 → row 30; Ctrl+End → row 999,999; Up ×2, PageUp → 999,988; Ctrl+Home → row 0 |
| 28–31 | In the plan: Down selects the root, Down ×3 and Right reach Nested Loops, Left returns to its parent Hash Match; Ctrl+4 moves to the details pane, which shows the selected operator |
| 32 | A mouse click on operator 10 selected Clustered Index Scan and filled the details panel (`plan-click.png`) |

**Gaps.**

- **Replace has no key (the failing condition).** The built-in find/replace panel (`gpui-component` `src/input/search.rs`) offers Replace and Replace All as buttons only. Tab cycles between its two text fields, and Enter in either field moves to the next match. The panel keeps the replacement text private, so the app cannot add the key from outside. Two routes:
  - **Patch the panel** in a fork, as the project already does for tokio-postgres. Declare two actions, bind keys to them in the panel's existing `SearchPanel` key context, and add two `.on_action` lines that call the panel's existing `replace_next` and `replace_all`. About 15 lines; read in the source, not built.
  - **Draw an own find bar** over the editor's public API (`set_search_query`, `replace_current_search_match`, `replace_all_search_matches`). That replaces the off-the-shelf panel, so it is the heavier route.
- **App bindings needed** (each one line in this spike):
  - Ctrl+Space for completion: gpui-component binds no key to `ShowCompletions`.
  - Ctrl+Home / Ctrl+End in the grid.
  - Selecting a first cell when the grid gets focus: the grid's arrow keys do nothing until a cell is selected.
  - F6 and Ctrl+1…4 for the panes. Tab cannot switch panes, because Tab indents in the editor.

## How the keys were sent, and what that does not cover

- **How.** The script sends keys through gpui's `Window::dispatch_keystroke`. That path runs gpui's keymap, the focused element's actions and the text input handler, so it covers everything except Windows' own key-message translation.
- **Why not SendInput.** The first attempt sent real key events with SendInput while the PC was in use. Some never reached the app. One probe without a foreground check sent an F12 press to whatever window was in front. SendInput was dropped for that reason.
- **One script effect not seen with real keys.** Enter that accepts a completion also adds a newline under `dispatch_keystroke` (steps 20 and 22). In the earlier SendInput run, accepting `customers` with Enter added no newline. So this looks like the dispatch path's simulated text input, not a gpui-component bug. It is not confirmed with a physical keyboard.
- **Not verified by hand.** Ctrl+Alt+Up/Down at a physical keyboard: some graphics drivers take that chord for screen rotation. Alt+click also adds a cursor (read in the source, not tried).

## Exe size

`gpui-spike.exe` is 36.4 MB (36,435,968 bytes) with the SQL grammar alone, against 57.5 MB for the research build with every grammar. 7-Zip at maximum compression makes it 5.5 MB, well under the 20 MB installer target.

## Not checked in this spike

- **Narrator** over the editor and grid.
- **A 5 MB script** in the editor. The editor's own documentation says it handles "up to about 50K lines"; the crate README claims 200K.
- **Typing latency.**
- **Cold start after a reboot.**
- **High-DPI scaling and dark theme.**
- **Find options.** The panel shows one toggle (case); it was not exercised, and no regex or whole-word option was seen.
- **Keys from a physical keyboard.** Every scripted key ran through gpui's dispatch rather than the Windows keyboard, as described above.
