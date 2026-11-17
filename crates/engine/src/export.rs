//! Paged result exports and direct run-to-file output, without retaining result rows.
//!
//! File operations block; run these operations on a blocking worker, with a runtime for
//! `run_to_file`. Destinations are never overwritten. Writes use `name.ext.partial` until
//! complete; errors leave that marker on disk, including a finalized workbook on cancellation.

use std::fs::File;
use std::path::{Path, PathBuf};

use futures::StreamExt;

use crate::danger::Engine;
use crate::session::{Column, Event, Run, Value};
use crate::spill::Reader;

mod clipboard;
mod csv;
mod insert;
pub(crate) mod text;
mod xlsx;

/// Maximum rows fetched per spill read; one unusually large cell can still require memory.
pub const PAGE_ROWS: usize = 256;

/// File encoding. `Default` is CSV with a UTF-8 BOM for Excel.
#[derive(Debug, Clone)]
pub enum Format {
    /// RFC 4180 records, with NULL represented by an empty field.
    Csv {
        /// Whether to prefix UTF-8 output with a byte-order mark.
        bom: bool,
    },
    /// Constant-memory workbook, repeating headers on each overflow sheet. Integers and
    /// decimals beyond 15 significant digits or Excel's numeric range stay as text, as do
    /// floats outside Excel's numeric range, out-of-range dates and offset-bearing times/timestamps.
    Xlsx,
    /// One INSERT per row, avoiding every engine's multi-row VALUES limit.
    Insert {
        /// SQL syntax used to quote identifiers and values.
        engine: Engine,
        /// Unquoted identifier components (schema, table, etc.). None uses the common
        /// base table, splitting its server-reported name on dots; pass components for
        /// identifiers that contain literal dots. Base column names override aliases.
        table: Option<Vec<String>>,
    },
}

impl Default for Format {
    fn default() -> Self {
        Self::Csv { bom: true }
    }
}

/// An export failure; partial files remain distinguishable from completed output.
#[derive(Debug, thiserror::Error)]
pub enum Error {
    /// Opening, flushing or publishing a file failed.
    #[error("Export file I/O failed: {0}")]
    Io(#[from] std::io::Error),
    /// The result store was unavailable.
    #[error(transparent)]
    Spill(#[from] crate::spill::Error),
    /// Clipboard output exceeded a caller's bound.
    #[error(transparent)]
    Clipboard(#[from] clipboard::Error),
    /// CSV encoding failed.
    #[error(transparent)]
    Csv(#[from] csv::Error),
    /// Workbook encoding failed.
    #[error(transparent)]
    Xlsx(#[from] xlsx::Error),
    /// SQL values or the target table could not be represented safely.
    #[error(transparent)]
    Insert(#[from] insert::Error),
    /// Rows arrived before column metadata, or had the wrong width.
    #[error("Invalid result row: expected {expected} cells, received {actual}")]
    RowWidth {
        /// Width required by the latest result header (zero without a header).
        expected: usize,
        /// Delivered cell count.
        actual: usize,
    },
    /// A spill store was still receiving rows, so no complete snapshot was available.
    #[error("Complete the result before exporting")]
    IncompleteResult,
}

/// One completed or explicitly partial file and the number of data rows written.
#[derive(Debug)]
pub struct FileReport {
    /// Actual output name; `.partial` means incomplete.
    pub path: PathBuf,
    /// Data rows successfully handed to the file encoder, excluding headers.
    pub rows: u64,
    /// Largest spill page fetched; zero for direct run-to-file output.
    pub max_page_rows: usize,
}

/// Files and non-row events for the UI; event order and native error details are preserved.
#[derive(Debug, Default)]
pub struct RunReport {
    /// One file per result set, including empty result sets.
    pub files: Vec<FileReport>,
    /// Messages, row counts, plans, errors and Done, in stream order.
    pub events: Vec<Event>,
}

/// Returns Excel-compatible quoted tab-separated text with headers and CRLF records.
/// NULL is empty. Refuses more than `max_rows` data rows or `max_bytes` UTF-8 bytes
/// (including headers); row reads are bounded by PAGE_ROWS. Requires a completed result.
pub fn clipboard(reader: &Reader, max_rows: usize, max_bytes: usize) -> Result<String, Error> {
    if !reader.is_complete()? {
        return Err(Error::IncompleteResult);
    }
    Ok(clipboard::write(reader, max_rows, max_bytes)?)
}

/// Writes a completed spill result to a new file; refuses existing destinations and leaves
/// `.partial` output on failure. Reports data rows and the largest page actually fetched.
pub fn write(reader: &Reader, path: &Path, format: &Format) -> Result<FileReport, Error> {
    if !reader.is_complete()? {
        return Err(Error::IncompleteResult);
    }
    let mut output = Output::new(path, format, reader.columns())?;
    let count = reader.row_count()?;
    for start in (0..count).step_by(PAGE_ROWS) {
        let page = reader.rows(start..start.saturating_add(PAGE_ROWS))?;
        output.report.max_page_rows = output.report.max_page_rows.max(page.len());
        for row in page {
            output.row(&row)?;
        }
    }
    output.finish(true)
}

/// Consumes a Run directly, without a grid or spill store. `rows.csv` produces
/// `rows-1.csv`, `rows-2.csv`, etc., even for a single result set. Non-row events are
/// returned for the messages tab. Any Error marks the active and subsequent files partial;
/// cancellation stops further rows and finalizes the active `.partial` file. Local errors
/// return Err and leave `.partial` files. Server errors remain events in a successful report.
pub async fn run_to_file(
    mut run: Run<'_>,
    path: &Path,
    format: &Format,
) -> Result<RunReport, Error> {
    let mut report = RunReport::default();
    let mut output: Option<Output> = None;
    let mut complete = true;
    while let Some(event) = run.next().await {
        match event {
            Event::ResultSetStarted { columns } => {
                if let Some(previous) = output.take() {
                    report.files.push(previous.finish(complete)?);
                }
                let mut name = path.file_stem().unwrap_or_default().to_os_string();
                name.push(format!("-{}", report.files.len() + 1));
                if let Some(extension) = path.extension() {
                    name.push(".");
                    name.push(extension);
                }
                output = Some(Output::new(&path.with_file_name(name), format, &columns)?);
            }
            Event::Rows(rows) => {
                for row in rows {
                    let current = output.as_mut().ok_or(Error::RowWidth {
                        expected: 0,
                        actual: row.len(),
                    })?;
                    current.row(&row)?;
                }
            }
            event => {
                if matches!(event, Event::Error { .. }) {
                    complete = false;
                }
                report.events.push(event);
            }
        }
    }
    if let Some(last) = output {
        report.files.push(last.finish(complete)?);
    }
    Ok(report)
}

enum Writer {
    Csv(Box<csv::Writer>),
    Xlsx(Box<xlsx::Writer>),
    Insert(insert::Writer),
}

struct Output {
    writer: Writer,
    path: PathBuf,
    partial: PathBuf,
    width: usize,
    report: FileReport,
}

impl Output {
    fn new(path: &Path, format: &Format, columns: &[Column]) -> Result<Self, Error> {
        if path.try_exists()? {
            return Err(
                std::io::Error::new(std::io::ErrorKind::AlreadyExists, "Export exists").into(),
            );
        }
        let mut partial = path.as_os_str().to_os_string();
        partial.push(".partial");
        let partial = PathBuf::from(partial);
        let file = File::create_new(&partial)?;
        let writer = match format {
            Format::Csv { bom } => Writer::Csv(Box::new(csv::Writer::new(file, columns, *bom)?)),
            Format::Xlsx => Writer::Xlsx(Box::new(xlsx::Writer::new(file, columns)?)),
            Format::Insert { engine, table } => Writer::Insert(insert::Writer::new(
                file,
                columns,
                *engine,
                table.as_deref(),
            )?),
        };
        Ok(Self {
            writer,
            path: path.to_owned(),
            partial: partial.clone(),
            width: columns.len(),
            report: FileReport {
                path: partial,
                rows: 0,
                max_page_rows: 0,
            },
        })
    }

    fn row(&mut self, row: &[Value]) -> Result<(), Error> {
        if row.len() != self.width {
            return Err(Error::RowWidth {
                expected: self.width,
                actual: row.len(),
            });
        }
        match &mut self.writer {
            Writer::Csv(writer) => writer.row(row)?,
            Writer::Xlsx(writer) => writer.row(row)?,
            Writer::Insert(writer) => writer.row(row)?,
        }
        self.report.rows += 1;
        Ok(())
    }

    fn finish(mut self, complete: bool) -> Result<FileReport, Error> {
        match self.writer {
            Writer::Csv(writer) => writer.finish()?,
            Writer::Xlsx(writer) => writer.finish()?,
            Writer::Insert(writer) => writer.finish()?,
        }
        if complete {
            // Hard linking publishes without replacing a destination created during writing.
            std::fs::hard_link(&self.partial, &self.path)?;
            std::fs::remove_file(&self.partial)?;
            self.report.path = self.path;
        }
        Ok(self.report)
    }
}
