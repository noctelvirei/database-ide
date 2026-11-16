//! PROTOTYPE: throwaway layout variants of Database IDE's main window. Not product code.
//!
//! Three structurally different layouts of the same window, over the same fake content:
//! A "Classic" (SSMS-like docked explorer and bottom tabs), B "Focused" (command palette,
//! no permanent explorer, drawers) and C "Workbench" (vertical tab list, inspector column).
//! Start with `--variant a|b|c`; F9 / Shift+F9 cycle while running.
//!
//! Screenshot flags, used by `shots.ps1`: `--palette` opens B's command palette at start,
//! `--plan-node N` selects plan operator N and shows the plan.
#![windows_subsystem = "windows"]

mod completion;
mod fake;
mod grid;
mod parts;
mod plan;
mod variant_a;
mod variant_b;
mod variant_c;

use std::rc::Rc;

use gpui::{
    App, AppContext, Bounds, Context, Entity, FocusHandle, Focusable, FontWeight, KeyBinding,
    Render, Window, WindowBounds, WindowOptions, actions, deferred, div, prelude::*, px, size,
};
use gpui_component::command::CommandState;
use gpui_component::input::{EditorState, ShowCompletions};
use gpui_component::table::{TableEvent, TableState};
use gpui_component::tree::TreeState;
use gpui_component::{ActiveTheme, Root};

use crate::grid::FileRows;
use crate::plan::PlanView;

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
        TogglePalette,
        ToggleDrawer,
        ToggleEditMode,
        GridFirst,
        GridLast,
    ]
);

#[derive(Clone, Copy, PartialEq, Debug)]
pub enum Variant {
    A,
    B,
    C,
}

impl Variant {
    const ALL: [Variant; 3] = [Variant::A, Variant::B, Variant::C];

    fn name(self) -> &'static str {
        match self {
            Variant::A => "A  Classic",
            Variant::B => "B  Focused",
            Variant::C => "C  Workbench",
        }
    }

    fn parse(s: &str) -> Option<Self> {
        match s.to_ascii_lowercase().as_str() {
            "a" => Some(Variant::A),
            "b" => Some(Variant::B),
            "c" => Some(Variant::C),
            _ => None,
        }
    }
}

/// A focusable region of the window. Which panes exist depends on the variant.
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

/// Which bottom view a variant with a tabbed results area shows.
#[derive(Clone, Copy, PartialEq, Debug)]
pub enum Bottom {
    Results,
    Messages,
    Plan,
}

/// What the user selected last, for C's inspector.
#[derive(Clone, Copy, PartialEq, Debug)]
pub enum Selection {
    Nothing,
    PlanNode,
    Object,
    Column(usize),
}

/// The whole window's state. Every variant renders from it; none owns state of its own.
pub struct Shell {
    pub variant: Variant,
    /// One editor per query tab, in `fake::TABS` order.
    pub editors: Vec<Entity<EditorState>>,
    pub active_tab: usize,
    pub table: Entity<TableState<FileRows>>,
    pub plan: Entity<PlanView>,
    pub explorer: Entity<TreeState>,
    /// Wraps the explorer tree, whose own focus handle is private.
    pub explorer_focus: FocusHandle,
    pub palette: Entity<CommandState>,
    pub palette_open: bool,
    pub bottom: Bottom,
    /// B: whether the results drawer is up.
    pub drawer_open: bool,
    /// B: whether the plan side panel is open.
    pub plan_panel_open: bool,
    pub edit_mode: bool,
    pub selection: Selection,
    pub messages_focus: FocusHandle,
    pub inspector_focus: FocusHandle,
    pub tab_list_focus: FocusHandle,
    last_tree_selection: Option<usize>,
}

impl Shell {
    /// The panes F6 cycles through in the current variant, in visual order.
    fn panes(&self) -> &'static [Pane] {
        match self.variant {
            Variant::A => &[Pane::Explorer, Pane::Editor, Pane::Results, Pane::Messages, Pane::Plan],
            Variant::B => &[Pane::Editor, Pane::Results, Pane::Messages, Pane::Plan],
            Variant::C => &[
                Pane::TabList,
                Pane::Explorer,
                Pane::Editor,
                Pane::Messages,
                Pane::Results,
                Pane::Plan,
                Pane::Inspector,
            ],
        }
    }

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

    fn focused_pane(&self, window: &Window, cx: &App) -> Option<Pane> {
        self.panes().iter().copied().find(|p| self.is_focused(*p, window, cx))
    }

    /// Makes `pane` visible in the current variant (switching a bottom tab, raising a drawer
    /// or opening a side panel), then focuses it.
    pub fn focus_pane(&mut self, pane: Pane, window: &mut Window, cx: &mut Context<Self>) {
        match (self.variant, pane) {
            (Variant::B, Pane::Explorer) => return self.open_palette(window, cx),
            (Variant::B, Pane::Inspector) | (Variant::A, Pane::Inspector) => return,
            (Variant::A, Pane::TabList) | (Variant::B, Pane::TabList) => return,
            (Variant::B, Pane::Results) => (self.drawer_open, self.bottom) = (true, Bottom::Results),
            (Variant::B, Pane::Messages) => {
                (self.drawer_open, self.bottom) = (true, Bottom::Messages)
            }
            (Variant::B, Pane::Plan) => self.plan_panel_open = true,
            (_, Pane::Results) => self.bottom = Bottom::Results,
            (Variant::A, Pane::Messages) => self.bottom = Bottom::Messages,
            (_, Pane::Plan) => self.bottom = Bottom::Plan,
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
        let panes = self.panes();
        let ix = self
            .focused_pane(window, cx)
            .and_then(|p| panes.iter().position(|q| *q == p))
            .unwrap_or(0) as isize;
        let next = panes[(ix + delta).rem_euclid(panes.len() as isize) as usize];
        self.focus_pane(next, window, cx);
    }

    pub fn select_tab(&mut self, tab: usize, window: &mut Window, cx: &mut Context<Self>) {
        self.active_tab = tab;
        self.focus_pane(Pane::Editor, window, cx);
    }

    fn cycle_tab(&mut self, delta: isize, window: &mut Window, cx: &mut Context<Self>) {
        let n = fake::TABS.len() as isize;
        let next = (self.active_tab as isize + delta).rem_euclid(n) as usize;
        self.select_tab(next, window, cx);
    }

    fn cycle_variant(&mut self, delta: isize, window: &mut Window, cx: &mut Context<Self>) {
        let ix = Variant::ALL.iter().position(|v| *v == self.variant).unwrap_or(0) as isize;
        self.variant = Variant::ALL[(ix + delta).rem_euclid(3) as usize];
        self.palette_open = false;
        self.plan.update(cx, |p, _| p.scale = plan_scale(self.variant));
        self.focus_pane(Pane::Editor, window, cx);
    }

    pub fn open_palette(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.variant != Variant::B {
            return;
        }
        self.palette_open = true;
        self.palette.update(cx, |p, cx| {
            p.set_query("", window, cx);
            p.focus(window, cx);
        });
        cx.notify();
    }

    pub fn close_palette(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.palette_open = false;
        self.focus_pane(Pane::Editor, window, cx);
    }

    fn grid_jump(&mut self, last: bool, cx: &mut Context<Self>) {
        self.table.update(cx, |t, cx| {
            let col = t.selected_cell().map_or(0, |(_, c)| c);
            let row = if last { grid::ROWS - 1 } else { 0 };
            t.set_selected_cell(row, col, cx);
            t.scroll_to_row(row, cx);
        });
    }

    /// Reveals and selects the explorer item with `id`, and records it as the selection.
    pub fn reveal_object(&mut self, id: &str, window: &mut Window, cx: &mut Context<Self>) {
        let id: gpui::SharedString = id.to_string().into();
        self.explorer.update(cx, |t, cx| {
            t.reveal_item(&id, gpui::ScrollStrategy::Center, cx);
            let ix = t.index_of(&id);
            t.set_selected_index(ix, cx);
        });
        // B has no explorer pane: its side panel shows the selected object instead.
        if self.variant != Variant::B {
            self.focus_pane(Pane::Explorer, window, cx);
        }
        self.selection = Selection::Object;
        cx.notify();
    }
}

/// How large each variant paints the plan: it lives in narrower places in B and C.
fn plan_scale(variant: Variant) -> f32 {
    match variant {
        Variant::A => 1.,
        Variant::B => 0.62,
        Variant::C => 0.85,
    }
}

impl Render for Shell {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = cx.theme().clone();
        let body = match self.variant {
            Variant::A => variant_a::render(self, window, cx).into_any_element(),
            Variant::B => variant_b::render(self, window, cx).into_any_element(),
            Variant::C => variant_c::render(self, window, cx).into_any_element(),
        };
        div()
            .id("shell")
            .size_full()
            .relative()
            .bg(theme.background)
            .text_color(theme.foreground)
            .on_action(cx.listener(|this, _: &NextPane, w, cx| this.cycle_pane(1, w, cx)))
            .on_action(cx.listener(|this, _: &PrevPane, w, cx| this.cycle_pane(-1, w, cx)))
            .on_action(cx.listener(|this, _: &FocusEditor, w, cx| {
                this.focus_pane(Pane::Editor, w, cx)
            }))
            .on_action(cx.listener(|this, _: &FocusResults, w, cx| {
                this.focus_pane(Pane::Results, w, cx)
            }))
            .on_action(cx.listener(|this, _: &FocusMessages, w, cx| {
                this.focus_pane(Pane::Messages, w, cx)
            }))
            .on_action(cx.listener(|this, _: &FocusPlan, w, cx| this.focus_pane(Pane::Plan, w, cx)))
            .on_action(cx.listener(|this, _: &FocusExplorer, w, cx| {
                this.focus_pane(Pane::Explorer, w, cx)
            }))
            .on_action(cx.listener(|this, _: &FocusInspector, w, cx| {
                this.focus_pane(Pane::Inspector, w, cx)
            }))
            .on_action(cx.listener(|this, _: &NextTab, w, cx| this.cycle_tab(1, w, cx)))
            .on_action(cx.listener(|this, _: &PrevTab, w, cx| this.cycle_tab(-1, w, cx)))
            .on_action(cx.listener(|this, _: &NextVariant, w, cx| this.cycle_variant(1, w, cx)))
            .on_action(cx.listener(|this, _: &PrevVariant, w, cx| this.cycle_variant(-1, w, cx)))
            .on_action(cx.listener(|this, _: &TogglePalette, w, cx| {
                if this.palette_open {
                    this.close_palette(w, cx)
                } else {
                    this.open_palette(w, cx)
                }
            }))
            .on_action(cx.listener(|this, _: &ToggleDrawer, _, cx| {
                this.drawer_open = !this.drawer_open;
                cx.notify();
            }))
            .on_action(cx.listener(|this, _: &ToggleEditMode, _, cx| {
                this.edit_mode = !this.edit_mode;
                cx.notify();
            }))
            .on_action(cx.listener(|this, _: &GridFirst, _, cx| this.grid_jump(false, cx)))
            .on_action(cx.listener(|this, _: &GridLast, _, cx| this.grid_jump(true, cx)))
            .child(body)
            .child(deferred(variant_pill(self.variant)).with_priority(10))
    }
}

/// The prototype's variant switcher: a black pill at bottom centre, not part of any design.
fn variant_pill(variant: Variant) -> impl IntoElement {
    div().absolute().bottom(px(44.)).left_0().right_0().flex().justify_center().child(
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
            .child(div().font_weight(FontWeight::BOLD).child(variant.name()))
            .child("F9 ▶"),
    )
}

fn main() -> anyhow::Result<()> {
    let args: Vec<String> = std::env::args().collect();
    let arg_after = |flag: &str| {
        args.iter().position(|a| a == flag).and_then(|i| args.get(i + 1)).cloned()
    };
    let variant = arg_after("--variant").and_then(|v| Variant::parse(&v)).unwrap_or(Variant::A);
    let open_palette = args.iter().any(|a| a == "--palette");
    let plan_node: Option<usize> = arg_after("--plan-node").and_then(|n| n.parse().ok());

    let dir = grid::data_dir();
    grid::ensure_data_file(&dir)?;
    let rows = FileRows::open(&dir)?;

    gpui_platform::application().run(move |cx: &mut App| {
        gpui_component::init(cx);
        plan::init(cx);
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
            KeyBinding::new("ctrl-tab", NextTab, None),
            KeyBinding::new("ctrl-shift-tab", PrevTab, None),
            KeyBinding::new("f9", NextVariant, None),
            KeyBinding::new("shift-f9", PrevVariant, None),
            KeyBinding::new("ctrl-k", TogglePalette, None),
            KeyBinding::new("ctrl-j", ToggleDrawer, None),
            KeyBinding::new("ctrl-e", ToggleEditMode, None),
            KeyBinding::new("ctrl-space", ShowCompletions, Some("Input")),
            KeyBinding::new("ctrl-home", GridFirst, Some("DataTable")),
            KeyBinding::new("ctrl-end", GridLast, Some("DataTable")),
        ]);
        let bounds = Bounds::centered(None, size(px(1600.), px(1000.)), cx);
        let options = WindowOptions {
            window_bounds: Some(WindowBounds::Windowed(bounds)),
            titlebar: Some(gpui::TitlebarOptions {
                title: Some("Database IDE - layout prototype".into()),
                ..Default::default()
            }),
            ..Default::default()
        };
        let opened = cx.open_window(options, |window, cx| {
            let editors: Vec<_> = fake::TABS
                .iter()
                .map(|t| {
                    cx.new(|cx| {
                        let mut state =
                            EditorState::new(window, cx).language("sql").default_value(t.sql);
                        state.lsp_mut().completion_provider =
                            Some(Rc::new(completion::SqlCompletion));
                        state
                    })
                })
                .collect();
            let table = cx.new(|cx| TableState::new(rows, window, cx).cell_selectable(true));
            let plan = cx.new(PlanView::new);
            let explorer = cx.new(|cx| TreeState::new(cx).items(fake::explorer_items()));
            let palette = cx.new(|cx| CommandState::new(window, cx));
            let view = cx.new(|cx| {
                cx.on_focus_in(&table.focus_handle(cx), window, |this: &mut Shell, _, cx| {
                    // The grid's arrow keys do nothing until a cell is selected.
                    this.table.update(cx, |t, cx| {
                        if t.selected_cell().is_none() {
                            t.set_selected_cell(0, 0, cx);
                        }
                    });
                })
                .detach();
                cx.subscribe(&table, |this: &mut Shell, _, event: &TableEvent, cx| {
                    if let TableEvent::SelectCell(_, col) | TableEvent::SelectColumn(col) = event {
                        this.selection = Selection::Column(*col);
                        cx.notify();
                    }
                })
                .detach();
                cx.observe(&plan, |this: &mut Shell, plan, cx| {
                    if plan.read(cx).selected.is_some() {
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
                let mut shell = Shell {
                    variant,
                    editors: editors.clone(),
                    active_tab: 0,
                    table,
                    plan: plan.clone(),
                    explorer,
                    explorer_focus: cx.focus_handle(),
                    palette,
                    palette_open: false,
                    bottom: Bottom::Results,
                    drawer_open: true,
                    plan_panel_open: false,
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
                    cx.on_focus_in(&handle, window, |_, _, cx| cx.notify()).detach();
                }
                plan.update(cx, |p, _| p.scale = plan_scale(variant));
                if let Some(node) = plan_node {
                    plan.update(cx, |p, cx| p.select(node, cx));
                    shell.selection = Selection::PlanNode;
                    shell.bottom = Bottom::Plan;
                    shell.plan_panel_open = true;
                }
                shell
            });
            view.update(cx, |shell, cx| {
                if open_palette {
                    shell.open_palette(window, cx);
                } else if plan_node.is_some() {
                    shell.focus_pane(Pane::Plan, window, cx);
                } else {
                    shell.focus_pane(Pane::Editor, window, cx);
                }
            });
            cx.new(|cx| Root::new(view, window, cx))
        });
        if opened.is_err() {
            cx.quit();
        }
        cx.activate(true);
    });
    Ok(())
}
