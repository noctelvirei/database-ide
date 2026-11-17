//! SQL Server browse provenance and complete, non-null catalog keys.

use std::collections::BTreeMap;

use futures::StreamExt;
use sqlparser::ast::{SetExpr, Statement, TableFactor};
use sqlparser::dialect::MsSqlDialect;
use sqlparser::parser::Parser;

use super::{EditColumn, Editability, EditableResult, Error};
use crate::danger::Engine;
use crate::session::{Column, Event, Session, TypeCategory, Value};
use crate::sql::{identifier, literal};

/// Describes SQL without executing it, using the tab's database and session context.
pub(super) async fn inspect(
    session: &mut dyn Session,
    statement: &str,
    columns: &[Column],
) -> Result<Editability, Error> {
    let sql = format!(
        "EXEC sys.sp_describe_first_result_set @tsql = {}, @params = NULL, @browse_information_mode = 1",
        literal(&Value::Text(statement.into()), Engine::SqlServer)?
    );
    let (fields, rows) = query(session, &sql).await?;
    let field = |row: &[Value], name: &str| -> Result<Value, Error> {
        fields
            .iter()
            .position(|field| field == name)
            .and_then(|i| row.get(i))
            .cloned()
            .ok_or_else(|| Error::Catalog(format!("missing {name}")))
    };
    let mut table = None;
    let mut policy = Vec::new();
    let mut comparison_types = Vec::new();
    let mut comparison_collations = Vec::new();
    for row in rows {
        if field(&row, "is_hidden")? == Value::Bool(true) {
            continue;
        }
        let text = |name: &str| -> Result<Option<String>, Error> {
            Ok(match field(&row, name)? {
                Value::Text(text) => Some(text),
                Value::Null => None,
                _ => return Err(Error::Catalog(format!("invalid {name}"))),
            })
        };
        let source = match (
            text("source_database")?,
            text("source_schema")?,
            text("source_table")?,
        ) {
            (Some(db), Some(schema), Some(table)) => Some(vec![db, schema, table]),
            _ => None,
        };
        if text("source_server")?.is_some() {
            return Ok(Editability::ReadOnly("unknown base table".into()));
        }
        let name = text("source_column")?;
        if name.is_some() {
            let Some(source) = source else {
                return Ok(Editability::ReadOnly("unknown base table".into()));
            };
            if table.as_ref().is_some_and(|table| *table != source) {
                return Ok(Editability::ReadOnly("columns from 2 tables".into()));
            }
            table = Some(source);
        }
        let ty = text("system_type_name")?.and_then(|ty| comparison_type(&ty));
        let i = policy.len();
        policy.push(EditColumn {
            read_only: name.is_none()
                || field(&row, "is_identity_column")? != Value::Bool(false)
                || field(&row, "is_computed_column")? != Value::Bool(false)
                || field(&row, "is_updateable")? != Value::Bool(true)
                || columns
                    .get(i)
                    .is_none_or(|column| column.category == TypeCategory::Bytes)
                || text("system_type_name")?
                    .is_some_and(|ty| matches!(ty.as_str(), "timestamp" | "rowversion")),
            comparable: name.is_some() && ty.is_some(),
            name,
        });
        comparison_types.push(ty);
        comparison_collations.push(text("collation_name")?);
    }
    if policy.len() != columns.len() {
        return Err(Error::Catalog(
            "description width differs from result".into(),
        ));
    }
    let Some(table) = table else {
        return Ok(Editability::ReadOnly("expression columns only".into()));
    };
    let qualified = table
        .iter()
        .map(|part| identifier(part, Engine::SqlServer))
        .collect::<Result<Vec<_>, _>>()?
        .join(".");
    let db = identifier(&table[0], Engine::SqlServer)?;
    let object = literal(&Value::Text(qualified), Engine::SqlServer)?;
    let statements = Parser::parse_sql(&MsSqlDialect {}, statement)
        .map_err(|error| Error::Catalog(error.to_string()))?;
    let Some(Statement::Query(parsed)) = statements.first() else {
        return Ok(Editability::ReadOnly("unknown base table".into()));
    };
    let SetExpr::Select(select) = parsed.body.as_ref() else {
        return Ok(Editability::ReadOnly("unknown base table".into()));
    };
    let TableFactor::Table { name, .. } = &select.from[0].relation else {
        return Ok(Editability::ReadOnly("unknown base table".into()));
    };
    let selected = literal(&Value::Text(name.to_string()), Engine::SqlServer)?;
    // Browse key flags can include hidden columns and do not identify complete indexes.
    let sql = format!(
        "SELECT i.index_id, c.name, c.is_nullable FROM {db}.sys.tables t
        JOIN {db}.sys.indexes i ON i.object_id = t.object_id
        JOIN {db}.sys.index_columns ic ON ic.object_id = i.object_id AND ic.index_id = i.index_id
        JOIN {db}.sys.columns c ON c.object_id = ic.object_id AND c.column_id = ic.column_id
        WHERE t.object_id = OBJECT_ID({object})
        AND t.object_id = OBJECT_ID({selected}, N'U') AND i.is_unique = 1
        AND i.is_disabled = 0 AND i.is_hypothetical = 0 AND i.has_filter = 0
        AND ic.key_ordinal > 0 ORDER BY i.is_primary_key DESC, i.index_id, ic.key_ordinal"
    );
    let (_, rows) = query(session, &sql).await?;
    let mut keys: BTreeMap<i64, Option<Vec<usize>>> = BTreeMap::new();
    for row in rows {
        let [
            Value::Integer(index),
            Value::Text(name),
            Value::Bool(nullable),
        ] = row.as_slice()
        else {
            return Err(Error::Catalog("invalid key description".into()));
        };
        let key = keys.entry(*index).or_insert_with(|| Some(Vec::new()));
        let column = policy
            .iter()
            .position(|column| column.name.as_ref() == Some(name) && column.comparable);
        if let (Some(key), Some(column), false) = (&mut *key, column, nullable) {
            key.push(column);
        } else {
            *key = None;
        }
    }
    let Some(key) = keys.into_values().flatten().find(|key| !key.is_empty()) else {
        return Ok(Editability::ReadOnly("no key column in result".into()));
    };
    Ok(Editability::Editable(EditableResult {
        engine: Engine::SqlServer,
        table,
        columns: policy,
        key,
        comparison_types,
        comparison_collations,
    }))
}

fn comparison_type(ty: &str) -> Option<String> {
    let base = ty.split('(').next()?;
    // Only built-in SQL type syntax may enter a generated CONVERT expression.
    let supported = matches!(
        base,
        "bigint"
            | "int"
            | "smallint"
            | "tinyint"
            | "bit"
            | "decimal"
            | "numeric"
            | "money"
            | "smallmoney"
            | "float"
            | "real"
            | "date"
            | "time"
            | "datetime"
            | "smalldatetime"
            | "datetime2"
            | "datetimeoffset"
            | "uniqueidentifier"
            | "char"
            | "varchar"
            | "nchar"
            | "nvarchar"
            | "binary"
            | "varbinary"
            | "timestamp"
            | "rowversion"
    );
    (supported
        && ty
            .bytes()
            .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b"(), ".contains(&b)))
    .then(|| match base {
        "timestamp" | "rowversion" => "binary(8)".into(),
        _ => ty.into(),
    })
}

async fn query(
    session: &mut dyn Session,
    sql: &str,
) -> Result<(Vec<String>, Vec<Vec<Value>>), Error> {
    let mut run = session.run(sql);
    let mut fields = Vec::new();
    let mut rows = Vec::new();
    let mut error = None;
    while let Some(event) = run.next().await {
        match event {
            Event::ResultSetStarted { columns } => {
                fields = columns.into_iter().map(|c| c.name).collect()
            }
            Event::Rows(batch) => rows.extend(batch),
            Event::Error { text, code, .. } => {
                error = Some(Error::Server {
                    text,
                    code,
                    change: None,
                })
            }
            _ => {}
        }
    }
    error.map_or(Ok((fields, rows)), Err)
}

#[cfg(test)]
mod tests {
    use super::comparison_type;

    #[test]
    fn legacy_lob_and_xml_originals_are_not_comparable() {
        assert_eq!(comparison_type("text"), None);
        assert_eq!(comparison_type("ntext"), None);
        assert_eq!(comparison_type("image"), None);
        assert_eq!(comparison_type("xml"), None);
    }

    #[test]
    fn built_in_types_preserve_precision_and_rowversion_uses_binary() {
        assert_eq!(comparison_type("real"), Some("real".into()));
        assert_eq!(comparison_type("datetime2(7)"), Some("datetime2(7)".into()));
        assert_eq!(
            comparison_type("decimal(38,4)"),
            Some("decimal(38,4)".into())
        );
        assert_eq!(
            comparison_type("nvarchar(max)"),
            Some("nvarchar(max)".into())
        );
        assert_eq!(comparison_type("timestamp"), Some("binary(8)".into()));
        assert_eq!(comparison_type("[dbo].[custom_type]"), None);
        assert_eq!(comparison_type("real); DELETE FROM t;--"), None);
    }
}
