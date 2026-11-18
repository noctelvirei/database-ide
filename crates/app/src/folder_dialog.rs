//! The folder dialog: a folder's name, and the colour and danger check its connections
//! inherit unless they set their own.

use dbide_engine::connections::Folder;
use gpui::{
    AppContext as _, Context, Entity, FocusHandle, Focusable as _, InteractiveElement as _,
    IntoElement, ParentElement as _, Render, SharedString, Styled as _, Window, div,
};
use gpui_component::ActiveTheme as _;
use gpui_component::form::{field, v_form};
use gpui_component::input::{Input, InputState};
use gpui_component::select::Select;
use serde::Serialize;

use crate::choice::{self, Choice, choice, label};
use crate::connection_dialog::DANGER_CHECKS;
use crate::palette;

/// What Save hands the explorer.
pub struct FolderOutput {
    pub name: String,
    pub colour: Option<String>,
    pub danger_check: Option<bool>,
}

/// What the dialog shows, recorded by the UI script.
#[derive(Debug, Serialize)]
pub struct FolderState {
    pub name: String,
    pub colour: &'static str,
    pub danger_check: &'static str,
    pub error: Option<SharedString>,
}

pub struct FolderForm {
    focus: FocusHandle,
    name: Entity<InputState>,
    colour: Entity<Choice>,
    danger_check: Entity<Choice>,
    error: Option<SharedString>,
}

impl FolderForm {
    /// A form filled from `editing`, or empty for a new folder.
    pub fn new(editing: Option<&Folder>, window: &mut Window, cx: &mut Context<Self>) -> Self {
        let name = editing.map_or_else(String::new, |f| f.name.clone());
        let name = cx.new(|cx| {
            let mut state = InputState::new(window, cx).placeholder("Folder name");
            state.set_value(name, window, cx);
            state
        });
        let colour = palette::known(editing.and_then(|f| f.colour.as_deref()));
        let danger = editing.and_then(|f| f.danger_check);
        Self {
            focus: cx.focus_handle(),
            name,
            colour: choice(&palette::CHOICES, colour, window, cx),
            danger_check: choice(&DANGER_CHECKS, danger, window, cx),
            error: None,
        }
    }

    pub fn first_field(&self, cx: &gpui::App) -> FocusHandle {
        self.name.focus_handle(cx)
    }

    /// The folder's settings, or `None` after showing why they can't be saved.
    pub fn output(&mut self, cx: &mut Context<Self>) -> Option<FolderOutput> {
        let name = self.name.read(cx).value().trim().to_string();
        if name.is_empty() {
            self.error = Some("Enter a name for the folder.".into());
            cx.notify();
            return None;
        }
        Some(FolderOutput {
            name,
            colour: choice::value(&palette::CHOICES, &self.colour, cx).map(str::to_owned),
            danger_check: choice::value(&DANGER_CHECKS, &self.danger_check, cx),
        })
    }

    pub fn state(&self, cx: &gpui::App) -> FolderState {
        FolderState {
            name: self.name.read(cx).value().to_string(),
            colour: label(&self.colour, cx),
            danger_check: label(&self.danger_check, cx),
            error: self.error.clone(),
        }
    }
}

impl Render for FolderForm {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let danger = cx.theme().danger;
        div()
            .track_focus(&self.focus)
            .on_action(choice::confirm_dialog(self.focus.clone()))
            .flex()
            .flex_col()
            .gap_2()
            .child(
                v_form()
                    .child(
                        field()
                            .label("Name")
                            .required(true)
                            .child(Input::new(&self.name)),
                    )
                    .child(field().label("Colour").child(Select::new(&self.colour)))
                    .child(
                        field()
                            .label("Danger check")
                            .child(Select::new(&self.danger_check)),
                    ),
            )
            .children(
                self.error
                    .clone()
                    .map(|error| div().text_color(danger).child(error)),
            )
    }
}
