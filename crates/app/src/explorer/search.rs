//! Searching every database of a connection for objects by name (docs/spec.md, "Object
//! explorer and Script as"). The filter box only filters the rows already loaded; Ctrl+Enter
//! in it searches every user database of the selected connection, including ones never
//! opened, and lists the hits under the connection in a "Search" row. A hit works like an
//! object row: Script as, Ctrl+N on its database. Databases the search couldn't open are
//! named in the messages strip with the reason.
//!
//! A new search replaces the last one on that connection, and clearing the filter drops every
//! search; a reply to a replaced search is ignored.

use dbide_engine::objects::{Kind as ObjectKind, Object, Search};
use gpui::Context;

use super::Explorer;
use super::Kind;
use super::objects::{Branch, Server, label, row_id, row_kind};
use crate::runtime::{Explore, Explored};

/// The most hits one search lists, across all databases.
const LIMIT: usize = 500;

/// The kinds a search finds: the ones listed under a database.
const KINDS: [ObjectKind; 5] = [
    ObjectKind::Table,
    ObjectKind::View,
    ObjectKind::MaterializedView,
    ObjectKind::Procedure,
    ObjectKind::Function,
];

/// A connection's last search.
pub(super) struct Found {
    /// The name searched for, as typed.
    name: String,
    hits: Hits,
}

enum Hits {
    Searching,
    Done(Vec<Object>),
    Failed(String),
}

impl Found {
    /// The hit in `database` with catalog key `key`.
    pub(super) fn hit(&self, database: &str, key: &str) -> Option<&Object> {
        match &self.hits {
            Hits::Done(hits) => hits
                .iter()
                .find(|o| o.id.database == database && o.id.key == key),
            Hits::Searching | Hits::Failed(_) => None,
        }
    }

    pub(super) fn is_searching(&self) -> bool {
        matches!(self.hits, Hits::Searching)
    }
}

impl Explorer {
    /// Ctrl+Enter in the filter box: searches every database of the selected row's connected
    /// connection for objects whose name holds the filter's text.
    pub(super) fn search_databases(&mut self, cx: &mut Context<Self>) {
        let name = self.filter.read(cx).value().trim().to_owned();
        if name.is_empty() {
            self.message(
                "Type part of a name in the filter box, then Ctrl+Enter.",
                cx,
            );
            return;
        }
        // An object row's ID starts with its connection's ID (objects.rs). Typing the name
        // usually filters the selected row away, so with nothing selected the search goes to
        // the only connected connection.
        let connection = match self.selected(cx) {
            Some(id) => id
                .split(super::objects::SEP)
                .next()
                .map(str::to_owned)
                .filter(|id| self.servers.contains_key(id)),
            None if self.servers.len() == 1 => self.servers.keys().next().cloned(),
            None => None,
        };
        let Some(connection) = connection else {
            self.message(
                "Select a connected connection, or a row under one, to search its databases.",
                cx,
            );
            return;
        };
        let server = self.servers.get_mut(&connection).expect("checked above");
        let call = Explore::Search {
            search: Search {
                name: name.clone(),
                kinds: KINDS.to_vec(),
                limit: LIMIT,
            },
            details: server.saved.connection.clone(),
            password: server.password.as_ref().map(|p| p.expose().to_owned()),
        };
        server.search = Some(Found {
            name,
            hits: Hits::Searching,
        });
        self.request(&connection, None, call, cx);
        self.rebuild(None, cx);
    }

    /// A search on `connection` replied. Ignored unless it is the connection's latest search.
    pub(super) fn searched(
        &mut self,
        connection: &str,
        search: &Search,
        outcome: Result<Explored, String>,
        cx: &mut Context<Self>,
    ) {
        let Some(found) = self
            .servers
            .get_mut(connection)
            .and_then(|s| s.search.as_mut())
            .filter(|f| f.name == search.name && f.is_searching())
        else {
            return;
        };
        let mut said = Vec::new();
        found.hits = match outcome {
            Ok(Explored::Search(report)) => {
                for skipped in &report.skipped {
                    said.push(format!(
                        "Couldn't search {}: {}",
                        skipped.database, skipped.reason
                    ));
                }
                let count = report.objects.len();
                said.push(match count {
                    LIMIT => format!(
                        "Found the first {LIMIT} objects named like \"{}\".",
                        found.name
                    ),
                    1 => format!("Found 1 object named like \"{}\".", found.name),
                    _ => format!("Found {count} objects named like \"{}\".", found.name),
                });
                Hits::Done(report.objects)
            }
            Ok(_) => unreachable!("a search returns a report"),
            Err(error) => {
                said.push(format!("Couldn't search the databases: {error}"));
                Hits::Failed(error)
            }
        };
        for text in said {
            self.message(text, cx);
        }
        self.rebuild(None, cx);
    }

    /// Clearing the filter drops every connection's search.
    pub(super) fn clear_searches(&mut self) {
        for server in self.servers.values_mut() {
            server.search = None;
        }
    }

    /// The "Search" row under `connection`, open, with its hits, while it has a search.
    pub(super) fn search_branch(&self, connection: &str, server: &Server) -> Option<Branch> {
        let found = server.search.as_ref()?;
        // Its ID ends like a placeholder's, so it never parses as a database or object row.
        let id = row_id(&[connection, "s", "n"]);
        let children = match &found.hits {
            Hits::Searching => vec![Branch::loading(&id)],
            Hits::Failed(error) => vec![Branch::note(&id, format!("Couldn't search: {error}"))],
            Hits::Done(hits) if hits.is_empty() => vec![Branch::note(&id, "No objects match")],
            Hits::Done(hits) => hits
                .iter()
                .map(|object| Branch {
                    id: row_id(&[connection, "h", &object.id.database, &object.id.key]),
                    label: format!(
                        "{} · {}",
                        object.id.database,
                        label(server.engine(), object)
                    ),
                    kind: row_kind(object.kind),
                    children: Vec::new(),
                    expanded: false,
                })
                .collect(),
        };
        Some(Branch {
            id,
            label: format!("Search: {}", found.name),
            kind: Kind::Group,
            children,
            expanded: true,
        })
    }
}
