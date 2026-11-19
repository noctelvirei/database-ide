//! The explorer pane: saved folders and connections as one tree, the dialogs that edit them,
//! connecting and disconnecting (`connect.rs`), and each connected connection's databases and
//! objects below it (`objects.rs`).
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
use dbide_engine::connections::{self, Connections, SavedConnection};
use dbide_engine::objects::Script;
use gpui::{
    App, AppContext as _, Context, Entity, EventEmitter, FocusHandle, Focusable, IntoElement,
    ParentElement as _, Render, SharedString, Styled as _, Window, div, prelude::*, px,
};
use gpui_component::dialog::DialogEntrance;
use gpui_component::input::{Input, InputState};
use gpui_component::list::ListItem;
use gpui_component::tree::{TreeEvent, TreeItem, TreeState, tree};
use gpui_component::{ActiveTheme as _, WindowExt as _};
use serde::Serialize;

use crate::connection_dialog::{ConnectionForm, FormState};
use crate::folder_dialog::{FolderForm, FolderState};
use crate::keymap::{
    ConnectItem, CutItem, DeleteItem, DisconnectItem, EditItem, FilterObjects, ItemMenu,
    NewConnection, NewFolder, PasteItem, PasteItemAtTop, RefreshItem, RenameItem, ScriptAlter,
    ScriptCreate, ScriptSelect,
};
use crate::palette;
use crate::query_tab::QueryTarget;
use crate::runtime::Runtime;

mod connect;
mod objects;
mod search;

use objects::{Branch, OpenMenu, Server, filtered};

/// What the explorer asks of the workbench.
pub enum ExplorerEvent {
    /// Something for the messages strip.
    Message(SharedString),
    /// Open a query tab on this connection; the password is already resolved.
    OpenQuery(Box<QueryTarget>),
    /// Open a query tab on this target holding `text`, from Script as.
    OpenScript {
        target: Box<QueryTarget>,
        text: String,
    },
    /// A restored tab's connection is signed in: hand it this target (`Explorer::resume`).
    Resume(Box<QueryTarget>),
}

/// One explorer row as the user sees it, top to bottom.
#[derive(Debug, Serialize)]
pub struct RowState {
    pub depth: usize,
    pub kind: Kind,
    pub name: String,
    /// The colour's name: the folder's own, or the connection's after inheritance.
    pub colour: Option<&'static str>,
    /// A connection's status; folders show `disconnected`, object rows have none.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub status: Option<Status>,
    pub selected: bool,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Kind {
    Folder,
    Connection,
    Database,
    /// A database's Tables, Views, Procedures or Functions.
    Group,
    Table,
    View,
    Procedure,
    Function,
    Index,
    /// A placeholder for a failed load or an empty list.
    Note,
    /// A placeholder for a list not loaded yet.
    Loading,
}

impl Kind {
    /// True for the rows that stand in for a list rather than show an item.
    pub fn is_placeholder(self) -> bool {
        matches!(self, Kind::Note | Kind::Loading)
    }
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
    pub filter: String,
    /// The open item menu's entries, top to bottom.
    pub menu: Option<Vec<SharedString>>,
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
    /// Connected connections, by ID.
    servers: HashMap<String, Server>,
    /// Connections being connected, each with its attempt number. A finished attempt keeps
    /// its session only while its number is still here: editing, deleting or disconnecting
    /// the connection removes the entry, and the late session is closed.
    connecting: HashMap<String, u64>,
    /// The number the next connect attempt gets.
    next_attempt: u64,
    /// Folders the user has opened, by ID.
    expanded: HashSet<String>,
    /// The item Ctrl+X picked up, waiting for Ctrl+V.
    cut: Option<String>,
    dialog: Option<OpenDialog>,
    /// The kind and label of each object row now in the tree, by ID.
    object_rows: HashMap<String, (Kind, String)>,
    filter: Entity<InputState>,
    menu: Option<OpenMenu>,
    /// The connection Ctrl+Enter searches when the filter has hidden the selected row.
    search_target: Option<String>,
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
            |this: &mut Self, _, event: &TreeEvent, cx| match event {
                TreeEvent::Expanded(id) => {
                    this.expanded.insert(id.to_string());
                    this.row_opened(id, cx);
                }
                TreeEvent::Collapsed(id) => {
                    this.expanded.remove(id.as_ref());
                }
            },
        )
        .detach();
        let focus = cx.focus_handle();
        // A click on the pane's empty space focuses the pane itself.
        cx.on_focus(&focus, window, |this, window, cx| this.focus(window, cx))
            .detach();
        let filter = Self::new_filter(window, cx);
        let mut this = Self {
            focus,
            tree,
            path,
            saved,
            problem,
            credentials: Rc::new(credentials),
            runtime,
            servers: HashMap::new(),
            connecting: HashMap::new(),
            next_attempt: 0,
            expanded: HashSet::new(),
            cut: None,
            dialog: None,
            object_rows: HashMap::new(),
            filter,
            menu: None,
            search_target: None,
        };
        this.rebuild(None, cx);
        this
    }

    /// Gives the explorer keyboard focus. The tree owns the arrow keys, so focus goes to the
    /// tree, with the first row selected if none is, so the arrows have somewhere to start.
    ///
    /// Callers use this rather than focusing the pane's handle: gpui runs focus listeners
    /// only while the window is active, so a listener alone misses a focus change made while
    /// another window is in front.
    pub fn focus(&self, window: &mut Window, cx: &mut Context<Self>) {
        self.tree.update(cx, |tree, cx| {
            if tree.selected_index().is_none() && tree.entry(0).is_some() {
                tree.set_selected_index(Some(0), cx);
            }
            tree.focus(window, cx);
        });
    }

    /// True while a connection is connecting, the tree is loading, or the open dialog is
    /// testing a connection.
    pub fn is_busy(&self, cx: &App) -> bool {
        !self.connecting.is_empty()
            || self.servers.values().any(Server::is_busy)
            || matches!(&self.dialog, Some(OpenDialog::Connection(form)) if form.read(cx).is_testing())
    }

    pub fn state(&self, cx: &App) -> ExplorerState {
        let tree = self.tree.read(cx);
        let selected = tree.selected_index();
        let rows = (0..)
            .map_while(|ix| tree.entry(ix).map(|entry| (ix, entry)))
            .filter_map(|(ix, entry)| {
                let id = entry.item().id.as_ref();
                let (kind, name, colour, status) = match self.describe(id) {
                    Some((kind, name, colour)) => (kind, name, colour, Some(self.status(id))),
                    None => {
                        let (kind, name) = self.object_rows.get(id)?.clone();
                        (kind, name, None, None)
                    }
                };
                Some(RowState {
                    depth: entry.depth(),
                    kind,
                    name,
                    colour: palette::name(colour),
                    status,
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
            filter: self.filter.read(cx).value().to_string(),
            menu: self.menu.as_ref().map(|open| open.labels.clone()),
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
        if self.servers.contains_key(id) {
            Status::Connected
        } else if self.connecting.contains_key(id) {
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

    /// Rebuilds the tree from the saved connections and what each connected one has loaded,
    /// keeping the open rows and showing only what the filter keeps, and selects `select`
    /// (or keeps the current selection).
    fn rebuild(&mut self, select: Option<&str>, cx: &mut Context<Self>) {
        let select = select.map(str::to_owned).or_else(|| self.selected(cx));
        let branches = filtered(self.items(None), &self.needle(cx));
        self.object_rows.clear();
        let items = self.tree_items(branches);
        self.tree.update(cx, |tree, cx| {
            tree.set_items(items, cx);
            let ix = select.and_then(|id| tree.index_of(&id.into()));
            tree.set_selected_index(ix, cx);
        });
        cx.notify();
    }

    /// The rows under `parent`: folders first, then connections, in saved order.
    fn items(&self, parent: Option<&str>) -> Vec<Branch> {
        let folders = self
            .saved
            .folders()
            .iter()
            .filter(|f| f.parent.as_deref() == parent)
            .map(|f| Branch {
                id: f.id.clone(),
                label: f.name.clone(),
                kind: Kind::Folder,
                children: self.items(Some(&f.id)),
                expanded: self.expanded.contains(&f.id),
            });
        let connections = self
            .saved
            .connections()
            .iter()
            .filter(|c| c.parent.as_deref() == parent)
            .map(|c| Branch {
                id: c.id.clone(),
                label: c.connection.name.clone(),
                kind: Kind::Connection,
                children: self.object_branches(&c.id),
                expanded: self.expanded.contains(&c.id),
            });
        folders.chain(connections).collect()
    }

    /// The tree's items for `branches`, noting each object row's kind and label.
    fn tree_items(&mut self, branches: Vec<Branch>) -> Vec<TreeItem> {
        branches
            .into_iter()
            .map(|branch| {
                if !matches!(branch.kind, Kind::Folder | Kind::Connection) {
                    self.object_rows
                        .insert(branch.id.clone(), (branch.kind, branch.label.clone()));
                }
                let children = self.tree_items(branch.children);
                TreeItem::new(branch.id, branch.label)
                    .children(children)
                    .expanded(branch.expanded)
            })
            .collect()
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

    /// Applies `edit` to a copy of the tree and writes the copy to `connections.toml`. The
    /// explorer takes the copy only once the file is written, so a failed edit or save leaves
    /// the screen and the file as they were. Edits that drop saved passwords return the
    /// connections whose passwords to delete, and callers delete them only after this
    /// succeeds (DAT-44).
    fn commit<T>(
        &mut self,
        edit: impl FnOnce(&mut Connections) -> Result<T, connections::Error>,
    ) -> Result<T, connections::Error> {
        let mut next = self.saved.clone();
        let value = edit(&mut next)?;
        next.save(&self.path)?;
        self.saved = next;
        Ok(value)
    }

    /// Opens the new connection dialog (Insert, or File, New connection from anywhere).
    pub fn new_connection(&mut self, window: &mut Window, cx: &mut Context<Self>) {
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
                .entrance(DialogEntrance::None)
                .w(px(520.))
                .child(form.clone())
                .footer(footer("Save (Enter)"))
                .on_ok(save)
                .on_close(closer(&explorer))
        });
    }

    /// Saves the dialog's connection and its password. Returns false to keep the dialog
    /// open, with the reason shown, when the fields are wrong or the save fails. Saving an
    /// edit disconnects the connection, so the next connect uses the new details.
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
        let mut deletions = Vec::new();
        let saved = self.commit(|saved| match editing {
            Some(id) => saved
                .update_connection(id, output.connection)
                .map(|pending| {
                    deletions = pending;
                    id.to_owned()
                }),
            None => saved.add_connection(output.connection, parent),
        });
        let id = match saved {
            Ok(id) => id,
            Err(error) => {
                let text = format!("Couldn't save {name}: {error}.").into();
                form.update(cx, |form, cx| form.show_error(text, cx));
                return false;
            }
        };
        for id in deletions {
            if let Err(error) = self.credentials.delete(&id) {
                self.message(
                    format!("Saved {name}, but couldn't delete its saved password: {error}."),
                    cx,
                );
            }
        }
        if editing.is_some() && self.forget_session(&id) {
            self.message(
                format!("Disconnected from {name}, because its details changed."),
                cx,
            );
        }
        if let (Some(password), Some(saved)) = (output.password, self.connection(&id))
            && let Err(error) = self.credentials.store(saved, &password)
        {
            self.message(format!("Saved {name}, but not its password: {error}."), cx);
        }
        if let Some(parent) = parent {
            self.expanded.insert(parent.to_owned());
        }
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
                .entrance(DialogEntrance::None)
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
        let name = output.name.clone();
        let saved = self.commit(|saved| {
            let id = match editing {
                Some(id) => saved.rename(id, &output.name).map(|()| id.to_owned()),
                None => saved.add_folder(&output.name, parent),
            }?;
            saved.set_folder_settings(&id, output.colour, output.danger_check)?;
            Ok(id)
        });
        match saved {
            Ok(id) => {
                if let Some(parent) = parent {
                    self.expanded.insert(parent.to_owned());
                }
                self.rebuild(Some(&id), cx);
                true
            }
            Err(error) => {
                let text = format!("Couldn't save folder {name}: {error}.").into();
                form.update(cx, |form, cx| form.show_error(text, cx));
                false
            }
        }
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
                        if let Err(error) = this.commit(|saved| saved.rename(&id, name)) {
                            // The dialog stays open with the name, so the user can retry.
                            this.message(format!("Couldn't rename: {error}."), cx);
                            return false;
                        }
                        this.rebuild(Some(&id), cx);
                        true
                    })
                }
            };
            dialog
                .title("Rename")
                .entrance(DialogEntrance::None)
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
            // `describe` covers saved rows only, so anything else is a connection.
            _ => format!("Delete connection {name} and its saved password?"),
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
                .entrance(DialogEntrance::None)
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
        let deletions = match self.commit(|saved| saved.delete(id)) {
            Ok(deletions) => deletions,
            Err(error) => {
                self.message(format!("Couldn't delete: {error}."), cx);
                return;
            }
        };
        for id in deletions {
            if let Err(error) = self.credentials.delete(&id) {
                self.message(
                    format!(
                        "Deleted the connection, but couldn't delete its saved password: {error}."
                    ),
                    cx,
                );
            }
        }
        // Close the sessions of every deleted connection.
        let kept: HashSet<String> = self
            .saved
            .connections()
            .iter()
            .map(|c| c.id.clone())
            .collect();
        for gone in before.difference(&kept) {
            self.forget_session(gone);
        }
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
        match self.commit(|saved| saved.move_to(&id, target.as_deref())) {
            Ok(()) => {
                self.cut = None;
                if let Some(target) = &target {
                    self.expanded.insert(target.clone());
                }
                self.rebuild(Some(&id), cx);
            }
            Err(connections::Error::Invalid(_)) => {
                self.message("A folder can't move into itself or a folder inside it.", cx);
            }
            Err(error) => self.message(format!("Couldn't move: {error}."), cx),
        }
    }
}

/// The dialog footer: Cancel and the dialog's own action, both reachable with Tab.
pub(crate) fn footer(action: &'static str) -> impl IntoElement {
    use gpui_component::button::{Button, ButtonVariants as _};
    use gpui_component::dialog::{DialogAction, DialogClose, DialogFooter};
    DialogFooter::new()
        .child(DialogClose::new().child(Button::new("cancel").label("Cancel (Esc)")))
        .child(DialogAction::new().child(Button::new("ok").primary().label(action)))
}

/// Forgets the open dialog once it closes, however it closed, and gives focus back to the
/// explorer, which opened it.
fn closer(
    explorer: &Entity<Explorer>,
) -> impl Fn(&gpui::ClickEvent, &mut Window, &mut App) + 'static {
    let explorer = explorer.clone();
    move |_, window, cx| {
        explorer.update(cx, |this, cx| {
            // A dialog that handed work elsewhere (a query tab) cleared itself, and focus
            // stays where that work put it.
            if this.dialog.take().is_some() {
                this.focus(window, cx);
            }
        })
    }
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
    status: Option<Status>,
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
                    let status = (kind == Kind::Connection).then(|| self.status(&id));
                    Some((
                        id,
                        RowLook {
                            kind,
                            colour,
                            status,
                        },
                    ))
                })
                .chain(self.object_rows.iter().map(|(id, (kind, _))| {
                    let look = RowLook {
                        kind: *kind,
                        colour: None,
                        status: None,
                    };
                    (id.clone(), look)
                }))
                .collect(),
        );
        let empty = self.saved.folders().is_empty() && self.saved.connections().is_empty();
        let muted = theme.muted_foreground;
        let success = theme.success;

        div()
            .id("explorer")
            .key_context("Explorer")
            .track_focus(&self.focus)
            .relative()
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
            .on_action(cx.listener(|this, _: &ConnectItem, w, cx| {
                // Enter in the filter box or the item menu also bubbles here; it belongs to them.
                let filtering = this.filter.focus_handle(cx).contains_focused(w, cx);
                if !filtering && this.menu.is_none() {
                    this.activate(w, cx)
                }
            }))
            .on_action(cx.listener(|this, _: &DisconnectItem, _, cx| this.disconnect(cx)))
            .on_action(cx.listener(|this, _: &FilterObjects, w, cx| this.focus_filter(w, cx)))
            .on_action(cx.listener(|this, _: &ItemMenu, w, cx| this.open_menu(w, cx)))
            .on_action(cx.listener(|this, _: &ScriptCreate, _, cx| this.script(Script::Create, cx)))
            .on_action(cx.listener(|this, _: &ScriptAlter, _, cx| this.script(Script::Alter, cx)))
            .on_action(cx.listener(|this, _: &ScriptSelect, _, cx| this.script(Script::Select, cx)))
            .on_action(cx.listener(|this, _: &RefreshItem, _, cx| this.refresh(cx)))
            .child(self.filter_box())
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
                        status: None,
                    });
                    let marker = match (entry.is_folder(), entry.is_expanded()) {
                        (true, true) => "▾",
                        (true, false) => "▸",
                        (false, _) => "",
                    };
                    let status = match status {
                        Some(Status::Connected) => Some(("connected", success)),
                        Some(Status::Connecting) => Some(("connecting…", muted)),
                        Some(Status::Disconnected) | None => None,
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
                            .child(
                                div()
                                    .when(kind.is_placeholder(), |label| label.text_color(muted))
                                    .child(entry.item().label.clone()),
                            )
                            .children(status.map(|(text, colour)| {
                                div().text_xs().text_color(colour).child(text)
                            })),
                    )
                })
                .flex_1(),
            )
            .children(self.menu_overlay())
    }
}
