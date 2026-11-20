//! The query history panel (Ctrl+Shift+H, View, Query history): a search box over past runs,
//! newest first. Up and Down choose a run, and Enter or a click reopens its text in a new tab
//! on its connection.

use std::cell::{Cell, RefCell};
use std::rc::Rc;

use gpui::{
    AppContext as _, Entity, Focusable as _, InteractiveElement as _, KeyDownEvent,
    ParentElement as _, SharedString, StatefulInteractiveElement as _, Styled as _, Window, div,
    prelude::FluentBuilder as _, px,
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
    /// Index into the matches of the chosen row.
    chosen: Rc<Cell<usize>>,
}

/// What a user can see of the panel.
#[derive(Debug, Serialize)]
pub struct HistoryState {
    pub typed: String,
    /// The matching runs' summaries, newest first.
    pub matches: Vec<String>,
    /// Index into `matches` of the chosen row, which Enter reopens.
    pub chosen: usize,
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
        let chosen = Rc::new(Cell::new(0));
        let reopen = Rc::new(reopen);
        let closed = Rc::new(closed);
        let first_open = Cell::new(true);
        let last_typed = RefCell::new(String::new());
        let (shown_input, shown, shown_chosen) = (input.clone(), entries.clone(), chosen.clone());
        window.open_dialog(cx, move |dialog, window, cx| {
            if first_open.replace(false) {
                shown_input.focus_handle(cx).focus(window, cx);
            }
            let typed = shown_input.read(cx).value().to_string();
            if *last_typed.borrow() != typed {
                shown_chosen.set(0);
                last_typed.replace(typed.clone());
            }
            let matches: Rc<Vec<Entry>> = Rc::new(
                history::search(&shown, &typed)
                    .into_iter()
                    .take(SHOWN)
                    .cloned()
                    .collect(),
            );
            let current = shown_chosen.get().min(matches.len().saturating_sub(1));
            let theme = cx.theme();
            let (muted, accent) = (theme.muted_foreground, theme.accent);
            let rows = matches.iter().enumerate().map(|(ix, entry)| {
                let (reopen, chosen) = (reopen.clone(), entry.clone());
                div()
                    .id(SharedString::from(format!("history-{ix}")))
                    .px_2()
                    .cursor_pointer()
                    .truncate()
                    .when(ix == current, |row| row.bg(accent))
                    .child(entry.summary(now_ms))
                    .on_click(move |_, window, cx| {
                        window.close_dialog(cx);
                        reopen(chosen.clone(), window, cx);
                    })
            });
            let step = {
                let (chosen, count) = (shown_chosen.clone(), matches.len());
                move |event: &KeyDownEvent, window: &mut Window, cx: &mut gpui::App| {
                    let next = match event.keystroke.key.as_str() {
                        "down" => (current + 1).min(count.saturating_sub(1)),
                        "up" => current.saturating_sub(1),
                        _ => return,
                    };
                    chosen.set(next);
                    cx.stop_propagation();
                    window.refresh();
                }
            };
            let (picked, reopen_picked) = (matches.get(current).cloned(), reopen.clone());
            let closed = closed.clone();
            dialog
                .title("Query history")
                .entrance(DialogEntrance::None)
                .w(px(640.))
                .child(
                    div()
                        .capture_key_down(step)
                        .child(Input::new(&shown_input))
                        .child(
                            div()
                                .id("history")
                                .max_h(px(360.))
                                .overflow_y_scroll()
                                .children(rows),
                        ),
                )
                .child(div().text_sm().text_color(muted).child(match picked {
                    Some(_) => "Up and Down choose a run; Enter opens it in a new tab.",
                    None => "No runs match.",
                }))
                .on_ok(move |_, window, cx| {
                    // Closed now, not by the dialog after its exit animation, so the focus
                    // it hands back lands before the reopened tab takes it.
                    window.close_dialog(cx);
                    if let Some(entry) = picked.clone() {
                        reopen_picked(entry, window, cx);
                    }
                    true
                })
                .on_close(move |_, window, cx| closed(window, cx))
        });
        Self {
            input,
            entries,
            now_ms,
            chosen,
        }
    }

    /// What the panel shows now.
    pub fn state(&self, cx: &gpui::App) -> HistoryState {
        let typed = self.input.read(cx).value().to_string();
        let matches: Vec<String> = history::search(&self.entries, &typed)
            .into_iter()
            .take(SHOWN)
            .map(|entry| entry.summary(self.now_ms))
            .collect();
        HistoryState {
            chosen: self.chosen.get().min(matches.len().saturating_sub(1)),
            matches,
            typed,
        }
    }
}
