//! The explorer pane's connections: saved folders and connections as one tree, the dialogs
//! that edit them, and connecting and disconnecting. U5 adds each connection's objects below
//! it.
//!
//! The tree is `connections.toml` (docs/spec.md, "Connections"). Every edit is saved at once.
//! If the file can't be read, the explorer shows why and refuses edits, so a file the user can
//! still repair is never overwritten. Results and failures go to the messages strip as
//! `ExplorerEvent::Message`.

use std::cell::Cell;
use std::collections::{HashMap, HashSet};
use std::path::PathBuf;
use std::rc::Rc;

use dbide_engine::connections::credentials::Credentials;
use dbide_engine::connections::{self, Connections, SavedConnection, SignInMethod};
use dbide_engine::session::Session;
use gpui::{
    App, AppContext as _, Context, Entity, EventEmitter, FocusHandle, Focusable, IntoElement,
    ParentElement as _, Render, SharedString, Styled as _, Window, div, prelude::*, px,
};
use gpui_component::input::{Input, InputState};
use gpui_component::list::ListItem;
use gpui_component::tree::{TreeEvent, TreeItem, TreeState, tree};
use gpui_component::{ActiveTheme as _, WindowExt as _};
use serde::Serialize;

use crate::connection_dialog::{ConnectionForm, FormState};
use crate::folder_dialog::{FolderForm, FolderState};
use crate::keymap::{
    ConnectItem, CutItem, DeleteItem, DisconnectItem, EditItem, NewConnection, NewFolder,
    PasteItem, PasteItemAtTop, RenameItem,
};
use crate::palette;
use crate::runtime::Runtime;

/// Something for the messages strip.
pub enum ExplorerEvent {
    Message(SharedString),
}

/// One explorer row as the user sees it, top to bottom.
#[derive(Debug, Serialize)]
pub struct RowState {
    pub depth: usize,
    pub kind: Kind,
    pub name: String,
    /// The colour's name: the folder's own, or the connection's after inheritance.
    pub colour: Option<&'static str>,
    pub status: Status,
    pub selected: bool,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Kind {
    Folder,
    Connection,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Status {
    Disconnected,
    Connecting,
    Connected,
}

/// The open dialog, as the UI script records it. The password prompt reports only whether
/// a password is typed.
#[derive(Debug, Serialize)]
#[serde(tag = "dialog", rename_all = "snake_case")]
pub enum DialogState {
    Connection(FormState),
    Folder(FolderState),
    Rename { name: String },
    Password { connection: String, typed: bool },
    Delete { name: String },
}

/// What a user can see of the explorer.
#[derive(Debug, Serialize)]
pub struct ExplorerState {
    pub rows: Vec<RowState>,
    /// Why connections can't be changed, when `connections.toml` couldn't be read.
    pub problem: Option<SharedString>,
    pub dialog: Option<DialogState>,
}

enum OpenDialog {
    Connection(Entity<ConnectionForm>),
    Folder(Entity<FolderForm>),
    Rename(Entity<InputState>),
    Password {
        connection: SharedString,
        input: Entity<InputState>,
    },
    Delete(SharedString),
}

pub struct Explorer {
    focus: FocusHandle,
    tree: Entity<TreeState>,
    path: PathBuf,
    saved: Connections,
    /// Set when `connections.toml` couldn't be read; edits are refused while it is.
    problem: Option<SharedString>,
    credentials: Rc<Credentials>,
    runtime: Runtime,
    sessions: HashMap<String, Box<dyn Session>>,
    connecting: HashSet<String>,
    /// Folders the user has opened, by ID.
    expanded: HashSet<String>,
    /// The item Ctrl+X picked up, waiting for Ctrl+V.
    cut: Option<String>,
    dialog: Option<OpenDialog>,
}

impl EventEmitter<ExplorerEvent> for Explorer {}

impl Explorer {
    /// Loads the connections saved at `path`; a missing file is an empty tree.
    pub fn new(
        path: PathBuf,
        credentials: Credentials,
        runtime: Runtime,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        let (saved, problem) = match Connections::load(&path) {
            Ok(saved) => (saved, None),
            Err(error) => (
                Connections::default(),
                Some(SharedString::from(format!(
                    "Can't read {}: {error}. Fix or move the file, then restart; until then \
                     connections can't be changed.",
                    path.display()
                ))),
            ),
        };
        let tree = cx.new(|cx| TreeState::new(cx));
        cx.subscribe(
            &tree,
            |this: &mut Self, _, event: &TreeEvent, _| match event {
                TreeEvent::Expanded(id) => {
                    this.expanded.insert(id.to_string());
                }
                TreeEvent::Collapsed(id) => {
                    this.expanded.remove(id.as_ref());
                }
            },
        )
        .detach();
        let focus = cx.focus_handle();
        // The tree owns the arrow keys, so focus on the pane goes on to the tree.
        cx.on_focus(&focus, window, |this, window, cx| {
            this.tree.update(cx, |tree, cx| tree.focus(window, cx));
        })
        .detach();
        let this = Self {
            focus,
            tree,
            path,
            saved,
            problem,
            credentials: Rc::new(credentials),
            runtime,
            sessions: HashMap::new(),
            connecting: HashSet::new(),
            expanded: HashSet::new(),
            cut: None,
            dialog: None,
        };
        this.rebuild(None, cx);
        this
    }

    pub fn state(&self, cx: &App) -> ExplorerState {
        let tree = self.tree.read(cx);
        let selected = tree.selected_index();
        let rows = (0..)
            .map_while(|ix| tree.entry(ix).map(|entry| (ix, entry)))
            .filter_map(|(ix, entry)| {
                let id = entry.item().id.as_ref();
                let (kind, name, colour) = self.describe(id)?;
                Some(RowState {
                    depth: entry.depth(),
                    kind,
                    name,
                    colour: palette::name(colour),
                    status: self.status(id),
                    selected: selected == Some(ix),
                })
            })
            .collect();
        let dialog = self.dialog.as_ref().map(|dialog| match dialog {
            OpenDialog::Connection(form) => DialogState::Connection(form.read(cx).state(cx)),
            OpenDialog::Folder(form) => DialogState::Folder(form.read(cx).state(cx)),
            OpenDialog::Rename(input) => DialogState::Rename {
                name: input.read(cx).value().to_string(),
            },
            OpenDialog::Password { connection, input } => DialogState::Password {
                connection: connection.to_string(),
                typed: !input.read(cx).value().is_empty(),
            },
            OpenDialog::Delete(name) => DialogState::Delete {
                name: name.to_string(),
            },
        });
        ExplorerState {
            rows,
            problem: self.problem.clone(),
            dialog,
        }
    }

    /// A row's kind, name and drawn colour, or `None` for an ID no longer saved.
    fn describe(&self, id: &str) -> Option<(Kind, String, Option<&str>)> {
        if let Some(folder) = self.saved.folders().iter().find(|f| f.id == id) {
            return Some((Kind::Folder, folder.name.clone(), folder.colour.as_deref()));
        }
        let saved = self.connection(id)?;
        let colour = self
            .saved
            .effective_settings(id)
            .ok()
            .and_then(|settings| settings.colour);
        Some((Kind::Connection, saved.connection.name.clone(), colour))
    }

    fn status(&self, id: &str) -> Status {
        if self.sessions.contains_key(id) {
            Status::Connected
        } else if self.connecting.contains(id) {
            Status::Connecting
        } else {
            Status::Disconnected
        }
    }

    fn connection(&self, id: &str) -> Option<&SavedConnection> {
        self.saved.connections().iter().find(|c| c.id == id)
    }

    fn is_folder(&self, id: &str) -> bool {
        self.saved.folders().iter().any(|f| f.id == id)
    }

    fn selected(&self, cx: &App) -> Option<String> {
        self.tree
            .read(cx)
            .selected_item()
            .map(|item| item.id.to_string())
    }

    /// The folder a new or pasted item goes in: the selected folder, or the selected
    /// connection's folder, or the top level.
    fn target_folder(&self, cx: &App) -> Option<String> {
        let id = self.selected(cx)?;
        if self.is_folder(&id) {
            Some(id)
        } else {
            self.connection(&id).and_then(|c| c.parent.clone())
        }
    }

    /// Rebuilds the tree from the saved connections, keeping the open folders, and selects
    /// `select` (or keeps the current selection).
    fn rebuild(&self, select: Option<&str>, cx: &mut Context<Self>) {
        let select = select.map(str::to_owned).or_else(|| self.selected(cx));
        let items = self.items(None);
        self.tree.update(cx, |tree, cx| {
            tree.set_items(items, cx);
            let ix = select.and_then(|id| tree.index_of(&id.into()));
            tree.set_selected_index(ix, cx);
        });
        cx.notify();
    }

    /// The tree items under `parent`: folders first, then connections, in saved order.
    fn items(&self, parent: Option<&str>) -> Vec<TreeItem> {
        let folders = self
            .saved
            .folders()
            .iter()
            .filter(|f| f.parent.as_deref() == parent)
            .map(|f| {
                TreeItem::new(f.id.clone(), f.name.clone())
                    .children(self.items(Some(&f.id)))
                    .expanded(self.expanded.contains(&f.id))
            });
        let connections = self
            .saved
            .connections()
            .iter()
            .filter(|c| c.parent.as_deref() == parent)
            .map(|c| TreeItem::new(c.id.clone(), c.connection.name.clone()));
        folders.chain(connections).collect()
    }

    fn message(&self, text: impl Into<SharedString>, cx: &mut Context<Self>) {
        cx.emit(ExplorerEvent::Message(text.into()));
    }

    /// True when edits may be saved; otherwise says why not.
    fn editable(&self, cx: &mut Context<Self>) -> bool {
        match &self.problem {
            None => true,
            Some(problem) => {
                self.message(problem.clone(), cx);
                false
            }
        }
    }

    /// Writes the tree to `connections.toml`, reporting a failure.
    fn persist(&self, cx: &mut Context<Self>) {
        if let Err(error) = self.saved.save(&self.path) {
            self.message(
                format!("Couldn't save {}: {error}.", self.path.display()),
                cx,
            );
        }
    }

    fn new_connection(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.editable(cx) {
            let parent = self.target_folder(cx);
            self.open_connection_dialog(None, parent, window, cx);
        }
    }

    fn open_connection_dialog(
        &mut self,
        editing: Option<SavedConnection>,
        parent: Option<String>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let title = if editing.is_some() {
            "Edit connection"
        } else {
            "New connection"
        };
        let id = editing.as_ref().map(|c| c.id.clone());
        let credentials = self.credentials.clone();
        let runtime = self.runtime.clone();
        let form = cx.new(|cx| ConnectionForm::new(editing, credentials, runtime, window, cx));
        self.dialog = Some(OpenDialog::Connection(form.clone()));
        let explorer = cx.entity();
        let first_open = Cell::new(true);
        window.open_dialog(cx, move |dialog, window, cx| {
            if first_open.replace(false) {
                form.read(cx).first_field(cx).focus(window, cx);
            }
            let save = {
                let (explorer, form, id, parent) =
                    (explorer.clone(), form.clone(), id.clone(), parent.clone());
                move |_: &gpui::ClickEvent, _: &mut Window, cx: &mut App| {
                    explorer.update(cx, |this, cx| {
                        this.save_connection(&form, id.as_deref(), parent.as_deref(), cx)
                    })
                }
            };
            dialog
                .title(title)
                .w(px(520.))
                .child(form.clone())
                .footer(footer("Save (Enter)"))
                .on_ok(save)
                .on_close(closer(&explorer))
        });
    }

    /// Saves the dialog's connection and its password. Returns false to keep the dialog
    /// open when the fields are wrong.
    fn save_connection(
        &mut self,
        form: &Entity<ConnectionForm>,
        editing: Option<&str>,
        parent: Option<&str>,
        cx: &mut Context<Self>,
    ) -> bool {
        let Some(output) = form.update(cx, |form, cx| form.output(cx)) else {
            return false;
        };
        let name = output.connection.name.clone();
        let saved = match editing {
            Some(id) => self
                .saved
                .update_connection(id, output.connection, &self.credentials)
                .map(|()| id.to_owned()),
            None => self.saved.add_connection(output.connection, parent),
        };
        let id = match saved {
            Ok(id) => id,
            Err(error) => {
                self.message(format!("Couldn't save {name}: {error}."), cx);
                return true;
            }
        };
        if let (Some(password), Some(saved)) = (output.password, self.connection(&id))
            && let Err(error) = self.credentials.store(saved, &password)
        {
            self.message(format!("Saved {name}, but not its password: {error}."), cx);
        }
        if let Some(parent) = parent {
            self.expanded.insert(parent.to_owned());
        }
        self.persist(cx);
        self.rebuild(Some(&id), cx);
        true
    }

    fn new_folder(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.editable(cx) {
            let parent = self.target_folder(cx);
            self.open_folder_dialog(None, parent, window, cx);
        }
    }

    fn open_folder_dialog(
        &mut self,
        editing: Option<String>,
        parent: Option<String>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let folder = editing
            .as_deref()
            .and_then(|id| self.saved.folders().iter().find(|f| f.id == id).cloned());
        let title = if folder.is_some() {
            "Edit folder"
        } else {
            "New folder"
        };
        let form = cx.new(|cx| FolderForm::new(folder.as_ref(), window, cx));
        self.dialog = Some(OpenDialog::Folder(form.clone()));
        let explorer = cx.entity();
        let first_open = Cell::new(true);
        window.open_dialog(cx, move |dialog, window, cx| {
            if first_open.replace(false) {
                form.read(cx).first_field(cx).focus(window, cx);
            }
            let save = {
                let (explorer, form, editing, parent) = (
                    explorer.clone(),
                    form.clone(),
                    editing.clone(),
                    parent.clone(),
                );
                move |_: &gpui::ClickEvent, _: &mut Window, cx: &mut App| {
                    explorer.update(cx, |this, cx| {
                        this.save_folder(&form, editing.as_deref(), parent.as_deref(), cx)
                    })
                }
            };
            dialog
                .title(title)
                .w(px(420.))
                .child(form.clone())
                .footer(footer("Save (Enter)"))
                .on_ok(save)
                .on_close(closer(&explorer))
        });
    }

    fn save_folder(
        &mut self,
        form: &Entity<FolderForm>,
        editing: Option<&str>,
        parent: Option<&str>,
        cx: &mut Context<Self>,
    ) -> bool {
        let Some(output) = form.update(cx, |form, cx| form.output(cx)) else {
            return false;
        };
        let saved = match editing {
            Some(id) => self.saved.rename(id, &output.name).map(|()| id.to_owned()),
            None => self.saved.add_folder(&output.name, parent),
        }
        .and_then(|id| {
            self.saved
                .set_folder_settings(&id, output.colour, output.danger_check)
                .map(|()| id)
        });
        match saved {
            Ok(id) => {
                if let Some(parent) = parent {
                    self.expanded.insert(parent.to_owned());
                }
                self.persist(cx);
                self.rebuild(Some(&id), cx);
            }
            Err(error) => self.message(
                format!("Couldn't save folder {}: {error}.", output.name),
                cx,
            ),
        }
        true
    }

    fn edit(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(id) = self.selected(cx) else { return };
        if !self.editable(cx) {
            return;
        }
        if self.is_folder(&id) {
            self.open_folder_dialog(Some(id), None, window, cx);
        } else if let Some(saved) = self.connection(&id).cloned() {
            self.open_connection_dialog(Some(saved), None, window, cx);
        }
    }

    fn rename(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(id) = self.selected(cx) else { return };
        let Some((_, name, _)) = self.describe(&id) else {
            return;
        };
        if !self.editable(cx) {
            return;
        }
        let input = cx.new(|cx| {
            let mut state = InputState::new(window, cx);
            state.set_value(name, window, cx);
            state
        });
        self.dialog = Some(OpenDialog::Rename(input.clone()));
        let explorer = cx.entity();
        let first_open = Cell::new(true);
        window.open_dialog(cx, move |dialog, window, cx| {
            if first_open.replace(false) {
                input.focus_handle(cx).focus(window, cx);
            }
            let save = {
                let (explorer, input, id) = (explorer.clone(), input.clone(), id.clone());
                move |_: &gpui::ClickEvent, _: &mut Window, cx: &mut App| {
                    let name = input.read(cx).value().trim().to_string();
                    explorer.update(cx, |this, cx| {
                        if name.is_empty() {
                            return false;
                        }
                        match this.saved.rename(&id, name) {
                            Ok(()) => this.persist(cx),
                            Err(error) => this.message(format!("Couldn't rename: {error}."), cx),
                        }
                        this.rebuild(Some(&id), cx);
                        true
                    })
                }
            };
            dialog
                .title("Rename")
                .w(px(420.))
                .child(Input::new(&input))
                .footer(footer("Rename (Enter)"))
                .on_ok(save)
                .on_close(closer(&explorer))
        });
    }

    fn delete(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(id) = self.selected(cx) else { return };
        let Some((kind, name, _)) = self.describe(&id) else {
            return;
        };
        if !self.editable(cx) {
            return;
        }
        let question: SharedString = match kind {
            Kind::Folder => format!("Delete folder {name} and every folder and connection in it?"),
            Kind::Connection => format!("Delete connection {name} and its saved password?"),
        }
        .into();
        self.dialog = Some(OpenDialog::Delete(name.into()));
        let explorer = cx.entity();
        window.open_alert_dialog(cx, move |alert, _, _| {
            let confirm = {
                let (explorer, id) = (explorer.clone(), id.clone());
                move |_: &gpui::ClickEvent, _: &mut Window, cx: &mut App| {
                    explorer.update(cx, |this, cx| this.delete_now(&id, cx));
                    true
                }
            };
            alert
                .title("Delete")
                .description(question.clone())
                .ok_text("Delete")
                .show_cancel(true)
                .on_ok(confirm)
                .on_close(closer(&explorer))
        });
    }

    fn delete_now(&mut self, id: &str, cx: &mut Context<Self>) {
        let before: HashSet<String> = self
            .saved
            .connections()
            .iter()
            .map(|c| c.id.clone())
            .collect();
        if let Err(error) = self.saved.delete(id, &self.credentials) {
            self.message(format!("Couldn't delete: {error}."), cx);
            return;
        }
        // Close the sessions of every deleted connection.
        let kept: HashSet<&str> = self
            .saved
            .connections()
            .iter()
            .map(|c| c.id.as_str())
            .collect();
        for gone in before.iter().filter(|id| !kept.contains(id.as_str())) {
            if let Some(session) = self.sessions.remove(gone) {
                self.runtime.close(session);
            }
        }
        self.persist(cx);
        self.rebuild(None, cx);
    }

    fn cut(&mut self, cx: &mut Context<Self>) {
        let Some(id) = self.selected(cx) else { return };
        let Some((_, name, _)) = self.describe(&id) else {
            return;
        };
        self.cut = Some(id);
        self.message(
            format!("Cut {name}. Select a folder and press Ctrl+V to move it there, or Ctrl+Shift+V for the top level."),
            cx,
        );
    }

    fn paste(&mut self, at_top: bool, cx: &mut Context<Self>) {
        let Some(id) = self.cut.clone() else {
            self.message(
                "Nothing is cut. Select a folder or connection and press Ctrl+X first.",
                cx,
            );
            return;
        };
        if !self.editable(cx) {
            return;
        }
        let target = if at_top { None } else { self.target_folder(cx) };
        match self.saved.move_to(&id, target.as_deref()) {
            Ok(()) => {
                self.cut = None;
                if let Some(target) = &target {
                    self.expanded.insert(target.clone());
                }
                self.persist(cx);
                self.rebuild(Some(&id), cx);
            }
            Err(connections::Error::Invalid(_)) => {
                self.message("A folder can't move into itself or a folder inside it.", cx);
            }
            Err(error) => self.message(format!("Couldn't move: {error}."), cx),
        }
    }

    /// Enter: opens or closes a folder, or connects a connection.
    fn activate(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(id) = self.selected(cx) else { return };
        if self.is_folder(&id) {
            if !self.expanded.remove(&id) {
                self.expanded.insert(id.clone());
            }
            self.rebuild(Some(&id), cx);
            return;
        }
        let Some(saved) = self.connection(&id).cloned() else {
            return;
        };
        match self.status(&id) {
            Status::Connected => {
                self.message(
                    format!("Already connected to {}.", saved.connection.name),
                    cx,
                );
            }
            Status::Connecting => {}
            Status::Disconnected => match saved.connection.sign_in {
                SignInMethod::Windows => self.connect(&saved, None, window, cx),
                SignInMethod::Password => match self.credentials.read(&saved) {
                    Ok(Some(secret)) => {
                        let password = secret.expose().to_owned();
                        self.connect(&saved, Some(password), window, cx);
                    }
                    Ok(None) => self.ask_password(saved, window, cx),
                    Err(error) => {
                        self.message(format!("Can't read the saved password: {error}."), cx);
                    }
                },
                SignInMethod::AskEachTime => self.ask_password(saved, window, cx),
            },
        }
    }

    fn ask_password(
        &mut self,
        saved: SavedConnection,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let input = cx.new(|cx| InputState::new(window, cx).masked(true));
        let connection: SharedString = saved.connection.name.clone().into();
        self.dialog = Some(OpenDialog::Password {
            connection: connection.clone(),
            input: input.clone(),
        });
        let explorer = cx.entity();
        let first_open = Cell::new(true);
        window.open_dialog(cx, move |dialog, window, cx| {
            if first_open.replace(false) {
                input.focus_handle(cx).focus(window, cx);
            }
            let go = {
                let (explorer, input, saved) = (explorer.clone(), input.clone(), saved.clone());
                move |_: &gpui::ClickEvent, window: &mut Window, cx: &mut App| {
                    let password = input.read(cx).value().to_string();
                    explorer.update(cx, |this, cx| {
                        this.connect(&saved, Some(password), window, cx)
                    });
                    true
                }
            };
            dialog
                .title(format!("Password for {connection}"))
                .w(px(420.))
                .child(Input::new(&input))
                .footer(footer("Connect (Enter)"))
                .on_ok(go)
                .on_close(closer(&explorer))
        });
    }

    fn connect(
        &mut self,
        saved: &SavedConnection,
        password: Option<String>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let id = saved.id.clone();
        let name = saved.connection.name.clone();
        self.connecting.insert(id.clone());
        self.message(format!("Connecting to {name}…"), cx);
        cx.notify();
        let attempt = self.runtime.connect(saved.connection.clone(), password);
        cx.spawn_in(window, async move |this, cx| {
            let outcome = attempt.await;
            let _ = this.update(cx, |this, cx| {
                this.connecting.remove(&id);
                match outcome {
                    // The connection was deleted while connecting.
                    Ok(session) if this.connection(&id).is_none() => this.runtime.close(session),
                    Ok(session) => {
                        this.sessions.insert(id, session);
                        this.message(format!("Connected to {name}."), cx);
                    }
                    Err(error) => {
                        this.message(format!("Couldn't connect to {name}. {}", error.message), cx);
                    }
                }
                cx.notify();
            });
        })
        .detach();
    }

    fn disconnect(&mut self, cx: &mut Context<Self>) {
        let Some(id) = self.selected(cx) else { return };
        if let Some(session) = self.sessions.remove(&id) {
            self.runtime.close(session);
            let name = self
                .describe(&id)
                .map(|(_, name, _)| name)
                .unwrap_or_default();
            self.message(format!("Disconnected from {name}."), cx);
            cx.notify();
        }
    }
}

/// The dialog footer: Cancel and the dialog's own action, both reachable with Tab.
fn footer(action: &'static str) -> impl IntoElement {
    use gpui_component::button::{Button, ButtonVariants as _};
    use gpui_component::dialog::{DialogAction, DialogClose, DialogFooter};
    DialogFooter::new()
        .child(DialogClose::new().child(Button::new("cancel").label("Cancel (Esc)")))
        .child(DialogAction::new().child(Button::new("ok").primary().label(action)))
}

/// Forgets the open dialog once it closes, however it closed.
fn closer(
    explorer: &Entity<Explorer>,
) -> impl Fn(&gpui::ClickEvent, &mut Window, &mut App) + 'static {
    let explorer = explorer.clone();
    move |_, _, cx| explorer.update(cx, |this, _| this.dialog = None)
}

impl Focusable for Explorer {
    fn focus_handle(&self, _: &App) -> FocusHandle {
        self.focus.clone()
    }
}

/// What one row draws besides its label.
#[derive(Clone, Copy)]
struct RowLook {
    kind: Kind,
    colour: Option<gpui::Hsla>,
    status: Status,
}

impl Render for Explorer {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = cx.theme().clone();
        // The rows draw from a snapshot, because the tree's row closure outlives this call.
        let rows: Rc<HashMap<String, RowLook>> = Rc::new(
            self.saved
                .folders()
                .iter()
                .map(|f| f.id.clone())
                .chain(self.saved.connections().iter().map(|c| c.id.clone()))
                .filter_map(|id| {
                    let (kind, _, colour) = self.describe(&id)?;
                    let colour = colour.and_then(palette::hsla);
                    let status = self.status(&id);
                    Some((
                        id,
                        RowLook {
                            kind,
                            colour,
                            status,
                        },
                    ))
                })
                .collect(),
        );
        let empty = self.saved.folders().is_empty() && self.saved.connections().is_empty();
        let muted = theme.muted_foreground;
        let success = theme.success;

        div()
            .id("explorer")
            .key_context("Explorer")
            .track_focus(&self.focus)
            .size_full()
            .flex()
            .flex_col()
            .on_action(cx.listener(|this, _: &NewConnection, w, cx| this.new_connection(w, cx)))
            .on_action(cx.listener(|this, _: &NewFolder, w, cx| this.new_folder(w, cx)))
            .on_action(cx.listener(|this, _: &EditItem, w, cx| this.edit(w, cx)))
            .on_action(cx.listener(|this, _: &RenameItem, w, cx| this.rename(w, cx)))
            .on_action(cx.listener(|this, _: &DeleteItem, w, cx| this.delete(w, cx)))
            .on_action(cx.listener(|this, _: &CutItem, _, cx| this.cut(cx)))
            .on_action(cx.listener(|this, _: &PasteItem, _, cx| this.paste(false, cx)))
            .on_action(cx.listener(|this, _: &PasteItemAtTop, _, cx| this.paste(true, cx)))
            .on_action(cx.listener(|this, _: &ConnectItem, w, cx| this.activate(w, cx)))
            .on_action(cx.listener(|this, _: &DisconnectItem, _, cx| this.disconnect(cx)))
            .children(self.problem.clone().map(|problem| {
                div()
                    .px_2()
                    .text_sm()
                    .text_color(theme.danger)
                    .child(problem)
            }))
            .when(empty && self.problem.is_none(), |this| {
                this.child(
                    div().px_2().text_sm().text_color(muted).child(
                        "No connections yet. Insert adds a connection, Ctrl+Shift+N a folder.",
                    ),
                )
            })
            .child(
                tree(&self.tree, move |ix, entry, selected, _, _| {
                    let id = entry.item().id.to_string();
                    // A row always has its look: both come from the same saved tree.
                    let RowLook {
                        kind,
                        colour,
                        status,
                    } = rows.get(&id).copied().unwrap_or(RowLook {
                        kind: Kind::Connection,
                        colour: None,
                        status: Status::Disconnected,
                    });
                    let marker = match (kind, entry.is_expanded()) {
                        (Kind::Folder, true) => "▾",
                        (Kind::Folder, false) => "▸",
                        (Kind::Connection, _) => "",
                    };
                    let status = match status {
                        Status::Connected => Some(("connected", success)),
                        Status::Connecting => Some(("connecting…", muted)),
                        Status::Disconnected => None,
                    };
                    ListItem::new(ix).selected(selected).child(
                        div()
                            .flex()
                            .flex_row()
                            .items_center()
                            .gap_1()
                            .pl(px(12. * entry.depth() as f32))
                            .child(div().w(px(10.)).child(marker))
                            .child(
                                div()
                                    .size(px(8.))
                                    .rounded_full()
                                    .when_some(colour, |dot, colour| dot.bg(colour)),
                            )
                            .child(entry.item().label.clone())
                            .children(status.map(|(text, colour)| {
                                div().text_xs().text_color(colour).child(text)
                            })),
                    )
                })
                .flex_1(),
            )
    }
}
