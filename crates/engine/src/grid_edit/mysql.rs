//! MySQL original-name provenance, transactional tables and complete non-null keys.

use std::collections::BTreeMap;

use futures::StreamExt;
use sqlparser::ast::{SetExpr, Statement, TableFactor};
use sqlparser::dialect::MySqlDialect;
use sqlparser::parser::Parser;

use super::{EditColumn, Editability, EditableResult, Error};
use crate::danger::Engine;
use crate::session::{Column, Event, Session, TypeCategory, Value};
use crate::sql::literal;

fn table_name(statement: &str) -> Option<Vec<String>> {
    let statements = Parser::parse_sql(&MySqlDialect {}, statement).ok()?;
    let [Statement::Query(query)] = statements.as_slice() else {
        return None;
    };
    let SetExpr::Select(select) = query.body.as_ref() else {
        return None;
    };
    let TableFactor::Table { name, .. } = &select.from.first()?.relation else {
        return None;
    };
    name.0
        .iter()
        .map(|part| part.as_ident().map(|id| id.value.clone()))
        .collect()
}

/// Resolves wire provenance and keys on the tab's session; views and nontransactional
/// tables stay read-only because Save must be able to roll back every statement.
pub(super) async fn inspect(
    session: &mut dyn Session,
    statement: &str,
    base_table: &str,
    columns: &[Column],
) -> Result<Editability, Error> {
    let mut table = crate::sql::qualified_identifier(base_table)?;
    if table.len() == 1 {
        let rows = query(session, "SELECT DATABASE()").await?;
        let Some([Value::Text(database)]) = rows.first().map(Vec::as_slice) else {
            return Ok(Editability::ReadOnly("unknown base table".into()));
        };
        table.insert(0, database.clone());
    }
    let [database, name] = table.as_slice() else {
        return Ok(Editability::ReadOnly("unknown base table".into()));
    };
    let Some(mut selected) = table_name(statement) else {
        return Ok(Editability::ReadOnly("unknown base table".into()));
    };
    if selected.len() == 1 {
        selected.insert(0, session.current_database().into());
    }
    let text = |text: &str| literal(&Value::Text(text.into()), Engine::MySql);
    let filter = format!(
        "TABLE_SCHEMA = {} AND TABLE_NAME = {}",
        text(database)?,
        text(name)?
    );
    // Check the selected relation too: a view may report its underlying table's names.
    let selected_filter = match selected.as_slice() {
        [db, name] => format!(
            "TABLE_SCHEMA = {} AND TABLE_NAME = {}",
            text(db)?,
            text(name)?
        ),
        _ => return Ok(Editability::ReadOnly("unknown base table".into())),
    };
    let rows = query(session, &format!(
        "SELECT ENGINE FROM information_schema.TABLES WHERE {filter} AND TABLE_TYPE = 'BASE TABLE'
         AND EXISTS (SELECT 1 FROM information_schema.TABLES WHERE {selected_filter} AND TABLE_TYPE = 'BASE TABLE')"
    )).await?;
    match rows.first().map(Vec::as_slice) {
        Some([Value::Text(engine)]) if engine == "InnoDB" => {}
        Some(_) => {
            return Ok(Editability::ReadOnly(
                "table does not support transactional Save".into(),
            ));
        }
        None => {
            return Ok(Editability::ReadOnly(
                "base table no longer exists or is not a table".into(),
            ));
        }
    }
    let catalog = query(session, &format!(
        "SELECT COLUMN_NAME, DATA_TYPE, EXTRA, CHARACTER_SET_NAME FROM information_schema.COLUMNS WHERE {filter} ORDER BY ORDINAL_POSITION"
    )).await?;
    let mut policy = Vec::new();
    let mut comparison_types = Vec::new();
    for column in columns {
        let found = catalog.iter().find(|row| match row.first() {
            Some(Value::Text(name)) => column.base_column.as_ref() == Some(name),
            _ => false,
        });
        let Some(row) = found else {
            policy.push(EditColumn {
                name: None,
                read_only: true,
                comparable: false,
            });
            comparison_types.push(None);
            continue;
        };
        let [
            Value::Text(name),
            Value::Text(ty),
            Value::Text(extra),
            charset,
        ] = row.as_slice()
        else {
            return Err(Error::Catalog("invalid column description".into()));
        };
        let comparison = match (ty.as_str(), charset) {
            ("float", _) => Some("FLOAT".into()),
            ("double", _) => Some("DOUBLE".into()),
            (_, Value::Text(charset)) => {
                if charset.is_empty()
                    || !charset
                        .bytes()
                        .all(|b| b.is_ascii_alphanumeric() || b == b'_')
                {
                    return Err(Error::Catalog("invalid character set".into()));
                }
                // Convert the original into the stored charset before byte comparison.
                Some(format!("CHAR CHARACTER SET {charset}"))
            }
            _ => None,
        };
        let comparable = !matches!(ty.as_str(), "json" | "geometry" | "vector");
        policy.push(EditColumn {
            name: Some(name.clone()),
            read_only: extra.contains("auto_increment")
                || extra.contains("VIRTUAL GENERATED")
                || extra.contains("STORED GENERATED")
                || column.category == TypeCategory::Bytes,
            comparable,
        });
        comparison_types.push(comparison);
    }
    let rows = query(session, &format!(
        "SELECT INDEX_NAME, COLUMN_NAME, NULLABLE, SUB_PART FROM information_schema.STATISTICS
         WHERE {filter} AND NON_UNIQUE = 0 ORDER BY (INDEX_NAME = 'PRIMARY') DESC, INDEX_NAME, SEQ_IN_INDEX"
    )).await?;
    let mut keys = BTreeMap::<String, Option<Vec<usize>>>::new();
    for row in rows {
        let [Value::Text(index), name, Value::Text(nullable), prefix] = row.as_slice() else {
            return Err(Error::Catalog("invalid key description".into()));
        };
        let key = keys
            .entry(index.clone())
            .or_insert_with(|| Some(Vec::new()));
        let column = match name {
            Value::Text(name) => policy
                .iter()
                .position(|column| column.name.as_ref() == Some(name) && column.comparable),
            _ => None,
        };
        if let (Some(key), Some(column), true, Value::Null) =
            (&mut *key, column, nullable.is_empty(), prefix)
        {
            key.push(column);
        } else {
            *key = None;
        }
    }
    let key = keys
        .remove("PRIMARY")
        .flatten()
        .or_else(|| keys.into_values().flatten().find(|key| !key.is_empty()));
    let Some(key) = key.filter(|key| !key.is_empty()) else {
        return Ok(Editability::ReadOnly("no key column in result".into()));
    };
    Ok(Editability::Editable(EditableResult {
        engine: Engine::MySql,
        table,
        columns: policy,
        key,
        comparison_types,
        comparison_collations: Vec::new(),
    }))
}

async fn query(session: &mut dyn Session, sql: &str) -> Result<Vec<Vec<Value>>, Error> {
    let mut run = session.run(sql);
    let mut rows = Vec::new();
    let mut failure = None;
    while let Some(event) = run.next().await {
        match event {
            Event::Rows(batch) => rows.extend(batch),
            Event::Error { text, code, .. } => {
                failure = Some(Error::Server {
                    text,
                    code,
                    change: None,
                })
            }
            _ => {}
        }
    }
    failure.map_or(Ok(rows), Err)
}
