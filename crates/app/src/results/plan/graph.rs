//! A plan as an SSMS-style graph (docs/spec.md, "UI" → "Plans"): each operator a box with its
//! icon, name, object, cost and rows; the statement's root on the left, its inputs to the
//! right, each child's rows flowing left along an elbowed arrow into its parent. An arrow's
//! width grows with the rows it carries: actual rows for an actual plan, estimated otherwise.
//!
//! Columns are tree depth; rows of the graph are the leaves in order, each parent level with
//! its first input, as SSMS draws it.

use gpui::{
    Context, InteractiveElement as _, ParentElement as _, Pixels, Point,
    StatefulInteractiveElement as _, Styled as _, div, img, prelude::*, px,
};
use gpui_component::ActiveTheme as _;

use super::{MISESTIMATE, PlanView, Row, icons, object_label, rows};
use crate::results::Results;

/// One operator box's width and height.
const BOX_WIDTH: f32 = 150.;
const BOX_HEIGHT: f32 = 120.;
/// The distance from one column's or graph row's start to the next's.
const COLUMN: f32 = 200.;
const LINE: f32 = 140.;
/// The empty margin around the graph.
const MARGIN: f32 = 12.;
/// The icon's side, and how far its centre (where arrows meet the box) sits below the top.
const ICON: f32 = 32.;
const ARROW_Y: f32 = 4. + ICON / 2.;

/// Where each operator's box goes, by row index: its column (tree depth) and graph row.
pub(super) struct Layout {
    places: Vec<(usize, usize)>,
    /// Each row's parent row; `None` for the root.
    parents: Vec<Option<usize>>,
    columns: usize,
    lines: usize,
}

impl Layout {
    /// Lays out `rows`, which are in tree order with parents before their children.
    pub(super) fn new(rows: &[Row]) -> Layout {
        let mut parents = Vec::with_capacity(rows.len());
        let mut open: Vec<usize> = Vec::new();
        for (ix, row) in rows.iter().enumerate() {
            open.truncate(row.depth);
            parents.push(open.last().copied());
            open.push(ix);
        }
        // Leaves take the graph rows in order; a parent sits level with its first input,
        // which follows it directly in tree order.
        let mut lines = vec![0; rows.len()];
        let mut next = 0;
        for ix in 0..rows.len() {
            let leaf = rows.get(ix + 1).is_none_or(|n| n.depth <= rows[ix].depth);
            if leaf {
                lines[ix] = next;
                next += 1;
            }
        }
        for ix in (0..rows.len()).rev() {
            if rows.get(ix + 1).is_some_and(|n| n.depth > rows[ix].depth) {
                lines[ix] = lines[ix + 1];
            }
        }
        Layout {
            places: rows.iter().zip(lines).map(|(r, l)| (r.depth, l)).collect(),
            parents,
            columns: rows.iter().map(|r| r.depth + 1).max().unwrap_or(0),
            lines: next,
        }
    }

    /// Row `ix`'s parent row, `None` for the root.
    pub(super) fn parent(&self, ix: usize) -> Option<usize> {
        self.parents.get(ix).copied().flatten()
    }

    /// Row `ix`'s box's top-left corner within the graph.
    fn corner(&self, ix: usize) -> (f32, f32) {
        let (column, line) = self.places[ix];
        (MARGIN + column as f32 * COLUMN, MARGIN + line as f32 * LINE)
    }

    /// The whole graph's width and height.
    fn size(&self) -> (f32, f32) {
        (
            2. * MARGIN + self.columns.saturating_sub(1) as f32 * COLUMN + BOX_WIDTH,
            2. * MARGIN + self.lines.saturating_sub(1) as f32 * LINE + BOX_HEIGHT,
        )
    }
}

/// An arrow's width for `rows` rows: one pixel for a row or none, about two more for each
/// tenfold, at most twelve.
fn arrow_width(rows: f64) -> f32 {
    (1. + 2. * rows.max(1.).log10() as f32).min(12.)
}

/// A line of the arrow, `width` thick, from `(x, y)` running `length` across or down.
fn segment(x: f32, y: f32, length: f32, width: f32, across: bool) -> gpui::Div {
    let (w, h) = if across {
        (length, width)
    } else {
        (width, length)
    };
    let (dx, dy) = if across {
        (0., width / 2.)
    } else {
        (width / 2., 0.)
    };
    div()
        .absolute()
        .left(px(x - dx))
        .top(px(y - dy))
        .w(px(w.max(1.)))
        .h(px(h.max(1.)))
}

impl PlanView {
    /// Scrolls the graph so the selected operator's box is in view.
    pub(super) fn reveal(&self) {
        let layout = &self.layout;
        let (x, y) = layout.corner(self.selected);
        let view = self.graph_scroll.bounds().size;
        let offset = self.graph_scroll.offset();
        let mut to: Point<Pixels> = offset;
        let fit = |start: f32, length: f32, view: Pixels, offset: Pixels| -> Pixels {
            let shown = -f32::from(offset);
            let view = f32::from(view);
            if start < shown {
                px(-start + MARGIN)
            } else if start + length > shown + view {
                px(-(start + length - view + MARGIN))
            } else {
                offset
            }
        };
        to.x = fit(x, BOX_WIDTH, view.width, offset.x).min(px(0.));
        to.y = fit(y, BOX_HEIGHT, view.height, offset.y).min(px(0.));
        if to != offset {
            self.graph_scroll.set_offset(to);
        }
    }

    /// The graph of this plan's operators. `ix` is the plan's section in the results pane,
    /// where a click on a box selects it.
    pub(super) fn graph(
        &self,
        ix: usize,
        focused: bool,
        cx: &mut Context<Results>,
    ) -> gpui::Stateful<gpui::Div> {
        let theme = cx.theme().clone();
        let layout = &self.layout;
        let mut arrows = Vec::new();
        for (child, row) in self.rows.iter().enumerate() {
            let Some(parent) = layout.parent(child) else {
                continue;
            };
            let op = self.operator(row);
            let carried = if self.actual {
                op.actual_rows.unwrap_or(0.)
            } else {
                op.estimated_rows_total
            };
            let width = arrow_width(carried);
            let (cx_, cy) = layout.corner(child);
            let (px_, py) = layout.corner(parent);
            let (from, to) = (px_ + BOX_WIDTH, cx_);
            let elbow = from + (to - from) / 2.;
            let (cy, py) = (cy + ARROW_Y, py + ARROW_Y);
            let colour = theme.muted_foreground;
            arrows.push(segment(elbow, cy, to - elbow, width, true).bg(colour));
            arrows.push(segment(elbow, cy.min(py), (cy - py).abs(), width, false).bg(colour));
            arrows.push(segment(from + 6., py, elbow - from - 6., width, true).bg(colour));
            // The head, pointing into the parent.
            arrows.push(
                div()
                    .absolute()
                    .left(px(from))
                    .top(px(py - 7.))
                    .text_size(px(10.))
                    .text_color(colour)
                    .child("◀"),
            );
        }
        let boxes = self.rows.iter().enumerate().map(|(row_ix, row)| {
            let op = self.operator(row);
            let (x, y) = layout.corner(row_ix);
            let error = op.estimate_error();
            let figures = match op.actual_rows {
                Some(actual) => {
                    format!("{} of {} rows", rows(actual), rows(op.estimated_rows_total))
                }
                None => format!("{} rows", rows(op.estimated_rows_total)),
            };
            let selected = row_ix == self.selected;
            div()
                .id(("operator-box", row_ix))
                .absolute()
                .left(px(x))
                .top(px(y))
                .w(px(BOX_WIDTH))
                .h(px(BOX_HEIGHT))
                .p_1()
                .flex()
                .flex_col()
                .items_center()
                .rounded_sm()
                .text_xs()
                .when(selected, |b| {
                    b.bg(if focused {
                        theme.accent
                    } else {
                        theme.secondary
                    })
                })
                .child(
                    img(icons::for_operator(op, self.savable && row_ix == 0))
                        .size(px(ICON))
                        .flex_none(),
                )
                .child(
                    div()
                        .max_w_full()
                        .flex_none()
                        .overflow_hidden()
                        .whitespace_nowrap()
                        .when(row_ix == self.costliest, |d| {
                            d.font_weight(gpui::FontWeight::BOLD)
                        })
                        .child(op.name.clone()),
                )
                .children(op.object.as_ref().map(|object| {
                    div()
                        .max_w_full()
                        .flex_none()
                        .overflow_hidden()
                        .whitespace_nowrap()
                        .text_color(theme.muted_foreground)
                        .child(object_label(object))
                }))
                .child(format!("Cost: {:.0}%", op.cost_percent))
                .child(
                    div()
                        .when(error.is_some_and(|e| e >= MISESTIMATE), |d| {
                            d.text_color(theme.warning)
                        })
                        .child(figures),
                )
                .on_click(cx.listener(move |results, _, window, cx| {
                    results.click_operator(ix, row_ix, window, cx);
                }))
        });
        let (width, height) = layout.size();
        div()
            .id(("plan-graph", ix))
            .flex_1()
            .min_h_0()
            .overflow_scroll()
            .track_scroll(&self.graph_scroll)
            .child(
                div()
                    .relative()
                    .w(px(width))
                    .h(px(height))
                    .children(arrows)
                    .children(boxes),
            )
    }
}
