use std::sync::atomic::AtomicU64;

// Builtin callable policy tags are generated from builtin_callables.py.
pub const BIND_KIND_CAPI_METHOD: i64 = 2;
pub const BIND_KIND_PACKED_BUILTIN: i64 = 3;
pub const BIND_KIND_TYPE_NEW_INIT: i64 = 4;
// Rust-only projections of explicit CFunctionConvention declarations.
pub const BIND_KIND_FIXED_NOARGS: i64 = 5;
pub const BIND_KIND_FIXED_ONE_OBJECT: i64 = 6;

pub const MAX_SMALL_LIST: usize = 16;
pub const ITER_EXHAUSTED: usize = usize::MAX;

// A generator's control region, the first `GENERATOR_CONTROL_BYTES` of its
// payload. Edge slots own a boxed value that heap traversal visits and
// detachment takes; raw words hold plain integers and are never edges.
/// Edge: the value `send()` passes to the resumed body.
pub const GEN_SEND_OFFSET: usize = 0;
/// Edge: the exception `throw()` injects at the resumed yield.
pub const GEN_THROW_OFFSET: usize = 8;
/// Raw word: the activation's frame state (`async_rt::generators::FrameState`).
pub const GEN_STATE_OFFSET: usize = 16;
/// Edge: the iterator a `yield from` delegates to.
pub const GEN_YIELD_FROM_OFFSET: usize = 32;
/// Every control-region slot that owns a reference, the one layout authority
/// for heap traversal and detachment.
pub const GEN_EDGE_OFFSETS: [usize; 3] = [GEN_SEND_OFFSET, GEN_THROW_OFFSET, GEN_YIELD_FROM_OFFSET];
/// Reserved raw words of the control region: zero, never edges. The
/// activation's saved exception state lives in its sidecar.
pub const GEN_RESERVED_OFFSETS: [usize; 2] = [24, 40];

pub const ASYNCGEN_GEN_OFFSET: usize = 0;
pub const ASYNCGEN_RUNNING_OFFSET: usize = 8;
pub const ASYNCGEN_PENDING_OFFSET: usize = 16;
pub const ASYNCGEN_FIRSTITER_OFFSET: usize = 24;
pub const ASYNCGEN_FINALIZER_OFFSET: usize = 32;
pub const ASYNCGEN_CONTROL_SIZE: usize = 40;
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
pub static FIELD_OFFSET_IC_HIT_COUNT: AtomicU64 = AtomicU64::new(0);
pub static FIELD_OFFSET_IC_MISS_COUNT: AtomicU64 = AtomicU64::new(0);
pub static DICT_STR_INT_PREHASH_HIT_COUNT: AtomicU64 = AtomicU64::new(0);
pub static DICT_STR_INT_PREHASH_MISS_COUNT: AtomicU64 = AtomicU64::new(0);
pub static DICT_STR_INT_PREHASH_DEOPT_COUNT: AtomicU64 = AtomicU64::new(0);
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
