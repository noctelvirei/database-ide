//! The menu bar (docs/spec.md, "Menu bar"): File, Edit, View, Query and Help across the top
//! of the window, so every action can be found without knowing its key. Each item runs the
//! same action its shortcut does and shows that shortcut, read from the keymap.
//!
//! The bar is built from gpui-component's `PopupMenu` rather than its `AppMenuBar`, whose
//! menus can't be opened by Alt+letter without changing gpui-kit. A menu is built when it
//! opens, from the workbench's situation at that moment, so greyed and ticked items are
//! always current. Its actions go to the focus that was active before it opened, or for Edit
//! outside the results, to the editor.
//!
//! Alt+F, Alt+E, Alt+V, Alt+Q and Alt+H open a menu, and F10 opens File; Left and Right move
//! to the next menu, and the menu's own Up, Down, Enter and Escape move through it. Help
//! opens the keyboard shortcuts or About in a panel over the window; Escape closes it.

use gpui::{
    Action, App, AsKeystroke as _, Context, DismissEvent, Entity, FocusHandle, Focusable as _,
    InteractiveElement as _, IntoElement, KeyBinding, KeyDownEvent, ParentElement as _, Render,
    SharedString, StatefulInteractiveElement as _, Styled as _, Window, deferred, div,
    prelude::FluentBuilder as _, px,
};
use gpui_component::ActiveTheme as _;
use gpui_component::h_flex;
use gpui_component::kbd::Kbd;
use gpui_component::menu::PopupMenu;
use serde::Serialize;

use crate::keymap::{
    CancelRun, ClosePanel, CopyBlock, EstimatedPlan, FocusExplorer, Format, NewConnection,
    NewQueryTab, NextPane, OpenFile, PickDatabase, PrevPane, Quit, Run, SaveFile, ShowAbout,
    ShowHistory, ShowShortcuts, ToggleActualPlan, TogglePlanView, ToggleResults,
    ToggleResultsToFile,
};

/// One menu of the bar, left to right.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
pub enum Menu {
    File,
    Edit,
    View,
    Query,
    Help,
}

impl Menu {
    pub const ALL: [Menu; 5] = [Menu::File, Menu::Edit, Menu::View, Menu::Query, Menu::Help];

    /// The title; its first letter, underlined, is the Alt key that opens it.
    fn title(self) -> &'static str {
        match self {
            Menu::File => "File",
            Menu::Edit => "Edit",
            Menu::View => "View",
            Menu::Query => "Query",
            Menu::Help => "Help",
        }
    }

    /// The menu `step` places along the bar, wrapping at either end.
    fn step(self, step: isize) -> Menu {
        let ix = Menu::ALL.iter().position(|m| *m == self).unwrap_or(0) as isize;
        Menu::ALL[(ix + step).rem_euclid(Menu::ALL.len() as isize) as usize]
    }
}

/// What the workbench is doing when a menu opens: it decides which items can act and which
/// are ticked.
#[derive(Clone)]
pub struct Situation {
    /// The active tab's session is idle, so it can run, plan or change database.
    pub ready: bool,
    /// Run can act: the tab is ready, or it is a restored tab that signs in first.
    pub runnable: bool,
    /// The active tab is running, so Cancel can act.
    pub running: bool,
    pub actual_plan: bool,
    pub results_to_file: bool,
    pub results_visible: bool,
    /// The active tab's plans show as a graph.
    pub plan_graph: bool,
    /// The results pane had focus: Edit's Copy and Select all work on its grid.
    pub in_results: bool,
    /// Where Edit's actions go, and the other menus' (the focus before the menu opened).
    pub edit_target: FocusHandle,
    pub previous: FocusHandle,
}

/// One menu item: what it says, what it runs, and whether it can run now.
struct Item {
    label: &'static str,
    action: Box<dyn Action>,
    enabled: bool,
    /// Some for an item that toggles a setting: whether it is on.
    checked: Option<bool>,
}

fn item(label: &'static str, action: impl Action, enabled: bool) -> Item {
    Item {
        label,
        action: Box::new(action),
        enabled,
        checked: None,
    }
}

fn toggle(label: &'static str, action: impl Action, on: bool) -> Item {
    Item {
        checked: Some(on),
        ..item(label, action, true)
    }
}

/// `menu`'s items in `s`. Items for features not built yet are left out.
fn items(menu: Menu, s: &Situation) -> Vec<Item> {
    use gpui_component::input;
    let editing = !s.in_results;
    match menu {
        Menu::File => vec![
            item("New query", NewQueryTab, true),
            item("New connection", NewConnection, true),
            item("Open…", OpenFile, true),
            item("Save", SaveFile, true),
            item("Exit", Quit, true),
        ],
        Menu::Edit => {
            let (copy, select_all): (Box<dyn Action>, Box<dyn Action>) = if s.in_results {
                (Box::new(CopyBlock), Box::new(crate::keymap::SelectAll))
            } else {
                (Box::new(input::Copy), Box::new(input::SelectAll))
            };
            vec![
                item("Undo", input::Undo, editing),
                item("Redo", input::Redo, editing),
                item("Cut", input::Cut, editing),
                Item {
                    label: "Copy",
                    action: copy,
                    enabled: true,
                    checked: None,
                },
                item("Paste", input::Paste, editing),
                Item {
                    label: "Select all",
                    action: select_all,
                    enabled: true,
                    checked: None,
                },
                item("Find", input::Search, editing),
                item("Replace", input::Replace, editing),
                item("Format", Format, true),
            ]
        }
        Menu::View => vec![
            item("Object explorer", FocusExplorer, true),
            toggle("Results", ToggleResults, s.results_visible),
            toggle("Plan as graph", TogglePlanView, s.plan_graph),
            item("Next pane", NextPane, true),
            item("Previous pane", PrevPane, true),
            item("Query history", ShowHistory, true),
        ],
        Menu::Query => vec![
            item("Run", Run, s.runnable),
            item("Cancel", CancelRun, s.running),
            item("Estimated plan", EstimatedPlan, s.ready),
            toggle("Include actual plan", ToggleActualPlan, s.actual_plan),
            toggle("Results to file", ToggleResultsToFile, s.results_to_file),
            item("Change database…", PickDatabase, s.ready),
        ],
        Menu::Help => vec![
            item("Keyboard shortcuts", ShowShortcuts, true),
            item("About Database IDE", ShowAbout, true),
        ],
    }
}

/// Where `menu`'s actions go.
fn target(menu: Menu, s: &Situation) -> FocusHandle {
    match menu {
        Menu::Edit => s.edit_target.clone(),
        _ => s.previous.clone(),
    }
}

/// The open menu, kept with the situation it was built for so Left and Right can open the
/// next menu the same way.
struct OpenMenu {
    menu: Menu,
    popup: Entity<PopupMenu>,
    items: Vec<ItemState>,
    situation: Situation,
}

/// A panel Help opens over the window.
enum Panel {
    /// Every menu item with the keys that run it, as rows of label and keys.
    Shortcuts(Vec<Shortcut>),
    About,
}

/// One row of the keyboard shortcuts panel.
#[derive(Clone, Debug, Serialize)]
pub struct Shortcut {
    pub action: &'static str,
    /// Keystrokes as Windows writes them, a chord's strokes separated by spaces.
    pub keys: String,
}

/// What a user can see of one menu item.
#[derive(Clone, Debug, Serialize)]
pub struct ItemState {
    pub label: &'static str,
    pub enabled: bool,
    pub checked: Option<bool>,
}

/// What a user can see of the menu bar: the open menu and its items, and the open panel.
#[derive(Debug, Serialize)]
pub struct MenuBarState {
    pub open: Option<Menu>,
    pub items: Vec<ItemState>,
    /// "shortcuts" or "about" while Help's panel is open.
    pub panel: Option<&'static str>,
    pub shortcuts: Vec<Shortcut>,
}

pub struct MenuBar {
    open: Option<OpenMenu>,
    panel: Option<Panel>,
    /// Focused while a panel is open, so Escape reaches it.
    panel_focus: FocusHandle,
    /// Where focus returns when the panel closes.
    panel_return: Option<FocusHandle>,
}

impl MenuBar {
    pub fn new(cx: &mut Context<Self>) -> Self {
        Self {
            open: None,
            panel: None,
            panel_focus: cx.focus_handle(),
            panel_return: None,
        }
    }

    /// Opens `menu` for `situation`, closing any open menu.
    pub fn open(
        &mut self,
        menu: Menu,
        situation: Situation,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let built = items(menu, &situation);
        let states = built
            .iter()
            .map(|i| ItemState {
                label: i.label,
                enabled: i.enabled,
                checked: i.checked,
            })
            .collect();
        let context = target(menu, &situation);
        let popup = PopupMenu::build(window, cx, move |mut popup, window, _| {
            for item in built {
                let chord = window
                    .highest_precedence_binding_for_action_in(&*item.action, &context)
                    .map(|binding| {
                        binding
                            .keystrokes()
                            .iter()
                            .map(|k| k.as_keystroke().clone())
                            .collect::<Vec<_>>()
                    })
                    .filter(|strokes| strokes.len() > 1);
                if let (Some(strokes), None) = (chord, item.checked) {
                    let label = item.label;
                    popup = popup.menu_element_with_disabled(
                        item.action,
                        !item.enabled,
                        move |_, _| chord_row(label, strokes.clone()),
                    );
                    continue;
                }
                popup = match item.checked {
                    Some(on) => popup.menu_with_check_and_disabled(
                        item.label,
                        on,
                        item.action,
                        !item.enabled,
                    ),
                    None => popup.menu_with_disabled(item.label, item.action, !item.enabled),
                };
            }
            popup.min_w(px(220.)).action_context(context)
        });
        cx.subscribe_in(&popup, window, |this, popup, _: &DismissEvent, _, cx| {
            if this.open.as_ref().is_some_and(|open| &open.popup == popup) {
                this.open = None;
                cx.notify();
            }
        })
        .detach();
        self.dismiss_panel(cx);
        popup.focus_handle(cx).focus(window, cx);
        self.open = Some(OpenMenu {
            menu,
            popup,
            items: states,
            situation,
        });
        cx.notify();
    }

    /// The open menu, while it holds focus. Whatever moves focus away (F8, Ctrl+Tab, a
    /// click) leaves it out of Escape's reach, so a menu without focus counts as closed and
    /// is not drawn.
    fn focused_menu(&self, window: &Window, cx: &App) -> Option<&OpenMenu> {
        self.open
            .as_ref()
            .filter(|open| open.popup.focus_handle(cx).contains_focused(window, cx))
    }

    /// The Help panel, while it holds focus, for the same reason as `focused_menu`.
    fn focused_panel(&self, window: &Window, cx: &App) -> Option<&Panel> {
        self.panel
            .as_ref()
            .filter(|_| self.panel_focus.contains_focused(window, cx))
    }

    /// The situation the open menu was built for, if a menu is open.
    pub fn situation(&self, window: &Window, cx: &App) -> Option<&Situation> {
        self.focused_menu(window, cx).map(|open| &open.situation)
    }

    /// Help's panels: the keyboard shortcuts of every menu item, or About.
    pub fn show_shortcuts(
        &mut self,
        situation: &Situation,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let mut rows = Vec::new();
        for menu in Menu::ALL {
            let focus = target(menu, situation);
            for item in items(menu, situation) {
                if let Some(binding) =
                    window.highest_precedence_binding_for_action_in(&*item.action, &focus)
                {
                    rows.push(Shortcut {
                        action: item.label,
                        keys: keys(&binding),
                    });
                }
            }
        }
        self.show_panel(Panel::Shortcuts(rows), window, cx);
    }

    pub fn show_about(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.show_panel(Panel::About, window, cx);
    }

    /// Where focus goes back to when the open Help panel closes, if one is open.
    pub fn panel_return(&self, window: &Window, cx: &App) -> Option<&FocusHandle> {
        self.focused_panel(window, cx)
            .and(self.panel_return.as_ref())
    }

    fn show_panel(&mut self, panel: Panel, window: &mut Window, cx: &mut Context<Self>) {
        self.panel_return = window.focused(cx);
        self.panel = Some(panel);
        self.panel_focus.focus(window, cx);
        cx.notify();
    }

    /// Forgets the Help panel without moving focus, as a menu opening takes focus from it.
    fn dismiss_panel(&mut self, cx: &mut Context<Self>) {
        if self.panel.take().is_some() {
            self.panel_return = None;
            cx.notify();
        }
    }

    fn close_panel(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.panel = None;
        if let Some(focus) = self.panel_return.take() {
            focus.focus(window, cx);
        }
        cx.notify();
    }

    /// Left or Right in an open menu: the menu beside it, opened as Alt+its letter would, so
    /// its items reflect the workbench now.
    fn step(&mut self, step: isize, window: &mut Window, cx: &mut Context<Self>) {
        if let Some(open) = &self.open {
            window.dispatch_action(open_action(open.menu.step(step)), cx);
        }
    }

    pub fn state(&self, window: &Window, cx: &App) -> MenuBarState {
        let open = self.focused_menu(window, cx);
        let panel = self.focused_panel(window, cx);
        MenuBarState {
            open: open.map(|o| o.menu),
            items: open.map(|o| o.items.clone()).unwrap_or_default(),
            panel: panel.map(|p| match p {
                Panel::Shortcuts(_) => "shortcuts",
                Panel::About => "about",
            }),
            shortcuts: match panel {
                Some(Panel::Shortcuts(rows)) => rows.clone(),
                _ => Vec::new(),
            },
        }
    }

    fn panel(&self, window: &Window, cx: &mut Context<Self>) -> Option<impl IntoElement> {
        let theme = cx.theme();
        let body = match self.focused_panel(window, cx)? {
            Panel::Shortcuts(rows) => div()
                .flex()
                .flex_col()
                .gap_1()
                .child(div().text_lg().child("Keyboard shortcuts"))
                .children(rows.iter().map(|row| {
                    div()
                        .flex()
                        .flex_row()
                        .justify_between()
                        .gap_4()
                        .child(row.action)
                        .child(
                            div()
                                .text_color(theme.muted_foreground)
                                .child(row.keys.clone()),
                        )
                })),
            Panel::About => div()
                .flex()
                .flex_col()
                .gap_1()
                .child(div().text_lg().child("Database IDE"))
                .child(format!("Version {}", env!("CARGO_PKG_VERSION")))
                .child("Licensed under MIT OR Apache-2.0.")
                .child(env!("CARGO_PKG_REPOSITORY"))
                .child(
                    div()
                        .pt_2()
                        .text_xs()
                        .text_color(theme.muted_foreground)
                        .child(include_str!("../assets/plan-icons/LICENSE.txt")),
                ),
        };
        Some(deferred(
            div()
                .id("menu-panel")
                .key_context("MenuPanel")
                .track_focus(&self.panel_focus)
                .on_action(cx.listener(|this, _: &ClosePanel, w, cx| this.close_panel(w, cx)))
                .absolute()
                .top(px(40.))
                .left(px(300.))
                .w(px(420.))
                .max_h(px(560.))
                .overflow_y_scroll()
                .p_4()
                .bg(theme.popover)
                .border_1()
                .border_color(theme.border)
                .rounded_md()
                .shadow_lg()
                .child(body)
                .child(
                    div()
                        .pt_2()
                        .text_sm()
                        .text_color(theme.muted_foreground)
                        .child("Escape closes this."),
                ),
        ))
    }
}

/// A menu row for an item bound to a chord: its label, then each keystroke as PopupMenu draws
/// a single one.
fn chord_row(label: &'static str, strokes: Vec<gpui::Keystroke>) -> impl IntoElement {
    h_flex()
        .w_full()
        .gap_3()
        .items_center()
        .justify_between()
        .child(label)
        .child(h_flex().gap_1().children(strokes.into_iter().map(|stroke| {
            Kbd::new(stroke)
                .p_0()
                .flex_nowrap()
                .border_0()
                .bg(gpui::transparent_white())
        })))
}

/// A binding's keystrokes as Windows writes them: `Ctrl+K Ctrl+F`.
fn keys(binding: &KeyBinding) -> String {
    binding
        .keystrokes()
        .iter()
        .map(|k| Kbd::format(k.as_keystroke()))
        .collect::<Vec<_>>()
        .join(" ")
}

impl Render for MenuBar {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = cx.theme().clone();
        let open = self
            .focused_menu(window, cx)
            .map(|o| (o.menu, o.popup.clone()));
        let titles = Menu::ALL.into_iter().map(|menu| {
            let (first, rest) = menu.title().split_at(1);
            let popup = open
                .as_ref()
                .filter(|(m, _)| *m == menu)
                .map(|(_, popup)| popup.clone());
            div()
                .id(SharedString::from(menu.title()))
                .relative()
                .px_2()
                .py_0p5()
                .rounded_sm()
                .cursor_pointer()
                .when(popup.is_some(), |title| title.bg(theme.accent))
                .hover(|title| title.bg(theme.accent))
                .child(
                    div()
                        .flex()
                        .flex_row()
                        .child(div().underline().child(first.to_owned()))
                        .child(rest.to_owned()),
                )
                .on_click(move |_, window, cx| {
                    window.dispatch_action(open_action(menu), cx);
                })
                .children(popup.map(|popup| {
                    deferred(div().absolute().top_full().left_0().child(popup)).with_priority(1)
                }))
        });
        div()
            .id("menu-bar")
            .key_context("MenuBar")
            .flex()
            .flex_row()
            .gap_1()
            .px_1()
            .text_sm()
            .border_b_1()
            .border_color(theme.border)
            .on_key_down(cx.listener(|this, event: &KeyDownEvent, w, cx| {
                let keystroke = &event.keystroke;
                if this.open.is_none() || keystroke.modifiers.modified() {
                    return;
                }
                match keystroke.key.as_str() {
                    "left" => this.step(-1, w, cx),
                    "right" => this.step(1, w, cx),
                    _ => {}
                }
            }))
            .children(titles)
            .children(self.panel(window, cx))
    }
}

/// The action that opens `menu`, as its Alt key does.
pub fn open_action(menu: Menu) -> Box<dyn Action> {
    use crate::keymap::{OpenEditMenu, OpenFileMenu, OpenHelpMenu, OpenQueryMenu, OpenViewMenu};
    match menu {
        Menu::File => Box::new(OpenFileMenu),
        Menu::Edit => Box::new(OpenEditMenu),
        Menu::View => Box::new(OpenViewMenu),
        Menu::Query => Box::new(OpenQueryMenu),
        Menu::Help => Box::new(OpenHelpMenu),
    }
}
