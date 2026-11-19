//! The query tab's two dialogs: the danger check's confirmation before a run, and the
//! database picker (Ctrl+U). Both give the editor focus back as they close.

use std::cell::Cell;

use dbide_engine::danger::DangerCheck;
use gpui::{
    App, AppContext as _, Context, Entity, Focusable as _, InteractiveElement as _,
    ParentElement as _, SharedString, StatefulInteractiveElement as _, Styled as _, Window, div,
    px,
};
use gpui_component::dialog::DialogEntrance;
use gpui_component::input::{Input, InputState};
use gpui_component::{ActiveTheme as _, WindowExt as _};
use serde::Serialize;

use super::QueryTab;

/// The open dialog, kept for the UI script's state lines.
pub(super) enum OpenDialog {
    Danger(DangerState),
    Picker {
        input: Entity<InputState>,
        databases: Vec<String>,
    },
}

/// One flagged statement as the dialog lists it.
#[derive(Clone, Debug, Serialize)]
pub struct Flagged {
    /// The editor line the statement starts on, counted from one.
    pub line: u32,
    pub reason: &'static str,
}

#[derive(Clone, Debug, Serialize)]
pub struct DangerState {
    pub statements: Vec<Flagged>,
    pub fully_checked: bool,
}

/// What a user can see of the tab's open dialog.
#[derive(Debug, Serialize)]
#[serde(tag = "dialog", rename_all = "snake_case")]
pub enum DialogState {
    Danger(DangerState),
    Picker { typed: String, matches: Vec<String> },
}

impl OpenDialog {
    pub(super) fn state(&self, cx: &App) -> DialogState {
        match self {
            OpenDialog::Danger(state) => DialogState::Danger(state.clone()),
            OpenDialog::Picker { input, databases } => {
                let typed = input.read(cx).value().to_string();
                DialogState::Picker {
                    matches: matches(databases, &typed),
                    typed,
                }
            }
        }
    }
}

/// The databases whose names contain `typed`, ignoring case, in list order.
fn matches(databases: &[String], typed: &str) -> Vec<String> {
    let typed = typed.trim().to_lowercase();
    databases
        .iter()
        .filter(|name| name.to_lowercase().contains(&typed))
        .cloned()
        .collect()
}

/// The database Enter picks: the exact name, else an exact match ignoring case, else the
/// first name containing the typed text, else the typed name itself, which the server may
/// still accept (system databases aren't listed). `None` when nothing is typed. PostgreSQL
/// names are case-sensitive, so `orders` must pick `orders` even when `Orders` comes first.
fn pick(databases: &[String], typed: &str) -> Option<String> {
    let typed = typed.trim();
    if typed.is_empty() {
        return None;
    }
    let exact = databases
        .iter()
        .find(|name| *name == typed)
        .or_else(|| {
            databases
                .iter()
                .find(|name| name.eq_ignore_ascii_case(typed))
        })
        .cloned();
    exact
        .or_else(|| matches(databases, typed).into_iter().next())
        .or_else(|| Some(typed.to_owned()))
}

/// A closer that clears the tab's dialog and gives its editor focus.
fn closer(tab: &Entity<QueryTab>) -> impl Fn(&gpui::ClickEvent, &mut Window, &mut App) + 'static {
    let tab = tab.clone();
    move |_, window, cx| {
        tab.update(cx, |tab, cx| {
            tab.dialog = None;
            tab.focus(window, cx);
        })
    }
}

/// Lists the batch's dangerous statements and runs it only if the user says so.
pub(super) fn ask_before_running(
    tab: &mut QueryTab,
    batch: String,
    first_line: u32,
    check: DangerCheck,
    window: &mut Window,
    cx: &mut Context<QueryTab>,
) {
    let state = DangerState {
        statements: check
            .statements
            .iter()
            .map(|s| Flagged {
                line: first_line + s.line as u32,
                reason: s.reason,
            })
            .collect(),
        fully_checked: check.fully_checked,
    };
    let mut description: Vec<String> = state
        .statements
        .iter()
        .map(|s| format!("Line {}: {}", s.line, s.reason))
        .collect();
    if !state.fully_checked {
        description.push(
            "The batch couldn't be fully checked, so other statements may also be dangerous."
                .into(),
        );
    }
    let description: SharedString = description.join("\n").into();
    tab.dialog = Some(OpenDialog::Danger(state));
    let entity = cx.entity();
    window.open_alert_dialog(cx, move |alert, _, _| {
        let run = {
            let (entity, batch) = (entity.clone(), batch.clone());
            move |_: &gpui::ClickEvent, window: &mut Window, cx: &mut App| {
                entity.update(cx, |tab, cx| {
                    tab.dialog = None;
                    tab.focus(window, cx);
                    let job = tab.run_job();
                    tab.start_run(batch.clone(), first_line, job, window, cx);
                });
                true
            }
        };
        alert
            .title("Run dangerous statements?")
            .entrance(DialogEntrance::None)
            .description(description.clone())
            .ok_text("Run anyway")
            .show_cancel(true)
            .on_ok(run)
            .on_close(closer(&entity))
    });
    cx.notify();
}

/// Opens the picker over `databases`: type to filter, Enter switches, a click on a name
/// switches to it.
pub(super) fn pick_database(
    tab: &mut QueryTab,
    databases: Vec<String>,
    window: &mut Window,
    cx: &mut Context<QueryTab>,
) {
    let input = cx.new(|cx| InputState::new(window, cx));
    tab.dialog = Some(OpenDialog::Picker {
        input: input.clone(),
        databases: databases.clone(),
    });
    let entity = cx.entity();
    let current = tab.database.clone().unwrap_or_default();
    let first_open = Cell::new(true);
    window.open_dialog(cx, move |dialog, window, cx| {
        if first_open.replace(false) {
            input.focus_handle(cx).focus(window, cx);
        }
        let typed = input.read(cx).value().to_string();
        let muted = cx.theme().muted_foreground;
        let rows = matches(&databases, &typed).into_iter().map(|name| {
            let entity = entity.clone();
            let chosen = name.clone();
            div()
                .id(SharedString::from(format!("database-{name}")))
                .px_2()
                .cursor_pointer()
                .child(name)
                .on_click(move |_, window, cx| {
                    let chosen = chosen.clone();
                    entity.update(cx, |tab, cx| {
                        tab.dialog = None;
                        tab.focus(window, cx);
                        tab.switch_database(chosen, window, cx);
                    });
                    window.close_dialog(cx);
                })
        });
        let switch = {
            let (entity, input, databases) = (entity.clone(), input.clone(), databases.clone());
            move |_: &gpui::ClickEvent, window: &mut Window, cx: &mut App| {
                let typed = input.read(cx).value().to_string();
                let Some(database) = pick(&databases, &typed) else {
                    return false;
                };
                entity.update(cx, |tab, cx| {
                    tab.dialog = None;
                    tab.focus(window, cx);
                    tab.switch_database(database, window, cx);
                });
                true
            }
        };
        dialog
            .title(format!("Database (now {current})"))
            .entrance(DialogEntrance::None)
            .w(px(420.))
            .child(Input::new(&input))
            .child(
                div()
                    .id("databases")
                    .max_h(px(240.))
                    .overflow_y_scroll()
                    .children(rows),
            )
            .child(
                div()
                    .text_xs()
                    .text_color(muted)
                    .child("Type to filter; Enter switches to the first match or the typed name."),
            )
            .footer(crate::explorer::footer("Switch (Enter)"))
            .on_ok(switch)
            .on_close(closer(&entity))
    });
    cx.notify();
}

#[cfg(test)]
mod tests {
    use super::pick;

    fn names() -> Vec<String> {
        ["orders", "Orders_Archive", "sales"]
            .map(String::from)
            .to_vec()
    }

    #[test]
    fn enter_picks_an_exact_name_before_a_partial_one() {
        assert_eq!(pick(&names(), "ORDERS"), Some("orders".into()));
    }

    #[test]
    fn enter_picks_the_name_typed_exactly_before_one_differing_in_case() {
        let names = ["Orders", "orders"].map(String::from);
        assert_eq!(pick(&names, "orders"), Some("orders".into()));
    }

    #[test]
    fn enter_picks_the_first_name_containing_the_typed_text() {
        assert_eq!(pick(&names(), "arch"), Some("Orders_Archive".into()));
    }

    #[test]
    fn enter_switches_to_an_unlisted_typed_name() {
        assert_eq!(pick(&names(), " master "), Some("master".into()));
    }

    #[test]
    fn enter_with_nothing_typed_picks_nothing() {
        assert_eq!(pick(&names(), "  "), None);
    }
}
