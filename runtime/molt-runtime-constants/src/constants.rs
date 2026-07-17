#![allow(dead_code)]

#[cfg(target_arch = "wasm32")]
use std::sync::OnceLock;
use std::sync::atomic::AtomicU64;

// Keep in sync with MOLT_BIND_KIND_OPEN in src/molt/frontend/__init__.py.
pub const BIND_KIND_OPEN: i64 = 1;
pub const BIND_KIND_CAPI_METHOD: i64 = 2;
pub const BIND_KIND_PACKED_BUILTIN: i64 = 3;
pub const BIND_KIND_TYPE_NEW_INIT: i64 = 4;

include!(concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/../wasm_table_layout.inc"
));

#[cfg(target_arch = "wasm32")]
static WASM_TABLE_BASE_RUNTIME: AtomicU64 = AtomicU64::new(WASM_TABLE_BASE_FALLBACK);

#[cfg(target_arch = "wasm32")]
fn wasm_table_base_from_env() -> Option<u64> {
    static ENV_BASE: OnceLock<Option<u64>> = OnceLock::new();
    *ENV_BASE.get_or_init(|| {
        std::env::var("MOLT_WASM_TABLE_BASE")
            .ok()
            .and_then(|raw| raw.parse::<u64>().ok())
            .filter(|base| *base > 0)
    })
}

#[cfg(target_arch = "wasm32")]
pub fn wasm_table_base() -> u64 {
    let base = WASM_TABLE_BASE_RUNTIME.load(std::sync::atomic::Ordering::Relaxed);
    if base > 0 && base != WASM_TABLE_BASE_FALLBACK {
        base
    } else if let Some(env_base) = wasm_table_base_from_env() {
        WASM_TABLE_BASE_RUNTIME.store(env_base, std::sync::atomic::Ordering::Relaxed);
        env_base
    } else if base > 0 {
        base
    } else {
        WASM_TABLE_BASE_FALLBACK
    }
}

#[cfg(target_arch = "wasm32")]
#[unsafe(no_mangle)]
pub extern "C" fn molt_set_wasm_table_base(base: u64) {
    if base > 0 {
        WASM_TABLE_BASE_RUNTIME.store(base, std::sync::atomic::Ordering::Relaxed);
    }
}

#[cfg(not(target_arch = "wasm32"))]
#[inline]
#[allow(dead_code)]
pub const fn wasm_table_base() -> u64 {
    WASM_TABLE_BASE_FALLBACK
}

#[inline]
pub const fn wasm_table_base_fallback() -> u64 {
    WASM_TABLE_BASE_FALLBACK
}

pub const MAX_SMALL_LIST: usize = 16;
pub const ITER_EXHAUSTED: usize = usize::MAX;

pub const GEN_SEND_OFFSET: usize = 0;
pub const GEN_THROW_OFFSET: usize = 8;
pub const GEN_CLOSED_OFFSET: usize = 16;
pub const GEN_EXC_DEPTH_OFFSET: usize = 24;
pub const GEN_YIELD_FROM_OFFSET: usize = 32;

pub const ASYNCGEN_GEN_OFFSET: usize = 0;
pub const ASYNCGEN_RUNNING_OFFSET: usize = 8;
pub const ASYNCGEN_PENDING_OFFSET: usize = 16;
pub const ASYNCGEN_FIRSTITER_OFFSET: usize = 24;
pub const ASYNCGEN_CONTROL_SIZE: usize = 32;
pub const ASYNCGEN_OP_ANEXT: i64 = 0;
pub const ASYNCGEN_OP_ASEND: i64 = 1;
pub const ASYNCGEN_OP_ATHROW: i64 = 2;
pub const ASYNCGEN_OP_ACLOSE: i64 = 3;

pub static CALL_DISPATCH_COUNT: AtomicU64 = AtomicU64::new(0);
pub static STRUCT_FIELD_STORE_COUNT: AtomicU64 = AtomicU64::new(0);
pub static ATTR_LOOKUP_COUNT: AtomicU64 = AtomicU64::new(0);
pub static HANDLE_RESOLVE_COUNT: AtomicU64 = AtomicU64::new(0);
pub static LAYOUT_GUARD_COUNT: AtomicU64 = AtomicU64::new(0);
pub static LAYOUT_GUARD_FAIL: AtomicU64 = AtomicU64::new(0);
pub static ALLOC_COUNT: AtomicU64 = AtomicU64::new(0);
pub static ALLOC_OBJECT_COUNT: AtomicU64 = AtomicU64::new(0);
pub static ALLOC_EXCEPTION_COUNT: AtomicU64 = AtomicU64::new(0);
pub static ALLOC_DICT_COUNT: AtomicU64 = AtomicU64::new(0);
pub static ALLOC_TUPLE_COUNT: AtomicU64 = AtomicU64::new(0);
pub static ALLOC_STRING_COUNT: AtomicU64 = AtomicU64::new(0);
pub static ALLOC_CALLARGS_COUNT: AtomicU64 = AtomicU64::new(0);
pub static ALLOC_BYTES_CALLARGS: AtomicU64 = AtomicU64::new(0);
pub static TRACEBACK_BUILD_COUNT: AtomicU64 = AtomicU64::new(0);
pub static TRACEBACK_BUILD_FRAMES: AtomicU64 = AtomicU64::new(0);
pub static TRACEBACK_SUPPRESS_COUNT: AtomicU64 = AtomicU64::new(0);
pub static ASYNC_POLL_COUNT: AtomicU64 = AtomicU64::new(0);
pub static ASYNC_PENDING_COUNT: AtomicU64 = AtomicU64::new(0);
pub static ASYNC_WAKEUP_COUNT: AtomicU64 = AtomicU64::new(0);
pub static ASYNC_SLEEP_REGISTER_COUNT: AtomicU64 = AtomicU64::new(0);

// Week 1 perf observability counters (Codon/general workload attribution).
pub static CALL_BIND_IC_HIT_COUNT: AtomicU64 = AtomicU64::new(0);
pub static CALL_BIND_IC_MISS_COUNT: AtomicU64 = AtomicU64::new(0);
pub static STRING_COUNT_CACHE_HIT_COUNT: AtomicU64 = AtomicU64::new(0);
pub static STRING_COUNT_CACHE_MISS_COUNT: AtomicU64 = AtomicU64::new(0);
pub static ATTR_SITE_NAME_CACHE_HIT_COUNT: AtomicU64 = AtomicU64::new(0);
pub static ATTR_SITE_NAME_CACHE_MISS_COUNT: AtomicU64 = AtomicU64::new(0);
pub static ATTR_IC_RESULT_HIT_COUNT: AtomicU64 = AtomicU64::new(0);
pub static ATTR_IC_RESULT_MISS_COUNT: AtomicU64 = AtomicU64::new(0);
pub static FIELD_OFFSET_IC_HIT_COUNT: AtomicU64 = AtomicU64::new(0);
pub static FIELD_OFFSET_IC_MISS_COUNT: AtomicU64 = AtomicU64::new(0);
pub static SPLIT_WS_ASCII_FAST_PATH_COUNT: AtomicU64 = AtomicU64::new(0);
pub static SPLIT_WS_UNICODE_PATH_COUNT: AtomicU64 = AtomicU64::new(0);
pub static DICT_STR_INT_PREHASH_HIT_COUNT: AtomicU64 = AtomicU64::new(0);
pub static DICT_STR_INT_PREHASH_MISS_COUNT: AtomicU64 = AtomicU64::new(0);
pub static DICT_STR_INT_PREHASH_DEOPT_COUNT: AtomicU64 = AtomicU64::new(0);
pub static TAQ_INGEST_CALL_COUNT: AtomicU64 = AtomicU64::new(0);
pub static TAQ_INGEST_SKIP_MARKER_COUNT: AtomicU64 = AtomicU64::new(0);
pub static ASCII_I64_PARSE_FAIL_COUNT: AtomicU64 = AtomicU64::new(0);
pub static CALL_INDIRECT_NONCALLABLE_DEOPT_COUNT: AtomicU64 = AtomicU64::new(0);
pub static INVOKE_FFI_BRIDGE_CAPABILITY_DENIED_COUNT: AtomicU64 = AtomicU64::new(0);
pub static GUARD_TAG_TYPE_MISMATCH_DEOPT_COUNT: AtomicU64 = AtomicU64::new(0);
pub static GUARD_DICT_SHAPE_LAYOUT_MISMATCH_DEOPT_COUNT: AtomicU64 = AtomicU64::new(0);
pub static GUARD_DICT_SHAPE_LAYOUT_FAIL_NULL_OBJ_COUNT: AtomicU64 = AtomicU64::new(0);
pub static GUARD_DICT_SHAPE_LAYOUT_FAIL_NON_OBJECT_COUNT: AtomicU64 = AtomicU64::new(0);
pub static GUARD_DICT_SHAPE_LAYOUT_FAIL_CLASS_MISMATCH_COUNT: AtomicU64 = AtomicU64::new(0);
pub static GUARD_DICT_SHAPE_LAYOUT_FAIL_NON_TYPE_CLASS_COUNT: AtomicU64 = AtomicU64::new(0);
pub static GUARD_DICT_SHAPE_LAYOUT_FAIL_EXPECTED_VERSION_INVALID_COUNT: AtomicU64 =
    AtomicU64::new(0);
pub static GUARD_DICT_SHAPE_LAYOUT_FAIL_VERSION_MISMATCH_COUNT: AtomicU64 = AtomicU64::new(0);

// Byte-level allocation tracking counters.
pub static ALLOC_BYTES_TOTAL: AtomicU64 = AtomicU64::new(0);
pub static ALLOC_BYTES_STRING: AtomicU64 = AtomicU64::new(0);
pub static ALLOC_BYTES_DICT: AtomicU64 = AtomicU64::new(0);
pub static ALLOC_BYTES_TUPLE: AtomicU64 = AtomicU64::new(0);
pub static ALLOC_BYTES_LIST: AtomicU64 = AtomicU64::new(0);
pub static ALLOC_BYTES_EXCEPTION: AtomicU64 = AtomicU64::new(0);
pub static AUX_CLASS_INLINE_COUNT: AtomicU64 = AtomicU64::new(0);
pub static AUX_STATE_INLINE_COUNT: AtomicU64 = AtomicU64::new(0);
pub static AUX_SIDECAR_COUNT: AtomicU64 = AtomicU64::new(0);
pub static AUX_SIDECAR_FREE_COUNT: AtomicU64 = AtomicU64::new(0);
pub static AUX_SIDECAR_ALLOC_FAILURE_COUNT: AtomicU64 = AtomicU64::new(0);
pub static AUX_SIDECAR_BYTES: AtomicU64 = AtomicU64::new(0);
pub static AUX_SIDECAR_FREE_BYTES: AtomicU64 = AtomicU64::new(0);
pub static GC_TRACK_COUNT: AtomicU64 = AtomicU64::new(0);
pub static GC_UNTRACK_COUNT: AtomicU64 = AtomicU64::new(0);
pub static GC_TRACKED_LIVE: AtomicU64 = AtomicU64::new(0);
pub static GC_REGISTRY_LOCK_CONTENTION_COUNT: AtomicU64 = AtomicU64::new(0);
pub static GC_REGISTRY_LOCK_WAIT_NS: AtomicU64 = AtomicU64::new(0);
pub static GC_TRACKED_HIGH_WATER: AtomicU64 = AtomicU64::new(0);
pub static GC_SNAPSHOT_ALLOC_FAILURE_COUNT: AtomicU64 = AtomicU64::new(0);

// Deallocation tracking counters (RC drop-insertion substrate, design 20).
// Incremented at the `dec_ref_ptr` zero-transition — the single actual
// deallocation path. The `live_objects = ALLOC_COUNT - DEALLOC_COUNT` identity
// is the leak gauge MOLT_PROFILE / MOLT_ASSERT_NO_LEAK consult. Per-type
// counters mirror the alloc-side per-type counters so a leak can be attributed
// to a concrete object family.
pub static DEALLOC_COUNT: AtomicU64 = AtomicU64::new(0);
pub static DEALLOC_BYTES_TOTAL: AtomicU64 = AtomicU64::new(0);
pub static DEALLOC_OBJECT_COUNT: AtomicU64 = AtomicU64::new(0);
pub static DEALLOC_BIGINT_COUNT: AtomicU64 = AtomicU64::new(0);
pub static DEALLOC_STRING_COUNT: AtomicU64 = AtomicU64::new(0);
pub static DEALLOC_DICT_COUNT: AtomicU64 = AtomicU64::new(0);
pub static DEALLOC_TUPLE_COUNT: AtomicU64 = AtomicU64::new(0);
pub static DEALLOC_EXCEPTION_COUNT: AtomicU64 = AtomicU64::new(0);
pub static DEALLOC_BYTES_EXCEPTION: AtomicU64 = AtomicU64::new(0);
