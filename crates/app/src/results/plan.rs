//! A plan in the results pane (docs/spec.md, "UI" → "Plans"): one statement's operators as a
//! tree table, each row indented under its parent, with the estimated and actual rows, the
//! estimate error, cost %, time and reads; or, after F9, as a graph (`graph.rs`). Misestimates
//! of 10× or more either way are flagged, and the statement's costliest operator is marked
//! and selected when the plan arrives. The selected operator's every raw property shows in
//! the inspector, and the selection stays put when F9 switches views.
//!
//! Keys while a plan has focus: Up and Down move the selection in tree order, Left goes to
//! the parent and Right to the first input, Home and End go to the first and last operator,
//! Ctrl+S saves the plan as a `.sqlplan` file. A click selects an operator and gives the plan
//! focus.

mod graph;
mod icons;

use std::sync::Arc;

use dbide_engine::plan::{Object, Operator, Plan, Property};
use gpui::{
    App, Context, FocusHandle, InteractiveElement as _, IntoElement, ParentElement as _,
    ScrollHandle, SharedString, StatefulInteractiveElement as _, Styled as _, div, prelude::*, px,
};
use gpui_component::ActiveTheme as _;
use serde::Serialize;

use super::Results;
use graph::Layout;

/// An estimate this many times too high or too low, or worse, is flagged.
const MISESTIMATE: f64 = 10.0;

/// How far each level of the tree is indented.
const INDENT: f32 = 14.;

/// One statement's plan, as the results pane shows it.
pub struct PlanView {
    /// The whole plan the statement came from: Save writes all of it, as SSMS does.
    plan: Arc<Plan>,
    /// Which of `plan`'s statements this view shows.
    statement: usize,
    /// True when the plan is SQL Server's showplan, the one format `.sqlplan` holds.
    savable: bool,
    /// The statement's SQL, when the server sent it.
    text: Option<String>,
    /// The operators in tree order, parents before their children.
    rows: Vec<Row>,
    /// The row with the highest own cost.
    costliest: usize,
    selected: usize,
    /// True when the server ran the statement, so actual rows and times are filled in.
    actual: bool,
    /// The rows' scroll position, so the selection stays in view.
    scroll: ScrollHandle,
    /// Where each operator goes in the graph, and each one's parent.
    layout: Layout,
    /// The graph's scroll position, so the selection stays in view there too.
    graph_scroll: ScrollHandle,
    /// True when the selection moved since the graph last scrolled to it (`graph.rs`).
    reveal_pending: std::rc::Rc<std::cell::Cell<bool>>,
    focus_handle: FocusHandle,
}

/// One operator's place in the tree; its children follow it as rows of their own.
struct Row {
    depth: usize,
    /// Child indices from the statement's root down to the operator; empty for the root.
    path: Vec<usize>,
}

/// What a user can see of one plan.
#[derive(Debug, Serialize)]
pub struct PlanState {
    /// The statement's SQL, when the server sent it.
    pub statement: Option<String>,
    pub actual: bool,
    pub focused: bool,
    /// True when shown as a graph rather than the tree table.
    pub graph: bool,
    /// In the graph, true when the selected operator's whole box is in view.
    pub selected_in_view: bool,
    /// Index into `operators` of the selected operator.
    pub selected: usize,
    pub operators: Vec<OperatorState>,
}

/// What a user can see of one operator's row.
#[derive(Debug, Serialize)]
pub struct OperatorState {
    /// How deep in the tree, the statement's root being 0.
    pub depth: usize,
    pub name: String,
    pub kind: String,
    pub object: Option<String>,
    /// Estimated rows over all executions or loops, the figure compared with actual rows.
    pub estimated_rows: f64,
    pub actual_rows: Option<f64>,
    /// How many times too high or too low the estimate was; `null` without actual rows,
    /// and for an infinite error.
    pub estimate_error: Option<f64>,
    /// True when the estimate is off by 10× or more.
    pub misestimate: bool,
    /// The operator's own share of the statement's cost.
    pub cost_percent: f64,
    /// True for the statement's costliest operator.
    pub costliest: bool,
    pub elapsed_ms: Option<f64>,
    pub logical_reads: Option<u64>,
}

/// Where a key moves the selection.
#[derive(Clone, Copy)]
pub enum Step {
    Previous,
    Next,
    First,
    Last,
    /// The operator this one feeds.
    Parent,
    /// The operator's first input.
    Child,
}

impl PlanView {
    /// One view per statement in `plan`, each selecting its costliest operator. `savable`
    /// says whether the plan can be saved as `.sqlplan`.
    pub fn for_statements(plan: Plan, savable: bool, cx: &mut App) -> Vec<PlanView> {
        let plan = Arc::new(plan);
        plan.statements
            .iter()
            .enumerate()
            .map(|(ix, statement)| {
                let mut rows = Vec::new();
                flatten(&statement.root, &mut Vec::new(), &mut rows);
                let op = |row: &Row| at(&statement.root, &row.path);
                let costliest = rows
                    .iter()
                    .enumerate()
                    .max_by(|(_, a), (_, b)| op(a).cost_percent.total_cmp(&op(b).cost_percent))
                    .map_or(0, |(ix, _)| ix);
                let actual = rows.iter().any(|r| op(r).actual_rows.is_some());
                let layout = Layout::new(&rows);
                PlanView {
                    plan: plan.clone(),
                    statement: ix,
                    savable,
                    text: statement.text.clone(),
                    rows,
                    costliest,
                    selected: costliest,
                    actual,
                    scroll: ScrollHandle::new(),
                    layout,
                    graph_scroll: ScrollHandle::new(),
                    reveal_pending: std::rc::Rc::new(std::cell::Cell::new(true)),
                    focus_handle: cx.focus_handle(),
                }
            })
            .collect()
    }

    pub fn focus_handle(&self) -> &FocusHandle {
        &self.focus_handle
    }

    /// The whole plan, and whether it can be saved as `.sqlplan`.
    pub fn plan(&self) -> (&Arc<Plan>, bool) {
        (&self.plan, self.savable)
    }

    /// The operator `row` stands for.
    fn operator(&self, row: &Row) -> &Operator {
        at(&self.plan.statements[self.statement].root, &row.path)
    }

    /// Selects operator `row`, as a click does.
    pub fn select_row(&mut self, row: usize) {
        self.selected = row.min(self.rows.len().saturating_sub(1));
    }

    /// Moves the selection, keeping it in view in both the table and the graph.
    pub fn select(&mut self, step: Step) {
        let last = self.rows.len().saturating_sub(1);
        let at = self.selected;
        self.selected = match step {
            Step::Previous => at.saturating_sub(1),
            Step::Next => (at + 1).min(last),
            Step::First => 0,
            Step::Last => last,
            Step::Parent => self.layout.parent(at).unwrap_or(at),
            // A first input follows its parent directly in tree order.
            Step::Child => match self.rows.get(at + 1) {
                Some(next) if next.depth > self.rows[at].depth => at + 1,
                _ => at,
            },
        };
        self.reveal();
    }

    /// Scrolls the selected operator into view in both the tree table and the graph, each the
    /// next time it is drawn: at once in the view shown, and in the other when F9 shows it.
    pub(super) fn reveal(&self) {
        self.scroll.scroll_to_item(self.selected);
        // The graph scrolls itself as it is laid out (`graph.rs`).
        self.reveal_pending.set(true);
    }

    /// What a user can see of the plan; `graph` says it is shown as a graph.
    pub fn state(&self, focused: bool, graph: bool) -> PlanState {
        PlanState {
            statement: self.text.clone(),
            actual: self.actual,
            focused,
            graph,
            selected_in_view: graph && self.selected_in_view(),
            selected: self.selected,
            operators: self
                .rows
                .iter()
                .enumerate()
                .map(|(ix, row)| {
                    let op = self.operator(row);
                    let error = op.estimate_error();
                    OperatorState {
                        depth: row.depth,
                        name: op.name.clone(),
                        kind: op.kind.clone(),
                        object: op.object.as_ref().map(object_label),
                        estimated_rows: op.estimated_rows_total,
                        actual_rows: op.actual_rows,
                        estimate_error: error.filter(|e| e.is_finite()),
                        misestimate: error.is_some_and(|e| e >= MISESTIMATE),
                        cost_percent: op.cost_percent,
                        costliest: ix == self.costliest,
                        elapsed_ms: op.elapsed_ms,
                        logical_reads: op.logical_reads,
                    }
                })
                .collect(),
        }
    }

    /// The selected operator's name and details for the inspector: the figures the table
    /// shows, its warnings, then every raw property the server sent.
    pub fn inspected(&self) -> Option<(String, Vec<Property>)> {
        let op = self.operator(self.rows.get(self.selected)?);
        let mut details = Vec::new();
        let mut add = |name: &str, value: String| {
            details.push(Property {
                name: name.into(),
                value,
            })
        };
        if op.kind != op.name {
            add("Logical operator", op.kind.clone());
        }
        if let Some(object) = &op.object {
            add("Object", object_label(object));
        }
        add("Estimated rows", rows(op.estimated_rows_total));
        if let Some(actual) = op.actual_rows {
            add("Actual rows", rows(actual));
        }
        add("Estimated cost", format!("{:.4}", op.subtree_cost));
        add("Cost", format!("{:.1}%", op.cost_percent));
        if let Some(ms) = op.elapsed_ms {
            add("Elapsed", format!("{ms:.1} ms"));
        }
        if let Some(ms) = op.cpu_ms {
            add("CPU", format!("{ms:.1} ms"));
        }
        if let Some(reads) = op.logical_reads {
            add("Logical reads", reads.to_string());
        }
        if let Some(reads) = op.physical_reads {
            add("Physical reads", reads.to_string());
        }
        for warning in &op.warnings {
            add("Warning", warning.clone());
        }
        details.extend(op.properties.iter().cloned());
        Some((op.name.clone(), details))
    }

    /// The plan with its heading: its number, whether it is actual, and its statement, then
    /// the tree table or, when `graph`, the graph. `ix` is its section in the results pane,
    /// where a click on an operator selects it.
    pub fn render(
        &self,
        ix: usize,
        number: usize,
        focused: bool,
        graph: bool,
        cx: &mut Context<Results>,
    ) -> impl IntoElement {
        let theme = cx.theme().clone();
        let kind = if self.actual { "actual" } else { "estimated" };
        let mut heading = format!("Plan {number} ({kind})");
        if let Some(text) = &self.text {
            let line = text.split_whitespace().collect::<Vec<_>>().join(" ");
            let line: String = line.chars().take(100).collect();
            heading.push_str(": ");
            heading.push_str(&line);
        }
        let header = row_cells(
            0,
            [
                "Operator",
                "Object",
                "Est. rows",
                "Rows",
                "Error",
                "Cost",
                "Time",
                "Reads",
            ]
            .map(SharedString::from),
        )
        .text_color(theme.muted_foreground);
        let rows = self.rows.iter().enumerate().map(|(row_ix, row)| {
            let op = self.operator(row);
            let error = op.estimate_error();
            let label = if op.kind.is_empty() || op.kind == op.name {
                op.name.clone()
            } else {
                format!("{} ({})", op.name, op.kind)
            };
            let cells = [
                label,
                op.object.as_ref().map(object_label).unwrap_or_default(),
                rows(op.estimated_rows_total),
                op.actual_rows.map(rows).unwrap_or_default(),
                match error {
                    Some(e) if e.is_finite() => format!("×{e:.0}"),
                    Some(_) => "×∞".into(),
                    None => String::new(),
                },
                format!("{:.0}%", op.cost_percent),
                op.elapsed_ms
                    .map(|ms| format!("{ms:.0} ms"))
                    .unwrap_or_default(),
                op.logical_reads.map(|r| r.to_string()).unwrap_or_default(),
            ]
            .map(SharedString::from);
            let misestimate = error.is_some_and(|e| e >= MISESTIMATE);
            row_cells(row.depth, cells)
                .id(("operator", row_ix))
                .when(row_ix == self.selected, |r| {
                    r.bg(if focused {
                        theme.accent
                    } else {
                        theme.secondary
                    })
                })
                .when(misestimate, |r| r.text_color(theme.warning))
                .when(row_ix == self.costliest, |r| {
                    r.font_weight(gpui::FontWeight::BOLD)
                })
                .on_click(cx.listener(move |results, _, window, cx| {
                    results.click_operator(ix, row_ix, window, cx);
                }))
        });
        let rows: Vec<_> = rows.collect();
        let body = if graph {
            self.graph(ix, focused, cx)
        } else {
            div()
                .id(("plan-table", ix))
                .flex_1()
                .min_h_0()
                .flex()
                .flex_col()
                .child(header)
                .child(
                    div()
                        .id(("plan-rows", ix))
                        .flex_1()
                        .min_h_0()
                        .flex()
                        .flex_col()
                        .overflow_y_scroll()
                        .track_scroll(&self.scroll)
                        .children(rows),
                )
        };
        div()
            .id(("plan", ix))
            .key_context("Plan")
            .track_focus(&self.focus_handle)
            .flex_1()
            .min_h(px(160.))
            .flex()
            .flex_col()
            .border_t_1()
            .border_color(theme.border)
            .child(
                div()
                    .px_2()
                    .text_xs()
                    .text_color(if focused {
                        theme.foreground
                    } else {
                        theme.muted_foreground
                    })
                    .child(heading),
            )
            .child(body)
    }
}

/// Appends a row for `operator`, at `path` below the root, and for its subtree, parents first.
fn flatten(operator: &Operator, path: &mut Vec<usize>, rows: &mut Vec<Row>) {
    rows.push(Row {
        depth: path.len(),
        path: path.clone(),
    });
    for (ix, child) in operator.children.iter().enumerate() {
        path.push(ix);
        flatten(child, path, rows);
        path.pop();
    }
}

/// The operator at `path` below `root`.
fn at<'a>(root: &'a Operator, path: &[usize]) -> &'a Operator {
    path.iter().fold(root, |op, &ix| &op.children[ix])
}

/// One table row: the operator column indented by `depth`, the figures right-aligned.
fn row_cells(depth: usize, cells: [SharedString; 8]) -> gpui::Div {
    const WIDTHS: [f32; 7] = [180., 80., 80., 56., 52., 64., 64.];
    let [operator, rest @ ..] = cells;
    div()
        .flex()
        .flex_row()
        .px_2()
        .text_xs()
        .child(
            div()
                .flex_1()
                .min_w(px(200.))
                .pl(px(depth as f32 * INDENT))
                .overflow_hidden()
                .whitespace_nowrap()
                .child(operator),
        )
        .children(
            rest.into_iter()
                .zip(WIDTHS)
                .enumerate()
                .map(|(ix, (cell, width))| {
                    let cell = div()
                        .w(px(width))
                        .overflow_hidden()
                        .whitespace_nowrap()
                        .child(cell);
                    if ix == 0 {
                        cell
                    } else {
                        cell.flex().justify_end()
                    }
                }),
        )
}

/// A table or index as `schema.table`, with the index after it.
fn object_label(object: &Object) -> String {
    let table = [&object.schema, &object.table]
        .into_iter()
        .flatten()
        .cloned()
        .collect::<Vec<_>>()
        .join(".");
    match &object.index {
        Some(index) if table.is_empty() => index.clone(),
        Some(index) => format!("{table} ({index})"),
        None => table,
    }
}

/// A row count: whole numbers as they are, fractions to one place.
fn rows(count: f64) -> String {
    if count.fract() == 0.0 {
        format!("{count:.0}")
    } else {
        format!("{count:.1}")
    }
}
