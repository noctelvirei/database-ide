//! PROTOTYPE. A custom-drawn execution plan tree, laid out right to left as SSMS does: the
//! root operator sits at the left and data flows leftward from the leaves. Everything in the
//! pane is painted on one canvas; clicks are hit-tested against the node boxes painted last.

use std::cell::RefCell;
use std::rc::Rc;

use gpui::{
    App, Bounds, Context, FocusHandle, Focusable, Hsla, IntoElement, KeyBinding, MouseButton,
    MouseDownEvent, PathBuilder, Pixels, Render, SharedString, TextAlign, Window, actions, canvas,
    div, hsla, point, prelude::*, px, quad, size,
};
use gpui_component::ActiveTheme;

actions!(plan, [SelectPrev, SelectNext, SelectParent, SelectChild]);

const CONTEXT: &str = "PlanTree";
const BOX_W: f32 = 104.;
const BOX_H: f32 = 64.;
const COL_W: f32 = 150.;
const ROW_H: f32 = 84.;
const MARGIN: f32 = 16.;

pub fn init(cx: &mut App) {
    cx.bind_keys([
        KeyBinding::new("up", SelectPrev, Some(CONTEXT)),
        KeyBinding::new("down", SelectNext, Some(CONTEXT)),
        KeyBinding::new("left", SelectParent, Some(CONTEXT)),
        KeyBinding::new("right", SelectChild, Some(CONTEXT)),
    ]);
}

/// Picks the placeholder icon shape painted in a node box.
#[derive(Clone, Copy)]
enum Icon {
    Result,
    Join,
    Scan,
    Seek,
    Lookup,
    Sort,
    Aggregate,
    Compute,
    Spool,
    Parallel,
}

pub struct Node {
    pub name: &'static str,
    icon: Icon,
    /// This operator's share of the whole plan's estimated cost, in percent.
    pub cost_pct: u32,
    /// Rows this operator passes to its parent.
    pub rows: u64,
    pub object: &'static str,
    pub detail: &'static str,
    parent: Option<usize>,
    /// Column (depth) and row of the node in the layout grid.
    col: usize,
    row: usize,
}

/// Builds the sample plan. Each entry is (depth, name, icon, cost %, rows, object, detail),
/// in pre-order, so a node's parent is the nearest earlier entry one level shallower.
fn sample_plan() -> Vec<Node> {
    use Icon::*;
    let spec: [(usize, &str, Icon, u32, u64, &str, &str); 15] = [
        (0, "SELECT", Result, 0, 1_204, "", "Cached plan size 96 KB"),
        (1, "Parallelism (Gather)", Parallel, 2, 1_204, "", "Gather streams"),
        (2, "Sort", Sort, 7, 1_204, "", "ORDER BY o.created_at DESC"),
        (3, "Hash Match (Inner Join)", Join, 18, 1_204, "", "Hash keys: c.region_id"),
        (4, "Nested Loops (Inner Join)", Join, 4, 48_310, "", "Outer refs: o.order_id"),
        (5, "Index Seek", Seek, 12, 48_310, "orders.ix_created_at", "Seek: created_at > '2026-01-01'"),
        (5, "Key Lookup", Lookup, 21, 48_310, "orders.pk_orders", "Output: status, amount"),
        (4, "Hash Match (Aggregate)", Aggregate, 9, 412, "", "GROUP BY c.region_id"),
        (5, "Compute Scalar", Compute, 1, 250_000, "", "Expr1004 = upper(c.name)"),
        (6, "Merge Join (Inner Join)", Join, 3, 250_000, "", "Merge: c.customer_id = r.id"),
        (7, "Clustered Index Scan", Scan, 10, 250_000, "customers.pk_customers", "Ordered forward"),
        (7, "Table Spool (Lazy)", Spool, 2, 5_000, "", "Rebinds 1, rewinds 4"),
        (8, "Index Scan", Scan, 2, 5_000, "regions.ix_name", "Ordered forward"),
        (3, "Filter", Compute, 1, 9, "", "Predicate: r.country = 'NL'"),
        (4, "Clustered Index Scan", Scan, 8, 12, "warehouses.pk_warehouses", "Unordered"),
    ];
    let mut nodes: Vec<Node> = Vec::new();
    let mut next_row = 0;
    for (i, (depth, name, icon, cost_pct, rows, object, detail)) in spec.into_iter().enumerate() {
        let parent = (0..i).rev().find(|&j| spec[j].0 + 1 == depth);
        // A first child shares its parent's row; later siblings start a new row below.
        let row = match parent {
            Some(p) if nodes.iter().skip(p + 1).all(|n| n.parent != Some(p)) => nodes[p].row,
            _ if i == 0 => 0,
            _ => {
                next_row += 1;
                next_row
            }
        };
        next_row = next_row.max(row);
        nodes.push(Node { name, icon, cost_pct, rows, object, detail, parent, col: depth, row });
    }
    nodes
}

pub struct PlanView {
    pub nodes: Vec<Node>,
    pub selected: Option<usize>,
    focus: FocusHandle,
    /// Node boxes in window coordinates, written by the last paint, read by clicks.
    hit_boxes: Rc<RefCell<Vec<Bounds<Pixels>>>>,
}

impl PlanView {
    pub fn new(cx: &mut Context<Self>) -> Self {
        Self {
            nodes: sample_plan(),
            selected: None,
            focus: cx.focus_handle(),
            hit_boxes: Rc::default(),
        }
    }

    fn select(&mut self, ix: usize, cx: &mut Context<Self>) {
        self.selected = Some(ix);
        cx.notify();
    }

    fn on_mouse_down(&mut self, event: &MouseDownEvent, window: &mut Window, cx: &mut Context<Self>) {
        window.focus(&self.focus, cx);
        let hit = self.hit_boxes.borrow().iter().position(|b| b.contains(&event.position));
        if let Some(ix) = hit {
            self.select(ix, cx);
        }
    }

    fn step(&mut self, delta: isize, cx: &mut Context<Self>) {
        let last = self.nodes.len() as isize - 1;
        let ix = self.selected.map_or(0, |s| (s as isize + delta).clamp(0, last));
        self.select(ix as usize, cx);
    }
}

impl Focusable for PlanView {
    fn focus_handle(&self, _: &App) -> FocusHandle {
        self.focus.clone()
    }
}

fn icon_color(icon: Icon) -> Hsla {
    let hue = match icon {
        Icon::Result => 0.58,
        Icon::Join => 0.08,
        Icon::Scan => 0.33,
        Icon::Seek => 0.45,
        Icon::Lookup => 0.75,
        Icon::Sort => 0.15,
        Icon::Aggregate => 0.9,
        Icon::Compute => 0.0,
        Icon::Spool => 0.52,
        Icon::Parallel => 0.66,
    };
    hsla(hue, 0.6, 0.5, 1.)
}

/// Paints the placeholder icon for `icon` in the square `b`.
fn paint_icon(icon: Icon, b: Bounds<Pixels>, window: &mut Window) {
    let color = icon_color(icon);
    let (x, y, w) = (b.origin.x, b.origin.y, b.size.width);
    let polygon = |points: &[(f32, f32)], window: &mut Window| {
        let mut path = PathBuilder::fill();
        let pts: Vec<_> = points.iter().map(|(px_, py_)| point(x + w * *px_, y + w * *py_)).collect();
        path.add_polygon(&pts, true);
        if let Ok(path) = path.build() {
            window.paint_path(path, color);
        }
    };
    match icon {
        Icon::Seek | Icon::Lookup => {
            window.paint_quad(quad(b, w / 2., color, px(0.), color, Default::default()));
        }
        Icon::Sort => polygon(&[(0.5, 0.), (1., 1.), (0., 1.)], window),
        Icon::Join => {
            let half = size(w * 0.65, w * 0.65);
            window.paint_quad(gpui::fill(Bounds::new(b.origin, half), color));
            let o = point(x + w * 0.35, y + w * 0.35);
            window.paint_quad(gpui::fill(Bounds::new(o, half), color.opacity(0.6)));
        }
        Icon::Aggregate | Icon::Compute => {
            polygon(&[(0.5, 0.), (1., 0.5), (0.5, 1.), (0., 0.5)], window)
        }
        Icon::Parallel => polygon(&[(0., 0.), (1., 0.5), (0., 1.)], window),
        Icon::Result | Icon::Scan | Icon::Spool => {
            window.paint_quad(gpui::fill(b, color));
            for i in 1..4 {
                let line_y = y + w * (i as f32 / 4.);
                let line = Bounds::new(point(x, line_y), size(w, px(1.5)));
                window.paint_quad(gpui::fill(line, gpui::white().opacity(0.8)));
            }
        }
    }
}

fn paint_text(text: &str, origin: gpui::Point<Pixels>, font: Pixels, color: Hsla, window: &mut Window, cx: &mut App) {
    let mut run = window.text_style().to_run(text.len());
    run.color = color;
    let line = window
        .text_system()
        .shape_line(SharedString::from(text.to_string()), font, &[run], None);
    let _ = line.paint(origin, font * 1.3, TextAlign::Left, None, window, cx);
}

/// Arrow thickness in pixels for an edge carrying `rows` rows: logarithmic, 1 to 14 px.
fn arrow_width(rows: u64) -> f32 {
    (1. + (rows.max(1) as f32).log10() * 2.2).clamp(1., 14.)
}

impl Render for PlanView {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let is_focused = self.focus.contains_focused(window, cx);
        let theme = cx.theme();
        let (fg, muted, border, accent, bg) =
            (theme.foreground, theme.muted_foreground, theme.border, theme.primary, theme.background);
        let focused_border = if is_focused { accent } else { border };
        let layout: Vec<(usize, usize, Option<usize>)> =
            self.nodes.iter().map(|n| (n.col, n.row, n.parent)).collect();
        let labels: Vec<(Icon, &'static str, u32, u64)> =
            self.nodes.iter().map(|n| (n.icon, n.name, n.cost_pct, n.rows)).collect();
        let selected = self.selected;
        let hit_boxes = self.hit_boxes.clone();

        div()
            .id("plan")
            .key_context(CONTEXT)
            .track_focus(&self.focus)
            .size_full()
            .bg(bg)
            .border_2()
            .border_color(focused_border)
            .on_mouse_down(MouseButton::Left, cx.listener(Self::on_mouse_down))
            .on_action(cx.listener(|this, _: &SelectPrev, _, cx| this.step(-1, cx)))
            .on_action(cx.listener(|this, _: &SelectNext, _, cx| this.step(1, cx)))
            .on_action(cx.listener(|this, _: &SelectParent, _, cx| {
                if let Some(p) = this.selected.and_then(|s| this.nodes[s].parent) {
                    this.select(p, cx);
                }
            }))
            .on_action(cx.listener(|this, _: &SelectChild, _, cx| {
                let s = this.selected.unwrap_or(0);
                if let Some(c) = this.nodes.iter().position(|n| n.parent == Some(s)) {
                    this.select(c, cx);
                }
            }))
            .child(
                canvas(
                    |_, _, _| {},
                    move |bounds, _, window, cx| {
                        let origin = bounds.origin;
                        let node_box = |col: usize, row: usize| {
                            Bounds::new(
                                point(
                                    origin.x + px(MARGIN + col as f32 * COL_W),
                                    origin.y + px(MARGIN + row as f32 * ROW_H),
                                ),
                                size(px(BOX_W), px(BOX_H)),
                            )
                        };
                        // Arrows first, so boxes paint over their ends. Each arrow runs from
                        // the child's left edge to the parent's right edge, elbowing halfway.
                        for (i, (col, row, parent)) in layout.iter().enumerate() {
                            let Some(p) = parent else { continue };
                            let child = node_box(*col, *row);
                            let parent_box = node_box(layout[*p].0, layout[*p].1);
                            let t = px(arrow_width(labels[i].3));
                            let y1 = child.origin.y + px(BOX_H / 2.);
                            let y0 = parent_box.origin.y + px(BOX_H / 2.);
                            let x1 = child.origin.x;
                            let x0 = parent_box.origin.x + px(BOX_W) + px(10.);
                            let xm = x0 + (x1 - x0) / 2.;
                            let color = muted;
                            window.paint_quad(gpui::fill(
                                Bounds::from_corners(point(xm, y1 - t / 2.), point(x1, y1 + t / 2.)),
                                color,
                            ));
                            window.paint_quad(gpui::fill(
                                Bounds::from_corners(point(xm - t / 2., y0 - t / 2.), point(xm + t / 2., y1 + t / 2.)),
                                color,
                            ));
                            window.paint_quad(gpui::fill(
                                Bounds::from_corners(point(x0, y0 - t / 2.), point(xm, y0 + t / 2.)),
                                color,
                            ));
                            let head = (t * 0.8).max(px(5.));
                            let mut path = PathBuilder::fill();
                            path.add_polygon(
                                &[
                                    point(x0 - px(10.), y0),
                                    point(x0, y0 - head),
                                    point(x0, y0 + head),
                                ],
                                true,
                            );
                            if let Ok(path) = path.build() {
                                window.paint_path(path, color);
                            }
                        }
                        let mut boxes = Vec::with_capacity(layout.len());
                        for (i, (col, row, _)) in layout.iter().enumerate() {
                            let b = node_box(*col, *row);
                            let is_sel = selected == Some(i);
                            window.paint_quad(quad(
                                b,
                                px(4.),
                                bg,
                                px(if is_sel { 2.5 } else { 1. }),
                                if is_sel { accent } else { border },
                                Default::default(),
                            ));
                            let icon = Bounds::new(
                                point(b.origin.x + px(BOX_W / 2. - 12.), b.origin.y + px(5.)),
                                size(px(24.), px(24.)),
                            );
                            paint_icon(labels[i].0, icon, window);
                            let (name, cost) = (labels[i].1, labels[i].2);
                            let short: String = name.chars().take(17).collect();
                            paint_text(&short, point(b.origin.x + px(4.), b.origin.y + px(32.)), px(11.), fg, window, cx);
                            paint_text(
                                &format!("Cost: {cost} %"),
                                point(b.origin.x + px(4.), b.origin.y + px(47.)),
                                px(11.),
                                if cost >= 15 { hsla(0., 0.7, 0.55, 1.) } else { muted },
                                window,
                                cx,
                            );
                            boxes.push(b);
                        }
                        *hit_boxes.borrow_mut() = boxes;
                    },
                )
                .size_full(),
            )
    }
}
