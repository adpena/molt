use std::sync::Arc;

use super::super::protocol::DaemonCacheKey;
use super::shared::DaemonCache;
use crate::backend_process::origin::DaemonCacheEntry;

pub(crate) fn insert_daemon_cache_entries(
    cache: &DaemonCache,
    cache_key: Option<&Arc<DaemonCacheKey>>,
    function_cache_key: Option<&Arc<DaemonCacheKey>>,
    entry: Arc<DaemonCacheEntry>,
) {
    if let Some(key) = cache_key {
        cache.insert(Arc::clone(key), Arc::clone(&entry));
    }
    if let Some(key) = function_cache_key
        && function_cache_key != cache_key
    {
        cache.insert(Arc::clone(key), entry);
    }
}
