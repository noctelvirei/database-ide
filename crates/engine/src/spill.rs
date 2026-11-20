//! One result set's rows on disk, with only column metadata and row offsets retained.
//!
//! A Store owns the file's lifetime and is the sole writer; cloneable Readers share its
//! index under a mutex. Range reads snapshot published bounds, then use independent file
//! handles outside the lock. Counts and status never lock. Disk operations block: async
//! callers should use a blocking task. Closing or dropping invalidates future reads;
//! in-flight reads may finish and keep the file alive only until they return.

use std::io::{BufReader, BufWriter, Read, Seek, SeekFrom, Write};
use std::ops::Range;
use std::path::Path;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex, MutexGuard};

use tempfile::NamedTempFile;

use crate::session::{Column, Value};

mod encoding;

#[cfg(test)]
mod tests;

/// A spill operation that could not complete; errors never include row contents.
#[derive(Debug, thiserror::Error)]
pub enum Error {
    /// Creating, writing, reading or deleting the temporary file failed.
    #[error("Spill file I/O failed: {0}")]
    Io(#[from] std::io::Error),
    /// A batch contained a row whose width differs from the result columns.
    #[error("Expected {expected} cells, received {actual}")]
    RowWidth {
        /// Number of result columns.
        expected: usize,
        /// Number of cells in the rejected row.
        actual: usize,
    },
    /// The owning Store was closed or dropped.
    #[error("Result is closed")]
    Closed,
    /// Completion was already marked; reads remain available.
    #[error("Result is complete")]
    Complete,
    /// An earlier append failed; published rows remain readable but writing cannot resume.
    #[error("An earlier spill write failed")]
    WriteFailed,
    /// A panic interrupted a locked operation; the store cannot safely resume.
    #[error("Spill store lock was poisoned")]
    Poisoned,
}

#[derive(Debug)]
struct State {
    file: Option<Arc<NamedTempFile>>,
    offsets: Vec<u64>,
    end: u64,
}

#[derive(Debug)]
struct Shared {
    columns: Vec<Column>,
    state: Mutex<State>,
    row_count: AtomicUsize,
    complete: AtomicBool,
    write_failed: AtomicBool,
}

impl Shared {
    fn check_poisoned(&self) -> Result<(), Error> {
        if self.state.is_poisoned() {
            return Err(Error::Poisoned);
        }
        Ok(())
    }

    fn lock(&self) -> Result<MutexGuard<'_, State>, Error> {
        self.state.lock().map_err(|_| Error::Poisoned)
    }
}

/// Sole writer; dropping it closes the result and removes its file after in-flight reads.
#[derive(Debug)]
pub struct Store {
    shared: Arc<Shared>,
}

impl Store {
    /// Creates an empty result in the OS temp folder, or in directory when provided.
    /// Returns an I/O error if a temporary file cannot be created; columns stay in order.
    pub fn new(columns: Vec<Column>, directory: Option<&Path>) -> Result<Self, Error> {
        let file = match directory {
            Some(directory) => NamedTempFile::new_in(directory)?,
            None => NamedTempFile::new()?,
        };
        Ok(Self {
            shared: Arc::new(Shared {
                columns,
                state: Mutex::new(State {
                    file: Some(Arc::new(file)),
                    offsets: Vec::new(),
                    end: 0,
                }),
                row_count: AtomicUsize::new(0),
                complete: AtomicBool::new(false),
                write_failed: AtomicBool::new(false),
            }),
        })
    }

    /// Returns an independent handle that stays valid until this Store closes or drops.
    pub fn reader(&self) -> Reader {
        Reader {
            shared: Arc::clone(&self.shared),
        }
    }

    /// Appends a delivered row batch, publishing it atomically after buffered writes finish.
    /// Width errors leave the store unchanged. I/O errors reject subsequent appends while
    /// retaining earlier rows. Input rows and temporary encoding buffers are not retained.
    pub fn append(&mut self, rows: Vec<Vec<Value>>) -> Result<(), Error> {
        let mut state = self.shared.lock()?;
        if state.file.is_none() {
            return Err(Error::Closed);
        }
        if self.shared.complete.load(Ordering::Acquire) {
            return Err(Error::Complete);
        }
        if self.shared.write_failed.load(Ordering::Acquire) {
            return Err(Error::WriteFailed);
        }
        for row in &rows {
            if row.len() != self.shared.columns.len() {
                return Err(Error::RowWidth {
                    expected: self.shared.columns.len(),
                    actual: row.len(),
                });
            }
        }
        let State {
            file, offsets, end, ..
        } = &mut *state;
        let old_count = offsets.len();
        let mut position = *end;
        let result = (|| -> Result<(), std::io::Error> {
            let mut file = file
                .as_ref()
                .ok_or_else(|| std::io::Error::other("Result is closed"))?
                .as_file();
            file.seek(SeekFrom::Start(position))?;
            let mut writer = BufWriter::new(file);
            let mut encoded = Vec::new();
            for row in rows {
                encoded.clear();
                for value in row {
                    encoding::write(&mut encoded, &value)?;
                }
                writer.write_all(&encoded)?;
                offsets.push(position);
                position += encoded.len() as u64;
            }
            writer.flush()
        })();
        if let Err(error) = result {
            state.offsets.truncate(old_count);
            self.shared.write_failed.store(true, Ordering::Release);
            return Err(error.into());
        }
        state.end = position;
        self.shared
            .row_count
            .store(state.offsets.len(), Ordering::Release);
        Ok(())
    }

    /// Number of successfully published rows, including after completion or closure.
    /// Never waits on a lock or I/O; returns Poisoned after an interrupted index operation.
    pub fn row_count(&self) -> Result<usize, Error> {
        self.shared.check_poisoned()?;
        Ok(self.shared.row_count.load(Ordering::Acquire))
    }

    /// Marks the result complete, idempotently; later appends fail and reads remain valid.
    /// Returns Closed, WriteFailed or Poisoned if the store is unavailable or incomplete.
    pub fn complete(&mut self) -> Result<(), Error> {
        let state = self.shared.lock()?;
        if state.file.is_none() {
            return Err(Error::Closed);
        }
        if self.shared.write_failed.load(Ordering::Acquire) {
            return Err(Error::WriteFailed);
        }
        self.shared.complete.store(true, Ordering::Release);
        Ok(())
    }

    /// Invalidates future reads without waiting for in-flight reads, idempotently.
    /// Deletes the file now, reporting errors, or after the last in-flight read returns.
    /// Deferred cleanup errors cannot be reported; a deletion error still closes the store.
    pub fn close(&mut self) -> Result<(), Error> {
        if let Some(file) = self.shared.lock()?.file.take()
            && let Ok(file) = Arc::try_unwrap(file)
        {
            file.close()?;
        }
        Ok(())
    }
}

impl Drop for Store {
    fn drop(&mut self) {
        let mut state = self
            .shared
            .state
            .lock()
            .unwrap_or_else(|error| error.into_inner());
        state.file.take();
    }
}

/// Cloneable paging handle; only in-flight reads extend the closed file's lifetime.
#[derive(Debug, Clone)]
pub struct Reader {
    shared: Arc<Shared>,
}

impl Reader {
    /// Result columns in their original order, available even after closure.
    pub fn columns(&self) -> &[Column] {
        &self.shared.columns
    }

    /// Published row count without waiting on locks or I/O, including after closure.
    /// Returns Poisoned after an interrupted index operation.
    pub fn row_count(&self) -> Result<usize, Error> {
        self.shared.check_poisoned()?;
        Ok(self.shared.row_count.load(Ordering::Acquire))
    }

    /// Whether completion was explicitly marked; closure alone does not mark completion.
    /// Never waits on locks or I/O. Once true, a subsequent row_count sees the final count.
    /// Returns Poisoned after an interrupted index operation.
    pub fn is_complete(&self) -> Result<bool, Error> {
        self.shared.check_poisoned()?;
        Ok(self.shared.complete.load(Ordering::Acquire))
    }

    /// Whether an append failed during I/O, permanently preventing further writes.
    /// Lock-free; width errors, closure and index poisoning alone do not set this flag.
    pub fn write_failed(&self) -> bool {
        self.shared.write_failed.load(Ordering::Acquire)
    }

    /// Allocated index storage in bytes, excluding fixed metadata and transient I/O buffers.
    /// Grows with row count, independent of cell sizes; returns Poisoned after a panic.
    pub fn index_memory_bytes(&self) -> Result<usize, Error> {
        Ok(self.shared.lock()?.offsets.capacity() * size_of::<u64>())
    }

    /// Fetches a zero-based, end-exclusive range, clamped to the currently published rows.
    /// Reversed ranges are empty. Allocates only the requested rows; returns Closed,
    /// Poisoned or an I/O error (including invalid private encoding) on failure.
    pub fn rows(&self, range: Range<usize>) -> Result<Vec<Vec<Value>>, Error> {
        let state = self.shared.lock()?;
        if state.file.is_none() {
            return Err(Error::Closed);
        }
        let start = range.start.min(state.offsets.len());
        let end = range.end.min(state.offsets.len());
        if start >= end {
            return Ok(Vec::new());
        }
        let offset = state.offsets[start];
        let limit = state.offsets.get(end).copied().unwrap_or(state.end);
        let retained_file = Arc::clone(state.file.as_ref().ok_or(Error::Closed)?);
        drop(state);
        let mut file = retained_file.reopen()?;
        file.seek(SeekFrom::Start(offset))?;
        let mut reader = BufReader::new(file.take(limit - offset));
        let mut rows = Vec::with_capacity(end - start);
        for _ in start..end {
            let mut row = Vec::with_capacity(self.shared.columns.len());
            for _ in &self.shared.columns {
                row.push(encoding::read(&mut reader)?);
            }
            rows.push(row);
        }
        Ok(rows)
    }
}
