use std::sync::Arc;

use super::super::protocol::DaemonCacheKey;
use super::shared::DaemonCache;

pub(crate) fn insert_daemon_cache_entries(
    cache: &DaemonCache,
    cache_key: Option<&Arc<DaemonCacheKey>>,
    function_cache_key: Option<&Arc<DaemonCacheKey>>,
    output_bytes: Arc<[u8]>,
) {
    if let Some(key) = cache_key {
        cache.insert(Arc::clone(key), Arc::clone(&output_bytes));
    }
    if let Some(key) = function_cache_key
        && function_cache_key != cache_key
    {
        cache.insert(Arc::clone(key), output_bytes);
    }
}
