//! Script forms using native SHOW CREATE definitions instead of reconstructing table DDL.

use super::{Error, literal, metadata, quote, rows};
use crate::objects::{Kind, Object, Script};
use crate::session::{Session, Value};

/// Produces the validated object's requested form; no generated DDL runs on the source.
pub(super) async fn script(
    session: &mut dyn Session,
    object: &Object,
    form: Script,
) -> Result<String, Error> {
    let qualified = format!("{}.{}", quote(&object.id.database)?, quote(&object.name)?);
    if form == Script::Select {
        let columns = metadata::columns(session, object)
            .await?
            .iter()
            .map(|column| quote(&column.name))
            .collect::<Result<Vec<_>, _>>()?
            .join(", ");
        return Ok(format!("SELECT {columns} FROM {qualified} LIMIT 1000;\n"));
    }
    if form == Script::Alter && object.kind == Kind::Table {
        // Every line must remain a comment even when an identifier contains a newline.
        let qualified = qualified.replace('\n', "\n-- ").replace('\r', "\r-- ");
        return Ok(format!(
            "-- ALTER TABLE {qualified}\n--     ADD COLUMN column_name data_type;\n"
        ));
    }
    if object.kind == Kind::Index {
        return index_script(session, object).await;
    }
    let keyword = match object.kind {
        Kind::Table => "TABLE",
        Kind::View => "VIEW",
        Kind::Procedure => "PROCEDURE",
        Kind::Function => "FUNCTION",
        _ => {
            return Err(Error::Unsupported {
                operation: "Script as",
                kind: object.kind,
            });
        }
    };
    let settings = rows(
        session,
        "SELECT @@SESSION.sql_mode, CAST(@@SESSION.sql_quote_show_create AS CHAR)",
    )
    .await?;
    let mode = text(settings.first().ok_or(Error::NotFound)?, 0)?;
    if text(settings.first().ok_or(Error::NotFound)?, 1)? != "1" {
        return Err(Error::QuotingDisabled);
    }
    if mode.split(',').any(|mode| mode == "ANSI_QUOTES") {
        return Err(Error::AnsiQuotes);
    }
    let result = rows(session, &format!("SHOW CREATE {keyword} {qualified}")).await?;
    let row = result.first().ok_or(Error::NotFound)?;
    let routine = matches!(object.kind, Kind::Procedure | Kind::Function);
    let mut definition = text(row, if routine { 2 } else { 1 })?.to_owned();
    let mode = if routine { text(row, 1)? } else { mode };
    if form == Script::Alter {
        let rest = definition
            .strip_prefix("CREATE ")
            .ok_or_else(|| Error::Catalog("View definition does not start with CREATE".into()))?;
        definition = format!("ALTER {rest}");
    }
    let mut sql = format!("SET SESSION sql_mode = {};\n", literal(mode)?);
    if routine || object.kind == Kind::View {
        let offset = if routine { 3 } else { 2 };
        sql.push_str(&format!(
            "SET SESSION character_set_client = 'utf8mb4';\nSET SESSION collation_connection = {};\n",
            literal(text(row, offset + 1)?)?
        ));
    }
    sql.push_str(definition.trim_end_matches(';'));
    sql.push_str(";\n");
    Ok(sql)
}

fn text(row: &[Value], index: usize) -> Result<&str, Error> {
    match row.get(index) {
        Some(Value::Text(text)) => Ok(text),
        _ => Err(Error::Catalog(
            "Missing SHOW CREATE definition or creation setting; check privileges".into(),
        )),
    }
}

async fn index_script(session: &mut dyn Session, object: &Object) -> Result<String, Error> {
    let (_, rest) = object.id.key.split_once(':').ok_or(Error::Identity)?;
    let (table_hex, _) = rest.split_once(':').ok_or(Error::Identity)?;
    // Resolve the parent via the catalog rather than decoding caller-supplied names into SQL.
    let parents = super::catalog(
        session,
        &format!(
            "object_key = CONCAT('table:', {}) AND kind = 'table'",
            literal(table_hex)?
        ),
        Some(1),
    )
    .await?;
    let table = parents.first().ok_or(Error::NotFound)?;
    let index = metadata::indexes(session, table)
        .await?
        .into_iter()
        .find(|index| index.object == *object)
        .ok_or(Error::NotFound)?;
    Ok(format!("{};\n", index.definition))
}
