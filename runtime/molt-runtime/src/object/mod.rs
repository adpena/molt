use std::alloc::Layout;
#[cfg(target_arch = "wasm32")]
use std::cell::Cell;
use std::collections::HashMap;
use std::io::Write;
use std::sync::atomic::{AtomicU64, Ordering as AtomicOrdering};
use std::sync::{Arc, Mutex, OnceLock};

use molt_codegen_abi::{
    MoltFlags, MoltRefCount, RefCountRelease, RefCountRevivalWindow, RetainError,
};
use molt_obj_model::{MoltObject, release_ptr, resolve_opaque_ptr, resolve_ptr};
use num_bigint::BigInt;

/// Global type version counter. Incremented whenever ANY class is modified
/// (attribute set/deleted, base class changed, __dict__ mutated).
/// Inline caches compare against this to detect staleness.
///
/// Release/acquire ordering makes the epoch a publication boundary for class
/// dictionary/MRO mutation. The default GIL path does not need the fence, but
/// free-threaded readers must never observe a new epoch with stale type state.
static GLOBAL_TYPE_VERSION: AtomicU64 = AtomicU64::new(1);

#[inline(always)]
pub fn global_type_version() -> u64 {
    GLOBAL_TYPE_VERSION.load(AtomicOrdering::Acquire)
}

#[inline(always)]
pub fn bump_type_version() -> u64 {
    GLOBAL_TYPE_VERSION.fetch_add(1, AtomicOrdering::AcqRel) + 1
}

pub(crate) mod accessors;
pub(crate) mod aux_header;
pub(crate) mod backing;
pub(crate) mod buffer2d;
pub(crate) mod builders;
pub mod float_repr;
pub(crate) mod foreign;
pub(crate) mod gc;
#[allow(dead_code)]
pub(crate) mod heap_kinds_generated;
pub(crate) mod heap_lifecycle;
#[allow(dead_code)]
pub mod inline_cache;
pub(crate) mod layout;
pub(crate) mod list_mutation;
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
pub(crate) mod refcount_opt;
pub(crate) mod seq_access;
#[allow(dead_code)]
pub mod string_intern;
#[allow(dead_code)]
pub(crate) mod type_ids;
pub(crate) mod utf8_cache;
pub(crate) mod weak_container;
pub(crate) mod weakref;

#[allow(unused_imports)]
pub(crate) use type_ids::*;

use aux_header::{
    MoltAuxSidecar, alloc_aux_sidecar, aux_sidecar_from_word, aux_sidecar_size, free_aux_sidecar,
};

use crate::async_rt::poll::ws_wait_poll_fn_addr;
use crate::{
    ALLOC_BYTES_DICT, ALLOC_BYTES_EXCEPTION, ALLOC_BYTES_LIST, ALLOC_BYTES_STRING,
    ALLOC_BYTES_TOTAL, ALLOC_BYTES_TUPLE, ALLOC_CALLARGS_COUNT, ALLOC_COUNT, ALLOC_DICT_COUNT,
    ALLOC_EXCEPTION_COUNT, ALLOC_OBJECT_COUNT, ALLOC_STRING_COUNT, ALLOC_TUPLE_COUNT,
    AUX_CLASS_INLINE_COUNT, AUX_STATE_INLINE_COUNT, DEALLOC_BIGINT_COUNT, DEALLOC_BYTES_EXCEPTION,
    DEALLOC_BYTES_TOTAL, DEALLOC_COUNT, DEALLOC_DICT_COUNT, DEALLOC_EXCEPTION_COUNT,
    DEALLOC_OBJECT_COUNT, DEALLOC_STRING_COUNT, DEALLOC_TUPLE_COUNT, PyToken,
    TYPE_ID_ASYNC_GENERATOR, TYPE_ID_BIGINT, TYPE_ID_BYTEARRAY, TYPE_ID_CODE, TYPE_ID_DICT,
    TYPE_ID_EXCEPTION, TYPE_ID_FILE_HANDLE, TYPE_ID_FUNCTION, TYPE_ID_GENERATOR, TYPE_ID_ITER,
    TYPE_ID_LIST_BUILDER, TYPE_ID_OBJECT, TYPE_ID_STRING, TYPE_ID_TUPLE, asyncgen_call_finalizer,
    asyncgen_registry_remove, asyncio_fd_watcher_poll_fn_addr, asyncio_gather_poll_fn_addr,
    asyncio_ready_runner_poll_fn_addr, asyncio_server_accept_loop_poll_fn_addr,
    asyncio_sock_accept_poll_fn_addr, asyncio_sock_connect_poll_fn_addr,
    asyncio_sock_recv_into_poll_fn_addr, asyncio_sock_recv_poll_fn_addr,
    asyncio_sock_recvfrom_into_poll_fn_addr, asyncio_sock_recvfrom_poll_fn_addr,
    asyncio_sock_sendall_poll_fn_addr, asyncio_sock_sendto_poll_fn_addr,
    asyncio_socket_reader_read_poll_fn_addr, asyncio_socket_reader_readline_poll_fn_addr,
    asyncio_stream_reader_read_poll_fn_addr, asyncio_stream_reader_readline_poll_fn_addr,
    asyncio_stream_send_all_poll_fn_addr, asyncio_timer_handle_poll_fn_addr,
    asyncio_wait_for_poll_fn_addr, asyncio_wait_poll_fn_addr, builtin_classes_if_initialized,
    bytearray_data, bytearray_len, bytearray_vec_ptr, code_filename_bits, code_name_bits,
    code_names_bits, code_varnames_bits, contextlib_async_exitstack_enter_context_poll_fn_addr,
    contextlib_async_exitstack_exit_poll_fn_addr, contextlib_asyncgen_enter_poll_fn_addr,
    contextlib_asyncgen_exit_poll_fn_addr, dict_hashes_ptr, dict_order_ptr, dict_table_ptr,
    io_wait_detach_resource, io_wait_poll_fn_addr, map_iters_ptr, process_poll_fn_addr,
    profile_hit, profile_hit_bytes, runtime_state, seq_vec_ptr, set_hashes_ptr, set_order_ptr,
    set_table_ptr, thread_poll_fn_addr, utf8_cache_remove, weakref_clear_for_ptr,
    ws_wait_detach_resource, zip_iters_ptr,
};
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
    pub type_id: u32,        // 4 bytes
    ref_count: MoltRefCount, // 4 bytes; representation owned by `molt-codegen-abi`
    flags: MoltFlags,        // 4 bytes (semantic bits declared below)
    pub size_class: u16,     // 2 bytes — index into SIZE_CLASS_TABLE
    pub aux_kind: u16,       // 2 bytes — interpretation of aux
    pub aux: MoltAuxWord,    // 8 bytes — inline value or stable sidecar address
}
// Total: 24 bytes. The common class/state lanes stay inline; coexistence,
// polling, and oversized allocations use one stable per-object sidecar.

const _: () = {
    assert!(std::mem::size_of::<MoltHeader>() == molt_codegen_abi::HEADER_SIZE_BYTES as usize);
    assert!(std::mem::align_of::<MoltHeader>() <= molt_codegen_abi::HEADER_ALLOC_ALIGN_BYTES);
    assert!(std::mem::offset_of!(MoltHeader, aux_kind) == 14);
    assert!(std::mem::offset_of!(MoltHeader, aux) == 16);
    assert!(
        std::mem::size_of::<MoltHeader>()
            .is_multiple_of(molt_codegen_abi::HEADER_ALLOC_ALIGN_BYTES)
    );
};

impl MoltHeader {
    /// Start the lifetime of the refcount field in freshly allocated storage.
    /// Calling an atomic method on merely zero-filled bytes is not sufficient
    /// to construct an `AtomicU32` under Rust's object-lifetime rules.
    #[inline(always)]
    pub(crate) unsafe fn initialize_refcount_before_publication(header: *mut Self, value: u32) {
        unsafe {
            std::ptr::write(
                std::ptr::addr_of_mut!((*header).ref_count),
                MoltRefCount::new(value),
            );
        }
    }

    /// Diagnostic/ABI snapshot under the active concurrency contract. This is
    /// atomic in native free-threaded builds and GIL-confined otherwise. The
    /// value is never an ownership token and cannot justify a later dereference.
    #[inline(always)]
    pub fn ref_count_snapshot(&self) -> u32 {
        self.ref_count.snapshot_acquire()
    }

    /// Snapshot used only while the caller already owns the object or holds
    /// mutation authority. It avoids an unnecessary acquire on ARM.
    #[inline(always)]
    pub(crate) fn owned_ref_count_snapshot(&self) -> u32 {
        self.ref_count.snapshot_owned()
    }

    #[inline(always)]
    pub(crate) fn is_uniquely_owned(&self) -> bool {
        self.ref_count.snapshot_acquire() == 1
    }

    /// Retain an object through an existing owned reference.
    #[inline(always)]
    pub(crate) fn retain_owned(&self, count: usize, label: &str) -> u32 {
        if self.has_flag(HEADER_FLAG_IMMORTAL) {
            return self.ref_count.snapshot_owned();
        }
        let Ok(count) = u32::try_from(count) else {
            fatal_refcount_overflow(label, self.ref_count.snapshot_owned(), count);
        };
        match self
            .ref_count
            .retain_owned(count, || self.has_flag(HEADER_FLAG_DEALLOCATING))
        {
            Ok(previous) => previous,
            Err(RetainError::Overflow) => {
                fatal_refcount_overflow(label, self.ref_count.snapshot_owned(), count as usize)
            }
            Err(RetainError::Zero | RetainError::Deallocating | RetainError::Immortal) => {
                fatal_terminal_retain(label)
            }
        }
    }

    /// Upgrade registry custody to one ordinary owner only while the object is live.
    #[inline(always)]
    pub(crate) fn try_retain_live(&self) -> bool {
        if self.has_flag(HEADER_FLAG_DEALLOCATING | HEADER_FLAG_IMMORTAL) {
            return false;
        }
        self.ref_count
            .try_retain_live(|| self.has_flag(HEADER_FLAG_DEALLOCATING))
    }

    /// Release one owned reference. A terminal result carries the acquire
    /// fence required before payload destruction.
    #[inline(always)]
    pub(crate) fn release_owned(&self, label: &str) -> RefCountRelease {
        match self.ref_count.release_owned() {
            Ok(transition) => transition,
            Err(previous) => {
                eprintln!("molt fatal: invalid refcount release in {label} (previous={previous})");
                std::process::abort();
            }
        }
    }

    /// Sole post-publication transition into immortal custody.
    #[inline(always)]
    pub(crate) fn make_immortal(&self) {
        // SAFETY: this transition is called only by the runtime's exclusive
        // immortalization path after publication and before further owners.
        unsafe { self.ref_count.make_immortal_exclusive() };
    }

    /// Runtime-shutdown-only inverse of `make_immortal`.
    #[inline(always)]
    pub(crate) fn make_mortal_for_shutdown(&self) {
        // SAFETY: runtime shutdown has stopped concurrent owner transitions.
        unsafe { self.ref_count.make_mortal_for_shutdown_exclusive() };
    }

    /// Bridge-only restoration of the stable ABI-view hold.
    #[inline(always)]
    pub(crate) fn restore_stable_view_hold(&self) {
        // SAFETY: the bridge owns the sole stable-view lifecycle transition.
        unsafe { self.ref_count.restore_stable_view_hold_exclusive() };
    }

    /// Bridge-only retirement of the stable ABI-view hold.
    #[inline(always)]
    pub(crate) fn retire_stable_view_hold(&self) {
        // SAFETY: the bridge proved the stable view is the final hold.
        unsafe { self.ref_count.retire_stable_view_hold_exclusive() };
    }

    /// Add the collector's temporary strong pin and publish its flag.
    #[inline(always)]
    pub(crate) fn pin_for_gc(&self) {
        if self.has_flag(HEADER_FLAG_GC_PINNED) {
            eprintln!("molt fatal: object pinned twice by cycle collector");
            std::process::abort();
        }
        self.retain_owned(1, "cycle collector pin");
        self.fetch_or_flags(HEADER_FLAG_GC_PINNED);
    }

    /// Open the sole Python-visible finalizer/weakref revival window.
    #[inline(always)]
    pub(crate) fn open_revival_window(
        &self,
        has_stable_view_hold: bool,
    ) -> RefCountRevivalWindow<'_> {
        let expected = u32::from(has_stable_view_hold);
        match self.ref_count.open_revival_window(has_stable_view_hold) {
            Ok(baseline) => baseline,
            Err(previous) => {
                eprintln!(
                    "molt fatal: invalid refcount opening revival window (expected={expected}, actual={previous})"
                );
                std::process::abort();
            }
        }
    }

    #[inline(always)]
    pub(crate) fn close_revival_window(&self, window: RefCountRevivalWindow<'_>) -> u32 {
        let expected = window.baseline();
        match window.close() {
            Ok(previous) => previous,
            Err(actual) => {
                eprintln!(
                    "molt fatal: invalid refcount closing revival window (expected={expected}, actual={actual})"
                );
                std::process::abort();
            }
        }
    }
}

#[cold]
#[inline(never)]
fn fatal_terminal_retain(label: &str) -> ! {
    eprintln!("molt fatal: owned retain attempted after terminal death in {label}");
    std::process::abort()
}

#[cold]
#[inline(never)]
fn fatal_refcount_overflow(label: &str, current: u32, count: usize) -> ! {
    eprintln!("molt fatal: refcount overflow in {label} (count={current}, add={count})");
    std::process::abort()
}

/// Flags in this class directly coordinate cross-thread state or object
/// lifetime. Every other flag is metadata whose payload visibility is already
/// established by `GC_UNPUBLISHED` publication, the GIL, or its owning lock.
const HEADER_FLAG_SYNCHRONIZED_STATE_MASK: u32 = HEADER_FLAG_GEN_RUNNING
    | HEADER_FLAG_GEN_STARTED
    | HEADER_FLAG_SPAWN_RETAIN
    | HEADER_FLAG_CANCEL_PENDING
    | HEADER_FLAG_BLOCK_ON
    | HEADER_FLAG_TASK_QUEUED
    | HEADER_FLAG_TASK_RUNNING
    | HEADER_FLAG_TASK_WAKE_PENDING
    | HEADER_FLAG_TASK_DONE
    | HEADER_FLAG_FINALIZER_RAN
    | HEADER_FLAG_HAS_WEAKREF
    | HEADER_FLAG_GC_COLLECTING
    | HEADER_FLAG_HAS_ABI_VIEW
    | HEADER_FLAG_GC_PINNED
    | HEADER_FLAG_DEALLOCATING
    | HEADER_FLAG_IS_WEAKREF;

#[inline(always)]
const fn flag_transition_is_synchronized(changed: u32) -> bool {
    changed & HEADER_FLAG_SYNCHRONIZED_STATE_MASK != 0
}

impl MoltHeader {
    /// Construct the flag word before an object is published. This pointer API
    /// deliberately avoids creating a reference to uninitialized atomic state.
    #[inline(always)]
    pub(crate) unsafe fn initialize_flags_before_publication(header: *mut Self, value: u32) {
        unsafe {
            std::ptr::write(
                std::ptr::addr_of_mut!((*header).flags),
                MoltFlags::new(value),
            );
        }
    }

    /// Construct an unpublished collector state directly, without any
    /// intermediate published flag word.
    #[inline(always)]
    pub(crate) unsafe fn initialize_flags_gc_unpublished(header: *mut Self, value: u32) {
        unsafe {
            std::ptr::write(
                std::ptr::addr_of_mut!((*header).flags),
                MoltFlags::new_unpublished(value),
            );
        }
    }

    #[inline(always)]
    pub(crate) fn load_metadata_flags(&self) -> u32 {
        self.flags.load(AtomicOrdering::Relaxed)
    }

    #[inline(always)]
    pub(crate) fn load_synchronized_flags(&self) -> u32 {
        self.flags.load(AtomicOrdering::Acquire)
    }

    #[inline(always)]
    pub(crate) fn has_flag(&self, flag: u32) -> bool {
        let ordering = if flag_transition_is_synchronized(flag) {
            AtomicOrdering::Acquire
        } else {
            AtomicOrdering::Relaxed
        };
        self.flags.load(ordering) & flag != 0
    }

    #[inline(always)]
    #[cfg(test)]
    pub(crate) fn store_flags(&self, value: u32) {
        self.flags.store(value, AtomicOrdering::Release);
    }

    #[inline(always)]
    pub(crate) fn fetch_or_flags(&self, value: u32) -> u32 {
        if flag_transition_is_synchronized(value) {
            self.flags.update_synchronized(value, 0)
        } else {
            self.flags.update_relaxed(value, 0)
        }
    }

    #[inline(always)]
    pub(crate) fn fetch_and_flags(&self, value: u32) -> u32 {
        let clear = !value;
        if flag_transition_is_synchronized(clear) {
            self.flags.update_synchronized(0, clear)
        } else {
            self.flags.update_relaxed(0, clear)
        }
    }

    #[inline(always)]
    pub(crate) fn compare_exchange_synchronized_flags(
        &self,
        current: u32,
        new: u32,
    ) -> Result<u32, u32> {
        self.flags.compare_exchange(
            current,
            new,
            AtomicOrdering::AcqRel,
            AtomicOrdering::Acquire,
        )
    }

    /// Atomically apply one coherent flag-state transition. Callers that move
    /// between states (not merely publish a sticky bit) must use this instead
    /// of independent clear/set operations, so lock-free readers never observe
    /// a torn intermediate state.
    #[inline(always)]
    pub(crate) fn update_flags(&self, set: u32, clear: u32) -> u32 {
        if flag_transition_is_synchronized(set | clear) {
            self.flags.update_synchronized(set, clear)
        } else {
            self.flags.update_relaxed(set, clear)
        }
    }

    /// Clear and return the selected bits as one indivisible consume action.
    #[inline(always)]
    pub(crate) fn take_flags(&self, selected: u32) -> u32 {
        self.fetch_and_flags(!selected) & selected
    }

    /// Publish `set` only while every `forbidden` bit remains clear. The
    /// predicate and publication share one CAS, closing check-then-set races at
    /// terminal lifetime boundaries.
    #[inline(always)]
    pub(crate) fn try_set_flags_unless(&self, set: u32, forbidden: u32) -> bool {
        let mut observed = self.load_synchronized_flags();
        loop {
            if observed & forbidden != 0 {
                return false;
            }
            let updated = observed | set;
            if updated == observed {
                return true;
            }
            match self.compare_exchange_synchronized_flags(observed, updated) {
                Ok(_) => return true,
                Err(actual) => observed = actual,
            }
        }
    }

    #[inline(always)]
    pub(crate) fn gc_publish_initialized(&self) {
        self.flags.publish_initialized();
    }

    #[inline(always)]
    pub(crate) fn gc_is_published(&self) -> bool {
        self.flags.is_published()
    }
}

const _: () = {
    assert!(std::mem::offset_of!(MoltHeader, flags) == 8);
};

/// Eight-byte auxiliary storage that is atomic on native free-thread-capable
/// targets and a zero-overhead `Cell` on wasm32's single-threaded runtime.
#[cfg(not(target_arch = "wasm32"))]
#[repr(transparent)]
pub struct MoltAuxWord(AtomicU64);

#[cfg(target_arch = "wasm32")]
#[repr(transparent)]
pub struct MoltAuxWord(Cell<u64>);

impl MoltAuxWord {
    #[inline]
    pub const fn new(value: u64) -> Self {
        #[cfg(not(target_arch = "wasm32"))]
        {
            Self(AtomicU64::new(value))
        }
        #[cfg(target_arch = "wasm32")]
        {
            Self(Cell::new(value))
        }
    }

    #[inline]
    pub fn load(&self, order: AtomicOrdering) -> u64 {
        #[cfg(not(target_arch = "wasm32"))]
        {
            self.0.load(order)
        }
        #[cfg(target_arch = "wasm32")]
        {
            let _ = order;
            self.0.get()
        }
    }

    #[inline]
    pub fn store(&self, value: u64, order: AtomicOrdering) {
        #[cfg(not(target_arch = "wasm32"))]
        {
            self.0.store(value, order);
        }
        #[cfg(target_arch = "wasm32")]
        {
            let _ = order;
            self.0.set(value);
        }
    }

    #[inline]
    pub fn swap(&self, value: u64, order: AtomicOrdering) -> u64 {
        #[cfg(not(target_arch = "wasm32"))]
        {
            self.0.swap(value, order)
        }
        #[cfg(target_arch = "wasm32")]
        {
            let _ = order;
            self.0.replace(value)
        }
    }

    #[inline]
    pub fn compare_exchange(
        &self,
        current: u64,
        new: u64,
        success: AtomicOrdering,
        failure: AtomicOrdering,
    ) -> Result<u64, u64> {
        #[cfg(not(target_arch = "wasm32"))]
        {
            self.0.compare_exchange(current, new, success, failure)
        }
        #[cfg(target_arch = "wasm32")]
        {
            let _ = (success, failure);
            let observed = self.0.get();
            if observed == current {
                self.0.set(new);
                Ok(observed)
            } else {
                Err(observed)
            }
        }
    }

    #[inline]
    pub fn fetch_or(&self, value: u64, order: AtomicOrdering) -> u64 {
        #[cfg(not(target_arch = "wasm32"))]
        {
            self.0.fetch_or(value, order)
        }
        #[cfg(target_arch = "wasm32")]
        {
            let _ = order;
            let old = self.0.get();
            self.0.set(old | value);
            old
        }
    }

    #[inline]
    pub fn fetch_update<F>(
        &self,
        set_order: AtomicOrdering,
        fetch_order: AtomicOrdering,
        f: F,
    ) -> Result<u64, u64>
    where
        F: FnMut(u64) -> Option<u64>,
    {
        #[cfg(not(target_arch = "wasm32"))]
        {
            self.0.fetch_update(set_order, fetch_order, f)
        }
        #[cfg(target_arch = "wasm32")]
        {
            let _ = (set_order, fetch_order);
            let mut f = f;
            let old = self.0.get();
            if let Some(new) = f(old) {
                self.0.set(new);
                Ok(old)
            } else {
                Err(old)
            }
        }
    }
}

const _: () = {
    assert!(std::mem::size_of::<MoltAuxWord>() == std::mem::size_of::<u64>());
    assert!(std::mem::align_of::<MoltAuxWord>() == std::mem::align_of::<u64>());
};

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
    pub(crate) base_bits: u64,
    pub(crate) data: *mut u8,
    pub(crate) offset: isize,
    pub(crate) len: usize,
    pub(crate) itemsize: usize,
    pub(crate) stride: isize,
    pub(crate) readonly: u8,
    pub(crate) ndim: u8,
    pub(crate) released: u8,
    pub(crate) _pad: [u8; 5],
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

pub(crate) const HEADER_FLAG_HAS_PTRS: u32 = molt_codegen_abi::HEADER_FLAG_HAS_PTRS;
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
pub(crate) const HEADER_FLAG_IMMORTAL: u32 = molt_codegen_abi::HEADER_FLAG_IMMORTAL;
// Ensure __del__ runs at most once even if the object resurrects itself.
pub(crate) const HEADER_FLAG_FINALIZER_RAN: u32 = 1 << 16;
// String content is an ASCII identifier stored in the global intern pool.
// Objects with this flag are also immortal (never freed).
pub(crate) const HEADER_FLAG_INTERNED: u32 = 1 << 17;
/// Internal class field-offset dictionaries become immutable at class seal.
pub(crate) const HEADER_FLAG_FROZEN_LAYOUT_MAP: u32 = 1 << 18;
/// Container (list, tuple, dict, set) has at least one element that is a heap
/// pointer (TAG_PTR).  When this flag is clear, `dec_ref` cleanup can skip
/// iterating over elements because they are all primitives (int/float/bool/None).
pub(crate) const HEADER_FLAG_CONTAINS_REFS: u32 = molt_codegen_abi::HEADER_FLAG_CONTAINS_REFS;

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

/// `TYPE_ID_FUNCTION` metadata bit: this function object is a C-extension
/// trampoline whose C ABI convention owns arity validation.
pub(crate) const HEADER_FLAG_FUNC_VARIADIC_TRAMPOLINE: u32 = 1 << 26;
/// Transient stop-the-world cycle collector candidate bit.
pub(crate) const HEADER_FLAG_GC_COLLECTING: u32 = 1 << 25;
/// A canonical CPython ABI view exists for this heap object. This is the
/// lock-free negative membership authority for ordinary runtime RC/GC paths.
pub(crate) const HEADER_FLAG_HAS_ABI_VIEW: u32 = 1 << 27;
/// This object is itself a registered weakref and owns its callback edge in
/// the runtime registry. The Python shim carries no duplicate lifetime state.
/// Transient cycle-collector pin for an ABI-rooted candidate.
pub(crate) const HEADER_FLAG_GC_PINNED: u32 = 1 << 28;
pub(crate) const HEADER_FLAG_GC_UNPUBLISHED: u32 = molt_codegen_abi::HEADER_FLAG_GC_UNPUBLISHED;
/// Terminal lifetime state: the object is untracked and every externally
/// discoverable sidecar is being detached before any child edge is released.
pub(crate) const HEADER_FLAG_DEALLOCATING: u32 = 1 << 30;

/// Lifetime-boundary bit: this object has had at least one weakref registered.
/// It gates the finalizer resurrection window and subsequent weakref detach.
/// Once `__del__` declines to resurrect, DEALLOCATING is published before the
/// weakrefs are cleared: callbacks observe a dead referent and cannot reopen
/// ownership. The sticky bit preserves a zero-lock negative path for objects
/// that never participated in weakref state.
pub(crate) const HEADER_FLAG_HAS_WEAKREF: u32 = 1 << 24;
/// Marks weak-reference objects themselves, distinct from referents whose
/// sticky `HAS_WEAKREF` bit opens the finalization window.
pub(crate) const HEADER_FLAG_IS_WEAKREF: u32 = 1 << 31;

// Keep every persistent and transient lifetime bit in this single registry and
// fail compilation on any future collision. Cold type policy intentionally
// lives in the type payload rather than consuming hot RC/GC header capacity.
const HEADER_FLAG_REGISTRY: [u32; 31] = [
    HEADER_FLAG_HAS_PTRS,
    HEADER_FLAG_GEN_RUNNING,
    HEADER_FLAG_GEN_STARTED,
    HEADER_FLAG_SPAWN_RETAIN,
    HEADER_FLAG_CANCEL_PENDING,
    HEADER_FLAG_BLOCK_ON,
    HEADER_FLAG_TASK_QUEUED,
    HEADER_FLAG_TASK_RUNNING,
    HEADER_FLAG_TASK_WAKE_PENDING,
    HEADER_FLAG_TASK_DONE,
    HEADER_FLAG_TRACEBACK_SUPPRESSED,
    HEADER_FLAG_COROUTINE,
    HEADER_FLAG_FUNC_TASK_TRAMPOLINE_KNOWN,
    HEADER_FLAG_FUNC_TASK_TRAMPOLINE_NEEDED,
    HEADER_FLAG_IMMORTAL,
    HEADER_FLAG_FINALIZER_RAN,
    HEADER_FLAG_INTERNED,
    HEADER_FLAG_FROZEN_LAYOUT_MAP,
    HEADER_FLAG_CONTAINS_REFS,
    HEADER_FLAG_RAW_ALLOC,
    HEADER_FLAG_ARENA,
    HEADER_FLAG_CLASS_HAS_FINALIZER,
    HEADER_FLAG_FUNC_REQUIRES_BINDER,
    HEADER_FLAG_HAS_WEAKREF,
    HEADER_FLAG_GC_COLLECTING,
    HEADER_FLAG_FUNC_VARIADIC_TRAMPOLINE,
    HEADER_FLAG_HAS_ABI_VIEW,
    HEADER_FLAG_GC_PINNED,
    HEADER_FLAG_GC_UNPUBLISHED,
    HEADER_FLAG_DEALLOCATING,
    HEADER_FLAG_IS_WEAKREF,
];

const _: () = {
    let mut i = 0;
    while i < HEADER_FLAG_REGISTRY.len() {
        assert!(HEADER_FLAG_REGISTRY[i].count_ones() == 1);
        let mut j = i + 1;
        while j < HEADER_FLAG_REGISTRY.len() {
            assert!(HEADER_FLAG_REGISTRY[i] & HEADER_FLAG_REGISTRY[j] == 0);
            j += 1;
        }
        i += 1;
    }
};

const CLASS_POLICY_NOT_BASE: u64 = 1;
const CLASS_POLICY_IMMUTABLE: u64 = 1 << 1;
const CLASS_POLICY_INSTANCE_KIND_EXPLICIT: u64 = 1 << 2;
const CLASS_POLICY_INSTANCE_SHAPE_EXPLICIT: u64 = 1 << 3;
const CLASS_POLICY_DEFINITION_FINISHED: u64 = 1 << 4;
const CLASS_POLICY_INSTANCE_SHAPE_SHIFT: u32 = 8;
const CLASS_POLICY_INSTANCE_SHAPE_MASK: u64 =
    (u16::MAX as u64) << CLASS_POLICY_INSTANCE_SHAPE_SHIFT;
const CLASS_POLICY_INSTANCE_KIND_SHIFT: u32 = molt_codegen_abi::CLASS_POLICY_INSTANCE_KIND_SHIFT;
const CLASS_POLICY_INSTANCE_KIND_MASK: u64 = (u32::MAX as u64) << CLASS_POLICY_INSTANCE_KIND_SHIFT;
const CLASS_POLICY_WORD_OFFSET: usize = molt_codegen_abi::CLASS_POLICY_WORD_OFFSET as usize;

#[inline]
unsafe fn class_policy_word<'a>(class_ptr: *mut u8) -> &'a MoltAuxWord {
    // TYPE_ID_TYPE payloads reserve this naturally aligned word exclusively
    // for monotonic class policy. Atomic publication keeps this boundary valid
    // when type reads become GIL-free; policy never pollutes the hot RC/GC word.
    unsafe { &*(class_ptr.add(CLASS_POLICY_WORD_OFFSET) as *const MoltAuxWord) }
}

pub(crate) use molt_runtime_core::{
    ObjectShapeId, ObjectShapeLifecycleFamily, ObjectShapeResourceSlot, object_shape_is_task,
    object_shape_lifecycle_family, object_shape_resource_slot,
};

#[inline]
pub(crate) unsafe fn class_instance_shape_id(class_ptr: *mut u8) -> ObjectShapeId {
    let word = unsafe { class_policy_word(class_ptr).load(AtomicOrdering::Acquire) };
    let encoded =
        ((word & CLASS_POLICY_INSTANCE_SHAPE_MASK) >> CLASS_POLICY_INSTANCE_SHAPE_SHIFT) as u16;
    ObjectShapeId::from_u16(encoded).expect("corrupt class instance-shape policy")
}

/// Install the payload shape owned by instances of a runtime-created class.
/// The shape is immutable once selected; a conflicting second writer is an
/// invariant violation rather than a compatibility lane.
pub(crate) unsafe fn class_set_instance_shape_id(class_ptr: *mut u8, shape: ObjectShapeId) -> bool {
    if class_ptr.is_null() || unsafe { object_type_id(class_ptr) } != TYPE_ID_TYPE {
        return false;
    }
    let encoded = (shape as u64) << CLASS_POLICY_INSTANCE_SHAPE_SHIFT;
    unsafe {
        class_policy_word(class_ptr)
            .fetch_update(AtomicOrdering::AcqRel, AtomicOrdering::Acquire, |word| {
                let current = word & CLASS_POLICY_INSTANCE_SHAPE_MASK;
                (current == 0 || current == encoded).then_some(
                    (word & !CLASS_POLICY_INSTANCE_SHAPE_MASK)
                        | encoded
                        | CLASS_POLICY_INSTANCE_SHAPE_EXPLICIT,
                )
            })
            .is_ok()
    }
}

pub(crate) unsafe fn class_can_inherit_instance_shape_id(
    class_ptr: *mut u8,
    inherited: ObjectShapeId,
) -> bool {
    let word = unsafe { class_policy_word(class_ptr).load(AtomicOrdering::Acquire) };
    let current =
        ((word & CLASS_POLICY_INSTANCE_SHAPE_MASK) >> CLASS_POLICY_INSTANCE_SHAPE_SHIFT) as u16;
    if inherited == ObjectShapeId::Plain {
        current == 0 || word & CLASS_POLICY_INSTANCE_SHAPE_EXPLICIT != 0
    } else {
        current == 0 || current == inherited as u16
    }
}

pub(crate) unsafe fn class_inherit_instance_shape_id(
    class_ptr: *mut u8,
    inherited: ObjectShapeId,
) -> bool {
    let encoded = (inherited as u64) << CLASS_POLICY_INSTANCE_SHAPE_SHIFT;
    unsafe {
        class_policy_word(class_ptr)
            .fetch_update(AtomicOrdering::AcqRel, AtomicOrdering::Acquire, |word| {
                let current = word & CLASS_POLICY_INSTANCE_SHAPE_MASK;
                let explicit = word & CLASS_POLICY_INSTANCE_SHAPE_EXPLICIT != 0;
                if inherited == ObjectShapeId::Plain {
                    return (current == 0 || explicit).then_some(word);
                }
                (current == 0 || current == encoded)
                    .then_some((word & !CLASS_POLICY_INSTANCE_SHAPE_MASK) | encoded)
            })
            .is_ok()
    }
}

#[inline]
unsafe fn class_add_policy(class_ptr: *mut u8, policy: u64) {
    unsafe {
        class_policy_word(class_ptr).fetch_or(policy, AtomicOrdering::AcqRel);
    }
}

#[inline]
unsafe fn class_has_policy(class_ptr: *mut u8, policy: u64) -> bool {
    unsafe { class_policy_word(class_ptr).load(AtomicOrdering::Acquire) & policy != 0 }
}

#[inline]
pub(crate) unsafe fn class_instance_type_id(class_ptr: *mut u8) -> u32 {
    let word = unsafe { class_policy_word(class_ptr).load(AtomicOrdering::Acquire) };
    let encoded = (word & CLASS_POLICY_INSTANCE_KIND_MASK) >> CLASS_POLICY_INSTANCE_KIND_SHIFT;
    if encoded == 0 {
        TYPE_ID_OBJECT
    } else {
        encoded as u32
    }
}

pub(crate) unsafe fn class_set_instance_type_id(class_ptr: *mut u8, type_id: u32) -> bool {
    if heap_layout_policy(type_id) != Some(HeapLayoutPolicy::Object)
        || heap_shape_policy(type_id) != Some(HeapShapePolicy::Class)
    {
        return false;
    }
    let encoded = (type_id as u64) << CLASS_POLICY_INSTANCE_KIND_SHIFT;
    unsafe {
        class_policy_word(class_ptr)
            .fetch_update(AtomicOrdering::AcqRel, AtomicOrdering::Acquire, |word| {
                let current = word & CLASS_POLICY_INSTANCE_KIND_MASK;
                (current == 0 || current == encoded).then_some(
                    (word & !CLASS_POLICY_INSTANCE_KIND_MASK)
                        | encoded
                        | CLASS_POLICY_INSTANCE_KIND_EXPLICIT,
                )
            })
            .is_ok()
    }
}

pub(crate) unsafe fn class_inherit_instance_type_id(
    class_ptr: *mut u8,
    inherited_type_id: u32,
) -> bool {
    if heap_layout_policy(inherited_type_id) != Some(HeapLayoutPolicy::Object)
        || heap_shape_policy(inherited_type_id) != Some(HeapShapePolicy::Class)
    {
        return false;
    }
    let inherited = (inherited_type_id as u64) << CLASS_POLICY_INSTANCE_KIND_SHIFT;
    unsafe {
        class_policy_word(class_ptr)
            .fetch_update(AtomicOrdering::AcqRel, AtomicOrdering::Acquire, |word| {
                let current = word & CLASS_POLICY_INSTANCE_KIND_MASK;
                let explicit = word & CLASS_POLICY_INSTANCE_KIND_EXPLICIT != 0;
                let current_type_id = if current == 0 {
                    TYPE_ID_OBJECT
                } else {
                    (current >> CLASS_POLICY_INSTANCE_KIND_SHIFT) as u32
                };
                if inherited_type_id == TYPE_ID_OBJECT {
                    return (current_type_id == TYPE_ID_OBJECT || explicit).then_some(word);
                }
                (current_type_id == TYPE_ID_OBJECT || current == inherited)
                    .then_some((word & !CLASS_POLICY_INSTANCE_KIND_MASK) | inherited)
            })
            .is_ok()
    }
}

pub(crate) unsafe fn class_can_inherit_instance_type_id(
    class_ptr: *mut u8,
    inherited_type_id: u32,
) -> bool {
    let word = unsafe { class_policy_word(class_ptr).load(AtomicOrdering::Acquire) };
    let current = word & CLASS_POLICY_INSTANCE_KIND_MASK;
    let current_type_id = if current == 0 {
        TYPE_ID_OBJECT
    } else {
        (current >> CLASS_POLICY_INSTANCE_KIND_SHIFT) as u32
    };
    if inherited_type_id == TYPE_ID_OBJECT {
        current_type_id == TYPE_ID_OBJECT || word & CLASS_POLICY_INSTANCE_KIND_EXPLICIT != 0
    } else {
        current_type_id == TYPE_ID_OBJECT || current_type_id == inherited_type_id
    }
}

pub(crate) unsafe fn class_set_not_base(_py: &PyToken<'_>, class_ptr: *mut u8) -> bool {
    crate::gil_assert();
    if class_ptr.is_null() || unsafe { object_type_id(class_ptr) } != TYPE_ID_TYPE {
        return false;
    }
    unsafe {
        class_add_policy(class_ptr, CLASS_POLICY_NOT_BASE);
    }
    true
}

pub(crate) unsafe fn class_is_not_base(_py: &PyToken<'_>, class_ptr: *mut u8) -> bool {
    crate::gil_assert();
    if class_ptr.is_null() || unsafe { object_type_id(class_ptr) } != TYPE_ID_TYPE {
        return false;
    }
    unsafe { class_has_policy(class_ptr, CLASS_POLICY_NOT_BASE) }
}

pub(crate) unsafe fn class_set_immutable(_py: &PyToken<'_>, class_ptr: *mut u8) -> bool {
    crate::gil_assert();
    if class_ptr.is_null() || unsafe { object_type_id(class_ptr) } != TYPE_ID_TYPE {
        return false;
    }
    unsafe {
        class_add_policy(class_ptr, CLASS_POLICY_IMMUTABLE);
    }
    true
}

pub(crate) unsafe fn class_is_immutable(_py: &PyToken<'_>, class_ptr: *mut u8) -> bool {
    crate::gil_assert();
    if class_ptr.is_null() || unsafe { object_type_id(class_ptr) } != TYPE_ID_TYPE {
        return false;
    }
    unsafe { class_has_policy(class_ptr, CLASS_POLICY_IMMUTABLE) }
}

pub(crate) unsafe fn class_definition_is_finished(class_ptr: *mut u8) -> bool {
    unsafe { class_has_policy(class_ptr, CLASS_POLICY_DEFINITION_FINISHED) }
}

// ---------------------------------------------------------------------------
// Header aux authority
// ---------------------------------------------------------------------------

pub(crate) const HEADER_AUX_KIND_NONE: u16 = molt_codegen_abi::HEADER_AUX_KIND_NONE;
pub(crate) const HEADER_AUX_KIND_CLASS_INLINE: u16 = molt_codegen_abi::HEADER_AUX_KIND_CLASS_INLINE;
pub(crate) const HEADER_AUX_KIND_STATE_INLINE: u16 = molt_codegen_abi::HEADER_AUX_KIND_STATE_INLINE;
pub(crate) const HEADER_AUX_KIND_SIDECAR: u16 = molt_codegen_abi::HEADER_AUX_KIND_SIDECAR;

#[inline]
const fn header_aux_storage_bytes(kind: u16) -> usize {
    if kind == HEADER_AUX_KIND_SIDECAR {
        aux_sidecar_size()
    } else {
        0
    }
}
pub(crate) const HEADER_CLASS_WORD_BORROWED: u64 = molt_codegen_abi::HEADER_CLASS_WORD_BORROWED;
pub(crate) const HEADER_CLASS_WORD_TAG_MASK: u64 = molt_codegen_abi::HEADER_CLASS_WORD_TAG_MASK;
pub(crate) const HEADER_CLASS_WORD_BITS_MASK: u64 = molt_codegen_abi::HEADER_CLASS_WORD_BITS_MASK;

#[derive(Clone, Copy, Debug)]
struct ObjectAuxSnapshot {
    kind: u16,
    word: u64,
}

/// Representation selected before GC tracking/publication. Constructors use
/// this when their future class/state/poll needs are known at allocation time.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum ObjectAuxPreselection {
    Default,
    ClassInline,
    StateInline,
    Sidecar,
}

#[inline]
unsafe fn header_aux_snapshot(header: *const MoltHeader) -> ObjectAuxSnapshot {
    unsafe {
        let kind = (*header).aux_kind;
        debug_assert!(matches!(
            kind,
            HEADER_AUX_KIND_NONE
                | HEADER_AUX_KIND_CLASS_INLINE
                | HEADER_AUX_KIND_STATE_INLINE
                | HEADER_AUX_KIND_SIDECAR
        ));
        ObjectAuxSnapshot {
            kind,
            word: (*header).aux.load(AtomicOrdering::Acquire),
        }
    }
}

#[inline]
unsafe fn object_aux_snapshot(data_ptr: *mut u8) -> ObjectAuxSnapshot {
    unsafe { header_aux_snapshot(header_from_obj_ptr(data_ptr)) }
}

#[inline]
unsafe fn sidecar_from_snapshot(snapshot: ObjectAuxSnapshot) -> &'static MoltAuxSidecar {
    debug_assert_eq!(snapshot.kind, HEADER_AUX_KIND_SIDECAR);
    unsafe { aux_sidecar_from_word(snapshot.word) }
}

/// Select the initial aux representation before the object becomes visible.
/// The kind and sidecar address are immutable after publication.
unsafe fn initialize_header_aux(
    header: *mut MoltHeader,
    type_id: u32,
    size_class: u16,
    total_size: usize,
    preselection: ObjectAuxPreselection,
) -> bool {
    unsafe {
        let (kind, word) = if size_class == 0 {
            let Some(word) = alloc_aux_sidecar(MoltAuxSidecar::new(0, 0, 0, total_size)) else {
                return false;
            };
            (HEADER_AUX_KIND_SIDECAR, word)
        } else if preselection == ObjectAuxPreselection::Sidecar
            || (preselection == ObjectAuxPreselection::Default
                && matches!(type_id, TYPE_ID_GENERATOR | TYPE_ID_ASYNC_GENERATOR))
        {
            let Some(word) = alloc_aux_sidecar(MoltAuxSidecar::new(0, 0, 0, 0)) else {
                return false;
            };
            (HEADER_AUX_KIND_SIDECAR, word)
        } else if preselection == ObjectAuxPreselection::ClassInline {
            (HEADER_AUX_KIND_CLASS_INLINE, 0)
        } else if preselection == ObjectAuxPreselection::StateInline
            || (preselection == ObjectAuxPreselection::Default
                && matches!(type_id, TYPE_ID_STRING | TYPE_ID_BYTES | TYPE_ID_BYTEARRAY))
        {
            (HEADER_AUX_KIND_STATE_INLINE, 0)
        } else {
            (HEADER_AUX_KIND_NONE, 0)
        };
        std::ptr::write(
            std::ptr::addr_of_mut!((*header).aux),
            MoltAuxWord::new(word),
        );
        (*header).aux_kind = kind;
        true
    }
}

/// Upgrade an unpublished object to stable sidecar storage while preserving
/// any inline class or state word. Published callers must never use this: a
/// kind/address transition after publication would be a torn two-word update.
///
/// # Safety
/// `data_ptr` must refer to a live object that has not been published to any
/// reader, registry, container, GC traversal, or foreign ABI view.
#[must_use]
pub(crate) unsafe fn object_init_sidecar_unpublished(data_ptr: *mut u8) -> bool {
    unsafe {
        let header = header_from_obj_ptr(data_ptr);
        let snapshot = header_aux_snapshot(header);
        if snapshot.kind == HEADER_AUX_KIND_SIDECAR {
            return true;
        }
        let (class_edge, state) = match snapshot.kind {
            HEADER_AUX_KIND_CLASS_INLINE => (snapshot.word, 0),
            HEADER_AUX_KIND_STATE_INLINE => (0, snapshot.word as i64),
            HEADER_AUX_KIND_NONE => (0, 0),
            _ => unreachable!("validated aux kind"),
        };
        let Some(word) = alloc_aux_sidecar(MoltAuxSidecar::new(class_edge, 0, state, 0)) else {
            return false;
        };
        (*header).aux.store(word, AtomicOrdering::Release);
        (*header).aux_kind = HEADER_AUX_KIND_SIDECAR;
        true
    }
}

/// Derive the total allocation size from a stable header snapshot. Oversized
/// objects carry their exact immutable size in their sidecar.
#[inline]
fn total_size_from_header_fields(size_class: u16, aux_kind: u16, aux_word: u64) -> usize {
    let sc = size_class as usize;
    if sc != 0 && sc < SIZE_CLASS_TABLE.len() {
        SIZE_CLASS_TABLE[sc]
    } else if aux_kind == HEADER_AUX_KIND_SIDECAR && aux_word != 0 {
        unsafe { aux_sidecar_from_word(aux_word) }.extended_size
    } else {
        // Immortal stack objects and arena allocations do not participate in
        // allocator deallocation and intentionally carry no exact size.
        0
    }
}

#[inline]
pub(crate) fn total_size_from_header(header: &MoltHeader, _data_ptr: *mut u8) -> usize {
    let snapshot = unsafe { header_aux_snapshot(header) };
    total_size_from_header_fields(header.size_class, snapshot.kind, snapshot.word)
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
    let layout =
        Layout::from_size_align(alloc_size, molt_codegen_abi::HEADER_ALLOC_ALIGN_BYTES).ok()?;
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

/// Get the poll function from stable sidecar storage.
#[inline]
pub(crate) fn object_poll_fn(data_ptr: *mut u8) -> u64 {
    let snapshot = unsafe { object_aux_snapshot(data_ptr) };
    if snapshot.kind != HEADER_AUX_KIND_SIDECAR {
        return 0;
    }
    unsafe { sidecar_from_snapshot(snapshot) }.poll_fn()
}

/// Read the immutable typed payload shape selected before publication.
///
/// Poll/task shapes live in the object's sidecar. Class-governed builtin
/// shapes live in the class policy word and are inherited with the class
/// layout. This is the only lifecycle dispatch authority for OBJECT payloads.
#[inline]
pub(crate) fn object_shape_id(data_ptr: *mut u8) -> ObjectShapeId {
    let snapshot = unsafe { object_aux_snapshot(data_ptr) };
    if snapshot.kind == HEADER_AUX_KIND_SIDECAR {
        let encoded = unsafe { sidecar_from_snapshot(snapshot) }.shape();
        if encoded != 0 {
            return ObjectShapeId::from_u16(encoded).expect("corrupt object shape id");
        }
    }
    let class_bits = unsafe { object_class_bits(data_ptr) };
    obj_from_bits(class_bits)
        .as_ptr()
        .map(|class_ptr| unsafe { class_instance_shape_id(class_ptr) })
        .unwrap_or(ObjectShapeId::Plain)
}

/// Set a sidecar-owned payload shape before the object is published.
/// Conflicting initialization is rejected; published mutation is forbidden.
#[must_use]
pub(crate) unsafe fn object_init_shape_unpublished(
    data_ptr: *mut u8,
    shape: ObjectShapeId,
) -> bool {
    unsafe {
        if !object_init_sidecar_unpublished(data_ptr) {
            return false;
        }
        let sidecar = sidecar_from_snapshot(object_aux_snapshot(data_ptr));
        sidecar
            .shape
            .fetch_update(AtomicOrdering::AcqRel, AtomicOrdering::Acquire, |current| {
                (current == 0 || current == shape as u64).then_some(shape as u64)
            })
            .is_ok()
    }
}

/// Resolve a task constructor's code pointer to its immutable lifecycle shape.
/// This conversion runs once while the object is unpublished; lifecycle paths
/// dispatch only on the resulting compact ID.
pub(crate) fn object_shape_for_poll_fn(poll_fn: u64) -> ObjectShapeId {
    if poll_fn == crate::promise_poll_fn_addr() {
        ObjectShapeId::Promise
    } else if poll_fn == crate::async_sleep_poll_fn_addr() {
        ObjectShapeId::AsyncSleep
    } else if poll_fn == crate::asyncgen_poll_fn_addr() {
        ObjectShapeId::AsyncGeneratorFuture
    } else if poll_fn == crate::anext_default_poll_fn_addr() {
        ObjectShapeId::AnextDefault
    } else if poll_fn == asyncio_wait_poll_fn_addr() {
        ObjectShapeId::AsyncioWait
    } else if poll_fn == asyncio_gather_poll_fn_addr() {
        ObjectShapeId::AsyncioGather
    } else if poll_fn == asyncio_wait_for_poll_fn_addr() {
        ObjectShapeId::AsyncioWaitFor
    } else if poll_fn == asyncio_timer_handle_poll_fn_addr() {
        ObjectShapeId::AsyncioTimerHandle
    } else if poll_fn == asyncio_fd_watcher_poll_fn_addr() {
        ObjectShapeId::AsyncioFdWatcher
    } else if poll_fn == asyncio_server_accept_loop_poll_fn_addr() {
        ObjectShapeId::AsyncioServerAcceptLoop
    } else if poll_fn == asyncio_ready_runner_poll_fn_addr() {
        ObjectShapeId::AsyncioReadyRunner
    } else if poll_fn == contextlib_asyncgen_enter_poll_fn_addr() {
        ObjectShapeId::ContextlibAsyncgenEnter
    } else if poll_fn == contextlib_asyncgen_exit_poll_fn_addr() {
        ObjectShapeId::ContextlibAsyncgenExit
    } else if poll_fn == contextlib_async_exitstack_enter_context_poll_fn_addr() {
        ObjectShapeId::ContextlibAsyncExitstackEnter
    } else if poll_fn == contextlib_async_exitstack_exit_poll_fn_addr() {
        ObjectShapeId::ContextlibAsyncExitstackExit
    } else if poll_fn == asyncio_socket_reader_read_poll_fn_addr() {
        ObjectShapeId::AsyncioSocketReaderRead
    } else if poll_fn == asyncio_socket_reader_readline_poll_fn_addr() {
        ObjectShapeId::AsyncioSocketReaderReadline
    } else if poll_fn == asyncio_stream_reader_read_poll_fn_addr() {
        ObjectShapeId::AsyncioStreamReaderRead
    } else if poll_fn == asyncio_stream_reader_readline_poll_fn_addr() {
        ObjectShapeId::AsyncioStreamReaderReadline
    } else if poll_fn == asyncio_stream_send_all_poll_fn_addr() {
        ObjectShapeId::AsyncioStreamSendAll
    } else if poll_fn == asyncio_sock_recv_poll_fn_addr() {
        ObjectShapeId::AsyncioSockRecv
    } else if poll_fn == asyncio_sock_connect_poll_fn_addr() {
        ObjectShapeId::AsyncioSockConnect
    } else if poll_fn == asyncio_sock_accept_poll_fn_addr() {
        ObjectShapeId::AsyncioSockAccept
    } else if poll_fn == asyncio_sock_recv_into_poll_fn_addr() {
        ObjectShapeId::AsyncioSockRecvInto
    } else if poll_fn == asyncio_sock_sendall_poll_fn_addr() {
        ObjectShapeId::AsyncioSockSendAll
    } else if poll_fn == asyncio_sock_recvfrom_poll_fn_addr() {
        ObjectShapeId::AsyncioSockRecvFrom
    } else if poll_fn == asyncio_sock_recvfrom_into_poll_fn_addr() {
        ObjectShapeId::AsyncioSockRecvFromInto
    } else if poll_fn == asyncio_sock_sendto_poll_fn_addr() {
        ObjectShapeId::AsyncioSockSendTo
    } else if poll_fn == thread_poll_fn_addr() {
        ObjectShapeId::ThreadTask
    } else if poll_fn == process_poll_fn_addr() {
        ObjectShapeId::ProcessTask
    } else if poll_fn == io_wait_poll_fn_addr() {
        ObjectShapeId::IoWait
    } else if poll_fn == ws_wait_poll_fn_addr() {
        ObjectShapeId::WebsocketWait
    } else if poll_fn != 0 {
        ObjectShapeId::GenericTaskPayload
    } else {
        ObjectShapeId::Plain
    }
}

/// Initialize a poll function before publication, selecting sidecar storage if
/// necessary.
///
/// # Safety
/// `data_ptr` must still satisfy `object_init_sidecar_unpublished`'s unpublished
/// object contract.
#[must_use]
pub(crate) unsafe fn object_init_poll_fn_unpublished(data_ptr: *mut u8, poll_fn: u64) -> bool {
    unsafe {
        if !object_init_sidecar_unpublished(data_ptr) {
            return false;
        }
        let sidecar = sidecar_from_snapshot(object_aux_snapshot(data_ptr));
        sidecar.poll_fn.store(poll_fn, AtomicOrdering::Release);
        true
    }
}

#[inline]
fn object_state_raw(data_ptr: *mut u8) -> i64 {
    let snapshot = unsafe { object_aux_snapshot(data_ptr) };
    match snapshot.kind {
        HEADER_AUX_KIND_STATE_INLINE => snapshot.word as i64,
        HEADER_AUX_KIND_SIDECAR => unsafe { sidecar_from_snapshot(snapshot) }.state(),
        _ => 0,
    }
}

#[inline]
/// Read non-class state. Class and state coexist in separate sidecar lanes.
pub(crate) fn object_state(data_ptr: *mut u8) -> i64 {
    object_state_raw(data_ptr)
}

/// Initialize state before publication, selecting inline or sidecar storage
/// while it is still safe to choose the object's immutable representation.
///
/// # Safety
/// `data_ptr` must not yet be published to any reader.
#[must_use]
pub(crate) unsafe fn object_init_state_unpublished(data_ptr: *mut u8, state: i64) -> bool {
    unsafe {
        let header = header_from_obj_ptr(data_ptr);
        let snapshot = header_aux_snapshot(header);
        match snapshot.kind {
            HEADER_AUX_KIND_NONE => {
                (*header).aux.store(state as u64, AtomicOrdering::Release);
                (*header).aux_kind = HEADER_AUX_KIND_STATE_INLINE;
            }
            HEADER_AUX_KIND_STATE_INLINE => {
                (*header).aux.store(state as u64, AtomicOrdering::Release);
            }
            HEADER_AUX_KIND_CLASS_INLINE => {
                if !object_init_sidecar_unpublished(data_ptr) {
                    return false;
                }
                sidecar_from_snapshot(object_aux_snapshot(data_ptr))
                    .state
                    .store(state as u64, AtomicOrdering::Release);
            }
            HEADER_AUX_KIND_SIDECAR => {
                sidecar_from_snapshot(snapshot)
                    .state
                    .store(state as u64, AtomicOrdering::Release);
            }
            _ => unreachable!("validated aux kind"),
        }
        true
    }
}

/// Mutate state without changing the published aux kind/address.
pub(crate) fn object_set_state(data_ptr: *mut u8, state: i64) {
    let snapshot = unsafe { object_aux_snapshot(data_ptr) };
    match snapshot.kind {
        HEADER_AUX_KIND_STATE_INLINE => unsafe {
            (*header_from_obj_ptr(data_ptr))
                .aux
                .store(state as u64, AtomicOrdering::Release);
        },
        HEADER_AUX_KIND_SIDECAR => unsafe {
            sidecar_from_snapshot(snapshot)
                .state
                .store(state as u64, AtomicOrdering::Release);
        },
        _ => panic!("published object state requires preselected state or sidecar aux storage"),
    }
}

// ---------------------------------------------------------------------------
// C API wrappers for aux state access (used by the native JIT backend). The
// helper preserves one ABI across inline-state and stable-sidecar objects.
// ---------------------------------------------------------------------------

/// Read the generator/coroutine state for the object at `data_ptr`.
/// Returns the state value (0 if the selected aux representation has no state lane).
#[unsafe(no_mangle)]
pub extern "C" fn molt_obj_get_state(data_ptr_bits: u64) -> i64 {
    let Some(data_ptr) = crate::provenance::abi::mut_ptr::<u8>(data_ptr_bits) else {
        return 0;
    };
    if data_ptr.is_null() {
        return 0;
    }
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
    let Some(data_ptr) = crate::provenance::abi::mut_ptr::<u8>(data_ptr_bits) else {
        return;
    };
    if data_ptr.is_null() {
        return;
    }
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
///     - `flags          = HEADER_FLAG_IMMORTAL` (so dec_ref_ptr
///       short-circuits and the runtime never tries to free a
///       stack pointer through the dealloc path; the class is
///       borrowed from the module-owned class object)
///     - `size_class     = 0`  (size lives nowhere — IMMORTAL
///       objects bypass the size lookup paths)
///     - `aux_kind       = CLASS_INLINE`
///     - `aux            = class_bits | BORROWED`
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
        let Some(payload) = crate::usize_from_bits(payload_size_bytes) else {
            return MoltObject::none().bits();
        };
        let Some(total) = std::mem::size_of::<MoltHeader>().checked_add(payload) else {
            return MoltObject::none().bits();
        };
        std::ptr::write_bytes(header_ptr, 0, total);
        let header = header_ptr as *mut MoltHeader;
        (*header).type_id = class_instance_type_id(cls_ptr);
        MoltHeader::initialize_refcount_before_publication(header, 1);
        MoltHeader::initialize_flags_before_publication(header, HEADER_FLAG_IMMORTAL);
        (*header).size_class = 0;
        (*header).aux_kind = HEADER_AUX_KIND_CLASS_INLINE;
        std::ptr::write(
            std::ptr::addr_of_mut!((*header).aux),
            MoltAuxWord::new(cls_bits | HEADER_CLASS_WORD_BORROWED),
        );
        let data_ptr = header_ptr.add(std::mem::size_of::<MoltHeader>());
        MoltObject::from_ptr(data_ptr).bits()
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
        if (*header_ptr).has_flag(HEADER_FLAG_HAS_ABI_VIEW) {
            molt_cpython_abi::bridge::GLOBAL_BRIDGE.prepare_runtime_immortal_for_shutdown(bits);
        }
        (*header_ptr).make_mortal_for_shutdown();
        (*header_ptr).fetch_and_flags(!(HEADER_FLAG_IMMORTAL | HEADER_FLAG_INTERNED));
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
        if (*header_ptr).has_flag(HEADER_FLAG_INTERNED) {
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

pub(crate) fn alloc_object_zeroed_with_aux(
    _py: &PyToken<'_>,
    total_size: usize,
    type_id: u32,
    aux: ObjectAuxPreselection,
) -> *mut u8 {
    alloc_object_zeroed_with_aux_policy(_py, total_size, type_id, aux, false)
}

pub(crate) fn alloc_object_zeroed_unpublished_with_aux(
    _py: &PyToken<'_>,
    total_size: usize,
    type_id: u32,
    aux: ObjectAuxPreselection,
) -> *mut u8 {
    alloc_object_zeroed_with_aux_policy(_py, total_size, type_id, aux, true)
}

fn alloc_object_zeroed_with_aux_policy(
    _py: &PyToken<'_>,
    total_size: usize,
    type_id: u32,
    aux: ObjectAuxPreselection,
    unpublished: bool,
) -> *mut u8 {
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
        let header = ptr as *mut MoltHeader;
        (*header).type_id = type_id;
        MoltHeader::initialize_refcount_before_publication(header, 1);
        if unpublished {
            MoltHeader::initialize_flags_gc_unpublished(header, 0);
        } else {
            MoltHeader::initialize_flags_before_publication(header, 0);
        }
        (*header).size_class = plan.size_class;
        if !initialize_header_aux(header, type_id, plan.size_class, total_size, aux) {
            std::alloc::dealloc(ptr, plan.layout);
            release_object_allocation_reservation(plan);
            return std::ptr::null_mut();
        }
        let aux_bytes = header_aux_storage_bytes(header_aux_snapshot(header).kind);
        let tracked_bytes = plan.alloc_size.saturating_add(aux_bytes);
        profile_hit(_py, &ALLOC_COUNT);
        profile_hit_bytes(_py, &ALLOC_BYTES_TOTAL, tracked_bytes as u64);
        profile_alloc_aux_kind(_py, header_aux_snapshot(header).kind);
        profile_alloc_type(_py, type_id);
        profile_alloc_type_bytes(_py, type_id, tracked_bytes);
        let data_ptr = ptr.add(std::mem::size_of::<MoltHeader>());
        gc::gc_track_if_cyclic(_py, data_ptr, type_id);
        data_ptr
    }
}

pub(crate) fn alloc_object(_py: &PyToken<'_>, total_size: usize, type_id: u32) -> *mut u8 {
    alloc_object_with_aux(_py, total_size, type_id, ObjectAuxPreselection::Default)
}

pub(crate) fn alloc_object_with_aux(
    _py: &PyToken<'_>,
    total_size: usize,
    type_id: u32,
    aux: ObjectAuxPreselection,
) -> *mut u8 {
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
    unsafe {
        // Zero the entire allocation so data fields past the header
        // start as null pointers / zero values.  This prevents the
        // deallocation path from misinterpreting stale heap data as
        // valid inner pointers (Vec*, DataclassDesc*, etc.) when an
        // object type allocates more space than it initializes.
        std::ptr::write_bytes(header_ptr, 0, plan.alloc_size);
        let header = header_ptr as *mut MoltHeader;
        (*header).type_id = type_id;
        MoltHeader::initialize_refcount_before_publication(header, 1);
        MoltHeader::initialize_flags_before_publication(header, 0);
        // Payload and size_class are already 0 from write_bytes.
        (*header).size_class = plan.size_class;
        if !initialize_header_aux(header, type_id, plan.size_class, total_size, aux) {
            std::alloc::dealloc(header_ptr, plan.layout);
            release_object_allocation_reservation(plan);
            return std::ptr::null_mut();
        }
        let aux_bytes = header_aux_storage_bytes(header_aux_snapshot(header).kind);
        let tracked_bytes = plan.alloc_size.saturating_add(aux_bytes);
        profile_hit(_py, &ALLOC_COUNT);
        profile_hit_bytes(_py, &ALLOC_BYTES_TOTAL, tracked_bytes as u64);
        profile_alloc_aux_kind(_py, header_aux_snapshot(header).kind);
        profile_alloc_type(_py, type_id);
        profile_alloc_type_bytes(_py, type_id, tracked_bytes);
        let data_ptr = header_ptr.add(std::mem::size_of::<MoltHeader>());
        gc::gc_track_if_cyclic(_py, data_ptr, type_id);
        data_ptr
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
    match heap_metrics_policy(type_id) {
        Some(HeapMetricsPolicy::Object) => profile_hit(_py, &ALLOC_OBJECT_COUNT),
        Some(HeapMetricsPolicy::Exception) => profile_hit(_py, &ALLOC_EXCEPTION_COUNT),
        Some(HeapMetricsPolicy::Dict) => profile_hit(_py, &ALLOC_DICT_COUNT),
        Some(HeapMetricsPolicy::Tuple) => profile_hit(_py, &ALLOC_TUPLE_COUNT),
        Some(HeapMetricsPolicy::String) => profile_hit(_py, &ALLOC_STRING_COUNT),
        Some(HeapMetricsPolicy::Callargs) => profile_hit(_py, &ALLOC_CALLARGS_COUNT),
        _ => {}
    }
}

#[cfg_attr(target_arch = "wasm32", inline(always))]
fn profile_alloc_aux_kind(_py: &PyToken<'_>, aux_kind: u16) {
    match aux_kind {
        HEADER_AUX_KIND_CLASS_INLINE => profile_hit(_py, &AUX_CLASS_INLINE_COUNT),
        HEADER_AUX_KIND_STATE_INLINE => profile_hit(_py, &AUX_STATE_INLINE_COUNT),
        _ => {}
    }
}

#[cfg_attr(target_arch = "wasm32", inline(always))]
fn profile_alloc_type_bytes(_py: &PyToken<'_>, type_id: u32, total_size: usize) {
    let bytes = total_size as u64;
    match heap_metrics_policy(type_id) {
        Some(HeapMetricsPolicy::Dict) => profile_hit_bytes(_py, &ALLOC_BYTES_DICT, bytes),
        Some(HeapMetricsPolicy::Exception) => profile_hit_bytes(_py, &ALLOC_BYTES_EXCEPTION, bytes),
        Some(HeapMetricsPolicy::Tuple) => profile_hit_bytes(_py, &ALLOC_BYTES_TUPLE, bytes),
        Some(HeapMetricsPolicy::String) => profile_hit_bytes(_py, &ALLOC_BYTES_STRING, bytes),
        Some(HeapMetricsPolicy::List) => profile_hit_bytes(_py, &ALLOC_BYTES_LIST, bytes),
        _ => {}
    }
}

/// Per-type dealloc counter dispatch (RC drop-insertion substrate, design 20).
/// Mirrors [`profile_alloc_type`]: called from the `dec_ref_ptr` zero-transition
/// so a leak in the `live = alloc - dealloc` gauge can be attributed to a
/// concrete object family.
#[cfg_attr(target_arch = "wasm32", inline(always))]
fn profile_dealloc_type(_py: &PyToken<'_>, type_id: u32, total_size: u64) {
    match heap_metrics_policy(type_id) {
        Some(HeapMetricsPolicy::Object) => profile_hit(_py, &DEALLOC_OBJECT_COUNT),
        Some(HeapMetricsPolicy::Bigint) => profile_hit(_py, &DEALLOC_BIGINT_COUNT),
        Some(HeapMetricsPolicy::String) => profile_hit(_py, &DEALLOC_STRING_COUNT),
        Some(HeapMetricsPolicy::Dict) => profile_hit(_py, &DEALLOC_DICT_COUNT),
        Some(HeapMetricsPolicy::Tuple) => profile_hit(_py, &DEALLOC_TUPLE_COUNT),
        Some(HeapMetricsPolicy::Exception) => {
            profile_hit(_py, &DEALLOC_EXCEPTION_COUNT);
            profile_hit_bytes(_py, &DEALLOC_BYTES_EXCEPTION, total_size);
        }
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

unsafe fn object_class_bits_from_word(word: u64) -> u64 {
    let bits = word & HEADER_CLASS_WORD_BITS_MASK;
    if bits == 0 {
        return 0;
    }
    let Some(class_ptr) = obj_from_bits(bits).as_ptr() else {
        return 0;
    };
    if unsafe { object_type_id(class_ptr) } != TYPE_ID_TYPE {
        return 0;
    }
    bits
}

#[inline]
unsafe fn object_class_word(ptr: *mut u8) -> u64 {
    let snapshot = unsafe { object_aux_snapshot(ptr) };
    match snapshot.kind {
        HEADER_AUX_KIND_CLASS_INLINE => snapshot.word,
        HEADER_AUX_KIND_SIDECAR => unsafe { sidecar_from_snapshot(snapshot) }.class_edge(),
        _ => 0,
    }
}

#[inline]
#[cfg(test)]
pub(crate) unsafe fn object_has_class_edge(ptr: *mut u8) -> bool {
    unsafe { object_class_word(ptr) & HEADER_CLASS_WORD_BITS_MASK != 0 }
}

#[inline]
pub(crate) unsafe fn object_class_edge_is_borrowed(ptr: *mut u8) -> bool {
    let word = unsafe { object_class_word(ptr) };
    word & HEADER_CLASS_WORD_BITS_MASK != 0 && word & HEADER_CLASS_WORD_BORROWED != 0
}

/// Return the object's validated class handle.
///
/// # Safety
/// `ptr` must identify a live object and the caller must hold the GIL. The GIL
/// is currently the lifetime guard that prevents a published replacement from
/// retiring the loaded class edge before its type header is validated.
pub(crate) unsafe fn object_class_bits(ptr: *mut u8) -> u64 {
    crate::gil_assert();
    unsafe { object_class_bits_from_word(object_class_word(ptr)) }
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

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum ClassEdgeOwnership {
    Owned,
    Borrowed,
}

#[inline]
fn class_word(bits: u64, ownership: ClassEdgeOwnership) -> u64 {
    debug_assert_eq!(bits & HEADER_CLASS_WORD_TAG_MASK, 0);
    if bits == 0 {
        0
    } else {
        bits | (u64::from(ownership == ClassEdgeOwnership::Borrowed) * HEADER_CLASS_WORD_BORROWED)
    }
}

#[inline]
unsafe fn validated_class_edge_bits(bits: u64) -> Option<u64> {
    if bits == 0 || obj_from_bits(bits).is_none() {
        return Some(0);
    }
    let class_ptr = obj_from_bits(bits).as_ptr()?;
    (unsafe { object_type_id(class_ptr) } == TYPE_ID_TYPE).then_some(bits)
}

#[inline]
unsafe fn class_edge_target(
    ptr: *mut u8,
    snapshot: ObjectAuxSnapshot,
) -> Option<&'static MoltAuxWord> {
    unsafe {
        match snapshot.kind {
            HEADER_AUX_KIND_CLASS_INLINE => Some(&(*header_from_obj_ptr(ptr)).aux),
            HEADER_AUX_KIND_SIDECAR => Some(&sidecar_from_snapshot(snapshot).class_edge),
            HEADER_AUX_KIND_NONE | HEADER_AUX_KIND_STATE_INLINE => None,
            _ => unreachable!("validated aux kind"),
        }
    }
}

unsafe fn replace_published_class_edge(
    _py: &PyToken<'_>,
    ptr: *mut u8,
    bits: u64,
    ownership: ClassEdgeOwnership,
) -> bool {
    crate::gil_assert();
    if (unsafe { (*header_from_obj_ptr(ptr)).load_synchronized_flags() } & HEADER_FLAG_DEALLOCATING)
        != 0
    {
        return false;
    }
    let Some(new_bits) = (unsafe { validated_class_edge_bits(bits) }) else {
        return false;
    };
    let snapshot = unsafe { object_aux_snapshot(ptr) };
    let Some(target) = (unsafe { class_edge_target(ptr, snapshot) }) else {
        return false;
    };

    let new_owned = new_bits != 0 && ownership == ClassEdgeOwnership::Owned;
    let new_word = class_word(new_bits, ownership);
    let mut old_word = target.load(AtomicOrdering::Acquire);
    loop {
        if old_word == new_word {
            return true;
        }
        if new_owned {
            inc_ref_bits(_py, new_bits);
        }
        match target.compare_exchange(
            old_word,
            new_word,
            AtomicOrdering::AcqRel,
            AtomicOrdering::Acquire,
        ) {
            Ok(_) => {
                let old_bits = unsafe { object_class_bits_from_word(old_word) };
                let old_owned = old_bits != 0 && old_word & HEADER_CLASS_WORD_BORROWED == 0;
                if old_owned {
                    dec_ref_bits(_py, old_bits);
                }
                return true;
            }
            Err(observed) => {
                if new_owned {
                    dec_ref_bits(_py, new_bits);
                }
                old_word = observed;
            }
        }
    }
}

/// Establish a class edge in an already-selected CLASS_INLINE or SIDECAR lane
/// while the object is still unpublished. Constructors must select the lane at
/// allocation and use this, not the published replacement API.
///
/// # Safety
/// `ptr` must not yet be visible to any reader or external registry.
#[must_use]
pub(crate) unsafe fn object_init_class_edge_unpublished(
    _py: &PyToken<'_>,
    ptr: *mut u8,
    bits: u64,
    ownership: ClassEdgeOwnership,
) -> bool {
    crate::gil_assert();
    let Some(new_bits) = (unsafe { validated_class_edge_bits(bits) }) else {
        return false;
    };
    let snapshot = unsafe { object_aux_snapshot(ptr) };
    let Some(target) = (unsafe { class_edge_target(ptr, snapshot) }) else {
        return false;
    };
    if target.load(AtomicOrdering::Acquire) != 0 {
        return false;
    }
    if new_bits != 0 && ownership == ClassEdgeOwnership::Owned {
        inc_ref_bits(_py, new_bits);
    }
    // Fresh-object initialization has exclusive ownership. A single release
    // publication is sufficient and avoids the redundant load/CAS loop paid by
    // the genuinely concurrent published replacement path.
    target.store(class_word(new_bits, ownership), AtomicOrdering::Release);
    true
}

/// Clear a class edge during exclusive unpublished rollback and discharge its
/// owned reference exactly once. This is deliberately separate from fresh
/// initialization so the latter can retain its empty-lane invariant.
///
/// # Safety
/// `ptr` must not be visible to any reader or external registry.
#[must_use]
pub(crate) unsafe fn object_clear_class_edge_unpublished(_py: &PyToken<'_>, ptr: *mut u8) -> bool {
    crate::gil_assert();
    let snapshot = unsafe { object_aux_snapshot(ptr) };
    let Some(target) = (unsafe { class_edge_target(ptr, snapshot) }) else {
        return false;
    };
    let old_word = target.swap(0, AtomicOrdering::AcqRel);
    let old_bits = unsafe { object_class_bits_from_word(old_word) };
    if old_bits != 0 && old_word & HEADER_CLASS_WORD_BORROWED == 0 {
        dec_ref_bits(_py, old_bits);
    }
    true
}

/// Atomically publish the common class lane empty and transfer its owned edge
/// to terminal lifecycle custody. Borrowed class identities are simply
/// cleared. The caller releases the returned edge only after every other
/// inline, backing, ABI, and side-registry source is empty.
pub(crate) unsafe fn object_detach_class_edge(ptr: *mut u8) -> u64 {
    let snapshot = unsafe { object_aux_snapshot(ptr) };
    let Some(target) = (unsafe { class_edge_target(ptr, snapshot) }) else {
        return 0;
    };
    let old_word = target.swap(0, AtomicOrdering::AcqRel);
    if old_word & HEADER_CLASS_WORD_BORROWED != 0 {
        0
    } else {
        unsafe { object_class_bits_from_word(old_word) }
    }
}

/// Replace an existing published class lane without changing aux kind/address.
pub(crate) unsafe fn object_replace_class_edge(
    _py: &PyToken<'_>,
    ptr: *mut u8,
    bits: u64,
    ownership: ClassEdgeOwnership,
) -> bool {
    unsafe { replace_published_class_edge(_py, ptr, bits, ownership) }
}

#[inline]
unsafe fn class_header_has_finalizer(class_ptr: *mut u8) -> bool {
    unsafe {
        object_type_id(class_ptr) == TYPE_ID_TYPE
            && ((*header_from_obj_ptr(class_ptr)).load_metadata_flags()
                & HEADER_FLAG_CLASS_HAS_FINALIZER)
                != 0
    }
}

pub(crate) unsafe fn object_class_has_finalizer(ptr: *mut u8) -> bool {
    unsafe {
        object_type_id(ptr) != TYPE_ID_TYPE
            && obj_from_bits(object_class_bits(ptr))
                .as_ptr()
                .is_some_and(|class_ptr| class_header_has_finalizer(class_ptr))
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
            let mro = crate::object::seq_access::pin_tuple(_py, mro_ptr)?;
            for class_bits in mro.iter().copied() {
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
            (*header).fetch_or_flags(HEADER_FLAG_CLASS_HAS_FINALIZER);
        } else {
            (*header).fetch_and_flags(!HEADER_FLAG_CLASS_HAS_FINALIZER);
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
        let fields_name = crate::intern_static_name(
            _py,
            &runtime_state(_py).interned.field_offsets_name,
            b"__molt_field_offsets__",
        );
        if let Some(dict_ptr) = obj_from_bits(crate::class_dict_bits(class_ptr)).as_ptr()
            && let Some(offsets_bits) = crate::dict_get_in_place(_py, dict_ptr, fields_name)
            && let Some(offsets_ptr) = obj_from_bits(offsets_bits).as_ptr()
            && object_type_id(offsets_ptr) == TYPE_ID_DICT
        {
            (*header_from_obj_ptr(offsets_ptr)).fetch_or_flags(HEADER_FLAG_FROZEN_LAYOUT_MAP);
        }
        class_add_policy(class_ptr, CLASS_POLICY_DEFINITION_FINISHED);
        // Publish the validated immutable size only after both metadata inputs
        // have been sealed. Competing future free-threaded readers may publish
        // the same value; a mismatch is an invariant failure.
        let _ = crate::call::class_init::class_layout_size_cached(_py, class_ptr);
    }
}

pub(crate) unsafe fn object_mark_has_ptrs(_py: &PyToken<'_>, ptr: *mut u8) {
    unsafe {
        crate::gil_assert();
        (*header_from_obj_ptr(ptr)).fetch_or_flags(HEADER_FLAG_HAS_PTRS);
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

pub(crate) unsafe fn memoryview_base_bits(ptr: *mut u8) -> u64 {
    unsafe { (*memoryview_ptr(ptr)).base_bits }
}

pub(crate) unsafe fn memoryview_data(ptr: *mut u8) -> *mut u8 {
    unsafe { (*memoryview_ptr(ptr)).data }
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

pub(crate) unsafe fn memoryview_released(ptr: *mut u8) -> bool {
    unsafe { (*memoryview_ptr(ptr)).released != 0 }
}

pub(crate) unsafe fn memoryview_mark_released(ptr: *mut u8) {
    unsafe {
        (*memoryview_ptr(ptr)).released = 1;
    }
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
        && let Some(ptr) = resolve_opaque_ptr(addr as u64)
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

#[inline(always)]
pub(crate) unsafe fn inc_ref_ptr(_py: &PyToken<'_>, ptr: *mut u8) {
    unsafe {
        crate::gil_assert();
        if ptr.is_null() {
            return;
        }
        let header_ptr = ptr.sub(std::mem::size_of::<MoltHeader>()) as *mut MoltHeader;
        let type_id = (*header_ptr).type_id;
        // Type-id validity is a MEMORY-SAFETY boundary, not a debug concern: a
        // header whose `type_id` is out of the valid heap range means `ptr` does
        // not point at a live molt object (use-after-free, wild pointer, or
        // corrupted header). Touching its `flags`/`ref_count` below — and later
        // routing it through the dealloc switch keyed on `type_id` — would be UB.
        // Fail closed in ALL profiles via the single canonical validator
        // (`is_valid_heap_type_id`), never a stripped `debug_assert!` with a
        // duplicate looser range that drifts from the authority.
        if !is_valid_heap_type_id(type_id) {
            eprintln!(
                "molt fatal: invalid object header in inc_ref ptr=0x{:x} type_id={} \
                 (use-after-free or corrupted header)",
                ptr as usize, type_id
            );
            std::process::abort();
        }
        if (*header_ptr).has_flag(HEADER_FLAG_IMMORTAL) {
            return;
        }
        if (*header_ptr).has_flag(HEADER_FLAG_DEALLOCATING) {
            eprintln!("molt fatal: owned INCREF attempted after terminal death");
            std::process::abort();
        }
        // Debug: trace bigint refcount increments
        if type_id == TYPE_ID_BIGINT && debug_bigint_rc() {
            let old = (*header_ptr).owned_ref_count_snapshot();
            eprintln!(
                "BIGINT_RC_INC ptr=0x{:x} count={} → {}",
                ptr as usize,
                old,
                old + 1
            );
        }
        if type_id == TYPE_ID_EXCEPTION && trace_exception_rc() {
            let old = (*header_ptr).owned_ref_count_snapshot();
            eprintln!("EXC_RC_INC ptr=0x{:x} {}→{}", ptr as usize, old, old + 1);
        }
        let previous = (*header_ptr).retain_owned(1, "inc_ref_ptr");
        let new_count = previous + 1;
        if previous == 1 && (*header_ptr).has_flag(HEADER_FLAG_HAS_ABI_VIEW) {
            let bits = MoltObject::from_ptr(ptr).bits();
            molt_cpython_abi::bridge::GLOBAL_BRIDGE.runtime_owner_added_from_view_hold(bits);
        }
        if debug_rc_object() {
            let header = &*header_ptr;
            if header.type_id == TYPE_ID_OBJECT && object_class_edge_is_borrowed(ptr) {
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

/// Batched increment: apply one checked typed transition for `count` retained
/// references instead of repeating the single-reference validation path.
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
        // Type-id validity is a MEMORY-SAFETY boundary (mirror `inc_ref_ptr`):
        // a corrupted/freed header must not have its `flags`/`ref_count` touched.
        let type_id = (*header_ptr).type_id;
        if !is_valid_heap_type_id(type_id) {
            eprintln!(
                "molt fatal: invalid object header in inc_ref_n ptr=0x{:x} type_id={} \
                 (use-after-free or corrupted header)",
                ptr as usize, type_id
            );
            std::process::abort();
        }
        if (*header_ptr).has_flag(HEADER_FLAG_IMMORTAL) {
            return;
        }
        if (*header_ptr).has_flag(HEADER_FLAG_DEALLOCATING) {
            eprintln!("molt fatal: owned batched INCREF attempted after terminal death");
            std::process::abort();
        }
        let previous = (*header_ptr).retain_owned(count as usize, "inc_ref_n_ptr");
        let new_count = previous + count;
        if previous == 1 && (*header_ptr).has_flag(HEADER_FLAG_HAS_ABI_VIEW) {
            let bits = MoltObject::from_ptr(ptr).bits();
            molt_cpython_abi::bridge::GLOBAL_BRIDGE.runtime_owner_added_from_view_hold(bits);
        }
        if debug_rc_object() {
            let header = &*header_ptr;
            if header.type_id == TYPE_ID_OBJECT && object_class_edge_is_borrowed(ptr) {
                eprintln!(
                    "molt rc inc_n ptr=0x{:x} count={} by={}",
                    ptr as usize, new_count, count
                );
            }
        }
    }
}

#[cfg(test)]
thread_local! {
    static FINALIZER_WINDOW_TEST_HOOK: std::cell::Cell<Option<fn(u64)>> = const {
        std::cell::Cell::new(None)
    };
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
    if !unsafe { object_class_has_finalizer(ptr) } {
        return;
    }
    if (unsafe { (*header_ptr).load_synchronized_flags() } & HEADER_FLAG_FINALIZER_RAN) != 0 {
        return;
    }
    #[cfg(test)]
    FINALIZER_WINDOW_TEST_HOOK.with(|slot| {
        if let Some(hook) = slot.replace(None) {
            hook(MoltObject::from_ptr(ptr).bits());
        }
    });
    let class_bits = unsafe { object_class_bits(ptr) };
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
    // The finalizer may replace/delete its own class attribute. Keep the raw
    // callable alive through post-failure policy repr/context reporting.
    inc_ref_bits(py, raw_del_bits);
    unsafe {
        (*header_ptr).fetch_or_flags(HEADER_FLAG_FINALIZER_RAN);
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
    crate::builtins::exceptions::run_unraisable_with_policy(
        py,
        || {
            if crate::object::ops_sys::runtime_target_minor(py) >= 14 {
                let rendered =
                    crate::builtins::exceptions::unraisable_context_repr(py, raw_del_bits);
                (
                    MoltObject::none().bits(),
                    Some(format!(
                        "Exception ignored while calling deallocator {rendered}"
                    )),
                )
            } else {
                (raw_del_bits, None)
            }
        },
        || {
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
        },
    );
    dec_ref_bits(py, raw_del_bits);
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
    // The revival ref opened by the caller stays held: `dec_ref_ptr` performs the
    // single closing `dec_ref` + resurrection check AFTER `weakref_clear_for_ptr`
    // runs, so a `__del__`-resurrect and a weakref-callback-resurrect collapse to
    // the SAME post-window check (CPython's finalize+ClearWeakRefs window).
}

/// Cyclic-collector finalizer: run `__del__` once without the acyclic rc=0
/// resurrection verdict. Cyclic resurrection is detected by the collector's
/// second reachability partition after finalizers run.
///
/// # Safety
/// `ptr` is a live unreachable object and the GIL is held.
pub(crate) unsafe fn maybe_run_object_finalizer_for_cycle(py: &PyToken<'_>, ptr: *mut u8) {
    unsafe { run_object_del_in_revival_window(py, ptr) };
}

unsafe fn drop_detached_tracked_vec<T>(vec_ptr: *mut Vec<T>) {
    if !vec_ptr.is_null() {
        drop(unsafe { backing::tracked_vec_box_from_raw(vec_ptr) });
    }
}

unsafe fn drop_detached_linear_builder_vec(ptr: *mut u8) {
    unsafe {
        let slot = ptr as *mut *mut Vec<u64>;
        let vec_ptr = slot.replace(std::ptr::null_mut());
        if vec_ptr.is_null() {
            return;
        }
        drop(backing::tracked_vec_box_from_raw(vec_ptr));
    }
}

#[inline]
fn terminal_resource_drop_no_unwind(drop_resources: impl FnOnce()) {
    #[cfg(panic = "unwind")]
    if std::panic::catch_unwind(std::panic::AssertUnwindSafe(drop_resources)).is_err() {
        std::process::abort();
    }
    #[cfg(not(panic = "unwind"))]
    drop_resources();
}

#[inline]
fn record_terminal_deallocation(py: &PyToken<'_>, type_id: u32, bytes: u64) {
    profile_hit(py, &DEALLOC_COUNT);
    profile_hit_bytes(py, &DEALLOC_BYTES_TOTAL, bytes);
    profile_dealloc_type(py, type_id, bytes);
}

/// Typed projection of the payload edges layered under OBJECT/WEAKREF.
/// Common class, inline-field, and instance-dict edges are owned by the heap
/// lifecycle dispatcher; this function enumerates only the selected subshape.
pub(crate) unsafe fn object_shape_visit_owned_edges(
    py: &PyToken<'_>,
    ptr: *mut u8,
    mut visit: impl FnMut(u64),
) {
    let shape = object_shape_id(ptr);
    if object_shape_is_task(shape) {
        let slots = unsafe { object_payload_size(ptr) } / std::mem::size_of::<u64>();
        let first_python_slot = usize::from(!matches!(
            object_shape_resource_slot(shape),
            ObjectShapeResourceSlot::None
        ));
        for index in first_python_slot..slots {
            visit(unsafe { *(ptr as *const u64).add(index) });
        }
        crate::async_rt::scheduler::task_visit_owned_edges(py, ptr, &mut visit);
        return;
    }
    unsafe {
        match object_shape_lifecycle_family(shape) {
            ObjectShapeLifecycleFamily::Plain => {}
            ObjectShapeLifecycleFamily::DictSubclass => {
                if let Some(&bits) = runtime_state(py)
                    .dict_subclass_storage
                    .lock()
                    .unwrap()
                    .get(&PtrSlot(ptr))
                {
                    visit(bits);
                }
                let payload = object_payload_size(ptr);
                if payload >= 2 * std::mem::size_of::<u64>() {
                    visit(*(ptr.add(payload - 2 * std::mem::size_of::<u64>()) as *const u64));
                }
            }
            ObjectShapeLifecycleFamily::Operator => {
                crate::builtins::operator::operator_visit_owned_edges(shape, ptr, visit);
            }
            ObjectShapeLifecycleFamily::Functools => {
                crate::builtins::functools::functools_visit_owned_edges(shape, ptr, visit);
            }
            ObjectShapeLifecycleFamily::Types => {
                crate::builtins::types::types_visit_owned_edges(shape, ptr, visit);
            }
            ObjectShapeLifecycleFamily::Itertools => {
                #[cfg(feature = "stdlib_itertools")]
                molt_runtime_itertools::itertools::itertools_visit_owned_edges(
                    shape as u16,
                    ptr,
                    visit,
                );
                #[cfg(not(feature = "stdlib_itertools"))]
                crate::builtins::itertools::itertools_visit_owned_edges(shape as u16, ptr, visit);
            }
            ObjectShapeLifecycleFamily::Task => {
                unreachable!("task family bypassed task projection")
            }
        }
    }
}

/// Idempotently detach and release every mutable edge owned by an OBJECT
/// subshape. State is published empty before any DECREF can re-enter it.
pub(crate) unsafe fn object_shape_clear_cycle_edges(
    py: &PyToken<'_>,
    ptr: *mut u8,
    detached_sink: &mut heap_lifecycle::DetachedEdgeSink,
) {
    let shape = object_shape_id(ptr);
    if object_shape_is_task(shape) {
        let slots = unsafe { object_payload_size(ptr) } / std::mem::size_of::<u64>();
        let none = MoltObject::none().bits();
        let resource_slot = object_shape_resource_slot(shape);
        let detached_resource = match resource_slot {
            ObjectShapeResourceSlot::IoSocket => io_wait_detach_resource(ptr),
            ObjectShapeResourceSlot::Websocket => ws_wait_detach_resource(ptr),
            ObjectShapeResourceSlot::None => none,
        };
        let first_python_slot = usize::from(resource_slot != ObjectShapeResourceSlot::None);
        for index in first_python_slot..slots {
            let bits = unsafe { (ptr as *mut u64).add(index).replace(none) };
            detached_sink.detach_if_heap(bits);
        }
        crate::async_rt::scheduler::task_detach_owned_edges(py, ptr, detached_sink);
        match resource_slot {
            ObjectShapeResourceSlot::IoSocket => detached_sink.detach_resource(
                heap_lifecycle::DetachedResource::IoSocket(detached_resource),
            ),
            ObjectShapeResourceSlot::Websocket => detached_sink.detach_resource(
                heap_lifecycle::DetachedResource::Websocket(detached_resource),
            ),
            ObjectShapeResourceSlot::None => {}
        }
        return;
    }
    unsafe {
        match object_shape_lifecycle_family(shape) {
            ObjectShapeLifecycleFamily::Plain => {}
            ObjectShapeLifecycleFamily::DictSubclass => {
                let side = runtime_state(py)
                    .dict_subclass_storage
                    .lock()
                    .unwrap()
                    .remove(&PtrSlot(ptr));
                let payload = object_payload_size(ptr);
                let tail = (payload >= 2 * std::mem::size_of::<u64>()).then(|| {
                    (ptr.add(payload - 2 * std::mem::size_of::<u64>()) as *mut u64)
                        .replace(MoltObject::none().bits())
                });
                if let Some(bits) = side {
                    detached_sink.detach_if_heap(bits);
                }
                if let Some(bits) = tail {
                    detached_sink.detach_if_heap(bits);
                }
            }
            ObjectShapeLifecycleFamily::Operator => {
                crate::builtins::operator::operator_detach_owned_edges(shape, ptr, |bits| {
                    detached_sink.detach_if_heap(bits)
                });
            }
            ObjectShapeLifecycleFamily::Functools => {
                crate::builtins::functools::functools_detach_owned_edges(shape, ptr, |bits| {
                    detached_sink.detach_if_heap(bits)
                });
            }
            ObjectShapeLifecycleFamily::Types => {
                crate::builtins::types::types_detach_owned_edges(shape, ptr, |bits| {
                    detached_sink.detach_if_heap(bits)
                });
            }
            ObjectShapeLifecycleFamily::Itertools => {
                #[cfg(feature = "stdlib_itertools")]
                molt_runtime_itertools::itertools::itertools_detach_owned_edges(
                    shape as u16,
                    ptr,
                    |bits| detached_sink.detach_if_heap(bits),
                );
                #[cfg(not(feature = "stdlib_itertools"))]
                crate::builtins::itertools::itertools_detach_owned_edges(
                    shape as u16,
                    ptr,
                    |bits| detached_sink.detach_if_heap(bits),
                );
            }
            ObjectShapeLifecycleFamily::Task => unreachable!("task family bypassed task clear"),
        }
        match object_shape_lifecycle_family(shape) {
            ObjectShapeLifecycleFamily::Functools => {
                detached_sink.detach_resource(heap_lifecycle::DetachedResource::Functools(
                    crate::builtins::functools::functools_detach_typed_resources(shape, ptr),
                ))
            }
            ObjectShapeLifecycleFamily::Itertools => {
                #[cfg(feature = "stdlib_itertools")]
                detached_sink.detach_resource(heap_lifecycle::DetachedResource::Itertools(
                    molt_runtime_itertools::itertools::itertools_detach_typed_resources(
                        shape as u16,
                        ptr,
                    ),
                ));
                #[cfg(not(feature = "stdlib_itertools"))]
                detached_sink.detach_resource(heap_lifecycle::DetachedResource::Itertools(
                    crate::builtins::itertools::itertools_detach_typed_resources(shape as u16, ptr),
                ));
            }
            _ => {}
        }
    }
}

/// # Safety
/// Dereferences raw pointer to decrement ref count. Frees memory if count reaches 0.
#[inline(always)]
pub(crate) unsafe fn dec_ref_ptr(py: &PyToken<'_>, ptr: *mut u8) {
    unsafe { dec_ref_ptr_with_validated_type_id(py, ptr, None) }
}

/// Terminal release entry for a caller that already validated the header while
/// holding the same GIL token. This preserves rich bits/frame diagnostics at
/// the object ABI without paying a second header load and range branch on every
/// generated `DecRef`.
#[inline(always)]
pub(crate) unsafe fn dec_ref_ptr_validated(py: &PyToken<'_>, ptr: *mut u8, type_id: u32) {
    debug_assert!(is_valid_heap_type_id(type_id));
    unsafe { dec_ref_ptr_with_validated_type_id(py, ptr, Some(type_id)) }
}

#[inline(always)]
unsafe fn dec_ref_ptr_with_validated_type_id(
    py: &PyToken<'_>,
    ptr: *mut u8,
    validated_type_id: Option<u32>,
) {
    unsafe {
        crate::gil_assert();
        if ptr.is_null() {
            return;
        }
        let header_ptr = ptr.sub(std::mem::size_of::<MoltHeader>()) as *mut MoltHeader;
        let type_id = validated_type_id.unwrap_or_else(|| (*header_ptr).type_id);
        // Type-id validity is a MEMORY-SAFETY boundary (see `inc_ref_ptr`). This
        // `type_id` drives the dealloc switch below (reading type-specific inner
        // pointers and freeing the backing memory), so an out-of-range value —
        // use-after-free, wild pointer, or corrupted header — would corrupt
        // unrelated memory. Fail closed in ALL profiles via the single canonical
        // validator before reading any other header field. `molt_dec_ref_obj`
        // already guards the bits-based entry; this closes the ptr-based hot path
        // it short-circuits past, so both DecRef entry points share one authority.
        if validated_type_id.is_none() && !is_valid_heap_type_id(type_id) {
            eprintln!(
                "molt fatal: invalid object header in dec_ref ptr=0x{:x} type_id={} \
                 (use-after-free or corrupted header)",
                ptr as usize, type_id
            );
            if std::env::var("MOLT_TRACE_INVALID_DECREF").as_deref() == Ok("1") {
                eprintln!(
                    "molt invalid dec_ref backtrace:\n{}",
                    std::backtrace::Backtrace::force_capture()
                );
            }
            std::process::abort();
        }
        let header_flags = (*header_ptr).load_synchronized_flags();
        let header_size_class = (*header_ptr).size_class;
        let header_aux = header_aux_snapshot(header_ptr);
        if (header_flags & HEADER_FLAG_IMMORTAL) != 0 {
            return;
        }
        // A zero refcount at dec_ref entry is an ownership invariant violation.
        // Do not make dec_ref idempotent: a stale post-free pointer may already
        // alias allocator metadata or a different object, so continuing would
        // corrupt unrelated runtime state.
        let release = (*header_ptr).release_owned("dec_ref_ptr");
        let prev = release.previous();
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
        if debug_rc_object() && type_id == TYPE_ID_OBJECT && object_class_edge_is_borrowed(ptr) {
            eprintln!(
                "molt rc dec ptr=0x{:x} count={}",
                ptr as usize,
                prev.saturating_sub(1)
            );
        }
        let view_hold_is_final = if prev == 2 && (header_flags & HEADER_FLAG_HAS_ABI_VIEW) != 0 {
            let bits = MoltObject::from_ptr(ptr).bits();
            match molt_cpython_abi::bridge::GLOBAL_BRIDGE.runtime_owner_dropped_to_view_hold(bits) {
                Some(true) => {
                    // Keep the stable view hold. A distinct revival pin is
                    // added only if arbitrary finalizer/weakref code will run.
                    true
                }
                Some(false) => return,
                None => false,
            }
        } else {
            false
        };
        if prev == 1 && (header_flags & HEADER_FLAG_HAS_ABI_VIEW) != 0 {
            let may_finalize = molt_cpython_abi::bridge::GLOBAL_BRIDGE
                .runtime_last_ref_dropped(MoltObject::from_ptr(ptr).bits());
            // Restore the stable view hold that this terminal decrement is
            // attempting to consume. Finalization owns a distinct pin above
            // it; direct C roots retain the restored hold without entering.
            (*header_ptr).restore_stable_view_hold();
            if !may_finalize {
                return;
            }
        }
        if release.reached_zero() || view_hold_is_final {
            if type_id == TYPE_ID_EXCEPTION && trace_exception_rc() {
                eprintln!("EXC_RC_FREE ptr=0x{:x} (rc hit 0, freeing)", ptr as usize);
            }
            // RC drop-insertion substrate (design 20): the rc=1→0 transition,
            // past the immortal and ABI-view early returns above. This is NOT yet a
            // confirmed deallocation: the finalize + weakref-clear revival window
            // below may run a `__del__` OR a weakref callback that RESURRECTS the
            // object (re-incrementing its refcount), in which case `dec_ref_ptr`
            // returns WITHOUT freeing. Counting the dealloc here would over-count
            // destructions and make `live = alloc - dealloc` UNDER-count live
            // objects — an unsound leak gauge under resurrection (phantom "no
            // leak"). So the dealloc counters are bumped only AFTER the revival
            // window's single resurrection check passes (see below); the byte
            // total is the one value that must be read from the header BEFORE the
            // window runs. Aux kind/address are immutable after publication and
            // oversized exact size is immutable inside the sidecar.
            let object_bytes =
                total_size_from_header_fields(header_size_class, header_aux.kind, header_aux.word);
            let dealloc_bytes =
                object_bytes.saturating_add(header_aux_storage_bytes(header_aux.kind)) as u64;
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
                    let vec_ptr = crate::object::seq_access::backing_identity(ptr);
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
            // currently derives a `__del__` finalizer from its class or has ever
            // exposed a weakref (`HAS_WEAKREF`). Objects with neither (the hot
            // path: ints, strings, tuples, plain instances) skip the revival
            // inc/dec AND the global weakref lock entirely and fall straight
            // through to the free tail with zero added cost.
            let needs_revival_window =
                (header_flags & HEADER_FLAG_HAS_WEAKREF) != 0 || object_class_has_finalizer(ptr);
            if needs_revival_window {
                let mut has_abi_view = (header_flags & HEADER_FLAG_HAS_ABI_VIEW) != 0;
                let view_bits = MoltObject::from_ptr(ptr).bits();
                if has_abi_view {
                    molt_cpython_abi::bridge::GLOBAL_BRIDGE.begin_finalization(view_bits);
                }
                // Open the revival window: the object is now live at rc≥1 so no
                // Python code below can observe (or free) it at rc=0. Use the raw
                // header increment (not `inc_ref_ptr`, which short-circuits on
                // IMMORTAL — already excluded above — and carries debug tracing);
                // the matching closes below are the authoritative resurrection
                // checks.
                let mut revival_window = (*header_ptr).open_revival_window(has_abi_view);
                let mut window_baseline = revival_window.baseline();
                // `__del__` runs INLINE at this rc→0 point, exactly as CPython
                // finalizes at Py_DECREF→0 (prompt timing: `del x; print()` runs
                // `__del__` before `print`), under a synthetic exception-handler
                // frame so an uncaught raise inside it is swallowed (written
                // unraisable) rather than killing the process — see
                // `run_object_del_in_revival_window` for the #65 root cause and the
                // run-once (`FINALIZER_RAN`) semantics. Non-finalizer objects that
                // only reach here for the weakref clear return immediately from it.
                run_object_del_in_revival_window(py, ptr);
                // Arbitrary finalizer code may be the first path to publish a
                // canonical C view. That publication adds the stable runtime
                // view hold and starts in RuntimeOwned bridge state. Reconcile
                // it into this already-open finalization transaction before
                // comparing against the revival baseline, otherwise the view
                // hold itself is mistaken for Python resurrection and leaks the
                // object in RuntimeOwned state.
                if !has_abi_view && (*header_ptr).has_flag(HEADER_FLAG_HAS_ABI_VIEW) {
                    molt_cpython_abi::bridge::GLOBAL_BRIDGE
                        .begin_finalization_for_new_view(view_bits);
                    has_abi_view = true;
                    window_baseline = revival_window.record_stable_view_hold().unwrap_or_else(
                        |baseline| {
                            eprintln!(
                                "molt fatal: invalid finalizer ABI-view baseline promotion ({baseline})"
                            );
                            std::process::abort();
                        },
                    );
                }
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
                if (*header_ptr).ref_count_snapshot() > window_baseline {
                    (*header_ptr).close_revival_window(revival_window);
                    if has_abi_view {
                        molt_cpython_abi::bridge::GLOBAL_BRIDGE
                            .finish_finalization(view_bits, true);
                    }
                    return;
                }
                if has_abi_view
                    && molt_cpython_abi::bridge::GLOBAL_BRIDGE.has_direct_c_refs(view_bits)
                {
                    (*header_ptr).close_revival_window(revival_window);
                    molt_cpython_abi::bridge::GLOBAL_BRIDGE.finish_finalization(view_bits, false);
                    return;
                }
                // `__del__` did not resurrect: death is now committed. Publish
                // DEALLOCATING before weakref clearing so callbacks cannot create
                // fresh weakrefs or synthesize a runtime owner from stale bits.
                (*header_ptr).fetch_or_flags(HEADER_FLAG_DEALLOCATING);
                gc::gc_untrack(py, ptr, type_id, gc::GcUntrackReason::Deallocation);
                // Detach weakrefs and invoke callbacks after the death verdict.
                // The referent remains allocated only as an internal pin; checked
                // retain/view publication rejects every attempt to reopen it.
                weakref_clear_for_ptr(py, ptr);
                if has_abi_view {
                    if (*header_ptr).ref_count_snapshot() != window_baseline
                        || molt_cpython_abi::bridge::GLOBAL_BRIDGE.has_direct_c_refs(view_bits)
                    {
                        eprintln!("molt fatal: weakref callback reopened committed-dead object");
                        std::process::abort();
                    }
                    (*header_ptr).close_revival_window(revival_window);
                    molt_cpython_abi::bridge::GLOBAL_BRIDGE.finish_finalization(view_bits, false);
                } else {
                    let prev_window = (*header_ptr).close_revival_window(revival_window);
                    if prev_window != 1 {
                        eprintln!("molt fatal: weakref callback reopened committed-dead object");
                        std::process::abort();
                    }
                }
                // DEFENSE-IN-DEPTH (P2): the revival window above ran arbitrary
                // Python (`__del__` and/or weakref callbacks) against this object's
                // LIVE storage. The dealloc switch below is keyed on `type_id`,
                // which was cached at function entry BEFORE that window; it selects
                // type-specific inner-pointer offsets and the backing-memory free.
                // In molt's subset nothing can legitimately retag a live object's
                // `type_id` (no ctypes header writes, no `__class__` reassignment
                // that changes the runtime type tag), so this MUST still hold. If a
                // finalizer/callback corrupted the header, re-reading proves it here
                // and aborts BEFORE we free with a mismatched layout (a silent
                // wrong-layout free is memory corruption, not a recoverable error).
                let type_id_after_window = (*header_ptr).type_id;
                if type_id_after_window != type_id {
                    eprintln!(
                        "molt fatal: object type_id changed across finalize/weakref \
                         window ptr=0x{:x} before={} after={} (header corrupted by a \
                         finalizer or weakref callback)",
                        ptr as usize, type_id, type_id_after_window
                    );
                    std::process::abort();
                }
            }
            // Arbitrary finalizer/weakref code may have published side state
            // after the entry snapshot. Reload only after the resurrection
            // verdict, then close every terminal sidecar from this authority.
            let terminal_flags = (*header_ptr).load_synchronized_flags();
            if (terminal_flags & HEADER_FLAG_HAS_ABI_VIEW) != 0 {
                // Consume the stable view hold only after every resurrection
                // opportunity has closed.
                (*header_ptr).retire_stable_view_hold();
            }
            // Past the resurrection check: the object is now actually being
            // destroyed. Commit the leak-gauge counters so DEALLOC_COUNT means
            // "objects truly freed", keeping `live = alloc - dealloc` exact
            // (resurrected objects are correctly NOT counted as dealloc'd until
            // their real final drop). `type_id` is the cached entry value; the
            // byte total was snapshotted before the window ran.
            if (terminal_flags & HEADER_FLAG_DEALLOCATING) == 0 {
                (*header_ptr).fetch_or_flags(HEADER_FLAG_DEALLOCATING);
                gc::gc_untrack(py, ptr, type_id, gc::GcUntrackReason::Deallocation);
            }
            // Remove tuple identity before child retirement, but keep its
            // packed projection and projection-owned C references alive until
            // every inline runtime item edge has been released in the tuple
            // type arm. This is the tuple analogue of exception field detach:
            // no reentrant lookup sees a terminal tuple and no child view loses
            // both ownership domains out of order.
            if type_id == TYPE_ID_ASYNC_GENERATOR {
                // Async-generator finalization is the last callback-bearing
                // phase. Count only after it returns because it may mutate the
                // generator's owned slots.
                asyncgen_call_finalizer(py, ptr);
            }
            // Remove every canonical bridge identity before detaching any
            // runtime source. The returned guard owns all projection C edges
            // and is released only after the complete source-empty barrier.
            let retired_runtime_view = if (terminal_flags & HEADER_FLAG_HAS_ABI_VIEW) != 0 {
                Some(
                    molt_cpython_abi::bridge::GLOBAL_BRIDGE
                        .retire_runtime_object_deferred(MoltObject::from_ptr(ptr).bits())
                        .unwrap_or_else(|| std::process::abort()),
                )
            } else {
                None
            };
            let (terminal_edge_count, terminal_resource_count) =
                heap_lifecycle::terminal_detach_capacity(py, ptr);
            let terminal_resource_count = terminal_resource_count
                .checked_add(usize::from(
                    (terminal_flags & HEADER_FLAG_HAS_ABI_VIEW) != 0,
                ))
                .unwrap_or_else(|| std::process::abort());
            let mut terminal_edges = heap_lifecycle::DetachedEdgeSink::terminal_with_capacities(
                terminal_edge_count,
                terminal_resource_count,
            );
            heap_lifecycle::detach_terminal_owned_edges(py, ptr, &mut terminal_edges);
            if type_id == TYPE_ID_ASYNC_GENERATOR {
                asyncgen_registry_remove(py, ptr);
            }
            if let Some(view) = retired_runtime_view {
                terminal_edges.detach_resource(heap_lifecycle::DetachedResource::RuntimeView(view));
            }
            terminal_edges.release_all(py);
            let total_size =
                total_size_from_header_fields(header_size_class, header_aux.kind, header_aux.word);
            terminal_resource_drop_no_unwind(|| {
                match heap_drop_policy(type_id) {
                    Some(HeapDropPolicy::String) => utf8_cache_remove(py, ptr as usize),
                    Some(HeapDropPolicy::Type) => {
                        bump_type_version();
                    }
                    Some(HeapDropPolicy::ListInt) => {
                        let storage = layout::list_int_storage_ptr(ptr);
                        if !storage.is_null() {
                            drop((*Box::from_raw(storage)).into_vec());
                        }
                    }
                    Some(HeapDropPolicy::ListBool) => {
                        let storage = layout::list_bool_storage_ptr(ptr);
                        if !storage.is_null() {
                            drop((*Box::from_raw(storage)).into_vec());
                        }
                    }
                    Some(HeapDropPolicy::List) => drop_detached_tracked_vec(seq_vec_ptr(ptr)),
                    Some(HeapDropPolicy::Dict) => {
                        drop_detached_tracked_vec(dict_order_ptr(ptr));
                        drop_detached_tracked_vec(dict_table_ptr(ptr));
                        drop_detached_tracked_vec(dict_hashes_ptr(ptr));
                    }
                    Some(
                        HeapDropPolicy::ListBuilder
                        | HeapDropPolicy::DictBuilder
                        | HeapDropPolicy::SetBuilder,
                    ) => {
                        drop_detached_linear_builder_vec(ptr);
                    }
                    Some(HeapDropPolicy::Bytearray) => {
                        drop_detached_tracked_vec(bytearray_vec_ptr(ptr))
                    }
                    Some(HeapDropPolicy::Set | HeapDropPolicy::Frozenset) => {
                        drop_detached_tracked_vec(set_order_ptr(ptr));
                        drop_detached_tracked_vec(set_table_ptr(ptr));
                        drop_detached_tracked_vec(set_hashes_ptr(ptr));
                    }
                    Some(HeapDropPolicy::Memoryview) => {
                        drop_detached_tracked_vec(memoryview_shape_ptr(ptr));
                        drop_detached_tracked_vec(memoryview_strides_ptr(ptr));
                    }
                    Some(HeapDropPolicy::Dataclass) => {
                        drop_detached_tracked_vec(dataclass_fields_ptr(ptr));
                        let desc = dataclass_desc_ptr(ptr);
                        if !desc.is_null() {
                            drop(Box::from_raw(desc));
                        }
                    }
                    Some(HeapDropPolicy::Map) => drop_detached_tracked_vec(map_iters_ptr(ptr)),
                    Some(HeapDropPolicy::WeakContainer) => {
                        weak_container::weakcontainer_drop_detached_state(ptr)
                    }
                    Some(HeapDropPolicy::Zip) => drop_detached_tracked_vec(zip_iters_ptr(ptr)),
                    Some(HeapDropPolicy::Buffer2d) => {
                        let buffer = buffer2d_ptr(ptr);
                        if !buffer.is_null() {
                            drop(Box::from_raw(buffer));
                        }
                    }
                    Some(HeapDropPolicy::GlobIter) => {
                        let state = glob_iter_state_ptr(ptr);
                        if !state.is_null() {
                            drop(Box::from_raw(state));
                        }
                    }
                    Some(HeapDropPolicy::Bigint) => std::ptr::drop_in_place(ptr as *mut BigInt),
                    Some(
                        HeapDropPolicy::None
                        | HeapDropPolicy::NativeHandle
                        | HeapDropPolicy::Foreign
                        | HeapDropPolicy::FileHandle
                        | HeapDropPolicy::ObjectShape
                        | HeapDropPolicy::Callargs
                        | HeapDropPolicy::Tuple
                        | HeapDropPolicy::Range
                        | HeapDropPolicy::Slice
                        | HeapDropPolicy::Code
                        | HeapDropPolicy::Function
                        | HeapDropPolicy::Module
                        | HeapDropPolicy::BoundMethod
                        | HeapDropPolicy::Property
                        | HeapDropPolicy::Super
                        | HeapDropPolicy::Classmethod
                        | HeapDropPolicy::Staticmethod
                        | HeapDropPolicy::GenericAlias
                        | HeapDropPolicy::Union
                        | HeapDropPolicy::DictView
                        | HeapDropPolicy::TracebackPayload
                        | HeapDropPolicy::Exception
                        | HeapDropPolicy::ContextManager
                        | HeapDropPolicy::Enumerate
                        | HeapDropPolicy::Filter
                        | HeapDropPolicy::Iter
                        | HeapDropPolicy::Reversed
                        | HeapDropPolicy::Generator
                        | HeapDropPolicy::AsyncGenerator
                        | HeapDropPolicy::CallIter,
                    ) => {}
                    None => {
                        eprintln!(
                            "molt fatal: unknown heap type id {type_id} reached deallocation"
                        );
                        std::process::abort();
                    }
                }
                release_ptr(ptr);
                // Notify the resource tracker only after typed backing state is gone.
                let _ = crate::resource::try_with_tracker(|t| t.on_free(total_size));
                if header_aux.kind == HEADER_AUX_KIND_SIDECAR {
                    free_aux_sidecar(header_aux.word);
                }
                if total_size != 0 && (header_flags & HEADER_FLAG_ARENA) == 0 {
                    let layout = std::alloc::Layout::from_size_align(total_size, 8)
                        .unwrap_or_else(|_| std::process::abort());
                    std::alloc::dealloc(header_ptr as *mut u8, layout);
                }
            });
            record_terminal_deallocation(py, type_id, dealloc_bytes);
        }
    }
}

#[cfg(test)]
mod tests {
    use std::sync::atomic::{AtomicUsize, Ordering};

    use crate::object::{
        ClassEdgeOwnership, HEADER_AUX_KIND_CLASS_INLINE, HEADER_AUX_KIND_SIDECAR,
        HEADER_AUX_KIND_STATE_INLINE, ObjectAuxPreselection, TYPE_ID_GENERATOR, TYPE_ID_OBJECT,
        TYPE_ID_STRING, alloc_object, alloc_object_with_aux, dec_ref_bits, object_class_bits,
        object_class_has_finalizer, object_has_class_edge, object_init_class_edge_unpublished,
        object_init_poll_fn_unpublished, object_init_sidecar_unpublished,
        object_init_state_unpublished, object_poll_fn, object_replace_class_edge, object_set_state,
        object_state, total_size_from_header,
    };
    use crate::resource::{LimitedTracker, ResourceLimits, UnlimitedTracker, set_tracker};

    static FINALIZER_VIEW_PTR: AtomicUsize = AtomicUsize::new(0);

    fn publish_borrowed_abi_view(bits: u64) {
        let ptr = unsafe { molt_cpython_abi::bridge::GLOBAL_BRIDGE.handle_to_borrowed_pyobj(bits) };
        assert!(!ptr.is_null());
        FINALIZER_VIEW_PTR.store(ptr.addr(), Ordering::SeqCst);
    }

    fn publish_direct_abi_view(bits: u64) {
        publish_borrowed_abi_view(bits);
        let ptr = core::ptr::with_exposed_provenance_mut::<molt_cpython_abi::abi_types::PyObject>(
            FINALIZER_VIEW_PTR.load(Ordering::SeqCst),
        );
        unsafe { molt_cpython_abi::api::refcount::Py_INCREF(ptr) };
    }

    fn with_builtin_object_finalizer_flag<R>(run: impl FnOnce(u64) -> R) -> R {
        let object_bits = crate::molt_object_new();
        crate::with_gil_entry_nopanic!(_py, {
            let class_ptr = crate::obj_from_bits(crate::builtin_classes(_py).object)
                .as_ptr()
                .expect("builtin object class");
            let class_header = unsafe { super::header_from_obj_ptr(class_ptr) };
            let old_flags = unsafe { (*class_header).load_metadata_flags() };
            unsafe {
                (*class_header).fetch_or_flags(super::HEADER_FLAG_CLASS_HAS_FINALIZER);
            }
            let result = run(object_bits);
            unsafe { (*class_header).store_flags(old_flags) };
            result
        })
    }

    #[test]
    fn finalizer_first_borrowed_abi_view_is_reconciled_and_retired() {
        let _guard = crate::test_support::RuntimeTestTransaction::new();
        FINALIZER_VIEW_PTR.store(0, Ordering::SeqCst);
        with_builtin_object_finalizer_flag(|object_bits| {
            super::FINALIZER_WINDOW_TEST_HOOK
                .with(|slot| slot.set(Some(publish_borrowed_abi_view)));
            crate::with_gil_entry_nopanic!(_py, {
                dec_ref_bits(_py, object_bits);
            });
            let ptr = core::ptr::with_exposed_provenance_mut::<molt_cpython_abi::abi_types::PyObject>(
                FINALIZER_VIEW_PTR.load(Ordering::SeqCst),
            );
            assert!(matches!(
                molt_cpython_abi::bridge::GLOBAL_BRIDGE.release_pyobj(ptr),
                molt_cpython_abi::bridge::PyObjRelease::Untracked
            ));
        });
    }

    #[test]
    fn finalizer_first_direct_c_view_roots_until_c_decref() {
        let _guard = crate::test_support::RuntimeTestTransaction::new();
        FINALIZER_VIEW_PTR.store(0, Ordering::SeqCst);
        with_builtin_object_finalizer_flag(|object_bits| {
            super::FINALIZER_WINDOW_TEST_HOOK.with(|slot| slot.set(Some(publish_direct_abi_view)));
            crate::with_gil_entry_nopanic!(_py, {
                dec_ref_bits(_py, object_bits);
            });
            assert!(
                molt_cpython_abi::bridge::GLOBAL_BRIDGE.has_direct_c_refs(object_bits),
                "direct C reference created by finalizer must root the runtime object"
            );
            let ptr = core::ptr::with_exposed_provenance_mut::<molt_cpython_abi::abi_types::PyObject>(
                FINALIZER_VIEW_PTR.load(Ordering::SeqCst),
            );
            unsafe { molt_cpython_abi::api::refcount::Py_DECREF(ptr) };
            assert!(matches!(
                molt_cpython_abi::bridge::GLOBAL_BRIDGE.release_pyobj(ptr),
                molt_cpython_abi::bridge::PyObjRelease::Untracked
            ));
        });
    }

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
    fn denied_sidecar_allocation_rolls_back_object_charge() {
        let _guard = crate::test_support::RuntimeTestTransaction::new();
        crate::with_gil_entry_nopanic!(_py, {
            let total = std::mem::size_of::<super::MoltHeader>();
            let plan = super::object_allocation_plan(total).expect("valid header-sized object");
            set_tracker(Box::new(LimitedTracker::new(&ResourceLimits {
                max_memory: Some(plan.alloc_size),
                ..Default::default()
            })));
            struct TrackerReset;
            impl Drop for TrackerReset {
                fn drop(&mut self) {
                    set_tracker(Box::new(UnlimitedTracker));
                }
            }
            let _reset = TrackerReset;

            let denied = alloc_object(_py, total, TYPE_ID_GENERATOR);
            assert!(
                denied.is_null(),
                "sidecar resource denial must fail the whole object allocation"
            );

            let allowed = alloc_object(_py, total, TYPE_ID_OBJECT);
            assert!(
                !allowed.is_null(),
                "sidecar denial must roll back the object's resource charge"
            );
            dec_ref_bits(_py, crate::MoltObject::from_ptr(allowed).bits());
        });
    }

    #[test]
    fn nonclass_state_never_impersonates_a_class_pointer() {
        let _guard = crate::test_support::RuntimeTestTransaction::new();
        crate::with_gil_entry_nopanic!(_py, {
            let ptr = crate::alloc_string(_py, b"state-discriminator");
            assert!(!ptr.is_null());
            let pointer_shaped_hash = crate::builtin_classes(_py).str;
            object_set_state(ptr, pointer_shaped_hash as i64);
            assert!(!unsafe { object_has_class_edge(ptr) });
            assert_eq!(unsafe { object_class_bits(ptr) }, 0);
            dec_ref_bits(_py, crate::MoltObject::from_ptr(ptr).bits());
        });
    }

    #[test]
    fn common_class_edge_stays_inline() {
        let _guard = crate::test_support::RuntimeTestTransaction::new();
        crate::with_gil_entry_nopanic!(_py, {
            let ptr = alloc_object_with_aux(
                _py,
                std::mem::size_of::<super::MoltHeader>(),
                TYPE_ID_OBJECT,
                ObjectAuxPreselection::ClassInline,
            );
            assert!(!ptr.is_null());
            let class_bits = crate::builtin_classes(_py).object;
            assert!(unsafe {
                object_init_class_edge_unpublished(
                    _py,
                    ptr,
                    class_bits,
                    ClassEdgeOwnership::Borrowed,
                )
            });
            let header = unsafe { &*super::header_from_obj_ptr(ptr) };
            assert_eq!(header.aux_kind, HEADER_AUX_KIND_CLASS_INLINE);
            assert_eq!(unsafe { object_class_bits(ptr) }, class_bits);
            dec_ref_bits(_py, crate::MoltObject::from_ptr(ptr).bits());
        });
    }

    #[test]
    fn fresh_and_published_class_edges_balance_owned_and_borrowed_refs() {
        let _guard = crate::test_support::RuntimeTestTransaction::new();
        crate::with_gil_entry_nopanic!(_py, {
            let name_ptr = crate::alloc_string(_py, b"AuxOwnershipClass");
            assert!(!name_ptr.is_null());
            let name_bits = crate::MoltObject::from_ptr(name_ptr).bits();
            let class_ptr = crate::alloc_class_obj(_py, name_bits);
            dec_ref_bits(_py, name_bits);
            assert!(!class_ptr.is_null());
            let class_bits = crate::MoltObject::from_ptr(class_ptr).bits();
            let initial = unsafe { (*super::header_from_obj_ptr(class_ptr)).ref_count_snapshot() };

            let owned_ptr = alloc_object_with_aux(
                _py,
                std::mem::size_of::<super::MoltHeader>(),
                TYPE_ID_OBJECT,
                ObjectAuxPreselection::ClassInline,
            );
            assert!(!owned_ptr.is_null());
            assert!(unsafe {
                object_init_class_edge_unpublished(
                    _py,
                    owned_ptr,
                    class_bits,
                    ClassEdgeOwnership::Owned,
                )
            });
            assert_eq!(
                unsafe { (*super::header_from_obj_ptr(class_ptr)).ref_count_snapshot() },
                initial + 1
            );
            assert!(!unsafe {
                object_init_class_edge_unpublished(
                    _py,
                    owned_ptr,
                    class_bits,
                    ClassEdgeOwnership::Borrowed,
                )
            });
            assert!(unsafe {
                object_replace_class_edge(_py, owned_ptr, class_bits, ClassEdgeOwnership::Borrowed)
            });
            assert_eq!(
                unsafe { (*super::header_from_obj_ptr(class_ptr)).ref_count_snapshot() },
                initial
            );
            assert!(unsafe {
                object_replace_class_edge(_py, owned_ptr, class_bits, ClassEdgeOwnership::Owned)
            });
            assert_eq!(
                unsafe { (*super::header_from_obj_ptr(class_ptr)).ref_count_snapshot() },
                initial + 1
            );
            assert!(unsafe {
                object_replace_class_edge(_py, owned_ptr, 0, ClassEdgeOwnership::Owned)
            });
            assert_eq!(
                unsafe { (*super::header_from_obj_ptr(class_ptr)).ref_count_snapshot() },
                initial
            );
            dec_ref_bits(_py, crate::MoltObject::from_ptr(owned_ptr).bits());

            let borrowed_ptr = alloc_object_with_aux(
                _py,
                std::mem::size_of::<super::MoltHeader>(),
                TYPE_ID_OBJECT,
                ObjectAuxPreselection::ClassInline,
            );
            assert!(!borrowed_ptr.is_null());
            assert!(unsafe {
                object_init_class_edge_unpublished(
                    _py,
                    borrowed_ptr,
                    class_bits,
                    ClassEdgeOwnership::Borrowed,
                )
            });
            assert_eq!(
                unsafe { (*super::header_from_obj_ptr(class_ptr)).ref_count_snapshot() },
                initial
            );
            assert!(unsafe {
                object_replace_class_edge(_py, borrowed_ptr, 0, ClassEdgeOwnership::Owned)
            });
            dec_ref_bits(_py, crate::MoltObject::from_ptr(borrowed_ptr).bits());
            dec_ref_bits(_py, class_bits);
        });
    }

    #[test]
    fn bare_object_constructor_preselects_replaceable_owned_class_edge() {
        let _guard = crate::test_support::RuntimeTestTransaction::new();
        let obj_bits = crate::molt_object_new();
        let (class_bits, initial_class_rc) = crate::with_gil_entry_nopanic!(_py, {
            let obj_ptr = crate::obj_from_bits(obj_bits)
                .as_ptr()
                .expect("bare object allocation");
            let header = unsafe { &*super::header_from_obj_ptr(obj_ptr) };
            assert_eq!(header.aux_kind, HEADER_AUX_KIND_CLASS_INLINE);
            assert!(
                header.gc_is_published(),
                "completed bare-object construction must be visible to cycle collection"
            );
            assert_eq!(
                unsafe { object_class_bits(obj_ptr) },
                crate::builtin_classes(_py).object
            );

            let name_ptr = crate::alloc_string(_py, b"ReplacementClass");
            assert!(!name_ptr.is_null());
            let name_bits = crate::MoltObject::from_ptr(name_ptr).bits();
            let class_ptr = crate::alloc_class_obj(_py, name_bits);
            dec_ref_bits(_py, name_bits);
            assert!(!class_ptr.is_null());
            (crate::MoltObject::from_ptr(class_ptr).bits(), unsafe {
                (*super::header_from_obj_ptr(class_ptr)).ref_count_snapshot()
            })
        });

        let obj_ptr = crate::obj_from_bits(obj_bits)
            .as_ptr()
            .expect("bare object allocation");
        let result = unsafe {
            crate::molt_object_set_class(
                crate::provenance::abi::expose_address(obj_ptr),
                class_bits,
            )
        };
        assert_eq!(result, crate::MoltObject::none().bits());

        crate::with_gil_entry_nopanic!(_py, {
            assert!(!crate::exception_pending(_py));
            assert_eq!(unsafe { object_class_bits(obj_ptr) }, class_bits);
            let class_rc = unsafe {
                (*super::header_from_obj_ptr(
                    crate::obj_from_bits(class_bits)
                        .as_ptr()
                        .expect("replacement class"),
                ))
                .ref_count_snapshot()
            };
            assert_eq!(
                class_rc,
                initial_class_rc + 1,
                "object replacement must acquire exactly one owned class edge"
            );
            dec_ref_bits(_py, obj_bits);
            assert_eq!(
                unsafe {
                    (*super::header_from_obj_ptr(
                        crate::obj_from_bits(class_bits)
                            .as_ptr()
                            .expect("replacement class"),
                    ))
                    .ref_count_snapshot()
                },
                initial_class_rc,
                "object teardown must discharge exactly its owned class edge"
            );
            dec_ref_bits(_py, class_bits);
        });
    }

    #[test]
    fn common_state_stays_inline() {
        let _guard = crate::test_support::RuntimeTestTransaction::new();
        crate::with_gil_entry_nopanic!(_py, {
            let ptr = crate::alloc_string(_py, b"inline-state");
            assert!(!ptr.is_null());
            object_set_state(ptr, 41);
            let header = unsafe { &*super::header_from_obj_ptr(ptr) };
            assert_eq!(header.aux_kind, HEADER_AUX_KIND_STATE_INLINE);
            assert_eq!(object_state(ptr), 41);
            dec_ref_bits(_py, crate::MoltObject::from_ptr(ptr).bits());
        });
    }

    #[test]
    fn denied_sidecar_upgrade_preserves_inline_state_and_representation() {
        let _guard = crate::test_support::RuntimeTestTransaction::new();
        crate::with_gil_entry_nopanic!(_py, {
            let ptr = crate::alloc_string(_py, b"preserve-inline-state");
            assert!(!ptr.is_null());
            object_set_state(ptr, 73);
            set_tracker(Box::new(LimitedTracker::new(&ResourceLimits {
                max_memory: Some(0),
                ..Default::default()
            })));
            let upgraded = unsafe { object_init_sidecar_unpublished(ptr) };
            set_tracker(Box::new(UnlimitedTracker));

            assert!(!upgraded);
            let header = unsafe { &*super::header_from_obj_ptr(ptr) };
            assert_eq!(header.aux_kind, HEADER_AUX_KIND_STATE_INLINE);
            assert_eq!(object_state(ptr), 73);
            dec_ref_bits(_py, crate::MoltObject::from_ptr(ptr).bits());
        });
    }

    #[test]
    fn class_state_and_poll_share_one_stable_sidecar() {
        let _guard = crate::test_support::RuntimeTestTransaction::new();
        crate::with_gil_entry_nopanic!(_py, {
            let ptr = alloc_object_with_aux(
                _py,
                std::mem::size_of::<super::MoltHeader>(),
                TYPE_ID_OBJECT,
                ObjectAuxPreselection::Sidecar,
            );
            assert!(!ptr.is_null());
            let class_bits = crate::builtin_classes(_py).object;
            assert!(unsafe {
                object_init_class_edge_unpublished(
                    _py,
                    ptr,
                    class_bits,
                    ClassEdgeOwnership::Borrowed,
                )
            });
            assert!(unsafe { object_init_state_unpublished(ptr, 73) });
            assert!(unsafe { object_init_poll_fn_unpublished(ptr, 0x1234) });
            let header = unsafe { &*super::header_from_obj_ptr(ptr) };
            let sidecar_addr = header.aux.load(std::sync::atomic::Ordering::Acquire);
            assert_eq!(header.aux_kind, HEADER_AUX_KIND_SIDECAR);
            assert_eq!(unsafe { object_class_bits(ptr) }, class_bits);
            assert_eq!(object_state(ptr), 73);
            assert_eq!(object_poll_fn(ptr), 0x1234);
            assert_eq!(
                header.aux.load(std::sync::atomic::Ordering::Acquire),
                sidecar_addr,
                "published sidecar address must never move"
            );
            dec_ref_bits(_py, crate::MoltObject::from_ptr(ptr).bits());
        });
    }

    #[test]
    fn non_object_heap_kind_derives_finalizer_policy_from_common_class_edge() {
        let _guard = crate::test_support::RuntimeTestTransaction::new();
        crate::with_gil_entry_nopanic!(_py, {
            let class_bits = crate::builtin_classes(_py).object;
            let class_ptr = crate::obj_from_bits(class_bits)
                .as_ptr()
                .expect("builtin object class");
            let class_header = unsafe { super::header_from_obj_ptr(class_ptr) };
            let old_flags = unsafe { (*class_header).load_metadata_flags() };
            struct RestoreClassFlags(*mut super::MoltHeader, u32);
            impl Drop for RestoreClassFlags {
                fn drop(&mut self) {
                    unsafe {
                        (*self.0).store_flags(self.1);
                    }
                }
            }
            let _restore = RestoreClassFlags(class_header, old_flags);
            unsafe {
                (*class_header).fetch_or_flags(super::HEADER_FLAG_CLASS_HAS_FINALIZER);
            }

            let ptr = alloc_object_with_aux(
                _py,
                std::mem::size_of::<super::MoltHeader>(),
                TYPE_ID_STRING,
                ObjectAuxPreselection::Sidecar,
            );
            assert!(!ptr.is_null());
            assert!(unsafe {
                object_init_class_edge_unpublished(
                    _py,
                    ptr,
                    class_bits,
                    ClassEdgeOwnership::Borrowed,
                )
            });
            assert!(unsafe { object_class_has_finalizer(ptr) });

            unsafe {
                (*class_header).store_flags(old_flags);
            }
            dec_ref_bits(_py, crate::MoltObject::from_ptr(ptr).bits());
        });
    }

    #[test]
    fn oversized_allocation_uses_immutable_sidecar_size() {
        let _guard = crate::test_support::RuntimeTestTransaction::new();
        crate::with_gil_entry_nopanic!(_py, {
            let total = super::SIZE_CLASS_TABLE
                .last()
                .copied()
                .expect("size classes")
                + 8;
            let ptr = alloc_object(_py, total, TYPE_ID_OBJECT);
            assert!(!ptr.is_null());
            let header = unsafe { &*super::header_from_obj_ptr(ptr) };
            assert_eq!(header.size_class, 0);
            assert_eq!(header.aux_kind, HEADER_AUX_KIND_SIDECAR);
            assert_eq!(total_size_from_header(header, ptr), total);
            dec_ref_bits(_py, crate::MoltObject::from_ptr(ptr).bits());
        });
    }

    #[cfg(all(not(target_arch = "wasm32"), feature = "free-threaded"))]
    #[test]
    fn disjoint_flag_publishers_preserve_both_bits() {
        use std::sync::{Arc, Barrier};

        let flags = Arc::new(super::MoltFlags::new(0));
        let barrier = Arc::new(Barrier::new(3));
        let mut workers = Vec::new();
        for bit in [
            super::HEADER_FLAG_CANCEL_PENDING,
            super::HEADER_FLAG_TASK_WAKE_PENDING,
        ] {
            let flags = Arc::clone(&flags);
            let barrier = Arc::clone(&barrier);
            workers.push(std::thread::spawn(move || {
                barrier.wait();
                flags.fetch_or(bit, std::sync::atomic::Ordering::AcqRel);
            }));
        }
        barrier.wait();
        for worker in workers {
            worker.join().expect("flag publisher must complete");
        }
        assert_eq!(
            flags.load(std::sync::atomic::Ordering::Acquire)
                & (super::HEADER_FLAG_CANCEL_PENDING | super::HEADER_FLAG_TASK_WAKE_PENDING),
            super::HEADER_FLAG_CANCEL_PENDING | super::HEADER_FLAG_TASK_WAKE_PENDING
        );
    }

    #[test]
    fn coherent_flag_transition_preserves_sibling_bits() {
        let mut header = std::mem::MaybeUninit::<super::MoltHeader>::zeroed();
        let header = header.as_mut_ptr();
        unsafe {
            super::MoltHeader::initialize_flags_before_publication(
                header,
                super::HEADER_FLAG_TASK_QUEUED | super::HEADER_FLAG_CANCEL_PENDING,
            );
            (*header).update_flags(
                super::HEADER_FLAG_TASK_RUNNING,
                super::HEADER_FLAG_TASK_QUEUED,
            );
            assert_eq!(
                (*header).load_synchronized_flags()
                    & (super::HEADER_FLAG_TASK_QUEUED
                        | super::HEADER_FLAG_TASK_RUNNING
                        | super::HEADER_FLAG_CANCEL_PENDING),
                super::HEADER_FLAG_TASK_RUNNING | super::HEADER_FLAG_CANCEL_PENDING
            );
        }
    }

    #[test]
    fn flag_ordering_classes_separate_metadata_from_synchronization() {
        assert!(!super::flag_transition_is_synchronized(
            super::HEADER_FLAG_IMMORTAL
                | super::HEADER_FLAG_HAS_PTRS
                | super::HEADER_FLAG_CONTAINS_REFS
        ));
        assert!(super::flag_transition_is_synchronized(
            super::HEADER_FLAG_TASK_DONE
        ));
        assert!(super::flag_transition_is_synchronized(
            super::HEADER_FLAG_DEALLOCATING | super::HEADER_FLAG_IMMORTAL
        ));
        assert!(!super::flag_transition_is_synchronized(0));
    }

    #[test]
    fn gc_publication_has_no_intermediate_visible_state() {
        let mut header = std::mem::MaybeUninit::<super::MoltHeader>::zeroed();
        let header = header.as_mut_ptr();
        unsafe {
            super::MoltHeader::initialize_flags_gc_unpublished(
                header,
                super::HEADER_FLAG_CONTAINS_REFS,
            );
            assert!(!(*header).gc_is_published());
            assert!((*header).has_flag(super::HEADER_FLAG_CONTAINS_REFS));
            (*header).gc_publish_initialized();
            assert!((*header).gc_is_published());
            assert!((*header).has_flag(super::HEADER_FLAG_CONTAINS_REFS));
        }
    }

    fn refcount_header(count: u32, flags: u32) -> super::MoltHeader {
        super::MoltHeader {
            type_id: 0,
            ref_count: molt_codegen_abi::MoltRefCount::new(count),
            flags: molt_codegen_abi::MoltFlags::new(flags),
            size_class: 0,
            aux_kind: 0,
            aux: super::MoltAuxWord::new(0),
        }
    }

    #[test]
    fn typed_refcount_transitions_cover_owned_live_gc_immortal_and_revival_states() {
        let owned = refcount_header(1, 0);
        assert_eq!(owned.retain_owned(0, "empty batch"), 1);
        assert_eq!(owned.retain_owned(3, "test batch"), 1);
        assert_eq!(owned.ref_count_snapshot(), 4);
        let release = owned.release_owned("test release");
        assert_eq!(release.previous(), 4);
        assert!(!release.reached_zero());
        assert_eq!(owned.ref_count_snapshot(), 3);

        let live = refcount_header(1, 0);
        assert!(live.try_retain_live());
        assert_eq!(live.ref_count_snapshot(), 2);
        assert!(!refcount_header(0, 0).try_retain_live());
        assert!(
            !refcount_header(
                molt_codegen_abi::IMMORTAL_REFCOUNT,
                super::HEADER_FLAG_IMMORTAL,
            )
            .try_retain_live()
        );
        assert!(!refcount_header(1, super::HEADER_FLAG_DEALLOCATING).try_retain_live());

        let gc = refcount_header(1, 0);
        gc.pin_for_gc();
        assert_eq!(gc.ref_count_snapshot(), 2);
        assert!(gc.has_flag(super::HEADER_FLAG_GC_PINNED));

        let ordinary_revival = refcount_header(0, 0);
        let ordinary_window = ordinary_revival.open_revival_window(false);
        assert_eq!(ordinary_window.baseline(), 1);
        assert_eq!(ordinary_revival.close_revival_window(ordinary_window), 1);
        assert_eq!(ordinary_revival.ref_count_snapshot(), 0);

        let view_revival = refcount_header(1, 0);
        let view_window = view_revival.open_revival_window(true);
        assert_eq!(view_window.baseline(), 2);
        assert_eq!(view_revival.close_revival_window(view_window), 2);
        assert_eq!(view_revival.ref_count_snapshot(), 1);
    }

    #[cfg(all(not(target_arch = "wasm32"), feature = "free-threaded"))]
    #[test]
    fn concurrent_refcount_roundtrips_preserve_the_baseline_owner() {
        use std::sync::{Arc, Barrier};

        const THREADS: usize = 4;
        const ITERATIONS: usize = 4_096;
        let refcount = Arc::new(molt_codegen_abi::MoltRefCount::new(1));
        let start = Arc::new(Barrier::new(THREADS));
        let mut workers = Vec::with_capacity(THREADS);
        for _ in 0..THREADS {
            let refcount = Arc::clone(&refcount);
            let start = Arc::clone(&start);
            workers.push(std::thread::spawn(move || {
                start.wait();
                for _ in 0..ITERATIONS {
                    refcount
                        .retain_owned(1, || false)
                        .expect("baseline owner keeps storage live");
                    assert!(
                        refcount
                            .release_owned()
                            .expect("worker release keeps storage live")
                            .previous()
                            > 1
                    );
                }
            }));
        }
        for worker in workers {
            worker.join().expect("refcount worker panicked");
        }
        assert_eq!(refcount.snapshot_acquire(), 1);
    }
}
