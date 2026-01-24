use std::cell::RefCell;
use std::collections::{HashMap, VecDeque};
use std::sync::{Arc, Mutex};

pub(crate) const UTF8_CACHE_BLOCK: usize = 4096;
pub(crate) const UTF8_CACHE_MIN_LEN: usize = 16 * 1024;
pub(crate) const UTF8_COUNT_PREFIX_MIN_LEN: usize = UTF8_CACHE_BLOCK;
pub(crate) const UTF8_CACHE_MAX_ENTRIES: usize = 128;
pub(crate) const UTF8_COUNT_CACHE_SHARDS: usize = 8;

pub(crate) struct Utf8IndexCache {
    pub(crate) offsets: Vec<usize>,
    pub(crate) prefix: Vec<i64>,
}

pub(crate) struct Utf8CountCache {
    pub(crate) needle: Vec<u8>,
    pub(crate) count: i64,
    pub(crate) prefix: Vec<i64>,
    pub(crate) hay_len: usize,
}

pub(crate) struct Utf8CountCacheEntry {
    pub(crate) key: usize,
    pub(crate) cache: Arc<Utf8CountCache>,
}

pub(crate) struct Utf8CountCacheStore {
    entries: HashMap<usize, Arc<Utf8CountCache>>,
    order: VecDeque<usize>,
    capacity: usize,
}

pub(crate) struct Utf8CacheStore {
    entries: HashMap<usize, Arc<Utf8IndexCache>>,
    order: VecDeque<usize>,
}

thread_local! {
    pub(crate) static UTF8_COUNT_TLS: RefCell<Option<Utf8CountCacheEntry>> = const { RefCell::new(None) };
}

pub(crate) fn clear_utf8_count_tls() {
    let _ = UTF8_COUNT_TLS.try_with(|cell| {
        cell.borrow_mut().take();
    });
}

pub(crate) fn build_utf8_count_cache() -> Vec<Mutex<Utf8CountCacheStore>> {
    let per_shard = (UTF8_CACHE_MAX_ENTRIES / UTF8_COUNT_CACHE_SHARDS).max(1);
    (0..UTF8_COUNT_CACHE_SHARDS)
        .map(|_| Mutex::new(Utf8CountCacheStore::new(per_shard)))
        .collect()
}

impl Utf8CountCacheStore {
    pub(crate) fn new(capacity: usize) -> Self {
        Self {
            entries: HashMap::new(),
            order: VecDeque::new(),
            capacity,
        }
    }

    pub(crate) fn get(&self, key: usize) -> Option<Arc<Utf8CountCache>> {
        self.entries.get(&key).cloned()
    }

    pub(crate) fn insert(&mut self, key: usize, cache: Arc<Utf8CountCache>) {
        if let std::collections::hash_map::Entry::Occupied(mut entry) = self.entries.entry(key) {
            entry.insert(cache);
            return;
        }
        self.entries.insert(key, cache);
        self.order.push_back(key);
        while self.entries.len() > self.capacity {
            if let Some(evict) = self.order.pop_front() {
                self.entries.remove(&evict);
            } else {
                break;
            }
        }
    }

    pub(crate) fn remove(&mut self, key: usize) {
        self.entries.remove(&key);
        self.order.retain(|entry| *entry != key);
    }
}

impl Utf8CacheStore {
    pub(crate) fn new() -> Self {
        Self {
            entries: HashMap::new(),
            order: VecDeque::new(),
        }
    }

    pub(crate) fn get(&self, key: usize) -> Option<Arc<Utf8IndexCache>> {
        self.entries.get(&key).cloned()
    }

    pub(crate) fn insert(&mut self, key: usize, cache: Arc<Utf8IndexCache>) {
        if self.entries.contains_key(&key) {
            return;
        }
        self.entries.insert(key, cache);
        self.order.push_back(key);
        while self.entries.len() > UTF8_CACHE_MAX_ENTRIES {
            if let Some(evict) = self.order.pop_front() {
                self.entries.remove(&evict);
            } else {
                break;
            }
        }
    }

    pub(crate) fn remove(&mut self, key: usize) {
        self.entries.remove(&key);
        self.order.retain(|entry| *entry != key);
    }
}
