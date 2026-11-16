//! PROTOTYPE. View A, "SSMS graph": operators as icon nodes laid out right to left (the
//! statement at the far left, data flowing leftward from the leaves), arrows as thick as the
//! actual rows they carry, cost and "actual of estimated" under each node, warning and
//! parallelism badges on the icon. The graph scrolls in both directions inside the pane.

use gpui::{
    App, Bounds, Context, FontWeight, Hsla, IntoElement, PathBuilder, Pixels, SharedString,
    TextAlign, Window, canvas, div, img, point, prelude::*, px,
};
use gpui_component::ActiveTheme;

use crate::icons;
use crate::model::{self, Plan};
use crate::plan_pane::{self, PlanPane};

const BOX_W: f32 = 128.;
const BOX_H: f32 = 104.;
const COL_W: f32 = 178.;
const ROW_H: f32 = 124.;
const MARGIN: f32 = 16.;
/// Accent for the selected node; matches the layout prototype's focus colour.
const SELECTED: Hsla = Hsla {
    h: 0.6,
    s: 0.85,
    l: 0.5,
    a: 1.,
};
pub const WARN_RED: Hsla = Hsla {
    h: 0.,
    s: 0.75,
    l: 0.45,
    a: 1.,
};

/// Grid cell (column = depth, row) of each operator. A first child shares its parent's row;
/// later siblings start below everything already placed, so subtrees never overlap.
fn layout(plan: &Plan) -> Vec<(usize, usize)> {
    let mut cells = vec![(0, 0); plan.ops.len()];
    let mut next_row = 0;
    fn place(
        plan: &Plan,
        op: usize,
        col: usize,
        row: usize,
        cells: &mut [(usize, usize)],
        next_row: &mut usize,
    ) {
        cells[op] = (col, row);
        *next_row = (*next_row).max(row);
        for (i, &c) in plan.ops[op].children.iter().enumerate() {
            let r = if i == 0 { row } else { *next_row + 1 };
            place(plan, c, col + 1, r, cells, next_row);
        }
    }
    place(plan, 0, 0, 0, &mut cells, &mut next_row);
    cells
}

fn box_origin((col, row): (usize, usize)) -> (f32, f32) {
    (MARGIN + col as f32 * COL_W, MARGIN + row as f32 * ROW_H)
}

/// Arrow thickness for `rows` rows: logarithmic, 1 to 14 px.
fn arrow_width(rows: f64) -> f32 {
    (1. + (rows.max(1.) as f32).log10() * 1.8).clamp(1., 14.)
}

/// Scrolls the graph so operator `op` is fully visible.
fn scroll_into_view(pane: &PlanPane, op: usize) {
    let (x, y) = box_origin(layout(pane.plan())[op]);
    let handle = &pane.graph_scroll;
    let view = handle.bounds().size;
    if view.width <= px(0.) {
        return;
    }
    let mut off = handle.offset();
    let (left, top) = (px(x - MARGIN), px(y - MARGIN));
    let (right, bottom) = (px(x + BOX_W + MARGIN), px(y + BOX_H + MARGIN));
    if left < -off.x {
        off.x = -left;
    } else if right > -off.x + view.width {
        off.x = -(right - view.width);
    }
    if top < -off.y {
        off.y = -top;
    } else if bottom > -off.y + view.height {
        off.y = -(bottom - view.height);
    }
    handle.set_offset(off);
}

fn paint_text(
    text: &str,
    origin: gpui::Point<Pixels>,
    color: Hsla,
    window: &mut Window,
    cx: &mut App,
) {
    let font = px(11.);
    let mut run = window.text_style().to_run(text.len());
    run.color = color;
    let line =
        window
            .text_system()
            .shape_line(SharedString::from(text.to_string()), font, &[run], None);
    let _ = line.paint(origin, font * 1.3, TextAlign::Left, None, window, cx);
}

pub fn render(
    pane: &mut PlanPane,
    window: &mut Window,
    cx: &mut Context<PlanPane>,
) -> impl IntoElement {
    let theme = cx.theme().clone();
    if pane.pending_reveal {
        // The scroll area's size is known only after its first layout; until then, retry.
        if pane.graph_scroll.bounds().size.width > px(0.) {
            pane.pending_reveal = false;
            if let Some(op) = pane.selection() {
                scroll_into_view(pane, op);
            }
        } else {
            window.request_animation_frame();
        }
    }
    let plan = pane.plan();
    let cells = layout(plan);
    let selected = pane.selection();
    let cols = cells.iter().map(|c| c.0).max().unwrap_or(0) + 1;
    let rows = cells.iter().map(|c| c.1).max().unwrap_or(0) + 1;
    let (content_w, content_h) = (
        MARGIN * 2. + cols as f32 * COL_W,
        MARGIN * 2. + rows as f32 * ROW_H,
    );

    // Edges as (child box origin, parent box origin, thickness, rows label).
    let edges: Vec<((f32, f32), (f32, f32), f32, String)> = plan
        .ops
        .iter()
        .enumerate()
        .filter_map(|(i, op)| {
            let p = op.parent?;
            Some((
                box_origin(cells[i]),
                box_origin(cells[p]),
                arrow_width(op.act_rows),
                model::rows(op.act_rows),
            ))
        })
        .collect();
    let (edge_colour, label_colour) = (theme.muted_foreground.opacity(0.7), theme.muted_foreground);
    let arrows = canvas(
        |_, _, _| {},
        move |bounds, _, window, cx| {
            let o = bounds.origin;
            for ((cx0, cy0), (px0, py0), t, label) in &edges {
                let t = px(*t);
                // Icons sit at the top of each box; arrows join icon centres, as in SSMS.
                let y1 = o.y + px(cy0 + 20.);
                let y0 = o.y + px(py0 + 20.);
                let x1 = o.x + px(*cx0 + BOX_W / 2. - 22.);
                let x0 = o.x + px(px0 + BOX_W / 2. + 30.);
                let xm = x0 + (x1 - x0) * 0.35;
                let fill = |a: gpui::Point<Pixels>, b: gpui::Point<Pixels>, w: &mut Window| {
                    w.paint_quad(gpui::fill(Bounds::from_corners(a, b), edge_colour));
                };
                fill(point(xm, y1 - t / 2.), point(x1, y1 + t / 2.), window);
                fill(
                    point(xm - t / 2., y0.min(y1) - t / 2.),
                    point(xm + t / 2., y0.max(y1) + t / 2.),
                    window,
                );
                fill(point(x0, y0 - t / 2.), point(xm, y0 + t / 2.), window);
                let head = (t * 0.9).max(px(5.));
                let mut path = PathBuilder::fill();
                path.add_polygon(
                    &[
                        point(x0 - px(9.), y0),
                        point(x0, y0 - head),
                        point(x0, y0 + head),
                    ],
                    true,
                );
                if let Ok(path) = path.build() {
                    window.paint_path(path, edge_colour);
                }
                paint_text(
                    label,
                    point(xm + px(6.), y1 - t / 2. - px(15.)),
                    label_colour,
                    window,
                    cx,
                );
            }
        },
    )
    .absolute()
    .top_0()
    .left_0()
    .w(px(content_w))
    .h(px(content_h));

    let nodes = plan.ops.iter().enumerate().map(|(i, op)| {
        let (x, y) = box_origin(cells[i]);
        let is_sel = selected == Some(i);
        let (factor, over) = op.misestimate();
        let off = op.is_misestimated();
        let icon = div()
            .relative()
            .size(px(36.))
            .child(img(icons::get(op.icon)).size(px(36.)))
            .when(op.parallel, |d| {
                d.child(
                    img(icons::get("overlay-parallelism"))
                        .absolute()
                        .left(px(-6.))
                        .bottom(px(-4.))
                        .size(px(16.)),
                )
            })
            .when(!op.warnings.is_empty(), |d| {
                d.child(
                    img(icons::get("overlay-warning"))
                        .absolute()
                        .right(px(-6.))
                        .bottom(px(-4.))
                        .size(px(16.)),
                )
            });
        let rows_line = if i == 0 {
            format!("{} rows", model::rows(op.act_rows))
        } else {
            format!(
                "{} of {} ({:.0}%)",
                model::rows(op.act_rows),
                model::rows(op.est_rows),
                100. * op.act_rows / op.est_rows.max(1.)
            )
        };
        div()
            .id(("op", i))
            .absolute()
            .left(px(x))
            .top(px(y))
            .w(px(BOX_W))
            .h(px(BOX_H))
            .flex()
            .flex_col()
            .items_center()
            .px_1()
            .pt_1()
            .rounded(px(4.))
            .border_color(if is_sel {
                SELECTED
            } else {
                gpui::transparent_black()
            })
            .border_2()
            .when(is_sel, |d| d.bg(SELECTED.opacity(0.08)))
            .text_xs()
            .line_height(px(14.))
            .child(icon)
            .child(
                div()
                    .mt_1()
                    .font_weight(FontWeight::BOLD)
                    .text_center()
                    .child(op.name.clone()),
            )
            .when(!op.logical.is_empty() && op.logical != op.name, |d| {
                d.child(
                    div()
                        .text_color(theme.muted_foreground)
                        .text_center()
                        .child(format!("({})", op.logical)),
                )
            })
            .child(
                div()
                    .text_color(if op.cost_pct >= 25. {
                        WARN_RED
                    } else {
                        theme.foreground
                    })
                    .child(format!("Cost: {:.0} %", op.cost_pct)),
            )
            .child(
                div()
                    .text_color(theme.muted_foreground)
                    .child(model::ms(op.total_ms)),
            )
            .child(
                div()
                    .when(off, |d| {
                        d.text_color(WARN_RED).font_weight(FontWeight::BOLD)
                    })
                    .when(!off, |d| d.text_color(theme.muted_foreground))
                    .child(rows_line)
                    .when(off, |d| {
                        d.child(format!(" {}", model::factor((factor, over))))
                    }),
            )
            .on_click(cx.listener(move |this, _, _, cx| this.select(i, cx)))
    });

    div()
        .size_full()
        .flex()
        .flex_col()
        .child(plan_pane::header(
            pane,
            "Click or arrows: Left = toward the result, Right = into inputs, Up/Down = step",
            cx,
        ))
        .child(
            div()
                .id("graph-scroll")
                .flex_1()
                .overflow_scroll()
                .track_scroll(&pane.graph_scroll)
                .child(
                    div()
                        .relative()
                        .w(px(content_w))
                        .h(px(content_h))
                        .child(arrows)
                        .children(nodes),
                ),
        )
}
