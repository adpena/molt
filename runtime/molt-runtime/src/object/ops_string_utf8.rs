//! UTF-8 and WTF-8 helpers for string operations.

use crate::object::utf8_cache::{
    UTF8_CACHE_BLOCK, UTF8_CACHE_MIN_LEN, UTF8_COUNT_CACHE_SHARDS, UTF8_COUNT_PREFIX_MIN_LEN,
    UTF8_COUNT_TLS, Utf8CountCache, Utf8CountCacheEntry, Utf8IndexCache,
};
use crate::*;
use memchr::memmem;
use std::sync::Arc;
use wtf8::{CodePoint, Wtf8};

use super::bytes_count_impl;

fn build_utf8_cache(bytes: &[u8]) -> Utf8IndexCache {
    let mut offsets = Vec::new();
    let mut prefix = Vec::new();
    let mut total = 0i64;
    let mut idx = 0usize;
    offsets.push(0);
    prefix.push(0);
    while idx < bytes.len() {
        let mut end = (idx + UTF8_CACHE_BLOCK).min(bytes.len());
        while end < bytes.len() && (bytes[end] & 0b1100_0000) == 0b1000_0000 {
            end += 1;
        }
        total += count_utf8_bytes(&bytes[idx..end]);
        offsets.push(end);
        prefix.push(total);
        idx = end;
    }
    Utf8IndexCache { offsets, prefix }
}

fn utf8_cache_get_or_build(
    _py: &PyToken<'_>,
    key: usize,
    bytes: &[u8],
) -> Option<Arc<Utf8IndexCache>> {
    if bytes.len() < UTF8_CACHE_MIN_LEN || bytes.is_ascii() {
        return None;
    }
    if let Ok(store) = runtime_state(_py).utf8_index_cache.lock()
        && let Some(cache) = store.get(key)
    {
        return Some(cache);
    }
    let cache = Arc::new(build_utf8_cache(bytes));
    if let Ok(mut store) = runtime_state(_py).utf8_index_cache.lock() {
        if let Some(existing) = store.get(key) {
            return Some(existing);
        }
        store.insert(key, cache.clone());
    }
    Some(cache)
}

pub(crate) fn utf8_cache_remove(_py: &PyToken<'_>, key: usize) {
    if let Ok(mut store) = runtime_state(_py).utf8_index_cache.lock() {
        store.remove(key);
    }
    utf8_count_cache_remove(_py, key);
    utf8_count_cache_tls_remove(key);
}

fn utf8_count_cache_shard(key: usize) -> usize {
    let mut x = key as u64;
    x ^= x >> 33;
    x = x.wrapping_mul(0xff51afd7ed558ccd);
    x ^= x >> 33;
    (x as usize) & (UTF8_COUNT_CACHE_SHARDS - 1)
}

fn utf8_count_cache_remove(_py: &PyToken<'_>, key: usize) {
    let shard = utf8_count_cache_shard(key);
    if let Some(store) = runtime_state(_py).utf8_count_cache.get(shard)
        && let Ok(mut guard) = store.lock()
    {
        guard.remove(key);
    }
}

pub(super) fn utf8_count_cache_lookup(
    _py: &PyToken<'_>,
    key: usize,
    needle: &[u8],
) -> Option<Arc<Utf8CountCache>> {
    if let Some(cache) = UTF8_COUNT_TLS.with(|cell| {
        cell.borrow().as_ref().and_then(|entry| {
            if entry.key == key && entry.cache.needle == needle {
                Some(entry.cache.clone())
            } else {
                None
            }
        })
    }) {
        profile_hit(_py, &STRING_COUNT_CACHE_HIT_COUNT);
        return Some(cache);
    }
    let shard = utf8_count_cache_shard(key);
    let store = runtime_state(_py)
        .utf8_count_cache
        .get(shard)?
        .lock()
        .ok()?;
    let cache = store.get(key)?;
    if cache.needle == needle {
        profile_hit(_py, &STRING_COUNT_CACHE_HIT_COUNT);
        return Some(cache);
    }
    None
}

fn build_utf8_count_prefix(hay_bytes: &[u8], needle: &[u8]) -> Vec<i64> {
    if hay_bytes.len() < UTF8_COUNT_PREFIX_MIN_LEN || needle.is_empty() {
        return Vec::new();
    }
    let blocks = hay_bytes.len().div_ceil(UTF8_CACHE_BLOCK);
    let mut prefix = vec![0i64; blocks + 1];
    let mut count = 0i64;
    let mut idx = 1usize;
    let mut next_boundary = UTF8_CACHE_BLOCK.min(hay_bytes.len());
    let finder = memmem::Finder::new(needle);
    for pos in finder.find_iter(hay_bytes) {
        while pos >= next_boundary && idx < prefix.len() {
            prefix[idx] = count;
            idx += 1;
            next_boundary = (next_boundary + UTF8_CACHE_BLOCK).min(hay_bytes.len());
        }
        count += 1;
    }
    while idx < prefix.len() {
        prefix[idx] = count;
        idx += 1;
    }
    prefix
}

pub(super) fn utf8_count_cache_store(
    _py: &PyToken<'_>,
    key: usize,
    hay_bytes: &[u8],
    needle: &[u8],
    count: i64,
    prefix: Vec<i64>,
) {
    let cache = Arc::new(Utf8CountCache {
        needle: needle.to_vec(),
        count,
        prefix,
        hay_len: hay_bytes.len(),
    });
    let shard = utf8_count_cache_shard(key);
    if let Some(store) = runtime_state(_py).utf8_count_cache.get(shard)
        && let Ok(mut guard) = store.lock()
    {
        guard.insert(key, cache.clone());
    }
    UTF8_COUNT_TLS.with(|cell| {
        *cell.borrow_mut() = Some(Utf8CountCacheEntry { key, cache });
    });
}

pub(super) fn utf8_count_cache_upgrade_prefix(
    _py: &PyToken<'_>,
    key: usize,
    cache: &Arc<Utf8CountCache>,
    hay_bytes: &[u8],
) -> Arc<Utf8CountCache> {
    if !cache.prefix.is_empty()
        || cache.hay_len != hay_bytes.len()
        || hay_bytes.len() < UTF8_COUNT_PREFIX_MIN_LEN
        || cache.needle.is_empty()
    {
        return cache.clone();
    }
    let prefix = build_utf8_count_prefix(hay_bytes, &cache.needle);
    if prefix.is_empty() {
        return cache.clone();
    }
    let upgraded = Arc::new(Utf8CountCache {
        needle: cache.needle.clone(),
        count: cache.count,
        prefix,
        hay_len: cache.hay_len,
    });
    let shard = utf8_count_cache_shard(key);
    if let Some(store) = runtime_state(_py).utf8_count_cache.get(shard)
        && let Ok(mut guard) = store.lock()
    {
        guard.insert(key, upgraded.clone());
    }
    UTF8_COUNT_TLS.with(|cell| {
        *cell.borrow_mut() = Some(Utf8CountCacheEntry {
            key,
            cache: upgraded.clone(),
        });
    });
    upgraded
}

fn utf8_count_cache_tls_remove(key: usize) {
    // Use try_with to avoid panicking during TLS destruction.
    // When thread-locals are being torn down (e.g., in ThreadLocalGuard::drop),
    // dec_ref on cached strings can reach this function after UTF8_COUNT_TLS
    // is already destroyed.
    let _ = UTF8_COUNT_TLS.try_with(|cell| {
        let mut guard = cell.borrow_mut();
        if guard.as_ref().is_some_and(|entry| entry.key == key) {
            *guard = None;
        }
    });
}

fn count_matches_range(
    hay_bytes: &[u8],
    needle: &[u8],
    window_start: usize,
    window_end: usize,
    start_min: usize,
    start_max: usize,
) -> i64 {
    if window_end <= window_start || start_min > start_max {
        return 0;
    }
    let finder = memmem::Finder::new(needle);
    let mut count = 0i64;
    for pos in finder.find_iter(&hay_bytes[window_start..window_end]) {
        let abs = window_start + pos;
        if abs < start_min {
            continue;
        }
        if abs > start_max {
            break;
        }
        count += 1;
    }
    count
}

pub(super) fn utf8_count_cache_count_slice(
    cache: &Utf8CountCache,
    hay_bytes: &[u8],
    start: usize,
    end: usize,
) -> i64 {
    let needle = &cache.needle;
    let needle_len = needle.len();
    if needle_len == 0 || end <= start {
        return 0;
    }
    if end - start < needle_len {
        return 0;
    }
    if cache.prefix.is_empty() || cache.hay_len != hay_bytes.len() {
        return bytes_count_impl(&hay_bytes[start..end], needle);
    }
    let end_limit = end - needle_len;
    let block = UTF8_CACHE_BLOCK;
    let start_block = start / block;
    let end_block = end_limit / block;
    if start_block == end_block {
        return bytes_count_impl(&hay_bytes[start..end], needle);
    }
    let mut total = 0i64;
    let block_end = ((start_block + 1) * block).min(hay_bytes.len());
    let left_scan_end = (block_end + needle_len - 1).min(end);
    let left_max = (block_end.saturating_sub(1)).min(end_limit);
    total += count_matches_range(hay_bytes, needle, start, left_scan_end, start, left_max);
    if end_block > start_block + 1 {
        total += cache.prefix[end_block] - cache.prefix[start_block + 1];
    }
    let right_block_start = (end_block * block).min(hay_bytes.len());
    if right_block_start <= end_limit {
        total += count_matches_range(
            hay_bytes,
            needle,
            right_block_start,
            end,
            right_block_start,
            end_limit,
        );
    }
    total
}

fn utf8_count_prefix_cached(bytes: &[u8], cache: &Utf8IndexCache, prefix_len: usize) -> i64 {
    let prefix_len = prefix_len.min(bytes.len());
    let block_idx = match cache.offsets.binary_search(&prefix_len) {
        Ok(idx) => idx,
        Err(idx) => idx.saturating_sub(1),
    };
    let mut total = *cache.prefix.get(block_idx).unwrap_or(&0);
    let start = *cache.offsets.get(block_idx).unwrap_or(&0);
    if start < prefix_len {
        total += count_utf8_bytes(&bytes[start..prefix_len]);
    }
    total
}

pub(crate) fn utf8_codepoint_count_cached(
    _py: &PyToken<'_>,
    bytes: &[u8],
    cache_key: Option<usize>,
) -> i64 {
    if bytes.is_ascii() {
        return bytes.len() as i64;
    }
    if let Some(key) = cache_key
        && let Some(cache) = utf8_cache_get_or_build(_py, key, bytes)
    {
        return *cache.prefix.last().unwrap_or(&0);
    }
    utf8_count_prefix_blocked(bytes, bytes.len())
}

pub(super) fn utf8_byte_to_char_index_cached(
    _py: &PyToken<'_>,
    bytes: &[u8],
    byte_idx: usize,
    cache_key: Option<usize>,
) -> i64 {
    if byte_idx == 0 {
        return 0;
    }
    if bytes.is_ascii() {
        return byte_idx.min(bytes.len()) as i64;
    }
    let prefix_len = byte_idx.min(bytes.len());
    if let Some(key) = cache_key
        && let Some(cache) = utf8_cache_get_or_build(_py, key, bytes)
    {
        return utf8_count_prefix_cached(bytes, &cache, prefix_len);
    }
    utf8_count_prefix_blocked(bytes, prefix_len)
}

pub(in crate::object) fn wtf8_from_bytes(bytes: &[u8]) -> &Wtf8 {
    // SAFETY: Molt string bytes are constructed as well-formed WTF-8.
    unsafe { &*(bytes as *const [u8] as *const Wtf8) }
}

pub(in crate::object) fn wtf8_codepoint_at(bytes: &[u8], idx: usize) -> Option<CodePoint> {
    wtf8_from_bytes(bytes).code_points().nth(idx)
}

#[allow(dead_code)]
fn wtf8_codepoint_count_scan(bytes: &[u8]) -> i64 {
    let mut idx = 0usize;
    let mut count = 0i64;
    while idx < bytes.len() {
        let width = utf8_char_width(bytes[idx]);
        if width == 0 {
            idx = idx.saturating_add(1);
        } else {
            idx = idx.saturating_add(width);
        }
        count += 1;
    }
    count
}

pub(in crate::object) fn wtf8_has_surrogates(bytes: &[u8]) -> bool {
    wtf8_from_bytes(bytes).as_str().is_none()
}

pub(in crate::object) fn push_wtf8_codepoint(out: &mut Vec<u8>, code: u32) {
    molt_stdlib_text::wtf8::push_wtf8_codepoint(out, code);
}

fn utf8_char_width(first: u8) -> usize {
    if first < 0xC0 {
        1
    } else if first < 0xE0 {
        2
    } else if first < 0xF0 {
        3
    } else if first < 0xF8 {
        4
    } else {
        1
    }
}

fn utf8_char_to_byte_index_scan(bytes: &[u8], target: usize) -> usize {
    let mut idx = 0usize;
    let mut count = 0usize;
    while idx < bytes.len() && count < target {
        let width = utf8_char_width(bytes[idx]);
        idx = idx.saturating_add(width);
        count = count.saturating_add(1);
    }
    idx.min(bytes.len())
}

pub(in crate::object) fn utf8_char_to_byte_index_cached(
    _py: &PyToken<'_>,
    bytes: &[u8],
    char_idx: i64,
    cache_key: Option<usize>,
) -> usize {
    if char_idx <= 0 {
        return 0;
    }
    if bytes.is_ascii() {
        return (char_idx as usize).min(bytes.len());
    }
    let total = utf8_codepoint_count_cached(_py, bytes, cache_key);
    if char_idx >= total {
        return bytes.len();
    }
    let target = char_idx as usize;
    if let Some(key) = cache_key
        && let Some(cache) = utf8_cache_get_or_build(_py, key, bytes)
    {
        let mut lo = 0usize;
        let mut hi = cache.prefix.len().saturating_sub(1);
        while lo < hi {
            let mid = (lo + hi).div_ceil(2);
            if (cache.prefix.get(mid).copied().unwrap_or(0) as usize) <= target {
                lo = mid;
            } else {
                hi = mid.saturating_sub(1);
            }
        }
        let mut count = cache.prefix.get(lo).copied().unwrap_or(0) as usize;
        let mut idx = cache.offsets.get(lo).copied().unwrap_or(0);
        while idx < bytes.len() && count < target {
            let width = utf8_char_width(bytes[idx]);
            idx = idx.saturating_add(width);
            count = count.saturating_add(1);
        }
        return idx.min(bytes.len());
    }
    utf8_char_to_byte_index_scan(bytes, target)
}

fn utf8_count_prefix_blocked(bytes: &[u8], prefix_len: usize) -> i64 {
    const BLOCK: usize = 4096;
    let mut total = 0i64;
    let mut idx = 0usize;
    while idx + BLOCK <= prefix_len {
        total += count_utf8_bytes(&bytes[idx..idx + BLOCK]);
        idx += BLOCK;
    }
    if idx < prefix_len {
        total += count_utf8_bytes(&bytes[idx..prefix_len]);
    }
    total
}

#[cfg(not(target_arch = "wasm32"))]
fn count_utf8_bytes(bytes: &[u8]) -> i64 {
    // simdutf::count_utf8 counts non-continuation bytes, which works
    // correctly on both valid UTF-8 and WTF-8. No validation needed.
    #[cfg(feature = "simdutf")]
    {
        simdutf::count_utf8(bytes) as i64
    }
    #[cfg(not(feature = "simdutf"))]
    {
        bytes.iter().filter(|&&b| (b & 0xC0) != 0x80).count() as i64
    }
}

#[cfg(target_arch = "wasm32")]
fn count_utf8_bytes(bytes: &[u8]) -> i64 {
    // WASM SIMD fast path: count non-continuation bytes directly.
    // Works on valid UTF-8 and WTF-8 alike — no validation needed.
    unsafe { count_utf8_codepoints_wasm_simd(bytes) }
}

#[cfg(target_arch = "wasm32")]
unsafe fn count_utf8_codepoints_wasm_simd(bytes: &[u8]) -> i64 {
    unsafe {
        use std::arch::wasm32::*;
        let mut count = 0i64;
        let mut i = 0usize;
        let cont_mask = u8x16_splat(0xC0);
        let cont_pat = u8x16_splat(0x80);
        while i + 16 <= bytes.len() {
            let chunk = v128_load(bytes.as_ptr().add(i) as *const v128);
            // Isolate top 2 bits, compare to 0x80 → continuation bytes
            let masked = v128_and(chunk, cont_mask);
            let is_cont = u8x16_eq(masked, cont_pat);
            // Bitmask: bit set for each continuation byte
            let mask = u8x16_bitmask(is_cont);
            // 16 minus number of continuation bytes = number of codepoint-starting bytes
            count += (16 - mask.count_ones()) as i64;
            i += 16;
        }
        // Scalar tail
        for &b in &bytes[i..] {
            if (b & 0xC0) != 0x80 {
                count += 1;
            }
        }
        count
    }
}
