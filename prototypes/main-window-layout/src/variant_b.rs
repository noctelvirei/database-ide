//! PROTOTYPE. Variant B, "Focused": no permanent explorer. A command palette (Ctrl+K) jumps to
//! query tabs, connections and objects. The editor takes the full width under a stripe in the
//! connection colour; results sit in a bottom drawer (Ctrl+J), and the plan or a jumped-to
//! object opens in a right-hand side panel.

use gpui::{
    App, Context, FontWeight, IntoElement, SharedString, Window, deferred, div, hsla, prelude::*,
    px,
};
use gpui_component::ActiveTheme;
use gpui_component::IndexPath;
use gpui_component::command::{Command, CommandGroup, CommandItem};
use gpui_component::input::Editor;
use gpui_component::scroll::ScrollbarMode;
use gpui_component::table::DataTable;

use crate::fake::{self, CONNECTIONS, FOLDERS, TABS};
use crate::parts::{self, conn_colour};
use crate::{Bottom, Pane, Selection, Shell};

pub fn render(shell: &mut Shell, window: &mut Window, cx: &mut Context<Shell>) -> impl IntoElement {
    let theme = cx.theme().clone();
    let tab = &TABS[shell.active_tab];
    let conn = &CONNECTIONS[tab.connection];
    let colour = conn_colour(tab.connection);
    let on_colour = parts::text_on(colour);
    let f_editor = shell.is_focused(Pane::Editor, window, cx);
    let f_results = shell.is_focused(Pane::Results, window, cx);
    let f_messages = shell.is_focused(Pane::Messages, window, cx);
    let f_plan = shell.is_focused(Pane::Plan, window, cx);

    // Top bar: the palette launcher is the main way to go anywhere; tabs are numbered chips.
    let top = div()
        .h(px(40.))
        .flex()
        .items_center()
        .gap_3()
        .px_3()
        .bg(theme.title_bar)
        .child(div().font_weight(FontWeight::BOLD).text_sm().child("Database IDE"))
        .child(
            div()
                .id("palette-launcher")
                .flex_1()
                .max_w(px(620.))
                .mx_auto()
                .flex()
                .items_center()
                .justify_between()
                .px_3()
                .py_1()
                .rounded(px(6.))
                .border_1()
                .border_color(theme.border)
                .bg(theme.background)
                .text_sm()
                .text_color(theme.muted_foreground)
                .child("Go to a connection, object or query tab...")
                .child(div().px_1().rounded(px(3.)).bg(theme.muted).child("Ctrl+K"))
                .on_click(cx.listener(|this, _, w, cx| this.open_palette(w, cx))),
        )
        .child(div().flex().gap_1().children(TABS.iter().enumerate().map(|(i, t)| {
            let active = i == shell.active_tab;
            div()
                .id(("tab-b", i))
                .flex()
                .items_center()
                .gap_1()
                .px_2()
                .py(px(2.))
                .rounded(px(4.))
                .text_xs()
                .when(active, |d| d.bg(theme.secondary).font_weight(FontWeight::BOLD))
                .child(parts::dot(conn_colour(t.connection), 7.))
                .child(t.title)
                .on_click(cx.listener(move |this, _, w, cx| this.select_tab(i, w, cx)))
        })));

    // The connection stripe: full width, in the connection colour, carrying the status facts.
    let fields = parts::status_fields(shell);
    let stripe = div()
        .h(px(34.))
        .flex()
        .items_center()
        .gap_4()
        .px_3()
        .bg(colour)
        .text_color(on_colour)
        .text_sm()
        .child(
            div()
                .px_2()
                .rounded(px(3.))
                .border_1()
                .border_color(on_colour)
                .font_weight(FontWeight::BOLD)
                .child(FOLDERS[conn.folder].name.to_uppercase()),
        )
        .children(fields.into_iter().map(|(k, v)| {
            div()
                .flex()
                .gap_1()
                .child(div().opacity(0.75).child(format!("{k}")))
                .child(div().font_weight(FontWeight::BOLD).child(v))
        }));

    let drawer = if shell.drawer_open {
        let body = match shell.bottom {
            _ if !tab.ran => parts::not_run(cx).into_any_element(),
            Bottom::Messages => parts::pane(f_messages, cx)
                .id("messages-b")
                .track_focus(&shell.messages_focus)
                .size_full()
                .child(parts::messages(shell.active_tab, cx))
                .into_any_element(),
            _ => parts::pane(f_results, cx)
                .size_full()
                .child(
                    DataTable::new(&shell.table)
                        .stripe(true)
                        .scrollbar_mode(ScrollbarMode::Always),
                )
                .into_any_element(),
        };
        div()
            .h(px(430.))
            .flex_none()
            .flex()
            .flex_col()
            .border_t_2()
            .border_color(theme.border)
            .shadow_lg()
            .child(drawer_header(shell, cx))
            .child(div().flex_1().child(body))
            .into_any_element()
    } else {
        drawer_header(shell, cx).into_any_element()
    };

    let editor_column = div()
        .flex_1()
        .min_w_0()
        .overflow_hidden()
        .h_full()
        .flex()
        .flex_col()
        .child(
            parts::pane(f_editor, cx)
                .flex_1()
                .child(Editor::new(&shell.editors[shell.active_tab]).h_full()),
        )
        .child(drawer);

    let side = shell.plan_panel_open.then(|| side_panel(shell, f_plan, cx));

    div()
        .size_full()
        .flex()
        .flex_col()
        .child(top)
        .child(stripe)
        .child(div().flex_1().flex().overflow_hidden().child(editor_column).children(side))
        .when(shell.palette_open, |d| d.child(deferred(palette(shell, cx)).with_priority(5)))
}

/// The drawer's header: its tabs when up, the whole collapsed drawer when down.
fn drawer_header(shell: &Shell, cx: &mut Context<Shell>) -> impl IntoElement {
    let theme = cx.theme();
    let ran = TABS[shell.active_tab].ran;
    div()
        .h(px(30.))
        .flex()
        .items_center()
        .gap_1()
        .px_2()
        .bg(theme.tab_bar)
        .border_t_1()
        .border_color(theme.border)
        .text_sm()
        .child(if shell.drawer_open { "▾" } else { "▴" })
        .children(
            [
                (Bottom::Results, if ran { "Results  1,000,000 rows" } else { "Results" }),
                (Bottom::Messages, if ran { "Messages  1 error" } else { "Messages" }),
            ]
            .into_iter()
            .map(|(b, label)| {
                let active = shell.drawer_open && shell.bottom == b;
                div()
                    .id(SharedString::from(label))
                    .px_2()
                    .rounded(px(3.))
                    .when(active, |d| d.bg(theme.background).font_weight(FontWeight::BOLD))
                    .when(label.contains("error"), |d| d.text_color(theme.danger))
                    .child(label)
                    .on_click(cx.listener(move |this, _, _, cx| {
                        this.bottom = b;
                        this.drawer_open = true;
                        cx.notify();
                    }))
            }),
        )
        .child(div().flex_1())
        .child(
            div()
                .id("plan-toggle")
                .px_2()
                .rounded(px(3.))
                .border_1()
                .border_color(theme.border)
                .child(if shell.plan_panel_open { "Hide plan" } else { "Show plan  Ctrl+4" })
                .on_click(cx.listener(|this, _, _, cx| {
                    this.plan_panel_open = !this.plan_panel_open;
                    cx.notify();
                })),
        )
        .child(
            div()
                .text_xs()
                .text_color(theme.muted_foreground)
                .child("Ctrl+J drawer  |  Ctrl+E edit mode"),
        )
}

/// The right-hand side panel: the jumped-to object, or the plan with operator details.
fn side_panel(shell: &Shell, focused: bool, cx: &mut Context<Shell>) -> impl IntoElement {
    let theme = cx.theme();
    let header = |title: String| {
        div()
            .flex()
            .items_center()
            .justify_between()
            .px_2()
            .py_1()
            .bg(theme.tab_bar)
            .text_sm()
            .font_weight(FontWeight::BOLD)
            .child(title)
            .child(
                div()
                    .id("close-side")
                    .px_1()
                    .child("x")
                    .on_click(cx.listener(|this, _, _, cx| {
                        this.plan_panel_open = false;
                        cx.notify();
                    })),
            )
    };
    let panel = div()
        .w(px(600.))
        .flex_none()
        .h_full()
        .flex()
        .flex_col()
        .border_l_1()
        .border_color(theme.border);
    if shell.selection == Selection::Object
        && let Some((id, label)) = shell.selected_object(cx)
    {
        let (title, rows) = fake::describe(&id, &label);
        return panel
            .child(header(title.to_string()))
            .child(div().p_3().child(parts::kv_rows(rows, cx)))
            .into_any_element();
    }
    let details = match parts::operator_rows(shell.plan.read(cx)) {
        Some((title, rows)) => div()
            .flex()
            .flex_col()
            .gap_1()
            .child(div().font_weight(FontWeight::BOLD).child(title))
            .child(parts::kv_rows(rows, cx)),
        None => div()
            .text_sm()
            .text_color(theme.muted_foreground)
            .child("Select an operator: arrow keys or click"),
    };
    panel
        .child(header("Actual plan".into()))
        .child(parts::pane(focused, cx).h(px(470.)).child(shell.plan.clone()))
        .child(div().p_3().border_t_1().border_color(theme.border).child(details))
        .into_any_element()
}

/// Where a palette entry jumps to.
#[derive(Clone, Debug)]
enum Target {
    Tab(usize),
    Connection(usize),
    Object(String),
    Plan,
    Drawer,
    EditMode,
}

/// The palette's groups, each a heading and its (label, target) entries.
fn palette_groups() -> Vec<(&'static str, Vec<(String, Target)>)> {
    vec![
        (
            "Query tabs",
            TABS.iter()
                .enumerate()
                .map(|(i, t)| {
                    let c = CONNECTIONS[t.connection].name;
                    (format!("{}   {c}.{}", t.title, t.database), Target::Tab(i))
                })
                .collect(),
        ),
        (
            "Connections",
            CONNECTIONS
                .iter()
                .enumerate()
                .map(|(i, c)| {
                    let f = FOLDERS[c.folder].name;
                    (format!("{}   {f} folder, {}", c.name, c.engine), Target::Connection(i))
                })
                .collect(),
        ),
        (
            "Objects",
            fake::objects().into_iter().map(|(id, label)| (label, Target::Object(id))).collect(),
        ),
        (
            "Actions",
            vec![
                ("Show actual plan".into(), Target::Plan),
                ("Show or hide results drawer".into(), Target::Drawer),
                ("Turn edit mode on or off".into(), Target::EditMode),
            ],
        ),
    ]
}

fn run_target(shell: &mut Shell, target: Target, window: &mut Window, cx: &mut Context<Shell>) {
    shell.close_palette(window, cx);
    match target {
        Target::Tab(i) => shell.select_tab(i, window, cx),
        Target::Connection(i) => {
            if let Some(tab) = TABS.iter().position(|t| t.connection == i) {
                shell.select_tab(tab, window, cx);
            }
        }
        Target::Object(id) => {
            shell.reveal_object(&id, window, cx);
            shell.plan_panel_open = true;
        }
        Target::Plan => {
            shell.selection = Selection::PlanNode;
            shell.focus_pane(Pane::Plan, window, cx);
        }
        Target::Drawer => shell.drawer_open = !shell.drawer_open,
        Target::EditMode => shell.edit_mode = !shell.edit_mode,
    }
    cx.notify();
}

/// The command palette over a dimmed window.
fn palette(shell: &Shell, cx: &mut Context<Shell>) -> impl IntoElement {
    let groups = palette_groups();
    let targets: Vec<Vec<Target>> =
        groups.iter().map(|(_, entries)| entries.iter().map(|(_, t)| t.clone()).collect()).collect();
    let weak = cx.entity().downgrade();
    let weak_cancel = weak.clone();
    let mut command = Command::new(&shell.palette)
        .placeholder("Jump to a query tab, connection or object")
        .max_h(px(560.))
        .on_confirm(move |ix: IndexPath, window: &mut Window, cx: &mut App| {
            let Some(target) = targets.get(ix.section).and_then(|g| g.get(ix.row)).cloned() else {
                return;
            };
            let _ = weak.update(cx, |this, cx| run_target(this, target, window, cx));
        })
        .on_cancel(move |window: &mut Window, cx: &mut App| {
            let _ = weak_cancel.update(cx, |this, cx| this.close_palette(window, cx));
        });
    for (label, entries) in groups {
        command = command.group(
            CommandGroup::new()
                .label(label)
                .items(entries.into_iter().map(|(l, _)| CommandItem::new().label(l))),
        );
    }
    let theme = cx.theme();
    div()
        .id("palette-backdrop")
        .absolute()
        .top_0()
        .left_0()
        .size_full()
        .bg(hsla(0., 0., 0., 0.35))
        .flex()
        .items_start()
        .justify_center()
        .on_click(cx.listener(|this, _, w, cx| this.close_palette(w, cx)))
        .child(
            div()
                .id("palette")
                .mt(px(70.))
                .w(px(680.))
                .h_auto()
                .bg(theme.popover)
                .rounded(px(8.))
                .shadow_lg()
                .border_1()
                .border_color(theme.border)
                .on_click(|_, _, cx| cx.stop_propagation())
                .child(command),
        )
}
