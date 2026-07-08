#![allow(dead_code, unused_imports)]
// === FILE: runtime/molt-runtime/src/builtins/fnmatch.rs ===
//
// fnmatch intrinsics: Unix filename pattern matching.
//
// fnmatch_filter and fnmatch_translate already live in functions.rs / io.rs.
// This file adds: fnmatch() and fnmatchcase().

use crate::*;
use molt_stdlib_text::fnmatch::{fnmatch_match_impl, fnmatch_normcase_text};

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
    crate::with_gil_entry_nopanic!(_py, {
        let filename = match require_str(_py, filename_bits, "filename") {
            Ok(s) => s,
            Err(bits) => return bits,
        };
        let pattern = match require_str(_py, pattern_bits, "pattern") {
            Ok(s) => s,
            Err(bits) => return bits,
        };
        let result = fnmatch_match_impl(
            &fnmatch_normcase_text(&filename),
            &fnmatch_normcase_text(&pattern),
        );
        MoltObject::from_bool(result).bits()
    })
}

/// `fnmatch.fnmatchcase(filename, pattern)` -> bool
///
/// Always case-sensitive matching.
#[unsafe(no_mangle)]
pub extern "C" fn molt_fnmatch_fnmatchcase(filename_bits: u64, pattern_bits: u64) -> u64 {
    crate::with_gil_entry_nopanic!(_py, {
        let filename = match require_str(_py, filename_bits, "filename") {
            Ok(s) => s,
            Err(bits) => return bits,
        };
        let pattern = match require_str(_py, pattern_bits, "pattern") {
            Ok(s) => s,
            Err(bits) => return bits,
        };
        MoltObject::from_bool(fnmatch_match_impl(&filename, &pattern)).bits()
    })
}
