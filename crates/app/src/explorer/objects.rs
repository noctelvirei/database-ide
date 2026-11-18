//! The objects under a connected connection (docs/spec.md, "Object explorer and Script as"):
//! its databases, each database's tables, views, procedures and functions, and each table's
//! indexes, each list loaded the first time its row is opened. Also the filter box, the item
//! menu (the context-menu key or Shift+F10) and Script as, which opens the script in a new
//! query tab.
//!
//! A connected connection keeps the session it connected with, which lists the databases, and
//! opens one more session per database the user opens, because a PostgreSQL session can't
//! change database. A session makes one catalog call at a time; calls that arrive while it is
//! busy wait in its queue. Each call carries the connect attempt it was made under, and a reply
//! for a connection since disconnected, edited or deleted closes its session instead of
//! keeping it.

use std::collections::{HashMap, VecDeque};

use dbide_engine::connections::SavedConnection;
use dbide_engine::danger::Engine;
use dbide_engine::objects::{self, Index, Kind as ObjectKind, Object, Script};
use dbide_engine::session::Session;
use gpui::{
    Action, AppContext as _, Context, DismissEvent, Entity, Focusable as _, IntoElement,
    ParentElement as _, SharedString, Styled as _, Window, div, px,
};
use gpui_component::input::{Input, InputEvent, InputState};
use gpui_component::menu::PopupMenu;

use super::{Explorer, ExplorerEvent, Kind};
use crate::keymap::{RefreshItem, ScriptAlter, ScriptCreate, ScriptSelect};
use crate::query_tab::{HeldPassword, QueryTarget};
use crate::runtime::{Explore, Explored, Runtime};

/// Separates the parts of an object row's ID; saved folder and connection IDs never hold it.
const SEP: char = '\u{1f}';

/// What a connected connection holds for its object tree.
pub(super) struct Server {
    /// The connect attempt this server came from; replies made under another are stale.
    attempt: u64,
    /// The details the connection connected with; database sessions open from them.
    saved: SavedConnection,
    /// The sign-in password, held while connected so database sessions and query tabs
    /// opened from the tree don't ask again.
    password: Option<HeldPassword>,
    /// The session the connection connected with; it lists the databases.
    root: Slot,
    databases: Load<Vec<String>>,
    /// The databases the user has opened, by name.
    catalogs: HashMap<String, Catalog>,
}

/// One database's session and what it has loaded.
#[derive(Default)]
struct Catalog {
    slot: Slot,
    objects: Load<Vec<Object>>,
    /// Indexes by their table's catalog key.
    indexes: HashMap<String, Load<Vec<Index>>>,
}

/// One session and the catalog calls waiting for it.
#[derive(Default)]
struct Slot {
    /// Absent while a call or the connect is under way, and before a database session opens.
    session: Option<Box<dyn Session>>,
    busy: bool,
    queue: VecDeque<Explore>,
}

/// A list's load state.
#[derive(Default)]
enum Load<T> {
    #[default]
    Unloaded,
    Loading,
    Loaded(T),
    Failed(String),
}

impl<T> From<Result<T, String>> for Load<T> {
    fn from(outcome: Result<T, String>) -> Self {
        outcome.map_or_else(Load::Failed, Load::Loaded)
    }
}

impl Server {
    pub(super) fn new(
        attempt: u64,
        saved: SavedConnection,
        password: Option<String>,
        session: Box<dyn Session>,
    ) -> Self {
        Self {
            attempt,
            saved,
            password: password.map(HeldPassword::new),
            root: Slot {
                session: Some(session),
                ..Slot::default()
            },
            databases: Load::Unloaded,
            catalogs: HashMap::new(),
        }
    }

    /// Closes every idle session; sessions busy in a call are closed when it replies.
    pub(super) fn close(self, runtime: &Runtime) {
        let catalogs = self.catalogs.into_values().map(|c| c.slot);
        for slot in std::iter::once(self.root).chain(catalogs) {
            if let Some(session) = slot.session {
                runtime.close(session);
            }
        }
    }

    pub(super) fn is_busy(&self) -> bool {
        std::iter::once(&self.root)
            .chain(self.catalogs.values().map(|c| &c.slot))
            .any(|slot| slot.busy || !slot.queue.is_empty())
    }

    fn engine(&self) -> Engine {
        self.saved.connection.engine
    }

    fn slot(&mut self, database: Option<&str>) -> Option<&mut Slot> {
        match database {
            None => Some(&mut self.root),
            Some(database) => self.catalogs.get_mut(database).map(|c| &mut c.slot),
        }
    }
}

impl Catalog {
    /// The loaded object or index with catalog key `key`.
    fn find(&self, key: &str) -> Option<&Object> {
        let Load::Loaded(objects) = &self.objects else {
            return None;
        };
        objects.iter().find(|o| o.id.key == key).or_else(|| {
            self.indexes.values().find_map(|load| match load {
                Load::Loaded(indexes) => {
                    indexes.iter().map(|i| &i.object).find(|o| o.id.key == key)
                }
                _ => None,
            })
        })
    }

    /// Forgets everything loaded, so opening the rows loads them again.
    fn forget(&mut self) {
        self.objects = Load::Unloaded;
        self.indexes.clear();
    }
}

/// The groups a database's objects are listed in, in display order.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Group {
    Tables,
    Views,
    Procedures,
    Functions,
}

impl Group {
    const ALL: [Group; 4] = [
        Group::Tables,
        Group::Views,
        Group::Procedures,
        Group::Functions,
    ];

    fn of(kind: ObjectKind) -> Option<Group> {
        match kind {
            ObjectKind::Table => Some(Group::Tables),
            ObjectKind::View | ObjectKind::MaterializedView => Some(Group::Views),
            ObjectKind::Procedure => Some(Group::Procedures),
            ObjectKind::Function => Some(Group::Functions),
            _ => None,
        }
    }

    fn label(self) -> &'static str {
        match self {
            Group::Tables => "Tables",
            Group::Views => "Views",
            Group::Procedures => "Procedures",
            Group::Functions => "Functions",
        }
    }

    fn tag(self) -> &'static str {
        match self {
            Group::Tables => "t",
            Group::Views => "v",
            Group::Procedures => "p",
            Group::Functions => "f",
        }
    }
}

/// What an object row stands for, read from its ID. The IDs are the parts joined by `SEP`:
/// `connection d database`, `connection g database group`, `connection o database key`, and
/// a placeholder's is its parent's ID followed by `n`.
enum Node {
    Database {
        connection: String,
        database: String,
    },
    Group {
        connection: String,
        database: String,
    },
    Object {
        connection: String,
        database: String,
        key: String,
    },
    Note,
}

impl Node {
    /// `None` for a saved folder's or connection's ID.
    fn parse(id: &str) -> Option<Node> {
        let parts: Vec<&str> = id.split(SEP).collect();
        let owned = |s: &&str| (*s).to_owned();
        Some(match parts.as_slice() {
            [connection, "d", database] => Node::Database {
                connection: owned(connection),
                database: owned(database),
            },
            [connection, "g", database, _] => Node::Group {
                connection: owned(connection),
                database: owned(database),
            },
            [connection, "o", database, key] => Node::Object {
                connection: owned(connection),
                database: owned(database),
                key: owned(key),
            },
            [_, .., "n"] => Node::Note,
            _ => return None,
        })
    }

    /// The connection and database the row belongs to; `None` for a placeholder.
    fn place(&self) -> Option<(&str, &str)> {
        match self {
            Node::Database {
                connection,
                database,
            }
            | Node::Group {
                connection,
                database,
            }
            | Node::Object {
                connection,
                database,
                ..
            } => Some((connection, database)),
            Node::Note => None,
        }
    }
}

fn row_id(parts: &[&str]) -> String {
    parts.join(&SEP.to_string())
}

/// One tree row before filtering, with every row under it.
pub(super) struct Branch {
    pub id: String,
    pub label: String,
    pub kind: Kind,
    pub children: Vec<Branch>,
    pub expanded: bool,
}

impl Branch {
    /// A placeholder row under `parent`: a failed load or an empty list.
    fn note(parent: &str, label: impl Into<String>) -> Branch {
        Branch {
            id: format!("{parent}{SEP}n"),
            label: label.into(),
            kind: Kind::Note,
            children: Vec::new(),
            expanded: false,
        }
    }

    /// The placeholder under `parent` while its list loads, or before it first does.
    fn loading(parent: &str) -> Branch {
        Branch {
            kind: Kind::Loading,
            ..Branch::note(parent, "Loading…")
        }
    }

    /// True for a real row whose label holds `needle`; placeholders never match.
    fn matches(&self, needle: &str) -> bool {
        !self.kind.is_placeholder() && self.label.to_lowercase().contains(needle)
    }
}

/// The rows the filter `needle` (lower case) keeps: a row that matches with everything under
/// it, a row with a match below it, opened so the match shows, and an open row whose list is
/// loading, so a refresh under the filter keeps its place and selection. An empty filter
/// keeps all.
pub(super) fn filtered(branches: Vec<Branch>, needle: &str) -> Vec<Branch> {
    if needle.is_empty() {
        return branches;
    }
    branches
        .into_iter()
        .filter_map(|mut branch| {
            if branch.matches(needle) {
                return Some(branch);
            }
            let loading = branch.expanded
                && matches!(branch.children.as_slice(), [only] if only.kind == Kind::Loading);
            if !loading {
                branch.children = filtered(branch.children, needle);
                branch.expanded = true;
            }
            (!branch.children.is_empty()).then_some(branch)
        })
        .collect()
}

/// The children placeholder or rows for a list in state `load`.
fn listed<T>(
    parent: &str,
    load: Option<&Load<Vec<T>>>,
    what: &str,
    rows: impl FnOnce(&[T]) -> Vec<Branch>,
) -> Vec<Branch> {
    match load {
        None | Some(Load::Unloaded | Load::Loading) => vec![Branch::loading(parent)],
        Some(Load::Failed(error)) => vec![Branch::note(
            parent,
            format!("Couldn't list {what}: {error}"),
        )],
        Some(Load::Loaded(items)) if items.is_empty() => {
            vec![Branch::note(parent, format!("No {what}"))]
        }
        Some(Load::Loaded(items)) => rows(items),
    }
}

/// An object's label: `schema.name`, or the name alone on MySQL, where the schema is the
/// database; a routine's arguments follow its name.
fn label(engine: Engine, object: &Object) -> String {
    let name = match &object.schema {
        Some(schema) if engine != Engine::MySql => format!("{schema}.{}", object.name),
        _ => object.name.clone(),
    };
    match &object.signature {
        Some(signature) => format!("{name}({signature})"),
        None => name,
    }
}

fn row_kind(kind: ObjectKind) -> Kind {
    match kind {
        ObjectKind::Table => Kind::Table,
        ObjectKind::View | ObjectKind::MaterializedView => Kind::View,
        ObjectKind::Procedure => Kind::Procedure,
        ObjectKind::Function => Kind::Function,
        _ => Kind::Index,
    }
}

fn supports(engine: Engine, kind: ObjectKind, form: Script) -> bool {
    match engine {
        Engine::SqlServer => objects::mssql::supports(kind, form),
        Engine::PostgreSql => objects::postgres::supports(kind, form),
        Engine::MySql => objects::mysql::supports(kind, form),
    }
}

/// The open item menu and its labels, top to bottom, for the UI script.
pub(super) struct OpenMenu {
    menu: Entity<PopupMenu>,
    pub labels: Vec<SharedString>,
}

impl Explorer {
    /// The filter box, filtering the rows as the user types.
    pub(super) fn new_filter(window: &mut Window, cx: &mut Context<Self>) -> Entity<InputState> {
        let filter = cx.new(|cx| InputState::new(window, cx).placeholder("Filter (Ctrl+F)"));
        cx.subscribe_in(
            &filter,
            window,
            |this, _, event: &InputEvent, window, cx| match event {
                InputEvent::Change => this.rebuild(None, cx),
                InputEvent::PressEnter { .. } => this.select_first_match(window, cx),
                InputEvent::Focus | InputEvent::Blur => {}
            },
        )
        .detach();
        filter
    }

    /// The filter's text, trimmed and in lower case.
    pub(super) fn needle(&self, cx: &gpui::App) -> String {
        self.filter.read(cx).value().trim().to_lowercase()
    }

    /// Ctrl+F: moves focus to the filter box.
    pub(super) fn focus_filter(&self, window: &mut Window, cx: &mut Context<Self>) {
        self.filter.focus_handle(cx).focus(window, cx);
    }

    /// Enter in the filter box: back to the tree, on the first row that matches.
    fn select_first_match(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let needle = self.needle(cx);
        let rows = &self.object_rows;
        self.tree.update(cx, |tree, cx| {
            let first = (0..).map_while(|ix| tree.entry(ix)).position(|entry| {
                let item = entry.item();
                let note = rows
                    .get(item.id.as_ref())
                    .is_some_and(|(k, _)| k.is_placeholder());
                !note && item.label.to_lowercase().contains(&needle)
            });
            if first.is_some() {
                tree.set_selected_index(first, cx);
            }
        });
        self.focus(window, cx);
    }

    pub(super) fn filter_box(&self) -> impl IntoElement {
        div().px_2().pb_1().child(Input::new(&self.filter))
    }

    /// The rows under connection `id`: its databases once connected, otherwise none.
    pub(super) fn object_branches(&self, id: &str) -> Vec<Branch> {
        let Some(server) = self.servers.get(id) else {
            return Vec::new();
        };
        listed(id, Some(&server.databases), "databases", |names| {
            names
                .iter()
                .map(|database| self.database_branch(id, server, database))
                .collect()
        })
    }

    fn database_branch(&self, connection: &str, server: &Server, database: &str) -> Branch {
        let id = row_id(&[connection, "d", database]);
        let catalog = server.catalogs.get(database);
        let children = listed(
            &id,
            catalog.map(|c| &c.objects),
            "tables, views, procedures or functions",
            |objects| {
                Group::ALL
                    .into_iter()
                    .filter_map(|group| {
                        let members: Vec<Branch> = objects
                            .iter()
                            .filter(|o| Group::of(o.kind) == Some(group))
                            .map(|o| self.object_branch(connection, server, database, o))
                            .collect();
                        let id = row_id(&[connection, "g", database, group.tag()]);
                        (!members.is_empty()).then(|| Branch {
                            expanded: self.expanded.contains(&id),
                            id,
                            label: group.label().to_owned(),
                            kind: Kind::Group,
                            children: members,
                        })
                    })
                    .collect()
            },
        );
        Branch {
            expanded: self.expanded.contains(&id),
            id,
            label: database.to_owned(),
            kind: Kind::Database,
            children,
        }
    }

    fn object_branch(
        &self,
        connection: &str,
        server: &Server,
        database: &str,
        object: &Object,
    ) -> Branch {
        let id = row_id(&[connection, "o", database, &object.id.key]);
        let children = if object.kind == ObjectKind::Table {
            let indexes = server
                .catalogs
                .get(database)
                .and_then(|c| c.indexes.get(&object.id.key));
            listed(&id, indexes, "indexes", |indexes| {
                indexes
                    .iter()
                    .map(|index| Branch {
                        id: row_id(&[connection, "o", database, &index.object.id.key]),
                        label: index.object.name.clone(),
                        kind: Kind::Index,
                        children: Vec::new(),
                        expanded: false,
                    })
                    .collect()
            })
        } else {
            Vec::new()
        };
        Branch {
            expanded: self.expanded.contains(&id),
            id,
            label: label(server.engine(), object),
            kind: row_kind(object.kind),
            children,
        }
    }

    /// Starts loading what row `id` lists, the first time it is opened.
    pub(super) fn row_opened(&mut self, id: &str, cx: &mut Context<Self>) {
        match Node::parse(id) {
            None => {
                if let Some(server) = self.servers.get_mut(id)
                    && matches!(server.databases, Load::Unloaded)
                {
                    server.databases = Load::Loading;
                    self.request(id, None, Explore::Databases, cx);
                }
            }
            Some(Node::Database {
                connection,
                database,
            }) => {
                let Some(server) = self.servers.get_mut(&connection) else {
                    return;
                };
                let catalog = server.catalogs.entry(database.clone()).or_default();
                if matches!(catalog.objects, Load::Unloaded) {
                    catalog.objects = Load::Loading;
                    self.request(&connection, Some(&database), Explore::Objects, cx);
                }
            }
            Some(Node::Object {
                connection,
                database,
                key,
            }) => {
                let Some(catalog) = self
                    .servers
                    .get_mut(&connection)
                    .and_then(|s| s.catalogs.get_mut(&database))
                else {
                    return;
                };
                let table = catalog
                    .find(&key)
                    .filter(|o| o.kind == ObjectKind::Table)
                    .cloned();
                if let Some(table) = table
                    && !catalog.indexes.contains_key(&key)
                {
                    catalog.indexes.insert(key, Load::Loading);
                    self.request(&connection, Some(&database), Explore::Indexes(table), cx);
                }
            }
            Some(Node::Group { .. } | Node::Note) => {}
        }
    }

    /// Enter on an object row: opens or closes it.
    pub(super) fn toggle(&mut self, id: &str, cx: &mut Context<Self>) {
        if !self.expanded.remove(id) {
            self.expanded.insert(id.to_owned());
            self.row_opened(id, cx);
        }
        self.rebuild(Some(id), cx);
    }

    /// Queues `call` on the connection's own session (`database` absent) or the database's.
    fn request(
        &mut self,
        connection: &str,
        database: Option<&str>,
        call: Explore,
        cx: &mut Context<Self>,
    ) {
        if let Some(slot) = self
            .servers
            .get_mut(connection)
            .and_then(|s| s.slot(database))
        {
            slot.queue.push_back(call);
            self.pump(connection, database, cx);
        }
    }

    /// Starts the slot's next call if its session is free, opening a database's session
    /// first if it has none.
    fn pump(&mut self, connection: &str, database: Option<&str>, cx: &mut Context<Self>) {
        let Some(server) = self.servers.get_mut(connection) else {
            return;
        };
        let (attempt, engine) = (server.attempt, server.engine());
        let mut details = server.saved.connection.clone();
        let password = server.password.as_ref().map(|p| p.expose().to_owned());
        let Some(slot) = server.slot(database) else {
            return;
        };
        if slot.busy || slot.queue.is_empty() {
            return;
        }
        slot.busy = true;
        let connection = connection.to_owned();
        let database = database.map(str::to_owned);
        match slot.session.take() {
            Some(session) => {
                let call = slot.queue.pop_front().expect("the queue isn't empty");
                let reply = self.runtime.explore(session, engine, call.clone());
                cx.spawn(async move |this, cx| {
                    let (session, outcome) = reply.await;
                    let _ = this.update(cx, |this, cx| {
                        this.replied(&connection, database.as_deref(), attempt, session, cx);
                        this.apply(&connection, database.as_deref(), call, outcome, cx);
                    });
                })
                .detach();
            }
            None => {
                let Some(database) = database else {
                    unreachable!("the connection's own session is away only while it is busy")
                };
                details.database = Some(database.clone());
                let opening = self.runtime.connect(details, password);
                cx.spawn(async move |this, cx| {
                    let outcome = opening.await.map_err(|error| error.message);
                    let _ = this.update(cx, |this, cx| {
                        this.opened(&connection, &database, attempt, outcome, cx)
                    });
                })
                .detach();
            }
        }
    }

    /// A call finished: its session goes back to the slot, or is closed if the connection
    /// has since gone, and the slot's next call starts.
    fn replied(
        &mut self,
        connection: &str,
        database: Option<&str>,
        attempt: u64,
        session: Box<dyn Session>,
        cx: &mut Context<Self>,
    ) {
        match self
            .servers
            .get_mut(connection)
            .filter(|s| s.attempt == attempt)
            .and_then(|s| s.slot(database))
        {
            Some(slot) => {
                slot.session = Some(session);
                slot.busy = false;
                self.pump(connection, database, cx);
            }
            None => self.runtime.close(session),
        }
    }

    /// A database's session opened, or failed to; a failure fails every call waiting for
    /// it, and the next call tries to open it again.
    fn opened(
        &mut self,
        connection: &str,
        database: &str,
        attempt: u64,
        outcome: Result<Box<dyn Session>, String>,
        cx: &mut Context<Self>,
    ) {
        let Some(slot) = self
            .servers
            .get_mut(connection)
            .filter(|s| s.attempt == attempt)
            .and_then(|s| s.slot(Some(database)))
        else {
            if let Ok(session) = outcome {
                self.runtime.close(session);
            }
            return;
        };
        slot.busy = false;
        match outcome {
            Ok(session) => {
                slot.session = Some(session);
                self.pump(connection, Some(database), cx);
            }
            Err(error) => {
                let waiting = std::mem::take(&mut slot.queue);
                for call in waiting {
                    self.apply(connection, Some(database), call, Err(error.clone()), cx);
                }
            }
        }
        cx.notify();
    }

    /// Stores a call's outcome, or for Script as, opens the script in a new query tab.
    fn apply(
        &mut self,
        connection: &str,
        database: Option<&str>,
        call: Explore,
        outcome: Result<Explored, String>,
        cx: &mut Context<Self>,
    ) {
        if let Explore::Script(object, _) = &call {
            match outcome {
                Ok(Explored::Script(text)) => {
                    if let Some(target) = self.database_target(connection, &object.id.database) {
                        cx.emit(ExplorerEvent::OpenScript {
                            target: Box::new(target),
                            text,
                        });
                    }
                }
                Ok(_) => unreachable!("a script returns text"),
                Err(error) => {
                    self.message(format!("Couldn't script {}: {error}.", object.name), cx);
                }
            }
            return;
        }
        let Some(server) = self.servers.get_mut(connection) else {
            return;
        };
        let catalog = database.and_then(|d| server.catalogs.get_mut(d));
        match (call, outcome) {
            (Explore::Databases, outcome) => {
                server.databases = Load::from(outcome.map(|explored| match explored {
                    Explored::Databases(names) => names,
                    _ => unreachable!("a database list returns databases"),
                }));
            }
            (Explore::Objects, outcome) => {
                if let Some(catalog) = catalog {
                    catalog.objects = Load::from(outcome.map(|explored| match explored {
                        Explored::Objects(objects) => objects,
                        _ => unreachable!("an object list returns objects"),
                    }));
                }
            }
            (Explore::Indexes(table), outcome) => {
                if let Some(catalog) = catalog {
                    let indexes = Load::from(outcome.map(|explored| match explored {
                        Explored::Indexes(indexes) => indexes,
                        _ => unreachable!("an index list returns indexes"),
                    }));
                    catalog.indexes.insert(table.id.key, indexes);
                }
            }
            (Explore::Script(..), _) => unreachable!("Script as is handled above"),
        }
        // Rows left open below the new list load their own lists now.
        self.load_open_rows(cx);
        self.rebuild(None, cx);
    }

    /// Starts loading every open row's list that isn't loaded: after a connect, a refresh,
    /// or a list arriving under rows that were already open.
    pub(super) fn load_open_rows(&mut self, cx: &mut Context<Self>) {
        let open: Vec<String> = self.expanded.iter().cloned().collect();
        for id in open {
            self.row_opened(&id, cx);
        }
    }

    /// A query tab's target on `database` of connected connection `connection`.
    fn database_target(&self, connection: &str, database: &str) -> Option<QueryTarget> {
        let server = self.servers.get(connection)?;
        let mut saved = server.saved.clone();
        saved.connection.database = Some(database.to_owned());
        let settings = self.saved.effective_settings(connection).ok();
        Some(QueryTarget {
            saved,
            colour: settings.as_ref().and_then(|s| s.colour).map(str::to_owned),
            danger_check: settings.is_none_or(|s| s.danger_check),
            password: server.password.clone(),
        })
    }

    /// Ctrl+N on a database or object row: a query tab on that database. Returns false
    /// for any other row.
    pub(super) fn query_on_selected_database(&mut self, cx: &mut Context<Self>) -> bool {
        let target = self
            .selected(cx)
            .and_then(|id| Node::parse(&id))
            .and_then(|node| {
                let (connection, database) = node.place()?;
                self.database_target(connection, database)
            });
        match target {
            Some(target) => {
                cx.emit(ExplorerEvent::OpenQuery(Box::new(target)));
                true
            }
            None => false,
        }
    }

    /// The selected object, with the connection and database it is in.
    fn selected_object(&self, cx: &gpui::App) -> Option<(String, String, Object)> {
        let Node::Object {
            connection,
            database,
            key,
        } = Node::parse(&self.selected(cx)?)?
        else {
            return None;
        };
        let object = self
            .servers
            .get(&connection)?
            .catalogs
            .get(&database)?
            .find(&key)?
            .clone();
        Some((connection, database, object))
    }

    /// Script as `form` on the selected object.
    pub(super) fn script(&mut self, form: Script, cx: &mut Context<Self>) {
        if let Some((connection, database, object)) = self.selected_object(cx) {
            self.request(
                &connection,
                Some(&database),
                Explore::Script(object, form),
                cx,
            );
        }
    }

    /// Reloads what the selected row lists: a connection's databases, or a database's objects
    /// for any row inside it. Open rows load again at once.
    pub(super) fn refresh(&mut self, cx: &mut Context<Self>) {
        let Some(id) = self.selected(cx) else { return };
        match Node::parse(&id) {
            None => {
                let Some(server) = self.servers.get_mut(&id) else {
                    return;
                };
                server.databases = Load::Unloaded;
                server.catalogs.values_mut().for_each(Catalog::forget);
            }
            Some(node) => {
                let Some((connection, database)) = node.place() else {
                    return;
                };
                let Some(catalog) = self
                    .servers
                    .get_mut(connection)
                    .and_then(|s| s.catalogs.get_mut(database))
                else {
                    return;
                };
                catalog.forget();
            }
        }
        self.load_open_rows(cx);
        self.rebuild(None, cx);
    }

    /// The item menu's entries for row `id`: Script as for an object, and Refresh for any row
    /// of a connected connection.
    fn menu_items(&self, id: &str) -> Vec<(&'static str, Box<dyn Action>)> {
        let refresh: (&'static str, Box<dyn Action>) = ("Refresh", Box::new(RefreshItem));
        match Node::parse(id) {
            None if self.servers.contains_key(id) => vec![refresh],
            None | Some(Node::Note) => Vec::new(),
            Some(Node::Database { .. } | Node::Group { .. }) => vec![refresh],
            Some(Node::Object {
                connection,
                database,
                key,
            }) => {
                let Some(server) = self.servers.get(&connection) else {
                    return Vec::new();
                };
                let Some(object) = server.catalogs.get(&database).and_then(|c| c.find(&key)) else {
                    return Vec::new();
                };
                let forms: [(&'static str, Script, Box<dyn Action>); 3] = [
                    ("Script as CREATE", Script::Create, Box::new(ScriptCreate)),
                    ("Script as ALTER", Script::Alter, Box::new(ScriptAlter)),
                    ("Script as SELECT", Script::Select, Box::new(ScriptSelect)),
                ];
                forms
                    .into_iter()
                    .filter(|(_, form, _)| supports(server.engine(), object.kind, *form))
                    .map(|(label, _, action)| (label, action))
                    .chain([refresh])
                    .collect()
            }
        }
    }

    /// The context-menu key or Shift+F10: the selected row's menu. As in Windows' menus,
    /// nothing is highlighted until Down, and Escape closes it.
    pub(super) fn open_menu(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(id) = self.selected(cx) else { return };
        let items = self.menu_items(&id);
        if items.is_empty() {
            self.message(
                "Nothing to do here. Script as works on objects; Refresh on a connected connection.",
                cx,
            );
            return;
        }
        let labels = items.iter().map(|(label, _)| (*label).into()).collect();
        // The chosen action goes to the explorer, which handles it for the selected row.
        let context = self.focus.clone();
        let menu = PopupMenu::build(window, cx, move |mut menu, _, _| {
            for (label, action) in items {
                menu = menu.menu(label, action);
            }
            menu.action_context(context)
        });
        cx.subscribe_in(&menu, window, |this, _, _: &DismissEvent, window, cx| {
            this.menu = None;
            this.focus(window, cx);
            cx.notify();
        })
        .detach();
        menu.focus_handle(cx).focus(window, cx);
        self.menu = Some(OpenMenu { menu, labels });
        cx.notify();
    }

    pub(super) fn menu_overlay(&self) -> Option<impl IntoElement> {
        let open = self.menu.as_ref()?;
        Some(
            div()
                .absolute()
                .top(px(40.))
                .left(px(24.))
                .child(open.menu.clone()),
        )
    }
}
