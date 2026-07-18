use std::sync::Arc;

use super::state::DaemonCache;

pub(crate) fn insert_daemon_cache_entries(
    cache: &mut DaemonCache,
    cache_key: &str,
    function_cache_key: &str,
    output_bytes: Arc<[u8]>,
) {
    if !cache_key.is_empty() && !function_cache_key.is_empty() && function_cache_key != cache_key {
        cache.insert(cache_key.to_string(), Arc::clone(&output_bytes));
        cache.insert(function_cache_key.to_string(), output_bytes);
    } else if !cache_key.is_empty() {
        cache.insert(cache_key.to_string(), output_bytes);
    } else if !function_cache_key.is_empty() {
        cache.insert(function_cache_key.to_string(), output_bytes);
    }
}
