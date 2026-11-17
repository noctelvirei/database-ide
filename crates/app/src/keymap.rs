//! The app's actions and their keys: SSMS's shortcuts as listed in the spec's Keyboard
//! table, plus the pane keys. Not remappable in v1.
//!
//! Find and replace (Ctrl+F, Ctrl+H, and Alt+R / Alt+A in the find panel) and the editor's
//! own keys are bound by gpui-component.

use gpui::{App, KeyBinding, actions};

actions!(
    workbench,
    [
        /// Moves focus to the next pane, wrapping after the last (F6).
        NextPane,
        /// Moves focus to the previous pane, wrapping before the first (Shift+F6).
        PrevPane,
        /// Moves focus to the object explorer (F8).
        FocusExplorer,
        /// Hides or shows the results pane (Ctrl+R).
        ToggleResults,
        /// Runs the selection, or the whole tab (F5, Ctrl+E).
        Run,
        /// Cancels the running query (Alt+Break).
        CancelRun,
        /// Shows the estimated plan (Ctrl+L).
        EstimatedPlan,
        /// Turns including the actual plan on or off (Ctrl+M).
        ToggleActualPlan,
        /// Switches the plan between tree and graph (F9).
        TogglePlanView,
        /// Opens a new query tab on the current connection (Ctrl+N).
        NewQueryTab,
        /// Moves to the next query tab (Ctrl+Tab).
        NextQueryTab,
        /// Comments out the selected lines (Ctrl+K Ctrl+C).
        CommentLines,
        /// Uncomments the selected lines (Ctrl+K Ctrl+U).
        UncommentLines,
        /// Formats the selection, or the whole tab (Ctrl+K Ctrl+F).
        Format,
        /// Reloads the autocomplete object list (Ctrl+Shift+R).
        RefreshAutocomplete,
        /// Picks the tab's current database (Ctrl+U).
        PickDatabase,
        /// Opens a `.sql` file (Ctrl+O).
        OpenFile,
        /// Saves the tab to its `.sql` file (Ctrl+S).
        SaveFile,
    ]
);

/// Binds every key above for the whole app. Call once, after `gpui_component::init`.
///
/// Alt+Break is bound as `alt-pause`, but gpui 0.3.12 does not translate the Windows
/// Pause/Break key into a keystroke, so a physical Alt+Break does not reach the app yet.
pub fn bind(cx: &mut App) {
    cx.bind_keys([
        KeyBinding::new("f6", NextPane, None),
        KeyBinding::new("shift-f6", PrevPane, None),
        KeyBinding::new("f8", FocusExplorer, None),
        KeyBinding::new("ctrl-r", ToggleResults, None),
        KeyBinding::new("f5", Run, None),
        KeyBinding::new("ctrl-e", Run, None),
        KeyBinding::new("alt-pause", CancelRun, None),
        KeyBinding::new("ctrl-l", EstimatedPlan, None),
        KeyBinding::new("ctrl-m", ToggleActualPlan, None),
        KeyBinding::new("f9", TogglePlanView, None),
        KeyBinding::new("ctrl-n", NewQueryTab, None),
        KeyBinding::new("ctrl-tab", NextQueryTab, None),
        KeyBinding::new("ctrl-k ctrl-c", CommentLines, None),
        KeyBinding::new("ctrl-k ctrl-u", UncommentLines, None),
        KeyBinding::new("ctrl-k ctrl-f", Format, None),
        KeyBinding::new("ctrl-shift-r", RefreshAutocomplete, None),
        KeyBinding::new("ctrl-u", PickDatabase, None),
        KeyBinding::new("ctrl-o", OpenFile, None),
        KeyBinding::new("ctrl-s", SaveFile, None),
    ]);
}
