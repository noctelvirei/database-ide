//! Best-effort column provenance, queried independently of the streaming connection.
//!
//! Nullability stays unknown: a NOT NULL table column still yields NULL through an outer
//! join, so the catalog can't say whether a result column may be NULL.

use std::collections::HashMap;
use std::collections::hash_map::Entry;
use std::time::Duration;

use tokio_postgres::{Client, Config, SimpleColumn};

use super::{ConnectionTask, values};
use crate::session::Column;

struct BaseColumn {
    table: String,
    column: String,
}

/// Session-owned catalog connection; credentials are never formatted.
pub(super) struct Metadata {
    config: Config,
    tls: native_tls::TlsConnector,
    connection: Option<(Client, ConnectionTask)>,
}

impl std::fmt::Debug for Metadata {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Metadata").finish_non_exhaustive()
    }
}

impl Metadata {
    /// Retains the original connection settings and opens no socket until needed.
    pub(super) fn new(config: Config, tls: native_tls::TlsConnector) -> Self {
        Self {
            config,
            tls,
            connection: None,
        }
    }

    /// Preserves labels and types on failure; catalog work is bounded to two seconds.
    /// Looked up per result set, never cached: a rename keeps the table OID and column
    /// number, so cached names would go stale.
    pub(super) async fn columns(&mut self, columns: &[SimpleColumn]) -> Vec<Column> {
        let mut result: Vec<_> = columns.iter().map(values::column).collect();
        let mut found: HashMap<(u32, i16), BaseColumn> = HashMap::new();
        let lookup = async {
            for (source, target) in columns.iter().zip(&mut result) {
                let key = (source.table_oid(), source.column_id());
                if key.0 == 0 || key.1 == 0 {
                    continue;
                }
                if let Entry::Vacant(slot) = found.entry(key) {
                    if self
                        .connection
                        .as_ref()
                        .is_some_and(|(client, _)| client.is_closed())
                    {
                        self.connection = None;
                    }
                    if self.connection.is_none() {
                        let (client, connection) = self
                            .config
                            .connect(postgres_native_tls::MakeTlsConnector::new(self.tls.clone()))
                            .await?;
                        let task = ConnectionTask(tokio::spawn(async move {
                            let _ = connection.await;
                        }));
                        self.connection = Some((client, task));
                    }
                    let (client, _) = self
                        .connection
                        .as_ref()
                        .expect("metadata connection is open");
                    let row = client
                        .query_opt(
                            "SELECT pg_catalog.quote_ident(n.nspname) || '.' ||
                                pg_catalog.quote_ident(c.relname), a.attname::text
                         FROM pg_catalog.pg_attribute a
                         JOIN pg_catalog.pg_class c ON c.oid = a.attrelid
                         JOIN pg_catalog.pg_namespace n ON n.oid = c.relnamespace
                         WHERE a.attrelid = $1 AND a.attnum = $2 AND NOT a.attisdropped",
                            &[&key.0, &key.1],
                        )
                        .await?;
                    if let Some(row) = row {
                        slot.insert(BaseColumn {
                            table: row.try_get(0)?,
                            column: row.try_get(1)?,
                        });
                    }
                }
                if let Some(base) = found.get(&key) {
                    target.base_table = Some(base.table.clone());
                    target.base_column = Some(base.column.clone());
                }
            }
            Ok::<_, tokio_postgres::Error>(())
        };
        if !matches!(
            tokio::time::timeout(Duration::from_secs(2), lookup).await,
            Ok(Ok(()))
        ) {
            // Discard timed-out queries rather than queueing more work behind them.
            self.connection = None;
        }
        result
    }
}
