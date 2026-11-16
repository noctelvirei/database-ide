//! Bounded, quoted TSV text for pasting complete cells into Excel.

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

pub(super) fn write(reader: &Reader, max_rows: usize, max_bytes: usize) -> Result<String, Error> {
    let count = reader.row_count()?;
    if count > max_rows {
        return Err(Error::Rows { limit: max_rows });
    }
    let mut output = String::new();
    record(
        &mut output,
        reader.columns().iter().map(|column| column.name.clone()),
        max_bytes,
    )?;
    for start in (0..count).step_by(super::PAGE_ROWS) {
        for row in reader.rows(start..start.saturating_add(super::PAGE_ROWS))? {
            record(&mut output, row.iter().map(super::text::cell), max_bytes)?;
        }
    }
    Ok(output)
}
