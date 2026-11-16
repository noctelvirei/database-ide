//! PROTOTYPE. Variant A, "Classic": the SSMS shape. Object explorer docked left, query tabs
//! across the top, the editor over a bottom panel with Results / Messages / Plan tabs, and a
//! status bar painted in the active connection's colour.

use gpui::{Context, FontWeight, IntoElement, SharedString, Window, div, prelude::*, px};
use gpui_component::ActiveTheme;
use gpui_component::input::Editor;
use gpui_component::scroll::ScrollbarMode;
use gpui_component::table::DataTable;

use crate::fake::{CONNECTIONS, TABS};
use crate::parts::{self, conn_colour};
use crate::{Bottom, Pane, Shell};

pub fn render(shell: &mut Shell, window: &mut Window, cx: &mut Context<Shell>) -> impl IntoElement {
    let theme = cx.theme().clone();
    let tab = &TABS[shell.active_tab];
    let colour = conn_colour(tab.connection);
    let focused = |p: Pane| shell.is_focused(p, window, cx);
    let (f_explorer, f_editor, f_results, f_messages, f_plan) = (
        focused(Pane::Explorer),
        focused(Pane::Editor),
        focused(Pane::Results),
        focused(Pane::Messages),
        focused(Pane::Plan),
    );

    let toolbar = div()
        .h(px(34.))
        .flex()
        .items_center()
        .gap_2()
        .px_2()
        .bg(theme.title_bar)
        .border_b_1()
        .border_color(theme.border)
        .text_sm()
        .child(div().font_weight(FontWeight::BOLD).child("Database IDE"))
        .children(["New query", "Run  F5", "Estimated plan", "Actual plan"].map(|b| {
            div().px_2().py(px(2.)).rounded(px(3.)).border_1().border_color(theme.border).child(b)
        }))
        .child(
            div()
                .ml_4()
                .px_2()
                .py(px(2.))
                .rounded(px(3.))
                .border_1()
                .border_color(theme.border)
                .child(format!("Database: {}  ▾", tab.database)),
        );

    let explorer = parts::pane(f_explorer, cx)
        .track_focus(&shell.explorer_focus)
        .w(px(330.))
        .flex_none()
        .h_full()
        .flex()
        .flex_col()
        .bg(theme.sidebar)
        .child(parts::heading("OBJECT EXPLORER   (Ctrl+5)", cx))
        .child(
            div()
                .mx_2()
                .mb_1()
                .px_2()
                .py(px(2.))
                .text_sm()
                .border_1()
                .border_color(theme.border)
                .rounded(px(3.))
                .text_color(theme.muted_foreground)
                .child("Filter objects in all databases..."),
        )
        .child(div().flex_1().child(parts::explorer(&shell.explorer)));

    let tab_strip = div()
        .flex()
        .bg(theme.tab_bar)
        .border_b_1()
        .border_color(theme.border)
        .children(TABS.iter().enumerate().map(|(i, t)| {
            let active = i == shell.active_tab;
            div()
                .id(("tab-a", i))
                .flex()
                .items_center()
                .gap_2()
                .px_3()
                .py_1()
                .border_t(px(3.))
                .border_color(conn_colour(t.connection))
                .border_r_1()
                .bg(if active { theme.background } else { theme.tab_bar })
                .text_sm()
                .child(div().font_weight(if active { FontWeight::BOLD } else { FontWeight::NORMAL }).child(t.title))
                .child(
                    div()
                        .text_xs()
                        .text_color(theme.muted_foreground)
                        .child(format!("{}.{}", CONNECTIONS[t.connection].name, t.database)),
                )
                .on_click(cx.listener(move |this, _, w, cx| this.select_tab(i, w, cx)))
        }));

    let bottom_tabs = div()
        .flex()
        .items_center()
        .bg(theme.tab_bar)
        .border_b_1()
        .border_color(theme.border)
        .children(
            [
                (Bottom::Results, "Results", "Ctrl+2"),
                (Bottom::Messages, "Messages (1 error)", "Ctrl+3"),
                (Bottom::Plan, "Actual plan", "Ctrl+4"),
            ]
            .into_iter()
            .map(|(b, label, key)| {
                let active = shell.bottom == b;
                div()
                    .id(SharedString::from(label))
                    .px_3()
                    .py_1()
                    .text_sm()
                    .border_b_2()
                    .border_color(if active { parts::FOCUS } else { theme.tab_bar })
                    .font_weight(if active { FontWeight::BOLD } else { FontWeight::NORMAL })
                    .text_color(if label.contains("error") { theme.danger } else { theme.foreground })
                    .child(format!("{label}  {key}"))
                    .on_click(cx.listener(move |this, _, _, cx| {
                        this.bottom = b;
                        cx.notify();
                    }))
            }),
        )
        .child(div().flex_1())
        .child(
            div()
                .px_2()
                .text_xs()
                .text_color(if shell.edit_mode { theme.danger } else { theme.muted_foreground })
                .child(if shell.edit_mode {
                    "EDIT MODE: changes are saved as one script"
                } else {
                    "Read-only grid  (Ctrl+E: edit mode)"
                }),
        );

    let bottom_body = match shell.bottom {
        _ if !tab.ran => parts::not_run(cx).into_any_element(),
        Bottom::Results => parts::pane(f_results, cx)
            .size_full()
            .child(DataTable::new(&shell.table).stripe(true).scrollbar_mode(ScrollbarMode::Always))
            .into_any_element(),
        Bottom::Messages => parts::pane(f_messages, cx)
            .id("messages-a")
            .track_focus(&shell.messages_focus)
            .size_full()
            .child(parts::messages(shell.active_tab, cx))
            .into_any_element(),
        Bottom::Plan => plan_with_details(shell, f_plan, cx).into_any_element(),
    };

    let main = div()
        .flex_1()
        .h_full()
        .flex()
        .flex_col()
        .child(tab_strip)
        .child(
            parts::pane(f_editor, cx)
                .h(px(330.))
                .child(Editor::new(&shell.editors[shell.active_tab]).h_full()),
        )
        .child(div().flex_1().flex().flex_col().child(bottom_tabs).child(div().flex_1().child(bottom_body)));

    let text = parts::text_on(colour);
    let status = div()
        .h(px(28.))
        .flex()
        .items_center()
        .gap_4()
        .px_3()
        .bg(colour)
        .text_color(text)
        .text_sm()
        .children(parts::status_fields(shell).into_iter().map(|(k, v)| {
            div()
                .flex()
                .gap_1()
                .child(div().opacity(0.75).child(format!("{k}:")))
                .child(div().font_weight(FontWeight::BOLD).child(v))
        }));

    div()
        .size_full()
        .flex()
        .flex_col()
        .child(toolbar)
        .child(div().flex_1().flex().overflow_hidden().child(explorer).child(main))
        .child(status)
}

/// The plan tree with the selected operator's details beside it.
fn plan_with_details(shell: &Shell, focused: bool, cx: &Context<Shell>) -> impl IntoElement {
    let theme = cx.theme();
    let plan = shell.plan.read(cx);
    let details = match parts::operator_rows(plan) {
        Some((title, rows)) => div()
            .flex()
            .flex_col()
            .gap_2()
            .child(div().font_weight(FontWeight::BOLD).child(title))
            .child(parts::kv_rows(rows, cx)),
        None => div().text_sm().text_color(theme.muted_foreground).child("Select an operator (arrow keys or click)"),
    };
    div()
        .size_full()
        .flex()
        .child(parts::pane(focused, cx).flex_1().h_full().child(shell.plan.clone()))
        .child(div().w(px(300.)).h_full().p_2().border_l_1().border_color(theme.border).child(details))
}
