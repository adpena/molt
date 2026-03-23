//! Compact dictionary implementation for Molt runtime.
//!
//! Two-phase design:
//!  - Small dict (≤8 entries): inline arrays, linear scan. Fits in 2 cache lines.
//!  - Large dict (>8 entries): open-addressing hash table with insertion-order key/value vecs.
//!
//! All keys and values are NaN-boxed `u64`. Key equality is `u64` equality (pointer equality
//! for interned strings; callers must intern before insertion if they need semantic equality).

const SMALL_CAP: usize = 8;
const EMPTY_SLOT: u32 = 0xFFFF_FFFF;
const DELETED_SLOT: u32 = 0xFFFF_FFFE;
const INITIAL_LARGE_CAP: usize = 16;
const LOAD_FACTOR_NUM: usize = 3;
const LOAD_FACTOR_DEN: usize = 4; // threshold = capacity * 3/4

// ---------------------------------------------------------------------------
// Small-dict inline storage
// ---------------------------------------------------------------------------

#[derive(Clone, Debug)]
struct SmallStorage {
    keys: [u64; SMALL_CAP],
    values: [u64; SMALL_CAP],
    hashes: [u64; SMALL_CAP],
}

impl SmallStorage {
    const fn new() -> Self {
        Self {
            keys: [0u64; SMALL_CAP],
            values: [0u64; SMALL_CAP],
            hashes: [0u64; SMALL_CAP],
        }
    }
}

// ---------------------------------------------------------------------------
// Large-dict heap storage
// ---------------------------------------------------------------------------

#[derive(Clone, Debug)]
struct LargeStorage {
    /// hash → slot index in keys/values/hashes.  EMPTY_SLOT or DELETED_SLOT are sentinels.
    indices: Vec<u32>,
    keys: Vec<u64>,
    values: Vec<u64>,
    hashes: Vec<u64>,
}

impl LargeStorage {
    fn with_capacity(cap: usize) -> Self {
        debug_assert!(cap.is_power_of_two());
        Self {
            indices: vec![EMPTY_SLOT; cap],
            keys: Vec::with_capacity(cap),
            values: Vec::with_capacity(cap),
            hashes: Vec::with_capacity(cap),
        }
    }

    #[inline]
    fn index_capacity(&self) -> usize {
        self.indices.len()
    }

    #[inline]
    fn slot_for_hash(&self, hash: u64) -> usize {
        (hash as usize) & (self.index_capacity() - 1)
    }

    /// Find the index-table position for the given key.
    /// Returns `Ok(pos)` if found (indices[pos] is the data slot),
    /// `Err(pos)` if not found (pos is the first usable empty/deleted slot).
    fn probe(&self, key: u64, hash: u64) -> Result<usize, usize> {
        let cap = self.index_capacity();
        let mut pos = self.slot_for_hash(hash);
        let mut first_deleted: Option<usize> = None;

        loop {
            let entry = self.indices[pos];
            if entry == EMPTY_SLOT {
                // Key not present; return best insertion point.
                return Err(first_deleted.unwrap_or(pos));
            } else if entry == DELETED_SLOT {
                if first_deleted.is_none() {
                    first_deleted = Some(pos);
                }
            } else {
                let slot = entry as usize;
                if self.hashes[slot] == hash && self.keys[slot] == key {
                    return Ok(pos);
                }
            }
            pos = (pos + 1) & (cap - 1);
        }
    }

    /// Insert a new key/value/hash triple (key must not already exist).
    fn insert_new(&mut self, key: u64, value: u64, hash: u64) {
        let slot = self.keys.len() as u32;
        self.keys.push(key);
        self.values.push(value);
        self.hashes.push(hash);

        let ins = match self.probe(key, hash) {
            Ok(_) => unreachable!("insert_new called for existing key"),
            Err(pos) => pos,
        };
        self.indices[ins] = slot;
    }

    /// Rebuild the indices table (used after resize or compaction after deletes).
    fn rebuild_indices(&mut self) {
        let cap = self.index_capacity();
        self.indices.fill(EMPTY_SLOT);
        for slot in 0..self.keys.len() {
            let hash = self.hashes[slot];
            let mut pos = (hash as usize) & (cap - 1);
            loop {
                let entry = self.indices[pos];
                if entry == EMPTY_SLOT || entry == DELETED_SLOT {
                    self.indices[pos] = slot as u32;
                    break;
                }
                pos = (pos + 1) & (cap - 1);
            }
        }
    }
}

// ---------------------------------------------------------------------------
// Storage enum
// ---------------------------------------------------------------------------

#[derive(Clone, Debug)]
enum Storage {
    Small(SmallStorage),
    Large(LargeStorage),
}

// ---------------------------------------------------------------------------
// CompactDict
// ---------------------------------------------------------------------------

/// A compact, cache-friendly dictionary for NaN-boxed keys and values.
///
/// Optimised for the common case of small Python dicts (≤8 entries). Graduated to
/// an open-addressing hash table when the 9th entry is inserted.
#[derive(Clone, Debug)]
pub struct CompactDict {
    storage: Storage,
    len: usize,
    /// Monotonically increasing mutation counter. Used by inline caches to detect
    /// stale entries without a full comparison.
    version: u64,
}

impl CompactDict {
    // ------------------------------------------------------------------
    // Construction
    // ------------------------------------------------------------------

    pub fn new() -> Self {
        Self {
            storage: Storage::Small(SmallStorage::new()),
            len: 0,
            version: 0,
        }
    }

    // ------------------------------------------------------------------
    // Accessors
    // ------------------------------------------------------------------

    #[inline]
    pub fn len(&self) -> usize {
        self.len
    }

    #[inline]
    pub fn is_empty(&self) -> bool {
        self.len == 0
    }

    /// Monotonic mutation counter. Incremented on every successful `set` or `delete`.
    #[inline]
    pub fn version(&self) -> u64 {
        self.version
    }

    // ------------------------------------------------------------------
    // Lookup
    // ------------------------------------------------------------------

    pub fn get(&self, key: u64, key_hash: u64) -> Option<u64> {
        match &self.storage {
            Storage::Small(s) => {
                for i in 0..self.len {
                    if s.hashes[i] == key_hash && s.keys[i] == key {
                        return Some(s.values[i]);
                    }
                }
                None
            }
            Storage::Large(s) => {
                s.probe(key, key_hash)
                    .ok()
                    .map(|pos| s.values[s.indices[pos] as usize])
            }
        }
    }

    pub fn contains_key(&self, key: u64, key_hash: u64) -> bool {
        self.get(key, key_hash).is_some()
    }

    // ------------------------------------------------------------------
    // Mutation
    // ------------------------------------------------------------------

    pub fn set(&mut self, key: u64, value: u64, key_hash: u64) {
        match &mut self.storage {
            Storage::Small(s) => {
                // Check for existing key.
                for i in 0..self.len {
                    if s.hashes[i] == key_hash && s.keys[i] == key {
                        s.values[i] = value;
                        self.version += 1;
                        return;
                    }
                }
                // Insert new.
                if self.len < SMALL_CAP {
                    let i = self.len;
                    s.keys[i] = key;
                    s.values[i] = value;
                    s.hashes[i] = key_hash;
                    self.len += 1;
                    self.version += 1;
                } else {
                    // Graduate to large storage.
                    self.graduate();
                    // Now insert into the large storage (no resize needed: initial cap is 16,
                    // threshold is 12, and we have exactly SMALL_CAP+1 = 9 entries).
                    if let Storage::Large(ls) = &mut self.storage {
                        ls.insert_new(key, value, key_hash);
                    }
                    self.len += 1;
                    self.version += 1;
                }
            }
            Storage::Large(s) => {
                match s.probe(key, key_hash) {
                    Ok(pos) => {
                        // Overwrite existing.
                        let slot = s.indices[pos] as usize;
                        s.values[slot] = value;
                        self.version += 1;
                    }
                    Err(_) => {
                        // Need to insert new. Check resize first.
                        // We must re-probe after resize so take ownership via clone trick.
                        if self.len + 1 > self.large_capacity() * LOAD_FACTOR_NUM / LOAD_FACTOR_DEN
                        {
                            self.resize_large();
                        }
                        if let Storage::Large(ls) = &mut self.storage {
                            ls.insert_new(key, value, key_hash);
                        }
                        self.len += 1;
                        self.version += 1;
                    }
                }
            }
        }
    }

    pub fn delete(&mut self, key: u64, key_hash: u64) -> Option<u64> {
        match &mut self.storage {
            Storage::Small(s) => {
                for i in 0..self.len {
                    if s.hashes[i] == key_hash && s.keys[i] == key {
                        let old_val = s.values[i];
                        // Shift remaining entries down.
                        let last = self.len - 1;
                        for j in i..last {
                            s.keys[j] = s.keys[j + 1];
                            s.values[j] = s.values[j + 1];
                            s.hashes[j] = s.hashes[j + 1];
                        }
                        self.len -= 1;
                        self.version += 1;
                        return Some(old_val);
                    }
                }
                None
            }
            Storage::Large(s) => {
                match s.probe(key, key_hash) {
                    Ok(pos) => {
                        let slot = s.indices[pos] as usize;
                        let old_val = s.values[slot];
                        // Mark index slot as deleted (tombstone).
                        s.indices[pos] = DELETED_SLOT;
                        // Logically remove from data arrays by zeroing (optional, keeps
                        // iteration correct via len tracking).
                        // We compact if too many tombstones accumulate, but for simplicity
                        // we leave data in place and just track len.
                        // Mark the data slot as deleted by setting a sentinel key.
                        // We use a compaction strategy: mark slot as skipped.
                        // Simplest correct approach: we compact by rebuilding.
                        // For now: zero out the slot values so iterators skip them.
                        // We track "active" slots by checking against a special sentinel.
                        // Instead: use a simpler "swap with last alive" approach.
                        let last_slot = s.keys.len() - 1;
                        if slot != last_slot {
                            // Swap the deleted slot with the last slot.
                            s.keys.swap(slot, last_slot);
                            s.values.swap(slot, last_slot);
                            s.hashes.swap(slot, last_slot);
                            // The moved entry's index in the indices table now points to
                            // `last_slot` but should point to `slot`. Fix it.
                            s.indices[pos] = DELETED_SLOT; // undo the swap marker
                            s.rebuild_indices_after_swap(slot, last_slot);
                        } else {
                            s.indices[pos] = DELETED_SLOT;
                        }
                        s.keys.pop();
                        s.values.pop();
                        s.hashes.pop();
                        // No need to rebuild again — rebuild_indices_after_swap already
                        // fixed the moved entry's index, and the popped entry's tombstone
                        // is harmless (will be cleaned on next resize).
                        self.len -= 1;
                        self.version += 1;
                        Some(old_val)
                    }
                    Err(_) => None,
                }
            }
        }
    }

    // ------------------------------------------------------------------
    // Iterators
    // ------------------------------------------------------------------

    pub fn keys(&self) -> impl Iterator<Item = u64> + '_ {
        CompactDictIter::new(self).map(|(k, _v)| k)
    }

    pub fn values(&self) -> impl Iterator<Item = u64> + '_ {
        CompactDictIter::new(self).map(|(_k, v)| v)
    }

    pub fn items(&self) -> impl Iterator<Item = (u64, u64)> + '_ {
        CompactDictIter::new(self)
    }

    // ------------------------------------------------------------------
    // Private helpers
    // ------------------------------------------------------------------

    /// Return the current index-table capacity for large storage (0 for small).
    fn large_capacity(&self) -> usize {
        match &self.storage {
            Storage::Small(_) => 0,
            Storage::Large(s) => s.index_capacity(),
        }
    }

    /// Graduate from small to large storage.
    fn graduate(&mut self) {
        let small = match &self.storage {
            Storage::Small(s) => s.clone(),
            Storage::Large(_) => return,
        };

        let mut large = LargeStorage::with_capacity(INITIAL_LARGE_CAP);
        for i in 0..self.len {
            large.insert_new(small.keys[i], small.values[i], small.hashes[i]);
        }
        self.storage = Storage::Large(large);
    }

    /// Double the indices table of the large storage and rebuild.
    fn resize_large(&mut self) {
        if let Storage::Large(s) = &mut self.storage {
            let new_cap = s.index_capacity() * 2;
            s.indices = vec![EMPTY_SLOT; new_cap];
            s.rebuild_indices();
        }
    }
}

// Helper method on LargeStorage for the swap-during-delete case.
impl LargeStorage {
    /// After swapping data slot `moved_to` ← `old_pos`, fix the index entry
    /// that still points to `old_pos` so it points to `moved_to` instead.
    /// O(1) amortized — probes the hash of the moved element to find its index slot.
    fn rebuild_indices_after_swap(&mut self, moved_to: usize, old_pos: usize) {
        let hash = self.hashes[moved_to]; // element now lives at moved_to
        let cap = self.index_capacity();
        let mut pos = (hash as usize) & (cap - 1);
        loop {
            let entry = self.indices[pos];
            if entry == old_pos as u32 {
                self.indices[pos] = moved_to as u32;
                return;
            }
            if entry == EMPTY_SLOT {
                // Shouldn't happen — the entry must exist. Fall back to full rebuild.
                self.rebuild_indices();
                return;
            }
            pos = (pos + 1) & (cap - 1);
        }
    }
}

// ---------------------------------------------------------------------------
// Iterator
// ---------------------------------------------------------------------------

struct CompactDictIter<'a> {
    dict: &'a CompactDict,
    pos: usize,
}

impl<'a> CompactDictIter<'a> {
    fn new(dict: &'a CompactDict) -> Self {
        Self { dict, pos: 0 }
    }
}

impl<'a> Iterator for CompactDictIter<'a> {
    type Item = (u64, u64);

    fn next(&mut self) -> Option<Self::Item> {
        match &self.dict.storage {
            Storage::Small(s) => {
                if self.pos < self.dict.len {
                    let i = self.pos;
                    self.pos += 1;
                    Some((s.keys[i], s.values[i]))
                } else {
                    None
                }
            }
            Storage::Large(s) => {
                if self.pos < s.keys.len() {
                    let i = self.pos;
                    self.pos += 1;
                    Some((s.keys[i], s.values[i]))
                } else {
                    None
                }
            }
        }
    }
}

impl Default for CompactDict {
    fn default() -> Self {
        Self::new()
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    // Helper: produce a simple hash for u64 keys in tests.
    fn h(key: u64) -> u64 {
        // FNV-1a inspired mix — good enough for unit tests.
        let mut x = key ^ 0xcbf29ce4_84222325;
        x ^= x >> 33;
        x = x.wrapping_mul(0xff51afd7_ed558ccd);
        x ^= x >> 33;
        x = x.wrapping_mul(0xc4ceb9fe_1a85ec53);
        x ^= x >> 33;
        x
    }

    #[test]
    fn test_new_creates_empty_dict() {
        let d = CompactDict::new();
        assert_eq!(d.len(), 0);
        assert!(d.is_empty());
    }

    #[test]
    fn test_set_get_round_trip() {
        let mut d = CompactDict::new();
        let k = 42u64;
        let v = 99u64;
        d.set(k, v, h(k));
        assert_eq!(d.get(k, h(k)), Some(v));
        assert_eq!(d.len(), 1);
    }

    #[test]
    fn test_set_overwrites_existing_key() {
        let mut d = CompactDict::new();
        let k = 1u64;
        d.set(k, 10, h(k));
        d.set(k, 20, h(k));
        assert_eq!(d.get(k, h(k)), Some(20));
        assert_eq!(d.len(), 1);
    }

    #[test]
    fn test_delete_returns_old_value() {
        let mut d = CompactDict::new();
        let k = 5u64;
        d.set(k, 55, h(k));
        let old = d.delete(k, h(k));
        assert_eq!(old, Some(55));
        assert_eq!(d.len(), 0);
        assert_eq!(d.get(k, h(k)), None);
    }

    #[test]
    fn test_delete_missing_key_returns_none() {
        let mut d = CompactDict::new();
        assert_eq!(d.delete(999, h(999)), None);
    }

    #[test]
    fn test_small_dict_no_indices_allocation() {
        let mut d = CompactDict::new();
        for i in 0..SMALL_CAP as u64 {
            d.set(i, i * 2, h(i));
        }
        assert!(matches!(d.storage, Storage::Small(_)));
        assert_eq!(d.len(), SMALL_CAP);
    }

    #[test]
    fn test_graduation_on_ninth_insert() {
        let mut d = CompactDict::new();
        for i in 0..SMALL_CAP as u64 {
            d.set(i, i, h(i));
        }
        assert!(matches!(d.storage, Storage::Small(_)));
        // Insert the 9th element.
        d.set(100u64, 999, h(100));
        assert!(matches!(d.storage, Storage::Large(_)));
        assert_eq!(d.len(), SMALL_CAP + 1);
        // All previously inserted keys still accessible.
        for i in 0..SMALL_CAP as u64 {
            assert_eq!(d.get(i, h(i)), Some(i), "key {} missing after graduation", i);
        }
        assert_eq!(d.get(100, h(100)), Some(999));
    }

    #[test]
    fn test_version_increments_on_set() {
        let mut d = CompactDict::new();
        let v0 = d.version();
        d.set(1, 1, h(1));
        assert_eq!(d.version(), v0 + 1);
        // Overwrite: version still increments.
        d.set(1, 2, h(1));
        assert_eq!(d.version(), v0 + 2);
    }

    #[test]
    fn test_version_increments_on_delete() {
        let mut d = CompactDict::new();
        d.set(1, 1, h(1));
        let v0 = d.version();
        d.delete(1, h(1));
        assert_eq!(d.version(), v0 + 1);
        // Delete miss: version does NOT increment.
        let v1 = d.version();
        d.delete(1, h(1));
        assert_eq!(d.version(), v1);
    }

    #[test]
    fn test_keys_iterator() {
        let mut d = CompactDict::new();
        for i in 0..4u64 {
            d.set(i, i * 10, h(i));
        }
        let mut keys: Vec<u64> = d.keys().collect();
        keys.sort_unstable();
        assert_eq!(keys, vec![0, 1, 2, 3]);
    }

    #[test]
    fn test_values_iterator() {
        let mut d = CompactDict::new();
        for i in 0..4u64 {
            d.set(i, i * 10, h(i));
        }
        let mut vals: Vec<u64> = d.values().collect();
        vals.sort_unstable();
        assert_eq!(vals, vec![0, 10, 20, 30]);
    }

    #[test]
    fn test_items_iterator() {
        let mut d = CompactDict::new();
        for i in 0..3u64 {
            d.set(i, i + 100, h(i));
        }
        let mut items: Vec<(u64, u64)> = d.items().collect();
        items.sort_unstable();
        assert_eq!(items, vec![(0, 100), (1, 101), (2, 102)]);
    }

    #[test]
    fn test_resize_on_load_factor_exceeded() {
        let mut d = CompactDict::new();
        // Insert enough to trigger a resize (INITIAL_LARGE_CAP=16, threshold=12).
        // 8 small + 5 large = 13 entries → should trigger resize.
        for i in 0..15u64 {
            d.set(i, i, h(i));
        }
        assert!(matches!(d.storage, Storage::Large(_)));
        assert_eq!(d.len(), 15);
        for i in 0..15u64 {
            assert_eq!(d.get(i, h(i)), Some(i), "missing key {}", i);
        }
    }

    #[test]
    fn test_contains_key() {
        let mut d = CompactDict::new();
        d.set(7, 77, h(7));
        assert!(d.contains_key(7, h(7)));
        assert!(!d.contains_key(8, h(8)));
    }

    #[test]
    fn test_len_tracking_through_insert_delete() {
        let mut d = CompactDict::new();
        for i in 0..5u64 {
            d.set(i, i, h(i));
        }
        assert_eq!(d.len(), 5);
        d.delete(2, h(2));
        assert_eq!(d.len(), 4);
        d.delete(2, h(2)); // double-delete is no-op
        assert_eq!(d.len(), 4);
        d.set(10, 100, h(10));
        assert_eq!(d.len(), 5);
    }

    #[test]
    fn test_large_dict_delete_and_reinsert() {
        let mut d = CompactDict::new();
        // Graduate to large storage.
        for i in 0..10u64 {
            d.set(i, i, h(i));
        }
        assert!(matches!(d.storage, Storage::Large(_)));
        // Delete a key.
        assert_eq!(d.delete(5, h(5)), Some(5));
        assert_eq!(d.len(), 9);
        assert_eq!(d.get(5, h(5)), None);
        // Re-insert the same key.
        d.set(5, 500, h(5));
        assert_eq!(d.len(), 10);
        assert_eq!(d.get(5, h(5)), Some(500));
        // Remaining keys still intact.
        for i in 0..10u64 {
            let expected = if i == 5 { 500 } else { i };
            assert_eq!(d.get(i, h(i)), Some(expected), "key {} wrong after delete+reinsert", i);
        }
    }

    #[test]
    fn test_default() {
        let d: CompactDict = Default::default();
        assert!(d.is_empty());
    }
}
