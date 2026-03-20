#![allow(dead_code, unused_imports)]
// === FILE: runtime/molt-runtime/src/builtins/fnmatch.rs ===
//
// fnmatch intrinsics: Unix filename pattern matching.
//
// fnmatch_filter and fnmatch_translate already live in functions.rs / io.rs.
// This file adds: fnmatch() and fnmatchcase().

use crate::*;

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

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

/// Match a filename against a pattern (case-sensitive).
#[cfg(feature = "stdlib_fs_extra")]
fn fnmatch_impl(name: &str, pattern: &str) -> bool {
    match glob::Pattern::new(pattern) {
        Ok(pat) => pat.matches(name),
        Err(_) => false,
    }
}

#[cfg(not(feature = "stdlib_fs_extra"))]
fn fnmatch_impl(_name: &str, _pattern: &str) -> bool {
    false
}

/// Match case-insensitive (normalize both to lowercase).
fn fnmatch_case_insensitive(name: &str, pattern: &str) -> bool {
    let name_lower = name.to_lowercase();
    let pat_lower = pattern.to_lowercase();
    fnmatch_impl(&name_lower, &pat_lower)
}

// ---------------------------------------------------------------------------
// Public intrinsics
// ---------------------------------------------------------------------------

/// `fnmatch.fnmatch(filename, pattern)` -> bool
///
/// Case-insensitive on platforms where the filesystem is case-insensitive
/// (Windows, macOS default). For simplicity, we follow CPython which normalizes
/// on Windows but not on Linux.
#[unsafe(no_mangle)]
pub extern "C" fn molt_fnmatch_fnmatch(filename_bits: u64, pattern_bits: u64) -> u64 {
    crate::with_gil_entry!(_py, {
        let filename = match require_str(_py, filename_bits, "filename") {
            Ok(s) => s,
            Err(bits) => return bits,
        };
        let pattern = match require_str(_py, pattern_bits, "pattern") {
            Ok(s) => s,
            Err(bits) => return bits,
        };
        // CPython: case-insensitive on Windows/macOS, case-sensitive on Linux
        #[cfg(any(target_os = "windows", target_os = "macos"))]
        let result = fnmatch_case_insensitive(&filename, &pattern);
        #[cfg(not(any(target_os = "windows", target_os = "macos")))]
        let result = fnmatch_impl(&filename, &pattern);
        MoltObject::from_bool(result).bits()
    })
}

/// `fnmatch.fnmatchcase(filename, pattern)` -> bool
///
/// Always case-sensitive matching.
#[unsafe(no_mangle)]
pub extern "C" fn molt_fnmatch_fnmatchcase(filename_bits: u64, pattern_bits: u64) -> u64 {
    crate::with_gil_entry!(_py, {
        let filename = match require_str(_py, filename_bits, "filename") {
            Ok(s) => s,
            Err(bits) => return bits,
        };
        let pattern = match require_str(_py, pattern_bits, "pattern") {
            Ok(s) => s,
            Err(bits) => return bits,
        };
        MoltObject::from_bool(fnmatch_impl(&filename, &pattern)).bits()
    })
}
