# Which UI toolkit can meet the speed and memory targets?

Research for issue #3, done 2026-11-16 on Windows 11 Pro 10.0.26300, Rust 1.93.0, WebView2 runtime 157.0.4341.46.

Targets: open in under 1 s; under 100 MB of memory with one connection; installer under 20 MB. The app also needs a results grid that scrolls through 1M+ rows, a usable SQL code editor and a graphical plan tree.

The doc keeps three kinds of statement apart:

- **Measured**: numbers from the benchmark below. Source code is in [`ui-toolkit-bench/`](ui-toolkit-bench/).
- **Cited**: claims from primary sources, each with a link and a date.
- **Judgement**: my reading of the first two, marked as such.

## Recommendation

**gpui, with the gpui-component library (now called GPUI Kit).** Make it conditional on a one-day spike, described under "What the spike must settle".

The reason (judgement):

- **The UI's own memory cost decides it.** With a SQL editor and a 100,000-row grid loaded, the measurements are below. Subtract the 59 MB the test data takes on its own, and the UI itself costs about 39 MB for gpui and about 98 MB for Tauri (WebView2). That gap holds under any memory metric and any data volume.

  | Loaded app | Private WS | Summed WS | Commit |
  |---|---|---|---|
  | gpui | 98.5 MB | 143 MB | 154 MB |
  | Tauri | 157 MB | 399 MB | 218 MB |

  - gpui passes the 100 MB target only on private working set. Which metric counts is the owner's decision (see Method).
- **Minimal window.** gpui, egui (glow) and Slint pass all three memory columns. Tauri passes only private working set (83 MB). It fails summed working set (319 MB) and commit (141 MB).
- **Ready-made widgets.** gpui-component ships a virtualised table and a code editor with tree-sitter SQL highlighting, find/replace and a completion hook. The current docs also list multi-cursor. Those docs describe 0.10, while 0.5.1 was the version measured (see the loaded-window notes).
- **Tauri** gets the best editor and existing plan viewers for free, but WebView2 costs too much. An empty page used 83 MB private. Loaded, the webview processes reached 96 MB before counting any data, so Tauri cannot meet a 100 MB target once the app holds results.
- **egui** (glow renderer) and **Slint** are the lightest: 23–28 MB. Neither has a code editor with multi-cursor and find/replace, so the editor would have to be written. egui is the fallback if the gpui spike fails.

The costs of gpui:

- It is pre-1.0, and its API breaks between releases.
- The current gpui-component (0.6 and later, on Zed's `gpui-pre` snapshots) needs a newer Rust than the 1.93 installed here.
  - So the loaded app was built with gpui-component 0.5.1 on crates.io gpui 0.2.2, from February 2026.
  - Updating the toolchain was refused in this session and needs the owner.
- Windows screen-reader support is new and unproven (see Accessibility).

## Measured results

### Method

[`measure.ps1`](ui-toolkit-bench/measure.ps1) starts the exe, waits for the app to write the time of its first frame, waits 5 s more, then sums memory over the process and **every descendant**. For Tauri that is `hello-tauri.exe` plus 6 × `msedgewebview2.exe`. It reports three memory columns:

- **Summed working set** (`Win32_Process.WorkingSetSize`). This is the figure the ticket asked for. It counts shared DLL pages once per process, so it overstates multi-process apps.
- **Private working set**: the "Memory" column in Task Manager. It is RAM no other process shares.
- **Commit**: private bytes, including memory that is reserved but not resident.

Further details:

- **Builds**: every app was built in release with `lto = true`, `codegen-units = 1` and `strip = true`.
- **Runs**: five per app. The table gives run 1 and the median of runs 2–5.
- **Run 1** came right after a build, with the exe in the file cache. It is not a cold start after a reboot, and no reboot test was made.
- **First-frame hooks differ**:
  - egui: first `ui()` call.
  - Slint: rendering notifier, `AfterRendering`.
  - gpui: first `render()` call, just before paint, so it is slightly optimistic.
  - Tauri: double `requestAnimationFrame` in the page, after paint, so it is slightly pessimistic.
- **Window sizes differ**: gpui opened at 1552×903 and Tauri at 800×600. The larger window costs gpui GPU memory, so its comparison is conservative.

**Open decision for the owner**: which of the three columns "under 100 MB" means. The table gives all three.

### Minimal window

| Toolkit, version | Startup run 1 / median (ms) | Processes | Summed WS (MB) | Private WS (MB) | Commit (MB) | Exe (MB) | Installer or 7z (MB) |
|---|---|---|---|---|---|---|---|
| egui/eframe 0.35, wgpu renderer (default) | 1178 / 467 | 1 | 210.8 | 122.4 | 424.3 | 12.3 | 3.7 (7z) |
| egui/eframe 0.35, glow renderer | 351 / 85 | 1 | 83.7 | 28.4 | 44.2 | 6.5 | 2.2 (7z) |
| Slint 1.18.1 (default femtovg) | 599 / 128 | 1 | 82.4 | 23.1 | 39.2 | 10.2 | 4.0 (7z) |
| gpui 0.2.2 (crates.io) | 468 / 152 | 1 | 56.2 | 26.6 | 63.8 | 6.1 | 1.7 (7z) |
| Tauri 2.12.6 + WebView2 | 862 / 460 | 7 | 318.8 | 83.0 | 140.6 | 4.3 | **1.32 NSIS installer** |

Notes on the table:

- **Tauri per process** (private WS, MB, from a separate run totalling 87.4): the host exe 4.2. The six `msedgewebview2.exe` children 27.6, 30.8, 14.0, 6.5, 2.7 and 1.6.
- **Tauri installer**: built with the default `downloadBootstrapper`. It assumes the WebView2 runtime is present or downloadable.
- **Native installers**: only Tauri's installer was built. For the native apps, the 7z of the exe at maximum compression stands in for an installer. Every candidate is far under 20 MB.
- **egui's default renderer**:
  - eframe 0.35 defaults to wgpu, which on this PC took 122 MB private and 424 MB commit for an empty window.
  - Switching to the `glow` feature cuts that to 28 MB. Passing the memory target depends on this feature flag.
- **Versions**:
  - egui 0.37 and Slint 1.19 are current.
  - 0.35 and 1.18.1 were the newest that build on Rust 1.93: egui 0.37 needs Rust 1.95.

### Loaded window: SQL editor plus a 100,000 × 10 grid

Only gpui and Tauri have an off-the-shelf editor with multi-cursor and find/replace (see Capabilities), so only those two were built loaded. Both apps hold the same 100,000 × 10 strings in Rust memory, such as `row 000123 col 4`. Screenshots confirmed that each drew its editor and grid.

- **gpui**: gpui-component 0.5.1:
  - `InputState::code_editor("sql")` with the tree-sitter SQL grammar
  - `Table` with a `TableDelegate` over the rows
- **Tauri**: CodeMirror 6 with `lang-sql` and `basicSetup`, bundled by esbuild to 437 KB:
  - a hand-written virtual grid
  - the page fetches only the visible rows through a `rows(start, count)` command, so the data is not held twice

A **data-only** process holding the same strings with no UI used **59.3 MB private**. Subtract it to get the UI's own cost.

| App | Startup run 1 / median (ms) | Summed WS (MB) | Private WS (MB) | Commit (MB) | Private WS minus data (MB) | Exe (MB) | Installer or 7z (MB) |
|---|---|---|---|---|---|---|---|
| gpui 0.2.2 + gpui-component 0.5.1 | 463 / 205 | 143.2 | 98.5 | 153.9 | ~39 | 57.5 | 5.5 (7z) |
| Tauri 2.12.6 + CodeMirror 6 | 1065 / 496 | 399.0 | 157.1 | 217.9 | ~98 | 4.5 | 1.45 NSIS installer |

Notes on the table:

- **Tauri per process** (private WS, MB, from a separate run totalling 159.2): host 63.3, which is mostly the data. The webview children 36.6, 27.3, 21.2, 6.5, 2.7 and 1.6, totalling 95.9.
- **gpui-component version**: the measured app uses 0.5.1, but the editor features cited in the Capabilities table come from the 0.10 docs. The 0.5.1 source has an `input/search.rs`, so find/replace is likely there. Multi-cursor in 0.5.1 is not verified. The 98.5 MB is a measurement of 0.5.1, not of the full 0.10 feature set.
- **gpui exe size**: 57.5 MB, because the `tree-sitter-languages` feature compiles in every bundled grammar. Building with only the SQL grammar would shrink it. Not measured.
- **Tauri startup**: the loaded app's run 1, at 1065 ms, missed the 1 s target. Its warm runs took 470–700 ms.
- **Not measured**:
  - scroll smoothness at 1M rows
  - typing latency
  - memory after scrolling the whole grid

Judgement: no toolkit can keep 1M result rows in memory under 100 MB. Our test strings take about 59 bytes per cell, so 1M × 10 cells would be about 590 MB. The engine has to page or spill large results to disk whichever UI is chosen. With paging, the UI's own cost decides the target: about 40 MB for gpui and about 100 MB for WebView2.

## Capabilities (cited)

| | Grid for 1M+ rows | Code editor (SQL, multi-cursor, find/replace, completion) | Plan tree drawing | Accessibility (Windows) | Maintenance, licence |
|---|---|---|---|---|---|
| **gpui + gpui-component** | `DataTable` virtualises rows; "hundreds of thousands of rows" claimed, no 1M benchmark [1] | Editor with tree-sitter or syntect highlighting, multi-cursor, built-in find/replace, folding, completion provider hook; claims 200K-line files [2] | `canvas()` element for custom painting [3] | AccessKit Windows adapter in `gpui-pre-windows` 0.3.12 [4]; Zed issue "Windows: Screen reader accessibility missing completely" still open, updated 2026-05-22 [5] | gpui pre-1.0, "breaking changes common" [6]; gpui-component 0.10.0 on 2026-11-16, weekly releases [7]; Apache-2.0 |
| **Tauri 2 (WebView2)** | Web grids: Glide Data Grid (canvas, "scales to millions of rows", MIT) [8]; AG Grid Community is MIT, but find, range selection and Excel export are Enterprise-only [9] | CodeMirror 6: multiple selections, search/replace, autocompletion, `lang-sql`; about 135 KB gzipped with basic setup [10]. Monaco (MIT) also possible [11] | Reuse HTML viewers: html-query-plan for SQL Server `.sqlplan` (MIT, last push 2024-03-04) [12]; pev2 for PostgreSQL (PostgreSQL licence, v1.25.0 2026-10-20) [13]; none established for MySQL | Chromium's UIA support (not verified here) | Tauri 2.12.6, 2026-11-16 [14]; Tauri 3.0.0-alpha.6 on crates.io 2026-11-06 [15]; Apache-2.0 / MIT |
| **egui/eframe** | `TableBuilder` `rows()` lays out only visible rows [16]; `egui_table` claims "millions of rows", repo archived, moved to rerun-io/egui_crates [17] | Highlighting via egui_extras/syntect [18]; egui_code_editor 0.5.0 has completion, no multi-cursor or find/replace [19]; no multi-cursor in `TextEdit` found | `Painter`: lines, rects, circles, text [20] | AccessKit on Windows, on by default in eframe [21] | 0.37.0 on 2026-10-15, about monthly; breaking changes each release [21]; MIT / Apache-2.0 |
| **Slint** | `ListView` instantiates only visible items [22]; `StandardTableView` with large models had freezes reported up to 2024, unresolved in thread [23] | `TextEdit` only; no highlighting, multi-cursor, find/replace or completion found [24] | `Path` element (SVG-style commands) [25] | Accessible roles since 1.6, via AccessKit [26] | 1.19.0 on 2026-11-16 [27]; GPLv3, or royalty-free if the app shows `AboutSlint` or a download-page badge [28] |
| **iced 0.14** | `table` widget (0.14.0) lays out every cell; no virtualisation [29] | `text_editor` with highlighter; no multi-cursor, find/replace or completion [30] | `Canvas` with paths [31] | Not shipped; issue #552 open [32] | 0.14.0 on 2025-12-07, 12–15 months between minors; "experimental" [33]; MIT |
| **wxDragon (wxWidgets)** | `DataViewVirtualListModel` [34] | `StyledTextCtrl` (Scintilla) [34] | Not checked | Native Win32 controls (not verified) | 0.9.24 on 2026-10-26; MIT / Apache-2.0 [35]. Not measured: needs CMake and Ninja |

Other contenders I looked at and set aside, all cited 2026-11-16:

- **Freya**: 0.6 alpha; it has a code editor but no table [36].
- **Dioxus Blitz**: beta, "many bugs and missing features" [37].
- **Floem**: last release 2024-11 [38].
- **Makepad**: 1.0.0, 2025-05 [39].
- **Xilem**: "experimental" [40].

## What the spike must settle

Before committing to gpui (judgement):

1. **Toolchain.** Update Rust to current stable and build against gpui-component 0.10 (`gpui-pre` 0.3.12), the version a real project would use. Re-measure the loaded app. This session could not: the toolchain install was refused.
2. **1M rows.** Back a `DataTable` with a paged source of 1M rows and check that the scrollbar, jumping to the end, and fast scrolling stay smooth.
3. **Editor fit.** Wire SQL completion through the completion provider. Check find/replace options such as regex and match case, and test with a 5 MB script.
4. **Screen reader.** Run Narrator over the editor and table on Windows.
5. **Exe size.** Build with only the SQL grammar.

If 1 or 2 fails, the fallback is **egui with the glow renderer**. It has the lowest startup (85 ms) and 28 MB. The cost is writing a SQL editor with multi-cursor, find/replace and completion on top of `TextEdit` or a custom widget.

## Sources

All fetched 2026-11-16.

1. https://gpui-kit.com/component/data-table, https://github.com/longbridge/gpui-component
2. https://gpui-kit.com/component/editor
3. https://docs.rs/gpui/latest/gpui/fn.canvas.html
4. https://crates.io/crates/gpui-pre-windows/0.3.12 (dependencies `accesskit`, `accesskit_windows`); https://github.com/zed-industries/zed/blob/main/crates/gpui_windows/Cargo.toml
5. https://github.com/zed-industries/zed/issues/41138
6. https://github.com/zed-industries/zed/blob/main/crates/gpui/README.md
7. https://crates.io/crates/gpui-component/versions
8. https://github.com/glideapps/glide-data-grid
9. https://github.com/ag-grid/ag-grid
10. https://codemirror.net/, https://codemirror.net/examples/bundle/
11. https://github.com/microsoft/monaco-editor
12. https://github.com/justinpealing/html-query-plan
13. https://github.com/dalibo/pev2/releases
14. https://github.com/tauri-apps/tauri/releases
15. https://crates.io/crates/tauri/versions
16. https://docs.rs/egui_extras/latest/egui_extras/struct.TableBody.html
17. https://github.com/rerun-io/egui_table
18. https://docs.rs/egui_extras/latest/egui_extras/syntax_highlighting/index.html
19. https://github.com/p4ymak/egui_code_editor
20. https://docs.rs/egui/latest/egui/struct.Painter.html
21. https://github.com/emilk/egui (README, releases)
22. https://docs.slint.dev/latest/docs/slint/reference/std-widgets/views/listview/
23. https://github.com/slint-ui/slint/discussions/3601
24. https://archive.slint.dev/1.5.1/docs/slint/src/language/widgets/textedit
25. https://docs.slint.dev/latest/docs/slint/reference/elements/path/
26. https://slint.dev/blog/slint-1.6-released
27. https://github.com/slint-ui/slint/releases
28. https://slint.dev/terms-and-conditions
29. https://github.com/iced-rs/iced/blob/master/widget/src/table.rs, https://github.com/iced-rs/iced/releases/tag/0.14.0
30. https://docs.rs/iced/latest/iced/widget/text_editor/struct.TextEditor.html
31. https://docs.rs/iced/latest/iced/widget/canvas/index.html
32. https://github.com/iced-rs/iced/issues/552
33. https://github.com/iced-rs/iced (README)
34. https://docs.rs/wxdragon/latest/wxdragon/all.html
35. https://github.com/AllenDang/wxdragon
36. https://github.com/marc2332/freya
37. https://github.com/DioxusLabs/blitz
38. https://crates.io/crates/floem
39. https://crates.io/crates/makepad-widgets
40. https://github.com/linebender/xilem
