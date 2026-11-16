//! PROTOTYPE. View B, "Tree table": one row per operator, indented under its parent and
//! expandable, with numeric columns a DBA scans down: estimated and actual rows, how far off
//! the estimate is (flagged at 10x), cost %, time and logical reads. Rows are tinted by
//! their share of the plan's cost, and the time cell by its share of the elapsed time.

use gpui::{Context, Div, FontWeight, Hsla, IntoElement, Window, div, hsla, img, prelude::*, px};
use gpui_component::ActiveTheme;

use crate::icons;
use crate::model;
use crate::plan_pane::{self, PlanPane};
use crate::view_graph::WARN_RED;

/// Column widths in px after the operator column, which takes the rest.
const COLS: [(&str, f32); 8] = [
    ("Object", 150.),
    ("Est rows", 72.),
    ("Act rows", 72.),
    ("Est/Act", 84.),
    ("Cost %", 86.),
    ("Time", 70.),
    ("Reads", 70.),
    ("", 24.),
];
const HEAT: f32 = 0.02;

fn cell(width: f32) -> Div {
    div()
        .w(px(width))
        .flex_none()
        .px_1()
        .overflow_hidden()
        .whitespace_nowrap()
}

fn heat(share: f64) -> Hsla {
    hsla(HEAT, 0.9, 0.55, (share as f32).clamp(0., 1.) * 0.45)
}

pub fn render(
    pane: &mut PlanPane,
    _window: &mut Window,
    cx: &mut Context<PlanPane>,
) -> impl IntoElement {
    let theme = cx.theme().clone();
    let rows = pane.visible_rows();
    let selected = pane.selection();
    let plan = pane.plan();
    let max_cost = plan.ops.iter().map(|o| o.cost_pct).fold(1., f64::max);
    let max_ms = plan.ops.iter().map(|o| o.self_ms).fold(0.001, f64::max);
    let depth = |mut i: usize| {
        let mut d = 0;
        while let Some(p) = plan.ops[i].parent {
            d += 1;
            i = p;
        }
        d
    };

    let header = div()
        .flex()
        .py_1()
        .border_b_1()
        .border_color(theme.border)
        .bg(theme.table_head)
        .text_xs()
        .font_weight(FontWeight::BOLD)
        .text_color(theme.muted_foreground)
        .child(div().flex_1().px_1().child("Operator"))
        .children(COLS.map(|(name, w)| {
            let numeric = !matches!(name, "Object" | "");
            cell(w).when(numeric, |d| d.text_right()).child(name)
        }));

    let body = rows.into_iter().map(|i| {
        let op = &plan.ops[i];
        let collapsed = pane.collapsed[pane.current].contains(&i);
        let has_kids = !op.children.is_empty();
        let is_sel = selected == Some(i);
        let (factor, over) = op.misestimate();
        let off = op.is_misestimated() && i != 0;
        let cost_share = op.cost_pct / max_cost;
        let arrow = if !has_kids {
            " "
        } else if collapsed {
            "▸"
        } else {
            "▾"
        };
        div()
            .id(("row", i))
            .flex()
            .items_center()
            .h(px(26.))
            .text_sm()
            .border_b_1()
            .border_color(theme.border.opacity(0.5))
            .bg(heat(cost_share))
            .when(is_sel, |d| {
                d.bg(theme.list_active)
                    .border_1()
                    .border_color(theme.list_active_border)
            })
            .child(
                div()
                    .flex_1()
                    .flex()
                    .items_center()
                    .gap_1()
                    .overflow_hidden()
                    .whitespace_nowrap()
                    .pl(px(4. + depth(i) as f32 * 16.))
                    .child(
                        div()
                            .id(("twisty", i))
                            .w(px(12.))
                            .text_color(theme.muted_foreground)
                            .child(arrow)
                            .on_click(cx.listener(move |this, _, _, cx| this.toggle_expand(i, cx))),
                    )
                    .child(img(icons::get(op.icon)).size(px(18.)).flex_none())
                    .child(div().font_weight(FontWeight::MEDIUM).child(op.name.clone()))
                    .when(!op.logical.is_empty() && op.logical != op.name, |d| {
                        d.child(
                            div()
                                .text_xs()
                                .text_color(theme.muted_foreground)
                                .child(op.logical.clone()),
                        )
                    })
                    .when(op.parallel, |d| {
                        d.child(
                            div()
                                .text_xs()
                                .text_color(theme.muted_foreground)
                                .child("‖"),
                        )
                    }),
            )
            .child(
                cell(COLS[0].1)
                    .text_xs()
                    .text_color(theme.muted_foreground)
                    .child(op.object.clone()),
            )
            .child(cell(COLS[1].1).text_right().child(model::rows(op.est_rows)))
            .child(cell(COLS[2].1).text_right().child(model::rows(op.act_rows)))
            .child(
                cell(COLS[3].1)
                    .text_right()
                    .when(off, |d| {
                        d.text_color(WARN_RED).font_weight(FontWeight::BOLD)
                    })
                    .when(!off, |d| d.text_color(theme.muted_foreground))
                    .child(if i == 0 {
                        String::new()
                    } else {
                        model::factor((factor, over))
                    }),
            )
            .child(
                cell(COLS[4].1).child(
                    div()
                        .flex()
                        .items_center()
                        .gap_1()
                        .justify_end()
                        .child(
                            div()
                                .h(px(6.))
                                .w(px((op.cost_pct as f32 * 0.4).clamp(0., 40.)))
                                .bg(heat(1.)),
                        )
                        .child(format!("{:.1}", op.cost_pct)),
                ),
            )
            .child(
                cell(COLS[5].1)
                    .text_right()
                    .bg(heat(op.self_ms / max_ms))
                    .child(model::ms(op.self_ms)),
            )
            .child(cell(COLS[6].1).text_right().child(model::rows(op.reads)))
            .child(
                cell(COLS[7].1)
                    .text_color(WARN_RED)
                    .child(if op.warnings.is_empty() { "" } else { "⚠" }),
            )
            .on_click(cx.listener(move |this, _, _, cx| this.select(i, cx)))
    });

    div()
        .size_full()
        .flex()
        .flex_col()
        .child(plan_pane::header(
            pane,
            "Up/Down move, Left/Right collapse/expand, Space toggles",
            cx,
        ))
        .child(header)
        .child(
            div()
                .id("table-body")
                .flex_1()
                .overflow_y_scroll()
                .children(body),
        )
}
