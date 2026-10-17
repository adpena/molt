use std::cmp::Reverse;
use std::collections::{BinaryHeap, HashMap};
use std::sync::Arc;

use super::super::protocol::DaemonCacheKey;

/// The daemon's byte-budgeted LRU of compiled outputs; `DaemonCache` shares it.
pub(crate) struct DaemonLru {
    pub(crate) entries: HashMap<Arc<DaemonCacheKey>, CacheEntry>,
    pub(crate) order: BinaryHeap<Reverse<(u64, Arc<DaemonCacheKey>)>>,
    pub(crate) clock: u64,
    pub(crate) bytes: usize,
    pub(crate) max_bytes: Option<usize>,
}

pub(crate) struct CacheEntry {
    pub(crate) bytes: Arc<[u8]>,
    pub(crate) stamp: u64,
}

impl DaemonLru {
    pub(crate) fn new(max_bytes: Option<usize>) -> Self {
        Self {
            entries: HashMap::new(),
            order: BinaryHeap::new(),
            clock: 0,
            bytes: 0,
            max_bytes,
        }
    }

    pub(crate) fn get_bytes(&mut self, key: &DaemonCacheKey) -> Option<Arc<[u8]>> {
        let key_ref = Arc::clone(self.entries.get_key_value(key)?.0);
        let entry = self.entries.get_mut(key)?;
        self.clock = self.clock.wrapping_add(1);
        let stamp = self.clock;
        entry.stamp = stamp;
        self.order.push(Reverse((stamp, key_ref)));
        let bytes = Arc::clone(&entry.bytes);
        self.compact_order();
        Some(bytes)
    }

    pub(crate) fn insert(&mut self, key: Arc<DaemonCacheKey>, value: Arc<[u8]>) {
        if let Some(prev) = self.entries.remove(key.as_ref()) {
            self.bytes = self.bytes.saturating_sub(prev.bytes.len());
        }
        self.clock = self.clock.wrapping_add(1);
        let stamp = self.clock;
        self.bytes = self.bytes.saturating_add(value.len());
        self.order.push(Reverse((stamp, Arc::clone(&key))));
        self.entries.insert(
            key,
            CacheEntry {
                bytes: value,
                stamp,
            },
        );
        self.evict();
    }

    fn evict(&mut self) {
        while self
            .max_bytes
            .is_some_and(|max_bytes| self.bytes > max_bytes)
        {
            let Some(Reverse((stamp, old_key))) = self.order.pop() else {
                break;
            };
            let is_live = self
                .entries
                .get(&old_key)
                .is_some_and(|entry| entry.stamp == stamp);
            if !is_live {
                continue;
            }
            if let Some(old_val) = self.entries.remove(&old_key) {
                self.bytes = self.bytes.saturating_sub(old_val.bytes.len());
            }
        }
        self.compact_order();
    }

    fn compact_order(&mut self) {
        // Compact stale generations after enough churn, including hit-only workloads.
        if self.order.len() > self.entries.len().saturating_mul(8).saturating_add(32) {
            self.order = self
                .entries
                .iter()
                .map(|(key, entry)| Reverse((entry.stamp, Arc::clone(key))))
                .collect();
        }
    }
}
