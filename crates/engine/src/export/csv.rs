//! RFC 4180 CSV encoding with optional Excel UTF-8 signature.

use std::fs::File;
use std::io::Write;

use crate::session::{Column, Value};

/// A CSV encoding or flush failure.
#[derive(Debug, thiserror::Error)]
pub enum Error {
    /// Writing a record failed.
    #[error("CSV write failed: {0}")]
    Csv(#[from] ::csv::Error),
    /// Writing the BOM or flushing failed.
    #[error("CSV file I/O failed: {0}")]
    Io(#[from] std::io::Error),
}

pub(super) struct Writer(::csv::Writer<File>);

impl Writer {
    pub(super) fn new(mut file: File, columns: &[Column], bom: bool) -> Result<Self, Error> {
        if bom {
            file.write_all(b"\xef\xbb\xbf")?;
        }
        let mut writer = ::csv::WriterBuilder::new()
            .terminator(::csv::Terminator::CRLF)
            .from_writer(file);
        writer.write_record(columns.iter().map(|column| &column.name))?;
        Ok(Self(writer))
    }

    pub(super) fn row(&mut self, row: &[Value]) -> Result<(), Error> {
        self.0.write_record(row.iter().map(super::cell_text))?;
        Ok(())
    }

    pub(super) fn finish(mut self) -> Result<(), Error> {
        self.0.flush()?;
        self.0.get_ref().sync_all()?;
        Ok(())
    }
}
