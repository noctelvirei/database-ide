//! SQL quoting shared by exports and grid Save scripts.

use crate::danger::Engine;
use crate::session::Value;

/// Decodes a server-reported qualified identifier, preserving quoted dots and escapes.
pub(crate) fn qualified_identifier(text: &str) -> Result<Vec<String>, Error> {
    use sqlparser::dialect::{Dialect, GenericDialect, MsSqlDialect};
    use sqlparser::parser::Parser;
    use sqlparser::tokenizer::Token;

    for dialect in [&GenericDialect {} as &dyn Dialect, &MsSqlDialect {}] {
        let Ok(mut parser) = Parser::new(dialect).try_with_sql(text) else {
            continue;
        };
        let Ok(name) = parser.parse_object_name(false) else {
            continue;
        };
        if parser.peek_token().token != Token::EOF {
            continue;
        }
        return name
            .0
            .iter()
            .map(|part| {
                part.as_ident()
                    .map(|name| name.value.clone())
                    .ok_or(Error::Identifier)
            })
            .collect();
    }
    Err(Error::Identifier)
}

/// Writes SQL Server's ` COLLATE <name>` clause, or None when the catalog name isn't a plain
/// collation name. COLLATE takes the name bare; a bracketed name is a syntax error.
pub(crate) fn mssql_collate(collation: &str) -> Option<String> {
    let plain = !collation.is_empty()
        && collation
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '_');
    plain.then(|| format!(" COLLATE {collation}"))
}

/// A value or identifier cannot be represented without changing its meaning.
#[derive(Debug, thiserror::Error)]
pub enum Error {
    /// Empty identifiers and NUL are never valid identifier components.
    #[error("SQL identifiers must be nonempty and contain no NUL")]
    Identifier,
    /// A numeric value is not SQL syntax, or text contains unsupported NUL.
    #[error("Value cannot be represented as a SQL literal")]
    Literal,
}

/// Quotes one unquoted identifier component; refuses empty names and NUL.
pub(crate) fn identifier(text: &str, engine: Engine) -> Result<String, Error> {
    if text.is_empty() || text.contains('\0') {
        return Err(Error::Identifier);
    }
    Ok(match engine {
        Engine::SqlServer => format!("[{}]", text.replace(']', "]]")),
        Engine::PostgreSql => format!("\"{}\"", text.replace('"', "\"\"")),
        Engine::MySql => format!("`{}`", text.replace('`', "``")),
    })
}

fn string(text: &str, engine: Engine) -> Result<String, Error> {
    if text.contains('\0') && engine != Engine::MySql {
        return Err(Error::Literal);
    }
    let quoted = text.replace('\'', "''");
    Ok(match engine {
        Engine::SqlServer => format!("N'{quoted}'"),
        Engine::PostgreSql => format!("E'{}'", quoted.replace('\\', "\\\\")),
        Engine::MySql => format!(
            "CONVERT(X'{}' USING utf8mb4)",
            crate::export::text::hex(text.as_bytes())
        ),
    })
}

fn decimal(text: &str) -> bool {
    let text = text.strip_prefix(['-', '+']).unwrap_or(text);
    let mut dot = false;
    let mut digits = 0;
    for byte in text.bytes() {
        if byte.is_ascii_digit() {
            digits += 1;
        } else if byte == b'.' && !dot {
            dot = true;
        } else {
            return false;
        }
    }
    digits != 0
}

/// Encodes an optimistic predicate's original value in the base column's comparison type.
pub(crate) fn original_literal(value: &Value, engine: Engine) -> Result<String, Error> {
    if engine == Engine::PostgreSql && !matches!(value, Value::Null | Value::Bytes(_)) {
        string(&crate::export::cell_text(value), engine)
    } else {
        literal(value, engine)
    }
}

/// Encodes one cell without depending on backslash-escape settings; refuses invalid numerics.
pub(crate) fn literal(value: &Value, engine: Engine) -> Result<String, Error> {
    Ok(match value {
        Value::Null => "NULL".into(),
        Value::Bool(value) if engine == Engine::SqlServer => u8::from(*value).to_string(),
        Value::Bool(true) => "TRUE".into(),
        Value::Bool(false) => "FALSE".into(),
        Value::Integer(value) => value.to_string(),
        Value::UnsignedInteger(value) => value.to_string(),
        Value::Decimal(value) if decimal(value) => value.clone(),
        Value::Float(value) if value.is_finite() => format!("{value:e}"),
        Value::Decimal(_) | Value::Float(_) => return Err(Error::Literal),
        Value::Bytes(value) => {
            let hex = crate::export::text::hex(value);
            match engine {
                Engine::SqlServer => format!("0x{hex}"),
                Engine::PostgreSql => format!("E'\\\\x{hex}'::bytea"),
                Engine::MySql => format!("_binary X'{hex}'"),
            }
        }
        _ => string(&crate::export::cell_text(value), engine)?,
    })
}

#[cfg(test)]
mod collate_tests {
    use super::mssql_collate;

    #[test]
    fn mssql_collation_names_are_written_bare() {
        assert_eq!(
            mssql_collate("Latin1_General_100_CI_AS_SC_UTF8").as_deref(),
            Some(" COLLATE Latin1_General_100_CI_AS_SC_UTF8")
        );
    }

    #[test]
    fn mssql_collation_names_that_need_quoting_are_refused() {
        assert_eq!(mssql_collate("Latin1]; DROP TABLE t; --"), None);
        assert_eq!(mssql_collate(""), None);
    }
}
