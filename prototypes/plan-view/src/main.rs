//! PROTOTYPE: throwaway plan view variants inside the chosen "Workbench" main window. Not
//! product code.
//!
//! Question: what should the execution plan view look like? Three structurally different
//! views of the same real plans: A "SSMS graph" (icon nodes right to left, thick arrows),
//! B "Tree table" (indented rows with numeric columns) and C "Icicle" (nested bars sized by
//! cost or time). All three sit in layout C's Plan tab and fill its right-hand inspector.
//!
//! Start with `--variant a|b|c`; F9 / Shift+F9 cycle views, F8 / Shift+F8 cycle the sample
//! plans (two SQL Server, one PostgreSQL). `--plan N` starts on plan N, `--select N` selects
//! operator N, `--metric time` sizes C by time, and `--keys f9,down` replays keys inside the
//! app 1.5 s apart, for checking without sending keys through the OS.
#![windows_subsystem = "windows"]

mod completion;
mod fake;
mod grid;
mod icons;
mod model;
mod parts;
mod pg_json;
mod plan_pane;
mod showplan;
mod view_graph;
mod view_icicle;
mod view_table;
mod workbench;

use std::rc::Rc;

use gpui::{
    App, AppContext, Bounds, Context, Entity, FocusHandle, Focusable, FontWeight, KeyBinding,
    Render, Window, WindowBounds, WindowOptions, actions, deferred, div, prelude::*, px, size,
};
use gpui_component::input::{EditorState, ShowCompletions};
use gpui_component::table::{TableEvent, TableState};
use gpui_component::tree::TreeState;
use gpui_component::{ActiveTheme, Root};

use crate::grid::FileRows;
use crate::model::Plan;
use crate::plan_pane::{Metric, PlanPane, View};

actions!(
    layout,
    [
        NextPane,
        PrevPane,
        FocusEditor,
        FocusResults,
        FocusMessages,
        FocusPlan,
        FocusExplorer,
        FocusInspector,
        NextTab,
        PrevTab,
        NextVariant,
        PrevVariant,
        ToggleEditMode,
        GridFirst,
        GridLast,
    ]
);

/// A focusable region of the window.
#[derive(Clone, Copy, PartialEq, Debug)]
pub enum Pane {
    TabList,
    Explorer,
    Editor,
    Results,
    Messages,
    Plan,
    Inspector,
}

/// Which view the lower middle area shows.
#[derive(Clone, Copy, PartialEq, Debug)]
pub enum Bottom {
    Results,
    Plan,
}

/// What the user selected last, for the inspector.
#[derive(Clone, Copy, PartialEq, Debug)]
pub enum Selection {
    Nothing,
    PlanNode,
    Object,
    Column(usize),
}

const PANES: [Pane; 7] = [
    Pane::TabList,
    Pane::Explorer,
    Pane::Editor,
    Pane::Messages,
    Pane::Results,
    Pane::Plan,
    Pane::Inspector,
];

/// The whole window's state.
pub struct Shell {
    /// One editor per query tab, in `fake::TABS` order, holding that tab's plan statement.
    pub editors: Vec<Entity<EditorState>>,
    pub active_tab: usize,
    pub table: Entity<TableState<FileRows>>,
    pub plan: Entity<PlanPane>,
    pub explorer: Entity<TreeState>,
    /// Wraps the explorer tree, whose own focus handle is private.
    pub explorer_focus: FocusHandle,
    pub bottom: Bottom,
    pub edit_mode: bool,
    pub selection: Selection,
    pub messages_focus: FocusHandle,
    pub inspector_focus: FocusHandle,
    pub tab_list_focus: FocusHandle,
    last_tree_selection: Option<usize>,
}

impl Shell {
    pub fn focus_handle_of(&self, pane: Pane, cx: &App) -> FocusHandle {
        match pane {
            Pane::TabList => self.tab_list_focus.clone(),
            Pane::Explorer => self.explorer_focus.clone(),
            Pane::Editor => self.editors[self.active_tab].focus_handle(cx),
            Pane::Results => self.table.focus_handle(cx),
            Pane::Messages => self.messages_focus.clone(),
            Pane::Plan => self.plan.focus_handle(cx),
            Pane::Inspector => self.inspector_focus.clone(),
        }
    }

    pub fn is_focused(&self, pane: Pane, window: &Window, cx: &App) -> bool {
        self.focus_handle_of(pane, cx).contains_focused(window, cx)
    }

    /// Shows `pane` (switching the lower area if needed), then focuses it.
    pub fn focus_pane(&mut self, pane: Pane, window: &mut Window, cx: &mut Context<Self>) {
        match pane {
            Pane::Results => self.bottom = Bottom::Results,
            Pane::Plan => self.bottom = Bottom::Plan,
            _ => {}
        }
        if pane == Pane::Explorer {
            self.explorer.update(cx, |t, cx| t.focus(window, cx));
        } else {
            let handle = self.focus_handle_of(pane, cx);
            window.focus(&handle, cx);
        }
        cx.notify();
    }

    fn cycle_pane(&mut self, delta: isize, window: &mut Window, cx: &mut Context<Self>) {
        let ix = PANES
            .iter()
            .position(|p| self.is_focused(*p, window, cx))
            .unwrap_or(0) as isize;
        let next = PANES[(ix + delta).rem_euclid(PANES.len() as isize) as usize];
        self.focus_pane(next, window, cx);
    }

    /// Switches to query tab `tab` and shows its plan, keeping focus where it was.
    pub fn select_tab(&mut self, tab: usize, cx: &mut Context<Self>) {
        self.active_tab = tab;
        self.plan.update(cx, |p, cx| {
            p.current = fake::TABS[tab].plan;
            p.pending_reveal = true;
            cx.notify();
        });
        if self.plan.read(cx).selection().is_some() {
            self.selection = Selection::PlanNode;
        }
        cx.notify();
    }

    fn cycle_tab(&mut self, delta: isize, cx: &mut Context<Self>) {
        let n = fake::TABS.len() as isize;
        self.select_tab(
            (self.active_tab as isize + delta).rem_euclid(n) as usize,
            cx,
        );
    }

    fn cycle_view(&mut self, delta: isize, cx: &mut Context<Self>) {
        self.plan.update(cx, |p, cx| {
            let ix = View::ALL.iter().position(|v| *v == p.view).unwrap_or(0) as isize;
            p.view = View::ALL[(ix + delta).rem_euclid(3) as usize];
            p.pending_reveal = true;
            cx.notify();
        });
        self.bottom = Bottom::Plan;
        cx.notify();
    }

    fn grid_jump(&mut self, last: bool, cx: &mut Context<Self>) {
        self.table.update(cx, |t, cx| {
            let col = t.selected_cell().map_or(0, |(_, c)| c);
            let row = if last { grid::ROWS - 1 } else { 0 };
            t.set_selected_cell(row, col, cx);
            t.scroll_to_row(row, cx);
        });
    }

    /// The explorer's selected item as (id, label).
    pub fn selected_object(&self, cx: &App) -> Option<(gpui::SharedString, gpui::SharedString)> {
        let entry = self.explorer.read(cx).selected_entry()?;
        Some((entry.item().id.clone(), entry.item().label.clone()))
    }
}

impl Render for Shell {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = cx.theme().clone();
        let pane = self.plan.read(cx);
        let pill = variant_pill(pane.view, pane.plan());
        let body = workbench::render(self, window, cx).into_any_element();
        div()
            .id("shell")
            .size_full()
            .relative()
            .bg(theme.background)
            .text_color(theme.foreground)
            .on_action(cx.listener(|this, _: &NextPane, w, cx| this.cycle_pane(1, w, cx)))
            .on_action(cx.listener(|this, _: &PrevPane, w, cx| this.cycle_pane(-1, w, cx)))
            .on_action(
                cx.listener(|this, _: &FocusEditor, w, cx| this.focus_pane(Pane::Editor, w, cx)),
            )
            .on_action(
                cx.listener(|this, _: &FocusResults, w, cx| this.focus_pane(Pane::Results, w, cx)),
            )
            .on_action(
                cx.listener(|this, _: &FocusMessages, w, cx| {
                    this.focus_pane(Pane::Messages, w, cx)
                }),
            )
            .on_action(cx.listener(|this, _: &FocusPlan, w, cx| this.focus_pane(Pane::Plan, w, cx)))
            .on_action(
                cx.listener(|this, _: &FocusExplorer, w, cx| {
                    this.focus_pane(Pane::Explorer, w, cx)
                }),
            )
            .on_action(cx.listener(|this, _: &FocusInspector, w, cx| {
                this.focus_pane(Pane::Inspector, w, cx)
            }))
            .on_action(cx.listener(|this, _: &NextTab, _, cx| this.cycle_tab(1, cx)))
            .on_action(cx.listener(|this, _: &PrevTab, _, cx| this.cycle_tab(-1, cx)))
            .on_action(cx.listener(|this, _: &NextVariant, _, cx| this.cycle_view(1, cx)))
            .on_action(cx.listener(|this, _: &PrevVariant, _, cx| this.cycle_view(-1, cx)))
            .on_action(cx.listener(|this, _: &ToggleEditMode, _, cx| {
                this.edit_mode = !this.edit_mode;
                cx.notify();
            }))
            .on_action(cx.listener(|this, _: &GridFirst, _, cx| this.grid_jump(false, cx)))
            .on_action(cx.listener(|this, _: &GridLast, _, cx| this.grid_jump(true, cx)))
            .child(body)
            .child(deferred(pill).with_priority(10))
    }
}

/// The prototype's switcher: a black pill at bottom centre, not part of any design.
fn variant_pill(view: View, plan: &Plan) -> gpui::Div {
    let engine = match plan.engine {
        model::Engine::SqlServer => "SQL Server",
        model::Engine::Postgres => "PostgreSQL",
    };
    div()
        .absolute()
        .bottom(px(44.))
        .left_0()
        .right_0()
        .flex()
        .justify_center()
        .child(
            div()
                .flex()
                .items_center()
                .gap_3()
                .px_4()
                .py_1()
                .rounded_full()
                .bg(gpui::black())
                .border_2()
                .border_color(gpui::yellow())
                .text_color(gpui::white())
                .text_sm()
                .shadow_lg()
                .child(div().text_color(gpui::yellow()).child("PROTOTYPE"))
                .child("◀ Shift+F9")
                .child(div().font_weight(FontWeight::BOLD).child(view.name()))
                .child("F9 ▶")
                .child(div().text_color(gpui::yellow()).child("|"))
                .child(
                    div()
                        .font_weight(FontWeight::BOLD)
                        .child(format!("{engine}: {}", plan.title)),
                )
                .child("F8 ▶"),
        )
}

/// Loads the bundled sample plans, in `fake::TABS` plan order.
fn load_plans() -> Vec<Plan> {
    let pg_query = include_str!("../plans/postgres-shop-query.sql");
    let statement = pg_query.find("SELECT").map_or(pg_query, |i| &pg_query[i..]);
    vec![
        showplan::parse(
            include_bytes!("../plans/row-count-spool-slow.sqlplan"),
            "row-count-spool-slow",
        ),
        showplan::parse(
            include_bytes!("../plans/serially-parallel.sqlplan"),
            "serially-parallel",
        ),
        pg_json::parse(
            include_str!("../plans/postgres-shop.json"),
            statement,
            "postgres-shop",
        ),
    ]
}

fn main() -> anyhow::Result<()> {
    let args: Vec<String> = std::env::args().collect();
    let arg_after = |flag: &str| {
        args.iter()
            .position(|a| a == flag)
            .and_then(|i| args.get(i + 1))
            .cloned()
    };
    let view = arg_after("--variant")
        .and_then(|v| View::parse(&v))
        .unwrap_or(View::Graph);
    let start_plan: usize = arg_after("--plan")
        .and_then(|n| n.parse().ok())
        .unwrap_or(0)
        .min(2);
    let select: Option<usize> = arg_after("--select").and_then(|n| n.parse().ok());
    let metric = if arg_after("--metric").as_deref() == Some("time") {
        Metric::Time
    } else {
        Metric::Cost
    };
    // Leaked once at startup: gpui's keystroke parser wants a 'static string.
    let keys: Option<&'static str> = arg_after("--keys").map(|k| &*k.leak());

    let dir = grid::data_dir();
    grid::ensure_data_file(&dir)?;
    let rows = FileRows::open(&dir)?;
    let plans = load_plans();

    gpui_platform::application().run(move |cx: &mut App| {
        gpui_component::init(cx);
        plan_pane::init(cx);
        cx.bind_keys([
            KeyBinding::new("f6", NextPane, None),
            KeyBinding::new("shift-f6", PrevPane, None),
            KeyBinding::new("ctrl-1", FocusEditor, None),
            KeyBinding::new("ctrl-2", FocusResults, None),
            KeyBinding::new("ctrl-3", FocusMessages, None),
            KeyBinding::new("ctrl-4", FocusPlan, None),
            KeyBinding::new("ctrl-5", FocusExplorer, None),
            KeyBinding::new("ctrl-6", FocusInspector, None),
            KeyBinding::new("ctrl-pagedown", NextTab, None),
            KeyBinding::new("ctrl-pageup", PrevTab, None),
            KeyBinding::new("f8", NextTab, None),
            KeyBinding::new("shift-f8", PrevTab, None),
            KeyBinding::new("f9", NextVariant, None),
            KeyBinding::new("shift-f9", PrevVariant, None),
            KeyBinding::new("ctrl-e", ToggleEditMode, None),
            KeyBinding::new("ctrl-space", ShowCompletions, Some("Input")),
            KeyBinding::new("ctrl-home", GridFirst, Some("DataTable")),
            KeyBinding::new("ctrl-end", GridLast, Some("DataTable")),
        ]);
        let bounds = Bounds::centered(None, size(px(1600.), px(1000.)), cx);
        let options = WindowOptions {
            window_bounds: Some(WindowBounds::Windowed(bounds)),
            titlebar: Some(gpui::TitlebarOptions {
                title: Some("Database IDE - plan view prototype".into()),
                ..Default::default()
            }),
            ..Default::default()
        };
        let opened = cx.open_window(options, |window, cx| {
            let editors: Vec<_> = fake::TABS
                .iter()
                .map(|t| {
                    let sql = plans[t.plan].statement.clone();
                    cx.new(|cx| {
                        let mut state = EditorState::new(window, cx)
                            .language("sql")
                            .default_value(sql);
                        state.lsp_mut().completion_provider =
                            Some(Rc::new(completion::SqlCompletion));
                        state
                    })
                })
                .collect();
            let table = cx.new(|cx| TableState::new(rows, window, cx).cell_selectable(true));
            let plan = cx.new(|cx| {
                let mut p = PlanPane::new(plans, cx);
                p.view = view;
                p.metric = metric;
                p
            });
            let explorer = cx.new(|cx| TreeState::new(cx).items(fake::explorer_items()));
            let view_entity = cx.new(|cx| {
                cx.on_focus_in(
                    &table.focus_handle(cx),
                    window,
                    |this: &mut Shell, _, cx| {
                        // The grid's arrow keys do nothing until a cell is selected.
                        this.table.update(cx, |t, cx| {
                            if t.selected_cell().is_none() {
                                t.set_selected_cell(0, 0, cx);
                            }
                        });
                    },
                )
                .detach();
                cx.subscribe(&table, |this: &mut Shell, _, event: &TableEvent, cx| {
                    if let TableEvent::SelectCell(_, col) | TableEvent::SelectColumn(col) = event {
                        this.selection = Selection::Column(*col);
                        cx.notify();
                    }
                })
                .detach();
                cx.observe(&plan, |this: &mut Shell, plan, cx| {
                    if plan.read(cx).selection().is_some() {
                        this.selection = Selection::PlanNode;
                    }
                    cx.notify();
                })
                .detach();
                cx.observe(&explorer, |this: &mut Shell, tree, cx| {
                    let now = tree.read(cx).selected_index();
                    if now != this.last_tree_selection {
                        this.last_tree_selection = now;
                        if now.is_some() {
                            this.selection = Selection::Object;
                        }
                    }
                    cx.notify();
                })
                .detach();
                let shell = Shell {
                    editors: editors.clone(),
                    active_tab: 0,
                    table,
                    plan: plan.clone(),
                    explorer,
                    explorer_focus: cx.focus_handle(),
                    bottom: Bottom::Plan,
                    edit_mode: false,
                    selection: Selection::Nothing,
                    messages_focus: cx.focus_handle(),
                    inspector_focus: cx.focus_handle(),
                    tab_list_focus: cx.focus_handle(),
                    last_tree_selection: None,
                };
                // Repaint the pane borders whenever focus moves into a pane.
                let mut handles: Vec<FocusHandle> =
                    editors.iter().map(|e| e.focus_handle(cx)).collect();
                handles.extend([
                    shell.table.focus_handle(cx),
                    plan.focus_handle(cx),
                    shell.explorer_focus.clone(),
                    shell.messages_focus.clone(),
                    shell.inspector_focus.clone(),
                    shell.tab_list_focus.clone(),
                ]);
                for handle in handles {
                    cx.on_focus_in(&handle, window, |_, _, cx| cx.notify())
                        .detach();
                }
                if let Some(keys) = keys {
                    let keystrokes: Vec<gpui::Keystroke> = keys
                        .split(',')
                        .filter_map(|k| gpui::Keystroke::parse(k.trim()).ok())
                        .collect();
                    // Checking only: replay keys through gpui's own dispatch, so no key goes
                    // through the OS to another window.
                    cx.spawn_in(window, async move |_, cx| {
                        let executor = cx.background_executor().clone();
                        for k in keystrokes {
                            executor.timer(std::time::Duration::from_millis(1500)).await;
                            let _ = cx.update(|window, cx| window.dispatch_keystroke(k, cx));
                        }
                    })
                    .detach();
                }
                shell
            });
            view_entity.update(cx, |shell, cx| {
                let tab = fake::TABS
                    .iter()
                    .position(|t| t.plan == start_plan)
                    .unwrap_or(0);
                shell.select_tab(tab, cx);
                if let Some(op) = select {
                    shell.plan.update(cx, |p, cx| {
                        let last = p.plan().ops.len() - 1;
                        p.select(op.min(last), cx);
                    });
                    shell.selection = Selection::PlanNode;
                }
                shell.focus_pane(Pane::Plan, window, cx);
            });
            cx.new(|cx| Root::new(view_entity, window, cx))
        });
        if opened.is_err() {
            cx.quit();
        }
        cx.activate(true);
    });
    Ok(())
}
