//! Writing results to files the user names (docs/spec.md, "Exports"): the file's extension
//! picks the format, and saving over an existing file only replaces it once the new one is
//! complete.

use std::path::{Path, PathBuf};

use dbide_engine::danger::Engine;
use dbide_engine::export::{self, FileReport, Format};
use dbide_engine::session::Column;
use dbide_engine::spill::Reader;

/// The name the save prompt suggests; CSV is the format most people want.
pub const SUGGESTED_NAME: &str = "results.csv";

/// The format for saving the result set `columns` to `path`: `.csv`, `.xlsx`, or `.sql` for
/// INSERT statements in `engine`'s dialect. The INSERTs target the result's base table, or
/// the file's name when the columns don't share one (SQL Server never reports one for a
/// plain SELECT). Any other extension is refused with the reason.
pub fn save_format(path: &Path, engine: Engine, columns: &[Column]) -> Result<Format, String> {
    match extension(path).as_deref() {
        Some("csv") => Ok(Format::Csv { bom: true }),
        Some("xlsx") => Ok(Format::Xlsx),
        Some("sql") => Ok(Format::Insert {
            engine,
            table: (!shares_base_table(columns)).then(|| vec![file_stem(path)]),
        }),
        _ => Err(format!(
            "Results save as .csv, .xlsx or .sql (INSERT statements), not {}.",
            path.display()
        )),
    }
}

/// The format for a run to file into `path`: `.csv` or `.xlsx`. A run's result sets aren't
/// known before it starts, so INSERTs, which need a target table, come only from a grid.
pub fn run_format(path: &Path) -> Result<Format, String> {
    match extension(path).as_deref() {
        Some("csv") => Ok(Format::Csv { bom: true }),
        Some("xlsx") => Ok(Format::Xlsx),
        _ => Err(format!(
            "Run to file writes .csv or .xlsx, not {}. Save a grid with Ctrl+S for INSERT \
             statements.",
            path.display()
        )),
    }
}

/// `path`'s extension in lower case.
fn extension(path: &Path) -> Option<String> {
    path.extension()
        .and_then(|e| e.to_str())
        .map(str::to_ascii_lowercase)
}

/// True when every column names the same base table: the rule the engine's INSERT writer
/// applies when given no table (`dbide_engine::export`, its `insert` module).
fn shares_base_table(columns: &[Column]) -> bool {
    let first = columns.first().and_then(|c| c.base_table.as_deref());
    first.is_some() && columns.iter().all(|c| c.base_table.as_deref() == first)
}

/// `dir/name.ext` gives `name`.
fn file_stem(path: &Path) -> String {
    path.file_stem()
        .unwrap_or_default()
        .to_string_lossy()
        .into_owned()
}

/// Writes the complete result `reader` to `path` in `format`. The engine never writes over
/// an existing file, so the rows go to a hidden file beside it, which then replaces `path`:
/// a failed export leaves an existing file as it was. Blocking; call it off the UI thread.
pub fn write(reader: &Reader, path: &Path, format: &Format) -> Result<FileReport, export::Error> {
    let staging = staging_path(path);
    // A staging file left by an export that crashed would make the engine refuse.
    let _ = std::fs::remove_file(&staging);
    let mut report = export::write(reader, &staging, format).inspect_err(|_| {
        // The engine keeps a failed write as `<name>.partial`; nobody knows this hidden name.
        let mut partial = staging.clone().into_os_string();
        partial.push(".partial");
        let _ = std::fs::remove_file(partial);
    })?;
    std::fs::rename(&staging, path)?;
    report.path = path.to_path_buf();
    Ok(report)
}

/// What the messages strip says about one written file.
pub fn written(file: &FileReport) -> String {
    let rows = match file.rows {
        1 => "1 row".to_owned(),
        rows => format!("{rows} rows"),
    };
    // The engine names a file `.partial` when the run or the write stopped before the end.
    if file.path.extension().is_some_and(|e| e == "partial") {
        format!(
            "Wrote {rows} to {} before it stopped; the file is incomplete.",
            file.path.display()
        )
    } else {
        format!("Wrote {rows} to {}.", file.path.display())
    }
}

/// `dir/~name.ext.saving` for `dir/name.ext`.
fn staging_path(path: &Path) -> PathBuf {
    let mut name = std::ffi::OsString::from("~");
    name.push(path.file_name().unwrap_or_default());
    name.push(".saving");
    path.with_file_name(name)
}
