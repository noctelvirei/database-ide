//! One result set's rows on disk, with only column metadata and row offsets retained.
//!
//! A Store owns the file's lifetime and is the sole writer; cloneable Readers share its
//! index and file under a mutex. Appends and range reads are serialized, so each read sees
//! whole published batches. Read bounded pages to let appends progress between reads.
//! Operations block: async callers should use a blocking task. Completing keeps rows
//! readable; closing or dropping the Store invalidates every Reader and removes the file.

use std::io::{BufReader, BufWriter, Read, Seek, SeekFrom, Write};
use std::ops::Range;
use std::path::Path;
use std::sync::{Arc, Mutex, MutexGuard};

use tempfile::NamedTempFile;

use crate::session::{Column, Value};

mod encoding;

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
    file: Option<NamedTempFile>,
    offsets: Vec<u64>,
    end: u64,
    complete: bool,
    write_failed: bool,
}

#[derive(Debug)]
struct Shared {
    columns: Vec<Column>,
    state: Mutex<State>,
}

impl Shared {
    fn lock(&self) -> Result<MutexGuard<'_, State>, Error> {
        self.state.lock().map_err(|_| Error::Poisoned)
    }
}

/// Sole writer and lifetime owner for a result set; dropping it removes its temporary file.
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
                    file: Some(file),
                    offsets: Vec::new(),
                    end: 0,
                    complete: false,
                    write_failed: false,
                }),
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
        if state.complete {
            return Err(Error::Complete);
        }
        if state.write_failed {
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
            let file = file
                .as_mut()
                .ok_or_else(|| std::io::Error::other("Result is closed"))?;
            file.seek(SeekFrom::Start(position))?;
            let mut writer = BufWriter::new(file.as_file_mut());
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
            state.write_failed = true;
            return Err(error.into());
        }
        state.end = position;
        Ok(())
    }

    /// Number of successfully published rows, including after completion or closure.
    /// Returns Poisoned if a panic interrupted a locked operation.
    pub fn row_count(&self) -> Result<usize, Error> {
        Ok(self.shared.lock()?.offsets.len())
    }

    /// Marks the result complete, idempotently; later appends fail and reads remain valid.
    /// Returns Closed, WriteFailed or Poisoned if the store is unavailable or incomplete.
    pub fn complete(&mut self) -> Result<(), Error> {
        let mut state = self.shared.lock()?;
        if state.file.is_none() {
            return Err(Error::Closed);
        }
        if state.write_failed {
            return Err(Error::WriteFailed);
        }
        state.complete = true;
        Ok(())
    }

    /// Invalidates readers and deletes the file, idempotently; reports deletion errors.
    /// A deletion error still closes the store; Drop cannot report cleanup errors.
    pub fn close(&mut self) -> Result<(), Error> {
        if let Some(file) = self.shared.lock()?.file.take() {
            file.close()?;
        }
        Ok(())
    }
}

impl Drop for Store {
    fn drop(&mut self) {
        // Even a poisoned lock must not keep the file alive through outstanding readers.
        let mut state = self
            .shared
            .state
            .lock()
            .unwrap_or_else(|error| error.into_inner());
        state.file.take();
    }
}

/// Cloneable paging handle; holds no row cache and does not extend the file's lifetime.
#[derive(Debug, Clone)]
pub struct Reader {
    shared: Arc<Shared>,
}

impl Reader {
    /// Result columns in their original order, available even after closure.
    pub fn columns(&self) -> &[Column] {
        &self.shared.columns
    }

    /// Number of successfully published rows; returns Poisoned after an interrupted lock.
    pub fn row_count(&self) -> Result<usize, Error> {
        Ok(self.shared.lock()?.offsets.len())
    }

    /// Whether completion was explicitly marked; closure alone does not mark completion.
    /// Returns Poisoned after an interrupted lock.
    pub fn is_complete(&self) -> Result<bool, Error> {
        Ok(self.shared.lock()?.complete)
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
        let mut state = self.shared.lock()?;
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
        let file = state.file.as_mut().ok_or(Error::Closed)?;
        file.seek(SeekFrom::Start(offset))?;
        let mut reader = BufReader::new(file.as_file_mut().take(limit - offset));
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
