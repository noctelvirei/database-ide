//! The explorer's filter box, and searching every database of a connection for objects by
//! name (docs/spec.md, "Object explorer and Script as"). The filter box only filters the rows
//! already loaded; Ctrl+Enter in it searches every user database of the selected connection, including ones never
//! opened, and lists the hits under the connection in a "Search" row. A hit works like an
//! object row: Script as, Ctrl+N on its database. Databases the search couldn't open are
//! named in the messages strip with the reason.
//!
//! A new search replaces the last one on that connection, and clearing the filter drops every
//! search; a reply to a replaced search is ignored.

use std::sync::atomic::{AtomicU64, Ordering};

use dbide_engine::objects::{Kind as ObjectKind, Object, Search};
use gpui::{Context, Focusable as _, IntoElement, ParentElement as _, Styled as _, Window, div};
use gpui_component::input::Input;

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
    /// Tells this search's reply from a replaced search's; unique in the process.
    generation: u64,
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
}

/// The connection that the row with ID `id` belongs to: an object row's ID starts with its
/// connection's ID (objects.rs).
fn connection_of(id: &str) -> &str {
    id.split(super::objects::SEP).next().unwrap_or(id)
}

impl Explorer {
    /// The filter's text, trimmed and in lower case.
    pub(super) fn needle(&self, cx: &gpui::App) -> String {
        self.filter.read(cx).value().trim().to_lowercase()
    }

    /// Ctrl+F: moves focus to the filter box.
    pub(super) fn focus_filter(&self, window: &mut Window, cx: &mut Context<Self>) {
        self.filter.focus_handle(cx).focus(window, cx);
    }

    /// Enter in the filter box: back to the tree, on the first row that matches.
    pub(super) fn select_first_match(&mut self, window: &mut Window, cx: &mut Context<Self>) {
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

    /// The filter's text changed: an empty filter drops every search; otherwise the selected
    /// row's connection, before the new text hides it, becomes the one Ctrl+Enter searches.
    pub(super) fn filter_changed(&mut self, cx: &mut Context<Self>) {
        if self.needle(cx).is_empty() {
            for server in self.servers.values_mut() {
                server.search = None;
            }
        }
        if let Some(id) = self.selected(cx) {
            self.search_target = Some(connection_of(&id).to_owned());
        }
    }

    /// Ctrl+Enter in the filter box: searches every database of the selected row's connected
    /// connection for objects whose name holds the filter's text. With no row selected, as
    /// when the filter has hidden it, it searches the connection selected before the filter
    /// changed, or the only connected one.
    pub(super) fn search_databases(&mut self, cx: &mut Context<Self>) {
        let name = self.filter.read(cx).value().trim().to_owned();
        if name.is_empty() {
            self.message(
                "Type part of a name in the filter box, then Ctrl+Enter.",
                cx,
            );
            return;
        }
        let connection = match self.selected(cx) {
            Some(id) => Some(connection_of(&id).to_owned()),
            None => self
                .search_target
                .clone()
                .filter(|id| self.servers.contains_key(id))
                .or_else(|| match self.servers.len() {
                    1 => self.servers.keys().next().cloned(),
                    _ => None,
                }),
        }
        .filter(|id| self.servers.contains_key(id));
        let Some(connection) = connection else {
            self.message(
                "Select a connected connection, or a row under one, to search its databases.",
                cx,
            );
            return;
        };
        self.start_search(&connection, name, cx);
    }

    /// Refresh on a search hit: runs `connection`'s last search again.
    pub(super) fn search_again(&mut self, connection: &str, cx: &mut Context<Self>) {
        let name = self.servers.get(connection).and_then(|s| s.search.as_ref());
        if let Some(name) = name.map(|f| f.name.clone()) {
            self.start_search(connection, name, cx);
        }
    }

    /// Searches every database of connected `connection` for `name`, replacing its last
    /// search.
    fn start_search(&mut self, connection: &str, name: String, cx: &mut Context<Self>) {
        static SEARCHES: AtomicU64 = AtomicU64::new(0);
        let generation = SEARCHES.fetch_add(1, Ordering::Relaxed);
        let Some(server) = self.servers.get_mut(connection) else {
            return;
        };
        let call = Explore::Search {
            generation,
            search: Search {
                name: name.clone(),
                kinds: KINDS.to_vec(),
                limit: LIMIT,
            },
            details: server.saved.connection.clone(),
            password: server.password.as_ref().map(|p| p.expose().to_owned()),
        };
        server.search = Some(Found {
            generation,
            name,
            hits: Hits::Searching,
        });
        self.request(connection, None, call, cx);
        self.rebuild(None, cx);
    }

    /// Search `generation` on `connection` replied. Ignored unless it is the connection's
    /// latest search.
    pub(super) fn searched(
        &mut self,
        connection: &str,
        generation: u64,
        outcome: Result<Explored, String>,
        cx: &mut Context<Self>,
    ) {
        let Some(found) = self
            .servers
            .get_mut(connection)
            .and_then(|s| s.search.as_mut())
            .filter(|f| f.generation == generation)
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
