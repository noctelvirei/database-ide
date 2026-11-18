//! Bounded, quoted TSV text for pasting complete cells into Excel.

use std::ops::Range;

use crate::spill::Reader;

/// Clipboard output was unavailable or larger than a caller's limits.
#[derive(Debug, thiserror::Error)]
pub enum Error {
    /// Data row count exceeds the bound, before reading any rows.
    #[error("Clipboard row limit exceeded ({limit} rows)")]
    Rows {
        /// Maximum data rows allowed.
        limit: usize,
    },
    /// UTF-8 output, including headers and quoting, exceeds the bound.
    #[error("Clipboard byte limit exceeded ({limit} bytes)")]
    Bytes {
        /// Maximum encoded bytes allowed.
        limit: usize,
    },
    /// A spill read failed.
    #[error(transparent)]
    Spill(#[from] crate::spill::Error),
}

fn push(output: &mut String, text: &str, limit: usize) -> Result<(), Error> {
    if text.len() > limit.saturating_sub(output.len()) {
        return Err(Error::Bytes { limit });
    }
    output.push_str(text);
    Ok(())
}

fn record(
    output: &mut String,
    cells: impl Iterator<Item = String>,
    limit: usize,
) -> Result<(), Error> {
    for (index, cell) in cells.enumerate() {
        if index != 0 {
            push(output, "\t", limit)?;
        }
        if cell.contains(['\t', '\r', '\n', '"']) {
            push(output, "\"", limit)?;
            for part in cell.split_inclusive('"') {
                push(output, part, limit)?;
                if part.ends_with('"') {
                    push(output, "\"", limit)?;
                }
            }
            push(output, "\"", limit)?;
        } else {
            push(output, &cell, limit)?;
        }
    }
    push(output, "\r\n", limit)
}

pub(super) fn write(
    reader: &Reader,
    rows: Range<usize>,
    columns: Range<usize>,
    include_headers: bool,
    max_rows: usize,
    max_bytes: usize,
) -> Result<String, Error> {
    let count = reader.row_count()?;
    let rows = rows.start.min(count)..rows.end.min(count);
    let width = reader.columns().len();
    let end = columns.end.min(width);
    let columns = columns.start.min(end)..end;
    if rows.end.saturating_sub(rows.start) > max_rows {
        return Err(Error::Rows { limit: max_rows });
    }
    let mut output = String::new();
    if include_headers {
        record(
            &mut output,
            reader.columns()[columns.clone()]
                .iter()
                .map(|column| column.name.clone()),
            max_bytes,
        )?;
    }
    for start in (rows.start..rows.end).step_by(super::PAGE_ROWS) {
        let end = start.saturating_add(super::PAGE_ROWS).min(rows.end);
        for row in reader.rows(start..end)? {
            record(
                &mut output,
                row[columns.clone()].iter().map(super::cell_text),
                max_bytes,
            )?;
        }
    }
    Ok(output)
}
