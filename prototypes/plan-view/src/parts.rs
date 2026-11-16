//! PROTOTYPE. Small pieces of the workbench: the explorer tree, the messages list, key-value
//! rows and the session facts.

use gpui::{
    AnyElement, App, Div, FontWeight, Hsla, IntoElement, ParentElement, SharedString, Styled, div,
    hsla, px,
};
use gpui_component::ActiveTheme;
use gpui_component::list::ListItem;
use gpui_component::tree::{TreeState, tree};

use crate::Shell;
use crate::fake::{self, CONNECTIONS, FOLDERS, ItemKind, TABS};
use crate::model::{self, Engine, Plan};

/// Border colour of the focused pane.
pub const FOCUS: Hsla = Hsla {
    h: 0.6,
    s: 0.85,
    l: 0.5,
    a: 1.,
};

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
                .child(
                    div()
                        .w(px(10.))
                        .text_color(theme.muted_foreground)
                        .child(arrow),
                )
                .child(marker)
                .child(
                    div()
                        .font_weight(weight)
                        .text_color(colour)
                        .child(item.label.clone()),
                ),
        )
    })
}

/// The messages a run of `plan`'s statement would print: the row count and timings.
pub fn messages(plan: &Plan, cx: &App) -> Div {
    let theme = cx.theme();
    let mut lines = vec![format!(
        "({} rows affected)",
        model::rows(plan.ops[0].act_rows)
    )];
    lines.extend(plan.facts.iter().map(|(k, v)| format!("{k}: {v}")));
    lines.push(match plan.engine {
        Engine::SqlServer => "Actual execution plan captured (SET STATISTICS XML ON).".into(),
        Engine::Postgres => "Plan captured with EXPLAIN (ANALYZE, BUFFERS, FORMAT JSON).".into(),
    });
    div()
        .p_2()
        .flex()
        .flex_col()
        .gap(px(2.))
        .font_family(theme.mono_font_family.clone())
        .text_sm()
        .text_color(theme.muted_foreground)
        .children(lines)
}

/// A vertical list of (label, value) rows.
pub fn kv_rows(rows: Vec<(SharedString, SharedString)>, cx: &App) -> Div {
    let theme = cx.theme();
    div()
        .flex()
        .flex_col()
        .gap_1()
        .children(rows.into_iter().map(|(k, v)| {
            div()
                .flex()
                .flex_col()
                .child(div().text_xs().text_color(theme.muted_foreground).child(k))
                .child(div().text_sm().child(v))
        }))
}

/// The session facts for the active tab, as (label, value).
pub fn status_fields(shell: &Shell, plan: &Plan) -> Vec<(&'static str, SharedString)> {
    let tab = &TABS[shell.active_tab];
    let conn = &CONNECTIONS[tab.connection];
    vec![
        ("Connection", conn.name.into()),
        ("Engine", conn.engine.into()),
        ("Database", tab.database.into()),
        ("User", conn.user.into()),
        ("Rows", model::rows(plan.ops[0].act_rows).into()),
        ("Elapsed", model::ms(plan.elapsed_ms).into()),
        (
            "Danger check",
            if conn.danger_check { "On" } else { "Off" }.into(),
        ),
    ]
}

/// White or near-black, whichever reads better on `bg`.
pub fn text_on(bg: Hsla) -> Hsla {
    if bg.l < 0.6 {
        gpui::white()
    } else {
        hsla(0., 0., 0.1, 1.)
    }
}
