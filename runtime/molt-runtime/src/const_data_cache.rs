use std::cell::RefCell;
#[cfg(target_arch = "wasm32")]
use std::sync::Mutex;
#[cfg(target_arch = "wasm32")]
use std::sync::OnceLock;

use crate::{PyToken, dec_ref_bits, inc_ref_bits, obj_from_bits};

// `const_str`, `const_bytes`, and heap `const_bigint` IR ops call runtime
// constructors with pointers into immutable compiler-emitted data segments.
// The pointer+len pair is therefore literal identity within a process.
const CONST_DATA_CACHE_SIZE: usize = 32; // must be power of 2

#[derive(Clone, Copy)]
pub(crate) enum ConstDataLiteralKind {
    String,
    Bytes,
    BigInt,
}

struct ConstDataCacheEntry {
    data_ptr: usize,
    len: usize,
    bits: u64,
}

struct ConstDataCache {
    slots: [Option<ConstDataCacheEntry>; CONST_DATA_CACHE_SIZE],
}

impl ConstDataCache {
    const fn new() -> Self {
        const NONE: Option<ConstDataCacheEntry> = None;
        Self {
            slots: [NONE; CONST_DATA_CACHE_SIZE],
        }
    }

    #[inline]
    fn slot_index(data_ptr: usize, len: usize) -> usize {
        let h = data_ptr.wrapping_mul(0x9e37_79b9) ^ len;
        h & (CONST_DATA_CACHE_SIZE - 1)
    }

    fn lookup(&self, data_ptr: usize, len: usize) -> Option<u64> {
        let idx = Self::slot_index(data_ptr, len);
        self.slots[idx]
            .as_ref()
            .filter(|entry| entry.data_ptr == data_ptr && entry.len == len)
            .map(|entry| entry.bits)
    }

    fn insert(&mut self, py: &PyToken<'_>, data_ptr: usize, len: usize, bits: u64) {
        let idx = Self::slot_index(data_ptr, len);
        if let Some(prev) = self.slots[idx].take() {
            dec_ref_bits(py, prev.bits);
        }
        inc_ref_bits(py, bits);
        mark_bits_immortal(bits);
        self.slots[idx] = Some(ConstDataCacheEntry {
            data_ptr,
            len,
            bits,
        });
    }

    fn clear(&mut self, py: &PyToken<'_>) {
        for slot in self.slots.iter_mut() {
            if let Some(prev) = slot.take() {
                dec_ref_bits(py, prev.bits);
            }
        }
    }
}

#[cfg(not(target_arch = "wasm32"))]
thread_local! {
    static CONST_STR_TLS: RefCell<ConstDataCache> = const { RefCell::new(ConstDataCache::new()) };
    static CONST_BYTES_TLS: RefCell<ConstDataCache> = const { RefCell::new(ConstDataCache::new()) };
    static CONST_BIGINT_TLS: RefCell<ConstDataCache> = const { RefCell::new(ConstDataCache::new()) };
}

#[cfg(target_arch = "wasm32")]
static CONST_STR_WASM: OnceLock<Mutex<ConstDataCache>> = OnceLock::new();
#[cfg(target_arch = "wasm32")]
static CONST_BYTES_WASM: OnceLock<Mutex<ConstDataCache>> = OnceLock::new();
#[cfg(target_arch = "wasm32")]
static CONST_BIGINT_WASM: OnceLock<Mutex<ConstDataCache>> = OnceLock::new();

fn mark_bits_immortal(bits: u64) {
    let obj = obj_from_bits(bits);
    if let Some(ptr) = obj.as_ptr() {
        unsafe {
            let header = crate::object::header_from_obj_ptr(ptr);
            (*header).flags |= crate::object::HEADER_FLAG_IMMORTAL;
        }
    }
}

fn with_cache<R>(kind: ConstDataLiteralKind, f: impl FnOnce(&mut ConstDataCache) -> R) -> R {
    #[cfg(target_arch = "wasm32")]
    {
        let cache = match kind {
            ConstDataLiteralKind::String => {
                CONST_STR_WASM.get_or_init(|| Mutex::new(ConstDataCache::new()))
            }
            ConstDataLiteralKind::Bytes => {
                CONST_BYTES_WASM.get_or_init(|| Mutex::new(ConstDataCache::new()))
            }
            ConstDataLiteralKind::BigInt => {
                CONST_BIGINT_WASM.get_or_init(|| Mutex::new(ConstDataCache::new()))
            }
        };
        let mut guard = cache.lock().unwrap();
        return f(&mut guard);
    }
    #[cfg(not(target_arch = "wasm32"))]
    {
        match kind {
            ConstDataLiteralKind::String => CONST_STR_TLS.with(|cell| f(&mut cell.borrow_mut())),
            ConstDataLiteralKind::Bytes => CONST_BYTES_TLS.with(|cell| f(&mut cell.borrow_mut())),
            ConstDataLiteralKind::BigInt => CONST_BIGINT_TLS.with(|cell| f(&mut cell.borrow_mut())),
        }
    }
}

pub(crate) fn const_data_literal_lookup(
    kind: ConstDataLiteralKind,
    data_ptr: usize,
    len: usize,
) -> Option<u64> {
    with_cache(kind, |cache| cache.lookup(data_ptr, len))
}

pub(crate) fn const_data_literal_insert(
    py: &PyToken<'_>,
    kind: ConstDataLiteralKind,
    data_ptr: usize,
    len: usize,
    bits: u64,
) {
    with_cache(kind, |cache| cache.insert(py, data_ptr, len, bits));
}

pub(crate) fn clear_const_data_literal_caches(py: &PyToken<'_>) {
    for kind in [
        ConstDataLiteralKind::String,
        ConstDataLiteralKind::Bytes,
        ConstDataLiteralKind::BigInt,
    ] {
        clear_const_data_literal_cache(py, kind);
    }
}

fn clear_const_data_literal_cache(py: &PyToken<'_>, kind: ConstDataLiteralKind) {
    #[cfg(target_arch = "wasm32")]
    {
        match kind {
            ConstDataLiteralKind::String => {
                let cache = CONST_STR_WASM.get_or_init(|| Mutex::new(ConstDataCache::new()));
                cache.lock().unwrap().clear(py);
            }
            ConstDataLiteralKind::Bytes => {
                let cache = CONST_BYTES_WASM.get_or_init(|| Mutex::new(ConstDataCache::new()));
                cache.lock().unwrap().clear(py);
            }
            ConstDataLiteralKind::BigInt => {
                let cache = CONST_BIGINT_WASM.get_or_init(|| Mutex::new(ConstDataCache::new()));
                cache.lock().unwrap().clear(py);
            }
        }
    }
    #[cfg(not(target_arch = "wasm32"))]
    {
        match kind {
            ConstDataLiteralKind::String => {
                let _ = CONST_STR_TLS.try_with(|cell| cell.borrow_mut().clear(py));
            }
            ConstDataLiteralKind::Bytes => {
                let _ = CONST_BYTES_TLS.try_with(|cell| cell.borrow_mut().clear(py));
            }
            ConstDataLiteralKind::BigInt => {
                let _ = CONST_BIGINT_TLS.try_with(|cell| cell.borrow_mut().clear(py));
            }
        }
    }
}
