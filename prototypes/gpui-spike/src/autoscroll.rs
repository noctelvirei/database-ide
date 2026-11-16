//! PROTOTYPE. Scrolls the results grid from the first row to the last by itself and records
//! the interval between frames, so scrolling smoothness has numbers rather than an opinion.
//!
//! `SPIKE_AUTOSCROLL_MODE` picks how the grid is moved, one step per frame:
//! - `jump` (default): `TableState::scroll_to_row`, `SPIKE_AUTOSCROLL_STEP` rows per frame.
//! - `wheel`: a scroll-wheel event dispatched over the grid, `STEP` rows' worth of pixels.
//! - `drag`: the vertical scrollbar thumb pressed and dragged down `STEP` pixels per frame.
//! `wheel` and `drag` go through gpui's input dispatch and the table's own handlers.

use std::cell::Cell;
use std::rc::Rc;
use std::time::{Duration, Instant};

use gpui::{
    App, Bounds, Entity, Modifiers, MouseButton, MouseDownEvent, MouseMoveEvent, MouseUpEvent,
    Pixels, PlatformInput, ScrollDelta, ScrollEvent, Window, point, px,
};
use gpui_component::table::TableState;

use crate::grid::{self, FileRows};

/// Grid row height in pixels at the default size, as drawn.
const ROW_PX: f32 = 32.;
/// Stop after this many frames whatever happens, so a stuck scroll still reports.
const MAX_FRAMES: usize = 6_000;

#[derive(Clone, Copy, PartialEq)]
enum Mode {
    Jump,
    Wheel,
    Drag,
}

struct Run {
    out: String,
    mode: Mode,
    step: usize,
    row: usize,
    /// Pointer y while dragging the scrollbar thumb.
    drag_y: Option<Pixels>,
    frames: usize,
    last: Option<Instant>,
    intervals_ms: Vec<f64>,
    started: Instant,
}

/// Starts the scroll one second from now if `SPIKE_AUTOSCROLL` names an output file.
/// `grid_bounds` holds the grid pane's bounds as last painted.
pub fn start(
    table: Entity<TableState<FileRows>>,
    grid_bounds: Rc<Cell<Bounds<Pixels>>>,
    window: &mut Window,
    cx: &mut App,
) {
    let Ok(out) = std::env::var("SPIKE_AUTOSCROLL") else { return };
    let mode = match std::env::var("SPIKE_AUTOSCROLL_MODE").as_deref() {
        Ok("wheel") => Mode::Wheel,
        Ok("drag") => Mode::Drag,
        _ => Mode::Jump,
    };
    let default_step = if mode == Mode::Drag { 1 } else { 997 };
    let step = std::env::var("SPIKE_AUTOSCROLL_STEP")
        .ok()
        .and_then(|s| s.parse().ok())
        .unwrap_or(default_step);
    let run = Run {
        out,
        mode,
        step,
        row: 0,
        drag_y: None,
        frames: 0,
        last: None,
        intervals_ms: Vec::new(),
        started: Instant::now(),
    };
    let handle = window.window_handle();
    cx.spawn(async move |cx| {
        cx.background_executor().timer(Duration::from_secs(1)).await;
        let _ = handle.update(cx, |_, window, _| frame(table, grid_bounds, run, window));
    })
    .detach();
}

fn last_visible_row(table: &Entity<TableState<FileRows>>, cx: &App) -> usize {
    table.read(cx).visible_range().rows().end
}

fn frame(
    table: Entity<TableState<FileRows>>,
    grid_bounds: Rc<Cell<Bounds<Pixels>>>,
    mut r: Run,
    window: &mut Window,
) {
    window.on_next_frame(move |window, cx| {
        let now = Instant::now();
        if let Some(last) = r.last {
            r.intervals_ms.push(now.duration_since(last).as_secs_f64() * 1000.);
        }
        if r.last.is_none() {
            r.started = now;
        }
        r.last = Some(now);
        r.frames += 1;
        let at_end = last_visible_row(&table, cx) >= grid::ROWS;
        let finished = match r.mode {
            Mode::Jump => r.row >= grid::ROWS,
            Mode::Wheel | Mode::Drag => at_end,
        };
        if finished || r.frames > MAX_FRAMES {
            if let Some(y) = r.drag_y {
                let up = MouseUpEvent {
                    button: MouseButton::Left,
                    position: point(grid_bounds.get().right() - px(6.), y),
                    modifiers: Modifiers::default(),
                    click_count: 1,
                };
                window.dispatch_event(PlatformInput::MouseUp(up), cx);
            }
            write_stats(&r, last_visible_row(&table, cx), grid_bounds.get());
            return;
        }
        let b = grid_bounds.get();
        match r.mode {
            Mode::Jump => {
                let row = r.row.min(grid::ROWS - 1);
                table.update(cx, |t, cx| t.scroll_to_row(row, cx));
                r.row = if row == grid::ROWS - 1 { grid::ROWS } else { r.row + r.step };
            }
            Mode::Wheel | Mode::Drag => {
                // Frame 1 only moves the pointer over the grid: gpui routes wheel events to the
                // hovered element and shows the scrollbar on hover. The thumb starts just under
                // the header row, at the grid's right edge.
                let x = b.right() - px(5.);
                let y = r.drag_y.unwrap_or(b.top() + px(ROW_PX + 6.));
                let wheel_at = b.center();
                let pressed = (r.mode == Mode::Drag && r.frames > 2).then_some(MouseButton::Left);
                let position = if r.mode == Mode::Wheel { wheel_at } else { point(x, y) };
                if r.frames == 1 || r.mode == Mode::Drag {
                    let moved = MouseMoveEvent { position, pressed_button: pressed, modifiers: Modifiers::default() };
                    window.dispatch_event(PlatformInput::MouseMove(moved), cx);
                }
                if r.frames > 1 && r.mode == Mode::Wheel {
                    let event = ScrollEvent {
                        position: wheel_at,
                        delta: ScrollDelta::Pixels(point(px(0.), px(-(r.step as f32) * ROW_PX))),
                        ..Default::default()
                    };
                    window.dispatch_event(PlatformInput::Scroll(event), cx);
                }
                if r.mode == Mode::Drag {
                    if r.frames == 2 {
                        let down = MouseDownEvent {
                            button: MouseButton::Left,
                            position,
                            modifiers: Modifiers::default(),
                            click_count: 1,
                            first_mouse: false,
                        };
                        window.dispatch_event(PlatformInput::MouseDown(down), cx);
                    }
                    r.drag_y = Some(if r.frames > 2 { y + px(r.step as f32) } else { y });
                }
            }
        }
        window.refresh();
        frame(table, grid_bounds, r, window);
    });
}

fn write_stats(r: &Run, last_row: usize, grid: Bounds<Pixels>) {
    let mut v = r.intervals_ms.clone();
    v.sort_by(|a, b| a.total_cmp(b));
    let pct = |p: f64| v.get(((v.len() as f64 - 1.) * p).round() as usize).copied().unwrap_or(0.);
    let over = |ms: f64| v.iter().filter(|x| **x > ms).count();
    let mode = match r.mode {
        Mode::Jump => "jump",
        Mode::Wheel => "wheel",
        Mode::Drag => "drag",
    };
    let text = format!(
        "{{\"mode\":\"{mode}\",\"grid\":\"{grid:?}\",\"step\":{},\"frames\":{},\"seconds\":{:.1},\"last_visible_row_end\":{last_row},\"p50_ms\":{:.2},\"p95_ms\":{:.2},\"p99_ms\":{:.2},\"max_ms\":{:.2},\"over_25ms\":{},\"over_50ms\":{}}}\n",
        r.step,
        v.len(),
        r.last.map_or(0., |l| l.duration_since(r.started).as_secs_f64()),
        pct(0.5),
        pct(0.95),
        pct(0.99),
        v.last().copied().unwrap_or(0.),
        over(25.),
        over(50.)
    );
    let _ = std::fs::write(&r.out, text);
}
