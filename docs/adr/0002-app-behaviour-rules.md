# App behaviour rules

This record holds the non-obvious rules behind the gpui app in `crates/app/src`: choices, invariants and server behaviour that the code alone doesn't show. Until 2026-11-20 each sat as a comment beside its code; CODE_STANDARDS.md now keeps such rules here, one place per rule.

A change to the code a rule names either keeps the rule or edits it here in the same commit. Rules are grouped by the file that holds the code.

## `crates/app/src/connection_dialog.rs`

- In ConnectionDialog::output (crates/app/src/connection_dialog.rs), a blank password while editing keeps the saved password, but a new connection saves exactly what was typed, even a blank password, so a server with an empty password can be saved.

## `crates/app/src/explorer.rs`

- A rename that fails to save returns false from the dialog's on_ok in Explorer's rename dialog (crates/app/src/explorer.rs), keeping the dialog open with the typed name so the user can retry; the error goes to the messages strip.
- The explorer's dialog close handler (closer in crates/app/src/explorer.rs) refocuses the explorer only when its dialog is still set: a dialog that handed work to a query tab clears self.dialog first, so focus stays where that tab put it.
- Explorer::render (crates/app/src/explorer.rs) builds each row's look into an Rc snapshot first, because the tree's row closure outlives the render call and can't borrow the explorer.
- Explorer's ConnectItem handler (crates/app/src/explorer.rs) ignores Enter while the filter box has focus or the item menu is open: their Enter bubbles up to the explorer as ConnectItem, and it belongs to them.
- Every row in the explorer's tree has a RowLook in Explorer::render's snapshot (crates/app/src/explorer.rs), because both are built from the same saved folders, connections and object_rows; the default look is only a fallback that should not occur.

## `crates/app/src/explorer/connect.rs`

- When a connect finishes after its connection was edited or deleted (the attempt in Explorer::connecting no longer matches, crates/app/src/explorer/connect.rs), the new session is closed rather than kept, because it was opened with out-of-date details.

## `crates/app/src/explorer/objects.rs`

- The explorer's item menu (Explorer::open_menu, crates/app/src/explorer/objects.rs) sets the explorer's focus handle as its action context, so the chosen action is dispatched to the explorer, which applies it to the selected row.

## `crates/app/src/export_file.rs`

- export_file::write (crates/app/src/export_file.rs) deletes the engine's `<staging>.partial` file after a failed export, because the engine keeps failed writes as `.partial` and nobody else knows the hidden staging name, so the file would be left behind unseen.

## `crates/app/src/history.rs`

- history::append (crates/app/src/history.rs) writes each entry and its newline in one write to a file opened in append mode, so a concurrent reader sees whole lines or none of the new one.
- history::load (crates/app/src/history.rs) sorts entries by start time, because runs are appended when they finish, so a long run lands after a later short one in the file.

## `crates/app/src/history_panel.rs`

- The history panel's on_ok (crates/app/src/history_panel.rs) closes the dialog itself before reopening the run, because the dialog's own close happens after its exit animation and would hand focus back after the new tab had taken it.

## `crates/app/src/keymap.rs`

- In keymap::bind (crates/app/src/keymap.rs), the tab's Ctrl+S is scoped to the Workbench context, because gpui ranks a context-less binding as matching at the focused element, above any context the focus sits inside, so a context-less SaveFile would beat the grid's and the plan's Ctrl+S.
- The menu-bar keys (Alt+F, F10, Alt+E, Alt+V, Alt+Q, Alt+H) in keymap::bind (crates/app/src/keymap.rs) are scoped to the Workbench context, like Ctrl+S, so a dialog's own keys stay its own.
- In keymap::bind (crates/app/src/keymap.rs), the Enter binding for ConfirmFromChoice in "Dialog > Select" must be bound after gpui-component's own Enter on a select, so ours wins; keymap::bind must run after gpui_component::init.

## `crates/app/src/main.rs`

- main (crates/app/src/main.rs) writes DBIDE_FIRST_FRAME from a gpui next-frame callback, because those run once the current frame has been drawn and presented, which is the moment the startup measurement needs.

## `crates/app/src/menu_bar.rs`

- MenuBar's popup builder (crates/app/src/menu_bar.rs) draws an item's row itself when its binding is a chord (Format, Ctrl+K Ctrl+F), because gpui-component's PopupMenu shows only a binding's first keystroke.
- MenuBar's DismissEvent handler (crates/app/src/menu_bar.rs) clears the open menu only if the dismissed popup is still the current one, because a menu replaced by Left or Right is dismissed after its successor is already open.
- The About panel (crates/app/src/menu_bar.rs) shows the embedded plan icons' licence text, because that licence requires its notice in every copy of the app; it must stay while the icons ship.
- The menu bar (crates/app/src/menu_bar.rs) reads Left and Right as raw key events to move between menus, because they reach the bar only when the open menu has no submenu to enter, and gpui-component keeps its own menu actions private.

## `crates/app/src/query_tab.rs`

- QueryTab's format command (crates/app/src/query_tab.rs) formats a selection's trimmed core and puts back its own leading and trailing whitespace, because the formatter trims a fragment's outer whitespace and a trailing line break may be what ends a `--` comment.
- When formatting a selection, QueryTab (crates/app/src/query_tab.rs) first sets the editor's selection to exactly the formatted range and then replaces it, so only that range changes and other cursors' selections are not replaced.
- QueryTab::go (crates/app/src/query_tab.rs) redraws on a timer while a run lasts, because row counts (in the grids, or the rows written for a run to file) grow on the runtime without sending an update.
- In QueryTab::go (crates/app/src/query_tab.rs), the update channel closing without a Finished update (which hands the session back) means the engine stopped; the tab marks the run failed and says so.
- QueryTab::apply (crates/app/src/query_tab.rs) converts engine line numbers, which count from one within the submitted batch, to editor lines, which count from zero, by adding the batch's first editor line and subtracting one.
- Only SQL Server plans can be saved (QueryTab::apply, crates/app/src/query_tab.rs), because `.sqlplan` files hold SQL Server's showplan XML only.
- QueryTab::note_event (crates/app/src/query_tab.rs) adds no message for result-set, row and plan events, because the runtime (crates/app/src/runtime.rs) turns those into result sets and plans before they reach the tab.

## `crates/app/src/query_tab/dialogs.rs`

- The danger dialog's description (crates/app/src/query_tab/dialogs.rs) is read from the tab's dialog state inside the dialog's builder closure, which runs on every draw, so row estimates appear as they arrive.
- The estimate listener in crates/app/src/query_tab/dialogs.rs ends when the `found` channel closes, which the runtime does when estimating finishes or is stopped; it needs no other stop signal.

## `crates/app/src/results/plan.rs`

- Plan rows (crates/app/src/results/plan.rs and plan/graph.rs) are in depth-first tree order, so an operator's first input is the row directly after it; Step::Child and the graph layout rely on this.

## `crates/app/src/results/plan/graph.rs`

- The plan graph layout (crates/app/src/results/plan/graph.rs) gives leaves successive rows in order and puts each parent level with its first input, which is the row directly after it in tree order.
- gpui's scroll offset is how far content has scrolled, as a negative distance; the graph's reveal in crates/app/src/results/plan/graph.rs negates it to get the first shown pixel and returns offsets in the same sign.
- In the plan graph's element (crates/app/src/results/plan/graph.rs), the boxes are the first children so a box's child index is its row (selected_in_view relies on it), and the last child spans the whole graph so the scroll area covers every box.

## `crates/app/src/runtime.rs`

- Runtime::connect (crates/app/src/runtime.rs) ignores a failed send of the connect outcome: the receiver is gone if the window closed meanwhile, and the unsent session is dropped, which closes it.
- Runtime::estimate_rows (crates/app/src/runtime.rs) ignores a failed send of an estimate: a closed receiver means the danger dialog has gone, and the stop signal that dropping it sends ends the estimating.
- The runtime's result collector (complete_current in crates/app/src/runtime.rs) ignores a failed spill-store completion, because a store that failed to write still serves the rows it kept and the grid's heading reports the loss.

## `crates/app/src/ui_script.rs`

- UiScript::load (crates/app/src/ui_script.rs) deletes leftover done, shot and mark signal files from an earlier run, because a waiting test runner would read them as this run's signals and stop early.
- UiScript::play (crates/app/src/ui_script.rs) parses script keys with Keystroke::parse_with_os, because Keystroke::parse wants a 'static key string; the OS argument only resolves `secondary-`, which scripts don't use.

## `crates/app/src/workbench.rs`

- Workbench::open_file (crates/app/src/workbench.rs) clears `opening` only after the tab has reported what came of the open, because is_busy reads it and a waiting UI script must not see the workbench as idle before the tab's message appears.
- When a menu opens from the results pane (Workbench's menu situation, crates/app/src/workbench.rs), the edit target is the focused grid itself rather than the pane, so the grid's own keys still work when the menu hands focus back; its actions reach the results by bubbling.
- Workbench (crates/app/src/workbench.rs) handles NewConnection itself because the File menu's New connection must work from any pane, while the Insert key is bound only in the Explorer context (crates/app/src/keymap.rs).

## `crates/app/src/workbench/files.rs`

- When a query tab asks to sign in (Workbench's QueryTabEvent::SignIn handler, crates/app/src/workbench/files.rs), earlier tabs still waiting stop waiting, because the explorer's sign-in is modal: a new one cancelled theirs, and they must not run when this one signs in.
