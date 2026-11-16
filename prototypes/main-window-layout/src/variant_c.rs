//! PROTOTYPE. Variant C, "Workbench": three columns. Query tabs are a vertical list on the
//! left, grouped by connection under their folder colours, with the object explorer below.
//! The middle holds the editor, an always-visible messages strip and results or plan. A
//! persistent inspector on the right shows the session and whatever was selected last:
//! a plan operator, an explorer object or a results column.

use gpui::{
    Context, FontWeight, IntoElement, KeyDownEvent, SharedString, Window, div, prelude::*, px,
};
use gpui_component::ActiveTheme;
use gpui_component::input::Editor;
use gpui_component::scroll::ScrollbarMode;
use gpui_component::table::DataTable;

use crate::fake::{self, CONNECTIONS, FOLDERS, TABS};
use crate::parts::{self, conn_colour};
use crate::{Bottom, Pane, Selection, Shell, grid};

/// Fake SQL types for the grid's columns, for the column inspector.
const COLUMN_TYPES: [&str; grid::COLS] = [
    "bigint",
    "nvarchar(200)",
    "datetime2(3)",
    "varchar(20)",
    "decimal(12,2)",
    "char(3)",
    "nvarchar(80)",
    "varchar(8)",
    "varchar(16)",
    "nvarchar(400)",
];

pub fn render(shell: &mut Shell, window: &mut Window, cx: &mut Context<Shell>) -> impl IntoElement {
    let theme = cx.theme().clone();
    let tab = &TABS[shell.active_tab];
    let conn = &CONNECTIONS[tab.connection];
    let colour = conn_colour(tab.connection);
    let focused = |p: Pane| shell.is_focused(p, window, cx);
    let (f_tabs, f_explorer, f_editor, f_messages, f_results, f_plan, f_inspector) = (
        focused(Pane::TabList),
        focused(Pane::Explorer),
        focused(Pane::Editor),
        focused(Pane::Messages),
        focused(Pane::Results),
        focused(Pane::Plan),
        focused(Pane::Inspector),
    );

    // Left column: query tabs grouped by connection, then the explorer.
    let tab_list = parts::pane(f_tabs, cx)
        .id("tab-list")
        .track_focus(&shell.tab_list_focus)
        .flex_none()
        .flex()
        .flex_col()
        .pb_2()
        .on_key_down(cx.listener(|this, ev: &KeyDownEvent, w, cx| {
            let n = TABS.len();
            match ev.keystroke.key.as_str() {
                "up" => this.active_tab = (this.active_tab + n - 1) % n,
                "down" => this.active_tab = (this.active_tab + 1) % n,
                "enter" => return this.focus_pane(Pane::Editor, w, cx),
                _ => return,
            }
            cx.stop_propagation();
            cx.notify();
        }))
        .child(parts::heading("QUERY TABS   (F6, then Up/Down)", cx))
        .children(CONNECTIONS.iter().enumerate().map(|(ci, c)| {
            let folder = &FOLDERS[c.folder];
            div()
                .flex()
                .flex_col()
                .child(
                    div()
                        .mx_2()
                        .mt_1()
                        .px_2()
                        .py(px(3.))
                        .rounded_t(px(4.))
                        .bg(folder.colour)
                        .text_color(parts::text_on(folder.colour))
                        .text_xs()
                        .font_weight(FontWeight::BOLD)
                        .child(format!("{} / {}   {}", folder.name.to_uppercase(), c.name, c.engine)),
                )
                .children(TABS.iter().enumerate().filter(|(_, t)| t.connection == ci).map(
                    |(i, t)| {
                        let active = i == shell.active_tab;
                        div()
                            .id(("tab-c", i))
                            .mx_2()
                            .px_2()
                            .py_1()
                            .border_l(px(4.))
                            .border_color(folder.colour)
                            .when(active, |d| d.bg(theme.list_active))
                            .flex()
                            .flex_col()
                            .child(
                                div()
                                    .text_sm()
                                    .when(active, |d| d.font_weight(FontWeight::BOLD))
                                    .child(t.title),
                            )
                            .child(
                                div().text_xs().text_color(theme.muted_foreground).child(
                                    if t.ran {
                                        format!("{}  |  ran 2.4 s, 1 error", t.database)
                                    } else {
                                        format!("{}  |  not run", t.database)
                                    },
                                ),
                            )
                            .on_click(cx.listener(move |this, _, w, cx| this.select_tab(i, w, cx)))
                    },
                ))
        }));

    let left = div()
        .w(px(290.))
        .flex_none()
        .h_full()
        .flex()
        .flex_col()
        .bg(theme.sidebar)
        .border_r_1()
        .border_color(theme.border)
        .child(tab_list)
        .child(
            parts::pane(f_explorer, cx)
                .track_focus(&shell.explorer_focus)
                .flex_1()
                .flex()
                .flex_col()
                .child(parts::heading("OBJECTS   (Ctrl+5)", cx))
                .child(div().flex_1().child(parts::explorer(&shell.explorer))),
        );

    // Middle column: a header naming where this tab runs, the editor, messages, then
    // results or plan.
    let header = div()
        .h(px(36.))
        .flex()
        .items_center()
        .gap_2()
        .px_2()
        .border_b_1()
        .border_color(theme.border)
        .text_sm()
        .child(
            div()
                .px_2()
                .py(px(2.))
                .rounded(px(4.))
                .bg(colour)
                .text_color(parts::text_on(colour))
                .font_weight(FontWeight::BOLD)
                .child(conn.name),
        )
        .child(format!("›  {} ▾  ›  {}", tab.database, tab.title))
        .child(div().flex_1())
        .when(conn.danger_check, |d| {
            d.child(
                div()
                    .px_2()
                    .rounded(px(3.))
                    .border_1()
                    .border_color(theme.danger)
                    .text_color(theme.danger)
                    .text_xs()
                    .child("Danger check on"),
            )
        })
        .child(div().px_2().rounded(px(3.)).border_1().border_color(theme.border).child("Run  F5"));

    let messages = parts::pane(f_messages, cx)
        .id("messages-c")
        .track_focus(&shell.messages_focus)
        .h(px(190.))
        .flex_none()
        .child(parts::heading("MESSAGES   (Ctrl+3)", cx))
        .child(parts::messages(shell.active_tab, cx));

    let switch = div()
        .flex()
        .items_center()
        .gap_1()
        .px_2()
        .py_1()
        .bg(theme.tab_bar)
        .text_sm()
        .children([(Bottom::Results, "Results  Ctrl+2"), (Bottom::Plan, "Plan  Ctrl+4")].map(
            |(b, label)| {
                let active = shell.bottom == b || (b == Bottom::Results && shell.bottom == Bottom::Messages);
                div()
                    .id(SharedString::from(label))
                    .px_2()
                    .rounded(px(3.))
                    .when(active, |d| d.bg(theme.background).font_weight(FontWeight::BOLD))
                    .child(label)
                    .on_click(cx.listener(move |this, _, _, cx| {
                        this.bottom = b;
                        cx.notify();
                    }))
            },
        ))
        .child(div().flex_1())
        .child(
            div()
                .text_xs()
                .text_color(if shell.edit_mode { theme.danger } else { theme.muted_foreground })
                .child(if shell.edit_mode { "EDIT MODE  (Ctrl+E)" } else { "Read-only  (Ctrl+E)" }),
        );

    let lower = match shell.bottom {
        _ if !tab.ran => parts::not_run(cx).into_any_element(),
        Bottom::Plan => parts::pane(f_plan, cx).size_full().child(shell.plan.clone()).into_any_element(),
        _ => parts::pane(f_results, cx)
            .size_full()
            .child(DataTable::new(&shell.table).stripe(true).scrollbar_mode(ScrollbarMode::Always))
            .into_any_element(),
    };

    let middle = div()
        .flex_1()
        .h_full()
        .flex()
        .flex_col()
        .overflow_hidden()
        .child(header)
        .child(
            parts::pane(f_editor, cx)
                .h(px(280.))
                .flex_none()
                .child(Editor::new(&shell.editors[shell.active_tab]).h_full()),
        )
        .child(messages)
        .child(switch)
        .child(div().flex_1().child(lower));

    let right = parts::pane(f_inspector, cx)
        .id("inspector")
        .track_focus(&shell.inspector_focus)
        .w(px(340.))
        .flex_none()
        .h_full()
        .flex()
        .flex_col()
        .bg(theme.sidebar)
        .child(parts::heading("SESSION", cx))
        .child(div().px_3().pb_2().child(session(shell, cx)))
        .child(div().h(px(1.)).bg(theme.border))
        .child(inspector(shell, cx));

    div().size_full().flex().child(left).child(middle).child(right)
}

/// The session facts as compact rows: what a status bar holds in the other variants.
fn session(shell: &Shell, cx: &Context<Shell>) -> impl IntoElement {
    let theme = cx.theme();
    div().flex().flex_col().gap(px(2.)).text_sm().children(parts::status_fields(shell).into_iter().map(
        |(k, v)| {
            div()
                .flex()
                .justify_between()
                .child(div().text_color(theme.muted_foreground).child(k))
                .child(div().font_weight(FontWeight::BOLD).child(v))
        },
    ))
}

/// The inspector's lower part: details of the last selection.
fn inspector(shell: &Shell, cx: &Context<Shell>) -> impl IntoElement {
    let theme = cx.theme();
    let (title, rows): (SharedString, Vec<(SharedString, SharedString)>) = match shell.selection {
        Selection::PlanNode => match parts::operator_rows(shell.plan.read(cx)) {
            Some((t, r)) => (format!("PLAN OPERATOR: {t}").into(), r),
            None => ("PLAN OPERATOR".into(), vec![]),
        },
        Selection::Object => match shell.selected_object(cx) {
            Some((id, label)) => {
                let (t, r) = fake::describe(&id, &label);
                (t.to_uppercase().into(), r)
            }
            None => ("OBJECT".into(), vec![]),
        },
        Selection::Column(col) => {
            let cell = shell.table.read(cx).selected_cell();
            (
                format!("RESULT COLUMN: {}", grid::COLUMN_NAMES[col]).into(),
                vec![
                    ("Type".into(), COLUMN_TYPES[col].into()),
                    ("Source".into(), "dbo.Orders (editable result: has PK)".into()),
                    ("Nulls in result".into(), "0".into()),
                    (
                        "Selected cell".into(),
                        cell.map_or("-".to_string(), |(r, c)| format!("row {r}, column {c}")).into(),
                    ),
                ],
            )
        }
        Selection::Nothing => (
            "SELECTION".into(),
            vec![("".into(), "Select a plan operator, an object or a results column.".into())],
        ),
    };
    div()
        .flex_1()
        .flex()
        .flex_col()
        .child(parts::heading(title, cx))
        .child(div().px_3().text_color(theme.foreground).child(parts::kv_rows(rows, cx)))
}
