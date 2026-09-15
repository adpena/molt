use std::cell::RefCell;
#[cfg(target_arch = "wasm32")]
use std::sync::Mutex;
#[cfg(target_arch = "wasm32")]
use std::sync::OnceLock;

use crate::{PyToken, dec_ref_bits, inc_ref_bits};

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

    fn insert(&mut self, data_ptr: usize, len: usize, bits: u64) -> Option<u64> {
        let idx = Self::slot_index(data_ptr, len);
        let previous = self.slots[idx].take().map(|entry| entry.bits);
        self.slots[idx] = Some(ConstDataCacheEntry {
            data_ptr,
            len,
            bits,
        });
        previous
    }

    fn take_entries(&mut self) -> [Option<ConstDataCacheEntry>; CONST_DATA_CACHE_SIZE] {
        std::mem::replace(&mut self.slots, ConstDataCache::new().slots)
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
    py: &PyToken<'_>,
    kind: ConstDataLiteralKind,
    data_ptr: usize,
    len: usize,
) -> Option<u64> {
    // Return an owned result before releasing shared WASM cache custody; a
    // different thread may evict this slot as soon as its mutex is released.
    with_cache(kind, |cache| {
        cache
            .lookup(data_ptr, len)
            .inspect(|&bits| inc_ref_bits(py, bits))
    })
}

pub(crate) fn const_data_literal_insert(
    py: &PyToken<'_>,
    kind: ConstDataLiteralKind,
    data_ptr: usize,
    len: usize,
    bits: u64,
) {
    // A bounded cache owns one ordinary reference, not the object's physical
    // lifetime. Eviction can reclaim an otherwise unused literal; callers own
    // independent results. Release after the TLS borrow or WASM mutex is gone.
    inc_ref_bits(py, bits);
    let previous = with_cache(kind, |cache| cache.insert(data_ptr, len, bits));
    if let Some(previous) = previous {
        dec_ref_bits(py, previous);
    }
}

/// Detach this execution context's owned literal-cache edges and report whether
/// any existed.
pub(crate) fn clear_const_data_literal_caches(py: &PyToken<'_>) -> bool {
    let mut detached = false;
    for kind in [
        ConstDataLiteralKind::String,
        ConstDataLiteralKind::Bytes,
        ConstDataLiteralKind::BigInt,
    ] {
        detached |= clear_const_data_literal_cache(py, kind);
    }
    detached
}

fn clear_const_data_literal_cache(py: &PyToken<'_>, kind: ConstDataLiteralKind) -> bool {
    #[cfg(target_arch = "wasm32")]
    {
        let previous = match kind {
            ConstDataLiteralKind::String => {
                let cache = CONST_STR_WASM.get_or_init(|| Mutex::new(ConstDataCache::new()));
                cache.lock().unwrap().take_entries()
            }
            ConstDataLiteralKind::Bytes => {
                let cache = CONST_BYTES_WASM.get_or_init(|| Mutex::new(ConstDataCache::new()));
                cache.lock().unwrap().take_entries()
            }
            ConstDataLiteralKind::BigInt => {
                let cache = CONST_BIGINT_WASM.get_or_init(|| Mutex::new(ConstDataCache::new()));
                cache.lock().unwrap().take_entries()
            }
        };
        let mut detached = false;
        for entry in previous.into_iter().flatten() {
            detached = true;
            dec_ref_bits(py, entry.bits);
        }
        detached
    }
    #[cfg(not(target_arch = "wasm32"))]
    {
        let previous = match kind {
            ConstDataLiteralKind::String => CONST_STR_TLS
                .try_with(|cell| cell.borrow_mut().take_entries())
                .ok(),
            ConstDataLiteralKind::Bytes => CONST_BYTES_TLS
                .try_with(|cell| cell.borrow_mut().take_entries())
                .ok(),
            ConstDataLiteralKind::BigInt => CONST_BIGINT_TLS
                .try_with(|cell| cell.borrow_mut().take_entries())
                .ok(),
        };
        let mut detached = false;
        if let Some(previous) = previous {
            for entry in previous.into_iter().flatten() {
                detached = true;
                dec_ref_bits(py, entry.bits);
            }
        }
        detached
    }
}
