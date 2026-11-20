//! The query tab's two dialogs: the danger check's confirmation before a run, and the
//! database picker (Ctrl+U). Both give the editor focus back as they close.

use std::cell::Cell;
use std::rc::Rc;

use dbide_engine::danger::{DangerCheck, DangerKind};
use futures::StreamExt as _;
use futures::channel::{mpsc, oneshot};
use gpui::{
    App, AppContext as _, Context, Entity, Focusable as _, InteractiveElement as _,
    ParentElement as _, SharedString, StatefulInteractiveElement as _, Styled as _, Window, div,
    px,
};
use gpui_component::dialog::DialogEntrance;
use gpui_component::input::{Input, InputState};
use gpui_component::{ActiveTheme as _, WindowExt as _};
use serde::Serialize;

use super::{QueryTab, Status};

/// The open dialog, kept for the UI script's state lines.
pub(super) enum OpenDialog {
    Danger {
        state: DangerState,
        /// Held while the tab estimates the statements' rows; closing the dialog drops it,
        /// which cancels the estimating and frees the session.
        _stop_estimating: Option<oneshot::Sender<()>>,
    },
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
    /// For an UPDATE or DELETE, the rows it would change; `None` for other kinds.
    pub rows: Option<Rows>,
}

/// What the danger dialog knows of the rows an UPDATE or DELETE would change, from the
/// optimizer's estimate (docs/spec.md, "Danger check").
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Rows {
    /// The tab is asking the server.
    Estimating,
    /// No estimate: the tab isn't connected, or the server gave none.
    Unknown,
    /// The optimizer's estimate, rounded to whole rows.
    About(u64),
}

impl Rows {
    fn text(self) -> String {
        match self {
            Rows::Estimating => "estimating rows…".into(),
            Rows::Unknown => "rows unknown".into(),
            Rows::About(1) => "about 1 row".into(),
            Rows::About(rows) => format!("about {rows} rows"),
        }
    }
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
            OpenDialog::Danger { state, .. } => DialogState::Danger(state.clone()),
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

impl DangerState {
    /// The dialog's text: one line per flagged statement, then the partial-check warning.
    fn description(&self) -> SharedString {
        let mut lines: Vec<String> = self
            .statements
            .iter()
            .map(|s| match s.rows {
                Some(rows) => format!("Line {}: {} ({})", s.line, s.reason, rows.text()),
                None => format!("Line {}: {}", s.line, s.reason),
            })
            .collect();
        if !self.fully_checked {
            lines.push(
                "The batch couldn't be fully checked, so other statements may also be dangerous."
                    .into(),
            );
        }
        lines.join("\n").into()
    }
}

/// Lists the batch's dangerous statements and runs it only if the user says so. On a free
/// session, the tab asks the server meanwhile how many rows each UPDATE or DELETE would
/// change, and the dialog fills them in as they arrive; "Run anyway" pressed before then
/// runs the batch once the session is back.
pub(super) fn ask_before_running(
    tab: &mut QueryTab,
    batch: String,
    first_line: u32,
    check: DangerCheck,
    window: &mut Window,
    cx: &mut Context<QueryTab>,
) {
    let estimating = tab.status == Status::Ready && tab.session.is_some();
    let state = DangerState {
        statements: check
            .statements
            .iter()
            .map(|s| Flagged {
                line: first_line + s.line as u32,
                reason: s.reason,
                rows: matches!(s.kind, DangerKind::Update | DangerKind::Delete).then_some(
                    if estimating {
                        Rows::Estimating
                    } else {
                        Rows::Unknown
                    },
                ),
            })
            .collect(),
        fully_checked: check.fully_checked,
    };
    let estimating = estimating && state.statements.iter().any(|s| s.rows.is_some());
    let run_when_free = Rc::new(Cell::new(false));
    let stop_estimating = if estimating {
        estimate(
            tab,
            &batch,
            first_line,
            check,
            run_when_free.clone(),
            window,
            cx,
        )
    } else {
        None
    };
    tab.dialog = Some(OpenDialog::Danger {
        state,
        _stop_estimating: stop_estimating,
    });
    let entity = cx.entity();
    window.open_alert_dialog(cx, move |alert, _, cx| {
        let run = {
            let (entity, batch) = (entity.clone(), batch.clone());
            let run_when_free = run_when_free.clone();
            move |_: &gpui::ClickEvent, window: &mut Window, cx: &mut App| {
                entity.update(cx, |tab, cx| {
                    // Closing the dialog cancels any estimating still under way.
                    tab.dialog = None;
                    tab.focus(window, cx);
                    if tab.status == Status::Estimating {
                        run_when_free.set(true);
                    } else {
                        tab.go(batch.clone(), first_line, window, cx);
                    }
                });
                true
            }
        };
        // Rebuilt on every draw, so estimates show as they arrive.
        let description = match &entity.read(cx).dialog {
            Some(OpenDialog::Danger { state, .. }) => state.description(),
            _ => SharedString::default(),
        };
        alert
            .title("Run dangerous statements?")
            .entrance(DialogEntrance::None)
            .description(description)
            .ok_text("Run anyway")
            .show_cancel(true)
            .on_ok(run)
            .on_close(closer(&entity))
    });
    cx.notify();
}

/// Takes the tab's session to estimate the flagged statements' rows, fills each estimate
/// into the dialog as it arrives while the dialog is open, then gives the session back and
/// runs the batch if `run_when_free` was set meanwhile. Returns the sender whose drop stops
/// the estimating; `None`, with nothing started, when the tab has no free session.
fn estimate(
    tab: &mut QueryTab,
    batch: &str,
    first_line: u32,
    check: DangerCheck,
    run_when_free: Rc<Cell<bool>>,
    window: &mut Window,
    cx: &mut Context<QueryTab>,
) -> Option<oneshot::Sender<()>> {
    let engine = tab.target.as_ref()?.saved.connection.engine;
    let session = tab.session.take()?;
    tab.status = Status::Estimating;
    let (stop, stopped) = oneshot::channel();
    let (found, mut arriving) = mpsc::unbounded();
    let estimating =
        tab.runtime
            .estimate_rows(session, batch.to_owned(), check, engine, found, stopped);
    let batch = batch.to_owned();
    cx.spawn_in(window, async move |this, cx| {
        // Ends when the estimating finishes or is stopped, which closes `found`.
        while let Some((index, estimate)) = arriving.next().await {
            let _ = this.update(cx, |this, cx| {
                if let Some(OpenDialog::Danger { state, .. }) = &mut this.dialog
                    && let Some(flagged) = state.statements.get_mut(index)
                    && flagged.rows.is_some()
                {
                    flagged.rows = Some(match estimate {
                        Some(rows) if rows.is_finite() => Rows::About(rows.max(0.0).round() as u64),
                        _ => Rows::Unknown,
                    });
                    cx.notify();
                }
            });
        }
        let session = estimating.await;
        let _ = this.update_in(cx, |this, window, cx| {
            this.give_back(session);
            if run_when_free.get() {
                this.go(batch, first_line, window, cx);
            }
            cx.notify();
        });
    })
    .detach();
    Some(stop)
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
