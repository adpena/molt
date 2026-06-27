use std::alloc::Layout;
use std::collections::HashMap;
use std::io::Write;
use std::sync::atomic::{AtomicU64, Ordering as AtomicOrdering};
use std::sync::{Arc, Mutex, OnceLock};

use molt_obj_model::MoltObject;
use num_bigint::BigInt;

/// Global type version counter. Incremented whenever ANY class is modified
/// (attribute set/deleted, base class changed, __dict__ mutated).
/// Inline caches compare against this to detect staleness.
///
/// Uses `Relaxed` ordering because all callers hold the GIL, which provides
/// the happens-before relationship. If the GIL is ever relaxed or removed,
/// these must be upgraded to `Acquire`/`Release`.
static GLOBAL_TYPE_VERSION: AtomicU64 = AtomicU64::new(1);

#[inline(always)]
pub fn global_type_version() -> u64 {
    GLOBAL_TYPE_VERSION.load(AtomicOrdering::Relaxed)
}

#[inline(always)]
pub fn bump_type_version() -> u64 {
    GLOBAL_TYPE_VERSION.fetch_add(1, AtomicOrdering::Relaxed) + 1
}

pub(crate) mod accessors;
pub(crate) mod backing;
pub(crate) mod buffer2d;
pub(crate) mod builders;
#[allow(dead_code)]
pub mod deopt;
#[allow(dead_code)]
pub mod dict_compact;
#[allow(dead_code)]
pub mod gil;
#[allow(dead_code)]
pub mod inline_cache;
pub(crate) mod layout;
pub(crate) mod memoryview;
pub(crate) mod native_handle;
pub(crate) mod ops;
pub(crate) mod ops_arith;
pub(crate) mod ops_builtins;
pub(crate) mod ops_bytes;
pub(crate) mod ops_compare;
pub(crate) mod ops_convert;
pub(crate) mod ops_dict;
pub(crate) mod ops_encoding;
pub(crate) mod ops_format;
pub(crate) mod ops_hash;
pub(crate) mod ops_heapq;
pub(crate) mod ops_iter;
pub(crate) mod ops_list;
pub(crate) mod ops_memoryview;
pub(crate) mod ops_set;
pub(crate) mod ops_slice;
pub(crate) mod ops_string;
pub(crate) mod ops_sys;
pub(crate) mod ops_vec;
pub(crate) mod refcount;
pub(crate) mod refcount_opt;
#[allow(dead_code)]
pub mod string_intern;
#[allow(dead_code)]
pub mod string_repr;
pub(crate) mod type_ids;
pub(crate) mod utf8_cache;
pub(crate) mod weakref;

use refcount::MoltRefCount;

#[allow(unused_imports)]
pub(crate) use type_ids::*;

use crate::async_rt::poll::ws_wait_poll_fn_addr;
#[cfg(not(feature = "stdlib_itertools"))]
use crate::builtins::itertools::itertools_drop_instance;
use crate::builtins::{
    functools::functools_drop_instance, operator::operator_drop_instance,
    types::types_drop_instance,
};
use crate::provenance::{release_ptr, resolve_ptr};
use crate::{
    ALLOC_BYTES_DICT, ALLOC_BYTES_LIST, ALLOC_BYTES_STRING, ALLOC_BYTES_TOTAL, ALLOC_BYTES_TUPLE,
    ALLOC_CALLARGS_COUNT, ALLOC_COUNT, ALLOC_DICT_COUNT, ALLOC_EXCEPTION_COUNT, ALLOC_OBJECT_COUNT,
    ALLOC_STRING_COUNT, ALLOC_TUPLE_COUNT, DEALLOC_BIGINT_COUNT, DEALLOC_BYTES_TOTAL,
    DEALLOC_COUNT, DEALLOC_DICT_COUNT, DEALLOC_OBJECT_COUNT, DEALLOC_STRING_COUNT,
    DEALLOC_TUPLE_COUNT, GEN_CLOSED_OFFSET, GEN_EXC_DEPTH_OFFSET, GEN_SEND_OFFSET,
    GEN_THROW_OFFSET, PyToken, TYPE_ID_ASYNC_GENERATOR, TYPE_ID_BIGINT, TYPE_ID_BOUND_METHOD,
    TYPE_ID_BUFFER2D, TYPE_ID_BYTEARRAY, TYPE_ID_CALL_ITER, TYPE_ID_CALLARGS, TYPE_ID_CLASSMETHOD,
    TYPE_ID_CODE, TYPE_ID_CONTEXT_MANAGER, TYPE_ID_DATACLASS, TYPE_ID_DICT,
    TYPE_ID_DICT_ITEMS_VIEW, TYPE_ID_DICT_KEYS_VIEW, TYPE_ID_DICT_VALUES_VIEW, TYPE_ID_ENUMERATE,
    TYPE_ID_EXCEPTION, TYPE_ID_FILE_HANDLE, TYPE_ID_FILTER, TYPE_ID_FROZENSET, TYPE_ID_FUNCTION,
    TYPE_ID_GENERATOR, TYPE_ID_GENERIC_ALIAS, TYPE_ID_GLOB_ITER, TYPE_ID_ITER, TYPE_ID_LIST,
    TYPE_ID_LIST_BUILDER, TYPE_ID_MAP, TYPE_ID_MEMORYVIEW, TYPE_ID_MODULE, TYPE_ID_NATIVE_HANDLE,
    TYPE_ID_OBJECT, TYPE_ID_PROPERTY, TYPE_ID_REVERSED, TYPE_ID_SET, TYPE_ID_SLICE,
    TYPE_ID_STATICMETHOD, TYPE_ID_STRING, TYPE_ID_TRACEBACK_PAYLOAD, TYPE_ID_TUPLE, TYPE_ID_UNION,
    TYPE_ID_ZIP, asyncgen_call_finalizer, asyncgen_gen_bits, asyncgen_pending_bits,
    asyncgen_registry_remove, asyncgen_running_bits, asyncio_fd_watcher_poll_fn_addr,
    asyncio_fd_watcher_task_drop, asyncio_gather_poll_fn_addr, asyncio_gather_task_drop,
    asyncio_ready_runner_poll_fn_addr, asyncio_ready_runner_task_drop,
    asyncio_server_accept_loop_poll_fn_addr, asyncio_server_accept_loop_task_drop,
    asyncio_sock_accept_poll_fn_addr, asyncio_sock_accept_task_drop,
    asyncio_sock_connect_poll_fn_addr, asyncio_sock_connect_task_drop,
    asyncio_sock_recv_into_poll_fn_addr, asyncio_sock_recv_into_task_drop,
    asyncio_sock_recv_poll_fn_addr, asyncio_sock_recv_task_drop,
    asyncio_sock_recvfrom_into_poll_fn_addr, asyncio_sock_recvfrom_into_task_drop,
    asyncio_sock_recvfrom_poll_fn_addr, asyncio_sock_recvfrom_task_drop,
    asyncio_sock_sendall_poll_fn_addr, asyncio_sock_sendall_task_drop,
    asyncio_sock_sendto_poll_fn_addr, asyncio_sock_sendto_task_drop,
    asyncio_socket_reader_read_poll_fn_addr, asyncio_socket_reader_read_task_drop,
    asyncio_socket_reader_readline_poll_fn_addr, asyncio_socket_reader_readline_task_drop,
    asyncio_stream_reader_read_poll_fn_addr, asyncio_stream_reader_read_task_drop,
    asyncio_stream_reader_readline_poll_fn_addr, asyncio_stream_reader_readline_task_drop,
    asyncio_stream_send_all_poll_fn_addr, asyncio_stream_send_all_task_drop,
    asyncio_timer_handle_poll_fn_addr, asyncio_timer_handle_task_drop,
    asyncio_wait_for_poll_fn_addr, asyncio_wait_for_task_drop, asyncio_wait_poll_fn_addr,
    asyncio_wait_task_drop, bound_method_func_bits, bound_method_self_bits,
    builtin_classes_if_initialized, bytearray_data, bytearray_len, bytearray_vec_ptr,
    call_iter_cached_tuple, call_iter_callable_bits, call_iter_sentinel_bits, callargs_dec_ref_all,
    callargs_ptr, classmethod_func_bits, code_arg_names_bits, code_filename_bits,
    code_kwonly_names_bits, code_linetable_bits, code_name_bits, code_names_bits,
    code_signature_posonly_bits, code_vararg_bits, code_varkw_bits, code_varnames_bits,
    context_payload_bits, contextlib_async_exitstack_enter_context_poll_fn_addr,
    contextlib_async_exitstack_enter_context_task_drop,
    contextlib_async_exitstack_exit_poll_fn_addr, contextlib_async_exitstack_exit_task_drop,
    contextlib_asyncgen_enter_poll_fn_addr, contextlib_asyncgen_enter_task_drop,
    contextlib_asyncgen_exit_poll_fn_addr, contextlib_asyncgen_exit_task_drop, dict_hashes_ptr,
    dict_order_ptr, dict_table_ptr, dict_view_dict_bits, enumerate_cached_inner,
    enumerate_cached_outer, enumerate_index_bits, enumerate_target_bits, exception_args_bits,
    exception_args_payload_bits, exception_cause_bits, exception_class_bits,
    exception_context_bits, exception_dict_bits, exception_kind_bits, exception_msg_bits,
    exception_suppress_bits, exception_trace_bits, exception_value_bits, filter_func_bits,
    filter_iter_bits, function_annotate_bits, function_annotations_bits, function_closure_bits,
    function_code_bits, function_dict_bits, generator_context_stack_drop,
    generator_exception_stack_drop, generic_alias_args_bits, generic_alias_origin_bits,
    io_wait_poll_fn_addr, io_wait_release_socket, issubclass_bits, iter_cached_tuple,
    iter_target_bits, map_cached_tuple, map_func_bits, map_iters_ptr, module_dict_bits,
    module_name_bits, process_poll_fn_addr, profile_hit, profile_hit_bytes, property_del_bits,
    property_get_bits, property_set_bits, range_start_bits, range_step_bits, range_stop_bits,
    reversed_target_bits, runtime_state, seq_vec_ptr, set_hashes_ptr, set_order_ptr, set_table_ptr,
    slice_start_bits, slice_step_bits, slice_stop_bits, staticmethod_func_bits,
    task_cancel_message_clear, thread_poll_fn_addr, traceback_payload_code_bits,
    traceback_payload_next_bits, union_type_args_bits, utf8_cache_remove, weakref_clear_for_ptr,
    ws_wait_release, zip_iters_ptr, zip_strict_bits,
};
#[cfg(feature = "stdlib_itertools")]
use molt_runtime_itertools::itertools::itertools_drop_instance;

#[cfg(not(target_arch = "wasm32"))]
use crate::{process_task_drop, thread_task_drop};

fn debug_alloc_list_builder() -> bool {
    static ENABLED: OnceLock<bool> = OnceLock::new();
    *ENABLED.get_or_init(|| {
        matches!(
            std::env::var("MOLT_DEBUG_ALLOC_LIST_BUILDER")
                .ok()
                .as_deref(),
            Some("1")
        )
    })
}

fn debug_alloc_object() -> bool {
    static ENABLED: OnceLock<bool> = OnceLock::new();
    *ENABLED.get_or_init(|| {
        matches!(
            std::env::var("MOLT_DEBUG_ALLOC_OBJECT").ok().as_deref(),
            Some("1")
        )
    })
}

fn debug_oom() -> bool {
    static ENABLED: OnceLock<bool> = OnceLock::new();
    *ENABLED.get_or_init(|| matches!(std::env::var("MOLT_DEBUG_OOM").ok().as_deref(), Some("1")))
}

#[inline]
fn debug_rc_object() -> bool {
    static ENABLED: OnceLock<bool> = OnceLock::new();
    *ENABLED.get_or_init(|| std::env::var("MOLT_DEBUG_RC_OBJECT").as_deref() == Ok("1"))
}

#[inline]
fn debug_dec_ref_zero() -> bool {
    static ENABLED: OnceLock<bool> = OnceLock::new();
    *ENABLED.get_or_init(|| std::env::var("MOLT_DEBUG_DECREF_ZERO").as_deref() == Ok("1"))
}

#[inline]
fn debug_file_rc() -> bool {
    static ENABLED: OnceLock<bool> = OnceLock::new();
    *ENABLED.get_or_init(|| std::env::var("MOLT_DEBUG_FILE_RC").as_deref() == Ok("1"))
}

#[inline]
fn trace_object_state() -> bool {
    static ENABLED: OnceLock<bool> = OnceLock::new();
    *ENABLED.get_or_init(|| std::env::var("MOLT_TRACE_OBJECT_STATE").as_deref() == Ok("1"))
}

/// Cached debug flag for tracing BigInt refcount inc/dec on the hot path.
/// Reading the env var on every refcount op would call libc `getenv` (mutex-
/// guarded), which dominates throughput on integer-heavy benchmarks even
/// when the var is unset. Cache once at first use.
#[inline]
fn debug_bigint_rc() -> bool {
    static ENABLED: OnceLock<bool> = OnceLock::new();
    *ENABLED.get_or_init(|| std::env::var("MOLT_DEBUG_BIGINT_RC").is_ok())
}

#[inline]
fn debug_object_rc() -> bool {
    static ENABLED: OnceLock<bool> = OnceLock::new();
    *ENABLED.get_or_init(|| std::env::var("MOLT_DEBUG_OBJECT_RC").is_ok())
}

/// Cached `MOLT_TRACE_EXC_RC` flag for tracing exception-object refcount
/// inc/dec/resurrect/free on the hot path. Like `debug_bigint_rc`, this gates a
/// diagnostic that prints every refcount transition of a `TYPE_ID_EXCEPTION`
/// object — the tool that pinned the exception-heavy retention leak (#77): a
/// raised-and-caught exception accrues 3 inc_ref but only 2 dec_ref per
/// iteration and ends at refcount 2, never freed. The live ownership authority
/// is the ExceptionRegions/drop-insertion model in design 45. Reading the env
/// refcount op would take the libc environ lock per call and tax every program,
/// so cache it once at first use — the diagnostic is exactly zero-cost when off.
#[inline]
fn trace_exception_rc() -> bool {
    static ENABLED: OnceLock<bool> = OnceLock::new();
    *ENABLED.get_or_init(|| std::env::var("MOLT_TRACE_EXC_RC").as_deref() == Ok("1"))
}

#[inline]
fn trace_decref_zero_function_all() -> bool {
    static ENABLED: OnceLock<bool> = OnceLock::new();
    *ENABLED.get_or_init(|| {
        matches!(
            std::env::var("MOLT_TRACE_DECREF_ZERO_FUNCTION_ALL")
                .ok()
                .as_deref(),
            Some("1")
        )
    })
}

fn flush_file_handle_on_drop(_py: &PyToken<'_>, handle: &mut MoltFileHandle) {
    if handle.write_buf.is_empty() {
        return;
    }
    let backend_state = Arc::clone(&handle.state);
    let Ok(mut guard) = backend_state.backend.lock() else {
        handle.write_buf.clear();
        return;
    };
    let Some(backend) = guard.as_mut() else {
        handle.write_buf.clear();
        return;
    };
    let bytes = std::mem::take(&mut handle.write_buf);
    match backend {
        MoltFileBackend::File(file) => {
            let mut written = 0usize;
            while written < bytes.len() {
                match file.write(&bytes[written..]) {
                    Ok(0) => break,
                    Ok(n) => written += n,
                    Err(_) => break,
                }
            }
            let _ = file.flush();
        }
        MoltFileBackend::Memory(mem) => {
            if handle.mem_bits == 0 || obj_from_bits(handle.mem_bits).is_none() {
                return;
            }
            let Some(mem_ptr) = obj_from_bits(handle.mem_bits).as_ptr() else {
                return;
            };
            if unsafe { object_type_id(mem_ptr) } != TYPE_ID_BYTEARRAY {
                return;
            }
            let vec_ptr = unsafe { bytearray_vec_ptr(mem_ptr) };
            if vec_ptr.is_null() {
                return;
            }
            let data = unsafe { &mut *vec_ptr };
            if mem.pos > data.len() {
                data.resize(mem.pos, 0);
            }
            let end = mem.pos.saturating_add(bytes.len());
            if end > data.len() {
                data.resize(end, 0);
            }
            data[mem.pos..end].copy_from_slice(&bytes);
            mem.pos = end;
        }
        MoltFileBackend::Text(_) => {}
    }
}

fn debug_alloc_object_type() -> Option<u32> {
    static FILTER: OnceLock<Option<u32>> = OnceLock::new();
    *FILTER.get_or_init(|| {
        std::env::var("MOLT_DEBUG_ALLOC_OBJECT_TYPE")
            .ok()
            .and_then(|value| value.parse::<u32>().ok())
    })
}

#[repr(C)]
pub struct MoltHeader {
    pub type_id: u32,            // 4 bytes
    pub ref_count: MoltRefCount, // 4 bytes
    pub flags: u32,              // 4 bytes (semantic bits declared below)
    pub size_class: u16,         // 2 bytes — index into SIZE_CLASS_TABLE
    pub cold_idx: u32,           // 4 bytes — index into COLD_HEADER_SLAB (0 = none)
    pub reserved: u32,           // 4 bytes — keeps payload 8-byte aligned
}
// Total: 24 bytes. poll_fn, state, extended_size live in MoltColdHeader.

#[derive(Copy, Clone, Debug, PartialEq, Eq, Hash)]
pub(crate) struct PtrSlot(pub(crate) *mut u8);

// Raw pointers are guarded by locks; it is safe to share slots across threads.
unsafe impl Send for PtrSlot {}
unsafe impl Sync for PtrSlot {}

pub(crate) struct DataclassDesc {
    pub(crate) name: String,
    pub(crate) field_names: Vec<String>,
    pub(crate) field_name_to_index: HashMap<String, usize>,
    pub(crate) frozen: bool,
    pub(crate) eq: bool,
    pub(crate) repr: bool,
    pub(crate) slots: bool,
    pub(crate) allows_dict: bool,
    pub(crate) class_bits: u64,
    pub(crate) field_flags: Vec<u8>,
    pub(crate) hash_mode: u8,
}

pub(crate) struct Buffer2D {
    pub(crate) rows: usize,
    pub(crate) cols: usize,
    pub(crate) data: Vec<i64>,
}

#[repr(C)]
pub(crate) struct MemoryView {
    pub(crate) owner_bits: u64,
    pub(crate) offset: isize,
    pub(crate) len: usize,
    pub(crate) itemsize: usize,
    pub(crate) stride: isize,
    pub(crate) readonly: u8,
    pub(crate) ndim: u8,
    pub(crate) _pad: [u8; 6],
    pub(crate) format_bits: u64,
    pub(crate) shape_ptr: *mut Vec<isize>,
    pub(crate) strides_ptr: *mut Vec<isize>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum MemoryViewFormatKind {
    Signed,
    Unsigned,
    Float,
    Bool,
    Char,
}

#[derive(Clone, Copy, Debug)]
pub(crate) struct MemoryViewFormat {
    pub(crate) code: u8,
    pub(crate) itemsize: usize,
    pub(crate) kind: MemoryViewFormatKind,
}

pub(crate) enum MoltFileBackend {
    File(std::fs::File),
    Memory(MoltMemoryBackend),
    Text(MoltTextBackend),
}

pub(crate) struct MoltMemoryBackend {
    pub(crate) pos: usize,
}

pub(crate) struct MoltTextBackend {
    pub(crate) data: Vec<char>,
    pub(crate) pos: usize,
}

pub(crate) struct MoltFileState {
    pub(crate) backend: Mutex<Option<MoltFileBackend>>,
    #[cfg(windows)]
    pub(crate) crt_fd: Mutex<Option<i64>>,
}

pub(crate) struct MoltFileHandle {
    pub(crate) state: Arc<MoltFileState>,
    pub(crate) readable: bool,
    pub(crate) writable: bool,
    pub(crate) text: bool,
    #[allow(dead_code)]
    pub(crate) closefd: bool,
    pub(crate) owns_fd: bool,
    pub(crate) closed: bool,
    pub(crate) detached: bool,
    pub(crate) line_buffering: bool,
    pub(crate) write_through: bool,
    #[allow(dead_code)]
    pub(crate) buffer_size: i64,
    #[allow(dead_code)]
    pub(crate) class_bits: u64,
    pub(crate) name_bits: u64,
    pub(crate) mode: String,
    pub(crate) encoding: Option<String>,
    pub(crate) encoding_original: Option<String>,
    pub(crate) text_bom_seen: bool,
    pub(crate) text_bom_written: bool,
    pub(crate) errors: Option<String>,
    pub(crate) newline: Option<String>,
    pub(crate) buffer_bits: u64,
    pub(crate) pending_byte: Option<u8>,
    pub(crate) text_pending_bytes: Vec<u8>,
    pub(crate) text_pending_text: Vec<u8>,
    pub(crate) mem_bits: u64,
    pub(crate) read_buf: Vec<u8>,
    pub(crate) read_pos: usize,
    pub(crate) write_buf: Vec<u8>,
    pub(crate) newlines_mask: u8,
    pub(crate) newlines_len: u8,
    pub(crate) newlines_seen: [u8; 3],
}

pub(crate) const NEWLINE_KIND_LF: u8 = 1;
pub(crate) const NEWLINE_KIND_CR: u8 = 1 << 1;
pub(crate) const NEWLINE_KIND_CRLF: u8 = 1 << 2;

pub(crate) const HEADER_FLAG_HAS_PTRS: u32 = 1;
pub(crate) const HEADER_FLAG_SKIP_CLASS_DECREF: u32 = 1 << 1;
pub(crate) const HEADER_FLAG_GEN_RUNNING: u32 = 1 << 2;
pub(crate) const HEADER_FLAG_GEN_STARTED: u32 = 1 << 3;
pub(crate) const HEADER_FLAG_SPAWN_RETAIN: u32 = 1 << 4;
pub(crate) const HEADER_FLAG_CANCEL_PENDING: u32 = 1 << 5;
pub(crate) const HEADER_FLAG_BLOCK_ON: u32 = 1 << 6;
pub(crate) const HEADER_FLAG_TASK_QUEUED: u32 = 1 << 7;
pub(crate) const HEADER_FLAG_TASK_RUNNING: u32 = 1 << 8;
pub(crate) const HEADER_FLAG_TASK_WAKE_PENDING: u32 = 1 << 9;
pub(crate) const HEADER_FLAG_TASK_DONE: u32 = 1 << 10;
pub(crate) const HEADER_FLAG_TRACEBACK_SUPPRESSED: u32 = 1 << 11;
pub(crate) const HEADER_FLAG_COROUTINE: u32 = 1 << 12;
pub(crate) const HEADER_FLAG_FUNC_TASK_TRAMPOLINE_KNOWN: u32 = 1 << 13;
pub(crate) const HEADER_FLAG_FUNC_TASK_TRAMPOLINE_NEEDED: u32 = 1 << 14;
// CPython-like "immortal" objects: refcount ops are skipped and the object is never freed.
// Use this only for runtime singletons/cached builtin callables.
pub(crate) const HEADER_FLAG_IMMORTAL: u32 = 1 << 15;
// Ensure __del__ runs at most once even if the object resurrects itself.
pub(crate) const HEADER_FLAG_FINALIZER_RAN: u32 = 1 << 16;
// String content is an ASCII identifier stored in the global intern pool.
// Objects with this flag are also immortal (never freed).
pub(crate) const HEADER_FLAG_INTERNED: u32 = 1 << 17;
/// `TYPE_ID_OBJECT` instance whose class MRO contains `__del__`. This makes the
/// non-finalizer rc->0 path an O(1) header test and keeps ordinary attribute
/// lookup out of finalizer dispatch.
pub(crate) const HEADER_FLAG_INSTANCE_HAS_FINALIZER: u32 = 1 << 18;
/// Container (list, tuple, dict, set) has at least one element that is a heap
/// pointer (TAG_PTR).  When this flag is clear, `dec_ref` cleanup can skip
/// iterating over elements because they are all primitives (int/float/bool/None).
pub(crate) const HEADER_FLAG_CONTAINS_REFS: u32 = 1 << 19;

/// Object was allocated via `molt_alloc` (raw allocation) — deallocation must
/// use the raw-alloc path rather than type-specific destructors.
pub(crate) const HEADER_FLAG_RAW_ALLOC: u32 = 1 << 20;

/// Object was bump-allocated inside a `ScopeArena`. Deallocation must NOT call
/// `std::alloc::dealloc`:
/// the arena reclaims memory in bulk when `molt_arena_free` runs at scope
/// exit. Set by `molt_arena_alloc_object`.
pub(crate) const HEADER_FLAG_ARENA: u32 = 1 << 21;

/// `TYPE_ID_TYPE` metadata bit: instances of this class are finalizer-sensitive
/// because the class MRO contains `__del__`.
pub(crate) const HEADER_FLAG_CLASS_HAS_FINALIZER: u32 = 1 << 22;

/// `TYPE_ID_FUNCTION` metadata bit: raw positional calls must route through the
/// argument binder before any fixed-arity ABI call. This is set for functions
/// with keyword-only params/defaults, `*args`, `**kwargs`, or a builtin bind
/// kind, and lets native inline probes reject complex call shapes with one
/// header-flag test.
pub(crate) const HEADER_FLAG_FUNC_REQUIRES_BINDER: u32 = 1 << 23;

/// Lifetime-boundary bit: this object has had at least one `weakref` registered
/// against it, so its `dec_ref_ptr` zero-transition must enter the finalize +
/// weakref-clear revival window (open a revival ref, run any weakref callbacks
/// while the object is provably live, then re-check resurrection) rather than
/// freeing immediately. Set once at `molt_weakref_register` and never cleared:
/// a weakref may later be dropped, but keeping the flag set only means the
/// (already cheap) `weakref_clear_for_ptr` lock + revival window run on final
/// death for an object that DID once expose a weakref — strictly narrower than
/// the previous unconditional per-`dec→0` weakref lock. This is the single
/// cached fact that lets non-weakref objects skip both the global weakref lock
/// and the extra revival inc/dec on the hottest free path, and — together with
/// `HEADER_FLAG_INSTANCE_HAS_FINALIZER` — is what makes the rc=0 weakref-callback
/// window structurally unreachable (the window is opened BEFORE any callback can
/// observe the object).
pub(crate) const HEADER_FLAG_HAS_WEAKREF: u32 = 1 << 24;

// ---------------------------------------------------------------------------
// Cold header pool — stores rarely-used per-object metadata (poll_fn, state,
// extended_size) separately from the hot MoltHeader so that the hot header
// can be kept small and cache-friendly.
// ---------------------------------------------------------------------------

/// Rarely-accessed per-object metadata, stored in a side pool keyed by the
/// object's data pointer address.
#[derive(Copy, Clone, Debug, Default)]
pub(crate) struct MoltColdHeader {
    /// Function pointer for polling (generators / async tasks).
    pub(crate) poll_fn: u64,
    /// State machine state (generators / async tasks / hash cache).
    pub(crate) state: i64,
    /// Exact allocation size for objects that exceed the size-class table.
    pub(crate) extended_size: usize,
}

/// Slab allocator for cold headers.  Entries are stored in a contiguous `Vec`
/// and referenced by a `u32` index stored in `MoltHeader::cold_idx`.
/// Index 0 is reserved as "no cold header".  This gives O(1) alloc, access,
/// and free — no hashing, no hash collisions, better cache locality.
struct ColdHeaderSlab {
    /// Slot 0 is unused (sentinel). Valid indices start at 1.
    entries: Vec<MoltColdHeader>,
    /// Slot liveness mirrors `entries` exactly.  A freed slot may be reused only
    /// after `free` has marked it non-live; this makes double-free detection a
    /// slab invariant instead of an allocator-side accident.
    live: Vec<bool>,
    /// Free-list of previously freed indices (LIFO reuse).
    free_list: Vec<u32>,
}

impl ColdHeaderSlab {
    fn new() -> Self {
        Self {
            // Slot 0 is the sentinel — push a dummy entry.
            entries: vec![MoltColdHeader::default()],
            live: vec![false],
            free_list: Vec::new(),
        }
    }

    /// Allocate a slot, returning its u32 index (always >= 1).
    /// Returns 0 if the slab is full (`u32::MAX` live cold headers).
    fn alloc(&mut self, cold: MoltColdHeader) -> u32 {
        while let Some(idx) = self.free_list.pop() {
            // Belt-and-suspenders: verify the recycled index is in bounds.
            // This defends against any residual free-list corruption.
            if (idx as usize) < self.entries.len() {
                if self.live[idx as usize] {
                    panic!(
                        "cold header slab free-list corruption: live slot {} was queued",
                        idx
                    );
                }
                self.entries[idx as usize] = cold;
                self.live[idx as usize] = true;
                return idx;
            }
            // Index was stale/corrupted — discard and fall through to push.
        }
        let idx = self.entries.len();
        if idx > u32::MAX as usize {
            // Slab full — too many live cold-header users.
            // Panic instead of returning 0, which would silently corrupt
            // object state (cold_idx=0 is the "no header" sentinel).
            panic!(
                "cold header slab exhausted ({} entries) — too many live \
                 cold-header users",
                self.entries.len()
            );
        }
        self.entries.push(cold);
        self.live.push(true);
        idx as u32
    }

    /// Get a reference to the cold header at `idx`.
    /// Returns `None` for index 0 (no cold header).
    #[inline]
    fn get(&self, idx: u32) -> Option<&MoltColdHeader> {
        if idx == 0 {
            None
        } else if self.live.get(idx as usize).copied().unwrap_or(false) {
            self.entries.get(idx as usize)
        } else {
            None
        }
    }

    /// Get a mutable reference to the cold header at `idx`.
    /// Returns `None` for index 0 (no cold header).
    #[inline]
    fn get_mut(&mut self, idx: u32) -> Option<&mut MoltColdHeader> {
        if idx == 0 {
            None
        } else if self.live.get(idx as usize).copied().unwrap_or(false) {
            self.entries.get_mut(idx as usize)
        } else {
            None
        }
    }

    /// Free the slot at `idx`, returning it to the free list.
    /// No-op for index 0.
    fn free(&mut self, idx: u32) {
        if idx == 0 {
            return;
        }
        // Zero out the entry to avoid stale data, then recycle.
        // Only push to free_list when the index is actually in bounds —
        // a corrupted cold_idx (e.g. from use-after-free or stale pointer
        // reuse) must not poison the free list.
        if (idx as usize) >= self.entries.len() {
            return;
        }
        if !self.live[idx as usize] {
            panic!("cold header slab double free for slot {}", idx);
        }
        if let Some(entry) = self.entries.get_mut(idx as usize) {
            *entry = MoltColdHeader::default();
            self.live[idx as usize] = false;
            self.free_list.push(idx);
        }
    }
}

static COLD_HEADER_SLAB: OnceLock<Mutex<ColdHeaderSlab>> = OnceLock::new();

fn cold_header_slab() -> &'static Mutex<ColdHeaderSlab> {
    COLD_HEADER_SLAB.get_or_init(|| Mutex::new(ColdHeaderSlab::new()))
}

/// Allocate a cold header, returning its slab index.
/// The caller must store this index in `MoltHeader::cold_idx`.
pub(crate) fn alloc_cold_header(cold: MoltColdHeader) -> u32 {
    let mut slab = cold_header_slab().lock().unwrap();
    slab.alloc(cold)
}

/// High bit of `MoltHeader::cold_idx` flagged as "shared" — many
/// instances of the same class point to one cold header (allocated
/// at first instantiation, reused for every subsequent instance).
/// Eliminates the per-instance `alloc_cold_header` mutex contention
/// in tight allocation loops, and — critically for stack-allocated
/// instances — encodes `class_bits` (which lives in the cold
/// header's `state` field) without per-instance heap allocation.
///
/// When the bit is set:
///   - `object_state` masks it off before slab lookup.
///   - `object_set_state` allocates a *private* cold header for the
///     mutating instance (since shared state cannot be modified
///     without affecting siblings).
///   - `object_set_poll_fn` similarly promotes shared → private.
///   - `free_cold_header_for_obj` is a no-op (the shared cold header
///     outlives any individual instance — only the class itself
///     owns and frees it).
///
/// The slab index is bounded by `COLD_HEADER_SLAB_CAP` which is well
/// below `2^31`, so the bit will never collide with a real index.
pub(crate) const SHARED_COLD_IDX_BIT: u32 = 1 << 31;

/// Mask off the shared-bit to recover the real slab index.
#[inline]
pub(crate) fn cold_idx_real(raw: u32) -> u32 {
    raw & !SHARED_COLD_IDX_BIT
}

/// Returns `true` when the cold_idx is flagged as shared and should
/// not be freed when the owning instance is deallocated.
#[inline]
pub(crate) fn cold_idx_is_shared(raw: u32) -> bool {
    raw & SHARED_COLD_IDX_BIT != 0
}

/// Retrieve a **copy** of the cold header at `idx`.
/// Returns `None` if idx == 0.
#[inline]
pub(crate) fn get_cold_header(idx: u32) -> Option<MoltColdHeader> {
    if idx == 0 {
        return None;
    }
    let slab = cold_header_slab().lock().unwrap();
    slab.get(idx).copied()
}

/// Free the cold header at `idx`, returning the slot to the free list.
/// No-op if idx == 0.
pub(crate) fn free_cold_header(idx: u32) {
    if idx == 0 {
        return;
    }
    let mut slab = cold_header_slab().lock().unwrap();
    slab.free(idx);
}

/// Returns the per-class shared cold-header index for `class_ptr`,
/// allocating one lazily on first call.  The shared cold header
/// stores `class_bits` (the boxed class reference) in its `state`
/// field, so all instances of this class can recover their class
/// via `object_state()` without needing a private cold header.
///
/// The returned value has `SHARED_COLD_IDX_BIT` set, signaling to
/// readers that no per-instance allocation occurred and to the
/// dealloc path that this idx must NOT be freed when the owning
/// instance dies (the class outlives its instances).
///
/// **Per-class cache**: the class object's `MoltHeader::reserved`
/// field — currently unused for typed classes — is repurposed to
/// cache the shared idx.  Accessed atomically; a losing concurrent
/// initializer wastes one cold-header slot but never produces an
/// inconsistent state.
///
/// **Why this works**: `MoltHeader::reserved` is `u32` and the
/// struct is `#[repr(C)]` with 4-byte aligned fields, so an in-place
/// `AtomicU32` view is sound (alignment + size match).
///
/// Safety: `class_ptr` must point to a live class object (TYPE_ID_TYPE).
pub(crate) unsafe fn ensure_shared_cold_idx(class_ptr: *mut u8) -> u32 {
    use std::sync::atomic::{AtomicU32, Ordering};
    unsafe {
        let header = header_from_obj_ptr(class_ptr);
        // SAFETY: `reserved` is a u32 with natural 4-byte alignment;
        // viewing it as AtomicU32 is sound on every supported target.
        let reserved_ptr = std::ptr::addr_of_mut!((*header).reserved) as *const AtomicU32;
        let reserved = &*reserved_ptr;
        let cached = reserved.load(Ordering::Acquire);
        if cached != 0 {
            return cached;
        }
        let class_bits = MoltObject::from_ptr(class_ptr).bits();
        let new_idx = alloc_cold_header(MoltColdHeader {
            state: class_bits as i64,
            ..MoltColdHeader::default()
        });
        let tagged = new_idx | SHARED_COLD_IDX_BIT;
        match reserved.compare_exchange(0, tagged, Ordering::AcqRel, Ordering::Acquire) {
            Ok(_) => tagged,
            Err(winner) => {
                // Lost the race: free our wasted alloc and use the winner's.
                free_cold_header(new_idx);
                winner
            }
        }
    }
}

/// Release the per-class shared cold header cached in `MoltHeader::reserved`.
///
/// Safety: `class_ptr` must point to a class object that is being destroyed.
unsafe fn free_shared_cold_idx_for_class(class_ptr: *mut u8) {
    unsafe {
        let header = header_from_obj_ptr(class_ptr);
        let raw = (*header).reserved;
        if cold_idx_is_shared(raw) {
            (*header).reserved = 0;
            free_cold_header(cold_idx_real(raw));
        }
    }
}

/// Derive the total allocation size from a header's `size_class`.
/// For oversized objects (size_class == 0) the exact size is stored in
/// the cold header's `extended_size`.
#[inline]
pub(crate) fn total_size_from_header_fields(size_class: u16, cold_idx: u32) -> usize {
    let sc = size_class as usize;
    if sc != 0 && sc < SIZE_CLASS_TABLE.len() {
        SIZE_CLASS_TABLE[sc]
    } else {
        // Oversized: look up cold header by slab index.  Strip the
        // shared bit before lookup — oversized objects don't share
        // cold headers (per-instance `extended_size` differs), but
        // the strip is harmless when the bit is clear.
        get_cold_header(cold_idx_real(cold_idx))
            .map(|c| c.extended_size)
            .unwrap_or(0)
    }
}

#[inline]
pub(crate) fn total_size_from_header(header: &MoltHeader, _data_ptr: *mut u8) -> usize {
    total_size_from_header_fields(header.size_class, header.cold_idx)
}

#[derive(Clone, Copy)]
struct ObjectAllocationPlan {
    alloc_size: usize,
    layout: Layout,
    size_class: u16,
}

#[inline]
pub(crate) fn checked_object_total_size(payload_size: usize) -> Option<usize> {
    payload_size.checked_add(std::mem::size_of::<MoltHeader>())
}

#[inline]
fn object_allocation_plan(total_size: usize) -> Option<ObjectAllocationPlan> {
    if total_size < std::mem::size_of::<MoltHeader>() {
        return None;
    }
    let size_class = size_class_for(total_size);
    let alloc_size = if size_class != 0 {
        SIZE_CLASS_TABLE.get(size_class as usize).copied()?
    } else {
        total_size
    };
    let layout = Layout::from_size_align(alloc_size, 8).ok()?;
    Some(ObjectAllocationPlan {
        alloc_size,
        layout,
        size_class,
    })
}

#[inline]
fn reserve_object_allocation(plan: ObjectAllocationPlan) -> bool {
    crate::resource::with_tracker(|t| t.on_allocate(plan.alloc_size)).is_ok()
}

#[inline]
fn release_object_allocation_reservation(plan: ObjectAllocationPlan) {
    let _ = crate::resource::try_with_tracker(|t| t.on_free(plan.alloc_size));
}

/// Get the poll_fn for an object. Returns 0 if no cold header exists.
/// Strips the shared bit since the shared cold header carries
/// `state=class_bits` only — `poll_fn` is generator/coroutine state
/// and a class instance reading `poll_fn` from a shared cold header
/// observes the (zero-initialised) shared `poll_fn` field, which is
/// the correct semantics for "no live coroutine state."
#[inline]
pub(crate) fn object_poll_fn(data_ptr: *mut u8) -> u64 {
    let raw_idx = unsafe { (*header_from_obj_ptr(data_ptr)).cold_idx };
    get_cold_header(cold_idx_real(raw_idx))
        .map(|c| c.poll_fn)
        .unwrap_or(0)
}

/// Set the poll_fn for an object, creating a cold header if needed.
/// Mirrors `object_set_state`'s shared→private promotion: writing
/// to a shared cold header would corrupt every sibling instance.
pub(crate) fn object_set_poll_fn(data_ptr: *mut u8, poll_fn: u64) {
    unsafe {
        let header = header_from_obj_ptr(data_ptr);
        let raw_idx = (*header).cold_idx;
        if cold_idx_is_shared(raw_idx) {
            // Promote: shared → private.  We don't preserve the
            // shared state's `state` field here — the only value
            // stored in shared cold headers is `class_bits`, and
            // setting `poll_fn` is unrelated; the new private cold
            // header has state=0, which is semantically "no class
            // bits stored" (the class_bits will be re-stored via
            // object_set_class_bits when needed).  In practice
            // poll_fn is only set on coroutine/generator objects,
            // not on user-class instances, so this branch is
            // effectively unreachable for shared cold idx.
            let new_idx = alloc_cold_header(MoltColdHeader {
                poll_fn,
                ..MoltColdHeader::default()
            });
            (*header).cold_idx = new_idx;
            return;
        }
        if raw_idx != 0 {
            let mut slab = cold_header_slab().lock().unwrap();
            if let Some(entry) = slab.get_mut(raw_idx) {
                entry.poll_fn = poll_fn;
            }
        } else {
            // Lazily allocate a cold header.
            let new_idx = alloc_cold_header(MoltColdHeader {
                poll_fn,
                ..MoltColdHeader::default()
            });
            (*header).cold_idx = new_idx;
        }
    }
}

/// Get the state for an object. Returns 0 if no cold header exists.
/// Strips the `SHARED_COLD_IDX_BIT` so shared cold headers
/// (allocated once per class for typed-class instances) read
/// transparently — both heap-alloc and stack-alloc instances of the
/// same class observe the class's stored `class_bits`.
#[inline]
pub(crate) fn object_state(data_ptr: *mut u8) -> i64 {
    let raw_idx = unsafe { (*header_from_obj_ptr(data_ptr)).cold_idx };
    get_cold_header(cold_idx_real(raw_idx))
        .map(|c| c.state)
        .unwrap_or(0)
}

/// Set the state for an object, creating a cold header if needed.
/// When the current cold_idx is the class's shared cold header, this
/// must allocate a *private* cold header instead — otherwise we'd
/// corrupt every sibling instance's state.  The new private cold
/// header inherits the shared state's value as a starting point so
/// callers that read-modify-write semantics are preserved (e.g.
/// generator-state transitions on instances that store class_bits).
pub(crate) fn object_set_state(data_ptr: *mut u8, state: i64) {
    unsafe {
        let header = header_from_obj_ptr(data_ptr);
        let raw_idx = (*header).cold_idx;
        if cold_idx_is_shared(raw_idx) {
            // Promote: shared → private.  The shared cold header
            // remains alive (still referenced by the class and
            // siblings); we simply give this instance its own.
            let new_idx = alloc_cold_header(MoltColdHeader {
                state,
                ..MoltColdHeader::default()
            });
            (*header).cold_idx = new_idx;
            return;
        }
        if raw_idx != 0 {
            let mut slab = cold_header_slab().lock().unwrap();
            if let Some(entry) = slab.get_mut(raw_idx) {
                entry.state = state;
            }
        } else {
            // Lazily allocate a cold header.
            let new_idx = alloc_cold_header(MoltColdHeader {
                state,
                ..MoltColdHeader::default()
            });
            (*header).cold_idx = new_idx;
        }
    }
}

// ---------------------------------------------------------------------------
// C API wrappers for cold-header state access (used by the native JIT backend).
// State was moved from the inline MoltHeader to MoltColdHeader, so the JIT can
// no longer do inline loads/stores — it must call through these functions.
// ---------------------------------------------------------------------------

/// Read the generator/coroutine state for the object at `data_ptr`.
/// Returns the state value (0 if no cold header exists).
#[unsafe(no_mangle)]
pub extern "C" fn molt_obj_get_state(data_ptr_bits: u64) -> i64 {
    let data_ptr = data_ptr_bits as usize as *mut u8;
    let state = object_state(data_ptr);
    if trace_object_state() {
        eprintln!(
            "molt object_state get ptr=0x{:x} state={}",
            data_ptr as usize, state
        );
    }
    state
}

/// Write the generator/coroutine state for the object at `data_ptr`.
#[unsafe(no_mangle)]
pub extern "C" fn molt_obj_set_state(data_ptr_bits: u64, state: i64) {
    let data_ptr = data_ptr_bits as usize as *mut u8;
    if trace_object_state() {
        eprintln!(
            "molt object_state set ptr=0x{:x} state={}",
            data_ptr as usize, state
        );
    }
    object_set_state(data_ptr, state);
}

/// Initialize a stack-allocated MoltObject in-place.  Used by the
/// native backend's `object_new_bound_stack` lowering: Cranelift
/// allocates a `StackSlot` of size `MoltHeader::SIZE +
/// payload_size_bytes` and calls into this helper to:
/// - zero the payload (StackSlot contents are undefined on entry,
///   so this is mandatory for soundness — a stale pointer in a
///   slot would corrupt subsequent `dec_ref` / `has_ptrs`
///   traversal),
/// - stamp the MoltHeader fields:
///     - `type_id        = TYPE_ID_OBJECT`
///     - `ref_count      = 1` (paired with IMMORTAL — never
///       decrements)
///     - `flags          = HEADER_FLAG_IMMORTAL |
///       HEADER_FLAG_SKIP_CLASS_DECREF` (so dec_ref_ptr
///       short-circuits and the runtime never tries to free a
///       stack pointer through the dealloc path; the class is
///       borrowed from the module-owned class object)
///     - `size_class     = 0`  (size lives nowhere — IMMORTAL
///       objects bypass the size lookup paths)
///     - `cold_idx       = ensure_shared_cold_idx(cls_ptr)`
///       (per-class shared cold header storing class_bits in
///       `state`; reads via `object_class_bits()` work
///       transparently)
///     - `reserved       = 0`
/// - return the tagged data pointer bits (header_ptr + 24).
///
/// Returns `MoltObject::none().bits()` if `cls_bits` does not point
/// to a valid type object.  The frontend gates the fold on
/// known-class identity, so this branch is the defense-in-depth
/// fallback rather than an expected runtime path.
///
/// **No class inc-ref**: we deliberately skip `inc_ref_bits(class)`
/// because (a) the class is module-resident and outlives the
/// function frame containing the StackSlot, (b) the symmetric
/// dec-ref on instance death would never run (IMMORTAL skips
/// dec_ref_ptr), so a balanced inc/dec would be lossy bookkeeping.
///
/// Safety: `header_ptr` must point to writable memory of at least
/// `MoltHeader::SIZE + payload_size_bytes` bytes, 8-byte aligned.
/// The Cranelift StackSlot allocation guarantees this.
#[unsafe(no_mangle)]
pub extern "C" fn molt_object_init_stack(
    header_ptr: *mut u8,
    cls_bits: u64,
    payload_size_bytes: u64,
) -> u64 {
    if header_ptr.is_null() {
        return MoltObject::none().bits();
    }
    let cls_ptr = match obj_from_bits(cls_bits).as_ptr() {
        Some(p) => p,
        None => return MoltObject::none().bits(),
    };
    unsafe {
        if object_type_id(cls_ptr) != TYPE_ID_TYPE {
            return MoltObject::none().bits();
        }
        let payload = payload_size_bytes as usize;
        let total = std::mem::size_of::<MoltHeader>() + payload;
        std::ptr::write_bytes(header_ptr, 0, total);
        let header = header_ptr as *mut MoltHeader;
        (*header).type_id = TYPE_ID_OBJECT;
        // ref_count is wrapped in MoltRefCount; replace whole field.
        std::ptr::write(
            std::ptr::addr_of_mut!((*header).ref_count),
            MoltRefCount::new(1),
        );
        (*header).flags = HEADER_FLAG_IMMORTAL | HEADER_FLAG_SKIP_CLASS_DECREF;
        (*header).size_class = 0;
        (*header).cold_idx = ensure_shared_cold_idx(cls_ptr);
        (*header).reserved = 0;
        let data_ptr = header_ptr.add(std::mem::size_of::<MoltHeader>());
        MoltObject::from_ptr(data_ptr).bits()
    }
}

/// C-ABI entry point for the Cranelift inline stack-alloc fast
/// path: decode `cls_bits` to a class pointer, verify it is a
/// `TYPE_ID_TYPE` object, then delegate to `ensure_shared_cold_idx`
/// which atomically allocates-or-reuses the per-class shared cold
/// header.  Returns the raw `u32` index (with `SHARED_COLD_IDX_BIT`
/// set) so the caller can store it directly into
/// `MoltHeader::cold_idx` at slot offset 16.
///
/// Returns `0` (the "no cold header" sentinel) when `cls_bits` is
/// not a valid pointer or the object is not a type — the codegen
/// guarantees neither branch is reachable on the typed-class
/// fast path (the frontend gates the OBJECT_NEW_BOUND fold on a
/// known-class identity at compile time), so the fallbacks are
/// defense-in-depth rather than expected runtime paths.
///
/// **Why a separate C-API**: the Cranelift inline stack-alloc
/// codegen replaces a single C-call to `molt_object_init_stack`
/// with inline header-stamping stores, but the cold-header
/// allocation can't be inlined — it does an atomic
/// compare-exchange against the class's `MoltHeader::reserved`
/// field plus a slab alloc on the cache-miss path.  Splitting it
/// into its own entry point lets the codegen issue exactly one
/// runtime call (for the cold idx) instead of going through the
/// full init_stack helper.
///
/// Safety: the caller (the codegen) guarantees `cls_bits` encodes
/// a live heap object that remains alive for the duration of the
/// call — the class is module-owned and never freed during a
/// function execution.
#[unsafe(no_mangle)]
pub extern "C" fn molt_ensure_shared_cold_idx(cls_bits: u64) -> u32 {
    let cls_ptr = match obj_from_bits(cls_bits).as_ptr() {
        Some(p) => p,
        None => return 0,
    };
    unsafe {
        if object_type_id(cls_ptr) != TYPE_ID_TYPE {
            return 0;
        }
        ensure_shared_cold_idx(cls_ptr)
    }
}

#[inline(always)]
pub(crate) fn obj_from_bits(bits: u64) -> MoltObject {
    MoltObject::from_bits(bits)
}

#[inline(always)]
pub(crate) fn inc_ref_bits(_py: &PyToken<'_>, bits: u64) {
    let obj = obj_from_bits(bits);
    if let Some(ptr) = obj.as_ptr() {
        unsafe { inc_ref_ptr(_py, ptr) };
    }
}

#[inline(always)]
pub(crate) fn dec_ref_bits(_py: &PyToken<'_>, bits: u64) {
    let obj = obj_from_bits(bits);
    if let Some(ptr) = obj.as_ptr() {
        unsafe { dec_ref_ptr(_py, ptr) };
    }
}

pub(crate) fn release_shutdown_owned_bits(_py: &PyToken<'_>, bits: u64) {
    let obj = obj_from_bits(bits);
    let Some(ptr) = obj.as_ptr() else {
        return;
    };
    unsafe {
        let header_ptr = ptr.sub(std::mem::size_of::<MoltHeader>()) as *mut MoltHeader;
        if (*header_ptr).ref_count.load(AtomicOrdering::Acquire) == u32::MAX {
            (*header_ptr).ref_count.store(1, AtomicOrdering::Release);
        }
        (*header_ptr).flags &= !(HEADER_FLAG_IMMORTAL | HEADER_FLAG_INTERNED);
    }
    dec_ref_bits(_py, bits);
}

pub(crate) fn release_shutdown_bits(_py: &PyToken<'_>, bits: u64) {
    let obj = obj_from_bits(bits);
    let Some(ptr) = obj.as_ptr() else {
        return;
    };
    unsafe {
        let header_ptr = ptr.sub(std::mem::size_of::<MoltHeader>()) as *mut MoltHeader;
        if ((*header_ptr).flags & HEADER_FLAG_INTERNED) != 0 {
            return;
        }
    }
    release_shutdown_owned_bits(_py, bits);
}

pub(crate) fn init_atomic_bits(
    py: &PyToken<'_>,
    slot: &AtomicU64,
    init: impl FnOnce() -> u64,
) -> u64 {
    let existing = slot.load(AtomicOrdering::Acquire);
    if existing != 0 {
        return existing;
    }
    let new_bits = init();
    if new_bits == 0 {
        return 0;
    }
    match slot.compare_exchange(0, new_bits, AtomicOrdering::AcqRel, AtomicOrdering::Acquire) {
        Ok(_) => new_bits,
        Err(prev) => {
            dec_ref_bits(py, new_bits);
            prev
        }
    }
}

pub(crate) fn pending_bits_i64() -> i64 {
    MoltObject::pending().bits() as i64
}

pub(crate) fn alloc_object_zeroed(_py: &PyToken<'_>, total_size: usize, type_id: u32) -> *mut u8 {
    crate::gil_assert();
    let Some(plan) = object_allocation_plan(total_size) else {
        if debug_oom() {
            eprintln!(
                "molt OOM alloc_object_zeroed type_id={} invalid total_size={}",
                type_id, total_size
            );
        }
        return std::ptr::null_mut();
    };
    if !reserve_object_allocation(plan) {
        return std::ptr::null_mut();
    }
    unsafe {
        let ptr = std::alloc::alloc_zeroed(plan.layout);
        if ptr.is_null() {
            release_object_allocation_reservation(plan);
            if debug_oom() {
                eprintln!(
                    "molt OOM alloc_object_zeroed type_id={} total_size={}",
                    type_id, total_size
                );
            }
            return std::ptr::null_mut();
        }
        profile_hit(_py, &ALLOC_COUNT);
        profile_hit_bytes(_py, &ALLOC_BYTES_TOTAL, plan.alloc_size as u64);
        profile_alloc_type(_py, type_id);
        profile_alloc_type_bytes(_py, type_id, plan.alloc_size);
        let header = ptr as *mut MoltHeader;
        (*header).type_id = type_id;
        (*header).ref_count.store(1, AtomicOrdering::Relaxed);
        (*header).flags = 0;
        (*header).size_class = plan.size_class;
        (*header).cold_idx = if plan.size_class == 0 {
            alloc_cold_header(MoltColdHeader {
                poll_fn: 0,
                state: 0,
                extended_size: total_size,
            })
        } else {
            0
        };
        ptr.add(std::mem::size_of::<MoltHeader>())
    }
}

pub(crate) fn alloc_object(_py: &PyToken<'_>, total_size: usize, type_id: u32) -> *mut u8 {
    if debug_alloc_object()
        && debug_alloc_object_type()
            .map(|filter| filter == type_id)
            .unwrap_or(true)
    {
        eprintln!(
            "molt debug alloc_object type_id={} total_size={} gil_held={}",
            type_id,
            total_size,
            crate::gil_held()
        );
    }
    crate::gil_assert();
    let Some(plan) = object_allocation_plan(total_size) else {
        if debug_oom() {
            eprintln!(
                "molt OOM alloc_object type_id={} invalid total_size={}",
                type_id, total_size
            );
        }
        return std::ptr::null_mut();
    };
    if debug_alloc_list_builder() && type_id == TYPE_ID_LIST_BUILDER {
        let expected = std::mem::size_of::<MoltHeader>() + std::mem::size_of::<*mut Vec<u64>>();
        eprintln!(
            "molt debug alloc_list_builder: total_size={} expected={}",
            total_size, expected
        );
    }
    if !reserve_object_allocation(plan) {
        return std::ptr::null_mut();
    }
    let header_ptr = unsafe { std::alloc::alloc(plan.layout) };
    if header_ptr.is_null() {
        release_object_allocation_reservation(plan);
        if debug_oom() {
            eprintln!(
                "molt OOM alloc_object type_id={} total_size={}",
                type_id, total_size
            );
        }
        return std::ptr::null_mut();
    }
    profile_hit(_py, &ALLOC_COUNT);
    profile_hit_bytes(_py, &ALLOC_BYTES_TOTAL, plan.alloc_size as u64);
    profile_alloc_type(_py, type_id);
    profile_alloc_type_bytes(_py, type_id, plan.alloc_size);
    unsafe {
        // Zero the entire allocation so data fields past the header
        // start as null pointers / zero values.  This prevents the
        // deallocation path from misinterpreting stale heap data as
        // valid inner pointers (Vec*, DataclassDesc*, etc.) when an
        // object type allocates more space than it initializes.
        std::ptr::write_bytes(header_ptr, 0, plan.alloc_size);
        let header = header_ptr as *mut MoltHeader;
        (*header).type_id = type_id;
        (*header).ref_count.store(1, AtomicOrdering::Relaxed);
        // flags, size_class, cold_idx are already 0 from write_bytes
        (*header).size_class = plan.size_class;
        if plan.size_class == 0 {
            (*header).cold_idx = alloc_cold_header(MoltColdHeader {
                poll_fn: 0,
                state: 0,
                extended_size: total_size,
            });
        }
        header_ptr.add(std::mem::size_of::<MoltHeader>())
    }
}

#[inline(always)]
pub(crate) unsafe fn header_from_obj_ptr(ptr: *mut u8) -> *mut MoltHeader {
    unsafe { ptr.sub(std::mem::size_of::<MoltHeader>()) as *mut MoltHeader }
}

// On wasm32 profile_hit is a guaranteed no-op, so inline this function to let
// the compiler eliminate the entire match body during dead-code elimination.
#[cfg_attr(target_arch = "wasm32", inline(always))]
fn profile_alloc_type(_py: &PyToken<'_>, type_id: u32) {
    match type_id {
        TYPE_ID_OBJECT => profile_hit(_py, &ALLOC_OBJECT_COUNT),
        TYPE_ID_EXCEPTION => profile_hit(_py, &ALLOC_EXCEPTION_COUNT),
        TYPE_ID_DICT => profile_hit(_py, &ALLOC_DICT_COUNT),
        TYPE_ID_TUPLE => profile_hit(_py, &ALLOC_TUPLE_COUNT),
        TYPE_ID_STRING => profile_hit(_py, &ALLOC_STRING_COUNT),
        TYPE_ID_CALLARGS => profile_hit(_py, &ALLOC_CALLARGS_COUNT),
        _ => {}
    }
}

#[cfg_attr(target_arch = "wasm32", inline(always))]
fn profile_alloc_type_bytes(_py: &PyToken<'_>, type_id: u32, total_size: usize) {
    let bytes = total_size as u64;
    match type_id {
        TYPE_ID_DICT => profile_hit_bytes(_py, &ALLOC_BYTES_DICT, bytes),
        TYPE_ID_TUPLE => profile_hit_bytes(_py, &ALLOC_BYTES_TUPLE, bytes),
        TYPE_ID_STRING => profile_hit_bytes(_py, &ALLOC_BYTES_STRING, bytes),
        TYPE_ID_LIST | TYPE_ID_LIST_BUILDER => profile_hit_bytes(_py, &ALLOC_BYTES_LIST, bytes),
        _ => {}
    }
}

/// Per-type dealloc counter dispatch (RC drop-insertion substrate, design 20).
/// Mirrors [`profile_alloc_type`]: called from the `dec_ref_ptr` zero-transition
/// so a leak in the `live = alloc - dealloc` gauge can be attributed to a
/// concrete object family.
#[cfg_attr(target_arch = "wasm32", inline(always))]
fn profile_dealloc_type(_py: &PyToken<'_>, type_id: u32) {
    match type_id {
        TYPE_ID_OBJECT => profile_hit(_py, &DEALLOC_OBJECT_COUNT),
        TYPE_ID_BIGINT => profile_hit(_py, &DEALLOC_BIGINT_COUNT),
        TYPE_ID_STRING => profile_hit(_py, &DEALLOC_STRING_COUNT),
        TYPE_ID_DICT => profile_hit(_py, &DEALLOC_DICT_COUNT),
        TYPE_ID_TUPLE => profile_hit(_py, &DEALLOC_TUPLE_COUNT),
        _ => {}
    }
}

#[inline(always)]
pub(crate) unsafe fn object_type_id(ptr: *mut u8) -> u32 {
    unsafe { (*header_from_obj_ptr(ptr)).type_id }
}

pub(crate) unsafe fn object_payload_size(ptr: *mut u8) -> usize {
    unsafe {
        let header = &*header_from_obj_ptr(ptr);
        total_size_from_header(header, ptr).saturating_sub(std::mem::size_of::<MoltHeader>())
    }
}

pub(crate) unsafe fn instance_dict_bits_ptr(ptr: *mut u8) -> *mut u64 {
    unsafe {
        // Only `TYPE_ID_OBJECT` instances reserve a trailing `__dict__` slot in their payload.
        // Calling this on other builtins (int/str/tuple/etc.) is UB (and can misalign).
        if object_type_id(ptr) != TYPE_ID_OBJECT {
            return std::ptr::null_mut();
        }
        let payload = object_payload_size(ptr);
        if payload < std::mem::size_of::<u64>() {
            return std::ptr::null_mut();
        }
        ptr.add(payload - std::mem::size_of::<u64>()) as *mut u64
    }
}

pub(crate) unsafe fn instance_dict_bits(ptr: *mut u8) -> u64 {
    unsafe {
        let slot = instance_dict_bits_ptr(ptr);
        if slot.is_null() {
            return 0;
        }
        *slot
    }
}

pub(crate) unsafe fn instance_set_dict_bits(_py: &PyToken<'_>, ptr: *mut u8, bits: u64) {
    unsafe {
        crate::gil_assert();
        let slot = instance_dict_bits_ptr(ptr);
        if slot.is_null() {
            return;
        }
        *slot = bits;
        // Materializing a non-zero __dict__ stores a pointer in the
        // trailing dict slot; mark `HEADER_FLAG_HAS_PTRS` so the
        // codegen-side store fast path (which uses HAS_PTRS as a
        // proxy for "no live pointer slot needs sync") falls back to
        // the runtime helper that performs the dict sync.  Clearing
        // (`bits == 0`) does not need the flag set since clearing
        // does not introduce a pointer slot.
        if bits != 0 {
            object_mark_has_ptrs(_py, ptr);
        }
    }
}

unsafe fn object_class_bits_from_state(state: i64) -> u64 {
    let bits = state as u64;
    if bits == 0 {
        return 0;
    }
    // Some TYPE_ID_OBJECT futures/tasks repurpose `state` for runtime state.
    // Treat it as a class only when it points to an actual type object.
    let Some(class_ptr) = obj_from_bits(bits).as_ptr() else {
        return 0;
    };
    if unsafe { object_type_id(class_ptr) } != TYPE_ID_TYPE {
        return 0;
    }
    bits
}

pub(crate) unsafe fn object_class_bits(ptr: *mut u8) -> u64 {
    let state = object_state(ptr);
    unsafe { object_class_bits_from_state(state) }
}

#[inline]
pub(crate) unsafe fn object_is_exact_builtin_dict(_py: &PyToken<'_>, ptr: *mut u8) -> bool {
    if unsafe { object_type_id(ptr) } != TYPE_ID_DICT {
        return false;
    }
    let class_bits = unsafe { object_class_bits(ptr) };
    class_bits == 0
        || builtin_classes_if_initialized(_py).is_some_and(|builtins| class_bits == builtins.dict)
}

pub(crate) unsafe fn object_set_class_bits(_py: &PyToken<'_>, ptr: *mut u8, bits: u64) {
    crate::gil_assert();
    object_set_state(ptr, bits as i64);
    unsafe {
        apply_instance_finalizer_flag_from_class(ptr, bits);
    }
}

#[inline]
unsafe fn class_header_has_finalizer(class_ptr: *mut u8) -> bool {
    unsafe {
        object_type_id(class_ptr) == TYPE_ID_TYPE
            && ((*header_from_obj_ptr(class_ptr)).flags & HEADER_FLAG_CLASS_HAS_FINALIZER) != 0
    }
}

unsafe fn apply_instance_finalizer_flag_from_class(ptr: *mut u8, class_bits: u64) {
    unsafe {
        let header = header_from_obj_ptr(ptr);
        if (*header).type_id != TYPE_ID_OBJECT {
            return;
        }
        let has_finalizer = obj_from_bits(class_bits)
            .as_ptr()
            .is_some_and(|class_ptr| class_header_has_finalizer(class_ptr));
        if has_finalizer {
            (*header).flags |= HEADER_FLAG_INSTANCE_HAS_FINALIZER;
        } else {
            (*header).flags &= !HEADER_FLAG_INSTANCE_HAS_FINALIZER;
        }
    }
}

unsafe fn class_lookup_raw_mro_dict_attr(
    _py: &PyToken<'_>,
    class_ptr: *mut u8,
    attr_bits: u64,
) -> Option<u64> {
    unsafe {
        let visit = |candidate_bits: u64| -> Option<u64> {
            let candidate_ptr = obj_from_bits(candidate_bits).as_ptr()?;
            if object_type_id(candidate_ptr) != TYPE_ID_TYPE {
                return None;
            }
            let dict_bits = layout::class_dict_bits(candidate_ptr);
            let dict_ptr = obj_from_bits(dict_bits).as_ptr()?;
            if object_type_id(dict_ptr) != TYPE_ID_DICT {
                return None;
            }
            crate::dict_get_in_place(_py, dict_ptr, attr_bits)
        };

        let mro_bits = layout::class_mro_bits(class_ptr);
        if let Some(mro_ptr) = obj_from_bits(mro_bits).as_ptr()
            && object_type_id(mro_ptr) == TYPE_ID_TUPLE
        {
            for class_bits in (*seq_vec_ptr(mro_ptr)).iter().copied() {
                if let Some(bits) = visit(class_bits) {
                    return Some(bits);
                }
            }
            return None;
        }
        visit(MoltObject::from_ptr(class_ptr).bits())
    }
}

pub(crate) unsafe fn class_refresh_finalizer_flag(_py: &PyToken<'_>, class_ptr: *mut u8) {
    unsafe {
        crate::gil_assert();
        if object_type_id(class_ptr) != TYPE_ID_TYPE {
            return;
        }
        let Some(del_name_bits) = crate::attr_name_bits_from_bytes(_py, b"__del__") else {
            return;
        };
        let has_finalizer = class_lookup_raw_mro_dict_attr(_py, class_ptr, del_name_bits).is_some();
        dec_ref_bits(_py, del_name_bits);

        let header = header_from_obj_ptr(class_ptr);
        if has_finalizer {
            (*header).flags |= HEADER_FLAG_CLASS_HAS_FINALIZER;
        } else {
            (*header).flags &= !HEADER_FLAG_CLASS_HAS_FINALIZER;
        }
    }
}

/// Seal a class object after bulk definition has materialized its namespace,
/// bases/MRO, and layout metadata.
///
/// Dynamic class attribute mutation still refreshes its own derived facts at the
/// mutation point. Bulk class construction can bypass those setters with raw
/// namespace copies, so every creation path routes through this single seal
/// before instances may be allocated from the class.
pub(crate) unsafe fn class_finish_definition(_py: &PyToken<'_>, class_ptr: *mut u8) {
    unsafe {
        class_refresh_finalizer_flag(_py, class_ptr);
    }
}

pub(crate) unsafe fn object_mark_has_ptrs(_py: &PyToken<'_>, ptr: *mut u8) {
    unsafe {
        crate::gil_assert();
        (*header_from_obj_ptr(ptr)).flags |= HEADER_FLAG_HAS_PTRS;
    }
}

#[inline(always)]
pub(crate) unsafe fn string_len(ptr: *mut u8) -> usize {
    unsafe { *(ptr as *const usize) }
}

#[inline(always)]
pub(crate) unsafe fn string_bytes(ptr: *mut u8) -> *const u8 {
    unsafe { ptr.add(std::mem::size_of::<usize>()) }
}

#[inline(always)]
pub(crate) unsafe fn bytes_len(ptr: *mut u8) -> usize {
    unsafe {
        if object_type_id(ptr) == TYPE_ID_BYTEARRAY {
            return bytearray_len(ptr);
        }
        string_len(ptr)
    }
}

pub(crate) unsafe fn intarray_len(ptr: *mut u8) -> usize {
    unsafe { *(ptr as *const usize) }
}

pub(crate) unsafe fn intarray_data(ptr: *mut u8) -> *const i64 {
    unsafe { ptr.add(std::mem::size_of::<usize>()) as *const i64 }
}

pub(crate) unsafe fn intarray_slice(ptr: *mut u8) -> &'static [i64] {
    unsafe { std::slice::from_raw_parts(intarray_data(ptr), intarray_len(ptr)) }
}

pub(crate) unsafe fn bytes_data(ptr: *mut u8) -> *const u8 {
    unsafe {
        if object_type_id(ptr) == TYPE_ID_BYTEARRAY {
            return bytearray_data(ptr);
        }
        string_bytes(ptr)
    }
}

pub(crate) unsafe fn memoryview_ptr(ptr: *mut u8) -> *mut MemoryView {
    ptr as *mut MemoryView
}

pub(crate) unsafe fn memoryview_owner_bits(ptr: *mut u8) -> u64 {
    unsafe { (*memoryview_ptr(ptr)).owner_bits }
}

pub(crate) unsafe fn memoryview_offset(ptr: *mut u8) -> isize {
    unsafe { (*memoryview_ptr(ptr)).offset }
}

pub(crate) unsafe fn memoryview_len(ptr: *mut u8) -> usize {
    unsafe { (*memoryview_ptr(ptr)).len }
}

pub(crate) unsafe fn memoryview_itemsize(ptr: *mut u8) -> usize {
    unsafe { (*memoryview_ptr(ptr)).itemsize }
}

pub(crate) unsafe fn memoryview_stride(ptr: *mut u8) -> isize {
    unsafe { (*memoryview_ptr(ptr)).stride }
}

pub(crate) unsafe fn memoryview_readonly(ptr: *mut u8) -> bool {
    unsafe { (*memoryview_ptr(ptr)).readonly != 0 }
}

pub(crate) unsafe fn memoryview_ndim(ptr: *mut u8) -> usize {
    unsafe { (*memoryview_ptr(ptr)).ndim as usize }
}

pub(crate) unsafe fn memoryview_format_bits(ptr: *mut u8) -> u64 {
    unsafe { (*memoryview_ptr(ptr)).format_bits }
}

pub(crate) unsafe fn memoryview_shape_ptr(ptr: *mut u8) -> *mut Vec<isize> {
    unsafe { (*memoryview_ptr(ptr)).shape_ptr }
}

pub(crate) unsafe fn memoryview_strides_ptr(ptr: *mut u8) -> *mut Vec<isize> {
    unsafe { (*memoryview_ptr(ptr)).strides_ptr }
}

pub(crate) unsafe fn memoryview_shape(ptr: *mut u8) -> Option<&'static [isize]> {
    unsafe {
        let shape_ptr = memoryview_shape_ptr(ptr);
        if shape_ptr.is_null() {
            return None;
        }
        Some(&*shape_ptr)
    }
}

pub(crate) unsafe fn memoryview_strides(ptr: *mut u8) -> Option<&'static [isize]> {
    unsafe {
        let strides_ptr = memoryview_strides_ptr(ptr);
        if strides_ptr.is_null() {
            return None;
        }
        Some(&*strides_ptr)
    }
}

pub(crate) unsafe fn dataclass_desc_ptr(ptr: *mut u8) -> *mut DataclassDesc {
    unsafe { *(ptr as *const *mut DataclassDesc) }
}

pub(crate) unsafe fn dataclass_fields_ptr(ptr: *mut u8) -> *mut Vec<u64> {
    unsafe { *(ptr.add(std::mem::size_of::<*mut DataclassDesc>()) as *const *mut Vec<u64>) }
}

pub(crate) unsafe fn dataclass_fields_ref(ptr: *mut u8) -> &'static Vec<u64> {
    unsafe { &*dataclass_fields_ptr(ptr) }
}

pub(crate) unsafe fn dataclass_fields_mut(ptr: *mut u8) -> &'static mut Vec<u64> {
    unsafe { &mut *dataclass_fields_ptr(ptr) }
}

pub(crate) unsafe fn dataclass_dict_bits_ptr(ptr: *mut u8) -> *mut u64 {
    unsafe {
        ptr.add(std::mem::size_of::<*mut DataclassDesc>() + std::mem::size_of::<*mut Vec<u64>>())
            as *mut u64
    }
}

pub(crate) unsafe fn dataclass_dict_bits(ptr: *mut u8) -> u64 {
    unsafe { *dataclass_dict_bits_ptr(ptr) }
}

pub(crate) unsafe fn dataclass_set_dict_bits(_py: &PyToken<'_>, ptr: *mut u8, bits: u64) {
    unsafe {
        crate::gil_assert();
        *dataclass_dict_bits_ptr(ptr) = bits;
        if bits != 0 {
            object_mark_has_ptrs(_py, ptr);
        }
    }
}

pub(crate) unsafe fn buffer2d_ptr(ptr: *mut u8) -> *mut Buffer2D {
    unsafe { *(ptr as *const *mut Buffer2D) }
}

/// Boxed `GlobIterState` pointer stored at payload offset 0 of a
/// `TYPE_ID_GLOB_ITER` object (mirrors `buffer2d_ptr`).
pub(crate) unsafe fn glob_iter_state_ptr(
    ptr: *mut u8,
) -> *mut crate::builtins::io_path_utils::GlobIterState {
    unsafe { *(ptr as *const *mut crate::builtins::io_path_utils::GlobIterState) }
}

pub(crate) unsafe fn file_handle_ptr(ptr: *mut u8) -> *mut MoltFileHandle {
    unsafe { *(ptr as *const *mut MoltFileHandle) }
}

pub(crate) fn maybe_ptr_from_bits(bits: u64) -> Option<*mut u8> {
    let obj = obj_from_bits(bits);
    obj.as_ptr()
}

#[inline(always)]
pub(crate) fn ptr_from_bits(bits: u64) -> *mut u8 {
    let obj = obj_from_bits(bits);
    if obj.is_ptr() {
        return obj.as_ptr().unwrap_or(std::ptr::null_mut());
    }
    if let Some(addr) = obj.as_int()
        && addr >= 0
        && let Some(ptr) = resolve_ptr(addr as u64)
    {
        return ptr;
    }
    resolve_ptr(bits).unwrap_or(std::ptr::null_mut())
}

#[inline(always)]
pub(crate) fn bits_from_ptr(ptr: *mut u8) -> u64 {
    MoltObject::from_ptr(ptr).bits()
}

/// # Safety
/// Dereferences raw pointer to increment ref count.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn molt_inc_ref(ptr: *mut u8) {
    unsafe {
        crate::with_gil_entry_nopanic!(_py, {
            inc_ref_ptr(_py, ptr);
        })
    }
}

/// # Safety
/// Dereferences raw pointer to decrement ref count. Frees memory if count reaches 0.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn molt_dec_ref(ptr: *mut u8) {
    unsafe {
        crate::with_gil_entry_nopanic!(_py, {
            dec_ref_ptr(_py, ptr);
        })
    }
}

/// # Safety
/// Dereferences raw pointer to increment ref count.
#[inline(always)]
pub(crate) unsafe fn inc_ref_ptr(_py: &PyToken<'_>, ptr: *mut u8) {
    unsafe {
        crate::gil_assert();
        if ptr.is_null() {
            return;
        }
        let header_ptr = ptr.sub(std::mem::size_of::<MoltHeader>()) as *mut MoltHeader;
        let type_id = (*header_ptr).type_id;
        debug_assert!(
            type_id > 0 && type_id <= 255,
            "inc_ref_ptr: invalid type_id {} at ptr {:?} — likely use-after-free",
            type_id,
            ptr
        );
        if ((*header_ptr).flags & HEADER_FLAG_IMMORTAL) != 0 {
            return;
        }
        // Debug: trace bigint refcount increments
        if type_id == TYPE_ID_BIGINT && debug_bigint_rc() {
            let old = (*header_ptr).ref_count.load(AtomicOrdering::Relaxed);
            eprintln!(
                "BIGINT_RC_INC ptr=0x{:x} count={} → {}",
                ptr as usize,
                old,
                old + 1
            );
        }
        if type_id == TYPE_ID_EXCEPTION && trace_exception_rc() {
            let old = (*header_ptr).ref_count.load(AtomicOrdering::Relaxed);
            eprintln!("EXC_RC_INC ptr=0x{:x} {}→{}", ptr as usize, old, old + 1);
        }
        let new_count = (*header_ptr)
            .ref_count
            .fetch_add(1, AtomicOrdering::Relaxed)
            + 1;
        if debug_rc_object() {
            let header = &*header_ptr;
            if header.type_id == TYPE_ID_OBJECT
                && (header.flags & HEADER_FLAG_SKIP_CLASS_DECREF) != 0
            {
                eprintln!("molt rc inc ptr=0x{:x} count={}", ptr as usize, new_count);
            }
        }
        if debug_file_rc() {
            let header = &*header_ptr;
            if header.type_id == TYPE_ID_FILE_HANDLE {
                eprintln!(
                    "molt file rc inc ptr=0x{:x} count={}",
                    ptr as usize, new_count
                );
            }
        }
    }
}

/// Batched increment: add `count` to the refcount in a single atomic
/// operation instead of `count` separate fetch_add(1) calls.
///
/// # Safety
/// Dereferences raw pointer to increment ref count.
pub(crate) unsafe fn inc_ref_n_ptr(_py: &PyToken<'_>, ptr: *mut u8, count: u32) {
    unsafe {
        crate::gil_assert();
        if ptr.is_null() || count == 0 {
            return;
        }
        let header_ptr = ptr.sub(std::mem::size_of::<MoltHeader>()) as *mut MoltHeader;
        if ((*header_ptr).flags & HEADER_FLAG_IMMORTAL) != 0 {
            return;
        }
        let new_count = (*header_ptr)
            .ref_count
            .fetch_add(count, AtomicOrdering::Relaxed)
            + count;
        if debug_rc_object() {
            let header = &*header_ptr;
            if header.type_id == TYPE_ID_OBJECT
                && (header.flags & HEADER_FLAG_SKIP_CLASS_DECREF) != 0
            {
                eprintln!(
                    "molt rc inc_n ptr=0x{:x} count={} by={}",
                    ptr as usize, new_count, count
                );
            }
        }
    }
}

/// Run the object's `__del__` finalizer INSIDE an already-open revival window.
///
/// CONTRACT: the caller (`dec_ref_ptr`) has already opened the revival window —
/// the object is live at rc≥1 across this whole call — and owns the single
/// closing `dec_ref` + resurrection check that follows. This function therefore
/// does NOT touch the refcount itself; it only runs `__del__` (under the
/// CPython-faithful exception save/clear/restore + synthetic-handler-frame
/// dance) and sets `HEADER_FLAG_FINALIZER_RAN` so the finalizer is run at most
/// once per object lifetime. Objects with no finalizer (or whose finalizer
/// already ran) return immediately without side effects; the caller's window
/// still covers the subsequent `weakref_clear_for_ptr`, so a weakref callback
/// can resurrect through the SAME window even for a `__del__`-free object.
unsafe fn run_object_del_in_revival_window(py: &PyToken<'_>, ptr: *mut u8) {
    let header_ptr = unsafe { header_from_obj_ptr(ptr) };
    if unsafe { (*header_ptr).type_id } != TYPE_ID_OBJECT {
        return;
    }
    if (unsafe { (*header_ptr).flags } & HEADER_FLAG_INSTANCE_HAS_FINALIZER) == 0 {
        return;
    }
    if (unsafe { (*header_ptr).flags } & HEADER_FLAG_FINALIZER_RAN) != 0 {
        return;
    }
    let cold_idx = unsafe { (*header_ptr).cold_idx };
    // Strip the shared bit: stack-allocated and shared-cold-header
    // instances point to the per-class cold header which carries
    // class_bits in its `state` field — we want that value, not 0.
    let class_state = get_cold_header(cold_idx_real(cold_idx))
        .map(|cold| cold.state)
        .unwrap_or(0);
    let class_bits = unsafe { object_class_bits_from_state(class_state) };
    if class_bits == 0 || obj_from_bits(class_bits).is_none() {
        return;
    }
    if let Some(class_ptr) = obj_from_bits(class_bits).as_ptr() {
        let class_name = unsafe {
            crate::string_obj_to_owned(obj_from_bits(layout::class_name_bits(class_ptr)))
        };
        if class_bits == crate::builtin_classes(py).traceback
            || class_name.as_deref() == Some("traceback")
            || class_bits == crate::builtin_classes(py).frame
            || class_name.as_deref() == Some("frame")
        {
            return;
        }
    }
    let Some(del_name_bits) = crate::attr_name_bits_from_bytes(py, b"__del__") else {
        return;
    };
    let raw_del_bits = obj_from_bits(class_bits)
        .as_ptr()
        .and_then(|class_ptr| unsafe {
            class_lookup_raw_mro_dict_attr(py, class_ptr, del_name_bits)
        });
    dec_ref_bits(py, del_name_bits);
    let Some(raw_del_bits) = raw_del_bits else {
        return;
    };
    unsafe {
        (*header_ptr).flags |= HEADER_FLAG_FINALIZER_RAN;
    }
    // CPython `PyObject_CallFinalizer` runs the finalizer with a CLEAN exception
    // state: `_PyErr_GetRaisedException` FETCHES (saves AND clears) any in-flight
    // exception before `tp_finalize`, then `_PyErr_SetRaisedException` restores it
    // afterward. The fetch-and-clear is load-bearing: when an exception is
    // unwinding the frame (a frame-local whose last reference dies on a `raise`
    // with no local handler), CPython still runs `__del__` during that unwind — it
    // does NOT skip the finalizer because an exception is pending. molt previously
    // SAVED the surrounding exception (below) but never cleared it, so the
    // `!exception_pending` gate guarding the `__del__` call — whose real purpose is
    // to detect a binding-time raise from `descriptor_bind` — wrongly suppressed
    // `__del__` on EVERY exception-unwind path (`resurrect_during_exception_unwind`:
    // the finalizer-aware DecRef is correctly placed and runs, but `__del__` never
    // fired → a dropped finalizer == leak, `box_len 0` vs CPython `1`). Mirror
    // CPython exactly: capture the in-flight exception, keep it alive across the
    // clear, then CLEAR so the binding + call run clean. It is restored after the
    // finalizer (the unraisable-write + restore block below).
    let prior_exc_bits = crate::builtins::exceptions::exception_last_bits_noinc(py)
        .filter(|bits| !obj_from_bits(*bits).is_none());
    if let Some(bits) = prior_exc_bits {
        inc_ref_bits(py, bits);
        crate::clear_exception(py);
        crate::builtins::exceptions::clear_exception_state(py);
    }
    // Run `__del__` lookup/binding/call under a SYNTHETIC exception-handler frame
    // so an uncaught raise inside it is recorded VALUE-BASED and swallowed below,
    // instead of killing the process. ROOT CAUSE of #65 (definitively measured):
    // when a raise reaches `molt_raise` with `exception_handler_active()` false
    // (the `EXCEPTION_STACK` empty), molt's uncaught-exception terminator runs
    // `std::process::exit(1)` (exceptions.rs). It is NOT a "native unwind" (that
    // misdiagnosis drove the now-reverted deferral apparatus) — it is a hard
    // process exit, which is why `catch_unwind` caught nothing and a baseline
    // change did nothing (the baseline does not gate the terminator; an empty
    // handler stack does). A finalizer runs at an empty handler stack unless a
    // surrounding `try:` happens to leave a frame on it — that is the observed
    // composition dependence. Pushing exactly one handler frame here makes
    // `molt_raise` take the value-based path, `call_callable0` return, and the
    // swallow run in EVERY context — CPython's implicit "ignore exceptions during
    // finalization" boundary, in runtime form. This mirrors the compiled
    // try-frame (`molt_exception_push`/`molt_exception_pop`); no `catch_unwind`, no
    // backend landing pad, no deferral, and `__del__` still runs INLINE at the
    // rc→0 point so finalization stays CPython-prompt.
    crate::builtins::exceptions::exception_stack_push();
    let del_bits = obj_from_bits(class_bits)
        .as_ptr()
        .and_then(|class_ptr| unsafe {
            crate::builtins::attr::descriptor_bind(py, raw_del_bits, class_ptr, Some(ptr))
        })
        .unwrap_or(0);
    if del_bits != 0 && !crate::exception_pending(py) {
        let result_bits = unsafe { crate::call_callable0(py, del_bits) };
        if !obj_from_bits(result_bits).is_none() {
            dec_ref_bits(py, result_bits);
        }
    }
    crate::builtins::exceptions::exception_stack_pop(py);
    if !obj_from_bits(del_bits).is_none() {
        dec_ref_bits(py, del_bits);
    }
    // CPython `PyObject_CallFinalizer` tail: an exception raised DURING the
    // finalizer (`__del__` itself, or the `descriptor_bind` above) is ignored —
    // `PyErr_WriteUnraisable` writes it to stderr and clears it — and only THEN is
    // any saved surrounding exception restored (`_PyErr_SetRaisedException`). The
    // prior exception was fetched-and-cleared before the finalizer ran, so any
    // pending exception here is unambiguously the finalizer's own raise: write it
    // unraisable and clear, regardless of whether a surrounding exception existed.
    // `__del__` ran under the synthetic handler frame above, so `molt_raise`
    // recorded the raise value-based rather than running the uncaught-exception
    // process-exit terminator (#65) — this branch is reachable.
    if crate::exception_pending(py) {
        if let Some(exc_bits) = crate::builtins::exceptions::exception_last_bits_noinc(py)
            && let Some(exc_ptr) = obj_from_bits(exc_bits).as_ptr()
        {
            let formatted =
                crate::builtins::exceptions::format_exception_with_traceback(py, exc_ptr);
            eprintln!("Exception ignored while calling deallocator:");
            if !formatted.is_empty() {
                eprint!("{formatted}");
            }
        }
        crate::clear_exception(py);
        crate::builtins::exceptions::clear_exception_state(py);
    }
    // Restore the surrounding exception saved before the finalizer ran, so it
    // continues to unwind the frame exactly as in CPython.
    if let Some(bits) = prior_exc_bits {
        crate::builtins::exceptions::exception_set_last_bits_raw(py, bits);
        dec_ref_bits(py, bits);
    }
    // The revival ref opened by the caller stays held: `dec_ref_ptr` performs the
    // single closing `dec_ref` + resurrection check AFTER `weakref_clear_for_ptr`
    // runs, so a `__del__`-resurrect and a weakref-callback-resurrect collapse to
    // the SAME post-window check (CPython's finalize+ClearWeakRefs window).
}

unsafe fn release_dealloc_tracked_bits_vec(
    py: &PyToken<'_>,
    vec_ptr: *mut Vec<u64>,
    header_flags: u32,
) {
    unsafe {
        if vec_ptr.is_null() {
            return;
        }
        let mut vec = backing::tracked_vec_box_from_raw(vec_ptr);
        if (header_flags & HEADER_FLAG_CONTAINS_REFS) == 0 {
            return;
        }
        let detached = std::mem::take(&mut *vec);
        for bits in detached {
            dec_ref_bits(py, bits);
        }
    }
}

/// # Safety
/// Dereferences raw pointer to decrement ref count. Frees memory if count reaches 0.
#[inline(always)]
pub(crate) unsafe fn dec_ref_ptr(py: &PyToken<'_>, ptr: *mut u8) {
    unsafe {
        crate::gil_assert();
        if ptr.is_null() {
            return;
        }
        let header_ptr = ptr.sub(std::mem::size_of::<MoltHeader>()) as *mut MoltHeader;
        let type_id = (*header_ptr).type_id;
        let header_flags = (*header_ptr).flags;
        let header_size_class = (*header_ptr).size_class;
        let header_cold_idx = (*header_ptr).cold_idx;
        if (header_flags & HEADER_FLAG_IMMORTAL) != 0 {
            return;
        }
        // A zero refcount at dec_ref entry is an ownership invariant violation.
        // Do not make dec_ref idempotent: a stale post-free pointer may already
        // alias allocator metadata or a different object, so continuing would
        // corrupt unrelated runtime state.
        let current = (*header_ptr).ref_count.load(AtomicOrdering::Acquire);
        if current == 0 {
            eprintln!(
                "molt fatal: refcount underflow before dec_ref ptr=0x{:x} type_id={}",
                ptr as usize, type_id
            );
            std::process::abort();
        }
        let prev = (*header_ptr).ref_count.fetch_sub(1, AtomicOrdering::AcqRel);
        if type_id == TYPE_ID_EXCEPTION && trace_exception_rc() {
            eprintln!("EXC_RC_DEC ptr=0x{:x} {}→{}", ptr as usize, prev, prev - 1);
        }
        if type_id == TYPE_ID_OBJECT && debug_object_rc() {
            if prev == 1 {
                eprintln!("[OBJECT DEC→0 FREE] ptr=0x{:x}", ptr as usize);
            } else {
                eprintln!(
                    "[OBJECT DEC {}→{}] ptr=0x{:x}",
                    prev,
                    prev.saturating_sub(1),
                    ptr as usize
                );
            }
        }
        // Debug: trace bigint refcount decrements
        if type_id == TYPE_ID_BIGINT && debug_bigint_rc() {
            eprintln!(
                "BIGINT_RC_DEC ptr=0x{:x} count={} → {}",
                ptr as usize,
                prev,
                prev.saturating_sub(1)
            );
            if prev == 1 {
                eprintln!("  BIGINT FREED at ptr=0x{:x}", ptr as usize);
            }
        }
        if debug_file_rc() && type_id == TYPE_ID_FILE_HANDLE {
            eprintln!(
                "molt file rc dec ptr=0x{:x} count={}",
                ptr as usize,
                prev.saturating_sub(1)
            );
        }
        if debug_rc_object()
            && type_id == TYPE_ID_OBJECT
            && (header_flags & HEADER_FLAG_SKIP_CLASS_DECREF) != 0
        {
            eprintln!(
                "molt rc dec ptr=0x{:x} count={}",
                ptr as usize,
                prev.saturating_sub(1)
            );
        }
        if prev == 1 {
            if type_id == TYPE_ID_EXCEPTION
                && crate::builtins::exceptions::exception_is_rooted(py, ptr)
            {
                // Pending exception roots (last-exception slots / active exception stacks)
                // must keep the object alive even if transient lowering bugs over-decref.
                if trace_exception_rc() {
                    eprintln!("EXC_RC_RESURRECT ptr=0x{:x} (rooted, rc 0→1)", ptr as usize);
                }
                (*header_ptr).ref_count.store(1, AtomicOrdering::Release);
                return;
            }
            if type_id == TYPE_ID_EXCEPTION && trace_exception_rc() {
                eprintln!("EXC_RC_FREE ptr=0x{:x} (rc hit 0, freeing)", ptr as usize);
            }
            MoltRefCount::acquire_fence();
            // RC drop-insertion substrate (design 20): the rc=1→0 transition,
            // past the immortal/rooted early-returns above. This is NOT yet a
            // confirmed deallocation: the finalize + weakref-clear revival window
            // below may run a `__del__` OR a weakref callback that RESURRECTS the
            // object (re-incrementing its refcount), in which case `dec_ref_ptr`
            // returns WITHOUT freeing. Counting the dealloc here would over-count
            // destructions and make `live = alloc - dealloc` UNDER-count live
            // objects — an unsound leak gauge under resurrection (phantom "no
            // leak"). So the dealloc counters are bumped only AFTER the revival
            // window's single resurrection check passes (see below); the byte
            // total is the one value that must be read from the header BEFORE the
            // window runs (a `__del__` can mutate/realloc the object, and for
            // oversized objects `total_size_from_header_fields` reads the cold
            // header's `extended_size`), so snapshot it here into a local and
            // commit it after the destruction is confirmed.
            let dealloc_bytes =
                total_size_from_header_fields(header_size_class, header_cold_idx) as u64;
            if debug_dec_ref_zero() {
                eprintln!(
                    "molt dec_ref_zero ptr=0x{:x} type_id={}",
                    ptr as usize, type_id
                );
                if type_id == TYPE_ID_CODE {
                    let filename_bits = code_filename_bits(ptr);
                    let name_bits = code_name_bits(ptr);
                    let varnames_bits = code_varnames_bits(ptr);
                    let names_bits = code_names_bits(ptr);
                    let filename = crate::string_obj_to_owned(obj_from_bits(filename_bits))
                        .unwrap_or_else(|| "<non-str>".to_string());
                    let name = crate::string_obj_to_owned(obj_from_bits(name_bits))
                        .unwrap_or_else(|| "<non-str>".to_string());
                    let varnames_ptr = obj_from_bits(varnames_bits)
                        .as_ptr()
                        .map(|p| p as usize)
                        .unwrap_or(0);
                    let names_ptr = obj_from_bits(names_bits)
                        .as_ptr()
                        .map(|p| p as usize)
                        .unwrap_or(0);
                    eprintln!(
                        "molt dec_ref_zero code name={} file={} varnames=0x{:x} names=0x{:x}",
                        name, filename, varnames_ptr, names_ptr
                    );
                } else if type_id == TYPE_ID_TUPLE {
                    let vec_ptr = seq_vec_ptr(ptr) as usize;
                    eprintln!(
                        "molt dec_ref_zero tuple ptr=0x{:x} vec=0x{:x}",
                        ptr as usize, vec_ptr
                    );
                }
            }
            if type_id == TYPE_ID_FUNCTION && {
                static TRACE: OnceLock<bool> = OnceLock::new();
                *TRACE.get_or_init(|| {
                    std::env::var("MOLT_TRACE_DECREF_ZERO_FUNCTION").as_deref() == Ok("1")
                })
            } {
                // Debug-only: cached builtin function objects must not be freed while still cached.
                // When they do hit zero, capture a backtrace to identify the incorrect owner.
                let freed_fn_ptr = crate::function_fn_ptr(ptr);
                let obj_init_subclass_ptr =
                    crate::molt_object_init_subclass as *const () as usize as u64;
                let type_init_ptr = crate::molt_type_init as *const () as usize as u64;
                if freed_fn_ptr == obj_init_subclass_ptr || freed_fn_ptr == type_init_ptr {
                    let bt = std::backtrace::Backtrace::force_capture();
                    eprintln!(
                        "molt dec_ref_zero function ptr=0x{:x} obj_init_subclass=0x{:x} type_init=0x{:x}\n{bt}",
                        freed_fn_ptr, obj_init_subclass_ptr, type_init_ptr,
                    );
                }
            }
            if type_id == TYPE_ID_FUNCTION && trace_decref_zero_function_all() {
                // Debug-only: when chasing refcount bugs, print which function is being freed.
                let freed_fn_ptr = crate::function_fn_ptr(ptr);
                let name_bits = crate::function_name_bits(py, ptr);
                let name = crate::string_obj_to_owned(crate::obj_from_bits(name_bits))
                    .unwrap_or_else(|| "<function>".to_string());
                let bt = std::backtrace::Backtrace::force_capture();
                eprintln!(
                    "molt dec_ref_zero function name={} fn_ptr=0x{:x} obj_ptr=0x{:x}\n{bt}",
                    name, freed_fn_ptr, ptr as usize,
                );
            }
            // FINALIZE + WEAKREF-CLEAR REVIVAL WINDOW (council #1 P0 fix).
            //
            // CPython's `_Py_Dealloc` runs `tp_finalize` (`__del__`) FIRST and,
            // only if the object was NOT resurrected by it, then runs
            // `PyObject_ClearWeakRefs` (the weakref callbacks) and `tp_dealloc`.
            // Crucially, BOTH the finalizer and the weakref callbacks execute with
            // the object's storage LIVE: CPython resurrects the object across each
            // Python-visible step. molt previously dropped the finalizer's
            // temporary revival ref BEFORE clearing weakrefs, so the callbacks ran
            // at rc=0 — a callback that re-touched the dying object's storage was a
            // use-after-free. The fix makes the revival window a first-class step
            // here in `dec_ref_ptr` (the Python lifetime boundary): ONE revival
            // ref is held across `__del__` AND, separately, across the weakref
            // clear, with a resurrection check after EACH Python-visible step. No
            // Python code ever runs while the object is at rc=0.
            //
            // The window is opened ONLY when the object actually participates — it
            // has a `__del__` finalizer (`INSTANCE_HAS_FINALIZER`) or has ever
            // exposed a weakref (`HAS_WEAKREF`). Objects with neither (the hot
            // path: ints, strings, tuples, plain instances) skip the revival
            // inc/dec AND the global weakref lock entirely and fall straight
            // through to the free tail with zero added cost.
            let window_flags =
                header_flags & (HEADER_FLAG_INSTANCE_HAS_FINALIZER | HEADER_FLAG_HAS_WEAKREF);
            if window_flags != 0 {
                // Open the revival window: the object is now live at rc≥1 so no
                // Python code below can observe (or free) it at rc=0. Use the raw
                // header increment (not `inc_ref_ptr`, which short-circuits on
                // IMMORTAL — already excluded above — and carries debug tracing);
                // the matching closes below are the authoritative resurrection
                // checks.
                (*header_ptr).ref_count.fetch_add(1, AtomicOrdering::Relaxed);
                // `__del__` runs INLINE at this rc→0 point, exactly as CPython
                // finalizes at Py_DECREF→0 (prompt timing: `del x; print()` runs
                // `__del__` before `print`), under a synthetic exception-handler
                // frame so an uncaught raise inside it is swallowed (written
                // unraisable) rather than killing the process — see
                // `run_object_del_in_revival_window` for the #65 root cause and the
                // run-once (`FINALIZER_RAN`) semantics. Non-finalizer objects that
                // only reach here for the weakref clear return immediately from it.
                run_object_del_in_revival_window(py, ptr);
                // After `__del__`, the only live reference should be this window's.
                // If `__del__` RESURRECTED the object (stashed `self`), the count
                // is now > 1: CPython aborts dealloc here WITHOUT clearing weakrefs
                // (a resurrected object keeps its weakrefs, and their callbacks do
                // NOT fire — `resurrect_with_weakref`/`resurrect_then_final_drop`).
                // Drop the window ref and return; the object stays alive at rc≥1
                // and a later final drop re-enters (FINALIZER_RAN already set, so
                // `__del__` never re-runs; the weakrefs are cleared on that real
                // death). The mid-window dec/check runs NO Python code, so the
                // object is never observable at rc=0.
                if (*header_ptr).ref_count.load(AtomicOrdering::Acquire) > 1 {
                    (*header_ptr).ref_count.fetch_sub(1, AtomicOrdering::AcqRel);
                    return;
                }
                // `__del__` did not resurrect. Clear weakrefs and run their
                // callbacks WHILE the object is still live (rc==1) in the same
                // window — never at rc=0. A callback that re-strengthens the
                // referent (or otherwise re-increments its refcount) makes the
                // close below see rc>1 and abort the free, exactly as for a
                // `__del__` resurrection. `weakref_clear_for_ptr` is a no-op (early
                // return after the global-lock probe) when no weakref is currently
                // registered, so a `__del__`-only object pays just that probe.
                weakref_clear_for_ptr(py, ptr);
                // Close the window: drop the revival ref. If the weakref callbacks
                // resurrected the object (rc still > 1 after this sub), abort the
                // free — the object is alive again and a later final drop re-enters
                // (the weakrefs were already cleared exactly once).
                let prev_window = (*header_ptr).ref_count.fetch_sub(1, AtomicOrdering::AcqRel);
                if prev_window > 1 {
                    return;
                }
            }
            // Past the resurrection check: the object is now actually being
            // destroyed. Commit the leak-gauge counters so DEALLOC_COUNT means
            // "objects truly freed", keeping `live = alloc - dealloc` exact
            // (resurrected objects are correctly NOT counted as dealloc'd until
            // their real final drop). `type_id` is the cached entry value; the
            // byte total was snapshotted before the window ran.
            profile_hit(py, &DEALLOC_COUNT);
            profile_hit_bytes(py, &DEALLOC_BYTES_TOTAL, dealloc_bytes);
            profile_dealloc_type(py, type_id);
            match type_id {
                // Hot path: most-frequently-freed types first
                TYPE_ID_STRING => {
                    utf8_cache_remove(py, ptr as usize);
                }
                // Heap-allocated NaN float: no inner refs to dec-ref.
                TYPE_ID_FLOAT => {}
                // Class objects: dec-ref all ref-counted fields and bump the
                // global type version so all inline caches that held a pointer
                // to this class are invalidated before the memory is reused.
                //
                // ORDERING IS CRITICAL: `molt_class_set_base` stores both the
                // class payload slots (bases, mro) AND the class dict entries
                // `__bases__`/`__mro__` as counted references.  We must
                // dec-ref the payload slots BEFORE dec-refing the dict, so
                // that when the dict cascade runs it sees refcount==1 (not 0)
                // and correctly frees those objects without a double-free.
                TYPE_ID_TYPE => {
                    let name_bits = layout::class_name_bits(ptr);
                    let dict_bits = layout::class_dict_bits(ptr);
                    let bases_bits = layout::class_bases_bits(ptr);
                    let mro_bits = layout::class_mro_bits(ptr);
                    let annotations_bits = layout::class_annotations_bits(ptr);
                    let annotate_bits = layout::class_annotate_bits(ptr);
                    let qualname_bits = layout::class_qualname_bits(ptr);
                    // Metaclass reference stored in the MoltHeader `state` slot
                    // by `molt_type_new` / `object_set_class_bits`.
                    let metaclass_bits = object_class_bits(ptr);

                    // Dec-ref non-dict slots first so the dict cascade doesn't
                    // see a refcount of zero for objects it also references.
                    // `dec_ref_bits` is a no-op for primitives (None, int, etc.)
                    // so we don't need to guard against None/zero explicitly —
                    // but we do guard against the zero-bits sentinel (bits==0
                    // is not a valid NaN-boxed heap pointer, and as_ptr() on it
                    // returns None, making dec_ref_bits a no-op anyway; the
                    // explicit guard is for clarity and avoids the function call).
                    dec_ref_bits(py, name_bits);
                    dec_ref_bits(py, bases_bits);
                    dec_ref_bits(py, mro_bits);
                    dec_ref_bits(py, annotations_bits);
                    dec_ref_bits(py, annotate_bits);
                    dec_ref_bits(py, qualname_bits);
                    dec_ref_bits(py, metaclass_bits);
                    // Dict last: its cascade will free __bases__ and __mro__
                    // after the slot refs above have been released.
                    dec_ref_bits(py, dict_bits);
                    free_shared_cold_idx_for_class(ptr);
                    // Invalidate all result-level inline caches that may hold a
                    // stale pointer to this now-freed class object.  Without
                    // this bump, caches that were written when type_version==N
                    // would still pass the version check after the class is freed
                    // and its memory reused, causing use-after-free in
                    // inc_ref_bits on the cached result.
                    bump_type_version();
                }
                TYPE_ID_LIST_INT => {
                    // list_int stores a *mut ListIntStorage (#[repr(C)]).
                    // Reconstruct the Vec to free the backing buffer.
                    let storage_ptr = layout::list_int_storage_ptr(ptr);
                    if !storage_ptr.is_null() {
                        let storage = *Box::from_raw(storage_ptr);
                        // Drop the backing buffer by reconstructing the Vec.
                        // Raw i64 elements have no inner refs to dec-ref.
                        drop(storage.into_vec());
                    }
                }
                TYPE_ID_LIST_BOOL => {
                    // list_bool stores a *mut ListBoolStorage (#[repr(C)]).
                    // Reconstruct the Vec<u8> to free the backing buffer.
                    // No inner refs to dec-ref — bools are inline values.
                    let storage_ptr = layout::list_bool_storage_ptr(ptr);
                    if !storage_ptr.is_null() {
                        let storage = *Box::from_raw(storage_ptr);
                        drop(storage.into_vec());
                    }
                }
                TYPE_ID_LIST | TYPE_ID_TUPLE => {
                    release_dealloc_tracked_bits_vec(py, seq_vec_ptr(ptr), header_flags);
                }
                TYPE_ID_DICT => {
                    let order_ptr = dict_order_ptr(ptr);
                    let table_ptr = dict_table_ptr(ptr);
                    let hashes_ptr = dict_hashes_ptr(ptr);
                    release_dealloc_tracked_bits_vec(py, order_ptr, header_flags);
                    if !table_ptr.is_null() {
                        drop(backing::tracked_vec_box_from_raw(table_ptr));
                    }
                    if !hashes_ptr.is_null() {
                        drop(backing::tracked_vec_box_from_raw(hashes_ptr));
                    }
                }
                TYPE_ID_LIST_BUILDER => {
                    let vec_ptr = *(ptr as *mut *mut Vec<u64>);
                    if !vec_ptr.is_null() {
                        drop(backing::tracked_vec_box_from_raw(vec_ptr));
                    }
                }
                TYPE_ID_BYTEARRAY => {
                    let vec_ptr = bytearray_vec_ptr(ptr);
                    if !vec_ptr.is_null() {
                        drop(backing::tracked_vec_box_from_raw(vec_ptr));
                    }
                }
                TYPE_ID_DICT_BUILDER => {
                    let vec_ptr = *(ptr as *mut *mut Vec<u64>);
                    if !vec_ptr.is_null() {
                        drop(backing::tracked_vec_box_from_raw(vec_ptr));
                    }
                }
                TYPE_ID_SET | TYPE_ID_FROZENSET => {
                    let order_ptr = set_order_ptr(ptr);
                    let table_ptr = set_table_ptr(ptr);
                    let hashes_ptr = set_hashes_ptr(ptr);
                    release_dealloc_tracked_bits_vec(py, order_ptr, header_flags);
                    if !table_ptr.is_null() {
                        drop(backing::tracked_vec_box_from_raw(table_ptr));
                    }
                    if !hashes_ptr.is_null() {
                        drop(backing::tracked_vec_box_from_raw(hashes_ptr));
                    }
                }
                TYPE_ID_SET_BUILDER => {
                    let vec_ptr = *(ptr as *mut *mut Vec<u64>);
                    if !vec_ptr.is_null() {
                        drop(backing::tracked_vec_box_from_raw(vec_ptr));
                    }
                }
                TYPE_ID_CALLARGS => {
                    let args_ptr = callargs_ptr(ptr);
                    if !args_ptr.is_null() {
                        crate::call::bind::note_callargs_free(py, ptr, args_ptr);
                        callargs_dec_ref_all(py, args_ptr);
                        drop(Box::from_raw(args_ptr));
                    }
                }
                TYPE_ID_MEMORYVIEW => {
                    let owner_bits = memoryview_owner_bits(ptr);
                    if owner_bits != 0 && !obj_from_bits(owner_bits).is_none() {
                        dec_ref_bits(py, owner_bits);
                    }
                    let format_bits = memoryview_format_bits(ptr);
                    if format_bits != 0 && !obj_from_bits(format_bits).is_none() {
                        dec_ref_bits(py, format_bits);
                    }
                    let shape_ptr = memoryview_shape_ptr(ptr);
                    if !shape_ptr.is_null() {
                        drop(backing::tracked_vec_box_from_raw(shape_ptr));
                    }
                    let strides_ptr = memoryview_strides_ptr(ptr);
                    if !strides_ptr.is_null() {
                        drop(backing::tracked_vec_box_from_raw(strides_ptr));
                    }
                }
                TYPE_ID_RANGE => {
                    let start_bits = range_start_bits(ptr);
                    let stop_bits = range_stop_bits(ptr);
                    let step_bits = range_step_bits(ptr);
                    if start_bits != 0 && !obj_from_bits(start_bits).is_none() {
                        dec_ref_bits(py, start_bits);
                    }
                    if stop_bits != 0 && !obj_from_bits(stop_bits).is_none() {
                        dec_ref_bits(py, stop_bits);
                    }
                    if step_bits != 0 && !obj_from_bits(step_bits).is_none() {
                        dec_ref_bits(py, step_bits);
                    }
                }
                TYPE_ID_SLICE => {
                    let start_bits = slice_start_bits(ptr);
                    let stop_bits = slice_stop_bits(ptr);
                    let step_bits = slice_step_bits(ptr);
                    if start_bits != 0 && !obj_from_bits(start_bits).is_none() {
                        dec_ref_bits(py, start_bits);
                    }
                    if stop_bits != 0 && !obj_from_bits(stop_bits).is_none() {
                        dec_ref_bits(py, stop_bits);
                    }
                    if step_bits != 0 && !obj_from_bits(step_bits).is_none() {
                        dec_ref_bits(py, step_bits);
                    }
                }
                TYPE_ID_DATACLASS => {
                    let desc_ptr = dataclass_desc_ptr(ptr);
                    let fields_ptr = dataclass_fields_ptr(ptr);
                    if !fields_ptr.is_null() {
                        let fields = backing::tracked_vec_box_from_raw(fields_ptr);
                        for &val_bits in fields.iter() {
                            if val_bits != 0 && !obj_from_bits(val_bits).is_none() {
                                dec_ref_bits(py, val_bits);
                            }
                        }
                    }
                    let dict_bits = dataclass_dict_bits(ptr);
                    if dict_bits != 0 && !obj_from_bits(dict_bits).is_none() {
                        dec_ref_bits(py, dict_bits);
                    }
                    if !desc_ptr.is_null() {
                        let class_bits = (*desc_ptr).class_bits;
                        if class_bits != 0 && !obj_from_bits(class_bits).is_none() {
                            dec_ref_bits(py, class_bits);
                        }
                        drop(Box::from_raw(desc_ptr));
                    }
                }
                TYPE_ID_CODE => {
                    let filename_bits = code_filename_bits(ptr);
                    let name_bits = code_name_bits(ptr);
                    let linetable_bits = code_linetable_bits(ptr);
                    let varnames_bits = code_varnames_bits(ptr);
                    let names_bits = code_names_bits(ptr);
                    let arg_names_bits = code_arg_names_bits(ptr);
                    let posonly_bits = code_signature_posonly_bits(ptr);
                    let kwonly_bits = code_kwonly_names_bits(ptr);
                    let vararg_bits = code_vararg_bits(ptr);
                    let varkw_bits = code_varkw_bits(ptr);
                    if filename_bits != 0 && !obj_from_bits(filename_bits).is_none() {
                        dec_ref_bits(py, filename_bits);
                    }
                    if name_bits != 0 && !obj_from_bits(name_bits).is_none() {
                        dec_ref_bits(py, name_bits);
                    }
                    if linetable_bits != 0 && !obj_from_bits(linetable_bits).is_none() {
                        dec_ref_bits(py, linetable_bits);
                    }
                    if varnames_bits != 0 && !obj_from_bits(varnames_bits).is_none() {
                        dec_ref_bits(py, varnames_bits);
                    }
                    if names_bits != 0 && !obj_from_bits(names_bits).is_none() {
                        dec_ref_bits(py, names_bits);
                    }
                    if arg_names_bits != 0 && !obj_from_bits(arg_names_bits).is_none() {
                        dec_ref_bits(py, arg_names_bits);
                    }
                    if posonly_bits != 0 && !obj_from_bits(posonly_bits).is_none() {
                        dec_ref_bits(py, posonly_bits);
                    }
                    if kwonly_bits != 0 && !obj_from_bits(kwonly_bits).is_none() {
                        dec_ref_bits(py, kwonly_bits);
                    }
                    if vararg_bits != 0 && !obj_from_bits(vararg_bits).is_none() {
                        dec_ref_bits(py, vararg_bits);
                    }
                    if varkw_bits != 0 && !obj_from_bits(varkw_bits).is_none() {
                        dec_ref_bits(py, varkw_bits);
                    }
                }
                TYPE_ID_FUNCTION => {
                    let dict_bits = function_dict_bits(ptr);
                    if dict_bits != 0 && !obj_from_bits(dict_bits).is_none() {
                        dec_ref_bits(py, dict_bits);
                    }
                    let annotations_bits = function_annotations_bits(ptr);
                    if annotations_bits != 0 && !obj_from_bits(annotations_bits).is_none() {
                        dec_ref_bits(py, annotations_bits);
                    }
                    let annotate_bits = function_annotate_bits(ptr);
                    if annotate_bits != 0 && !obj_from_bits(annotate_bits).is_none() {
                        dec_ref_bits(py, annotate_bits);
                    }
                    let code_bits = function_code_bits(ptr);
                    if code_bits != 0 && !obj_from_bits(code_bits).is_none() {
                        dec_ref_bits(py, code_bits);
                    }
                    let closure_bits = function_closure_bits(ptr);
                    if closure_bits != 0 && !obj_from_bits(closure_bits).is_none() {
                        dec_ref_bits(py, closure_bits);
                    }
                }
                TYPE_ID_BOUND_METHOD => {
                    let func_bits = bound_method_func_bits(ptr);
                    let self_bits = bound_method_self_bits(ptr);
                    if func_bits != 0 && !obj_from_bits(func_bits).is_none() {
                        dec_ref_bits(py, func_bits);
                    }
                    if self_bits != 0 && !obj_from_bits(self_bits).is_none() {
                        dec_ref_bits(py, self_bits);
                    }
                }
                TYPE_ID_PROPERTY => {
                    let get_bits = property_get_bits(ptr);
                    let set_bits = property_set_bits(ptr);
                    let del_bits = property_del_bits(ptr);
                    if get_bits != 0 && !obj_from_bits(get_bits).is_none() {
                        dec_ref_bits(py, get_bits);
                    }
                    if set_bits != 0 && !obj_from_bits(set_bits).is_none() {
                        dec_ref_bits(py, set_bits);
                    }
                    if del_bits != 0 && !obj_from_bits(del_bits).is_none() {
                        dec_ref_bits(py, del_bits);
                    }
                }
                TYPE_ID_CLASSMETHOD => {
                    let func_bits = classmethod_func_bits(ptr);
                    if func_bits != 0 && !obj_from_bits(func_bits).is_none() {
                        dec_ref_bits(py, func_bits);
                    }
                }
                TYPE_ID_STATICMETHOD => {
                    let func_bits = staticmethod_func_bits(ptr);
                    if func_bits != 0 && !obj_from_bits(func_bits).is_none() {
                        dec_ref_bits(py, func_bits);
                    }
                }
                TYPE_ID_GENERIC_ALIAS => {
                    let origin_bits = generic_alias_origin_bits(ptr);
                    let args_bits = generic_alias_args_bits(ptr);
                    if origin_bits != 0 && !obj_from_bits(origin_bits).is_none() {
                        dec_ref_bits(py, origin_bits);
                    }
                    if args_bits != 0 && !obj_from_bits(args_bits).is_none() {
                        dec_ref_bits(py, args_bits);
                    }
                }
                TYPE_ID_UNION => {
                    let args_bits = union_type_args_bits(ptr);
                    if args_bits != 0 && !obj_from_bits(args_bits).is_none() {
                        dec_ref_bits(py, args_bits);
                    }
                }
                TYPE_ID_DICT_KEYS_VIEW | TYPE_ID_DICT_VALUES_VIEW | TYPE_ID_DICT_ITEMS_VIEW => {
                    let dict_bits = dict_view_dict_bits(ptr);
                    if dict_bits != 0 && !obj_from_bits(dict_bits).is_none() {
                        dec_ref_bits(py, dict_bits);
                    }
                }
                TYPE_ID_TRACEBACK_PAYLOAD => {
                    let code_bits = traceback_payload_code_bits(ptr);
                    if code_bits != 0 && !obj_from_bits(code_bits).is_none() {
                        dec_ref_bits(py, code_bits);
                    }
                    let next_bits = traceback_payload_next_bits(ptr);
                    if next_bits != 0 && !obj_from_bits(next_bits).is_none() {
                        dec_ref_bits(py, next_bits);
                    }
                }
                TYPE_ID_NATIVE_HANDLE => {
                    native_handle::native_handle_drop(ptr);
                }
                TYPE_ID_EXCEPTION => {
                    let exc_kind_bits = exception_kind_bits(ptr);
                    if exc_kind_bits != 0 && !obj_from_bits(exc_kind_bits).is_none() {
                        dec_ref_bits(py, exc_kind_bits);
                    }
                    let exc_msg_bits = exception_msg_bits(ptr);
                    if exc_msg_bits != 0 && !obj_from_bits(exc_msg_bits).is_none() {
                        dec_ref_bits(py, exc_msg_bits);
                    }
                    let exc_type_bits = exception_class_bits(ptr);
                    if exc_type_bits != 0 && !obj_from_bits(exc_type_bits).is_none() {
                        dec_ref_bits(py, exc_type_bits);
                    }
                    let exc_args_bits = exception_args_bits(ptr);
                    if exc_args_bits != 0 && !obj_from_bits(exc_args_bits).is_none() {
                        dec_ref_bits(py, exc_args_bits);
                    }
                    let exc_args_payload_bits = exception_args_payload_bits(ptr);
                    if exc_args_payload_bits != 0 && !obj_from_bits(exc_args_payload_bits).is_none()
                    {
                        dec_ref_bits(py, exc_args_payload_bits);
                    }
                    let exc_cause_bits = exception_cause_bits(ptr);
                    if exc_cause_bits != 0 && !obj_from_bits(exc_cause_bits).is_none() {
                        dec_ref_bits(py, exc_cause_bits);
                    }
                    let exc_ctx_bits = exception_context_bits(ptr);
                    if exc_ctx_bits != 0 && !obj_from_bits(exc_ctx_bits).is_none() {
                        dec_ref_bits(py, exc_ctx_bits);
                    }
                    let exc_trace_bits = exception_trace_bits(ptr);
                    if exc_trace_bits != 0 && !obj_from_bits(exc_trace_bits).is_none() {
                        dec_ref_bits(py, exc_trace_bits);
                    }
                    let exc_suppress_bits = exception_suppress_bits(ptr);
                    if exc_suppress_bits != 0 && !obj_from_bits(exc_suppress_bits).is_none() {
                        dec_ref_bits(py, exc_suppress_bits);
                    }
                    let exc_val_bits = exception_value_bits(ptr);
                    if exc_val_bits != 0 && !obj_from_bits(exc_val_bits).is_none() {
                        dec_ref_bits(py, exc_val_bits);
                    }
                    let exc_dict_bits = exception_dict_bits(ptr);
                    if exc_dict_bits != 0 && !obj_from_bits(exc_dict_bits).is_none() {
                        dec_ref_bits(py, exc_dict_bits);
                    }
                }
                TYPE_ID_CONTEXT_MANAGER => {
                    let payload_bits = context_payload_bits(ptr);
                    if payload_bits != 0 && !obj_from_bits(payload_bits).is_none() {
                        dec_ref_bits(py, payload_bits);
                    }
                }
                TYPE_ID_MODULE => {
                    crate::c_api::c_api_module_on_module_teardown(py, ptr);
                    let dict_bits = module_dict_bits(ptr);
                    if dict_bits != 0 && !obj_from_bits(dict_bits).is_none() {
                        dec_ref_bits(py, dict_bits);
                    }
                    let name_bits = module_name_bits(ptr);
                    if name_bits != 0 && !obj_from_bits(name_bits).is_none() {
                        dec_ref_bits(py, name_bits);
                    }
                }
                TYPE_ID_ENUMERATE => {
                    let target_bits = enumerate_target_bits(ptr);
                    if target_bits != 0 && !obj_from_bits(target_bits).is_none() {
                        dec_ref_bits(py, target_bits);
                    }
                    let idx_bits = enumerate_index_bits(ptr);
                    if idx_bits != 0 && !obj_from_bits(idx_bits).is_none() {
                        dec_ref_bits(py, idx_bits);
                    }
                    // Drop cached (idx, val) inner tuple — held by the
                    // cache slot at refcount=1.
                    let cached_inner = enumerate_cached_inner(ptr);
                    if !cached_inner.is_null() {
                        dec_ref_ptr(py, cached_inner);
                    }
                    // Drop cached (item, done) outer wrapper.
                    let cached_outer = enumerate_cached_outer(ptr);
                    if !cached_outer.is_null() {
                        dec_ref_ptr(py, cached_outer);
                    }
                }
                TYPE_ID_FILTER => {
                    let func_bits = filter_func_bits(ptr);
                    let iter_bits = filter_iter_bits(ptr);
                    if func_bits != 0 && !obj_from_bits(func_bits).is_none() {
                        dec_ref_bits(py, func_bits);
                    }
                    if iter_bits != 0 && !obj_from_bits(iter_bits).is_none() {
                        dec_ref_bits(py, iter_bits);
                    }
                }
                TYPE_ID_MAP => {
                    let func_bits = map_func_bits(ptr);
                    let iters_ptr = map_iters_ptr(ptr);
                    if func_bits != 0 && !obj_from_bits(func_bits).is_none() {
                        dec_ref_bits(py, func_bits);
                    }
                    if !iters_ptr.is_null() {
                        let iters = backing::tracked_vec_box_from_raw(iters_ptr);
                        for bits in iters.iter() {
                            dec_ref_bits(py, *bits);
                        }
                    }
                    // Drop cached (value, done) wrapper tuple.
                    let cached = map_cached_tuple(ptr);
                    if !cached.is_null() {
                        dec_ref_ptr(py, cached);
                    }
                }
                TYPE_ID_ITER => {
                    let target_bits = iter_target_bits(ptr);
                    if target_bits != 0 && !obj_from_bits(target_bits).is_none() {
                        dec_ref_bits(py, target_bits);
                    }
                    let cached = iter_cached_tuple(ptr);
                    if !cached.is_null() {
                        dec_ref_ptr(py, cached);
                    }
                }
                TYPE_ID_REVERSED => {
                    let target_bits = reversed_target_bits(ptr);
                    if target_bits != 0 && !obj_from_bits(target_bits).is_none() {
                        dec_ref_bits(py, target_bits);
                    }
                }
                TYPE_ID_ZIP => {
                    let iters_ptr = zip_iters_ptr(ptr);
                    if !iters_ptr.is_null() {
                        let iters = backing::tracked_vec_box_from_raw(iters_ptr);
                        for bits in iters.iter() {
                            dec_ref_bits(py, *bits);
                        }
                    }
                    let strict_bits = zip_strict_bits(ptr);
                    if strict_bits != 0 && !obj_from_bits(strict_bits).is_none() {
                        dec_ref_bits(py, strict_bits);
                    }
                }
                TYPE_ID_GENERATOR => {
                    let send_bits = *(ptr.add(GEN_SEND_OFFSET) as *const u64);
                    let throw_bits = *(ptr.add(GEN_THROW_OFFSET) as *const u64);
                    let closed_bits = *(ptr.add(GEN_CLOSED_OFFSET) as *const u64);
                    let depth_bits = *(ptr.add(GEN_EXC_DEPTH_OFFSET) as *const u64);
                    dec_ref_bits(py, send_bits);
                    dec_ref_bits(py, throw_bits);
                    dec_ref_bits(py, closed_bits);
                    dec_ref_bits(py, depth_bits);
                    generator_exception_stack_drop(py, ptr);
                    generator_context_stack_drop(py, ptr);
                }
                TYPE_ID_ASYNC_GENERATOR => {
                    let pending_bits = asyncgen_pending_bits(ptr);
                    let running_bits = asyncgen_running_bits(ptr);
                    let gen_bits = asyncgen_gen_bits(ptr);
                    asyncgen_call_finalizer(py, ptr);
                    if pending_bits != 0 && !obj_from_bits(pending_bits).is_none() {
                        dec_ref_bits(py, pending_bits);
                    }
                    if running_bits != 0 && !obj_from_bits(running_bits).is_none() {
                        dec_ref_bits(py, running_bits);
                    }
                    if gen_bits != 0 && !obj_from_bits(gen_bits).is_none() {
                        dec_ref_bits(py, gen_bits);
                    }
                    asyncgen_registry_remove(py, ptr);
                }
                TYPE_ID_BUFFER2D => {
                    let buffer_ptr = buffer2d_ptr(ptr);
                    if !buffer_ptr.is_null() {
                        drop(Box::from_raw(buffer_ptr));
                    }
                }
                TYPE_ID_GLOB_ITER => {
                    // State holds only Rust Strings (no MoltObject refs); a plain
                    // box-drop frees the entire streaming work-stack.
                    let state_ptr = glob_iter_state_ptr(ptr);
                    if !state_ptr.is_null() {
                        drop(Box::from_raw(state_ptr));
                    }
                }
                TYPE_ID_FILE_HANDLE => {
                    let handle_ptr = file_handle_ptr(ptr);
                    if !handle_ptr.is_null() {
                        let handle = &mut *handle_ptr;
                        flush_file_handle_on_drop(py, handle);
                        // Match CPython: file handles close their underlying backend/FD on drop.
                        // This is required for correct semantics in cases like open(0) where the
                        // file descriptor should be closed once the last reference is released.
                        crate::builtins::io::file_handle_close_ptr(ptr);
                        if handle.name_bits != 0 && !obj_from_bits(handle.name_bits).is_none() {
                            dec_ref_bits(py, handle.name_bits);
                        }
                        if handle.buffer_bits != 0 && !obj_from_bits(handle.buffer_bits).is_none() {
                            dec_ref_bits(py, handle.buffer_bits);
                        }
                        if handle.mem_bits != 0 && !obj_from_bits(handle.mem_bits).is_none() {
                            dec_ref_bits(py, handle.mem_bits);
                        }
                        drop(Box::from_raw(handle_ptr));
                    }
                }
                TYPE_ID_CALL_ITER => {
                    let sentinel_bits = call_iter_sentinel_bits(ptr);
                    let callable_bits = call_iter_callable_bits(ptr);
                    if sentinel_bits != 0 && !obj_from_bits(sentinel_bits).is_none() {
                        dec_ref_bits(py, sentinel_bits);
                    }
                    if callable_bits != 0 && !obj_from_bits(callable_bits).is_none() {
                        dec_ref_bits(py, callable_bits);
                    }
                    // Drop cached (value, done) wrapper tuple.
                    let cached = call_iter_cached_tuple(ptr);
                    if !cached.is_null() {
                        dec_ref_ptr(py, cached);
                    }
                }
                TYPE_ID_OBJECT => {
                    let poll_fn = object_poll_fn(ptr);
                    if poll_fn == asyncio_wait_for_poll_fn_addr() {
                        asyncio_wait_for_task_drop(py, ptr);
                    } else if poll_fn == asyncio_wait_poll_fn_addr() {
                        asyncio_wait_task_drop(py, ptr);
                    } else if poll_fn == asyncio_gather_poll_fn_addr() {
                        asyncio_gather_task_drop(py, ptr);
                    } else if poll_fn == asyncio_timer_handle_poll_fn_addr() {
                        asyncio_timer_handle_task_drop(py, ptr);
                    } else if poll_fn == asyncio_fd_watcher_poll_fn_addr() {
                        asyncio_fd_watcher_task_drop(py, ptr);
                    } else if poll_fn == asyncio_server_accept_loop_poll_fn_addr() {
                        asyncio_server_accept_loop_task_drop(py, ptr);
                    } else if poll_fn == asyncio_ready_runner_poll_fn_addr() {
                        asyncio_ready_runner_task_drop(py, ptr);
                    } else if poll_fn == contextlib_asyncgen_enter_poll_fn_addr() {
                        contextlib_asyncgen_enter_task_drop(py, ptr);
                    } else if poll_fn == contextlib_asyncgen_exit_poll_fn_addr() {
                        contextlib_asyncgen_exit_task_drop(py, ptr);
                    } else if poll_fn == contextlib_async_exitstack_exit_poll_fn_addr() {
                        contextlib_async_exitstack_exit_task_drop(py, ptr);
                    } else if poll_fn == contextlib_async_exitstack_enter_context_poll_fn_addr() {
                        contextlib_async_exitstack_enter_context_task_drop(py, ptr);
                    } else if poll_fn == asyncio_socket_reader_read_poll_fn_addr() {
                        asyncio_socket_reader_read_task_drop(py, ptr);
                    } else if poll_fn == asyncio_socket_reader_readline_poll_fn_addr() {
                        asyncio_socket_reader_readline_task_drop(py, ptr);
                    } else if poll_fn == asyncio_stream_reader_read_poll_fn_addr() {
                        asyncio_stream_reader_read_task_drop(py, ptr);
                    } else if poll_fn == asyncio_stream_reader_readline_poll_fn_addr() {
                        asyncio_stream_reader_readline_task_drop(py, ptr);
                    } else if poll_fn == asyncio_stream_send_all_poll_fn_addr() {
                        asyncio_stream_send_all_task_drop(py, ptr);
                    } else if poll_fn == asyncio_sock_recv_poll_fn_addr() {
                        asyncio_sock_recv_task_drop(py, ptr);
                    } else if poll_fn == asyncio_sock_connect_poll_fn_addr() {
                        asyncio_sock_connect_task_drop(py, ptr);
                    } else if poll_fn == asyncio_sock_accept_poll_fn_addr() {
                        asyncio_sock_accept_task_drop(py, ptr);
                    } else if poll_fn == asyncio_sock_recv_into_poll_fn_addr() {
                        asyncio_sock_recv_into_task_drop(py, ptr);
                    } else if poll_fn == asyncio_sock_sendall_poll_fn_addr() {
                        asyncio_sock_sendall_task_drop(py, ptr);
                    } else if poll_fn == asyncio_sock_recvfrom_poll_fn_addr() {
                        asyncio_sock_recvfrom_task_drop(py, ptr);
                    } else if poll_fn == asyncio_sock_recvfrom_into_poll_fn_addr() {
                        asyncio_sock_recvfrom_into_task_drop(py, ptr);
                    } else if poll_fn == asyncio_sock_sendto_poll_fn_addr() {
                        asyncio_sock_sendto_task_drop(py, ptr);
                    } else if poll_fn == thread_poll_fn_addr() {
                        #[cfg(not(target_arch = "wasm32"))]
                        thread_task_drop(py, ptr);
                    } else if poll_fn == process_poll_fn_addr() {
                        #[cfg(not(target_arch = "wasm32"))]
                        process_task_drop(py, ptr);
                    } else if poll_fn == io_wait_poll_fn_addr() {
                        io_wait_release_socket(py, ptr);
                    } else if poll_fn == ws_wait_poll_fn_addr() {
                        ws_wait_release(py, ptr);
                    }
                    if poll_fn != 0 {
                        task_cancel_message_clear(py, ptr);
                    }
                    let class_bits = object_class_bits(ptr);
                    let builtins = builtin_classes_if_initialized(py);
                    if let Some(builtins) = builtins
                        && class_bits != 0
                        && issubclass_bits(class_bits, builtins.dict)
                    {
                        let payload = object_payload_size(ptr);
                        let slot = PtrSlot(ptr);
                        let mut storage = runtime_state(py).dict_subclass_storage.lock().unwrap();
                        if let Some(bits) = storage.remove(&slot)
                            && bits != 0
                            && !obj_from_bits(bits).is_none()
                        {
                            dec_ref_bits(py, bits);
                        }
                        drop(storage);
                        if payload >= 2 * std::mem::size_of::<u64>() {
                            let storage_ptr =
                                ptr.add(payload - 2 * std::mem::size_of::<u64>()) as *mut u64;
                            let storage_bits = *storage_ptr;
                            if storage_bits != 0 && !obj_from_bits(storage_bits).is_none() {
                                dec_ref_bits(py, storage_bits);
                            }
                        }
                    }
                    let _ = operator_drop_instance(py, ptr)
                        || itertools_drop_instance(py, ptr)
                        || functools_drop_instance(py, ptr)
                        || types_drop_instance(py, ptr);
                    // Design A (#86 — single field-ownership authority): release the
                    // instance's inline typed attribute fields. The inline slots are
                    // the SOLE owner of their pointer refs (`object_field_set_ptr_raw`
                    // inc_refs on store); the runtime free is the one authority that
                    // releases them. Without this, every NON-folded object (a class
                    // with `__del__`, a @dataclass, dynamic/metaclass/decorated
                    // classes — all of which decline the constructor fold) leaks its
                    // object-valued attributes and skips their `__del__`. Gated on
                    // `HAS_PTRS` so primitive-only objects pay nothing. Folded objects
                    // release their fields via the compiler drop pass and are
                    // stack-promoted/immortal (they never reach this runtime free
                    // path), so there is no double-free.
                    if (header_flags & HEADER_FLAG_HAS_PTRS) != 0 {
                        let class_bits = object_class_bits(ptr);
                        if let Some(class_ptr) = obj_from_bits(class_bits).as_ptr() {
                            crate::builtins::attr::dec_ref_object_inline_fields(py, ptr, class_ptr);
                        }
                    }
                    let dict_bits = instance_dict_bits(ptr);
                    if dict_bits != 0 && !obj_from_bits(dict_bits).is_none() {
                        dec_ref_bits(py, dict_bits);
                    }
                    if (header_flags & HEADER_FLAG_SKIP_CLASS_DECREF) == 0 {
                        let class_bits = object_class_bits(ptr);
                        if class_bits != 0 && !obj_from_bits(class_bits).is_none() {
                            dec_ref_bits(py, class_bits);
                        }
                    }
                }
                TYPE_ID_BIGINT => {
                    std::ptr::drop_in_place(ptr as *mut BigInt);
                }
                _ => {}
            }
            release_ptr(ptr);
            let total_size = total_size_from_header_fields(header_size_class, header_cold_idx);
            // Notify the resource tracker that this object's memory is freed.
            let _ = crate::resource::try_with_tracker(|t| t.on_free(total_size));
            // Shared cold headers (per-class, see SHARED_COLD_IDX_BIT)
            // outlive any individual instance — only the class's own
            // dealloc path frees them.  Skip free for shared idx; the
            // real-bit case is handled normally (free_cold_header is
            // already a no-op on idx == 0).
            if !cold_idx_is_shared(header_cold_idx) {
                free_cold_header(header_cold_idx);
            }
            if total_size == 0 {
                return;
            }
            // Arena-allocated objects live inside a bump region and must NOT
            // be passed to the global allocator. The scope arena reclaims via
            // `molt_arena_free` at scope exit.
            if (header_flags & HEADER_FLAG_ARENA) != 0 {
                return;
            }
            let layout = std::alloc::Layout::from_size_align(total_size, 8).unwrap();
            std::alloc::dealloc(header_ptr as *mut u8, layout);
        }
    }
}

#[cfg(test)]
mod tests {
    use crate::object::{TYPE_ID_OBJECT, alloc_object, dec_ref_bits};
    use crate::resource::{LimitedTracker, ResourceLimits, UnlimitedTracker, set_tracker};

    #[test]
    fn object_allocator_rejects_impossible_layout_without_panicking() {
        crate::with_gil_entry_nopanic!(_py, {
            let ptr = alloc_object(_py, usize::MAX, TYPE_ID_OBJECT);
            assert!(
                ptr.is_null(),
                "impossible object layout must fail closed instead of panicking"
            );
        });
    }

    #[test]
    fn denied_object_allocation_does_not_poison_tracker_state() {
        crate::with_gil_entry_nopanic!(_py, {
            let small_total = std::mem::size_of::<super::MoltHeader>();
            let small_plan =
                super::object_allocation_plan(small_total).expect("valid header-sized object");
            let large_total = small_plan.alloc_size + 1;
            let large_plan =
                super::object_allocation_plan(large_total).expect("valid larger object");
            assert!(large_plan.alloc_size > small_plan.alloc_size);

            set_tracker(Box::new(LimitedTracker::new(&ResourceLimits {
                max_memory: Some(small_plan.alloc_size),
                ..Default::default()
            })));
            struct TrackerReset;
            impl Drop for TrackerReset {
                fn drop(&mut self) {
                    set_tracker(Box::new(UnlimitedTracker));
                }
            }
            let _reset = TrackerReset;

            let denied = alloc_object(_py, large_total, TYPE_ID_OBJECT);
            assert!(denied.is_null());

            let allowed = alloc_object(_py, small_total, TYPE_ID_OBJECT);
            assert!(
                !allowed.is_null(),
                "denied allocation must not leave a phantom resource charge"
            );
            dec_ref_bits(_py, crate::MoltObject::from_ptr(allowed).bits());
        });
    }

    #[test]
    fn cold_header_slab_rejects_out_of_bounds_free() {
        use super::{ColdHeaderSlab, MoltColdHeader};

        let mut slab = ColdHeaderSlab::new();
        let idx1 = slab.alloc(MoltColdHeader::default());
        assert!(idx1 >= 1);
        let idx2 = slab.alloc(MoltColdHeader::default());
        assert!(idx2 >= 1);
        let len_before_free = slab.entries.len();
        let free_list_len_before = slab.free_list.len();

        slab.free(24427);

        assert_eq!(slab.free_list.len(), free_list_len_before);
        assert_eq!(slab.entries.len(), len_before_free);
        assert_eq!(slab.live.len(), len_before_free);

        let idx3 = slab.alloc(MoltColdHeader::default());
        assert!(idx3 >= 1);

        slab.free(idx1);
        assert_eq!(slab.free_list.len(), free_list_len_before + 1);
        assert!(slab.get(idx1).is_none());
        let idx4 = slab.alloc(MoltColdHeader::default());
        assert_eq!(idx4, idx1);
        assert!(slab.get(idx4).is_some());
    }

    #[test]
    #[should_panic(expected = "cold header slab double free")]
    fn cold_header_slab_rejects_double_free() {
        use super::{ColdHeaderSlab, MoltColdHeader};

        let mut slab = ColdHeaderSlab::new();
        let idx = slab.alloc(MoltColdHeader::default());

        slab.free(idx);
        slab.free(idx);
    }

    #[test]
    fn cold_header_slab_supports_more_than_65535_live_entries() {
        use super::{ColdHeaderSlab, MoltColdHeader};

        let result = std::panic::catch_unwind(|| {
            let mut slab = ColdHeaderSlab::new();
            for _ in 0..70_000 {
                let _ = slab.alloc(MoltColdHeader::default());
            }
            slab.entries.len()
        });

        match result {
            Ok(len) => assert!(
                len > 65_536,
                "expected slab to hold more than 65,536 entries, got {len}"
            ),
            Err(_) => panic!("cold header slab should scale beyond 65,535 live entries"),
        }
    }
}
