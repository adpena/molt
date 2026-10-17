use std::sync::Arc;

use molt_passes::resident_cache::ResidentCache;

use super::super::protocol::DaemonCacheKey;

/// The shared compiler eviction authority; keys and output storage both count.
pub(crate) struct DaemonLru {
    resident: ResidentCache<Arc<DaemonCacheKey>, Arc<[u8]>>,
}

impl DaemonLru {
    pub(crate) fn new(max_bytes: Option<usize>) -> Self {
        Self {
            resident: ResidentCache::new(max_bytes),
        }
    }

    pub(crate) fn get_bytes(&mut self, key: &DaemonCacheKey) -> Option<Arc<[u8]>> {
        self.resident.get(key).map(Arc::clone)
    }

    pub(crate) fn insert(&mut self, key: Arc<DaemonCacheKey>, value: Arc<[u8]>) {
        let allocation_bytes = key
            .retained_bytes()
            .saturating_add(value.len())
            .saturating_add(2 * std::mem::size_of::<usize>()); // output Arc header
        self.resident.insert(key, value, allocation_bytes);
    }

    pub(crate) fn usage(&self) -> (usize, usize) {
        (self.resident.len(), self.resident.bytes())
    }
}
