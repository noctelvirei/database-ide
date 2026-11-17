//! PostgreSQL table identity, non-null unique keys and equality support.

use futures::StreamExt;
use serde::Deserialize;

use super::{EditColumn, Editability, EditableResult, Error};
use crate::danger::Engine;
use crate::session::{Column, Event, Session, TypeCategory, Value};
use crate::sql::literal;

#[derive(Debug, Deserialize)]
struct Table {
    table: Vec<String>,
    columns: Vec<CatalogColumn>,
    keys: Vec<Vec<String>>,
}

#[derive(Debug, Deserialize)]
struct CatalogColumn {
    name: String,
    read_only: bool,
    comparable: bool,
}

/// Resolves quoted provenance on the same session, retaining result column order.
pub(super) async fn inspect(
    session: &mut dyn Session,
    base_table: &str,
    columns: &[Column],
) -> Result<Editability, Error> {
    // Provenance already uses PostgreSQL-quoted qualified names. Resolve it as regclass,
    // then retrieve separate identifier components so embedded dots stay in their names.
    let table = literal(&Value::Text(base_table.into()), Engine::PostgreSql)?;
    let sql = QUERY.replace("$TABLE", &table);
    let mut run = session.run(&sql);
    let mut table = None;
    let mut failure = None;
    while let Some(event) = run.next().await {
        match event {
            Event::Rows(rows) if failure.is_none() => {
                for row in rows {
                    match row.as_slice() {
                        [Value::Text(json)] if table.is_none() => {
                            match serde_json::from_str::<Table>(json) {
                                Ok(found) => table = Some(found),
                                Err(error) => failure = Some(Error::Catalog(error.to_string())),
                            }
                        }
                        _ => {
                            failure = Some(Error::Catalog("expected one table description".into()))
                        }
                    }
                }
            }
            Event::Error { text, code, .. } if failure.is_none() => {
                failure = Some(Error::Server {
                    text,
                    code,
                    change: None,
                });
            }
            _ => {}
        }
    }
    if let Some(error) = failure {
        return Err(error);
    }
    let Some(table) = table else {
        return Ok(Editability::ReadOnly(
            "base table no longer exists or is not a table".into(),
        ));
    };
    let policy: Vec<_> = columns
        .iter()
        .map(|column| {
            let catalog = column.base_column.as_ref().and_then(|name| {
                table
                    .columns
                    .iter()
                    .find(|candidate| candidate.name == *name)
            });
            EditColumn {
                name: catalog.map(|column| column.name.clone()),
                read_only: catalog.is_none_or(|column| column.read_only)
                    || column.category == TypeCategory::Bytes,
                comparable: catalog.is_some_and(|column| column.comparable),
            }
        })
        .collect();
    let key = table.keys.iter().find_map(|key| {
        key.iter()
            .map(|name| {
                policy
                    .iter()
                    .position(|column| column.name.as_ref() == Some(name) && column.comparable)
            })
            .collect::<Option<Vec<_>>>()
    });
    let Some(key) = key.filter(|key| !key.is_empty()) else {
        return Ok(Editability::ReadOnly("no key column in result".into()));
    };
    Ok(Editability::Editable(EditableResult {
        engine: Engine::PostgreSql,
        table: table.table,
        columns: policy,
        key,
        comparison_types: Vec::new(),
        comparison_collations: Vec::new(),
    }))
}

// A constraint's definition is display text, not a structured list of key columns. Use
// pg_index instead; INCLUDE columns, expressions and partial/deferred indexes are not keys.
const QUERY: &str = r#"
WITH RECURSIVE relation AS (
    SELECT c.oid, c.relname, n.nspname FROM pg_catalog.pg_class c
    JOIN pg_catalog.pg_namespace n ON n.oid = c.relnamespace
    WHERE c.oid = pg_catalog.to_regclass($TABLE) AND c.relkind IN ('r', 'p')
), attributes AS (
    SELECT a.* FROM pg_catalog.pg_attribute a JOIN relation r ON r.oid = a.attrelid
    WHERE a.attnum > 0 AND NOT a.attisdropped
), type_edges AS (
    SELECT oid AS parent, typbasetype AS child FROM pg_catalog.pg_type WHERE typtype = 'd'
    UNION ALL
    SELECT oid, typelem FROM pg_catalog.pg_type WHERE typcategory = 'A' AND typelem <> 0
    UNION ALL
    SELECT t.oid, a.atttypid FROM pg_catalog.pg_type t
    JOIN pg_catalog.pg_attribute a ON a.attrelid = t.typrelid
    WHERE t.typtype = 'c' AND a.attnum > 0 AND NOT a.attisdropped
), types AS (
    SELECT a.attnum, a.atttypid AS oid FROM attributes a
    UNION
    SELECT parent.attnum, edge.child FROM types parent
    JOIN type_edges edge ON edge.parent = parent.oid
), comparison AS (
    SELECT t.attnum, bool_and(EXISTS (
        SELECT 1 FROM pg_catalog.pg_operator o
        WHERE o.oprname = '=' AND o.oprleft = o.oprright
        AND (o.oprleft = t.oid OR EXISTS (
            SELECT 1 FROM pg_catalog.pg_cast cast_rule
            WHERE cast_rule.castsource = t.oid AND cast_rule.casttarget = o.oprleft
                AND cast_rule.castcontext = 'i'))
        AND o.oprresult = 16 AND pg_catalog.pg_operator_is_visible(o.oid)
    ) OR native.typtype IN ('e', 'r', 'm')) AS comparable
    FROM types t JOIN pg_catalog.pg_type native ON native.oid = t.oid
    WHERE NOT EXISTS (SELECT 1 FROM type_edges edge WHERE edge.parent = t.oid)
    GROUP BY t.attnum
)
SELECT pg_catalog.json_build_object(
    'table', ARRAY[r.nspname::text, r.relname::text],
    'columns', COALESCE((SELECT pg_catalog.json_agg(pg_catalog.json_build_object(
        'name', a.attname, 'read_only', a.attidentity <> '' OR a.attgenerated <> ''
            OR EXISTS (SELECT 1 FROM types t WHERE t.attnum = a.attnum AND t.oid = 17),
        'comparable', COALESCE(c.comparable, false)) ORDER BY a.attnum)
        FROM attributes a LEFT JOIN comparison c ON c.attnum = a.attnum), '[]'::json),
    'keys', COALESCE((SELECT pg_catalog.json_agg(k.names ORDER BY k.primary_key DESC, k.indexrelid)
        FROM (SELECT i.indexrelid, i.indisprimary AS primary_key,
            pg_catalog.json_agg(a.attname ORDER BY key.ordinality) AS names
            FROM pg_catalog.pg_index i
            CROSS JOIN LATERAL unnest(i.indkey) WITH ORDINALITY AS key(attnum, ordinality)
            LEFT JOIN attributes a ON a.attnum = key.attnum
            WHERE i.indrelid = r.oid AND i.indisunique AND i.indisvalid AND i.indisready
                AND i.indimmediate AND i.indpred IS NULL AND i.indexprs IS NULL
                AND key.ordinality <= i.indnkeyatts
            GROUP BY i.indexrelid, i.indisprimary
            HAVING bool_and(a.attnotnull AND a.attnum IS NOT NULL)) k), '[]'::json)
)::text FROM relation r
"#;
