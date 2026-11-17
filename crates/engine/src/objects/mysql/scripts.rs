//! Script forms using native SHOW CREATE definitions instead of reconstructing table DDL.

use sqlparser::dialect::MySqlDialect;
use sqlparser::tokenizer::{Location, Token, Tokenizer};

use super::{Error, literal, metadata, quote, rows, utf8_literal};
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
    // Canonical table/view text uses backslash escapes even when SHOW ran with them disabled.
    let mode = if routine {
        text(row, 1)?.to_owned()
    } else {
        mode.split(',')
            .filter(|mode| *mode != "NO_BACKSLASH_ESCAPES")
            .collect::<Vec<_>>()
            .join(",")
    };
    if object.kind == Kind::View {
        definition = view_literals(session, &definition, text(row, 2)?).await?;
    }
    if form == Script::Alter {
        let rest = definition
            .strip_prefix("CREATE ")
            .ok_or_else(|| Error::Catalog("View definition does not start with CREATE".into()))?;
        definition = format!("ALTER {rest}");
    }
    let mut sql = format!("SET SESSION sql_mode = {};\n", literal(&mode)?);
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

/// Restores introduced literal bytes after SHOW's result conversion, retaining literal coercibility.
// shortcut: SHOW conversion must be lossless; use a binary SHOW path to support already-lost bytes.
async fn view_literals(
    session: &mut dyn Session,
    definition: &str,
    client_charset: &str,
) -> Result<String, Error> {
    let tokens = Tokenizer::new(&MySqlDialect {}, definition)
        .tokenize_with_location()
        .map_err(|error| Error::Catalog(error.to_string()))?;
    let tokens = tokens
        .iter()
        .filter(|token| !matches!(token.token, Token::Whitespace(_)))
        .collect::<Vec<_>>();
    let mut sql = String::new();
    let mut copied = 0;
    for pair in tokens.windows(2) {
        if let (Token::Word(word), Token::SingleQuotedString(value)) =
            (&pair[0].token, &pair[1].token)
            && word.quote_style.is_none()
            && word.value.starts_with('_')
        {
            // SHOW tags the entire definition with its creation client charset, even mixed literals.
            let result = rows(
                session,
                &format!(
                    "SELECT HEX(CONVERT({} USING {}))",
                    utf8_literal(value),
                    quote(client_charset)?
                ),
            )
            .await?;
            let hex = text(result.first().ok_or(Error::NotFound)?, 0)?;
            if !hex.len().is_multiple_of(2) || !hex.bytes().all(|byte| byte.is_ascii_hexdigit()) {
                return Err(Error::Catalog("Expected hexadecimal view literal".into()));
            }
            let start = byte_offset(definition, pair[0].span.start);
            let end = byte_offset(definition, pair[1].span.end);
            sql.push_str(&definition[copied..start]);
            sql.push_str(&format!("{} X'{hex}'", word.value));
            copied = end;
        }
    }
    sql.push_str(&definition[copied..]);
    Ok(sql)
}

fn byte_offset(sql: &str, location: Location) -> usize {
    let mut lines = sql.split_inclusive('\n');
    let preceding = lines
        .by_ref()
        .take(location.line as usize - 1)
        .map(str::len)
        .sum::<usize>();
    preceding
        + lines
            .next()
            .unwrap_or_default()
            .chars()
            .take(location.column as usize - 1)
            .map(char::len_utf8)
            .sum::<usize>()
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
