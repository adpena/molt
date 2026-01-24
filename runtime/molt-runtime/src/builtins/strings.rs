use memchr::{memchr, memmem};
use molt_obj_model::MoltObject;

use crate::{
    alloc_list_with_capacity, alloc_string, dec_ref_bits, inc_ref_bits, obj_from_bits,
    object_type_id, seq_vec, string_bytes, utf8_codepoint_count_cached, MAX_SMALL_LIST,
    TYPE_ID_STRING,
};

pub(crate) fn bytes_find_impl(hay_bytes: &[u8], needle_bytes: &[u8]) -> i64 {
    if needle_bytes.is_empty() {
        return 0;
    }
    if needle_bytes.len() == 1 {
        return memchr(needle_bytes[0], hay_bytes)
            .map(|v| v as i64)
            .unwrap_or(-1);
    }
    if needle_bytes.len() <= 4 {
        return bytes_find_short(hay_bytes, needle_bytes)
            .map(|v| v as i64)
            .unwrap_or(-1);
    }
    memmem::find(hay_bytes, needle_bytes)
        .map(|v| v as i64)
        .unwrap_or(-1)
}

pub(crate) fn bytes_rfind_impl(hay_bytes: &[u8], needle_bytes: &[u8]) -> i64 {
    if needle_bytes.is_empty() {
        return hay_bytes.len() as i64;
    }
    if needle_bytes.len() == 1 {
        return memchr::memrchr(needle_bytes[0], hay_bytes)
            .map(|v| v as i64)
            .unwrap_or(-1);
    }
    let finder = memmem::Finder::new(needle_bytes);
    let mut last = None;
    for idx in finder.find_iter(hay_bytes) {
        last = Some(idx);
    }
    last.map(|v| v as i64).unwrap_or(-1)
}

pub(crate) fn bytes_count_impl(hay_bytes: &[u8], needle_bytes: &[u8]) -> i64 {
    if needle_bytes.is_empty() {
        return hay_bytes.len() as i64 + 1;
    }
    if needle_bytes.len() == 1 {
        return memchr::memchr_iter(needle_bytes[0], hay_bytes).count() as i64;
    }
    if needle_bytes.len() <= 4 {
        return bytes_count_short(hay_bytes, needle_bytes);
    }
    let finder = memmem::Finder::new(needle_bytes);
    finder.find_iter(hay_bytes).count() as i64
}

fn bytes_find_short(hay: &[u8], needle: &[u8]) -> Option<usize> {
    let needle_len = needle.len();
    let first = needle[0];
    let mut search = 0usize;
    match needle_len {
        2 => {
            let b1 = needle[1];
            while let Some(idx) = memchr_fast(first, &hay[search..]) {
                let pos = search + idx;
                if pos + 1 < hay.len() && hay[pos + 1] == b1 {
                    return Some(pos);
                }
                search = pos + 1;
            }
        }
        3 => {
            let b1 = needle[1];
            let b2 = needle[2];
            while let Some(idx) = memchr_fast(first, &hay[search..]) {
                let pos = search + idx;
                if pos + 2 < hay.len() && hay[pos + 1] == b1 && hay[pos + 2] == b2 {
                    return Some(pos);
                }
                search = pos + 1;
            }
        }
        4 => {
            let b1 = needle[1];
            let b2 = needle[2];
            let b3 = needle[3];
            while let Some(idx) = memchr_fast(first, &hay[search..]) {
                let pos = search + idx;
                if pos + 3 < hay.len()
                    && hay[pos + 1] == b1
                    && hay[pos + 2] == b2
                    && hay[pos + 3] == b3
                {
                    return Some(pos);
                }
                search = pos + 1;
            }
        }
        _ => {
            while let Some(idx) = memchr_fast(first, &hay[search..]) {
                let pos = search + idx;
                if pos + needle_len <= hay.len() && &hay[pos..pos + needle_len] == needle {
                    return Some(pos);
                }
                search = pos + 1;
            }
        }
    }
    None
}

fn bytes_count_short(hay: &[u8], needle: &[u8]) -> i64 {
    let needle_len = needle.len();
    let first = needle[0];
    let mut count = 0i64;
    let mut search = 0usize;
    match needle_len {
        2 => {
            let b1 = needle[1];
            let mut next_allowed = 0usize;
            for pos in memchr::memchr_iter(first, hay) {
                if pos < next_allowed {
                    continue;
                }
                if pos + 1 < hay.len() && hay[pos + 1] == b1 {
                    count += 1;
                    next_allowed = pos + 2;
                }
            }
        }
        3 => {
            let b1 = needle[1];
            let b2 = needle[2];
            while let Some(idx) = memchr_fast(first, &hay[search..]) {
                let pos = search + idx;
                if pos + 2 < hay.len() && hay[pos + 1] == b1 && hay[pos + 2] == b2 {
                    count += 1;
                    search = pos + 3;
                } else {
                    search = pos + 1;
                }
            }
        }
        4 => {
            let b1 = needle[1];
            let b2 = needle[2];
            let b3 = needle[3];
            while let Some(idx) = memchr_fast(first, &hay[search..]) {
                let pos = search + idx;
                if pos + 3 < hay.len()
                    && hay[pos + 1] == b1
                    && hay[pos + 2] == b2
                    && hay[pos + 3] == b3
                {
                    count += 1;
                    search = pos + 4;
                } else {
                    search = pos + 1;
                }
            }
        }
        _ => {
            while let Some(idx) = memchr_fast(first, &hay[search..]) {
                let pos = search + idx;
                if pos + needle_len <= hay.len() && &hay[pos..pos + needle_len] == needle {
                    count += 1;
                    search = pos + needle_len;
                } else {
                    search = pos + 1;
                }
            }
        }
    }
    count
}

fn memchr_fast(needle: u8, hay: &[u8]) -> Option<usize> {
    let (supported, idx) = memchr_simd128(needle, hay);
    if supported {
        return idx;
    }
    memchr(needle, hay)
}

#[cfg(all(target_arch = "wasm32", target_os = "unknown"))]
fn memchr_simd128(needle: u8, hay: &[u8]) -> (bool, Option<usize>) {
    if !std::arch::is_wasm_feature_detected!("simd128") {
        return (false, None);
    }
    unsafe {
        use std::arch::wasm32::*;
        let mut idx = 0usize;
        let needle_vec = u8x16_splat(needle);
        while idx + 16 <= hay.len() {
            let chunk = v128_load(hay.as_ptr().add(idx) as *const v128);
            let mask = u8x16_eq(chunk, needle_vec);
            let bits = u8x16_bitmask(mask) as u32;
            if bits != 0 {
                return (true, Some(idx + bits.trailing_zeros() as usize));
            }
            idx += 16;
        }
        if idx < hay.len() {
            if let Some(tail_idx) = memchr(needle, &hay[idx..]) {
                return (true, Some(idx + tail_idx));
            }
        }
    }
    (true, None)
}

#[cfg(all(target_arch = "wasm32", not(target_os = "unknown")))]
fn memchr_simd128(_needle: u8, _hay: &[u8]) -> (bool, Option<usize>) {
    (false, None)
}

pub(crate) fn replace_bytes_impl(hay: &[u8], needle: &[u8], replacement: &[u8]) -> Option<Vec<u8>> {
    if needle.is_empty() {
        let mut out = Vec::with_capacity(hay.len() + replacement.len() * (hay.len() + 1));
        out.extend_from_slice(replacement);
        for &b in hay {
            out.push(b);
            out.extend_from_slice(replacement);
        }
        return Some(out);
    }
    if needle == replacement {
        return Some(hay.to_vec());
    }
    if needle.len() == 1 {
        let needle_byte = needle[0];
        if replacement.len() == 1 {
            let mut out = hay.to_vec();
            let repl = replacement[0];
            if repl != needle_byte {
                for byte in &mut out {
                    if *byte == needle_byte {
                        *byte = repl;
                    }
                }
            }
            return Some(out);
        }
        let mut count = 0usize;
        let mut offset = 0usize;
        while let Some(idx) = memchr(needle_byte, &hay[offset..]) {
            count += 1;
            offset += idx + 1;
        }
        let extra = replacement.len().saturating_sub(1) * count;
        let mut out = Vec::with_capacity(hay.len().saturating_add(extra));
        let mut start = 0usize;
        let mut search = 0usize;
        while let Some(idx) = memchr(needle_byte, &hay[search..]) {
            let absolute = search + idx;
            out.extend_from_slice(&hay[start..absolute]);
            out.extend_from_slice(replacement);
            start = absolute + 1;
            search = start;
        }
        out.extend_from_slice(&hay[start..]);
        return Some(out);
    }
    if needle.len() == replacement.len() {
        let mut out = hay.to_vec();
        if needle.len() == 2 {
            let n0 = needle[0];
            let n1 = needle[1];
            let r0 = replacement[0];
            let r1 = replacement[1];
            let mut i = 0usize;
            while i + 1 < hay.len() {
                if hay[i] == n0 && hay[i + 1] == n1 {
                    out[i] = r0;
                    out[i + 1] = r1;
                    i += 2;
                } else {
                    i += 1;
                }
            }
            return Some(out);
        }
        let finder = memmem::Finder::new(needle);
        for idx in finder.find_iter(hay) {
            out[idx..idx + needle.len()].copy_from_slice(replacement);
        }
        return Some(out);
    }
    let mut out = Vec::with_capacity(hay.len());
    let finder = memmem::Finder::new(needle);
    let mut start = 0usize;
    for idx in finder.find_iter(hay) {
        out.extend_from_slice(&hay[start..idx]);
        out.extend_from_slice(replacement);
        start = idx + needle.len();
    }
    out.extend_from_slice(&hay[start..]);
    Some(out)
}

pub(crate) fn replace_bytes_impl_limit(
    hay: &[u8],
    needle: &[u8],
    replacement: &[u8],
    limit: usize,
) -> Vec<u8> {
    if limit == 0 {
        return hay.to_vec();
    }
    if needle.is_empty() {
        let max_inserts = limit.min(hay.len() + 1);
        if max_inserts == 0 {
            return hay.to_vec();
        }
        let extra = replacement.len().saturating_mul(max_inserts);
        let mut out = Vec::with_capacity(hay.len().saturating_add(extra));
        let mut inserted = 0usize;
        if inserted < max_inserts {
            out.extend_from_slice(replacement);
            inserted += 1;
        }
        for &byte in hay {
            out.push(byte);
            if inserted < max_inserts {
                out.extend_from_slice(replacement);
                inserted += 1;
            }
        }
        return out;
    }
    let mut out = Vec::with_capacity(hay.len());
    let finder = memmem::Finder::new(needle);
    let mut start = 0usize;
    for (replaced, idx) in finder.find_iter(hay).enumerate() {
        if replaced >= limit {
            break;
        }
        out.extend_from_slice(&hay[start..idx]);
        out.extend_from_slice(replacement);
        start = idx + needle.len();
    }
    out.extend_from_slice(&hay[start..]);
    out
}

pub(crate) fn replace_string_impl(
    hay_ptr: *mut u8,
    hay_bytes: &[u8],
    needle_bytes: &[u8],
    replacement_bytes: &[u8],
    count: i64,
) -> Option<Vec<u8>> {
    if count == 0 {
        return Some(hay_bytes.to_vec());
    }
    if count > 0 {
        let limit = count as usize;
        if needle_bytes.is_empty() {
            if hay_bytes.is_ascii() && replacement_bytes.is_ascii() {
                return Some(replace_bytes_impl_limit(
                    hay_bytes,
                    needle_bytes,
                    replacement_bytes,
                    limit,
                ));
            }
            let hay_str = unsafe { std::str::from_utf8_unchecked(hay_bytes) };
            let replacement_str = unsafe { std::str::from_utf8_unchecked(replacement_bytes) };
            let codepoints =
                utf8_codepoint_count_cached(hay_bytes, Some(hay_ptr as usize)) as usize;
            let max_inserts = limit.min(codepoints + 1);
            if max_inserts == 0 {
                return Some(hay_bytes.to_vec());
            }
            let mut out =
                String::with_capacity(hay_str.len() + replacement_str.len() * max_inserts);
            let mut inserted = 0usize;
            if inserted < max_inserts {
                out.push_str(replacement_str);
                inserted += 1;
            }
            for ch in hay_str.chars() {
                out.push(ch);
                if inserted < max_inserts {
                    out.push_str(replacement_str);
                    inserted += 1;
                }
            }
            return Some(out.into_bytes());
        }
        return Some(replace_bytes_impl_limit(
            hay_bytes,
            needle_bytes,
            replacement_bytes,
            limit,
        ));
    }
    if needle_bytes.is_empty() {
        if hay_bytes.is_ascii() && replacement_bytes.is_ascii() {
            return replace_bytes_impl(hay_bytes, needle_bytes, replacement_bytes);
        }
        let hay_str = unsafe { std::str::from_utf8_unchecked(hay_bytes) };
        let replacement_str = unsafe { std::str::from_utf8_unchecked(replacement_bytes) };
        let codepoints = utf8_codepoint_count_cached(hay_bytes, Some(hay_ptr as usize)) as usize;
        let mut out =
            String::with_capacity(hay_str.len() + replacement_str.len() * (codepoints + 1));
        out.push_str(replacement_str);
        for ch in hay_str.chars() {
            out.push(ch);
            out.push_str(replacement_str);
        }
        return Some(out.into_bytes());
    }
    replace_bytes_impl(hay_bytes, needle_bytes, replacement_bytes)
}

unsafe fn list_push_owned(list_ptr: *mut u8, val_bits: u64) {
    let elems = seq_vec(list_ptr);
    elems.push(val_bits);
}

fn alloc_list_empty_with_capacity(capacity: usize) -> *mut u8 {
    let cap = capacity.max(MAX_SMALL_LIST);
    alloc_list_with_capacity(&[], cap)
}

const SPLIT_CACHE_MAX_ENTRIES: usize = 8;
const SPLIT_CACHE_MAX_LEN: usize = 32;

struct SplitTokenCacheEntry {
    bits: u64,
    len: usize,
}

fn split_cache_lookup(cache: &[SplitTokenCacheEntry], part: &[u8]) -> Option<u64> {
    if part.len() > SPLIT_CACHE_MAX_LEN {
        return None;
    }
    for entry in cache {
        if entry.len != part.len() {
            continue;
        }
        let obj = obj_from_bits(entry.bits);
        let Some(ptr) = obj.as_ptr() else {
            continue;
        };
        unsafe {
            if object_type_id(ptr) != TYPE_ID_STRING {
                continue;
            }
            let bytes = std::slice::from_raw_parts(string_bytes(ptr), entry.len);
            if bytes == part {
                return Some(entry.bits);
            }
        }
    }
    None
}

fn split_cache_store(cache: &mut Vec<SplitTokenCacheEntry>, bits: u64, len: usize) {
    if len > SPLIT_CACHE_MAX_LEN || cache.len() >= SPLIT_CACHE_MAX_ENTRIES {
        return;
    }
    cache.push(SplitTokenCacheEntry { bits, len });
}

fn split_string_push_part(
    cache: &mut Vec<SplitTokenCacheEntry>,
    list_ptr: *mut u8,
    list_bits: u64,
    part: &[u8],
) -> bool {
    if let Some(bits) = split_cache_lookup(cache, part) {
        inc_ref_bits(bits);
        unsafe {
            list_push_owned(list_ptr, bits);
        }
        return true;
    }
    let ptr = alloc_string(part);
    if ptr.is_null() {
        dec_ref_bits(list_bits);
        return false;
    }
    let bits = MoltObject::from_ptr(ptr).bits();
    unsafe {
        list_push_owned(list_ptr, bits);
    }
    split_cache_store(cache, bits, part.len());
    true
}

fn split_string_bytes_to_list(hay: &[u8], needle: &[u8]) -> Option<u64> {
    let mut cache = Vec::new();
    if needle.len() == 1 {
        let count = memchr::memchr_iter(needle[0], hay).count();
        let list_ptr = alloc_list_empty_with_capacity(count + 1);
        if list_ptr.is_null() {
            return None;
        }
        let list_bits = MoltObject::from_ptr(list_ptr).bits();
        let mut start = 0usize;
        for idx in memchr::memchr_iter(needle[0], hay) {
            let part = &hay[start..idx];
            if !split_string_push_part(&mut cache, list_ptr, list_bits, part) {
                return None;
            }
            start = idx + needle.len();
        }
        let part = &hay[start..];
        if !split_string_push_part(&mut cache, list_ptr, list_bits, part) {
            return None;
        }
        return Some(list_bits);
    }
    let mut indices = Vec::new();
    let finder = memmem::Finder::new(needle);
    for idx in finder.find_iter(hay) {
        indices.push(idx);
    }
    let list_ptr = alloc_list_empty_with_capacity(indices.len() + 1);
    if list_ptr.is_null() {
        return None;
    }
    let list_bits = MoltObject::from_ptr(list_ptr).bits();
    let mut start = 0usize;
    for idx in indices {
        let part = &hay[start..idx];
        if !split_string_push_part(&mut cache, list_ptr, list_bits, part) {
            return None;
        }
        start = idx + needle.len();
    }
    let part = &hay[start..];
    if !split_string_push_part(&mut cache, list_ptr, list_bits, part) {
        return None;
    }
    Some(list_bits)
}

pub(crate) fn split_string_bytes_to_list_maxsplit(
    hay: &[u8],
    needle: &[u8],
    maxsplit: i64,
) -> Option<u64> {
    if maxsplit < 0 {
        return split_string_bytes_to_list(hay, needle);
    }
    let mut cache = Vec::new();
    let list_ptr = alloc_list_empty_with_capacity(4);
    if list_ptr.is_null() {
        return None;
    }
    let list_bits = MoltObject::from_ptr(list_ptr).bits();
    if maxsplit == 0 {
        if !split_string_push_part(&mut cache, list_ptr, list_bits, hay) {
            return None;
        }
        return Some(list_bits);
    }
    let mut start = 0usize;
    let mut splits = 0i64;
    if needle.len() == 1 {
        for idx in memchr::memchr_iter(needle[0], hay) {
            if splits >= maxsplit {
                break;
            }
            let part = &hay[start..idx];
            if !split_string_push_part(&mut cache, list_ptr, list_bits, part) {
                return None;
            }
            start = idx + needle.len();
            splits += 1;
        }
        let part = &hay[start..];
        if !split_string_push_part(&mut cache, list_ptr, list_bits, part) {
            return None;
        }
        return Some(list_bits);
    }
    let finder = memmem::Finder::new(needle);
    for idx in finder.find_iter(hay) {
        if splits >= maxsplit {
            break;
        }
        let part = &hay[start..idx];
        if !split_string_push_part(&mut cache, list_ptr, list_bits, part) {
            return None;
        }
        start = idx + needle.len();
        splits += 1;
    }
    let part = &hay[start..];
    if !split_string_push_part(&mut cache, list_ptr, list_bits, part) {
        return None;
    }
    Some(list_bits)
}

pub(crate) fn rsplit_string_bytes_to_list_maxsplit(
    hay: &[u8],
    needle: &[u8],
    maxsplit: i64,
) -> Option<u64> {
    if maxsplit < 0 {
        return split_string_bytes_to_list(hay, needle);
    }
    let mut cache = Vec::new();
    let list_ptr = alloc_list_empty_with_capacity(4);
    if list_ptr.is_null() {
        return None;
    }
    let list_bits = MoltObject::from_ptr(list_ptr).bits();
    if maxsplit == 0 {
        if !split_string_push_part(&mut cache, list_ptr, list_bits, hay) {
            return None;
        }
        return Some(list_bits);
    }
    let mut end = hay.len();
    let mut splits = 0i64;
    let mut parts: Vec<(usize, usize)> = Vec::new();
    while splits < maxsplit {
        let idx = bytes_rfind_impl(&hay[..end], needle);
        if idx < 0 {
            break;
        }
        let idx = idx as usize;
        parts.push((idx + needle.len(), end));
        end = idx;
        splits += 1;
    }
    parts.push((0, end));
    parts.reverse();
    for (start, end) in parts {
        let part = &hay[start..end];
        if !split_string_push_part(&mut cache, list_ptr, list_bits, part) {
            return None;
        }
    }
    Some(list_bits)
}

pub(crate) fn bytes_strip_range(
    hay: &[u8],
    strip: &[u8],
    left: bool,
    right: bool,
) -> (usize, usize) {
    if strip.is_empty() {
        return (0, hay.len());
    }
    let mut table = [false; 256];
    for &byte in strip {
        table[byte as usize] = true;
    }
    let mut start = 0usize;
    let mut end = hay.len();
    if left {
        while start < end && table[hay[start] as usize] {
            start += 1;
        }
    }
    if right {
        while end > start && table[hay[end - 1] as usize] {
            end -= 1;
        }
    }
    (start, end)
}

fn split_bytes_to_list<F>(hay: &[u8], needle: &[u8], mut alloc: F) -> Option<u64>
where
    F: FnMut(&[u8]) -> *mut u8,
{
    if needle.len() == 1 {
        let count = memchr::memchr_iter(needle[0], hay).count();
        let list_ptr = alloc_list_empty_with_capacity(count + 1);
        if list_ptr.is_null() {
            return None;
        }
        let list_bits = MoltObject::from_ptr(list_ptr).bits();
        let mut start = 0usize;
        for idx in memchr::memchr_iter(needle[0], hay) {
            let part = &hay[start..idx];
            let ptr = alloc(part);
            if ptr.is_null() {
                dec_ref_bits(list_bits);
                return None;
            }
            unsafe {
                list_push_owned(list_ptr, MoltObject::from_ptr(ptr).bits());
            }
            start = idx + needle.len();
        }
        let part = &hay[start..];
        let ptr = alloc(part);
        if ptr.is_null() {
            dec_ref_bits(list_bits);
            return None;
        }
        unsafe {
            list_push_owned(list_ptr, MoltObject::from_ptr(ptr).bits());
        }
        return Some(list_bits);
    }
    let mut indices = Vec::new();
    let finder = memmem::Finder::new(needle);
    for idx in finder.find_iter(hay) {
        indices.push(idx);
    }
    let list_ptr = alloc_list_empty_with_capacity(indices.len() + 1);
    if list_ptr.is_null() {
        return None;
    }
    let list_bits = MoltObject::from_ptr(list_ptr).bits();
    let mut start = 0usize;
    for idx in indices {
        let part = &hay[start..idx];
        let ptr = alloc(part);
        if ptr.is_null() {
            dec_ref_bits(list_bits);
            return None;
        }
        unsafe {
            list_push_owned(list_ptr, MoltObject::from_ptr(ptr).bits());
        }
        start = idx + needle.len();
    }
    let part = &hay[start..];
    let ptr = alloc(part);
    if ptr.is_null() {
        dec_ref_bits(list_bits);
        return None;
    }
    unsafe {
        list_push_owned(list_ptr, MoltObject::from_ptr(ptr).bits());
    }
    Some(list_bits)
}

pub(crate) fn split_bytes_to_list_maxsplit<F>(
    hay: &[u8],
    needle: &[u8],
    maxsplit: i64,
    mut alloc: F,
) -> Option<u64>
where
    F: FnMut(&[u8]) -> *mut u8,
{
    if maxsplit < 0 {
        return split_bytes_to_list(hay, needle, alloc);
    }
    let list_ptr = alloc_list_empty_with_capacity(4);
    if list_ptr.is_null() {
        return None;
    }
    let list_bits = MoltObject::from_ptr(list_ptr).bits();
    if maxsplit == 0 {
        let ptr = alloc(hay);
        if ptr.is_null() {
            dec_ref_bits(list_bits);
            return None;
        }
        unsafe {
            list_push_owned(list_ptr, MoltObject::from_ptr(ptr).bits());
        }
        return Some(list_bits);
    }
    let mut start = 0usize;
    let mut splits = 0i64;
    if needle.len() == 1 {
        for idx in memchr::memchr_iter(needle[0], hay) {
            if splits >= maxsplit {
                break;
            }
            let part = &hay[start..idx];
            let ptr = alloc(part);
            if ptr.is_null() {
                dec_ref_bits(list_bits);
                return None;
            }
            unsafe {
                list_push_owned(list_ptr, MoltObject::from_ptr(ptr).bits());
            }
            start = idx + needle.len();
            splits += 1;
        }
        let part = &hay[start..];
        let ptr = alloc(part);
        if ptr.is_null() {
            dec_ref_bits(list_bits);
            return None;
        }
        unsafe {
            list_push_owned(list_ptr, MoltObject::from_ptr(ptr).bits());
        }
        return Some(list_bits);
    }
    let finder = memmem::Finder::new(needle);
    for idx in finder.find_iter(hay) {
        if splits >= maxsplit {
            break;
        }
        let part = &hay[start..idx];
        let ptr = alloc(part);
        if ptr.is_null() {
            dec_ref_bits(list_bits);
            return None;
        }
        unsafe {
            list_push_owned(list_ptr, MoltObject::from_ptr(ptr).bits());
        }
        start = idx + needle.len();
        splits += 1;
    }
    let part = &hay[start..];
    let ptr = alloc(part);
    if ptr.is_null() {
        dec_ref_bits(list_bits);
        return None;
    }
    unsafe {
        list_push_owned(list_ptr, MoltObject::from_ptr(ptr).bits());
    }
    Some(list_bits)
}

pub(crate) fn rsplit_bytes_to_list_maxsplit<F>(
    hay: &[u8],
    needle: &[u8],
    maxsplit: i64,
    mut alloc: F,
) -> Option<u64>
where
    F: FnMut(&[u8]) -> *mut u8,
{
    if maxsplit < 0 {
        return split_bytes_to_list(hay, needle, alloc);
    }
    let list_ptr = alloc_list_empty_with_capacity(4);
    if list_ptr.is_null() {
        return None;
    }
    let list_bits = MoltObject::from_ptr(list_ptr).bits();
    if maxsplit == 0 {
        let ptr = alloc(hay);
        if ptr.is_null() {
            dec_ref_bits(list_bits);
            return None;
        }
        unsafe {
            list_push_owned(list_ptr, MoltObject::from_ptr(ptr).bits());
        }
        return Some(list_bits);
    }
    let mut end = hay.len();
    let mut splits = 0i64;
    let mut parts: Vec<(usize, usize)> = Vec::new();
    while splits < maxsplit {
        let idx = bytes_rfind_impl(&hay[..end], needle);
        if idx < 0 {
            break;
        }
        let idx = idx as usize;
        parts.push((idx + needle.len(), end));
        end = idx;
        splits += 1;
    }
    parts.push((0, end));
    parts.reverse();
    for (start, end) in parts {
        let part = &hay[start..end];
        let ptr = alloc(part);
        if ptr.is_null() {
            dec_ref_bits(list_bits);
            return None;
        }
        unsafe {
            list_push_owned(list_ptr, MoltObject::from_ptr(ptr).bits());
        }
    }
    Some(list_bits)
}

fn split_bytes_whitespace_to_list<F>(hay: &[u8], mut alloc: F) -> Option<u64>
where
    F: FnMut(&[u8]) -> *mut u8,
{
    let list_ptr = alloc_list_empty_with_capacity(4);
    if list_ptr.is_null() {
        return None;
    }
    let list_bits = MoltObject::from_ptr(list_ptr).bits();
    let mut start: Option<usize> = None;
    for (idx, byte) in hay.iter().enumerate() {
        if byte.is_ascii_whitespace() {
            if let Some(s) = start {
                let part = &hay[s..idx];
                let ptr = alloc(part);
                if ptr.is_null() {
                    dec_ref_bits(list_bits);
                    return None;
                }
                unsafe {
                    list_push_owned(list_ptr, MoltObject::from_ptr(ptr).bits());
                }
                start = None;
            }
        } else if start.is_none() {
            start = Some(idx);
        }
    }
    if let Some(s) = start {
        let part = &hay[s..];
        let ptr = alloc(part);
        if ptr.is_null() {
            dec_ref_bits(list_bits);
            return None;
        }
        unsafe {
            list_push_owned(list_ptr, MoltObject::from_ptr(ptr).bits());
        }
    }
    Some(list_bits)
}

pub(crate) fn split_bytes_whitespace_to_list_maxsplit<F>(
    hay: &[u8],
    maxsplit: i64,
    mut alloc: F,
) -> Option<u64>
where
    F: FnMut(&[u8]) -> *mut u8,
{
    if maxsplit < 0 {
        return split_bytes_whitespace_to_list(hay, alloc);
    }
    let list_ptr = alloc_list_empty_with_capacity(4);
    if list_ptr.is_null() {
        return None;
    }
    let list_bits = MoltObject::from_ptr(list_ptr).bits();
    let mut idx = 0usize;
    while idx < hay.len() && hay[idx].is_ascii_whitespace() {
        idx += 1;
    }
    if idx >= hay.len() {
        return Some(list_bits);
    }
    if maxsplit == 0 {
        let ptr = alloc(&hay[idx..]);
        if ptr.is_null() {
            dec_ref_bits(list_bits);
            return None;
        }
        unsafe {
            list_push_owned(list_ptr, MoltObject::from_ptr(ptr).bits());
        }
        return Some(list_bits);
    }
    let mut start = idx;
    let mut splits = 0i64;
    while idx < hay.len() {
        if hay[idx].is_ascii_whitespace() {
            let part = &hay[start..idx];
            let ptr = alloc(part);
            if ptr.is_null() {
                dec_ref_bits(list_bits);
                return None;
            }
            unsafe {
                list_push_owned(list_ptr, MoltObject::from_ptr(ptr).bits());
            }
            splits += 1;
            idx += 1;
            while idx < hay.len() && hay[idx].is_ascii_whitespace() {
                idx += 1;
            }
            if idx >= hay.len() {
                return Some(list_bits);
            }
            if splits >= maxsplit {
                let ptr = alloc(&hay[idx..]);
                if ptr.is_null() {
                    dec_ref_bits(list_bits);
                    return None;
                }
                unsafe {
                    list_push_owned(list_ptr, MoltObject::from_ptr(ptr).bits());
                }
                return Some(list_bits);
            }
            start = idx;
        } else {
            idx += 1;
        }
    }
    let part = &hay[start..];
    let ptr = alloc(part);
    if ptr.is_null() {
        dec_ref_bits(list_bits);
        return None;
    }
    unsafe {
        list_push_owned(list_ptr, MoltObject::from_ptr(ptr).bits());
    }
    Some(list_bits)
}

pub(crate) fn rsplit_bytes_whitespace_to_list_maxsplit<F>(
    hay: &[u8],
    maxsplit: i64,
    mut alloc: F,
) -> Option<u64>
where
    F: FnMut(&[u8]) -> *mut u8,
{
    if maxsplit < 0 {
        return split_bytes_whitespace_to_list(hay, alloc);
    }
    let list_ptr = alloc_list_empty_with_capacity(4);
    if list_ptr.is_null() {
        return None;
    }
    let list_bits = MoltObject::from_ptr(list_ptr).bits();
    let mut end = hay.len();
    while end > 0 && hay[end - 1].is_ascii_whitespace() {
        end -= 1;
    }
    if end == 0 {
        return Some(list_bits);
    }
    if maxsplit == 0 {
        let mut start = 0usize;
        while start < end && hay[start].is_ascii_whitespace() {
            start += 1;
        }
        let ptr = alloc(&hay[start..end]);
        if ptr.is_null() {
            dec_ref_bits(list_bits);
            return None;
        }
        unsafe {
            list_push_owned(list_ptr, MoltObject::from_ptr(ptr).bits());
        }
        return Some(list_bits);
    }
    let mut parts: Vec<(usize, usize)> = Vec::new();
    let mut splits = 0i64;
    let mut idx = end;
    while splits < maxsplit {
        while idx > 0 && !hay[idx - 1].is_ascii_whitespace() {
            idx -= 1;
        }
        if idx == 0 {
            break;
        }
        parts.push((idx, end));
        splits += 1;
        while idx > 0 && hay[idx - 1].is_ascii_whitespace() {
            idx -= 1;
        }
        end = idx;
        if end == 0 {
            break;
        }
    }
    if end > 0 {
        let mut start = 0usize;
        while start < end && hay[start].is_ascii_whitespace() {
            start += 1;
        }
        if start < end {
            parts.push((start, end));
        }
    }
    parts.reverse();
    for (start, end) in parts {
        let ptr = alloc(&hay[start..end]);
        if ptr.is_null() {
            dec_ref_bits(list_bits);
            return None;
        }
        unsafe {
            list_push_owned(list_ptr, MoltObject::from_ptr(ptr).bits());
        }
    }
    Some(list_bits)
}

fn is_linebreak_byte(byte: u8) -> bool {
    matches!(byte, b'\n' | b'\r')
}

pub(crate) fn splitlines_bytes_to_list<F>(hay: &[u8], keepends: bool, mut alloc: F) -> Option<u64>
where
    F: FnMut(&[u8]) -> *mut u8,
{
    let list_ptr = alloc_list_empty_with_capacity(4);
    if list_ptr.is_null() {
        return None;
    }
    let list_bits = MoltObject::from_ptr(list_ptr).bits();
    let mut start = 0usize;
    let mut idx = 0usize;
    while idx < hay.len() {
        let byte = hay[idx];
        if is_linebreak_byte(byte) {
            let mut break_end = idx + 1;
            if byte == b'\r' && break_end < hay.len() && hay[break_end] == b'\n' {
                break_end += 1;
            }
            let end = idx;
            let part = if keepends {
                &hay[start..break_end]
            } else {
                &hay[start..end]
            };
            let ptr = alloc(part);
            if ptr.is_null() {
                dec_ref_bits(list_bits);
                return None;
            }
            unsafe {
                list_push_owned(list_ptr, MoltObject::from_ptr(ptr).bits());
            }
            start = break_end;
            idx = break_end;
            continue;
        }
        idx += 1;
    }
    if start < hay.len() {
        let part = &hay[start..];
        let ptr = alloc(part);
        if ptr.is_null() {
            dec_ref_bits(list_bits);
            return None;
        }
        unsafe {
            list_push_owned(list_ptr, MoltObject::from_ptr(ptr).bits());
        }
    }
    Some(list_bits)
}

fn split_string_whitespace_to_list(hay: &[u8]) -> Option<u64> {
    let Ok(hay_str) = std::str::from_utf8(hay) else {
        return None;
    };
    let list_ptr = alloc_list_empty_with_capacity(4);
    if list_ptr.is_null() {
        return None;
    }
    let list_bits = MoltObject::from_ptr(list_ptr).bits();
    for part in hay_str.split_whitespace() {
        let ptr = alloc_string(part.as_bytes());
        if ptr.is_null() {
            dec_ref_bits(list_bits);
            return None;
        }
        unsafe {
            list_push_owned(list_ptr, MoltObject::from_ptr(ptr).bits());
        }
    }
    Some(list_bits)
}

pub(crate) fn split_string_whitespace_to_list_maxsplit(hay: &str, maxsplit: i64) -> Option<u64> {
    if maxsplit < 0 {
        return split_string_whitespace_to_list(hay.as_bytes());
    }
    let list_ptr = alloc_list_empty_with_capacity(4);
    if list_ptr.is_null() {
        return None;
    }
    let list_bits = MoltObject::from_ptr(list_ptr).bits();
    let bytes = hay.as_bytes();
    let mut start_opt = None;
    for (idx, ch) in hay.char_indices() {
        if !ch.is_whitespace() {
            start_opt = Some(idx);
            break;
        }
    }
    let Some(mut start) = start_opt else {
        return Some(list_bits);
    };
    if maxsplit == 0 {
        let ptr = alloc_string(&bytes[start..]);
        if ptr.is_null() {
            dec_ref_bits(list_bits);
            return None;
        }
        unsafe {
            list_push_owned(list_ptr, MoltObject::from_ptr(ptr).bits());
        }
        return Some(list_bits);
    }
    let mut splits = 0i64;
    let mut iter = hay.char_indices();
    while let Some((idx, ch)) = iter.next() {
        if idx < start {
            continue;
        }
        if ch.is_whitespace() {
            let part = &bytes[start..idx];
            let ptr = alloc_string(part);
            if ptr.is_null() {
                dec_ref_bits(list_bits);
                return None;
            }
            unsafe {
                list_push_owned(list_ptr, MoltObject::from_ptr(ptr).bits());
            }
            splits += 1;
            if splits >= maxsplit {
                let mut rest_start = None;
                for (j, ch2) in iter.by_ref() {
                    if !ch2.is_whitespace() {
                        rest_start = Some(j);
                        break;
                    }
                }
                if let Some(rest_start) = rest_start {
                    let ptr = alloc_string(&bytes[rest_start..]);
                    if ptr.is_null() {
                        dec_ref_bits(list_bits);
                        return None;
                    }
                    unsafe {
                        list_push_owned(list_ptr, MoltObject::from_ptr(ptr).bits());
                    }
                }
                return Some(list_bits);
            }
            let mut next_start = None;
            for (j, ch2) in iter.by_ref() {
                if !ch2.is_whitespace() {
                    next_start = Some(j);
                    break;
                }
            }
            if let Some(next_start) = next_start {
                start = next_start;
            } else {
                return Some(list_bits);
            }
        }
    }
    let ptr = alloc_string(&bytes[start..]);
    if ptr.is_null() {
        dec_ref_bits(list_bits);
        return None;
    }
    unsafe {
        list_push_owned(list_ptr, MoltObject::from_ptr(ptr).bits());
    }
    Some(list_bits)
}

pub(crate) fn rsplit_string_whitespace_to_list_maxsplit(hay: &str, maxsplit: i64) -> Option<u64> {
    if maxsplit < 0 {
        return split_string_whitespace_to_list(hay.as_bytes());
    }
    let list_ptr = alloc_list_empty_with_capacity(4);
    if list_ptr.is_null() {
        return None;
    }
    let list_bits = MoltObject::from_ptr(list_ptr).bits();
    let bytes = hay.as_bytes();
    let indices: Vec<(usize, char)> = hay.char_indices().collect();
    let mut end = hay.len();
    let mut pos = indices.len();
    while pos > 0 {
        let (_byte_idx, ch) = indices[pos - 1];
        if ch.is_whitespace() {
            end = indices[pos - 1].0;
            pos -= 1;
        } else {
            break;
        }
    }
    if end == 0 {
        return Some(list_bits);
    }
    if maxsplit == 0 {
        let mut start_opt = None;
        for (byte_idx, ch) in indices.iter() {
            if *byte_idx >= end {
                break;
            }
            if !ch.is_whitespace() {
                start_opt = Some(*byte_idx);
                break;
            }
        }
        if let Some(start) = start_opt {
            let ptr = alloc_string(&bytes[start..end]);
            if ptr.is_null() {
                dec_ref_bits(list_bits);
                return None;
            }
            unsafe {
                list_push_owned(list_ptr, MoltObject::from_ptr(ptr).bits());
            }
        }
        return Some(list_bits);
    }
    let mut parts: Vec<(usize, usize)> = Vec::new();
    let mut splits = 0i64;
    while splits < maxsplit {
        while pos > 0 {
            let (byte_idx, ch) = indices[pos - 1];
            if byte_idx >= end {
                pos -= 1;
                continue;
            }
            if !ch.is_whitespace() {
                pos -= 1;
                continue;
            }
            break;
        }
        if pos == 0 {
            break;
        }
        let mut ws_start = pos;
        while ws_start > 0 {
            let (byte_idx, ch) = indices[ws_start - 1];
            if byte_idx >= end {
                ws_start -= 1;
                continue;
            }
            if ch.is_whitespace() {
                ws_start -= 1;
            } else {
                break;
            }
        }
        let (ws_last_idx, ws_last_ch) = indices[pos - 1];
        let part_start = ws_last_idx + ws_last_ch.len_utf8();
        parts.push((part_start, end));
        end = indices[ws_start].0;
        splits += 1;
        pos = ws_start;
        if end == 0 {
            break;
        }
    }
    if end > 0 {
        let mut start_opt = None;
        for (byte_idx, ch) in indices.iter() {
            if *byte_idx >= end {
                break;
            }
            if !ch.is_whitespace() {
                start_opt = Some(*byte_idx);
                break;
            }
        }
        if let Some(start) = start_opt {
            parts.push((start, end));
        }
    }
    parts.reverse();
    for (start, end) in parts {
        let ptr = alloc_string(&bytes[start..end]);
        if ptr.is_null() {
            dec_ref_bits(list_bits);
            return None;
        }
        unsafe {
            list_push_owned(list_ptr, MoltObject::from_ptr(ptr).bits());
        }
    }
    Some(list_bits)
}

fn is_linebreak_char(ch: char) -> bool {
    matches!(
        ch,
        '\n' | '\r'
            | '\x0b'
            | '\x0c'
            | '\x1c'
            | '\x1d'
            | '\x1e'
            | '\u{85}'
            | '\u{2028}'
            | '\u{2029}'
    )
}

pub(crate) fn splitlines_string_to_list(hay_str: &str, keepends: bool) -> Option<u64> {
    let list_ptr = alloc_list_empty_with_capacity(4);
    if list_ptr.is_null() {
        return None;
    }
    let list_bits = MoltObject::from_ptr(list_ptr).bits();
    let mut start = 0usize;
    let mut iter = hay_str.char_indices().peekable();
    while let Some((idx, ch)) = iter.next() {
        if !is_linebreak_char(ch) {
            continue;
        }
        let mut break_end = idx + ch.len_utf8();
        if ch == '\r' {
            if let Some(&(next_idx, next_ch)) = iter.peek() {
                if next_ch == '\n' {
                    iter.next();
                    break_end = next_idx + next_ch.len_utf8();
                }
            }
        }
        let end = idx;
        let part = if keepends {
            &hay_str[start..break_end]
        } else {
            &hay_str[start..end]
        };
        let ptr = alloc_string(part.as_bytes());
        if ptr.is_null() {
            dec_ref_bits(list_bits);
            return None;
        }
        unsafe {
            list_push_owned(list_ptr, MoltObject::from_ptr(ptr).bits());
        }
        start = break_end;
    }
    if start < hay_str.len() {
        let part = &hay_str[start..];
        let ptr = alloc_string(part.as_bytes());
        if ptr.is_null() {
            dec_ref_bits(list_bits);
            return None;
        }
        unsafe {
            list_push_owned(list_ptr, MoltObject::from_ptr(ptr).bits());
        }
    }
    Some(list_bits)
}

#[cfg(not(target_arch = "wasm32"))]
fn memchr_simd128(_needle: u8, _hay: &[u8]) -> (bool, Option<usize>) {
    (false, None)
}
