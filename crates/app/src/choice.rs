//! Drop-down choices over a fixed table of `(label, value)` rows, as the dialogs use them.

use gpui::{App, AppContext as _, Entity, FocusHandle, Window};
use gpui_component::IndexPath;
use gpui_component::dialog::Confirm;
use gpui_component::select::SelectState;

use crate::keymap::ConfirmFromChoice;

/// A select whose rows are a table's labels.
pub type Choice = SelectState<Vec<&'static str>>;

/// A select over `options`' labels with `current`'s row chosen, or the first row when
/// `current` isn't in the table.
pub fn choice<T: PartialEq + Copy>(
    options: &[(&'static str, T)],
    current: T,
    window: &mut Window,
    cx: &mut App,
) -> Entity<Choice> {
    let labels: Vec<&'static str> = options.iter().map(|(label, _)| *label).collect();
    let row = options.iter().position(|(_, v)| *v == current).unwrap_or(0);
    cx.new(|cx| SelectState::new(labels, Some(IndexPath::new(row)), window, cx))
}

/// The chosen row's value. Every choice starts with a row chosen and offers only the
/// table's labels, so the first row's value is never reached in practice.
pub fn value<T: Copy>(options: &[(&'static str, T)], select: &Entity<Choice>, cx: &App) -> T {
    let label = label(select, cx);
    options
        .iter()
        .find(|(l, _)| *l == label)
        .map_or(options[0].1, |(_, v)| *v)
}

/// The chosen row's label.
pub fn label(select: &Entity<Choice>, cx: &App) -> &'static str {
    select
        .read(cx)
        .selected_value()
        .copied()
        .unwrap_or_default()
}

/// The handler for Enter on a closed choice in a dialog: presses the dialog's action, as
/// Enter does on a closed drop-down in a Windows dialog. The select would otherwise take
/// Enter to open its list; Down, Alt+Down and Space still open it.
///
/// Focus moves to `form`, the dialog's content, first, so the dialog rather than the
/// select receives the `Confirm`.
pub fn confirm_dialog(form: FocusHandle) -> impl Fn(&ConfirmFromChoice, &mut Window, &mut App) {
    move |_, window, cx| {
        window.focus(&form, cx);
        window.dispatch_action(Box::new(Confirm { secondary: false }), cx);
    }
}
