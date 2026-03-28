//! Reference count verification mode.
//!
//! When the `refcount_verify` feature is enabled, every MoltObject increment
//! and decrement is tracked. On drop, any object with a non-zero reference
//! count triggers a panic with diagnostic information.
//!
//! This catches:
//! - Leaked references (refcount never reaches 0)
//! - Double-free (refcount goes below 0)
//! - Use-after-free (access after refcount reached 0)
//!
//! Inspired by Monty's ref-count-panic feature.

#[cfg(feature = "refcount_verify")]
use std::collections::HashMap;
#[cfg(feature = "refcount_verify")]
use std::sync::Mutex;

#[cfg(feature = "refcount_verify")]
static REFCOUNT_MAP: Mutex<Option<HashMap<u64, RefCountEntry>>> = Mutex::new(None);

#[cfg(feature = "refcount_verify")]
#[derive(Debug)]
pub struct RefCountEntry {
    pub object_bits: u64,
    pub refcount: i64,
    pub creation_location: &'static str,
}

/// Initialize the global refcount tracking map.
///
/// Must be called once at the start of a test or verification session.
/// When the `refcount_verify` feature is disabled, this is a no-op.
#[cfg(feature = "refcount_verify")]
pub fn init_refcount_tracking() {
    let mut map = REFCOUNT_MAP.lock().unwrap_or_else(|e| e.into_inner());
    *map = Some(HashMap::new());
}

/// Record an increment of the reference count for the object identified by `bits`.
///
/// If this is the first time `bits` is seen, a new entry is created with the
/// given `location` for diagnostics.
#[cfg(feature = "refcount_verify")]
pub fn track_inc_ref(bits: u64, location: &'static str) {
    let mut map_guard = REFCOUNT_MAP.lock().unwrap_or_else(|e| e.into_inner());
    if let Some(ref mut map) = *map_guard {
        let entry = map.entry(bits).or_insert(RefCountEntry {
            object_bits: bits,
            refcount: 0,
            creation_location: location,
        });
        entry.refcount += 1;
    }
}

/// Record a decrement of the reference count for the object identified by `bits`.
///
/// Panics immediately if the refcount drops below zero (double-free detection).
#[cfg(feature = "refcount_verify")]
pub fn track_dec_ref(bits: u64) {
    let mut map_guard = REFCOUNT_MAP.lock().unwrap_or_else(|e| e.into_inner());
    if let Some(ref mut map) = *map_guard {
        if let Some(entry) = map.get_mut(&bits) {
            entry.refcount -= 1;
            if entry.refcount < 0 {
                panic!(
                    "REFCOUNT UNDERFLOW: object {:#x} refcount went to {} (created at {})",
                    bits, entry.refcount, entry.creation_location
                );
            }
        }
    }
}

/// Verify that all tracked objects have been freed (refcount == 0).
///
/// Panics with a detailed report listing every leaked object, its remaining
/// refcount, and the location where it was first tracked.
#[cfg(feature = "refcount_verify")]
pub fn verify_all_freed() {
    let map_guard = REFCOUNT_MAP.lock().unwrap_or_else(|e| e.into_inner());
    if let Some(ref map) = *map_guard {
        let leaks: Vec<_> = map.values().filter(|e| e.refcount > 0).collect();
        if !leaks.is_empty() {
            let mut msg = format!("REFCOUNT LEAKS DETECTED ({} objects):\n", leaks.len());
            for leak in &leaks {
                msg.push_str(&format!(
                    "  object {:#x}: refcount={}, created at {}\n",
                    leak.object_bits, leak.refcount, leak.creation_location
                ));
            }
            panic!("{msg}");
        }
    }
}

/// Return the current tracked refcount for an object, or `None` if untracked.
///
/// Useful in tests to assert expected reference counts at specific points.
#[cfg(feature = "refcount_verify")]
pub fn get_refcount(bits: u64) -> Option<i64> {
    let map_guard = REFCOUNT_MAP.lock().unwrap_or_else(|e| e.into_inner());
    if let Some(ref map) = *map_guard {
        map.get(&bits).map(|e| e.refcount)
    } else {
        None
    }
}

/// Reset tracking state, clearing all entries.
///
/// Useful between test cases to avoid cross-test interference.
#[cfg(feature = "refcount_verify")]
pub fn reset_refcount_tracking() {
    let mut map = REFCOUNT_MAP.lock().unwrap_or_else(|e| e.into_inner());
    if let Some(ref mut m) = *map {
        m.clear();
    }
}

// ---------------------------------------------------------------------------
// No-ops when feature is disabled — zero-cost, always inlined away.
// ---------------------------------------------------------------------------

#[cfg(not(feature = "refcount_verify"))]
#[inline(always)]
pub fn init_refcount_tracking() {}

#[cfg(not(feature = "refcount_verify"))]
#[inline(always)]
pub fn track_inc_ref(_bits: u64, _location: &'static str) {}

#[cfg(not(feature = "refcount_verify"))]
#[inline(always)]
pub fn track_dec_ref(_bits: u64) {}

#[cfg(not(feature = "refcount_verify"))]
#[inline(always)]
pub fn verify_all_freed() {}

#[cfg(not(feature = "refcount_verify"))]
#[inline(always)]
pub fn get_refcount(_bits: u64) -> Option<i64> {
    None
}

#[cfg(not(feature = "refcount_verify"))]
#[inline(always)]
pub fn reset_refcount_tracking() {}

// ---------------------------------------------------------------------------
// Tests (run with: cargo test -p molt-runtime --features refcount_verify)
// ---------------------------------------------------------------------------

#[cfg(test)]
#[cfg(feature = "refcount_verify")]
mod tests {
    use super::*;

    // Tests mutate global state, so serialize them.
    fn with_fresh_tracking<F: FnOnce()>(f: F) {
        let _lock = crate::TEST_MUTEX.lock().unwrap_or_else(|e| e.into_inner());
        init_refcount_tracking();
        f();
        reset_refcount_tracking();
    }

    #[test]
    fn balanced_refs_pass_verification() {
        with_fresh_tracking(|| {
            track_inc_ref(0xA, "test::balanced");
            track_inc_ref(0xA, "test::balanced");
            track_dec_ref(0xA);
            track_dec_ref(0xA);
            verify_all_freed(); // should not panic
        });
    }

    #[test]
    #[should_panic(expected = "REFCOUNT UNDERFLOW")]
    fn double_free_panics() {
        with_fresh_tracking(|| {
            track_inc_ref(0xB, "test::double_free");
            track_dec_ref(0xB);
            track_dec_ref(0xB); // underflow
        });
    }

    #[test]
    #[should_panic(expected = "REFCOUNT LEAKS DETECTED")]
    fn leak_detected_on_verify() {
        with_fresh_tracking(|| {
            track_inc_ref(0xC, "test::leak");
            track_inc_ref(0xD, "test::leak");
            track_dec_ref(0xC);
            // 0xD is never decremented -> leak
            verify_all_freed();
        });
    }

    #[test]
    fn get_refcount_tracks_correctly() {
        with_fresh_tracking(|| {
            assert_eq!(get_refcount(0xE), None);
            track_inc_ref(0xE, "test::get_rc");
            assert_eq!(get_refcount(0xE), Some(1));
            track_inc_ref(0xE, "test::get_rc");
            assert_eq!(get_refcount(0xE), Some(2));
            track_dec_ref(0xE);
            assert_eq!(get_refcount(0xE), Some(1));
        });
    }

    #[test]
    fn reset_clears_all_entries() {
        with_fresh_tracking(|| {
            track_inc_ref(0xF, "test::reset");
            reset_refcount_tracking();
            assert_eq!(get_refcount(0xF), None);
        });
    }
}
