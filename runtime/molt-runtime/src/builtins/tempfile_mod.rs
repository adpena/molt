#![allow(dead_code, unused_imports)]
// === FILE: runtime/molt-runtime/src/builtins/tempfile_mod.rs ===
//
// tempfile intrinsics: secure temporary files and directories.
//
// Delegates to the Rust `tempfile` crate for secure temp file creation.

#[cfg(target_arch = "wasm32")]
use crate::libc_compat as libc;

use crate::*;
use std::path::{Path, PathBuf};

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

#[inline]
fn str_bits(py: &PyToken<'_>, s: &str) -> u64 {
    let ptr = alloc_string(py, s.as_bytes());
    if ptr.is_null() {
        MoltObject::none().bits()
    } else {
        MoltObject::from_ptr(ptr).bits()
    }
}

fn require_str_opt(_py: &PyToken<'_>, bits: u64) -> Option<String> {
    if obj_from_bits(bits).is_none() {
        None
    } else {
        string_obj_to_owned(obj_from_bits(bits))
    }
}

// ---------------------------------------------------------------------------
// Public intrinsics
// ---------------------------------------------------------------------------

/// `tempfile.gettempdir()` -> str
#[unsafe(no_mangle)]
pub extern "C" fn molt_tempfile_gettempdir() -> u64 {
    crate::with_gil_entry!(_py, {
        let dir = std::env::temp_dir();
        str_bits(_py, &dir.to_string_lossy())
    })
}

/// `tempfile.gettempdirb()` -> bytes
#[unsafe(no_mangle)]
pub extern "C" fn molt_tempfile_gettempdirb() -> u64 {
    crate::with_gil_entry!(_py, {
        let dir = std::env::temp_dir();
        let s = dir.to_string_lossy();
        let ptr = alloc_bytes(_py, s.as_bytes());
        if ptr.is_null() {
            raise_exception::<u64>(_py, "MemoryError", "out of memory")
        } else {
            MoltObject::from_ptr(ptr).bits()
        }
    })
}

/// `tempfile.mkdtemp(suffix=None, prefix=None, dir=None)` -> str
///
/// Creates a temporary directory and returns its path.
#[unsafe(no_mangle)]
pub extern "C" fn molt_tempfile_mkdtemp(suffix_bits: u64, prefix_bits: u64, dir_bits: u64) -> u64 {
    crate::with_gil_entry!(_py, {
        if !has_capability(_py, "fs.write") {
            return raise_exception::<u64>(_py, "PermissionError", "missing fs.write capability");
        }
        let suffix = require_str_opt(_py, suffix_bits).unwrap_or_default();
        let prefix = require_str_opt(_py, prefix_bits).unwrap_or_else(|| "tmp".to_string());
        let base_dir = match require_str_opt(_py, dir_bits) {
            Some(d) => PathBuf::from(d),
            None => std::env::temp_dir(),
        };

        // Use tempfile crate to create a secure temp dir
        match tempfile::Builder::new()
            .prefix(&prefix)
            .suffix(&suffix)
            .tempdir_in(&base_dir)
        {
            Ok(td) => {
                // into_path() so the directory persists after the TempDir is dropped
                let path = td.keep();
                str_bits(_py, &path.to_string_lossy())
            }
            Err(err) => raise_os_error::<u64>(_py, err, "mkdtemp"),
        }
    })
}

/// `tempfile.mkstemp(suffix=None, prefix=None, dir=None, text=False)`
/// -> tuple[int, str]  (fd, path)
#[cfg(not(target_arch = "wasm32"))]
#[unsafe(no_mangle)]
pub extern "C" fn molt_tempfile_mkstemp(suffix_bits: u64, prefix_bits: u64, dir_bits: u64) -> u64 {
    crate::with_gil_entry!(_py, {
        if !has_capability(_py, "fs.write") {
            return raise_exception::<u64>(_py, "PermissionError", "missing fs.write capability");
        }
        let suffix = require_str_opt(_py, suffix_bits).unwrap_or_default();
        let prefix = require_str_opt(_py, prefix_bits).unwrap_or_else(|| "tmp".to_string());
        let base_dir = match require_str_opt(_py, dir_bits) {
            Some(d) => PathBuf::from(d),
            None => std::env::temp_dir(),
        };

        match tempfile::Builder::new()
            .prefix(&prefix)
            .suffix(&suffix)
            .tempfile_in(&base_dir)
        {
            Ok(named) => {
                let path = named.path().to_string_lossy().into_owned();
                // Keep the file open; extract the raw fd
                #[cfg(unix)]
                let fd = {
                    use std::os::unix::io::IntoRawFd;
                    let (file, _path) = match named.keep() {
                        Ok(kept) => kept,
                        Err(e) => {
                            return raise_exception::<u64>(
                                _py, "OSError",
                                &format!("tempfile keep failed: {e}"),
                            );
                        }
                    };
                    file.into_raw_fd() as i64
                };
                #[cfg(windows)]
                let fd = {
                    use std::os::windows::io::IntoRawHandle;
                    let (file, _path) = match named.keep() {
                        Ok(kept) => kept,
                        Err(e) => {
                            return raise_exception::<u64>(
                                _py, "OSError",
                                &format!("tempfile keep failed: {e}"),
                            );
                        }
                    };
                    file.into_raw_handle() as i64
                };
                #[cfg(not(any(unix, windows)))]
                let fd: i64 = -1;

                let elems = [MoltObject::from_int(fd).bits(), str_bits(_py, &path)];
                let ptr = alloc_tuple(_py, &elems);
                if ptr.is_null() {
                    return raise_exception::<u64>(_py, "MemoryError", "out of memory");
                }
                MoltObject::from_ptr(ptr).bits()
            }
            Err(err) => raise_os_error::<u64>(_py, err, "mkstemp"),
        }
    })
}

#[cfg(target_arch = "wasm32")]
#[unsafe(no_mangle)]
pub extern "C" fn molt_tempfile_mkstemp(
    _suffix_bits: u64,
    _prefix_bits: u64,
    _dir_bits: u64,
) -> u64 {
    crate::with_gil_entry!(_py, {
        raise_os_error_errno::<u64>(_py, libc::ENOSYS as i64, "mkstemp")
    })
}

/// `tempfile.NamedTemporaryFile(suffix=None, prefix=None, dir=None, delete=True)`
/// -> tuple[int, str, bool]  (fd, path, delete)
///
/// Creates a named temporary file. Returns the fd, the path, and the delete flag
/// so the Python wrapper can manage cleanup.
#[cfg(not(target_arch = "wasm32"))]
#[unsafe(no_mangle)]
pub extern "C" fn molt_tempfile_named(
    suffix_bits: u64,
    prefix_bits: u64,
    dir_bits: u64,
    delete_bits: u64,
) -> u64 {
    crate::with_gil_entry!(_py, {
        if !has_capability(_py, "fs.write") {
            return raise_exception::<u64>(_py, "PermissionError", "missing fs.write capability");
        }
        let suffix = require_str_opt(_py, suffix_bits).unwrap_or_default();
        let prefix = require_str_opt(_py, prefix_bits).unwrap_or_else(|| "tmp".to_string());
        let base_dir = match require_str_opt(_py, dir_bits) {
            Some(d) => PathBuf::from(d),
            None => std::env::temp_dir(),
        };
        let delete = if obj_from_bits(delete_bits).is_none() {
            true
        } else {
            is_truthy(_py, obj_from_bits(delete_bits))
        };

        match tempfile::Builder::new()
            .prefix(&prefix)
            .suffix(&suffix)
            .tempfile_in(&base_dir)
        {
            Ok(named) => {
                let path = named.path().to_string_lossy().into_owned();
                #[cfg(unix)]
                let fd = {
                    use std::os::unix::io::IntoRawFd;
                    let (file, _path) = match named.keep() {
                        Ok(kept) => kept,
                        Err(e) => {
                            return raise_exception::<u64>(
                                _py, "OSError",
                                &format!("tempfile keep failed: {e}"),
                            );
                        }
                    };
                    file.into_raw_fd() as i64
                };
                #[cfg(windows)]
                let fd = {
                    use std::os::windows::io::IntoRawHandle;
                    let (file, _path) = match named.keep() {
                        Ok(kept) => kept,
                        Err(e) => {
                            return raise_exception::<u64>(
                                _py, "OSError",
                                &format!("tempfile keep failed: {e}"),
                            );
                        }
                    };
                    file.into_raw_handle() as i64
                };
                #[cfg(not(any(unix, windows)))]
                let fd: i64 = -1;

                let elems = [
                    MoltObject::from_int(fd).bits(),
                    str_bits(_py, &path),
                    MoltObject::from_bool(delete).bits(),
                ];
                let ptr = alloc_tuple(_py, &elems);
                if ptr.is_null() {
                    return raise_exception::<u64>(_py, "MemoryError", "out of memory");
                }
                MoltObject::from_ptr(ptr).bits()
            }
            Err(err) => raise_os_error::<u64>(_py, err, "NamedTemporaryFile"),
        }
    })
}

#[cfg(target_arch = "wasm32")]
#[unsafe(no_mangle)]
pub extern "C" fn molt_tempfile_named(
    _suffix_bits: u64,
    _prefix_bits: u64,
    _dir_bits: u64,
    _delete_bits: u64,
) -> u64 {
    crate::with_gil_entry!(_py, {
        raise_os_error_errno::<u64>(_py, libc::ENOSYS as i64, "NamedTemporaryFile")
    })
}

/// `tempfile.TemporaryDirectory(suffix=None, prefix=None, dir=None)`
/// -> str  (path of the created directory)
///
/// Cleanup is handled by the Python wrapper's __exit__.
#[unsafe(no_mangle)]
pub extern "C" fn molt_tempfile_tempdir(suffix_bits: u64, prefix_bits: u64, dir_bits: u64) -> u64 {
    crate::with_gil_entry!(_py, {
        if !has_capability(_py, "fs.write") {
            return raise_exception::<u64>(_py, "PermissionError", "missing fs.write capability");
        }
        let suffix = require_str_opt(_py, suffix_bits).unwrap_or_default();
        let prefix = require_str_opt(_py, prefix_bits).unwrap_or_else(|| "tmp".to_string());
        let base_dir = match require_str_opt(_py, dir_bits) {
            Some(d) => PathBuf::from(d),
            None => std::env::temp_dir(),
        };

        match tempfile::Builder::new()
            .prefix(&prefix)
            .suffix(&suffix)
            .tempdir_in(&base_dir)
        {
            Ok(td) => {
                let path = td.keep();
                str_bits(_py, &path.to_string_lossy())
            }
            Err(err) => raise_os_error::<u64>(_py, err, "TemporaryDirectory"),
        }
    })
}

/// `tempfile._cleanup(path)` -> None
///
/// Removes a temporary directory tree. Used by TemporaryDirectory.__exit__.
#[unsafe(no_mangle)]
pub extern "C" fn molt_tempfile_cleanup(path_bits: u64) -> u64 {
    crate::with_gil_entry!(_py, {
        if !has_capability(_py, "fs.write") {
            return raise_exception::<u64>(_py, "PermissionError", "missing fs.write capability");
        }
        let s = match string_obj_to_owned(obj_from_bits(path_bits)) {
            Some(s) => s,
            None => return raise_exception::<u64>(_py, "TypeError", "path must be str"),
        };
        let _ = std::fs::remove_dir_all(&s);
        MoltObject::none().bits()
    })
}

/// `tempfile.tempdir_path()` -> str  (the system temp directory)
/// Alias for gettempdir for internal use.
#[unsafe(no_mangle)]
pub extern "C" fn molt_tempfile_tempdir_path() -> u64 {
    molt_tempfile_gettempdir()
}
