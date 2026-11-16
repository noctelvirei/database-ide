//! PROTOTYPE: throwaway one-day spike for the UI toolkit decision (issue #8). Not product code.
//!
//! One window with every feature loaded at startup: a SQL editor with completion, a results
//! grid paged from a 1,000,000-row file on disk, a custom-drawn plan tree and a details panel.
//!
//! Environment variables used by the measurement scripts:
//! - `BENCH_FIRST_FRAME`: file to write the epoch ms of the first `render()` call to.
//! - `SPIKE_DUMP`: file F12 writes the app state to, as JSON, for the keyboard script.
//! - `SPIKE_AUTOSCROLL`: file to write frame-time statistics to after scrolling the grid
//!   from the first row to the last; see `autoscroll.rs` for the modes.
//! - `SPIKE_KEYSCRIPT`: file to append the keyboard script's per-step state to; runs it.
//! - `SPIKE_DATA_DIR`: folder for the data file (default: a folder in the temp directory).
#![windows_subsystem = "windows"]

mod autoscroll;
mod completion;
mod grid;
mod keyscript;
mod plan;

use std::cell::Cell;
use std::rc::Rc;
use std::time::{SystemTime, UNIX_EPOCH};

use gpui::{
    App, AppContext, Bounds, Context, Pixels, canvas, Entity, FocusHandle, Focusable, KeyBinding, Render,
    SharedString, Window, WindowBounds, WindowOptions, actions, div, prelude::*, px, size,
};
use gpui_component::input::{Editor, EditorState, ShowCompletions};
use gpui_component::table::{DataTable, TableState};
use gpui_component::{ActiveTheme, Root};

use crate::grid::FileRows;
use crate::plan::PlanView;

actions!(
    spike,
    [NextPane, PrevPane, FocusEditor, FocusGrid, FocusPlan, FocusDetails, GridFirst, GridLast, DumpState]
);

const SAMPLE_SQL: &str = "SELECT o.order_id, o.created_at, c.name, r.name AS region
FROM orders AS o
JOIN customers AS c ON c.customer_id = o.customer_id
JOIN regions AS r ON r.region_id = c.region_id
WHERE o.created_at > '2026-01-01'
  AND o.status = 'paid'
ORDER BY o.created_at DESC;
";

#[derive(Clone, Copy, PartialEq, Debug)]
enum Pane {
    Editor,
    Grid,
    Plan,
    Details,
}

const PANES: [Pane; 4] = [Pane::Editor, Pane::Grid, Pane::Plan, Pane::Details];

pub(crate) struct Workbench {
    editor: Entity<EditorState>,
    table: Entity<TableState<FileRows>>,
    pub(crate) plan: Entity<PlanView>,
    details_focus: FocusHandle,
    /// The grid pane's bounds as last painted, for the scroll measurement.
    grid_bounds: Rc<Cell<Bounds<Pixels>>>,
    first_frame_written: bool,
}

impl Workbench {
    fn focus_handle_of(&self, pane: Pane, cx: &App) -> FocusHandle {
        match pane {
            Pane::Editor => self.editor.focus_handle(cx),
            Pane::Grid => self.table.focus_handle(cx),
            Pane::Plan => self.plan.focus_handle(cx),
            Pane::Details => self.details_focus.clone(),
        }
    }

    fn focused_pane(&self, window: &Window, cx: &App) -> Option<Pane> {
        PANES
            .into_iter()
            .find(|p| self.focus_handle_of(*p, cx).contains_focused(window, cx))
    }

    fn focus_pane(&mut self, pane: Pane, window: &mut Window, cx: &mut Context<Self>) {
        let handle = self.focus_handle_of(pane, cx);
        window.focus(&handle, cx);
        cx.notify();
    }

    fn cycle(&mut self, delta: isize, window: &mut Window, cx: &mut Context<Self>) {
        let current = self.focused_pane(window, cx).unwrap_or(Pane::Details);
        let ix = PANES.iter().position(|p| *p == current).unwrap_or(0) as isize;
        let next = PANES[(ix + delta).rem_euclid(PANES.len() as isize) as usize];
        self.focus_pane(next, window, cx);
    }

    fn grid_jump(&mut self, last: bool, cx: &mut Context<Self>) {
        self.table.update(cx, |t, cx| {
            let col = t.selected_cell().map_or(0, |(_, c)| c);
            let row = if last { grid::ROWS - 1 } else { 0 };
            t.set_selected_cell(row, col, cx);
            t.scroll_to_row(row, cx);
        });
    }

    /// Writes the state the keyboard script checks to `SPIKE_DUMP`, as JSON.
    fn dump_state(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Ok(path) = std::env::var("SPIKE_DUMP") else { return };
        let _ = std::fs::write(path, self.state_json(window, cx));
    }

    /// The state the keyboard script checks, as one line of JSON.
    pub(crate) fn state_json(&mut self, window: &mut Window, cx: &mut Context<Self>) -> String {
        let pane = self.focused_pane(window, cx);
        let editor = self.editor.read(cx);
        let search = editor.search_session();
        let completion = editor.completion_menu_state();
        let table = self.table.read(cx);
        let plan = self.plan.read(cx);
        format!(
            "{{\"focused_pane\":\"{:?}\",\"editor_text\":{:?},\"cursor\":{},\"completion_open\":{},\"completion_items\":{:?},\"search_open\":{},\"replace_mode\":{},\"search_query\":{:?},\"grid_selected_cell\":{:?},\"grid_pages_read\":{},\"grid_cached_pages\":{},\"plan_selected\":{:?}}}\n",
            pane,
            editor.value().to_string(),
            editor.cursor(),
            completion.open,
            completion.items.iter().take(8).map(|i| i.label.clone()).collect::<Vec<_>>(),
            search.open,
            search.replace_mode,
            search.query,
            table.selected_cell(),
            table.delegate().pages_read,
            table.delegate().cached_pages(),
            plan.selected.map(|i| plan.nodes[i].name),
        )
    }
}

fn pane_frame(focused: bool, cx: &App) -> gpui::Div {
    let theme = cx.theme();
    div()
        .border_2()
        .border_color(if focused { plan::FOCUS_BLUE } else { theme.border })
        .overflow_hidden()
}

impl Render for Workbench {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        if !self.first_frame_written {
            self.first_frame_written = true;
            if let Ok(path) = std::env::var("BENCH_FIRST_FRAME") {
                let ms = SystemTime::now().duration_since(UNIX_EPOCH).map_or(0, |d| d.as_millis());
                let _ = std::fs::write(path, ms.to_string());
            }
            autoscroll::start(self.table.clone(), self.grid_bounds.clone(), window, cx);
            if let Ok(out) = std::env::var("SPIKE_KEYSCRIPT") {
                cx.spawn_in(window, async move |this, cx| keyscript::start(out, this, cx).await)
                    .detach();
            }
        }
        let focused = self.focused_pane(window, cx);
        let theme = cx.theme().clone();
        let plan = self.plan.read(cx);
        let details: Vec<(SharedString, SharedString)> = match plan.selected {
            Some(i) => {
                let n = &plan.nodes[i];
                vec![
                    ("Operator".into(), n.name.into()),
                    ("Estimated cost".into(), format!("{} %", n.cost_pct).into()),
                    ("Rows".into(), n.rows.to_string().into()),
                    ("Object".into(), if n.object.is_empty() { "-" } else { n.object }.into()),
                    ("Detail".into(), n.detail.into()),
                ]
            }
            None => vec![("".into(), "Select a plan operator (Ctrl+3, arrow keys)".into())],
        };

        div()
            .id("workbench")
            .size_full()
            .flex()
            .flex_col()
            .bg(theme.background)
            .text_color(theme.foreground)
            .on_action(cx.listener(|this, _: &NextPane, w, cx| this.cycle(1, w, cx)))
            .on_action(cx.listener(|this, _: &PrevPane, w, cx| this.cycle(-1, w, cx)))
            .on_action(cx.listener(|this, _: &FocusEditor, w, cx| this.focus_pane(Pane::Editor, w, cx)))
            .on_action(cx.listener(|this, _: &FocusGrid, w, cx| this.focus_pane(Pane::Grid, w, cx)))
            .on_action(cx.listener(|this, _: &FocusPlan, w, cx| this.focus_pane(Pane::Plan, w, cx)))
            .on_action(cx.listener(|this, _: &FocusDetails, w, cx| this.focus_pane(Pane::Details, w, cx)))
            .on_action(cx.listener(|this, _: &GridFirst, _, cx| this.grid_jump(false, cx)))
            .on_action(cx.listener(|this, _: &GridLast, _, cx| this.grid_jump(true, cx)))
            .on_action(cx.listener(|this, _: &DumpState, w, cx| this.dump_state(w, cx)))
            .capture_key_down(|event, _, _| {
                // Measurement only: log every key the window receives.
                if let Ok(dump) = std::env::var("SPIKE_DUMP") {
                    use std::io::Write as _;
                    if let Ok(mut f) = std::fs::OpenOptions::new().create(true).append(true).open(format!("{dump}.keys.txt")) {
                        let _ = writeln!(f, "{}", format!("{:?}", event.keystroke));
                    }
                }
            })
            .child(
                div()
                    .px_2()
                    .py_1()
                    .text_sm()
                    .text_color(theme.muted_foreground)
                    .child("PROTOTYPE gpui spike  |  F6 / Shift+F6 or Ctrl+1..4: switch pane  |  Ctrl+Space: completion  |  Ctrl+F / Ctrl+H: find / replace  |  Ctrl+Alt+Down: add cursor"),
            )
            .child(
                pane_frame(focused == Some(Pane::Editor), cx)
                    .h(px(230.))
                    .child(Editor::new(&self.editor).h_full()),
            )
            .child(
                pane_frame(focused == Some(Pane::Grid), cx)
                    .flex_1()
                    .relative()
                    .child(
                        DataTable::new(&self.table)
                            .stripe(true)
                            .scrollbar_mode(gpui_component::scroll::ScrollbarMode::Always),
                    )
                    .child({
                        let grid_bounds = self.grid_bounds.clone();
                        canvas(move |b, _, _| grid_bounds.set(b), |_, _, _, _| {})
                            .absolute()
                            .top_0()
                            .left_0()
                            .size_full()
                    }),
            )
            .child(
                div()
                    .h(px(400.))
                    .flex()
                    .flex_row()
                    .child(div().flex_1().child(self.plan.clone()))
                    .child(
                        pane_frame(focused == Some(Pane::Details), cx)
                            .id("details")
                            .track_focus(&self.details_focus)
                            .w(px(340.))
                            .p_2()
                            .flex()
                            .flex_col()
                            .gap_1()
                            .child(div().font_weight(gpui::FontWeight::BOLD).child("Operator details"))
                            .children(details.into_iter().map(|(k, v)| {
                                div()
                                    .flex()
                                    .flex_col()
                                    .child(div().text_xs().text_color(theme.muted_foreground).child(k))
                                    .child(div().text_sm().child(v))
                            })),
                    ),
            )
    }
}

fn main() -> anyhow::Result<()> {
    let dir = grid::data_dir();
    if std::env::args().any(|a| a == "--generate") {
        return grid::ensure_data_file(&dir);
    }
    grid::ensure_data_file(&dir)?;
    let rows = FileRows::open(&dir)?;
    // The window has no console, so panics go to a file next to the state dump.
    if let Ok(dump) = std::env::var("SPIKE_DUMP") {
        std::panic::set_hook(Box::new(move |info| {
            let bt = std::backtrace::Backtrace::force_capture();
            let _ = std::fs::write(format!("{dump}.panic.txt"), format!("{info}
{bt}"));
        }));
    }

    gpui_platform::application().run(move |cx: &mut App| {
        gpui_component::init(cx);
        plan::init(cx);
        cx.bind_keys([
            KeyBinding::new("f6", NextPane, None),
            KeyBinding::new("shift-f6", PrevPane, None),
            KeyBinding::new("ctrl-1", FocusEditor, None),
            KeyBinding::new("ctrl-2", FocusGrid, None),
            KeyBinding::new("ctrl-3", FocusPlan, None),
            KeyBinding::new("ctrl-4", FocusDetails, None),
            KeyBinding::new("f12", DumpState, None),
            KeyBinding::new("ctrl-space", ShowCompletions, Some("Input")),
            KeyBinding::new("ctrl-home", GridFirst, Some("DataTable")),
            KeyBinding::new("ctrl-end", GridLast, Some("DataTable")),
        ]);
        let bounds = Bounds::centered(None, size(px(1500.), px(1000.)), cx);
        let options = WindowOptions {
            window_bounds: Some(WindowBounds::Windowed(bounds)),
            ..Default::default()
        };
        let opened = cx.open_window(options, |window, cx| {
            let editor = cx.new(|cx| {
                let mut state = EditorState::new(window, cx).language("sql").default_value(SAMPLE_SQL);
                state.lsp_mut().completion_provider = Some(Rc::new(completion::SqlCompletion));
                state
            });
            let table = cx.new(|cx| TableState::new(rows, window, cx).cell_selectable(true));
            let plan = cx.new(PlanView::new);
            let details_focus = cx.focus_handle();
            let view = cx.new(|cx| {
                // Repaint the pane borders whenever focus moves between panes.
                for handle in [
                    editor.focus_handle(cx),
                    table.focus_handle(cx),
                    plan.focus_handle(cx),
                    details_focus.clone(),
                ] {
                    cx.on_focus_in(&handle, window, |_, _, cx| cx.notify()).detach();
                }
                // The grid's arrow keys do nothing until a cell is selected, so select the
                // first cell when the grid first gets focus.
                cx.on_focus_in(&table.focus_handle(cx), window, |this: &mut Workbench, _, cx| {
                    this.table.update(cx, |t, cx| {
                        if t.selected_cell().is_none() {
                            t.set_selected_cell(0, 0, cx);
                        }
                    });
                })
                .detach();
                cx.observe(&plan, |_, _, cx| cx.notify()).detach();
                Workbench {
                    editor: editor.clone(),
                    table,
                    plan,
                    details_focus,
                    grid_bounds: Rc::default(),
                    first_frame_written: false,
                }
            });
            editor.update(cx, |e, cx| e.focus(window, cx));
            cx.new(|cx| Root::new(view, window, cx))
        });
        if opened.is_err() {
            cx.quit();
        }
        cx.activate(true);
    });
    Ok(())
}
