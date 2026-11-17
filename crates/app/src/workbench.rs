//! The main window's root view: layout C "Workbench" from the spec. Query tabs and the
//! object explorer on the left, the editor with its messages strip and the results in the
//! middle, the inspector on the right.
//!
//! U1 builds the frame only: the panes, moving focus between them, and every key in the
//! keymap. Actions whose feature is not built yet say so in the messages strip.

use gpui::{
    App, AppContext as _, Context, Entity, FocusHandle, Focusable as _, IntoElement,
    ParentElement as _, Render, SharedString, Styled as _, Window, div, prelude::*, px,
};
use gpui_component::ActiveTheme as _;
use gpui_component::input::{Editor, EditorState};
use serde::Serialize;

use crate::keymap::{
    CancelRun, CommentLines, EstimatedPlan, FocusExplorer, Format, NewQueryTab, NextPane,
    NextQueryTab, OpenFile, PickDatabase, PrevPane, RefreshAutocomplete, Run, SaveFile,
    ToggleActualPlan, TogglePlanView, ToggleResults, UncommentLines,
};

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
    pub editor_text: String,
}

pub struct Workbench {
    editor: Entity<EditorState>,
    /// Focus handles for every pane except the editor, which owns its own.
    pane_focus: [FocusHandle; 6],
    results_visible: bool,
    last_action: Option<&'static str>,
    messages: Vec<SharedString>,
    /// Called once, from the first `render`.
    on_first_frame: Option<Box<dyn FnOnce(&mut Window, &mut Context<Self>)>>,
}

impl Workbench {
    /// Builds the workbench with an empty SQL editor focused. `on_first_frame` runs once,
    /// when the window first draws: the startup measurement and the UI script hook in there.
    pub fn new(
        on_first_frame: impl FnOnce(&mut Window, &mut Context<Self>) + 'static,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        let editor = cx.new(|cx| EditorState::new(window, cx).language("sql"));
        let pane_focus = std::array::from_fn(|_| cx.focus_handle());
        let this = Self {
            editor,
            pane_focus,
            results_visible: true,
            last_action: None,
            messages: Vec::new(),
            on_first_frame: Some(Box::new(on_first_frame)),
        };
        // Repaint the pane outlines whenever focus moves.
        for pane in Pane::ALL {
            let handle = this.focus_handle_of(pane, cx);
            cx.on_focus_in(&handle, window, |_, _, cx| cx.notify())
                .detach();
        }
        this.editor.update(cx, |e, cx| e.focus(window, cx));
        this
    }

    fn focus_handle_of(&self, pane: Pane, cx: &App) -> FocusHandle {
        match pane {
            Pane::Editor => self.editor.focus_handle(cx),
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
        let handle = self.focus_handle_of(pane, cx);
        window.focus(&handle, cx);
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
        self.messages
            .push(format!("{name} is not built yet.").into());
        cx.notify();
    }

    pub fn visible_state(&self, window: &Window, cx: &App) -> VisibleState {
        VisibleState {
            focused_pane: self.focused_pane(window, cx),
            last_action: self.last_action,
            results_visible: self.results_visible,
            messages: self.messages.clone(),
            editor_text: self.editor.read(cx).value().to_string(),
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
                theme.ring
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
            Pane::Editor => frame,
            other => frame.track_focus(&self.focus_handle_of(other, cx)),
        }
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
            .w(px(240.))
            .flex()
            .flex_col()
            .child(self.pane(Pane::QueryTabs, focused, cx).flex_1())
            .child(self.pane(Pane::Explorer, focused, cx).flex_1());
        let messages = self
            .pane(Pane::Messages, focused, cx)
            .h(px(96.))
            .px_2()
            .text_sm()
            .children(self.messages.iter().rev().take(4).rev().cloned());
        let mut middle = div()
            .flex_1()
            .flex()
            .flex_col()
            .child(
                self.pane(Pane::Editor, focused, cx)
                    .flex_1()
                    .child(Editor::new(&self.editor).h_full()),
            )
            .child(messages);
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
            .on_action(cx.listener(|this, _: &Run, _, cx| this.not_built("Run", cx)))
            .on_action(cx.listener(|this, _: &CancelRun, _, cx| this.not_built("CancelRun", cx)))
            .on_action(
                cx.listener(|this, _: &EstimatedPlan, _, cx| this.not_built("EstimatedPlan", cx)),
            )
            .on_action(cx.listener(|this, _: &ToggleActualPlan, _, cx| {
                this.not_built("ToggleActualPlan", cx)
            }))
            .on_action(
                cx.listener(|this, _: &TogglePlanView, _, cx| this.not_built("TogglePlanView", cx)),
            )
            .on_action(
                cx.listener(|this, _: &NewQueryTab, _, cx| this.not_built("NewQueryTab", cx)),
            )
            .on_action(
                cx.listener(|this, _: &NextQueryTab, _, cx| this.not_built("NextQueryTab", cx)),
            )
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
            .on_action(
                cx.listener(|this, _: &PickDatabase, _, cx| this.not_built("PickDatabase", cx)),
            )
            .on_action(cx.listener(|this, _: &OpenFile, _, cx| this.not_built("OpenFile", cx)))
            .on_action(cx.listener(|this, _: &SaveFile, _, cx| this.not_built("SaveFile", cx)))
            .child(left)
            .child(middle)
            .child(inspector)
    }
}
