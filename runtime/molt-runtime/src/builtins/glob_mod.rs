#![allow(dead_code, unused_imports)]
// === FILE: runtime/molt-runtime/src/builtins/glob_mod.rs ===
//
// glob intrinsics: Unix-style pathname pattern expansion.
//
// glob_escape, glob_has_magic, glob_translate already live in io.rs.
// This file adds: glob() and iglob().

use crate::audit::{AuditArgs, audit_capability_decision};
use crate::*;
use std::path::Path;

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

fn list_of_strings(py: &PyToken<'_>, items: &[String]) -> u64 {
    let bits: Vec<u64> = items.iter().map(|s| str_bits(py, s)).collect();
    let ptr = alloc_list(py, &bits);
    if ptr.is_null() {
        raise_exception::<u64>(py, "MemoryError", "out of memory")
    } else {
        MoltObject::from_ptr(ptr).bits()
    }
}

fn require_str(py: &PyToken<'_>, bits: u64, label: &str) -> Result<String, u64> {
    match string_obj_to_owned(obj_from_bits(bits)) {
        Some(s) => Ok(s),
        None => Err(raise_exception::<u64>(
            py,
            "TypeError",
            &format!("{label} must be str"),
        )),
    }
}

// ---------------------------------------------------------------------------
// Public intrinsics
// ---------------------------------------------------------------------------

/// `glob.glob(pathname, *, root_dir=None, dir_fd=None, recursive=False,
///            include_hidden=False)` -> list[str]
///
/// `root_dir_bits` is str | None. `recursive_bits` is bool.
#[unsafe(no_mangle)]
pub extern "C" fn molt_glob_glob(
    pathname_bits: u64,
    root_dir_bits: u64,
    recursive_bits: u64,
) -> u64 {
    crate::with_gil_entry_nopanic!(_py, {
        let allowed = has_capability(_py, "fs.read");
        audit_capability_decision("glob.glob", "fs.read", AuditArgs::None, allowed);
        if !allowed {
            return raise_exception::<u64>(_py, "PermissionError", "missing fs.read capability");
        }
        let pathname = match require_str(_py, pathname_bits, "pathname") {
            Ok(s) => s,
            Err(bits) => return bits,
        };
        let root_dir = if obj_from_bits(root_dir_bits).is_none() {
            None
        } else {
            match require_str(_py, root_dir_bits, "root_dir") {
                Ok(s) => Some(s),
                Err(bits) => return bits,
            }
        };
        let _recursive = is_truthy(_py, obj_from_bits(recursive_bits));

        let pattern = match &root_dir {
            Some(rd) => format!("{rd}/{pathname}"),
            None => pathname,
        };

        let mut results = Vec::new();
        match glob::glob(&pattern) {
            Ok(paths) => {
                for p in paths.flatten() {
                    results.push(p.to_string_lossy().into_owned());
                }
            }
            Err(err) => {
                return raise_exception::<u64>(
                    _py,
                    "ValueError",
                    &format!("invalid glob pattern: {err}"),
                );
            }
        }
        list_of_strings(_py, &results)
    })
}

/// `glob.iglob(...)` - same as glob but returns a list (in Molt, generators
/// compile to eager lists; this is semantically equivalent).
#[unsafe(no_mangle)]
pub extern "C" fn molt_glob_iglob(
    pathname_bits: u64,
    root_dir_bits: u64,
    recursive_bits: u64,
) -> u64 {
    // In AOT context, iglob == glob (no lazy iteration).
    molt_glob_glob(pathname_bits, root_dir_bits, recursive_bits)
}
