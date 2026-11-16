//! SQL literals and identifiers for replayable, single-row INSERT statements.

use std::fs::File;
use std::io::{BufWriter, Write};

use crate::danger::Engine;
use crate::session::{Column, Value};

/// A SQL export that could not be represented or written.
#[derive(Debug, thiserror::Error)]
pub enum Error {
    /// No explicit table and no unambiguous common base table.
    #[error("Supply an INSERT target table; columns do not share one base table")]
    MissingTable,
    /// An identifier was empty or contained a zero byte.
    #[error("INSERT identifiers must be nonempty and contain no NUL")]
    Identifier,
    /// SQL Server/PostgreSQL text cannot contain NUL, or a numeric value is not SQL syntax.
    #[error("Value cannot be represented as a SQL literal")]
    Literal,
    /// Writing or flushing failed.
    #[error("INSERT file I/O failed: {0}")]
    Io(#[from] std::io::Error),
}

pub(super) struct Writer {
    file: BufWriter<File>,
    prefix: String,
    engine: Engine,
}

fn identifier(text: &str, engine: Engine) -> Result<String, Error> {
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
        // Hex conversion avoids dependence on NO_BACKSLASH_ESCAPES and connection encoding.
        Engine::MySql => format!(
            "CONVERT(X'{}' USING utf8mb4)",
            super::text::hex(text.as_bytes())
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

fn literal(value: &Value, engine: Engine) -> Result<String, Error> {
    Ok(match value {
        Value::Null => "NULL".into(),
        Value::Bool(value) if engine == Engine::SqlServer => u8::from(*value).to_string(),
        Value::Bool(true) => "TRUE".into(),
        Value::Bool(false) => "FALSE".into(),
        Value::Integer(value) => value.to_string(),
        Value::UnsignedInteger(value) => value.to_string(),
        Value::Decimal(value) if decimal(value) => value.clone(),
        Value::Float(value) if value.is_finite() => value.to_string(),
        Value::Decimal(_) | Value::Float(_) => return Err(Error::Literal),
        Value::Bytes(value) => {
            let hex = super::text::hex(value);
            match engine {
                Engine::SqlServer => format!("0x{hex}"),
                Engine::PostgreSql => format!("E'\\\\x{hex}'::bytea"),
                Engine::MySql => format!("_binary X'{hex}'"),
            }
        }
        _ => string(&super::text::cell(value), engine)?,
    })
}

impl Writer {
    pub(super) fn new(
        file: File,
        columns: &[Column],
        engine: Engine,
        table: Option<&[String]>,
    ) -> Result<Self, Error> {
        let inferred;
        let table = match table {
            Some(table) => table,
            None => {
                let base = columns
                    .first()
                    .and_then(|column| column.base_table.as_deref())
                    .ok_or(Error::MissingTable)?;
                if !columns
                    .iter()
                    .all(|column| column.base_table.as_deref() == Some(base))
                {
                    return Err(Error::MissingTable);
                }
                inferred = base.split('.').map(str::to_owned).collect::<Vec<_>>();
                &inferred
            }
        };
        if table.is_empty() || columns.is_empty() {
            return Err(Error::Identifier);
        }
        let table = table
            .iter()
            .map(|part| identifier(part, engine))
            .collect::<Result<Vec<_>, _>>()?
            .join(".");
        let columns = columns
            .iter()
            .map(|column| {
                identifier(
                    column.base_column.as_deref().unwrap_or(&column.name),
                    engine,
                )
            })
            .collect::<Result<Vec<_>, _>>()?
            .join(", ");
        Ok(Self {
            file: BufWriter::new(file),
            prefix: format!("INSERT INTO {table} ({columns}) VALUES ("),
            engine,
        })
    }

    pub(super) fn row(&mut self, row: &[Value]) -> Result<(), Error> {
        let values = row
            .iter()
            .map(|value| literal(value, self.engine))
            .collect::<Result<Vec<_>, _>>()?;
        writeln!(self.file, "{}{});", self.prefix, values.join(", "))?;
        Ok(())
    }

    pub(super) fn finish(mut self) -> Result<(), Error> {
        self.file.flush()?;
        self.file.get_ref().sync_all()?;
        Ok(())
    }
}
