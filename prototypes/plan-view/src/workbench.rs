//! PROTOTYPE. The chosen main window, layout C "Workbench" (issue #13), trimmed to what the
//! plan view needs. Left: query tabs grouped by connection, then the explorer. Middle: a
//! header, the editor, a messages strip, then Results or Plan. Right: the inspector, which
//! shows the session and whatever was selected last, here mostly a plan operator.

use gpui::{
    Context, FontWeight, IntoElement, KeyDownEvent, SharedString, Window, div, prelude::*, px,
};
use gpui_component::ActiveTheme;
use gpui_component::input::Editor;
use gpui_component::scroll::ScrollbarMode;
use gpui_component::table::DataTable;

use crate::fake::{self, CONNECTIONS, FOLDERS, TABS};
use crate::model::{self, Op};
use crate::parts::{self, conn_colour};
use crate::view_graph::WARN_RED;
use crate::{Bottom, Pane, Selection, Shell, grid};

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
    let pane = shell.plan.read(cx);
    let plan = &pane.plans[tab.plan];

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
                "up" => this.select_tab((this.active_tab + n - 1) % n, cx),
                "down" => this.select_tab((this.active_tab + 1) % n, cx),
                "enter" => return this.focus_pane(Pane::Editor, w, cx),
                _ => return,
            }
            cx.stop_propagation();
        }))
        .child(parts::heading("QUERY TABS   (F8 next)", cx))
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
                        .child(format!(
                            "{} / {}   {}",
                            folder.name.to_uppercase(),
                            c.name,
                            c.engine
                        )),
                )
                .children(
                    TABS.iter()
                        .enumerate()
                        .filter(|(_, t)| t.connection == ci)
                        .map(|(i, t)| {
                            let active = i == shell.active_tab;
                            let p = &pane.plans[t.plan];
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
                                .child(div().text_xs().text_color(theme.muted_foreground).child(
                                    format!(
                                        "{}  |  ran {}, plan captured",
                                        t.database,
                                        model::ms(p.elapsed_ms)
                                    ),
                                ))
                                .on_click(cx.listener(move |this, _, _, cx| this.select_tab(i, cx)))
                        }),
                )
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
        .child(
            div()
                .px_2()
                .rounded(px(3.))
                .border_1()
                .border_color(theme.border)
                .child("Run  F5"),
        );

    let messages = parts::pane(f_messages, cx)
        .id("messages-c")
        .track_focus(&shell.messages_focus)
        .h(px(190.))
        .flex_none()
        .child(parts::heading("MESSAGES   (Ctrl+3)", cx))
        .child(parts::messages(plan, cx));

    let switch = div()
        .flex()
        .items_center()
        .gap_1()
        .px_2()
        .py_1()
        .bg(theme.tab_bar)
        .text_sm()
        .children(
            [
                (Bottom::Results, "Results  Ctrl+2"),
                (Bottom::Plan, "Plan  Ctrl+4"),
            ]
            .map(|(b, label)| {
                div()
                    .id(SharedString::from(label))
                    .px_2()
                    .rounded(px(3.))
                    .when(shell.bottom == b, |d| {
                        d.bg(theme.background).font_weight(FontWeight::BOLD)
                    })
                    .child(label)
                    .on_click(cx.listener(move |this, _, _, cx| {
                        this.bottom = b;
                        cx.notify();
                    }))
            }),
        )
        .child(div().flex_1())
        .child(
            div()
                .text_xs()
                .text_color(if shell.edit_mode {
                    theme.danger
                } else {
                    theme.muted_foreground
                })
                .child(if shell.edit_mode {
                    "EDIT MODE  (Ctrl+E)"
                } else {
                    "Read-only  (Ctrl+E)"
                }),
        );

    let lower = match shell.bottom {
        Bottom::Plan => parts::pane(f_plan, cx)
            .size_full()
            .child(shell.plan.clone())
            .into_any_element(),
        Bottom::Results => parts::pane(f_results, cx)
            .size_full()
            .child(
                DataTable::new(&shell.table)
                    .stripe(true)
                    .scrollbar_mode(ScrollbarMode::Always),
            )
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
        .overflow_y_scroll()
        .bg(theme.sidebar)
        .child(parts::heading("SESSION", cx))
        .child(div().px_3().pb_2().child(session(shell, cx)))
        .child(div().h(px(1.)).bg(theme.border))
        .child(inspector(shell, cx));

    div()
        .size_full()
        .flex()
        .child(left)
        .child(middle)
        .child(right)
}

/// The session facts as compact rows.
fn session(shell: &Shell, cx: &Context<Shell>) -> impl IntoElement {
    let theme = cx.theme();
    let pane = shell.plan.read(cx);
    let plan = &pane.plans[TABS[shell.active_tab].plan];
    div().flex().flex_col().gap(px(2.)).text_sm().children(
        parts::status_fields(shell, plan).into_iter().map(|(k, v)| {
            div()
                .flex()
                .justify_between()
                .child(div().text_color(theme.muted_foreground).child(k))
                .child(div().font_weight(FontWeight::BOLD).child(v))
        }),
    )
}

/// Everything known about one plan operator, warnings and misestimates first.
fn operator(op: &Op, cx: &Context<Shell>) -> gpui::Div {
    let theme = cx.theme();
    let (factor, over) = op.misestimate();
    let alert = |text: String| {
        div()
            .px_2()
            .py_1()
            .rounded(px(3.))
            .bg(WARN_RED.opacity(0.12))
            .text_color(WARN_RED)
            .text_sm()
            .child(text)
    };
    let mut rows: Vec<(SharedString, SharedString)> = vec![
        (
            "Object".into(),
            if op.object.is_empty() {
                "-".into()
            } else {
                op.object.clone().into()
            },
        ),
        (
            "Estimated rows (all executions)".into(),
            model::rows(op.est_rows).into(),
        ),
        ("Actual rows".into(), model::rows(op.act_rows).into()),
        ("Executions".into(), model::rows(op.executions).into()),
        (
            "Cost: own / with inputs".into(),
            format!("{:.1} %  /  {:.1} %", op.cost_pct, op.subtree_cost_pct).into(),
        ),
        (
            "Time: own / with inputs".into(),
            format!("{}  /  {}", model::ms(op.self_ms), model::ms(op.total_ms)).into(),
        ),
        ("Logical reads (own)".into(), model::rows(op.reads).into()),
        (
            "Parallel".into(),
            if op.parallel { "yes" } else { "no" }.into(),
        ),
    ];
    rows.extend(
        op.details
            .iter()
            .map(|(k, v)| (k.clone().into(), v.clone().into())),
    );
    div()
        .px_3()
        .flex()
        .flex_col()
        .gap_2()
        .children(op.warnings.iter().map(|w| alert(format!("⚠ {w}"))))
        .when(op.is_misestimated() && op.parent.is_some(), |d| {
            d.child(alert(format!(
                "Estimate off {}×: {} estimated, {} actual ({})",
                model::rows(factor.round()),
                model::rows(op.est_rows),
                model::rows(op.act_rows),
                if over {
                    "more rows than planned"
                } else {
                    "fewer rows than planned"
                }
            )))
        })
        .child(
            div()
                .text_color(theme.foreground)
                .child(parts::kv_rows(rows, cx)),
        )
}

/// The inspector's lower part: details of the last selection.
fn inspector(shell: &Shell, cx: &Context<Shell>) -> impl IntoElement {
    let theme = cx.theme();
    let pane = shell.plan.read(cx);
    let body = match shell.selection {
        Selection::PlanNode => match pane.selection() {
            Some(i) => {
                let op = &pane.plan().ops[i];
                Some((
                    format!("PLAN OPERATOR: {}", op.label()).into(),
                    operator(op, cx),
                ))
            }
            None => None,
        },
        Selection::Object => shell.selected_object(cx).map(|(id, label)| {
            let (t, r) = fake::describe(&id, &label);
            (
                t.to_uppercase().into(),
                div().px_3().child(parts::kv_rows(r, cx)),
            )
        }),
        Selection::Column(col) => Some((
            format!("RESULT COLUMN: {}", grid::COLUMN_NAMES[col]).into(),
            div().px_3().child(parts::kv_rows(
                vec![("Type".into(), "see results".into())],
                cx,
            )),
        )),
        Selection::Nothing => None,
    };
    let (title, body): (SharedString, gpui::Div) = body.unwrap_or((
        "SELECTION".into(),
        div()
            .px_3()
            .text_sm()
            .text_color(theme.muted_foreground)
            .child("Select a plan operator, an object or a results column."),
    ));
    div()
        .flex()
        .flex_col()
        .pb_4()
        .child(parts::heading(title, cx))
        .child(body)
}
