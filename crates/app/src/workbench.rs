//! The main window's root view: layout C "Workbench" from the spec. Query tabs and the
//! object explorer on the left, the active tab's editor with its messages strip and the
//! results in the middle, the inspector on the right.
//!
//! U1 built the frame: the panes, moving focus between them, and every key in the keymap.
//! U2 put the saved connections in the explorer (`explorer.rs`); U3 the query tabs
//! (`query_tab.rs`) and the messages strip (`messages.rs`); U7 the plans (`results/plan.rs`).
//! Actions whose feature is not built yet say so in the messages strip. U10 kept the tabs,
//! files and history (`workbench/files.rs`).

mod files;

use std::path::PathBuf;

use gpui::{
    App, AppContext as _, Context, Entity, FocusHandle, Focusable as _, IntoElement, KeyDownEvent,
    ParentElement as _, Render, ScrollHandle, SharedString, Styled as _, Subscription, WeakEntity,
    Window, div, prelude::*, px,
};
use gpui_component::button::Button;
use gpui_component::input::{Editor, InputEvent};
use gpui_component::{ActiveTheme as _, Disableable as _};
use serde::Serialize;

use crate::explorer::{Explorer, ExplorerEvent, ExplorerState};
use crate::history_panel::{HistoryPanel, HistoryState};
use crate::keymap::{
    CancelRun, CommentLines, EstimatedPlan, FocusExplorer, Format, GoToError, NewConnection,
    NewQueryTab, NextError, NextPane, NextQueryTab, OpenEditMenu, OpenFile, OpenFileMenu,
    OpenHelpMenu, OpenQueryMenu, OpenViewMenu, PickDatabase, PrevPane, PreviousError, Quit,
    RefreshAutocomplete, Run, SaveFile, ShowAbout, ShowHistory, ShowShortcuts, ToggleActualPlan,
    TogglePlanView, ToggleResults, ToggleResultsToFile, UncommentLines,
};
use crate::menu_bar::{Menu, MenuBar, MenuBarState, Situation};
use crate::messages::Tone;
use crate::palette;
use crate::query_tab::dialogs::DialogState;
use crate::query_tab::file::QueryTabEvent;
use crate::query_tab::{QueryTab, QueryTarget, TabState};
use crate::results::plan::PlanState;
use crate::results::{GridState, Inspected};
use crate::runtime::Runtime;

/// How many of the newest messages the strip renders; it shows as many as fit.
const MESSAGES_SHOWN: usize = 50;

/// A focusable region of the workbench, in F6 order.
#[derive(Clone, Copy, PartialEq, Eq, Debug, Serialize)]
pub enum Pane {
    QueryTabs,
    Explorer,
    Editor,
    Messages,
    Results,
    Inspector,
}

impl Pane {
    const ALL: [Pane; 6] = [
        Pane::QueryTabs,
        Pane::Explorer,
        Pane::Editor,
        Pane::Messages,
        Pane::Results,
        Pane::Inspector,
    ];

    fn title(self) -> &'static str {
        match self {
            Pane::QueryTabs => "Query tabs",
            Pane::Explorer => "Object explorer",
            Pane::Editor => "Editor",
            Pane::Messages => "Messages",
            Pane::Results => "Results",
            Pane::Inspector => "Inspector",
        }
    }
}

/// What a user can see of the workbench, recorded after each `--ui-script` step.
#[derive(Debug, Serialize)]
pub struct VisibleState {
    /// The pane holding keyboard focus, if any.
    pub focused_pane: Option<Pane>,
    /// The last workbench action a key or click triggered, by its action name.
    pub last_action: Option<&'static str>,
    pub results_visible: bool,
    /// The messages strip's lines, oldest first.
    pub messages: Vec<SharedString>,
    /// The line of `messages` the keyboard has chosen, if any.
    pub chosen_message: Option<usize>,
    /// The active tab's editor text.
    pub editor_text: String,
    pub tabs: Vec<TabState>,
    /// Index into `tabs` of the tab the editor shows.
    pub active_tab: usize,
    /// The active tab's open dialog.
    pub tab_dialog: Option<DialogState>,
    pub explorer: ExplorerState,
    /// The active tab's result grids, top to bottom.
    pub results: Vec<GridState>,
    /// The active tab's plans, below its grids, one per statement.
    pub plans: Vec<PlanState>,
    /// What the inspector shows: the selected cell in full, or the selected plan operator.
    pub inspector: Option<Inspected>,
    /// How far down the inspector's value is scrolled, in pixels.
    pub inspector_scrolled: f32,
    /// The text on the system clipboard, so a script can check a copy.
    pub clipboard: Option<String>,
    pub menu_bar: MenuBarState,
    /// The query history panel, while it is open.
    pub history: Option<HistoryState>,
}

/// Work to run when the window first draws.
type FirstFrameHook = Box<dyn FnOnce(&mut Window, &mut Context<Workbench>)>;

pub struct Workbench {
    /// Never empty: the app starts with one tab and tabs don't close yet.
    tabs: Vec<Entity<QueryTab>>,
    active: usize,
    /// The number the next tab's title gets.
    next_tab_number: u32,
    runtime: Runtime,
    explorer: Entity<Explorer>,
    menu_bar: Entity<MenuBar>,
    /// Focus handles for the panes that don't own one; the editor and explorer do.
    pane_focus: [FocusHandle; 6],
    results_visible: bool,
    /// The inspector value's scroll position.
    inspector_scroll: ScrollHandle,
    last_action: Option<&'static str>,
    /// True while Ctrl+O asks for a file or reads it, or the history loads.
    opening: bool,
    /// Called once, from the first `render`.
    on_first_frame: Option<FirstFrameHook>,
    /// Where the open tabs and the query history are kept; `None` without an app-data folder.
    tabs_file: Option<PathBuf>,
    history_file: Option<PathBuf>,
    /// Restored tabs waiting for the explorer to sign in to their connection.
    waking: Vec<WeakEntity<QueryTab>>,
    /// Bumped by every change; a pending save runs only if no later change came.
    save_generation: u64,
    /// The query history panel, while it is open.
    history: Option<HistoryPanel>,
    /// Keeps the open tabs when the app quits.
    _save_on_quit: Subscription,
}

impl Workbench {
    /// Builds the workbench with the tabs kept from the last session, or one unconnected
    /// tab, the first one's editor focused.
    /// `on_first_frame` runs once, when the window first draws: the startup measurement and
    /// the UI script hook in there.
    pub fn new(
        explorer: Entity<Explorer>,
        runtime: Runtime,
        on_first_frame: impl FnOnce(&mut Window, &mut Context<Self>) + 'static,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        let pane_focus = std::array::from_fn(|_| cx.focus_handle());
        cx.subscribe_in(
            &explorer,
            window,
            |this: &mut Self, _, event: &ExplorerEvent, window, cx| match event {
                ExplorerEvent::Message(text) => this.note(text.clone(), cx),
                ExplorerEvent::OpenQuery(target) => {
                    this.open_tab(Some(target.as_ref().clone()), window, cx);
                }
                ExplorerEvent::OpenScript { target, text } => {
                    this.open_tab(Some(target.as_ref().clone()), window, cx);
                    let editor = this.active_tab().read(cx).editor().clone();
                    editor.update(cx, |editor, cx| editor.set_value(text.clone(), window, cx));
                }
                ExplorerEvent::Resume(target) => this.resume_waking(target, window, cx),
            },
        )
        .detach();
        let mut this = Self {
            tabs: Vec::new(),
            active: 0,
            next_tab_number: 1,
            runtime,
            explorer,
            menu_bar: cx.new(MenuBar::new),
            pane_focus,
            results_visible: true,
            inspector_scroll: ScrollHandle::new(),
            last_action: None,
            opening: false,
            on_first_frame: Some(Box::new(on_first_frame)),
            tabs_file: crate::open_tabs::path(),
            history_file: crate::history::path(),
            waking: Vec::new(),
            save_generation: 0,
            history: None,
            _save_on_quit: cx.on_app_quit(|this, cx| {
                this.save_tabs(cx);
                async {}
            }),
        };
        this.restore_tabs(window, cx);
        // Repaint the pane outlines whenever focus moves.
        for pane in Pane::ALL {
            if pane == Pane::Editor {
                continue;
            }
            let handle = this.focus_handle_of(pane, cx);
            cx.on_focus_in(&handle, window, |_, _, cx| cx.notify())
                .detach();
        }
        this
    }

    fn active_tab(&self) -> &Entity<QueryTab> {
        &self.tabs[self.active]
    }

    /// The title the next new tab gets.
    fn next_title(&mut self) -> SharedString {
        self.next_tab_number += 1;
        format!("Query {}", self.next_tab_number - 1).into()
    }

    /// Opens a tab on `target` (or unconnected), makes it active and focuses its editor.
    fn open_tab(
        &mut self,
        target: Option<QueryTarget>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let (title, runtime) = (self.next_title(), self.runtime.clone());
        let tab = cx.new(|cx| QueryTab::new(title, target, runtime, window, cx));
        self.add_tab(tab, window, cx);
        self.schedule_save(cx);
    }

    /// Adds `tab` to the list, makes it active and focuses its editor.
    fn add_tab(&mut self, tab: Entity<QueryTab>, window: &mut Window, cx: &mut Context<Self>) {
        // The tab list, the strip and the Stop button show the tab's state.
        cx.observe(&tab, |_, _, cx| cx.notify()).detach();
        cx.subscribe_in(
            &tab,
            window,
            |this, tab, event: &QueryTabEvent, window, cx| {
                this.on_tab_event(tab, event, window, cx);
            },
        )
        .detach();
        // Typing changes the tab's dot and is kept a second after it stops.
        let editor = tab.read(cx).editor().clone();
        cx.subscribe(&editor, |this, _, event: &InputEvent, cx| {
            if let InputEvent::Change = event {
                this.schedule_save(cx);
                cx.notify();
            }
        })
        .detach();
        // The inspector shows the results' selected cell.
        let results = tab.read(cx).results().clone();
        cx.observe(&results, |_, _, cx| cx.notify()).detach();
        cx.on_focus_in(
            &tab.read(cx).editor().focus_handle(cx),
            window,
            |_, _, cx| cx.notify(),
        )
        .detach();
        self.tabs.push(tab);
        self.show_tab(self.tabs.len() - 1, window, cx);
    }

    /// Makes tab `ix` active and gives its editor focus.
    fn show_tab(&mut self, ix: usize, window: &mut Window, cx: &mut Context<Self>) {
        if let Some(leaving) = self.tabs.get(self.active).filter(|_| self.active != ix) {
            leaving.update(cx, |tab, _| tab.set_shown(false));
        }
        self.tabs[ix].update(cx, |tab, _| tab.set_shown(true));
        self.active = ix;
        self.tabs[ix].update(cx, |tab, cx| tab.focus(window, cx));
        cx.notify();
    }

    /// Ctrl+N: a tab on the connection selected in the explorer when the explorer has
    /// focus, otherwise on the active tab's connection, otherwise unconnected.
    fn new_query_tab(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.focused_pane(window, cx) == Some(Pane::Explorer)
            && self
                .explorer
                .update(cx, |explorer, cx| explorer.open_query(window, cx))
        {
            // The explorer signs in, then asks for the tab with `ExplorerEvent::OpenQuery`.
            return;
        }
        self.open_beside_active(window, cx);
    }

    /// Puts one of the app's notices in the strip of the tab on screen.
    fn note(&mut self, text: impl Into<SharedString>, cx: &mut Context<Self>) {
        let text = text.into();
        self.active_tab()
            .clone()
            .update(cx, |tab, cx| tab.note(text, cx));
    }

    fn focus_handle_of(&self, pane: Pane, cx: &App) -> FocusHandle {
        match pane {
            Pane::Editor => self.active_tab().read(cx).editor().focus_handle(cx),
            Pane::Explorer => self.explorer.focus_handle(cx),
            Pane::Results => self.active_tab().read(cx).results().focus_handle(cx),
            other => self.pane_focus[other as usize].clone(),
        }
    }

    fn focused_pane(&self, window: &Window, cx: &App) -> Option<Pane> {
        Pane::ALL
            .into_iter()
            .find(|p| self.focus_handle_of(*p, cx).contains_focused(window, cx))
    }

    /// The panes F6 visits: every pane that is on screen.
    fn visible_panes(&self) -> Vec<Pane> {
        Pane::ALL
            .into_iter()
            .filter(|p| *p != Pane::Results || self.results_visible)
            .collect()
    }

    fn focus_pane(&mut self, pane: Pane, window: &mut Window, cx: &mut Context<Self>) {
        match pane {
            Pane::Explorer => self
                .explorer
                .update(cx, |explorer, cx| explorer.focus(window, cx)),
            Pane::Editor => self
                .active_tab()
                .update(cx, |tab, cx| tab.focus(window, cx)),
            Pane::Results => {
                let results = self.active_tab().read(cx).results().clone();
                results.update(cx, |results, cx| results.focus(window, cx));
            }
            other => window.focus(&self.pane_focus[other as usize], cx),
        }
        cx.notify();
    }

    /// Moves focus `step` panes along the F6 order, wrapping at either end. With no pane
    /// focused, F6 starts from the first pane and Shift+F6 from the last.
    fn cycle(&mut self, step: isize, window: &mut Window, cx: &mut Context<Self>) {
        let panes = self.visible_panes();
        let count = panes.len() as isize;
        let current = self
            .focused_pane(window, cx)
            .and_then(|p| panes.iter().position(|q| *q == p))
            .map_or(if step > 0 { -1 } else { count }, |ix| ix as isize);
        let next = panes[(current + step).rem_euclid(count) as usize];
        self.focus_pane(next, window, cx);
    }

    fn toggle_results(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.results_visible = !self.results_visible;
        if !self.results_visible && self.focused_pane(window, cx) == Some(Pane::Results) {
            self.focus_pane(Pane::Editor, window, cx);
        }
        cx.notify();
    }

    /// Records an action whose feature a later ticket builds.
    fn not_built(&mut self, name: &'static str, cx: &mut Context<Self>) {
        self.last_action = Some(name);
        self.note(format!("{name} is not built yet."), cx);
    }

    /// Ctrl+O: asks for a file and opens a `.sql` file in a new tab, or a `.sqlplan` in the
    /// active tab's results in place of its last run's.
    fn open_file(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let asked = crate::file_prompt::open_path(cx);
        let tab = self.active_tab().clone();
        self.opening = true;
        cx.spawn_in(window, async move |this, cx| {
            let path = asked.await;
            // The tab says what came of it before the workbench reads as idle again.
            let finish = |this: &mut Self| this.opening = false;
            let Some(path) = path else {
                let _ = this.update(cx, |this, _| finish(this));
                return;
            };
            let is_sql = path
                .extension()
                .is_some_and(|e| e.eq_ignore_ascii_case("sql"));
            if is_sql {
                let _ = this.update_in(cx, |this, window, cx| {
                    finish(this);
                    this.open_sql_file(path, window, cx);
                });
                return;
            }
            let shown = path.display().to_string();
            let is_plan = path
                .extension()
                .is_some_and(|e| e.eq_ignore_ascii_case("sqlplan"));
            if !is_plan {
                let _ = this.update(cx, |this, cx| {
                    finish(this);
                    this.note(
                        format!("Couldn't open {shown}: only .sql and .sqlplan files open."),
                        cx,
                    );
                });
                return;
            }
            let opened = cx
                .background_spawn(async move { dbide_engine::plan::showplan::open(&path) })
                .await;
            tab.update(cx, |tab, cx| match opened {
                Ok(plan) => {
                    if tab.show_plan(plan, cx) {
                        tab.note(format!("Opened {shown}."), cx);
                    }
                }
                Err(error) => tab.note(format!("Couldn't open {shown}: {error}"), cx),
            });
            let _ = this.update(cx, |this, _| finish(this));
        })
        .detach();
    }

    /// True while a connect, a connection test, or a tab's run, switch or list is under way.
    pub fn is_busy(&self, cx: &App) -> bool {
        self.opening
            || self.explorer.read(cx).is_busy(cx)
            || self.tabs.iter().any(|t| t.read(cx).is_busy(cx))
    }

    /// True once the active tab's run has started on the server.
    pub fn is_running(&self, cx: &App) -> bool {
        self.active_tab().read(cx).has_started_run()
    }

    pub fn visible_state(&self, window: &Window, cx: &App) -> VisibleState {
        let tab = self.active_tab().read(cx);
        VisibleState {
            focused_pane: self.focused_pane(window, cx),
            last_action: self.last_action,
            results_visible: self.results_visible,
            messages: tab
                .messages()
                .lines()
                .iter()
                .map(|m| m.text.clone())
                .collect(),
            chosen_message: tab.messages().chosen(),
            editor_text: tab.editor().read(cx).value().to_string(),
            tabs: self.tabs.iter().map(|t| t.read(cx).state(cx)).collect(),
            active_tab: self.active,
            tab_dialog: tab.dialog_state(cx),
            explorer: self.explorer.read(cx).state(cx),
            results: tab.results().read(cx).state(cx),
            plans: tab.results().read(cx).plan_state(),
            inspector: tab.results().read(cx).inspected(cx),
            clipboard: cx.read_from_clipboard().and_then(|item| item.text()),
            inspector_scrolled: f32::from(self.inspector_scroll.offset().y).abs(),
            menu_bar: self.menu_bar.read(cx).state(window, cx),
            history: self.history.as_ref().map(|panel| panel.state(cx)),
        }
    }

    /// What the menus need to know now. The tab's state is always current; the focus is the
    /// one before any menu or Help panel took it.
    fn situation(&self, window: &Window, cx: &App) -> Situation {
        let bar = self.menu_bar.read(cx);
        let asleep = self.active_tab().read(cx).is_asleep();
        let tab = self.active_tab().read(cx).state(cx);
        let ready = tab.status == crate::query_tab::Status::Ready;
        let mut situation = Situation {
            ready,
            runnable: ready || asleep,
            running: tab.status == crate::query_tab::Status::Running,
            actual_plan: tab.actual_plan,
            results_to_file: tab.results_to_file,
            results_visible: self.results_visible,
            plan_graph: self.active_tab().read(cx).results().read(cx).plan_graph(),
            in_results: false,
            edit_target: self.focus_handle_of(Pane::Editor, cx),
            previous: self.focus_handle_of(Pane::Editor, cx),
        };
        if let Some(open) = bar.situation(window, cx) {
            situation.in_results = open.in_results;
            situation.edit_target = open.edit_target.clone();
            situation.previous = open.previous.clone();
            return situation;
        }
        let Some(before) = bar.panel_return(window, cx).cloned().or(window.focused(cx)) else {
            return situation;
        };
        // In the results, the focused grid itself, so its own keys still work once the menu
        // hands focus back; its actions reach the results by bubbling.
        if self
            .focus_handle_of(Pane::Results, cx)
            .contains(&before, window)
        {
            situation.in_results = true;
            situation.edit_target = before.clone();
        }
        situation.previous = before;
        situation
    }

    /// Alt+its letter, F10 or a click: opens `menu`.
    fn open_menu(&mut self, menu: Menu, window: &mut Window, cx: &mut Context<Self>) {
        let situation = self.situation(window, cx);
        self.menu_bar
            .update(cx, |bar, cx| bar.open(menu, situation, window, cx));
    }

    fn pane(&self, pane: Pane, focused: Option<Pane>, cx: &App) -> gpui::Stateful<gpui::Div> {
        let theme = cx.theme();
        let frame = div()
            .id(pane.title())
            .flex()
            .flex_col()
            .border_2()
            .border_color(if focused == Some(pane) {
                theme.primary
            } else {
                theme.border
            })
            .overflow_hidden()
            .child(
                div()
                    .px_2()
                    .text_xs()
                    .text_color(theme.muted_foreground)
                    .child(pane.title()),
            );
        match pane {
            Pane::Editor | Pane::Explorer | Pane::Results => frame,
            other => frame.track_focus(&self.focus_handle_of(other, cx)),
        }
    }

    /// Scrolls the inspector's value for a key pressed while the inspector has focus:
    /// Up and Down by a line, Page Up and Page Down by a screen, Home and End to the ends.
    fn scroll_inspector(&mut self, key: &str, cx: &mut Context<Self>) {
        const LINE: f32 = 20.;
        let scroll = &self.inspector_scroll;
        let page = f32::from(scroll.bounds().size.height).max(LINE);
        let bottom = -f32::from(scroll.max_offset().y);
        let offset = scroll.offset();
        let y = f32::from(offset.y);
        let y = match key {
            "up" => y + LINE,
            "down" => y - LINE,
            "pageup" => y + page,
            "pagedown" => y - page,
            "home" => 0.,
            "end" => bottom,
            _ => return,
        };
        scroll.set_offset(gpui::point(offset.x, px(y.clamp(bottom, 0.))));
        cx.stop_propagation();
        cx.notify();
    }

    /// The query tabs pane: one row per tab, marked with its connection's colour.
    fn tab_list(&self, focused: Option<Pane>, cx: &mut Context<Self>) -> gpui::Stateful<gpui::Div> {
        let theme = cx.theme().clone();
        let rows = self.tabs.iter().enumerate().map(|(ix, tab)| {
            let tab = tab.read(cx);
            let marker = tab.colour().and_then(palette::hsla).unwrap_or(theme.border);
            div()
                .id(("tab", ix))
                .flex()
                .flex_row()
                .gap_1()
                .px_1()
                .when(ix == self.active, |row| row.bg(theme.accent))
                .cursor_pointer()
                .child(div().w(px(4.)).bg(marker))
                .child(div().text_sm().child(tab.label(cx)))
                .on_click(cx.listener(move |this, _, window, cx| this.show_tab(ix, window, cx)))
        });
        self.pane(Pane::QueryTabs, focused, cx).children(rows)
    }

    /// Run and Stop for the active tab, and a run to file's progress. Stop is the way to
    /// cancel from a real keyboard on Windows, where gpui doesn't pass Alt+Break through
    /// (keymap.rs).
    fn run_bar(&self, cx: &mut Context<Self>) -> gpui::Div {
        let tab = self.active_tab().read(cx);
        let running = tab.is_running();
        let progress = tab.file_progress().map(|(path, rows)| {
            let name = path.file_name().unwrap_or_default().to_string_lossy();
            format!("Writing {name}: {rows} rows so far")
        });
        div()
            .flex()
            .flex_row()
            .gap_2()
            .px_2()
            .py_1()
            .child(
                Button::new("run")
                    .label("Run (F5)")
                    .disabled(running)
                    .on_click(cx.listener(|this, _, window, cx| {
                        this.last_action = Some("Run");
                        let tab = this.active_tab().clone();
                        tab.update(cx, |tab, cx| tab.run(window, cx));
                    })),
            )
            .child(
                Button::new("stop")
                    .label("Stop (Alt+Break)")
                    .disabled(!running)
                    .on_click(cx.listener(|this, _, _, cx| {
                        this.last_action = Some("CancelRun");
                        let tab = this.active_tab().clone();
                        tab.update(cx, |tab, cx| tab.cancel(cx));
                    })),
            )
            .children(progress.map(|text| div().text_sm().child(text)))
    }

    /// The active tab's messages strip: newest at the bottom, errors in red, the chosen error
    /// marked.
    fn message_strip(
        &self,
        focused: Option<Pane>,
        cx: &mut Context<Self>,
    ) -> gpui::Stateful<gpui::Div> {
        let theme = cx.theme().clone();
        let tab = self.active_tab().clone();
        let messages = tab.read(cx).messages();
        let lines = messages.lines();
        let newest = lines.len().saturating_sub(MESSAGES_SHOWN);
        let chosen = messages.chosen();
        let rows = lines[newest..].iter().enumerate().map(|(offset, message)| {
            let ix = newest + offset;
            let colour = match message.tone {
                Tone::Plain => theme.foreground,
                Tone::Warning => theme.warning,
                Tone::Error => theme.danger,
            };
            div()
                .id(("message", ix))
                .text_color(colour)
                .when(chosen == Some(ix), |row| row.bg(theme.accent))
                .when(message.line.is_some(), |row| row.cursor_pointer())
                .child(message.text.clone())
                .on_click({
                    let tab = tab.clone();
                    move |_, window, cx| {
                        tab.update(cx, |tab, cx| tab.go_to_message(ix, window, cx));
                    }
                })
        });
        let rows: Vec<_> = rows.collect();
        self.pane(Pane::Messages, focused, cx)
            .key_context("Messages")
            .h(px(120.))
            .px_2()
            .text_sm()
            .child(
                div()
                    .flex_1()
                    .flex()
                    .flex_col()
                    .justify_end()
                    .overflow_hidden()
                    .children(rows),
            )
            .on_action(cx.listener(|this, _: &PreviousError, _, cx| {
                let tab = this.active_tab().clone();
                tab.update(cx, |tab, cx| tab.step_error(false, cx));
            }))
            .on_action(cx.listener(|this, _: &NextError, _, cx| {
                let tab = this.active_tab().clone();
                tab.update(cx, |tab, cx| tab.step_error(true, cx));
            }))
            .on_action(cx.listener(|this, _: &GoToError, window, cx| {
                let tab = this.active_tab().clone();
                tab.update(cx, |tab, cx| tab.go_to_chosen(window, cx));
            }))
    }
}

impl Render for Workbench {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        if let Some(on_first_frame) = self.on_first_frame.take() {
            on_first_frame(window, cx);
        }
        let focused = self.focused_pane(window, cx);
        let theme = cx.theme().clone();

        let left = div()
            .w(px(260.))
            .flex()
            .flex_col()
            .child(self.tab_list(focused, cx).flex_1())
            .child(
                self.pane(Pane::Explorer, focused, cx)
                    .flex_1()
                    .child(self.explorer.clone()),
            );
        let editor = self.active_tab().read(cx).editor().clone();
        let mut middle = div()
            .flex_1()
            .flex()
            .flex_col()
            .child(
                self.pane(Pane::Editor, focused, cx)
                    .flex_1()
                    .child(self.run_bar(cx))
                    .child(Editor::new(&editor).h_full()),
            )
            .child(self.message_strip(focused, cx));
        let results = self.active_tab().read(cx).results().clone();
        if self.results_visible {
            middle = middle.child(
                self.pane(Pane::Results, focused, cx)
                    .flex_1()
                    .child(results.clone()),
            );
        }
        let inspector = self
            .pane(Pane::Inspector, focused, cx)
            .w(px(300.))
            .on_key_down(cx.listener(|this, event: &KeyDownEvent, _, cx| {
                this.scroll_inspector(&event.keystroke.key, cx);
            }))
            .child(inspector_body(
                results.read(cx).inspected(cx),
                &self.inspector_scroll,
                cx,
            ));

        div()
            .id("workbench")
            .key_context("Workbench")
            .size_full()
            .flex()
            .flex_col()
            .bg(theme.background)
            .text_color(theme.foreground)
            .on_action(cx.listener(|this, _: &NextPane, w, cx| {
                this.last_action = Some("NextPane");
                this.cycle(1, w, cx);
            }))
            .on_action(cx.listener(|this, _: &PrevPane, w, cx| {
                this.last_action = Some("PrevPane");
                this.cycle(-1, w, cx);
            }))
            .on_action(cx.listener(|this, _: &FocusExplorer, w, cx| {
                this.last_action = Some("FocusExplorer");
                this.focus_pane(Pane::Explorer, w, cx);
            }))
            .on_action(cx.listener(|this, _: &ToggleResults, w, cx| {
                this.last_action = Some("ToggleResults");
                this.toggle_results(w, cx);
            }))
            .on_action(cx.listener(|this, _: &Run, w, cx| {
                this.last_action = Some("Run");
                this.active_tab()
                    .clone()
                    .update(cx, |tab, cx| tab.run(w, cx));
            }))
            .on_action(cx.listener(|this, _: &CancelRun, _, cx| {
                this.last_action = Some("CancelRun");
                this.active_tab()
                    .clone()
                    .update(cx, |tab, cx| tab.cancel(cx));
            }))
            .on_action(cx.listener(|this, _: &EstimatedPlan, w, cx| {
                this.last_action = Some("EstimatedPlan");
                this.active_tab()
                    .clone()
                    .update(cx, |tab, cx| tab.estimated_plan(w, cx));
            }))
            .on_action(cx.listener(|this, _: &ToggleActualPlan, _, cx| {
                this.last_action = Some("ToggleActualPlan");
                this.active_tab()
                    .clone()
                    .update(cx, |tab, cx| tab.toggle_actual_plan(cx));
            }))
            .on_action(cx.listener(|this, _: &ToggleResultsToFile, _, cx| {
                this.last_action = Some("ToggleResultsToFile");
                this.active_tab()
                    .clone()
                    .update(cx, |tab, cx| tab.toggle_results_to_file(cx));
            }))
            .on_action(cx.listener(|this, _: &TogglePlanView, _, cx| {
                this.last_action = Some("TogglePlanView");
                let results = this.active_tab().read(cx).results().clone();
                results.update(cx, |results, cx| results.toggle_plan_view(cx));
            }))
            .on_action(cx.listener(|this, _: &NewQueryTab, w, cx| {
                this.last_action = Some("NewQueryTab");
                this.new_query_tab(w, cx);
            }))
            .on_action(cx.listener(|this, _: &NextQueryTab, w, cx| {
                this.last_action = Some("NextQueryTab");
                let next = (this.active + 1) % this.tabs.len();
                this.show_tab(next, w, cx);
            }))
            .on_action(
                cx.listener(|this, _: &CommentLines, _, cx| this.not_built("CommentLines", cx)),
            )
            .on_action(
                cx.listener(|this, _: &UncommentLines, _, cx| this.not_built("UncommentLines", cx)),
            )
            .on_action(cx.listener(|this, _: &Format, w, cx| {
                this.last_action = Some("Format");
                this.active_tab()
                    .clone()
                    .update(cx, |tab, cx| tab.format(w, cx));
            }))
            .on_action(cx.listener(|this, _: &RefreshAutocomplete, _, cx| {
                this.not_built("RefreshAutocomplete", cx)
            }))
            .on_action(cx.listener(|this, _: &PickDatabase, w, cx| {
                this.last_action = Some("PickDatabase");
                this.active_tab()
                    .clone()
                    .update(cx, |tab, cx| tab.pick_database(w, cx));
            }))
            // Insert is bound in the explorer only; File, New connection reaches it from anywhere.
            .on_action(cx.listener(|this, _: &NewConnection, w, cx| {
                this.explorer
                    .update(cx, |explorer, cx| explorer.new_connection(w, cx));
            }))
            .on_action(cx.listener(|this, _: &OpenFile, w, cx| {
                this.last_action = Some("OpenFile");
                this.open_file(w, cx);
            }))
            .on_action(cx.listener(|this, _: &SaveFile, w, cx| {
                this.last_action = Some("SaveFile");
                this.save_file(w, cx);
            }))
            .on_action(cx.listener(|this, _: &ShowHistory, w, cx| {
                this.last_action = Some("ShowHistory");
                this.show_history(w, cx);
            }))
            .on_action(
                cx.listener(|this, _: &OpenFileMenu, w, cx| this.open_menu(Menu::File, w, cx)),
            )
            .on_action(
                cx.listener(|this, _: &OpenEditMenu, w, cx| this.open_menu(Menu::Edit, w, cx)),
            )
            .on_action(
                cx.listener(|this, _: &OpenViewMenu, w, cx| this.open_menu(Menu::View, w, cx)),
            )
            .on_action(
                cx.listener(|this, _: &OpenQueryMenu, w, cx| this.open_menu(Menu::Query, w, cx)),
            )
            .on_action(
                cx.listener(|this, _: &OpenHelpMenu, w, cx| this.open_menu(Menu::Help, w, cx)),
            )
            .on_action(cx.listener(|_, _: &Quit, _, cx| cx.quit()))
            .on_action(cx.listener(|this, _: &ShowShortcuts, w, cx| {
                this.last_action = Some("ShowShortcuts");
                let situation = this.situation(w, cx);
                this.menu_bar
                    .update(cx, |bar, cx| bar.show_shortcuts(&situation, w, cx));
            }))
            .on_action(cx.listener(|this, _: &ShowAbout, w, cx| {
                this.last_action = Some("ShowAbout");
                this.menu_bar.update(cx, |bar, cx| bar.show_about(w, cx));
            }))
            .child(self.menu_bar.clone())
            .child(
                div()
                    .flex_1()
                    .min_h_0()
                    .flex()
                    .flex_row()
                    .child(left)
                    .child(middle)
                    .child(inspector),
            )
    }
}

/// The inspector's contents, scrolling under `scroll`: the selected cell's column, type and
/// whole value, or the selected plan operator's name and its details, one per line.
fn inspector_body(inspected: Option<Inspected>, scroll: &ScrollHandle, cx: &App) -> gpui::Div {
    let theme = cx.theme();
    let Some(inspected) = inspected else {
        return div()
            .px_2()
            .text_sm()
            .text_color(theme.muted_foreground)
            .child("Select a cell or a plan operator in the results to see it in full.");
    };
    let value = div()
        .id("inspector-value")
        .flex_1()
        .min_h_0()
        .overflow_y_scroll()
        .track_scroll(scroll);
    let (heading, value) = match inspected {
        Inspected::Cell {
            column,
            type_name,
            text,
        } => (
            format!("{column} ({type_name})"),
            match text {
                Some(text) => value.child(text),
                None => value
                    .italic()
                    .text_color(theme.muted_foreground)
                    .child("NULL"),
            },
        ),
        Inspected::Operator { operator, details } => (
            operator,
            value
                .flex()
                .flex_col()
                .children(details.into_iter().map(|detail| {
                    div()
                        .child(
                            div()
                                .text_xs()
                                .text_color(theme.muted_foreground)
                                .child(detail.name),
                        )
                        .child(detail.value)
                })),
        ),
    };
    div()
        .px_2()
        .flex_1()
        .min_h_0()
        .flex()
        .flex_col()
        .gap_1()
        .text_sm()
        .child(div().text_color(theme.muted_foreground).child(heading))
        .child(value)
}
