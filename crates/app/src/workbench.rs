//! The main window's root view: layout C "Workbench" from the spec. Query tabs and the
//! object explorer on the left, the active tab's editor with its messages strip and the
//! results in the middle, the inspector on the right.
//!
//! U1 built the frame: the panes, moving focus between them, and every key in the keymap.
//! U2 put the saved connections in the explorer (`explorer.rs`); U3 the query tabs
//! (`query_tab.rs`) and the messages strip (`messages.rs`). Actions whose feature is not
//! built yet say so in the messages strip.

use gpui::{
    App, AppContext as _, Context, Entity, FocusHandle, Focusable as _, IntoElement,
    ParentElement as _, Render, SharedString, Styled as _, Window, div, prelude::*, px,
};
use gpui_component::button::Button;
use gpui_component::input::Editor;
use gpui_component::{ActiveTheme as _, Disableable as _};
use serde::Serialize;

use crate::explorer::{Explorer, ExplorerEvent, ExplorerState};
use crate::keymap::{
    CancelRun, CommentLines, EstimatedPlan, FocusExplorer, Format, GoToError, NewQueryTab,
    NextError, NextPane, NextQueryTab, OpenFile, PickDatabase, PrevPane, PreviousError,
    RefreshAutocomplete, Run, SaveFile, ToggleActualPlan, TogglePlanView, ToggleResults,
    UncommentLines,
};
use crate::messages::Tone;
use crate::palette;
use crate::query_tab::dialogs::DialogState;
use crate::query_tab::{QueryTab, QueryTarget, TabState};
use crate::runtime::Runtime;

/// How many of the newest messages the strip renders; it shows as many as fit.
// shortcut: the strip draws the newest lines only, with no scrollback; a long run's early
// messages scroll out of reach. Upgrade to a virtual list when runs print more than this.
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
    /// Focus handles for the panes that don't own one; the editor and explorer do.
    pane_focus: [FocusHandle; 6],
    results_visible: bool,
    last_action: Option<&'static str>,
    /// Called once, from the first `render`.
    on_first_frame: Option<FirstFrameHook>,
}

impl Workbench {
    /// Builds the workbench with one unconnected query tab, its editor focused.
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
            },
        )
        .detach();
        let mut this = Self {
            tabs: Vec::new(),
            active: 0,
            next_tab_number: 1,
            runtime,
            explorer,
            pane_focus,
            results_visible: true,
            last_action: None,
            on_first_frame: Some(Box::new(on_first_frame)),
        };
        this.open_tab(None, window, cx);
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

    /// Opens a tab on `target` (or unconnected), makes it active and focuses its editor.
    fn open_tab(
        &mut self,
        target: Option<QueryTarget>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let title: SharedString = format!("Query {}", self.next_tab_number).into();
        self.next_tab_number += 1;
        let runtime = self.runtime.clone();
        let tab = cx.new(|cx| QueryTab::new(title, target, runtime, window, cx));
        // The tab list, the strip and the Stop button show the tab's state.
        cx.observe(&tab, |_, _, cx| cx.notify()).detach();
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
        let target = self.active_tab().read(cx).target().cloned();
        self.open_tab(target, window, cx);
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

    /// True while a connect, a connection test, or a tab's run, switch or list is under way.
    pub fn is_busy(&self, cx: &App) -> bool {
        self.explorer.read(cx).is_busy(cx) || self.tabs.iter().any(|t| t.read(cx).is_busy())
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
        }
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
            Pane::Editor | Pane::Explorer => frame,
            other => frame.track_focus(&self.focus_handle_of(other, cx)),
        }
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
                .child(div().text_sm().child(tab.label()))
                .on_click(cx.listener(move |this, _, window, cx| this.show_tab(ix, window, cx)))
        });
        self.pane(Pane::QueryTabs, focused, cx).children(rows)
    }

    /// Run and Stop for the active tab. Stop is the way to cancel from a real keyboard on
    /// Windows, where gpui doesn't pass Alt+Break through (keymap.rs).
    fn run_bar(&self, cx: &mut Context<Self>) -> gpui::Div {
        let running = self.active_tab().read(cx).is_running();
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
        if self.results_visible {
            middle = middle.child(self.pane(Pane::Results, focused, cx).flex_1());
        }
        let inspector = self.pane(Pane::Inspector, focused, cx).w(px(300.));

        div()
            .id("workbench")
            .key_context("Workbench")
            .size_full()
            .flex()
            .flex_row()
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
            .on_action(
                cx.listener(|this, _: &EstimatedPlan, _, cx| this.not_built("EstimatedPlan", cx)),
            )
            .on_action(cx.listener(|this, _: &ToggleActualPlan, _, cx| {
                this.not_built("ToggleActualPlan", cx)
            }))
            .on_action(
                cx.listener(|this, _: &TogglePlanView, _, cx| this.not_built("TogglePlanView", cx)),
            )
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
            .on_action(cx.listener(|this, _: &Format, _, cx| this.not_built("Format", cx)))
            .on_action(cx.listener(|this, _: &RefreshAutocomplete, _, cx| {
                this.not_built("RefreshAutocomplete", cx)
            }))
            .on_action(cx.listener(|this, _: &PickDatabase, w, cx| {
                this.last_action = Some("PickDatabase");
                this.active_tab()
                    .clone()
                    .update(cx, |tab, cx| tab.pick_database(w, cx));
            }))
            .on_action(cx.listener(|this, _: &OpenFile, _, cx| this.not_built("OpenFile", cx)))
            .on_action(cx.listener(|this, _: &SaveFile, _, cx| this.not_built("SaveFile", cx)))
            .child(left)
            .child(middle)
            .child(inspector)
    }
}
