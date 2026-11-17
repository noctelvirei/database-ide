//! SQL literals and identifiers for replayable, single-row INSERT statements.

use std::fs::File;
use std::io::{BufWriter, Write};

use crate::danger::Engine;
use crate::session::{Column, Value};
use crate::sql::{identifier, literal};

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

impl From<crate::sql::Error> for Error {
    fn from(error: crate::sql::Error) -> Self {
        match error {
            crate::sql::Error::Identifier => Self::Identifier,
            crate::sql::Error::Literal => Self::Literal,
        }
    }
}
