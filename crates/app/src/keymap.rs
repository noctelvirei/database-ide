//! The app's actions and their keys: SSMS's shortcuts as listed in the spec's Keyboard
//! table, plus the pane keys. Not remappable in v1.
//!
//! Find and replace (Ctrl+F, Ctrl+H, and Alt+R / Alt+A in the find panel) and the editor's
//! own keys are bound by gpui-component, as are a dialog's keys: Tab and Shift+Tab between
//! fields, Enter for its action and Escape to cancel.
//!
//! The messages strip's Up, Down and Enter work while it has focus, to choose an error and
//! go to its line.
//!
//! The results grids' keys work while a grid has focus (results.rs lists them). Plain
//! arrows, Home, End, Page Up and Page Down are gpui-component's table keys. Ctrl+C and
//! Ctrl+Shift+C copy as SSMS does, without and with headers.
//!
//! The explorer's keys work while it has focus, so they don't take keys from the editor.
//! They follow Windows' file explorer where it has one: F2 renames, Delete deletes,
//! Alt+Enter opens properties, Ctrl+Shift+N makes a folder, Ctrl+X and Ctrl+V move.

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

actions!(
    explorer,
    [
        /// Adds a connection in the selected folder (Insert).
        NewConnection,
        /// Adds a folder in the selected folder (Ctrl+Shift+N).
        NewFolder,
        /// Opens the selected connection or folder's dialog (Alt+Enter).
        EditItem,
        /// Renames the selected connection or folder (F2).
        RenameItem,
        /// Deletes the selected connection, or folder and all it holds, after asking (Delete).
        DeleteItem,
        /// Picks up the selected item to move it (Ctrl+X).
        CutItem,
        /// Moves the picked-up item into the selected folder (Ctrl+V).
        PasteItem,
        /// Moves the picked-up item to the top level (Ctrl+Shift+V).
        PasteItemAtTop,
        /// Connects the selected connection, or opens or closes a folder (Enter).
        ConnectItem,
        /// Disconnects the selected connection (Ctrl+Shift+D).
        DisconnectItem,
        /// Tests the connection dialog's details (Alt+T).
        TestConnection,
        /// Presses a dialog's action from a closed drop-down (Enter).
        ConfirmFromChoice,
    ]
);

actions!(
    messages,
    [
        /// Chooses the previous error that names a line (Up).
        PreviousError,
        /// Chooses the next error that names a line (Down).
        NextError,
        /// Moves the editor's caret to the chosen error's line (Enter).
        GoToError,
    ]
);

actions!(
    results,
    [
        /// Grows the selected block by a row upwards (Shift+Up).
        ExtendUp,
        /// Grows the selected block by a row downwards (Shift+Down).
        ExtendDown,
        /// Grows the selected block by a column to the left (Shift+Left).
        ExtendLeft,
        /// Grows the selected block by a column to the right (Shift+Right).
        ExtendRight,
        /// Selects the grid's first cell (Ctrl+Home).
        FirstCell,
        /// Selects the grid's last cell: the last column of the last row so far (Ctrl+End).
        LastCell,
        /// Selects every cell of the grid (Ctrl+A).
        SelectAll,
        /// Selects the selected cell's row (Shift+Space).
        SelectRow,
        /// Selects the selected cell's column (Ctrl+Space).
        SelectColumn,
        /// Copies the selected block as tab-separated text (Ctrl+C).
        CopyBlock,
        /// Copies the selected block with its column names (Ctrl+Shift+C).
        CopyBlockWithHeaders,
        /// Moves to the next result set's grid (Ctrl+Down).
        NextResult,
        /// Moves to the previous result set's grid (Ctrl+Up).
        PreviousResult,
        /// Fills the results pane with the focused grid, or stacks them again (Alt+Enter).
        ToggleMaximise,
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
        KeyBinding::new("insert", NewConnection, Some("Explorer")),
        KeyBinding::new("ctrl-shift-n", NewFolder, Some("Explorer")),
        KeyBinding::new("alt-enter", EditItem, Some("Explorer")),
        KeyBinding::new("f2", RenameItem, Some("Explorer")),
        KeyBinding::new("delete", DeleteItem, Some("Explorer")),
        KeyBinding::new("ctrl-x", CutItem, Some("Explorer")),
        KeyBinding::new("ctrl-v", PasteItem, Some("Explorer")),
        KeyBinding::new("ctrl-shift-v", PasteItemAtTop, Some("Explorer")),
        KeyBinding::new("enter", ConnectItem, Some("Explorer")),
        KeyBinding::new("ctrl-shift-d", DisconnectItem, Some("Explorer")),
        KeyBinding::new("alt-t", TestConnection, Some("ConnectionForm")),
        KeyBinding::new("shift-up", ExtendUp, Some("Results")),
        KeyBinding::new("shift-down", ExtendDown, Some("Results")),
        KeyBinding::new("shift-left", ExtendLeft, Some("Results")),
        KeyBinding::new("shift-right", ExtendRight, Some("Results")),
        KeyBinding::new("ctrl-home", FirstCell, Some("Results")),
        KeyBinding::new("ctrl-end", LastCell, Some("Results")),
        KeyBinding::new("ctrl-a", SelectAll, Some("Results")),
        KeyBinding::new("shift-space", SelectRow, Some("Results")),
        KeyBinding::new("ctrl-space", SelectColumn, Some("Results")),
        KeyBinding::new("ctrl-c", CopyBlock, Some("Results")),
        KeyBinding::new("ctrl-shift-c", CopyBlockWithHeaders, Some("Results")),
        KeyBinding::new("ctrl-down", NextResult, Some("Results")),
        KeyBinding::new("ctrl-up", PreviousResult, Some("Results")),
        KeyBinding::new("alt-enter", ToggleMaximise, Some("Results")),
        KeyBinding::new("up", PreviousError, Some("Messages")),
        KeyBinding::new("down", NextError, Some("Messages")),
        KeyBinding::new("enter", GoToError, Some("Messages")),
        // Bound after gpui-component's own Enter on a select, so this one wins.
        KeyBinding::new("enter", ConfirmFromChoice, Some("Dialog > Select")),
    ]);
}
