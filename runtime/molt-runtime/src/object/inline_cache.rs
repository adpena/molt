//! Inline cache (IC) data structures for attribute access.
//!
//! An IC caches the result of an attribute lookup (class identity → slot
//! offset) so that subsequent accesses to the same class can skip the
//! hash-table lookup.
//!
//! # Correctness guarantee
//! ICs are **best-effort**: a stale IC produces a cache *miss*, never a wrong
//! result.  All atomic operations therefore use `Ordering::Relaxed`.  The GIL
//! provides the happens-before relationship required for correctness; if the GIL
//! is ever removed these should be upgraded to `Acquire`/`Release`.

use std::sync::OnceLock;
use std::sync::atomic::{AtomicU32, AtomicU64, Ordering};

// ---------------------------------------------------------------------------
// InlineCache — 24 bytes, class-keyed to avoid TYPE_ID_OBJECT aliasing
// ---------------------------------------------------------------------------

/// A single inline-cache entry.
///
/// Layout (24 bytes):
/// ```text
/// [cached_class_bits: u64][cached_offset: u32][padding][cached_version: u64]
/// ```
pub struct InlineCache {
    /// NaN-boxed class object identity of the last successfully cached type,
    /// or `0` if empty.
    cached_class_bits: AtomicU64,
    /// Slot offset into the object's attribute storage.
    cached_offset: AtomicU32,
    /// Class layout version at the time this entry was written.
    cached_version: AtomicU64,
}

impl InlineCache {
    /// Create an empty (all-zero) cache entry.
    pub const fn new() -> Self {
        Self {
            cached_class_bits: AtomicU64::new(0),
            cached_offset: AtomicU32::new(0),
            cached_version: AtomicU64::new(0),
        }
    }

    /// Probe the cache.
    ///
    /// Returns `Some(offset)` when:
    /// - `class_bits` is non-zero,
    /// - `class_bits` matches the cached class object, **and**
    /// - `class_version` matches the cached layout version (entry is not
    ///   stale).
    ///
    /// Returns `None` on any miss.
    #[inline(always)]
    pub fn probe(&self, class_bits: u64, class_version: u64) -> Option<u32> {
        if class_bits == 0 {
            return None;
        }
        // Read version first so that if a concurrent writer is halfway through
        // an `update`, a version mismatch will cause a miss rather than us
        // seeing a partially-written offset.
        let cached_ver = self.cached_version.load(Ordering::Relaxed);
        let cached_class_bits = self.cached_class_bits.load(Ordering::Relaxed);

        if cached_class_bits == class_bits && cached_ver == class_version {
            Some(self.cached_offset.load(Ordering::Relaxed))
        } else {
            None
        }
    }

    /// Populate the cache after a miss.
    ///
    /// Writes `class_bits`, `offset`, and `class_version`. Callers must supply
    /// the current version of the same class object represented by
    /// `class_bits`.
    #[inline(always)]
    pub fn update(&self, class_bits: u64, offset: u32, class_version: u64) {
        self.cached_class_bits.store(class_bits, Ordering::Relaxed);
        self.cached_offset.store(offset, Ordering::Relaxed);
        self.cached_version.store(class_version, Ordering::Relaxed);
    }

    /// Invalidate this entry by clearing `cached_class_bits`.
    ///
    /// Any subsequent `probe` will return `None`.
    #[inline(always)]
    pub fn invalidate(&self) {
        self.cached_class_bits.store(0, Ordering::Relaxed);
    }
}

impl Default for InlineCache {
    fn default() -> Self {
        Self::new()
    }
}

// ---------------------------------------------------------------------------
// InlineCacheTable — global table of 4096 IC slots
// ---------------------------------------------------------------------------

/// The maximum number of IC slots in the global table.
pub const IC_TABLE_CAPACITY: usize = 4096;

/// A flat table of [`InlineCache`] entries indexed by a compile-time constant.
pub struct InlineCacheTable {
    entries: Vec<InlineCache>,
}

impl InlineCacheTable {
    fn new() -> Self {
        let mut entries = Vec::with_capacity(IC_TABLE_CAPACITY);
        for _ in 0..IC_TABLE_CAPACITY {
            entries.push(InlineCache::new());
        }
        Self { entries }
    }

    /// Return a reference to the IC entry at `index`.
    ///
    /// # Panics (debug) / UB (release)
    /// Get the inline cache entry at `index`.
    /// Bounds-checked in all builds — the cost is one comparison per attribute
    /// access site, which the branch predictor eliminates (always-taken).
    /// Returns a no-op empty cache for out-of-bounds indices rather than panicking,
    /// so a corrupt IC index degrades to a cache miss, not a crash.
    #[inline(always)]
    pub fn get(&self, index: usize) -> &InlineCache {
        static EMPTY: InlineCache = InlineCache::new();
        if index < self.entries.len() {
            &self.entries[index]
        } else {
            // Out of bounds: return empty cache (always misses, never UB)
            &EMPTY
        }
    }
}

// ---------------------------------------------------------------------------
// Global singleton
// ---------------------------------------------------------------------------

static GLOBAL_IC_TABLE: OnceLock<InlineCacheTable> = OnceLock::new();

/// Return the process-wide [`InlineCacheTable`].
///
/// The table is initialised lazily on first call.
pub fn global_ic_table() -> &'static InlineCacheTable {
    GLOBAL_IC_TABLE.get_or_init(InlineCacheTable::new)
}

// ---------------------------------------------------------------------------
// Unit tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    const CLASS_A: u64 = 0x7ff8_0000_0000_0042;
    const CLASS_B: u64 = 0x7ff8_0000_0000_0099;

    /// A fresh IC must miss for any non-zero class identity.
    #[test]
    fn probe_miss_on_empty_cache() {
        let ic = InlineCache::new();
        assert_eq!(ic.probe(CLASS_A, 1), None);
    }

    /// After `update`, a `probe` with the same class identity and version must
    /// hit.
    #[test]
    fn update_then_probe_hit() {
        let ic = InlineCache::new();
        ic.update(CLASS_A, 7, 5);
        assert_eq!(ic.probe(CLASS_A, 5), Some(7));
    }

    /// A probe must miss when the version has changed (stale entry).
    #[test]
    fn probe_miss_after_version_change() {
        let ic = InlineCache::new();
        ic.update(CLASS_A, 7, 5);
        // Version advanced — entry is now stale.
        assert_eq!(ic.probe(CLASS_A, 6), None);
    }

    /// A probe must miss when the class identity differs, even when two
    /// instances share the same low-level object representation.
    #[test]
    fn probe_miss_after_class_identity_change() {
        let ic = InlineCache::new();
        ic.update(CLASS_A, 7, 5);
        assert_eq!(ic.probe(CLASS_B, 5), None);
    }

    /// After `invalidate`, any probe must miss.
    #[test]
    fn invalidate_clears_cache() {
        let ic = InlineCache::new();
        ic.update(CLASS_A, 7, 5);
        ic.invalidate();
        assert_eq!(ic.probe(CLASS_A, 5), None);
    }

    /// `probe` with class_bits == 0 must always miss (0 is the sentinel for
    /// "empty").
    #[test]
    fn probe_zero_class_bits_always_misses() {
        let ic = InlineCache::new();
        // Manually poke a non-zero version so we know the check is on class bits.
        ic.update(CLASS_A, 3, 1);
        assert_eq!(ic.probe(0, 1), None);
    }

    /// Smoke-test the global table: all entries start empty.
    #[test]
    fn global_table_entries_start_empty() {
        let table = global_ic_table();
        // Spot-check a few indices.
        assert_eq!(table.get(0).probe(CLASS_A, 1), None);
        assert_eq!(table.get(100).probe(CLASS_A, 1), None);
        assert_eq!(table.get(IC_TABLE_CAPACITY - 1).probe(CLASS_A, 1), None);
    }

    /// Round-trip through the global table.
    #[test]
    fn global_table_update_and_probe() {
        let table = global_ic_table();
        let ic = table.get(256);
        ic.update(CLASS_A, 12, 3);
        assert_eq!(ic.probe(CLASS_A, 3), Some(12));
        ic.invalidate();
        assert_eq!(ic.probe(CLASS_A, 3), None);
    }
}
