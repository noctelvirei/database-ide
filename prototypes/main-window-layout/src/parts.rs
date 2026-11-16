//! PROTOTYPE. Small pieces every variant may place wherever its layout wants: the explorer
//! tree, the messages list, a placeholder for tabs that have not run, the operator details
//! and the status fields. Layout itself is never shared: each variant owns its own.

use gpui::{
    AnyElement, App, Div, FontWeight, Hsla, IntoElement, ParentElement, SharedString, Styled,
    div, hsla, px,
};
use gpui_component::ActiveTheme;
use gpui_component::list::ListItem;
use gpui_component::tree::{TreeState, tree};

use crate::fake::{self, CONNECTIONS, FOLDERS, ItemKind, MessageKind, TABS};
use crate::plan::PlanView;
use crate::{Shell, plan};

/// Border colour of the focused pane.
pub const FOCUS: Hsla = plan::FOCUS_BLUE;

/// A pane container whose border shows whether keyboard focus is inside it.
pub fn pane(focused: bool, cx: &App) -> Div {
    div()
        .border_1()
        .border_color(if focused { FOCUS } else { cx.theme().border })
        .overflow_hidden()
}

/// A small upper-case heading over a section.
pub fn heading(text: impl Into<SharedString>, cx: &App) -> Div {
    div()
        .px_2()
        .py_1()
        .text_xs()
        .font_weight(FontWeight::BOLD)
        .text_color(cx.theme().muted_foreground)
        .child(text.into())
}

/// The colour of a connection: its folder's colour.
pub fn conn_colour(conn: usize) -> Hsla {
    FOLDERS[CONNECTIONS[conn].folder].colour
}

/// A filled dot in `colour`.
pub fn dot(colour: Hsla, size: f32) -> Div {
    div().size(px(size)).rounded_full().bg(colour).flex_none()
}

/// The object explorer tree, rendered with folder and connection colours.
pub fn explorer(state: &gpui::Entity<TreeState>) -> impl IntoElement {
    tree(state, |ix, entry, _selected, _window, cx| {
        let item = entry.item();
        let kind = fake::item_kind(&item.id);
        let theme = cx.theme();
        let marker: AnyElement = match kind {
            ItemKind::Folder(f) => div()
                .size(px(10.))
                .rounded(px(2.))
                .bg(FOLDERS[f].colour)
                .flex_none()
                .into_any_element(),
            ItemKind::Connection(c) => dot(conn_colour(c), 8.).into_any_element(),
            _ => div().w(px(8.)).flex_none().into_any_element(),
        };
        let arrow = if entry.is_folder() {
            if entry.is_expanded() { "▾" } else { "▸" }
        } else {
            " "
        };
        let weight = match kind {
            ItemKind::Folder(_) | ItemKind::Connection(_) => FontWeight::BOLD,
            _ => FontWeight::NORMAL,
        };
        let colour = match kind {
            ItemKind::Group => theme.muted_foreground,
            _ => theme.foreground,
        };
        ListItem::new(ix).py(px(1.)).child(
            div()
                .pl(px(entry.depth() as f32 * 14.))
                .flex()
                .items_center()
                .gap_1()
                .text_sm()
                .whitespace_nowrap()
                .child(div().w(px(10.)).text_color(theme.muted_foreground).child(arrow))
                .child(marker)
                .child(div().font_weight(weight).text_color(colour).child(item.label.clone())),
        )
    })
}

/// The messages of tab `tab`'s last run, one line each; errors in red.
pub fn messages(tab: usize, cx: &App) -> Div {
    let theme = cx.theme();
    if !TABS[tab].ran {
        return not_run(cx);
    }
    div()
        .p_2()
        .flex()
        .flex_col()
        .gap(px(2.))
        .font_family(theme.mono_font_family.clone())
        .text_sm()
        .children(fake::MESSAGES.iter().map(|(kind, text)| {
            let colour = match kind {
                MessageKind::Error => theme.danger,
                MessageKind::Info | MessageKind::Rows => theme.muted_foreground,
                MessageKind::Print => theme.foreground,
            };
            div().text_color(colour).child(*text)
        }))
}

/// Shown in place of results, messages or a plan for a tab that has not run.
pub fn not_run(cx: &App) -> Div {
    div()
        .size_full()
        .flex()
        .items_center()
        .justify_center()
        .text_sm()
        .text_color(cx.theme().muted_foreground)
        .child("Not run yet. F5 runs the batch (not wired in this prototype).")
}

/// (label, value) rows for the selected plan operator, or `None` when nothing is selected.
pub fn operator_rows(plan: &PlanView) -> Option<(SharedString, Vec<(SharedString, SharedString)>)> {
    let n = &plan.nodes[plan.selected?];
    // Fake actual rows: a few operators misestimate, to show the warning.
    let actual = if n.cost_pct >= 15 { n.rows * 9 } else { n.rows };
    Some((
        n.name.into(),
        vec![
            ("Estimated cost".into(), format!("{} %", n.cost_pct).into()),
            ("Estimated rows".into(), n.rows.to_string().into()),
            (
                "Actual rows".into(),
                if actual != n.rows {
                    format!("{actual}  (warning: 9x the estimate)").into()
                } else {
                    actual.to_string().into()
                },
            ),
            ("Object".into(), if n.object.is_empty() { "-" } else { n.object }.into()),
            ("Detail".into(), n.detail.into()),
        ],
    ))
}

/// A vertical list of (label, value) rows.
pub fn kv_rows(rows: Vec<(SharedString, SharedString)>, cx: &App) -> Div {
    let theme = cx.theme();
    div().flex().flex_col().gap_1().children(rows.into_iter().map(|(k, v)| {
        div()
            .flex()
            .flex_col()
            .child(div().text_xs().text_color(theme.muted_foreground).child(k))
            .child(div().text_sm().child(v))
    }))
}

/// The facts every variant's status area shows for the active tab, as (label, value).
pub fn status_fields(shell: &Shell) -> Vec<(&'static str, SharedString)> {
    let tab = &TABS[shell.active_tab];
    let conn = &CONNECTIONS[tab.connection];
    vec![
        ("Connection", conn.name.into()),
        ("Engine", conn.engine.into()),
        ("Database", tab.database.into()),
        ("User", conn.user.into()),
        ("Rows", if tab.ran { fake::ROW_COUNT } else { "-" }.into()),
        ("Elapsed", if tab.ran { fake::ELAPSED } else { "-" }.into()),
        (
            "Edit mode",
            if shell.edit_mode { "ON: 0 pending changes" } else { "Read-only" }.into(),
        ),
        ("Danger check", if conn.danger_check { "On" } else { "Off" }.into()),
    ]
}

/// White or near-black, whichever reads better on `bg`.
pub fn text_on(bg: Hsla) -> Hsla {
    if bg.l < 0.6 { gpui::white() } else { hsla(0., 0., 0.1, 1.) }
}
