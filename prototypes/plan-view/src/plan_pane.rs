//! PROTOTYPE. The Plan tab's content: holds the parsed plans and the selection, and draws
//! them with whichever of the three plan views is active. Each view lives in its own module;
//! this one owns state, keys and the per-view meaning of the arrow keys.

use std::collections::HashSet;

use gpui::{
    App, Context, FocusHandle, Focusable, IntoElement, KeyBinding, Render, ScrollHandle, Window,
    actions, div, prelude::*,
};
use gpui_component::ActiveTheme;

use crate::model::Plan;
use crate::{view_graph, view_icicle, view_table};

actions!(
    plan_pane,
    [Up, Down, Left, Right, ToggleMetric, ToggleExpand]
);

const CONTEXT: &str = "PlanPane";

pub fn init(cx: &mut App) {
    cx.bind_keys([
        KeyBinding::new("up", Up, Some(CONTEXT)),
        KeyBinding::new("down", Down, Some(CONTEXT)),
        KeyBinding::new("left", Left, Some(CONTEXT)),
        KeyBinding::new("right", Right, Some(CONTEXT)),
        KeyBinding::new("t", ToggleMetric, Some(CONTEXT)),
        KeyBinding::new("space", ToggleExpand, Some(CONTEXT)),
    ]);
}

/// The three plan views being compared.
#[derive(Clone, Copy, PartialEq, Debug)]
pub enum View {
    Graph,
    Table,
    Icicle,
}

impl View {
    pub const ALL: [View; 3] = [View::Graph, View::Table, View::Icicle];

    pub fn name(self) -> &'static str {
        match self {
            View::Graph => "A  SSMS graph",
            View::Table => "B  Tree table",
            View::Icicle => "C  Icicle",
        }
    }

    pub fn parse(s: &str) -> Option<Self> {
        match s.to_ascii_lowercase().as_str() {
            "a" => Some(View::Graph),
            "b" => Some(View::Table),
            "c" => Some(View::Icicle),
            _ => None,
        }
    }
}

/// What C sizes its bars by.
#[derive(Clone, Copy, PartialEq, Debug)]
pub enum Metric {
    Cost,
    Time,
}

pub struct PlanPane {
    pub plans: Vec<Plan>,
    /// Index into `plans` of the plan on screen.
    pub current: usize,
    pub view: View,
    /// Selected operator per plan.
    pub selected: Vec<Option<usize>>,
    /// B: operators whose children are hidden, per plan.
    pub collapsed: Vec<HashSet<usize>>,
    pub metric: Metric,
    /// A: the graph's scroll position.
    pub graph_scroll: ScrollHandle,
    /// A: scroll the selection into view at the next render that knows the pane's size.
    pub pending_reveal: bool,
    focus: FocusHandle,
}

impl PlanPane {
    pub fn new(plans: Vec<Plan>, cx: &mut Context<Self>) -> Self {
        let n = plans.len();
        Self {
            plans,
            current: 0,
            view: View::Graph,
            selected: vec![None; n],
            collapsed: vec![HashSet::new(); n],
            metric: Metric::Cost,
            graph_scroll: ScrollHandle::new(),
            pending_reveal: false,
            focus: cx.focus_handle(),
        }
    }

    pub fn plan(&self) -> &Plan {
        &self.plans[self.current]
    }

    pub fn selection(&self) -> Option<usize> {
        self.selected[self.current]
    }

    pub fn select(&mut self, op: usize, cx: &mut Context<Self>) {
        self.selected[self.current] = Some(op);
        // Reveal the operator in B by expanding its ancestors.
        let mut p = self.plan().ops[op].parent;
        while let Some(i) = p {
            self.collapsed[self.current].remove(&i);
            p = self.plans[self.current].ops[i].parent;
        }
        self.pending_reveal = true;
        cx.notify();
    }

    /// B: operators in display order, skipping the children of collapsed ones.
    pub fn visible_rows(&self) -> Vec<usize> {
        let plan = self.plan();
        let collapsed = &self.collapsed[self.current];
        let mut out = Vec::new();
        let mut stack = vec![0];
        while let Some(i) = stack.pop() {
            out.push(i);
            if !collapsed.contains(&i) {
                stack.extend(plan.ops[i].children.iter().rev());
            }
        }
        out
    }

    /// C: an operator's children, largest first by the current metric.
    pub fn hottest_children(&self, op: usize) -> Vec<usize> {
        let plan = self.plan();
        let mut kids = plan.ops[op].children.clone();
        kids.sort_by(|a, b| self.size_of(*b).total_cmp(&self.size_of(*a)));
        kids
    }

    /// C: the inclusive size of an operator by the current metric.
    pub fn size_of(&self, op: usize) -> f64 {
        let o = &self.plan().ops[op];
        match self.metric {
            Metric::Cost => o.subtree_cost_pct,
            Metric::Time => o.total_ms,
        }
    }

    pub fn toggle_expand(&mut self, op: usize, cx: &mut Context<Self>) {
        let set = &mut self.collapsed[self.current];
        if !set.remove(&op) {
            set.insert(op);
        }
        cx.notify();
    }

    fn arrow(&mut self, key: Arrow, cx: &mut Context<Self>) {
        let Some(sel) = self.selection() else {
            return self.select(0, cx);
        };
        let ops = &self.plan().ops;
        let parent = ops[sel].parent;
        let first_child = ops[sel].children.first().copied();
        let next = match (self.view, key) {
            // A: data flows right to left, so Left goes toward the root.
            (View::Graph, Arrow::Left) => parent,
            (View::Graph, Arrow::Right) => first_child,
            (View::Graph, Arrow::Up) => sel.checked_sub(1),
            (View::Graph, Arrow::Down) => Some((sel + 1).min(ops.len() - 1)),
            (View::Table, Arrow::Up | Arrow::Down) => {
                let rows = self.visible_rows();
                let at = rows.iter().position(|r| *r == sel).unwrap_or(0);
                let to = if key == Arrow::Up {
                    at.saturating_sub(1)
                } else {
                    (at + 1).min(rows.len() - 1)
                };
                Some(rows[to])
            }
            (View::Table, Arrow::Left) => {
                if first_child.is_some() && !self.collapsed[self.current].contains(&sel) {
                    return self.toggle_expand(sel, cx);
                }
                parent
            }
            (View::Table, Arrow::Right) => {
                if self.collapsed[self.current].contains(&sel) {
                    return self.toggle_expand(sel, cx);
                }
                first_child
            }
            // C: the root is the top bar; siblings run hottest first, left to right.
            (View::Icicle, Arrow::Up) => parent,
            (View::Icicle, Arrow::Down) => self.hottest_children(sel).first().copied(),
            (View::Icicle, Arrow::Left | Arrow::Right) => {
                let siblings = parent
                    .map(|p| self.hottest_children(p))
                    .unwrap_or(vec![sel]);
                let at = siblings.iter().position(|s| *s == sel).unwrap_or(0);
                let to = if key == Arrow::Left {
                    at.saturating_sub(1)
                } else {
                    (at + 1).min(siblings.len() - 1)
                };
                Some(siblings[to])
            }
        };
        if let Some(n) = next {
            self.select(n, cx);
        }
    }
}

/// The four arrow keys, so `arrow` can match on view and key together.
#[derive(Clone, Copy, PartialEq, Debug)]
enum Arrow {
    Up,
    Down,
    Left,
    Right,
}

impl Focusable for PlanPane {
    fn focus_handle(&self, _: &App) -> FocusHandle {
        self.focus.clone()
    }
}

impl Render for PlanPane {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let body = match self.view {
            View::Graph => view_graph::render(self, window, cx).into_any_element(),
            View::Table => view_table::render(self, window, cx).into_any_element(),
            View::Icicle => view_icicle::render(self, window, cx).into_any_element(),
        };
        div()
            .id("plan-pane")
            .key_context(CONTEXT)
            .track_focus(&self.focus)
            .size_full()
            .bg(cx.theme().background)
            .on_mouse_down(
                gpui::MouseButton::Left,
                cx.listener(|this, _, w, cx| w.focus(&this.focus, cx)),
            )
            .on_action(cx.listener(|this, _: &Up, _, cx| this.arrow(Arrow::Up, cx)))
            .on_action(cx.listener(|this, _: &Down, _, cx| this.arrow(Arrow::Down, cx)))
            .on_action(cx.listener(|this, _: &Left, _, cx| this.arrow(Arrow::Left, cx)))
            .on_action(cx.listener(|this, _: &Right, _, cx| this.arrow(Arrow::Right, cx)))
            .on_action(cx.listener(|this, _: &ToggleMetric, _, cx| {
                this.metric = if this.metric == Metric::Cost {
                    Metric::Time
                } else {
                    Metric::Cost
                };
                cx.notify();
            }))
            .on_action(cx.listener(|this, _: &ToggleExpand, _, cx| {
                if let Some(s) = this.selection() {
                    this.toggle_expand(s, cx);
                }
            }))
            .child(body)
    }
}

/// The strip above every view: which plan, how long it ran, how many operators carry
/// warnings or misestimates, and the view's own key hint.
pub fn header(pane: &PlanPane, hint: &str, cx: &App) -> gpui::Div {
    let theme = cx.theme();
    let plan = pane.plan();
    let warnings = plan.ops.iter().filter(|o| !o.warnings.is_empty()).count();
    let misestimates = plan.ops.iter().filter(|o| o.is_misestimated()).count();
    div()
        .flex()
        .items_center()
        .gap_3()
        .px_2()
        .py_1()
        .border_b_1()
        .border_color(theme.border)
        .text_xs()
        .child(div().font_weight(gpui::FontWeight::BOLD).child(format!(
            "{}  ·  ran {}",
            plan.title,
            crate::model::ms(plan.elapsed_ms)
        )))
        .when(warnings > 0, |d| {
            d.child(
                div()
                    .text_color(view_graph::WARN_RED)
                    .child(format!("⚠ {warnings} with warnings")),
            )
        })
        .when(misestimates > 0, |d| {
            d.child(
                div()
                    .text_color(view_graph::WARN_RED)
                    .child(format!("{misestimates} row estimates off by 10× or more")),
            )
        })
        .child(div().flex_1())
        .child(
            div()
                .text_color(theme.muted_foreground)
                .child(hint.to_string()),
        )
}
