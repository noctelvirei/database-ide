//! The query history panel (Ctrl+Shift+H, View, Query history): a search box over past runs,
//! newest first. Enter or a click reopens a run's text in a new tab on its connection.

use std::cell::Cell;
use std::rc::Rc;

use gpui::{
    AppContext as _, Entity, Focusable as _, InteractiveElement as _, ParentElement as _,
    SharedString, StatefulInteractiveElement as _, Styled as _, Window, div, px,
};
use gpui_component::dialog::DialogEntrance;
use gpui_component::input::{Input, InputState};
use gpui_component::{ActiveTheme as _, WindowExt as _};
use serde::Serialize;

use crate::history::{self, Entry};

/// The rows shown at most; the search narrows a longer history.
const SHOWN: usize = 200;

/// The open panel.
pub struct HistoryPanel {
    input: Entity<InputState>,
    entries: Rc<Vec<Entry>>,
    now_ms: u64,
}

/// What a user can see of the panel.
#[derive(Debug, Serialize)]
pub struct HistoryState {
    pub typed: String,
    /// The matching runs' summaries, newest first.
    pub matches: Vec<String>,
}

impl HistoryPanel {
    /// Opens the panel over `entries` (newest first). `reopen` runs with the chosen entry
    /// after the panel closes; `closed` runs when Escape or the close button shuts it.
    pub fn open(
        entries: Vec<Entry>,
        reopen: impl Fn(Entry, &mut Window, &mut gpui::App) + 'static,
        closed: impl Fn(&mut Window, &mut gpui::App) + 'static,
        window: &mut Window,
        cx: &mut gpui::App,
    ) -> Self {
        let input = cx.new(|cx| InputState::new(window, cx));
        let entries = Rc::new(entries);
        let now_ms = history::now_ms();
        let reopen = Rc::new(reopen);
        let closed = Rc::new(closed);
        let first_open = Cell::new(true);
        let (shown_input, shown) = (input.clone(), entries.clone());
        window.open_dialog(cx, move |dialog, window, cx| {
            if first_open.replace(false) {
                shown_input.focus_handle(cx).focus(window, cx);
            }
            let typed = shown_input.read(cx).value().to_string();
            let matches: Vec<Entry> = history::search(&shown, &typed)
                .into_iter()
                .take(SHOWN)
                .cloned()
                .collect();
            let muted = cx.theme().muted_foreground;
            let rows = matches.iter().enumerate().map(|(ix, entry)| {
                let (reopen, chosen) = (reopen.clone(), entry.clone());
                div()
                    .id(SharedString::from(format!("history-{ix}")))
                    .px_2()
                    .cursor_pointer()
                    .truncate()
                    .child(entry.summary(now_ms))
                    .on_click(move |_, window, cx| {
                        window.close_dialog(cx);
                        reopen(chosen.clone(), window, cx);
                    })
            });
            let first = matches.first().cloned();
            let reopen_first = reopen.clone();
            let closed = closed.clone();
            dialog
                .title("Query history")
                .entrance(DialogEntrance::None)
                .w(px(640.))
                .child(Input::new(&shown_input))
                .child(
                    div()
                        .id("history")
                        .max_h(px(360.))
                        .overflow_y_scroll()
                        .children(rows),
                )
                .child(div().text_sm().text_color(muted).child(match first {
                    Some(_) => "Enter opens the first run in a new tab.",
                    None => "No runs match.",
                }))
                .on_ok(move |_, window, cx| {
                    // Closed now, not by the dialog after its exit animation, so the focus
                    // it hands back lands before the reopened tab takes it.
                    window.close_dialog(cx);
                    if let Some(entry) = first.clone() {
                        reopen_first(entry, window, cx);
                    }
                    true
                })
                .on_close(move |_, window, cx| closed(window, cx))
        });
        Self {
            input,
            entries,
            now_ms,
        }
    }

    /// What the panel shows now.
    pub fn state(&self, cx: &gpui::App) -> HistoryState {
        let typed = self.input.read(cx).value().to_string();
        HistoryState {
            matches: history::search(&self.entries, &typed)
                .into_iter()
                .take(SHOWN)
                .map(|entry| entry.summary(self.now_ms))
                .collect(),
            typed,
        }
    }
}
