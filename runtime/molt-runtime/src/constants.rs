use std::sync::atomic::AtomicU64;

// Keep in sync with MOLT_BIND_KIND_OPEN in src/molt/frontend/__init__.py.
pub(crate) const BIND_KIND_OPEN: i64 = 1;

pub(crate) const WASM_TABLE_BASE_FALLBACK: u64 = 256;

#[cfg(target_arch = "wasm32")]
static WASM_TABLE_BASE_RUNTIME: AtomicU64 = AtomicU64::new(WASM_TABLE_BASE_FALLBACK);

#[cfg(target_arch = "wasm32")]
pub(crate) fn wasm_table_base() -> u64 {
    let base = WASM_TABLE_BASE_RUNTIME.load(std::sync::atomic::Ordering::Relaxed);
    if base > 0 {
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
pub(crate) const fn wasm_table_base() -> u64 {
    WASM_TABLE_BASE_FALLBACK
}

pub(crate) const INLINE_INT_MIN_I128: i128 = -(1_i128 << 46);
pub(crate) const INLINE_INT_MAX_I128: i128 = (1_i128 << 46) - 1;
pub(crate) const MAX_SMALL_LIST: usize = 16;
pub(crate) const ITER_EXHAUSTED: usize = usize::MAX;

pub(crate) const FUNC_DEFAULT_NONE: i64 = 1;
pub(crate) const FUNC_DEFAULT_DICT_POP: i64 = 2;
pub(crate) const FUNC_DEFAULT_DICT_UPDATE: i64 = 3;
pub(crate) const FUNC_DEFAULT_REPLACE_COUNT: i64 = 4;
pub(crate) const FUNC_DEFAULT_NEG_ONE: i64 = 5;
pub(crate) const FUNC_DEFAULT_ZERO: i64 = 6;
pub(crate) const FUNC_DEFAULT_MISSING: i64 = 7;
pub(crate) const FUNC_DEFAULT_NONE2: i64 = 8;
pub(crate) const FUNC_DEFAULT_IO_RAW: i64 = 9;
pub(crate) const FUNC_DEFAULT_IO_TEXT_WRAPPER: i64 = 10;

pub(crate) const GEN_SEND_OFFSET: usize = 0;
pub(crate) const GEN_THROW_OFFSET: usize = 8;
pub(crate) const GEN_CLOSED_OFFSET: usize = 16;
pub(crate) const GEN_EXC_DEPTH_OFFSET: usize = 24;
pub(crate) const GEN_YIELD_FROM_OFFSET: usize = 32;
pub(crate) const GEN_CONTROL_SIZE: usize = 48;

pub(crate) const ASYNCGEN_GEN_OFFSET: usize = 0;
pub(crate) const ASYNCGEN_RUNNING_OFFSET: usize = 8;
pub(crate) const ASYNCGEN_PENDING_OFFSET: usize = 16;
pub(crate) const ASYNCGEN_FIRSTITER_OFFSET: usize = 24;
pub(crate) const ASYNCGEN_CONTROL_SIZE: usize = 32;
pub(crate) const ASYNCGEN_OP_ANEXT: i64 = 0;
pub(crate) const ASYNCGEN_OP_ASEND: i64 = 1;
pub(crate) const ASYNCGEN_OP_ATHROW: i64 = 2;
pub(crate) const ASYNCGEN_OP_ACLOSE: i64 = 3;

pub(crate) const TASK_KIND_FUTURE: u64 = 0;
pub(crate) const TASK_KIND_GENERATOR: u64 = 1;
pub(crate) const TASK_KIND_COROUTINE: u64 = 2;

pub(crate) static CALL_DISPATCH_COUNT: AtomicU64 = AtomicU64::new(0);
pub(crate) static STRUCT_FIELD_STORE_COUNT: AtomicU64 = AtomicU64::new(0);
pub(crate) static ATTR_LOOKUP_COUNT: AtomicU64 = AtomicU64::new(0);
pub(crate) static HANDLE_RESOLVE_COUNT: AtomicU64 = AtomicU64::new(0);
pub(crate) static LAYOUT_GUARD_COUNT: AtomicU64 = AtomicU64::new(0);
pub(crate) static LAYOUT_GUARD_FAIL: AtomicU64 = AtomicU64::new(0);
pub(crate) static ALLOC_COUNT: AtomicU64 = AtomicU64::new(0);
pub(crate) static ALLOC_OBJECT_COUNT: AtomicU64 = AtomicU64::new(0);
pub(crate) static ALLOC_EXCEPTION_COUNT: AtomicU64 = AtomicU64::new(0);
pub(crate) static ALLOC_DICT_COUNT: AtomicU64 = AtomicU64::new(0);
pub(crate) static ALLOC_TUPLE_COUNT: AtomicU64 = AtomicU64::new(0);
pub(crate) static ALLOC_STRING_COUNT: AtomicU64 = AtomicU64::new(0);
pub(crate) static ALLOC_CALLARGS_COUNT: AtomicU64 = AtomicU64::new(0);
pub(crate) static TRACEBACK_BUILD_COUNT: AtomicU64 = AtomicU64::new(0);
pub(crate) static TRACEBACK_BUILD_FRAMES: AtomicU64 = AtomicU64::new(0);
pub(crate) static TRACEBACK_SUPPRESS_COUNT: AtomicU64 = AtomicU64::new(0);
pub(crate) static ASYNC_POLL_COUNT: AtomicU64 = AtomicU64::new(0);
pub(crate) static ASYNC_PENDING_COUNT: AtomicU64 = AtomicU64::new(0);
pub(crate) static ASYNC_WAKEUP_COUNT: AtomicU64 = AtomicU64::new(0);
pub(crate) static ASYNC_SLEEP_REGISTER_COUNT: AtomicU64 = AtomicU64::new(0);

// Week 1 perf observability counters (Codon/general workload attribution).
pub(crate) static CALL_BIND_IC_HIT_COUNT: AtomicU64 = AtomicU64::new(0);
pub(crate) static CALL_BIND_IC_MISS_COUNT: AtomicU64 = AtomicU64::new(0);
pub(crate) static ATTR_SITE_NAME_CACHE_HIT_COUNT: AtomicU64 = AtomicU64::new(0);
pub(crate) static ATTR_SITE_NAME_CACHE_MISS_COUNT: AtomicU64 = AtomicU64::new(0);
pub(crate) static SPLIT_WS_ASCII_FAST_PATH_COUNT: AtomicU64 = AtomicU64::new(0);
pub(crate) static SPLIT_WS_UNICODE_PATH_COUNT: AtomicU64 = AtomicU64::new(0);
pub(crate) static DICT_STR_INT_PREHASH_HIT_COUNT: AtomicU64 = AtomicU64::new(0);
pub(crate) static DICT_STR_INT_PREHASH_MISS_COUNT: AtomicU64 = AtomicU64::new(0);
pub(crate) static DICT_STR_INT_PREHASH_DEOPT_COUNT: AtomicU64 = AtomicU64::new(0);
pub(crate) static TAQ_INGEST_CALL_COUNT: AtomicU64 = AtomicU64::new(0);
pub(crate) static TAQ_INGEST_SKIP_MARKER_COUNT: AtomicU64 = AtomicU64::new(0);
pub(crate) static ASCII_I64_PARSE_FAIL_COUNT: AtomicU64 = AtomicU64::new(0);
pub(crate) static CALL_INDIRECT_NONCALLABLE_DEOPT_COUNT: AtomicU64 = AtomicU64::new(0);
pub(crate) static INVOKE_FFI_BRIDGE_CAPABILITY_DENIED_COUNT: AtomicU64 = AtomicU64::new(0);
pub(crate) static GUARD_TAG_TYPE_MISMATCH_DEOPT_COUNT: AtomicU64 = AtomicU64::new(0);
pub(crate) static GUARD_DICT_SHAPE_LAYOUT_MISMATCH_DEOPT_COUNT: AtomicU64 = AtomicU64::new(0);
pub(crate) static GUARD_DICT_SHAPE_LAYOUT_FAIL_NULL_OBJ_COUNT: AtomicU64 = AtomicU64::new(0);
pub(crate) static GUARD_DICT_SHAPE_LAYOUT_FAIL_NON_OBJECT_COUNT: AtomicU64 = AtomicU64::new(0);
pub(crate) static GUARD_DICT_SHAPE_LAYOUT_FAIL_CLASS_MISMATCH_COUNT: AtomicU64 = AtomicU64::new(0);
pub(crate) static GUARD_DICT_SHAPE_LAYOUT_FAIL_NON_TYPE_CLASS_COUNT: AtomicU64 = AtomicU64::new(0);
pub(crate) static GUARD_DICT_SHAPE_LAYOUT_FAIL_EXPECTED_VERSION_INVALID_COUNT: AtomicU64 =
    AtomicU64::new(0);
pub(crate) static GUARD_DICT_SHAPE_LAYOUT_FAIL_VERSION_MISMATCH_COUNT: AtomicU64 =
    AtomicU64::new(0);
