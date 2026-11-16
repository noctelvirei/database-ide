//! PROTOTYPE. View C, "Icicle": the statement is a full-width bar on top; each operator's
//! inputs sit below it, sharing its width in proportion to their inclusive cost or time
//! (T switches), largest first from the left. Colour shows an operator's own share, so a hot
//! leaf stands out even under a wide parent. A strip below lists the hottest operators.

use gpui::{Context, FontWeight, Hsla, IntoElement, Window, div, hsla, prelude::*, px, relative};
use gpui_component::ActiveTheme;

use crate::model;
use crate::plan_pane::{self, Metric, PlanPane};
use crate::view_graph::WARN_RED;

const ROW_H: f32 = 30.;
const SELECTED: Hsla = Hsla {
    h: 0.6,
    s: 0.85,
    l: 0.5,
    a: 1.,
};

/// A bar: operator, depth (row), and left edge and width as fractions of the pane.
struct Bar {
    op: usize,
    depth: usize,
    start: f64,
    width: f64,
}

fn layout(pane: &PlanPane) -> Vec<Bar> {
    let mut bars = Vec::new();
    let mut stack = vec![(0usize, 0usize, 0f64, 1f64)];
    while let Some((op, depth, start, width)) = stack.pop() {
        bars.push(Bar {
            op,
            depth,
            start,
            width,
        });
        let kids = pane.hottest_children(op);
        let total: f64 = kids.iter().map(|&k| pane.size_of(k)).sum();
        // Children overlap in time under parallelism, so they can add up to more than the
        // parent; then they share the parent's width instead of overflowing it.
        let scale = width / pane.size_of(op).max(total).max(f64::MIN_POSITIVE);
        let mut x = start;
        for k in kids {
            let w = pane.size_of(k) * scale;
            stack.push((k, depth + 1, x, w));
            x += w;
        }
    }
    bars
}

/// An operator's own share of the whole plan, 0..=1, by the current metric.
fn own_share(pane: &PlanPane, op: usize) -> f64 {
    let o = &pane.plan().ops[op];
    match pane.metric {
        Metric::Cost => o.cost_pct / 100.,
        Metric::Time => o.self_ms / pane.size_of(0).max(f64::MIN_POSITIVE),
    }
}

/// Pale yellow for cold operators through orange to red for the hottest.
fn heat(share: f64) -> Hsla {
    let s = share.clamp(0., 1.).sqrt() as f32;
    hsla(0.13 - 0.13 * s, 0.85, 0.86 - 0.36 * s, 1.)
}

pub fn render(
    pane: &mut PlanPane,
    _window: &mut Window,
    cx: &mut Context<PlanPane>,
) -> impl IntoElement {
    let theme = cx.theme().clone();
    let bars = layout(pane);
    let depth = bars.iter().map(|b| b.depth).max().unwrap_or(0) + 1;
    let selected = pane.selection();
    let plan = pane.plan();
    let root_size = pane.size_of(0);
    let metric_name = match pane.metric {
        Metric::Cost => "estimated cost",
        Metric::Time => "actual time",
    };

    let bar_elements = bars.iter().filter(|b| b.width > 0.0005).map(|b| {
        let i = b.op;
        let op = &plan.ops[i];
        let share = own_share(pane, i);
        let fill = heat(share);
        let text = if fill.l < 0.6 {
            gpui::white()
        } else {
            gpui::black()
        };
        let off = op.is_misestimated() && i != 0;
        let (factor, over) = op.misestimate();
        let size = pane.size_of(i);
        let amount = match pane.metric {
            Metric::Cost => format!("{:.1}%", size),
            Metric::Time => model::ms(size),
        };
        let mut label = String::new();
        if !op.warnings.is_empty() {
            label.push_str("⚠ ");
        }
        label.push_str(&op.label());
        label.push_str("  ");
        label.push_str(&amount);
        if off {
            label.push_str(&format!("  {}", model::factor((factor, over))));
        }
        let is_sel = selected == Some(i);
        div()
            .id(("bar", i))
            .absolute()
            .top(px(b.depth as f32 * ROW_H))
            .left(relative(b.start as f32))
            .w(relative(b.width as f32))
            .h(px(ROW_H))
            .p(px(1.))
            .child(
                div()
                    .size_full()
                    .rounded(px(3.))
                    .bg(fill)
                    .text_color(text)
                    .text_xs()
                    .px_1()
                    .flex()
                    .items_center()
                    .overflow_hidden()
                    .whitespace_nowrap()
                    .when(off, |d| d.border_b(px(4.)).border_color(WARN_RED))
                    .when(is_sel, |d| {
                        d.border_2()
                            .border_color(SELECTED)
                            .font_weight(FontWeight::BOLD)
                    })
                    .child(label),
            )
            .on_click(cx.listener(move |this, _, _, cx| this.select(i, cx)))
    });

    // The five operators with the largest own share, as a jump list.
    let mut hottest: Vec<usize> = (1..plan.ops.len()).collect();
    hottest.sort_by(|a, b| own_share(pane, *b).total_cmp(&own_share(pane, *a)));
    let hot_list = div()
        .flex()
        .flex_wrap()
        .items_center()
        .gap_2()
        .px_2()
        .py_1()
        .border_t_1()
        .border_color(theme.border)
        .text_xs()
        .child(
            div()
                .text_color(theme.muted_foreground)
                .child(format!("Hottest by own {metric_name}:")),
        )
        .children(hottest.into_iter().take(5).map(|i| {
            let op = &plan.ops[i];
            let share = own_share(pane, i);
            div()
                .id(("hot", i))
                .px_1()
                .rounded(px(3.))
                .bg(heat(share))
                .text_color(if heat(share).l < 0.6 {
                    gpui::white()
                } else {
                    gpui::black()
                })
                .child(format!("{}  {:.0}%", op.name, share * 100.))
                .on_click(cx.listener(move |this, _, _, cx| this.select(i, cx)))
        }));

    let hint = format!(
        "Width = inclusive {metric_name} ({} total), T switches  ·  Up parent, Down hottest input, Left/Right siblings",
        match pane.metric {
            Metric::Cost => "100%".to_string(),
            Metric::Time => model::ms(root_size),
        }
    );
    div()
        .size_full()
        .flex()
        .flex_col()
        .child(plan_pane::header(pane, &hint, cx))
        .child(
            div()
                .id("icicle-scroll")
                .flex_1()
                .overflow_y_scroll()
                .p_2()
                .child(
                    div()
                        .relative()
                        .w_full()
                        .h(px(depth as f32 * ROW_H))
                        .children(bar_elements),
                ),
        )
        .child(hot_list)
}
