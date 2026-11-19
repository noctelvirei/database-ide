//! Replacing a file whole: the new contents go to a hidden file beside it, which then takes
//! its place, so a failed or interrupted write leaves the old file as it was and two writes
//! to one file never mix their bytes.

use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};

/// Writes `bytes` to `path`, replacing any file there only once all of them are written.
/// Blocking; call it off the UI thread for anything large.
pub fn write(path: &Path, bytes: &[u8]) -> std::io::Result<()> {
    let staging = staging_path(path);
    std::fs::write(&staging, bytes)
        .and_then(|()| std::fs::rename(&staging, path))
        .inspect_err(|_| {
            let _ = std::fs::remove_file(&staging);
        })
}

/// A staging name beside `path` that no other write uses: `dir/~name.ext.<process>-<n>.saving`
/// for `dir/name.ext`, `n` counting this process's writes.
pub fn staging_path(path: &Path) -> PathBuf {
    static WRITES: AtomicU64 = AtomicU64::new(0);
    let write = WRITES.fetch_add(1, Ordering::Relaxed);
    let mut name = std::ffi::OsString::from("~");
    name.push(path.file_name().unwrap_or_default());
    name.push(format!(".{}-{write}.saving", std::process::id()));
    path.with_file_name(name)
}
