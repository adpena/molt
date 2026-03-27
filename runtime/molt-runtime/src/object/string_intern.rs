//! String interning pool for Molt runtime.
//!
//! # Overview
//!
//! Interned strings are deduplicated: two strings with the same content share a
//! single allocation. This means comparison of two interned strings can use
//! **pointer equality** (one instruction) rather than a full byte-by-byte scan.
//!
//! # Pointer equality semantics
//!
//! A `&str` is a fat pointer: `(data_ptr: *const u8, len: usize)`.
//! [`std::ptr::eq`] on `&str` compares **both** fields, so two `&str` slices
//! are considered equal by `ptr::eq` only when they point to the exact same
//! memory region of the exact same length. Because every interned string is
//! unique in the pool (deduplicated by content), any two interned strings with
//! the same content will resolve to the identical `&'static str` fat pointer.
//!
//! **Important:** `ptr::eq` is only valid for *interned-vs-interned* comparisons.
//! For mixed comparisons (one interned, one heap-allocated), always fall back to
//! byte equality (`==`).
//!
//! # Lifetime
//!
//! Interned strings are leaked via [`Box::leak`]. They live for the duration of
//! the process. This is intentional: interned strings are meant to be cheap,
//! long-lived identifiers (attribute names, keyword names, module names). The
//! pool is never freed.

use std::collections::HashSet;
use std::sync::{Mutex, OnceLock};

// ---------------------------------------------------------------------------
// Global pool
// ---------------------------------------------------------------------------

/// Returns a reference to the global intern pool.
///
/// The pool is a `HashSet<&'static str>`. We store raw `&'static str` pointers
/// (leaked via `Box::leak`) so that every entry is already stable in memory.
fn intern_pool() -> &'static Mutex<HashSet<&'static str>> {
    static POOL: OnceLock<Mutex<HashSet<&'static str>>> = OnceLock::new();
    POOL.get_or_init(|| Mutex::new(HashSet::new()))
}

// ---------------------------------------------------------------------------
// Public API
// ---------------------------------------------------------------------------

/// Intern `s`, returning a `&'static str` with the same contents.
///
/// If an identical string is already in the pool, the existing pointer is
/// returned (no allocation). Otherwise, `s` is copied onto the heap, leaked,
/// and stored in the pool.
///
/// # Performance
///
/// - Hash + lookup: O(len) — unavoidable.
/// - On a hit (common case for identifiers): zero allocations.
/// - On a miss: one `Box<str>` allocation + leak.
#[inline]
pub fn intern(s: &str) -> &'static str {
    let mut pool = intern_pool().lock().unwrap();
    if let Some(&existing) = pool.get(s) {
        return existing;
    }
    // Allocate a stable copy and leak it so the lifetime is `'static`.
    let leaked: &'static str = Box::leak(s.to_owned().into_boxed_str());
    pool.insert(leaked);
    leaked
}

/// Check whether `s` is already interned, without inserting it.
///
/// Returns `Some(&'static str)` if found, `None` otherwise.
#[inline]
pub fn get_interned(s: &str) -> Option<&'static str> {
    let pool = intern_pool().lock().unwrap();
    pool.get(s).copied()
}

/// Returns `true` when `s` looks like a Python identifier: `[a-zA-Z_][a-zA-Z0-9_]*`.
///
/// This runs on every string creation so it must be fast. We use byte-level
/// checks (all valid Python identifiers are ASCII in the common case handled
/// here) and avoid any heap allocation or regex machinery.
///
/// Non-ASCII strings are not considered identifier-like by this function;
/// they will not be auto-interned.
#[inline]
pub fn is_identifier_like(s: &str) -> bool {
    if s.is_empty() {
        return false;
    }
    let bytes = s.as_bytes();
    // First byte: [a-zA-Z_]
    let first = bytes[0];
    if !first.is_ascii_alphabetic() && first != b'_' {
        return false;
    }
    // Remaining bytes: [a-zA-Z0-9_]
    for &b in &bytes[1..] {
        if !b.is_ascii_alphanumeric() && b != b'_' {
            return false;
        }
    }
    true
}

/// Diagnostic: returns the number of strings currently in the intern pool.
#[inline]
pub fn intern_pool_size() -> usize {
    intern_pool().lock().unwrap().len()
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn intern_deduplicates() {
        let a = intern("__init__");
        let b = intern("__init__");
        // Same pointer — pointer equality holds.
        assert!(
            std::ptr::eq(a, b),
            "intern must return the same pointer for equal strings"
        );
    }

    #[test]
    fn intern_different_strings() {
        let a = intern("foo_dedup_test");
        let b = intern("bar_dedup_test");
        assert!(!std::ptr::eq(a, b), "distinct strings must not alias");
    }

    #[test]
    fn get_interned_miss() {
        // A string that is (very likely) not in the pool yet.
        let unique = "xyzzy_not_yet_interned_9182736450";
        assert!(get_interned(unique).is_none());
    }

    #[test]
    fn get_interned_hit() {
        let s = "get_interned_hit_sentinel";
        intern(s);
        assert!(get_interned(s).is_some());
    }

    #[test]
    fn pool_size_increases() {
        let before = intern_pool_size();
        // Use a unique string to guarantee a miss.
        let unique = format!("pool_size_test_{}", before);
        intern(&unique);
        assert!(intern_pool_size() > before);
    }

    #[test]
    fn is_identifier_like_valid() {
        assert!(is_identifier_like("x"));
        assert!(is_identifier_like("_"));
        assert!(is_identifier_like("__init__"));
        assert!(is_identifier_like("CamelCase"));
        assert!(is_identifier_like("snake_case_123"));
        assert!(is_identifier_like("A1B2C3"));
        assert!(is_identifier_like("_private"));
    }

    #[test]
    fn is_identifier_like_invalid() {
        assert!(!is_identifier_like(""));
        assert!(!is_identifier_like("1abc"));
        assert!(!is_identifier_like("hello world"));
        assert!(!is_identifier_like("foo-bar"));
        assert!(!is_identifier_like("3.14"));
        assert!(!is_identifier_like("a.b"));
        // Non-ASCII: not handled as identifier-like.
        assert!(!is_identifier_like("café"));
    }
}
