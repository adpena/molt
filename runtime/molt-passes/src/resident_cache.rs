//! One resident eviction owner for compiler artifacts. Persistence, quarantine
//! and concurrency remain with the callers. This crate is compiler-side only.

use std::borrow::Borrow;
use std::collections::hash_map::RandomState;
use std::hash::Hash;

use hashlink::LinkedHashMap;

/// An exact LRU bounded by a conservative retained-ownership charge.
///
/// Callers supply transitive allocation bytes, including shared allocations
/// conservatively per entry. We add inline entry storage and four pointer words
/// for links/table slots. Allocator headers, capacity slack and caller-held
/// values are not an RSS measure. Evicted keys/values are dropped immediately;
/// sparse table/free-node capacity is reclaimed on mutations, never on hits.
pub struct ResidentCache<K, V> {
    entries: LinkedHashMap<K, Entry<V>, RandomState>,
    bytes: usize,
    max_bytes: Option<usize>,
}

struct Entry<V> {
    value: V,
    bytes: usize,
}

impl<K: Eq + Hash, V> ResidentCache<K, V> {
    pub fn new(max_bytes: Option<usize>) -> Self {
        Self {
            entries: LinkedHashMap::with_hasher(RandomState::new()),
            bytes: 0,
            max_bytes,
        }
    }

    pub fn len(&self) -> usize {
        self.entries.len()
    }

    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }

    /// Charged ownership bytes, including metadata; not process RSS.
    pub fn bytes(&self) -> usize {
        self.bytes
    }

    pub fn max_bytes(&self) -> Option<usize> {
        self.max_bytes
    }

    /// Test the resident boundary without changing the eviction oracle.
    #[cfg(test)]
    pub(crate) fn peek<Q: Hash + Eq + ?Sized>(&self, key: &Q) -> Option<&V>
    where
        K: Borrow<Q>,
    {
        self.entries.get(key).map(|entry| &entry.value)
    }

    /// Resident values, oldest first, without promoting any of them.
    pub fn values(&self) -> impl Iterator<Item = &V> {
        self.entries.values().map(|entry| &entry.value)
    }

    /// Promote in place. No key clone, allocation, clock or stale generation.
    pub fn get<Q: Hash + Eq + ?Sized>(&mut self, key: &Q) -> Option<&V>
    where
        K: Borrow<Q>,
    {
        self.entries.to_back(key).map(|entry| &entry.value)
    }

    pub fn remove<Q: Hash + Eq + ?Sized>(&mut self, key: &Q) -> Option<V>
    where
        K: Borrow<Q>,
    {
        let entry = self.entries.remove(key)?;
        self.bytes -= entry.bytes;
        self.reclaim_sparse_storage();
        Some(entry.value)
    }

    /// Replace and promote, or reject an oversized entry without displacing
    /// unrelated entries. An inadmissible replacement retires its old value.
    pub fn insert(&mut self, key: K, value: V, allocation_bytes: usize) {
        if let Some(old) = self.entries.remove(&key) {
            self.bytes -= old.bytes;
        }
        let bytes = allocation_bytes
            .checked_add(std::mem::size_of::<(K, Entry<V>)>())
            .and_then(|bytes| bytes.checked_add(4 * std::mem::size_of::<usize>()));
        let Some(bytes) = bytes.filter(|bytes| self.max_bytes.is_none_or(|max| *bytes <= max))
        else {
            self.reclaim_sparse_storage();
            return;
        };
        // Subtraction avoids overflow, including an explicitly unbounded cache.
        let max = self.max_bytes.unwrap_or(usize::MAX);
        while self.bytes > max - bytes {
            let (_, oldest) = self.entries.pop_front().expect("charged resident entry");
            self.bytes -= oldest.bytes;
        }
        self.bytes += bytes;
        self.entries.insert(key, Entry { value, bytes });
        self.reclaim_sparse_storage();
    }

    fn reclaim_sparse_storage(&mut self) {
        if self.entries.capacity() > self.entries.len().saturating_mul(4) {
            self.entries.shrink_to_fit();
        }
    }

    #[cfg(test)]
    pub(crate) fn capacity(&self) -> usize {
        self.entries.capacity()
    }
}

#[cfg(test)]
mod tests;
