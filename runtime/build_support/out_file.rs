//! Build-script outputs that keep their mtime when their bytes do not change.
//!
//! Cargo and rustc judge a dependent current by comparing input mtimes, so a
//! build script that reruns (any `rerun-if-env-changed` value, such as the
//! PATH that each landing gate projects, changing) and rewrites identical
//! OUT_DIR files makes every output that includes them stale: on 2026-11-07 a
//! freshly built molt-wasm-host turned stale mid-landing that way.

use std::fs;
use std::io;
use std::path::Path;

/// Write `contents` to `path` unless the file already holds exactly them.
///
/// A change is written beside the target and renamed over it, so a reader
/// sees the old bytes or the new ones, never a partial file.
pub fn write_if_changed(path: impl AsRef<Path>, contents: impl AsRef<[u8]>) -> io::Result<()> {
    let path = path.as_ref();
    let contents = contents.as_ref();
    match fs::read(path) {
        Ok(existing) if existing == contents => return Ok(()),
        Ok(_) => {}
        Err(error) if error.kind() == io::ErrorKind::NotFound => {}
        Err(error) => return Err(error),
    }
    let name = path
        .file_name()
        .ok_or_else(|| io::Error::new(io::ErrorKind::InvalidInput, "output has no file name"))?;
    let mut staged_name = std::ffi::OsString::from(".");
    staged_name.push(name);
    staged_name.push(".tmp");
    let staged = path.with_file_name(staged_name);
    fs::write(&staged, contents)?;
    fs::rename(&staged, path)
}
