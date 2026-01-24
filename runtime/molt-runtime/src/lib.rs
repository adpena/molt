//! Molt Runtime Core
//! Handles memory management, task scheduling, channels, and FFI boundaries.
//!
//! Locking contract (contributor guidance):
//! - Runtime mutation is serialized by the GIL-like lock.
//! - The GIL is the outermost lock; do not acquire it while holding other runtime locks.
//! - Provenance registry locks live in `molt-obj-model` and are sharded; keep their
//!   critical sections small and avoid taking them while holding the GIL for long paths.
//! - Avoid blocking host I/O while holding the GIL; release or schedule work instead.

use crossbeam_channel::{bounded, unbounded, Receiver, Sender};
use getrandom::getrandom;
use molt_obj_model::MoltObject;
use num_bigint::{BigInt, Sign};
use num_integer::Integer;
use num_traits::{Signed, ToPrimitive, Zero};
use std::borrow::Cow;
use std::cell::{Cell, RefCell};
#[cfg(not(target_arch = "wasm32"))]
use std::collections::{HashMap, HashSet, VecDeque};
use std::ffi::{CStr, CString, OsString};
use std::fs::OpenOptions;
use std::io::{Cursor, ErrorKind, Read, Seek, Write};
#[cfg(not(target_arch = "wasm32"))]
use std::net::{IpAddr, Ipv4Addr, Ipv6Addr, SocketAddr, ToSocketAddrs};
#[cfg(unix)]
use std::os::fd::BorrowedFd;
#[cfg(not(target_arch = "wasm32"))]
use std::os::raw::{c_int, c_void};
#[cfg(unix)]
use std::os::unix::io::{AsRawFd, FromRawFd, IntoRawFd, RawFd};
#[cfg(windows)]
use std::os::windows::io::{AsRawSocket, BorrowedSocket, FromRawSocket, IntoRawSocket, RawSocket};
#[cfg(not(target_arch = "wasm32"))]
use std::process::Stdio;
use std::sync::atomic::{AtomicBool, AtomicI32, AtomicU64, AtomicUsize, Ordering as AtomicOrdering};
#[cfg(not(target_arch = "wasm32"))]
use std::sync::Condvar;
use std::sync::{Arc, Mutex, OnceLock};
use std::thread;
use std::time::{Duration, Instant};

#[cfg(not(target_arch = "wasm32"))]
use mio::{Events, Interest, Poll, Token, Waker};
#[cfg(not(target_arch = "wasm32"))]
use socket2::{Domain, Protocol, SockAddr, SockRef, Socket, Type};

macro_rules! fn_addr {
    ($func:path) => {
        $func as *const () as usize as u64
    };
}

mod object;
mod async_rt;
mod builtins;
mod call;
mod concurrency;
mod provenance;
mod state;

use crate::async_rt::*;
use crate::builtins::attr::{
    attr_error, attr_lookup_ptr_allow_missing, attr_name_bits_from_bytes, class_attr_lookup,
    class_attr_lookup_raw_mro, class_field_offset, dataclass_attr_lookup_raw, descriptor_bind,
    descriptor_cache_lookup, descriptor_cache_store, descriptor_is_data, descriptor_method_bits,
    descriptor_no_deleter, descriptor_no_setter, dir_collect_from_class_bits,
    dir_collect_from_instance, instance_bits_for_call, is_iterator_bits, module_attr_lookup,
    object_attr_lookup_raw, property_no_deleter, property_no_setter, raise_attr_name_type_error,
};
pub(crate) use crate::builtins::classes::{
    builtin_classes, builtin_classes_shutdown, builtin_type_bits, class_name_for_error,
    is_builtin_class_bits, BuiltinClasses,
};
use crate::builtins::containers::{
    dict_len, dict_method_bits, dict_order, dict_order_ptr, dict_table, dict_table_ptr,
    dict_view_as_set_bits, dict_view_dict_bits, dict_view_entry, dict_view_len,
    frozenset_method_bits, is_set_inplace_rhs_type, is_set_like_type, is_set_view_type, list_len,
    list_method_bits, set_len, set_method_bits, set_order, set_order_ptr, set_table, set_table_ptr,
    tuple_len,
};
pub(crate) use crate::builtins::exceptions::{
    alloc_exception, alloc_exception_from_class_bits, clear_exception, clear_exception_state,
    clear_exception_type_cache, exception_args_bits, exception_args_from_iterable,
    exception_cause_bits, exception_class_bits, exception_context_align_depth,
    exception_context_bits, exception_context_fallback_pop, exception_context_fallback_push,
    exception_dict_bits, exception_kind_bits, exception_message_from_args, exception_method_bits,
    exception_msg_bits, exception_pending, exception_set_stop_iteration_value,
    exception_stack_depth, exception_stack_pop, exception_stack_push, exception_stack_set_depth,
    exception_store_args_and_message, exception_suppress_bits, exception_trace_bits,
    exception_type_bits, exception_type_bits_from_name, exception_value_bits, format_exception,
    format_exception_message, generator_exception_stack_drop, generator_exception_stack_store,
    generator_exception_stack_take, generator_raise_active, molt_exception_clear,
    molt_exception_kind, molt_exception_last, molt_raise, raise_exception,
    raise_key_error_with_key, raise_not_iterable, raise_unsupported_inplace, record_exception,
    set_generator_raise, set_task_raise_active, task_exception_depth_drop,
    task_exception_depth_store, task_exception_depth_take, task_exception_handler_stack_drop,
    task_exception_handler_stack_store, task_exception_handler_stack_take,
    task_exception_stack_drop, task_exception_stack_store, task_exception_stack_take,
    task_last_exception_drop, task_raise_active, ExceptionSentinel, ACTIVE_EXCEPTION_FALLBACK,
    ACTIVE_EXCEPTION_STACK, EXCEPTION_STACK, GENERATOR_EXCEPTION_STACKS, GENERATOR_RAISE,
    TASK_RAISE_ACTIVE,
};
#[cfg(not(target_arch = "wasm32"))]
pub(crate) use crate::builtins::exceptions::{raise_os_error, raise_os_error_errno};
pub(crate) use crate::builtins::numbers::{
    bigint_bits, bigint_from_f64_trunc, bigint_ptr_from_bits, bigint_ref, bigint_to_inline,
    compare_numbers, float_pair_from_obj, index_bigint_from_obj, index_i64_from_obj,
    index_i64_with_overflow, inline_int_from_i128, int_bits_from_bigint, int_bits_from_i128,
    int_bits_from_i64, round_float_ndigits, round_half_even, split_maxsplit_from_obj, to_bigint,
    to_f64, to_i64,
};
use crate::builtins::strings::{
    bytes_count_impl, bytes_find_impl, bytes_rfind_impl, bytes_strip_range, replace_bytes_impl,
    replace_bytes_impl_limit, replace_string_impl, rsplit_bytes_to_list_maxsplit,
    rsplit_bytes_whitespace_to_list_maxsplit, rsplit_string_bytes_to_list_maxsplit,
    rsplit_string_whitespace_to_list_maxsplit, split_bytes_to_list_maxsplit,
    split_bytes_whitespace_to_list_maxsplit, split_string_bytes_to_list_maxsplit,
    split_string_whitespace_to_list_maxsplit, splitlines_bytes_to_list, splitlines_string_to_list,
};
#[allow(unused_imports)]
pub(crate) use crate::call::bind::molt_callargs_push_kw;
pub(crate) use crate::call::bind::{
    callargs_ptr, molt_call_bind, molt_callargs_expand_kwstar, molt_callargs_expand_star,
    molt_callargs_new, molt_callargs_push_pos,
};
pub(crate) use crate::call::dispatch::{
    call_callable0, call_callable1, call_callable2, call_callable3, callable_arity,
};
pub(crate) use crate::call::function::{
    call_function_obj0, call_function_obj1, call_function_obj2, call_function_obj3,
    call_function_obj4, call_function_obj_vec,
};
use crate::concurrency::GilGuard;
pub use crate::object::ops::*;
pub use crate::object::{molt_dec_ref, molt_inc_ref, MoltHeader};
pub(crate) use crate::object::{
    alloc_object, alloc_object_zeroed_with_pool, bits_from_ptr, buffer2d_ptr, bytes_data, bytes_len,
    dataclass_desc_ptr, dataclass_dict_bits, dataclass_fields_mut, dataclass_fields_ref,
    dataclass_set_dict_bits, dec_ref_bits, file_handle_ptr, header_from_obj_ptr, inc_ref_bits,
    init_atomic_bits, instance_dict_bits, instance_set_dict_bits, intarray_len, intarray_slice,
    maybe_ptr_from_bits,
    memoryview_format_bits, memoryview_itemsize, memoryview_len, memoryview_ndim, memoryview_offset,
    memoryview_owner_bits, memoryview_ptr, memoryview_readonly, memoryview_shape,
    memoryview_stride, memoryview_strides,
    obj_from_bits, object_class_bits, object_mark_has_ptrs, object_payload_size,
    object_set_class_bits, object_type_id, pending_bits_i64, ptr_from_bits, string_bytes,
    string_len, Buffer2D, DataclassDesc, MemoryView, MemoryViewFormat, MemoryViewFormatKind,
    MoltFileHandle, MoltFileState, PtrSlot, HEADER_FLAG_CANCEL_PENDING, HEADER_FLAG_GEN_RUNNING,
    HEADER_FLAG_GEN_STARTED, HEADER_FLAG_SKIP_CLASS_DECREF, HEADER_FLAG_SPAWN_RETAIN,
    OBJECT_POOL_BUCKETS, OBJECT_POOL_TLS,
};
pub(crate) use crate::object::ops::{
    decode_string_list, decode_value_list, dict_clear_method, dict_copy_method, dict_fromkeys_method,
    dict_get_method, dict_items_method, dict_keys_method, dict_pop_method, dict_popitem_method,
    dict_setdefault_method, dict_update_apply, dict_update_method, dict_update_set_in_place,
    dict_update_set_via_store, dict_values_method, format_obj, format_obj_str,
    frozenset_from_iter_bits, list_from_iter_bits, tuple_from_iter_bits, utf8_cache_remove,
    utf8_codepoint_count_cached,
};
use crate::provenance::{release_ptr, reset_ptr_registry, resolve_ptr};
pub(crate) use crate::state::runtime_state::runtime_state;
pub(crate) use crate::state::{
    CONTEXT_STACK, DEFAULT_RECURSION_LIMIT, FRAME_STACK, GIL_DEPTH, PARSE_ARENA, RECURSION_DEPTH,
    RECURSION_LIMIT, REPR_DEPTH, REPR_STACK,
};
use crate::state::RuntimeState;

// Keep in sync with MOLT_BIND_KIND_OPEN in src/molt/frontend/__init__.py.
const BIND_KIND_OPEN: i64 = 1;

const IO_EVENT_READ: u32 = 1;
const IO_EVENT_WRITE: u32 = 1 << 1;
const IO_EVENT_ERROR: u32 = 1 << 2;

#[cfg(all(unix, any(target_os = "linux", target_os = "android")))]
const SOCK_NONBLOCK_FLAG: i32 = libc::SOCK_NONBLOCK;
#[cfg(all(unix, not(any(target_os = "linux", target_os = "android"))))]
const SOCK_NONBLOCK_FLAG: i32 = 0;
#[cfg(all(unix, any(target_os = "linux", target_os = "android")))]
const SOCK_CLOEXEC_FLAG: i32 = libc::SOCK_CLOEXEC;
#[cfg(all(unix, not(any(target_os = "linux", target_os = "android"))))]
const SOCK_CLOEXEC_FLAG: i32 = 0;

#[cfg(target_arch = "wasm32")]
const WASM_TABLE_BASE: u64 = 256;
#[cfg(target_arch = "wasm32")]
const WASM_TABLE_IDX_ASYNC_SLEEP: u64 = WASM_TABLE_BASE + 1;
#[cfg(target_arch = "wasm32")]
const WASM_TABLE_IDX_ANEXT_DEFAULT_POLL: u64 = WASM_TABLE_BASE + 2;
#[cfg(target_arch = "wasm32")]
const WASM_TABLE_IDX_ASYNCGEN_POLL: u64 = WASM_TABLE_BASE + 3;
#[cfg(target_arch = "wasm32")]
const WASM_TABLE_IDX_IO_WAIT: u64 = WASM_TABLE_BASE + 4;
#[cfg(target_arch = "wasm32")]
const WASM_TABLE_IDX_THREAD_POLL: u64 = WASM_TABLE_BASE + 5;
#[cfg(target_arch = "wasm32")]
const WASM_TABLE_IDX_PROCESS_POLL: u64 = WASM_TABLE_BASE + 6;

#[inline]
fn async_sleep_poll_fn_addr() -> u64 {
    #[cfg(target_arch = "wasm32")]
    {
        // Keep in sync with wasm table layout in runtime/molt-backend/src/wasm.rs.
        WASM_TABLE_IDX_ASYNC_SLEEP
    }
    #[cfg(not(target_arch = "wasm32"))]
    {
        fn_addr!(molt_async_sleep)
    }
}

#[inline]
fn anext_default_poll_fn_addr() -> u64 {
    #[cfg(target_arch = "wasm32")]
    {
        // Keep in sync with wasm table layout in runtime/molt-backend/src/wasm.rs.
        WASM_TABLE_IDX_ANEXT_DEFAULT_POLL
    }
    #[cfg(not(target_arch = "wasm32"))]
    {
        fn_addr!(molt_anext_default_poll)
    }
}

#[inline]
fn asyncgen_poll_fn_addr() -> u64 {
    #[cfg(target_arch = "wasm32")]
    {
        // Keep in sync with wasm table layout in runtime/molt-backend/src/wasm.rs.
        WASM_TABLE_IDX_ASYNCGEN_POLL
    }
    #[cfg(not(target_arch = "wasm32"))]
    {
        fn_addr!(molt_asyncgen_poll)
    }
}

#[inline]
fn io_wait_poll_fn_addr() -> u64 {
    #[cfg(target_arch = "wasm32")]
    {
        WASM_TABLE_IDX_IO_WAIT
    }
    #[cfg(not(target_arch = "wasm32"))]
    {
        fn_addr!(molt_io_wait)
    }
}

#[inline]
fn thread_poll_fn_addr() -> u64 {
    #[cfg(target_arch = "wasm32")]
    {
        WASM_TABLE_IDX_THREAD_POLL
    }
    #[cfg(not(target_arch = "wasm32"))]
    {
        fn_addr!(molt_thread_poll)
    }
}

#[inline]
fn process_poll_fn_addr() -> u64 {
    #[cfg(target_arch = "wasm32")]
    {
        WASM_TABLE_IDX_PROCESS_POLL
    }
    #[cfg(not(target_arch = "wasm32"))]
    {
        fn_addr!(molt_process_poll)
    }
}

#[cfg(target_arch = "wasm32")]
#[link(wasm_import_module = "env")]
extern "C" {
    #[link_name = "molt_call_indirect0"]
    pub(crate) fn molt_call_indirect0(func_idx: u64) -> i64;
    #[link_name = "molt_call_indirect1"]
    pub(crate) fn molt_call_indirect1(func_idx: u64, arg0: u64) -> i64;
    #[link_name = "molt_call_indirect2"]
    pub(crate) fn molt_call_indirect2(func_idx: u64, arg0: u64, arg1: u64) -> i64;
    #[link_name = "molt_call_indirect3"]
    pub(crate) fn molt_call_indirect3(func_idx: u64, arg0: u64, arg1: u64, arg2: u64) -> i64;
    #[link_name = "molt_call_indirect4"]
    pub(crate) fn molt_call_indirect4(
        func_idx: u64,
        arg0: u64,
        arg1: u64,
        arg2: u64,
        arg3: u64,
    ) -> i64;
    #[link_name = "molt_call_indirect5"]
    pub(crate) fn molt_call_indirect5(
        func_idx: u64,
        arg0: u64,
        arg1: u64,
        arg2: u64,
        arg3: u64,
        arg4: u64,
    ) -> i64;
    #[link_name = "molt_call_indirect6"]
    pub(crate) fn molt_call_indirect6(
        func_idx: u64,
        arg0: u64,
        arg1: u64,
        arg2: u64,
        arg3: u64,
        arg4: u64,
        arg5: u64,
    ) -> i64;
    #[link_name = "molt_call_indirect7"]
    pub(crate) fn molt_call_indirect7(
        func_idx: u64,
        arg0: u64,
        arg1: u64,
        arg2: u64,
        arg3: u64,
        arg4: u64,
        arg5: u64,
        arg6: u64,
    ) -> i64;
    #[link_name = "molt_call_indirect8"]
    pub(crate) fn molt_call_indirect8(
        func_idx: u64,
        arg0: u64,
        arg1: u64,
        arg2: u64,
        arg3: u64,
        arg4: u64,
        arg5: u64,
        arg6: u64,
        arg7: u64,
    ) -> i64;
    #[link_name = "molt_call_indirect9"]
    pub(crate) fn molt_call_indirect9(
        func_idx: u64,
        arg0: u64,
        arg1: u64,
        arg2: u64,
        arg3: u64,
        arg4: u64,
        arg5: u64,
        arg6: u64,
        arg7: u64,
        arg8: u64,
    ) -> i64;
    #[link_name = "molt_call_indirect10"]
    pub(crate) fn molt_call_indirect10(
        func_idx: u64,
        arg0: u64,
        arg1: u64,
        arg2: u64,
        arg3: u64,
        arg4: u64,
        arg5: u64,
        arg6: u64,
        arg7: u64,
        arg8: u64,
        arg9: u64,
    ) -> i64;
    #[link_name = "molt_call_indirect11"]
    pub(crate) fn molt_call_indirect11(
        func_idx: u64,
        arg0: u64,
        arg1: u64,
        arg2: u64,
        arg3: u64,
        arg4: u64,
        arg5: u64,
        arg6: u64,
        arg7: u64,
        arg8: u64,
        arg9: u64,
        arg10: u64,
    ) -> i64;
    #[link_name = "molt_call_indirect12"]
    pub(crate) fn molt_call_indirect12(
        func_idx: u64,
        arg0: u64,
        arg1: u64,
        arg2: u64,
        arg3: u64,
        arg4: u64,
        arg5: u64,
        arg6: u64,
        arg7: u64,
        arg8: u64,
        arg9: u64,
        arg10: u64,
        arg11: u64,
    ) -> i64;
    #[link_name = "molt_call_indirect13"]
    pub(crate) fn molt_call_indirect13(
        func_idx: u64,
        arg0: u64,
        arg1: u64,
        arg2: u64,
        arg3: u64,
        arg4: u64,
        arg5: u64,
        arg6: u64,
        arg7: u64,
        arg8: u64,
        arg9: u64,
        arg10: u64,
        arg11: u64,
        arg12: u64,
    ) -> i64;
    #[link_name = "molt_db_query_host"]
    fn molt_db_query_host(req_bits: u64, len_bits: u64, out_bits: u64, token_id: u64) -> i32;
    #[link_name = "molt_db_exec_host"]
    fn molt_db_exec_host(req_bits: u64, len_bits: u64, out_bits: u64, token_id: u64) -> i32;
}

#[cfg(target_arch = "wasm32")]
#[link(wasm_import_module = "wasi_snapshot_preview1")]
extern "C" {
    fn environ_sizes_get(environ_count: *mut u32, environ_buf_size: *mut u32) -> u16;
    fn environ_get(environ: *mut *mut u8, environ_buf: *mut u8) -> u16;
}

struct Utf8IndexCache {
    offsets: Vec<usize>,
    prefix: Vec<i64>,
}

struct Utf8CountCache {
    needle: Vec<u8>,
    count: i64,
    prefix: Vec<i64>,
    hay_len: usize,
}

struct Utf8CountCacheEntry {
    key: usize,
    cache: Arc<Utf8CountCache>,
}

struct AttrNameCacheEntry {
    bytes: Vec<u8>,
    bits: u64,
}

#[derive(Clone)]
struct DescriptorCacheEntry {
    class_bits: u64,
    attr_name: Vec<u8>,
    version: u64,
    data_desc_bits: Option<u64>,
    class_attr_bits: Option<u64>,
}

#[derive(Clone, Copy)]
struct FrameEntry {
    code_bits: u64,
    line: i64,
}

struct InternedNames {
    bases_name: AtomicU64,
    mro_name: AtomicU64,
    get_name: AtomicU64,
    set_name: AtomicU64,
    delete_name: AtomicU64,
    set_name_method: AtomicU64,
    getattr_name: AtomicU64,
    getattribute_name: AtomicU64,
    call_name: AtomicU64,
    init_name: AtomicU64,
    new_name: AtomicU64,
    enter_name: AtomicU64,
    exit_name: AtomicU64,
    setattr_name: AtomicU64,
    delattr_name: AtomicU64,
    write_name: AtomicU64,
    flush_name: AtomicU64,
    sys_name: AtomicU64,
    stdout_name: AtomicU64,
    modules_name: AtomicU64,
    all_name: AtomicU64,
    fspath_name: AtomicU64,
    dict_name: AtomicU64,
    molt_dict_data_name: AtomicU64,
    class_name: AtomicU64,
    annotations_name: AtomicU64,
    annotate_name: AtomicU64,
    field_offsets_name: AtomicU64,
    molt_layout_size: AtomicU64,
    float_name: AtomicU64,
    index_name: AtomicU64,
    int_name: AtomicU64,
    round_name: AtomicU64,
    trunc_name: AtomicU64,
    repr_name: AtomicU64,
    str_name: AtomicU64,
    format_name: AtomicU64,
    qualname_name: AtomicU64,
    name_name: AtomicU64,
    f_lasti_name: AtomicU64,
    f_code_name: AtomicU64,
    f_lineno_name: AtomicU64,
    tb_frame_name: AtomicU64,
    tb_lineno_name: AtomicU64,
    tb_next_name: AtomicU64,
    molt_arg_names: AtomicU64,
    molt_posonly: AtomicU64,
    molt_kwonly_names: AtomicU64,
    molt_vararg: AtomicU64,
    molt_varkw: AtomicU64,
    molt_closure_size: AtomicU64,
    molt_is_coroutine: AtomicU64,
    molt_is_generator: AtomicU64,
    molt_bind_kind: AtomicU64,
    defaults_name: AtomicU64,
    kwdefaults_name: AtomicU64,
    lt_name: AtomicU64,
    le_name: AtomicU64,
    gt_name: AtomicU64,
    ge_name: AtomicU64,
    eq_name: AtomicU64,
    ne_name: AtomicU64,
    add_name: AtomicU64,
    radd_name: AtomicU64,
    mul_name: AtomicU64,
    rmul_name: AtomicU64,
    sub_name: AtomicU64,
    rsub_name: AtomicU64,
    truediv_name: AtomicU64,
    rtruediv_name: AtomicU64,
    floordiv_name: AtomicU64,
    rfloordiv_name: AtomicU64,
    or_name: AtomicU64,
    ror_name: AtomicU64,
    and_name: AtomicU64,
    rand_name: AtomicU64,
    xor_name: AtomicU64,
    rxor_name: AtomicU64,
    iadd_name: AtomicU64,
    isub_name: AtomicU64,
    ior_name: AtomicU64,
    iand_name: AtomicU64,
    ixor_name: AtomicU64,
}

impl InternedNames {
    fn new() -> Self {
        Self {
            bases_name: AtomicU64::new(0),
            mro_name: AtomicU64::new(0),
            get_name: AtomicU64::new(0),
            set_name: AtomicU64::new(0),
            delete_name: AtomicU64::new(0),
            set_name_method: AtomicU64::new(0),
            getattr_name: AtomicU64::new(0),
            getattribute_name: AtomicU64::new(0),
            call_name: AtomicU64::new(0),
            init_name: AtomicU64::new(0),
            new_name: AtomicU64::new(0),
            enter_name: AtomicU64::new(0),
            exit_name: AtomicU64::new(0),
            setattr_name: AtomicU64::new(0),
            delattr_name: AtomicU64::new(0),
            write_name: AtomicU64::new(0),
            flush_name: AtomicU64::new(0),
            sys_name: AtomicU64::new(0),
            stdout_name: AtomicU64::new(0),
            modules_name: AtomicU64::new(0),
            all_name: AtomicU64::new(0),
            fspath_name: AtomicU64::new(0),
            dict_name: AtomicU64::new(0),
            molt_dict_data_name: AtomicU64::new(0),
            class_name: AtomicU64::new(0),
            annotations_name: AtomicU64::new(0),
            annotate_name: AtomicU64::new(0),
            field_offsets_name: AtomicU64::new(0),
            molt_layout_size: AtomicU64::new(0),
            float_name: AtomicU64::new(0),
            index_name: AtomicU64::new(0),
            int_name: AtomicU64::new(0),
            round_name: AtomicU64::new(0),
            trunc_name: AtomicU64::new(0),
            repr_name: AtomicU64::new(0),
            str_name: AtomicU64::new(0),
            format_name: AtomicU64::new(0),
            qualname_name: AtomicU64::new(0),
            name_name: AtomicU64::new(0),
            f_lasti_name: AtomicU64::new(0),
            f_code_name: AtomicU64::new(0),
            f_lineno_name: AtomicU64::new(0),
            tb_frame_name: AtomicU64::new(0),
            tb_lineno_name: AtomicU64::new(0),
            tb_next_name: AtomicU64::new(0),
            molt_arg_names: AtomicU64::new(0),
            molt_posonly: AtomicU64::new(0),
            molt_kwonly_names: AtomicU64::new(0),
            molt_vararg: AtomicU64::new(0),
            molt_varkw: AtomicU64::new(0),
            molt_closure_size: AtomicU64::new(0),
            molt_is_coroutine: AtomicU64::new(0),
            molt_is_generator: AtomicU64::new(0),
            molt_bind_kind: AtomicU64::new(0),
            defaults_name: AtomicU64::new(0),
            kwdefaults_name: AtomicU64::new(0),
            lt_name: AtomicU64::new(0),
            le_name: AtomicU64::new(0),
            gt_name: AtomicU64::new(0),
            ge_name: AtomicU64::new(0),
            eq_name: AtomicU64::new(0),
            ne_name: AtomicU64::new(0),
            add_name: AtomicU64::new(0),
            radd_name: AtomicU64::new(0),
            mul_name: AtomicU64::new(0),
            rmul_name: AtomicU64::new(0),
            sub_name: AtomicU64::new(0),
            rsub_name: AtomicU64::new(0),
            truediv_name: AtomicU64::new(0),
            rtruediv_name: AtomicU64::new(0),
            floordiv_name: AtomicU64::new(0),
            rfloordiv_name: AtomicU64::new(0),
            or_name: AtomicU64::new(0),
            ror_name: AtomicU64::new(0),
            and_name: AtomicU64::new(0),
            rand_name: AtomicU64::new(0),
            xor_name: AtomicU64::new(0),
            rxor_name: AtomicU64::new(0),
            iadd_name: AtomicU64::new(0),
            isub_name: AtomicU64::new(0),
            ior_name: AtomicU64::new(0),
            iand_name: AtomicU64::new(0),
            ixor_name: AtomicU64::new(0),
        }
    }
}

struct MethodCache {
    dict_keys: AtomicU64,
    dict_values: AtomicU64,
    dict_items: AtomicU64,
    dict_get: AtomicU64,
    dict_pop: AtomicU64,
    dict_clear: AtomicU64,
    dict_copy: AtomicU64,
    dict_popitem: AtomicU64,
    dict_setdefault: AtomicU64,
    dict_update: AtomicU64,
    dict_fromkeys: AtomicU64,
    dict_getitem: AtomicU64,
    dict_setitem: AtomicU64,
    dict_delitem: AtomicU64,
    dict_iter: AtomicU64,
    dict_len: AtomicU64,
    dict_contains: AtomicU64,
    dict_reversed: AtomicU64,
    set_add: AtomicU64,
    set_discard: AtomicU64,
    set_remove: AtomicU64,
    set_pop: AtomicU64,
    set_clear: AtomicU64,
    set_update: AtomicU64,
    set_union: AtomicU64,
    set_intersection: AtomicU64,
    set_difference: AtomicU64,
    set_symdiff: AtomicU64,
    set_intersection_update: AtomicU64,
    set_difference_update: AtomicU64,
    set_symdiff_update: AtomicU64,
    set_isdisjoint: AtomicU64,
    set_issubset: AtomicU64,
    set_issuperset: AtomicU64,
    set_copy: AtomicU64,
    set_iter: AtomicU64,
    set_len: AtomicU64,
    set_contains: AtomicU64,
    frozenset_union: AtomicU64,
    frozenset_intersection: AtomicU64,
    frozenset_difference: AtomicU64,
    frozenset_symdiff: AtomicU64,
    frozenset_isdisjoint: AtomicU64,
    frozenset_issubset: AtomicU64,
    frozenset_issuperset: AtomicU64,
    frozenset_copy: AtomicU64,
    frozenset_iter: AtomicU64,
    frozenset_len: AtomicU64,
    frozenset_contains: AtomicU64,
    list_append: AtomicU64,
    list_extend: AtomicU64,
    list_insert: AtomicU64,
    list_remove: AtomicU64,
    list_pop: AtomicU64,
    list_clear: AtomicU64,
    list_init: AtomicU64,
    list_copy: AtomicU64,
    list_reverse: AtomicU64,
    list_count: AtomicU64,
    list_index: AtomicU64,
    list_sort: AtomicU64,
    list_add: AtomicU64,
    list_mul: AtomicU64,
    list_rmul: AtomicU64,
    list_iadd: AtomicU64,
    list_imul: AtomicU64,
    list_getitem: AtomicU64,
    list_setitem: AtomicU64,
    list_delitem: AtomicU64,
    list_iter: AtomicU64,
    list_len: AtomicU64,
    list_contains: AtomicU64,
    list_reversed: AtomicU64,
    str_iter: AtomicU64,
    str_len: AtomicU64,
    str_contains: AtomicU64,
    str_count: AtomicU64,
    str_startswith: AtomicU64,
    str_endswith: AtomicU64,
    str_find: AtomicU64,
    str_rfind: AtomicU64,
    str_format: AtomicU64,
    str_upper: AtomicU64,
    str_lower: AtomicU64,
    str_strip: AtomicU64,
    str_lstrip: AtomicU64,
    str_rstrip: AtomicU64,
    str_split: AtomicU64,
    str_rsplit: AtomicU64,
    str_splitlines: AtomicU64,
    str_partition: AtomicU64,
    str_rpartition: AtomicU64,
    str_replace: AtomicU64,
    str_join: AtomicU64,
    str_encode: AtomicU64,
    bytes_iter: AtomicU64,
    bytes_len: AtomicU64,
    bytes_contains: AtomicU64,
    bytes_count: AtomicU64,
    bytes_startswith: AtomicU64,
    bytes_endswith: AtomicU64,
    bytes_find: AtomicU64,
    bytes_rfind: AtomicU64,
    bytes_split: AtomicU64,
    bytes_rsplit: AtomicU64,
    bytes_reversed: AtomicU64,
    bytes_strip: AtomicU64,
    bytes_lstrip: AtomicU64,
    bytes_rstrip: AtomicU64,
    bytes_splitlines: AtomicU64,
    bytes_partition: AtomicU64,
    bytes_rpartition: AtomicU64,
    bytes_replace: AtomicU64,
    bytes_decode: AtomicU64,
    bytearray_iter: AtomicU64,
    bytearray_len: AtomicU64,
    bytearray_contains: AtomicU64,
    bytearray_count: AtomicU64,
    bytearray_startswith: AtomicU64,
    bytearray_endswith: AtomicU64,
    bytearray_find: AtomicU64,
    bytearray_rfind: AtomicU64,
    bytearray_split: AtomicU64,
    bytearray_rsplit: AtomicU64,
    bytearray_reversed: AtomicU64,
    bytearray_strip: AtomicU64,
    bytearray_lstrip: AtomicU64,
    bytearray_rstrip: AtomicU64,
    bytearray_splitlines: AtomicU64,
    bytearray_partition: AtomicU64,
    bytearray_rpartition: AtomicU64,
    bytearray_replace: AtomicU64,
    bytearray_decode: AtomicU64,
    bytearray_setitem: AtomicU64,
    bytearray_delitem: AtomicU64,
    slice_indices: AtomicU64,
    slice_hash: AtomicU64,
    slice_eq: AtomicU64,
    slice_reduce: AtomicU64,
    slice_reduce_ex: AtomicU64,
    memoryview_tobytes: AtomicU64,
    memoryview_cast: AtomicU64,
    memoryview_setitem: AtomicU64,
    memoryview_delitem: AtomicU64,
    file_read: AtomicU64,
    file_readline: AtomicU64,
    file_readlines: AtomicU64,
    file_readinto: AtomicU64,
    file_write: AtomicU64,
    file_writelines: AtomicU64,
    file_flush: AtomicU64,
    file_close: AtomicU64,
    file_detach: AtomicU64,
    file_reconfigure: AtomicU64,
    file_seek: AtomicU64,
    file_tell: AtomicU64,
    file_fileno: AtomicU64,
    file_truncate: AtomicU64,
    file_readable: AtomicU64,
    file_writable: AtomicU64,
    file_seekable: AtomicU64,
    file_isatty: AtomicU64,
    file_iter: AtomicU64,
    file_next: AtomicU64,
    file_enter: AtomicU64,
    file_exit: AtomicU64,
    asyncgen_aiter: AtomicU64,
    asyncgen_anext: AtomicU64,
    asyncgen_asend: AtomicU64,
    asyncgen_athrow: AtomicU64,
    asyncgen_aclose: AtomicU64,
    object_getattribute: AtomicU64,
    object_init: AtomicU64,
    object_setattr: AtomicU64,
    object_delattr: AtomicU64,
    object_eq: AtomicU64,
    object_ne: AtomicU64,
    exception_init: AtomicU64,
    exception_new: AtomicU64,
    generic_alias_class_getitem: AtomicU64,
}

impl MethodCache {
    fn new() -> Self {
        Self {
            dict_keys: AtomicU64::new(0),
            dict_values: AtomicU64::new(0),
            dict_items: AtomicU64::new(0),
            dict_get: AtomicU64::new(0),
            dict_pop: AtomicU64::new(0),
            dict_clear: AtomicU64::new(0),
            dict_copy: AtomicU64::new(0),
            dict_popitem: AtomicU64::new(0),
            dict_setdefault: AtomicU64::new(0),
            dict_update: AtomicU64::new(0),
            dict_fromkeys: AtomicU64::new(0),
            dict_getitem: AtomicU64::new(0),
            dict_setitem: AtomicU64::new(0),
            dict_delitem: AtomicU64::new(0),
            dict_iter: AtomicU64::new(0),
            dict_len: AtomicU64::new(0),
            dict_contains: AtomicU64::new(0),
            dict_reversed: AtomicU64::new(0),
            set_add: AtomicU64::new(0),
            set_discard: AtomicU64::new(0),
            set_remove: AtomicU64::new(0),
            set_pop: AtomicU64::new(0),
            set_clear: AtomicU64::new(0),
            set_update: AtomicU64::new(0),
            set_union: AtomicU64::new(0),
            set_intersection: AtomicU64::new(0),
            set_difference: AtomicU64::new(0),
            set_symdiff: AtomicU64::new(0),
            set_intersection_update: AtomicU64::new(0),
            set_difference_update: AtomicU64::new(0),
            set_symdiff_update: AtomicU64::new(0),
            set_isdisjoint: AtomicU64::new(0),
            set_issubset: AtomicU64::new(0),
            set_issuperset: AtomicU64::new(0),
            set_copy: AtomicU64::new(0),
            set_iter: AtomicU64::new(0),
            set_len: AtomicU64::new(0),
            set_contains: AtomicU64::new(0),
            frozenset_union: AtomicU64::new(0),
            frozenset_intersection: AtomicU64::new(0),
            frozenset_difference: AtomicU64::new(0),
            frozenset_symdiff: AtomicU64::new(0),
            frozenset_isdisjoint: AtomicU64::new(0),
            frozenset_issubset: AtomicU64::new(0),
            frozenset_issuperset: AtomicU64::new(0),
            frozenset_copy: AtomicU64::new(0),
            frozenset_iter: AtomicU64::new(0),
            frozenset_len: AtomicU64::new(0),
            frozenset_contains: AtomicU64::new(0),
            list_append: AtomicU64::new(0),
            list_extend: AtomicU64::new(0),
            list_insert: AtomicU64::new(0),
            list_remove: AtomicU64::new(0),
            list_pop: AtomicU64::new(0),
            list_clear: AtomicU64::new(0),
            list_init: AtomicU64::new(0),
            list_copy: AtomicU64::new(0),
            list_reverse: AtomicU64::new(0),
            list_count: AtomicU64::new(0),
            list_index: AtomicU64::new(0),
            list_sort: AtomicU64::new(0),
            list_add: AtomicU64::new(0),
            list_mul: AtomicU64::new(0),
            list_rmul: AtomicU64::new(0),
            list_iadd: AtomicU64::new(0),
            list_imul: AtomicU64::new(0),
            list_getitem: AtomicU64::new(0),
            list_setitem: AtomicU64::new(0),
            list_delitem: AtomicU64::new(0),
            list_iter: AtomicU64::new(0),
            list_len: AtomicU64::new(0),
            list_contains: AtomicU64::new(0),
            list_reversed: AtomicU64::new(0),
            str_iter: AtomicU64::new(0),
            str_len: AtomicU64::new(0),
            str_contains: AtomicU64::new(0),
            str_count: AtomicU64::new(0),
            str_startswith: AtomicU64::new(0),
            str_endswith: AtomicU64::new(0),
            str_find: AtomicU64::new(0),
            str_rfind: AtomicU64::new(0),
            str_format: AtomicU64::new(0),
            str_upper: AtomicU64::new(0),
            str_lower: AtomicU64::new(0),
            str_strip: AtomicU64::new(0),
            str_lstrip: AtomicU64::new(0),
            str_rstrip: AtomicU64::new(0),
            str_split: AtomicU64::new(0),
            str_rsplit: AtomicU64::new(0),
            str_splitlines: AtomicU64::new(0),
            str_partition: AtomicU64::new(0),
            str_rpartition: AtomicU64::new(0),
            str_replace: AtomicU64::new(0),
            str_join: AtomicU64::new(0),
            str_encode: AtomicU64::new(0),
            bytes_iter: AtomicU64::new(0),
            bytes_len: AtomicU64::new(0),
            bytes_contains: AtomicU64::new(0),
            bytes_count: AtomicU64::new(0),
            bytes_startswith: AtomicU64::new(0),
            bytes_endswith: AtomicU64::new(0),
            bytes_find: AtomicU64::new(0),
            bytes_rfind: AtomicU64::new(0),
            bytes_split: AtomicU64::new(0),
            bytes_rsplit: AtomicU64::new(0),
            bytes_reversed: AtomicU64::new(0),
            bytes_strip: AtomicU64::new(0),
            bytes_lstrip: AtomicU64::new(0),
            bytes_rstrip: AtomicU64::new(0),
            bytes_splitlines: AtomicU64::new(0),
            bytes_partition: AtomicU64::new(0),
            bytes_rpartition: AtomicU64::new(0),
            bytes_replace: AtomicU64::new(0),
            bytes_decode: AtomicU64::new(0),
            bytearray_iter: AtomicU64::new(0),
            bytearray_len: AtomicU64::new(0),
            bytearray_contains: AtomicU64::new(0),
            bytearray_count: AtomicU64::new(0),
            bytearray_startswith: AtomicU64::new(0),
            bytearray_endswith: AtomicU64::new(0),
            bytearray_find: AtomicU64::new(0),
            bytearray_rfind: AtomicU64::new(0),
            bytearray_split: AtomicU64::new(0),
            bytearray_rsplit: AtomicU64::new(0),
            bytearray_reversed: AtomicU64::new(0),
            bytearray_strip: AtomicU64::new(0),
            bytearray_lstrip: AtomicU64::new(0),
            bytearray_rstrip: AtomicU64::new(0),
            bytearray_splitlines: AtomicU64::new(0),
            bytearray_partition: AtomicU64::new(0),
            bytearray_rpartition: AtomicU64::new(0),
            bytearray_replace: AtomicU64::new(0),
            bytearray_decode: AtomicU64::new(0),
            bytearray_setitem: AtomicU64::new(0),
            bytearray_delitem: AtomicU64::new(0),
            slice_indices: AtomicU64::new(0),
            slice_hash: AtomicU64::new(0),
            slice_eq: AtomicU64::new(0),
            slice_reduce: AtomicU64::new(0),
            slice_reduce_ex: AtomicU64::new(0),
            memoryview_tobytes: AtomicU64::new(0),
            memoryview_cast: AtomicU64::new(0),
            memoryview_setitem: AtomicU64::new(0),
            memoryview_delitem: AtomicU64::new(0),
            file_read: AtomicU64::new(0),
            file_readline: AtomicU64::new(0),
            file_readlines: AtomicU64::new(0),
            file_readinto: AtomicU64::new(0),
            file_write: AtomicU64::new(0),
            file_writelines: AtomicU64::new(0),
            file_flush: AtomicU64::new(0),
            file_close: AtomicU64::new(0),
            file_detach: AtomicU64::new(0),
            file_reconfigure: AtomicU64::new(0),
            file_seek: AtomicU64::new(0),
            file_tell: AtomicU64::new(0),
            file_fileno: AtomicU64::new(0),
            file_truncate: AtomicU64::new(0),
            file_readable: AtomicU64::new(0),
            file_writable: AtomicU64::new(0),
            file_seekable: AtomicU64::new(0),
            file_isatty: AtomicU64::new(0),
            file_iter: AtomicU64::new(0),
            file_next: AtomicU64::new(0),
            file_enter: AtomicU64::new(0),
            file_exit: AtomicU64::new(0),
            asyncgen_aiter: AtomicU64::new(0),
            asyncgen_anext: AtomicU64::new(0),
            asyncgen_asend: AtomicU64::new(0),
            asyncgen_athrow: AtomicU64::new(0),
            asyncgen_aclose: AtomicU64::new(0),
            object_getattribute: AtomicU64::new(0),
            object_init: AtomicU64::new(0),
            object_setattr: AtomicU64::new(0),
            object_delattr: AtomicU64::new(0),
            object_eq: AtomicU64::new(0),
            object_ne: AtomicU64::new(0),
            exception_init: AtomicU64::new(0),
            exception_new: AtomicU64::new(0),
            generic_alias_class_getitem: AtomicU64::new(0),
        }
    }
}

struct Utf8CountCacheStore {
    entries: HashMap<usize, Arc<Utf8CountCache>>,
    order: VecDeque<usize>,
    capacity: usize,
}

fn build_utf8_count_cache() -> Vec<Mutex<Utf8CountCacheStore>> {
    let per_shard = (UTF8_CACHE_MAX_ENTRIES / UTF8_COUNT_CACHE_SHARDS).max(1);
    (0..UTF8_COUNT_CACHE_SHARDS)
        .map(|_| Mutex::new(Utf8CountCacheStore::new(per_shard)))
        .collect()
}

impl Utf8CountCacheStore {
    fn new(capacity: usize) -> Self {
        Self {
            entries: HashMap::new(),
            order: VecDeque::new(),
            capacity,
        }
    }

    fn get(&self, key: usize) -> Option<Arc<Utf8CountCache>> {
        self.entries.get(&key).cloned()
    }

    fn insert(&mut self, key: usize, cache: Arc<Utf8CountCache>) {
        if let std::collections::hash_map::Entry::Occupied(mut entry) = self.entries.entry(key) {
            entry.insert(cache);
            return;
        }
        self.entries.insert(key, cache);
        self.order.push_back(key);
        while self.entries.len() > self.capacity {
            if let Some(evict) = self.order.pop_front() {
                self.entries.remove(&evict);
            } else {
                break;
            }
        }
    }

    fn remove(&mut self, key: usize) {
        self.entries.remove(&key);
        self.order.retain(|entry| *entry != key);
    }
}

struct Utf8CacheStore {
    entries: HashMap<usize, Arc<Utf8IndexCache>>,
    order: VecDeque<usize>,
}

impl Utf8CacheStore {
    fn new() -> Self {
        Self {
            entries: HashMap::new(),
            order: VecDeque::new(),
        }
    }

    fn get(&self, key: usize) -> Option<Arc<Utf8IndexCache>> {
        self.entries.get(&key).cloned()
    }

    fn insert(&mut self, key: usize, cache: Arc<Utf8IndexCache>) {
        if self.entries.contains_key(&key) {
            return;
        }
        self.entries.insert(key, cache);
        self.order.push_back(key);
        while self.entries.len() > UTF8_CACHE_MAX_ENTRIES {
            if let Some(evict) = self.order.pop_front() {
                self.entries.remove(&evict);
            } else {
                break;
            }
        }
    }

    fn remove(&mut self, key: usize) {
        self.entries.remove(&key);
        self.order.retain(|entry| *entry != key);
    }
}

const TYPE_ID_STRING: u32 = 200;
const TYPE_ID_OBJECT: u32 = 100;
const TYPE_ID_LIST: u32 = 201;
const TYPE_ID_BYTES: u32 = 202;
const TYPE_ID_LIST_BUILDER: u32 = 203;
const TYPE_ID_DICT: u32 = 204;
const TYPE_ID_DICT_BUILDER: u32 = 205;
const TYPE_ID_TUPLE: u32 = 206;
const TYPE_ID_DICT_KEYS_VIEW: u32 = 207;
const TYPE_ID_DICT_VALUES_VIEW: u32 = 208;
const TYPE_ID_DICT_ITEMS_VIEW: u32 = 209;
const TYPE_ID_ITER: u32 = 210;
const TYPE_ID_BYTEARRAY: u32 = 211;
const TYPE_ID_RANGE: u32 = 212;
const TYPE_ID_SLICE: u32 = 213;
const TYPE_ID_EXCEPTION: u32 = 214;
const TYPE_ID_DATACLASS: u32 = 215;
const TYPE_ID_BUFFER2D: u32 = 216;
const TYPE_ID_CONTEXT_MANAGER: u32 = 217;
const TYPE_ID_FILE_HANDLE: u32 = 218;
const TYPE_ID_MEMORYVIEW: u32 = 219;
const TYPE_ID_INTARRAY: u32 = 220;
const TYPE_ID_FUNCTION: u32 = 221;
const TYPE_ID_BOUND_METHOD: u32 = 222;
const TYPE_ID_MODULE: u32 = 223;
const TYPE_ID_TYPE: u32 = 224;
const TYPE_ID_GENERATOR: u32 = 225;
const TYPE_ID_CLASSMETHOD: u32 = 226;
const TYPE_ID_STATICMETHOD: u32 = 227;
const TYPE_ID_PROPERTY: u32 = 228;
const TYPE_ID_SUPER: u32 = 229;
const TYPE_ID_SET: u32 = 230;
const TYPE_ID_SET_BUILDER: u32 = 231;
const TYPE_ID_FROZENSET: u32 = 232;
const TYPE_ID_BIGINT: u32 = 233;
const TYPE_ID_ENUMERATE: u32 = 234;
const TYPE_ID_CALLARGS: u32 = 235;
const TYPE_ID_NOT_IMPLEMENTED: u32 = 236;
const TYPE_ID_CALL_ITER: u32 = 237;
const TYPE_ID_REVERSED: u32 = 238;
const TYPE_ID_ZIP: u32 = 239;
const TYPE_ID_MAP: u32 = 240;
const TYPE_ID_FILTER: u32 = 241;
const TYPE_ID_CODE: u32 = 242;
const TYPE_ID_ELLIPSIS: u32 = 243;
const TYPE_ID_GENERIC_ALIAS: u32 = 244;
const TYPE_ID_ASYNC_GENERATOR: u32 = 245;

const INLINE_INT_MIN_I128: i128 = -(1_i128 << 46);
const INLINE_INT_MAX_I128: i128 = (1_i128 << 46) - 1;
const MAX_SMALL_LIST: usize = 16;
const ITER_EXHAUSTED: usize = usize::MAX;
const FUNC_DEFAULT_NONE: i64 = 1;
const FUNC_DEFAULT_DICT_POP: i64 = 2;
const FUNC_DEFAULT_DICT_UPDATE: i64 = 3;
const FUNC_DEFAULT_REPLACE_COUNT: i64 = 4;
const FUNC_DEFAULT_NEG_ONE: i64 = 5;
const FUNC_DEFAULT_ZERO: i64 = 6;
const FUNC_DEFAULT_MISSING: i64 = 7;
const GEN_SEND_OFFSET: usize = 0;
const GEN_THROW_OFFSET: usize = 8;
const GEN_CLOSED_OFFSET: usize = 16;
const GEN_EXC_DEPTH_OFFSET: usize = 24;
const GEN_CONTROL_SIZE: usize = 48;
const ASYNCGEN_GEN_OFFSET: usize = 0;
const ASYNCGEN_RUNNING_OFFSET: usize = 8;
const ASYNCGEN_PENDING_OFFSET: usize = 16;
const ASYNCGEN_CONTROL_SIZE: usize = 24;
const ASYNCGEN_OP_ANEXT: i64 = 0;
const ASYNCGEN_OP_ASEND: i64 = 1;
const ASYNCGEN_OP_ATHROW: i64 = 2;
const ASYNCGEN_OP_ACLOSE: i64 = 3;
const TASK_KIND_FUTURE: u64 = 0;
const TASK_KIND_GENERATOR: u64 = 1;
const UTF8_CACHE_BLOCK: usize = 4096;
const UTF8_CACHE_MIN_LEN: usize = 16 * 1024;
const UTF8_COUNT_PREFIX_MIN_LEN: usize = UTF8_CACHE_BLOCK;
const UTF8_CACHE_MAX_ENTRIES: usize = 128;
const UTF8_COUNT_CACHE_SHARDS: usize = 8;
pub(crate) const TYPE_TAG_ANY: i64 = 0;
pub(crate) const TYPE_TAG_INT: i64 = 1;
pub(crate) const TYPE_TAG_FLOAT: i64 = 2;
pub(crate) const TYPE_TAG_BOOL: i64 = 3;
pub(crate) const TYPE_TAG_NONE: i64 = 4;
pub(crate) const TYPE_TAG_STR: i64 = 5;
pub(crate) const TYPE_TAG_BYTES: i64 = 6;
pub(crate) const TYPE_TAG_BYTEARRAY: i64 = 7;
pub(crate) const TYPE_TAG_LIST: i64 = 8;
pub(crate) const TYPE_TAG_TUPLE: i64 = 9;
pub(crate) const TYPE_TAG_DICT: i64 = 10;
pub(crate) const TYPE_TAG_RANGE: i64 = 11;
pub(crate) const TYPE_TAG_SLICE: i64 = 12;
pub(crate) const TYPE_TAG_DATACLASS: i64 = 13;
pub(crate) const TYPE_TAG_BUFFER2D: i64 = 14;
pub(crate) const TYPE_TAG_MEMORYVIEW: i64 = 15;
pub(crate) const TYPE_TAG_INTARRAY: i64 = 16;
pub(crate) const TYPE_TAG_SET: i64 = 17;
pub(crate) const TYPE_TAG_FROZENSET: i64 = 18;
pub(crate) const BUILTIN_TAG_OBJECT: i64 = 100;
pub(crate) const BUILTIN_TAG_TYPE: i64 = 101;
pub(crate) const BUILTIN_TAG_BASE_EXCEPTION: i64 = 102;
pub(crate) const BUILTIN_TAG_EXCEPTION: i64 = 103;

pub(crate) static CALL_DISPATCH_COUNT: AtomicU64 = AtomicU64::new(0);
static STRUCT_FIELD_STORE_COUNT: AtomicU64 = AtomicU64::new(0);
static ATTR_LOOKUP_COUNT: AtomicU64 = AtomicU64::new(0);
static LAYOUT_GUARD_COUNT: AtomicU64 = AtomicU64::new(0);
static LAYOUT_GUARD_FAIL: AtomicU64 = AtomicU64::new(0);
static ALLOC_COUNT: AtomicU64 = AtomicU64::new(0);
static ASYNC_POLL_COUNT: AtomicU64 = AtomicU64::new(0);
static ASYNC_PENDING_COUNT: AtomicU64 = AtomicU64::new(0);
static ASYNC_WAKEUP_COUNT: AtomicU64 = AtomicU64::new(0);
static ASYNC_SLEEP_REGISTER_COUNT: AtomicU64 = AtomicU64::new(0);
static ASYNCGEN_REGISTRY: OnceLock<Mutex<HashSet<PtrSlot>>> = OnceLock::new();
static FN_PTR_CODE: OnceLock<Mutex<HashMap<u64, u64>>> = OnceLock::new();
const PY_HASH_BITS: u32 = 61;
const PY_HASH_MODULUS: u64 = (1u64 << PY_HASH_BITS) - 1;
const PY_HASH_INF: i64 = 314_159;
const PY_HASH_NONE: i64 = 0xfca86420;
const PY_HASHSEED_MAX: u64 = 4_294_967_295;

fn profile_enabled() -> bool {
    *runtime_state().profile_enabled.get_or_init(|| {
        std::env::var("MOLT_PROFILE")
            .map(|val| !val.is_empty() && val != "0")
            .unwrap_or(false)
    })
}

#[no_mangle]
pub extern "C" fn molt_profile_enabled() -> u64 {
    if profile_enabled() {
        1
    } else {
        0
    }
}

pub(crate) fn profile_hit(counter: &AtomicU64) {
    if profile_enabled() {
        counter.fetch_add(1, AtomicOrdering::Relaxed);
    }
}

#[no_mangle]
pub extern "C" fn molt_profile_struct_field_store() {
    profile_hit(&STRUCT_FIELD_STORE_COUNT);
}

macro_rules! file_handle_require_attached {
    ($handle:expr) => {
        if $handle.detached {
            return raise_exception::<_>("ValueError", file_handle_detached_message($handle));
        }
    };
}

fn slice_bounds_from_args(
    start_bits: u64,
    end_bits: u64,
    has_start: bool,
    has_end: bool,
    len: i64,
) -> (i64, i64, i64) {
    let msg = "slice indices must be integers or None or have an __index__ method";
    let start_obj = if has_start {
        Some(obj_from_bits(start_bits))
    } else {
        None
    };
    let end_obj = if has_end {
        Some(obj_from_bits(end_bits))
    } else {
        None
    };
    let mut start = if let Some(obj) = start_obj {
        if obj.is_none() {
            0
        } else {
            index_i64_from_obj(start_bits, msg)
        }
    } else {
        0
    };
    let mut end = if let Some(obj) = end_obj {
        if obj.is_none() {
            len
        } else {
            index_i64_from_obj(end_bits, msg)
        }
    } else {
        len
    };
    if start < 0 {
        start += len;
    }
    if end < 0 {
        end += len;
    }
    let start_raw = start;
    if start < 0 {
        start = 0;
    }
    if end < 0 {
        end = 0;
    }
    if start > len {
        start = len;
    }
    if end > len {
        end = len;
    }
    (start, end, start_raw)
}

fn slice_match(slice: &[u8], needle: &[u8], start_raw: i64, total: i64, suffix: bool) -> bool {
    if needle.is_empty() {
        return start_raw <= total;
    }
    if suffix {
        slice.ends_with(needle)
    } else {
        slice.starts_with(needle)
    }
}

fn is_truthy(obj: MoltObject) -> bool {
    if obj.is_none() {
        return false;
    }
    if let Some(b) = obj.as_bool() {
        return b;
    }
    if let Some(i) = obj.as_int() {
        return i != 0;
    }
    if let Some(f) = obj.as_float() {
        return f != 0.0;
    }
    if let Some(ptr) = obj.as_ptr() {
        unsafe {
            let type_id = object_type_id(ptr);
            if type_id == TYPE_ID_STRING {
                return string_len(ptr) > 0;
            }
            if type_id == TYPE_ID_BYTES {
                return string_len(ptr) > 0;
            }
            if type_id == TYPE_ID_BYTEARRAY {
                return string_len(ptr) > 0;
            }
            if type_id == TYPE_ID_MEMORYVIEW {
                return memoryview_len(ptr) > 0;
            }
            if type_id == TYPE_ID_BIGINT {
                return !bigint_ref(ptr).is_zero();
            }
            if type_id == TYPE_ID_LIST {
                return list_len(ptr) > 0;
            }
            if type_id == TYPE_ID_TUPLE {
                return tuple_len(ptr) > 0;
            }
            if type_id == TYPE_ID_INTARRAY {
                return intarray_len(ptr) > 0;
            }
            if type_id == TYPE_ID_DICT {
                return dict_len(ptr) > 0;
            }
            if type_id == TYPE_ID_OBJECT {
                return true;
            }
            if type_id == TYPE_ID_SET {
                return set_len(ptr) > 0;
            }
            if type_id == TYPE_ID_FROZENSET {
                return set_len(ptr) > 0;
            }
            if type_id == TYPE_ID_BUFFER2D {
                let buf_ptr = buffer2d_ptr(ptr);
                if buf_ptr.is_null() {
                    return false;
                }
                let buf = &*buf_ptr;
                return buf.rows.saturating_mul(buf.cols) > 0;
            }
            if type_id == TYPE_ID_DICT_KEYS_VIEW
                || type_id == TYPE_ID_DICT_VALUES_VIEW
                || type_id == TYPE_ID_DICT_ITEMS_VIEW
            {
                return dict_view_len(ptr) > 0;
            }
            if type_id == TYPE_ID_RANGE {
                let len = range_len_i64(range_start(ptr), range_stop(ptr), range_step(ptr));
                return len > 0;
            }
            if type_id == TYPE_ID_ITER {
                return true;
            }
            if type_id == TYPE_ID_GENERATOR {
                return true;
            }
            if type_id == TYPE_ID_ASYNC_GENERATOR {
                return true;
            }
            if type_id == TYPE_ID_ENUMERATE {
                return true;
            }
            if type_id == TYPE_ID_CALL_ITER
                || type_id == TYPE_ID_REVERSED
                || type_id == TYPE_ID_ZIP
                || type_id == TYPE_ID_MAP
                || type_id == TYPE_ID_FILTER
            {
                return true;
            }
            if type_id == TYPE_ID_SLICE {
                return true;
            }
            if type_id == TYPE_ID_DATACLASS {
                return true;
            }
            if type_id == TYPE_ID_CONTEXT_MANAGER {
                return true;
            }
            if type_id == TYPE_ID_FILE_HANDLE {
                return true;
            }
            return true;
        }
    }
    false
}

fn type_name(obj: MoltObject) -> Cow<'static, str> {
    if obj.is_int() {
        return Cow::Borrowed("int");
    }
    if obj.is_float() {
        return Cow::Borrowed("float");
    }
    if obj.is_bool() {
        return Cow::Borrowed("bool");
    }
    if obj.is_none() {
        return Cow::Borrowed("NoneType");
    }
    if let Some(ptr) = obj.as_ptr() {
        unsafe {
            return match object_type_id(ptr) {
                TYPE_ID_STRING => Cow::Borrowed("str"),
                TYPE_ID_BYTES => Cow::Borrowed("bytes"),
                TYPE_ID_BYTEARRAY => Cow::Borrowed("bytearray"),
                TYPE_ID_LIST => Cow::Borrowed("list"),
                TYPE_ID_TUPLE => Cow::Borrowed("tuple"),
                TYPE_ID_DICT => Cow::Borrowed("dict"),
                TYPE_ID_DICT_KEYS_VIEW => Cow::Borrowed("dict_keys"),
                TYPE_ID_DICT_VALUES_VIEW => Cow::Borrowed("dict_values"),
                TYPE_ID_DICT_ITEMS_VIEW => Cow::Borrowed("dict_items"),
                TYPE_ID_SET => Cow::Borrowed("set"),
                TYPE_ID_FROZENSET => Cow::Borrowed("frozenset"),
                TYPE_ID_BIGINT => Cow::Borrowed("int"),
                TYPE_ID_RANGE => Cow::Borrowed("range"),
                TYPE_ID_SLICE => Cow::Borrowed("slice"),
                TYPE_ID_MEMORYVIEW => Cow::Borrowed("memoryview"),
                TYPE_ID_INTARRAY => Cow::Borrowed("intarray"),
                TYPE_ID_NOT_IMPLEMENTED => Cow::Borrowed("NotImplementedType"),
                TYPE_ID_ELLIPSIS => Cow::Borrowed("ellipsis"),
                TYPE_ID_EXCEPTION => Cow::Borrowed("Exception"),
                TYPE_ID_DATACLASS => Cow::Owned(class_name_for_error(type_of_bits(obj.bits()))),
                TYPE_ID_BUFFER2D => Cow::Borrowed("buffer2d"),
                TYPE_ID_CONTEXT_MANAGER => Cow::Borrowed("context_manager"),
                TYPE_ID_FILE_HANDLE => Cow::Owned(class_name_for_error(type_of_bits(obj.bits()))),
                TYPE_ID_FUNCTION => Cow::Borrowed("function"),
                TYPE_ID_BOUND_METHOD => Cow::Borrowed("method"),
                TYPE_ID_CODE => Cow::Borrowed("code"),
                TYPE_ID_MODULE => Cow::Borrowed("module"),
                TYPE_ID_TYPE => Cow::Borrowed("type"),
                TYPE_ID_GENERIC_ALIAS => Cow::Borrowed("types.GenericAlias"),
                TYPE_ID_GENERATOR => Cow::Borrowed("generator"),
                TYPE_ID_ASYNC_GENERATOR => Cow::Borrowed("async_generator"),
                TYPE_ID_ENUMERATE => Cow::Borrowed("enumerate"),
                TYPE_ID_CALL_ITER => Cow::Borrowed("callable_iterator"),
                TYPE_ID_REVERSED => Cow::Borrowed("reversed"),
                TYPE_ID_ZIP => Cow::Borrowed("zip"),
                TYPE_ID_MAP => Cow::Borrowed("map"),
                TYPE_ID_FILTER => Cow::Borrowed("filter"),
                TYPE_ID_CLASSMETHOD => Cow::Borrowed("classmethod"),
                TYPE_ID_STATICMETHOD => Cow::Borrowed("staticmethod"),
                TYPE_ID_PROPERTY => Cow::Borrowed("property"),
                TYPE_ID_SUPER => Cow::Borrowed("super"),
                TYPE_ID_OBJECT => Cow::Owned(class_name_for_error(type_of_bits(obj.bits()))),
                _ => Cow::Borrowed("object"),
            };
        }
    }
    Cow::Borrowed("object")
}

enum BinaryDunderOutcome {
    Value(u64),
    NotImplemented,
    Missing,
    Error,
}

unsafe fn call_dunder_raw(
    raw_bits: u64,
    owner_ptr: *mut u8,
    instance_ptr: Option<*mut u8>,
    arg_bits: u64,
) -> BinaryDunderOutcome {
    let Some(inst_ptr) = instance_ptr else {
        return BinaryDunderOutcome::Missing;
    };
    let Some(bound_bits) = descriptor_bind(raw_bits, owner_ptr, Some(inst_ptr)) else {
        if exception_pending() {
            return BinaryDunderOutcome::Error;
        }
        return BinaryDunderOutcome::Missing;
    };
    let res_bits = call_callable1(bound_bits, arg_bits);
    dec_ref_bits(bound_bits);
    if exception_pending() {
        dec_ref_bits(res_bits);
        return BinaryDunderOutcome::Error;
    }
    if is_not_implemented_bits(res_bits) {
        dec_ref_bits(res_bits);
        return BinaryDunderOutcome::NotImplemented;
    }
    BinaryDunderOutcome::Value(res_bits)
}

unsafe fn call_binary_dunder(
    lhs_bits: u64,
    rhs_bits: u64,
    op_name_bits: u64,
    rop_name_bits: u64,
) -> Option<u64> {
    let lhs_obj = obj_from_bits(lhs_bits);
    let rhs_obj = obj_from_bits(rhs_bits);
    let lhs_ptr = lhs_obj.as_ptr();
    let rhs_ptr = rhs_obj.as_ptr();

    let lhs_type_bits = type_of_bits(lhs_bits);
    let rhs_type_bits = type_of_bits(rhs_bits);
    let lhs_type_ptr = obj_from_bits(lhs_type_bits).as_ptr();
    let rhs_type_ptr = obj_from_bits(rhs_type_bits).as_ptr();

    let lhs_op_raw = lhs_type_ptr.and_then(|ptr| class_attr_lookup_raw_mro(ptr, op_name_bits));
    let rhs_rop_raw = rhs_type_ptr.and_then(|ptr| class_attr_lookup_raw_mro(ptr, rop_name_bits));

    let rhs_is_subclass =
        rhs_type_bits != lhs_type_bits && issubclass_bits(rhs_type_bits, lhs_type_bits);
    let prefer_rhs = rhs_is_subclass
        && rhs_rop_raw.is_some()
        && lhs_op_raw.map_or(true, |lhs_raw| lhs_raw != rhs_rop_raw.unwrap());

    let mut tried_rhs = false;
    if prefer_rhs {
        if let (Some(rhs_ptr), Some(rhs_type_ptr), Some(rhs_raw)) =
            (rhs_ptr, rhs_type_ptr, rhs_rop_raw)
        {
            tried_rhs = true;
            match call_dunder_raw(rhs_raw, rhs_type_ptr, Some(rhs_ptr), lhs_bits) {
                BinaryDunderOutcome::Value(bits) => return Some(bits),
                BinaryDunderOutcome::Error => return Some(MoltObject::none().bits()),
                BinaryDunderOutcome::NotImplemented | BinaryDunderOutcome::Missing => {}
            }
        }
    }

    if let (Some(lhs_ptr), Some(lhs_type_ptr), Some(lhs_raw)) = (lhs_ptr, lhs_type_ptr, lhs_op_raw)
    {
        match call_dunder_raw(lhs_raw, lhs_type_ptr, Some(lhs_ptr), rhs_bits) {
            BinaryDunderOutcome::Value(bits) => return Some(bits),
            BinaryDunderOutcome::Error => return Some(MoltObject::none().bits()),
            BinaryDunderOutcome::NotImplemented | BinaryDunderOutcome::Missing => {}
        }
    }

    if !tried_rhs {
        if let (Some(rhs_ptr), Some(rhs_type_ptr), Some(rhs_raw)) =
            (rhs_ptr, rhs_type_ptr, rhs_rop_raw)
        {
            match call_dunder_raw(rhs_raw, rhs_type_ptr, Some(rhs_ptr), lhs_bits) {
                BinaryDunderOutcome::Value(bits) => return Some(bits),
                BinaryDunderOutcome::Error => return Some(MoltObject::none().bits()),
                BinaryDunderOutcome::NotImplemented | BinaryDunderOutcome::Missing => {}
            }
        }
    }
    None
}

unsafe fn call_inplace_dunder(lhs_bits: u64, rhs_bits: u64, op_name_bits: u64) -> Option<u64> {
    if let Some(lhs_ptr) = obj_from_bits(lhs_bits).as_ptr() {
        if let Some(call_bits) = attr_lookup_ptr(lhs_ptr, op_name_bits) {
            let res_bits = call_callable1(call_bits, rhs_bits);
            dec_ref_bits(call_bits);
            if exception_pending() {
                dec_ref_bits(res_bits);
                return Some(MoltObject::none().bits());
            }
            if !is_not_implemented_bits(res_bits) {
                return Some(res_bits);
            }
            dec_ref_bits(res_bits);
        }
        if exception_pending() {
            return Some(MoltObject::none().bits());
        }
    }
    None
}

fn obj_eq(lhs: MoltObject, rhs: MoltObject) -> bool {
    if let (Some(li), Some(ri)) = (to_i64(lhs), to_i64(rhs)) {
        return li == ri;
    }
    if lhs.is_none() && rhs.is_none() {
        return true;
    }
    if lhs.is_float() || rhs.is_float() {
        if let (Some(lf), Some(rf)) = (to_f64(lhs), to_f64(rhs)) {
            return lf == rf;
        }
    }
    if let (Some(l_big), Some(r_big)) = (to_bigint(lhs), to_bigint(rhs)) {
        return l_big == r_big;
    }
    if let (Some(lp), Some(rp)) = (
        maybe_ptr_from_bits(lhs.bits()),
        maybe_ptr_from_bits(rhs.bits()),
    ) {
        unsafe {
            let ltype = object_type_id(lp);
            let rtype = object_type_id(rp);
            if ltype != rtype {
                if (ltype == TYPE_ID_BYTES && rtype == TYPE_ID_BYTEARRAY)
                    || (ltype == TYPE_ID_BYTEARRAY && rtype == TYPE_ID_BYTES)
                {
                    let l_len = bytes_len(lp);
                    let r_len = bytes_len(rp);
                    if l_len != r_len {
                        return false;
                    }
                    let l_bytes = std::slice::from_raw_parts(bytes_data(lp), l_len);
                    let r_bytes = std::slice::from_raw_parts(bytes_data(rp), r_len);
                    return l_bytes == r_bytes;
                }
                if is_set_like_type(ltype) && is_set_like_type(rtype) {
                    let l_elems = set_order(lp);
                    let r_elems = set_order(rp);
                    if l_elems.len() != r_elems.len() {
                        return false;
                    }
                    let r_table = set_table(rp);
                    for key_bits in l_elems.iter().copied() {
                        if set_find_entry(r_elems, r_table, key_bits).is_none() {
                            return false;
                        }
                    }
                    return true;
                }
                return false;
            }
            if ltype == TYPE_ID_STRING {
                let l_len = string_len(lp);
                let r_len = string_len(rp);
                if l_len != r_len {
                    return false;
                }
                let l_bytes = std::slice::from_raw_parts(string_bytes(lp), l_len);
                let r_bytes = std::slice::from_raw_parts(string_bytes(rp), r_len);
                return l_bytes == r_bytes;
            }
            if ltype == TYPE_ID_BYTES || ltype == TYPE_ID_BYTEARRAY {
                let l_len = bytes_len(lp);
                let r_len = bytes_len(rp);
                if l_len != r_len {
                    return false;
                }
                let l_bytes = std::slice::from_raw_parts(bytes_data(lp), l_len);
                let r_bytes = std::slice::from_raw_parts(bytes_data(rp), r_len);
                return l_bytes == r_bytes;
            }
            if ltype == TYPE_ID_TUPLE {
                let l_elems = seq_vec_ref(lp);
                let r_elems = seq_vec_ref(rp);
                if l_elems.len() != r_elems.len() {
                    return false;
                }
                for (l_val, r_val) in l_elems.iter().zip(r_elems.iter()) {
                    if !obj_eq(obj_from_bits(*l_val), obj_from_bits(*r_val)) {
                        return false;
                    }
                }
                return true;
            }
            if ltype == TYPE_ID_SLICE {
                let l_start = slice_start_bits(lp);
                let l_stop = slice_stop_bits(lp);
                let l_step = slice_step_bits(lp);
                let r_start = slice_start_bits(rp);
                let r_stop = slice_stop_bits(rp);
                let r_step = slice_step_bits(rp);
                if !obj_eq(obj_from_bits(l_start), obj_from_bits(r_start)) {
                    return false;
                }
                if !obj_eq(obj_from_bits(l_stop), obj_from_bits(r_stop)) {
                    return false;
                }
                if !obj_eq(obj_from_bits(l_step), obj_from_bits(r_step)) {
                    return false;
                }
                return true;
            }
            if ltype == TYPE_ID_GENERIC_ALIAS {
                let l_origin = generic_alias_origin_bits(lp);
                let l_args = generic_alias_args_bits(lp);
                let r_origin = generic_alias_origin_bits(rp);
                let r_args = generic_alias_args_bits(rp);
                return obj_eq(obj_from_bits(l_origin), obj_from_bits(r_origin))
                    && obj_eq(obj_from_bits(l_args), obj_from_bits(r_args));
            }
            if ltype == TYPE_ID_LIST {
                let l_elems = seq_vec_ref(lp);
                let r_elems = seq_vec_ref(rp);
                if l_elems.len() != r_elems.len() {
                    return false;
                }
                for (l_val, r_val) in l_elems.iter().zip(r_elems.iter()) {
                    if !obj_eq(obj_from_bits(*l_val), obj_from_bits(*r_val)) {
                        return false;
                    }
                }
                return true;
            }
            if ltype == TYPE_ID_DICT {
                let l_pairs = dict_order(lp);
                let r_pairs = dict_order(rp);
                if l_pairs.len() != r_pairs.len() {
                    return false;
                }
                let r_table = dict_table(rp);
                let entries = l_pairs.len() / 2;
                for entry_idx in 0..entries {
                    let key_bits = l_pairs[entry_idx * 2];
                    let val_bits = l_pairs[entry_idx * 2 + 1];
                    let Some(r_entry_idx) = dict_find_entry(r_pairs, r_table, key_bits) else {
                        return false;
                    };
                    let r_val_bits = r_pairs[r_entry_idx * 2 + 1];
                    if !obj_eq(obj_from_bits(val_bits), obj_from_bits(r_val_bits)) {
                        return false;
                    }
                }
                return true;
            }
            if ltype == TYPE_ID_SET || ltype == TYPE_ID_FROZENSET {
                let l_elems = set_order(lp);
                let r_elems = set_order(rp);
                if l_elems.len() != r_elems.len() {
                    return false;
                }
                let r_table = set_table(rp);
                for key_bits in l_elems.iter().copied() {
                    if set_find_entry(r_elems, r_table, key_bits).is_none() {
                        return false;
                    }
                }
                return true;
            }
            if ltype == TYPE_ID_DATACLASS {
                let l_desc = dataclass_desc_ptr(lp);
                let r_desc = dataclass_desc_ptr(rp);
                if l_desc.is_null() || r_desc.is_null() {
                    return false;
                }
                let l_desc = &*l_desc;
                let r_desc = &*r_desc;
                if !l_desc.eq || !r_desc.eq {
                    return lp == rp;
                }
                if l_desc.name != r_desc.name || l_desc.field_names != r_desc.field_names {
                    return false;
                }
                let l_vals = dataclass_fields_ref(lp);
                let r_vals = dataclass_fields_ref(rp);
                if l_vals.len() != r_vals.len() {
                    return false;
                }
                for (l_val, r_val) in l_vals.iter().zip(r_vals.iter()) {
                    if !obj_eq(obj_from_bits(*l_val), obj_from_bits(*r_val)) {
                        return false;
                    }
                }
                return true;
            }
        }
        return lp == rp;
    }
    false
}

#[inline]
unsafe fn call_poll_fn(poll_fn_addr: u64, task_ptr: *mut u8) -> i64 {
    let addr = task_ptr.expose_provenance() as u64;
    #[cfg(target_arch = "wasm32")]
    {
        if std::env::var("MOLT_WASM_POLL_DEBUG").as_deref() == Ok("1") {
            eprintln!("molt wasm poll: fn=0x{poll_fn_addr:x}");
        }
        if poll_fn_addr < WASM_TABLE_BASE {
            return raise_exception::<i64>("RuntimeError", "invalid wasm poll function");
        }
        return molt_call_indirect1(poll_fn_addr, addr);
    }
    #[cfg(not(target_arch = "wasm32"))]
    {
        let poll_fn: extern "C" fn(u64) -> i64 = std::mem::transmute(poll_fn_addr as usize);
        poll_fn(addr)
    }
}

unsafe fn poll_future_with_task_stack(task_ptr: *mut u8, poll_fn_addr: u64) -> i64 {
    let prev_task = CURRENT_TASK.with(|cell| {
        let prev = cell.get();
        cell.set(task_ptr);
        prev
    });
    let caller_depth = exception_stack_depth();
    let caller_handlers = EXCEPTION_STACK.with(|stack| std::mem::take(&mut *stack.borrow_mut()));
    let caller_active =
        ACTIVE_EXCEPTION_STACK.with(|stack| std::mem::take(&mut *stack.borrow_mut()));
    let caller_context = caller_active
        .last()
        .copied()
        .unwrap_or(MoltObject::none().bits());
    exception_context_fallback_push(caller_context);
    let task_handlers = task_exception_handler_stack_take(task_ptr);
    EXCEPTION_STACK.with(|stack| {
        *stack.borrow_mut() = task_handlers;
    });
    let task_active = task_exception_stack_take(task_ptr);
    ACTIVE_EXCEPTION_STACK.with(|stack| {
        *stack.borrow_mut() = task_active;
    });
    let task_depth = task_exception_depth_take(task_ptr);
    exception_stack_set_depth(task_depth);
    let prev_raise = task_raise_active();
    set_task_raise_active(true);
    let res = call_poll_fn(poll_fn_addr, task_ptr);
    set_task_raise_active(prev_raise);
    let new_depth = exception_stack_depth();
    task_exception_depth_store(task_ptr, new_depth);
    exception_context_align_depth(new_depth);
    let task_handlers = EXCEPTION_STACK.with(|stack| std::mem::take(&mut *stack.borrow_mut()));
    task_exception_handler_stack_store(task_ptr, task_handlers);
    let task_active = ACTIVE_EXCEPTION_STACK.with(|stack| std::mem::take(&mut *stack.borrow_mut()));
    task_exception_stack_store(task_ptr, task_active);
    ACTIVE_EXCEPTION_STACK.with(|stack| {
        *stack.borrow_mut() = caller_active;
    });
    EXCEPTION_STACK.with(|stack| {
        *stack.borrow_mut() = caller_handlers;
    });
    exception_stack_set_depth(caller_depth);
    exception_context_fallback_pop();
    CURRENT_TASK.with(|cell| cell.set(prev_task));
    res
}

// TODO(semantics, owner:runtime, milestone:TC2, priority:P1, status:partial): move dict
// subclass storage out of instance __dict__ so mapping contents are not exposed via attributes.
unsafe fn dict_subclass_storage_bits(ptr: *mut u8) -> Option<u64> {
    let class_bits = object_class_bits(ptr);
    if class_bits == 0 {
        return None;
    }
    let builtins = builtin_classes();
    if !issubclass_bits(class_bits, builtins.dict) {
        return None;
    }
    let mut dict_bits = instance_dict_bits(ptr);
    if dict_bits == 0 {
        let dict_ptr = alloc_dict_with_pairs(&[]);
        if dict_ptr.is_null() {
            return None;
        }
        dict_bits = MoltObject::from_ptr(dict_ptr).bits();
        instance_set_dict_bits(ptr, dict_bits);
    }
    let dict_ptr = obj_from_bits(dict_bits).as_ptr()?;
    if object_type_id(dict_ptr) != TYPE_ID_DICT {
        return None;
    }
    let storage_name_bits = intern_static_name(
        &runtime_state().interned.molt_dict_data_name,
        b"__molt_dict_data__",
    );
    if let Some(storage_bits) = dict_get_in_place(dict_ptr, storage_name_bits) {
        return Some(storage_bits);
    }
    let storage_ptr = alloc_dict_with_pairs(&[]);
    if storage_ptr.is_null() {
        return None;
    }
    let storage_bits = MoltObject::from_ptr(storage_ptr).bits();
    dict_set_in_place(dict_ptr, storage_name_bits, storage_bits);
    if exception_pending() {
        dec_ref_bits(storage_bits);
        return None;
    }
    dec_ref_bits(storage_bits);
    dict_get_in_place(dict_ptr, storage_name_bits)
}

unsafe fn dict_like_bits_from_ptr(ptr: *mut u8) -> Option<u64> {
    if object_type_id(ptr) == TYPE_ID_DICT {
        return Some(MoltObject::from_ptr(ptr).bits());
    }
    if object_type_id(ptr) == TYPE_ID_OBJECT {
        return dict_subclass_storage_bits(ptr);
    }
    None
}

fn memoryview_format_from_str(format: &str) -> Option<MemoryViewFormat> {
    let code = if format.len() == 1 {
        format.as_bytes()[0]
    } else if format.len() == 2 && format.as_bytes()[0] == b'@' {
        format.as_bytes()[1]
    } else {
        return None;
    };
    let (itemsize, kind) = match code {
        b'b' => (1, MemoryViewFormatKind::Signed),
        b'B' => (1, MemoryViewFormatKind::Unsigned),
        b'h' => (2, MemoryViewFormatKind::Signed),
        b'H' => (2, MemoryViewFormatKind::Unsigned),
        b'i' => (4, MemoryViewFormatKind::Signed),
        b'I' => (4, MemoryViewFormatKind::Unsigned),
        b'l' => (
            std::mem::size_of::<libc::c_long>(),
            MemoryViewFormatKind::Signed,
        ),
        b'L' => (
            std::mem::size_of::<libc::c_long>(),
            MemoryViewFormatKind::Unsigned,
        ),
        b'q' => (8, MemoryViewFormatKind::Signed),
        b'Q' => (8, MemoryViewFormatKind::Unsigned),
        b'n' => (std::mem::size_of::<isize>(), MemoryViewFormatKind::Signed),
        b'N' => (std::mem::size_of::<isize>(), MemoryViewFormatKind::Unsigned),
        b'P' => (
            std::mem::size_of::<*const u8>(),
            MemoryViewFormatKind::Unsigned,
        ),
        b'f' => (4, MemoryViewFormatKind::Float),
        b'd' => (8, MemoryViewFormatKind::Float),
        b'?' => (1, MemoryViewFormatKind::Bool),
        b'c' => (1, MemoryViewFormatKind::Char),
        _ => return None,
    };
    Some(MemoryViewFormat {
        code,
        itemsize,
        kind,
    })
}

fn memoryview_format_from_bits(bits: u64) -> Option<MemoryViewFormat> {
    let format = string_obj_to_owned(obj_from_bits(bits))?;
    memoryview_format_from_str(&format)
}

fn memoryview_shape_product(shape: &[isize]) -> Option<i128> {
    let mut total: i128 = 1;
    for &dim in shape {
        if dim < 0 {
            return None;
        }
        let dim_val: i128 = dim as i128;
        total = total.checked_mul(dim_val)?;
    }
    Some(total)
}

fn memoryview_nbytes_big(shape: &[isize], itemsize: usize) -> Option<i128> {
    let total = memoryview_shape_product(shape)?;
    let itemsize = i128::try_from(itemsize).ok()?;
    total.checked_mul(itemsize)
}

fn memoryview_is_c_contiguous(shape: &[isize], strides: &[isize], itemsize: usize) -> bool {
    if shape.len() != strides.len() {
        return false;
    }
    let mut expected = itemsize as isize;
    for idx in (0..shape.len()).rev() {
        let dim = shape[idx];
        let stride = strides[idx];
        if dim > 1 && stride != expected {
            return false;
        }
        expected = expected.saturating_mul(dim.max(1));
    }
    true
}

unsafe fn memoryview_is_c_contiguous_view(ptr: *mut u8) -> bool {
    let shape = memoryview_shape(ptr).unwrap_or(&[]);
    let strides = memoryview_strides(ptr).unwrap_or(&[]);
    memoryview_is_c_contiguous(shape, strides, memoryview_itemsize(ptr))
}

unsafe fn memoryview_nbytes(ptr: *mut u8) -> usize {
    let shape = memoryview_shape(ptr).unwrap_or(&[]);
    let itemsize = memoryview_itemsize(ptr);
    if let Some(total) = memoryview_nbytes_big(shape, itemsize) {
        if total >= 0 && total <= usize::MAX as i128 {
            return total as usize;
        }
    }
    0
}

fn tuple_from_isize_slice(values: &[isize]) -> u64 {
    let mut elems = Vec::with_capacity(values.len());
    for &val in values {
        elems.push(MoltObject::from_int(val as i64).bits());
    }
    let ptr = alloc_tuple(&elems);
    if ptr.is_null() {
        MoltObject::none().bits()
    } else {
        MoltObject::from_ptr(ptr).bits()
    }
}

unsafe fn bytes_like_slice_raw(ptr: *mut u8) -> Option<&'static [u8]> {
    let type_id = object_type_id(ptr);
    if type_id == TYPE_ID_BYTES || type_id == TYPE_ID_BYTEARRAY {
        let len = bytes_len(ptr);
        let data = bytes_data(ptr);
        return Some(std::slice::from_raw_parts(data, len));
    }
    None
}

unsafe fn bytes_like_slice_raw_mut(ptr: *mut u8) -> Option<&'static mut [u8]> {
    let type_id = object_type_id(ptr);
    if type_id == TYPE_ID_BYTEARRAY {
        let vec = bytearray_vec(ptr);
        return Some(vec.as_mut_slice());
    }
    None
}

unsafe fn memoryview_bytes_slice(ptr: *mut u8) -> Option<&'static [u8]> {
    if memoryview_itemsize(ptr) != 1 || memoryview_stride(ptr) != 1 {
        return None;
    }
    let owner_bits = memoryview_owner_bits(ptr);
    let owner = obj_from_bits(owner_bits);
    let owner_ptr = owner.as_ptr()?;
    let base = bytes_like_slice_raw(owner_ptr)?;
    let offset = memoryview_offset(ptr);
    if offset < 0 {
        return None;
    }
    let offset = offset as usize;
    let len = memoryview_len(ptr);
    if offset > base.len() || offset + len > base.len() {
        return None;
    }
    Some(&base[offset..offset + len])
}

unsafe fn memoryview_bytes_slice_mut(ptr: *mut u8) -> Option<&'static mut [u8]> {
    if memoryview_itemsize(ptr) != 1 || memoryview_stride(ptr) != 1 {
        return None;
    }
    let owner_bits = memoryview_owner_bits(ptr);
    let owner = obj_from_bits(owner_bits);
    let owner_ptr = owner.as_ptr()?;
    let base = bytes_like_slice_raw_mut(owner_ptr)?;
    let offset = memoryview_offset(ptr);
    if offset < 0 {
        return None;
    }
    let offset = offset as usize;
    let len = memoryview_len(ptr);
    if offset > base.len() || offset + len > base.len() {
        return None;
    }
    Some(&mut base[offset..offset + len])
}

unsafe fn memoryview_write_bytes(ptr: *mut u8, data: &[u8]) -> Result<usize, String> {
    if memoryview_readonly(ptr) {
        return Err("memoryview is readonly".to_string());
    }
    if let Some(slice) = memoryview_bytes_slice_mut(ptr) {
        let n = data.len().min(slice.len());
        slice[..n].copy_from_slice(&data[..n]);
        return Ok(n);
    }
    let owner_bits = memoryview_owner_bits(ptr);
    let owner = obj_from_bits(owner_bits);
    let owner_ptr = owner
        .as_ptr()
        .ok_or_else(|| "invalid memoryview owner".to_string())?;
    let base =
        bytes_like_slice_raw_mut(owner_ptr).ok_or_else(|| "unsupported buffer".to_string())?;
    let shape = memoryview_shape(ptr).ok_or_else(|| "invalid memoryview shape".to_string())?;
    let strides =
        memoryview_strides(ptr).ok_or_else(|| "invalid memoryview strides".to_string())?;
    if shape.len() != strides.len() {
        return Err("invalid memoryview strides".to_string());
    }
    let itemsize = memoryview_itemsize(ptr);
    let total_bytes = memoryview_nbytes_big(shape, itemsize)
        .ok_or_else(|| "invalid memoryview size".to_string())?;
    if total_bytes < 0 {
        return Err("invalid memoryview size".to_string());
    }
    let total_bytes = total_bytes as usize;
    let write_bytes = data.len().min(total_bytes);
    let offset = memoryview_offset(ptr);
    if offset < 0 {
        return Err("invalid memoryview offset".to_string());
    }
    if memoryview_is_c_contiguous(shape, strides, itemsize) {
        let start = offset as usize;
        let end = start.saturating_add(write_bytes);
        if end > base.len() {
            return Err("memoryview out of bounds".to_string());
        }
        base[start..end].copy_from_slice(&data[..write_bytes]);
        return Ok(write_bytes);
    }
    let total =
        memoryview_shape_product(shape).ok_or_else(|| "invalid memoryview shape".to_string())?;
    if total < 0 {
        return Err("invalid memoryview shape".to_string());
    }
    let total = total as usize;
    let mut indices = vec![0isize; shape.len()];
    let mut written = 0usize;
    for _ in 0..total {
        if written >= write_bytes {
            break;
        }
        let mut pos = offset;
        for (idx, stride) in indices.iter().zip(strides.iter()) {
            pos = pos
                .checked_add(idx.saturating_mul(*stride))
                .ok_or_else(|| "memoryview out of bounds".to_string())?;
        }
        if pos < 0 {
            return Err("memoryview out of bounds".to_string());
        }
        let start = pos as usize;
        let remaining = write_bytes - written;
        let copy_len = itemsize.min(remaining);
        let end = start.saturating_add(copy_len);
        if end > base.len() {
            return Err("memoryview out of bounds".to_string());
        }
        base[start..end].copy_from_slice(&data[written..written + copy_len]);
        written += copy_len;
        for dim in (0..indices.len()).rev() {
            indices[dim] += 1;
            if indices[dim] < shape[dim] {
                break;
            }
            indices[dim] = 0;
        }
    }
    Ok(written)
}

unsafe fn memoryview_collect_bytes(ptr: *mut u8) -> Option<Vec<u8>> {
    let owner_bits = memoryview_owner_bits(ptr);
    let owner = obj_from_bits(owner_bits);
    let owner_ptr = owner.as_ptr()?;
    let base = bytes_like_slice_raw(owner_ptr)?;
    let shape = memoryview_shape(ptr)?;
    let strides = memoryview_strides(ptr)?;
    if shape.len() != strides.len() {
        return None;
    }
    let nbytes = memoryview_nbytes_big(shape, memoryview_itemsize(ptr))?;
    if nbytes < 0 || nbytes > base.len() as i128 {
        return None;
    }
    let nbytes = nbytes as usize;
    let offset = memoryview_offset(ptr);
    if offset < 0 {
        return None;
    }
    let mut out = Vec::with_capacity(nbytes);
    if memoryview_is_c_contiguous(shape, strides, memoryview_itemsize(ptr)) {
        let end = offset.checked_add(nbytes as isize)?;
        if end < 0 {
            return None;
        }
        let start = offset as usize;
        let end = end as usize;
        if end > base.len() {
            return None;
        }
        out.extend_from_slice(&base[start..end]);
        return Some(out);
    }
    let total = memoryview_shape_product(shape)?;
    if total < 0 {
        return None;
    }
    let total = total as usize;
    let mut indices = vec![0isize; shape.len()];
    for _ in 0..total {
        let mut pos = offset;
        for (idx, stride) in indices.iter().zip(strides.iter()) {
            pos = pos.checked_add(idx.saturating_mul(*stride))?;
        }
        if pos < 0 {
            return None;
        }
        let pos = pos as usize;
        let itemsize = memoryview_itemsize(ptr);
        if pos + itemsize > base.len() {
            return None;
        }
        out.extend_from_slice(&base[pos..pos + itemsize]);
        for axis in (0..indices.len()).rev() {
            indices[axis] += 1;
            if indices[axis] < shape[axis] {
                break;
            }
            indices[axis] = 0;
        }
    }
    Some(out)
}

unsafe fn memoryview_read_scalar(data: &[u8], offset: isize, fmt: MemoryViewFormat) -> Option<u64> {
    if offset < 0 {
        return None;
    }
    let offset = offset as usize;
    if offset + fmt.itemsize > data.len() {
        return None;
    }
    match fmt.kind {
        MemoryViewFormatKind::Char => {
            let ptr = alloc_bytes(&[data[offset]]);
            if ptr.is_null() {
                return None;
            }
            Some(MoltObject::from_ptr(ptr).bits())
        }
        MemoryViewFormatKind::Bool => Some(MoltObject::from_bool(data[offset] != 0).bits()),
        MemoryViewFormatKind::Float => {
            if fmt.itemsize == 4 {
                let bytes: [u8; 4] = data[offset..offset + 4].try_into().ok()?;
                let val = f32::from_ne_bytes(bytes) as f64;
                Some(MoltObject::from_float(val).bits())
            } else if fmt.itemsize == 8 {
                let bytes: [u8; 8] = data[offset..offset + 8].try_into().ok()?;
                let val = f64::from_ne_bytes(bytes);
                Some(MoltObject::from_float(val).bits())
            } else {
                None
            }
        }
        MemoryViewFormatKind::Signed => {
            let val = match fmt.itemsize {
                1 => i64::from(i8::from_ne_bytes([data[offset]])),
                2 => {
                    let bytes: [u8; 2] = data[offset..offset + 2].try_into().ok()?;
                    i64::from(i16::from_ne_bytes(bytes))
                }
                4 => {
                    let bytes: [u8; 4] = data[offset..offset + 4].try_into().ok()?;
                    i64::from(i32::from_ne_bytes(bytes))
                }
                8 => {
                    let bytes: [u8; 8] = data[offset..offset + 8].try_into().ok()?;
                    i64::from_ne_bytes(bytes)
                }
                _ => return None,
            };
            Some(MoltObject::from_int(val).bits())
        }
        MemoryViewFormatKind::Unsigned => {
            let val = match fmt.itemsize {
                1 => u64::from(data[offset]),
                2 => {
                    let bytes: [u8; 2] = data[offset..offset + 2].try_into().ok()?;
                    u64::from(u16::from_ne_bytes(bytes))
                }
                4 => {
                    let bytes: [u8; 4] = data[offset..offset + 4].try_into().ok()?;
                    u64::from(u32::from_ne_bytes(bytes))
                }
                8 => {
                    let bytes: [u8; 8] = data[offset..offset + 8].try_into().ok()?;
                    u64::from_ne_bytes(bytes)
                }
                _ => return None,
            };
            if val <= i64::MAX as u64 {
                Some(MoltObject::from_int(val as i64).bits())
            } else {
                Some(bigint_bits(BigInt::from(val)))
            }
        }
    }
}

unsafe fn memoryview_write_scalar(
    data: &mut [u8],
    offset: isize,
    fmt: MemoryViewFormat,
    val_bits: u64,
) -> Option<()> {
    if offset < 0 {
        return None;
    }
    let offset = offset as usize;
    if offset + fmt.itemsize > data.len() {
        return None;
    }
    match fmt.kind {
        MemoryViewFormatKind::Char => {
            let val_obj = obj_from_bits(val_bits);
            let Some(ptr) = val_obj.as_ptr() else {
                raise_exception::<u64>(
                    "TypeError",
                    &format!("memoryview: invalid type for format '{}'", fmt.code as char),
                );
                return None;
            };
            if object_type_id(ptr) != TYPE_ID_BYTES {
                raise_exception::<u64>(
                    "TypeError",
                    &format!("memoryview: invalid type for format '{}'", fmt.code as char),
                );
                return None;
            }
            let bytes = bytes_like_slice_raw(ptr).unwrap_or(&[]);
            if bytes.len() != 1 {
                raise_exception::<u64>(
                    "ValueError",
                    &format!(
                        "memoryview: invalid value for format '{}'",
                        fmt.code as char
                    ),
                );
                return None;
            }
            data[offset] = bytes[0];
            Some(())
        }
        MemoryViewFormatKind::Bool => {
            data[offset] = if is_truthy(obj_from_bits(val_bits)) {
                1
            } else {
                0
            };
            Some(())
        }
        MemoryViewFormatKind::Float => {
            let Some(val) = to_f64(obj_from_bits(val_bits)) else {
                raise_exception::<u64>(
                    "TypeError",
                    &format!("memoryview: invalid type for format '{}'", fmt.code as char),
                );
                return None;
            };
            if fmt.itemsize == 4 {
                let bytes = (val as f32).to_ne_bytes();
                data[offset..offset + 4].copy_from_slice(&bytes);
                return Some(());
            }
            if fmt.itemsize == 8 {
                let bytes = val.to_ne_bytes();
                data[offset..offset + 8].copy_from_slice(&bytes);
                return Some(());
            }
            None
        }
        MemoryViewFormatKind::Signed | MemoryViewFormatKind::Unsigned => {
            let err_msg = format!("memoryview: invalid type for format '{}'", fmt.code as char);
            let value = index_bigint_from_obj(val_bits, &err_msg)?;
            let bits = (fmt.itemsize * 8) as u32;
            let (min, max) = if fmt.kind == MemoryViewFormatKind::Signed {
                let limit = BigInt::from(1u64) << (bits - 1);
                (-limit.clone(), limit - 1)
            } else {
                (BigInt::from(0u8), (BigInt::from(1u64) << bits) - 1)
            };
            if value < min || value > max {
                raise_exception::<u64>(
                    "ValueError",
                    &format!(
                        "memoryview: invalid value for format '{}'",
                        fmt.code as char
                    ),
                );
                return None;
            }
            if fmt.kind == MemoryViewFormatKind::Signed {
                let val_i64 = value.to_i64().unwrap_or(0);
                let bytes = match fmt.itemsize {
                    1 => (val_i64 as i8).to_ne_bytes().to_vec(),
                    2 => (val_i64 as i16).to_ne_bytes().to_vec(),
                    4 => (val_i64 as i32).to_ne_bytes().to_vec(),
                    8 => val_i64.to_ne_bytes().to_vec(),
                    _ => return None,
                };
                data[offset..offset + fmt.itemsize].copy_from_slice(&bytes);
                return Some(());
            }
            let val_u64 = value.to_u64().unwrap_or(0);
            let bytes = match fmt.itemsize {
                1 => (val_u64 as u8).to_ne_bytes().to_vec(),
                2 => (val_u64 as u16).to_ne_bytes().to_vec(),
                4 => (val_u64 as u32).to_ne_bytes().to_vec(),
                8 => val_u64.to_ne_bytes().to_vec(),
                _ => return None,
            };
            data[offset..offset + fmt.itemsize].copy_from_slice(&bytes);
            Some(())
        }
    }
}

unsafe fn bytes_like_slice(ptr: *mut u8) -> Option<&'static [u8]> {
    let type_id = object_type_id(ptr);
    if type_id == TYPE_ID_MEMORYVIEW {
        return memoryview_bytes_slice(ptr);
    }
    bytes_like_slice_raw(ptr)
}

unsafe fn seq_vec_ptr(ptr: *mut u8) -> *mut Vec<u64> {
    *(ptr as *mut *mut Vec<u64>)
}

unsafe fn seq_vec(ptr: *mut u8) -> &'static mut Vec<u64> {
    let vec_ptr = seq_vec_ptr(ptr);
    &mut *vec_ptr
}

unsafe fn seq_vec_ref(ptr: *mut u8) -> &'static Vec<u64> {
    let vec_ptr = seq_vec_ptr(ptr);
    &*vec_ptr
}

unsafe fn bytearray_vec_ptr(ptr: *mut u8) -> *mut Vec<u8> {
    *(ptr as *mut *mut Vec<u8>)
}

unsafe fn bytearray_vec(ptr: *mut u8) -> &'static mut Vec<u8> {
    let vec_ptr = bytearray_vec_ptr(ptr);
    &mut *vec_ptr
}

unsafe fn bytearray_vec_ref(ptr: *mut u8) -> &'static Vec<u8> {
    let vec_ptr = bytearray_vec_ptr(ptr);
    &*vec_ptr
}

unsafe fn bytearray_len(ptr: *mut u8) -> usize {
    bytearray_vec_ref(ptr).len()
}

unsafe fn bytearray_data(ptr: *mut u8) -> *const u8 {
    bytearray_vec_ref(ptr).as_ptr()
}

unsafe fn iter_target_bits(ptr: *mut u8) -> u64 {
    *(ptr as *const u64)
}

unsafe fn iter_index(ptr: *mut u8) -> usize {
    *(ptr.add(std::mem::size_of::<u64>()) as *const usize)
}

unsafe fn iter_set_index(ptr: *mut u8, idx: usize) {
    *(ptr.add(std::mem::size_of::<u64>()) as *mut usize) = idx;
}

unsafe fn enumerate_target_bits(ptr: *mut u8) -> u64 {
    *(ptr as *const u64)
}

unsafe fn enumerate_index_bits(ptr: *mut u8) -> u64 {
    *(ptr.add(std::mem::size_of::<u64>()) as *const u64)
}

unsafe fn enumerate_set_index_bits(ptr: *mut u8, idx_bits: u64) {
    *(ptr.add(std::mem::size_of::<u64>()) as *mut u64) = idx_bits;
}

unsafe fn call_iter_callable_bits(ptr: *mut u8) -> u64 {
    *(ptr as *const u64)
}

unsafe fn call_iter_sentinel_bits(ptr: *mut u8) -> u64 {
    *(ptr.add(std::mem::size_of::<u64>()) as *const u64)
}

unsafe fn reversed_target_bits(ptr: *mut u8) -> u64 {
    *(ptr as *const u64)
}

unsafe fn reversed_index(ptr: *mut u8) -> usize {
    *(ptr.add(std::mem::size_of::<u64>()) as *const usize)
}

unsafe fn reversed_set_index(ptr: *mut u8, idx: usize) {
    *(ptr.add(std::mem::size_of::<u64>()) as *mut usize) = idx;
}

unsafe fn zip_iters_ptr(ptr: *mut u8) -> *mut Vec<u64> {
    *(ptr as *mut *mut Vec<u64>)
}

unsafe fn map_func_bits(ptr: *mut u8) -> u64 {
    *(ptr as *const u64)
}

unsafe fn map_iters_ptr(ptr: *mut u8) -> *mut Vec<u64> {
    *(ptr.add(std::mem::size_of::<u64>()) as *mut *mut Vec<u64>)
}

unsafe fn filter_func_bits(ptr: *mut u8) -> u64 {
    *(ptr as *const u64)
}

unsafe fn filter_iter_bits(ptr: *mut u8) -> u64 {
    *(ptr.add(std::mem::size_of::<u64>()) as *const u64)
}

unsafe fn range_start(ptr: *mut u8) -> i64 {
    *(ptr as *const i64)
}

unsafe fn range_stop(ptr: *mut u8) -> i64 {
    *(ptr.add(std::mem::size_of::<i64>()) as *const i64)
}

unsafe fn range_step(ptr: *mut u8) -> i64 {
    *(ptr.add(2 * std::mem::size_of::<i64>()) as *const i64)
}

unsafe fn slice_start_bits(ptr: *mut u8) -> u64 {
    *(ptr as *const u64)
}

unsafe fn slice_stop_bits(ptr: *mut u8) -> u64 {
    *(ptr.add(std::mem::size_of::<u64>()) as *const u64)
}

unsafe fn slice_step_bits(ptr: *mut u8) -> u64 {
    *(ptr.add(2 * std::mem::size_of::<u64>()) as *const u64)
}

unsafe fn generic_alias_origin_bits(ptr: *mut u8) -> u64 {
    *(ptr as *const u64)
}

unsafe fn generic_alias_args_bits(ptr: *mut u8) -> u64 {
    *(ptr.add(std::mem::size_of::<u64>()) as *const u64)
}

unsafe fn context_enter_fn(ptr: *mut u8) -> *const () {
    *(ptr as *const *const ())
}

unsafe fn context_exit_fn(ptr: *mut u8) -> *const () {
    *(ptr.add(std::mem::size_of::<*const ()>()) as *const *const ())
}

unsafe fn context_payload_bits(ptr: *mut u8) -> u64 {
    *(ptr.add(2 * std::mem::size_of::<*const ()>()) as *const u64)
}

#[allow(dead_code)]
pub(crate) unsafe fn function_fn_ptr(ptr: *mut u8) -> u64 {
    *(ptr as *const u64)
}

#[allow(dead_code)]
pub(crate) unsafe fn function_arity(ptr: *mut u8) -> u64 {
    *(ptr.add(std::mem::size_of::<u64>()) as *const u64)
}

#[allow(dead_code)]
unsafe fn function_dict_bits(ptr: *mut u8) -> u64 {
    *(ptr.add(2 * std::mem::size_of::<u64>()) as *const u64)
}

unsafe fn function_name_bits(ptr: *mut u8) -> u64 {
    let dict_bits = function_dict_bits(ptr);
    if dict_bits != 0 {
        if let Some(dict_ptr) = obj_from_bits(dict_bits).as_ptr() {
            if object_type_id(dict_ptr) == TYPE_ID_DICT {
                let qual_bits =
                    intern_static_name(&runtime_state().interned.qualname_name, b"__qualname__");
                if let Some(bits) = dict_get_in_place(dict_ptr, qual_bits) {
                    return bits;
                }
                let name_bits =
                    intern_static_name(&runtime_state().interned.name_name, b"__name__");
                if let Some(bits) = dict_get_in_place(dict_ptr, name_bits) {
                    return bits;
                }
            }
        }
    }
    MoltObject::none().bits()
}

unsafe fn function_set_dict_bits(ptr: *mut u8, bits: u64) {
    *(ptr.add(2 * std::mem::size_of::<u64>()) as *mut u64) = bits;
}

pub(crate) unsafe fn function_closure_bits(ptr: *mut u8) -> u64 {
    *(ptr.add(3 * std::mem::size_of::<u64>()) as *const u64)
}

unsafe fn function_set_closure_bits(ptr: *mut u8, bits: u64) {
    *(ptr.add(3 * std::mem::size_of::<u64>()) as *mut u64) = bits;
    if bits != 0 {
        inc_ref_bits(bits);
    }
}

unsafe fn function_code_bits(ptr: *mut u8) -> u64 {
    *(ptr.add(4 * std::mem::size_of::<u64>()) as *const u64)
}

#[allow(dead_code)]
pub(crate) unsafe fn function_trampoline_ptr(ptr: *mut u8) -> u64 {
    *(ptr.add(5 * std::mem::size_of::<u64>()) as *const u64)
}

unsafe fn function_annotations_bits(ptr: *mut u8) -> u64 {
    *(ptr.add(6 * std::mem::size_of::<u64>()) as *const u64)
}

unsafe fn function_set_annotations_bits(ptr: *mut u8, bits: u64) {
    let slot = ptr.add(6 * std::mem::size_of::<u64>()) as *mut u64;
    let old_bits = *slot;
    if old_bits != 0 {
        dec_ref_bits(old_bits);
    }
    *slot = bits;
    if bits != 0 {
        inc_ref_bits(bits);
    }
}

unsafe fn function_annotate_bits(ptr: *mut u8) -> u64 {
    *(ptr.add(7 * std::mem::size_of::<u64>()) as *const u64)
}

unsafe fn function_set_annotate_bits(ptr: *mut u8, bits: u64) {
    let slot = ptr.add(7 * std::mem::size_of::<u64>()) as *mut u64;
    let old_bits = *slot;
    if old_bits != 0 {
        dec_ref_bits(old_bits);
    }
    *slot = bits;
    if bits != 0 {
        inc_ref_bits(bits);
    }
}

unsafe fn function_set_code_bits(ptr: *mut u8, bits: u64) {
    let slot = ptr.add(4 * std::mem::size_of::<u64>()) as *mut u64;
    let old_bits = *slot;
    if old_bits != bits {
        if old_bits != 0 {
            dec_ref_bits(old_bits);
        }
        if bits != 0 {
            inc_ref_bits(bits);
        }
        *slot = bits;
    }
    let fn_ptr = function_fn_ptr(ptr);
    fn_ptr_code_set(fn_ptr, bits);
}

unsafe fn function_set_trampoline_ptr(ptr: *mut u8, bits: u64) {
    *(ptr.add(5 * std::mem::size_of::<u64>()) as *mut u64) = bits;
}

pub(crate) unsafe fn ensure_function_code_bits(func_ptr: *mut u8) -> u64 {
    let existing = function_code_bits(func_ptr);
    if existing != 0 {
        return existing;
    }
    let mut name_bits = function_name_bits(func_ptr);
    let mut owned_name = false;
    let name_ok = if let Some(name_ptr) = obj_from_bits(name_bits).as_ptr() {
        object_type_id(name_ptr) == TYPE_ID_STRING
    } else {
        false
    };
    if !name_ok {
        let name_ptr = alloc_string(b"<unknown>");
        if name_ptr.is_null() {
            return MoltObject::none().bits();
        }
        name_bits = MoltObject::from_ptr(name_ptr).bits();
        owned_name = true;
    }
    let filename_ptr = alloc_string(b"<molt-builtin>");
    if filename_ptr.is_null() {
        if owned_name {
            dec_ref_bits(name_bits);
        }
        return MoltObject::none().bits();
    }
    let filename_bits = MoltObject::from_ptr(filename_ptr).bits();
    let code_ptr = alloc_code_obj(filename_bits, name_bits, 0, MoltObject::none().bits());
    dec_ref_bits(filename_bits);
    if owned_name {
        dec_ref_bits(name_bits);
    }
    if code_ptr.is_null() {
        return MoltObject::none().bits();
    }
    let code_bits = MoltObject::from_ptr(code_ptr).bits();
    function_set_code_bits(func_ptr, code_bits);
    code_bits
}

unsafe fn code_filename_bits(ptr: *mut u8) -> u64 {
    *(ptr as *const u64)
}

unsafe fn code_name_bits(ptr: *mut u8) -> u64 {
    *(ptr.add(std::mem::size_of::<u64>()) as *const u64)
}

unsafe fn code_firstlineno(ptr: *mut u8) -> i64 {
    *(ptr.add(2 * std::mem::size_of::<u64>()) as *const i64)
}

unsafe fn code_linetable_bits(ptr: *mut u8) -> u64 {
    *(ptr.add(3 * std::mem::size_of::<u64>()) as *const u64)
}

pub(crate) unsafe fn bound_method_func_bits(ptr: *mut u8) -> u64 {
    *(ptr as *const u64)
}

pub(crate) unsafe fn bound_method_self_bits(ptr: *mut u8) -> u64 {
    *(ptr.add(std::mem::size_of::<u64>()) as *const u64)
}

unsafe fn module_name_bits(ptr: *mut u8) -> u64 {
    *(ptr as *const u64)
}

unsafe fn module_dict_bits(ptr: *mut u8) -> u64 {
    *(ptr.add(std::mem::size_of::<u64>()) as *const u64)
}

pub(crate) unsafe fn class_name_bits(ptr: *mut u8) -> u64 {
    *(ptr as *const u64)
}

unsafe fn class_dict_bits(ptr: *mut u8) -> u64 {
    *(ptr.add(std::mem::size_of::<u64>()) as *const u64)
}

unsafe fn class_bases_bits(ptr: *mut u8) -> u64 {
    *(ptr.add(2 * std::mem::size_of::<u64>()) as *const u64)
}

unsafe fn class_set_bases_bits(ptr: *mut u8, bits: u64) {
    *(ptr.add(2 * std::mem::size_of::<u64>()) as *mut u64) = bits;
}

unsafe fn class_mro_bits(ptr: *mut u8) -> u64 {
    *(ptr.add(3 * std::mem::size_of::<u64>()) as *const u64)
}

unsafe fn class_set_mro_bits(ptr: *mut u8, bits: u64) {
    *(ptr.add(3 * std::mem::size_of::<u64>()) as *mut u64) = bits;
}

unsafe fn class_layout_version_bits(ptr: *mut u8) -> u64 {
    *(ptr.add(4 * std::mem::size_of::<u64>()) as *const u64)
}

unsafe fn class_set_layout_version_bits(ptr: *mut u8, bits: u64) {
    *(ptr.add(4 * std::mem::size_of::<u64>()) as *mut u64) = bits;
}

unsafe fn class_annotations_bits(ptr: *mut u8) -> u64 {
    *(ptr.add(5 * std::mem::size_of::<u64>()) as *const u64)
}

unsafe fn class_set_annotations_bits(ptr: *mut u8, bits: u64) {
    let slot = ptr.add(5 * std::mem::size_of::<u64>()) as *mut u64;
    let old_bits = *slot;
    if old_bits != 0 {
        dec_ref_bits(old_bits);
    }
    *slot = bits;
    if bits != 0 {
        inc_ref_bits(bits);
    }
}

unsafe fn class_annotate_bits(ptr: *mut u8) -> u64 {
    *(ptr.add(6 * std::mem::size_of::<u64>()) as *const u64)
}

unsafe fn class_set_annotate_bits(ptr: *mut u8, bits: u64) {
    let slot = ptr.add(6 * std::mem::size_of::<u64>()) as *mut u64;
    let old_bits = *slot;
    if old_bits != 0 {
        dec_ref_bits(old_bits);
    }
    *slot = bits;
    if bits != 0 {
        inc_ref_bits(bits);
    }
}

fn class_break_cycles(bits: u64) {
    let Some(ptr) = obj_from_bits(bits).as_ptr() else {
        return;
    };
    unsafe {
        if object_type_id(ptr) != TYPE_ID_TYPE {
            return;
        }
        let none_bits = MoltObject::none().bits();
        let bases_bits = class_bases_bits(ptr);
        let mro_bits = class_mro_bits(ptr);
        if !obj_from_bits(bases_bits).is_none() {
            dec_ref_bits(bases_bits);
        }
        if !obj_from_bits(mro_bits).is_none() {
            dec_ref_bits(mro_bits);
        }
        class_set_bases_bits(ptr, none_bits);
        class_set_mro_bits(ptr, none_bits);
        class_set_annotations_bits(ptr, 0);
        class_set_annotate_bits(ptr, 0);
        let dict_bits = class_dict_bits(ptr);
        if let Some(dict_ptr) = obj_from_bits(dict_bits).as_ptr() {
            if object_type_id(dict_ptr) == TYPE_ID_DICT {
                dict_clear_in_place(dict_ptr);
            }
        }
    }
}

unsafe fn class_bump_layout_version(ptr: *mut u8) {
    let current = class_layout_version_bits(ptr);
    class_set_layout_version_bits(ptr, current.wrapping_add(1));
}

unsafe fn classmethod_func_bits(ptr: *mut u8) -> u64 {
    *(ptr as *const u64)
}

unsafe fn staticmethod_func_bits(ptr: *mut u8) -> u64 {
    *(ptr as *const u64)
}

unsafe fn property_get_bits(ptr: *mut u8) -> u64 {
    *(ptr as *const u64)
}

unsafe fn property_set_bits(ptr: *mut u8) -> u64 {
    *(ptr.add(std::mem::size_of::<u64>()) as *const u64)
}

unsafe fn property_del_bits(ptr: *mut u8) -> u64 {
    *(ptr.add(2 * std::mem::size_of::<u64>()) as *const u64)
}

unsafe fn super_type_bits(ptr: *mut u8) -> u64 {
    *(ptr as *const u64)
}

unsafe fn super_obj_bits(ptr: *mut u8) -> u64 {
    *(ptr.add(std::mem::size_of::<u64>()) as *const u64)
}

fn range_len_i64(start: i64, stop: i64, step: i64) -> i64 {
    if step == 0 {
        return 0;
    }
    if step > 0 {
        if start >= stop {
            return 0;
        }
        let span = stop - start - 1;
        return 1 + span / step;
    }
    if start <= stop {
        return 0;
    }
    let step_abs = -step;
    let span = start - stop - 1;
    1 + span / step_abs
}

#[derive(Clone, Copy)]
struct HashSecret {
    k0: u64,
    k1: u64,
}

static HASH_MODULUS_BIG: OnceLock<BigInt> = OnceLock::new();

fn hash_modulus_big() -> &'static BigInt {
    HASH_MODULUS_BIG.get_or_init(|| BigInt::from(PY_HASH_MODULUS))
}

fn hash_secret() -> &'static HashSecret {
    runtime_state().hash_secret.get_or_init(init_hash_secret)
}

fn init_hash_secret() -> HashSecret {
    match std::env::var("PYTHONHASHSEED") {
        Ok(value) => {
            if value == "random" {
                return random_hash_secret();
            }
            let seed: u32 = value.parse().unwrap_or_else(|_| fatal_hash_seed(&value));
            if seed == 0 {
                return HashSecret { k0: 0, k1: 0 };
            }
            let bytes = lcg_hash_seed(seed);
            HashSecret {
                k0: u64::from_ne_bytes(bytes[..8].try_into().unwrap()),
                k1: u64::from_ne_bytes(bytes[8..].try_into().unwrap()),
            }
        }
        Err(_) => random_hash_secret(),
    }
}

fn fatal_hash_seed(value: &str) -> ! {
    eprintln!(
        "Fatal Python error: PYTHONHASHSEED must be \"random\" or an integer in range [0; {PY_HASHSEED_MAX}]"
    );
    eprintln!("PYTHONHASHSEED={value}");
    std::process::exit(1);
}

fn random_hash_secret() -> HashSecret {
    let mut bytes = [0u8; 16];
    if let Err(err) = getrandom(&mut bytes) {
        eprintln!("Failed to initialize hash seed: {err}");
        std::process::exit(1);
    }
    HashSecret {
        k0: u64::from_ne_bytes(bytes[..8].try_into().unwrap()),
        k1: u64::from_ne_bytes(bytes[8..].try_into().unwrap()),
    }
}

fn lcg_hash_seed(seed: u32) -> [u8; 16] {
    let mut out = [0u8; 16];
    let mut x = seed;
    for byte in out.iter_mut() {
        x = x.wrapping_mul(214013).wrapping_add(2531011);
        *byte = ((x >> 16) & 0xff) as u8;
    }
    out
}

struct SipHasher13 {
    v0: u64,
    v1: u64,
    v2: u64,
    v3: u64,
    tail: u64,
    ntail: usize,
    total_len: u64,
}

impl SipHasher13 {
    fn new(k0: u64, k1: u64) -> Self {
        Self {
            v0: 0x736f6d6570736575 ^ k0,
            v1: 0x646f72616e646f6d ^ k1,
            v2: 0x6c7967656e657261 ^ k0,
            v3: 0x7465646279746573 ^ k1,
            tail: 0,
            ntail: 0,
            total_len: 0,
        }
    }

    fn sip_round(&mut self) {
        self.v0 = self.v0.wrapping_add(self.v1);
        self.v1 = self.v1.rotate_left(13);
        self.v1 ^= self.v0;
        self.v0 = self.v0.rotate_left(32);
        self.v2 = self.v2.wrapping_add(self.v3);
        self.v3 = self.v3.rotate_left(16);
        self.v3 ^= self.v2;
        self.v0 = self.v0.wrapping_add(self.v3);
        self.v3 = self.v3.rotate_left(21);
        self.v3 ^= self.v0;
        self.v2 = self.v2.wrapping_add(self.v1);
        self.v1 = self.v1.rotate_left(17);
        self.v1 ^= self.v2;
        self.v2 = self.v2.rotate_left(32);
    }

    fn process_block(&mut self, block: u64) {
        self.v3 ^= block;
        self.sip_round();
        self.v0 ^= block;
    }

    fn update(&mut self, bytes: &[u8]) {
        self.total_len = self.total_len.wrapping_add(bytes.len() as u64);
        for &byte in bytes {
            self.tail |= (byte as u64) << (8 * self.ntail);
            self.ntail += 1;
            if self.ntail == 8 {
                self.process_block(self.tail);
                self.tail = 0;
                self.ntail = 0;
            }
        }
    }

    fn finish(mut self) -> u64 {
        let b = self.tail | ((self.total_len & 0xff) << 56);
        self.process_block(b);
        self.v2 ^= 0xff;
        for _ in 0..3 {
            self.sip_round();
        }
        self.v0 ^ self.v1 ^ self.v2 ^ self.v3
    }
}

fn fix_hash(hash: i64) -> i64 {
    if hash == -1 {
        -2
    } else {
        hash
    }
}

fn exp_mod(exp: i32) -> u32 {
    if exp >= 0 {
        (exp as u32) % PY_HASH_BITS
    } else {
        PY_HASH_BITS - 1 - ((-1 - exp) as u32 % PY_HASH_BITS)
    }
}

fn pow2_mod(exp: u32) -> u64 {
    let mut value = 1u64;
    for _ in 0..exp {
        value <<= 1;
        if value >= PY_HASH_MODULUS {
            value -= PY_HASH_MODULUS;
        }
    }
    value
}

fn reduce_mersenne(mut value: u128) -> u64 {
    let mask = PY_HASH_MODULUS as u128;
    value = (value & mask) + (value >> PY_HASH_BITS);
    value = (value & mask) + (value >> PY_HASH_BITS);
    if value >= mask {
        value -= mask;
    }
    if value == mask {
        0
    } else {
        value as u64
    }
}

fn mul_mod_mersenne(lhs: u64, rhs: u64) -> u64 {
    reduce_mersenne((lhs as u128) * (rhs as u128))
}

fn frexp(value: f64) -> (f64, i32) {
    if value == 0.0 {
        return (0.0, 0);
    }
    let bits = value.to_bits();
    let mut exp = ((bits >> 52) & 0x7ff) as i32;
    let mut mant = bits & ((1u64 << 52) - 1);
    if exp == 0 {
        let mut e = -1022;
        while mant & (1u64 << 52) == 0 {
            mant <<= 1;
            e -= 1;
        }
        exp = e;
        mant &= (1u64 << 52) - 1;
    } else {
        exp -= 1022;
    }
    let frac_bits = (1022u64 << 52) | mant;
    let frac = f64::from_bits(frac_bits);
    (frac, exp)
}

fn hash_bytes_with_secret(bytes: &[u8], secret: &HashSecret) -> i64 {
    if bytes.is_empty() {
        return 0;
    }
    let mut hasher = SipHasher13::new(secret.k0, secret.k1);
    hasher.update(bytes);
    fix_hash(hasher.finish() as i64)
}

fn hash_bytes(bytes: &[u8]) -> i64 {
    hash_bytes_with_secret(bytes, hash_secret())
}

fn hash_string_bytes(bytes: &[u8]) -> i64 {
    if bytes.is_empty() {
        return 0;
    }
    let secret = hash_secret();
    let Ok(text) = std::str::from_utf8(bytes) else {
        return hash_bytes_with_secret(bytes, secret);
    };
    let mut max_codepoint = 0u32;
    for ch in text.chars() {
        max_codepoint = max_codepoint.max(ch as u32);
    }
    let mut hasher = SipHasher13::new(secret.k0, secret.k1);
    if max_codepoint <= 0xff {
        for ch in text.chars() {
            hasher.update(&[ch as u8]);
        }
    } else if max_codepoint <= 0xffff {
        for ch in text.chars() {
            let bytes = (ch as u16).to_ne_bytes();
            hasher.update(&bytes);
        }
    } else {
        for ch in text.chars() {
            let bytes = (ch as u32).to_ne_bytes();
            hasher.update(&bytes);
        }
    }
    fix_hash(hasher.finish() as i64)
}

fn hash_string(ptr: *mut u8) -> i64 {
    let header = unsafe { header_from_obj_ptr(ptr) };
    let cached = unsafe { (*header).state };
    if cached != 0 {
        return cached.wrapping_sub(1);
    }
    let len = unsafe { string_len(ptr) };
    let bytes = unsafe { std::slice::from_raw_parts(string_bytes(ptr), len) };
    let hash = hash_string_bytes(bytes);
    unsafe {
        (*header).state = hash.wrapping_add(1);
    }
    hash
}

fn hash_bytes_cached(ptr: *mut u8, bytes: &[u8]) -> i64 {
    let header = unsafe { header_from_obj_ptr(ptr) };
    let cached = unsafe { (*header).state };
    if cached != 0 {
        return cached.wrapping_sub(1);
    }
    let hash = hash_bytes(bytes);
    unsafe {
        (*header).state = hash.wrapping_add(1);
    }
    hash
}

fn hash_int(val: i64) -> i64 {
    let mut mag = val as i128;
    let sign = if mag < 0 { -1 } else { 1 };
    if mag < 0 {
        mag = -mag;
    }
    let modulus = PY_HASH_MODULUS as i128;
    let mut hash = (mag % modulus) as i64;
    if sign < 0 {
        hash = -hash;
    }
    fix_hash(hash)
}

fn hash_bigint(ptr: *mut u8) -> i64 {
    let big = unsafe { bigint_ref(ptr) };
    let sign = big.sign();
    let modulus = hash_modulus_big();
    let hash = big.abs().mod_floor(modulus);
    let mut hash = hash.to_i64().unwrap_or(0);
    if sign == Sign::Minus {
        hash = -hash;
    }
    fix_hash(hash)
}

fn hash_float(val: f64) -> i64 {
    if val.is_nan() {
        return 0;
    }
    if val.is_infinite() {
        return if val.is_sign_positive() {
            PY_HASH_INF
        } else {
            -PY_HASH_INF
        };
    }
    if val == 0.0 {
        return 0;
    }
    let value = val.abs();
    let mut sign = 1i64;
    if val.is_sign_negative() {
        sign = -1;
    }
    let (mut frac, mut exp) = frexp(value);
    let mut hash = 0u64;
    while frac != 0.0 {
        frac *= (1u64 << 28) as f64;
        let intpart = frac as u64;
        frac -= intpart as f64;
        hash = ((hash << 28) & PY_HASH_MODULUS) | intpart;
        exp -= 28;
    }
    let exp = exp_mod(exp);
    hash = mul_mod_mersenne(hash, pow2_mod(exp));
    let hash = (hash as i64) * sign;
    fix_hash(hash)
}

fn hash_tuple(ptr: *mut u8) -> i64 {
    let elems = unsafe { seq_vec_ref(ptr) };
    #[cfg(target_pointer_width = "64")]
    {
        const XXPRIME_1: u64 = 11400714785074694791;
        const XXPRIME_2: u64 = 14029467366897019727;
        const XXPRIME_5: u64 = 2870177450012600261;
        let mut acc = XXPRIME_5;
        for &elem in elems.iter() {
            let lane = hash_bits_signed(elem);
            if exception_pending() {
                return 0;
            }
            acc = acc.wrapping_add((lane as u64).wrapping_mul(XXPRIME_2));
            acc = acc.rotate_left(31);
            acc = acc.wrapping_mul(XXPRIME_1);
        }
        acc = acc.wrapping_add((elems.len() as u64) ^ (XXPRIME_5 ^ 3527539));
        if acc == u64::MAX {
            return 1546275796;
        }
        acc as i64
    }
    #[cfg(target_pointer_width = "32")]
    {
        const XXPRIME_1: u32 = 2654435761;
        const XXPRIME_2: u32 = 2246822519;
        const XXPRIME_5: u32 = 374761393;
        let mut acc = XXPRIME_5;
        for &elem in elems.iter() {
            let lane = hash_bits_signed(elem);
            if exception_pending() {
                return 0;
            }
            acc = acc.wrapping_add((lane as u32).wrapping_mul(XXPRIME_2));
            acc = acc.rotate_left(13);
            acc = acc.wrapping_mul(XXPRIME_1);
        }
        acc = acc.wrapping_add((elems.len() as u32) ^ (XXPRIME_5 ^ 3527539));
        if acc == u32::MAX {
            return 1546275796;
        }
        return (acc as i32) as i64;
    }
}

fn hash_generic_alias(ptr: *mut u8) -> i64 {
    let origin_bits = unsafe { generic_alias_origin_bits(ptr) };
    let args_bits = unsafe { generic_alias_args_bits(ptr) };
    #[cfg(target_pointer_width = "64")]
    {
        const XXPRIME_1: u64 = 11400714785074694791;
        const XXPRIME_2: u64 = 14029467366897019727;
        const XXPRIME_5: u64 = 2870177450012600261;
        let mut acc = XXPRIME_5;
        for lane_bits in [origin_bits, args_bits] {
            let lane = hash_bits_signed(lane_bits);
            if exception_pending() {
                return 0;
            }
            acc = acc.wrapping_add((lane as u64).wrapping_mul(XXPRIME_2));
            acc = acc.rotate_left(31);
            acc = acc.wrapping_mul(XXPRIME_1);
        }
        acc = acc.wrapping_add(2u64 ^ (XXPRIME_5 ^ 3527539));
        if acc == u64::MAX {
            return 1546275796;
        }
        acc as i64
    }
    #[cfg(target_pointer_width = "32")]
    {
        const XXPRIME_1: u32 = 2654435761;
        const XXPRIME_2: u32 = 2246822519;
        const XXPRIME_5: u32 = 374761393;
        let mut acc = XXPRIME_5;
        for lane_bits in [origin_bits, args_bits] {
            let lane = hash_bits_signed(lane_bits);
            if exception_pending() {
                return 0;
            }
            acc = acc.wrapping_add((lane as u32).wrapping_mul(XXPRIME_2));
            acc = acc.rotate_left(13);
            acc = acc.wrapping_mul(XXPRIME_1);
        }
        acc = acc.wrapping_add(2u32 ^ (XXPRIME_5 ^ 3527539));
        if acc == u32::MAX {
            return 1546275796;
        }
        return (acc as i32) as i64;
    }
}

#[cfg(target_pointer_width = "64")]
fn slice_hash_acc(lanes: [u64; 3]) -> u64 {
    const XXPRIME_1: u64 = 11400714785074694791;
    const XXPRIME_2: u64 = 14029467366897019727;
    const XXPRIME_5: u64 = 2870177450012600261;
    let mut acc = XXPRIME_5;
    for lane in lanes {
        acc = acc.wrapping_add(lane.wrapping_mul(XXPRIME_2));
        acc = acc.rotate_left(31);
        acc = acc.wrapping_mul(XXPRIME_1);
    }
    acc
}

#[cfg(target_pointer_width = "32")]
fn slice_hash_acc(lanes: [u32; 3]) -> u32 {
    const XXPRIME_1: u32 = 2654435761;
    const XXPRIME_2: u32 = 2246822519;
    const XXPRIME_5: u32 = 374761393;
    let mut acc = XXPRIME_5;
    for lane in lanes {
        acc = acc.wrapping_add(lane.wrapping_mul(XXPRIME_2));
        acc = acc.rotate_left(13);
        acc = acc.wrapping_mul(XXPRIME_1);
    }
    acc
}

fn hash_slice_bits(start_bits: u64, stop_bits: u64, step_bits: u64) -> Option<i64> {
    let mut lanes = [0i64; 3];
    let elems = [start_bits, stop_bits, step_bits];
    for (idx, bits) in elems.iter().enumerate() {
        lanes[idx] = hash_bits_signed(*bits);
        if exception_pending() {
            return None;
        }
    }
    #[cfg(target_pointer_width = "64")]
    {
        let acc = slice_hash_acc([lanes[0] as u64, lanes[1] as u64, lanes[2] as u64]);
        if acc == u64::MAX {
            return Some(1546275796);
        }
        Some(acc as i64)
    }
    #[cfg(target_pointer_width = "32")]
    {
        let acc = slice_hash_acc([lanes[0] as u32, lanes[1] as u32, lanes[2] as u32]);
        if acc == u32::MAX {
            return Some(1546275796);
        }
        return Some((acc as i32) as i64);
    }
}

fn shuffle_frozenset_hash(hash: u64) -> u64 {
    let mixed = (hash ^ 89869747u64) ^ (hash << 16);
    mixed.wrapping_mul(3644798167u64)
}

fn hash_frozenset(ptr: *mut u8) -> i64 {
    let elems = unsafe { set_order(ptr) };
    let mut hash = 0u64;
    for &elem in elems.iter() {
        hash ^= shuffle_frozenset_hash(hash_bits(elem));
    }
    if elems.len() & 1 == 1 {
        hash ^= shuffle_frozenset_hash(0);
    }
    hash ^= ((elems.len() as u64) + 1).wrapping_mul(1927868237u64);
    hash ^= (hash >> 11) ^ (hash >> 25);
    hash = hash.wrapping_mul(69069u64).wrapping_add(907133923u64);
    if hash == u64::MAX {
        hash = 590923713u64;
    }
    hash as i64
}

fn hash_pointer(ptr: u64) -> i64 {
    let hash = (ptr >> 4) as i64;
    fix_hash(hash)
}

fn hash_unhashable(obj: MoltObject) -> i64 {
    let name = type_name(obj);
    let msg = format!("unhashable type: '{name}'");
    return raise_exception::<_>("TypeError", &msg);
}

fn is_unhashable_type(type_id: u32) -> bool {
    matches!(
        type_id,
        TYPE_ID_LIST
            | TYPE_ID_DICT
            | TYPE_ID_SET
            | TYPE_ID_BYTEARRAY
            | TYPE_ID_MEMORYVIEW
            | TYPE_ID_LIST_BUILDER
            | TYPE_ID_DICT_BUILDER
            | TYPE_ID_SET_BUILDER
            | TYPE_ID_DICT_KEYS_VIEW
            | TYPE_ID_DICT_VALUES_VIEW
            | TYPE_ID_DICT_ITEMS_VIEW
            | TYPE_ID_CALLARGS
    )
}

fn hash_bits_signed(bits: u64) -> i64 {
    let obj = obj_from_bits(bits);
    if let Some(i) = obj.as_int() {
        return hash_int(i);
    }
    if let Some(b) = obj.as_bool() {
        return hash_int(if b { 1 } else { 0 });
    }
    if obj.is_none() {
        return PY_HASH_NONE;
    }
    if let Some(f) = obj.as_float() {
        return hash_float(f);
    }
    if let Some(ptr) = obj.as_ptr() {
        unsafe {
            let type_id = object_type_id(ptr);
            if is_unhashable_type(type_id) {
                return hash_unhashable(obj);
            }
            if type_id == TYPE_ID_STRING {
                return hash_string(ptr);
            }
            if type_id == TYPE_ID_BYTES || type_id == TYPE_ID_BYTEARRAY {
                let len = bytes_len(ptr);
                let bytes = std::slice::from_raw_parts(bytes_data(ptr), len);
                return hash_bytes_cached(ptr, bytes);
            }
            if type_id == TYPE_ID_BIGINT {
                return hash_bigint(ptr);
            }
            if type_id == TYPE_ID_TUPLE {
                return hash_tuple(ptr);
            }
            if type_id == TYPE_ID_GENERIC_ALIAS {
                return hash_generic_alias(ptr);
            }
            if type_id == TYPE_ID_SLICE {
                let start_bits = slice_start_bits(ptr);
                let stop_bits = slice_stop_bits(ptr);
                let step_bits = slice_step_bits(ptr);
                if let Some(hash) = hash_slice_bits(start_bits, stop_bits, step_bits) {
                    return hash;
                }
                return 0;
            }
            if type_id == TYPE_ID_FROZENSET {
                return hash_frozenset(ptr);
            }
        }
        return hash_pointer(ptr as u64);
    }
    hash_pointer(bits)
}

fn hash_bits(bits: u64) -> u64 {
    hash_bits_signed(bits) as u64
}

fn ensure_hashable(key_bits: u64) -> bool {
    let obj = obj_from_bits(key_bits);
    if let Some(ptr) = obj.as_ptr() {
        unsafe {
            let type_id = object_type_id(ptr);
            if is_unhashable_type(type_id) {
                let name = type_name(obj);
                let msg = format!("unhashable type: '{name}'");
                return raise_exception::<_>("TypeError", &msg);
            }
        }
    }
    true
}

fn dict_table_capacity(entries: usize) -> usize {
    let mut cap = entries.saturating_mul(2).next_power_of_two();
    if cap < 8 {
        cap = 8;
    }
    cap
}

fn dict_insert_entry(order: &[u64], table: &mut [usize], entry_idx: usize) {
    let mask = table.len() - 1;
    let key_bits = order[entry_idx * 2];
    let mut slot = (hash_bits(key_bits) as usize) & mask;
    loop {
        if table[slot] == 0 {
            table[slot] = entry_idx + 1;
            return;
        }
        slot = (slot + 1) & mask;
    }
}

fn dict_rebuild(order: &[u64], table: &mut Vec<usize>, capacity: usize) {
    table.clear();
    table.resize(capacity, 0);
    let entry_count = order.len() / 2;
    for entry_idx in 0..entry_count {
        dict_insert_entry(order, table, entry_idx);
    }
}

fn dict_find_entry(order: &[u64], table: &[usize], key_bits: u64) -> Option<usize> {
    if table.is_empty() {
        return None;
    }
    let mask = table.len() - 1;
    let mut slot = (hash_bits(key_bits) as usize) & mask;
    loop {
        let entry = table[slot];
        if entry == 0 {
            return None;
        }
        let entry_idx = entry - 1;
        let entry_key = order[entry_idx * 2];
        if obj_eq(obj_from_bits(entry_key), obj_from_bits(key_bits)) {
            return Some(entry_idx);
        }
        slot = (slot + 1) & mask;
    }
}

fn set_table_capacity(entries: usize) -> usize {
    dict_table_capacity(entries)
}

fn set_insert_entry(order: &[u64], table: &mut [usize], entry_idx: usize) {
    let mask = table.len() - 1;
    let key_bits = order[entry_idx];
    let mut slot = (hash_bits(key_bits) as usize) & mask;
    loop {
        if table[slot] == 0 {
            table[slot] = entry_idx + 1;
            return;
        }
        slot = (slot + 1) & mask;
    }
}

fn set_rebuild(order: &[u64], table: &mut Vec<usize>, capacity: usize) {
    table.clear();
    table.resize(capacity, 0);
    for entry_idx in 0..order.len() {
        set_insert_entry(order, table, entry_idx);
    }
}

fn set_find_entry(order: &[u64], table: &[usize], key_bits: u64) -> Option<usize> {
    if table.is_empty() {
        return None;
    }
    let mask = table.len() - 1;
    let mut slot = (hash_bits(key_bits) as usize) & mask;
    loop {
        let entry = table[slot];
        if entry == 0 {
            return None;
        }
        let entry_idx = entry - 1;
        let entry_key = order[entry_idx];
        if obj_eq(obj_from_bits(entry_key), obj_from_bits(key_bits)) {
            return Some(entry_idx);
        }
        slot = (slot + 1) & mask;
    }
}

fn alloc_bytes_like_with_len(len: usize, type_id: u32) -> *mut u8 {
    let total = std::mem::size_of::<MoltHeader>() + std::mem::size_of::<usize>() + len;
    let ptr = alloc_object(total, type_id);
    if ptr.is_null() {
        return ptr;
    }
    unsafe {
        let len_ptr = ptr as *mut usize;
        *len_ptr = len;
    }
    ptr
}

fn alloc_string(bytes: &[u8]) -> *mut u8 {
    let ptr = alloc_bytes_like_with_len(bytes.len(), TYPE_ID_STRING);
    if ptr.is_null() {
        return ptr;
    }
    unsafe {
        let data_ptr = ptr.add(std::mem::size_of::<usize>());
        std::ptr::copy_nonoverlapping(bytes.as_ptr(), data_ptr, bytes.len());
    }
    ptr
}

fn alloc_bytes_like(bytes: &[u8], type_id: u32) -> *mut u8 {
    let ptr = alloc_bytes_like_with_len(bytes.len(), type_id);
    if ptr.is_null() {
        return ptr;
    }
    unsafe {
        let data_ptr = ptr.add(std::mem::size_of::<usize>());
        std::ptr::copy_nonoverlapping(bytes.as_ptr(), data_ptr, bytes.len());
    }
    ptr
}

fn concat_bytes_like(left: &[u8], right: &[u8], type_id: u32) -> Option<u64> {
    let total = left.len().checked_add(right.len())?;
    if type_id == TYPE_ID_BYTEARRAY {
        let mut out = Vec::with_capacity(total);
        out.extend_from_slice(left);
        out.extend_from_slice(right);
        let ptr = alloc_bytearray(&out);
        if ptr.is_null() {
            return None;
        }
        return Some(MoltObject::from_ptr(ptr).bits());
    }
    let ptr = alloc_bytes_like_with_len(total, type_id);
    if ptr.is_null() {
        return None;
    }
    unsafe {
        let data_ptr = ptr.add(std::mem::size_of::<usize>());
        std::ptr::copy_nonoverlapping(left.as_ptr(), data_ptr, left.len());
        std::ptr::copy_nonoverlapping(right.as_ptr(), data_ptr.add(left.len()), right.len());
    }
    Some(MoltObject::from_ptr(ptr).bits())
}

fn alloc_bytes(bytes: &[u8]) -> *mut u8 {
    alloc_bytes_like(bytes, TYPE_ID_BYTES)
}

fn alloc_bytearray(bytes: &[u8]) -> *mut u8 {
    let cap = if bytes.len() <= MAX_SMALL_LIST {
        MAX_SMALL_LIST
    } else {
        bytes.len()
    };
    alloc_bytearray_with_capacity(bytes, cap)
}

fn alloc_bytearray_with_capacity(bytes: &[u8], capacity: usize) -> *mut u8 {
    let cap = capacity.max(bytes.len());
    let total = std::mem::size_of::<MoltHeader>()
        + std::mem::size_of::<*mut Vec<u8>>()
        + std::mem::size_of::<u64>();
    let ptr = alloc_object(total, TYPE_ID_BYTEARRAY);
    if ptr.is_null() {
        return ptr;
    }
    unsafe {
        let mut vec = Vec::with_capacity(cap);
        vec.extend_from_slice(bytes);
        let vec_ptr = Box::into_raw(Box::new(vec));
        *(ptr as *mut *mut Vec<u8>) = vec_ptr;
    }
    ptr
}

fn alloc_bytearray_with_len(len: usize) -> *mut u8 {
    let total = std::mem::size_of::<MoltHeader>()
        + std::mem::size_of::<*mut Vec<u8>>()
        + std::mem::size_of::<u64>();
    let ptr = alloc_object(total, TYPE_ID_BYTEARRAY);
    if ptr.is_null() {
        return ptr;
    }
    unsafe {
        let vec = vec![0u8; len];
        let vec_ptr = Box::into_raw(Box::new(vec));
        *(ptr as *mut *mut Vec<u8>) = vec_ptr;
    }
    ptr
}

fn alloc_intarray(values: &[i64]) -> *mut u8 {
    let total = std::mem::size_of::<MoltHeader>()
        + std::mem::size_of::<usize>()
        + std::mem::size_of_val(values);
    let ptr = alloc_object(total, TYPE_ID_INTARRAY);
    if ptr.is_null() {
        return ptr;
    }
    unsafe {
        let len_ptr = ptr as *mut usize;
        *len_ptr = values.len();
        let data_ptr = ptr.add(std::mem::size_of::<usize>()) as *mut i64;
        std::ptr::copy_nonoverlapping(values.as_ptr(), data_ptr, values.len());
    }
    ptr
}

fn alloc_memoryview(
    owner_bits: u64,
    offset: isize,
    len: usize,
    itemsize: usize,
    stride: isize,
    readonly: bool,
    format_bits: u64,
) -> *mut u8 {
    let total = std::mem::size_of::<MoltHeader>() + std::mem::size_of::<MemoryView>();
    let ptr = alloc_object(total, TYPE_ID_MEMORYVIEW);
    if ptr.is_null() {
        return ptr;
    }
    unsafe {
        let shape = Box::new(vec![len as isize]);
        let strides = Box::new(vec![stride]);
        let mv_ptr = memoryview_ptr(ptr);
        (*mv_ptr).owner_bits = owner_bits;
        (*mv_ptr).offset = offset;
        (*mv_ptr).len = len;
        (*mv_ptr).itemsize = itemsize;
        (*mv_ptr).stride = stride;
        (*mv_ptr).readonly = if readonly { 1 } else { 0 };
        (*mv_ptr).ndim = 1;
        (*mv_ptr)._pad = [0; 6];
        (*mv_ptr).format_bits = format_bits;
        (*mv_ptr).shape_ptr = Box::into_raw(shape);
        (*mv_ptr).strides_ptr = Box::into_raw(strides);
    }
    inc_ref_bits(owner_bits);
    inc_ref_bits(format_bits);
    ptr
}

fn alloc_memoryview_shaped(
    owner_bits: u64,
    offset: isize,
    itemsize: usize,
    readonly: bool,
    format_bits: u64,
    shape: Vec<isize>,
    strides: Vec<isize>,
) -> *mut u8 {
    let total = std::mem::size_of::<MoltHeader>() + std::mem::size_of::<MemoryView>();
    let ptr = alloc_object(total, TYPE_ID_MEMORYVIEW);
    if ptr.is_null() {
        return ptr;
    }
    let ndim = shape.len();
    let len = shape.first().copied().unwrap_or(0).max(0) as usize;
    let stride = strides.first().copied().unwrap_or(0);
    unsafe {
        let mv_ptr = memoryview_ptr(ptr);
        (*mv_ptr).owner_bits = owner_bits;
        (*mv_ptr).offset = offset;
        (*mv_ptr).len = len;
        (*mv_ptr).itemsize = itemsize;
        (*mv_ptr).stride = stride;
        (*mv_ptr).readonly = if readonly { 1 } else { 0 };
        (*mv_ptr).ndim = ndim.min(u8::MAX as usize) as u8;
        (*mv_ptr)._pad = [0; 6];
        (*mv_ptr).format_bits = format_bits;
        (*mv_ptr).shape_ptr = Box::into_raw(Box::new(shape));
        (*mv_ptr).strides_ptr = Box::into_raw(Box::new(strides));
    }
    inc_ref_bits(owner_bits);
    inc_ref_bits(format_bits);
    ptr
}

fn fill_repeated_bytes(dst: &mut [u8], pattern: &[u8]) {
    if pattern.is_empty() {
        return;
    }
    if pattern.len() == 1 {
        dst.fill(pattern[0]);
        return;
    }
    let mut filled = pattern.len().min(dst.len());
    dst[..filled].copy_from_slice(&pattern[..filled]);
    while filled < dst.len() {
        let copy_len = std::cmp::min(filled, dst.len() - filled);
        let (head, tail) = dst.split_at_mut(filled);
        tail[..copy_len].copy_from_slice(&head[..copy_len]);
        filled += copy_len;
    }
}

unsafe fn dict_set_in_place(ptr: *mut u8, key_bits: u64, val_bits: u64) {
    if !ensure_hashable(key_bits) {
        return;
    }
    let order = dict_order(ptr);
    let table = dict_table(ptr);
    if let Some(entry_idx) = dict_find_entry(order, table, key_bits) {
        let val_idx = entry_idx * 2 + 1;
        let old_bits = order[val_idx];
        if old_bits != val_bits {
            dec_ref_bits(old_bits);
            inc_ref_bits(val_bits);
            order[val_idx] = val_bits;
        }
        return;
    }

    let new_entries = (order.len() / 2) + 1;
    let needs_resize = table.is_empty() || new_entries * 10 >= table.len() * 7;
    if needs_resize {
        let capacity = dict_table_capacity(new_entries);
        dict_rebuild(order, table, capacity);
    }

    order.push(key_bits);
    order.push(val_bits);
    inc_ref_bits(key_bits);
    inc_ref_bits(val_bits);
    let entry_idx = order.len() / 2 - 1;
    dict_insert_entry(order, table, entry_idx);
}

unsafe fn set_add_in_place(ptr: *mut u8, key_bits: u64) {
    if !ensure_hashable(key_bits) {
        return;
    }
    let order = set_order(ptr);
    let table = set_table(ptr);
    if set_find_entry(order, table, key_bits).is_some() {
        return;
    }

    let new_entries = order.len() + 1;
    let needs_resize = table.is_empty() || new_entries * 10 >= table.len() * 7;
    if needs_resize {
        let capacity = set_table_capacity(new_entries);
        set_rebuild(order, table, capacity);
    }

    order.push(key_bits);
    inc_ref_bits(key_bits);
    let entry_idx = order.len() - 1;
    set_insert_entry(order, table, entry_idx);
}

unsafe fn dict_get_in_place(ptr: *mut u8, key_bits: u64) -> Option<u64> {
    if !ensure_hashable(key_bits) {
        return None;
    }
    let order = dict_order(ptr);
    let table = dict_table(ptr);
    dict_find_entry(order, table, key_bits).map(|idx| order[idx * 2 + 1])
}

unsafe fn set_del_in_place(ptr: *mut u8, key_bits: u64) -> bool {
    if !ensure_hashable(key_bits) {
        return false;
    }
    let order = set_order(ptr);
    let table = set_table(ptr);
    let Some(entry_idx) = set_find_entry(order, table, key_bits) else {
        return false;
    };
    let key_val = order[entry_idx];
    dec_ref_bits(key_val);
    order.remove(entry_idx);
    let entries = order.len();
    let capacity = set_table_capacity(entries.max(1));
    set_rebuild(order, table, capacity);
    true
}

unsafe fn set_replace_entries(ptr: *mut u8, entries: &[u64]) {
    let order = set_order(ptr);
    for entry in order.iter().copied() {
        dec_ref_bits(entry);
    }
    order.clear();
    for entry in entries {
        inc_ref_bits(*entry);
        order.push(*entry);
    }
    let table = set_table(ptr);
    let capacity = set_table_capacity(order.len().max(1));
    set_rebuild(order, table, capacity);
}

unsafe fn dict_del_in_place(ptr: *mut u8, key_bits: u64) -> bool {
    if !ensure_hashable(key_bits) {
        return false;
    }
    let order = dict_order(ptr);
    let table = dict_table(ptr);
    let Some(entry_idx) = dict_find_entry(order, table, key_bits) else {
        return false;
    };
    let key_idx = entry_idx * 2;
    let val_idx = key_idx + 1;
    let key_val = order[key_idx];
    let val_val = order[val_idx];
    dec_ref_bits(key_val);
    dec_ref_bits(val_val);
    order.drain(key_idx..=val_idx);
    let entries = order.len() / 2;
    let capacity = dict_table_capacity(entries.max(1));
    dict_rebuild(order, table, capacity);
    true
}

unsafe fn dict_clear_in_place(ptr: *mut u8) {
    let order = dict_order(ptr);
    for pair in order.chunks_exact(2) {
        dec_ref_bits(pair[0]);
        dec_ref_bits(pair[1]);
    }
    order.clear();
    let table = dict_table(ptr);
    table.clear();
}

fn alloc_list_with_capacity(elems: &[u64], capacity: usize) -> *mut u8 {
    let cap = capacity.max(elems.len());
    let total = std::mem::size_of::<MoltHeader>()
        + std::mem::size_of::<*mut DataclassDesc>()
        + std::mem::size_of::<*mut Vec<u64>>()
        + std::mem::size_of::<u64>();
    let ptr = alloc_object(total, TYPE_ID_LIST);
    if ptr.is_null() {
        return ptr;
    }
    unsafe {
        let mut vec = Vec::with_capacity(cap);
        vec.extend_from_slice(elems);
        for &elem in elems {
            inc_ref_bits(elem);
        }
        let vec_ptr = Box::into_raw(Box::new(vec));
        *(ptr as *mut *mut Vec<u64>) = vec_ptr;
    }
    ptr
}

fn alloc_list(elems: &[u64]) -> *mut u8 {
    let cap = if elems.len() <= MAX_SMALL_LIST {
        MAX_SMALL_LIST
    } else {
        elems.len()
    };
    alloc_list_with_capacity(elems, cap)
}

fn alloc_tuple_with_capacity(elems: &[u64], capacity: usize) -> *mut u8 {
    let cap = capacity.max(elems.len());
    let total = std::mem::size_of::<MoltHeader>()
        + std::mem::size_of::<*mut Vec<u64>>()
        + std::mem::size_of::<u64>();
    let ptr = alloc_object(total, TYPE_ID_TUPLE);
    if ptr.is_null() {
        return ptr;
    }
    unsafe {
        let mut vec = Vec::with_capacity(cap);
        vec.extend_from_slice(elems);
        for &elem in elems {
            inc_ref_bits(elem);
        }
        let vec_ptr = Box::into_raw(Box::new(vec));
        *(ptr as *mut *mut Vec<u64>) = vec_ptr;
    }
    ptr
}

fn alloc_tuple(elems: &[u64]) -> *mut u8 {
    let cap = if elems.len() <= MAX_SMALL_LIST {
        MAX_SMALL_LIST
    } else {
        elems.len()
    };
    alloc_tuple_with_capacity(elems, cap)
}

fn alloc_range(start: i64, stop: i64, step: i64) -> *mut u8 {
    let total = std::mem::size_of::<MoltHeader>() + 3 * std::mem::size_of::<i64>();
    let ptr = alloc_object(total, TYPE_ID_RANGE);
    if ptr.is_null() {
        return ptr;
    }
    unsafe {
        *(ptr as *mut i64) = start;
        *(ptr.add(std::mem::size_of::<i64>()) as *mut i64) = stop;
        *(ptr.add(2 * std::mem::size_of::<i64>()) as *mut i64) = step;
    }
    ptr
}

fn alloc_slice_obj(start_bits: u64, stop_bits: u64, step_bits: u64) -> *mut u8 {
    let total = std::mem::size_of::<MoltHeader>() + 3 * std::mem::size_of::<u64>();
    let ptr = alloc_object(total, TYPE_ID_SLICE);
    if ptr.is_null() {
        return ptr;
    }
    unsafe {
        *(ptr as *mut u64) = start_bits;
        *(ptr.add(std::mem::size_of::<u64>()) as *mut u64) = stop_bits;
        *(ptr.add(2 * std::mem::size_of::<u64>()) as *mut u64) = step_bits;
        inc_ref_bits(start_bits);
        inc_ref_bits(stop_bits);
        inc_ref_bits(step_bits);
    }
    ptr
}

fn alloc_generic_alias(origin_bits: u64, args_bits: u64) -> *mut u8 {
    let total = std::mem::size_of::<MoltHeader>() + 2 * std::mem::size_of::<u64>();
    let ptr = alloc_object(total, TYPE_ID_GENERIC_ALIAS);
    if ptr.is_null() {
        return ptr;
    }
    unsafe {
        *(ptr as *mut u64) = origin_bits;
        *(ptr.add(std::mem::size_of::<u64>()) as *mut u64) = args_bits;
        inc_ref_bits(origin_bits);
        inc_ref_bits(args_bits);
    }
    ptr
}

fn alloc_context_manager(enter_fn: *const (), exit_fn: *const (), payload_bits: u64) -> *mut u8 {
    let total = std::mem::size_of::<MoltHeader>()
        + 2 * std::mem::size_of::<*const ()>()
        + std::mem::size_of::<u64>();
    let ptr = alloc_object(total, TYPE_ID_CONTEXT_MANAGER);
    if ptr.is_null() {
        return ptr;
    }
    unsafe {
        *(ptr as *mut *const ()) = enter_fn;
        *(ptr.add(std::mem::size_of::<*const ()>()) as *mut *const ()) = exit_fn;
        *(ptr.add(2 * std::mem::size_of::<*const ()>()) as *mut u64) = payload_bits;
        inc_ref_bits(payload_bits);
    }
    ptr
}

fn alloc_function_obj(fn_ptr: u64, arity: u64) -> *mut u8 {
    let total = std::mem::size_of::<MoltHeader>() + 8 * std::mem::size_of::<u64>();
    let ptr = alloc_object(total, TYPE_ID_FUNCTION);
    if ptr.is_null() {
        return ptr;
    }
    unsafe {
        *(ptr as *mut u64) = fn_ptr;
        *(ptr.add(std::mem::size_of::<u64>()) as *mut u64) = arity;
        *(ptr.add(2 * std::mem::size_of::<u64>()) as *mut u64) = 0;
        *(ptr.add(3 * std::mem::size_of::<u64>()) as *mut u64) = 0;
        *(ptr.add(4 * std::mem::size_of::<u64>()) as *mut u64) = 0;
        *(ptr.add(5 * std::mem::size_of::<u64>()) as *mut u64) = 0;
        *(ptr.add(6 * std::mem::size_of::<u64>()) as *mut u64) = 0;
        let none_bits = MoltObject::none().bits();
        *(ptr.add(7 * std::mem::size_of::<u64>()) as *mut u64) = none_bits;
        inc_ref_bits(none_bits);
    }
    ptr
}

fn alloc_code_obj(
    filename_bits: u64,
    name_bits: u64,
    firstlineno: i64,
    linetable_bits: u64,
) -> *mut u8 {
    let total = std::mem::size_of::<MoltHeader>() + 4 * std::mem::size_of::<u64>();
    let ptr = alloc_object(total, TYPE_ID_CODE);
    if ptr.is_null() {
        return ptr;
    }
    unsafe {
        *(ptr as *mut u64) = filename_bits;
        *(ptr.add(std::mem::size_of::<u64>()) as *mut u64) = name_bits;
        *(ptr.add(2 * std::mem::size_of::<u64>()) as *mut i64) = firstlineno;
        *(ptr.add(3 * std::mem::size_of::<u64>()) as *mut u64) = linetable_bits;
        if filename_bits != 0 {
            inc_ref_bits(filename_bits);
        }
        if name_bits != 0 {
            inc_ref_bits(name_bits);
        }
        if linetable_bits != 0 {
            inc_ref_bits(linetable_bits);
        }
    }
    ptr
}

fn alloc_bound_method_obj(func_bits: u64, self_bits: u64) -> *mut u8 {
    let total = std::mem::size_of::<MoltHeader>() + 2 * std::mem::size_of::<u64>();
    let ptr = alloc_object(total, TYPE_ID_BOUND_METHOD);
    if ptr.is_null() {
        return ptr;
    }
    unsafe {
        *(ptr as *mut u64) = func_bits;
        *(ptr.add(std::mem::size_of::<u64>()) as *mut u64) = self_bits;
        inc_ref_bits(func_bits);
        inc_ref_bits(self_bits);
    }
    ptr
}

fn alloc_module_obj(name_bits: u64) -> *mut u8 {
    let dict_ptr = alloc_dict_with_pairs(&[]);
    if dict_ptr.is_null() {
        return std::ptr::null_mut();
    }
    let dict_bits = MoltObject::from_ptr(dict_ptr).bits();
    let total = std::mem::size_of::<MoltHeader>() + 2 * std::mem::size_of::<u64>();
    let ptr = alloc_object(total, TYPE_ID_MODULE);
    if ptr.is_null() {
        dec_ref_bits(dict_bits);
        return ptr;
    }
    unsafe {
        *(ptr as *mut u64) = name_bits;
        *(ptr.add(std::mem::size_of::<u64>()) as *mut u64) = dict_bits;
        inc_ref_bits(name_bits);
        inc_ref_bits(dict_bits);
    }
    ptr
}

fn alloc_class_obj(name_bits: u64) -> *mut u8 {
    let dict_ptr = alloc_dict_with_pairs(&[]);
    if dict_ptr.is_null() {
        return std::ptr::null_mut();
    }
    let dict_bits = MoltObject::from_ptr(dict_ptr).bits();
    let bases_bits = MoltObject::none().bits();
    let mro_bits = MoltObject::none().bits();
    let total = std::mem::size_of::<MoltHeader>() + 7 * std::mem::size_of::<u64>();
    let ptr = alloc_object(total, TYPE_ID_TYPE);
    if ptr.is_null() {
        dec_ref_bits(dict_bits);
        return ptr;
    }
    unsafe {
        *(ptr as *mut u64) = name_bits;
        *(ptr.add(std::mem::size_of::<u64>()) as *mut u64) = dict_bits;
        *(ptr.add(2 * std::mem::size_of::<u64>()) as *mut u64) = bases_bits;
        *(ptr.add(3 * std::mem::size_of::<u64>()) as *mut u64) = mro_bits;
        *(ptr.add(4 * std::mem::size_of::<u64>()) as *mut u64) = 0;
        *(ptr.add(5 * std::mem::size_of::<u64>()) as *mut u64) = 0;
        let none_bits = MoltObject::none().bits();
        *(ptr.add(6 * std::mem::size_of::<u64>()) as *mut u64) = none_bits;
        inc_ref_bits(name_bits);
        inc_ref_bits(bases_bits);
        inc_ref_bits(mro_bits);
        inc_ref_bits(none_bits);
    }
    ptr
}

fn alloc_classmethod_obj(func_bits: u64) -> *mut u8 {
    let total = std::mem::size_of::<MoltHeader>() + std::mem::size_of::<u64>();
    let ptr = alloc_object(total, TYPE_ID_CLASSMETHOD);
    if ptr.is_null() {
        return ptr;
    }
    unsafe {
        *(ptr as *mut u64) = func_bits;
        inc_ref_bits(func_bits);
    }
    ptr
}

fn alloc_staticmethod_obj(func_bits: u64) -> *mut u8 {
    let total = std::mem::size_of::<MoltHeader>() + std::mem::size_of::<u64>();
    let ptr = alloc_object(total, TYPE_ID_STATICMETHOD);
    if ptr.is_null() {
        return ptr;
    }
    unsafe {
        *(ptr as *mut u64) = func_bits;
        inc_ref_bits(func_bits);
    }
    ptr
}

fn alloc_property_obj(get_bits: u64, set_bits: u64, del_bits: u64) -> *mut u8 {
    let total = std::mem::size_of::<MoltHeader>() + 3 * std::mem::size_of::<u64>();
    let ptr = alloc_object(total, TYPE_ID_PROPERTY);
    if ptr.is_null() {
        return ptr;
    }
    unsafe {
        *(ptr as *mut u64) = get_bits;
        *(ptr.add(std::mem::size_of::<u64>()) as *mut u64) = set_bits;
        *(ptr.add(2 * std::mem::size_of::<u64>()) as *mut u64) = del_bits;
        inc_ref_bits(get_bits);
        inc_ref_bits(set_bits);
        inc_ref_bits(del_bits);
    }
    ptr
}

fn alloc_super_obj(type_bits: u64, obj_bits: u64) -> *mut u8 {
    let total = std::mem::size_of::<MoltHeader>() + 2 * std::mem::size_of::<u64>();
    let ptr = alloc_object(total, TYPE_ID_SUPER);
    if ptr.is_null() {
        return ptr;
    }
    unsafe {
        *(ptr as *mut u64) = type_bits;
        *(ptr.add(std::mem::size_of::<u64>()) as *mut u64) = obj_bits;
        inc_ref_bits(type_bits);
        inc_ref_bits(obj_bits);
    }
    ptr
}

#[allow(clippy::too_many_arguments)]
fn alloc_file_handle_with_state(
    state: Arc<MoltFileState>,
    readable: bool,
    writable: bool,
    text: bool,
    closefd: bool,
    owns_fd: bool,
    line_buffering: bool,
    write_through: bool,
    buffer_size: i64,
    class_bits: u64,
    name_bits: u64,
    mode: String,
    encoding: Option<String>,
    errors: Option<String>,
    newline: Option<String>,
    buffer_bits: u64,
) -> *mut u8 {
    let total = std::mem::size_of::<MoltHeader>() + std::mem::size_of::<*mut MoltFileHandle>();
    let ptr = alloc_object(total, TYPE_ID_FILE_HANDLE);
    if ptr.is_null() {
        return ptr;
    }
    let handle = Box::new(MoltFileHandle {
        state,
        readable,
        writable,
        text,
        closefd,
        owns_fd,
        closed: false,
        detached: false,
        line_buffering,
        write_through,
        buffer_size,
        class_bits,
        name_bits,
        mode,
        encoding,
        errors,
        newline,
        buffer_bits,
        pending_byte: None,
    });
    if name_bits != 0 {
        inc_ref_bits(name_bits);
    }
    if buffer_bits != 0 {
        inc_ref_bits(buffer_bits);
    }
    let handle_ptr = Box::into_raw(handle);
    unsafe {
        *(ptr as *mut *mut MoltFileHandle) = handle_ptr;
    }
    ptr
}

extern "C" fn context_null_enter(payload_bits: u64) -> u64 {
    inc_ref_bits(payload_bits);
    payload_bits
}

extern "C" fn context_null_exit(_payload_bits: u64, _exc_bits: u64) -> u64 {
    MoltObject::from_bool(false).bits()
}

extern "C" fn context_closing_enter(payload_bits: u64) -> u64 {
    inc_ref_bits(payload_bits);
    payload_bits
}

extern "C" fn context_closing_exit(payload_bits: u64, _exc_bits: u64) -> u64 {
    close_payload(payload_bits);
    MoltObject::from_bool(false).bits()
}

fn context_stack_push(ctx_bits: u64) {
    CONTEXT_STACK.with(|stack| {
        stack.borrow_mut().push(ctx_bits);
    });
    inc_ref_bits(ctx_bits);
}

fn context_stack_pop(expected_bits: u64) {
    let result = CONTEXT_STACK.with(|stack| {
        let mut stack = stack.borrow_mut();
        let Some(bits) = stack.pop() else {
            return Err("context manager stack underflow");
        };
        if bits != expected_bits {
            return Err("context manager stack mismatch");
        }
        Ok(bits)
    });
    match result {
        Ok(bits) => dec_ref_bits(bits),
        Err(msg) => return raise_exception::<_>("RuntimeError", msg),
    }
}

unsafe fn context_exit_unchecked(ctx_bits: u64, exc_bits: u64) {
    let ctx_obj = obj_from_bits(ctx_bits);
    let Some(ptr) = ctx_obj.as_ptr() else {
        return;
    };
    let type_id = object_type_id(ptr);
    if type_id == TYPE_ID_CONTEXT_MANAGER {
        let exit_fn_addr = context_exit_fn(ptr);
        if exit_fn_addr.is_null() {
            return;
        }
        let exit_fn =
            std::mem::transmute::<*const (), extern "C" fn(u64, u64) -> u64>(exit_fn_addr);
        exit_fn(context_payload_bits(ptr), exc_bits);
        return;
    }
    if type_id == TYPE_ID_FILE_HANDLE {
        file_handle_exit(ptr, exc_bits);
        return;
    }
    let exit_name_bits = intern_static_name(&runtime_state().interned.exit_name, b"__exit__");
    let Some(exit_bits) = attr_lookup_ptr_allow_missing(ptr, exit_name_bits) else {
        return;
    };
    let none_bits = MoltObject::none().bits();
    let exc_obj = obj_from_bits(exc_bits);
    let (exc_type_bits, exc_val_bits, tb_bits) = if exc_obj.is_none() {
        (none_bits, none_bits, none_bits)
    } else {
        let tb_bits = exc_obj
            .as_ptr()
            .map(|ptr| exception_trace_bits(ptr))
            .unwrap_or(none_bits);
        (type_of_bits(exc_bits), exc_bits, tb_bits)
    };
    let _ = call_callable3(exit_bits, exc_type_bits, exc_val_bits, tb_bits);
    dec_ref_bits(exit_bits);
}

fn context_stack_depth() -> usize {
    CONTEXT_STACK.with(|stack| stack.borrow().len())
}

fn context_stack_unwind_to(depth: usize, exc_bits: u64) {
    let contexts = CONTEXT_STACK.with(|stack| {
        let mut stack = stack.borrow_mut();
        if depth > stack.len() {
            return Err("context manager stack underflow");
        }
        let tail = stack.split_off(depth);
        Ok(tail)
    });
    match contexts {
        Ok(contexts) => {
            for bits in contexts.into_iter().rev() {
                unsafe { context_exit_unchecked(bits, exc_bits) };
                dec_ref_bits(bits);
            }
        }
        Err(msg) => return raise_exception::<_>("RuntimeError", msg),
    }
}

fn context_stack_unwind(exc_bits: u64) {
    context_stack_unwind_to(0, exc_bits);
}

fn file_handle_close_ptr(ptr: *mut u8) -> bool {
    if ptr.is_null() {
        return false;
    }
    unsafe {
        let handle_ptr = file_handle_ptr(ptr);
        if handle_ptr.is_null() {
            return false;
        }
        let handle = &mut *handle_ptr;
        if handle.closed {
            return false;
        }
        handle.closed = true;
        if !handle.owns_fd {
            return false;
        }
        let mut guard = handle.state.file.lock().unwrap();
        guard.take().is_some()
    }
}

unsafe fn file_handle_enter(ptr: *mut u8) -> u64 {
    let bits = MoltObject::from_ptr(ptr).bits();
    let handle_ptr = file_handle_ptr(ptr);
    if !handle_ptr.is_null() {
        let handle = &mut *handle_ptr;
        file_handle_require_attached!(handle);
        if file_handle_is_closed(handle) {
            return raise_exception::<_>("ValueError", "I/O operation on closed file");
        }
        handle.closed = false;
    }
    inc_ref_bits(bits);
    bits
}

unsafe fn file_handle_exit(ptr: *mut u8, _exc_bits: u64) -> u64 {
    let handle_ptr = file_handle_ptr(ptr);
    if !handle_ptr.is_null() {
        let handle = &mut *handle_ptr;
        file_handle_require_attached!(handle);
        file_handle_close_ptr(ptr);
        handle.closed = true;
    }
    MoltObject::from_bool(false).bits()
}

fn close_payload(payload_bits: u64) {
    let payload = obj_from_bits(payload_bits);
    let Some(ptr) = payload.as_ptr() else {
        return raise_exception::<_>("AttributeError", "object has no attribute 'close'");
    };
    unsafe {
        if object_type_id(ptr) == TYPE_ID_FILE_HANDLE {
            let handle_ptr = file_handle_ptr(ptr);
            if !handle_ptr.is_null() {
                let handle = &*handle_ptr;
                file_handle_require_attached!(handle);
            }
            file_handle_close_ptr(ptr);
            return;
        }
    }
    return raise_exception::<_>("AttributeError", "object has no attribute 'close'");
}

pub(crate) fn frame_stack_push(code_bits: u64) {
    if code_bits != 0 {
        inc_ref_bits(code_bits);
    }
    let line = if let Some(ptr) = obj_from_bits(code_bits).as_ptr() {
        unsafe {
            if object_type_id(ptr) == TYPE_ID_CODE {
                code_firstlineno(ptr)
            } else {
                0
            }
        }
    } else {
        0
    };
    FRAME_STACK.with(|stack| {
        stack.borrow_mut().push(FrameEntry { code_bits, line });
    });
}

fn frame_stack_set_line(line: i64) {
    FRAME_STACK.with(|stack| {
        if let Some(entry) = stack.borrow_mut().last_mut() {
            entry.line = line;
        }
    });
}

pub(crate) fn frame_stack_pop() {
    FRAME_STACK.with(|stack| {
        if let Some(entry) = stack.borrow_mut().pop() {
            if entry.code_bits != 0 {
                dec_ref_bits(entry.code_bits);
            }
        }
    });
}

unsafe fn alloc_frame_obj(code_bits: u64, line: i64) -> Option<u64> {
    // TODO(introspection, owner:runtime, milestone:TC2, priority:P1, status:partial): add full frame fields (f_back, f_globals, f_locals) and keep f_lasti/f_lineno live-updated.
    let builtins = builtin_classes();
    let class_obj = obj_from_bits(builtins.frame);
    let class_ptr = class_obj.as_ptr()?;
    if object_type_id(class_ptr) != TYPE_ID_TYPE {
        return None;
    }
    let frame_bits = alloc_instance_for_class(class_ptr);
    let frame_ptr = obj_from_bits(frame_bits).as_ptr()?;
    let f_code_bits = intern_static_name(&runtime_state().interned.f_code_name, b"f_code");
    let f_lineno_bits = intern_static_name(&runtime_state().interned.f_lineno_name, b"f_lineno");
    let f_lasti_bits = intern_static_name(&runtime_state().interned.f_lasti_name, b"f_lasti");
    let line_bits = MoltObject::from_int(line).bits();
    let lasti_bits = MoltObject::from_int(-1).bits();
    let dict_ptr = alloc_dict_with_pairs(&[
        f_code_bits,
        code_bits,
        f_lineno_bits,
        line_bits,
        f_lasti_bits,
        lasti_bits,
    ]);
    if dict_ptr.is_null() {
        dec_ref_bits(frame_bits);
        return None;
    }
    let dict_bits = MoltObject::from_ptr(dict_ptr).bits();
    instance_set_dict_bits(frame_ptr, dict_bits);
    object_mark_has_ptrs(frame_ptr);
    Some(frame_bits)
}

unsafe fn alloc_traceback_obj(frame_bits: u64, line: i64, next_bits: u64) -> Option<u64> {
    let builtins = builtin_classes();
    let class_obj = obj_from_bits(builtins.traceback);
    let class_ptr = class_obj.as_ptr()?;
    if object_type_id(class_ptr) != TYPE_ID_TYPE {
        return None;
    }
    let tb_bits = alloc_instance_for_class(class_ptr);
    let tb_ptr = obj_from_bits(tb_bits).as_ptr()?;
    let tb_frame_bits = intern_static_name(&runtime_state().interned.tb_frame_name, b"tb_frame");
    let tb_lineno_bits = intern_static_name(&runtime_state().interned.tb_lineno_name, b"tb_lineno");
    let tb_next_bits = intern_static_name(&runtime_state().interned.tb_next_name, b"tb_next");
    let line_bits = MoltObject::from_int(line).bits();
    let dict_ptr = alloc_dict_with_pairs(&[
        tb_frame_bits,
        frame_bits,
        tb_lineno_bits,
        line_bits,
        tb_next_bits,
        next_bits,
    ]);
    if dict_ptr.is_null() {
        dec_ref_bits(tb_bits);
        return None;
    }
    let dict_bits = MoltObject::from_ptr(dict_ptr).bits();
    instance_set_dict_bits(tb_ptr, dict_bits);
    object_mark_has_ptrs(tb_ptr);
    Some(tb_bits)
}

fn frame_stack_trace_bits() -> Option<u64> {
    let entries = FRAME_STACK.with(|stack| stack.borrow().clone());
    if entries.is_empty() {
        return None;
    }
    let mut next_bits = MoltObject::none().bits();
    let mut built_any = false;
    for entry in entries.into_iter().rev() {
        if entry.code_bits == 0 {
            continue;
        }
        let Some(code_ptr) = obj_from_bits(entry.code_bits).as_ptr() else {
            continue;
        };
        unsafe {
            if object_type_id(code_ptr) != TYPE_ID_CODE {
                continue;
            }
            let mut line = entry.line;
            if line <= 0 {
                line = code_firstlineno(code_ptr);
            }
            let Some(frame_bits) = alloc_frame_obj(entry.code_bits, line) else {
                if !obj_from_bits(next_bits).is_none() {
                    dec_ref_bits(next_bits);
                }
                return None;
            };
            let Some(tb_bits) = alloc_traceback_obj(frame_bits, line, next_bits) else {
                dec_ref_bits(frame_bits);
                if !obj_from_bits(next_bits).is_none() {
                    dec_ref_bits(next_bits);
                }
                return None;
            };
            dec_ref_bits(frame_bits);
            if !obj_from_bits(next_bits).is_none() {
                dec_ref_bits(next_bits);
            }
            next_bits = tb_bits;
            built_any = true;
        }
    }
    if !built_any || obj_from_bits(next_bits).is_none() {
        if !obj_from_bits(next_bits).is_none() {
            dec_ref_bits(next_bits);
        }
        return None;
    }
    Some(next_bits)
}

fn recursion_limit_get() -> usize {
    RECURSION_LIMIT.with(|limit| limit.get())
}

fn recursion_limit_set(limit: usize) {
    RECURSION_LIMIT.with(|cell| cell.set(limit));
}

pub(crate) fn recursion_guard_enter() -> bool {
    let limit = recursion_limit_get();
    RECURSION_DEPTH.with(|depth| {
        let current = depth.get();
        if current + 1 > limit {
            false
        } else {
            depth.set(current + 1);
            true
        }
    })
}

pub(crate) fn recursion_guard_exit() {
    RECURSION_DEPTH.with(|depth| {
        let current = depth.get();
        if current > 0 {
            depth.set(current - 1);
        }
    });
}

fn intern_static_name(slot: &AtomicU64, name: &'static [u8]) -> u64 {
    init_atomic_bits(slot, || {
        let ptr = alloc_string(name);
        if ptr.is_null() {
            MoltObject::none().bits()
        } else {
            MoltObject::from_ptr(ptr).bits()
        }
    })
}

fn builtin_func_bits(slot: &AtomicU64, fn_ptr: u64, arity: u64) -> u64 {
    builtin_func_bits_with_default(slot, fn_ptr, arity, 0)
}

fn builtin_func_bits_with_default(
    slot: &AtomicU64,
    fn_ptr: u64,
    arity: u64,
    default_kind: i64,
) -> u64 {
    init_atomic_bits(slot, || {
        let ptr = alloc_function_obj(fn_ptr, arity);
        if ptr.is_null() {
            MoltObject::none().bits()
        } else {
            if default_kind != 0 {
                let bits = MoltObject::from_int(default_kind).bits();
                unsafe {
                    function_set_dict_bits(ptr, bits);
                }
            }
            MoltObject::from_ptr(ptr).bits()
        }
    })
}

fn builtin_classmethod_bits(slot: &AtomicU64, fn_ptr: u64, arity: u64) -> u64 {
    init_atomic_bits(slot, || {
        let func_ptr = alloc_function_obj(fn_ptr, arity);
        if func_ptr.is_null() {
            return MoltObject::none().bits();
        }
        let func_bits = MoltObject::from_ptr(func_ptr).bits();
        let cm_ptr = alloc_classmethod_obj(func_bits);
        if cm_ptr.is_null() {
            dec_ref_bits(func_bits);
            return MoltObject::none().bits();
        }
        dec_ref_bits(func_bits);
        MoltObject::from_ptr(cm_ptr).bits()
    })
}

fn missing_bits() -> u64 {
    init_atomic_bits(&runtime_state().special_cache.molt_missing, || {
        let total_size = std::mem::size_of::<MoltHeader>();
        let ptr = alloc_object(total_size, TYPE_ID_OBJECT);
        if ptr.is_null() {
            MoltObject::none().bits()
        } else {
            MoltObject::from_ptr(ptr).bits()
        }
    })
}

fn not_implemented_bits() -> u64 {
    init_atomic_bits(&runtime_state().special_cache.molt_not_implemented, || {
        let total_size = std::mem::size_of::<MoltHeader>();
        let ptr = alloc_object(total_size, TYPE_ID_NOT_IMPLEMENTED);
        if ptr.is_null() {
            MoltObject::none().bits()
        } else {
            MoltObject::from_ptr(ptr).bits()
        }
    })
}

fn is_not_implemented_bits(bits: u64) -> bool {
    if let Some(ptr) = maybe_ptr_from_bits(bits) {
        unsafe { object_type_id(ptr) == TYPE_ID_NOT_IMPLEMENTED }
    } else {
        false
    }
}

fn ellipsis_bits() -> u64 {
    init_atomic_bits(&runtime_state().special_cache.molt_ellipsis, || {
        let total_size = std::mem::size_of::<MoltHeader>();
        let ptr = alloc_object(total_size, TYPE_ID_ELLIPSIS);
        if ptr.is_null() {
            MoltObject::none().bits()
        } else {
            MoltObject::from_ptr(ptr).bits()
        }
    })
}

fn slice_method_bits(name: &str) -> Option<u64> {
    match name {
        "indices" => Some(builtin_func_bits(
            &runtime_state().method_cache.slice_indices,
            fn_addr!(molt_slice_indices),
            2,
        )),
        "__hash__" => Some(builtin_func_bits(
            &runtime_state().method_cache.slice_hash,
            fn_addr!(molt_slice_hash),
            1,
        )),
        "__eq__" => Some(builtin_func_bits(
            &runtime_state().method_cache.slice_eq,
            fn_addr!(molt_slice_eq),
            2,
        )),
        "__reduce__" => Some(builtin_func_bits(
            &runtime_state().method_cache.slice_reduce,
            fn_addr!(molt_slice_reduce),
            1,
        )),
        "__reduce_ex__" => Some(builtin_func_bits(
            &runtime_state().method_cache.slice_reduce_ex,
            fn_addr!(molt_slice_reduce_ex),
            2,
        )),
        _ => None,
    }
}

fn string_method_bits(name: &str) -> Option<u64> {
    match name {
        "__iter__" => Some(builtin_func_bits(
            &runtime_state().method_cache.str_iter,
            fn_addr!(molt_iter),
            1,
        )),
        "__len__" => Some(builtin_func_bits(
            &runtime_state().method_cache.str_len,
            fn_addr!(molt_len),
            1,
        )),
        "__contains__" => Some(builtin_func_bits(
            &runtime_state().method_cache.str_contains,
            fn_addr!(molt_contains),
            2,
        )),
        "count" => Some(builtin_func_bits(
            &runtime_state().method_cache.str_count,
            fn_addr!(molt_string_count_slice),
            6,
        )),
        "startswith" => Some(builtin_func_bits(
            &runtime_state().method_cache.str_startswith,
            fn_addr!(molt_string_startswith_slice),
            6,
        )),
        "endswith" => Some(builtin_func_bits(
            &runtime_state().method_cache.str_endswith,
            fn_addr!(molt_string_endswith_slice),
            6,
        )),
        "find" => Some(builtin_func_bits(
            &runtime_state().method_cache.str_find,
            fn_addr!(molt_string_find_slice),
            6,
        )),
        "rfind" => Some(builtin_func_bits(
            &runtime_state().method_cache.str_rfind,
            fn_addr!(molt_string_rfind_slice),
            6,
        )),
        "format" => Some(builtin_func_bits(
            &runtime_state().method_cache.str_format,
            fn_addr!(molt_string_format_method),
            3,
        )),
        "upper" => Some(builtin_func_bits(
            &runtime_state().method_cache.str_upper,
            fn_addr!(molt_string_upper),
            1,
        )),
        "lower" => Some(builtin_func_bits(
            &runtime_state().method_cache.str_lower,
            fn_addr!(molt_string_lower),
            1,
        )),
        "strip" => Some(builtin_func_bits_with_default(
            &runtime_state().method_cache.str_strip,
            fn_addr!(molt_string_strip),
            2,
            FUNC_DEFAULT_NONE,
        )),
        "lstrip" => Some(builtin_func_bits_with_default(
            &runtime_state().method_cache.str_lstrip,
            fn_addr!(molt_string_lstrip),
            2,
            FUNC_DEFAULT_NONE,
        )),
        "rstrip" => Some(builtin_func_bits_with_default(
            &runtime_state().method_cache.str_rstrip,
            fn_addr!(molt_string_rstrip),
            2,
            FUNC_DEFAULT_NONE,
        )),
        "split" => Some(builtin_func_bits(
            &runtime_state().method_cache.str_split,
            fn_addr!(molt_string_split_max),
            3,
        )),
        "rsplit" => Some(builtin_func_bits(
            &runtime_state().method_cache.str_rsplit,
            fn_addr!(molt_string_rsplit_max),
            3,
        )),
        "splitlines" => Some(builtin_func_bits_with_default(
            &runtime_state().method_cache.str_splitlines,
            fn_addr!(molt_string_splitlines),
            2,
            FUNC_DEFAULT_NONE,
        )),
        "partition" => Some(builtin_func_bits(
            &runtime_state().method_cache.str_partition,
            fn_addr!(molt_string_partition),
            2,
        )),
        "rpartition" => Some(builtin_func_bits(
            &runtime_state().method_cache.str_rpartition,
            fn_addr!(molt_string_rpartition),
            2,
        )),
        "replace" => Some(builtin_func_bits_with_default(
            &runtime_state().method_cache.str_replace,
            fn_addr!(molt_string_replace),
            4,
            FUNC_DEFAULT_REPLACE_COUNT,
        )),
        "join" => Some(builtin_func_bits(
            &runtime_state().method_cache.str_join,
            fn_addr!(molt_string_join),
            2,
        )),
        "encode" => Some(builtin_func_bits(
            &runtime_state().method_cache.str_encode,
            fn_addr!(molt_string_encode),
            3,
        )),
        _ => None,
    }
}

fn bytes_method_bits(name: &str) -> Option<u64> {
    match name {
        "__iter__" => Some(builtin_func_bits(
            &runtime_state().method_cache.bytes_iter,
            fn_addr!(molt_iter),
            1,
        )),
        "__len__" => Some(builtin_func_bits(
            &runtime_state().method_cache.bytes_len,
            fn_addr!(molt_len),
            1,
        )),
        "__contains__" => Some(builtin_func_bits(
            &runtime_state().method_cache.bytes_contains,
            fn_addr!(molt_contains),
            2,
        )),
        "count" => Some(builtin_func_bits(
            &runtime_state().method_cache.bytes_count,
            fn_addr!(molt_bytes_count_slice),
            6,
        )),
        "find" => Some(builtin_func_bits(
            &runtime_state().method_cache.bytes_find,
            fn_addr!(molt_bytes_find_slice),
            6,
        )),
        "rfind" => Some(builtin_func_bits(
            &runtime_state().method_cache.bytes_rfind,
            fn_addr!(molt_bytes_rfind_slice),
            6,
        )),
        "split" => Some(builtin_func_bits(
            &runtime_state().method_cache.bytes_split,
            fn_addr!(molt_bytes_split_max),
            3,
        )),
        "rsplit" => Some(builtin_func_bits(
            &runtime_state().method_cache.bytes_rsplit,
            fn_addr!(molt_bytes_rsplit_max),
            3,
        )),
        "strip" => Some(builtin_func_bits_with_default(
            &runtime_state().method_cache.bytes_strip,
            fn_addr!(molt_bytes_strip),
            2,
            FUNC_DEFAULT_NONE,
        )),
        "lstrip" => Some(builtin_func_bits_with_default(
            &runtime_state().method_cache.bytes_lstrip,
            fn_addr!(molt_bytes_lstrip),
            2,
            FUNC_DEFAULT_NONE,
        )),
        "rstrip" => Some(builtin_func_bits_with_default(
            &runtime_state().method_cache.bytes_rstrip,
            fn_addr!(molt_bytes_rstrip),
            2,
            FUNC_DEFAULT_NONE,
        )),
        "startswith" => Some(builtin_func_bits(
            &runtime_state().method_cache.bytes_startswith,
            fn_addr!(molt_bytes_startswith_slice),
            6,
        )),
        "endswith" => Some(builtin_func_bits(
            &runtime_state().method_cache.bytes_endswith,
            fn_addr!(molt_bytes_endswith_slice),
            6,
        )),
        "__reversed__" => Some(builtin_func_bits(
            &runtime_state().method_cache.bytes_reversed,
            fn_addr!(molt_reversed_builtin),
            1,
        )),
        "splitlines" => Some(builtin_func_bits_with_default(
            &runtime_state().method_cache.bytes_splitlines,
            fn_addr!(molt_bytes_splitlines),
            2,
            FUNC_DEFAULT_NONE,
        )),
        "partition" => Some(builtin_func_bits(
            &runtime_state().method_cache.bytes_partition,
            fn_addr!(molt_bytes_partition),
            2,
        )),
        "rpartition" => Some(builtin_func_bits(
            &runtime_state().method_cache.bytes_rpartition,
            fn_addr!(molt_bytes_rpartition),
            2,
        )),
        "replace" => Some(builtin_func_bits_with_default(
            &runtime_state().method_cache.bytes_replace,
            fn_addr!(molt_bytes_replace),
            4,
            FUNC_DEFAULT_REPLACE_COUNT,
        )),
        "decode" => Some(builtin_func_bits(
            &runtime_state().method_cache.bytes_decode,
            fn_addr!(molt_bytes_decode),
            3,
        )),
        _ => None,
    }
}

fn bytearray_method_bits(name: &str) -> Option<u64> {
    match name {
        "__iter__" => Some(builtin_func_bits(
            &runtime_state().method_cache.bytearray_iter,
            fn_addr!(molt_iter),
            1,
        )),
        "__len__" => Some(builtin_func_bits(
            &runtime_state().method_cache.bytearray_len,
            fn_addr!(molt_len),
            1,
        )),
        "__contains__" => Some(builtin_func_bits(
            &runtime_state().method_cache.bytearray_contains,
            fn_addr!(molt_contains),
            2,
        )),
        "count" => Some(builtin_func_bits(
            &runtime_state().method_cache.bytearray_count,
            fn_addr!(molt_bytearray_count_slice),
            6,
        )),
        "find" => Some(builtin_func_bits(
            &runtime_state().method_cache.bytearray_find,
            fn_addr!(molt_bytearray_find_slice),
            6,
        )),
        "rfind" => Some(builtin_func_bits(
            &runtime_state().method_cache.bytearray_rfind,
            fn_addr!(molt_bytearray_rfind_slice),
            6,
        )),
        "split" => Some(builtin_func_bits(
            &runtime_state().method_cache.bytearray_split,
            fn_addr!(molt_bytearray_split_max),
            3,
        )),
        "rsplit" => Some(builtin_func_bits(
            &runtime_state().method_cache.bytearray_rsplit,
            fn_addr!(molt_bytearray_rsplit_max),
            3,
        )),
        "strip" => Some(builtin_func_bits_with_default(
            &runtime_state().method_cache.bytearray_strip,
            fn_addr!(molt_bytearray_strip),
            2,
            FUNC_DEFAULT_NONE,
        )),
        "lstrip" => Some(builtin_func_bits_with_default(
            &runtime_state().method_cache.bytearray_lstrip,
            fn_addr!(molt_bytearray_lstrip),
            2,
            FUNC_DEFAULT_NONE,
        )),
        "rstrip" => Some(builtin_func_bits_with_default(
            &runtime_state().method_cache.bytearray_rstrip,
            fn_addr!(molt_bytearray_rstrip),
            2,
            FUNC_DEFAULT_NONE,
        )),
        "startswith" => Some(builtin_func_bits(
            &runtime_state().method_cache.bytearray_startswith,
            fn_addr!(molt_bytearray_startswith_slice),
            6,
        )),
        "endswith" => Some(builtin_func_bits(
            &runtime_state().method_cache.bytearray_endswith,
            fn_addr!(molt_bytearray_endswith_slice),
            6,
        )),
        "__reversed__" => Some(builtin_func_bits(
            &runtime_state().method_cache.bytearray_reversed,
            fn_addr!(molt_reversed_builtin),
            1,
        )),
        "__setitem__" => Some(builtin_func_bits(
            &runtime_state().method_cache.bytearray_setitem,
            fn_addr!(molt_setitem_method),
            3,
        )),
        "__delitem__" => Some(builtin_func_bits(
            &runtime_state().method_cache.bytearray_delitem,
            fn_addr!(molt_delitem_method),
            2,
        )),
        "splitlines" => Some(builtin_func_bits_with_default(
            &runtime_state().method_cache.bytearray_splitlines,
            fn_addr!(molt_bytearray_splitlines),
            2,
            FUNC_DEFAULT_NONE,
        )),
        "partition" => Some(builtin_func_bits(
            &runtime_state().method_cache.bytearray_partition,
            fn_addr!(molt_bytearray_partition),
            2,
        )),
        "rpartition" => Some(builtin_func_bits(
            &runtime_state().method_cache.bytearray_rpartition,
            fn_addr!(molt_bytearray_rpartition),
            2,
        )),
        "replace" => Some(builtin_func_bits_with_default(
            &runtime_state().method_cache.bytearray_replace,
            fn_addr!(molt_bytearray_replace),
            4,
            FUNC_DEFAULT_REPLACE_COUNT,
        )),
        "decode" => Some(builtin_func_bits(
            &runtime_state().method_cache.bytearray_decode,
            fn_addr!(molt_bytearray_decode),
            3,
        )),
        _ => None,
    }
}

fn builtin_class_method_bits(class_bits: u64, name: &str) -> Option<u64> {
    let builtins = builtin_classes();
    if name == "__class_getitem__" {
        if class_bits == builtins.list
            || class_bits == builtins.dict
            || class_bits == builtins.tuple
            || class_bits == builtins.set
            || class_bits == builtins.frozenset
            || class_bits == builtins.type_obj
        {
            return Some(builtin_classmethod_bits(
                &runtime_state().method_cache.generic_alias_class_getitem,
                fn_addr!(molt_generic_alias_new),
                2,
            ));
        }
    }
    if class_bits == builtins.object {
        return object_method_bits(name);
    }
    if class_bits == builtins.base_exception || class_bits == builtins.exception {
        return exception_method_bits(name);
    }
    if class_bits == builtins.dict {
        return dict_method_bits(name);
    }
    if class_bits == builtins.list {
        return list_method_bits(name);
    }
    if class_bits == builtins.set {
        return set_method_bits(name);
    }
    if class_bits == builtins.frozenset {
        return frozenset_method_bits(name);
    }
    if class_bits == builtins.str {
        return string_method_bits(name);
    }
    if class_bits == builtins.bytes {
        return bytes_method_bits(name);
    }
    if class_bits == builtins.bytearray {
        return bytearray_method_bits(name);
    }
    if class_bits == builtins.slice {
        return slice_method_bits(name);
    }
    if class_bits == builtins.memoryview {
        return memoryview_method_bits(name);
    }
    if class_bits == builtins.file
        || class_bits == builtins.file_io
        || class_bits == builtins.buffered_reader
        || class_bits == builtins.buffered_writer
        || class_bits == builtins.buffered_random
        || class_bits == builtins.text_io_wrapper
    {
        return file_method_bits(name);
    }
    None
}

fn object_method_bits(name: &str) -> Option<u64> {
    match name {
        "__getattribute__" => Some(builtin_func_bits(
            &runtime_state().method_cache.object_getattribute,
            fn_addr!(molt_object_getattribute),
            2,
        )),
        "__init__" => Some(builtin_func_bits(
            &runtime_state().method_cache.object_init,
            fn_addr!(molt_object_init),
            1,
        )),
        "__setattr__" => Some(builtin_func_bits(
            &runtime_state().method_cache.object_setattr,
            fn_addr!(molt_object_setattr),
            3,
        )),
        "__delattr__" => Some(builtin_func_bits(
            &runtime_state().method_cache.object_delattr,
            fn_addr!(molt_object_delattr),
            2,
        )),
        "__eq__" => Some(builtin_func_bits(
            &runtime_state().method_cache.object_eq,
            fn_addr!(molt_object_eq),
            2,
        )),
        "__ne__" => Some(builtin_func_bits(
            &runtime_state().method_cache.object_ne,
            fn_addr!(molt_object_ne),
            2,
        )),
        _ => None,
    }
}

fn memoryview_method_bits(name: &str) -> Option<u64> {
    match name {
        "tobytes" => Some(builtin_func_bits(
            &runtime_state().method_cache.memoryview_tobytes,
            fn_addr!(molt_memoryview_tobytes),
            1,
        )),
        "cast" => Some(builtin_func_bits(
            &runtime_state().method_cache.memoryview_cast,
            fn_addr!(molt_memoryview_cast),
            4,
        )),
        "__setitem__" => Some(builtin_func_bits(
            &runtime_state().method_cache.memoryview_setitem,
            fn_addr!(molt_setitem_method),
            3,
        )),
        "__delitem__" => Some(builtin_func_bits(
            &runtime_state().method_cache.memoryview_delitem,
            fn_addr!(molt_delitem_method),
            2,
        )),
        _ => None,
    }
}

fn file_method_bits(name: &str) -> Option<u64> {
    // TODO(stdlib-compat, owner:runtime, milestone:SL1): add remaining file APIs
    // (readinto1, encoding/errors lookups) once buffer/encoding layers are
    // fully implemented.
    match name {
        "read" => Some(builtin_func_bits_with_default(
            &runtime_state().method_cache.file_read,
            fn_addr!(molt_file_read),
            2,
            FUNC_DEFAULT_NEG_ONE,
        )),
        "readline" => Some(builtin_func_bits_with_default(
            &runtime_state().method_cache.file_readline,
            fn_addr!(molt_file_readline),
            2,
            FUNC_DEFAULT_NEG_ONE,
        )),
        "readlines" => Some(builtin_func_bits_with_default(
            &runtime_state().method_cache.file_readlines,
            fn_addr!(molt_file_readlines),
            2,
            FUNC_DEFAULT_NEG_ONE,
        )),
        "readinto" => Some(builtin_func_bits(
            &runtime_state().method_cache.file_readinto,
            fn_addr!(molt_file_readinto),
            2,
        )),
        "write" => Some(builtin_func_bits(
            &runtime_state().method_cache.file_write,
            fn_addr!(molt_file_write),
            2,
        )),
        "writelines" => Some(builtin_func_bits(
            &runtime_state().method_cache.file_writelines,
            fn_addr!(molt_file_writelines),
            2,
        )),
        "flush" => Some(builtin_func_bits(
            &runtime_state().method_cache.file_flush,
            fn_addr!(molt_file_flush),
            1,
        )),
        "close" => Some(builtin_func_bits(
            &runtime_state().method_cache.file_close,
            fn_addr!(molt_file_close),
            1,
        )),
        "detach" => Some(builtin_func_bits(
            &runtime_state().method_cache.file_detach,
            fn_addr!(molt_file_detach),
            1,
        )),
        "reconfigure" => Some(builtin_func_bits(
            &runtime_state().method_cache.file_reconfigure,
            fn_addr!(molt_file_reconfigure),
            6,
        )),
        "seek" => Some(builtin_func_bits_with_default(
            &runtime_state().method_cache.file_seek,
            fn_addr!(molt_file_seek),
            3,
            FUNC_DEFAULT_ZERO,
        )),
        "tell" => Some(builtin_func_bits(
            &runtime_state().method_cache.file_tell,
            fn_addr!(molt_file_tell),
            1,
        )),
        "fileno" => Some(builtin_func_bits(
            &runtime_state().method_cache.file_fileno,
            fn_addr!(molt_file_fileno),
            1,
        )),
        "truncate" => Some(builtin_func_bits_with_default(
            &runtime_state().method_cache.file_truncate,
            fn_addr!(molt_file_truncate),
            2,
            FUNC_DEFAULT_NONE,
        )),
        "readable" => Some(builtin_func_bits(
            &runtime_state().method_cache.file_readable,
            fn_addr!(molt_file_readable),
            1,
        )),
        "writable" => Some(builtin_func_bits(
            &runtime_state().method_cache.file_writable,
            fn_addr!(molt_file_writable),
            1,
        )),
        "seekable" => Some(builtin_func_bits(
            &runtime_state().method_cache.file_seekable,
            fn_addr!(molt_file_seekable),
            1,
        )),
        "isatty" => Some(builtin_func_bits(
            &runtime_state().method_cache.file_isatty,
            fn_addr!(molt_file_isatty),
            1,
        )),
        "__iter__" => Some(builtin_func_bits(
            &runtime_state().method_cache.file_iter,
            fn_addr!(molt_file_iter),
            1,
        )),
        "__next__" => Some(builtin_func_bits(
            &runtime_state().method_cache.file_next,
            fn_addr!(molt_file_next),
            1,
        )),
        "__enter__" => Some(builtin_func_bits(
            &runtime_state().method_cache.file_enter,
            fn_addr!(molt_file_enter),
            1,
        )),
        "__exit__" => Some(builtin_func_bits(
            &runtime_state().method_cache.file_exit,
            fn_addr!(molt_file_exit_method),
            4,
        )),
        _ => None,
    }
}

fn asyncgen_method_bits(name: &str) -> Option<u64> {
    match name {
        "__aiter__" => Some(builtin_func_bits(
            &runtime_state().method_cache.asyncgen_aiter,
            fn_addr!(molt_asyncgen_aiter),
            1,
        )),
        "__anext__" => Some(builtin_func_bits(
            &runtime_state().method_cache.asyncgen_anext,
            fn_addr!(molt_asyncgen_anext),
            1,
        )),
        "asend" => Some(builtin_func_bits(
            &runtime_state().method_cache.asyncgen_asend,
            fn_addr!(molt_asyncgen_asend),
            2,
        )),
        "athrow" => Some(builtin_func_bits(
            &runtime_state().method_cache.asyncgen_athrow,
            fn_addr!(molt_asyncgen_athrow),
            2,
        )),
        "aclose" => Some(builtin_func_bits(
            &runtime_state().method_cache.asyncgen_aclose,
            fn_addr!(molt_asyncgen_aclose),
            1,
        )),
        _ => None,
    }
}

unsafe fn class_mro_ref(class_ptr: *mut u8) -> Option<&'static Vec<u64>> {
    let mro_bits = class_mro_bits(class_ptr);
    let mro_obj = obj_from_bits(mro_bits);
    let mro_ptr = mro_obj.as_ptr()?;
    if object_type_id(mro_ptr) != TYPE_ID_TUPLE {
        return None;
    }
    Some(seq_vec_ref(mro_ptr))
}

fn class_mro_vec(class_bits: u64) -> Vec<u64> {
    let obj = obj_from_bits(class_bits);
    let Some(ptr) = obj.as_ptr() else {
        return vec![class_bits];
    };
    unsafe {
        if object_type_id(ptr) != TYPE_ID_TYPE {
            return vec![class_bits];
        }
        if let Some(mro) = class_mro_ref(ptr) {
            return mro.clone();
        }
        let mut out = vec![class_bits];
        let bases_bits = class_bases_bits(ptr);
        let bases = class_bases_vec(bases_bits);
        for base in bases {
            out.extend(class_mro_vec(base));
        }
        out
    }
}

fn class_bases_vec(bits: u64) -> Vec<u64> {
    let obj = obj_from_bits(bits);
    if obj.is_none() || bits == 0 {
        return Vec::new();
    }
    if let Some(ptr) = obj.as_ptr() {
        unsafe {
            match object_type_id(ptr) {
                TYPE_ID_TYPE => return vec![bits],
                TYPE_ID_TUPLE => return seq_vec_ref(ptr).clone(),
                _ => {}
            }
        }
    }
    Vec::new()
}

fn type_of_bits(val_bits: u64) -> u64 {
    let builtins = builtin_classes();
    let obj = obj_from_bits(val_bits);
    if obj.is_none() {
        return builtins.none_type;
    }
    if obj.is_bool() {
        return builtins.bool;
    }
    if obj.is_int() {
        return builtins.int;
    }
    if obj.is_float() {
        return builtins.float;
    }
    if let Some(ptr) = obj.as_ptr() {
        unsafe {
            return match object_type_id(ptr) {
                TYPE_ID_STRING => builtins.str,
                TYPE_ID_BYTES => builtins.bytes,
                TYPE_ID_BYTEARRAY => builtins.bytearray,
                TYPE_ID_LIST => builtins.list,
                TYPE_ID_TUPLE => builtins.tuple,
                TYPE_ID_DICT => builtins.dict,
                TYPE_ID_SET => builtins.set,
                TYPE_ID_FROZENSET => builtins.frozenset,
                TYPE_ID_BIGINT => builtins.int,
                TYPE_ID_RANGE => builtins.range,
                TYPE_ID_SLICE => builtins.slice,
                TYPE_ID_MEMORYVIEW => builtins.memoryview,
                TYPE_ID_FILE_HANDLE => {
                    let handle_ptr = file_handle_ptr(ptr);
                    if !handle_ptr.is_null() {
                        let handle = &*handle_ptr;
                        if handle.class_bits != 0 {
                            return handle.class_bits;
                        }
                    }
                    builtins.file
                }
                TYPE_ID_NOT_IMPLEMENTED => builtins.not_implemented_type,
                TYPE_ID_ELLIPSIS => builtins.ellipsis_type,
                TYPE_ID_EXCEPTION => {
                    let class_bits = exception_class_bits(ptr);
                    if !obj_from_bits(class_bits).is_none() && class_bits != 0 {
                        class_bits
                    } else {
                        exception_type_bits(exception_kind_bits(ptr))
                    }
                }
                TYPE_ID_FUNCTION => builtins.function,
                TYPE_ID_CODE => builtins.code,
                TYPE_ID_MODULE => builtins.module,
                TYPE_ID_TYPE => builtins.type_obj,
                TYPE_ID_GENERIC_ALIAS => builtins.generic_alias,
                TYPE_ID_SUPER => builtins.super_type,
                TYPE_ID_DATACLASS => {
                    let desc_ptr = dataclass_desc_ptr(ptr);
                    if !desc_ptr.is_null() {
                        let class_bits = (*desc_ptr).class_bits;
                        if class_bits != 0 {
                            return class_bits;
                        }
                    }
                    builtins.object
                }
                TYPE_ID_OBJECT => {
                    let class_bits = object_class_bits(ptr);
                    if class_bits != 0 {
                        class_bits
                    } else {
                        builtins.object
                    }
                }
                _ => builtins.object,
            };
        }
    }
    if let Some(ptr) = maybe_ptr_from_bits(val_bits) {
        unsafe {
            let class_bits = object_class_bits(ptr);
            if class_bits != 0 {
                return class_bits;
            }
        }
    }
    builtins.object
}

fn collect_classinfo_isinstance(class_bits: u64, out: &mut Vec<u64>) {
    let obj = obj_from_bits(class_bits);
    let Some(ptr) = obj.as_ptr() else {
        return raise_exception::<_>(
            "TypeError",
            "isinstance() arg 2 must be a type or tuple of types",
        );
    };
    unsafe {
        match object_type_id(ptr) {
            TYPE_ID_TYPE => out.push(class_bits),
            TYPE_ID_TUPLE => {
                let items = seq_vec_ref(ptr);
                for item in items.iter() {
                    collect_classinfo_isinstance(*item, out);
                }
            }
            _ => {
                return raise_exception::<_>(
                    "TypeError",
                    "isinstance() arg 2 must be a type or tuple of types",
                )
            }
        }
    }
}

fn collect_classinfo_issubclass(class_bits: u64, out: &mut Vec<u64>) {
    let obj = obj_from_bits(class_bits);
    let Some(ptr) = obj.as_ptr() else {
        return raise_exception::<_>(
            "TypeError",
            "issubclass() arg 2 must be a class or tuple of classes",
        );
    };
    unsafe {
        match object_type_id(ptr) {
            TYPE_ID_TYPE => out.push(class_bits),
            TYPE_ID_TUPLE => {
                let items = seq_vec_ref(ptr);
                for item in items.iter() {
                    collect_classinfo_issubclass(*item, out);
                }
            }
            _ => {
                return raise_exception::<_>(
                    "TypeError",
                    "issubclass() arg 2 must be a class or tuple of classes",
                )
            }
        }
    }
}

fn issubclass_bits(sub_bits: u64, class_bits: u64) -> bool {
    if sub_bits == class_bits {
        return true;
    }
    let obj = obj_from_bits(sub_bits);
    let Some(ptr) = obj.as_ptr() else {
        return false;
    };
    unsafe {
        if object_type_id(ptr) != TYPE_ID_TYPE {
            return false;
        }
        if let Some(mro) = class_mro_ref(ptr) {
            return mro.contains(&class_bits);
        }
    }
    class_mro_vec(sub_bits).contains(&class_bits)
}

fn isinstance_bits(val_bits: u64, class_bits: u64) -> bool {
    let mut classes = Vec::new();
    collect_classinfo_isinstance(class_bits, &mut classes);
    let val_type = type_of_bits(val_bits);
    for class_bits in classes {
        if issubclass_bits(val_type, class_bits) {
            return true;
        }
    }
    false
}

fn alloc_dict_with_pairs(pairs: &[u64]) -> *mut u8 {
    let total = std::mem::size_of::<MoltHeader>()
        + std::mem::size_of::<*mut Vec<u64>>()
        + std::mem::size_of::<*mut Vec<usize>>();
    let ptr = alloc_object(total, TYPE_ID_DICT);
    if ptr.is_null() {
        return ptr;
    }
    unsafe {
        let order = Vec::with_capacity(pairs.len());
        let table = Vec::new();
        let order_ptr = Box::into_raw(Box::new(order));
        let table_ptr = Box::into_raw(Box::new(table));
        *(ptr as *mut *mut Vec<u64>) = order_ptr;
        *(ptr.add(std::mem::size_of::<*mut Vec<u64>>()) as *mut *mut Vec<usize>) = table_ptr;
        for pair in pairs.chunks(2) {
            if pair.len() == 2 {
                dict_set_in_place(ptr, pair[0], pair[1]);
            }
        }
    }
    ptr
}

fn alloc_set_like_with_entries(entries: &[u64], type_id: u32) -> *mut u8 {
    let total = std::mem::size_of::<MoltHeader>()
        + std::mem::size_of::<*mut Vec<u64>>()
        + std::mem::size_of::<*mut Vec<usize>>();
    let ptr = alloc_object(total, type_id);
    if ptr.is_null() {
        return ptr;
    }
    unsafe {
        let order = Vec::with_capacity(entries.len());
        let mut table = Vec::new();
        if !entries.is_empty() {
            table.resize(set_table_capacity(entries.len()), 0);
        }
        let order_ptr = Box::into_raw(Box::new(order));
        let table_ptr = Box::into_raw(Box::new(table));
        *(ptr as *mut *mut Vec<u64>) = order_ptr;
        *(ptr.add(std::mem::size_of::<*mut Vec<u64>>()) as *mut *mut Vec<usize>) = table_ptr;
        for &entry in entries {
            set_add_in_place(ptr, entry);
        }
    }
    ptr
}

fn alloc_set_with_entries(entries: &[u64]) -> *mut u8 {
    alloc_set_like_with_entries(entries, TYPE_ID_SET)
}

#[no_mangle]
pub extern "C" fn molt_header_size() -> u64 {
    std::mem::size_of::<MoltHeader>() as u64
}

#[no_mangle]
pub extern "C" fn molt_alloc(size_bits: u64) -> u64 {
    let size = usize_from_bits(size_bits);
    let total_size = size + std::mem::size_of::<MoltHeader>();
    let obj_ptr = alloc_object_zeroed_with_pool(total_size, TYPE_ID_OBJECT);
    if obj_ptr.is_null() {
        return MoltObject::none().bits();
    }
    MoltObject::from_ptr(obj_ptr).bits()
}

#[no_mangle]
pub extern "C" fn molt_alloc_class(size_bits: u64, class_bits: u64) -> u64 {
    let size = usize_from_bits(size_bits);
    let total_size = size + std::mem::size_of::<MoltHeader>();
    let obj_ptr = alloc_object_zeroed_with_pool(total_size, TYPE_ID_OBJECT);
    if obj_ptr.is_null() {
        return MoltObject::none().bits();
    }
    unsafe {
        if class_bits != 0 {
            let class_obj = obj_from_bits(class_bits);
            let Some(class_ptr) = class_obj.as_ptr() else {
                return raise_exception::<_>("TypeError", "class must be a type object");
            };
            if object_type_id(class_ptr) != TYPE_ID_TYPE {
                return raise_exception::<_>("TypeError", "class must be a type object");
            }
            object_set_class_bits(obj_ptr, class_bits);
            inc_ref_bits(class_bits);
        }
    }
    MoltObject::from_ptr(obj_ptr).bits()
}

#[no_mangle]
pub extern "C" fn molt_alloc_class_trusted(size_bits: u64, class_bits: u64) -> u64 {
    let size = usize_from_bits(size_bits);
    let total_size = size + std::mem::size_of::<MoltHeader>();
    let obj_ptr = alloc_object_zeroed_with_pool(total_size, TYPE_ID_OBJECT);
    if obj_ptr.is_null() {
        return MoltObject::none().bits();
    }
    unsafe {
        if class_bits != 0 {
            object_set_class_bits(obj_ptr, class_bits);
            inc_ref_bits(class_bits);
        }
    }
    MoltObject::from_ptr(obj_ptr).bits()
}

#[no_mangle]
pub extern "C" fn molt_alloc_class_static(size_bits: u64, class_bits: u64) -> u64 {
    let size = usize_from_bits(size_bits);
    let total_size = size + std::mem::size_of::<MoltHeader>();
    let obj_ptr = alloc_object_zeroed_with_pool(total_size, TYPE_ID_OBJECT);
    if obj_ptr.is_null() {
        return MoltObject::none().bits();
    }
    unsafe {
        if class_bits != 0 {
            object_set_class_bits(obj_ptr, class_bits);
        }
        let header = header_from_obj_ptr(obj_ptr);
        (*header).flags |= HEADER_FLAG_SKIP_CLASS_DECREF;
    }
    MoltObject::from_ptr(obj_ptr).bits()
}

// --- List Builder ---

#[no_mangle]
pub extern "C" fn molt_list_builder_new(capacity_bits: u64) -> u64 {
    // Allocate wrapper object
    let total = std::mem::size_of::<MoltHeader>() + std::mem::size_of::<*mut Vec<u64>>(); // Store pointer to Vec
    let ptr = alloc_object(total, TYPE_ID_LIST_BUILDER);
    if ptr.is_null() {
        return 0;
    }
    unsafe {
        let capacity_hint = usize_from_bits(capacity_bits);
        let vec = Box::new(Vec::<u64>::with_capacity(capacity_hint));
        let vec_ptr = Box::into_raw(vec);
        *(ptr as *mut *mut Vec<u64>) = vec_ptr;
    }
    bits_from_ptr(ptr)
}

pub(crate) struct PtrDropGuard {
    ptr: *mut u8,
    active: bool,
}

impl PtrDropGuard {
    pub(crate) fn new(ptr: *mut u8) -> Self {
        Self {
            ptr,
            active: !ptr.is_null(),
        }
    }

    pub(crate) fn release(&mut self) {
        self.active = false;
    }
}

impl Drop for PtrDropGuard {
    fn drop(&mut self) {
        if self.active && !self.ptr.is_null() {
            unsafe {
                molt_dec_ref(self.ptr);
            }
        }
    }
}

pub unsafe extern "C" fn molt_list_builder_append(builder_bits: u64, val: u64) {
    let builder_ptr = ptr_from_bits(builder_bits);
    if builder_ptr.is_null() {
        return;
    }
    let vec_ptr = *(builder_ptr as *mut *mut Vec<u64>);
    if vec_ptr.is_null() {
        return;
    }
    let vec = &mut *vec_ptr;
    vec.push(val);
}

#[no_mangle]
/// # Safety
/// Caller must ensure `builder_bits` is valid and points to a list builder.
pub unsafe extern "C" fn molt_list_builder_finish(builder_bits: u64) -> u64 {
    let builder_ptr = ptr_from_bits(builder_bits);
    if builder_ptr.is_null() {
        return MoltObject::none().bits();
    }
    let _guard = PtrDropGuard::new(builder_ptr);
    let vec_ptr = *(builder_ptr as *mut *mut Vec<u64>);
    if vec_ptr.is_null() {
        return MoltObject::none().bits();
    }
    *(builder_ptr as *mut *mut Vec<u64>) = std::ptr::null_mut();

    // Reconstruct Box to drop it later, but we need the data
    let vec = Box::from_raw(vec_ptr);
    let slice = vec.as_slice();
    let capacity = vec.capacity().max(MAX_SMALL_LIST);
    let list_ptr = alloc_list_with_capacity(slice, capacity);

    // Builder object will be cleaned up by GC/Ref counting eventually,
    // but the Vec heap allocation is owned by the Box we just reconstructed.
    // So dropping 'vec' here frees the temporary buffer. Correct.

    if list_ptr.is_null() {
        MoltObject::none().bits()
    } else {
        MoltObject::from_ptr(list_ptr).bits()
    }
}

#[no_mangle]
/// # Safety
/// Caller must ensure `builder_bits` is valid and points to a tuple builder.
pub unsafe extern "C" fn molt_tuple_builder_finish(builder_bits: u64) -> u64 {
    let builder_ptr = ptr_from_bits(builder_bits);
    if builder_ptr.is_null() {
        return MoltObject::none().bits();
    }
    let _guard = PtrDropGuard::new(builder_ptr);
    let vec_ptr = *(builder_ptr as *mut *mut Vec<u64>);
    if vec_ptr.is_null() {
        return MoltObject::none().bits();
    }
    *(builder_ptr as *mut *mut Vec<u64>) = std::ptr::null_mut();

    let vec = Box::from_raw(vec_ptr);
    let slice = vec.as_slice();
    let capacity = vec.capacity().max(MAX_SMALL_LIST);
    let tuple_ptr = alloc_tuple_with_capacity(slice, capacity);

    if tuple_ptr.is_null() {
        MoltObject::none().bits()
    } else {
        MoltObject::from_ptr(tuple_ptr).bits()
    }
}

#[no_mangle]
pub extern "C" fn molt_range_new(start_bits: u64, stop_bits: u64, step_bits: u64) -> u64 {
    let start = match to_i64(obj_from_bits(start_bits)) {
        Some(val) => val,
        None => return MoltObject::none().bits(),
    };
    let stop = match to_i64(obj_from_bits(stop_bits)) {
        Some(val) => val,
        None => return MoltObject::none().bits(),
    };
    let step = match to_i64(obj_from_bits(step_bits)) {
        Some(val) => val,
        None => return MoltObject::none().bits(),
    };
    if step == 0 {
        return raise_exception::<_>("ValueError", "range() arg 3 must not be zero");
    }
    let ptr = alloc_range(start, stop, step);
    if ptr.is_null() {
        MoltObject::none().bits()
    } else {
        MoltObject::from_ptr(ptr).bits()
    }
}

#[no_mangle]
pub extern "C" fn molt_slice_new(start_bits: u64, stop_bits: u64, step_bits: u64) -> u64 {
    let ptr = alloc_slice_obj(start_bits, stop_bits, step_bits);
    if ptr.is_null() {
        MoltObject::none().bits()
    } else {
        MoltObject::from_ptr(ptr).bits()
    }
}

fn slice_indices_adjust(mut idx: BigInt, len: &BigInt, lower: &BigInt, upper: &BigInt) -> BigInt {
    if idx.is_negative() {
        idx += len;
    }
    if idx < *lower {
        return lower.clone();
    }
    if idx > *upper {
        return upper.clone();
    }
    idx
}

fn slice_reduce_tuple(slice_ptr: *mut u8) -> u64 {
    unsafe {
        let start_bits = slice_start_bits(slice_ptr);
        let stop_bits = slice_stop_bits(slice_ptr);
        let step_bits = slice_step_bits(slice_ptr);
        let args_ptr = alloc_tuple(&[start_bits, stop_bits, step_bits]);
        if args_ptr.is_null() {
            return MoltObject::none().bits();
        }
        let args_bits = MoltObject::from_ptr(args_ptr).bits();
        let class_bits = builtin_classes().slice;
        let res_ptr = alloc_tuple(&[class_bits, args_bits]);
        if res_ptr.is_null() {
            return MoltObject::none().bits();
        }
        MoltObject::from_ptr(res_ptr).bits()
    }
}

#[no_mangle]
pub extern "C" fn molt_slice_indices(slice_bits: u64, length_bits: u64) -> u64 {
    let Some(slice_ptr) = obj_from_bits(slice_bits).as_ptr() else {
        return MoltObject::none().bits();
    };
    unsafe {
        if object_type_id(slice_ptr) != TYPE_ID_SLICE {
            return MoltObject::none().bits();
        }
        let msg = "slice indices must be integers or None or have an __index__ method";
        let Some(len) = index_bigint_from_obj(length_bits, msg) else {
            return MoltObject::none().bits();
        };
        if len.is_negative() {
            return raise_exception::<_>("ValueError", "length should not be negative");
        }
        let start_obj = obj_from_bits(slice_start_bits(slice_ptr));
        let stop_obj = obj_from_bits(slice_stop_bits(slice_ptr));
        let step_obj = obj_from_bits(slice_step_bits(slice_ptr));
        let step = if step_obj.is_none() {
            BigInt::from(1)
        } else {
            let Some(step_val) = index_bigint_from_obj(step_obj.bits(), msg) else {
                return MoltObject::none().bits();
            };
            step_val
        };
        if step.is_zero() {
            return raise_exception::<_>("ValueError", "slice step cannot be zero");
        }
        let step_neg = step.is_negative();
        let lower = if step_neg {
            BigInt::from(-1)
        } else {
            BigInt::from(0)
        };
        let upper = if step_neg { &len - 1 } else { len.clone() };
        let start = if start_obj.is_none() {
            if step_neg {
                upper.clone()
            } else {
                lower.clone()
            }
        } else {
            let Some(idx) = index_bigint_from_obj(start_obj.bits(), msg) else {
                return MoltObject::none().bits();
            };
            slice_indices_adjust(idx, &len, &lower, &upper)
        };
        let stop = if stop_obj.is_none() {
            if step_neg {
                lower.clone()
            } else {
                upper.clone()
            }
        } else {
            let Some(idx) = index_bigint_from_obj(stop_obj.bits(), msg) else {
                return MoltObject::none().bits();
            };
            slice_indices_adjust(idx, &len, &lower, &upper)
        };
        let start_bits = int_bits_from_bigint(start);
        let stop_bits = int_bits_from_bigint(stop);
        let step_bits = int_bits_from_bigint(step);
        let tuple_ptr = alloc_tuple(&[start_bits, stop_bits, step_bits]);
        if tuple_ptr.is_null() {
            return MoltObject::none().bits();
        }
        MoltObject::from_ptr(tuple_ptr).bits()
    }
}

#[no_mangle]
pub extern "C" fn molt_slice_hash(slice_bits: u64) -> u64 {
    let Some(slice_ptr) = obj_from_bits(slice_bits).as_ptr() else {
        return MoltObject::none().bits();
    };
    unsafe {
        if object_type_id(slice_ptr) != TYPE_ID_SLICE {
            return MoltObject::none().bits();
        }
        let start_bits = slice_start_bits(slice_ptr);
        let stop_bits = slice_stop_bits(slice_ptr);
        let step_bits = slice_step_bits(slice_ptr);
        let Some(hash) = hash_slice_bits(start_bits, stop_bits, step_bits) else {
            return MoltObject::none().bits();
        };
        int_bits_from_i64(hash)
    }
}

#[no_mangle]
pub extern "C" fn molt_slice_eq(slice_bits: u64, other_bits: u64) -> u64 {
    let Some(slice_ptr) = obj_from_bits(slice_bits).as_ptr() else {
        return not_implemented_bits();
    };
    let Some(other_ptr) = obj_from_bits(other_bits).as_ptr() else {
        return not_implemented_bits();
    };
    unsafe {
        if object_type_id(slice_ptr) != TYPE_ID_SLICE {
            return not_implemented_bits();
        }
        if object_type_id(other_ptr) != TYPE_ID_SLICE {
            return not_implemented_bits();
        }
        let start_eq = molt_eq(slice_start_bits(slice_ptr), slice_start_bits(other_ptr));
        if exception_pending() {
            return MoltObject::none().bits();
        }
        if !is_truthy(obj_from_bits(start_eq)) {
            return MoltObject::from_bool(false).bits();
        }
        let stop_eq = molt_eq(slice_stop_bits(slice_ptr), slice_stop_bits(other_ptr));
        if exception_pending() {
            return MoltObject::none().bits();
        }
        if !is_truthy(obj_from_bits(stop_eq)) {
            return MoltObject::from_bool(false).bits();
        }
        let step_eq = molt_eq(slice_step_bits(slice_ptr), slice_step_bits(other_ptr));
        if exception_pending() {
            return MoltObject::none().bits();
        }
        if !is_truthy(obj_from_bits(step_eq)) {
            return MoltObject::from_bool(false).bits();
        }
        MoltObject::from_bool(true).bits()
    }
}

#[no_mangle]
pub extern "C" fn molt_slice_reduce(slice_bits: u64) -> u64 {
    let Some(slice_ptr) = obj_from_bits(slice_bits).as_ptr() else {
        return MoltObject::none().bits();
    };
    unsafe {
        if object_type_id(slice_ptr) != TYPE_ID_SLICE {
            return MoltObject::none().bits();
        }
        slice_reduce_tuple(slice_ptr)
    }
}

#[no_mangle]
pub extern "C" fn molt_slice_reduce_ex(slice_bits: u64, _protocol_bits: u64) -> u64 {
    molt_slice_reduce(slice_bits)
}

#[no_mangle]
pub extern "C" fn molt_dataclass_new(
    name_bits: u64,
    field_names_bits: u64,
    values_bits: u64,
    flags_bits: u64,
) -> u64 {
    let name_obj = obj_from_bits(name_bits);
    let name = match string_obj_to_owned(name_obj) {
        Some(val) => val,
        None => return raise_exception::<_>("TypeError", "dataclass name must be a str"),
    };
    let field_names_obj = obj_from_bits(field_names_bits);
    let field_names = match decode_string_list(field_names_obj) {
        Some(val) => val,
        None => {
            return raise_exception::<_>(
                "TypeError",
                "dataclass field names must be a list/tuple of str",
            )
        }
    };
    let values_obj = obj_from_bits(values_bits);
    let values = match decode_value_list(values_obj) {
        Some(val) => val,
        None => return raise_exception::<_>("TypeError", "dataclass values must be a list/tuple"),
    };
    if field_names.len() != values.len() {
        return raise_exception::<_>("TypeError", "dataclass constructor argument mismatch");
    }
    let flags = to_i64(obj_from_bits(flags_bits)).unwrap_or(0) as u64;
    let frozen = (flags & 0x1) != 0;
    let eq = (flags & 0x2) != 0;
    let repr = (flags & 0x4) != 0;
    let slots = (flags & 0x8) != 0;
    let desc = Box::new(DataclassDesc {
        name,
        field_names,
        frozen,
        eq,
        repr,
        slots,
        class_bits: 0,
    });
    let desc_ptr = Box::into_raw(desc);

    let total = std::mem::size_of::<MoltHeader>()
        + std::mem::size_of::<*mut Vec<u64>>()
        + std::mem::size_of::<u64>();
    let ptr = alloc_object(total, TYPE_ID_DATACLASS);
    if ptr.is_null() {
        unsafe { drop(Box::from_raw(desc_ptr)) };
        return MoltObject::none().bits();
    }
    unsafe {
        let mut vec = Vec::with_capacity(values.len());
        vec.extend_from_slice(&values);
        for &val in values.iter() {
            inc_ref_bits(val);
        }
        let vec_ptr = Box::into_raw(Box::new(vec));
        *(ptr as *mut *mut DataclassDesc) = desc_ptr;
        *(ptr.add(std::mem::size_of::<*mut DataclassDesc>()) as *mut *mut Vec<u64>) = vec_ptr;
        dataclass_set_dict_bits(ptr, 0);
    }
    MoltObject::from_ptr(ptr).bits()
}

#[no_mangle]
pub extern "C" fn molt_dataclass_get(obj_bits: u64, index_bits: u64) -> u64 {
    let obj = obj_from_bits(obj_bits);
    let idx = match obj_from_bits(index_bits).as_int() {
        Some(val) => val,
        None => return raise_exception::<_>("TypeError", "dataclass field index must be int"),
    };
    if let Some(ptr) = obj.as_ptr() {
        unsafe {
            if object_type_id(ptr) != TYPE_ID_DATACLASS {
                return MoltObject::none().bits();
            }
            let fields = dataclass_fields_ref(ptr);
            if idx < 0 || idx as usize >= fields.len() {
                return raise_exception::<_>("TypeError", "dataclass field index out of range");
            }
            let val = fields[idx as usize];
            inc_ref_bits(val);
            return val;
        }
    }
    MoltObject::none().bits()
}

#[no_mangle]
pub extern "C" fn molt_dataclass_set(obj_bits: u64, index_bits: u64, val_bits: u64) -> u64 {
    let obj = obj_from_bits(obj_bits);
    let idx = match obj_from_bits(index_bits).as_int() {
        Some(val) => val,
        None => return raise_exception::<_>("TypeError", "dataclass field index must be int"),
    };
    if let Some(ptr) = obj.as_ptr() {
        unsafe {
            if object_type_id(ptr) != TYPE_ID_DATACLASS {
                return MoltObject::none().bits();
            }
            let desc_ptr = dataclass_desc_ptr(ptr);
            if !desc_ptr.is_null() && (*desc_ptr).frozen {
                return raise_exception::<_>(
                    "TypeError",
                    "cannot assign to frozen dataclass field",
                );
            }
            let fields = dataclass_fields_mut(ptr);
            if idx < 0 || idx as usize >= fields.len() {
                return raise_exception::<_>("TypeError", "dataclass field index out of range");
            }
            let old_bits = fields[idx as usize];
            if old_bits != val_bits {
                dec_ref_bits(old_bits);
                inc_ref_bits(val_bits);
                fields[idx as usize] = val_bits;
            }
            return obj_bits;
        }
    }
    MoltObject::none().bits()
}

#[no_mangle]
pub extern "C" fn molt_dataclass_set_class(obj_bits: u64, class_bits: u64) -> u64 {
    let obj = obj_from_bits(obj_bits);
    let Some(ptr) = obj.as_ptr() else {
        return raise_exception::<_>("TypeError", "dataclass expects object");
    };
    unsafe {
        if object_type_id(ptr) != TYPE_ID_DATACLASS {
            return raise_exception::<_>("TypeError", "dataclass expects object");
        }
        if class_bits != 0 {
            let class_obj = obj_from_bits(class_bits);
            let Some(class_ptr) = class_obj.as_ptr() else {
                return raise_exception::<_>("TypeError", "class must be a type object");
            };
            if object_type_id(class_ptr) != TYPE_ID_TYPE {
                return raise_exception::<_>("TypeError", "class must be a type object");
            }
        }
        let desc_ptr = dataclass_desc_ptr(ptr);
        if !desc_ptr.is_null() {
            let old_bits = (*desc_ptr).class_bits;
            if old_bits != 0 {
                dec_ref_bits(old_bits);
            }
            (*desc_ptr).class_bits = class_bits;
            if class_bits != 0 {
                inc_ref_bits(class_bits);
            }
        }
    }
    MoltObject::none().bits()
}

#[no_mangle]
pub extern "C" fn molt_func_new(fn_ptr: u64, trampoline_ptr: u64, arity: u64) -> u64 {
    let ptr = alloc_function_obj(fn_ptr, arity);
    if ptr.is_null() {
        MoltObject::none().bits()
    } else {
        unsafe {
            function_set_trampoline_ptr(ptr, trampoline_ptr);
        }
        MoltObject::from_ptr(ptr).bits()
    }
}

#[no_mangle]
pub extern "C" fn molt_func_new_closure(
    fn_ptr: u64,
    trampoline_ptr: u64,
    arity: u64,
    closure_bits: u64,
) -> u64 {
    let ptr = alloc_function_obj(fn_ptr, arity);
    if ptr.is_null() {
        return MoltObject::none().bits();
    }
    unsafe {
        function_set_closure_bits(ptr, closure_bits);
        function_set_trampoline_ptr(ptr, trampoline_ptr);
    }
    MoltObject::from_ptr(ptr).bits()
}

#[no_mangle]
pub extern "C" fn molt_code_new(
    filename_bits: u64,
    name_bits: u64,
    firstlineno_bits: u64,
    linetable_bits: u64,
) -> u64 {
    let filename_obj = obj_from_bits(filename_bits);
    let Some(filename_ptr) = filename_obj.as_ptr() else {
        return raise_exception::<_>("TypeError", "code filename must be str");
    };
    unsafe {
        if object_type_id(filename_ptr) != TYPE_ID_STRING {
            return raise_exception::<_>("TypeError", "code filename must be str");
        }
    }
    let name_obj = obj_from_bits(name_bits);
    let Some(name_ptr) = name_obj.as_ptr() else {
        return raise_exception::<_>("TypeError", "code name must be str");
    };
    unsafe {
        if object_type_id(name_ptr) != TYPE_ID_STRING {
            return raise_exception::<_>("TypeError", "code name must be str");
        }
    }
    if !obj_from_bits(linetable_bits).is_none() {
        let Some(table_ptr) = obj_from_bits(linetable_bits).as_ptr() else {
            return raise_exception::<_>("TypeError", "code linetable must be tuple or None");
        };
        unsafe {
            if object_type_id(table_ptr) != TYPE_ID_TUPLE {
                return raise_exception::<_>("TypeError", "code linetable must be tuple or None");
            }
        }
    }
    let firstlineno = to_i64(obj_from_bits(firstlineno_bits)).unwrap_or(0);
    let ptr = alloc_code_obj(filename_bits, name_bits, firstlineno, linetable_bits);
    if ptr.is_null() {
        MoltObject::none().bits()
    } else {
        MoltObject::from_ptr(ptr).bits()
    }
}

#[no_mangle]
pub extern "C" fn molt_bound_method_new(func_bits: u64, self_bits: u64) -> u64 {
    let func_obj = obj_from_bits(func_bits);
    let Some(func_ptr) = func_obj.as_ptr() else {
        return raise_exception::<_>("TypeError", "bound method expects function object");
    };
    unsafe {
        if object_type_id(func_ptr) != TYPE_ID_FUNCTION {
            return raise_exception::<_>("TypeError", "bound method expects function object");
        }
    }
    let ptr = alloc_bound_method_obj(func_bits, self_bits);
    if ptr.is_null() {
        MoltObject::none().bits()
    } else {
        MoltObject::from_ptr(ptr).bits()
    }
}

#[no_mangle]
pub extern "C" fn molt_module_new(name_bits: u64) -> u64 {
    let name_obj = obj_from_bits(name_bits);
    let Some(name_ptr) = name_obj.as_ptr() else {
        return raise_exception::<_>("TypeError", "module name must be str");
    };
    unsafe {
        if object_type_id(name_ptr) != TYPE_ID_STRING {
            return raise_exception::<_>("TypeError", "module name must be str");
        }
    }
    let ptr = alloc_module_obj(name_bits);
    if ptr.is_null() {
        return MoltObject::none().bits();
    }
    unsafe {
        let dict_bits = module_dict_bits(ptr);
        let dict_obj = obj_from_bits(dict_bits);
        if let Some(dict_ptr) = dict_obj.as_ptr() {
            if object_type_id(dict_ptr) == TYPE_ID_DICT {
                let key_ptr = alloc_string(b"__name__");
                if !key_ptr.is_null() {
                    let key_bits = MoltObject::from_ptr(key_ptr).bits();
                    dict_set_in_place(dict_ptr, key_bits, name_bits);
                    dec_ref_bits(key_bits);
                }
            }
        }
    }
    MoltObject::from_ptr(ptr).bits()
}

#[no_mangle]
pub extern "C" fn molt_class_new(name_bits: u64) -> u64 {
    let name_obj = obj_from_bits(name_bits);
    let Some(name_ptr) = name_obj.as_ptr() else {
        return raise_exception::<_>("TypeError", "class name must be str");
    };
    unsafe {
        if object_type_id(name_ptr) != TYPE_ID_STRING {
            return raise_exception::<_>("TypeError", "class name must be str");
        }
    }
    let ptr = alloc_class_obj(name_bits);
    if ptr.is_null() {
        return MoltObject::none().bits();
    }
    MoltObject::from_ptr(ptr).bits()
}

#[no_mangle]
pub extern "C" fn molt_builtin_type(tag_bits: u64) -> u64 {
    let tag = match to_i64(obj_from_bits(tag_bits)) {
        Some(val) => val,
        None => return raise_exception::<_>("TypeError", "builtin type tag must be int"),
    };
    let Some(bits) = builtin_type_bits(tag) else {
        return raise_exception::<_>("TypeError", "unknown builtin type tag");
    };
    inc_ref_bits(bits);
    bits
}

#[no_mangle]
pub extern "C" fn molt_type_of(val_bits: u64) -> u64 {
    let bits = type_of_bits(val_bits);
    inc_ref_bits(bits);
    bits
}

#[no_mangle]
pub extern "C" fn molt_isinstance(val_bits: u64, class_bits: u64) -> u64 {
    MoltObject::from_bool(isinstance_bits(val_bits, class_bits)).bits()
}

#[no_mangle]
pub extern "C" fn molt_issubclass(sub_bits: u64, class_bits: u64) -> u64 {
    let obj = obj_from_bits(sub_bits);
    let Some(ptr) = obj.as_ptr() else {
        return raise_exception::<_>("TypeError", "issubclass() arg 1 must be a class");
    };
    unsafe {
        if object_type_id(ptr) != TYPE_ID_TYPE {
            return raise_exception::<_>("TypeError", "issubclass() arg 1 must be a class");
        }
    }
    let mut classes = Vec::new();
    collect_classinfo_issubclass(class_bits, &mut classes);
    for class_bits in classes {
        if issubclass_bits(sub_bits, class_bits) {
            return MoltObject::from_bool(true).bits();
        }
    }
    MoltObject::from_bool(false).bits()
}

#[no_mangle]
pub extern "C" fn molt_object_new() -> u64 {
    let obj_bits = molt_alloc(std::mem::size_of::<u64>() as u64);
    let Some(obj_ptr) = resolve_obj_ptr(obj_bits) else {
        return MoltObject::none().bits();
    };
    let class_bits = builtin_classes().object;
    unsafe {
        let _ = molt_object_set_class(obj_ptr, class_bits);
    }
    obj_bits
}

fn c3_merge(mut seqs: Vec<Vec<u64>>) -> Option<Vec<u64>> {
    let mut result = Vec::new();
    loop {
        seqs.retain(|seq| !seq.is_empty());
        if seqs.is_empty() {
            return Some(result);
        }
        let mut candidate = None;
        'outer: for seq in &seqs {
            let head = seq[0];
            let mut in_tail = false;
            for other in &seqs {
                if other.iter().skip(1).any(|val| *val == head) {
                    in_tail = true;
                    break;
                }
            }
            if !in_tail {
                candidate = Some(head);
                break 'outer;
            }
        }
        let cand = candidate?;
        result.push(cand);
        for seq in &mut seqs {
            if !seq.is_empty() && seq[0] == cand {
                seq.remove(0);
            }
        }
    }
}

fn compute_mro(class_bits: u64, bases: &[u64]) -> Option<Vec<u64>> {
    let mut seqs = Vec::with_capacity(bases.len() + 1);
    for base in bases {
        seqs.push(class_mro_vec(*base));
    }
    seqs.push(bases.to_vec());
    let mut out = vec![class_bits];
    let merged = c3_merge(seqs)?;
    out.extend(merged);
    Some(out)
}

#[no_mangle]
pub extern "C" fn molt_class_set_base(class_bits: u64, base_bits: u64) -> u64 {
    let class_obj = obj_from_bits(class_bits);
    let Some(class_ptr) = class_obj.as_ptr() else {
        return raise_exception::<_>("TypeError", "class must be a type object");
    };
    unsafe {
        if object_type_id(class_ptr) != TYPE_ID_TYPE {
            return raise_exception::<_>("TypeError", "class must be a type object");
        }
    }
    let mut bases_vec = Vec::new();
    let mut bases_owned = false;
    let bases_bits = if obj_from_bits(base_bits).is_none() || base_bits == 0 {
        let tuple_ptr = alloc_tuple(&[]);
        if tuple_ptr.is_null() {
            return MoltObject::none().bits();
        }
        bases_owned = true;
        MoltObject::from_ptr(tuple_ptr).bits()
    } else {
        let base_obj = obj_from_bits(base_bits);
        let Some(base_ptr) = base_obj.as_ptr() else {
            return raise_exception::<_>(
                "TypeError",
                "base must be a type object or tuple of types",
            );
        };
        unsafe {
            match object_type_id(base_ptr) {
                TYPE_ID_TYPE => {
                    bases_vec.push(base_bits);
                    let tuple_ptr = alloc_tuple(&[base_bits]);
                    if tuple_ptr.is_null() {
                        return MoltObject::none().bits();
                    }
                    bases_owned = true;
                    MoltObject::from_ptr(tuple_ptr).bits()
                }
                TYPE_ID_TUPLE => {
                    for item in seq_vec_ref(base_ptr).iter() {
                        bases_vec.push(*item);
                    }
                    base_bits
                }
                _ => {
                    return raise_exception::<_>(
                        "TypeError",
                        "base must be a type object or tuple of types",
                    )
                }
            }
        }
    };

    if bases_vec.is_empty() {
        bases_vec = class_bases_vec(bases_bits);
    }
    let mut seen = HashSet::new();
    for base in &bases_vec {
        if !seen.insert(*base) {
            let name = class_name_for_error(*base);
            let msg = format!("duplicate base class {name}");
            return raise_exception::<_>("TypeError", &msg);
        }
    }
    for base in bases_vec.iter() {
        let base_obj = obj_from_bits(*base);
        let Some(base_ptr) = base_obj.as_ptr() else {
            return raise_exception::<_>("TypeError", "base must be a type object");
        };
        unsafe {
            if object_type_id(base_ptr) != TYPE_ID_TYPE {
                return raise_exception::<_>("TypeError", "base must be a type object");
            }
            if base_ptr == class_ptr {
                return raise_exception::<_>("TypeError", "class cannot inherit from itself");
            }
        }
    }

    let mro = match compute_mro(class_bits, &bases_vec) {
        Some(val) => val,
        None => {
            return raise_exception::<_>(
                "TypeError",
                "Cannot create a consistent method resolution order (MRO) for bases",
            );
        }
    };
    let mro_ptr = alloc_tuple(&mro);
    if mro_ptr.is_null() {
        return MoltObject::none().bits();
    }
    let mro_bits = MoltObject::from_ptr(mro_ptr).bits();

    unsafe {
        let old_bases = class_bases_bits(class_ptr);
        let old_mro = class_mro_bits(class_ptr);
        let mut updated = false;
        if old_bases != bases_bits {
            dec_ref_bits(old_bases);
            if !bases_owned {
                inc_ref_bits(bases_bits);
            }
            class_set_bases_bits(class_ptr, bases_bits);
            updated = true;
        }
        if old_mro != mro_bits {
            dec_ref_bits(old_mro);
            class_set_mro_bits(class_ptr, mro_bits);
            updated = true;
        }
        let dict_bits = class_dict_bits(class_ptr);
        if let Some(dict_ptr) = obj_from_bits(dict_bits).as_ptr() {
            if object_type_id(dict_ptr) == TYPE_ID_DICT {
                let bases_name =
                    intern_static_name(&runtime_state().interned.bases_name, b"__bases__");
                let mro_name = intern_static_name(&runtime_state().interned.mro_name, b"__mro__");
                dict_set_in_place(dict_ptr, bases_name, bases_bits);
                dict_set_in_place(dict_ptr, mro_name, mro_bits);
            }
        }
        if updated {
            class_bump_layout_version(class_ptr);
        }
    }
    MoltObject::none().bits()
}

#[no_mangle]
pub extern "C" fn molt_class_apply_set_name(class_bits: u64) -> u64 {
    let class_obj = obj_from_bits(class_bits);
    let Some(class_ptr) = class_obj.as_ptr() else {
        return raise_exception::<_>("TypeError", "class must be a type object");
    };
    unsafe {
        if object_type_id(class_ptr) != TYPE_ID_TYPE {
            return raise_exception::<_>("TypeError", "class must be a type object");
        }
        let dict_bits = class_dict_bits(class_ptr);
        let Some(dict_ptr) = obj_from_bits(dict_bits).as_ptr() else {
            return MoltObject::none().bits();
        };
        if object_type_id(dict_ptr) != TYPE_ID_DICT {
            return MoltObject::none().bits();
        }
        let entries = dict_order(dict_ptr).clone();
        let set_name_bits =
            intern_static_name(&runtime_state().interned.set_name_method, b"__set_name__");
        for pair in entries.chunks(2) {
            if pair.len() != 2 {
                continue;
            }
            let name_bits = pair[0];
            let val_bits = pair[1];
            let Some(val_ptr) = maybe_ptr_from_bits(val_bits) else {
                continue;
            };
            if let Some(set_name) = attr_lookup_ptr_allow_missing(val_ptr, set_name_bits) {
                let _ = call_callable2(set_name, class_bits, name_bits);
                dec_ref_bits(set_name);
            }
        }
    }
    MoltObject::none().bits()
}

#[no_mangle]
pub extern "C" fn molt_class_layout_version(class_bits: u64) -> u64 {
    let class_obj = obj_from_bits(class_bits);
    let Some(class_ptr) = class_obj.as_ptr() else {
        return raise_exception::<_>("TypeError", "class must be a type object");
    };
    unsafe {
        if object_type_id(class_ptr) != TYPE_ID_TYPE {
            return raise_exception::<_>("TypeError", "class must be a type object");
        }
        MoltObject::from_int(class_layout_version_bits(class_ptr) as i64).bits()
    }
}

#[no_mangle]
pub extern "C" fn molt_class_set_layout_version(class_bits: u64, version_bits: u64) -> u64 {
    let class_obj = obj_from_bits(class_bits);
    let Some(class_ptr) = class_obj.as_ptr() else {
        return raise_exception::<_>("TypeError", "class must be a type object");
    };
    unsafe {
        if object_type_id(class_ptr) != TYPE_ID_TYPE {
            return raise_exception::<_>("TypeError", "class must be a type object");
        }
        let version = match to_i64(obj_from_bits(version_bits)) {
            Some(val) if val >= 0 => val as u64,
            _ => return raise_exception::<_>("TypeError", "layout version must be int"),
        };
        class_set_layout_version_bits(class_ptr, version);
    }
    MoltObject::none().bits()
}

#[no_mangle]
pub extern "C" fn molt_super_new(type_bits: u64, obj_bits: u64) -> u64 {
    let type_obj = obj_from_bits(type_bits);
    let Some(type_ptr) = type_obj.as_ptr() else {
        return raise_exception::<_>("TypeError", "super() arg 1 must be a type");
    };
    unsafe {
        if object_type_id(type_ptr) != TYPE_ID_TYPE {
            return raise_exception::<_>("TypeError", "super() arg 1 must be a type");
        }
    }
    let obj = obj_from_bits(obj_bits);
    if obj.is_none() || obj_bits == 0 {
        return raise_exception::<_>(
            "TypeError",
            "super() arg 2 must be an instance or subtype of type",
        );
    }
    let obj_type_bits = if let Some(obj_ptr) = obj.as_ptr() {
        unsafe {
            if object_type_id(obj_ptr) == TYPE_ID_TYPE {
                obj_bits
            } else {
                type_of_bits(obj_bits)
            }
        }
    } else {
        type_of_bits(obj_bits)
    };
    if !issubclass_bits(obj_type_bits, type_bits) {
        return raise_exception::<_>(
            "TypeError",
            "super() arg 2 must be an instance or subtype of type",
        );
    }
    let ptr = alloc_super_obj(type_bits, obj_bits);
    if ptr.is_null() {
        MoltObject::none().bits()
    } else {
        MoltObject::from_ptr(ptr).bits()
    }
}

#[no_mangle]
pub extern "C" fn molt_classmethod_new(func_bits: u64) -> u64 {
    let ptr = alloc_classmethod_obj(func_bits);
    if ptr.is_null() {
        MoltObject::none().bits()
    } else {
        MoltObject::from_ptr(ptr).bits()
    }
}

#[no_mangle]
pub extern "C" fn molt_generic_alias_new(origin_bits: u64, args_bits: u64) -> u64 {
    let args_obj = obj_from_bits(args_bits);
    let args_tuple_bits = if let Some(args_ptr) = args_obj.as_ptr() {
        unsafe {
            if object_type_id(args_ptr) == TYPE_ID_TUPLE {
                args_bits
            } else {
                let tuple_ptr = alloc_tuple(&[args_bits]);
                if tuple_ptr.is_null() {
                    return MoltObject::none().bits();
                }
                MoltObject::from_ptr(tuple_ptr).bits()
            }
        }
    } else {
        let tuple_ptr = alloc_tuple(&[args_bits]);
        if tuple_ptr.is_null() {
            return MoltObject::none().bits();
        }
        MoltObject::from_ptr(tuple_ptr).bits()
    };
    let owned_args = args_tuple_bits != args_bits;
    let ptr = alloc_generic_alias(origin_bits, args_tuple_bits);
    if owned_args {
        dec_ref_bits(args_tuple_bits);
    }
    if ptr.is_null() {
        MoltObject::none().bits()
    } else {
        MoltObject::from_ptr(ptr).bits()
    }
}

#[no_mangle]
pub extern "C" fn molt_staticmethod_new(func_bits: u64) -> u64 {
    let ptr = alloc_staticmethod_obj(func_bits);
    if ptr.is_null() {
        MoltObject::none().bits()
    } else {
        MoltObject::from_ptr(ptr).bits()
    }
}

#[no_mangle]
pub extern "C" fn molt_property_new(get_bits: u64, set_bits: u64, del_bits: u64) -> u64 {
    let ptr = alloc_property_obj(get_bits, set_bits, del_bits);
    if ptr.is_null() {
        MoltObject::none().bits()
    } else {
        MoltObject::from_ptr(ptr).bits()
    }
}

/// # Safety
/// `obj_ptr` must point to a valid Molt object header that can be mutated, and
/// `class_bits` must be either zero or a valid Molt type object.
#[no_mangle]
pub unsafe extern "C" fn molt_object_set_class(obj_ptr: *mut u8, class_bits: u64) -> u64 {
    if obj_ptr.is_null() {
        return raise_exception::<_>("AttributeError", "object has no class");
    }
    let header = header_from_obj_ptr(obj_ptr);
    if (*header).poll_fn != 0 {
        return raise_exception::<_>("TypeError", "cannot set class on async object");
    }
    if class_bits != 0 {
        let class_obj = obj_from_bits(class_bits);
        let Some(class_ptr) = class_obj.as_ptr() else {
            return raise_exception::<_>("TypeError", "class must be a type object");
        };
        if object_type_id(class_ptr) != TYPE_ID_TYPE {
            return raise_exception::<_>("TypeError", "class must be a type object");
        }
    }
    let skip_class_ref = ((*header).flags & HEADER_FLAG_SKIP_CLASS_DECREF) != 0;
    let old_bits = object_class_bits(obj_ptr);
    if old_bits != 0 && !skip_class_ref {
        dec_ref_bits(old_bits);
    }
    object_set_class_bits(obj_ptr, class_bits);
    if class_bits != 0 && !skip_class_ref {
        inc_ref_bits(class_bits);
    }
    MoltObject::none().bits()
}

fn resolve_obj_ptr(bits: u64) -> Option<*mut u8> {
    if let Some(ptr) = obj_from_bits(bits).as_ptr() {
        return Some(ptr);
    }
    None
}

fn resolve_task_ptr(bits: u64) -> Option<*mut u8> {
    let obj = obj_from_bits(bits);
    if let Some(ptr) = obj.as_ptr() {
        return Some(ptr);
    }
    if !obj.is_float() {
        return None;
    }
    let high = bits >> 48;
    if high == 0 || high == 0xffff {
        let addr = bits as usize;
        if addr < 4096 || (addr & 0x7) != 0 {
            return None;
        }
        return resolve_ptr(bits);
    }
    None
}

/// # Safety
/// `obj_ptr` must point to a valid object with enough payload for `offset`.
unsafe fn object_field_get_ptr_raw(obj_ptr: *mut u8, offset: usize) -> u64 {
    if obj_ptr.is_null() {
        return raise_exception::<_>("TypeError", "object field access on non-object");
    }
    let slot = obj_ptr.add(offset) as *const u64;
    let bits = *slot;
    inc_ref_bits(bits);
    bits
}

/// # Safety
/// `obj_ptr` must point to a valid object with enough payload for `offset`.
unsafe fn object_field_set_ptr_raw(obj_ptr: *mut u8, offset: usize, val_bits: u64) -> u64 {
    if obj_ptr.is_null() {
        return raise_exception::<_>("TypeError", "object field access on non-object");
    }
    profile_hit(&STRUCT_FIELD_STORE_COUNT);
    let slot = obj_ptr.add(offset) as *mut u64;
    let old_bits = *slot;
    let old_is_ptr = obj_from_bits(old_bits).as_ptr().is_some();
    let new_is_ptr = obj_from_bits(val_bits).as_ptr().is_some();
    if new_is_ptr {
        object_mark_has_ptrs(obj_ptr);
    }
    if !old_is_ptr && !new_is_ptr {
        *slot = val_bits;
        return MoltObject::none().bits();
    }
    if old_bits != val_bits {
        dec_ref_bits(old_bits);
        inc_ref_bits(val_bits);
        *slot = val_bits;
    }
    MoltObject::none().bits()
}

/// # Safety
/// `obj_ptr` must point to a valid object with enough payload for `offset`.
/// Intended for initializing freshly allocated objects with immediate values.
unsafe fn object_field_init_ptr_raw(obj_ptr: *mut u8, offset: usize, val_bits: u64) -> u64 {
    if obj_ptr.is_null() {
        return raise_exception::<_>("TypeError", "object field access on non-object");
    }
    let slot = obj_ptr.add(offset) as *mut u64;
    let old_bits = *slot;
    debug_assert!(
        old_bits == 0 || obj_from_bits(old_bits).as_ptr().is_none(),
        "object_field_init used on slot with pointer contents"
    );
    if obj_from_bits(val_bits).as_ptr().is_some() {
        object_mark_has_ptrs(obj_ptr);
    }
    *slot = val_bits;
    MoltObject::none().bits()
}

/// # Safety
/// `obj_ptr` must point to a valid object with enough payload for `offset_bits`.
#[no_mangle]
pub unsafe extern "C" fn molt_object_field_get_ptr(obj_ptr: *mut u8, offset_bits: u64) -> u64 {
    let offset = usize_from_bits(offset_bits);
    object_field_get_ptr_raw(obj_ptr, offset)
}

/// # Safety
/// `obj_ptr` must point to a valid object with enough payload for `offset_bits`.
#[no_mangle]
pub unsafe extern "C" fn molt_object_field_set_ptr(
    obj_ptr: *mut u8,
    offset_bits: u64,
    val_bits: u64,
) -> u64 {
    let offset = usize_from_bits(offset_bits);
    object_field_set_ptr_raw(obj_ptr, offset, val_bits)
}

/// # Safety
/// `obj_ptr` must point to a valid object with enough payload for `offset_bits`.
/// Intended for initializing freshly allocated objects with immediate values.
#[no_mangle]
pub unsafe extern "C" fn molt_object_field_init_ptr(
    obj_ptr: *mut u8,
    offset_bits: u64,
    val_bits: u64,
) -> u64 {
    let offset = usize_from_bits(offset_bits);
    object_field_init_ptr_raw(obj_ptr, offset, val_bits)
}

unsafe fn guard_layout_match(obj_ptr: *mut u8, class_bits: u64, expected_version: u64) -> bool {
    profile_hit(&LAYOUT_GUARD_COUNT);
    if obj_ptr.is_null() {
        profile_hit(&LAYOUT_GUARD_FAIL);
        return false;
    }
    let header = header_from_obj_ptr(obj_ptr);
    if (*header).type_id != TYPE_ID_OBJECT {
        profile_hit(&LAYOUT_GUARD_FAIL);
        return false;
    }
    let obj_class_bits = object_class_bits(obj_ptr);
    if obj_class_bits == 0 || obj_class_bits != class_bits {
        profile_hit(&LAYOUT_GUARD_FAIL);
        return false;
    }
    let class_obj = obj_from_bits(class_bits);
    let Some(class_ptr) = class_obj.as_ptr() else {
        profile_hit(&LAYOUT_GUARD_FAIL);
        return false;
    };
    if object_type_id(class_ptr) != TYPE_ID_TYPE {
        profile_hit(&LAYOUT_GUARD_FAIL);
        return false;
    }
    let version = class_layout_version_bits(class_ptr);
    let expected = match to_i64(obj_from_bits(expected_version)) {
        Some(val) if val >= 0 => val as u64,
        _ => {
            profile_hit(&LAYOUT_GUARD_FAIL);
            return false;
        }
    };
    if version != expected {
        profile_hit(&LAYOUT_GUARD_FAIL);
        return false;
    }
    true
}

/// # Safety
/// `obj_ptr` must point to a valid object with a class.
#[no_mangle]
pub unsafe extern "C" fn molt_guard_layout_ptr(
    obj_ptr: *mut u8,
    class_bits: u64,
    expected_version: u64,
) -> u64 {
    MoltObject::from_bool(guard_layout_match(obj_ptr, class_bits, expected_version)).bits()
}

/// # Safety
/// `obj_ptr` must point to a valid object with enough payload for `offset_bits`.
#[no_mangle]
pub unsafe extern "C" fn molt_guarded_field_get_ptr(
    obj_ptr: *mut u8,
    class_bits: u64,
    expected_version: u64,
    offset_bits: u64,
    attr_name_ptr: *const u8,
    attr_name_len_bits: u64,
) -> u64 {
    let offset = usize_from_bits(offset_bits);
    if guard_layout_match(obj_ptr, class_bits, expected_version) {
        return object_field_get_ptr_raw(obj_ptr, offset);
    }
    molt_get_attr_ptr(obj_ptr, attr_name_ptr, attr_name_len_bits) as u64
}

/// # Safety
/// `obj_ptr` must point to a valid object with enough payload for `offset_bits`.
#[no_mangle]
pub unsafe extern "C" fn molt_guarded_field_set_ptr(
    obj_ptr: *mut u8,
    class_bits: u64,
    expected_version: u64,
    offset_bits: u64,
    val_bits: u64,
    attr_name_ptr: *const u8,
    attr_name_len_bits: u64,
) -> u64 {
    let offset = usize_from_bits(offset_bits);
    if guard_layout_match(obj_ptr, class_bits, expected_version) {
        return object_field_set_ptr_raw(obj_ptr, offset, val_bits);
    }
    molt_set_attr_ptr(obj_ptr, attr_name_ptr, attr_name_len_bits, val_bits) as u64
}

/// # Safety
/// `obj_ptr` must point to a valid object with enough payload for `offset_bits`.
#[no_mangle]
pub unsafe extern "C" fn molt_guarded_field_init_ptr(
    obj_ptr: *mut u8,
    class_bits: u64,
    expected_version: u64,
    offset_bits: u64,
    val_bits: u64,
    attr_name_ptr: *const u8,
    attr_name_len_bits: u64,
) -> u64 {
    let offset = usize_from_bits(offset_bits);
    if guard_layout_match(obj_ptr, class_bits, expected_version) {
        return object_field_init_ptr_raw(obj_ptr, offset, val_bits);
    }
    molt_set_attr_ptr(obj_ptr, attr_name_ptr, attr_name_len_bits, val_bits) as u64
}

/// # Safety
/// `obj_bits` must reference a valid object with enough payload for `offset_bits`.
#[no_mangle]
pub unsafe extern "C" fn molt_object_field_get(obj_bits: u64, offset_bits: u64) -> u64 {
    let Some(obj_ptr) = resolve_obj_ptr(obj_bits) else {
        return raise_exception::<_>("TypeError", "object field access on non-object");
    };
    let offset = usize_from_bits(offset_bits);
    let slot = obj_ptr.add(offset) as *const u64;
    let bits = *slot;
    inc_ref_bits(bits);
    bits
}

/// # Safety
/// `obj_bits` must reference a valid object with enough payload for `offset_bits`.
#[no_mangle]
pub unsafe extern "C" fn molt_object_field_set(
    obj_bits: u64,
    offset_bits: u64,
    val_bits: u64,
) -> u64 {
    let Some(obj_ptr) = resolve_obj_ptr(obj_bits) else {
        return raise_exception::<_>("TypeError", "object field access on non-object");
    };
    let offset = usize_from_bits(offset_bits);
    profile_hit(&STRUCT_FIELD_STORE_COUNT);
    let slot = obj_ptr.add(offset) as *mut u64;
    let old_bits = *slot;
    let old_is_ptr = obj_from_bits(old_bits).as_ptr().is_some();
    let new_is_ptr = obj_from_bits(val_bits).as_ptr().is_some();
    if new_is_ptr {
        object_mark_has_ptrs(obj_ptr);
    }
    if !old_is_ptr && !new_is_ptr {
        *slot = val_bits;
        return MoltObject::none().bits();
    }
    if old_bits != val_bits {
        dec_ref_bits(old_bits);
        inc_ref_bits(val_bits);
        *slot = val_bits;
    }
    MoltObject::none().bits()
}

/// # Safety
/// `obj_bits` must reference a valid object with enough payload for `offset_bits`.
/// Intended for initializing freshly allocated objects with immediate values.
#[no_mangle]
pub unsafe extern "C" fn molt_object_field_init(
    obj_bits: u64,
    offset_bits: u64,
    val_bits: u64,
) -> u64 {
    let Some(obj_ptr) = resolve_obj_ptr(obj_bits) else {
        return raise_exception::<_>("TypeError", "object field access on non-object");
    };
    let offset = usize_from_bits(offset_bits);
    let slot = obj_ptr.add(offset) as *mut u64;
    let old_bits = *slot;
    debug_assert!(
        old_bits == 0 || obj_from_bits(old_bits).as_ptr().is_none(),
        "object_field_init used on slot with pointer contents"
    );
    if obj_from_bits(val_bits).as_ptr().is_some() {
        object_mark_has_ptrs(obj_ptr);
    }
    *slot = val_bits;
    MoltObject::none().bits()
}

#[no_mangle]
pub extern "C" fn molt_module_cache_get(name_bits: u64) -> u64 {
    let name = match string_obj_to_owned(obj_from_bits(name_bits)) {
        Some(val) => val,
        None => return raise_exception::<_>("TypeError", "module name must be str"),
    };
    let cache = crate::builtins::exceptions::internals::module_cache();
    let guard = cache.lock().unwrap();
    if let Some(bits) = guard.get(&name) {
        inc_ref_bits(*bits);
        return *bits;
    }
    MoltObject::none().bits()
}

fn sys_modules_dict_ptr(sys_bits: u64) -> Option<*mut u8> {
    let sys_obj = obj_from_bits(sys_bits);
    let sys_ptr = sys_obj.as_ptr()?;
    unsafe {
        if object_type_id(sys_ptr) != TYPE_ID_MODULE {
            return None;
        }
        let dict_bits = module_dict_bits(sys_ptr);
        let dict_ptr = match obj_from_bits(dict_bits).as_ptr() {
            Some(ptr) if object_type_id(ptr) == TYPE_ID_DICT => ptr,
            _ => return None,
        };
        let modules_name_bits =
            intern_static_name(&runtime_state().interned.modules_name, b"modules");
        if obj_from_bits(modules_name_bits).is_none() {
            return None;
        }
        let mut modules_bits = dict_get_in_place(dict_ptr, modules_name_bits);
        if modules_bits.is_none() {
            let new_ptr = alloc_dict_with_pairs(&[]);
            if new_ptr.is_null() {
                return None;
            }
            let new_bits = MoltObject::from_ptr(new_ptr).bits();
            dict_set_in_place(dict_ptr, modules_name_bits, new_bits);
            modules_bits = Some(new_bits);
            dec_ref_bits(new_bits);
        }
        let modules_ptr = match obj_from_bits(modules_bits?).as_ptr() {
            Some(ptr) if object_type_id(ptr) == TYPE_ID_DICT => ptr,
            _ => return raise_exception::<_>("TypeError", "sys.modules must be dict"),
        };
        Some(modules_ptr)
    }
}

#[no_mangle]
pub extern "C" fn molt_module_cache_set(name_bits: u64, module_bits: u64) -> u64 {
    let name = match string_obj_to_owned(obj_from_bits(name_bits)) {
        Some(val) => val,
        None => return raise_exception::<_>("TypeError", "module name must be str"),
    };
    let is_sys = name == "sys";
    let (sys_bits, cached_modules) = {
        let cache = crate::builtins::exceptions::internals::module_cache();
        let mut guard = cache.lock().unwrap();
        if let Some(old) = guard.insert(name, module_bits) {
            dec_ref_bits(old);
        }
        inc_ref_bits(module_bits);
        if is_sys {
            let entries = guard
                .iter()
                .map(|(key, &bits)| (key.clone(), bits))
                .collect::<Vec<_>>();
            (Some(module_bits), Some(entries))
        } else {
            (guard.get("sys").copied(), None)
        }
    };
    if let Some(sys_bits) = sys_bits {
        if let Some(modules_ptr) = sys_modules_dict_ptr(sys_bits) {
            if let Some(entries) = cached_modules {
                for (key, bits) in entries {
                    let key_ptr = alloc_string(key.as_bytes());
                    if key_ptr.is_null() {
                        return raise_exception::<_>("MemoryError", "out of memory");
                    }
                    let key_bits = MoltObject::from_ptr(key_ptr).bits();
                    unsafe {
                        dict_set_in_place(modules_ptr, key_bits, bits);
                    }
                    dec_ref_bits(key_bits);
                }
            } else {
                unsafe {
                    dict_set_in_place(modules_ptr, name_bits, module_bits);
                }
            }
        }
    }
    MoltObject::none().bits()
}

#[no_mangle]
pub extern "C" fn molt_module_get_attr(module_bits: u64, attr_bits: u64) -> u64 {
    let module_obj = obj_from_bits(module_bits);
    let Some(module_ptr) = module_obj.as_ptr() else {
        return raise_exception::<_>("TypeError", "module attribute access expects module");
    };
    unsafe {
        if object_type_id(module_ptr) != TYPE_ID_MODULE {
            return raise_exception::<_>("TypeError", "module attribute access expects module");
        }
        let dict_bits = module_dict_bits(module_ptr);
        let dict_obj = obj_from_bits(dict_bits);
        let _dict_ptr = match dict_obj.as_ptr() {
            Some(ptr) if object_type_id(ptr) == TYPE_ID_DICT => ptr,
            _ => return raise_exception::<_>("TypeError", "module dict missing"),
        };
        if let Some(val) = module_attr_lookup(module_ptr, attr_bits) {
            return val;
        }
        let module_name =
            string_obj_to_owned(obj_from_bits(module_name_bits(module_ptr))).unwrap_or_default();
        let attr_name =
            string_obj_to_owned(obj_from_bits(attr_bits)).unwrap_or_else(|| "<attr>".to_string());
        let msg = format!("module '{module_name}' has no attribute '{attr_name}'");
        return raise_exception::<_>("AttributeError", &msg);
    }
}

#[no_mangle]
pub extern "C" fn molt_module_get_global(module_bits: u64, name_bits: u64) -> u64 {
    let module_obj = obj_from_bits(module_bits);
    let Some(module_ptr) = module_obj.as_ptr() else {
        return raise_exception::<_>("TypeError", "module attribute access expects module");
    };
    unsafe {
        if object_type_id(module_ptr) != TYPE_ID_MODULE {
            return raise_exception::<_>("TypeError", "module attribute access expects module");
        }
        let dict_bits = module_dict_bits(module_ptr);
        let dict_obj = obj_from_bits(dict_bits);
        let dict_ptr = match dict_obj.as_ptr() {
            Some(ptr) if object_type_id(ptr) == TYPE_ID_DICT => ptr,
            _ => return raise_exception::<_>("TypeError", "module dict missing"),
        };
        if let Some(val) = dict_get_in_place(dict_ptr, name_bits) {
            inc_ref_bits(val);
            return val;
        }
        let name =
            string_obj_to_owned(obj_from_bits(name_bits)).unwrap_or_else(|| "<name>".to_string());
        let msg = format!("name '{name}' is not defined");
        return raise_exception::<_>("NameError", &msg);
    }
}

#[no_mangle]
pub extern "C" fn molt_module_del_global(module_bits: u64, name_bits: u64) -> u64 {
    let module_obj = obj_from_bits(module_bits);
    let Some(module_ptr) = module_obj.as_ptr() else {
        return raise_exception::<_>("TypeError", "module attribute access expects module");
    };
    unsafe {
        if object_type_id(module_ptr) != TYPE_ID_MODULE {
            return raise_exception::<_>("TypeError", "module attribute access expects module");
        }
        let dict_bits = module_dict_bits(module_ptr);
        let dict_obj = obj_from_bits(dict_bits);
        let dict_ptr = match dict_obj.as_ptr() {
            Some(ptr) if object_type_id(ptr) == TYPE_ID_DICT => ptr,
            _ => return raise_exception::<_>("TypeError", "module dict missing"),
        };
        if dict_del_in_place(dict_ptr, name_bits) {
            return MoltObject::none().bits();
        }
        let name =
            string_obj_to_owned(obj_from_bits(name_bits)).unwrap_or_else(|| "<name>".to_string());
        let msg = format!("name '{name}' is not defined");
        return raise_exception::<_>("NameError", &msg);
    }
}

#[no_mangle]
pub extern "C" fn molt_module_get_name(module_bits: u64, attr_bits: u64) -> u64 {
    // Keep wasm import parity; module __name__ is stored in the module dict.
    molt_module_get_attr(module_bits, attr_bits)
}

#[no_mangle]
pub extern "C" fn molt_module_set_attr(module_bits: u64, attr_bits: u64, val_bits: u64) -> u64 {
    let module_obj = obj_from_bits(module_bits);
    let Some(module_ptr) = module_obj.as_ptr() else {
        return raise_exception::<_>("TypeError", "module attribute set expects module");
    };
    unsafe {
        if object_type_id(module_ptr) != TYPE_ID_MODULE {
            return raise_exception::<_>("TypeError", "module attribute set expects module");
        }
        let dict_bits = module_dict_bits(module_ptr);
        let dict_obj = obj_from_bits(dict_bits);
        let dict_ptr = match dict_obj.as_ptr() {
            Some(ptr) if object_type_id(ptr) == TYPE_ID_DICT => ptr,
            _ => return raise_exception::<_>("TypeError", "module dict missing"),
        };
        let annotations_bits = intern_static_name(
            &runtime_state().interned.annotations_name,
            b"__annotations__",
        );
        if obj_eq(obj_from_bits(attr_bits), obj_from_bits(annotations_bits)) {
            dict_set_in_place(dict_ptr, attr_bits, val_bits);
            let annotate_bits =
                intern_static_name(&runtime_state().interned.annotate_name, b"__annotate__");
            let none_bits = MoltObject::none().bits();
            dict_set_in_place(dict_ptr, annotate_bits, none_bits);
            return MoltObject::none().bits();
        }
        let annotate_bits =
            intern_static_name(&runtime_state().interned.annotate_name, b"__annotate__");
        if obj_eq(obj_from_bits(attr_bits), obj_from_bits(annotate_bits)) {
            let val_obj = obj_from_bits(val_bits);
            if !val_obj.is_none() {
                let callable_ok = is_truthy(obj_from_bits(molt_is_callable(val_bits)));
                if !callable_ok {
                    return raise_exception::<_>(
                        "TypeError",
                        "__annotate__ must be callable or None",
                    );
                }
            }
            dict_set_in_place(dict_ptr, attr_bits, val_bits);
            if !val_obj.is_none() {
                dict_del_in_place(dict_ptr, annotations_bits);
            }
            return MoltObject::none().bits();
        }
        dict_set_in_place(dict_ptr, attr_bits, val_bits);
    }
    MoltObject::none().bits()
}

#[no_mangle]
pub extern "C" fn molt_module_import_star(src_bits: u64, dst_bits: u64) -> u64 {
    let src_obj = obj_from_bits(src_bits);
    let Some(src_ptr) = src_obj.as_ptr() else {
        return raise_exception::<_>("TypeError", "module import expects module");
    };
    let dst_obj = obj_from_bits(dst_bits);
    let Some(dst_ptr) = dst_obj.as_ptr() else {
        return raise_exception::<_>("TypeError", "module import expects module");
    };
    unsafe {
        if object_type_id(src_ptr) != TYPE_ID_MODULE || object_type_id(dst_ptr) != TYPE_ID_MODULE {
            return raise_exception::<_>("TypeError", "module import expects module");
        }
        let src_dict_bits = module_dict_bits(src_ptr);
        let dst_dict_bits = module_dict_bits(dst_ptr);
        let src_dict_obj = obj_from_bits(src_dict_bits);
        let dst_dict_obj = obj_from_bits(dst_dict_bits);
        let src_dict_ptr = match src_dict_obj.as_ptr() {
            Some(ptr) if object_type_id(ptr) == TYPE_ID_DICT => ptr,
            _ => return raise_exception::<_>("TypeError", "module dict missing"),
        };
        let dst_dict_ptr = match dst_dict_obj.as_ptr() {
            Some(ptr) if object_type_id(ptr) == TYPE_ID_DICT => ptr,
            _ => return raise_exception::<_>("TypeError", "module dict missing"),
        };
        let module_name =
            string_obj_to_owned(obj_from_bits(module_name_bits(src_ptr))).unwrap_or_default();
        let all_name_bits = intern_static_name(&runtime_state().interned.all_name, b"__all__");
        if let Some(all_bits) = dict_get_in_place(src_dict_ptr, all_name_bits) {
            let iter_bits = molt_iter(all_bits);
            if exception_pending() {
                return MoltObject::none().bits();
            }
            loop {
                let pair_bits = molt_iter_next(iter_bits);
                let pair_obj = obj_from_bits(pair_bits);
                let Some(pair_ptr) = pair_obj.as_ptr() else {
                    return MoltObject::none().bits();
                };
                if object_type_id(pair_ptr) != TYPE_ID_TUPLE {
                    return MoltObject::none().bits();
                }
                let elems = seq_vec_ref(pair_ptr);
                if elems.len() < 2 {
                    return MoltObject::none().bits();
                }
                let done_bits = elems[1];
                if is_truthy(obj_from_bits(done_bits)) {
                    break;
                }
                let name_bits = elems[0];
                let name_obj = obj_from_bits(name_bits);
                if let Some(name_ptr) = name_obj.as_ptr() {
                    if object_type_id(name_ptr) != TYPE_ID_STRING {
                        let type_name = class_name_for_error(type_of_bits(name_bits));
                        let msg =
                            format!("Item in {module_name}.__all__ must be str, not {type_name}");
                        return raise_exception::<_>("TypeError", &msg);
                    }
                } else {
                    let type_name = class_name_for_error(type_of_bits(name_bits));
                    let msg = format!("Item in {module_name}.__all__ must be str, not {type_name}");
                    return raise_exception::<_>("TypeError", &msg);
                }
                let Some(val_bits) = dict_get_in_place(src_dict_ptr, name_bits) else {
                    let name = string_obj_to_owned(obj_from_bits(name_bits)).unwrap_or_default();
                    let msg = format!("module '{module_name}' has no attribute '{name}'");
                    return raise_exception::<_>("AttributeError", &msg);
                };
                dict_set_in_place(dst_dict_ptr, name_bits, val_bits);
            }
            return MoltObject::none().bits();
        }

        let order = dict_order(src_dict_ptr);
        for idx in (0..order.len()).step_by(2) {
            let name_bits = order[idx];
            let name_obj = obj_from_bits(name_bits);
            let Some(name_ptr) = name_obj.as_ptr() else {
                continue;
            };
            if object_type_id(name_ptr) != TYPE_ID_STRING {
                continue;
            }
            let name_len = string_len(name_ptr);
            if name_len > 0 {
                let name_bytes = std::slice::from_raw_parts(string_bytes(name_ptr), name_len);
                if name_bytes[0] == b'_' {
                    continue;
                }
            }
            let val_bits = order[idx + 1];
            dict_set_in_place(dst_dict_ptr, name_bits, val_bits);
        }
    }
    MoltObject::none().bits()
}

unsafe fn generator_slot_ptr(ptr: *mut u8, offset: usize) -> *mut u64 {
    ptr.add(offset) as *mut u64
}

unsafe fn generator_set_slot(ptr: *mut u8, offset: usize, bits: u64) {
    let slot = generator_slot_ptr(ptr, offset);
    let old_bits = *slot;
    dec_ref_bits(old_bits);
    inc_ref_bits(bits);
    *slot = bits;
}

/// # Safety
/// `self_ptr` must point to a valid closure storage region and `offset` must be
/// within the allocated payload.
#[no_mangle]
pub unsafe extern "C" fn molt_closure_load(self_ptr: *mut u8, offset: u64) -> u64 {
    if self_ptr.is_null() {
        return MoltObject::none().bits();
    }
    let slot = self_ptr.add(offset as usize) as *mut u64;
    let bits = *slot;
    inc_ref_bits(bits);
    bits
}

/// # Safety
/// `self_ptr` must point to a valid closure storage region and `offset` must be
/// within the allocated payload.
#[no_mangle]
pub unsafe extern "C" fn molt_closure_store(self_ptr: *mut u8, offset: u64, bits: u64) -> u64 {
    if self_ptr.is_null() {
        return MoltObject::none().bits();
    }
    let slot = self_ptr.add(offset as usize) as *mut u64;
    let old_bits = *slot;
    dec_ref_bits(old_bits);
    inc_ref_bits(bits);
    *slot = bits;
    MoltObject::none().bits()
}

unsafe fn generator_closed(ptr: *mut u8) -> bool {
    let bits = *generator_slot_ptr(ptr, GEN_CLOSED_OFFSET);
    obj_from_bits(bits).as_bool().unwrap_or(false)
}

unsafe fn generator_set_closed(ptr: *mut u8, closed: bool) {
    let bits = MoltObject::from_bool(closed).bits();
    generator_set_slot(ptr, GEN_CLOSED_OFFSET, bits);
}

unsafe fn generator_running(ptr: *mut u8) -> bool {
    let header = header_from_obj_ptr(ptr);
    ((*header).flags & HEADER_FLAG_GEN_RUNNING) != 0
}

unsafe fn generator_set_running(ptr: *mut u8, running: bool) {
    let header = header_from_obj_ptr(ptr);
    if running {
        (*header).flags |= HEADER_FLAG_GEN_RUNNING;
    } else {
        (*header).flags &= !HEADER_FLAG_GEN_RUNNING;
    }
}

unsafe fn generator_started(ptr: *mut u8) -> bool {
    let header = header_from_obj_ptr(ptr);
    ((*header).flags & HEADER_FLAG_GEN_STARTED) != 0
}

unsafe fn generator_set_started(ptr: *mut u8) {
    let header = header_from_obj_ptr(ptr);
    (*header).flags |= HEADER_FLAG_GEN_STARTED;
}

unsafe fn generator_pending_throw(ptr: *mut u8) -> bool {
    let bits = *generator_slot_ptr(ptr, GEN_THROW_OFFSET);
    !obj_from_bits(bits).is_none()
}

fn generator_done_tuple(value_bits: u64) -> u64 {
    let done_bits = MoltObject::from_bool(true).bits();
    let tuple_ptr = alloc_tuple(&[value_bits, done_bits]);
    if tuple_ptr.is_null() {
        MoltObject::none().bits()
    } else {
        MoltObject::from_ptr(tuple_ptr).bits()
    }
}

fn generator_unpack_pair(bits: u64) -> Option<(u64, bool)> {
    let obj = obj_from_bits(bits);
    let ptr = obj.as_ptr()?;
    unsafe {
        if object_type_id(ptr) != TYPE_ID_TUPLE {
            return None;
        }
        let elems = seq_vec_ref(ptr);
        if elems.len() < 2 {
            return None;
        }
        let done = obj_from_bits(elems[1]).as_bool().unwrap_or(false);
        Some((elems[0], done))
    }
}

#[no_mangle]
pub extern "C" fn molt_task_new(poll_fn_addr: u64, closure_size: u64, kind_bits: u64) -> u64 {
    let type_id = match kind_bits {
        TASK_KIND_FUTURE => TYPE_ID_OBJECT,
        TASK_KIND_GENERATOR => TYPE_ID_GENERATOR,
        _ => {
            return raise_exception::<_>("TypeError", "unknown task kind");
        }
    };
    if type_id == TYPE_ID_GENERATOR && (closure_size as usize) < GEN_CONTROL_SIZE {
        return raise_exception::<_>("TypeError", "generator task closure too small");
    }
    let total_size = std::mem::size_of::<MoltHeader>() + closure_size as usize;
    let ptr = alloc_object(total_size, type_id);
    if ptr.is_null() {
        return MoltObject::none().bits();
    }
    unsafe {
        let slots = closure_size as usize / std::mem::size_of::<u64>();
        if slots > 0 {
            let payload_ptr = ptr as *mut u64;
            for idx in 0..slots {
                *payload_ptr.add(idx) = MoltObject::none().bits();
            }
        }
        let header = header_from_obj_ptr(ptr);
        (*header).poll_fn = poll_fn_addr;
        (*header).state = 0;
        if type_id == TYPE_ID_GENERATOR && closure_size as usize >= GEN_CONTROL_SIZE {
            *generator_slot_ptr(ptr, GEN_SEND_OFFSET) = MoltObject::none().bits();
            *generator_slot_ptr(ptr, GEN_THROW_OFFSET) = MoltObject::none().bits();
            *generator_slot_ptr(ptr, GEN_CLOSED_OFFSET) = MoltObject::from_bool(false).bits();
            *generator_slot_ptr(ptr, GEN_EXC_DEPTH_OFFSET) = MoltObject::from_int(1).bits();
        }
    }
    MoltObject::from_ptr(ptr).bits()
}

/// # Safety
/// - `task_bits` must be a valid pointer to a Molt task with a valid header.
/// - `token_bits` must be an integer cancel token id.
#[no_mangle]
pub unsafe extern "C" fn molt_task_register_token_owned(task_bits: u64, token_bits: u64) -> u64 {
    let Some(task_ptr) = resolve_task_ptr(task_bits) else {
        return raise_exception::<_>("TypeError", "object is not awaitable");
    };
    let id = match token_id_from_bits(token_bits) {
        Some(id) => id,
        None => return raise_exception::<_>("TypeError", "cancel token id must be int"),
    };
    register_task_token(task_ptr, id);
    MoltObject::none().bits()
}

#[no_mangle]
pub extern "C" fn molt_generator_new(poll_fn_addr: u64, closure_size: u64) -> u64 {
    molt_task_new(poll_fn_addr, closure_size, TASK_KIND_GENERATOR)
}

#[no_mangle]
pub extern "C" fn molt_is_generator(obj_bits: u64) -> u64 {
    let is_gen = maybe_ptr_from_bits(obj_bits)
        .is_some_and(|ptr| unsafe { object_type_id(ptr) == TYPE_ID_GENERATOR });
    MoltObject::from_bool(is_gen).bits()
}

#[no_mangle]
pub extern "C" fn molt_is_bound_method(obj_bits: u64) -> u64 {
    let is_bound = maybe_ptr_from_bits(obj_bits)
        .is_some_and(|ptr| unsafe { object_type_id(ptr) == TYPE_ID_BOUND_METHOD });
    MoltObject::from_bool(is_bound).bits()
}

#[no_mangle]
pub extern "C" fn molt_is_function_obj(obj_bits: u64) -> u64 {
    let is_func = maybe_ptr_from_bits(obj_bits)
        .is_some_and(|ptr| unsafe { object_type_id(ptr) == TYPE_ID_FUNCTION });
    MoltObject::from_bool(is_func).bits()
}

#[no_mangle]
pub extern "C" fn molt_function_is_generator(func_bits: u64) -> u64 {
    let obj = obj_from_bits(func_bits);
    let Some(ptr) = obj.as_ptr() else {
        return MoltObject::from_bool(false).bits();
    };
    unsafe {
        if object_type_id(ptr) != TYPE_ID_FUNCTION {
            return MoltObject::from_bool(false).bits();
        }
        let name_bits = intern_static_name(
            &runtime_state().interned.molt_is_generator,
            b"__molt_is_generator__",
        );
        let Some(bits) = function_attr_bits(ptr, name_bits) else {
            return MoltObject::from_bool(false).bits();
        };
        MoltObject::from_bool(is_truthy(obj_from_bits(bits))).bits()
    }
}

#[no_mangle]
pub extern "C" fn molt_function_is_coroutine(func_bits: u64) -> u64 {
    let obj = obj_from_bits(func_bits);
    let Some(ptr) = obj.as_ptr() else {
        return MoltObject::from_bool(false).bits();
    };
    unsafe {
        if object_type_id(ptr) != TYPE_ID_FUNCTION {
            return MoltObject::from_bool(false).bits();
        }
        let name_bits = intern_static_name(
            &runtime_state().interned.molt_is_coroutine,
            b"__molt_is_coroutine__",
        );
        let Some(bits) = function_attr_bits(ptr, name_bits) else {
            return MoltObject::from_bool(false).bits();
        };
        MoltObject::from_bool(is_truthy(obj_from_bits(bits))).bits()
    }
}

#[no_mangle]
pub extern "C" fn molt_is_callable(obj_bits: u64) -> u64 {
    let is_callable = maybe_ptr_from_bits(obj_bits).is_some_and(|ptr| unsafe {
        match object_type_id(ptr) {
            TYPE_ID_FUNCTION | TYPE_ID_BOUND_METHOD | TYPE_ID_TYPE => true,
            TYPE_ID_OBJECT => {
                let call_bits =
                    intern_static_name(&runtime_state().interned.call_name, b"__call__");
                let dict_bits = instance_dict_bits(ptr);
                if dict_bits != 0 {
                    if let Some(dict_ptr) = obj_from_bits(dict_bits).as_ptr() {
                        if object_type_id(dict_ptr) == TYPE_ID_DICT
                            && dict_get_in_place(dict_ptr, call_bits).is_some()
                        {
                            return true;
                        }
                    }
                }
                let class_bits = object_class_bits(ptr);
                if class_bits != 0 {
                    if let Some(class_ptr) = obj_from_bits(class_bits).as_ptr() {
                        if object_type_id(class_ptr) == TYPE_ID_TYPE {
                            return class_attr_lookup_raw_mro(class_ptr, call_bits).is_some();
                        }
                    }
                }
                false
            }
            TYPE_ID_DATACLASS => {
                let call_bits =
                    intern_static_name(&runtime_state().interned.call_name, b"__call__");
                let desc_ptr = dataclass_desc_ptr(ptr);
                if !desc_ptr.is_null() && !(*desc_ptr).slots {
                    let dict_bits = dataclass_dict_bits(ptr);
                    if dict_bits != 0 {
                        if let Some(dict_ptr) = obj_from_bits(dict_bits).as_ptr() {
                            if object_type_id(dict_ptr) == TYPE_ID_DICT
                                && dict_get_in_place(dict_ptr, call_bits).is_some()
                            {
                                return true;
                            }
                        }
                    }
                }
                if !desc_ptr.is_null() {
                    let class_bits = (*desc_ptr).class_bits;
                    if class_bits != 0 {
                        if let Some(class_ptr) = obj_from_bits(class_bits).as_ptr() {
                            if object_type_id(class_ptr) == TYPE_ID_TYPE {
                                return class_attr_lookup_raw_mro(class_ptr, call_bits).is_some();
                            }
                        }
                    }
                }
                false
            }
            _ => false,
        }
    });
    MoltObject::from_bool(is_callable).bits()
}

#[no_mangle]
pub extern "C" fn molt_function_default_kind(func_bits: u64) -> i64 {
    let obj = obj_from_bits(func_bits);
    let Some(ptr) = obj.as_ptr() else {
        return 0;
    };
    unsafe {
        if object_type_id(ptr) != TYPE_ID_FUNCTION {
            return 0;
        }
        let dict_bits = function_dict_bits(ptr);
        if dict_bits == 0 {
            return 0;
        }
        obj_from_bits(dict_bits).as_int().unwrap_or(0)
    }
}

#[no_mangle]
pub extern "C" fn molt_function_closure_bits(func_bits: u64) -> u64 {
    let obj = obj_from_bits(func_bits);
    let Some(ptr) = obj.as_ptr() else {
        return 0;
    };
    unsafe {
        if object_type_id(ptr) != TYPE_ID_FUNCTION {
            return 0;
        }
        function_closure_bits(ptr)
    }
}

#[no_mangle]
pub extern "C" fn molt_call_arity_error(expected: i64, got: i64) -> u64 {
    let msg = format!("call arity mismatch (expected {expected}, got {got})");
    return raise_exception::<_>("TypeError", &msg);
}

#[no_mangle]
pub extern "C" fn molt_generator_send(gen_bits: u64, send_bits: u64) -> u64 {
    let Some(ptr) = maybe_ptr_from_bits(gen_bits) else {
        return raise_exception::<_>("TypeError", "expected generator");
    };
    unsafe {
        if object_type_id(ptr) != TYPE_ID_GENERATOR {
            return raise_exception::<_>("TypeError", "expected generator");
        }
        if generator_closed(ptr) {
            return generator_done_tuple(MoltObject::none().bits());
        }
        generator_set_slot(ptr, GEN_SEND_OFFSET, send_bits);
        generator_set_slot(ptr, GEN_THROW_OFFSET, MoltObject::none().bits());
        let header = header_from_obj_ptr(ptr);
        let poll_fn_addr = (*header).poll_fn;
        if poll_fn_addr == 0 {
            return generator_done_tuple(MoltObject::none().bits());
        }
        let caller_depth = exception_stack_depth();
        let caller_active =
            ACTIVE_EXCEPTION_STACK.with(|stack| std::mem::take(&mut *stack.borrow_mut()));
        let caller_context = caller_active
            .last()
            .copied()
            .unwrap_or(MoltObject::none().bits());
        exception_context_fallback_push(caller_context);
        let gen_active = generator_exception_stack_take(ptr);
        ACTIVE_EXCEPTION_STACK.with(|stack| {
            *stack.borrow_mut() = gen_active;
        });
        let gen_depth_bits = *generator_slot_ptr(ptr, GEN_EXC_DEPTH_OFFSET);
        let gen_depth = to_i64(obj_from_bits(gen_depth_bits)).unwrap_or(0);
        let gen_depth = if gen_depth < 0 { 0 } else { gen_depth as usize };
        exception_stack_set_depth(gen_depth);
        let prev_raise = generator_raise_active();
        set_generator_raise(true);
        generator_set_started(ptr);
        generator_set_running(ptr, true);
        let res = call_poll_fn(poll_fn_addr, ptr);
        generator_set_running(ptr, false);
        set_generator_raise(prev_raise);
        let pending = exception_pending();
        let exc_bits = if pending {
            let bits = molt_exception_last();
            clear_exception();
            bits
        } else {
            MoltObject::none().bits()
        };
        let new_depth = exception_stack_depth();
        generator_set_slot(
            ptr,
            GEN_EXC_DEPTH_OFFSET,
            MoltObject::from_int(new_depth as i64).bits(),
        );
        exception_context_align_depth(new_depth);
        let gen_active =
            ACTIVE_EXCEPTION_STACK.with(|stack| std::mem::take(&mut *stack.borrow_mut()));
        generator_exception_stack_store(ptr, gen_active);
        ACTIVE_EXCEPTION_STACK.with(|stack| {
            *stack.borrow_mut() = caller_active;
        });
        exception_stack_set_depth(caller_depth);
        exception_context_fallback_pop();
        if pending {
            generator_set_closed(ptr, true);
            let raised = molt_raise(exc_bits);
            dec_ref_bits(exc_bits);
            return raised;
        }
        res as u64
    }
}

#[no_mangle]
pub extern "C" fn molt_generator_throw(gen_bits: u64, exc_bits: u64) -> u64 {
    let Some(ptr) = maybe_ptr_from_bits(gen_bits) else {
        return raise_exception::<_>("TypeError", "expected generator");
    };
    unsafe {
        if object_type_id(ptr) != TYPE_ID_GENERATOR {
            return raise_exception::<_>("TypeError", "expected generator");
        }
        if generator_closed(ptr) {
            return generator_done_tuple(MoltObject::none().bits());
        }
        if !generator_started(ptr) {
            generator_set_closed(ptr, true);
            return molt_raise(exc_bits);
        }
        generator_set_slot(ptr, GEN_THROW_OFFSET, exc_bits);
        generator_set_slot(ptr, GEN_SEND_OFFSET, MoltObject::none().bits());
        let header = header_from_obj_ptr(ptr);
        let poll_fn_addr = (*header).poll_fn;
        if poll_fn_addr == 0 {
            return generator_done_tuple(MoltObject::none().bits());
        }
        let caller_depth = exception_stack_depth();
        let caller_active =
            ACTIVE_EXCEPTION_STACK.with(|stack| std::mem::take(&mut *stack.borrow_mut()));
        let caller_context = caller_active
            .last()
            .copied()
            .unwrap_or(MoltObject::none().bits());
        exception_context_fallback_push(caller_context);
        let gen_active = generator_exception_stack_take(ptr);
        ACTIVE_EXCEPTION_STACK.with(|stack| {
            *stack.borrow_mut() = gen_active;
        });
        let gen_depth_bits = *generator_slot_ptr(ptr, GEN_EXC_DEPTH_OFFSET);
        let gen_depth = to_i64(obj_from_bits(gen_depth_bits)).unwrap_or(0);
        let gen_depth = if gen_depth < 0 { 0 } else { gen_depth as usize };
        exception_stack_set_depth(gen_depth);
        let prev_raise = generator_raise_active();
        set_generator_raise(true);
        generator_set_started(ptr);
        generator_set_running(ptr, true);
        let res = call_poll_fn(poll_fn_addr, ptr);
        generator_set_running(ptr, false);
        set_generator_raise(prev_raise);
        let pending = exception_pending();
        let exc_bits = if pending {
            let bits = molt_exception_last();
            clear_exception();
            bits
        } else {
            MoltObject::none().bits()
        };
        let new_depth = exception_stack_depth();
        generator_set_slot(
            ptr,
            GEN_EXC_DEPTH_OFFSET,
            MoltObject::from_int(new_depth as i64).bits(),
        );
        exception_context_align_depth(new_depth);
        let gen_active =
            ACTIVE_EXCEPTION_STACK.with(|stack| std::mem::take(&mut *stack.borrow_mut()));
        generator_exception_stack_store(ptr, gen_active);
        ACTIVE_EXCEPTION_STACK.with(|stack| {
            *stack.borrow_mut() = caller_active;
        });
        exception_stack_set_depth(caller_depth);
        exception_context_fallback_pop();
        if pending {
            generator_set_closed(ptr, true);
            let raised = molt_raise(exc_bits);
            dec_ref_bits(exc_bits);
            return raised;
        }
        res as u64
    }
}

unsafe fn generator_resume_bits(gen_bits: u64) -> u64 {
    let Some(ptr) = maybe_ptr_from_bits(gen_bits) else {
        return raise_exception::<_>("TypeError", "expected generator");
    };
    if object_type_id(ptr) != TYPE_ID_GENERATOR {
        return raise_exception::<_>("TypeError", "expected generator");
    }
    if generator_closed(ptr) {
        return generator_done_tuple(MoltObject::none().bits());
    }
    let header = header_from_obj_ptr(ptr);
    let poll_fn_addr = (*header).poll_fn;
    if poll_fn_addr == 0 {
        return generator_done_tuple(MoltObject::none().bits());
    }
    let caller_depth = exception_stack_depth();
    let caller_active =
        ACTIVE_EXCEPTION_STACK.with(|stack| std::mem::take(&mut *stack.borrow_mut()));
    let caller_context = caller_active
        .last()
        .copied()
        .unwrap_or(MoltObject::none().bits());
    exception_context_fallback_push(caller_context);
    let gen_active = generator_exception_stack_take(ptr);
    ACTIVE_EXCEPTION_STACK.with(|stack| {
        *stack.borrow_mut() = gen_active;
    });
    let gen_depth_bits = *generator_slot_ptr(ptr, GEN_EXC_DEPTH_OFFSET);
    let gen_depth = to_i64(obj_from_bits(gen_depth_bits)).unwrap_or(0);
    let gen_depth = if gen_depth < 0 { 0 } else { gen_depth as usize };
    exception_stack_set_depth(gen_depth);
    let prev_raise = generator_raise_active();
    set_generator_raise(true);
    generator_set_started(ptr);
    generator_set_running(ptr, true);
    let res = call_poll_fn(poll_fn_addr, ptr);
    generator_set_running(ptr, false);
    set_generator_raise(prev_raise);
    let exc_pending = exception_pending();
    let exc_bits = if exc_pending {
        let bits = molt_exception_last();
        clear_exception();
        bits
    } else {
        MoltObject::none().bits()
    };
    let new_depth = exception_stack_depth();
    generator_set_slot(
        ptr,
        GEN_EXC_DEPTH_OFFSET,
        MoltObject::from_int(new_depth as i64).bits(),
    );
    exception_context_align_depth(new_depth);
    let gen_active = ACTIVE_EXCEPTION_STACK.with(|stack| std::mem::take(&mut *stack.borrow_mut()));
    generator_exception_stack_store(ptr, gen_active);
    ACTIVE_EXCEPTION_STACK.with(|stack| {
        *stack.borrow_mut() = caller_active;
    });
    exception_stack_set_depth(caller_depth);
    exception_context_fallback_pop();
    if exc_pending {
        generator_set_closed(ptr, true);
        let raised = molt_raise(exc_bits);
        dec_ref_bits(exc_bits);
        return raised;
    }
    res as u64
}

#[no_mangle]
pub extern "C" fn molt_generator_close(gen_bits: u64) -> u64 {
    let Some(ptr) = maybe_ptr_from_bits(gen_bits) else {
        return raise_exception::<_>("TypeError", "expected generator");
    };
    unsafe {
        if object_type_id(ptr) != TYPE_ID_GENERATOR {
            return raise_exception::<_>("TypeError", "expected generator");
        }
        if generator_closed(ptr) {
            return MoltObject::none().bits();
        }
        if !generator_started(ptr) {
            generator_set_closed(ptr, true);
            return MoltObject::none().bits();
        }
        let exc_ptr = alloc_exception("GeneratorExit", "");
        if exc_ptr.is_null() {
            return MoltObject::none().bits();
        }
        let exc_bits = MoltObject::from_ptr(exc_ptr).bits();
        generator_set_slot(ptr, GEN_THROW_OFFSET, exc_bits);
        dec_ref_bits(exc_bits);
        generator_set_slot(ptr, GEN_SEND_OFFSET, MoltObject::none().bits());
        let header = header_from_obj_ptr(ptr);
        let poll_fn_addr = (*header).poll_fn;
        if poll_fn_addr == 0 {
            generator_set_closed(ptr, true);
            return MoltObject::none().bits();
        }
        let caller_depth = exception_stack_depth();
        let caller_active =
            ACTIVE_EXCEPTION_STACK.with(|stack| std::mem::take(&mut *stack.borrow_mut()));
        let caller_context = caller_active
            .last()
            .copied()
            .unwrap_or(MoltObject::none().bits());
        exception_context_fallback_push(caller_context);
        let gen_active = generator_exception_stack_take(ptr);
        ACTIVE_EXCEPTION_STACK.with(|stack| {
            *stack.borrow_mut() = gen_active;
        });
        let gen_depth_bits = *generator_slot_ptr(ptr, GEN_EXC_DEPTH_OFFSET);
        let gen_depth = to_i64(obj_from_bits(gen_depth_bits)).unwrap_or(0);
        let gen_depth = if gen_depth < 0 { 0 } else { gen_depth as usize };
        exception_stack_set_depth(gen_depth);
        let prev_raise = generator_raise_active();
        set_generator_raise(true);
        generator_set_started(ptr);
        generator_set_running(ptr, true);
        let res = call_poll_fn(poll_fn_addr, ptr) as u64;
        generator_set_running(ptr, false);
        set_generator_raise(prev_raise);
        let pending = exception_pending();
        let exc_bits = if pending {
            let bits = molt_exception_last();
            clear_exception();
            bits
        } else {
            MoltObject::none().bits()
        };
        let new_depth = exception_stack_depth();
        generator_set_slot(
            ptr,
            GEN_EXC_DEPTH_OFFSET,
            MoltObject::from_int(new_depth as i64).bits(),
        );
        exception_context_align_depth(new_depth);
        let gen_active =
            ACTIVE_EXCEPTION_STACK.with(|stack| std::mem::take(&mut *stack.borrow_mut()));
        generator_exception_stack_store(ptr, gen_active);
        ACTIVE_EXCEPTION_STACK.with(|stack| {
            *stack.borrow_mut() = caller_active;
        });
        exception_stack_set_depth(caller_depth);
        exception_context_fallback_pop();
        if pending {
            let exc_obj = obj_from_bits(exc_bits);
            let is_exit = if let Some(exc_ptr) = exc_obj.as_ptr() {
                if object_type_id(exc_ptr) == TYPE_ID_EXCEPTION {
                    let kind = string_obj_to_owned(obj_from_bits(exception_kind_bits(exc_ptr)))
                        .unwrap_or_default();
                    kind == "GeneratorExit"
                } else {
                    false
                }
            } else {
                false
            };
            if is_exit {
                dec_ref_bits(exc_bits);
                generator_set_closed(ptr, true);
                return MoltObject::none().bits();
            }
            let raised = molt_raise(exc_bits);
            dec_ref_bits(exc_bits);
            return raised;
        }
        if let Some((_val, done)) = generator_unpack_pair(res) {
            if !done {
                return raise_exception::<_>("RuntimeError", "generator ignored GeneratorExit");
            }
        }
        generator_set_closed(ptr, true);
    }
    MoltObject::none().bits()
}

fn asyncgen_registry_insert(ptr: *mut u8) {
    if ptr.is_null() {
        return;
    }
    let mut guard = asyncgen_registry().lock().unwrap();
    guard.insert(PtrSlot(ptr));
}

fn asyncgen_registry_remove(ptr: *mut u8) {
    if ptr.is_null() {
        return;
    }
    let mut guard = asyncgen_registry().lock().unwrap();
    guard.remove(&PtrSlot(ptr));
}

fn asyncgen_registry_take() -> Vec<u64> {
    let mut guard = asyncgen_registry().lock().unwrap();
    if guard.is_empty() {
        return Vec::new();
    }
    let mut gens = Vec::with_capacity(guard.len());
    for slot in guard.iter() {
        if slot.0.is_null() {
            continue;
        }
        let bits = MoltObject::from_ptr(slot.0).bits();
        inc_ref_bits(bits);
        gens.push(bits);
    }
    guard.clear();
    gens
}

unsafe fn asyncgen_slot_ptr(ptr: *mut u8, offset: usize) -> *mut u64 {
    ptr.add(offset) as *mut u64
}

unsafe fn asyncgen_gen_bits(ptr: *mut u8) -> u64 {
    *asyncgen_slot_ptr(ptr, ASYNCGEN_GEN_OFFSET)
}

unsafe fn asyncgen_running_bits(ptr: *mut u8) -> u64 {
    *asyncgen_slot_ptr(ptr, ASYNCGEN_RUNNING_OFFSET)
}

unsafe fn asyncgen_pending_bits(ptr: *mut u8) -> u64 {
    *asyncgen_slot_ptr(ptr, ASYNCGEN_PENDING_OFFSET)
}

unsafe fn asyncgen_set_running_bits(ptr: *mut u8, bits: u64) {
    let slot = asyncgen_slot_ptr(ptr, ASYNCGEN_RUNNING_OFFSET);
    let old_bits = *slot;
    if old_bits != bits {
        dec_ref_bits(old_bits);
        inc_ref_bits(bits);
        *slot = bits;
    }
}

unsafe fn asyncgen_set_pending_bits(ptr: *mut u8, bits: u64) {
    let slot = asyncgen_slot_ptr(ptr, ASYNCGEN_PENDING_OFFSET);
    let old_bits = *slot;
    if old_bits != bits {
        dec_ref_bits(old_bits);
        inc_ref_bits(bits);
        *slot = bits;
    }
}

unsafe fn asyncgen_clear_pending_bits(ptr: *mut u8) {
    asyncgen_set_pending_bits(ptr, MoltObject::none().bits());
}

unsafe fn asyncgen_clear_running_bits(ptr: *mut u8) {
    asyncgen_set_running_bits(ptr, MoltObject::none().bits());
}

unsafe fn asyncgen_running(ptr: *mut u8) -> bool {
    !obj_from_bits(asyncgen_running_bits(ptr)).is_none()
}

unsafe fn asyncgen_await_bits(ptr: *mut u8) -> u64 {
    let running_bits = asyncgen_running_bits(ptr);
    let Some(running_ptr) = maybe_ptr_from_bits(running_bits) else {
        return MoltObject::none().bits();
    };
    let awaited = {
        let map = task_waiting_on().lock().unwrap();
        map.get(&PtrSlot(running_ptr)).copied()
    };
    let Some(PtrSlot(awaited_ptr)) = awaited else {
        return MoltObject::none().bits();
    };
    if awaited_ptr.is_null() {
        return MoltObject::none().bits();
    }
    let bits = MoltObject::from_ptr(awaited_ptr).bits();
    inc_ref_bits(bits);
    bits
}

unsafe fn asyncgen_code_bits(ptr: *mut u8) -> u64 {
    let gen_bits = asyncgen_gen_bits(ptr);
    let Some(gen_ptr) = maybe_ptr_from_bits(gen_bits) else {
        return MoltObject::none().bits();
    };
    if object_type_id(gen_ptr) != TYPE_ID_GENERATOR {
        return MoltObject::none().bits();
    }
    let header = header_from_obj_ptr(gen_ptr);
    let poll_fn_addr = (*header).poll_fn;
    let code_bits = fn_ptr_code_get(poll_fn_addr);
    if code_bits == 0 {
        return MoltObject::none().bits();
    }
    inc_ref_bits(code_bits);
    code_bits
}

fn asyncgen_running_message(op: i64) -> &'static str {
    match op {
        ASYNCGEN_OP_ANEXT => "anext(): asynchronous generator is already running",
        ASYNCGEN_OP_ASEND => "asend(): asynchronous generator is already running",
        ASYNCGEN_OP_ATHROW => "athrow(): asynchronous generator is already running",
        ASYNCGEN_OP_ACLOSE => "aclose(): asynchronous generator is already running",
        _ => "asynchronous generator is already running",
    }
}

unsafe fn asyncgen_future_new(asyncgen_bits: u64, op_kind: i64, arg_bits: u64) -> u64 {
    let payload = (3 * std::mem::size_of::<u64>()) as u64;
    let obj_bits = molt_future_new(asyncgen_poll_fn_addr(), payload);
    if obj_from_bits(obj_bits).is_none() {
        return obj_bits;
    }
    let Some(obj_ptr) = resolve_obj_ptr(obj_bits) else {
        return MoltObject::none().bits();
    };
    let payload_ptr = obj_ptr as *mut u64;
    *payload_ptr = asyncgen_bits;
    *payload_ptr.add(1) = MoltObject::from_int(op_kind).bits();
    *payload_ptr.add(2) = arg_bits;
    inc_ref_bits(asyncgen_bits);
    inc_ref_bits(arg_bits);
    obj_bits
}

#[no_mangle]
pub extern "C" fn molt_asyncgen_new(gen_bits: u64) -> u64 {
    let Some(gen_ptr) = maybe_ptr_from_bits(gen_bits) else {
        return raise_exception::<_>("TypeError", "expected generator");
    };
    unsafe {
        if object_type_id(gen_ptr) != TYPE_ID_GENERATOR {
            return raise_exception::<_>("TypeError", "expected generator");
        }
        let total = std::mem::size_of::<MoltHeader>() + ASYNCGEN_CONTROL_SIZE;
        let ptr = alloc_object(total, TYPE_ID_ASYNC_GENERATOR);
        if ptr.is_null() {
            return MoltObject::none().bits();
        }
        let payload_ptr = ptr as *mut u64;
        *payload_ptr = gen_bits;
        inc_ref_bits(gen_bits);
        *payload_ptr.add(1) = MoltObject::none().bits();
        *payload_ptr.add(2) = MoltObject::none().bits();
        object_mark_has_ptrs(ptr);
        asyncgen_registry_insert(ptr);
        MoltObject::from_ptr(ptr).bits()
    }
}

#[no_mangle]
pub extern "C" fn molt_asyncgen_aiter(asyncgen_bits: u64) -> u64 {
    let Some(ptr) = maybe_ptr_from_bits(asyncgen_bits) else {
        return raise_exception::<_>("TypeError", "expected async generator");
    };
    unsafe {
        if object_type_id(ptr) != TYPE_ID_ASYNC_GENERATOR {
            return raise_exception::<_>("TypeError", "expected async generator");
        }
    }
    inc_ref_bits(asyncgen_bits);
    asyncgen_bits
}

#[no_mangle]
pub extern "C" fn molt_asyncgen_anext(asyncgen_bits: u64) -> u64 {
    unsafe { asyncgen_future_new(asyncgen_bits, ASYNCGEN_OP_ANEXT, MoltObject::none().bits()) }
}

#[no_mangle]
pub extern "C" fn molt_asyncgen_asend(asyncgen_bits: u64, val_bits: u64) -> u64 {
    unsafe { asyncgen_future_new(asyncgen_bits, ASYNCGEN_OP_ASEND, val_bits) }
}

#[no_mangle]
pub extern "C" fn molt_asyncgen_athrow(asyncgen_bits: u64, exc_bits: u64) -> u64 {
    unsafe { asyncgen_future_new(asyncgen_bits, ASYNCGEN_OP_ATHROW, exc_bits) }
}

#[no_mangle]
pub extern "C" fn molt_asyncgen_aclose(asyncgen_bits: u64) -> u64 {
    let exc_ptr = alloc_exception("GeneratorExit", "");
    if exc_ptr.is_null() {
        return MoltObject::none().bits();
    }
    let exc_bits = MoltObject::from_ptr(exc_ptr).bits();
    let future_bits = unsafe { asyncgen_future_new(asyncgen_bits, ASYNCGEN_OP_ACLOSE, exc_bits) };
    dec_ref_bits(exc_bits);
    future_bits
}

#[no_mangle]
pub extern "C" fn molt_asyncgen_shutdown() -> u64 {
    let gens = asyncgen_registry_take();
    for gen_bits in gens {
        let future_bits = molt_asyncgen_aclose(gen_bits);
        if !obj_from_bits(future_bits).is_none() {
            unsafe {
                let _ = molt_block_on(future_bits);
            }
            if exception_pending() {
                let exc_bits = molt_exception_last();
                molt_exception_clear();
                dec_ref_bits(exc_bits);
            }
            dec_ref_bits(future_bits);
        }
        dec_ref_bits(gen_bits);
    }
    MoltObject::none().bits()
}

#[no_mangle]
pub unsafe extern "C" fn molt_asyncgen_poll(obj_bits: u64) -> i64 {
    let obj_ptr = ptr_from_bits(obj_bits);
    if obj_ptr.is_null() {
        return MoltObject::none().bits() as i64;
    }
    let header = header_from_obj_ptr(obj_ptr);
    let payload_bytes = (*header)
        .size
        .saturating_sub(std::mem::size_of::<MoltHeader>());
    if payload_bytes < 3 * std::mem::size_of::<u64>() {
        return MoltObject::none().bits() as i64;
    }
    let payload_ptr = obj_ptr as *mut u64;
    let asyncgen_bits = *payload_ptr;
    let op_bits = *payload_ptr.add(1);
    let arg_bits = *payload_ptr.add(2);
    let op = to_i64(obj_from_bits(op_bits)).unwrap_or(-1);
    let Some(asyncgen_ptr) = maybe_ptr_from_bits(asyncgen_bits) else {
        return raise_exception::<i64>("TypeError", "expected async generator");
    };
    if object_type_id(asyncgen_ptr) != TYPE_ID_ASYNC_GENERATOR {
        return raise_exception::<i64>("TypeError", "expected async generator");
    }
    let gen_bits = asyncgen_gen_bits(asyncgen_ptr);
    let Some(gen_ptr) = maybe_ptr_from_bits(gen_bits) else {
        return raise_exception::<i64>("TypeError", "expected generator");
    };
    if object_type_id(gen_ptr) != TYPE_ID_GENERATOR {
        return raise_exception::<i64>("TypeError", "expected generator");
    }
    let running_bits = asyncgen_running_bits(asyncgen_ptr);
    let running_obj = obj_from_bits(running_bits);
    if (*header).state == 0 {
        if !running_obj.is_none() && running_bits != obj_bits {
            return raise_exception::<i64>("RuntimeError", asyncgen_running_message(op));
        }
    } else if !running_obj.is_none() && running_bits != obj_bits {
        return raise_exception::<i64>("RuntimeError", asyncgen_running_message(op));
    }
    if generator_running(gen_ptr) {
        return raise_exception::<i64>("RuntimeError", asyncgen_running_message(op));
    }
    let pending_bits = asyncgen_pending_bits(asyncgen_ptr);
    if !obj_from_bits(pending_bits).is_none() {
        if matches!(op, ASYNCGEN_OP_ANEXT | ASYNCGEN_OP_ASEND) {
            inc_ref_bits(pending_bits);
            asyncgen_clear_pending_bits(asyncgen_ptr);
            let raised = molt_raise(pending_bits);
            dec_ref_bits(pending_bits);
            return raised as i64;
        }
    }

    let res_bits = if (*header).state != 0 {
        generator_resume_bits(gen_bits)
    } else {
        match op {
            ASYNCGEN_OP_ANEXT => {
                if generator_closed(gen_ptr) {
                    if generator_pending_throw(gen_ptr) {
                        let throw_bits = *generator_slot_ptr(gen_ptr, GEN_THROW_OFFSET);
                        inc_ref_bits(throw_bits);
                        generator_set_slot(gen_ptr, GEN_THROW_OFFSET, MoltObject::none().bits());
                        let raised = molt_raise(throw_bits);
                        dec_ref_bits(throw_bits);
                        return raised as i64;
                    }
                    return raise_exception::<i64>("StopAsyncIteration", "");
                }
                if generator_pending_throw(gen_ptr) {
                    generator_resume_bits(gen_bits)
                } else {
                    molt_generator_send(gen_bits, MoltObject::none().bits())
                }
            }
            ASYNCGEN_OP_ASEND => {
                if generator_closed(gen_ptr) {
                    if generator_pending_throw(gen_ptr) {
                        return generator_resume_bits(gen_bits) as i64;
                    }
                    return raise_exception::<i64>("StopAsyncIteration", "");
                }
                if !generator_started(gen_ptr) && !obj_from_bits(arg_bits).is_none() {
                    return raise_exception::<i64>(
                        "TypeError",
                        "can't send non-None value to a just-started async generator",
                    );
                }
                if generator_pending_throw(gen_ptr) {
                    generator_resume_bits(gen_bits)
                } else {
                    molt_generator_send(gen_bits, arg_bits)
                }
            }
            ASYNCGEN_OP_ATHROW => {
                if generator_closed(gen_ptr) {
                    if generator_pending_throw(gen_ptr) {
                        return raise_exception::<i64>("StopAsyncIteration", "");
                    }
                    return MoltObject::none().bits() as i64;
                }
                molt_generator_throw(gen_bits, arg_bits)
            }
            ASYNCGEN_OP_ACLOSE => {
                if generator_closed(gen_ptr) {
                    return MoltObject::none().bits() as i64;
                }
                if !generator_started(gen_ptr) {
                    generator_set_closed(gen_ptr, true);
                    return MoltObject::none().bits() as i64;
                }
                molt_generator_throw(gen_bits, arg_bits)
            }
            _ => return raise_exception::<i64>("TypeError", "invalid async generator op"),
        }
    };

    if exception_pending() {
        if running_bits == obj_bits {
            asyncgen_clear_running_bits(asyncgen_ptr);
        }
        (*header).state = 0;
        if op == ASYNCGEN_OP_ACLOSE {
            let exc_bits = molt_exception_last();
            let kind_bits = molt_exception_kind(exc_bits);
            let kind = string_obj_to_owned(obj_from_bits(kind_bits));
            dec_ref_bits(kind_bits);
            if matches!(
                kind.as_deref(),
                Some("GeneratorExit" | "StopAsyncIteration")
            ) {
                molt_exception_clear();
                dec_ref_bits(exc_bits);
                generator_set_closed(gen_ptr, true);
                return MoltObject::none().bits() as i64;
            }
            dec_ref_bits(exc_bits);
        }
        return res_bits as i64;
    }

    if res_bits as i64 == pending_bits_i64() {
        asyncgen_set_running_bits(asyncgen_ptr, obj_bits);
        (*header).state = 1;
        return res_bits as i64;
    }

    if running_bits == obj_bits {
        asyncgen_clear_running_bits(asyncgen_ptr);
    }
    (*header).state = 0;

    if let Some((val_bits, done)) = generator_unpack_pair(res_bits) {
        if !done {
            inc_ref_bits(val_bits);
        }
        dec_ref_bits(res_bits);
        if op == ASYNCGEN_OP_ACLOSE {
            generator_set_closed(gen_ptr, true);
            if done {
                return MoltObject::none().bits() as i64;
            }
            if !obj_from_bits(arg_bits).is_none() {
                asyncgen_set_pending_bits(asyncgen_ptr, arg_bits);
                generator_set_slot(gen_ptr, GEN_THROW_OFFSET, MoltObject::none().bits());
            }
            return raise_exception::<i64>("RuntimeError", "async generator ignored GeneratorExit");
        }
        if done {
            match op {
                ASYNCGEN_OP_ANEXT | ASYNCGEN_OP_ASEND => {
                    return raise_exception::<i64>("StopAsyncIteration", "");
                }
                ASYNCGEN_OP_ATHROW => {
                    return MoltObject::none().bits() as i64;
                }
                _ => {
                    return MoltObject::none().bits() as i64;
                }
            }
        }
        return val_bits as i64;
    }

    res_bits as i64
}

#[no_mangle]
pub extern "C" fn molt_context_new(
    enter_fn: *const (),
    exit_fn: *const (),
    payload_bits: u64,
) -> u64 {
    if enter_fn.is_null() || exit_fn.is_null() {
        return raise_exception::<_>("TypeError", "context manager hooks must be non-null");
    }
    let ptr = alloc_context_manager(enter_fn, exit_fn, payload_bits);
    if ptr.is_null() {
        MoltObject::none().bits()
    } else {
        MoltObject::from_ptr(ptr).bits()
    }
}

#[no_mangle]
pub extern "C" fn molt_context_enter(ctx_bits: u64) -> u64 {
    let ctx_obj = obj_from_bits(ctx_bits);
    let Some(ptr) = ctx_obj.as_ptr() else {
        return raise_exception::<_>("TypeError", "context manager must be an object");
    };
    unsafe {
        let type_id = object_type_id(ptr);
        match type_id {
            TYPE_ID_CONTEXT_MANAGER => {
                let enter_fn_addr = context_enter_fn(ptr);
                if enter_fn_addr.is_null() {
                    return raise_exception::<_>("TypeError", "context manager missing __enter__");
                }
                let enter_fn =
                    std::mem::transmute::<*const (), extern "C" fn(u64) -> u64>(enter_fn_addr);
                let res = enter_fn(context_payload_bits(ptr));
                if exception_pending() {
                    return MoltObject::none().bits();
                }
                context_stack_push(ctx_bits);
                res
            }
            TYPE_ID_FILE_HANDLE => {
                let res = file_handle_enter(ptr);
                if exception_pending() {
                    return MoltObject::none().bits();
                }
                context_stack_push(ctx_bits);
                res
            }
            _ => {
                let enter_name_bits =
                    intern_static_name(&runtime_state().interned.enter_name, b"__enter__");
                let Some(enter_bits) = attr_lookup_ptr_allow_missing(ptr, enter_name_bits) else {
                    return raise_exception::<_>("TypeError", "context manager missing __enter__");
                };
                let res = call_callable0(enter_bits);
                dec_ref_bits(enter_bits);
                if exception_pending() {
                    return MoltObject::none().bits();
                }
                context_stack_push(ctx_bits);
                res
            }
        }
    }
}

#[no_mangle]
pub extern "C" fn molt_context_exit(ctx_bits: u64, exc_bits: u64) -> u64 {
    let ctx_obj = obj_from_bits(ctx_bits);
    let Some(ptr) = ctx_obj.as_ptr() else {
        return raise_exception::<_>("TypeError", "context manager must be an object");
    };
    unsafe {
        let type_id = object_type_id(ptr);
        match type_id {
            TYPE_ID_CONTEXT_MANAGER => {
                let exit_fn_addr = context_exit_fn(ptr);
                if exit_fn_addr.is_null() {
                    return raise_exception::<_>("TypeError", "context manager missing __exit__");
                }
                let exit_fn =
                    std::mem::transmute::<*const (), extern "C" fn(u64, u64) -> u64>(exit_fn_addr);
                context_stack_pop(ctx_bits);
                exit_fn(context_payload_bits(ptr), exc_bits)
            }
            TYPE_ID_FILE_HANDLE => {
                let res = file_handle_exit(ptr, exc_bits);
                context_stack_pop(ctx_bits);
                res
            }
            _ => {
                let exit_name_bits =
                    intern_static_name(&runtime_state().interned.exit_name, b"__exit__");
                let Some(exit_bits) = attr_lookup_ptr_allow_missing(ptr, exit_name_bits) else {
                    context_stack_pop(ctx_bits);
                    return raise_exception::<_>("TypeError", "context manager missing __exit__");
                };
                let none_bits = MoltObject::none().bits();
                let exc_obj = obj_from_bits(exc_bits);
                let (exc_type_bits, exc_val_bits, tb_bits) = if exc_obj.is_none() {
                    (none_bits, none_bits, none_bits)
                } else {
                    let tb_bits = exc_obj
                        .as_ptr()
                        .map(|ptr| exception_trace_bits(ptr))
                        .unwrap_or(none_bits);
                    (type_of_bits(exc_bits), exc_bits, tb_bits)
                };
                let res = call_callable3(exit_bits, exc_type_bits, exc_val_bits, tb_bits);
                dec_ref_bits(exit_bits);
                context_stack_pop(ctx_bits);
                res
            }
        }
    }
}

#[no_mangle]
pub extern "C" fn molt_context_unwind(exc_bits: u64) -> u64 {
    context_stack_unwind(exc_bits);
    MoltObject::none().bits()
}

#[no_mangle]
pub extern "C" fn molt_context_depth() -> u64 {
    MoltObject::from_int(context_stack_depth() as i64).bits()
}

#[no_mangle]
pub extern "C" fn molt_context_unwind_to(depth_bits: u64, exc_bits: u64) -> u64 {
    let depth = match to_i64(obj_from_bits(depth_bits)) {
        Some(val) if val >= 0 => val as usize,
        _ => return raise_exception::<_>("TypeError", "context depth must be a non-negative int"),
    };
    context_stack_unwind_to(depth, exc_bits);
    MoltObject::none().bits()
}

#[no_mangle]
pub extern "C" fn molt_context_null(payload_bits: u64) -> u64 {
    let enter_fn = context_null_enter as *const ();
    let exit_fn = context_null_exit as *const ();
    let ptr = alloc_context_manager(enter_fn, exit_fn, payload_bits);
    if ptr.is_null() {
        MoltObject::none().bits()
    } else {
        MoltObject::from_ptr(ptr).bits()
    }
}

#[no_mangle]
pub extern "C" fn molt_context_closing(payload_bits: u64) -> u64 {
    let enter_fn = context_closing_enter as *const ();
    let exit_fn = context_closing_exit as *const ();
    let ptr = alloc_context_manager(enter_fn, exit_fn, payload_bits);
    if ptr.is_null() {
        MoltObject::none().bits()
    } else {
        MoltObject::from_ptr(ptr).bits()
    }
}

struct FileMode {
    options: OpenOptions,
    readable: bool,
    writable: bool,
    append: bool,
    create: bool,
    truncate: bool,
    create_new: bool,
    text: bool,
}

fn parse_file_mode(mode: &str) -> Result<FileMode, String> {
    let mut kind: Option<char> = None;
    let mut kind_dup = false;
    let mut read = false;
    let mut write = false;
    let mut append = false;
    let mut truncate = false;
    let mut create = false;
    let mut create_new = false;
    let mut saw_plus = 0usize;
    let mut saw_text = false;
    let mut saw_binary = false;

    for ch in mode.chars() {
        match ch {
            'r' | 'w' | 'a' | 'x' => {
                if let Some(prev) = kind {
                    if prev == ch {
                        kind_dup = true;
                    } else {
                        return Err(
                            "must have exactly one of create/read/write/append mode".to_string()
                        );
                    }
                } else {
                    kind = Some(ch);
                }
                match ch {
                    'r' => read = true,
                    'w' => {
                        write = true;
                        truncate = true;
                        create = true;
                    }
                    'a' => {
                        write = true;
                        append = true;
                        create = true;
                    }
                    'x' => {
                        write = true;
                        create = true;
                        create_new = true;
                    }
                    _ => {}
                }
            }
            '+' => {
                saw_plus += 1;
                read = true;
                write = true;
            }
            'b' => saw_binary = true,
            't' => saw_text = true,
            _ => return Err(format!("invalid mode: '{mode}'")),
        }
    }

    if saw_binary && saw_text {
        return Err("can't have text and binary mode at once".to_string());
    }
    if saw_plus > 1 {
        return Err(format!("invalid mode: '{mode}'"));
    }
    if kind.is_none() {
        return Err(
            "Must have exactly one of create/read/write/append mode and at most one plus"
                .to_string(),
        );
    }
    if kind_dup {
        return Err(format!("invalid mode: '{mode}'"));
    }

    let mut options = OpenOptions::new();
    options
        .read(read)
        .write(write)
        .append(append)
        .truncate(truncate)
        .create(create);
    if create_new {
        options.create_new(true);
    }
    Ok(FileMode {
        options,
        readable: read,
        writable: write,
        append,
        create,
        truncate,
        create_new,
        text: !saw_binary,
    })
}

fn open_arg_type(bits: u64, name: &str, allow_none: bool) -> Option<String> {
    let obj = obj_from_bits(bits);
    if allow_none && obj.is_none() {
        return None;
    }
    if let Some(text) = string_obj_to_owned(obj) {
        return Some(text);
    }
    let type_name = class_name_for_error(type_of_bits(bits));
    let msg = if allow_none {
        format!("open() argument '{name}' must be str or None, not {type_name}")
    } else {
        format!("open() argument '{name}' must be str, not {type_name}")
    };
    return raise_exception::<_>("TypeError", &msg);
}

fn open_arg_newline(bits: u64) -> Option<String> {
    let obj = obj_from_bits(bits);
    if obj.is_none() {
        return None;
    }
    let Some(text) = string_obj_to_owned(obj) else {
        let type_name = class_name_for_error(type_of_bits(bits));
        let msg = format!("open() argument 'newline' must be str or None, not {type_name}");
        return raise_exception::<_>("TypeError", &msg);
    };
    match text.as_str() {
        "" | "\n" | "\r" | "\r\n" => Some(text),
        _ => {
            let msg = format!("illegal newline value: {text}");
            return raise_exception::<_>("ValueError", &msg);
        }
    }
}

fn reconfigure_arg_type(bits: u64, name: &str) -> Option<String> {
    let obj = obj_from_bits(bits);
    if obj.is_none() {
        return None;
    }
    if let Some(text) = string_obj_to_owned(obj) {
        return Some(text);
    }
    let type_name = class_name_for_error(type_of_bits(bits));
    let msg = format!("reconfigure() argument '{name}' must be str or None, not {type_name}");
    return raise_exception::<_>("TypeError", &msg);
}

fn reconfigure_arg_newline(bits: u64) -> Option<String> {
    let obj = obj_from_bits(bits);
    if obj.is_none() {
        return None;
    }
    let Some(text) = string_obj_to_owned(obj) else {
        let type_name = class_name_for_error(type_of_bits(bits));
        let msg = format!("reconfigure() argument 'newline' must be str or None, not {type_name}");
        return raise_exception::<_>("TypeError", &msg);
    };
    match text.as_str() {
        "" | "\n" | "\r" | "\r\n" => Some(text),
        _ => {
            let msg = format!("illegal newline value: {text}");
            return raise_exception::<_>("ValueError", &msg);
        }
    }
}

fn open_arg_encoding(bits: u64) -> Option<String> {
    open_arg_type(bits, "encoding", true)
}

fn open_arg_errors(bits: u64) -> Option<String> {
    open_arg_type(bits, "errors", true)
}

fn file_mode_to_flags(mode: &FileMode) -> i32 {
    #[allow(clippy::useless_conversion)]
    let mut flags = 0;
    if mode.readable && !mode.writable {
        flags |= libc::O_RDONLY;
    } else if mode.writable && !mode.readable {
        flags |= libc::O_WRONLY;
    } else {
        flags |= libc::O_RDWR;
    }
    if mode.append {
        flags |= libc::O_APPEND;
    }
    if mode.create {
        flags |= libc::O_CREAT;
    }
    if mode.truncate {
        flags |= libc::O_TRUNC;
    }
    if mode.create_new {
        flags |= libc::O_EXCL;
    }
    flags
}

#[cfg(unix)]
fn file_from_fd(fd: i64) -> Option<std::fs::File> {
    use std::os::fd::FromRawFd;
    if fd < 0 {
        return None;
    }
    Some(unsafe { std::fs::File::from_raw_fd(fd as i32) })
}

#[cfg(windows)]
fn file_from_fd(fd: i64) -> Option<std::fs::File> {
    use std::os::windows::io::FromRawHandle;
    let handle = unsafe { libc::_get_osfhandle(fd as libc::c_int) };
    if handle == -1 {
        return None;
    }
    Some(unsafe { std::fs::File::from_raw_handle(handle as *mut _) })
}

#[cfg(not(any(unix, windows)))]
fn file_from_fd(_fd: i64) -> Option<std::fs::File> {
    None
}

#[cfg(unix)]
fn dup_fd(fd: i64) -> Option<i64> {
    if fd < 0 {
        return None;
    }
    let duped = unsafe { libc::dup(fd as libc::c_int) };
    if duped < 0 {
        None
    } else {
        Some(duped as i64)
    }
}

#[cfg(windows)]
fn dup_fd(fd: i64) -> Option<i64> {
    if fd < 0 {
        return None;
    }
    let duped = unsafe { libc::_dup(fd as libc::c_int) };
    if duped < 0 {
        None
    } else {
        Some(duped as i64)
    }
}

#[cfg(not(any(unix, windows)))]
fn dup_fd(_fd: i64) -> Option<i64> {
    None
}

fn path_from_bits(file_bits: u64) -> Result<std::path::PathBuf, String> {
    let obj = obj_from_bits(file_bits);
    if let Some(text) = string_obj_to_owned(obj) {
        return Ok(std::path::PathBuf::from(text));
    }
    if let Some(ptr) = obj.as_ptr() {
        unsafe {
            let type_id = object_type_id(ptr);
            if type_id == TYPE_ID_BYTES {
                let len = bytes_len(ptr);
                let bytes = std::slice::from_raw_parts(bytes_data(ptr), len);
                #[cfg(unix)]
                {
                    use std::os::unix::ffi::OsStringExt;
                    let path = std::ffi::OsString::from_vec(bytes.to_vec());
                    return Ok(std::path::PathBuf::from(path));
                }
                #[cfg(windows)]
                {
                    let path = std::str::from_utf8(bytes)
                        .map_err(|_| "open path bytes must be utf-8".to_string())?;
                    return Ok(std::path::PathBuf::from(path));
                }
            }
            let fspath_name_bits =
                intern_static_name(&runtime_state().interned.fspath_name, b"__fspath__");
            if let Some(call_bits) = attr_lookup_ptr(ptr, fspath_name_bits) {
                let res_bits = call_callable0(call_bits);
                dec_ref_bits(call_bits);
                if exception_pending() {
                    return Err("open failed".to_string());
                }
                let res_obj = obj_from_bits(res_bits);
                if let Some(text) = string_obj_to_owned(res_obj) {
                    dec_ref_bits(res_bits);
                    return Ok(std::path::PathBuf::from(text));
                }
                if let Some(res_ptr) = res_obj.as_ptr() {
                    if object_type_id(res_ptr) == TYPE_ID_BYTES {
                        let len = bytes_len(res_ptr);
                        let bytes = std::slice::from_raw_parts(bytes_data(res_ptr), len);
                        #[cfg(unix)]
                        {
                            use std::os::unix::ffi::OsStringExt;
                            let path = std::ffi::OsString::from_vec(bytes.to_vec());
                            dec_ref_bits(res_bits);
                            return Ok(std::path::PathBuf::from(path));
                        }
                        #[cfg(windows)]
                        {
                            let path = std::str::from_utf8(bytes)
                                .map_err(|_| "open path bytes must be utf-8".to_string())?;
                            dec_ref_bits(res_bits);
                            return Ok(std::path::PathBuf::from(path));
                        }
                    }
                }
                let res_type = class_name_for_error(type_of_bits(res_bits));
                dec_ref_bits(res_bits);
                let obj_type = class_name_for_error(type_of_bits(file_bits));
                return Err(format!(
                    "expected {obj_type}.__fspath__() to return str or bytes, not {res_type}"
                ));
            }
        }
    }
    let obj_type = class_name_for_error(type_of_bits(file_bits));
    Err(format!(
        "expected str, bytes or os.PathLike object, not {obj_type}"
    ))
}

fn open_arg_path(file_bits: u64) -> Result<(std::path::PathBuf, u64), String> {
    let obj = obj_from_bits(file_bits);
    if let Some(text) = string_obj_to_owned(obj) {
        let name_ptr = alloc_string(text.as_bytes());
        if name_ptr.is_null() {
            return Err("open failed".to_string());
        }
        let name_bits = MoltObject::from_ptr(name_ptr).bits();
        return Ok((std::path::PathBuf::from(text), name_bits));
    }
    if let Some(ptr) = obj.as_ptr() {
        unsafe {
            let type_id = object_type_id(ptr);
            if type_id == TYPE_ID_BYTES {
                let len = bytes_len(ptr);
                let bytes = std::slice::from_raw_parts(bytes_data(ptr), len);
                let name_ptr = alloc_bytes(bytes);
                if name_ptr.is_null() {
                    return Err("open failed".to_string());
                }
                let name_bits = MoltObject::from_ptr(name_ptr).bits();
                #[cfg(unix)]
                {
                    use std::os::unix::ffi::OsStringExt;
                    let path = std::ffi::OsString::from_vec(bytes.to_vec());
                    return Ok((std::path::PathBuf::from(path), name_bits));
                }
                #[cfg(windows)]
                {
                    let path = std::str::from_utf8(bytes)
                        .map_err(|_| "open path bytes must be utf-8".to_string())?;
                    return Ok((std::path::PathBuf::from(path), name_bits));
                }
            }
            let fspath_name_bits =
                intern_static_name(&runtime_state().interned.fspath_name, b"__fspath__");
            if let Some(call_bits) = attr_lookup_ptr(ptr, fspath_name_bits) {
                let res_bits = call_callable0(call_bits);
                dec_ref_bits(call_bits);
                if exception_pending() {
                    return Err("open failed".to_string());
                }
                let res_obj = obj_from_bits(res_bits);
                if let Some(text) = string_obj_to_owned(res_obj) {
                    let name_ptr = alloc_string(text.as_bytes());
                    if name_ptr.is_null() {
                        return Err("open failed".to_string());
                    }
                    let name_bits = MoltObject::from_ptr(name_ptr).bits();
                    dec_ref_bits(res_bits);
                    return Ok((std::path::PathBuf::from(text), name_bits));
                }
                if let Some(res_ptr) = res_obj.as_ptr() {
                    if object_type_id(res_ptr) == TYPE_ID_BYTES {
                        let len = bytes_len(res_ptr);
                        let bytes = std::slice::from_raw_parts(bytes_data(res_ptr), len);
                        let name_ptr = alloc_bytes(bytes);
                        if name_ptr.is_null() {
                            return Err("open failed".to_string());
                        }
                        let name_bits = MoltObject::from_ptr(name_ptr).bits();
                        #[cfg(unix)]
                        {
                            use std::os::unix::ffi::OsStringExt;
                            let path = std::ffi::OsString::from_vec(bytes.to_vec());
                            dec_ref_bits(res_bits);
                            return Ok((std::path::PathBuf::from(path), name_bits));
                        }
                        #[cfg(windows)]
                        {
                            let path = std::str::from_utf8(bytes)
                                .map_err(|_| "open path bytes must be utf-8".to_string())?;
                            dec_ref_bits(res_bits);
                            return Ok((std::path::PathBuf::from(path), name_bits));
                        }
                    }
                }
                let res_type = class_name_for_error(type_of_bits(res_bits));
                dec_ref_bits(res_bits);
                let obj_type = class_name_for_error(type_of_bits(file_bits));
                return Err(format!(
                    "expected {obj_type}.__fspath__() to return str or bytes, not {res_type}"
                ));
            }
        }
    }
    let obj_type = class_name_for_error(type_of_bits(file_bits));
    Err(format!(
        "expected str, bytes or os.PathLike object, not {obj_type}"
    ))
}

#[allow(clippy::too_many_arguments)]
fn open_impl(
    file_bits: u64,
    mode_bits: u64,
    buffering_bits: u64,
    encoding_bits: u64,
    errors_bits: u64,
    newline_bits: u64,
    closefd_bits: u64,
    opener_bits: u64,
) -> u64 {
    struct BitsGuard(u64);
    impl Drop for BitsGuard {
        fn drop(&mut self) {
            if self.0 != 0 {
                dec_ref_bits(self.0);
            }
        }
    }

    let mode_obj = obj_from_bits(mode_bits);
    if mode_obj.is_none() {
        return raise_exception::<_>(
            "TypeError",
            "open() argument 'mode' must be str, not NoneType",
        );
    }
    let mode = match string_obj_to_owned(mode_obj) {
        Some(mode) => mode,
        None => {
            let type_name = class_name_for_error(type_of_bits(mode_bits));
            let msg = format!("open() argument 'mode' must be str, not {type_name}");
            return raise_exception::<_>("TypeError", &msg);
        }
    };
    let mode_info = match parse_file_mode(&mode) {
        Ok(parsed) => parsed,
        Err(msg) => return raise_exception::<_>("ValueError", &msg),
    };
    if mode_info.readable && !has_capability("fs.read") {
        return raise_exception::<_>("PermissionError", "missing fs.read capability");
    }
    if mode_info.writable && !has_capability("fs.write") {
        return raise_exception::<_>("PermissionError", "missing fs.write capability");
    }

    let buffering = {
        let obj = obj_from_bits(buffering_bits);
        if obj.is_none() {
            return raise_exception::<_>(
                "TypeError",
                "'NoneType' object cannot be interpreted as an integer",
            );
        }
        let type_name = class_name_for_error(type_of_bits(buffering_bits));
        let msg = format!("'{type_name}' object cannot be interpreted as an integer");
        index_i64_from_obj(buffering_bits, &msg)
    };
    let buffering = if buffering < 0 { -1 } else { buffering };
    let line_buffering = buffering == 1 && mode_info.text;
    if buffering == 0 && mode_info.text {
        return raise_exception::<_>("ValueError", "can't have unbuffered text I/O");
    }

    let encoding = if mode_info.text {
        open_arg_encoding(encoding_bits)
    } else if !obj_from_bits(encoding_bits).is_none() {
        return raise_exception::<_>(
            "ValueError",
            "binary mode doesn't take an encoding argument",
        );
    } else {
        None
    };
    if exception_pending() {
        return MoltObject::none().bits();
    }
    let errors = if mode_info.text {
        open_arg_errors(errors_bits)
    } else if !obj_from_bits(errors_bits).is_none() {
        return raise_exception::<_>("ValueError", "binary mode doesn't take an errors argument");
    } else {
        None
    };
    if exception_pending() {
        return MoltObject::none().bits();
    }
    let newline = if mode_info.text {
        open_arg_newline(newline_bits)
    } else if !obj_from_bits(newline_bits).is_none() {
        return raise_exception::<_>("ValueError", "binary mode doesn't take a newline argument");
    } else {
        None
    };
    if exception_pending() {
        return MoltObject::none().bits();
    }

    let closefd = is_truthy(obj_from_bits(closefd_bits));
    let opener_obj = obj_from_bits(opener_bits);
    let opener_is_none = opener_obj.is_none();

    let mut path_guard = BitsGuard(0);
    let mut path = None;
    let mut fd: Option<i64> = None;
    let path_name_bits = if let Some(i) = to_i64(obj_from_bits(file_bits)) {
        fd = Some(i);
        let bits = MoltObject::from_int(i).bits();
        path_guard.0 = bits;
        bits
    } else {
        match open_arg_path(file_bits) {
            Ok((resolved, name_bits)) => {
                if !closefd {
                    return raise_exception::<_>(
                        "ValueError",
                        "Cannot use closefd=False with file name",
                    );
                }
                path = Some(resolved);
                path_guard.0 = name_bits;
                name_bits
            }
            Err(msg) => return raise_exception::<_>("TypeError", &msg),
        }
    };

    let mut file = None;
    if let Some(fd_val) = fd {
        if !opener_is_none {
            return raise_exception::<_>("ValueError", "opener only works with file path");
        }
        let effective_fd = if closefd {
            fd_val
        } else {
            match dup_fd(fd_val) {
                Some(val) => val,
                None => {
                    return raise_exception::<_>("OSError", "open failed");
                }
            }
        };
        if let Some(handle) = file_from_fd(effective_fd) {
            file = Some(handle);
        } else {
            return raise_exception::<_>("OSError", "open failed");
        }
    } else if let Some(path) = path {
        let flags = file_mode_to_flags(&mode_info);
        if !opener_is_none {
            if !is_truthy(obj_from_bits(molt_is_callable(opener_bits))) {
                let type_name = class_name_for_error(type_of_bits(opener_bits));
                let msg = format!("'{type_name}' object is not callable");
                return raise_exception::<_>("TypeError", &msg);
            }
            let path_bits = path_name_bits;
            let flags_bits = MoltObject::from_int(flags as i64).bits();
            let fd_bits = unsafe { call_callable2(opener_bits, path_bits, flags_bits) };
            if exception_pending() {
                return MoltObject::none().bits();
            }
            if let Some(fd_val) = to_i64(obj_from_bits(fd_bits)) {
                if let Some(handle) = file_from_fd(fd_val) {
                    file = Some(handle);
                } else {
                    return raise_exception::<_>("OSError", "open failed");
                }
            } else {
                let type_name = class_name_for_error(type_of_bits(fd_bits));
                let msg = format!("expected opener to return int, got {type_name}");
                return raise_exception::<_>("TypeError", &msg);
            }
            dec_ref_bits(fd_bits);
        } else {
            file = match mode_info.options.open(&path) {
                Ok(file) => Some(file),
                Err(err) => {
                    let short = match err.kind() {
                        ErrorKind::NotFound => "No such file or directory".to_string(),
                        ErrorKind::PermissionDenied => "Permission denied".to_string(),
                        ErrorKind::AlreadyExists => "File exists".to_string(),
                        ErrorKind::InvalidInput => "Invalid argument".to_string(),
                        ErrorKind::IsADirectory => "Is a directory".to_string(),
                        ErrorKind::NotADirectory => "Not a directory".to_string(),
                        _ => err.to_string(),
                    };
                    let path_display = path.to_string_lossy();
                    let msg = if let Some(code) = err.raw_os_error() {
                        format!("[Errno {code}] {short}: '{path_display}'")
                    } else {
                        format!("{short}: '{path_display}'")
                    };
                    match err.kind() {
                        ErrorKind::AlreadyExists => {
                            return raise_exception::<_>("FileExistsError", &msg)
                        }
                        ErrorKind::NotFound => {
                            return raise_exception::<_>("FileNotFoundError", &msg)
                        }
                        ErrorKind::PermissionDenied => {
                            return raise_exception::<_>("PermissionError", &msg)
                        }
                        ErrorKind::IsADirectory => {
                            return raise_exception::<_>("IsADirectoryError", &msg)
                        }
                        ErrorKind::NotADirectory => {
                            return raise_exception::<_>("NotADirectoryError", &msg)
                        }
                        _ => return raise_exception::<_>("OSError", &msg),
                    }
                }
            };
        }
    }
    let Some(file) = file else {
        return raise_exception::<_>("OSError", "open failed");
    };

    // TODO(stdlib-compat, owner:runtime, milestone:SL1): extend encoding support
    // beyond utf-8/ascii/latin-1 and expand error handlers for text I/O.
    let encoding = if mode_info.text {
        let encoding = encoding.unwrap_or_else(|| "utf-8".to_string());
        let (label, _kind) = match normalize_text_encoding(&encoding) {
            Ok(val) => val,
            Err(msg) => return raise_exception::<_>("LookupError", &msg),
        };
        Some(label)
    } else {
        None
    };
    let errors = if mode_info.text {
        Some(errors.unwrap_or_else(|| "strict".to_string()))
    } else {
        None
    };

    let state = Arc::new(MoltFileState {
        file: Mutex::new(Some(file)),
    });
    let builtins = builtin_classes();
    let buffered_class_bits = if mode_info.readable && mode_info.writable {
        builtins.buffered_random
    } else if mode_info.writable {
        builtins.buffered_writer
    } else {
        builtins.buffered_reader
    };
    let binary_class_bits = if buffering == 0 {
        builtins.file_io
    } else {
        buffered_class_bits
    };
    let handle_class_bits = if mode_info.text {
        builtins.text_io_wrapper
    } else {
        binary_class_bits
    };
    let buffer_class_bits = if mode_info.text {
        buffered_class_bits
    } else {
        0
    };
    let buffer_size = if buffering == 0 { 0 } else { buffering };
    let buffer_bits = if mode_info.text {
        let buffer_ptr = alloc_file_handle_with_state(
            Arc::clone(&state),
            mode_info.readable,
            mode_info.writable,
            false,
            false,
            true,
            false,
            false,
            buffer_size,
            buffer_class_bits,
            path_name_bits,
            mode.clone(),
            None,
            None,
            None,
            0,
        );
        if buffer_ptr.is_null() {
            return MoltObject::none().bits();
        }
        MoltObject::from_ptr(buffer_ptr).bits()
    } else {
        0
    };
    let ptr = alloc_file_handle_with_state(
        state,
        mode_info.readable,
        mode_info.writable,
        mode_info.text,
        closefd,
        true,
        line_buffering,
        false,
        buffer_size,
        handle_class_bits,
        path_name_bits,
        mode,
        encoding,
        errors,
        newline,
        buffer_bits,
    );
    if buffer_bits != 0 {
        dec_ref_bits(buffer_bits);
    }
    if ptr.is_null() {
        MoltObject::none().bits()
    } else {
        MoltObject::from_ptr(ptr).bits()
    }
}

#[no_mangle]
pub extern "C" fn molt_file_open(path_bits: u64, mode_bits: u64) -> u64 {
    let none = MoltObject::none().bits();
    open_impl(
        path_bits,
        mode_bits,
        MoltObject::from_int(-1).bits(),
        none,
        none,
        none,
        MoltObject::from_bool(true).bits(),
        none,
    )
}

#[no_mangle]
pub extern "C" fn molt_path_exists(path_bits: u64) -> u64 {
    if !has_capability("fs.read") {
        return raise_exception::<_>("PermissionError", "missing fs.read capability");
    }
    let path = match path_from_bits(path_bits) {
        Ok(path) => path,
        Err(msg) => return raise_exception::<_>("TypeError", &msg),
    };
    MoltObject::from_bool(std::fs::metadata(path).is_ok()).bits()
}

#[no_mangle]
pub extern "C" fn molt_path_unlink(path_bits: u64) -> u64 {
    if !has_capability("fs.write") {
        return raise_exception::<_>("PermissionError", "missing fs.write capability");
    }
    let path = match path_from_bits(path_bits) {
        Ok(path) => path,
        Err(msg) => return raise_exception::<_>("TypeError", &msg),
    };
    match std::fs::remove_file(&path) {
        Ok(()) => MoltObject::none().bits(),
        Err(err) => {
            let msg = err.to_string();
            match err.kind() {
                ErrorKind::NotFound => return raise_exception::<_>("FileNotFoundError", &msg),
                ErrorKind::PermissionDenied => {
                    return raise_exception::<_>("PermissionError", &msg)
                }
                ErrorKind::IsADirectory => return raise_exception::<_>("IsADirectoryError", &msg),
                _ => return raise_exception::<_>("OSError", &msg),
            }
        }
    }
}

#[no_mangle]
pub extern "C" fn molt_open_builtin(
    file_bits: u64,
    mode_bits: u64,
    buffering_bits: u64,
    encoding_bits: u64,
    errors_bits: u64,
    newline_bits: u64,
    closefd_bits: u64,
    opener_bits: u64,
) -> u64 {
    open_impl(
        file_bits,
        mode_bits,
        buffering_bits,
        encoding_bits,
        errors_bits,
        newline_bits,
        closefd_bits,
        opener_bits,
    )
}

#[derive(Debug)]
struct DecodeError {
    pos: usize,
    byte: u8,
    message: &'static str,
}

enum DecodeFailure {
    Byte {
        pos: usize,
        byte: u8,
        message: &'static str,
    },
    Range {
        start: usize,
        end: usize,
        message: &'static str,
    },
    UnknownErrorHandler(String),
}

#[derive(Clone, Copy, Debug)]
enum TextEncodingKind {
    Utf8,
    Ascii,
    Latin1,
}

struct TextEncodeError {
    pos: usize,
    ch: char,
    message: &'static str,
}

fn normalize_text_encoding(encoding: &str) -> Result<(String, TextEncodingKind), String> {
    let normalized = encoding.to_ascii_lowercase().replace('_', "-");
    match normalized.as_str() {
        "utf-8" | "utf8" => Ok(("utf-8".to_string(), TextEncodingKind::Utf8)),
        "ascii" => Ok(("ascii".to_string(), TextEncodingKind::Ascii)),
        "latin-1" | "latin1" | "iso-8859-1" | "iso8859-1" => {
            Ok(("latin-1".to_string(), TextEncodingKind::Latin1))
        }
        _ => Err(format!("unknown encoding: {encoding}")),
    }
}

fn text_encoding_kind(label: &str) -> TextEncodingKind {
    match label {
        "ascii" => TextEncodingKind::Ascii,
        "latin-1" => TextEncodingKind::Latin1,
        _ => TextEncodingKind::Utf8,
    }
}

fn validate_error_handler(errors: &str) -> Result<(), String> {
    if matches!(errors, "strict" | "ignore" | "replace") {
        Ok(())
    } else {
        Err(format!("unknown error handler name '{errors}'"))
    }
}

fn decode_utf8_with_errors(bytes: &[u8], errors: &str) -> Result<String, DecodeError> {
    match errors {
        "ignore" => {
            let mut out = String::new();
            let mut idx = 0usize;
            while idx < bytes.len() {
                match std::str::from_utf8(&bytes[idx..]) {
                    Ok(chunk) => {
                        out.push_str(chunk);
                        break;
                    }
                    Err(err) => {
                        let valid = err.valid_up_to();
                        if valid > 0 {
                            let chunk =
                                unsafe { std::str::from_utf8_unchecked(&bytes[idx..idx + valid]) };
                            out.push_str(chunk);
                            idx += valid;
                        }
                        let skip = err.error_len().unwrap_or(1);
                        idx = idx.saturating_add(skip);
                    }
                }
            }
            Ok(out)
        }
        "replace" => Ok(String::from_utf8_lossy(bytes).into_owned()),
        _ => match std::str::from_utf8(bytes) {
            Ok(text) => Ok(text.to_string()),
            Err(err) => {
                let pos = err.valid_up_to();
                let byte = bytes.get(pos).copied().unwrap_or(0);
                Err(DecodeError {
                    pos,
                    byte,
                    message: "invalid start byte",
                })
            }
        },
    }
}

fn decode_text_with_errors(
    bytes: &[u8],
    encoding: TextEncodingKind,
    errors: &str,
) -> Result<String, DecodeError> {
    match encoding {
        TextEncodingKind::Utf8 => decode_utf8_with_errors(bytes, errors),
        TextEncodingKind::Ascii => {
            let mut out = String::with_capacity(bytes.len());
            for (idx, &byte) in bytes.iter().enumerate() {
                if byte <= 0x7f {
                    out.push(byte as char);
                } else {
                    match errors {
                        "ignore" => {}
                        "replace" => out.push('\u{FFFD}'),
                        _ => {
                            return Err(DecodeError {
                                pos: idx,
                                byte,
                                message: "ordinal not in range(128)",
                            });
                        }
                    }
                }
            }
            Ok(out)
        }
        TextEncodingKind::Latin1 => {
            let mut out = String::with_capacity(bytes.len());
            for &byte in bytes {
                out.push(char::from(byte));
            }
            Ok(out)
        }
    }
}

fn encode_text_with_errors(
    text: &str,
    encoding: TextEncodingKind,
    errors: &str,
) -> Result<Vec<u8>, TextEncodeError> {
    match encoding {
        TextEncodingKind::Utf8 => Ok(text.as_bytes().to_vec()),
        TextEncodingKind::Ascii => {
            let mut out = Vec::with_capacity(text.len());
            for (idx, ch) in text.chars().enumerate() {
                let value = ch as u32;
                if value <= 0x7f {
                    out.push(value as u8);
                } else {
                    match errors {
                        "ignore" => {}
                        "replace" => out.push(b'?'),
                        _ => {
                            return Err(TextEncodeError {
                                pos: idx,
                                ch,
                                message: "ordinal not in range(128)",
                            });
                        }
                    }
                }
            }
            Ok(out)
        }
        TextEncodingKind::Latin1 => {
            let mut out = Vec::with_capacity(text.len());
            for (idx, ch) in text.chars().enumerate() {
                let value = ch as u32;
                if value <= 0xff {
                    out.push(value as u8);
                } else {
                    match errors {
                        "ignore" => {}
                        "replace" => out.push(b'?'),
                        _ => {
                            return Err(TextEncodeError {
                                pos: idx,
                                ch,
                                message: "ordinal not in range(256)",
                            });
                        }
                    }
                }
            }
            Ok(out)
        }
    }
}

const TEXT_COOKIE_SHIFT: u32 = 9;
const TEXT_COOKIE_PENDING_FLAG: u64 = 1 << 8;
const TEXT_COOKIE_BYTE_MASK: u64 = 0xff;

fn text_cookie_encode(pos: u64, pending: Option<u8>) -> Result<i64, String> {
    let mut value = pos
        .checked_shl(TEXT_COOKIE_SHIFT)
        .ok_or_else(|| "tell overflow".to_string())?;
    if let Some(byte) = pending {
        value |= TEXT_COOKIE_PENDING_FLAG | (byte as u64);
    }
    if value > i64::MAX as u64 {
        return Err("tell overflow".to_string());
    }
    Ok(value as i64)
}

fn text_cookie_decode(cookie: i64) -> Result<(u64, Option<u8>), String> {
    if cookie < 0 {
        return Err("negative seek position".to_string());
    }
    let raw = cookie as u64;
    let pending = if (raw & TEXT_COOKIE_PENDING_FLAG) != 0 {
        Some((raw & TEXT_COOKIE_BYTE_MASK) as u8)
    } else {
        None
    };
    let pos = raw >> TEXT_COOKIE_SHIFT;
    Ok((pos, pending))
}

fn translate_universal_newlines(bytes: &[u8]) -> Vec<u8> {
    let mut out = Vec::with_capacity(bytes.len());
    let mut idx = 0usize;
    while idx < bytes.len() {
        match bytes[idx] {
            b'\r' => {
                if idx + 1 < bytes.len() && bytes[idx + 1] == b'\n' {
                    idx += 2;
                } else {
                    idx += 1;
                }
                out.push(b'\n');
            }
            byte => {
                out.push(byte);
                idx += 1;
            }
        }
    }
    out
}

fn translate_write_newlines(text: &str, newline: Option<&str>) -> String {
    let target = match newline {
        None => {
            if cfg!(windows) {
                "\r\n"
            } else {
                "\n"
            }
        }
        Some("") | Some("\n") => "\n",
        Some(value) => value,
    };
    if target == "\n" {
        return text.to_string();
    }
    text.replace('\n', target)
}

fn file_handle_detached_message(handle: &MoltFileHandle) -> &'static str {
    if handle.text {
        "underlying buffer has been detached"
    } else {
        "raw stream has been detached"
    }
}

fn file_handle_is_closed(handle: &MoltFileHandle) -> bool {
    if handle.closed {
        return true;
    }
    handle.state.file.lock().unwrap().is_none()
}

#[no_mangle]
pub extern "C" fn molt_file_read(handle_bits: u64, size_bits: u64) -> u64 {
    let handle_obj = obj_from_bits(handle_bits);
    let Some(ptr) = handle_obj.as_ptr() else {
        return raise_exception::<_>("TypeError", "expected file handle");
    };
    unsafe {
        if object_type_id(ptr) != TYPE_ID_FILE_HANDLE {
            return raise_exception::<_>("TypeError", "expected file handle");
        }
        let handle_ptr = file_handle_ptr(ptr);
        if handle_ptr.is_null() {
            return raise_exception::<_>("RuntimeError", "file handle missing");
        }
        let handle = &mut *handle_ptr;
        file_handle_require_attached!(handle);
        if file_handle_is_closed(handle) {
            return raise_exception::<_>("ValueError", "I/O operation on closed file");
        }
        if !handle.readable {
            return raise_exception::<_>("UnsupportedOperation", "not readable");
        }
        let mut guard = handle.state.file.lock().unwrap();
        let Some(file) = guard.as_mut() else {
            return raise_exception::<_>("ValueError", "I/O operation on closed file");
        };
        let mut buf = Vec::new();
        let size_obj = obj_from_bits(size_bits);
        let size = if size_obj.is_none() {
            None
        } else {
            match to_i64(size_obj) {
                Some(val) if val < 0 => None,
                Some(val) => Some(val as usize),
                None => {
                    let type_name = class_name_for_error(type_of_bits(size_bits));
                    let msg = format!("argument should be integer or None, not '{type_name}'");
                    return raise_exception::<_>("TypeError", &msg);
                }
            }
        };
        let mut remaining = size;
        let mut at_eof = false;
        if let Some(pending) = handle.pending_byte.take() {
            if let Some(rem) = remaining {
                if rem == 0 {
                    handle.pending_byte = Some(pending);
                } else {
                    buf.push(pending);
                    remaining = Some(rem.saturating_sub(1));
                }
            } else {
                buf.push(pending);
            }
        }
        match remaining {
            Some(0) => {}
            Some(len) => {
                if len > 0 {
                    let start = buf.len();
                    buf.resize(start + len, 0);
                    let n = match file.read(&mut buf[start..]) {
                        Ok(n) => n,
                        Err(_) => return raise_exception::<_>("OSError", "read failed"),
                    };
                    buf.truncate(start + n);
                    if n < len {
                        at_eof = true;
                    }
                }
            }
            None => {
                if file.read_to_end(&mut buf).is_err() {
                    return raise_exception::<_>("OSError", "read failed");
                }
                at_eof = true;
            }
        }
        if handle.text {
            if handle.newline.is_none() && buf.last() == Some(&b'\r') && !at_eof {
                handle.pending_byte = Some(b'\r');
                buf.pop();
            }
            let bytes = if handle.newline.is_none() {
                translate_universal_newlines(&buf)
            } else {
                buf
            };
            let errors = handle.errors.as_deref().unwrap_or("strict");
            if let Err(msg) = validate_error_handler(errors) {
                return raise_exception::<_>("LookupError", &msg);
            }
            let encoding_label = handle.encoding.as_deref().unwrap_or("utf-8");
            let encoding = text_encoding_kind(encoding_label);
            let text = match decode_text_with_errors(&bytes, encoding, errors) {
                Ok(text) => text,
                Err(err) => {
                    let msg = format!(
                        "'{encoding_label}' codec can't decode byte 0x{:02x} in position {}: {}",
                        err.byte, err.pos, err.message
                    );
                    return raise_exception::<_>("UnicodeDecodeError", &msg);
                }
            };
            let out_ptr = alloc_string(text.as_bytes());
            if out_ptr.is_null() {
                MoltObject::none().bits()
            } else {
                MoltObject::from_ptr(out_ptr).bits()
            }
        } else {
            let out_ptr = alloc_bytes(&buf);
            if out_ptr.is_null() {
                MoltObject::none().bits()
            } else {
                MoltObject::from_ptr(out_ptr).bits()
            }
        }
    }
}

fn file_read_byte(
    pending_byte: &mut Option<u8>,
    file: &mut std::fs::File,
) -> std::io::Result<Option<u8>> {
    if let Some(pending) = pending_byte.take() {
        return Ok(Some(pending));
    }
    let mut buf = [0u8; 1];
    let read = file.read(&mut buf)?;
    if read == 0 {
        Ok(None)
    } else {
        Ok(Some(buf[0]))
    }
}

fn file_unread_byte(pending_byte: &mut Option<u8>, byte: u8) {
    *pending_byte = Some(byte);
}

fn file_readline_bytes(
    pending_byte: &mut Option<u8>,
    file: &mut std::fs::File,
    newline: Option<&str>,
    text: bool,
    size: Option<usize>,
) -> std::io::Result<Vec<u8>> {
    // TODO(stdlib-compat, owner:runtime, milestone:SL1): size limits should
    // count decoded chars for text I/O, not raw bytes.
    let mut out: Vec<u8> = Vec::new();
    loop {
        if let Some(limit) = size {
            if out.len() >= limit {
                break;
            }
        }
        let Some(byte) = file_read_byte(pending_byte, file)? else {
            break;
        };
        if text {
            match newline {
                None => {
                    if byte == b'\n' {
                        out.push(b'\n');
                        break;
                    }
                    if byte == b'\r' {
                        if let Some(next) = file_read_byte(pending_byte, file)? {
                            if next != b'\n' {
                                file_unread_byte(pending_byte, next);
                            }
                        }
                        out.push(b'\n');
                        break;
                    }
                    out.push(byte);
                }
                Some("") => {
                    if byte == b'\n' {
                        out.push(b'\n');
                        break;
                    }
                    if byte == b'\r' {
                        if let Some(next) = file_read_byte(pending_byte, file)? {
                            if next == b'\n' {
                                out.push(b'\r');
                                out.push(b'\n');
                                break;
                            }
                            file_unread_byte(pending_byte, next);
                        }
                        out.push(b'\r');
                        break;
                    }
                    out.push(byte);
                }
                Some("\n") => {
                    out.push(byte);
                    if byte == b'\n' {
                        break;
                    }
                }
                Some("\r") => {
                    out.push(byte);
                    if byte == b'\r' {
                        break;
                    }
                }
                Some("\r\n") => {
                    if byte == b'\r' {
                        if let Some(next) = file_read_byte(pending_byte, file)? {
                            if next == b'\n' {
                                out.push(b'\r');
                                out.push(b'\n');
                                break;
                            }
                            file_unread_byte(pending_byte, next);
                        }
                    }
                    out.push(byte);
                }
                Some(_) => {
                    out.push(byte);
                }
            }
        } else {
            out.push(byte);
            if byte == b'\n' {
                break;
            }
        }
    }
    Ok(out)
}

#[no_mangle]
pub extern "C" fn molt_file_readline(handle_bits: u64, size_bits: u64) -> u64 {
    let handle_obj = obj_from_bits(handle_bits);
    let Some(ptr) = handle_obj.as_ptr() else {
        return raise_exception::<_>("TypeError", "expected file handle");
    };
    unsafe {
        if object_type_id(ptr) != TYPE_ID_FILE_HANDLE {
            return raise_exception::<_>("TypeError", "expected file handle");
        }
        let handle_ptr = file_handle_ptr(ptr);
        if handle_ptr.is_null() {
            return raise_exception::<_>("RuntimeError", "file handle missing");
        }
        let handle = &mut *handle_ptr;
        file_handle_require_attached!(handle);
        if file_handle_is_closed(handle) {
            return raise_exception::<_>("ValueError", "I/O operation on closed file");
        }
        if !handle.readable {
            return raise_exception::<_>("UnsupportedOperation", "not readable");
        }
        let size_obj = obj_from_bits(size_bits);
        let size = if size_obj.is_none() {
            None
        } else {
            match to_i64(size_obj) {
                Some(val) if val < 0 => None,
                Some(val) => Some(val as usize),
                None => {
                    let type_name = class_name_for_error(type_of_bits(size_bits));
                    let msg = format!("'{type_name}' object cannot be interpreted as an integer");
                    return raise_exception::<_>("TypeError", &msg);
                }
            }
        };
        let text = handle.text;
        let newline_owned = if text {
            handle.newline.clone()
        } else {
            Some("\n".to_string())
        };
        let newline = newline_owned.as_deref();
        let mut pending_byte = handle.pending_byte.take();
        let mut guard = handle.state.file.lock().unwrap();
        let Some(file) = guard.as_mut() else {
            return raise_exception::<_>("ValueError", "I/O operation on closed file");
        };
        let bytes = match file_readline_bytes(&mut pending_byte, file, newline, text, size) {
            Ok(bytes) => bytes,
            Err(_) => {
                handle.pending_byte = pending_byte;
                return raise_exception::<_>("OSError", "read failed");
            }
        };
        handle.pending_byte = pending_byte;
        if text {
            let errors = handle.errors.as_deref().unwrap_or("strict");
            if let Err(msg) = validate_error_handler(errors) {
                return raise_exception::<_>("LookupError", &msg);
            }
            let encoding_label = handle.encoding.as_deref().unwrap_or("utf-8");
            let encoding = text_encoding_kind(encoding_label);
            let text = match decode_text_with_errors(&bytes, encoding, errors) {
                Ok(text) => text,
                Err(err) => {
                    let msg = format!(
                        "'{encoding_label}' codec can't decode byte 0x{:02x} in position {}: {}",
                        err.byte, err.pos, err.message
                    );
                    return raise_exception::<_>("UnicodeDecodeError", &msg);
                }
            };
            let out_ptr = alloc_string(text.as_bytes());
            if out_ptr.is_null() {
                MoltObject::none().bits()
            } else {
                MoltObject::from_ptr(out_ptr).bits()
            }
        } else {
            let out_ptr = alloc_bytes(&bytes);
            if out_ptr.is_null() {
                MoltObject::none().bits()
            } else {
                MoltObject::from_ptr(out_ptr).bits()
            }
        }
    }
}

#[no_mangle]
pub extern "C" fn molt_file_readlines(handle_bits: u64, hint_bits: u64) -> u64 {
    let handle_obj = obj_from_bits(handle_bits);
    let Some(ptr) = handle_obj.as_ptr() else {
        return raise_exception::<_>("TypeError", "expected file handle");
    };
    unsafe {
        if object_type_id(ptr) != TYPE_ID_FILE_HANDLE {
            return raise_exception::<_>("TypeError", "expected file handle");
        }
        let handle_ptr = file_handle_ptr(ptr);
        if handle_ptr.is_null() {
            return raise_exception::<_>("RuntimeError", "file handle missing");
        }
        let handle = &mut *handle_ptr;
        file_handle_require_attached!(handle);
        if file_handle_is_closed(handle) {
            return raise_exception::<_>("ValueError", "I/O operation on closed file");
        }
        if !handle.readable {
            return raise_exception::<_>("UnsupportedOperation", "not readable");
        }
        let hint_obj = obj_from_bits(hint_bits);
        let hint = if hint_obj.is_none() {
            None
        } else {
            match to_i64(hint_obj) {
                Some(val) if val <= 0 => None,
                Some(val) => Some(val as usize),
                None => {
                    let type_name = class_name_for_error(type_of_bits(hint_bits));
                    let msg = format!("argument should be integer or None, not '{type_name}'");
                    return raise_exception::<_>("TypeError", &msg);
                }
            }
        };
        let text = handle.text;
        let newline_owned = if text {
            handle.newline.clone()
        } else {
            Some("\n".to_string())
        };
        let newline = newline_owned.as_deref();
        let mut pending_byte = handle.pending_byte.take();
        let mut guard = handle.state.file.lock().unwrap();
        let Some(file) = guard.as_mut() else {
            return raise_exception::<_>("ValueError", "I/O operation on closed file");
        };
        let mut lines: Vec<u64> = Vec::new();
        let mut total = 0usize;
        loop {
            let bytes = match file_readline_bytes(&mut pending_byte, file, newline, text, None) {
                Ok(bytes) => bytes,
                Err(_) => {
                    handle.pending_byte = pending_byte;
                    return raise_exception::<_>("OSError", "read failed");
                }
            };
            if bytes.is_empty() {
                break;
            }
            total = total.saturating_add(bytes.len());
            if text {
                let errors = handle.errors.as_deref().unwrap_or("strict");
                if let Err(msg) = validate_error_handler(errors) {
                    return raise_exception::<_>("LookupError", &msg);
                }
                let encoding_label = handle.encoding.as_deref().unwrap_or("utf-8");
                let encoding = text_encoding_kind(encoding_label);
                let text = match decode_text_with_errors(&bytes, encoding, errors) {
                    Ok(text) => text,
                    Err(err) => {
                        let msg = format!(
                            "'{encoding_label}' codec can't decode byte 0x{:02x} in position {}: {}",
                            err.byte, err.pos, err.message
                        );
                        return raise_exception::<_>("UnicodeDecodeError", &msg);
                    }
                };
                let line_ptr = alloc_string(text.as_bytes());
                if line_ptr.is_null() {
                    return MoltObject::none().bits();
                }
                lines.push(MoltObject::from_ptr(line_ptr).bits());
            } else {
                let line_ptr = alloc_bytes(&bytes);
                if line_ptr.is_null() {
                    return MoltObject::none().bits();
                }
                lines.push(MoltObject::from_ptr(line_ptr).bits());
            }
            if let Some(limit) = hint {
                if total >= limit {
                    break;
                }
            }
        }
        handle.pending_byte = pending_byte;
        let list_ptr = alloc_list(lines.as_slice());
        if list_ptr.is_null() {
            for bits in lines {
                dec_ref_bits(bits);
            }
            return MoltObject::none().bits();
        }
        for bits in lines {
            dec_ref_bits(bits);
        }
        MoltObject::from_ptr(list_ptr).bits()
    }
}

#[no_mangle]
pub extern "C" fn molt_file_readinto(handle_bits: u64, buffer_bits: u64) -> u64 {
    let handle_obj = obj_from_bits(handle_bits);
    let Some(ptr) = handle_obj.as_ptr() else {
        return raise_exception::<_>("TypeError", "expected file handle");
    };
    unsafe {
        if object_type_id(ptr) != TYPE_ID_FILE_HANDLE {
            return raise_exception::<_>("TypeError", "expected file handle");
        }
        let handle_ptr = file_handle_ptr(ptr);
        if handle_ptr.is_null() {
            return raise_exception::<_>("RuntimeError", "file handle missing");
        }
        let handle = &mut *handle_ptr;
        file_handle_require_attached!(handle);
        if file_handle_is_closed(handle) {
            return raise_exception::<_>("ValueError", "I/O operation on closed file");
        }
        if !handle.readable {
            return raise_exception::<_>("UnsupportedOperation", "read");
        }
        if handle.text {
            return raise_exception::<_>("OSError", "readinto() unsupported for text files");
        }
        let mut export = BufferExport {
            ptr: 0,
            len: 0,
            readonly: 0,
            stride: 0,
            itemsize: 0,
        };
        if molt_buffer_export(buffer_bits, &mut export) != 0 || export.readonly != 0 {
            return raise_exception::<_>(
                "TypeError",
                "readinto() argument must be a writable bytes-like object",
            );
        }
        if export.itemsize != 1 || export.stride != 1 {
            return raise_exception::<_>(
                "TypeError",
                "readinto() argument must be a writable bytes-like object",
            );
        }
        let len = export.len as usize;
        if len == 0 {
            return MoltObject::from_int(0).bits();
        }
        let buf = std::slice::from_raw_parts_mut(export.ptr as *mut u8, len);
        let mut guard = handle.state.file.lock().unwrap();
        let Some(file) = guard.as_mut() else {
            return raise_exception::<_>("ValueError", "I/O operation on closed file");
        };
        let n = match file.read(buf) {
            Ok(n) => n,
            Err(_) => return raise_exception::<_>("OSError", "read failed"),
        };
        MoltObject::from_int(n as i64).bits()
    }
}

#[no_mangle]
pub extern "C" fn molt_file_detach(handle_bits: u64) -> u64 {
    let handle_obj = obj_from_bits(handle_bits);
    let Some(ptr) = handle_obj.as_ptr() else {
        return raise_exception::<_>("TypeError", "expected file handle");
    };
    unsafe {
        if object_type_id(ptr) != TYPE_ID_FILE_HANDLE {
            return raise_exception::<_>("TypeError", "expected file handle");
        }
        let handle_ptr = file_handle_ptr(ptr);
        if handle_ptr.is_null() {
            return raise_exception::<_>("RuntimeError", "file handle missing");
        }
        let handle = &mut *handle_ptr;
        if handle.detached {
            return raise_exception::<_>("ValueError", file_handle_detached_message(handle));
        }
        if file_handle_is_closed(handle) {
            return raise_exception::<_>("ValueError", "I/O operation on closed file");
        }
        if handle.text {
            let buffer_bits = handle.buffer_bits;
            if buffer_bits == 0 {
                return raise_exception::<_>("ValueError", file_handle_detached_message(handle));
            }
            let buffer_obj = obj_from_bits(buffer_bits);
            if let Some(buffer_ptr) = buffer_obj.as_ptr() {
                if object_type_id(buffer_ptr) == TYPE_ID_FILE_HANDLE {
                    let buffer_handle_ptr = file_handle_ptr(buffer_ptr);
                    if !buffer_handle_ptr.is_null() {
                        let buffer_handle = &mut *buffer_handle_ptr;
                        buffer_handle.pending_byte = handle.pending_byte.take();
                    }
                }
            }
            handle.buffer_bits = MoltObject::none().bits();
            handle.detached = true;
            handle.owns_fd = false;
            return buffer_bits;
        }
        let raw_ptr = alloc_file_handle_with_state(
            Arc::clone(&handle.state),
            handle.readable,
            handle.writable,
            false,
            handle.closefd,
            handle.owns_fd,
            handle.line_buffering,
            handle.write_through,
            handle.buffer_size,
            handle.class_bits,
            handle.name_bits,
            handle.mode.clone(),
            None,
            None,
            None,
            0,
        );
        if raw_ptr.is_null() {
            return MoltObject::none().bits();
        }
        let raw_handle_ptr = file_handle_ptr(raw_ptr);
        if !raw_handle_ptr.is_null() {
            let raw_handle = &mut *raw_handle_ptr;
            raw_handle.pending_byte = handle.pending_byte.take();
        }
        handle.detached = true;
        handle.owns_fd = false;
        MoltObject::from_ptr(raw_ptr).bits()
    }
}

#[no_mangle]
pub extern "C" fn molt_file_reconfigure(
    handle_bits: u64,
    encoding_bits: u64,
    errors_bits: u64,
    newline_bits: u64,
    line_buffering_bits: u64,
    write_through_bits: u64,
) -> u64 {
    let handle_obj = obj_from_bits(handle_bits);
    let Some(ptr) = handle_obj.as_ptr() else {
        return raise_exception::<_>("TypeError", "expected file handle");
    };
    unsafe {
        if object_type_id(ptr) != TYPE_ID_FILE_HANDLE {
            return raise_exception::<_>("TypeError", "expected file handle");
        }
        let handle_ptr = file_handle_ptr(ptr);
        if handle_ptr.is_null() {
            return raise_exception::<_>("RuntimeError", "file handle missing");
        }
        let handle = &mut *handle_ptr;
        file_handle_require_attached!(handle);
        if file_handle_is_closed(handle) {
            return raise_exception::<_>("ValueError", "I/O operation on closed file");
        }
        if !handle.text {
            return raise_exception::<_>("UnsupportedOperation", "not a text file");
        }
        let mut guard = handle.state.file.lock().unwrap();
        let Some(file) = guard.as_mut() else {
            return raise_exception::<_>("ValueError", "I/O operation on closed file");
        };
        if file.flush().is_err() {
            return raise_exception::<_>("OSError", "flush failed");
        }
        drop(guard);

        let missing = missing_bits();
        let mut new_encoding = handle.encoding.clone();
        if encoding_bits != missing {
            if let Some(encoding) = reconfigure_arg_type(encoding_bits, "encoding") {
                let (label, _kind) = match normalize_text_encoding(&encoding) {
                    Ok(val) => val,
                    Err(msg) => return raise_exception::<_>("LookupError", &msg),
                };
                new_encoding = Some(label);
            }
        }
        let mut new_errors = handle.errors.clone();
        if errors_bits != missing {
            if let Some(errors) = reconfigure_arg_type(errors_bits, "errors") {
                new_errors = Some(errors);
            }
        }
        let mut new_newline = handle.newline.clone();
        if newline_bits != missing {
            new_newline = reconfigure_arg_newline(newline_bits);
        }
        let mut new_line_buffering = handle.line_buffering;
        if line_buffering_bits != missing {
            let obj = obj_from_bits(line_buffering_bits);
            if !obj.is_none() {
                let val = match to_i64(obj) {
                    Some(val) => val != 0,
                    None => {
                        let type_name = class_name_for_error(type_of_bits(line_buffering_bits));
                        let msg =
                            format!("'{type_name}' object cannot be interpreted as an integer");
                        return raise_exception::<_>("TypeError", &msg);
                    }
                };
                new_line_buffering = val;
            }
        }
        let mut new_write_through = handle.write_through;
        if write_through_bits != missing {
            let obj = obj_from_bits(write_through_bits);
            if !obj.is_none() {
                let val = match to_i64(obj) {
                    Some(val) => val != 0,
                    None => {
                        let type_name = class_name_for_error(type_of_bits(write_through_bits));
                        let msg =
                            format!("'{type_name}' object cannot be interpreted as an integer");
                        return raise_exception::<_>("TypeError", &msg);
                    }
                };
                new_write_through = val;
            }
        }

        handle.encoding = new_encoding;
        handle.errors = new_errors;
        if newline_bits != missing {
            handle.pending_byte = None;
        }
        handle.newline = new_newline;
        handle.line_buffering = new_line_buffering;
        handle.write_through = new_write_through;
        MoltObject::none().bits()
    }
}

#[no_mangle]
pub extern "C" fn molt_file_seek(handle_bits: u64, offset_bits: u64, whence_bits: u64) -> u64 {
    let handle_obj = obj_from_bits(handle_bits);
    let Some(ptr) = handle_obj.as_ptr() else {
        return raise_exception::<_>("TypeError", "expected file handle");
    };
    unsafe {
        if object_type_id(ptr) != TYPE_ID_FILE_HANDLE {
            return raise_exception::<_>("TypeError", "expected file handle");
        }
        let handle_ptr = file_handle_ptr(ptr);
        if handle_ptr.is_null() {
            return raise_exception::<_>("RuntimeError", "file handle missing");
        }
        let handle = &mut *handle_ptr;
        file_handle_require_attached!(handle);
        if file_handle_is_closed(handle) {
            return raise_exception::<_>("ValueError", "I/O operation on closed file");
        }
        let offset = match to_i64(obj_from_bits(offset_bits)) {
            Some(val) => val,
            None => {
                let type_name = class_name_for_error(type_of_bits(offset_bits));
                let msg = format!("'{type_name}' object cannot be interpreted as an integer");
                return raise_exception::<_>("TypeError", &msg);
            }
        };
        let whence = match to_i64(obj_from_bits(whence_bits)) {
            Some(val) => val,
            None => {
                let type_name = class_name_for_error(type_of_bits(whence_bits));
                let msg = format!("'{type_name}' object cannot be interpreted as an integer");
                return raise_exception::<_>("TypeError", &msg);
            }
        };
        let mut guard = handle.state.file.lock().unwrap();
        let Some(file) = guard.as_mut() else {
            return raise_exception::<_>("ValueError", "I/O operation on closed file");
        };
        if handle.text && whence == 0 {
            let (pos, pending) = match text_cookie_decode(offset) {
                Ok(val) => val,
                Err(msg) => return raise_exception::<_>("ValueError", &msg),
            };
            let pos = match file.seek(std::io::SeekFrom::Start(pos)) {
                Ok(pos) => pos,
                Err(_) => return raise_exception::<_>("OSError", "seek failed"),
            };
            handle.pending_byte = pending;
            let cookie = match text_cookie_encode(pos, pending) {
                Ok(val) => val,
                Err(msg) => return raise_exception::<_>("OSError", &msg),
            };
            return MoltObject::from_int(cookie).bits();
        }
        let from = match whence {
            0 => {
                if offset < 0 {
                    let msg = format!("negative seek position {offset}");
                    return raise_exception::<_>("ValueError", &msg);
                }
                std::io::SeekFrom::Start(offset as u64)
            }
            1 => std::io::SeekFrom::Current(offset),
            2 => std::io::SeekFrom::End(offset),
            _ => return raise_exception::<_>("ValueError", "invalid whence"),
        };
        let pos = match file.seek(from) {
            Ok(pos) => pos,
            Err(_) => return raise_exception::<_>("OSError", "seek failed"),
        };
        handle.pending_byte = None;
        if handle.text {
            let cookie = match text_cookie_encode(pos, None) {
                Ok(val) => val,
                Err(msg) => return raise_exception::<_>("OSError", &msg),
            };
            MoltObject::from_int(cookie).bits()
        } else {
            MoltObject::from_int(pos as i64).bits()
        }
    }
}

#[no_mangle]
pub extern "C" fn molt_file_tell(handle_bits: u64) -> u64 {
    let handle_obj = obj_from_bits(handle_bits);
    let Some(ptr) = handle_obj.as_ptr() else {
        return raise_exception::<_>("TypeError", "expected file handle");
    };
    unsafe {
        if object_type_id(ptr) != TYPE_ID_FILE_HANDLE {
            return raise_exception::<_>("TypeError", "expected file handle");
        }
        let handle_ptr = file_handle_ptr(ptr);
        if handle_ptr.is_null() {
            return raise_exception::<_>("RuntimeError", "file handle missing");
        }
        let handle = &*handle_ptr;
        file_handle_require_attached!(handle);
        if file_handle_is_closed(handle) {
            return raise_exception::<_>("ValueError", "I/O operation on closed file");
        }
        let mut guard = handle.state.file.lock().unwrap();
        let Some(file) = guard.as_mut() else {
            return raise_exception::<_>("ValueError", "I/O operation on closed file");
        };
        let pos = match file.stream_position() {
            Ok(pos) => pos,
            Err(_) => return raise_exception::<_>("OSError", "tell failed"),
        };
        if handle.text {
            let cookie = match text_cookie_encode(pos, handle.pending_byte) {
                Ok(val) => val,
                Err(msg) => return raise_exception::<_>("OSError", &msg),
            };
            MoltObject::from_int(cookie).bits()
        } else {
            MoltObject::from_int(pos as i64).bits()
        }
    }
}

#[no_mangle]
pub extern "C" fn molt_file_fileno(handle_bits: u64) -> u64 {
    let handle_obj = obj_from_bits(handle_bits);
    let Some(ptr) = handle_obj.as_ptr() else {
        return raise_exception::<_>("TypeError", "expected file handle");
    };
    unsafe {
        if object_type_id(ptr) != TYPE_ID_FILE_HANDLE {
            return raise_exception::<_>("TypeError", "expected file handle");
        }
        let handle_ptr = file_handle_ptr(ptr);
        if handle_ptr.is_null() {
            return raise_exception::<_>("RuntimeError", "file handle missing");
        }
        let handle = &*handle_ptr;
        file_handle_require_attached!(handle);
        if file_handle_is_closed(handle) {
            return raise_exception::<_>("ValueError", "I/O operation on closed file");
        }
        let guard = handle.state.file.lock().unwrap();
        let Some(file) = guard.as_ref() else {
            return raise_exception::<_>("ValueError", "I/O operation on closed file");
        };
        #[cfg(unix)]
        {
            use std::os::fd::AsRawFd;
            MoltObject::from_int(file.as_raw_fd() as i64).bits()
        }
        #[cfg(windows)]
        {
            // TODO(stdlib-compat, owner:runtime, milestone:SL1): return CRT fd on
            // Windows instead of raw handle for fileno parity.
            use std::os::windows::io::AsRawHandle;
            MoltObject::from_int(file.as_raw_handle() as i64).bits()
        }
        #[cfg(not(any(unix, windows)))]
        {
            return raise_exception::<_>("OSError", "fileno is unsupported on this platform");
        }
    }
}

#[no_mangle]
pub extern "C" fn molt_file_truncate(handle_bits: u64, size_bits: u64) -> u64 {
    let handle_obj = obj_from_bits(handle_bits);
    let Some(ptr) = handle_obj.as_ptr() else {
        return raise_exception::<_>("TypeError", "expected file handle");
    };
    unsafe {
        if object_type_id(ptr) != TYPE_ID_FILE_HANDLE {
            return raise_exception::<_>("TypeError", "expected file handle");
        }
        let handle_ptr = file_handle_ptr(ptr);
        if handle_ptr.is_null() {
            return raise_exception::<_>("RuntimeError", "file handle missing");
        }
        let handle = &*handle_ptr;
        file_handle_require_attached!(handle);
        if file_handle_is_closed(handle) {
            return raise_exception::<_>("ValueError", "I/O operation on closed file");
        }
        if !handle.writable {
            return raise_exception::<_>("UnsupportedOperation", "truncate");
        }
        let mut guard = handle.state.file.lock().unwrap();
        let Some(file) = guard.as_mut() else {
            return raise_exception::<_>("ValueError", "I/O operation on closed file");
        };
        let size = if obj_from_bits(size_bits).is_none() {
            match file.stream_position() {
                Ok(pos) => pos,
                Err(_) => return raise_exception::<_>("OSError", "tell failed"),
            }
        } else {
            let val = match to_i64(obj_from_bits(size_bits)) {
                Some(val) => val,
                None => {
                    let type_name = class_name_for_error(type_of_bits(size_bits));
                    let msg = format!("'{type_name}' object cannot be interpreted as an integer");
                    return raise_exception::<_>("TypeError", &msg);
                }
            };
            if val < 0 {
                return raise_exception::<_>("OSError", "Invalid argument");
            }
            val as u64
        };
        if file.set_len(size).is_err() {
            return raise_exception::<_>("OSError", "truncate failed");
        }
        MoltObject::from_int(size as i64).bits()
    }
}

#[no_mangle]
pub extern "C" fn molt_file_readable(handle_bits: u64) -> u64 {
    let handle_obj = obj_from_bits(handle_bits);
    let Some(ptr) = handle_obj.as_ptr() else {
        return raise_exception::<_>("TypeError", "expected file handle");
    };
    unsafe {
        if object_type_id(ptr) != TYPE_ID_FILE_HANDLE {
            return raise_exception::<_>("TypeError", "expected file handle");
        }
        let handle_ptr = file_handle_ptr(ptr);
        if handle_ptr.is_null() {
            return raise_exception::<_>("RuntimeError", "file handle missing");
        }
        let handle = &*handle_ptr;
        file_handle_require_attached!(handle);
        if file_handle_is_closed(handle) {
            return raise_exception::<_>("ValueError", "I/O operation on closed file");
        }
        MoltObject::from_bool(handle.readable).bits()
    }
}

#[no_mangle]
pub extern "C" fn molt_file_writable(handle_bits: u64) -> u64 {
    let handle_obj = obj_from_bits(handle_bits);
    let Some(ptr) = handle_obj.as_ptr() else {
        return raise_exception::<_>("TypeError", "expected file handle");
    };
    unsafe {
        if object_type_id(ptr) != TYPE_ID_FILE_HANDLE {
            return raise_exception::<_>("TypeError", "expected file handle");
        }
        let handle_ptr = file_handle_ptr(ptr);
        if handle_ptr.is_null() {
            return raise_exception::<_>("RuntimeError", "file handle missing");
        }
        let handle = &*handle_ptr;
        file_handle_require_attached!(handle);
        if file_handle_is_closed(handle) {
            return raise_exception::<_>("ValueError", "I/O operation on closed file");
        }
        MoltObject::from_bool(handle.writable).bits()
    }
}

#[no_mangle]
pub extern "C" fn molt_file_seekable(handle_bits: u64) -> u64 {
    let handle_obj = obj_from_bits(handle_bits);
    let Some(ptr) = handle_obj.as_ptr() else {
        return raise_exception::<_>("TypeError", "expected file handle");
    };
    unsafe {
        if object_type_id(ptr) != TYPE_ID_FILE_HANDLE {
            return raise_exception::<_>("TypeError", "expected file handle");
        }
        let handle_ptr = file_handle_ptr(ptr);
        if handle_ptr.is_null() {
            return raise_exception::<_>("RuntimeError", "file handle missing");
        }
        let handle = &*handle_ptr;
        file_handle_require_attached!(handle);
        if file_handle_is_closed(handle) {
            return raise_exception::<_>("ValueError", "I/O operation on closed file");
        }
        let mut guard = handle.state.file.lock().unwrap();
        let Some(file) = guard.as_mut() else {
            return raise_exception::<_>("ValueError", "I/O operation on closed file");
        };
        let seekable = file.stream_position().is_ok();
        MoltObject::from_bool(seekable).bits()
    }
}

#[no_mangle]
pub extern "C" fn molt_file_isatty(handle_bits: u64) -> u64 {
    let handle_obj = obj_from_bits(handle_bits);
    let Some(ptr) = handle_obj.as_ptr() else {
        return raise_exception::<_>("TypeError", "expected file handle");
    };
    unsafe {
        if object_type_id(ptr) != TYPE_ID_FILE_HANDLE {
            return raise_exception::<_>("TypeError", "expected file handle");
        }
        let handle_ptr = file_handle_ptr(ptr);
        if handle_ptr.is_null() {
            return raise_exception::<_>("RuntimeError", "file handle missing");
        }
        let handle = &*handle_ptr;
        file_handle_require_attached!(handle);
        if file_handle_is_closed(handle) {
            return raise_exception::<_>("ValueError", "I/O operation on closed file");
        }
        let guard = handle.state.file.lock().unwrap();
        let Some(file) = guard.as_ref() else {
            return raise_exception::<_>("ValueError", "I/O operation on closed file");
        };
        #[cfg(unix)]
        {
            use std::os::fd::AsRawFd;
            let isatty = libc::isatty(file.as_raw_fd()) == 1;
            MoltObject::from_bool(isatty).bits()
        }
        #[cfg(windows)]
        {
            // TODO(stdlib-compat, owner:runtime, milestone:SL1): map Windows console
            // handles to CRT fds (or call GetFileType) for accurate isatty.
            let _ = file;
            MoltObject::from_bool(false).bits()
        }
        #[cfg(not(any(unix, windows)))]
        {
            let _ = file;
            MoltObject::from_bool(false).bits()
        }
    }
}

#[no_mangle]
pub extern "C" fn molt_file_iter(handle_bits: u64) -> u64 {
    let handle_obj = obj_from_bits(handle_bits);
    let Some(ptr) = handle_obj.as_ptr() else {
        return raise_exception::<_>("TypeError", "expected file handle");
    };
    unsafe {
        if object_type_id(ptr) != TYPE_ID_FILE_HANDLE {
            return raise_exception::<_>("TypeError", "expected file handle");
        }
        let handle_ptr = file_handle_ptr(ptr);
        if handle_ptr.is_null() {
            return raise_exception::<_>("RuntimeError", "file handle missing");
        }
        let handle = &*handle_ptr;
        file_handle_require_attached!(handle);
        if file_handle_is_closed(handle) {
            return raise_exception::<_>("ValueError", "I/O operation on closed file");
        }
    }
    inc_ref_bits(handle_bits);
    handle_bits
}

#[no_mangle]
pub extern "C" fn molt_file_next(handle_bits: u64) -> u64 {
    let line_bits = molt_file_readline(handle_bits, MoltObject::from_int(-1).bits());
    if exception_pending() {
        return MoltObject::none().bits();
    }
    let line_obj = obj_from_bits(line_bits);
    let empty = if let Some(ptr) = line_obj.as_ptr() {
        unsafe {
            match object_type_id(ptr) {
                TYPE_ID_STRING => string_len(ptr) == 0,
                TYPE_ID_BYTES | TYPE_ID_BYTEARRAY => bytes_len(ptr) == 0,
                _ => false,
            }
        }
    } else {
        false
    };
    if empty {
        dec_ref_bits(line_bits);
        return raise_exception::<_>("StopIteration", "");
    }
    line_bits
}

#[no_mangle]
pub extern "C" fn molt_file_enter(handle_bits: u64) -> u64 {
    let handle_obj = obj_from_bits(handle_bits);
    let Some(ptr) = handle_obj.as_ptr() else {
        return raise_exception::<_>("TypeError", "expected file handle");
    };
    unsafe {
        if object_type_id(ptr) != TYPE_ID_FILE_HANDLE {
            return raise_exception::<_>("TypeError", "expected file handle");
        }
        file_handle_enter(ptr)
    }
}

#[no_mangle]
pub extern "C" fn molt_file_exit(handle_bits: u64, exc_bits: u64) -> u64 {
    let handle_obj = obj_from_bits(handle_bits);
    let Some(ptr) = handle_obj.as_ptr() else {
        return raise_exception::<_>("TypeError", "expected file handle");
    };
    unsafe {
        if object_type_id(ptr) != TYPE_ID_FILE_HANDLE {
            return raise_exception::<_>("TypeError", "expected file handle");
        }
        file_handle_exit(ptr, exc_bits)
    }
}

#[no_mangle]
pub extern "C" fn molt_file_exit_method(
    handle_bits: u64,
    _exc_type_bits: u64,
    exc_bits: u64,
    _tb_bits: u64,
) -> u64 {
    molt_file_exit(handle_bits, exc_bits)
}

#[no_mangle]
pub extern "C" fn molt_file_write(handle_bits: u64, data_bits: u64) -> u64 {
    let handle_obj = obj_from_bits(handle_bits);
    let Some(ptr) = handle_obj.as_ptr() else {
        return raise_exception::<_>("TypeError", "expected file handle");
    };
    unsafe {
        if object_type_id(ptr) != TYPE_ID_FILE_HANDLE {
            return raise_exception::<_>("TypeError", "expected file handle");
        }
        let handle_ptr = file_handle_ptr(ptr);
        if handle_ptr.is_null() {
            return raise_exception::<_>("RuntimeError", "file handle missing");
        }
        let handle = &mut *handle_ptr;
        file_handle_require_attached!(handle);
        if file_handle_is_closed(handle) {
            return raise_exception::<_>("ValueError", "I/O operation on closed file");
        }
        if !handle.writable {
            return raise_exception::<_>("UnsupportedOperation", "not writable");
        }
        let mut guard = handle.state.file.lock().unwrap();
        let Some(file) = guard.as_mut() else {
            return raise_exception::<_>("ValueError", "I/O operation on closed file");
        };
        let data_obj = obj_from_bits(data_bits);
        let (bytes, written_len): (Vec<u8>, usize) = if handle.text {
            let text = match string_obj_to_owned(data_obj) {
                Some(text) => text,
                None => {
                    return raise_exception::<_>("TypeError", "write expects str for text mode")
                }
            };
            let errors = handle.errors.as_deref().unwrap_or("strict");
            let newline = handle.newline.as_deref();
            if let Err(msg) = validate_error_handler(errors) {
                return raise_exception::<_>("LookupError", &msg);
            }
            let translated = translate_write_newlines(&text, newline);
            let encoding_label = handle.encoding.as_deref().unwrap_or("utf-8");
            let encoding = text_encoding_kind(encoding_label);
            let bytes = match encode_text_with_errors(&translated, encoding, errors) {
                Ok(bytes) => bytes,
                Err(err) => {
                    let msg = format!(
                        "'{encoding_label}' codec can't encode character '{}' in position {}: {}",
                        err.ch, err.pos, err.message
                    );
                    return raise_exception::<_>("UnicodeEncodeError", &msg);
                }
            };
            (bytes, text.chars().count())
        } else {
            let Some(data_ptr) = data_obj.as_ptr() else {
                return raise_exception::<_>("TypeError", "write expects bytes or bytearray");
            };
            let type_id = object_type_id(data_ptr);
            if type_id != TYPE_ID_BYTES && type_id != TYPE_ID_BYTEARRAY {
                return raise_exception::<_>("TypeError", "write expects bytes or bytearray");
            }
            let len = bytes_len(data_ptr);
            let raw = std::slice::from_raw_parts(bytes_data(data_ptr), len);
            (raw.to_vec(), len)
        };
        if file.write_all(&bytes).is_err() {
            return raise_exception::<_>("OSError", "write failed");
        }
        let should_flush =
            handle.write_through || (handle.line_buffering && bytes.contains(&b'\n'));
        if should_flush && file.flush().is_err() {
            return raise_exception::<_>("OSError", "flush failed");
        }
        MoltObject::from_int(written_len as i64).bits()
    }
}

#[no_mangle]
pub extern "C" fn molt_file_writelines(handle_bits: u64, lines_bits: u64) -> u64 {
    let handle_obj = obj_from_bits(handle_bits);
    let Some(ptr) = handle_obj.as_ptr() else {
        return raise_exception::<_>("TypeError", "expected file handle");
    };
    unsafe {
        if object_type_id(ptr) != TYPE_ID_FILE_HANDLE {
            return raise_exception::<_>("TypeError", "expected file handle");
        }
        let handle_ptr = file_handle_ptr(ptr);
        if handle_ptr.is_null() {
            return raise_exception::<_>("RuntimeError", "file handle missing");
        }
        let handle = &mut *handle_ptr;
        if file_handle_is_closed(handle) {
            return raise_exception::<_>("ValueError", "I/O operation on closed file");
        }
        if !handle.writable {
            return raise_exception::<_>("UnsupportedOperation", "not writable");
        }
    }
    let iter_bits = molt_iter(lines_bits);
    if obj_from_bits(iter_bits).is_none() {
        return raise_exception::<_>("TypeError", "writelines() argument must be iterable");
    }
    loop {
        let pair_bits = molt_iter_next(iter_bits);
        if exception_pending() {
            return MoltObject::none().bits();
        }
        let pair_obj = obj_from_bits(pair_bits);
        let Some(pair_ptr) = pair_obj.as_ptr() else {
            return MoltObject::none().bits();
        };
        unsafe {
            if object_type_id(pair_ptr) != TYPE_ID_TUPLE {
                return MoltObject::none().bits();
            }
            let elems = seq_vec_ref(pair_ptr);
            if elems.len() < 2 {
                return MoltObject::none().bits();
            }
            let done_bits = elems[1];
            if is_truthy(obj_from_bits(done_bits)) {
                break;
            }
            let line_bits = elems[0];
            let _ = molt_file_write(handle_bits, line_bits);
            if exception_pending() {
                return MoltObject::none().bits();
            }
        }
    }
    MoltObject::none().bits()
}

#[no_mangle]
pub extern "C" fn molt_file_flush(handle_bits: u64) -> u64 {
    let handle_obj = obj_from_bits(handle_bits);
    let Some(ptr) = handle_obj.as_ptr() else {
        return raise_exception::<_>("TypeError", "expected file handle");
    };
    unsafe {
        if object_type_id(ptr) != TYPE_ID_FILE_HANDLE {
            return raise_exception::<_>("TypeError", "expected file handle");
        }
        let handle_ptr = file_handle_ptr(ptr);
        if handle_ptr.is_null() {
            return raise_exception::<_>("RuntimeError", "file handle missing");
        }
        let handle = &*handle_ptr;
        file_handle_require_attached!(handle);
        if file_handle_is_closed(handle) {
            return raise_exception::<_>("ValueError", "I/O operation on closed file");
        }
        let mut guard = handle.state.file.lock().unwrap();
        let Some(file) = guard.as_mut() else {
            return raise_exception::<_>("ValueError", "I/O operation on closed file");
        };
        if file.flush().is_err() {
            return raise_exception::<_>("OSError", "flush failed");
        }
    }
    MoltObject::none().bits()
}

#[no_mangle]
pub extern "C" fn molt_file_close(handle_bits: u64) -> u64 {
    let handle_obj = obj_from_bits(handle_bits);
    let Some(ptr) = handle_obj.as_ptr() else {
        return raise_exception::<_>("TypeError", "expected file handle");
    };
    unsafe {
        if object_type_id(ptr) != TYPE_ID_FILE_HANDLE {
            return raise_exception::<_>("TypeError", "expected file handle");
        }
        let handle_ptr = file_handle_ptr(ptr);
        if handle_ptr.is_null() {
            return raise_exception::<_>("RuntimeError", "file handle missing");
        }
        let handle = &*handle_ptr;
        file_handle_require_attached!(handle);
    }
    file_handle_close_ptr(ptr);
    MoltObject::none().bits()
}

#[no_mangle]
pub extern "C" fn molt_bridge_unavailable(msg_bits: u64) -> u64 {
    let msg = format_obj_str(obj_from_bits(msg_bits));
    eprintln!("Molt bridge unavailable: {msg}");
    std::process::exit(1);
}

#[no_mangle]
pub extern "C" fn molt_buffer2d_new(rows_bits: u64, cols_bits: u64, init_bits: u64) -> u64 {
    let rows = match to_i64(obj_from_bits(rows_bits)) {
        Some(val) if val >= 0 => val as usize,
        _ => return raise_exception::<_>("TypeError", "rows must be a non-negative int"),
    };
    let cols = match to_i64(obj_from_bits(cols_bits)) {
        Some(val) if val >= 0 => val as usize,
        _ => return raise_exception::<_>("TypeError", "cols must be a non-negative int"),
    };
    let init = match obj_from_bits(init_bits).as_int() {
        Some(val) => val,
        None => return raise_exception::<_>("TypeError", "init must be an int"),
    };
    let total = std::mem::size_of::<MoltHeader>() + std::mem::size_of::<*mut Buffer2D>();
    let ptr = alloc_object(total, TYPE_ID_BUFFER2D);
    if ptr.is_null() {
        return MoltObject::none().bits();
    }
    let size = rows.saturating_mul(cols);
    let buf = Box::new(Buffer2D {
        rows,
        cols,
        data: vec![init; size],
    });
    let buf_ptr = Box::into_raw(buf);
    unsafe {
        *(ptr as *mut *mut Buffer2D) = buf_ptr;
    }
    MoltObject::from_ptr(ptr).bits()
}

#[no_mangle]
pub extern "C" fn molt_buffer2d_get(obj_bits: u64, row_bits: u64, col_bits: u64) -> u64 {
    let row = match to_i64(obj_from_bits(row_bits)) {
        Some(val) if val >= 0 => val as usize,
        _ => return raise_exception::<_>("TypeError", "row must be a non-negative int"),
    };
    let col = match to_i64(obj_from_bits(col_bits)) {
        Some(val) if val >= 0 => val as usize,
        _ => return raise_exception::<_>("TypeError", "col must be a non-negative int"),
    };
    let obj = obj_from_bits(obj_bits);
    if let Some(ptr) = obj.as_ptr() {
        unsafe {
            if object_type_id(ptr) != TYPE_ID_BUFFER2D {
                return MoltObject::none().bits();
            }
            let buf = buffer2d_ptr(ptr);
            if buf.is_null() {
                return MoltObject::none().bits();
            }
            let buf = &*buf;
            if row >= buf.rows || col >= buf.cols {
                return raise_exception::<_>("IndexError", "buffer2d index out of range");
            }
            let idx = row * buf.cols + col;
            return MoltObject::from_int(buf.data[idx]).bits();
        }
    }
    MoltObject::none().bits()
}

#[no_mangle]
pub extern "C" fn molt_buffer2d_set(
    obj_bits: u64,
    row_bits: u64,
    col_bits: u64,
    val_bits: u64,
) -> u64 {
    let row = match to_i64(obj_from_bits(row_bits)) {
        Some(val) if val >= 0 => val as usize,
        _ => return raise_exception::<_>("TypeError", "row must be a non-negative int"),
    };
    let col = match to_i64(obj_from_bits(col_bits)) {
        Some(val) if val >= 0 => val as usize,
        _ => return raise_exception::<_>("TypeError", "col must be a non-negative int"),
    };
    let val = match obj_from_bits(val_bits).as_int() {
        Some(v) => v,
        None => return raise_exception::<_>("TypeError", "value must be an int"),
    };
    let obj = obj_from_bits(obj_bits);
    if let Some(ptr) = obj.as_ptr() {
        unsafe {
            if object_type_id(ptr) != TYPE_ID_BUFFER2D {
                return MoltObject::none().bits();
            }
            let buf = buffer2d_ptr(ptr);
            if buf.is_null() {
                return MoltObject::none().bits();
            }
            let buf = &mut *buf;
            if row >= buf.rows || col >= buf.cols {
                return raise_exception::<_>("IndexError", "buffer2d index out of range");
            }
            let idx = row * buf.cols + col;
            buf.data[idx] = val;
            return MoltObject::none().bits();
        }
    }
    MoltObject::none().bits()
}

#[no_mangle]
pub extern "C" fn molt_buffer2d_matmul(a_bits: u64, b_bits: u64) -> u64 {
    let a = obj_from_bits(a_bits);
    let b = obj_from_bits(b_bits);
    let (a_ptr, b_ptr) = match (a.as_ptr(), b.as_ptr()) {
        (Some(ap), Some(bp)) => (ap, bp),
        _ => return raise_exception::<_>("TypeError", "matmul expects buffer2d operands"),
    };
    unsafe {
        if object_type_id(a_ptr) != TYPE_ID_BUFFER2D || object_type_id(b_ptr) != TYPE_ID_BUFFER2D {
            return raise_exception::<_>("TypeError", "matmul expects buffer2d operands");
        }
        let a_buf = buffer2d_ptr(a_ptr);
        let b_buf = buffer2d_ptr(b_ptr);
        if a_buf.is_null() || b_buf.is_null() {
            return MoltObject::none().bits();
        }
        let a_buf = &*a_buf;
        let b_buf = &*b_buf;
        if a_buf.cols != b_buf.rows {
            return raise_exception::<_>("ValueError", "matmul dimension mismatch");
        }
        let rows = a_buf.rows;
        let cols = b_buf.cols;
        let inner = a_buf.cols;
        let total = std::mem::size_of::<MoltHeader>() + std::mem::size_of::<*mut Buffer2D>();
        let ptr = alloc_object(total, TYPE_ID_BUFFER2D);
        if ptr.is_null() {
            return MoltObject::none().bits();
        }
        let mut data = vec![0i64; rows.saturating_mul(cols)];
        for i in 0..rows {
            for j in 0..cols {
                let mut acc = 0i64;
                for k in 0..inner {
                    let left = a_buf.data[i * inner + k];
                    let right = b_buf.data[k * cols + j];
                    acc = acc.wrapping_add(left.wrapping_mul(right));
                }
                data[i * cols + j] = acc;
            }
        }
        let buf = Box::new(Buffer2D { rows, cols, data });
        let buf_ptr = Box::into_raw(buf);
        *(ptr as *mut *mut Buffer2D) = buf_ptr;
        MoltObject::from_ptr(ptr).bits()
    }
}

#[no_mangle]
pub extern "C" fn molt_dict_new(capacity_bits: u64) -> u64 {
    let total = std::mem::size_of::<MoltHeader>()
        + std::mem::size_of::<*mut Vec<u64>>()
        + std::mem::size_of::<*mut Vec<usize>>();
    let ptr = alloc_object(total, TYPE_ID_DICT);
    if ptr.is_null() {
        return MoltObject::none().bits();
    }
    unsafe {
        let capacity_hint = usize_from_bits(capacity_bits);
        let order = Vec::with_capacity(capacity_hint * 2);
        let mut table = Vec::new();
        if capacity_hint > 0 {
            table.resize(dict_table_capacity(capacity_hint), 0);
        }
        let order_ptr = Box::into_raw(Box::new(order));
        let table_ptr = Box::into_raw(Box::new(table));
        *(ptr as *mut *mut Vec<u64>) = order_ptr;
        *(ptr.add(std::mem::size_of::<*mut Vec<u64>>()) as *mut *mut Vec<usize>) = table_ptr;
    }
    MoltObject::from_ptr(ptr).bits()
}

enum DictSeqError {
    NotIterable,
    BadLen(usize),
    Exception,
}

fn dict_pair_from_item(item_bits: u64) -> Result<(u64, u64), DictSeqError> {
    let item_obj = obj_from_bits(item_bits);
    if let Some(item_ptr) = item_obj.as_ptr() {
        unsafe {
            let type_id = object_type_id(item_ptr);
            if type_id == TYPE_ID_LIST || type_id == TYPE_ID_TUPLE {
                let elems = seq_vec_ref(item_ptr);
                if elems.len() != 2 {
                    return Err(DictSeqError::BadLen(elems.len()));
                }
                return Ok((elems[0], elems[1]));
            }
        }
    }
    let iter_bits = molt_iter(item_bits);
    if obj_from_bits(iter_bits).is_none() {
        return Err(DictSeqError::NotIterable);
    }
    let mut elems = Vec::new();
    loop {
        let pair_bits = molt_iter_next(iter_bits);
        if exception_pending() {
            return Err(DictSeqError::Exception);
        }
        let pair_obj = obj_from_bits(pair_bits);
        let Some(pair_ptr) = pair_obj.as_ptr() else {
            return Err(DictSeqError::Exception);
        };
        unsafe {
            if object_type_id(pair_ptr) != TYPE_ID_TUPLE {
                return Err(DictSeqError::Exception);
            }
            let pair_elems = seq_vec_ref(pair_ptr);
            if pair_elems.len() < 2 {
                return Err(DictSeqError::Exception);
            }
            let done_bits = pair_elems[1];
            if is_truthy(obj_from_bits(done_bits)) {
                break;
            }
            elems.push(pair_elems[0]);
        }
    }
    if elems.len() != 2 {
        return Err(DictSeqError::BadLen(elems.len()));
    }
    Ok((elems[0], elems[1]))
}

#[no_mangle]
pub extern "C" fn molt_dict_from_obj(obj_bits: u64) -> u64 {
    let obj = obj_from_bits(obj_bits);
    let mut capacity = 0usize;
    if let Some(ptr) = obj.as_ptr() {
        unsafe {
            if object_type_id(ptr) == TYPE_ID_DICT {
                capacity = dict_len(ptr);
            }
        }
    }
    let dict_bits = molt_dict_new(capacity as u64);
    if obj_from_bits(dict_bits).is_none() {
        return MoltObject::none().bits();
    }
    let Some(_dict_ptr) = maybe_ptr_from_bits(dict_bits) else {
        return MoltObject::none().bits();
    };
    unsafe {
        let _ = dict_update_apply(dict_bits, dict_update_set_in_place, obj_bits);
    }
    if exception_pending() {
        return MoltObject::none().bits();
    }
    dict_bits
}

#[no_mangle]
pub extern "C" fn molt_set_new(capacity_bits: u64) -> u64 {
    let total = std::mem::size_of::<MoltHeader>()
        + std::mem::size_of::<*mut Vec<u64>>()
        + std::mem::size_of::<*mut Vec<usize>>();
    let ptr = alloc_object(total, TYPE_ID_SET);
    if ptr.is_null() {
        return MoltObject::none().bits();
    }
    unsafe {
        let capacity_hint = usize_from_bits(capacity_bits);
        let order = Vec::with_capacity(capacity_hint);
        let mut table = Vec::new();
        if capacity_hint > 0 {
            table.resize(set_table_capacity(capacity_hint), 0);
        }
        let order_ptr = Box::into_raw(Box::new(order));
        let table_ptr = Box::into_raw(Box::new(table));
        *(ptr as *mut *mut Vec<u64>) = order_ptr;
        *(ptr.add(std::mem::size_of::<*mut Vec<u64>>()) as *mut *mut Vec<usize>) = table_ptr;
    }
    MoltObject::from_ptr(ptr).bits()
}

#[no_mangle]
pub extern "C" fn molt_frozenset_new(capacity_bits: u64) -> u64 {
    let total = std::mem::size_of::<MoltHeader>()
        + std::mem::size_of::<*mut Vec<u64>>()
        + std::mem::size_of::<*mut Vec<usize>>();
    let ptr = alloc_object(total, TYPE_ID_FROZENSET);
    if ptr.is_null() {
        return MoltObject::none().bits();
    }
    unsafe {
        let capacity_hint = usize_from_bits(capacity_bits);
        let order = Vec::with_capacity(capacity_hint);
        let mut table = Vec::new();
        if capacity_hint > 0 {
            table.resize(set_table_capacity(capacity_hint), 0);
        }
        let order_ptr = Box::into_raw(Box::new(order));
        let table_ptr = Box::into_raw(Box::new(table));
        *(ptr as *mut *mut Vec<u64>) = order_ptr;
        *(ptr.add(std::mem::size_of::<*mut Vec<u64>>()) as *mut *mut Vec<usize>) = table_ptr;
    }
    MoltObject::from_ptr(ptr).bits()
}

#[no_mangle]
pub extern "C" fn molt_dict_builder_new(capacity_bits: u64) -> u64 {
    let total = std::mem::size_of::<MoltHeader>() + std::mem::size_of::<*mut Vec<u64>>();
    let ptr = alloc_object(total, TYPE_ID_DICT_BUILDER);
    if ptr.is_null() {
        return 0;
    }
    unsafe {
        let capacity_hint = usize_from_bits(capacity_bits);
        let vec = Vec::with_capacity(capacity_hint * 2);
        let vec_ptr = Box::into_raw(Box::new(vec));
        *(ptr as *mut *mut Vec<u64>) = vec_ptr;
    }
    bits_from_ptr(ptr)
}

#[no_mangle]
/// # Safety
/// Caller must ensure `builder_bits` is valid and points to a dict builder.
pub unsafe extern "C" fn molt_dict_builder_append(builder_bits: u64, key: u64, val: u64) {
    let builder_ptr = ptr_from_bits(builder_bits);
    if builder_ptr.is_null() {
        return;
    }
    let vec_ptr = *(builder_ptr as *mut *mut Vec<u64>);
    if vec_ptr.is_null() {
        return;
    }
    let vec = &mut *vec_ptr;
    vec.push(key);
    vec.push(val);
}

#[no_mangle]
/// # Safety
/// Caller must ensure `builder_bits` is valid and points to a dict builder.
pub unsafe extern "C" fn molt_dict_builder_finish(builder_bits: u64) -> u64 {
    let builder_ptr = ptr_from_bits(builder_bits);
    if builder_ptr.is_null() {
        return MoltObject::none().bits();
    }
    let _guard = PtrDropGuard::new(builder_ptr);
    let vec_ptr = *(builder_ptr as *mut *mut Vec<u64>);
    if vec_ptr.is_null() {
        return MoltObject::none().bits();
    }
    *(builder_ptr as *mut *mut Vec<u64>) = std::ptr::null_mut();
    let vec = Box::from_raw(vec_ptr);
    let ptr = alloc_dict_with_pairs(vec.as_slice());
    if ptr.is_null() {
        return MoltObject::none().bits();
    }
    MoltObject::from_ptr(ptr).bits()
}

// --- Set Builder ---

#[no_mangle]
pub extern "C" fn molt_set_builder_new(capacity_bits: u64) -> u64 {
    let total = std::mem::size_of::<MoltHeader>() + std::mem::size_of::<*mut Vec<u64>>();
    let ptr = alloc_object(total, TYPE_ID_SET_BUILDER);
    if ptr.is_null() {
        return 0;
    }
    unsafe {
        let capacity_hint = usize_from_bits(capacity_bits);
        let vec = Vec::with_capacity(capacity_hint);
        let vec_ptr = Box::into_raw(Box::new(vec));
        *(ptr as *mut *mut Vec<u64>) = vec_ptr;
    }
    bits_from_ptr(ptr)
}

#[no_mangle]
/// # Safety
/// Caller must ensure `builder_bits` is valid and points to a set builder.
pub unsafe extern "C" fn molt_set_builder_append(builder_bits: u64, key: u64) {
    let builder_ptr = ptr_from_bits(builder_bits);
    if builder_ptr.is_null() {
        return;
    }
    let vec_ptr = *(builder_ptr as *mut *mut Vec<u64>);
    if vec_ptr.is_null() {
        return;
    }
    let vec = &mut *vec_ptr;
    vec.push(key);
}

#[no_mangle]
/// # Safety
/// Caller must ensure `builder_bits` is valid and points to a set builder.
pub unsafe extern "C" fn molt_set_builder_finish(builder_bits: u64) -> u64 {
    let builder_ptr = ptr_from_bits(builder_bits);
    if builder_ptr.is_null() {
        return MoltObject::none().bits();
    }
    let _guard = PtrDropGuard::new(builder_ptr);
    let vec_ptr = *(builder_ptr as *mut *mut Vec<u64>);
    if vec_ptr.is_null() {
        return MoltObject::none().bits();
    }
    *(builder_ptr as *mut *mut Vec<u64>) = std::ptr::null_mut();
    let vec = Box::from_raw(vec_ptr);
    let ptr = alloc_set_with_entries(vec.as_slice());
    if ptr.is_null() {
        return MoltObject::none().bits();
    }
    MoltObject::from_ptr(ptr).bits()
}

// --- Channels ---

pub struct MoltChannel {
    pub sender: Sender<i64>,
    pub receiver: Receiver<i64>,
}

pub struct MoltStream {
    pub sender: Sender<Vec<u8>>,
    pub receiver: Receiver<Vec<u8>>,
    pub closed: AtomicBool,
    pub refs: AtomicUsize,
}

pub struct MoltWebSocket {
    pub sender: Sender<Vec<u8>>,
    pub receiver: Receiver<Vec<u8>>,
    pub closed: AtomicBool,
    pub send_hook: Option<extern "C" fn(*mut u8, *const u8, usize) -> i64>,
    pub recv_hook: Option<extern "C" fn(*mut u8) -> i64>,
    pub close_hook: Option<extern "C" fn(*mut u8)>,
    pub hook_ctx: *mut u8,
}

// TODO(runtime, owner:runtime, milestone:RT1, priority:P3): consolidate channel
// creation/send/recv helpers once ExceptionSentinel supports channel pointers to
// reduce duplication across wasm/native exports.

#[cfg(target_arch = "wasm32")]
#[no_mangle]
pub extern "C" fn molt_chan_new(capacity_bits: u64) -> *mut u8 {
    let capacity = match to_i64(obj_from_bits(capacity_bits)) {
        Some(val) => val,
        None => return raise_exception::<_>("TypeError", "channel capacity must be an integer"),
    };
    if capacity < 0 {
        return raise_exception::<_>("ValueError", "channel capacity must be non-negative");
    }
    let capacity = capacity as usize;
    let (s, r) = if capacity == 0 {
        unbounded()
    } else {
        bounded(capacity)
    };
    let chan = Box::new(MoltChannel {
        sender: s,
        receiver: r,
    });
    Box::into_raw(chan) as *mut u8
}

#[cfg(not(target_arch = "wasm32"))]
#[no_mangle]
pub extern "C" fn molt_chan_new(capacity_bits: u64) -> u64 {
    let capacity = match to_i64(obj_from_bits(capacity_bits)) {
        Some(val) => val,
        None => return raise_exception::<_>("TypeError", "channel capacity must be an integer"),
    };
    if capacity < 0 {
        return raise_exception::<_>("ValueError", "channel capacity must be non-negative");
    }
    let capacity = capacity as usize;
    let (s, r) = if capacity == 0 {
        unbounded()
    } else {
        bounded(capacity)
    };
    let chan = Box::new(MoltChannel {
        sender: s,
        receiver: r,
    });
    bits_from_ptr(Box::into_raw(chan) as *mut u8)
}

#[cfg(target_arch = "wasm32")]
#[no_mangle]
/// # Safety
/// Caller must ensure `chan_ptr` is a valid channel pointer.
pub unsafe extern "C" fn molt_chan_drop(chan_ptr: *mut u8) {
    if chan_ptr.is_null() {
        return;
    }
    drop(Box::from_raw(chan_ptr as *mut MoltChannel));
}

#[cfg(not(target_arch = "wasm32"))]
#[no_mangle]
/// # Safety
/// Caller must ensure `chan_bits` is a valid channel pointer.
pub unsafe extern "C" fn molt_chan_drop(chan_bits: u64) {
    let chan_ptr = ptr_from_bits(chan_bits);
    if chan_ptr.is_null() {
        return;
    }
    release_ptr(chan_ptr);
    drop(Box::from_raw(chan_ptr as *mut MoltChannel));
}

#[cfg(target_arch = "wasm32")]
#[no_mangle]
/// # Safety
/// Caller must ensure `chan_ptr` is a valid channel pointer.
pub unsafe extern "C" fn molt_chan_send(chan_ptr: *mut u8, val: i64) -> i64 {
    let chan = &*(chan_ptr as *mut MoltChannel);
    match chan.sender.try_send(val) {
        Ok(_) => 0,                   // Ready(None)
        Err(_) => pending_bits_i64(), // PENDING
    }
}

#[cfg(not(target_arch = "wasm32"))]
#[no_mangle]
/// # Safety
/// Caller must ensure `chan_bits` is a valid channel pointer.
pub unsafe extern "C" fn molt_chan_send(chan_bits: u64, val: i64) -> i64 {
    let chan_ptr = ptr_from_bits(chan_bits);
    let chan = &*(chan_ptr as *mut MoltChannel);
    match chan.sender.try_send(val) {
        Ok(_) => 0,                   // Ready(None)
        Err(_) => pending_bits_i64(), // PENDING
    }
}

#[cfg(target_arch = "wasm32")]
#[no_mangle]
/// # Safety
/// Caller must ensure `chan_ptr` is a valid channel pointer.
pub unsafe extern "C" fn molt_chan_recv(chan_ptr: *mut u8) -> i64 {
    let chan = &*(chan_ptr as *mut MoltChannel);
    match chan.receiver.try_recv() {
        Ok(val) => val,
        Err(_) => pending_bits_i64(), // PENDING
    }
}

#[cfg(not(target_arch = "wasm32"))]
#[no_mangle]
/// # Safety
/// Caller must ensure `chan_bits` is a valid channel pointer.
pub unsafe extern "C" fn molt_chan_recv(chan_bits: u64) -> i64 {
    let chan_ptr = ptr_from_bits(chan_bits);
    let chan = &*(chan_ptr as *mut MoltChannel);
    match chan.receiver.try_recv() {
        Ok(val) => val,
        Err(_) => pending_bits_i64(), // PENDING
    }
}

fn bytes_channel(capacity: usize) -> (Sender<Vec<u8>>, Receiver<Vec<u8>>) {
    if capacity == 0 {
        unbounded()
    } else {
        bounded(capacity)
    }
}

#[no_mangle]
pub extern "C" fn molt_stream_new(capacity_bits: u64) -> u64 {
    let capacity = usize_from_bits(capacity_bits);
    let (s, r) = bytes_channel(capacity);
    let stream = Box::new(MoltStream {
        sender: s,
        receiver: r,
        closed: AtomicBool::new(false),
        refs: AtomicUsize::new(1),
    });
    bits_from_ptr(Box::into_raw(stream) as *mut u8)
}

#[no_mangle]
/// # Safety
/// Caller must ensure `stream_bits` is a valid stream pointer.
pub unsafe extern "C" fn molt_stream_clone(stream_bits: u64) -> u64 {
    let stream_ptr = ptr_from_bits(stream_bits);
    if stream_ptr.is_null() {
        return MoltObject::none().bits();
    }
    let stream = &*(stream_ptr as *mut MoltStream);
    stream.refs.fetch_add(1, AtomicOrdering::AcqRel);
    stream_bits
}

#[no_mangle]
/// # Safety
/// Caller must ensure `stream_bits` is valid; `data_ptr` must be readable for `len_bits` bytes.
pub unsafe extern "C" fn molt_stream_send(
    stream_bits: u64,
    data_ptr: *const u8,
    len_bits: u64,
) -> i64 {
    let stream_ptr = ptr_from_bits(stream_bits);
    let len = usize_from_bits(len_bits);
    if stream_ptr.is_null() || (data_ptr.is_null() && len != 0) {
        return pending_bits_i64();
    }
    let stream = &*(stream_ptr as *mut MoltStream);
    let bytes = std::slice::from_raw_parts(data_ptr, len).to_vec();
    match stream.sender.try_send(bytes) {
        Ok(_) => 0,
        Err(_) => pending_bits_i64(),
    }
}

#[no_mangle]
/// # Safety
/// Caller must ensure `stream_bits` is valid; `data_bits` must be bytes-like.
pub unsafe extern "C" fn molt_stream_send_obj(stream_bits: u64, data_bits: u64) -> u64 {
    let send_data = match send_data_from_bits(data_bits) {
        Ok(data) => data,
        Err(msg) => return raise_exception::<_>("TypeError", &msg),
    };
    let (data_ptr, data_len, owned): (*const u8, usize, Option<Vec<u8>>) = match send_data {
        SendData::Borrowed(ptr, len) => (ptr, len, None),
        SendData::Owned(vec) => {
            let ptr = vec.as_ptr();
            let len = vec.len();
            (ptr, len, Some(vec))
        }
    };
    let _owned_guard = owned;
    molt_stream_send(stream_bits, data_ptr, data_len as u64) as u64
}

#[no_mangle]
/// # Safety
/// Caller must ensure `stream_bits` is a valid stream pointer.
pub unsafe extern "C" fn molt_stream_recv(stream_bits: u64) -> i64 {
    let stream_ptr = ptr_from_bits(stream_bits);
    if stream_ptr.is_null() {
        return MoltObject::none().bits() as i64;
    }
    let stream = &*(stream_ptr as *mut MoltStream);
    match stream.receiver.try_recv() {
        Ok(bytes) => {
            let ptr = alloc_bytes(&bytes);
            if ptr.is_null() {
                MoltObject::none().bits() as i64
            } else {
                MoltObject::from_ptr(ptr).bits() as i64
            }
        }
        Err(_) => {
            if stream.closed.load(AtomicOrdering::Relaxed) {
                MoltObject::none().bits() as i64
            } else {
                pending_bits_i64()
            }
        }
    }
}

#[no_mangle]
/// # Safety
/// Caller must ensure `stream_bits` is a valid stream pointer.
pub unsafe extern "C" fn molt_stream_close(stream_bits: u64) {
    let stream_ptr = ptr_from_bits(stream_bits);
    if stream_ptr.is_null() {
        return;
    }
    let stream = &*(stream_ptr as *mut MoltStream);
    stream.closed.store(true, AtomicOrdering::Relaxed);
}

#[no_mangle]
/// # Safety
/// Caller must ensure `out_left` and `out_right` are valid writable pointers.
pub unsafe extern "C" fn molt_ws_pair(
    capacity_bits: u64,
    out_left: *mut u64,
    out_right: *mut u64,
) -> i32 {
    if out_left.is_null() || out_right.is_null() {
        return 2;
    }
    let capacity = usize_from_bits(capacity_bits);
    let (a_tx, a_rx) = bytes_channel(capacity);
    let (b_tx, b_rx) = bytes_channel(capacity);
    let left = Box::new(MoltWebSocket {
        sender: a_tx,
        receiver: b_rx,
        closed: AtomicBool::new(false),
        send_hook: None,
        recv_hook: None,
        close_hook: None,
        hook_ctx: std::ptr::null_mut(),
    });
    let right = Box::new(MoltWebSocket {
        sender: b_tx,
        receiver: a_rx,
        closed: AtomicBool::new(false),
        send_hook: None,
        recv_hook: None,
        close_hook: None,
        hook_ctx: std::ptr::null_mut(),
    });
    *out_left = bits_from_ptr(Box::into_raw(left) as *mut u8);
    *out_right = bits_from_ptr(Box::into_raw(right) as *mut u8);
    0
}

#[no_mangle]
pub extern "C" fn molt_ws_new_with_hooks(
    send_hook: usize,
    recv_hook: usize,
    close_hook: usize,
    hook_ctx: *mut u8,
) -> *mut u8 {
    let send_hook = if send_hook == 0 {
        None
    } else {
        Some(unsafe {
            std::mem::transmute::<usize, extern "C" fn(*mut u8, *const u8, usize) -> i64>(send_hook)
        })
    };
    let recv_hook = if recv_hook == 0 {
        None
    } else {
        Some(unsafe { std::mem::transmute::<usize, extern "C" fn(*mut u8) -> i64>(recv_hook) })
    };
    let close_hook = if close_hook == 0 {
        None
    } else {
        Some(unsafe { std::mem::transmute::<usize, extern "C" fn(*mut u8)>(close_hook) })
    };
    let (s, r) = bytes_channel(0);
    let ws = Box::new(MoltWebSocket {
        sender: s,
        receiver: r,
        closed: AtomicBool::new(false),
        send_hook,
        recv_hook,
        close_hook,
        hook_ctx,
    });
    Box::into_raw(ws) as *mut u8
}

type WsConnectHook = extern "C" fn(*const u8, usize) -> *mut u8;
type DbHostHook = extern "C" fn(*const u8, usize, *mut u64, u64) -> i32;

static WS_CONNECT_HOOK: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);
static DB_QUERY_HOOK: AtomicUsize = AtomicUsize::new(0);
static DB_EXEC_HOOK: AtomicUsize = AtomicUsize::new(0);

#[no_mangle]
pub extern "C" fn molt_ws_set_connect_hook(ptr: usize) {
    WS_CONNECT_HOOK.store(ptr, AtomicOrdering::Release);
}

#[no_mangle]
pub extern "C" fn molt_db_set_query_hook(ptr: usize) {
    DB_QUERY_HOOK.store(ptr, AtomicOrdering::Release);
}

#[no_mangle]
pub extern "C" fn molt_db_set_exec_hook(ptr: usize) {
    DB_EXEC_HOOK.store(ptr, AtomicOrdering::Release);
}

fn load_capabilities() -> HashSet<String> {
    let mut set = HashSet::new();
    let caps = std::env::var("MOLT_CAPABILITIES").unwrap_or_default();
    for cap in caps.split(',') {
        let cap = cap.trim();
        if !cap.is_empty() {
            set.insert(cap.to_string());
        }
    }
    set
}

fn has_capability(name: &str) -> bool {
    let caps = runtime_state().capabilities.get_or_init(load_capabilities);
    caps.contains(name)
}

#[no_mangle]
/// # Safety
/// Caller must ensure `url_ptr` is valid for `url_len` bytes and `out` is writable.
pub unsafe extern "C" fn molt_ws_connect(
    url_ptr: *const u8,
    url_len_bits: u64,
    out: *mut u64,
) -> i32 {
    if out.is_null() {
        return 2;
    }
    let url_len = usize_from_bits(url_len_bits);
    if url_ptr.is_null() && url_len != 0 {
        return 1;
    }
    if !has_capability("websocket.connect") {
        return 6;
    }
    let hook_ptr = WS_CONNECT_HOOK.load(AtomicOrdering::Acquire);
    if hook_ptr == 0 {
        // TODO(molt): Provide a host-level connect hook for production sockets.
        return 7;
    }
    let hook: WsConnectHook = std::mem::transmute(hook_ptr);
    let ws_ptr = hook(url_ptr, url_len);
    if ws_ptr.is_null() {
        return 7;
    }
    *out = bits_from_ptr(ws_ptr);
    0
}

#[no_mangle]
/// # Safety
/// Caller must ensure `req_ptr` is valid for `len_bits` bytes and `out` is writable.
pub unsafe extern "C" fn molt_db_query(
    req_ptr: *const u8,
    len_bits: u64,
    out: *mut u64,
    token_bits: u64,
) -> i32 {
    let len = usize_from_bits(len_bits);
    if out.is_null() {
        return 2;
    }
    if req_ptr.is_null() && len != 0 {
        return 1;
    }
    if !has_capability("db.read") {
        return 6;
    }
    cancel_tokens();
    let token_id = match token_id_from_bits(token_bits) {
        Some(0) => current_token_id(),
        Some(id) => id,
        None => return 1,
    };
    #[cfg(target_arch = "wasm32")]
    {
        return molt_db_query_host(req_ptr as u64, len_bits, out as u64, token_id);
    }
    #[cfg(not(target_arch = "wasm32"))]
    {
        let hook_ptr = DB_QUERY_HOOK.load(AtomicOrdering::Acquire);
        if hook_ptr == 0 {
            return 7;
        }
        let hook: DbHostHook = std::mem::transmute(hook_ptr);
        hook(req_ptr, len, out, token_id)
    }
}

#[no_mangle]
/// # Safety
/// Caller must ensure `req_ptr` is valid for `len_bits` bytes and `out` is writable.
pub unsafe extern "C" fn molt_db_exec(
    req_ptr: *const u8,
    len_bits: u64,
    out: *mut u64,
    token_bits: u64,
) -> i32 {
    let len = usize_from_bits(len_bits);
    if out.is_null() {
        return 2;
    }
    if req_ptr.is_null() && len != 0 {
        return 1;
    }
    if !has_capability("db.write") {
        return 6;
    }
    cancel_tokens();
    let token_id = match token_id_from_bits(token_bits) {
        Some(0) => current_token_id(),
        Some(id) => id,
        None => return 1,
    };
    #[cfg(target_arch = "wasm32")]
    {
        return molt_db_exec_host(req_ptr as u64, len_bits, out as u64, token_id);
    }
    #[cfg(not(target_arch = "wasm32"))]
    {
        let hook_ptr = DB_EXEC_HOOK.load(AtomicOrdering::Acquire);
        if hook_ptr == 0 {
            return 7;
        }
        let hook: DbHostHook = std::mem::transmute(hook_ptr);
        hook(req_ptr, len, out, token_id)
    }
}

#[no_mangle]
/// # Safety
/// Caller must ensure `ws_bits` is valid; `data_ptr` must be readable for `len_bits` bytes.
pub unsafe extern "C" fn molt_ws_send(ws_bits: u64, data_ptr: *const u8, len_bits: u64) -> i64 {
    let ws_ptr = ptr_from_bits(ws_bits);
    let len = usize_from_bits(len_bits);
    if ws_ptr.is_null() || (data_ptr.is_null() && len != 0) {
        return pending_bits_i64();
    }
    let ws = &*(ws_ptr as *mut MoltWebSocket);
    if let Some(hook) = ws.send_hook {
        return hook(ws.hook_ctx, data_ptr, len);
    }
    let bytes = std::slice::from_raw_parts(data_ptr, len).to_vec();
    match ws.sender.try_send(bytes) {
        Ok(_) => 0,
        Err(_) => pending_bits_i64(),
    }
}

#[no_mangle]
/// # Safety
/// Caller must ensure `ws_bits` is a valid websocket pointer.
pub unsafe extern "C" fn molt_ws_recv(ws_bits: u64) -> i64 {
    let ws_ptr = ptr_from_bits(ws_bits);
    if ws_ptr.is_null() {
        return MoltObject::none().bits() as i64;
    }
    let ws = &*(ws_ptr as *mut MoltWebSocket);
    if let Some(hook) = ws.recv_hook {
        return hook(ws.hook_ctx);
    }
    match ws.receiver.try_recv() {
        Ok(bytes) => {
            let ptr = alloc_bytes(&bytes);
            if ptr.is_null() {
                MoltObject::none().bits() as i64
            } else {
                MoltObject::from_ptr(ptr).bits() as i64
            }
        }
        Err(_) => {
            if ws.closed.load(AtomicOrdering::Relaxed) {
                MoltObject::none().bits() as i64
            } else {
                pending_bits_i64()
            }
        }
    }
}

#[no_mangle]
/// # Safety
/// Caller must ensure `ws_bits` is a valid websocket pointer.
pub unsafe extern "C" fn molt_ws_close(ws_bits: u64) {
    let ws_ptr = ptr_from_bits(ws_bits);
    if ws_ptr.is_null() {
        return;
    }
    let ws = &*(ws_ptr as *mut MoltWebSocket);
    if let Some(hook) = ws.close_hook {
        hook(ws.hook_ctx);
    }
    ws.closed.store(true, AtomicOrdering::Relaxed);
}

#[no_mangle]
/// # Safety
/// Caller must ensure `stream_bits` is a valid stream pointer.
pub unsafe extern "C" fn molt_stream_drop(stream_bits: u64) {
    let stream_ptr = ptr_from_bits(stream_bits);
    if stream_ptr.is_null() {
        return;
    }
    let stream = &*(stream_ptr as *mut MoltStream);
    if stream.refs.fetch_sub(1, AtomicOrdering::AcqRel) > 1 {
        return;
    }
    release_ptr(stream_ptr);
    drop(Box::from_raw(stream_ptr as *mut MoltStream));
}

#[no_mangle]
/// # Safety
/// Caller must ensure `ws_bits` is a valid websocket pointer.
pub unsafe extern "C" fn molt_ws_drop(ws_bits: u64) {
    let ws_ptr = ptr_from_bits(ws_bits);
    if ws_ptr.is_null() {
        return;
    }
    let ws = &*(ws_ptr as *mut MoltWebSocket);
    if !ws.closed.load(AtomicOrdering::Relaxed) {
        if let Some(hook) = ws.close_hook {
            hook(ws.hook_ctx);
        }
    }
    release_ptr(ws_ptr);
    drop(Box::from_raw(ws_ptr as *mut MoltWebSocket));
}

// --- Sockets ---

#[cfg(not(target_arch = "wasm32"))]
enum MoltSocketKind {
    Closed,
    Pending(Socket),
    TcpStream(mio::net::TcpStream),
    TcpListener(mio::net::TcpListener),
    UdpSocket(mio::net::UdpSocket),
    #[cfg(unix)]
    UnixStream(mio::net::UnixStream),
    #[cfg(unix)]
    UnixListener(mio::net::UnixListener),
    #[cfg(unix)]
    UnixDatagram(mio::net::UnixDatagram),
}

#[cfg(not(target_arch = "wasm32"))]
struct MoltSocketInner {
    kind: MoltSocketKind,
    family: i32,
    sock_type: i32,
    proto: i32,
    connect_pending: bool,
}

#[cfg(not(target_arch = "wasm32"))]
struct MoltSocket {
    inner: Mutex<MoltSocketInner>,
    timeout: Mutex<Option<Duration>>,
    closed: AtomicBool,
    refs: AtomicUsize,
}

#[cfg(not(target_arch = "wasm32"))]
#[cfg(unix)]
type SocketFd = RawFd;
#[cfg(not(target_arch = "wasm32"))]
#[cfg(windows)]
type SocketFd = RawSocket;

#[cfg(not(target_arch = "wasm32"))]
fn socket_fd_map() -> &'static Mutex<HashMap<SocketFd, PtrSlot>> {
    static SOCKET_FD_MAP: OnceLock<Mutex<HashMap<SocketFd, PtrSlot>>> = OnceLock::new();
    SOCKET_FD_MAP.get_or_init(|| Mutex::new(HashMap::new()))
}

#[cfg(not(target_arch = "wasm32"))]
fn socket_register_fd(socket_ptr: *mut u8) {
    if socket_ptr.is_null() {
        return;
    }
    let socket = unsafe { &*(socket_ptr as *mut MoltSocket) };
    let guard = socket.inner.lock().unwrap();
    #[cfg(unix)]
    let fd = guard.raw_fd();
    #[cfg(windows)]
    let fd = guard.raw_socket();
    drop(guard);
    if let Some(fd) = fd {
        socket_fd_map()
            .lock()
            .unwrap()
            .insert(fd, PtrSlot(socket_ptr));
    }
}

#[cfg(not(target_arch = "wasm32"))]
fn socket_unregister_fd(socket_ptr: *mut u8) {
    if socket_ptr.is_null() {
        return;
    }
    let socket = unsafe { &*(socket_ptr as *mut MoltSocket) };
    let guard = socket.inner.lock().unwrap();
    #[cfg(unix)]
    let fd = guard.raw_fd();
    #[cfg(windows)]
    let fd = guard.raw_socket();
    drop(guard);
    if let Some(fd) = fd {
        socket_fd_map().lock().unwrap().remove(&fd);
    }
}

#[cfg(not(target_arch = "wasm32"))]
fn socket_ptr_from_fd(fd: SocketFd) -> Option<*mut u8> {
    socket_fd_map().lock().unwrap().get(&fd).map(|slot| slot.0)
}

#[cfg(not(target_arch = "wasm32"))]
fn socket_ptr_from_bits_or_fd(socket_bits: u64) -> *mut u8 {
    let ptr = ptr_from_bits(socket_bits);
    if !ptr.is_null() {
        return ptr;
    }
    if let Some(fd) = to_i64(obj_from_bits(socket_bits)) {
        if fd < 0 {
            return std::ptr::null_mut();
        }
        #[cfg(unix)]
        {
            return socket_ptr_from_fd(fd as RawFd).unwrap_or(std::ptr::null_mut());
        }
        #[cfg(windows)]
        {
            return socket_ptr_from_fd(fd as RawSocket).unwrap_or(std::ptr::null_mut());
        }
    }
    std::ptr::null_mut()
}

#[cfg(not(target_arch = "wasm32"))]
impl MoltSocketInner {
    fn source_mut(&mut self) -> Option<&mut dyn mio::event::Source> {
        match &mut self.kind {
            MoltSocketKind::Closed => None,
            MoltSocketKind::TcpStream(stream) => Some(stream),
            MoltSocketKind::TcpListener(listener) => Some(listener),
            MoltSocketKind::UdpSocket(sock) => Some(sock),
            #[cfg(unix)]
            MoltSocketKind::UnixStream(stream) => Some(stream),
            #[cfg(unix)]
            MoltSocketKind::UnixListener(listener) => Some(listener),
            #[cfg(unix)]
            MoltSocketKind::UnixDatagram(sock) => Some(sock),
            MoltSocketKind::Pending(_) => None,
        }
    }

    fn is_stream(&self) -> bool {
        match self.kind {
            MoltSocketKind::TcpStream(_)
            | MoltSocketKind::Pending(_)
            | MoltSocketKind::TcpListener(_) => true,
            #[cfg(unix)]
            MoltSocketKind::UnixStream(_) | MoltSocketKind::UnixListener(_) => true,
            _ => false,
        }
    }

    #[cfg(unix)]
    fn raw_fd(&self) -> Option<RawFd> {
        let fd = match &self.kind {
            MoltSocketKind::Pending(sock) => sock.as_raw_fd(),
            MoltSocketKind::TcpStream(sock) => sock.as_raw_fd(),
            MoltSocketKind::TcpListener(sock) => sock.as_raw_fd(),
            MoltSocketKind::UdpSocket(sock) => sock.as_raw_fd(),
            MoltSocketKind::UnixStream(sock) => sock.as_raw_fd(),
            MoltSocketKind::UnixListener(sock) => sock.as_raw_fd(),
            MoltSocketKind::UnixDatagram(sock) => sock.as_raw_fd(),
            MoltSocketKind::Closed => return None,
        };
        Some(fd)
    }

    #[cfg(windows)]
    fn raw_socket(&self) -> Option<RawSocket> {
        let sock = match &self.kind {
            MoltSocketKind::Pending(sock) => sock.as_raw_socket(),
            MoltSocketKind::TcpStream(sock) => sock.as_raw_socket(),
            MoltSocketKind::TcpListener(sock) => sock.as_raw_socket(),
            MoltSocketKind::UdpSocket(sock) => sock.as_raw_socket(),
            MoltSocketKind::Closed => return None,
        };
        Some(sock)
    }
}

#[cfg(not(target_arch = "wasm32"))]
fn with_socket_mut<R, F>(socket_ptr: *mut u8, f: F) -> Result<R, std::io::Error>
where
    F: FnOnce(&mut MoltSocketInner) -> Result<R, std::io::Error>,
{
    if socket_ptr.is_null() {
        return Err(std::io::Error::new(
            ErrorKind::InvalidInput,
            "invalid socket",
        ));
    }
    let socket = unsafe { &*(socket_ptr as *mut MoltSocket) };
    if socket.closed.load(AtomicOrdering::Relaxed) {
        return Err(std::io::Error::new(
            ErrorKind::NotConnected,
            "socket is closed",
        ));
    }
    let mut guard = socket.inner.lock().unwrap();
    f(&mut *guard)
}

#[cfg(not(target_arch = "wasm32"))]
fn socket_timeout(socket_ptr: *mut u8) -> Option<Duration> {
    if socket_ptr.is_null() {
        return None;
    }
    let socket = unsafe { &*(socket_ptr as *mut MoltSocket) };
    let guard = socket.timeout.lock().unwrap();
    *guard
}

#[cfg(not(target_arch = "wasm32"))]
fn socket_set_timeout(socket_ptr: *mut u8, timeout: Option<Duration>) {
    if socket_ptr.is_null() {
        return;
    }
    let socket = unsafe { &*(socket_ptr as *mut MoltSocket) };
    let mut guard = socket.timeout.lock().unwrap();
    *guard = timeout;
}

#[cfg(not(target_arch = "wasm32"))]
fn socket_mark_closed(socket_ptr: *mut u8) {
    if socket_ptr.is_null() {
        return;
    }
    let socket = unsafe { &*(socket_ptr as *mut MoltSocket) };
    socket.closed.store(true, AtomicOrdering::Relaxed);
}

#[cfg(not(target_arch = "wasm32"))]
fn socket_ref_inc(socket_ptr: *mut u8) {
    if socket_ptr.is_null() {
        return;
    }
    let socket = unsafe { &*(socket_ptr as *mut MoltSocket) };
    socket.refs.fetch_add(1, AtomicOrdering::AcqRel);
}

#[cfg(not(target_arch = "wasm32"))]
fn socket_ref_dec(socket_ptr: *mut u8) {
    if socket_ptr.is_null() {
        return;
    }
    let socket = unsafe { &*(socket_ptr as *mut MoltSocket) };
    if socket.refs.fetch_sub(1, AtomicOrdering::AcqRel) != 1 {
        return;
    }
    if !socket.closed.load(AtomicOrdering::Relaxed) {
        runtime_state().io_poller().deregister_socket(socket_ptr);
        socket.closed.store(true, AtomicOrdering::Relaxed);
        let mut guard = socket.inner.lock().unwrap();
        guard.kind = MoltSocketKind::Closed;
    }
    release_ptr(socket_ptr);
    unsafe {
        drop(Box::from_raw(socket_ptr as *mut MoltSocket));
    }
}

#[cfg(not(target_arch = "wasm32"))]
enum SendData {
    Borrowed(*const u8, usize),
    Owned(Vec<u8>),
}

#[cfg(not(target_arch = "wasm32"))]
fn io_wait_release_socket(future_ptr: *mut u8) {
    if future_ptr.is_null() {
        return;
    }
    let header = unsafe { header_from_obj_ptr(future_ptr) };
    let payload_bytes = unsafe {
        (*header)
            .size
            .saturating_sub(std::mem::size_of::<MoltHeader>())
    };
    if payload_bytes < std::mem::size_of::<u64>() {
        return;
    }
    let payload_ptr = future_ptr as *mut u64;
    let socket_bits = unsafe { *payload_ptr };
    let socket_ptr = socket_ptr_from_bits_or_fd(socket_bits);
    if !socket_ptr.is_null() {
        socket_ref_dec(socket_ptr);
    }
}

#[cfg(not(target_arch = "wasm32"))]
fn send_data_from_bits(bits: u64) -> Result<SendData, String> {
    let obj = obj_from_bits(bits);
    let Some(ptr) = obj.as_ptr() else {
        return Err("send expects bytes-like object".to_string());
    };
    unsafe {
        let type_id = object_type_id(ptr);
        if type_id == TYPE_ID_BYTES || type_id == TYPE_ID_BYTEARRAY {
            let len = bytes_len(ptr);
            let data = bytes_data(ptr);
            return Ok(SendData::Borrowed(data, len));
        }
        if type_id == TYPE_ID_MEMORYVIEW {
            if let Some(slice) = memoryview_bytes_slice(ptr) {
                return Ok(SendData::Borrowed(slice.as_ptr(), slice.len()));
            }
            if let Some(vec) = memoryview_collect_bytes(ptr) {
                return Ok(SendData::Owned(vec));
            }
        }
    }
    Err("send expects bytes-like object".to_string())
}

#[cfg(not(target_arch = "wasm32"))]
fn require_capability<T: ExceptionSentinel>(caps: &[&str], label: &str) -> Result<(), T> {
    if caps.iter().any(|cap| has_capability(cap)) {
        Ok(())
    } else {
        let msg = format!("missing {label} capability");
        Err(raise_exception::<T>("PermissionError", &msg))
    }
}

#[cfg(not(target_arch = "wasm32"))]
fn require_time_wall_capability<T: ExceptionSentinel>() -> Result<(), T> {
    require_capability(&["time.wall", "time"], "time.wall")
}

#[cfg(not(target_arch = "wasm32"))]
fn require_net_capability<T: ExceptionSentinel>(caps: &[&str]) -> Result<(), T> {
    require_capability(caps, "net")
}

#[cfg(not(target_arch = "wasm32"))]
fn require_process_capability<T: ExceptionSentinel>(caps: &[&str]) -> Result<(), T> {
    require_capability(caps, "process")
}

#[cfg(not(target_arch = "wasm32"))]
fn host_from_bits(bits: u64) -> Result<Option<String>, String> {
    let obj = obj_from_bits(bits);
    if obj.is_none() {
        return Ok(None);
    }
    if let Some(text) = string_obj_to_owned(obj) {
        return Ok(Some(text));
    }
    if let Some(ptr) = obj.as_ptr() {
        unsafe {
            if object_type_id(ptr) == TYPE_ID_BYTES {
                let len = bytes_len(ptr);
                let bytes = std::slice::from_raw_parts(bytes_data(ptr), len);
                let text = std::str::from_utf8(bytes)
                    .map_err(|_| "host bytes must be utf-8".to_string())?;
                return Ok(Some(text.to_string()));
            }
        }
    }
    let obj_type = class_name_for_error(type_of_bits(bits));
    Err(format!("host must be str, bytes, or None, not {obj_type}"))
}

#[cfg(not(target_arch = "wasm32"))]
fn port_from_bits(bits: u64) -> Result<u16, String> {
    let obj = obj_from_bits(bits);
    if let Some(port) = to_i64(obj) {
        if port < 0 || port > u16::MAX as i64 {
            return Err("port out of range".to_string());
        }
        return Ok(port as u16);
    }
    if let Some(text) = string_obj_to_owned(obj) {
        let port = text
            .parse::<u16>()
            .map_err(|_| "port must be int".to_string())?;
        return Ok(port);
    }
    let obj_type = class_name_for_error(type_of_bits(bits));
    Err(format!("port must be int or str, not {obj_type}"))
}

#[cfg(not(target_arch = "wasm32"))]
fn service_from_bits(bits: u64) -> Result<Option<String>, String> {
    let obj = obj_from_bits(bits);
    if obj.is_none() {
        return Ok(None);
    }
    if let Some(port) = to_i64(obj) {
        if port < 0 || port > u16::MAX as i64 {
            return Err("port out of range".to_string());
        }
        return Ok(Some(port.to_string()));
    }
    if let Some(text) = string_obj_to_owned(obj) {
        return Ok(Some(text));
    }
    if let Some(ptr) = obj.as_ptr() {
        unsafe {
            if object_type_id(ptr) == TYPE_ID_BYTES {
                let len = bytes_len(ptr);
                let bytes = std::slice::from_raw_parts(bytes_data(ptr), len);
                let text = std::str::from_utf8(bytes)
                    .map_err(|_| "service bytes must be utf-8".to_string())?;
                return Ok(Some(text.to_string()));
            }
        }
    }
    let obj_type = class_name_for_error(type_of_bits(bits));
    Err(format!("service must be int or str, not {obj_type}"))
}

#[cfg(not(target_arch = "wasm32"))]
fn sockaddr_from_bits(addr_bits: u64, family: i32) -> Result<SockAddr, String> {
    if family == libc::AF_UNIX {
        #[cfg(unix)]
        {
            let path = path_from_bits(addr_bits)?;
            return SockAddr::unix(path).map_err(|err| err.to_string());
        }
        #[cfg(not(unix))]
        {
            return Err("AF_UNIX is unsupported".to_string());
        }
    }
    let obj = obj_from_bits(addr_bits);
    let Some(ptr) = obj.as_ptr() else {
        return Err("address must be tuple".to_string());
    };
    unsafe {
        let type_id = object_type_id(ptr);
        if type_id != TYPE_ID_TUPLE && type_id != TYPE_ID_LIST {
            return Err("address must be tuple".to_string());
        }
        let elems = seq_vec_ref(ptr);
        if elems.len() < 2 {
            return Err("address must be (host, port)".to_string());
        }
        let host = host_from_bits(elems[0])?;
        let port = port_from_bits(elems[1])?;
        if family == libc::AF_INET {
            let host = host.unwrap_or_else(|| "0.0.0.0".to_string());
            let ip = host
                .parse::<Ipv4Addr>()
                .or_else(|_| {
                    (host.as_str(), port)
                        .to_socket_addrs()
                        .ok()
                        .and_then(|mut iter| {
                            iter.find_map(|addr| match addr.ip() {
                                IpAddr::V4(v4) => Some(v4),
                                _ => None,
                            })
                        })
                        .ok_or(())
                })
                .map_err(|_| "invalid IPv4 address".to_string())?;
            return Ok(SockAddr::from(SocketAddr::new(IpAddr::V4(ip), port)));
        }
        if family == libc::AF_INET6 {
            let host = host.unwrap_or_else(|| "::".to_string());
            let mut flowinfo = 0u32;
            let mut scope_id = 0u32;
            if elems.len() >= 3 {
                flowinfo = to_i64(obj_from_bits(elems[2])).unwrap_or(0) as u32;
            }
            if elems.len() >= 4 {
                scope_id = to_i64(obj_from_bits(elems[3])).unwrap_or(0) as u32;
            }
            let ip = host
                .parse::<Ipv6Addr>()
                .or_else(|_| {
                    (host.as_str(), port)
                        .to_socket_addrs()
                        .ok()
                        .and_then(|mut iter| {
                            iter.find_map(|addr| match addr.ip() {
                                IpAddr::V6(v6) => Some(v6),
                                _ => None,
                            })
                        })
                        .ok_or(())
                })
                .map_err(|_| "invalid IPv6 address".to_string())?;
            let addr = SocketAddr::V6(std::net::SocketAddrV6::new(ip, port, flowinfo, scope_id));
            return Ok(SockAddr::from(addr));
        }
    }
    Err("unsupported address family".to_string())
}

#[cfg(not(target_arch = "wasm32"))]
fn sockaddr_to_bits(addr: &SockAddr) -> u64 {
    if let Some(sockaddr) = addr.as_socket() {
        match sockaddr {
            SocketAddr::V4(v4) => {
                let host = v4.ip().to_string();
                let host_ptr = alloc_string(host.as_bytes());
                if host_ptr.is_null() {
                    return MoltObject::none().bits();
                }
                let host_bits = MoltObject::from_ptr(host_ptr).bits();
                let port_bits = MoltObject::from_int(v4.port() as i64).bits();
                let tuple_ptr = alloc_tuple(&[host_bits, port_bits]);
                dec_ref_bits(host_bits);
                if tuple_ptr.is_null() {
                    MoltObject::none().bits()
                } else {
                    MoltObject::from_ptr(tuple_ptr).bits()
                }
            }
            SocketAddr::V6(v6) => {
                let host = v6.ip().to_string();
                let host_ptr = alloc_string(host.as_bytes());
                if host_ptr.is_null() {
                    return MoltObject::none().bits();
                }
                let host_bits = MoltObject::from_ptr(host_ptr).bits();
                let port_bits = MoltObject::from_int(v6.port() as i64).bits();
                let flow_bits = MoltObject::from_int(v6.flowinfo() as i64).bits();
                let scope_bits = MoltObject::from_int(v6.scope_id() as i64).bits();
                let tuple_ptr = alloc_tuple(&[host_bits, port_bits, flow_bits, scope_bits]);
                dec_ref_bits(host_bits);
                if tuple_ptr.is_null() {
                    MoltObject::none().bits()
                } else {
                    MoltObject::from_ptr(tuple_ptr).bits()
                }
            }
        }
    } else {
        #[cfg(unix)]
        {
            if let Some(path) = addr.as_pathname() {
                let text = path.to_string_lossy();
                let ptr = alloc_string(text.as_bytes());
                if ptr.is_null() {
                    MoltObject::none().bits()
                } else {
                    MoltObject::from_ptr(ptr).bits()
                }
            } else {
                MoltObject::none().bits()
            }
        }
        #[cfg(not(unix))]
        {
            MoltObject::none().bits()
        }
    }
}

#[cfg(not(target_arch = "wasm32"))]
fn socket_wait_ready(socket_ptr: *mut u8, events: u32) -> Result<(), std::io::Error> {
    let timeout = socket_timeout(socket_ptr);
    if let Some(timeout) = timeout {
        if timeout == Duration::ZERO {
            return Err(std::io::Error::new(ErrorKind::WouldBlock, "would block"));
        }
        runtime_state()
            .io_poller()
            .wait_blocking(socket_ptr, events, Some(timeout))
            .map(|_| ())
    } else {
        runtime_state()
            .io_poller()
            .wait_blocking(socket_ptr, events, None)
            .map(|_| ())
    }
}

#[cfg(not(target_arch = "wasm32"))]
fn os_string_from_bits(bits: u64) -> Result<OsString, String> {
    let path = path_from_bits(bits)?;
    Ok(path.into_os_string())
}

#[cfg(not(target_arch = "wasm32"))]
fn argv_from_bits(args_bits: u64) -> Result<Vec<OsString>, String> {
    let obj = obj_from_bits(args_bits);
    if obj.is_none() {
        return Err("args must be a sequence".to_string());
    }
    if let Some(ptr) = obj.as_ptr() {
        let type_id = unsafe { object_type_id(ptr) };
        if type_id == TYPE_ID_LIST || type_id == TYPE_ID_TUPLE {
            let elems = unsafe { seq_vec_ref(ptr) };
            let mut args = Vec::with_capacity(elems.len());
            for &elem in elems.iter() {
                args.push(os_string_from_bits(elem)?);
            }
            return Ok(args);
        }
    }
    Ok(vec![os_string_from_bits(args_bits)?])
}

#[cfg(not(target_arch = "wasm32"))]
fn env_from_bits(env_bits: u64) -> Result<Option<Vec<(OsString, OsString)>>, String> {
    let obj = obj_from_bits(env_bits);
    if obj.is_none() {
        return Ok(None);
    }
    let Some(ptr) = obj.as_ptr() else {
        return Err("env must be a dict".to_string());
    };
    unsafe {
        if object_type_id(ptr) != TYPE_ID_DICT {
            return Err("env must be a dict".to_string());
        }
        let order = dict_order(ptr);
        let mut out = Vec::with_capacity(order.len() / 2);
        let mut idx = 0;
        while idx + 1 < order.len() {
            let key_bits = order[idx];
            let val_bits = order[idx + 1];
            out.push((
                os_string_from_bits(key_bits)?,
                os_string_from_bits(val_bits)?,
            ));
            idx += 2;
        }
        Ok(Some(out))
    }
}

#[cfg(unix)]
type LibcSocket = c_int;
#[cfg(windows)]
type LibcSocket = libc::SOCKET;

#[cfg(unix)]
fn libc_socket(fd: RawFd) -> LibcSocket {
    fd
}
#[cfg(windows)]
fn libc_socket(fd: RawSocket) -> LibcSocket {
    fd as LibcSocket
}

#[cfg(unix)]
fn socket_is_acceptor(socket: &Socket) -> bool {
    let fd = socket.as_raw_fd();
    let mut val: c_int = 0;
    let mut len = std::mem::size_of::<c_int>() as libc::socklen_t;
    let ret = unsafe {
        libc::getsockopt(
            fd,
            libc::SOL_SOCKET,
            libc::SO_ACCEPTCONN,
            &mut val as *mut _ as *mut c_void,
            &mut len,
        )
    };
    ret == 0 && val != 0
}

#[cfg(windows)]
fn socket_is_acceptor(_socket: &Socket) -> bool {
    false
}

#[cfg(unix)]
fn with_sockref<T, F>(fd: RawFd, f: F) -> T
where
    F: FnOnce(SockRef<'_>) -> T,
{
    let borrowed = unsafe { BorrowedFd::borrow_raw(fd) };
    let sock_ref = SockRef::from(&borrowed);
    f(sock_ref)
}

#[cfg(windows)]
fn with_sockref<T, F>(socket: RawSocket, f: F) -> T
where
    F: FnOnce(SockRef<'_>) -> T,
{
    let borrowed = unsafe { BorrowedSocket::borrow_raw(socket) };
    let sock_ref = SockRef::from(&borrowed);
    f(sock_ref)
}

#[cfg(unix)]
fn take_error_raw(fd: RawFd) -> std::io::Result<Option<std::io::Error>> {
    with_sockref(fd, |sock_ref| sock_ref.take_error())
}

#[cfg(windows)]
fn take_error_raw(socket: RawSocket) -> std::io::Result<Option<std::io::Error>> {
    with_sockref(socket, |sock_ref| sock_ref.take_error())
}

#[cfg(unix)]
fn take_error_mio<T: AsRawFd>(sock: &T) -> std::io::Result<Option<std::io::Error>> {
    take_error_raw(sock.as_raw_fd())
}

#[cfg(windows)]
fn take_error_mio<T: AsRawSocket>(sock: &T) -> std::io::Result<Option<std::io::Error>> {
    take_error_raw(sock.as_raw_socket())
}

#[cfg(not(target_arch = "wasm32"))]
#[no_mangle]
pub extern "C" fn molt_socket_new(
    family_bits: u64,
    type_bits: u64,
    proto_bits: u64,
    fileno_bits: u64,
) -> u64 {
    if require_net_capability::<u64>(&["net", "net.connect", "net.listen", "net.bind"]).is_err() {
        return MoltObject::none().bits();
    }
    let family = match to_i64(obj_from_bits(family_bits)) {
        Some(val) => val as i32,
        None => return raise_exception::<_>("TypeError", "family must be int"),
    };
    let sock_type = match to_i64(obj_from_bits(type_bits)) {
        Some(val) => val as i32,
        None => return raise_exception::<_>("TypeError", "type must be int"),
    };
    let proto = to_i64(obj_from_bits(proto_bits)).unwrap_or(0) as i32;
    let fileno = if obj_from_bits(fileno_bits).is_none() {
        None
    } else {
        match to_i64(obj_from_bits(fileno_bits)) {
            Some(val) => Some(val),
            None => return raise_exception::<_>("TypeError", "fileno must be int or None"),
        }
    };
    let domain = match family {
        val if val == libc::AF_INET => Domain::IPV4,
        val if val == libc::AF_INET6 => Domain::IPV6,
        #[cfg(unix)]
        val if val == libc::AF_UNIX => Domain::UNIX,
        _ => {
            return raise_os_error_errno::<u64>(
                libc::EAFNOSUPPORT as i64,
                "address family not supported",
            );
        }
    };
    #[cfg(unix)]
    let base_type = sock_type & !(SOCK_NONBLOCK_FLAG | SOCK_CLOEXEC_FLAG);
    #[cfg(not(unix))]
    let base_type = sock_type;
    let socket_type = match base_type {
        val if val == libc::SOCK_STREAM => Type::STREAM,
        val if val == libc::SOCK_DGRAM => Type::DGRAM,
        val if val == libc::SOCK_RAW => Type::from(val),
        _ => {
            return raise_os_error_errno::<u64>(libc::EPROTOTYPE as i64, "unsupported socket type");
        }
    };
    let mut socket = match fileno {
        Some(raw) => unsafe {
            if raw < 0 {
                return raise_os_error_errno::<u64>(libc::EBADF as i64, "bad file descriptor");
            }
            #[cfg(unix)]
            {
                Socket::from_raw_fd(raw as RawFd)
            }
            #[cfg(windows)]
            {
                Socket::from_raw_socket(raw as RawSocket)
            }
        },
        None => match Socket::new(domain, socket_type, Some(Protocol::from(proto))) {
            Ok(sock) => sock,
            Err(err) => return raise_os_error::<u64>(err, "socket"),
        },
    };
    if let Err(err) = socket.set_nonblocking(true) {
        return raise_os_error::<u64>(err, "socket");
    }
    let timeout = {
        #[cfg(unix)]
        {
            if (sock_type & SOCK_NONBLOCK_FLAG) != 0 {
                Some(Duration::ZERO)
            } else {
                None
            }
        }
        #[cfg(not(unix))]
        {
            None
        }
    };
    let mut connect_pending = false;
    let kind = if base_type == libc::SOCK_DGRAM {
        #[cfg(unix)]
        if family == libc::AF_UNIX {
            let raw_fd = socket.into_raw_fd();
            let std_sock = unsafe { std::os::unix::net::UnixDatagram::from_raw_fd(raw_fd) };
            if let Err(err) = std_sock.set_nonblocking(true) {
                return raise_os_error::<u64>(err, "socket");
            }
            MoltSocketKind::UnixDatagram(mio::net::UnixDatagram::from_std(std_sock))
        } else {
            let std_sock: std::net::UdpSocket = socket.into();
            if let Err(err) = std_sock.set_nonblocking(true) {
                return raise_os_error::<u64>(err, "socket");
            }
            MoltSocketKind::UdpSocket(mio::net::UdpSocket::from_std(std_sock))
        }
        #[cfg(not(unix))]
        {
            let std_sock: std::net::UdpSocket = socket.into();
            if let Err(err) = std_sock.set_nonblocking(true) {
                return raise_os_error::<u64>(err, "socket");
            }
            MoltSocketKind::UdpSocket(mio::net::UdpSocket::from_std(std_sock))
        }
    } else if let Some(_raw) = fileno {
        let acceptor = socket_is_acceptor(&socket);
        if acceptor {
            #[cfg(unix)]
            if family == libc::AF_UNIX {
                let raw_fd = socket.into_raw_fd();
                let std_listener = unsafe { std::os::unix::net::UnixListener::from_raw_fd(raw_fd) };
                if let Err(err) = std_listener.set_nonblocking(true) {
                    return raise_os_error::<u64>(err, "socket");
                }
                MoltSocketKind::UnixListener(mio::net::UnixListener::from_std(std_listener))
            } else {
                let std_listener: std::net::TcpListener = socket.into();
                if let Err(err) = std_listener.set_nonblocking(true) {
                    return raise_os_error::<u64>(err, "socket");
                }
                MoltSocketKind::TcpListener(mio::net::TcpListener::from_std(std_listener))
            }
            #[cfg(not(unix))]
            {
                let std_listener: std::net::TcpListener = socket.into();
                if let Err(err) = std_listener.set_nonblocking(true) {
                    return raise_os_error::<u64>(err, "socket");
                }
                MoltSocketKind::TcpListener(mio::net::TcpListener::from_std(std_listener))
            }
        } else {
            #[cfg(unix)]
            if family == libc::AF_UNIX {
                let raw_fd = socket.into_raw_fd();
                let std_stream = unsafe { std::os::unix::net::UnixStream::from_raw_fd(raw_fd) };
                if let Err(err) = std_stream.set_nonblocking(true) {
                    return raise_os_error::<u64>(err, "socket");
                }
                MoltSocketKind::UnixStream(mio::net::UnixStream::from_std(std_stream))
            } else {
                let std_stream: std::net::TcpStream = socket.into();
                if let Err(err) = std_stream.set_nonblocking(true) {
                    return raise_os_error::<u64>(err, "socket");
                }
                MoltSocketKind::TcpStream(mio::net::TcpStream::from_std(std_stream))
            }
            #[cfg(not(unix))]
            {
                let std_stream: std::net::TcpStream = socket.into();
                if let Err(err) = std_stream.set_nonblocking(true) {
                    return raise_os_error::<u64>(err, "socket");
                }
                MoltSocketKind::TcpStream(mio::net::TcpStream::from_std(std_stream))
            }
        }
    } else {
        MoltSocketKind::Pending(socket)
    };
    let socket = Box::new(MoltSocket {
        inner: Mutex::new(MoltSocketInner {
            kind,
            family,
            sock_type: base_type,
            proto,
            connect_pending,
        }),
        timeout: Mutex::new(timeout),
        closed: AtomicBool::new(false),
        refs: AtomicUsize::new(1),
    });
    let socket_ptr = Box::into_raw(socket) as *mut u8;
    socket_register_fd(socket_ptr);
    bits_from_ptr(socket_ptr)
}

#[cfg(not(target_arch = "wasm32"))]
#[no_mangle]
pub unsafe extern "C" fn molt_socket_close(sock_bits: u64) -> u64 {
    let socket_ptr = ptr_from_bits(sock_bits);
    if socket_ptr.is_null() {
        return MoltObject::none().bits();
    }
    let socket = &*(socket_ptr as *mut MoltSocket);
    if socket.closed.load(AtomicOrdering::Relaxed) {
        return MoltObject::none().bits();
    }
    socket_unregister_fd(socket_ptr);
    runtime_state().io_poller().deregister_socket(socket_ptr);
    socket.closed.store(true, AtomicOrdering::Relaxed);
    {
        let mut guard = socket.inner.lock().unwrap();
        guard.kind = MoltSocketKind::Closed;
    }
    MoltObject::none().bits()
}

#[cfg(not(target_arch = "wasm32"))]
#[no_mangle]
pub unsafe extern "C" fn molt_socket_drop(sock_bits: u64) {
    let socket_ptr = ptr_from_bits(sock_bits);
    if socket_ptr.is_null() {
        return;
    }
    let socket = &*(socket_ptr as *mut MoltSocket);
    if !socket.closed.load(AtomicOrdering::Relaxed) {
        let _ = molt_socket_close(sock_bits);
    }
    socket_ref_dec(socket_ptr);
}

#[cfg(not(target_arch = "wasm32"))]
#[no_mangle]
pub unsafe extern "C" fn molt_socket_clone(sock_bits: u64) -> u64 {
    let socket_ptr = ptr_from_bits(sock_bits);
    if socket_ptr.is_null() {
        return MoltObject::none().bits();
    }
    socket_ref_inc(socket_ptr);
    sock_bits
}

#[cfg(target_arch = "wasm32")]
#[no_mangle]
pub extern "C" fn molt_socket_clone(_sock_bits: u64) -> u64 {
    return raise_exception::<_>("RuntimeError", "socket clone unsupported on wasm");
}

#[cfg(not(target_arch = "wasm32"))]
#[no_mangle]
pub unsafe extern "C" fn molt_socket_fileno(sock_bits: u64) -> u64 {
    let socket_ptr = ptr_from_bits(sock_bits);
    if socket_ptr.is_null() {
        return MoltObject::from_int(-1).bits();
    }
    let socket = &*(socket_ptr as *mut MoltSocket);
    if socket.closed.load(AtomicOrdering::Relaxed) {
        return MoltObject::from_int(-1).bits();
    }
    let guard = socket.inner.lock().unwrap();
    #[cfg(unix)]
    let fd = match &guard.kind {
        MoltSocketKind::Pending(sock) => sock.as_raw_fd() as i64,
        MoltSocketKind::TcpStream(sock) => sock.as_raw_fd() as i64,
        MoltSocketKind::TcpListener(sock) => sock.as_raw_fd() as i64,
        MoltSocketKind::UdpSocket(sock) => sock.as_raw_fd() as i64,
        #[cfg(unix)]
        MoltSocketKind::UnixStream(sock) => sock.as_raw_fd() as i64,
        #[cfg(unix)]
        MoltSocketKind::UnixListener(sock) => sock.as_raw_fd() as i64,
        #[cfg(unix)]
        MoltSocketKind::UnixDatagram(sock) => sock.as_raw_fd() as i64,
        MoltSocketKind::Closed => -1,
    };
    #[cfg(windows)]
    let fd = match &guard.kind {
        MoltSocketKind::Pending(sock) => sock.as_raw_socket() as i64,
        MoltSocketKind::TcpStream(sock) => sock.as_raw_socket() as i64,
        MoltSocketKind::TcpListener(sock) => sock.as_raw_socket() as i64,
        MoltSocketKind::UdpSocket(sock) => sock.as_raw_socket() as i64,
        MoltSocketKind::Closed => -1,
    };
    MoltObject::from_int(fd).bits()
}

#[cfg(not(target_arch = "wasm32"))]
#[no_mangle]
pub unsafe extern "C" fn molt_socket_gettimeout(sock_bits: u64) -> u64 {
    let socket_ptr = ptr_from_bits(sock_bits);
    if socket_ptr.is_null() {
        return MoltObject::none().bits();
    }
    let timeout = socket_timeout(socket_ptr);
    match timeout {
        None => MoltObject::none().bits(),
        Some(val) => MoltObject::from_float(val.as_secs_f64()).bits(),
    }
}

#[cfg(not(target_arch = "wasm32"))]
#[no_mangle]
pub unsafe extern "C" fn molt_socket_settimeout(sock_bits: u64, timeout_bits: u64) -> u64 {
    let socket_ptr = ptr_from_bits(sock_bits);
    if socket_ptr.is_null() {
        return MoltObject::none().bits();
    }
    let obj = obj_from_bits(timeout_bits);
    if obj.is_none() {
        socket_set_timeout(socket_ptr, None);
        return MoltObject::none().bits();
    }
    let Some(timeout) = to_f64(obj) else {
        return raise_exception::<_>("TypeError", "timeout must be float or None");
    };
    if !timeout.is_finite() || timeout < 0.0 {
        return raise_exception::<_>("ValueError", "timeout must be non-negative");
    }
    let duration = if timeout == 0.0 {
        Duration::ZERO
    } else {
        Duration::from_secs_f64(timeout)
    };
    socket_set_timeout(socket_ptr, Some(duration));
    MoltObject::none().bits()
}

#[cfg(not(target_arch = "wasm32"))]
#[no_mangle]
pub unsafe extern "C" fn molt_socket_setblocking(sock_bits: u64, flag_bits: u64) -> u64 {
    let socket_ptr = ptr_from_bits(sock_bits);
    if socket_ptr.is_null() {
        return MoltObject::none().bits();
    }
    let flag = obj_from_bits(flag_bits).as_bool().unwrap_or(false);
    if flag {
        socket_set_timeout(socket_ptr, None);
    } else {
        socket_set_timeout(socket_ptr, Some(Duration::ZERO));
    }
    MoltObject::none().bits()
}

#[cfg(not(target_arch = "wasm32"))]
#[no_mangle]
pub unsafe extern "C" fn molt_socket_getblocking(sock_bits: u64) -> u64 {
    let socket_ptr = ptr_from_bits(sock_bits);
    if socket_ptr.is_null() {
        return MoltObject::from_bool(false).bits();
    }
    let timeout = socket_timeout(socket_ptr);
    let blocking = match timeout {
        None => true,
        Some(val) if val == Duration::ZERO => false,
        Some(_) => true,
    };
    MoltObject::from_bool(blocking).bits()
}

#[cfg(not(target_arch = "wasm32"))]
#[no_mangle]
pub unsafe extern "C" fn molt_socket_bind(sock_bits: u64, addr_bits: u64) -> u64 {
    if require_net_capability::<u64>(&["net", "net.bind", "net"]).is_err() {
        return MoltObject::none().bits();
    }
    let socket_ptr = ptr_from_bits(sock_bits);
    if socket_ptr.is_null() {
        return MoltObject::none().bits();
    }
    let family = {
        let socket = &*(socket_ptr as *mut MoltSocket);
        let guard = socket.inner.lock().unwrap();
        guard.family
    };
    let sockaddr = match sockaddr_from_bits(addr_bits, family) {
        Ok(addr) => addr,
        Err(msg) => return raise_exception::<_>("TypeError", &msg),
    };
    let res = with_socket_mut(socket_ptr, |inner| match &inner.kind {
        MoltSocketKind::Pending(sock) => sock.bind(&sockaddr).map_err(|e| e),
        MoltSocketKind::TcpListener(_) | MoltSocketKind::TcpStream(_) => Err(std::io::Error::new(
            ErrorKind::InvalidInput,
            "socket already bound",
        )),
        MoltSocketKind::UdpSocket(sock) => {
            #[cfg(unix)]
            let raw = sock.as_raw_fd();
            #[cfg(windows)]
            let raw = sock.as_raw_socket();
            with_sockref(raw, |sock_ref| sock_ref.bind(&sockaddr))
        }
        #[cfg(unix)]
        MoltSocketKind::UnixStream(_) | MoltSocketKind::UnixListener(_) => Err(
            std::io::Error::new(ErrorKind::InvalidInput, "socket already bound"),
        ),
        #[cfg(unix)]
        MoltSocketKind::UnixDatagram(sock) => {
            #[cfg(unix)]
            let raw = sock.as_raw_fd();
            #[cfg(windows)]
            let raw = sock.as_raw_socket();
            with_sockref(raw, |sock_ref| sock_ref.bind(&sockaddr))
        }
        MoltSocketKind::Closed => Err(std::io::Error::new(
            ErrorKind::NotConnected,
            "socket closed",
        )),
    });
    match res {
        Ok(_) => MoltObject::none().bits(),
        Err(err) => raise_os_error::<u64>(err, "bind"),
    }
}

#[cfg(not(target_arch = "wasm32"))]
#[no_mangle]
pub unsafe extern "C" fn molt_socket_listen(sock_bits: u64, backlog_bits: u64) -> u64 {
    if require_net_capability::<u64>(&["net", "net.listen", "net"]).is_err() {
        return MoltObject::none().bits();
    }
    let socket_ptr = ptr_from_bits(sock_bits);
    if socket_ptr.is_null() {
        return MoltObject::none().bits();
    }
    let backlog = to_i64(obj_from_bits(backlog_bits)).unwrap_or(128).max(0) as i32;
    let res = with_socket_mut(socket_ptr, |inner| {
        match std::mem::replace(&mut inner.kind, MoltSocketKind::Closed) {
            MoltSocketKind::Pending(sock) => match sock.listen(backlog) {
                Ok(_) => {
                    #[cfg(unix)]
                    if inner.family == libc::AF_UNIX {
                        let raw_fd = sock.into_raw_fd();
                        let std_listener =
                            unsafe { std::os::unix::net::UnixListener::from_raw_fd(raw_fd) };
                        std_listener.set_nonblocking(true)?;
                        inner.kind = MoltSocketKind::UnixListener(
                            mio::net::UnixListener::from_std(std_listener),
                        );
                    } else {
                        let std_listener: std::net::TcpListener = sock.into();
                        std_listener.set_nonblocking(true)?;
                        inner.kind = MoltSocketKind::TcpListener(mio::net::TcpListener::from_std(
                            std_listener,
                        ));
                    }
                    #[cfg(not(unix))]
                    {
                        let std_listener: std::net::TcpListener = sock.into();
                        std_listener.set_nonblocking(true)?;
                        inner.kind = MoltSocketKind::TcpListener(mio::net::TcpListener::from_std(
                            std_listener,
                        ));
                    }
                    Ok(())
                }
                Err(err) => {
                    inner.kind = MoltSocketKind::Pending(sock);
                    Err(err)
                }
            },
            other => {
                inner.kind = other;
                Err(std::io::Error::new(
                    ErrorKind::InvalidInput,
                    "socket not in listenable state",
                ))
            }
        }
    });
    match res {
        Ok(_) => MoltObject::none().bits(),
        Err(err) => raise_os_error::<u64>(err, "listen"),
    }
}

#[cfg(not(target_arch = "wasm32"))]
#[no_mangle]
pub unsafe extern "C" fn molt_socket_accept(sock_bits: u64) -> u64 {
    if require_net_capability::<u64>(&["net", "net.listen", "net"]).is_err() {
        return MoltObject::none().bits();
    }
    let socket_ptr = ptr_from_bits(sock_bits);
    if socket_ptr.is_null() {
        return MoltObject::none().bits();
    }
    loop {
        let timeout = socket_timeout(socket_ptr);
        let (accepted_kind, addr_bits, family) = {
            let socket = &*(socket_ptr as *mut MoltSocket);
            let mut guard = socket.inner.lock().unwrap();
            match &mut guard.kind {
                MoltSocketKind::TcpListener(listener) => match listener.accept() {
                    Ok((stream, addr)) => (
                        MoltSocketKind::TcpStream(stream),
                        sockaddr_to_bits(&SockAddr::from(addr)),
                        guard.family,
                    ),
                    Err(err) => {
                        if err.kind() == ErrorKind::WouldBlock {
                            (MoltSocketKind::Closed, 0, guard.family)
                        } else {
                            return raise_os_error::<u64>(err, "accept");
                        }
                    }
                },
                #[cfg(unix)]
                MoltSocketKind::UnixListener(listener) => match listener.accept() {
                    Ok((stream, addr)) => {
                        let addr_bits = if let Some(path) = addr.as_pathname() {
                            let text = path.to_string_lossy();
                            let ptr = alloc_string(text.as_bytes());
                            if ptr.is_null() {
                                MoltObject::none().bits()
                            } else {
                                MoltObject::from_ptr(ptr).bits()
                            }
                        } else {
                            MoltObject::none().bits()
                        };
                        (MoltSocketKind::UnixStream(stream), addr_bits, guard.family)
                    }
                    Err(err) => {
                        if err.kind() == ErrorKind::WouldBlock {
                            (MoltSocketKind::Closed, 0, guard.family)
                        } else {
                            return raise_os_error::<u64>(err, "accept");
                        }
                    }
                },
                _ => {
                    return raise_os_error_errno::<u64>(libc::EINVAL as i64, "socket not listening")
                }
            }
        };
        if addr_bits == 0 {
            if let Err(wait_err) = socket_wait_ready(socket_ptr, IO_EVENT_READ) {
                if wait_err.kind() == ErrorKind::TimedOut {
                    return raise_exception::<u64>("TimeoutError", "timed out");
                }
                return raise_os_error::<u64>(wait_err, "accept");
            }
            continue;
        }
        let socket = Box::new(MoltSocket {
            inner: Mutex::new(MoltSocketInner {
                kind: accepted_kind,
                family,
                sock_type: libc::SOCK_STREAM,
                proto: 0,
                connect_pending: false,
            }),
            timeout: Mutex::new(timeout),
            closed: AtomicBool::new(false),
            refs: AtomicUsize::new(1),
        });
        let socket_ptr = Box::into_raw(socket) as *mut u8;
        socket_register_fd(socket_ptr);
        let handle_bits = bits_from_ptr(socket_ptr);
        let tuple_ptr = alloc_tuple(&[handle_bits, addr_bits]);
        if tuple_ptr.is_null() {
            return MoltObject::none().bits();
        }
        return MoltObject::from_ptr(tuple_ptr).bits();
    }
}

#[cfg(not(target_arch = "wasm32"))]
#[no_mangle]
pub unsafe extern "C" fn molt_socket_connect(sock_bits: u64, addr_bits: u64) -> u64 {
    if require_net_capability::<u64>(&["net", "net.connect", "net"]).is_err() {
        return MoltObject::none().bits();
    }
    let socket_ptr = ptr_from_bits(sock_bits);
    if socket_ptr.is_null() {
        return MoltObject::none().bits();
    }
    let family = {
        let socket = &*(socket_ptr as *mut MoltSocket);
        let guard = socket.inner.lock().unwrap();
        guard.family
    };
    let sockaddr = match sockaddr_from_bits(addr_bits, family) {
        Ok(addr) => addr,
        Err(msg) => return raise_exception::<_>("TypeError", &msg),
    };
    let timeout = socket_timeout(socket_ptr);
    let res = with_socket_mut(socket_ptr, |inner| {
        if inner.connect_pending {
            return Ok(true);
        }
        match std::mem::replace(&mut inner.kind, MoltSocketKind::Closed) {
            MoltSocketKind::Pending(sock) => match sock.connect(&sockaddr) {
                Ok(_) => {
                    #[cfg(unix)]
                    if inner.family == libc::AF_UNIX {
                        let raw_fd = sock.into_raw_fd();
                        let std_stream =
                            unsafe { std::os::unix::net::UnixStream::from_raw_fd(raw_fd) };
                        std_stream.set_nonblocking(true)?;
                        inner.kind =
                            MoltSocketKind::UnixStream(mio::net::UnixStream::from_std(std_stream));
                    } else {
                        let std_stream: std::net::TcpStream = sock.into();
                        std_stream.set_nonblocking(true)?;
                        inner.kind =
                            MoltSocketKind::TcpStream(mio::net::TcpStream::from_std(std_stream));
                    }
                    #[cfg(not(unix))]
                    {
                        let std_stream: std::net::TcpStream = sock.into();
                        std_stream.set_nonblocking(true)?;
                        inner.kind =
                            MoltSocketKind::TcpStream(mio::net::TcpStream::from_std(std_stream));
                    }
                    inner.connect_pending = false;
                    Ok(false)
                }
                Err(err) => {
                    if err.kind() == ErrorKind::WouldBlock
                        || err.raw_os_error() == Some(libc::EINPROGRESS)
                    {
                        #[cfg(unix)]
                        if inner.family == libc::AF_UNIX {
                            let raw_fd = sock.into_raw_fd();
                            let std_stream =
                                unsafe { std::os::unix::net::UnixStream::from_raw_fd(raw_fd) };
                            std_stream.set_nonblocking(true)?;
                            inner.kind = MoltSocketKind::UnixStream(
                                mio::net::UnixStream::from_std(std_stream),
                            );
                        } else {
                            let std_stream: std::net::TcpStream = sock.into();
                            std_stream.set_nonblocking(true)?;
                            inner.kind = MoltSocketKind::TcpStream(mio::net::TcpStream::from_std(
                                std_stream,
                            ));
                        }
                        #[cfg(not(unix))]
                        {
                            let std_stream: std::net::TcpStream = sock.into();
                            std_stream.set_nonblocking(true)?;
                            inner.kind = MoltSocketKind::TcpStream(mio::net::TcpStream::from_std(
                                std_stream,
                            ));
                        }
                        inner.connect_pending = true;
                    } else {
                        inner.kind = MoltSocketKind::Pending(sock);
                    }
                    Err(err)
                }
            },
            other => {
                inner.kind = other;
                Err(std::io::Error::new(
                    ErrorKind::InvalidInput,
                    "socket already connected",
                ))
            }
        }
    });
    match res {
        Ok(pending) => {
            if pending {
                if let Err(err) = socket_wait_ready(socket_ptr, IO_EVENT_WRITE) {
                    if err.kind() == ErrorKind::TimedOut {
                        return raise_exception::<u64>("TimeoutError", "timed out");
                    }
                    if err.kind() == ErrorKind::WouldBlock {
                        return raise_os_error_errno::<u64>(
                            libc::EINPROGRESS as i64,
                            "operation in progress",
                        );
                    }
                    return raise_os_error::<u64>(err, "connect");
                }
                let err = with_socket_mut(socket_ptr, |inner| match &inner.kind {
                    MoltSocketKind::TcpStream(stream) => take_error_mio(stream),
                    #[cfg(unix)]
                    MoltSocketKind::UnixStream(stream) => take_error_mio(stream),
                    _ => Ok(None),
                });
                match err {
                    Ok(None) => MoltObject::none().bits(),
                    Ok(Some(err)) => raise_os_error::<u64>(err, "connect"),
                    Err(err) => raise_os_error::<u64>(err, "connect"),
                }
            } else {
                MoltObject::none().bits()
            }
        }
        Err(err)
            if err.kind() == ErrorKind::WouldBlock
                || err.raw_os_error() == Some(libc::EINPROGRESS) =>
        {
            match timeout {
                Some(val) if val == Duration::ZERO => {
                    raise_os_error_errno::<u64>(libc::EINPROGRESS as i64, "operation in progress")
                }
                _ => {
                    if let Err(wait_err) = socket_wait_ready(socket_ptr, IO_EVENT_WRITE) {
                        if wait_err.kind() == ErrorKind::TimedOut {
                            return raise_exception::<u64>("TimeoutError", "timed out");
                        }
                        return raise_os_error::<u64>(wait_err, "connect");
                    }
                    let err = with_socket_mut(socket_ptr, |inner| match &inner.kind {
                        MoltSocketKind::TcpStream(stream) => take_error_mio(stream),
                        #[cfg(unix)]
                        MoltSocketKind::UnixStream(stream) => take_error_mio(stream),
                        _ => Ok(None),
                    });
                    match err {
                        Ok(None) => MoltObject::none().bits(),
                        Ok(Some(err)) => raise_os_error::<u64>(err, "connect"),
                        Err(err) => raise_os_error::<u64>(err, "connect"),
                    }
                }
            }
        }
        Err(err) => raise_os_error::<u64>(err, "connect"),
    }
}

#[cfg(not(target_arch = "wasm32"))]
#[no_mangle]
pub unsafe extern "C" fn molt_socket_connect_ex(sock_bits: u64, addr_bits: u64) -> u64 {
    if require_net_capability::<u64>(&["net", "net.connect", "net"]).is_err() {
        return MoltObject::none().bits();
    }
    let socket_ptr = ptr_from_bits(sock_bits);
    if socket_ptr.is_null() {
        return MoltObject::from_int(libc::EBADF as i64).bits();
    }
    let family = {
        let socket = &*(socket_ptr as *mut MoltSocket);
        let guard = socket.inner.lock().unwrap();
        guard.family
    };
    let sockaddr = match sockaddr_from_bits(addr_bits, family) {
        Ok(addr) => addr,
        Err(_msg) => return MoltObject::from_int(libc::EAFNOSUPPORT as i64).bits(),
    };
    let res = with_socket_mut(socket_ptr, |inner| {
        if inner.connect_pending {
            let err = match &inner.kind {
                MoltSocketKind::TcpStream(stream) => take_error_mio(stream),
                #[cfg(unix)]
                MoltSocketKind::UnixStream(stream) => take_error_mio(stream),
                _ => Ok(None),
            };
            return match err {
                Ok(None) => {
                    inner.connect_pending = false;
                    Ok(0)
                }
                Ok(Some(err)) => {
                    inner.connect_pending = false;
                    Ok(err.raw_os_error().unwrap_or(libc::EIO) as i64)
                }
                Err(err) => Ok(err.raw_os_error().unwrap_or(libc::EIO) as i64),
            };
        }
        match std::mem::replace(&mut inner.kind, MoltSocketKind::Closed) {
            MoltSocketKind::Pending(sock) => match sock.connect(&sockaddr) {
                Ok(_) => {
                    #[cfg(unix)]
                    if inner.family == libc::AF_UNIX {
                        let raw_fd = sock.into_raw_fd();
                        let std_stream =
                            unsafe { std::os::unix::net::UnixStream::from_raw_fd(raw_fd) };
                        std_stream.set_nonblocking(true)?;
                        inner.kind =
                            MoltSocketKind::UnixStream(mio::net::UnixStream::from_std(std_stream));
                    } else {
                        let std_stream: std::net::TcpStream = sock.into();
                        std_stream.set_nonblocking(true)?;
                        inner.kind =
                            MoltSocketKind::TcpStream(mio::net::TcpStream::from_std(std_stream));
                    }
                    #[cfg(not(unix))]
                    {
                        let std_stream: std::net::TcpStream = sock.into();
                        std_stream.set_nonblocking(true)?;
                        inner.kind =
                            MoltSocketKind::TcpStream(mio::net::TcpStream::from_std(std_stream));
                    }
                    inner.connect_pending = false;
                    Ok(0)
                }
                Err(err) => {
                    let errno = err.raw_os_error().unwrap_or(libc::EIO);
                    if err.kind() == ErrorKind::WouldBlock
                        || errno == libc::EINPROGRESS
                        || errno == libc::EALREADY
                    {
                        #[cfg(unix)]
                        if inner.family == libc::AF_UNIX {
                            let raw_fd = sock.into_raw_fd();
                            let std_stream =
                                unsafe { std::os::unix::net::UnixStream::from_raw_fd(raw_fd) };
                            std_stream.set_nonblocking(true)?;
                            inner.kind = MoltSocketKind::UnixStream(
                                mio::net::UnixStream::from_std(std_stream),
                            );
                        } else {
                            let std_stream: std::net::TcpStream = sock.into();
                            std_stream.set_nonblocking(true)?;
                            inner.kind = MoltSocketKind::TcpStream(mio::net::TcpStream::from_std(
                                std_stream,
                            ));
                        }
                        #[cfg(not(unix))]
                        {
                            let std_stream: std::net::TcpStream = sock.into();
                            std_stream.set_nonblocking(true)?;
                            inner.kind = MoltSocketKind::TcpStream(mio::net::TcpStream::from_std(
                                std_stream,
                            ));
                        }
                        inner.connect_pending = true;
                    } else {
                        inner.kind = MoltSocketKind::Pending(sock);
                    }
                    Ok(errno as i64)
                }
            },
            other => {
                inner.kind = other;
                Ok(libc::EISCONN as i64)
            }
        }
    });
    match res {
        Ok(val) => MoltObject::from_int(val).bits(),
        Err(err) => MoltObject::from_int(err.raw_os_error().unwrap_or(libc::EIO) as i64).bits(),
    }
}

#[cfg(not(target_arch = "wasm32"))]
#[no_mangle]
pub unsafe extern "C" fn molt_socket_recv(sock_bits: u64, size_bits: u64, flags_bits: u64) -> u64 {
    let socket_ptr = ptr_from_bits(sock_bits);
    if socket_ptr.is_null() {
        return MoltObject::none().bits();
    }
    let size = to_i64(obj_from_bits(size_bits)).unwrap_or(0).max(0) as usize;
    let flags = to_i64(obj_from_bits(flags_bits)).unwrap_or(0) as i32;
    #[cfg(unix)]
    let dontwait = (flags & libc::MSG_DONTWAIT) != 0;
    #[cfg(not(unix))]
    let dontwait = false;
    if size == 0 {
        let ptr = alloc_bytes(&[]);
        if ptr.is_null() {
            return MoltObject::none().bits();
        }
        return MoltObject::from_ptr(ptr).bits();
    }
    let mut buf = vec![0u8; size];
    loop {
        let res = with_socket_mut(socket_ptr, |inner| {
            #[cfg(unix)]
            let fd = inner
                .raw_fd()
                .ok_or_else(|| std::io::Error::new(ErrorKind::NotConnected, "socket closed"))?;
            #[cfg(windows)]
            let fd = inner
                .raw_socket()
                .ok_or_else(|| std::io::Error::new(ErrorKind::NotConnected, "socket closed"))?;
            let ret = unsafe {
                libc::recv(
                    libc_socket(fd),
                    buf.as_mut_ptr() as *mut c_void,
                    buf.len(),
                    flags,
                )
            };
            if ret >= 0 {
                Ok(ret as usize)
            } else {
                Err(std::io::Error::last_os_error())
            }
        });
        match res {
            Ok(n) => {
                let ptr = alloc_bytes(&buf[..n]);
                if ptr.is_null() {
                    return MoltObject::none().bits();
                }
                return MoltObject::from_ptr(ptr).bits();
            }
            Err(err) if err.kind() == ErrorKind::WouldBlock => {
                if dontwait {
                    return raise_os_error::<u64>(err, "recv");
                }
                if let Err(wait_err) = socket_wait_ready(socket_ptr, IO_EVENT_READ) {
                    if wait_err.kind() == ErrorKind::TimedOut {
                        return raise_exception::<u64>("TimeoutError", "timed out");
                    }
                    return raise_os_error::<u64>(wait_err, "recv");
                }
                continue;
            }
            Err(err) => return raise_os_error::<u64>(err, "recv"),
        }
    }
}

#[cfg(not(target_arch = "wasm32"))]
#[no_mangle]
pub unsafe extern "C" fn molt_socket_recv_into(
    sock_bits: u64,
    buffer_bits: u64,
    size_bits: u64,
    flags_bits: u64,
) -> u64 {
    let socket_ptr = ptr_from_bits(sock_bits);
    if socket_ptr.is_null() {
        return MoltObject::from_int(0).bits();
    }
    let buffer_obj = obj_from_bits(buffer_bits);
    let buffer_ptr = buffer_obj.as_ptr();
    if buffer_ptr.is_none() {
        return raise_exception::<_>("TypeError", "recv_into requires a writable buffer");
    }
    let buffer_ptr = buffer_ptr.unwrap();
    let size = to_i64(obj_from_bits(size_bits)).unwrap_or(-1);
    let mut target_len = 0usize;
    let mut use_memoryview = false;
    let type_id = object_type_id(buffer_ptr);
    if type_id == TYPE_ID_BYTEARRAY {
        target_len = bytearray_len(buffer_ptr);
    } else if type_id == TYPE_ID_MEMORYVIEW {
        if memoryview_readonly(buffer_ptr) {
            return raise_exception::<_>("TypeError", "recv_into requires a writable buffer");
        }
        target_len = memoryview_len(buffer_ptr);
        use_memoryview = true;
    } else {
        return raise_exception::<_>("TypeError", "recv_into requires a writable buffer");
    }
    let size = if size < 0 {
        target_len
    } else {
        (size as usize).min(target_len)
    };
    let flags = to_i64(obj_from_bits(flags_bits)).unwrap_or(0) as i32;
    #[cfg(unix)]
    let dontwait = (flags & libc::MSG_DONTWAIT) != 0;
    #[cfg(not(unix))]
    let dontwait = false;
    loop {
        let res = with_socket_mut(socket_ptr, |inner| {
            #[cfg(unix)]
            let fd = inner
                .raw_fd()
                .ok_or_else(|| std::io::Error::new(ErrorKind::NotConnected, "socket closed"))?;
            #[cfg(windows)]
            let fd = inner
                .raw_socket()
                .ok_or_else(|| std::io::Error::new(ErrorKind::NotConnected, "socket closed"))?;
            if use_memoryview {
                // TODO(perf, owner:runtime, milestone:RT2, priority:P2, status:planned): write directly into contiguous memoryview buffers to avoid temp allocation/copy on recv_into.
                let mut tmp = vec![0u8; size];
                let ret = unsafe {
                    libc::recv(
                        libc_socket(fd),
                        tmp.as_mut_ptr() as *mut c_void,
                        tmp.len(),
                        flags,
                    )
                };
                if ret >= 0 {
                    Ok((ret as usize, Some(tmp)))
                } else {
                    Err(std::io::Error::last_os_error())
                }
            } else {
                let buf = bytearray_vec(buffer_ptr);
                let ret = unsafe {
                    libc::recv(
                        libc_socket(fd),
                        buf.as_mut_ptr() as *mut c_void,
                        size,
                        flags,
                    )
                };
                if ret >= 0 {
                    Ok((ret as usize, None))
                } else {
                    Err(std::io::Error::last_os_error())
                }
            }
        });
        match res {
            Ok((n, tmp)) => {
                if use_memoryview {
                    let bytes = tmp.as_ref().map(|v| &v[..n]).unwrap_or(&[]);
                    if let Err(msg) = memoryview_write_bytes(buffer_ptr, bytes) {
                        return raise_exception::<u64>("TypeError", &msg);
                    }
                }
                return MoltObject::from_int(n as i64).bits();
            }
            Err(err) if err.kind() == ErrorKind::WouldBlock => {
                if dontwait {
                    return raise_os_error::<u64>(err, "recv_into");
                }
                if let Err(wait_err) = socket_wait_ready(socket_ptr, IO_EVENT_READ) {
                    if wait_err.kind() == ErrorKind::TimedOut {
                        return raise_exception::<u64>("TimeoutError", "timed out");
                    }
                    return raise_os_error::<u64>(wait_err, "recv_into");
                }
                continue;
            }
            Err(err) => return raise_os_error::<u64>(err, "recv_into"),
        }
    }
}

#[cfg(not(target_arch = "wasm32"))]
#[no_mangle]
pub unsafe extern "C" fn molt_socket_send(sock_bits: u64, data_bits: u64, flags_bits: u64) -> u64 {
    let socket_ptr = ptr_from_bits(sock_bits);
    if socket_ptr.is_null() {
        return MoltObject::from_int(0).bits();
    }
    let flags = to_i64(obj_from_bits(flags_bits)).unwrap_or(0) as i32;
    #[cfg(unix)]
    let dontwait = (flags & libc::MSG_DONTWAIT) != 0;
    #[cfg(not(unix))]
    let dontwait = false;
    let send_data = match send_data_from_bits(data_bits) {
        Ok(data) => data,
        Err(msg) => return raise_exception::<_>("TypeError", &msg),
    };
    let (data_ptr, data_len, owned): (*const u8, usize, Option<Vec<u8>>) = match send_data {
        SendData::Borrowed(ptr, len) => (ptr, len, None),
        SendData::Owned(vec) => {
            let ptr = vec.as_ptr();
            let len = vec.len();
            (ptr, len, Some(vec))
        }
    };
    let _owned_guard = owned;
    if data_len == 0 {
        return MoltObject::from_int(0).bits();
    }
    loop {
        let res = with_socket_mut(socket_ptr, |inner| {
            #[cfg(unix)]
            let fd = inner
                .raw_fd()
                .ok_or_else(|| std::io::Error::new(ErrorKind::NotConnected, "socket closed"))?;
            #[cfg(windows)]
            let fd = inner
                .raw_socket()
                .ok_or_else(|| std::io::Error::new(ErrorKind::NotConnected, "socket closed"))?;
            let ret =
                unsafe { libc::send(libc_socket(fd), data_ptr as *const c_void, data_len, flags) };
            if ret >= 0 {
                Ok(ret as usize)
            } else {
                Err(std::io::Error::last_os_error())
            }
        });
        match res {
            Ok(n) => return MoltObject::from_int(n as i64).bits(),
            Err(err) if err.kind() == ErrorKind::WouldBlock => {
                if dontwait {
                    return raise_os_error::<u64>(err, "send");
                }
                if let Err(wait_err) = socket_wait_ready(socket_ptr, IO_EVENT_WRITE) {
                    if wait_err.kind() == ErrorKind::TimedOut {
                        return raise_exception::<u64>("TimeoutError", "timed out");
                    }
                    return raise_os_error::<u64>(wait_err, "send");
                }
                continue;
            }
            Err(err) => return raise_os_error::<u64>(err, "send"),
        }
    }
}

#[cfg(not(target_arch = "wasm32"))]
#[no_mangle]
pub unsafe extern "C" fn molt_socket_sendall(
    sock_bits: u64,
    data_bits: u64,
    flags_bits: u64,
) -> u64 {
    let socket_ptr = ptr_from_bits(sock_bits);
    if socket_ptr.is_null() {
        return MoltObject::none().bits();
    }
    let flags = to_i64(obj_from_bits(flags_bits)).unwrap_or(0) as i32;
    #[cfg(unix)]
    let dontwait = (flags & libc::MSG_DONTWAIT) != 0;
    #[cfg(not(unix))]
    let dontwait = false;
    let send_data = match send_data_from_bits(data_bits) {
        Ok(data) => data,
        Err(msg) => return raise_exception::<_>("TypeError", &msg),
    };
    let (data_ptr, data_len, owned): (*const u8, usize, Option<Vec<u8>>) = match send_data {
        SendData::Borrowed(ptr, len) => (ptr, len, None),
        SendData::Owned(vec) => {
            let ptr = vec.as_ptr();
            let len = vec.len();
            (ptr, len, Some(vec))
        }
    };
    let _owned_guard = owned;
    let mut offset = 0usize;
    while offset < data_len {
        let slice_ptr = unsafe { data_ptr.add(offset) };
        let slice_len = data_len - offset;
        let res = with_socket_mut(socket_ptr, |inner| {
            #[cfg(unix)]
            let fd = inner
                .raw_fd()
                .ok_or_else(|| std::io::Error::new(ErrorKind::NotConnected, "socket closed"))?;
            #[cfg(windows)]
            let fd = inner
                .raw_socket()
                .ok_or_else(|| std::io::Error::new(ErrorKind::NotConnected, "socket closed"))?;
            let ret = unsafe {
                libc::send(
                    libc_socket(fd),
                    slice_ptr as *const c_void,
                    slice_len,
                    flags,
                )
            };
            if ret >= 0 {
                Ok(ret as usize)
            } else {
                Err(std::io::Error::last_os_error())
            }
        });
        match res {
            Ok(0) => return raise_os_error_errno::<u64>(libc::EPIPE as i64, "broken pipe"),
            Ok(n) => offset += n,
            Err(err) if err.kind() == ErrorKind::WouldBlock => {
                if dontwait {
                    return raise_os_error::<u64>(err, "sendall");
                }
                if let Err(wait_err) = socket_wait_ready(socket_ptr, IO_EVENT_WRITE) {
                    if wait_err.kind() == ErrorKind::TimedOut {
                        return raise_exception::<u64>("TimeoutError", "timed out");
                    }
                    return raise_os_error::<u64>(wait_err, "sendall");
                }
            }
            Err(err) => return raise_os_error::<u64>(err, "sendall"),
        }
    }
    MoltObject::none().bits()
}

#[cfg(not(target_arch = "wasm32"))]
#[no_mangle]
pub unsafe extern "C" fn molt_socket_sendto(
    sock_bits: u64,
    data_bits: u64,
    flags_bits: u64,
    addr_bits: u64,
) -> u64 {
    let socket_ptr = ptr_from_bits(sock_bits);
    if socket_ptr.is_null() {
        return MoltObject::from_int(0).bits();
    }
    let family = {
        let socket = &*(socket_ptr as *mut MoltSocket);
        let guard = socket.inner.lock().unwrap();
        guard.family
    };
    let sockaddr = match sockaddr_from_bits(addr_bits, family) {
        Ok(addr) => addr,
        Err(msg) => return raise_exception::<_>("TypeError", &msg),
    };
    let flags = to_i64(obj_from_bits(flags_bits)).unwrap_or(0) as i32;
    #[cfg(unix)]
    let dontwait = (flags & libc::MSG_DONTWAIT) != 0;
    #[cfg(not(unix))]
    let dontwait = false;
    let send_data = match send_data_from_bits(data_bits) {
        Ok(data) => data,
        Err(msg) => return raise_exception::<_>("TypeError", &msg),
    };
    let (data_ptr, data_len, owned): (*const u8, usize, Option<Vec<u8>>) = match send_data {
        SendData::Borrowed(ptr, len) => (ptr, len, None),
        SendData::Owned(vec) => {
            let ptr = vec.as_ptr();
            let len = vec.len();
            (ptr, len, Some(vec))
        }
    };
    let _owned_guard = owned;
    loop {
        let res = with_socket_mut(socket_ptr, |inner| {
            #[cfg(unix)]
            let fd = inner
                .raw_fd()
                .ok_or_else(|| std::io::Error::new(ErrorKind::NotConnected, "socket closed"))?;
            #[cfg(windows)]
            let fd = inner
                .raw_socket()
                .ok_or_else(|| std::io::Error::new(ErrorKind::NotConnected, "socket closed"))?;
            let ret = unsafe {
                libc::sendto(
                    libc_socket(fd),
                    data_ptr as *const c_void,
                    data_len,
                    flags,
                    sockaddr.as_ptr(),
                    sockaddr.len(),
                )
            };
            if ret >= 0 {
                Ok(ret as usize)
            } else {
                Err(std::io::Error::last_os_error())
            }
        });
        match res {
            Ok(n) => return MoltObject::from_int(n as i64).bits(),
            Err(err) if err.kind() == ErrorKind::WouldBlock => {
                if dontwait {
                    return raise_os_error::<u64>(err, "sendto");
                }
                if let Err(wait_err) = socket_wait_ready(socket_ptr, IO_EVENT_WRITE) {
                    if wait_err.kind() == ErrorKind::TimedOut {
                        return raise_exception::<u64>("TimeoutError", "timed out");
                    }
                    return raise_os_error::<u64>(wait_err, "sendto");
                }
                continue;
            }
            Err(err) => return raise_os_error::<u64>(err, "sendto"),
        }
    }
}

#[cfg(not(target_arch = "wasm32"))]
#[no_mangle]
pub unsafe extern "C" fn molt_socket_recvfrom(
    sock_bits: u64,
    size_bits: u64,
    flags_bits: u64,
) -> u64 {
    let socket_ptr = ptr_from_bits(sock_bits);
    if socket_ptr.is_null() {
        return MoltObject::none().bits();
    }
    let size = to_i64(obj_from_bits(size_bits)).unwrap_or(0).max(0) as usize;
    let flags = to_i64(obj_from_bits(flags_bits)).unwrap_or(0) as i32;
    #[cfg(unix)]
    let dontwait = (flags & libc::MSG_DONTWAIT) != 0;
    #[cfg(not(unix))]
    let dontwait = false;
    let mut buf = vec![0u8; size];
    loop {
        let res = with_socket_mut(socket_ptr, |inner| {
            #[cfg(unix)]
            let fd = inner
                .raw_fd()
                .ok_or_else(|| std::io::Error::new(ErrorKind::NotConnected, "socket closed"))?;
            #[cfg(windows)]
            let fd = inner
                .raw_socket()
                .ok_or_else(|| std::io::Error::new(ErrorKind::NotConnected, "socket closed"))?;
            let mut storage: libc::sockaddr_storage = unsafe { std::mem::zeroed() };
            let mut len = std::mem::size_of::<libc::sockaddr_storage>() as libc::socklen_t;
            let ret = unsafe {
                libc::recvfrom(
                    libc_socket(fd),
                    buf.as_mut_ptr() as *mut c_void,
                    buf.len(),
                    flags,
                    &mut storage as *mut _ as *mut libc::sockaddr,
                    &mut len,
                )
            };
            if ret >= 0 {
                let addr = unsafe { SockAddr::new(storage, len) };
                Ok((ret as usize, addr))
            } else {
                Err(std::io::Error::last_os_error())
            }
        });
        match res {
            Ok((n, addr)) => {
                let data_ptr = alloc_bytes(&buf[..n]);
                if data_ptr.is_null() {
                    return MoltObject::none().bits();
                }
                let data_bits = MoltObject::from_ptr(data_ptr).bits();
                let addr_bits = sockaddr_to_bits(&addr);
                let tuple_ptr = alloc_tuple(&[data_bits, addr_bits]);
                dec_ref_bits(data_bits);
                if tuple_ptr.is_null() {
                    return MoltObject::none().bits();
                }
                return MoltObject::from_ptr(tuple_ptr).bits();
            }
            Err(err) if err.kind() == ErrorKind::WouldBlock => {
                if dontwait {
                    return raise_os_error::<u64>(err, "recvfrom");
                }
                if let Err(wait_err) = socket_wait_ready(socket_ptr, IO_EVENT_READ) {
                    if wait_err.kind() == ErrorKind::TimedOut {
                        return raise_exception::<u64>("TimeoutError", "timed out");
                    }
                    return raise_os_error::<u64>(wait_err, "recvfrom");
                }
                continue;
            }
            Err(err) => return raise_os_error::<u64>(err, "recvfrom"),
        }
    }
}

#[cfg(not(target_arch = "wasm32"))]
#[no_mangle]
pub unsafe extern "C" fn molt_socket_shutdown(sock_bits: u64, how_bits: u64) -> u64 {
    let socket_ptr = ptr_from_bits(sock_bits);
    if socket_ptr.is_null() {
        return MoltObject::none().bits();
    }
    let how = to_i64(obj_from_bits(how_bits)).unwrap_or(2) as i32;
    let res = with_socket_mut(socket_ptr, |inner| {
        #[cfg(unix)]
        let fd = inner
            .raw_fd()
            .ok_or_else(|| std::io::Error::new(ErrorKind::NotConnected, "socket closed"))?;
        #[cfg(windows)]
        let fd = inner
            .raw_socket()
            .ok_or_else(|| std::io::Error::new(ErrorKind::NotConnected, "socket closed"))?;
        let ret = unsafe { libc::shutdown(libc_socket(fd), how) };
        if ret == 0 {
            Ok(())
        } else {
            Err(std::io::Error::last_os_error())
        }
    });
    match res {
        Ok(_) => MoltObject::none().bits(),
        Err(err) => raise_os_error::<u64>(err, "shutdown"),
    }
}

#[cfg(not(target_arch = "wasm32"))]
#[no_mangle]
pub unsafe extern "C" fn molt_socket_getsockname(sock_bits: u64) -> u64 {
    let socket_ptr = ptr_from_bits(sock_bits);
    if socket_ptr.is_null() {
        return MoltObject::none().bits();
    }
    let res = with_socket_mut(socket_ptr, |inner| {
        #[cfg(unix)]
        let fd = inner
            .raw_fd()
            .ok_or_else(|| std::io::Error::new(ErrorKind::NotConnected, "socket closed"))?;
        #[cfg(windows)]
        let fd = inner
            .raw_socket()
            .ok_or_else(|| std::io::Error::new(ErrorKind::NotConnected, "socket closed"))?;
        let mut storage: libc::sockaddr_storage = unsafe { std::mem::zeroed() };
        let mut len = std::mem::size_of::<libc::sockaddr_storage>() as libc::socklen_t;
        let ret = unsafe {
            libc::getsockname(
                libc_socket(fd),
                &mut storage as *mut _ as *mut libc::sockaddr,
                &mut len,
            )
        };
        if ret == 0 {
            Ok(unsafe { SockAddr::new(storage, len) })
        } else {
            Err(std::io::Error::last_os_error())
        }
    });
    match res {
        Ok(addr) => sockaddr_to_bits(&addr),
        Err(err) => raise_os_error::<u64>(err, "getsockname"),
    }
}

#[cfg(not(target_arch = "wasm32"))]
#[no_mangle]
pub unsafe extern "C" fn molt_socket_getpeername(sock_bits: u64) -> u64 {
    let socket_ptr = ptr_from_bits(sock_bits);
    if socket_ptr.is_null() {
        return MoltObject::none().bits();
    }
    let res = with_socket_mut(socket_ptr, |inner| {
        #[cfg(unix)]
        let fd = inner
            .raw_fd()
            .ok_or_else(|| std::io::Error::new(ErrorKind::NotConnected, "socket closed"))?;
        #[cfg(windows)]
        let fd = inner
            .raw_socket()
            .ok_or_else(|| std::io::Error::new(ErrorKind::NotConnected, "socket closed"))?;
        let mut storage: libc::sockaddr_storage = unsafe { std::mem::zeroed() };
        let mut len = std::mem::size_of::<libc::sockaddr_storage>() as libc::socklen_t;
        let ret = unsafe {
            libc::getpeername(
                libc_socket(fd),
                &mut storage as *mut _ as *mut libc::sockaddr,
                &mut len,
            )
        };
        if ret == 0 {
            Ok(unsafe { SockAddr::new(storage, len) })
        } else {
            Err(std::io::Error::last_os_error())
        }
    });
    match res {
        Ok(addr) => sockaddr_to_bits(&addr),
        Err(err) => raise_os_error::<u64>(err, "getpeername"),
    }
}

#[cfg(not(target_arch = "wasm32"))]
#[no_mangle]
pub unsafe extern "C" fn molt_socket_setsockopt(
    sock_bits: u64,
    level_bits: u64,
    opt_bits: u64,
    value_bits: u64,
) -> u64 {
    let socket_ptr = ptr_from_bits(sock_bits);
    if socket_ptr.is_null() {
        return MoltObject::none().bits();
    }
    let level = to_i64(obj_from_bits(level_bits)).unwrap_or(0) as i32;
    let optname = to_i64(obj_from_bits(opt_bits)).unwrap_or(0) as i32;
    let obj = obj_from_bits(value_bits);
    let res = with_socket_mut(socket_ptr, |inner| {
        #[cfg(unix)]
        let fd = inner
            .raw_fd()
            .ok_or_else(|| std::io::Error::new(ErrorKind::NotConnected, "socket closed"))?;
        #[cfg(windows)]
        let fd = inner
            .raw_socket()
            .ok_or_else(|| std::io::Error::new(ErrorKind::NotConnected, "socket closed"))?;
        if let Some(val) = to_i64(obj) {
            let val = val as c_int;
            let ret = unsafe {
                libc::setsockopt(
                    libc_socket(fd),
                    level,
                    optname,
                    &val as *const _ as *const c_void,
                    std::mem::size_of::<c_int>() as libc::socklen_t,
                )
            };
            if ret == 0 {
                Ok(())
            } else {
                Err(std::io::Error::last_os_error())
            }
        } else if let Some(ptr) = obj.as_ptr() {
            let bytes = unsafe { bytes_like_slice_raw(ptr) }
                .ok_or_else(|| std::io::Error::new(ErrorKind::InvalidInput, "invalid optval"))?;
            let ret = unsafe {
                libc::setsockopt(
                    libc_socket(fd),
                    level,
                    optname,
                    bytes.as_ptr() as *const c_void,
                    bytes.len() as libc::socklen_t,
                )
            };
            if ret == 0 {
                Ok(())
            } else {
                Err(std::io::Error::last_os_error())
            }
        } else {
            Err(std::io::Error::new(
                ErrorKind::InvalidInput,
                "invalid optval",
            ))
        }
    });
    match res {
        Ok(_) => MoltObject::none().bits(),
        Err(err) => raise_os_error::<u64>(err, "setsockopt"),
    }
}

#[cfg(not(target_arch = "wasm32"))]
#[no_mangle]
pub unsafe extern "C" fn molt_socket_getsockopt(
    sock_bits: u64,
    level_bits: u64,
    opt_bits: u64,
    buflen_bits: u64,
) -> u64 {
    let socket_ptr = ptr_from_bits(sock_bits);
    if socket_ptr.is_null() {
        return MoltObject::none().bits();
    }
    let level = to_i64(obj_from_bits(level_bits)).unwrap_or(0) as i32;
    let optname = to_i64(obj_from_bits(opt_bits)).unwrap_or(0) as i32;
    let buflen = if obj_from_bits(buflen_bits).is_none() {
        None
    } else {
        Some(to_i64(obj_from_bits(buflen_bits)).unwrap_or(0).max(0) as usize)
    };
    let res = with_socket_mut(socket_ptr, |inner| {
        #[cfg(unix)]
        let fd = inner
            .raw_fd()
            .ok_or_else(|| std::io::Error::new(ErrorKind::NotConnected, "socket closed"))?;
        #[cfg(windows)]
        let fd = inner
            .raw_socket()
            .ok_or_else(|| std::io::Error::new(ErrorKind::NotConnected, "socket closed"))?;
        if let Some(buflen) = buflen {
            let mut buf = vec![0u8; buflen];
            let mut len = buflen as libc::socklen_t;
            let ret = unsafe {
                libc::getsockopt(
                    libc_socket(fd),
                    level,
                    optname,
                    buf.as_mut_ptr() as *mut c_void,
                    &mut len,
                )
            };
            if ret == 0 {
                let ptr = alloc_bytes(&buf[..len as usize]);
                if ptr.is_null() {
                    Err(std::io::Error::new(ErrorKind::Other, "allocation failed"))
                } else {
                    Ok(MoltObject::from_ptr(ptr).bits())
                }
            } else {
                Err(std::io::Error::last_os_error())
            }
        } else {
            let mut val: c_int = 0;
            let mut len = std::mem::size_of::<c_int>() as libc::socklen_t;
            let ret = unsafe {
                libc::getsockopt(
                    libc_socket(fd),
                    level,
                    optname,
                    &mut val as *mut _ as *mut c_void,
                    &mut len,
                )
            };
            if ret == 0 {
                Ok(MoltObject::from_int(val as i64).bits())
            } else {
                Err(std::io::Error::last_os_error())
            }
        }
    });
    match res {
        Ok(bits) => bits,
        Err(err) => raise_os_error::<u64>(err, "getsockopt"),
    }
}

#[cfg(not(target_arch = "wasm32"))]
#[no_mangle]
pub unsafe extern "C" fn molt_socket_detach(sock_bits: u64) -> u64 {
    let socket_ptr = ptr_from_bits(sock_bits);
    if socket_ptr.is_null() {
        return MoltObject::from_int(-1).bits();
    }
    let socket = &*(socket_ptr as *mut MoltSocket);
    socket_unregister_fd(socket_ptr);
    runtime_state().io_poller().deregister_socket(socket_ptr);
    socket.closed.store(true, AtomicOrdering::Relaxed);
    let raw = {
        let mut guard = socket.inner.lock().unwrap();
        let kind = std::mem::replace(&mut guard.kind, MoltSocketKind::Closed);
        #[cfg(unix)]
        {
            match kind {
                MoltSocketKind::Pending(sock) => sock.into_raw_fd() as i64,
                MoltSocketKind::TcpStream(sock) => sock.into_raw_fd() as i64,
                MoltSocketKind::TcpListener(sock) => sock.into_raw_fd() as i64,
                MoltSocketKind::UdpSocket(sock) => sock.into_raw_fd() as i64,
                MoltSocketKind::UnixStream(sock) => sock.into_raw_fd() as i64,
                MoltSocketKind::UnixListener(sock) => sock.into_raw_fd() as i64,
                MoltSocketKind::UnixDatagram(sock) => sock.into_raw_fd() as i64,
                MoltSocketKind::Closed => -1,
            }
        }
        #[cfg(windows)]
        {
            match kind {
                MoltSocketKind::Pending(sock) => sock.into_raw_socket() as i64,
                MoltSocketKind::TcpStream(sock) => sock.into_raw_socket() as i64,
                MoltSocketKind::TcpListener(sock) => sock.into_raw_socket() as i64,
                MoltSocketKind::UdpSocket(sock) => sock.into_raw_socket() as i64,
                MoltSocketKind::Closed => -1,
            }
        }
    };
    MoltObject::from_int(raw).bits()
}

#[cfg(target_arch = "wasm32")]
// TODO(wasm-parity, owner:runtime, milestone:RT2, priority:P0, status:missing): implement WASI socket support and io_poller-backed readiness for wasm targets.
fn wasm_socket_unavailable<T: ExceptionSentinel>() -> T {
    raise_exception("RuntimeError", "socket unsupported on wasm")
}

#[cfg(target_arch = "wasm32")]
#[no_mangle]
pub extern "C" fn molt_socket_new(
    _family_bits: u64,
    _type_bits: u64,
    _proto_bits: u64,
    _fileno_bits: u64,
) -> u64 {
    wasm_socket_unavailable()
}

#[cfg(target_arch = "wasm32")]
#[no_mangle]
pub extern "C" fn molt_socket_close(_sock_bits: u64) -> u64 {
    wasm_socket_unavailable()
}

#[cfg(target_arch = "wasm32")]
#[no_mangle]
pub extern "C" fn molt_socket_drop(_sock_bits: u64) {
    wasm_socket_unavailable()
}

#[cfg(target_arch = "wasm32")]
#[no_mangle]
pub extern "C" fn molt_socket_fileno(_sock_bits: u64) -> u64 {
    wasm_socket_unavailable()
}

#[cfg(target_arch = "wasm32")]
#[no_mangle]
pub extern "C" fn molt_socket_gettimeout(_sock_bits: u64) -> u64 {
    wasm_socket_unavailable()
}

#[cfg(target_arch = "wasm32")]
#[no_mangle]
pub extern "C" fn molt_socket_settimeout(_sock_bits: u64, _timeout_bits: u64) -> u64 {
    wasm_socket_unavailable()
}

#[cfg(target_arch = "wasm32")]
#[no_mangle]
pub extern "C" fn molt_socket_setblocking(_sock_bits: u64, _flag_bits: u64) -> u64 {
    wasm_socket_unavailable()
}

#[cfg(target_arch = "wasm32")]
#[no_mangle]
pub extern "C" fn molt_socket_getblocking(_sock_bits: u64) -> u64 {
    wasm_socket_unavailable()
}

#[cfg(target_arch = "wasm32")]
#[no_mangle]
pub extern "C" fn molt_socket_bind(_sock_bits: u64, _addr_bits: u64) -> u64 {
    wasm_socket_unavailable()
}

#[cfg(target_arch = "wasm32")]
#[no_mangle]
pub extern "C" fn molt_socket_listen(_sock_bits: u64, _backlog_bits: u64) -> u64 {
    wasm_socket_unavailable()
}

#[cfg(target_arch = "wasm32")]
#[no_mangle]
pub extern "C" fn molt_socket_accept(_sock_bits: u64) -> u64 {
    wasm_socket_unavailable()
}

#[cfg(target_arch = "wasm32")]
#[no_mangle]
pub extern "C" fn molt_socket_connect(_sock_bits: u64, _addr_bits: u64) -> u64 {
    wasm_socket_unavailable()
}

#[cfg(target_arch = "wasm32")]
#[no_mangle]
pub extern "C" fn molt_socket_connect_ex(_sock_bits: u64, _addr_bits: u64) -> u64 {
    wasm_socket_unavailable()
}

#[cfg(target_arch = "wasm32")]
#[no_mangle]
pub extern "C" fn molt_socket_recv(_sock_bits: u64, _size_bits: u64, _flags_bits: u64) -> u64 {
    wasm_socket_unavailable()
}

#[cfg(target_arch = "wasm32")]
#[no_mangle]
pub extern "C" fn molt_socket_recv_into(
    _sock_bits: u64,
    _buffer_bits: u64,
    _size_bits: u64,
    _flags_bits: u64,
) -> u64 {
    wasm_socket_unavailable()
}

#[cfg(target_arch = "wasm32")]
#[no_mangle]
pub extern "C" fn molt_socket_send(_sock_bits: u64, _data_bits: u64, _flags_bits: u64) -> u64 {
    wasm_socket_unavailable()
}

#[cfg(target_arch = "wasm32")]
#[no_mangle]
pub extern "C" fn molt_socket_sendall(_sock_bits: u64, _data_bits: u64, _flags_bits: u64) -> u64 {
    wasm_socket_unavailable()
}

#[cfg(target_arch = "wasm32")]
#[no_mangle]
pub extern "C" fn molt_socket_sendto(
    _sock_bits: u64,
    _data_bits: u64,
    _flags_bits: u64,
    _addr_bits: u64,
) -> u64 {
    wasm_socket_unavailable()
}

#[cfg(target_arch = "wasm32")]
#[no_mangle]
pub extern "C" fn molt_socket_recvfrom(_sock_bits: u64, _size_bits: u64, _flags_bits: u64) -> u64 {
    wasm_socket_unavailable()
}

#[cfg(target_arch = "wasm32")]
#[no_mangle]
pub extern "C" fn molt_socket_shutdown(_sock_bits: u64, _how_bits: u64) -> u64 {
    wasm_socket_unavailable()
}

#[cfg(target_arch = "wasm32")]
#[no_mangle]
pub extern "C" fn molt_socket_getsockname(_sock_bits: u64) -> u64 {
    wasm_socket_unavailable()
}

#[cfg(target_arch = "wasm32")]
#[no_mangle]
pub extern "C" fn molt_socket_getpeername(_sock_bits: u64) -> u64 {
    wasm_socket_unavailable()
}

#[cfg(target_arch = "wasm32")]
#[no_mangle]
pub extern "C" fn molt_socket_setsockopt(
    _sock_bits: u64,
    _level_bits: u64,
    _opt_bits: u64,
    _value_bits: u64,
) -> u64 {
    wasm_socket_unavailable()
}

#[cfg(target_arch = "wasm32")]
#[no_mangle]
pub extern "C" fn molt_socket_getsockopt(
    _sock_bits: u64,
    _level_bits: u64,
    _opt_bits: u64,
    _buflen_bits: u64,
) -> u64 {
    wasm_socket_unavailable()
}

#[cfg(target_arch = "wasm32")]
#[no_mangle]
pub extern "C" fn molt_socket_detach(_sock_bits: u64) -> u64 {
    wasm_socket_unavailable()
}

#[cfg(not(target_arch = "wasm32"))]
#[no_mangle]
pub unsafe extern "C" fn molt_socketpair(family_bits: u64, type_bits: u64, proto_bits: u64) -> u64 {
    if require_net_capability::<u64>(&["net", "net.connect", "net.listen", "net.bind"]).is_err() {
        return MoltObject::none().bits();
    }
    let family = if obj_from_bits(family_bits).is_none() {
        #[cfg(unix)]
        {
            libc::AF_UNIX
        }
        #[cfg(not(unix))]
        {
            libc::AF_INET
        }
    } else {
        match to_i64(obj_from_bits(family_bits)) {
            Some(val) => val as i32,
            None => return raise_exception::<_>("TypeError", "family must be int or None"),
        }
    };
    let sock_type = if obj_from_bits(type_bits).is_none() {
        libc::SOCK_STREAM
    } else {
        match to_i64(obj_from_bits(type_bits)) {
            Some(val) => val as i32,
            None => return raise_exception::<_>("TypeError", "type must be int or None"),
        }
    };
    let proto = if obj_from_bits(proto_bits).is_none() {
        0
    } else {
        match to_i64(obj_from_bits(proto_bits)) {
            Some(val) => val as i32,
            None => return raise_exception::<_>("TypeError", "proto must be int or None"),
        }
    };
    #[cfg(unix)]
    {
        if family != libc::AF_UNIX {
            return raise_os_error_errno::<u64>(libc::EAFNOSUPPORT as i64, "socketpair family");
        }
        let mut fds = [0 as libc::c_int; 2];
        let ret = libc::socketpair(family, sock_type, proto, fds.as_mut_ptr());
        if ret != 0 {
            return raise_os_error::<u64>(std::io::Error::last_os_error(), "socketpair");
        }
        let left_bits = molt_socket_new(
            MoltObject::from_int(family as i64).bits(),
            MoltObject::from_int(sock_type as i64).bits(),
            MoltObject::from_int(proto as i64).bits(),
            MoltObject::from_int(fds[0] as i64).bits(),
        );
        let right_bits = molt_socket_new(
            MoltObject::from_int(family as i64).bits(),
            MoltObject::from_int(sock_type as i64).bits(),
            MoltObject::from_int(proto as i64).bits(),
            MoltObject::from_int(fds[1] as i64).bits(),
        );
        let tuple_ptr = alloc_tuple(&[left_bits, right_bits]);
        if tuple_ptr.is_null() {
            return MoltObject::none().bits();
        }
        return MoltObject::from_ptr(tuple_ptr).bits();
    }
    #[cfg(windows)]
    {
        // TODO(perf, owner:runtime, milestone:RT2, priority:P2, status:planned): implement a native Windows socketpair using WSAPROTOCOL_INFO or AF_UNIX to avoid loopback TCP overhead.
        if family != libc::AF_INET && family != libc::AF_INET6 {
            return raise_os_error_errno::<u64>(libc::EAFNOSUPPORT as i64, "socketpair family");
        }
        let loopback = if family == libc::AF_INET6 {
            "[::1]:0"
        } else {
            "127.0.0.1:0"
        };
        let listener = match std::net::TcpListener::bind(loopback) {
            Ok(l) => l,
            Err(err) => return raise_os_error::<u64>(err, "socketpair"),
        };
        let addr = match listener.local_addr() {
            Ok(addr) => addr,
            Err(err) => return raise_os_error::<u64>(err, "socketpair"),
        };
        let client = match std::net::TcpStream::connect(addr) {
            Ok(stream) => stream,
            Err(err) => return raise_os_error::<u64>(err, "socketpair"),
        };
        let (server, _) = match listener.accept() {
            Ok(pair) => pair,
            Err(err) => return raise_os_error::<u64>(err, "socketpair"),
        };
        let left_fd = client.into_raw_socket();
        let right_fd = server.into_raw_socket();
        let left_bits = molt_socket_new(
            MoltObject::from_int(family as i64).bits(),
            MoltObject::from_int(sock_type as i64).bits(),
            MoltObject::from_int(proto as i64).bits(),
            MoltObject::from_int(left_fd as i64).bits(),
        );
        let right_bits = molt_socket_new(
            MoltObject::from_int(family as i64).bits(),
            MoltObject::from_int(sock_type as i64).bits(),
            MoltObject::from_int(proto as i64).bits(),
            MoltObject::from_int(right_fd as i64).bits(),
        );
        let tuple_ptr = alloc_tuple(&[left_bits, right_bits]);
        if tuple_ptr.is_null() {
            return MoltObject::none().bits();
        }
        MoltObject::from_ptr(tuple_ptr).bits()
    }
}

#[cfg(target_arch = "wasm32")]
#[no_mangle]
pub extern "C" fn molt_socketpair(_family_bits: u64, _type_bits: u64, _proto_bits: u64) -> u64 {
    return raise_exception::<_>("RuntimeError", "socketpair unsupported on wasm");
}

#[cfg(not(target_arch = "wasm32"))]
#[no_mangle]
pub unsafe extern "C" fn molt_socket_getaddrinfo(
    host_bits: u64,
    port_bits: u64,
    family_bits: u64,
    type_bits: u64,
    proto_bits: u64,
    flags_bits: u64,
) -> u64 {
    if require_net_capability::<u64>(&["net", "net.connect", "net.bind", "net"]).is_err() {
        return MoltObject::none().bits();
    }
    let host = match host_from_bits(host_bits) {
        Ok(val) => val,
        Err(msg) => return raise_exception::<_>("TypeError", &msg),
    };
    let service = match service_from_bits(port_bits) {
        Ok(val) => val,
        Err(msg) => return raise_exception::<_>("TypeError", &msg),
    };
    let family = to_i64(obj_from_bits(family_bits)).unwrap_or(0) as i32;
    let sock_type = to_i64(obj_from_bits(type_bits)).unwrap_or(0) as i32;
    let proto = to_i64(obj_from_bits(proto_bits)).unwrap_or(0) as i32;
    let flags = to_i64(obj_from_bits(flags_bits)).unwrap_or(0) as i32;

    let host_cstr = host
        .as_ref()
        .and_then(|val| CString::new(val.as_str()).ok());
    let service_cstr = service
        .as_ref()
        .and_then(|val| CString::new(val.as_str()).ok());
    if host.is_some() && host_cstr.is_none() {
        return raise_exception::<u64>("TypeError", "host contains NUL byte");
    }
    if service.is_some() && service_cstr.is_none() {
        return raise_exception::<u64>("TypeError", "service contains NUL byte");
    }
    let mut hints: libc::addrinfo = std::mem::zeroed();
    hints.ai_family = family;
    hints.ai_socktype = sock_type;
    hints.ai_protocol = proto;
    hints.ai_flags = flags;

    let mut res: *mut libc::addrinfo = std::ptr::null_mut();
    let err = libc::getaddrinfo(
        host_cstr
            .as_ref()
            .map(|s| s.as_ptr())
            .unwrap_or(std::ptr::null()),
        service_cstr
            .as_ref()
            .map(|s| s.as_ptr())
            .unwrap_or(std::ptr::null()),
        &hints as *const libc::addrinfo,
        &mut res as *mut *mut libc::addrinfo,
    );
    if err != 0 {
        let msg = CStr::from_ptr(libc::gai_strerror(err))
            .to_string_lossy()
            .to_string();
        let msg = format!("[Errno {err}] {msg}");
        return raise_os_error_errno::<u64>(err as i64, &msg);
    }

    let mut out: Vec<u64> = Vec::new();
    // TODO(perf, owner:runtime, milestone:RT2, priority:P2, status:planned): switch to a list builder to avoid extra refcount churn while assembling addrinfo results.
    let mut cur = res;
    while !cur.is_null() {
        let ai = &*cur;
        let mut storage: libc::sockaddr_storage = unsafe { std::mem::zeroed() };
        let len = ai.ai_addrlen;
        unsafe {
            std::ptr::copy_nonoverlapping(
                ai.ai_addr as *const u8,
                &mut storage as *mut _ as *mut u8,
                len as usize,
            );
        }
        let sockaddr = unsafe { SockAddr::new(storage, len) };
        let sockaddr_bits = sockaddr_to_bits(&sockaddr);
        let canon_bits = if !ai.ai_canonname.is_null() {
            let name = CStr::from_ptr(ai.ai_canonname).to_string_lossy();
            let ptr = alloc_string(name.as_bytes());
            if ptr.is_null() {
                MoltObject::none().bits()
            } else {
                MoltObject::from_ptr(ptr).bits()
            }
        } else {
            MoltObject::none().bits()
        };
        let family_bits = MoltObject::from_int(ai.ai_family as i64).bits();
        let sock_type_bits = MoltObject::from_int(ai.ai_socktype as i64).bits();
        let proto_bits = MoltObject::from_int(ai.ai_protocol as i64).bits();
        let tuple_ptr = alloc_tuple(&[
            family_bits,
            sock_type_bits,
            proto_bits,
            canon_bits,
            sockaddr_bits,
        ]);
        if tuple_ptr.is_null() {
            dec_ref_bits(canon_bits);
            break;
        }
        let tuple_bits = MoltObject::from_ptr(tuple_ptr).bits();
        out.push(tuple_bits);
        dec_ref_bits(canon_bits);
        cur = ai.ai_next;
    }
    libc::freeaddrinfo(res);
    let list_ptr = alloc_list(&out);
    if list_ptr.is_null() {
        for bits in out {
            dec_ref_bits(bits);
        }
        return MoltObject::none().bits();
    }
    let list_bits = MoltObject::from_ptr(list_ptr).bits();
    for bits in out {
        dec_ref_bits(bits);
    }
    list_bits
}

#[cfg(target_arch = "wasm32")]
#[no_mangle]
pub extern "C" fn molt_socket_getaddrinfo(
    _host_bits: u64,
    _port_bits: u64,
    _family_bits: u64,
    _type_bits: u64,
    _proto_bits: u64,
    _flags_bits: u64,
) -> u64 {
    return raise_exception::<_>("RuntimeError", "getaddrinfo unsupported on wasm");
}

#[cfg(not(target_arch = "wasm32"))]
#[no_mangle]
pub unsafe extern "C" fn molt_socket_getnameinfo(addr_bits: u64, flags_bits: u64) -> u64 {
    let flags = to_i64(obj_from_bits(flags_bits)).unwrap_or(0) as i32;
    let obj = obj_from_bits(addr_bits);
    let Some(ptr) = obj.as_ptr() else {
        return raise_exception::<_>("TypeError", "sockaddr must be tuple");
    };
    let type_id = object_type_id(ptr);
    if type_id != TYPE_ID_TUPLE && type_id != TYPE_ID_LIST {
        return raise_exception::<_>("TypeError", "sockaddr must be tuple");
    }
    let elems = seq_vec_ref(ptr);
    let family = if elems.len() >= 4 {
        libc::AF_INET6
    } else {
        libc::AF_INET
    };
    let sockaddr = match sockaddr_from_bits(addr_bits, family) {
        Ok(addr) => addr,
        Err(msg) => return raise_exception::<_>("TypeError", &msg),
    };
    let mut host_buf = vec![0u8; libc::NI_MAXHOST as usize + 1];
    let mut serv_buf = vec![0u8; libc::NI_MAXSERV as usize + 1];
    let ret = libc::getnameinfo(
        sockaddr.as_ptr(),
        sockaddr.len(),
        host_buf.as_mut_ptr() as *mut libc::c_char,
        host_buf.len() as libc::socklen_t,
        serv_buf.as_mut_ptr() as *mut libc::c_char,
        serv_buf.len() as libc::socklen_t,
        flags,
    );
    if ret != 0 {
        let msg = CStr::from_ptr(libc::gai_strerror(ret))
            .to_string_lossy()
            .to_string();
        let msg = format!("[Errno {ret}] {msg}");
        return raise_os_error_errno::<u64>(ret as i64, &msg);
    }
    let host = CStr::from_ptr(host_buf.as_ptr() as *const libc::c_char).to_string_lossy();
    let serv = CStr::from_ptr(serv_buf.as_ptr() as *const libc::c_char).to_string_lossy();
    let host_ptr = alloc_string(host.as_bytes());
    if host_ptr.is_null() {
        return MoltObject::none().bits();
    }
    let serv_ptr = alloc_string(serv.as_bytes());
    if serv_ptr.is_null() {
        dec_ref_bits(MoltObject::from_ptr(host_ptr).bits());
        return MoltObject::none().bits();
    }
    let host_bits = MoltObject::from_ptr(host_ptr).bits();
    let serv_bits = MoltObject::from_ptr(serv_ptr).bits();
    let tuple_ptr = alloc_tuple(&[host_bits, serv_bits]);
    dec_ref_bits(host_bits);
    dec_ref_bits(serv_bits);
    if tuple_ptr.is_null() {
        return MoltObject::none().bits();
    }
    MoltObject::from_ptr(tuple_ptr).bits()
}

#[cfg(target_arch = "wasm32")]
#[no_mangle]
pub extern "C" fn molt_socket_getnameinfo(_addr_bits: u64, _flags_bits: u64) -> u64 {
    return raise_exception::<_>("RuntimeError", "getnameinfo unsupported on wasm");
}

#[cfg(not(target_arch = "wasm32"))]
#[no_mangle]
pub unsafe extern "C" fn molt_socket_gethostname() -> u64 {
    let mut buf = vec![0u8; 256];
    let ret = libc::gethostname(buf.as_mut_ptr() as *mut libc::c_char, buf.len());
    if ret != 0 {
        return raise_os_error::<u64>(std::io::Error::last_os_error(), "gethostname");
    }
    if let Some(pos) = buf.iter().position(|b| *b == 0) {
        buf.truncate(pos);
    }
    let ptr = alloc_string(&buf);
    if ptr.is_null() {
        return MoltObject::none().bits();
    }
    MoltObject::from_ptr(ptr).bits()
}

#[cfg(target_arch = "wasm32")]
#[no_mangle]
pub extern "C" fn molt_socket_gethostname() -> u64 {
    return raise_exception::<_>("RuntimeError", "gethostname unsupported on wasm");
}

#[cfg(not(target_arch = "wasm32"))]
#[no_mangle]
pub unsafe extern "C" fn molt_socket_getservbyname(name_bits: u64, proto_bits: u64) -> u64 {
    let name = match host_from_bits(name_bits) {
        Ok(Some(val)) => val,
        Ok(None) => return raise_exception::<_>("TypeError", "service name cannot be None"),
        Err(msg) => return raise_exception::<_>("TypeError", &msg),
    };
    let proto = match host_from_bits(proto_bits) {
        Ok(val) => val,
        Err(msg) => return raise_exception::<_>("TypeError", &msg),
    };
    let name_cstr = CString::new(name).map_err(|_| ()).ok();
    if name_cstr.is_none() {
        return raise_exception::<_>("TypeError", "service name contains NUL byte");
    }
    let proto_cstr = proto
        .as_ref()
        .and_then(|val| CString::new(val.as_str()).ok());
    if proto.is_some() && proto_cstr.is_none() {
        return raise_exception::<_>("TypeError", "proto contains NUL byte");
    }
    let serv = libc::getservbyname(
        name_cstr.as_ref().unwrap().as_ptr(),
        proto_cstr
            .as_ref()
            .map(|s| s.as_ptr())
            .unwrap_or(std::ptr::null()),
    );
    if serv.is_null() {
        return raise_os_error_errno::<u64>(libc::ENOENT as i64, "service not found");
    }
    let port = libc::ntohs((*serv).s_port as u16) as i64;
    MoltObject::from_int(port).bits()
}

#[cfg(target_arch = "wasm32")]
#[no_mangle]
pub extern "C" fn molt_socket_getservbyname(_name_bits: u64, _proto_bits: u64) -> u64 {
    return raise_exception::<_>("RuntimeError", "getservbyname unsupported on wasm");
}

#[cfg(not(target_arch = "wasm32"))]
#[no_mangle]
pub unsafe extern "C" fn molt_socket_getservbyport(port_bits: u64, proto_bits: u64) -> u64 {
    let port = match to_i64(obj_from_bits(port_bits)) {
        Some(val) if val >= 0 && val <= u16::MAX as i64 => val as u16,
        _ => return raise_exception::<_>("TypeError", "port must be int"),
    };
    let proto = match host_from_bits(proto_bits) {
        Ok(val) => val,
        Err(msg) => return raise_exception::<_>("TypeError", &msg),
    };
    let proto_cstr = proto
        .as_ref()
        .and_then(|val| CString::new(val.as_str()).ok());
    if proto.is_some() && proto_cstr.is_none() {
        return raise_exception::<_>("TypeError", "proto contains NUL byte");
    }
    let serv = libc::getservbyport(
        libc::htons(port) as i32,
        proto_cstr
            .as_ref()
            .map(|s| s.as_ptr())
            .unwrap_or(std::ptr::null()),
    );
    if serv.is_null() {
        return raise_os_error_errno::<u64>(libc::ENOENT as i64, "service not found");
    }
    let name = CStr::from_ptr((*serv).s_name).to_string_lossy();
    let ptr = alloc_string(name.as_bytes());
    if ptr.is_null() {
        return MoltObject::none().bits();
    }
    MoltObject::from_ptr(ptr).bits()
}

#[cfg(target_arch = "wasm32")]
#[no_mangle]
pub extern "C" fn molt_socket_getservbyport(_port_bits: u64, _proto_bits: u64) -> u64 {
    return raise_exception::<_>("RuntimeError", "getservbyport unsupported on wasm");
}

#[cfg(not(target_arch = "wasm32"))]
#[no_mangle]
pub unsafe extern "C" fn molt_socket_inet_pton(family_bits: u64, address_bits: u64) -> u64 {
    let family = to_i64(obj_from_bits(family_bits)).unwrap_or(0) as i32;
    let addr = match host_from_bits(address_bits) {
        Ok(Some(val)) => val,
        Ok(None) => return raise_exception::<_>("TypeError", "address cannot be None"),
        Err(msg) => return raise_exception::<_>("TypeError", &msg),
    };
    if family == libc::AF_INET {
        let ip: Ipv4Addr = match addr.parse() {
            Ok(ip) => ip,
            Err(_) => {
                return raise_os_error_errno::<u64>(libc::EINVAL as i64, "invalid IPv4 address")
            }
        };
        let ptr = alloc_bytes(&ip.octets());
        if ptr.is_null() {
            return MoltObject::none().bits();
        }
        return MoltObject::from_ptr(ptr).bits();
    }
    if family == libc::AF_INET6 {
        let ip: Ipv6Addr = match addr.parse() {
            Ok(ip) => ip,
            Err(_) => {
                return raise_os_error_errno::<u64>(libc::EINVAL as i64, "invalid IPv6 address")
            }
        };
        let ptr = alloc_bytes(&ip.octets());
        if ptr.is_null() {
            return MoltObject::none().bits();
        }
        return MoltObject::from_ptr(ptr).bits();
    }
    return raise_exception::<_>("ValueError", "unsupported address family");
}

#[cfg(target_arch = "wasm32")]
#[no_mangle]
pub extern "C" fn molt_socket_inet_pton(_family_bits: u64, _address_bits: u64) -> u64 {
    return raise_exception::<_>("RuntimeError", "inet_pton unsupported on wasm");
}

#[cfg(not(target_arch = "wasm32"))]
#[no_mangle]
pub unsafe extern "C" fn molt_socket_inet_ntop(family_bits: u64, packed_bits: u64) -> u64 {
    let family = to_i64(obj_from_bits(family_bits)).unwrap_or(0) as i32;
    let obj = obj_from_bits(packed_bits);
    let data = if let Some(ptr) = obj.as_ptr() {
        let type_id = object_type_id(ptr);
        if type_id == TYPE_ID_BYTES || type_id == TYPE_ID_BYTEARRAY {
            let len = bytes_len(ptr);
            let slice = std::slice::from_raw_parts(bytes_data(ptr), len);
            slice.to_vec()
        } else if type_id == TYPE_ID_MEMORYVIEW {
            if let Some(slice) = memoryview_bytes_slice(ptr) {
                slice.to_vec()
            } else if let Some(vec) = memoryview_collect_bytes(ptr) {
                vec
            } else {
                return raise_exception::<_>("TypeError", "packed address must be bytes-like");
            }
        } else {
            return raise_exception::<_>("TypeError", "packed address must be bytes-like");
        }
    } else {
        return raise_exception::<_>("TypeError", "packed address must be bytes-like");
    };
    if family == libc::AF_INET {
        if data.len() != 4 {
            return raise_exception::<_>("ValueError", "invalid IPv4 packed length");
        }
        let addr = Ipv4Addr::new(data[0], data[1], data[2], data[3]);
        let text = addr.to_string();
        let ptr = alloc_string(text.as_bytes());
        if ptr.is_null() {
            return MoltObject::none().bits();
        }
        return MoltObject::from_ptr(ptr).bits();
    }
    if family == libc::AF_INET6 {
        if data.len() != 16 {
            return raise_exception::<_>("ValueError", "invalid IPv6 packed length");
        }
        let mut octets = [0u8; 16];
        octets.copy_from_slice(&data[..16]);
        let addr = Ipv6Addr::from(octets);
        let text = addr.to_string();
        let ptr = alloc_string(text.as_bytes());
        if ptr.is_null() {
            return MoltObject::none().bits();
        }
        return MoltObject::from_ptr(ptr).bits();
    }
    return raise_exception::<_>("ValueError", "unsupported address family");
}

#[cfg(target_arch = "wasm32")]
#[no_mangle]
pub extern "C" fn molt_socket_inet_ntop(_family_bits: u64, _packed_bits: u64) -> u64 {
    return raise_exception::<_>("RuntimeError", "inet_ntop unsupported on wasm");
}

#[cfg(not(target_arch = "wasm32"))]
#[no_mangle]
pub extern "C" fn molt_socket_has_ipv6() -> u64 {
    let supported = std::net::TcpListener::bind("[::1]:0").is_ok();
    MoltObject::from_bool(supported).bits()
}

#[cfg(target_arch = "wasm32")]
#[no_mangle]
pub extern "C" fn molt_socket_has_ipv6() -> u64 {
    MoltObject::from_bool(false).bits()
}

#[cfg(not(target_arch = "wasm32"))]
#[no_mangle]
pub unsafe extern "C" fn molt_io_wait(obj_bits: u64) -> i64 {
    let obj_ptr = ptr_from_bits(obj_bits);
    if obj_ptr.is_null() {
        return MoltObject::none().bits() as i64;
    }
    let header = header_from_obj_ptr(obj_ptr);
    let payload_bytes = (*header)
        .size
        .saturating_sub(std::mem::size_of::<MoltHeader>());
    let payload_len = payload_bytes / std::mem::size_of::<u64>();
    if payload_len < 2 {
        return raise_exception::<i64>("TypeError", "io wait payload too small");
    }
    let payload_ptr = obj_ptr as *mut u64;
    let socket_bits = *payload_ptr;
    let events_bits = *payload_ptr.add(1);
    let socket_ptr = socket_ptr_from_bits_or_fd(socket_bits);
    if socket_ptr.is_null() {
        return raise_exception::<i64>("TypeError", "invalid socket");
    }
    let events = to_i64(obj_from_bits(events_bits)).unwrap_or(0) as u32;
    if events == 0 {
        return raise_exception::<i64>("ValueError", "events must be non-zero");
    }
    if (*header).state == 0 {
        let mut timeout: Option<f64> = None;
        if payload_len >= 3 {
            let timeout_bits = *payload_ptr.add(2);
            let timeout_obj = obj_from_bits(timeout_bits);
            if !timeout_obj.is_none() {
                if let Some(val) = to_f64(timeout_obj) {
                    if !val.is_finite() || val < 0.0 {
                        return raise_exception::<i64>(
                            "ValueError",
                            "timeout must be non-negative",
                        );
                    }
                    timeout = Some(val);
                } else {
                    return raise_exception::<i64>("TypeError", "timeout must be float or None");
                }
            }
        }
        if let Some(val) = timeout {
            if val == 0.0 {
                return raise_exception::<i64>("TimeoutError", "timed out");
            }
            let deadline = monotonic_now_secs() + val;
            let deadline_bits = MoltObject::from_float(deadline).bits();
            if payload_len >= 3 {
                dec_ref_bits(*payload_ptr.add(2));
                *payload_ptr.add(2) = deadline_bits;
                inc_ref_bits(deadline_bits);
            }
        }
        if let Err(err) = runtime_state()
            .io_poller()
            .register_wait(obj_ptr, socket_ptr, events)
        {
            return raise_os_error::<i64>(err, "io_wait");
        }
        (*header).state = 1;
        return pending_bits_i64();
    }
    if let Some(mask) = runtime_state().io_poller().take_ready(obj_ptr) {
        let res_bits = MoltObject::from_int(mask as i64).bits();
        return res_bits as i64;
    }
    if payload_len >= 3 {
        let deadline_obj = obj_from_bits(*payload_ptr.add(2));
        if let Some(deadline) = to_f64(deadline_obj) {
            if deadline.is_finite() && monotonic_now_secs() >= deadline {
                runtime_state().io_poller().cancel_waiter(obj_ptr);
                return raise_exception::<i64>("TimeoutError", "timed out");
            }
        }
    }
    pending_bits_i64()
}

#[cfg(not(target_arch = "wasm32"))]
#[no_mangle]
pub extern "C" fn molt_io_wait_new(socket_bits: u64, events_bits: u64, timeout_bits: u64) -> u64 {
    if require_net_capability::<u64>(&["net", "net.poll"]).is_err() {
        return MoltObject::none().bits();
    }
    let socket_ptr = socket_ptr_from_bits_or_fd(socket_bits);
    if socket_ptr.is_null() {
        return raise_exception::<_>("TypeError", "invalid socket");
    }
    let events = match to_i64(obj_from_bits(events_bits)) {
        Some(val) => val,
        None => return raise_exception::<_>("TypeError", "events must be int"),
    };
    if events == 0 {
        return raise_exception::<_>("ValueError", "events must be non-zero");
    }
    let obj_bits = molt_future_new(
        io_wait_poll_fn_addr(),
        (3 * std::mem::size_of::<u64>()) as u64,
    );
    let Some(obj_ptr) = resolve_obj_ptr(obj_bits) else {
        return MoltObject::none().bits();
    };
    unsafe {
        let payload_ptr = obj_ptr as *mut u64;
        *payload_ptr = socket_bits;
        *payload_ptr.add(1) = events_bits;
        *payload_ptr.add(2) = timeout_bits;
        inc_ref_bits(events_bits);
        inc_ref_bits(timeout_bits);
    }
    socket_ref_inc(socket_ptr);
    obj_bits
}

#[cfg(target_arch = "wasm32")]
#[no_mangle]
pub extern "C" fn molt_io_wait_new(
    _socket_bits: u64,
    _events_bits: u64,
    _timeout_bits: u64,
) -> u64 {
    return raise_exception::<_>("RuntimeError", "io wait unsupported on wasm");
}

#[cfg(target_arch = "wasm32")]
#[no_mangle]
pub unsafe extern "C" fn molt_io_wait(_obj_bits: u64) -> i64 {
    // TODO(wasm-parity, owner:runtime, milestone:RT2, priority:P0, status:missing): wire io_wait to wasm host I/O readiness once wasm sockets land.
    pending_bits_i64()
}

#[cfg(not(target_arch = "wasm32"))]
#[no_mangle]
pub unsafe extern "C" fn molt_thread_submit(
    callable_bits: u64,
    args_bits: u64,
    kwargs_bits: u64,
) -> u64 {
    let future_bits = molt_future_new(thread_poll_fn_addr(), 0);
    let Some(future_ptr) = resolve_obj_ptr(future_bits) else {
        return MoltObject::none().bits();
    };
    let state = Arc::new(ThreadTaskState::new(future_ptr));
    runtime_state()
        .thread_tasks
        .lock()
        .unwrap()
        .insert(PtrSlot(future_ptr), Arc::clone(&state));
    inc_ref_bits(callable_bits);
    if !obj_from_bits(args_bits).is_none() {
        inc_ref_bits(args_bits);
    }
    if !obj_from_bits(kwargs_bits).is_none() {
        inc_ref_bits(kwargs_bits);
    }
    runtime_state().thread_pool().submit(ThreadWorkItem {
        task: state,
        callable_bits,
        args_bits,
        kwargs_bits,
    });
    future_bits
}

#[cfg(target_arch = "wasm32")]
#[no_mangle]
pub unsafe extern "C" fn molt_thread_submit(
    _callable_bits: u64,
    _args_bits: u64,
    _kwargs_bits: u64,
) -> u64 {
    raise_exception::<u64>("RuntimeError", "thread submit unsupported on wasm")
}

#[cfg(not(target_arch = "wasm32"))]
#[no_mangle]
pub unsafe extern "C" fn molt_thread_poll(obj_bits: u64) -> i64 {
    let obj_ptr = ptr_from_bits(obj_bits);
    if obj_ptr.is_null() {
        return MoltObject::none().bits() as i64;
    }
    let Some(state) = runtime_state()
        .thread_tasks
        .lock()
        .unwrap()
        .get(&PtrSlot(obj_ptr))
        .cloned()
    else {
        return raise_exception::<i64>("RuntimeError", "thread task missing");
    };
    if state.done.load(AtomicOrdering::Acquire) {
        task_take_cancel_pending(obj_ptr);
    } else if task_cancel_pending(obj_ptr) {
        task_take_cancel_pending(obj_ptr);
        state.cancelled.store(true, AtomicOrdering::Release);
        return raise_cancelled_with_message::<i64>(obj_ptr);
    }
    if !state.done.load(AtomicOrdering::Acquire) {
        return pending_bits_i64();
    }
    if let Some(exc_bits) = state.exception.lock().unwrap().as_ref().copied() {
        let res_bits = molt_raise(exc_bits);
        return res_bits as i64;
    }
    if let Some(result_bits) = state.result.lock().unwrap().as_ref().copied() {
        inc_ref_bits(result_bits);
        return result_bits as i64;
    }
    MoltObject::none().bits() as i64
}

#[cfg(target_arch = "wasm32")]
#[no_mangle]
pub unsafe extern "C" fn molt_thread_poll(_obj_bits: u64) -> i64 {
    pending_bits_i64()
}

// --- Process ---

#[cfg(not(target_arch = "wasm32"))]
const PROCESS_STDIO_INHERIT: i32 = 0;
#[cfg(not(target_arch = "wasm32"))]
const PROCESS_STDIO_PIPE: i32 = 1;
#[cfg(not(target_arch = "wasm32"))]
const PROCESS_STDIO_DEVNULL: i32 = 2;

#[cfg(not(target_arch = "wasm32"))]
fn process_stdio_mode(bits: u64, name: &str) -> i32 {
    let obj = obj_from_bits(bits);
    if obj.is_none() {
        return PROCESS_STDIO_INHERIT;
    }
    match to_i64(obj) {
        Some(val) => {
            let val = val as i32;
            match val {
                PROCESS_STDIO_INHERIT | PROCESS_STDIO_PIPE | PROCESS_STDIO_DEVNULL => val,
                _ => return raise_exception::<_>("ValueError", &format!("invalid {name} mode")),
            }
        }
        None => {
            return raise_exception::<_>("TypeError", &format!("{name} must be int or None"));
        }
    }
}

#[cfg(not(target_arch = "wasm32"))]
fn spawn_process_reader(mut reader: impl Read + Send + 'static, stream_bits: u64) {
    unsafe {
        let _ = molt_stream_clone(stream_bits);
    }
    thread::spawn(move || {
        let stream_ptr = ptr_from_bits(stream_bits);
        if stream_ptr.is_null() {
            return;
        }
        let stream = unsafe { &*(stream_ptr as *mut MoltStream) };
        let sender = stream.sender.clone();
        let mut buf = [0u8; 8192];
        loop {
            match reader.read(&mut buf) {
                Ok(0) => break,
                Ok(n) => {
                    let bytes = buf[..n].to_vec();
                    let _ = sender.send(bytes);
                }
                Err(_) => break,
            }
        }
        stream.closed.store(true, AtomicOrdering::Release);
        unsafe {
            molt_stream_drop(stream_bits);
        }
    });
}

#[cfg(not(target_arch = "wasm32"))]
fn spawn_process_writer(mut writer: impl Write + Send + 'static, stream_bits: u64) {
    unsafe {
        let _ = molt_stream_clone(stream_bits);
    }
    thread::spawn(move || {
        let stream_ptr = ptr_from_bits(stream_bits);
        if stream_ptr.is_null() {
            return;
        }
        let stream = unsafe { &*(stream_ptr as *mut MoltStream) };
        let receiver = stream.receiver.clone();
        loop {
            match receiver.recv_timeout(Duration::from_millis(50)) {
                Ok(bytes) => {
                    if bytes.is_empty() {
                        continue;
                    }
                    let _ = writer.write_all(&bytes);
                    let _ = writer.flush();
                }
                Err(crossbeam_channel::RecvTimeoutError::Timeout) => {
                    if stream.closed.load(AtomicOrdering::Acquire) {
                        break;
                    }
                }
                Err(crossbeam_channel::RecvTimeoutError::Disconnected) => break,
            }
        }
        stream.closed.store(true, AtomicOrdering::Release);
        unsafe {
            molt_stream_drop(stream_bits);
        }
    });
}

#[cfg(not(target_arch = "wasm32"))]
#[no_mangle]
pub unsafe extern "C" fn molt_process_spawn(
    args_bits: u64,
    env_bits: u64,
    cwd_bits: u64,
    stdin_bits: u64,
    stdout_bits: u64,
    stderr_bits: u64,
) -> u64 {
    if require_process_capability::<u64>(&["process", "process.exec"]).is_err() {
        return MoltObject::none().bits();
    }
    let args = match argv_from_bits(args_bits) {
        Ok(val) => val,
        Err(msg) => return raise_exception::<_>("TypeError", &msg),
    };
    if args.is_empty() {
        return raise_exception::<_>("ValueError", "args must not be empty");
    }
    let mut cmd = std::process::Command::new(&args[0]);
    if args.len() > 1 {
        cmd.args(&args[1..]);
    }
    let env_entries = match env_from_bits(env_bits) {
        Ok(val) => val,
        Err(msg) => return raise_exception::<_>("TypeError", &msg),
    };
    if let Some(env_entries) = env_entries {
        cmd.env_clear();
        for (key, value) in env_entries {
            cmd.env(key, value);
        }
    }
    if !obj_from_bits(cwd_bits).is_none() {
        let cwd = match path_from_bits(cwd_bits) {
            Ok(path) => path,
            Err(msg) => return raise_exception::<_>("TypeError", &msg),
        };
        cmd.current_dir(cwd);
    }
    let stdin_mode = process_stdio_mode(stdin_bits, "stdin");
    let stdout_mode = process_stdio_mode(stdout_bits, "stdout");
    let stderr_mode = process_stdio_mode(stderr_bits, "stderr");

    let stdin_stream = if stdin_mode == PROCESS_STDIO_PIPE {
        molt_stream_new(0)
    } else {
        0
    };
    let stdout_stream = if stdout_mode == PROCESS_STDIO_PIPE {
        molt_stream_new(0)
    } else {
        0
    };
    let stderr_stream = if stderr_mode == PROCESS_STDIO_PIPE {
        molt_stream_new(0)
    } else {
        0
    };

    match stdin_mode {
        PROCESS_STDIO_PIPE => cmd.stdin(Stdio::piped()),
        PROCESS_STDIO_DEVNULL => cmd.stdin(Stdio::null()),
        _ => cmd.stdin(Stdio::inherit()),
    };
    match stdout_mode {
        PROCESS_STDIO_PIPE => cmd.stdout(Stdio::piped()),
        PROCESS_STDIO_DEVNULL => cmd.stdout(Stdio::null()),
        _ => cmd.stdout(Stdio::inherit()),
    };
    match stderr_mode {
        PROCESS_STDIO_PIPE => cmd.stderr(Stdio::piped()),
        PROCESS_STDIO_DEVNULL => cmd.stderr(Stdio::null()),
        _ => cmd.stderr(Stdio::inherit()),
    };

    let mut child = match cmd.spawn() {
        Ok(child) => child,
        Err(err) => {
            if stdin_stream != 0 {
                molt_stream_drop(stdin_stream);
            }
            if stdout_stream != 0 {
                molt_stream_drop(stdout_stream);
            }
            if stderr_stream != 0 {
                molt_stream_drop(stderr_stream);
            }
            return raise_os_error::<u64>(err, "spawn");
        }
    };

    if stdin_stream != 0 {
        if let Some(stdin) = child.stdin.take() {
            spawn_process_writer(stdin, stdin_stream);
        }
    }
    if stdout_stream != 0 {
        if let Some(stdout) = child.stdout.take() {
            spawn_process_reader(stdout, stdout_stream);
        }
    }
    if stderr_stream != 0 {
        if let Some(stderr) = child.stderr.take() {
            spawn_process_reader(stderr, stderr_stream);
        }
    }

    let pid = child.id();
    let state = Arc::new(ProcessState {
        child: Mutex::new(child),
        pid,
        exit_code: AtomicI32::new(PROCESS_EXIT_PENDING),
        wait_future: Mutex::new(None),
        stdin_stream,
        stdout_stream,
        stderr_stream,
        wait_lock: Mutex::new(()),
        condvar: Condvar::new(),
    });
    let worker_state = Arc::clone(&state);
    thread::spawn(move || process_wait_worker(worker_state));
    let handle = Box::new(MoltProcessHandle { state });
    bits_from_ptr(Box::into_raw(handle) as *mut u8)
}

#[cfg(not(target_arch = "wasm32"))]
#[no_mangle]
pub unsafe extern "C" fn molt_process_wait_future(proc_bits: u64) -> u64 {
    let proc_ptr = ptr_from_bits(proc_bits);
    if proc_ptr.is_null() {
        return MoltObject::none().bits();
    }
    let handle = &*(proc_ptr as *mut MoltProcessHandle);
    let state = Arc::clone(&handle.state);
    if let Some(existing) = *state.wait_future.lock().unwrap() {
        let bits = MoltObject::from_ptr(existing.0).bits();
        inc_ref_bits(bits);
        return bits;
    }
    let future_bits = molt_future_new(process_poll_fn_addr(), 0);
    let Some(future_ptr) = resolve_obj_ptr(future_bits) else {
        return MoltObject::none().bits();
    };
    let task_state = Arc::new(ProcessTaskState {
        future_ptr,
        process: state,
        cancelled: AtomicBool::new(false),
    });
    runtime_state()
        .process_tasks
        .lock()
        .unwrap()
        .insert(PtrSlot(future_ptr), Arc::clone(&task_state));
    *task_state.process.wait_future.lock().unwrap() = Some(PtrSlot(future_ptr));
    future_bits
}

#[cfg(not(target_arch = "wasm32"))]
#[no_mangle]
pub unsafe extern "C" fn molt_process_poll(obj_bits: u64) -> i64 {
    let obj_ptr = ptr_from_bits(obj_bits);
    if obj_ptr.is_null() {
        return MoltObject::none().bits() as i64;
    }
    let Some(state) = process_task_state(obj_ptr) else {
        return raise_exception::<i64>("RuntimeError", "process task missing");
    };
    if state.process.exit_code.load(AtomicOrdering::Acquire) != PROCESS_EXIT_PENDING {
        task_take_cancel_pending(obj_ptr);
    } else if task_cancel_pending(obj_ptr) {
        task_take_cancel_pending(obj_ptr);
        state.cancelled.store(true, AtomicOrdering::Release);
        return raise_cancelled_with_message::<i64>(obj_ptr);
    }
    let code = state.process.exit_code.load(AtomicOrdering::Acquire);
    if code == PROCESS_EXIT_PENDING {
        return pending_bits_i64();
    }
    MoltObject::from_int(code as i64).bits() as i64
}

#[cfg(target_arch = "wasm32")]
#[no_mangle]
pub unsafe extern "C" fn molt_process_spawn(
    _args_bits: u64,
    _env_bits: u64,
    _cwd_bits: u64,
    _stdin_bits: u64,
    _stdout_bits: u64,
    _stderr_bits: u64,
) -> u64 {
    raise_exception::<u64>("RuntimeError", "process spawn unsupported on wasm")
}

#[cfg(target_arch = "wasm32")]
#[no_mangle]
pub unsafe extern "C" fn molt_process_wait_future(_proc_bits: u64) -> u64 {
    raise_exception::<u64>("RuntimeError", "process wait unsupported on wasm")
}

#[cfg(target_arch = "wasm32")]
#[no_mangle]
pub unsafe extern "C" fn molt_process_poll(_obj_bits: u64) -> i64 {
    pending_bits_i64()
}

#[cfg(not(target_arch = "wasm32"))]
#[no_mangle]
pub unsafe extern "C" fn molt_process_pid(proc_bits: u64) -> u64 {
    let proc_ptr = ptr_from_bits(proc_bits);
    if proc_ptr.is_null() {
        return MoltObject::from_int(0).bits();
    }
    let handle = &*(proc_ptr as *mut MoltProcessHandle);
    MoltObject::from_int(handle.state.pid as i64).bits()
}

#[cfg(not(target_arch = "wasm32"))]
#[no_mangle]
pub unsafe extern "C" fn molt_process_returncode(proc_bits: u64) -> u64 {
    let proc_ptr = ptr_from_bits(proc_bits);
    if proc_ptr.is_null() {
        return MoltObject::none().bits();
    }
    let handle = &*(proc_ptr as *mut MoltProcessHandle);
    let code = handle.state.exit_code.load(AtomicOrdering::Acquire);
    if code == PROCESS_EXIT_PENDING {
        MoltObject::none().bits()
    } else {
        MoltObject::from_int(code as i64).bits()
    }
}

#[cfg(not(target_arch = "wasm32"))]
#[no_mangle]
pub unsafe extern "C" fn molt_process_kill(proc_bits: u64) -> u64 {
    let proc_ptr = ptr_from_bits(proc_bits);
    if proc_ptr.is_null() {
        return MoltObject::none().bits();
    }
    let handle = &*(proc_ptr as *mut MoltProcessHandle);
    if handle.state.exit_code.load(AtomicOrdering::Acquire) != PROCESS_EXIT_PENDING {
        return MoltObject::none().bits();
    }
    let mut guard = handle.state.child.lock().unwrap();
    if let Err(err) = guard.kill() {
        return raise_os_error::<u64>(err, "kill");
    }
    MoltObject::none().bits()
}

#[cfg(not(target_arch = "wasm32"))]
#[no_mangle]
pub unsafe extern "C" fn molt_process_terminate(proc_bits: u64) -> u64 {
    let proc_ptr = ptr_from_bits(proc_bits);
    if proc_ptr.is_null() {
        return MoltObject::none().bits();
    }
    let handle = &*(proc_ptr as *mut MoltProcessHandle);
    if handle.state.exit_code.load(AtomicOrdering::Acquire) != PROCESS_EXIT_PENDING {
        return MoltObject::none().bits();
    }
    #[cfg(unix)]
    {
        let pid = handle.state.pid as i32;
        let res = libc::kill(pid, libc::SIGTERM);
        if res != 0 {
            return raise_os_error::<u64>(std::io::Error::last_os_error(), "terminate");
        }
        return MoltObject::none().bits();
    }
    #[cfg(not(unix))]
    {
        let mut guard = handle.state.child.lock().unwrap();
        if let Err(err) = guard.kill() {
            return raise_os_error::<u64>(err, "terminate");
        }
        MoltObject::none().bits()
    }
}

#[cfg(not(target_arch = "wasm32"))]
#[no_mangle]
pub unsafe extern "C" fn molt_process_stdin(proc_bits: u64) -> u64 {
    let proc_ptr = ptr_from_bits(proc_bits);
    if proc_ptr.is_null() {
        return MoltObject::none().bits();
    }
    let handle = &*(proc_ptr as *mut MoltProcessHandle);
    if handle.state.stdin_stream == 0 {
        return MoltObject::none().bits();
    }
    molt_stream_clone(handle.state.stdin_stream)
}

#[cfg(not(target_arch = "wasm32"))]
#[no_mangle]
pub unsafe extern "C" fn molt_process_stdout(proc_bits: u64) -> u64 {
    let proc_ptr = ptr_from_bits(proc_bits);
    if proc_ptr.is_null() {
        return MoltObject::none().bits();
    }
    let handle = &*(proc_ptr as *mut MoltProcessHandle);
    if handle.state.stdout_stream == 0 {
        return MoltObject::none().bits();
    }
    molt_stream_clone(handle.state.stdout_stream)
}

#[cfg(not(target_arch = "wasm32"))]
#[no_mangle]
pub unsafe extern "C" fn molt_process_stderr(proc_bits: u64) -> u64 {
    let proc_ptr = ptr_from_bits(proc_bits);
    if proc_ptr.is_null() {
        return MoltObject::none().bits();
    }
    let handle = &*(proc_ptr as *mut MoltProcessHandle);
    if handle.state.stderr_stream == 0 {
        return MoltObject::none().bits();
    }
    molt_stream_clone(handle.state.stderr_stream)
}

#[cfg(not(target_arch = "wasm32"))]
#[no_mangle]
pub unsafe extern "C" fn molt_process_drop(proc_bits: u64) {
    let proc_ptr = ptr_from_bits(proc_bits);
    if proc_ptr.is_null() {
        return;
    }
    release_ptr(proc_ptr);
    drop(Box::from_raw(proc_ptr as *mut MoltProcessHandle));
}

// --- IO Poller ---

#[cfg(not(target_arch = "wasm32"))]
struct IoWaiter {
    socket_id: usize,
    events: u32,
}

#[cfg(not(target_arch = "wasm32"))]
struct IoSocketEntry {
    token: Token,
    interests: Interest,
    waiters: Vec<PtrSlot>,
    blocking_waiters: Vec<Arc<BlockingWaiter>>,
}

#[cfg(not(target_arch = "wasm32"))]
struct IoPoller {
    poll: Mutex<Poll>,
    events: Mutex<Events>,
    waker: Waker,
    running: AtomicBool,
    worker: Mutex<Option<thread::JoinHandle<()>>>,
    next_token: AtomicUsize,
    tokens: Mutex<HashMap<Token, usize>>,
    sockets: Mutex<HashMap<usize, IoSocketEntry>>,
    waiters: Mutex<HashMap<PtrSlot, IoWaiter>>,
    ready: Mutex<HashMap<PtrSlot, u32>>,
}

#[cfg(not(target_arch = "wasm32"))]
struct BlockingWaiter {
    events: u32,
    ready: Mutex<Option<u32>>,
    condvar: Condvar,
}

#[cfg(not(target_arch = "wasm32"))]
struct ThreadTaskState {
    future_ptr: *mut u8,
    done: AtomicBool,
    cancelled: AtomicBool,
    result: Mutex<Option<u64>>,
    exception: Mutex<Option<u64>>,
    wait_lock: Mutex<()>,
    condvar: Condvar,
}

// Raw pointers are managed via runtime locks; task state is safe to share across threads.
unsafe impl Send for ThreadTaskState {}
unsafe impl Sync for ThreadTaskState {}

#[cfg(not(target_arch = "wasm32"))]
struct ProcessState {
    child: Mutex<std::process::Child>,
    pid: u32,
    exit_code: AtomicI32,
    wait_future: Mutex<Option<PtrSlot>>,
    stdin_stream: u64,
    stdout_stream: u64,
    stderr_stream: u64,
    wait_lock: Mutex<()>,
    condvar: Condvar,
}

#[cfg(not(target_arch = "wasm32"))]
struct ProcessTaskState {
    future_ptr: *mut u8,
    process: Arc<ProcessState>,
    cancelled: AtomicBool,
}

// Process tasks only touch shared state under locks; safe to share across threads.
unsafe impl Send for ProcessTaskState {}
unsafe impl Sync for ProcessTaskState {}

#[cfg(not(target_arch = "wasm32"))]
struct MoltProcessHandle {
    state: Arc<ProcessState>,
}

#[cfg(not(target_arch = "wasm32"))]
impl Drop for ProcessState {
    fn drop(&mut self) {
        if self.exit_code.load(AtomicOrdering::Acquire) == PROCESS_EXIT_PENDING {
            if let Ok(mut guard) = self.child.lock() {
                let _ = guard.kill();
            }
        }
        if self.stdin_stream != 0 {
            unsafe {
                molt_stream_drop(self.stdin_stream);
            }
        }
        if self.stdout_stream != 0 {
            unsafe {
                molt_stream_drop(self.stdout_stream);
            }
        }
        if self.stderr_stream != 0 {
            unsafe {
                molt_stream_drop(self.stderr_stream);
            }
        }
    }
}

#[cfg(not(target_arch = "wasm32"))]
struct ThreadWorkItem {
    task: Arc<ThreadTaskState>,
    callable_bits: u64,
    args_bits: u64,
    kwargs_bits: u64,
}

#[cfg(not(target_arch = "wasm32"))]
enum ThreadWork {
    Run(ThreadWorkItem),
    Shutdown,
}

#[cfg(not(target_arch = "wasm32"))]
struct ThreadPool {
    sender: Sender<ThreadWork>,
    receiver: Receiver<ThreadWork>,
    handles: Mutex<Vec<thread::JoinHandle<()>>>,
    worker_count: AtomicUsize,
}

#[cfg(not(target_arch = "wasm32"))]
impl ThreadPool {
    fn new() -> Self {
        let (sender, receiver) = unbounded();
        let pool = Self {
            sender,
            receiver,
            handles: Mutex::new(Vec::new()),
            worker_count: AtomicUsize::new(0),
        };
        pool.start_workers();
        pool
    }

    fn start_workers(&self) {
        let count = std::thread::available_parallelism()
            .map(|n| n.get())
            .unwrap_or(4)
            .max(1);
        let mut handles = self.handles.lock().unwrap();
        self.worker_count.store(count, AtomicOrdering::Release);
        for _ in 0..count {
            let rx = self.receiver.clone();
            let handle = thread::spawn(move || thread_worker(rx));
            handles.push(handle);
        }
    }

    fn submit(&self, item: ThreadWorkItem) {
        let _ = self.sender.send(ThreadWork::Run(item));
    }

    fn shutdown(&self) {
        let count = self.worker_count.load(AtomicOrdering::Acquire).max(1);
        for _ in 0..count {
            let _ = self.sender.send(ThreadWork::Shutdown);
        }
        let handles = {
            let mut guard = self.handles.lock().unwrap();
            std::mem::take(&mut *guard)
        };
        for handle in handles {
            let _ = handle.join();
        }
    }
}

#[cfg(not(target_arch = "wasm32"))]
impl ThreadTaskState {
    fn new(future_ptr: *mut u8) -> Self {
        Self {
            future_ptr,
            done: AtomicBool::new(false),
            cancelled: AtomicBool::new(false),
            result: Mutex::new(None),
            exception: Mutex::new(None),
            wait_lock: Mutex::new(()),
            condvar: Condvar::new(),
        }
    }

    fn set_result(&self, bits: u64) {
        let mut guard = self.result.lock().unwrap();
        *guard = Some(bits);
    }

    fn set_exception(&self, bits: u64) {
        let mut guard = self.exception.lock().unwrap();
        *guard = Some(bits);
    }

    fn notify_done(&self) {
        self.done.store(true, AtomicOrdering::Release);
        self.condvar.notify_all();
    }

    fn wait_blocking(&self, timeout: Option<Duration>) {
        if self.done.load(AtomicOrdering::Acquire) {
            return;
        }
        let mut guard = self.wait_lock.lock().unwrap();
        loop {
            if self.done.load(AtomicOrdering::Acquire) {
                break;
            }
            match timeout {
                Some(wait) => {
                    let (next, _) = self.condvar.wait_timeout(guard, wait).unwrap();
                    guard = next;
                    break;
                }
                None => {
                    guard = self.condvar.wait(guard).unwrap();
                }
            }
        }
    }
}

#[cfg(not(target_arch = "wasm32"))]
impl ProcessTaskState {
    fn wait_blocking(&self, timeout: Option<Duration>) {
        if self.process.exit_code.load(AtomicOrdering::Acquire) != PROCESS_EXIT_PENDING {
            return;
        }
        let mut guard = self.process.wait_lock.lock().unwrap();
        loop {
            if self.process.exit_code.load(AtomicOrdering::Acquire) != PROCESS_EXIT_PENDING {
                break;
            }
            match timeout {
                Some(wait) => {
                    let (next, _) = self.process.condvar.wait_timeout(guard, wait).unwrap();
                    guard = next;
                    break;
                }
                None => {
                    guard = self.process.condvar.wait(guard).unwrap();
                }
            }
        }
    }
}

#[cfg(not(target_arch = "wasm32"))]
const PROCESS_EXIT_PENDING: i32 = i32::MIN;

#[cfg(not(target_arch = "wasm32"))]
fn exit_code_from_status(status: std::process::ExitStatus) -> i32 {
    if let Some(code) = status.code() {
        return code;
    }
    #[cfg(unix)]
    {
        use std::os::unix::process::ExitStatusExt;
        if let Some(sig) = status.signal() {
            return -(sig as i32);
        }
    }
    -1
}

#[cfg(not(target_arch = "wasm32"))]
fn process_wait_worker(state: Arc<ProcessState>) {
    loop {
        if state.exit_code.load(AtomicOrdering::Acquire) != PROCESS_EXIT_PENDING {
            break;
        }
        let mut guard = state.child.lock().unwrap();
        match guard.try_wait() {
            Ok(Some(status)) => {
                let code = exit_code_from_status(status);
                state.exit_code.store(code, AtomicOrdering::Release);
                drop(guard);
                state.condvar.notify_all();
                if let Some(future) = state.wait_future.lock().unwrap().take() {
                    let waiters = await_waiters_take(future.0);
                    for waiter in waiters {
                        wake_task_ptr(waiter.0);
                    }
                }
                break;
            }
            Ok(None) => {}
            Err(_) => {}
        }
        drop(guard);
        thread::sleep(Duration::from_millis(10));
    }
}

#[cfg(not(target_arch = "wasm32"))]
fn thread_worker(rx: Receiver<ThreadWork>) {
    loop {
        let work = match rx.recv() {
            Ok(work) => work,
            Err(_) => break,
        };
        match work {
            ThreadWork::Shutdown => break,
            ThreadWork::Run(item) => {
                let _gil = GilGuard::new();
                let ThreadWorkItem {
                    task,
                    callable_bits,
                    args_bits,
                    kwargs_bits,
                } = item;
                let result_bits = call_thread_callable(callable_bits, args_bits, kwargs_bits);
                dec_ref_bits(callable_bits);
                if !obj_from_bits(args_bits).is_none() {
                    dec_ref_bits(args_bits);
                }
                if !obj_from_bits(kwargs_bits).is_none() {
                    dec_ref_bits(kwargs_bits);
                }
                let cancelled = task.cancelled.load(AtomicOrdering::Acquire);
                if exception_pending() {
                    let exc_bits = molt_exception_last();
                    molt_exception_clear();
                    if cancelled {
                        dec_ref_bits(exc_bits);
                    } else {
                        task.set_exception(exc_bits);
                    }
                } else if cancelled {
                    if !obj_from_bits(result_bits).is_none() {
                        dec_ref_bits(result_bits);
                    }
                } else {
                    task.set_result(result_bits);
                }
                task.notify_done();
                let waiters = await_waiters_take(task.future_ptr);
                for waiter in waiters {
                    wake_task_ptr(waiter.0);
                }
            }
        }
    }
    crate::state::clear_worker_thread_state();
}

#[cfg(not(target_arch = "wasm32"))]
fn call_thread_callable(callable_bits: u64, args_bits: u64, kwargs_bits: u64) -> u64 {
    let args_obj = obj_from_bits(args_bits);
    let kwargs_obj = obj_from_bits(kwargs_bits);
    let has_args = !args_obj.is_none();
    let has_kwargs = !kwargs_obj.is_none();
    if !has_args && !has_kwargs {
        return unsafe { call_callable0(callable_bits) };
    }
    let builder_bits = molt_callargs_new(0, 0);
    if builder_bits == 0 {
        return MoltObject::none().bits();
    }
    if has_args {
        let _ = unsafe { molt_callargs_expand_star(builder_bits, args_bits) };
        if exception_pending() {
            dec_ref_bits(builder_bits);
            return MoltObject::none().bits();
        }
    }
    if has_kwargs {
        let _ = unsafe { molt_callargs_expand_kwstar(builder_bits, kwargs_bits) };
        if exception_pending() {
            dec_ref_bits(builder_bits);
            return MoltObject::none().bits();
        }
    }
    molt_call_bind(callable_bits, builder_bits)
}

#[cfg(not(target_arch = "wasm32"))]
impl IoPoller {
    fn new() -> Self {
        let poll = Poll::new().expect("io poller");
        let waker = Waker::new(poll.registry(), Token(0)).expect("io waker");
        Self {
            poll: Mutex::new(poll),
            events: Mutex::new(Events::with_capacity(256)),
            waker,
            running: AtomicBool::new(true),
            worker: Mutex::new(None),
            next_token: AtomicUsize::new(1),
            tokens: Mutex::new(HashMap::new()),
            sockets: Mutex::new(HashMap::new()),
            waiters: Mutex::new(HashMap::new()),
            ready: Mutex::new(HashMap::new()),
        }
    }

    fn start_worker(self: &Arc<Self>) {
        let poller = Arc::clone(self);
        let handle = thread::spawn(move || io_worker(poller));
        let mut guard = self.worker.lock().unwrap();
        *guard = Some(handle);
    }

    fn shutdown(&self) {
        if !self.running.swap(false, AtomicOrdering::SeqCst) {
            return;
        }
        let _ = self.waker.wake();
        let handle = { self.worker.lock().unwrap().take() };
        if let Some(handle) = handle {
            let _ = handle.join();
        }
    }

    fn register_wait(
        &self,
        future_ptr: *mut u8,
        socket_ptr: *mut u8,
        events: u32,
    ) -> Result<(), std::io::Error> {
        if future_ptr.is_null() || socket_ptr.is_null() {
            return Err(std::io::Error::new(
                ErrorKind::InvalidInput,
                "invalid io wait",
            ));
        }
        let waiter_key = PtrSlot(future_ptr);
        {
            let mut waiters = self.waiters.lock().unwrap();
            if waiters.contains_key(&waiter_key) {
                return Ok(());
            }
            waiters.insert(
                waiter_key,
                IoWaiter {
                    socket_id: socket_ptr as usize,
                    events,
                },
            );
        }
        let socket_id = socket_ptr as usize;
        let mut sockets = self.sockets.lock().unwrap();
        let token = sockets
            .get(&socket_id)
            .map(|entry| entry.token)
            .unwrap_or_else(|| {
                let token = Token(self.next_token.fetch_add(1, AtomicOrdering::Relaxed));
                sockets.insert(
                    socket_id,
                    IoSocketEntry {
                        token,
                        interests: Interest::READABLE,
                        waiters: Vec::new(),
                        blocking_waiters: Vec::new(),
                    },
                );
                self.tokens.lock().unwrap().insert(token, socket_id);
                token
            });
        let entry = sockets.get_mut(&socket_id).expect("socket entry");
        if !entry.waiters.contains(&waiter_key) {
            entry.waiters.push(waiter_key);
        }
        let interest = interest_from_events(events);
        let needs_register = entry.waiters.len() == 1;
        let mut updated = false;
        if needs_register {
            entry.interests = interest;
            updated = true;
        } else {
            let new_interest = entry.interests | interest;
            if new_interest != entry.interests {
                entry.interests = new_interest;
                updated = true;
            }
        }
        let interests = entry.interests;
        drop(sockets);
        if needs_register {
            with_socket_mut(socket_ptr, |sock| {
                let source = sock.source_mut().ok_or_else(|| {
                    std::io::Error::new(ErrorKind::InvalidInput, "socket not pollable")
                })?;
                self.poll
                    .lock()
                    .unwrap()
                    .registry()
                    .register(source, token, interests)
            })?;
        } else if updated {
            with_socket_mut(socket_ptr, |sock| {
                let source = sock.source_mut().ok_or_else(|| {
                    std::io::Error::new(ErrorKind::InvalidInput, "socket not pollable")
                })?;
                self.poll
                    .lock()
                    .unwrap()
                    .registry()
                    .reregister(source, token, interests)
            })?;
        }
        let _ = self.waker.wake();
        Ok(())
    }

    fn cancel_waiter(&self, future_ptr: *mut u8) {
        if future_ptr.is_null() {
            return;
        }
        let waiter_key = PtrSlot(future_ptr);
        let mut waiters = self.waiters.lock().unwrap();
        let Some(waiter) = waiters.remove(&waiter_key) else {
            return;
        };
        let mut sockets = self.sockets.lock().unwrap();
        if let Some(entry) = sockets.get_mut(&waiter.socket_id) {
            if let Some(pos) = entry.waiters.iter().position(|val| *val == waiter_key) {
                entry.waiters.swap_remove(pos);
            }
            if entry.waiters.is_empty() {
                let token = entry.token;
                sockets.remove(&waiter.socket_id);
                self.tokens.lock().unwrap().remove(&token);
                drop(sockets);
                let _ = with_socket_mut(waiter.socket_id as *mut u8, |sock| {
                    let source = sock.source_mut().ok_or_else(|| {
                        std::io::Error::new(ErrorKind::InvalidInput, "socket not pollable")
                    })?;
                    self.poll.lock().unwrap().registry().deregister(source)
                });
            }
        }
    }

    fn mark_ready(&self, future_ptr: PtrSlot, ready: u32) {
        let mut ready_map = self.ready.lock().unwrap();
        ready_map
            .entry(future_ptr)
            .and_modify(|val| *val |= ready)
            .or_insert(ready);
    }

    fn take_ready(&self, future_ptr: *mut u8) -> Option<u32> {
        if future_ptr.is_null() {
            return None;
        }
        let mut ready_map = self.ready.lock().unwrap();
        ready_map.remove(&PtrSlot(future_ptr))
    }

    fn socket_for_token(&self, token: Token) -> Option<usize> {
        let tokens = self.tokens.lock().unwrap();
        tokens.get(&token).copied()
    }

    fn deregister_socket(&self, socket_ptr: *mut u8) {
        if socket_ptr.is_null() {
            return;
        }
        let socket_id = socket_ptr as usize;
        let mut waiters = self.waiters.lock().unwrap();
        let mut sockets = self.sockets.lock().unwrap();
        let entry = sockets.remove(&socket_id);
        if let Some(entry) = entry {
            self.tokens.lock().unwrap().remove(&entry.token);
            let mut ready_futures: Vec<PtrSlot> = Vec::new();
            for waiter in entry.waiters {
                waiters.remove(&waiter);
                ready_futures.push(waiter);
            }
            for waiter in entry.blocking_waiters {
                let mut guard = waiter.ready.lock().unwrap();
                *guard = Some(IO_EVENT_ERROR);
                drop(guard);
                waiter.condvar.notify_all();
            }
            drop(waiters);
            drop(sockets);
            let _ = with_socket_mut(socket_ptr, |sock| {
                let source = sock.source_mut().ok_or_else(|| {
                    std::io::Error::new(ErrorKind::InvalidInput, "socket not pollable")
                })?;
                self.poll.lock().unwrap().registry().deregister(source)
            });
            for future in ready_futures {
                self.mark_ready(future, IO_EVENT_ERROR);
                let tasks = await_waiters_take(future.0);
                for waiter in tasks {
                    wake_task_ptr(waiter.0);
                }
            }
        }
    }

    fn wait_blocking(
        &self,
        socket_ptr: *mut u8,
        events: u32,
        timeout: Option<Duration>,
    ) -> Result<u32, std::io::Error> {
        if socket_ptr.is_null() {
            return Err(std::io::Error::new(
                ErrorKind::InvalidInput,
                "invalid socket",
            ));
        }
        let waiter = Arc::new(BlockingWaiter {
            events,
            ready: Mutex::new(None),
            condvar: Condvar::new(),
        });
        let waiter_id = Arc::as_ptr(&waiter) as usize;
        let socket_id = socket_ptr as usize;
        let mut sockets = self.sockets.lock().unwrap();
        let token = sockets
            .get(&socket_id)
            .map(|entry| entry.token)
            .unwrap_or_else(|| {
                let token = Token(self.next_token.fetch_add(1, AtomicOrdering::Relaxed));
                sockets.insert(
                    socket_id,
                    IoSocketEntry {
                        token,
                        interests: Interest::READABLE,
                        waiters: Vec::new(),
                        blocking_waiters: Vec::new(),
                    },
                );
                self.tokens.lock().unwrap().insert(token, socket_id);
                token
            });
        let entry = sockets.get_mut(&socket_id).expect("socket entry");
        entry.blocking_waiters.push(Arc::clone(&waiter));
        let interest = interest_from_events(events);
        let mut updated = false;
        let needs_register = entry.waiters.is_empty() && entry.blocking_waiters.len() == 1;
        if needs_register {
            entry.interests = interest;
            updated = true;
        } else {
            let new_interest = entry.interests | interest;
            if new_interest != entry.interests {
                entry.interests = new_interest;
                updated = true;
            }
        }
        let interests = entry.interests;
        drop(sockets);
        if updated {
            with_socket_mut(socket_ptr, |sock| {
                if needs_register {
                    {
                        let source = sock.source_mut().ok_or_else(|| {
                            std::io::Error::new(ErrorKind::InvalidInput, "socket not pollable")
                        })?;
                        self.poll
                            .lock()
                            .unwrap()
                            .registry()
                            .register(source, token, interests)
                    }
                } else {
                    {
                        let source = sock.source_mut().ok_or_else(|| {
                            std::io::Error::new(ErrorKind::InvalidInput, "socket not pollable")
                        })?;
                        self.poll
                            .lock()
                            .unwrap()
                            .registry()
                            .reregister(source, token, interests)
                    }
                }
            })?;
        }
        let _ = self.waker.wake();
        let deadline = timeout.map(|dur| Instant::now() + dur);
        let mut guard = waiter.ready.lock().unwrap();
        loop {
            if let Some(ready) = *guard {
                return Ok(ready);
            }
            if let Some(deadline) = deadline {
                let now = Instant::now();
                if now >= deadline {
                    break;
                }
                let timeout = deadline - now;
                let (next, _) = waiter.condvar.wait_timeout(guard, timeout).unwrap();
                guard = next;
            } else {
                guard = waiter.condvar.wait(guard).unwrap();
            }
        }
        drop(guard);
        let mut sockets = self.sockets.lock().unwrap();
        if let Some(entry) = sockets.get_mut(&socket_id) {
            entry
                .blocking_waiters
                .retain(|candidate| Arc::as_ptr(candidate) as usize != waiter_id);
            if entry.waiters.is_empty() && entry.blocking_waiters.is_empty() {
                let token = entry.token;
                sockets.remove(&socket_id);
                self.tokens.lock().unwrap().remove(&token);
                drop(sockets);
                let _ = with_socket_mut(socket_ptr, |sock| {
                    let source = sock.source_mut().ok_or_else(|| {
                        std::io::Error::new(ErrorKind::InvalidInput, "socket not pollable")
                    })?;
                    self.poll.lock().unwrap().registry().deregister(source)
                });
            }
        }
        Err(std::io::Error::new(ErrorKind::TimedOut, "timed out"))
    }
}

#[cfg(not(target_arch = "wasm32"))]
fn interest_from_events(events: u32) -> Interest {
    let mut interest = None;
    if (events & IO_EVENT_READ) != 0 {
        interest = Some(Interest::READABLE);
    }
    if (events & IO_EVENT_WRITE) != 0 {
        interest = Some(match interest {
            Some(existing) => existing | Interest::WRITABLE,
            None => Interest::WRITABLE,
        });
    }
    interest.unwrap_or(Interest::READABLE)
}

#[cfg(not(target_arch = "wasm32"))]
fn io_worker(poller: Arc<IoPoller>) {
    loop {
        if !poller.running.load(AtomicOrdering::Acquire) {
            break;
        }
        let mut events = poller.events.lock().unwrap();
        let _ = poller
            .poll
            .lock()
            .unwrap()
            .poll(&mut *events, Some(Duration::from_millis(250)));
        if !poller.running.load(AtomicOrdering::Acquire) {
            break;
        }
        let mut ready_futures: Vec<(PtrSlot, u32)> = Vec::new();
        {
            let mut waiters = poller.waiters.lock().unwrap();
            let mut sockets = poller.sockets.lock().unwrap();
            for event in events.iter() {
                if event.token() == Token(0) {
                    continue;
                }
                let Some(socket_id) = poller.socket_for_token(event.token()) else {
                    continue;
                };
                let Some(entry) = sockets.get_mut(&socket_id) else {
                    continue;
                };
                let mut ready_mask = 0;
                if event.is_readable() {
                    ready_mask |= IO_EVENT_READ;
                }
                if event.is_writable() {
                    ready_mask |= IO_EVENT_WRITE;
                }
                if event.is_error() || event.is_read_closed() || event.is_write_closed() {
                    ready_mask |= IO_EVENT_ERROR | IO_EVENT_READ | IO_EVENT_WRITE;
                }
                if ready_mask == 0 {
                    continue;
                }
                let mut remaining: Vec<PtrSlot> = Vec::with_capacity(entry.waiters.len());
                for waiter in entry.waiters.drain(..) {
                    if let Some(info) = waiters.get(&waiter) {
                        if (info.events & ready_mask) != 0 {
                            ready_futures.push((waiter, ready_mask));
                            waiters.remove(&waiter);
                        } else {
                            remaining.push(waiter);
                        }
                    }
                }
                entry.waiters = remaining;
                if !entry.blocking_waiters.is_empty() {
                    let mut remaining_blocking: Vec<Arc<BlockingWaiter>> =
                        Vec::with_capacity(entry.blocking_waiters.len());
                    for waiter in entry.blocking_waiters.drain(..) {
                        if (waiter.events & ready_mask) != 0 {
                            let mut guard = waiter.ready.lock().unwrap();
                            *guard = Some(ready_mask);
                            drop(guard);
                            waiter.condvar.notify_all();
                        } else {
                            remaining_blocking.push(waiter);
                        }
                    }
                    entry.blocking_waiters = remaining_blocking;
                }
            }
        }
        drop(events);
        for (future, mask) in ready_futures {
            poller.mark_ready(future, mask);
            let waiters = await_waiters_take(future.0);
            for waiter in waiters {
                wake_task_ptr(waiter.0);
            }
        }
    }
}

thread_local! {
    static ATTR_NAME_TLS: RefCell<Option<AttrNameCacheEntry>> = const { RefCell::new(None) };
    static DESCRIPTOR_CACHE_TLS: RefCell<Option<DescriptorCacheEntry>> = const { RefCell::new(None) };
    static UTF8_COUNT_TLS: RefCell<Option<Utf8CountCacheEntry>> = const { RefCell::new(None) };
    static BLOCK_ON_TASK: Cell<*mut u8> = const { Cell::new(std::ptr::null_mut()) };
}

/// # Safety
/// `parent_bits` must be either `None` or an integer token id.
#[no_mangle]
pub unsafe extern "C" fn molt_cancel_token_new(parent_bits: u64) -> u64 {
    cancel_tokens();
    let parent_id = match token_id_from_bits(parent_bits) {
        Some(0) => current_token_id(),
        Some(id) => id,
        None => {
            return raise_exception::<_>("TypeError", "cancel token parent must be int or None")
        }
    };
    let id = NEXT_CANCEL_TOKEN_ID.fetch_add(1, AtomicOrdering::Relaxed);
    let mut map = cancel_tokens().lock().unwrap();
    map.insert(
        id,
        CancelTokenEntry {
            parent: parent_id,
            cancelled: false,
            refs: 1,
        },
    );
    MoltObject::from_int(id as i64).bits()
}

/// # Safety
/// `token_bits` must be an integer token id.
#[no_mangle]
pub unsafe extern "C" fn molt_cancel_token_clone(token_bits: u64) -> u64 {
    let id = match token_id_from_bits(token_bits) {
        Some(id) => id,
        None => return raise_exception::<_>("TypeError", "cancel token id must be int"),
    };
    retain_token(id);
    MoltObject::none().bits()
}

/// # Safety
/// `token_bits` must be an integer token id.
#[no_mangle]
pub unsafe extern "C" fn molt_cancel_token_drop(token_bits: u64) -> u64 {
    let id = match token_id_from_bits(token_bits) {
        Some(id) => id,
        None => return raise_exception::<_>("TypeError", "cancel token id must be int"),
    };
    release_token(id);
    MoltObject::none().bits()
}

/// # Safety
/// `token_bits` must be an integer token id.
#[no_mangle]
pub unsafe extern "C" fn molt_cancel_token_cancel(token_bits: u64) -> u64 {
    let id = match token_id_from_bits(token_bits) {
        Some(id) => id,
        None => return raise_exception::<_>("TypeError", "cancel token id must be int"),
    };
    let mut map = cancel_tokens().lock().unwrap();
    if let Some(entry) = map.get_mut(&id) {
        entry.cancelled = true;
    }
    drop(map);
    wake_tasks_for_cancelled_tokens();
    MoltObject::none().bits()
}

/// # Safety
/// `token_bits` must be an integer token id.
#[no_mangle]
pub unsafe extern "C" fn molt_cancel_token_is_cancelled(token_bits: u64) -> u64 {
    let id = match token_id_from_bits(token_bits) {
        Some(id) => id,
        None => return raise_exception::<_>("TypeError", "cancel token id must be int"),
    };
    MoltObject::from_bool(token_is_cancelled(id)).bits()
}

/// # Safety
/// `token_bits` must be an integer token id or `None`.
#[no_mangle]
pub unsafe extern "C" fn molt_cancel_token_set_current(token_bits: u64) -> u64 {
    let id = match token_id_from_bits(token_bits) {
        Some(0) => 1,
        Some(id) => id,
        None => return raise_exception::<_>("TypeError", "cancel token id must be int"),
    };
    let prev = set_current_token(id);
    CURRENT_TASK.with(|cell| {
        let task = cell.get();
        if !task.is_null() {
            register_task_token(task, id);
        }
    });
    MoltObject::from_int(prev as i64).bits()
}

/// # Safety
/// Requires the cancel token tables to be initialized by the runtime.
#[no_mangle]
pub unsafe extern "C" fn molt_cancel_token_get_current() -> u64 {
    cancel_tokens();
    MoltObject::from_int(current_token_id() as i64).bits()
}

/// # Safety
/// Requires the cancel token tables to be initialized by the runtime.
#[no_mangle]
pub unsafe extern "C" fn molt_cancelled() -> u64 {
    cancel_tokens();
    MoltObject::from_bool(token_is_cancelled(current_token_id())).bits()
}

/// # Safety
/// Requires the cancel token tables to be initialized by the runtime.
#[no_mangle]
pub unsafe extern "C" fn molt_cancel_current() -> u64 {
    cancel_tokens();
    let id = current_token_id();
    let mut map = cancel_tokens().lock().unwrap();
    if let Some(entry) = map.get_mut(&id) {
        entry.cancelled = true;
    }
    drop(map);
    wake_tasks_for_cancelled_tokens();
    MoltObject::none().bits()
}

/// # Safety
/// - `task_bits` must be a valid pointer to a Molt task with a valid header.
#[no_mangle]
pub unsafe extern "C" fn molt_spawn(task_bits: u64) {
    let Some(task_ptr) = resolve_task_ptr(task_bits) else {
        return raise_exception::<_>("TypeError", "object is not awaitable");
    };
    cancel_tokens();
    let token = current_token_id();
    register_task_token(task_ptr, token);
    let header = task_ptr.sub(std::mem::size_of::<MoltHeader>()) as *mut MoltHeader;
    if ((*header).flags & HEADER_FLAG_SPAWN_RETAIN) == 0 {
        (*header).flags |= HEADER_FLAG_SPAWN_RETAIN;
        inc_ref_bits(MoltObject::from_ptr(task_ptr).bits());
    }
    runtime_state().scheduler().enqueue(MoltTask {
        future_ptr: task_ptr,
    });
}

#[cfg(not(target_arch = "wasm32"))]
fn is_block_on_task(task_ptr: *mut u8) -> bool {
    BLOCK_ON_TASK.with(|cell| cell.get() == task_ptr)
}

/// # Safety
/// - `task_bits` must be a valid pointer to a Molt task with a valid header.
#[no_mangle]
pub unsafe extern "C" fn molt_block_on(task_bits: u64) -> i64 {
    let Some(task_ptr) = resolve_task_ptr(task_bits) else {
        return raise_exception::<_>("TypeError", "object is not awaitable");
    };
    cancel_tokens();
    let header = task_ptr.sub(std::mem::size_of::<MoltHeader>()) as *mut MoltHeader;
    let poll_fn_addr = (*header).poll_fn;
    if poll_fn_addr == 0 {
        return 0;
    }
    let prev_task = CURRENT_TASK.with(|cell| {
        let prev = cell.get();
        cell.set(task_ptr);
        prev
    });
    let token = ensure_task_token(task_ptr, current_token_id());
    let prev_token = set_current_token(token);
    let caller_depth = exception_stack_depth();
    let caller_handlers = EXCEPTION_STACK.with(|stack| std::mem::take(&mut *stack.borrow_mut()));
    let caller_active =
        ACTIVE_EXCEPTION_STACK.with(|stack| std::mem::take(&mut *stack.borrow_mut()));
    let caller_context = caller_active
        .last()
        .copied()
        .unwrap_or(MoltObject::none().bits());
    exception_context_fallback_push(caller_context);
    let task_handlers = task_exception_handler_stack_take(task_ptr);
    EXCEPTION_STACK.with(|stack| {
        *stack.borrow_mut() = task_handlers;
    });
    let task_active = task_exception_stack_take(task_ptr);
    ACTIVE_EXCEPTION_STACK.with(|stack| {
        *stack.borrow_mut() = task_active;
    });
    let task_depth = task_exception_depth_take(task_ptr);
    exception_stack_set_depth(task_depth);
    BLOCK_ON_TASK.with(|cell| cell.set(task_ptr));
    let prev_raise = task_raise_active();
    set_task_raise_active(true);
    let result = loop {
        let mut res = {
            let _gil = GilGuard::new();
            call_poll_fn(poll_fn_addr, task_ptr)
        };
        if task_cancel_pending(task_ptr) {
            task_take_cancel_pending(task_ptr);
            res = raise_cancelled_with_message::<i64>(task_ptr);
        }
        let pending = res == pending_bits_i64();
        record_async_poll(task_ptr, pending, "block_on");
        if pending {
            let deadline = runtime_state()
                .sleep_queue()
                .take_blocking_deadline(task_ptr);
            if let Some(awaited_ptr) = task_waiting_on_future(task_ptr) {
                if block_on_wait_event(awaited_ptr, deadline) {
                    continue;
                }
            }
            if let Some(deadline) = deadline {
                let now = Instant::now();
                if deadline > now {
                    std::thread::sleep(deadline - now);
                }
            } else {
                std::thread::sleep(Duration::from_micros(50));
            }
            continue;
        }
        break res;
    };
    let new_depth = exception_stack_depth();
    task_exception_depth_store(task_ptr, new_depth);
    exception_context_align_depth(new_depth);
    let task_handlers = EXCEPTION_STACK.with(|stack| std::mem::take(&mut *stack.borrow_mut()));
    task_exception_handler_stack_store(task_ptr, task_handlers);
    let task_active = ACTIVE_EXCEPTION_STACK.with(|stack| std::mem::take(&mut *stack.borrow_mut()));
    task_exception_stack_store(task_ptr, task_active);
    ACTIVE_EXCEPTION_STACK.with(|stack| {
        *stack.borrow_mut() = caller_active;
    });
    EXCEPTION_STACK.with(|stack| {
        *stack.borrow_mut() = caller_handlers;
    });
    exception_stack_set_depth(caller_depth);
    exception_context_fallback_pop();
    BLOCK_ON_TASK.with(|cell| cell.set(std::ptr::null_mut()));
    set_task_raise_active(prev_raise);
    set_current_token(prev_token);
    CURRENT_TASK.with(|cell| cell.set(prev_task));
    clear_task_token(task_ptr);
    result
}

#[no_mangle]
pub extern "C" fn molt_future_poll_fn(future_bits: u64) -> u64 {
    let obj = obj_from_bits(future_bits);
    let Some(ptr) = obj.as_ptr() else {
        if std::env::var("MOLT_DEBUG_AWAITABLE").is_ok() {
            eprintln!(
                "Molt awaitable debug: bits=0x{:x} type={}",
                future_bits,
                type_name(obj)
            );
        }
        raise_exception::<()>("TypeError", "object is not awaitable");
        return 0;
    };
    unsafe {
        let _gil = GilGuard::new();
        let header = header_from_obj_ptr(ptr);
        let poll_fn_addr = (*header).poll_fn;
        if poll_fn_addr == 0 {
            if std::env::var("MOLT_DEBUG_AWAITABLE").is_ok() {
                let mut class_name = None;
                if object_type_id(ptr) == TYPE_ID_OBJECT {
                    let class_bits = object_class_bits(ptr);
                    if class_bits != 0 {
                        class_name = Some(class_name_for_error(class_bits));
                    }
                }
                eprintln!(
                    "Molt awaitable debug: bits=0x{:x} type={} class={} poll=0x0 state={} size={}",
                    future_bits,
                    type_name(obj),
                    class_name.as_deref().unwrap_or("-"),
                    (*header).state,
                    (*header).size
                );
            }
            raise_exception::<()>("TypeError", "object is not awaitable");
            return 0;
        }
        poll_fn_addr
    }
}

#[no_mangle]
pub extern "C" fn molt_future_poll(future_bits: u64) -> i64 {
    let obj = obj_from_bits(future_bits);
    let Some(ptr) = obj.as_ptr() else {
        raise_exception::<i64>("TypeError", "object is not awaitable");
        return 0;
    };
    unsafe {
        let header = header_from_obj_ptr(ptr);
        let poll_fn_addr = (*header).poll_fn;
        if poll_fn_addr == 0 {
            raise_exception::<i64>("TypeError", "object is not awaitable");
            return 0;
        }
        let res = poll_future_with_task_stack(ptr, poll_fn_addr);
        let current_task = current_task_ptr();
        if res == pending_bits_i64() {
            if !current_task.is_null() && ptr != current_task {
                await_waiter_register(current_task, ptr);
            }
        } else if !current_task.is_null() {
            await_waiter_clear(current_task);
        }
        if !current_task.is_null() {
            let current_cancelled = task_cancel_pending(current_task);
            if current_cancelled {
                task_take_cancel_pending(current_task);
                return raise_cancelled_with_message::<i64>(current_task);
            }
        }
        if res != pending_bits_i64() && !current_task.is_null() && ptr != current_task {
            let awaited_exception = {
                let guard = task_last_exceptions().lock().unwrap();
                guard.get(&PtrSlot(ptr)).copied()
            };
            if let Some(exc_ptr) = awaited_exception {
                record_exception(exc_ptr.0);
            } else {
                let prev_task = CURRENT_TASK.with(|cell| {
                    let prev = cell.get();
                    cell.set(ptr);
                    prev
                });
                let exc_bits = if exception_pending() {
                    molt_exception_last()
                } else {
                    MoltObject::none().bits()
                };
                CURRENT_TASK.with(|cell| cell.set(prev_task));
                if let Some(exc_ptr) = maybe_ptr_from_bits(exc_bits) {
                    record_exception(exc_ptr);
                }
                if !obj_from_bits(exc_bits).is_none() {
                    dec_ref_bits(exc_bits);
                }
            }
        }
        if res != pending_bits_i64() && !task_has_token(ptr) {
            task_exception_stack_drop(ptr);
            task_exception_depth_drop(ptr);
        }
        res
    }
}

fn cancel_future_task(task_ptr: *mut u8, msg_bits: Option<u64>) {
    if task_ptr.is_null() {
        return;
    }
    match msg_bits {
        Some(bits) => task_cancel_message_set(task_ptr, bits),
        None => task_cancel_message_clear(task_ptr),
    }
    task_set_cancel_pending(task_ptr);
    unsafe {
        let header = header_from_obj_ptr(task_ptr);
        let poll_fn = (*header).poll_fn;
        if poll_fn == thread_poll_fn_addr() {
            #[cfg(not(target_arch = "wasm32"))]
            if let Some(state) = thread_task_state(task_ptr) {
                state.cancelled.store(true, AtomicOrdering::Release);
                state.condvar.notify_all();
            }
        }
        if poll_fn == process_poll_fn_addr() {
            #[cfg(not(target_arch = "wasm32"))]
            if let Some(state) = process_task_state(task_ptr) {
                state.cancelled.store(true, AtomicOrdering::Release);
                state.process.condvar.notify_all();
            }
        }
        if poll_fn == io_wait_poll_fn_addr() {
            #[cfg(not(target_arch = "wasm32"))]
            runtime_state().io_poller().cancel_waiter(task_ptr);
        }
        if ((*header).flags & HEADER_FLAG_SPAWN_RETAIN) != 0 {
            wake_task_ptr(task_ptr);
        }
    }
    let waiters = await_waiters_take(task_ptr);
    for waiter in waiters {
        wake_task_ptr(waiter.0);
    }
}

/// # Safety
/// - `future_bits` must be a valid pointer to a Molt future.
#[no_mangle]
pub unsafe extern "C" fn molt_future_cancel(future_bits: u64) -> u64 {
    let Some(task_ptr) = resolve_task_ptr(future_bits) else {
        return raise_exception::<_>("TypeError", "object is not awaitable");
    };
    cancel_future_task(task_ptr, None);
    MoltObject::none().bits()
}

/// # Safety
/// - `future_bits` must be a valid pointer to a Molt future.
#[no_mangle]
pub unsafe extern "C" fn molt_future_cancel_msg(future_bits: u64, msg_bits: u64) -> u64 {
    let Some(task_ptr) = resolve_task_ptr(future_bits) else {
        return raise_exception::<_>("TypeError", "object is not awaitable");
    };
    cancel_future_task(task_ptr, Some(msg_bits));
    MoltObject::none().bits()
}

/// # Safety
/// - `future_bits` must be a valid pointer to a Molt future.
#[no_mangle]
pub unsafe extern "C" fn molt_future_cancel_clear(future_bits: u64) -> u64 {
    let Some(task_ptr) = resolve_task_ptr(future_bits) else {
        return raise_exception::<_>("TypeError", "object is not awaitable");
    };
    task_cancel_message_clear(task_ptr);
    MoltObject::none().bits()
}

#[no_mangle]
pub extern "C" fn molt_future_new(poll_fn_addr: u64, closure_size: u64) -> u64 {
    let obj_bits = molt_task_new(poll_fn_addr, closure_size, TASK_KIND_FUTURE);
    if std::env::var("MOLT_DEBUG_AWAITABLE").is_ok() {
        if let Some(obj_ptr) = resolve_obj_ptr(obj_bits) {
            unsafe {
                let header = header_from_obj_ptr(obj_ptr);
                eprintln!(
                    "Molt future init debug: bits=0x{:x} poll=0x{:x} size={}",
                    obj_bits,
                    poll_fn_addr,
                    (*header).size
                );
            }
        }
    }
    obj_bits
}

#[no_mangle]
pub extern "C" fn molt_async_sleep_new(delay_bits: u64, result_bits: u64) -> u64 {
    let obj_bits = molt_future_new(
        async_sleep_poll_fn_addr(),
        (2 * std::mem::size_of::<u64>()) as u64,
    );
    let Some(obj_ptr) = resolve_obj_ptr(obj_bits) else {
        return MoltObject::none().bits();
    };
    unsafe {
        let payload_ptr = obj_ptr as *mut u64;
        *payload_ptr = delay_bits;
        *payload_ptr.add(1) = result_bits;
        inc_ref_bits(delay_bits);
        inc_ref_bits(result_bits);
    }
    obj_bits
}

/// # Safety
/// - `obj_bits` must be a valid pointer if the runtime associates a future with it.
#[no_mangle]
pub unsafe extern "C" fn molt_async_sleep(obj_bits: u64) -> i64 {
    let _obj_ptr = ptr_from_bits(obj_bits);
    if _obj_ptr.is_null() {
        return MoltObject::none().bits() as i64;
    }
    let task_ptr = current_task_ptr();
    if !task_ptr.is_null() && task_cancel_pending(task_ptr) {
        task_take_cancel_pending(task_ptr);
        return raise_cancelled_with_message::<i64>(task_ptr);
    }
    let header = header_from_obj_ptr(_obj_ptr);
    let payload_bytes = (*header)
        .size
        .saturating_sub(std::mem::size_of::<MoltHeader>());
    let payload_len = payload_bytes / std::mem::size_of::<u64>();
    let payload_ptr = _obj_ptr as *mut u64;
    if (*header).state == 0 {
        let delay_secs = if payload_len >= 1 {
            let delay_bits = *payload_ptr;
            let float_bits = molt_float_from_obj(delay_bits);
            let delay_obj = obj_from_bits(float_bits);
            delay_obj.as_float().unwrap_or(0.0)
        } else {
            0.0
        };
        let delay_secs = if delay_secs.is_finite() && delay_secs > 0.0 {
            delay_secs
        } else {
            0.0
        };
        if payload_len >= 1 {
            let deadline = monotonic_now_secs() + delay_secs;
            *payload_ptr = MoltObject::from_float(deadline).bits();
        }
        (*header).state = 1;
        return pending_bits_i64();
    }

    if payload_len >= 1 {
        let deadline_obj = obj_from_bits(*payload_ptr);
        if let Some(deadline) = to_f64(deadline_obj) {
            if deadline.is_finite() && monotonic_now_secs() < deadline {
                return pending_bits_i64();
            }
        }
    }

    let result_bits = if payload_len >= 2 {
        *payload_ptr.add(1)
    } else {
        MoltObject::none().bits()
    };
    inc_ref_bits(result_bits);
    result_bits as i64
}

/// # Safety
/// - `obj_bits` must be a valid pointer to a Molt future allocated with payload slots.
#[no_mangle]
pub unsafe extern "C" fn molt_anext_default_poll(obj_bits: u64) -> i64 {
    let _obj_ptr = ptr_from_bits(obj_bits);
    if _obj_ptr.is_null() {
        return MoltObject::none().bits() as i64;
    }
    let header = header_from_obj_ptr(_obj_ptr);
    let payload_bytes = (*header)
        .size
        .saturating_sub(std::mem::size_of::<MoltHeader>());
    if payload_bytes < 3 * std::mem::size_of::<u64>() {
        return MoltObject::none().bits() as i64;
    }
    let payload_ptr = _obj_ptr as *mut u64;
    let iter_bits = *payload_ptr;
    let default_bits = *payload_ptr.add(1);
    if (*header).state == 0 {
        let await_bits = molt_anext(iter_bits);
        inc_ref_bits(await_bits);
        *payload_ptr.add(2) = await_bits;
        (*header).state = 1;
    }
    let await_bits = *payload_ptr.add(2);
    let Some(await_ptr) = maybe_ptr_from_bits(await_bits) else {
        return MoltObject::none().bits() as i64;
    };
    let await_header = header_from_obj_ptr(await_ptr);
    let poll_fn_addr = (*await_header).poll_fn;
    if poll_fn_addr == 0 {
        return MoltObject::none().bits() as i64;
    }
    let res = molt_future_poll(await_bits);
    if res == pending_bits_i64() {
        return res;
    }
    if exception_pending() {
        let exc_bits = molt_exception_last();
        let kind_bits = molt_exception_kind(exc_bits);
        let kind = string_obj_to_owned(obj_from_bits(kind_bits));
        dec_ref_bits(kind_bits);
        if kind.as_deref() == Some("StopAsyncIteration") {
            molt_exception_clear();
            dec_ref_bits(exc_bits);
            inc_ref_bits(default_bits);
            return default_bits as i64;
        }
        dec_ref_bits(exc_bits);
    }
    res
}

/// # Safety
/// - `task_ptr` must be a valid Molt task pointer.
/// - `future_ptr` must point to a valid Molt future.
#[no_mangle]
pub unsafe extern "C" fn molt_sleep_register(task_ptr: *mut u8, future_ptr: *mut u8) -> u64 {
    if task_ptr.is_null() || future_ptr.is_null() {
        return 0;
    }
    let header = header_from_obj_ptr(future_ptr);
    let poll_fn = (*header).poll_fn;
    if poll_fn != async_sleep_poll_fn_addr() && poll_fn != io_wait_poll_fn_addr() {
        return 0;
    }
    if (*header).state == 0 {
        return 0;
    }
    let payload_bytes = (*header)
        .size
        .saturating_sub(std::mem::size_of::<MoltHeader>());
    let payload_ptr = future_ptr as *mut u64;
    let deadline_obj = if poll_fn == async_sleep_poll_fn_addr() {
        if payload_bytes < std::mem::size_of::<u64>() {
            return 0;
        }
        obj_from_bits(*payload_ptr)
    } else {
        if payload_bytes < 3 * std::mem::size_of::<u64>() {
            return 0;
        }
        obj_from_bits(*payload_ptr.add(2))
    };
    let Some(deadline_secs) = to_f64(deadline_obj) else {
        return 0;
    };
    if !deadline_secs.is_finite() {
        return 0;
    }
    let deadline = instant_from_monotonic_secs(deadline_secs);
    if deadline <= Instant::now() {
        return 0;
    }
    #[cfg(target_arch = "wasm32")]
    {
        runtime_state()
            .sleep_queue()
            .register_blocking(task_ptr, deadline);
        1
    }
    #[cfg(not(target_arch = "wasm32"))]
    {
        if is_block_on_task(task_ptr) {
            runtime_state()
                .sleep_queue()
                .register_blocking(task_ptr, deadline);
        } else {
            runtime_state()
                .sleep_queue()
                .register_scheduler(task_ptr, deadline);
        }
        1
    }
}

// --- NaN-boxed ops ---
// (moved to runtime/molt-runtime/src/object/ops.rs)
#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicBool, AtomicUsize};

    static EXIT_CALLED: AtomicBool = AtomicBool::new(false);
    static RUNTIME_GUARD_COUNT: AtomicUsize = AtomicUsize::new(0);

    struct RuntimeTestGuard;

    impl RuntimeTestGuard {
        fn new() -> Self {
            if RUNTIME_GUARD_COUNT.fetch_add(1, AtomicOrdering::SeqCst) == 0 {
                crate::state::runtime_state::molt_runtime_init();
            }
            Self
        }
    }

    impl Drop for RuntimeTestGuard {
        fn drop(&mut self) {
            if RUNTIME_GUARD_COUNT.fetch_sub(1, AtomicOrdering::SeqCst) == 1 {
                crate::state::runtime_state::molt_runtime_shutdown();
            }
        }
    }

    extern "C" fn test_enter(payload_bits: u64) -> u64 {
        payload_bits
    }

    extern "C" fn test_exit(_payload_bits: u64, _exc_bits: u64) -> u64 {
        EXIT_CALLED.store(true, AtomicOrdering::SeqCst);
        MoltObject::from_bool(false).bits()
    }

    #[test]
    fn context_unwind_runs_exit() {
        let _runtime = RuntimeTestGuard::new();
        EXIT_CALLED.store(false, AtomicOrdering::SeqCst);
        let ctx_bits = molt_context_new(
            test_enter as *const (),
            test_exit as *const (),
            MoltObject::none().bits(),
        );
        let _ = molt_context_enter(ctx_bits);
        let _ = molt_context_unwind(MoltObject::none().bits());
        assert!(EXIT_CALLED.load(AtomicOrdering::SeqCst));
        if let Some(ptr) = obj_from_bits(ctx_bits).as_ptr() {
            unsafe { molt_dec_ref(ptr) };
        }
    }

    #[test]
    fn file_handle_close_marks_closed() {
        std::env::set_var("MOLT_CAPABILITIES", "fs.read,fs.write");
        let _runtime = RuntimeTestGuard::new();
        let tmp_dir = std::env::temp_dir();
        let file_name = format!(
            "molt_test_{}.txt",
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        );
        let path = tmp_dir.join(file_name);
        let path_bytes = path.to_string_lossy().into_owned();

        let path_ptr = alloc_string(path_bytes.as_bytes());
        assert!(!path_ptr.is_null());
        let mode_ptr = alloc_string(b"w");
        assert!(!mode_ptr.is_null());
        let handle_bits = molt_file_open(
            MoltObject::from_ptr(path_ptr).bits(),
            MoltObject::from_ptr(mode_ptr).bits(),
        );
        let handle_obj = obj_from_bits(handle_bits);
        let Some(handle_ptr) = handle_obj.as_ptr() else {
            panic!("file handle missing");
        };
        unsafe {
            let fh_ptr = file_handle_ptr(handle_ptr);
            assert!(!fh_ptr.is_null());
            let fh = &*fh_ptr;
            let guard = fh.state.file.lock().unwrap();
            assert!(guard.is_some());
        }
        let _ = molt_file_close(handle_bits);
        unsafe {
            let fh_ptr = file_handle_ptr(handle_ptr);
            let fh = &*fh_ptr;
            let guard = fh.state.file.lock().unwrap();
            assert!(guard.is_none());
        }
        if let Some(ptr) = obj_from_bits(handle_bits).as_ptr() {
            unsafe { molt_dec_ref(ptr) };
        }
        unsafe {
            molt_dec_ref(path_ptr);
            molt_dec_ref(mode_ptr);
        }
        let _ = std::fs::remove_file(path);
    }

    #[test]
    fn file_readinto_reads_bytes() {
        std::env::set_var("MOLT_CAPABILITIES", "fs.read,fs.write");
        let _runtime = RuntimeTestGuard::new();
        let tmp_dir = std::env::temp_dir();
        let file_name = format!(
            "molt_test_readinto_{}.bin",
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        );
        let path = tmp_dir.join(file_name);
        std::fs::write(&path, b"hello").expect("write test file");
        let path_bytes = path.to_string_lossy().into_owned();

        let path_ptr = alloc_string(path_bytes.as_bytes());
        assert!(!path_ptr.is_null());
        let mode_ptr = alloc_string(b"rb");
        assert!(!mode_ptr.is_null());
        let handle_bits = molt_file_open(
            MoltObject::from_ptr(path_ptr).bits(),
            MoltObject::from_ptr(mode_ptr).bits(),
        );
        let buf_ptr = alloc_bytearray_with_len(5);
        assert!(!buf_ptr.is_null());
        let buf_bits = MoltObject::from_ptr(buf_ptr).bits();
        let read_bits = molt_file_readinto(handle_bits, buf_bits);
        assert_eq!(to_i64(obj_from_bits(read_bits)).unwrap_or(-1), 5);
        unsafe {
            assert_eq!(bytearray_vec_ref(buf_ptr).as_slice(), b"hello");
        }
        let _ = molt_file_close(handle_bits);
        if let Some(ptr) = obj_from_bits(handle_bits).as_ptr() {
            unsafe { molt_dec_ref(ptr) };
        }
        unsafe {
            molt_dec_ref(buf_ptr);
            molt_dec_ref(path_ptr);
            molt_dec_ref(mode_ptr);
        }
        let _ = std::fs::remove_file(path);
    }

    #[test]
    fn file_writelines_writes_all() {
        std::env::set_var("MOLT_CAPABILITIES", "fs.read,fs.write");
        let _runtime = RuntimeTestGuard::new();
        let tmp_dir = std::env::temp_dir();
        let file_name = format!(
            "molt_test_writelines_{}.bin",
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        );
        let path = tmp_dir.join(file_name);
        let path_bytes = path.to_string_lossy().into_owned();

        let path_ptr = alloc_string(path_bytes.as_bytes());
        assert!(!path_ptr.is_null());
        let mode_ptr = alloc_string(b"wb");
        assert!(!mode_ptr.is_null());
        let handle_bits = molt_file_open(
            MoltObject::from_ptr(path_ptr).bits(),
            MoltObject::from_ptr(mode_ptr).bits(),
        );
        let line1_ptr = alloc_bytes(b"hello\n");
        let line2_ptr = alloc_bytes(b"world\n");
        assert!(!line1_ptr.is_null());
        assert!(!line2_ptr.is_null());
        let line1_bits = MoltObject::from_ptr(line1_ptr).bits();
        let line2_bits = MoltObject::from_ptr(line2_ptr).bits();
        let list_ptr = alloc_list(&[line1_bits, line2_bits]);
        assert!(!list_ptr.is_null());
        let list_bits = MoltObject::from_ptr(list_ptr).bits();
        let _ = molt_file_writelines(handle_bits, list_bits);
        let _ = molt_file_close(handle_bits);

        let contents = std::fs::read(&path).expect("read test file");
        assert_eq!(contents, b"hello\nworld\n");
        if let Some(ptr) = obj_from_bits(handle_bits).as_ptr() {
            unsafe { molt_dec_ref(ptr) };
        }
        unsafe {
            molt_dec_ref(list_ptr);
            molt_dec_ref(line1_ptr);
            molt_dec_ref(line2_ptr);
            molt_dec_ref(path_ptr);
            molt_dec_ref(mode_ptr);
        }
        let _ = std::fs::remove_file(path);
    }
}

// --- JSON ---

/// # Safety
/// Dereferences raw pointers. Caller must ensure ptr is valid UTF-8 of at least len bytes.
#[no_mangle]
pub unsafe extern "C" fn molt_json_parse_int(ptr: *const u8, len_bits: u64) -> i64 {
    let len = usize_from_bits(len_bits);
    let s = {
        let slice = std::slice::from_raw_parts(ptr, len);
        std::str::from_utf8(slice).unwrap()
    };
    let v: serde_json::Value = serde_json::from_str(s).unwrap();
    v.as_i64().unwrap_or(0)
}

fn value_to_object(value: serde_json::Value, arena: &mut TempArena) -> Result<MoltObject, i32> {
    match value {
        serde_json::Value::Null => Ok(MoltObject::none()),
        serde_json::Value::Bool(b) => Ok(MoltObject::from_bool(b)),
        serde_json::Value::Number(n) => {
            if let Some(i) = n.as_i64() {
                Ok(MoltObject::from_int(i))
            } else if let Some(f) = n.as_f64() {
                Ok(MoltObject::from_float(f))
            } else {
                Err(2)
            }
        }
        serde_json::Value::String(s) => {
            let ptr = alloc_string(s.as_bytes());
            if ptr.is_null() {
                Err(2)
            } else {
                Ok(MoltObject::from_ptr(ptr))
            }
        }
        serde_json::Value::Array(items) => {
            if items.len() > MAX_SMALL_LIST {
                return Err(2);
            }
            if items.is_empty() {
                let ptr = alloc_list(&[]);
                if ptr.is_null() {
                    return Err(2);
                }
                return Ok(MoltObject::from_ptr(ptr));
            }
            let len = items.len();
            let elems_ptr = arena.alloc_slice::<u64>(len);
            if elems_ptr.is_null() {
                return Err(2);
            }
            for (idx, item) in items.into_iter().enumerate() {
                let obj = value_to_object(item, arena)?;
                unsafe {
                    *elems_ptr.add(idx) = obj.bits();
                }
            }
            let elems = unsafe { std::slice::from_raw_parts(elems_ptr, len) };
            let ptr = alloc_list(elems);
            if ptr.is_null() {
                Err(2)
            } else {
                Ok(MoltObject::from_ptr(ptr))
            }
        }
        serde_json::Value::Object(map) => {
            if map.len() > MAX_SMALL_LIST {
                return Err(2);
            }
            if map.is_empty() {
                let ptr = alloc_dict_with_pairs(&[]);
                if ptr.is_null() {
                    return Err(2);
                }
                return Ok(MoltObject::from_ptr(ptr));
            }
            let len = map.len();
            let pairs_ptr = arena.alloc_slice::<u64>(len * 2);
            if pairs_ptr.is_null() {
                return Err(2);
            }
            for (idx, (key, value)) in map.into_iter().enumerate() {
                let key_ptr = alloc_string(key.as_bytes());
                if key_ptr.is_null() {
                    return Err(2);
                }
                let val_obj = value_to_object(value, arena)?;
                unsafe {
                    *pairs_ptr.add(idx * 2) = MoltObject::from_ptr(key_ptr).bits();
                    *pairs_ptr.add(idx * 2 + 1) = val_obj.bits();
                }
            }
            let pairs = unsafe { std::slice::from_raw_parts(pairs_ptr, len * 2) };
            let ptr = alloc_dict_with_pairs(pairs);
            if ptr.is_null() {
                Err(2)
            } else {
                Ok(MoltObject::from_ptr(ptr))
            }
        }
    }
}

fn msgpack_value_to_object(value: rmpv::Value, arena: &mut TempArena) -> Result<MoltObject, i32> {
    match value {
        rmpv::Value::Nil => Ok(MoltObject::none()),
        rmpv::Value::Boolean(b) => Ok(MoltObject::from_bool(b)),
        rmpv::Value::Integer(i) => {
            if let Some(v) = i.as_i64() {
                Ok(MoltObject::from_int(v))
            } else if let Some(v) = i.as_u64() {
                if v <= i64::MAX as u64 {
                    Ok(MoltObject::from_int(v as i64))
                } else {
                    Err(2)
                }
            } else {
                Err(2)
            }
        }
        rmpv::Value::F32(f) => Ok(MoltObject::from_float(f as f64)),
        rmpv::Value::F64(f) => Ok(MoltObject::from_float(f)),
        rmpv::Value::String(s) => {
            let s = s.as_str().ok_or(2)?;
            let ptr = alloc_string(s.as_bytes());
            if ptr.is_null() {
                Err(2)
            } else {
                Ok(MoltObject::from_ptr(ptr))
            }
        }
        rmpv::Value::Binary(b) => {
            let ptr = alloc_bytes(&b);
            if ptr.is_null() {
                Err(2)
            } else {
                Ok(MoltObject::from_ptr(ptr))
            }
        }
        rmpv::Value::Array(items) => {
            if items.len() > MAX_SMALL_LIST {
                return Err(2);
            }
            if items.is_empty() {
                let ptr = alloc_list(&[]);
                if ptr.is_null() {
                    return Err(2);
                }
                return Ok(MoltObject::from_ptr(ptr));
            }
            let len = items.len();
            let elems_ptr = arena.alloc_slice::<u64>(len);
            if elems_ptr.is_null() {
                return Err(2);
            }
            for (idx, item) in items.into_iter().enumerate() {
                let obj = msgpack_value_to_object(item, arena)?;
                unsafe {
                    *elems_ptr.add(idx) = obj.bits();
                }
            }
            let elems = unsafe { std::slice::from_raw_parts(elems_ptr, len) };
            let ptr = alloc_list(elems);
            if ptr.is_null() {
                Err(2)
            } else {
                Ok(MoltObject::from_ptr(ptr))
            }
        }
        rmpv::Value::Map(items) => {
            if items.len() > MAX_SMALL_LIST {
                return Err(2);
            }
            if items.is_empty() {
                let ptr = alloc_dict_with_pairs(&[]);
                if ptr.is_null() {
                    return Err(2);
                }
                return Ok(MoltObject::from_ptr(ptr));
            }
            let len = items.len();
            let pairs_ptr = arena.alloc_slice::<u64>(len * 2);
            if pairs_ptr.is_null() {
                return Err(2);
            }
            for (idx, (key, value)) in items.into_iter().enumerate() {
                let key_obj = msgpack_key_to_object(key)?;
                let val_obj = msgpack_value_to_object(value, arena)?;
                unsafe {
                    *pairs_ptr.add(idx * 2) = key_obj.bits();
                    *pairs_ptr.add(idx * 2 + 1) = val_obj.bits();
                }
            }
            let pairs = unsafe { std::slice::from_raw_parts(pairs_ptr, len * 2) };
            let ptr = alloc_dict_with_pairs(pairs);
            if ptr.is_null() {
                Err(2)
            } else {
                Ok(MoltObject::from_ptr(ptr))
            }
        }
        _ => Err(2),
    }
}

fn msgpack_key_to_object(value: rmpv::Value) -> Result<MoltObject, i32> {
    match value {
        rmpv::Value::Nil => Ok(MoltObject::none()),
        rmpv::Value::Boolean(b) => Ok(MoltObject::from_bool(b)),
        rmpv::Value::Integer(i) => {
            if let Some(v) = i.as_i64() {
                Ok(MoltObject::from_int(v))
            } else if let Some(v) = i.as_u64() {
                if v <= i64::MAX as u64 {
                    Ok(MoltObject::from_int(v as i64))
                } else {
                    Err(2)
                }
            } else {
                Err(2)
            }
        }
        rmpv::Value::String(s) => {
            let ptr = alloc_string(s.as_bytes());
            if ptr.is_null() {
                Err(2)
            } else {
                Ok(MoltObject::from_ptr(ptr))
            }
        }
        rmpv::Value::Binary(b) => {
            let ptr = alloc_bytes(&b);
            if ptr.is_null() {
                Err(2)
            } else {
                Ok(MoltObject::from_ptr(ptr))
            }
        }
        _ => Err(2),
    }
}

fn cbor_value_to_object(
    value: serde_cbor::Value,
    arena: &mut TempArena,
) -> Result<MoltObject, i32> {
    match value {
        serde_cbor::Value::Null => Ok(MoltObject::none()),
        serde_cbor::Value::Bool(b) => Ok(MoltObject::from_bool(b)),
        serde_cbor::Value::Integer(i) => {
            if i < i64::MIN as i128 || i > i64::MAX as i128 {
                return Err(2);
            }
            Ok(MoltObject::from_int(i as i64))
        }
        serde_cbor::Value::Float(f) => Ok(MoltObject::from_float(f)),
        serde_cbor::Value::Text(s) => {
            let ptr = alloc_string(s.as_bytes());
            if ptr.is_null() {
                Err(2)
            } else {
                Ok(MoltObject::from_ptr(ptr))
            }
        }
        serde_cbor::Value::Bytes(b) => {
            let ptr = alloc_bytes(&b);
            if ptr.is_null() {
                Err(2)
            } else {
                Ok(MoltObject::from_ptr(ptr))
            }
        }
        serde_cbor::Value::Array(items) => {
            if items.len() > MAX_SMALL_LIST {
                return Err(2);
            }
            if items.is_empty() {
                let ptr = alloc_list(&[]);
                if ptr.is_null() {
                    return Err(2);
                }
                return Ok(MoltObject::from_ptr(ptr));
            }
            let len = items.len();
            let elems_ptr = arena.alloc_slice::<u64>(len);
            if elems_ptr.is_null() {
                return Err(2);
            }
            for (idx, item) in items.into_iter().enumerate() {
                let obj = cbor_value_to_object(item, arena)?;
                unsafe {
                    *elems_ptr.add(idx) = obj.bits();
                }
            }
            let elems = unsafe { std::slice::from_raw_parts(elems_ptr, len) };
            let ptr = alloc_list(elems);
            if ptr.is_null() {
                Err(2)
            } else {
                Ok(MoltObject::from_ptr(ptr))
            }
        }
        serde_cbor::Value::Map(items) => {
            if items.len() > MAX_SMALL_LIST {
                return Err(2);
            }
            if items.is_empty() {
                let ptr = alloc_dict_with_pairs(&[]);
                if ptr.is_null() {
                    return Err(2);
                }
                return Ok(MoltObject::from_ptr(ptr));
            }
            let len = items.len();
            let pairs_ptr = arena.alloc_slice::<u64>(len * 2);
            if pairs_ptr.is_null() {
                return Err(2);
            }
            for (idx, (key, value)) in items.into_iter().enumerate() {
                let key_obj = cbor_key_to_object(key)?;
                let val_obj = cbor_value_to_object(value, arena)?;
                unsafe {
                    *pairs_ptr.add(idx * 2) = key_obj.bits();
                    *pairs_ptr.add(idx * 2 + 1) = val_obj.bits();
                }
            }
            let pairs = unsafe { std::slice::from_raw_parts(pairs_ptr, len * 2) };
            let ptr = alloc_dict_with_pairs(pairs);
            if ptr.is_null() {
                Err(2)
            } else {
                Ok(MoltObject::from_ptr(ptr))
            }
        }
        _ => Err(2),
    }
}

fn cbor_key_to_object(value: serde_cbor::Value) -> Result<MoltObject, i32> {
    match value {
        serde_cbor::Value::Null => Ok(MoltObject::none()),
        serde_cbor::Value::Bool(b) => Ok(MoltObject::from_bool(b)),
        serde_cbor::Value::Integer(i) => {
            let i_val = i;
            if i_val < i64::MIN as i128 || i_val > i64::MAX as i128 {
                Err(2)
            } else {
                Ok(MoltObject::from_int(i_val as i64))
            }
        }
        serde_cbor::Value::Text(s) => {
            let ptr = alloc_string(s.as_bytes());
            if ptr.is_null() {
                Err(2)
            } else {
                Ok(MoltObject::from_ptr(ptr))
            }
        }
        serde_cbor::Value::Bytes(b) => {
            let ptr = alloc_bytes(&b);
            if ptr.is_null() {
                Err(2)
            } else {
                Ok(MoltObject::from_ptr(ptr))
            }
        }
        _ => Err(2),
    }
}

unsafe fn parse_json_scalar(
    ptr: *const u8,
    len: usize,
    arena: &mut TempArena,
) -> Result<MoltObject, i32> {
    let slice = std::slice::from_raw_parts(ptr, len);
    let s = std::str::from_utf8(slice).map_err(|_| 1)?;
    let v: serde_json::Value = serde_json::from_str(s).map_err(|_| 1)?;
    value_to_object(v, arena)
}

/// # Safety
/// Caller must ensure ptr is valid for len bytes.
#[no_mangle]
pub unsafe extern "C" fn molt_string_from_bytes(
    ptr: *const u8,
    len_bits: u64,
    out: *mut u64,
) -> i32 {
    let len = usize_from_bits(len_bits);
    if out.is_null() {
        return 2;
    }
    if ptr.is_null() && len != 0 {
        return 1;
    }
    let slice = std::slice::from_raw_parts(ptr, len);
    if std::str::from_utf8(slice).is_err() {
        return 1;
    }
    let obj_ptr = alloc_string(slice);
    if obj_ptr.is_null() {
        return 2;
    }
    *out = MoltObject::from_ptr(obj_ptr).bits();
    0
}

/// # Safety
/// Caller must ensure ptr is valid for len bytes.
#[no_mangle]
pub unsafe extern "C" fn molt_bytes_from_bytes(
    ptr: *const u8,
    len_bits: u64,
    out: *mut u64,
) -> i32 {
    let len = usize_from_bits(len_bits);
    if out.is_null() {
        return 2;
    }
    if ptr.is_null() && len != 0 {
        return 1;
    }
    let slice = std::slice::from_raw_parts(ptr, len);
    let obj_ptr = alloc_bytes(slice);
    if obj_ptr.is_null() {
        return 2;
    }
    *out = MoltObject::from_ptr(obj_ptr).bits();
    0
}

static ERRNO_CONSTANTS_CACHE: AtomicU64 = AtomicU64::new(0);
static SOCKET_CONSTANTS_CACHE: AtomicU64 = AtomicU64::new(0);

fn base_errno_constants() -> Vec<(&'static str, i64)> {
    vec![
        ("EACCES", libc::EACCES as i64),
        ("EAGAIN", libc::EAGAIN as i64),
        ("EALREADY", libc::EALREADY as i64),
        ("ECHILD", libc::ECHILD as i64),
        ("ECONNABORTED", libc::ECONNABORTED as i64),
        ("ECONNREFUSED", libc::ECONNREFUSED as i64),
        ("ECONNRESET", libc::ECONNRESET as i64),
        ("EEXIST", libc::EEXIST as i64),
        ("EINPROGRESS", libc::EINPROGRESS as i64),
        ("EINTR", libc::EINTR as i64),
        ("EISDIR", libc::EISDIR as i64),
        ("ENOENT", libc::ENOENT as i64),
        ("ENOTDIR", libc::ENOTDIR as i64),
        ("EPERM", libc::EPERM as i64),
        ("EPIPE", libc::EPIPE as i64),
        ("ESRCH", libc::ESRCH as i64),
        ("ETIMEDOUT", libc::ETIMEDOUT as i64),
        ("EWOULDBLOCK", libc::EWOULDBLOCK as i64),
        ("ESHUTDOWN", libc::ESHUTDOWN as i64),
    ]
}

fn collect_errno_constants() -> Vec<(&'static str, i64)> {
    // TODO(stdlib-compat, owner:runtime, milestone:SL2, priority:P1, status:partial): expand errno constants to match CPython's full table on each platform.
    #[cfg(target_os = "freebsd")]
    {
        let mut out = base_errno_constants();
        out.push(("ENOTCAPABLE", libc::ENOTCAPABLE as i64));
        out
    }
    #[cfg(not(target_os = "freebsd"))]
    {
        base_errno_constants()
    }
}

fn socket_constants() -> Vec<(&'static str, i64)> {
    let mut out = vec![
        ("AF_INET", libc::AF_INET as i64),
        ("AF_INET6", libc::AF_INET6 as i64),
        ("SOCK_STREAM", libc::SOCK_STREAM as i64),
        ("SOCK_DGRAM", libc::SOCK_DGRAM as i64),
        ("SOCK_RAW", libc::SOCK_RAW as i64),
        ("SOL_SOCKET", libc::SOL_SOCKET as i64),
        ("SO_REUSEADDR", libc::SO_REUSEADDR as i64),
        ("SO_KEEPALIVE", libc::SO_KEEPALIVE as i64),
        ("SO_SNDBUF", libc::SO_SNDBUF as i64),
        ("SO_RCVBUF", libc::SO_RCVBUF as i64),
        ("SO_ERROR", libc::SO_ERROR as i64),
        ("SO_LINGER", libc::SO_LINGER as i64),
        ("SO_BROADCAST", libc::SO_BROADCAST as i64),
        ("IPPROTO_TCP", libc::IPPROTO_TCP as i64),
        ("IPPROTO_UDP", libc::IPPROTO_UDP as i64),
        ("IPPROTO_IPV6", libc::IPPROTO_IPV6 as i64),
        ("IPV6_V6ONLY", libc::IPV6_V6ONLY as i64),
        ("TCP_NODELAY", libc::TCP_NODELAY as i64),
        ("SHUT_RD", libc::SHUT_RD as i64),
        ("SHUT_WR", libc::SHUT_WR as i64),
        ("SHUT_RDWR", libc::SHUT_RDWR as i64),
        ("AI_PASSIVE", libc::AI_PASSIVE as i64),
        ("AI_CANONNAME", libc::AI_CANONNAME as i64),
        ("AI_NUMERICHOST", libc::AI_NUMERICHOST as i64),
        ("AI_NUMERICSERV", libc::AI_NUMERICSERV as i64),
        ("NI_NUMERICHOST", libc::NI_NUMERICHOST as i64),
        ("NI_NUMERICSERV", libc::NI_NUMERICSERV as i64),
        ("MSG_PEEK", libc::MSG_PEEK as i64),
    ];
    #[cfg(unix)]
    {
        out.push(("AF_UNIX", libc::AF_UNIX as i64));
    }
    #[cfg(unix)]
    {
        if SOCK_NONBLOCK_FLAG != 0 {
            out.push(("SOCK_NONBLOCK", SOCK_NONBLOCK_FLAG as i64));
        }
        if SOCK_CLOEXEC_FLAG != 0 {
            out.push(("SOCK_CLOEXEC", SOCK_CLOEXEC_FLAG as i64));
        }
    }
    #[cfg(unix)]
    {
        out.push(("MSG_DONTWAIT", libc::MSG_DONTWAIT as i64));
    }
    #[cfg(any(
        target_os = "linux",
        target_os = "android",
        target_os = "macos",
        target_os = "ios",
        target_os = "freebsd",
        target_os = "netbsd",
        target_os = "openbsd",
        target_os = "dragonfly"
    ))]
    {
        out.push(("SO_REUSEPORT", libc::SO_REUSEPORT as i64));
    }
    out.push(("EAI_AGAIN", libc::EAI_AGAIN as i64));
    out.push(("EAI_FAIL", libc::EAI_FAIL as i64));
    out.push(("EAI_FAMILY", libc::EAI_FAMILY as i64));
    out.push(("EAI_NONAME", libc::EAI_NONAME as i64));
    out.push(("EAI_SERVICE", libc::EAI_SERVICE as i64));
    out.push(("EAI_SOCKTYPE", libc::EAI_SOCKTYPE as i64));
    out
}

#[no_mangle]
pub extern "C" fn molt_errno_constants() -> u64 {
    init_atomic_bits(&ERRNO_CONSTANTS_CACHE, || {
        let constants = collect_errno_constants();
        let mut pairs = Vec::with_capacity(constants.len() * 2);
        let mut reverse_pairs = Vec::with_capacity(constants.len() * 2);
        let mut owned_bits = Vec::with_capacity(constants.len() * 2);
        for (name, value) in constants {
            let name_ptr = alloc_string(name.as_bytes());
            if name_ptr.is_null() {
                for bits in owned_bits {
                    dec_ref_bits(bits);
                }
                return MoltObject::none().bits();
            }
            let name_bits = MoltObject::from_ptr(name_ptr).bits();
            let value_bits = MoltObject::from_int(value).bits();
            pairs.push(name_bits);
            pairs.push(value_bits);
            reverse_pairs.push(value_bits);
            reverse_pairs.push(name_bits);
            owned_bits.push(name_bits);
            owned_bits.push(value_bits);
        }
        let dict_ptr = alloc_dict_with_pairs(&pairs);
        if dict_ptr.is_null() {
            for bits in owned_bits {
                dec_ref_bits(bits);
            }
            return MoltObject::none().bits();
        }
        let reverse_ptr = alloc_dict_with_pairs(&reverse_pairs);
        if reverse_ptr.is_null() {
            dec_ref_bits(MoltObject::from_ptr(dict_ptr).bits());
            for bits in owned_bits {
                dec_ref_bits(bits);
            }
            return MoltObject::none().bits();
        }
        let dict_bits = MoltObject::from_ptr(dict_ptr).bits();
        let reverse_bits = MoltObject::from_ptr(reverse_ptr).bits();
        let tuple_ptr = alloc_tuple(&[dict_bits, reverse_bits]);
        for bits in owned_bits {
            dec_ref_bits(bits);
        }
        dec_ref_bits(dict_bits);
        dec_ref_bits(reverse_bits);
        if tuple_ptr.is_null() {
            return MoltObject::none().bits();
        }
        MoltObject::from_ptr(tuple_ptr).bits()
    })
}

#[no_mangle]
pub extern "C" fn molt_socket_constants() -> u64 {
    init_atomic_bits(&SOCKET_CONSTANTS_CACHE, || {
        let constants = socket_constants();
        let mut pairs = Vec::with_capacity(constants.len() * 2);
        let mut owned_bits = Vec::with_capacity(constants.len() * 2);
        for (name, value) in constants {
            let name_ptr = alloc_string(name.as_bytes());
            if name_ptr.is_null() {
                for bits in owned_bits {
                    dec_ref_bits(bits);
                }
                return MoltObject::none().bits();
            }
            let name_bits = MoltObject::from_ptr(name_ptr).bits();
            let value_bits = MoltObject::from_int(value).bits();
            pairs.push(name_bits);
            pairs.push(value_bits);
            owned_bits.push(name_bits);
            owned_bits.push(value_bits);
        }
        let dict_ptr = alloc_dict_with_pairs(&pairs);
        if dict_ptr.is_null() {
            for bits in owned_bits {
                dec_ref_bits(bits);
            }
            return MoltObject::none().bits();
        }
        let dict_bits = MoltObject::from_ptr(dict_ptr).bits();
        for bits in owned_bits {
            dec_ref_bits(bits);
        }
        dict_bits
    })
}

#[no_mangle]
pub extern "C" fn molt_env_get(key_bits: u64, default_bits: u64) -> u64 {
    let key = match string_obj_to_owned(obj_from_bits(key_bits)) {
        Some(key) => key,
        None => return default_bits,
    };
    #[cfg(target_arch = "wasm32")]
    {
        let Some(bytes) = wasm_env_get_bytes(&key) else {
            return default_bits;
        };
        let Ok(val) = std::str::from_utf8(&bytes) else {
            return default_bits;
        };
        let ptr = alloc_string(val.as_bytes());
        if ptr.is_null() {
            default_bits
        } else {
            MoltObject::from_ptr(ptr).bits()
        }
    }
    #[cfg(not(target_arch = "wasm32"))]
    {
        match std::env::var(key) {
            Ok(val) => {
                let ptr = alloc_string(val.as_bytes());
                if ptr.is_null() {
                    default_bits
                } else {
                    MoltObject::from_ptr(ptr).bits()
                }
            }
            Err(_) => default_bits,
        }
    }
}

#[cfg(target_arch = "wasm32")]
fn wasm_env_get_bytes(key: &str) -> Option<Vec<u8>> {
    let mut env_count = 0u32;
    let mut buf_size = 0u32;
    let rc = unsafe { environ_sizes_get(&mut env_count, &mut buf_size) };
    if rc != 0 || env_count == 0 || buf_size == 0 {
        return None;
    }
    let env_count = usize::try_from(env_count).ok()?;
    let buf_size = usize::try_from(buf_size).ok()?;
    let mut ptrs = vec![std::ptr::null_mut(); env_count];
    let mut buf = vec![0u8; buf_size];
    let rc = unsafe { environ_get(ptrs.as_mut_ptr(), buf.as_mut_ptr()) };
    if rc != 0 {
        return None;
    }
    let base = buf.as_ptr();
    let key_bytes = key.as_bytes();
    for ptr in ptrs {
        if ptr.is_null() {
            continue;
        }
        let offset = unsafe { ptr.offset_from(base) };
        if offset < 0 {
            continue;
        }
        let offset = offset as usize;
        if offset >= buf.len() {
            continue;
        }
        let slice = &buf[offset..];
        let end = slice.iter().position(|b| *b == 0).unwrap_or(slice.len());
        let entry = &slice[..end];
        let Some(eq) = entry.iter().position(|b| *b == b'=') else {
            continue;
        };
        if &entry[..eq] == key_bytes {
            return Some(entry[eq + 1..].to_vec());
        }
    }
    None
}

/// # Safety
/// Dereferences raw pointers. Caller must ensure ptr is valid UTF-8 of at least len bytes.
#[no_mangle]
pub unsafe extern "C" fn molt_json_parse_scalar(
    ptr: *const u8,
    len_bits: u64,
    out: *mut u64,
) -> i32 {
    let len = usize_from_bits(len_bits);
    if out.is_null() {
        return 2;
    }
    let obj = PARSE_ARENA.with(|arena| {
        let mut arena = arena.borrow_mut();
        let result = parse_json_scalar(ptr, len, &mut arena);
        arena.reset();
        result
    });
    let obj = match obj {
        Ok(val) => val,
        Err(code) => return code,
    };
    *out = obj.bits();
    0
}

/// # Safety
/// Caller must ensure ptr is valid for len bytes.
#[no_mangle]
pub unsafe extern "C" fn molt_msgpack_parse_scalar(
    ptr: *const u8,
    len_bits: u64,
    out: *mut u64,
) -> i32 {
    let len = usize_from_bits(len_bits);
    if out.is_null() {
        return 2;
    }
    let slice = std::slice::from_raw_parts(ptr, len);
    let mut cursor = Cursor::new(slice);
    let v = match rmpv::decode::read_value(&mut cursor) {
        Ok(val) => val,
        Err(_) => return 1,
    };
    let obj = PARSE_ARENA.with(|arena| {
        let mut arena = arena.borrow_mut();
        let result = msgpack_value_to_object(v, &mut arena);
        arena.reset();
        result
    });
    let obj = match obj {
        Ok(val) => val,
        Err(code) => return code,
    };
    *out = obj.bits();
    0
}

/// # Safety
/// Caller must ensure ptr is valid for len bytes.
#[no_mangle]
pub unsafe extern "C" fn molt_cbor_parse_scalar(
    ptr: *const u8,
    len_bits: u64,
    out: *mut u64,
) -> i32 {
    let len = usize_from_bits(len_bits);
    if out.is_null() {
        return 2;
    }
    let slice = std::slice::from_raw_parts(ptr, len);
    let v: serde_cbor::Value = match serde_cbor::from_slice(slice) {
        Ok(val) => val,
        Err(_) => return 1,
    };
    let obj = PARSE_ARENA.with(|arena| {
        let mut arena = arena.borrow_mut();
        let result = cbor_value_to_object(v, &mut arena);
        arena.reset();
        result
    });
    let obj = match obj {
        Ok(val) => val,
        Err(code) => return code,
    };
    *out = obj.bits();
    0
}

#[no_mangle]
pub extern "C" fn molt_json_parse_scalar_obj(obj_bits: u64) -> u64 {
    let obj = obj_from_bits(obj_bits);
    let Some(ptr) = obj.as_ptr() else {
        return raise_exception::<_>("TypeError", "json.parse expects str");
    };
    unsafe {
        if object_type_id(ptr) != TYPE_ID_STRING {
            let msg = format!("json.parse expects str, got {}", type_name(obj));
            return raise_exception::<_>("TypeError", &msg);
        }
        let len = string_len(ptr);
        let data = string_bytes(ptr);
        let obj = PARSE_ARENA.with(|arena| {
            let mut arena = arena.borrow_mut();
            let result = parse_json_scalar(data, len, &mut arena);
            arena.reset();
            result
        });
        match obj {
            Ok(val) => val.bits(),
            Err(_) => return raise_exception::<_>("ValueError", "invalid JSON payload"),
        }
    }
}

#[no_mangle]
pub extern "C" fn molt_msgpack_parse_scalar_obj(obj_bits: u64) -> u64 {
    let obj = obj_from_bits(obj_bits);
    let Some(ptr) = obj.as_ptr() else {
        return raise_exception::<_>("TypeError", "msgpack.parse expects bytes");
    };
    unsafe {
        let type_id = object_type_id(ptr);
        if type_id != TYPE_ID_BYTES && type_id != TYPE_ID_BYTEARRAY {
            let msg = format!("msgpack.parse expects bytes, got {}", type_name(obj));
            return raise_exception::<_>("TypeError", &msg);
        }
        let len = bytes_len(ptr);
        let data = bytes_data(ptr);
        let slice = std::slice::from_raw_parts(data, len);
        let mut cursor = Cursor::new(slice);
        let v = match rmpv::decode::read_value(&mut cursor) {
            Ok(val) => val,
            Err(_) => return raise_exception::<_>("ValueError", "invalid msgpack payload"),
        };
        let obj = PARSE_ARENA.with(|arena| {
            let mut arena = arena.borrow_mut();
            let result = msgpack_value_to_object(v, &mut arena);
            arena.reset();
            result
        });
        match obj {
            Ok(val) => val.bits(),
            Err(_) => return raise_exception::<_>("ValueError", "invalid msgpack payload"),
        }
    }
}

#[no_mangle]
pub extern "C" fn molt_cbor_parse_scalar_obj(obj_bits: u64) -> u64 {
    let obj = obj_from_bits(obj_bits);
    let Some(ptr) = obj.as_ptr() else {
        return raise_exception::<_>("TypeError", "cbor.parse expects bytes");
    };
    unsafe {
        let type_id = object_type_id(ptr);
        if type_id != TYPE_ID_BYTES && type_id != TYPE_ID_BYTEARRAY {
            let msg = format!("cbor.parse expects bytes, got {}", type_name(obj));
            return raise_exception::<_>("TypeError", &msg);
        }
        let len = bytes_len(ptr);
        let data = bytes_data(ptr);
        let slice = std::slice::from_raw_parts(data, len);
        let v: serde_cbor::Value = match serde_cbor::from_slice(slice) {
            Ok(val) => val,
            Err(_) => return raise_exception::<_>("ValueError", "invalid cbor payload"),
        };
        let obj = PARSE_ARENA.with(|arena| {
            let mut arena = arena.borrow_mut();
            let result = cbor_value_to_object(v, &mut arena);
            arena.reset();
            result
        });
        match obj {
            Ok(val) => val.bits(),
            Err(_) => return raise_exception::<_>("ValueError", "invalid cbor payload"),
        }
    }
}

// --- Generic ---

#[inline]
fn usize_from_bits(bits: u64) -> usize {
    debug_assert!(bits <= usize::MAX as u64);
    bits as usize
}

pub(crate) unsafe fn lookup_call_attr(obj_ptr: *mut u8) -> Option<u64> {
    let call_name_bits = intern_static_name(&runtime_state().interned.call_name, b"__call__");
    attr_lookup_ptr(obj_ptr, call_name_bits)
}

unsafe fn class_layout_size(class_ptr: *mut u8) -> usize {
    let size_name_bits = intern_static_name(
        &runtime_state().interned.molt_layout_size,
        b"__molt_layout_size__",
    );
    if let Some(size_bits) = class_attr_lookup_raw_mro(class_ptr, size_name_bits) {
        if let Some(size) = obj_from_bits(size_bits).as_int() {
            if size > 0 {
                return size as usize;
            }
        }
    }
    8
}

unsafe fn alloc_instance_for_class(class_ptr: *mut u8) -> u64 {
    let class_bits = MoltObject::from_ptr(class_ptr).bits();
    let size = class_layout_size(class_ptr);
    let total_size = size + std::mem::size_of::<MoltHeader>();
    let obj_ptr = alloc_object_zeroed_with_pool(total_size, TYPE_ID_OBJECT);
    if obj_ptr.is_null() {
        return MoltObject::none().bits();
    }
    object_set_class_bits(obj_ptr, class_bits);
    inc_ref_bits(class_bits);
    MoltObject::from_ptr(obj_ptr).bits()
}

pub(crate) unsafe fn call_class_init_with_args(class_ptr: *mut u8, args: &[u64]) -> u64 {
    let class_bits = MoltObject::from_ptr(class_ptr).bits();
    let builtins = builtin_classes();
    if issubclass_bits(class_bits, builtins.base_exception) {
        let new_name_bits = intern_static_name(&runtime_state().interned.new_name, b"__new__");
        let inst_bits = if let Some(new_bits) = class_attr_lookup_raw_mro(class_ptr, new_name_bits)
        {
            let builder_bits = molt_callargs_new(args.len() as u64 + 1, 0);
            if builder_bits == 0 {
                return MoltObject::none().bits();
            }
            let _ = molt_callargs_push_pos(builder_bits, class_bits);
            for &arg in args {
                let _ = molt_callargs_push_pos(builder_bits, arg);
            }
            let inst_bits = molt_call_bind(new_bits, builder_bits);
            if exception_pending() {
                return MoltObject::none().bits();
            }
            if !isinstance_bits(inst_bits, class_bits) {
                return inst_bits;
            }
            inst_bits
        } else {
            let args_ptr = alloc_tuple(args);
            if args_ptr.is_null() {
                return MoltObject::none().bits();
            }
            let args_bits = MoltObject::from_ptr(args_ptr).bits();
            let exc_ptr = alloc_exception_from_class_bits(class_bits, args_bits);
            dec_ref_bits(args_bits);
            if exc_ptr.is_null() {
                return MoltObject::none().bits();
            }
            MoltObject::from_ptr(exc_ptr).bits()
        };
        let Some(inst_ptr) = obj_from_bits(inst_bits).as_ptr() else {
            return inst_bits;
        };
        let init_name_bits = intern_static_name(&runtime_state().interned.init_name, b"__init__");
        let Some(init_bits) =
            class_attr_lookup(class_ptr, class_ptr, Some(inst_ptr), init_name_bits)
        else {
            return inst_bits;
        };
        let pos_capacity = args.len() as u64;
        let builder_bits = molt_callargs_new(pos_capacity, 0);
        if builder_bits == 0 {
            return inst_bits;
        }
        for &arg in args {
            let _ = molt_callargs_push_pos(builder_bits, arg);
        }
        let _ = molt_call_bind(init_bits, builder_bits);
        return inst_bits;
    }
    if class_bits == builtins.slice {
        match args.len() {
            0 => {
                return raise_exception::<_>(
                    "TypeError",
                    "slice expected at least 1 argument, got 0",
                );
            }
            1 => {
                return molt_slice_new(
                    MoltObject::none().bits(),
                    args[0],
                    MoltObject::none().bits(),
                );
            }
            2 => {
                return molt_slice_new(args[0], args[1], MoltObject::none().bits());
            }
            3 => {
                return molt_slice_new(args[0], args[1], args[2]);
            }
            _ => {
                let msg = format!("slice expected at most 3 arguments, got {}", args.len());
                return raise_exception::<_>("TypeError", &msg);
            }
        }
    }
    if class_bits == builtins.list {
        match args.len() {
            0 => {
                let ptr = alloc_list(&[]);
                if ptr.is_null() {
                    return MoltObject::none().bits();
                }
                return MoltObject::from_ptr(ptr).bits();
            }
            1 => {
                let Some(bits) = list_from_iter_bits(args[0]) else {
                    return MoltObject::none().bits();
                };
                return bits;
            }
            _ => {
                let msg = format!("list expected at most 1 argument, got {}", args.len());
                return raise_exception::<_>("TypeError", &msg);
            }
        }
    }
    if class_bits == builtins.tuple {
        match args.len() {
            0 => {
                let ptr = alloc_tuple(&[]);
                if ptr.is_null() {
                    return MoltObject::none().bits();
                }
                return MoltObject::from_ptr(ptr).bits();
            }
            1 => {
                let Some(bits) = tuple_from_iter_bits(args[0]) else {
                    return MoltObject::none().bits();
                };
                return bits;
            }
            _ => {
                let msg = format!("tuple expected at most 1 argument, got {}", args.len());
                return raise_exception::<_>("TypeError", &msg);
            }
        }
    }
    if class_bits == builtins.dict {
        match args.len() {
            0 => return molt_dict_new(0),
            1 => return molt_dict_from_obj(args[0]),
            _ => {
                let msg = format!("dict expected at most 1 argument, got {}", args.len());
                return raise_exception::<_>("TypeError", &msg);
            }
        }
    }
    if class_bits == builtins.set {
        match args.len() {
            0 => return molt_set_new(0),
            1 => {
                let set_bits = molt_set_new(0);
                if obj_from_bits(set_bits).is_none() {
                    return MoltObject::none().bits();
                }
                let _ = molt_set_update(set_bits, args[0]);
                if exception_pending() {
                    dec_ref_bits(set_bits);
                    return MoltObject::none().bits();
                }
                return set_bits;
            }
            _ => {
                let msg = format!("set expected at most 1 argument, got {}", args.len());
                return raise_exception::<_>("TypeError", &msg);
            }
        }
    }
    if class_bits == builtins.frozenset {
        match args.len() {
            0 => return molt_frozenset_new(0),
            1 => {
                let Some(bits) = frozenset_from_iter_bits(args[0]) else {
                    return MoltObject::none().bits();
                };
                return bits;
            }
            _ => {
                let msg = format!("frozenset expected at most 1 argument, got {}", args.len());
                return raise_exception::<_>("TypeError", &msg);
            }
        }
    }
    if class_bits == builtins.range {
        match args.len() {
            0 => {
                return raise_exception::<_>(
                    "TypeError",
                    "range expected at least 1 argument, got 0",
                );
            }
            1 => {
                let start_bits = MoltObject::from_int(0).bits();
                let step_bits = MoltObject::from_int(1).bits();
                return molt_range_new(start_bits, args[0], step_bits);
            }
            2 => {
                let step_bits = MoltObject::from_int(1).bits();
                return molt_range_new(args[0], args[1], step_bits);
            }
            3 => {
                return molt_range_new(args[0], args[1], args[2]);
            }
            _ => {
                let msg = format!("range expected at most 3 arguments, got {}", args.len());
                return raise_exception::<_>("TypeError", &msg);
            }
        }
    }
    if class_bits == builtins.bytes {
        match args.len() {
            0 => {
                let ptr = alloc_bytes(&[]);
                if ptr.is_null() {
                    return MoltObject::none().bits();
                }
                return MoltObject::from_ptr(ptr).bits();
            }
            1 => return molt_bytes_from_obj(args[0]),
            2 => return molt_bytes_from_str(args[0], args[1], MoltObject::none().bits()),
            3 => return molt_bytes_from_str(args[0], args[1], args[2]),
            _ => {
                let msg = format!("bytes() takes at most 3 arguments ({} given)", args.len());
                return raise_exception::<_>("TypeError", &msg);
            }
        }
    }
    if class_bits == builtins.bytearray {
        match args.len() {
            0 => {
                let ptr = alloc_bytearray(&[]);
                if ptr.is_null() {
                    return MoltObject::none().bits();
                }
                return MoltObject::from_ptr(ptr).bits();
            }
            1 => return molt_bytearray_from_obj(args[0]),
            2 => return molt_bytearray_from_str(args[0], args[1], MoltObject::none().bits()),
            3 => return molt_bytearray_from_str(args[0], args[1], args[2]),
            _ => {
                let msg = format!(
                    "bytearray() takes at most 3 arguments ({} given)",
                    args.len()
                );
                return raise_exception::<_>("TypeError", &msg);
            }
        }
    }
    if class_bits == builtins.str {
        match args.len() {
            0 => {
                let ptr = alloc_string(b"");
                if ptr.is_null() {
                    return MoltObject::none().bits();
                }
                return MoltObject::from_ptr(ptr).bits();
            }
            1 => return molt_str_from_obj(args[0]),
            _ => {
                let obj = obj_from_bits(args[0]);
                let is_bytes_like = obj.as_ptr().is_some_and(|ptr| unsafe {
                    let type_id = object_type_id(ptr);
                    type_id == TYPE_ID_BYTES || type_id == TYPE_ID_BYTEARRAY
                });
                if !is_bytes_like {
                    let msg = format!(
                        "decoding to str: need a bytes-like object, {} found",
                        type_name(obj)
                    );
                    return raise_exception::<_>("TypeError", &msg);
                }
                // TODO(stdlib-compat, str-encoding): support encoding/errors args for
                // bytes-like inputs and match CPython's UnicodeDecodeError details.
                return raise_exception::<_>(
                    "NotImplementedError",
                    "str() encoding arguments are not supported yet",
                );
            }
        }
    }
    let inst_bits = alloc_instance_for_class(class_ptr);
    let Some(inst_ptr) = obj_from_bits(inst_bits).as_ptr() else {
        return inst_bits;
    };
    let init_name_bits = intern_static_name(&runtime_state().interned.init_name, b"__init__");
    let Some(init_bits) = class_attr_lookup(class_ptr, class_ptr, Some(inst_ptr), init_name_bits)
    else {
        return inst_bits;
    };
    let pos_capacity = args.len() as u64;
    let builder_bits = molt_callargs_new(pos_capacity, 0);
    if builder_bits == 0 {
        return inst_bits;
    }
    for &arg in args {
        let _ = molt_callargs_push_pos(builder_bits, arg);
    }
    let _ = molt_call_bind(init_bits, builder_bits);
    inst_bits
}

pub(crate) fn raise_not_callable(obj: MoltObject) -> u64 {
    let msg = format!("'{}' object is not callable", type_name(obj));
    return raise_exception::<_>("TypeError", &msg);
}

pub(crate) unsafe fn call_builtin_type_if_needed(
    call_bits: u64,
    call_ptr: *mut u8,
    args: &[u64],
) -> Option<u64> {
    if is_builtin_class_bits(call_bits) {
        return Some(call_class_init_with_args(call_ptr, args));
    }
    None
}

pub(crate) unsafe fn try_call_generator(func_bits: u64, args: &[u64]) -> Option<u64> {
    let func_obj = obj_from_bits(func_bits);
    let func_ptr = func_obj.as_ptr()?;
    if object_type_id(func_ptr) != TYPE_ID_FUNCTION {
        return None;
    }
    let is_gen = function_attr_bits(
        func_ptr,
        intern_static_name(
            &runtime_state().interned.molt_is_generator,
            b"__molt_is_generator__",
        ),
    )
    .is_some_and(|bits| is_truthy(obj_from_bits(bits)));
    if !is_gen {
        return None;
    }
    let size_bits = function_attr_bits(
        func_ptr,
        intern_static_name(
            &runtime_state().interned.molt_closure_size,
            b"__molt_closure_size__",
        ),
    )
    .unwrap_or_else(|| MoltObject::none().bits());
    let Some(size_val) = obj_from_bits(size_bits).as_int() else {
        return raise_exception::<_>("TypeError", "call expects function object");
    };
    if size_val < 0 {
        return raise_exception::<_>("TypeError", "closure size must be non-negative");
    }
    let closure_size = size_val as usize;
    let fn_ptr = function_fn_ptr(func_ptr);
    let closure_bits = function_closure_bits(func_ptr);
    let mut payload: Vec<u64> =
        Vec::with_capacity(args.len() + if closure_bits != 0 { 1 } else { 0 });
    if closure_bits != 0 {
        payload.push(closure_bits);
    }
    payload.extend(args.iter().copied());
    let base = GEN_CONTROL_SIZE;
    let needed = base + payload.len() * std::mem::size_of::<u64>();
    if closure_size < needed {
        return raise_exception::<_>("TypeError", "call expects function object");
    }
    let obj_bits = molt_generator_new(fn_ptr, closure_size as u64);
    let Some(obj_ptr) = obj_from_bits(obj_bits).as_ptr() else {
        return Some(MoltObject::none().bits());
    };
    let mut offset = base;
    for val_bits in payload {
        let slot = obj_ptr.add(offset) as *mut u64;
        *slot = val_bits;
        inc_ref_bits(val_bits);
        offset += std::mem::size_of::<u64>();
    }
    Some(obj_bits)
}

unsafe fn function_attr_bits(func_ptr: *mut u8, attr_bits: u64) -> Option<u64> {
    let dict_bits = function_dict_bits(func_ptr);
    if dict_bits == 0 {
        return None;
    }
    let dict_ptr = obj_from_bits(dict_bits).as_ptr()?;
    if object_type_id(dict_ptr) != TYPE_ID_DICT {
        return None;
    }
    dict_get_in_place(dict_ptr, attr_bits)
}

#[no_mangle]
unsafe fn attr_lookup_ptr(obj_ptr: *mut u8, attr_bits: u64) -> Option<u64> {
    profile_hit(&ATTR_LOOKUP_COUNT);
    let type_id = object_type_id(obj_ptr);
    if type_id == TYPE_ID_MODULE {
        return module_attr_lookup(obj_ptr, attr_bits);
    }
    if type_id == TYPE_ID_BOUND_METHOD {
        let name = string_obj_to_owned(obj_from_bits(attr_bits));
        if let Some(name) = name.as_deref() {
            match name {
                "__func__" => {
                    let func_bits = bound_method_func_bits(obj_ptr);
                    inc_ref_bits(func_bits);
                    return Some(func_bits);
                }
                "__self__" => {
                    let self_bits = bound_method_self_bits(obj_ptr);
                    inc_ref_bits(self_bits);
                    return Some(self_bits);
                }
                "__name__" | "__qualname__" | "__doc__" => {
                    let func_bits = bound_method_func_bits(obj_ptr);
                    if let Some(func_ptr) = obj_from_bits(func_bits).as_ptr() {
                        if object_type_id(func_ptr) == TYPE_ID_FUNCTION {
                            if let Some(bits) = function_attr_bits(func_ptr, attr_bits) {
                                inc_ref_bits(bits);
                                return Some(bits);
                            }
                        }
                    }
                }
                _ => {}
            }
        }
    }
    if type_id == TYPE_ID_EXCEPTION {
        let name = string_obj_to_owned(obj_from_bits(attr_bits));
        let attr_name = name.as_deref()?;
        match attr_name {
            "__cause__" => {
                let bits = exception_cause_bits(obj_ptr);
                inc_ref_bits(bits);
                return Some(bits);
            }
            "__context__" => {
                let bits = exception_context_bits(obj_ptr);
                inc_ref_bits(bits);
                return Some(bits);
            }
            "__suppress_context__" => {
                let bits = exception_suppress_bits(obj_ptr);
                inc_ref_bits(bits);
                return Some(bits);
            }
            "__traceback__" => {
                let bits = exception_trace_bits(obj_ptr);
                inc_ref_bits(bits);
                return Some(bits);
            }
            "__class__" => {
                let mut class_bits = exception_class_bits(obj_ptr);
                if obj_from_bits(class_bits).is_none() || class_bits == 0 {
                    let new_bits = exception_type_bits(exception_kind_bits(obj_ptr));
                    let slot = obj_ptr.add(7 * std::mem::size_of::<u64>()) as *mut u64;
                    let old_bits = *slot;
                    if old_bits != new_bits {
                        dec_ref_bits(old_bits);
                        inc_ref_bits(new_bits);
                        *slot = new_bits;
                    }
                    class_bits = new_bits;
                }
                inc_ref_bits(class_bits);
                return Some(class_bits);
            }
            "__dict__" => {
                let mut dict_bits = exception_dict_bits(obj_ptr);
                if obj_from_bits(dict_bits).is_none() || dict_bits == 0 {
                    let dict_ptr = alloc_dict_with_pairs(&[]);
                    if dict_ptr.is_null() {
                        return None;
                    }
                    let new_bits = MoltObject::from_ptr(dict_ptr).bits();
                    let slot = obj_ptr.add(9 * std::mem::size_of::<u64>()) as *mut u64;
                    let old_bits = *slot;
                    if old_bits != new_bits {
                        dec_ref_bits(old_bits);
                        *slot = new_bits;
                    }
                    dict_bits = new_bits;
                }
                inc_ref_bits(dict_bits);
                return Some(dict_bits);
            }
            "args" => {
                let mut args_bits = exception_args_bits(obj_ptr);
                if obj_from_bits(args_bits).is_none() || args_bits == 0 {
                    let ptr = alloc_tuple(&[]);
                    if ptr.is_null() {
                        return None;
                    }
                    let new_bits = MoltObject::from_ptr(ptr).bits();
                    let slot = obj_ptr.add(8 * std::mem::size_of::<u64>()) as *mut u64;
                    let old_bits = *slot;
                    if old_bits != new_bits {
                        dec_ref_bits(old_bits);
                        *slot = new_bits;
                    }
                    args_bits = new_bits;
                }
                inc_ref_bits(args_bits);
                return Some(args_bits);
            }
            "value" => {
                let kind = string_obj_to_owned(obj_from_bits(exception_kind_bits(obj_ptr)));
                if kind.as_deref() == Some("StopIteration") {
                    let bits = exception_value_bits(obj_ptr);
                    inc_ref_bits(bits);
                    return Some(bits);
                }
            }
            _ => {}
        }
        let dict_bits = exception_dict_bits(obj_ptr);
        if !obj_from_bits(dict_bits).is_none() && dict_bits != 0 {
            if let Some(dict_ptr) = obj_from_bits(dict_bits).as_ptr() {
                if object_type_id(dict_ptr) == TYPE_ID_DICT {
                    if let Some(bits) = dict_get_in_place(dict_ptr, attr_bits) {
                        inc_ref_bits(bits);
                        return Some(bits);
                    }
                }
            }
        }
        let mut class_bits = exception_class_bits(obj_ptr);
        if obj_from_bits(class_bits).is_none() || class_bits == 0 {
            class_bits = exception_type_bits(exception_kind_bits(obj_ptr));
        }
        if let Some(class_ptr) = obj_from_bits(class_bits).as_ptr() {
            if object_type_id(class_ptr) == TYPE_ID_TYPE {
                if let Some(val_bits) =
                    class_attr_lookup(class_ptr, class_ptr, Some(obj_ptr), attr_bits)
                {
                    return Some(val_bits);
                }
                if exception_pending() {
                    return None;
                }
            }
        }
    }
    if type_id == TYPE_ID_GENERATOR {
        if let Some(name) = string_obj_to_owned(obj_from_bits(attr_bits)) {
            match name.as_str() {
                "gi_running" => {
                    return Some(MoltObject::from_bool(generator_running(obj_ptr)).bits());
                }
                "gi_frame" => {
                    if generator_closed(obj_ptr) {
                        return Some(MoltObject::none().bits());
                    }
                    let lasti = if generator_started(obj_ptr) { 0 } else { -1 };
                    let frame_bits = molt_object_new();
                    let Some(frame_ptr) = maybe_ptr_from_bits(frame_bits) else {
                        return Some(MoltObject::none().bits());
                    };
                    let name_bits =
                        intern_static_name(&runtime_state().interned.f_lasti_name, b"f_lasti");
                    let val_bits = MoltObject::from_int(lasti).bits();
                    let dict_ptr = alloc_dict_with_pairs(&[name_bits, val_bits]);
                    if !dict_ptr.is_null() {
                        let dict_bits = MoltObject::from_ptr(dict_ptr).bits();
                        instance_set_dict_bits(frame_ptr, dict_bits);
                        object_mark_has_ptrs(frame_ptr);
                    }
                    return Some(frame_bits);
                }
                _ => {}
            }
        }
    }
    if type_id == TYPE_ID_ASYNC_GENERATOR {
        if let Some(name) = string_obj_to_owned(obj_from_bits(attr_bits)) {
            match name.as_str() {
                "ag_running" => {
                    let gen_bits = asyncgen_gen_bits(obj_ptr);
                    let gen_running = if let Some(gen_ptr) = maybe_ptr_from_bits(gen_bits) {
                        object_type_id(gen_ptr) == TYPE_ID_GENERATOR && generator_running(gen_ptr)
                    } else {
                        false
                    };
                    let running = asyncgen_running(obj_ptr) || gen_running;
                    return Some(MoltObject::from_bool(running).bits());
                }
                "ag_await" => {
                    let await_bits = asyncgen_await_bits(obj_ptr);
                    return Some(await_bits);
                }
                "ag_code" => {
                    let code_bits = asyncgen_code_bits(obj_ptr);
                    return Some(code_bits);
                }
                "ag_frame" => {
                    let gen_bits = asyncgen_gen_bits(obj_ptr);
                    let Some(gen_ptr) = maybe_ptr_from_bits(gen_bits) else {
                        return Some(MoltObject::none().bits());
                    };
                    if object_type_id(gen_ptr) != TYPE_ID_GENERATOR {
                        return Some(MoltObject::none().bits());
                    }
                    if generator_closed(gen_ptr) {
                        return Some(MoltObject::none().bits());
                    }
                    let lasti = if generator_started(gen_ptr) { 0 } else { -1 };
                    let frame_bits = molt_object_new();
                    let Some(frame_ptr) = maybe_ptr_from_bits(frame_bits) else {
                        return Some(MoltObject::none().bits());
                    };
                    let name_bits =
                        intern_static_name(&runtime_state().interned.f_lasti_name, b"f_lasti");
                    let val_bits = MoltObject::from_int(lasti).bits();
                    let dict_ptr = alloc_dict_with_pairs(&[name_bits, val_bits]);
                    if !dict_ptr.is_null() {
                        let dict_bits = MoltObject::from_ptr(dict_ptr).bits();
                        instance_set_dict_bits(frame_ptr, dict_bits);
                        object_mark_has_ptrs(frame_ptr);
                    }
                    return Some(frame_bits);
                }
                _ => {}
            }
            if let Some(func_bits) = asyncgen_method_bits(name.as_str()) {
                let self_bits = MoltObject::from_ptr(obj_ptr).bits();
                let bound_bits = molt_bound_method_new(func_bits, self_bits);
                return Some(bound_bits);
            }
        }
    }
    if type_id == TYPE_ID_MEMORYVIEW {
        let name = string_obj_to_owned(obj_from_bits(attr_bits))?;
        match name.as_str() {
            "format" => {
                let bits = memoryview_format_bits(obj_ptr);
                inc_ref_bits(bits);
                return Some(bits);
            }
            "itemsize" => {
                return Some(MoltObject::from_int(memoryview_itemsize(obj_ptr) as i64).bits());
            }
            "ndim" => {
                return Some(MoltObject::from_int(memoryview_ndim(obj_ptr) as i64).bits());
            }
            "shape" => {
                let shape = memoryview_shape(obj_ptr).unwrap_or(&[]);
                return Some(tuple_from_isize_slice(shape));
            }
            "strides" => {
                let strides = memoryview_strides(obj_ptr).unwrap_or(&[]);
                return Some(tuple_from_isize_slice(strides));
            }
            "readonly" => {
                return Some(MoltObject::from_bool(memoryview_readonly(obj_ptr)).bits());
            }
            "nbytes" => {
                return Some(MoltObject::from_int(memoryview_nbytes(obj_ptr) as i64).bits());
            }
            _ => {}
        }
        if let Some(func_bits) = memoryview_method_bits(name.as_str()) {
            let self_bits = MoltObject::from_ptr(obj_ptr).bits();
            let bound_bits = molt_bound_method_new(func_bits, self_bits);
            return Some(bound_bits);
        }
    }
    if type_id == TYPE_ID_SLICE {
        if let Some(name) = string_obj_to_owned(obj_from_bits(attr_bits)) {
            match name.as_str() {
                "start" => {
                    let bits = slice_start_bits(obj_ptr);
                    inc_ref_bits(bits);
                    return Some(bits);
                }
                "stop" => {
                    let bits = slice_stop_bits(obj_ptr);
                    inc_ref_bits(bits);
                    return Some(bits);
                }
                "step" => {
                    let bits = slice_step_bits(obj_ptr);
                    inc_ref_bits(bits);
                    return Some(bits);
                }
                _ => {}
            }
            if let Some(func_bits) = slice_method_bits(name.as_str()) {
                let self_bits = MoltObject::from_ptr(obj_ptr).bits();
                let bound_bits = molt_bound_method_new(func_bits, self_bits);
                return Some(bound_bits);
            }
        }
    }
    if type_id == TYPE_ID_GENERIC_ALIAS {
        if let Some(name) = string_obj_to_owned(obj_from_bits(attr_bits)) {
            match name.as_str() {
                "__origin__" => {
                    let bits = generic_alias_origin_bits(obj_ptr);
                    inc_ref_bits(bits);
                    return Some(bits);
                }
                "__args__" => {
                    let bits = generic_alias_args_bits(obj_ptr);
                    inc_ref_bits(bits);
                    return Some(bits);
                }
                "__parameters__" => {
                    // TODO(type-coverage, owner:runtime, milestone:TC3, priority:P2, status:partial):
                    // derive __parameters__ from TypeVar/ParamSpec/TypeVarTuple when typing supports them.
                    let tuple_ptr = alloc_tuple(&[]);
                    if tuple_ptr.is_null() {
                        return Some(MoltObject::none().bits());
                    }
                    return Some(MoltObject::from_ptr(tuple_ptr).bits());
                }
                "__unpacked__" => {
                    return Some(MoltObject::from_bool(false).bits());
                }
                _ => {}
            }
        }
    }
    if type_id == TYPE_ID_FILE_HANDLE {
        if let Some(name) = string_obj_to_owned(obj_from_bits(attr_bits)) {
            let handle_ptr = file_handle_ptr(obj_ptr);
            if handle_ptr.is_null() {
                return None;
            }
            let handle = &*handle_ptr;
            match name.as_str() {
                "__class__" => {
                    let class_bits = if handle.class_bits != 0 {
                        handle.class_bits
                    } else {
                        builtin_classes().file
                    };
                    inc_ref_bits(class_bits);
                    return Some(class_bits);
                }
                "closed" => {
                    if handle.detached {
                        return raise_exception::<_>(
                            "ValueError",
                            file_handle_detached_message(handle),
                        );
                    }
                    return Some(MoltObject::from_bool(file_handle_is_closed(handle)).bits());
                }
                "name" => {
                    if handle.detached {
                        return raise_exception::<_>(
                            "ValueError",
                            file_handle_detached_message(handle),
                        );
                    }
                    if handle.name_bits != 0 {
                        inc_ref_bits(handle.name_bits);
                        return Some(handle.name_bits);
                    }
                    return Some(MoltObject::none().bits());
                }
                "mode" => {
                    if handle.detached && !handle.text {
                        return raise_exception::<_>(
                            "ValueError",
                            file_handle_detached_message(handle),
                        );
                    }
                    let ptr = alloc_string(handle.mode.as_bytes());
                    if ptr.is_null() {
                        return Some(MoltObject::none().bits());
                    }
                    return Some(MoltObject::from_ptr(ptr).bits());
                }
                "encoding" => {
                    if !handle.text {
                        return None;
                    }
                    let Some(encoding) = handle.encoding.as_deref() else {
                        return Some(MoltObject::none().bits());
                    };
                    let ptr = alloc_string(encoding.as_bytes());
                    if ptr.is_null() {
                        return Some(MoltObject::none().bits());
                    }
                    return Some(MoltObject::from_ptr(ptr).bits());
                }
                "errors" => {
                    if !handle.text {
                        return None;
                    }
                    let Some(errors) = handle.errors.as_deref() else {
                        return Some(MoltObject::none().bits());
                    };
                    let ptr = alloc_string(errors.as_bytes());
                    if ptr.is_null() {
                        return Some(MoltObject::none().bits());
                    }
                    return Some(MoltObject::from_ptr(ptr).bits());
                }
                "newline" => {
                    if !handle.text {
                        return None;
                    }
                    let Some(newline) = handle.newline.as_deref() else {
                        return Some(MoltObject::none().bits());
                    };
                    let ptr = alloc_string(newline.as_bytes());
                    if ptr.is_null() {
                        return Some(MoltObject::none().bits());
                    }
                    return Some(MoltObject::from_ptr(ptr).bits());
                }
                "line_buffering" => {
                    return Some(MoltObject::from_bool(handle.line_buffering).bits());
                }
                "write_through" => {
                    if !handle.text {
                        return None;
                    }
                    return Some(MoltObject::from_bool(handle.write_through).bits());
                }
                "buffer" => {
                    if handle.detached {
                        return Some(MoltObject::none().bits());
                    }
                    if handle.buffer_bits != 0 {
                        inc_ref_bits(handle.buffer_bits);
                        return Some(handle.buffer_bits);
                    }
                    return None;
                }
                _ => {}
            }
            if handle.text && name == "readinto" {
                return None;
            }
            if !handle.text && name == "reconfigure" {
                return None;
            }
            if let Some(func_bits) = file_method_bits(name.as_str()) {
                let self_bits = MoltObject::from_ptr(obj_ptr).bits();
                let bound_bits = molt_bound_method_new(func_bits, self_bits);
                return Some(bound_bits);
            }
        }
    }
    if type_id == TYPE_ID_DICT {
        if let Some(name) = string_obj_to_owned(obj_from_bits(attr_bits)) {
            if name == "fromkeys" {
                if let Some(func_bits) = dict_method_bits(name.as_str()) {
                    let self_bits = MoltObject::from_ptr(obj_ptr).bits();
                    let class_bits = type_of_bits(self_bits);
                    let bound_bits = molt_bound_method_new(func_bits, class_bits);
                    return Some(bound_bits);
                }
            }
            if let Some(func_bits) = dict_method_bits(name.as_str()) {
                let self_bits = MoltObject::from_ptr(obj_ptr).bits();
                let bound_bits = molt_bound_method_new(func_bits, self_bits);
                return Some(bound_bits);
            }
        }
    }
    if type_id == TYPE_ID_SET {
        if let Some(name) = string_obj_to_owned(obj_from_bits(attr_bits)) {
            if let Some(func_bits) = set_method_bits(name.as_str()) {
                let self_bits = MoltObject::from_ptr(obj_ptr).bits();
                let bound_bits = molt_bound_method_new(func_bits, self_bits);
                return Some(bound_bits);
            }
        }
    }
    if type_id == TYPE_ID_FROZENSET {
        if let Some(name) = string_obj_to_owned(obj_from_bits(attr_bits)) {
            if let Some(func_bits) = frozenset_method_bits(name.as_str()) {
                let self_bits = MoltObject::from_ptr(obj_ptr).bits();
                let bound_bits = molt_bound_method_new(func_bits, self_bits);
                return Some(bound_bits);
            }
        }
    }
    if type_id == TYPE_ID_LIST {
        if let Some(name) = string_obj_to_owned(obj_from_bits(attr_bits)) {
            if let Some(func_bits) = list_method_bits(name.as_str()) {
                let self_bits = MoltObject::from_ptr(obj_ptr).bits();
                let bound_bits = molt_bound_method_new(func_bits, self_bits);
                return Some(bound_bits);
            }
        }
    }
    if type_id == TYPE_ID_STRING {
        if let Some(name) = string_obj_to_owned(obj_from_bits(attr_bits)) {
            if let Some(func_bits) = string_method_bits(name.as_str()) {
                let self_bits = MoltObject::from_ptr(obj_ptr).bits();
                let bound_bits = molt_bound_method_new(func_bits, self_bits);
                return Some(bound_bits);
            }
        }
    }
    if type_id == TYPE_ID_BYTES {
        if let Some(name) = string_obj_to_owned(obj_from_bits(attr_bits)) {
            if let Some(func_bits) = bytes_method_bits(name.as_str()) {
                let self_bits = MoltObject::from_ptr(obj_ptr).bits();
                let bound_bits = molt_bound_method_new(func_bits, self_bits);
                return Some(bound_bits);
            }
        }
    }
    if type_id == TYPE_ID_BYTEARRAY {
        if let Some(name) = string_obj_to_owned(obj_from_bits(attr_bits)) {
            if let Some(func_bits) = bytearray_method_bits(name.as_str()) {
                let self_bits = MoltObject::from_ptr(obj_ptr).bits();
                let bound_bits = molt_bound_method_new(func_bits, self_bits);
                return Some(bound_bits);
            }
        }
    }
    if type_id == TYPE_ID_TYPE {
        if let Some(name) = string_obj_to_owned(obj_from_bits(attr_bits)) {
            if name == "__class__" {
                let builtins = builtin_classes();
                inc_ref_bits(builtins.type_obj);
                return Some(builtins.type_obj);
            }
            if name == "__dict__" {
                let dict_bits = class_dict_bits(obj_ptr);
                inc_ref_bits(dict_bits);
                return Some(dict_bits);
            }
            if name == "__annotate__" {
                let mut annotate_bits = class_annotate_bits(obj_ptr);
                if annotate_bits == 0 {
                    let annotate_name_bits = intern_static_name(
                        &runtime_state().interned.annotate_name,
                        b"__annotate__",
                    );
                    let dict_bits = class_dict_bits(obj_ptr);
                    if let Some(dict_ptr) = obj_from_bits(dict_bits).as_ptr() {
                        if object_type_id(dict_ptr) == TYPE_ID_DICT {
                            if let Some(val_bits) = dict_get_in_place(dict_ptr, annotate_name_bits)
                            {
                                annotate_bits = val_bits;
                                class_set_annotate_bits(obj_ptr, annotate_bits);
                            }
                        }
                    }
                    if annotate_bits == 0 {
                        annotate_bits = MoltObject::none().bits();
                    }
                }
                inc_ref_bits(annotate_bits);
                return Some(annotate_bits);
            }
            if name == "__annotations__" {
                let annotations_bits = intern_static_name(
                    &runtime_state().interned.annotations_name,
                    b"__annotations__",
                );
                let dict_bits = class_dict_bits(obj_ptr);
                if let Some(dict_ptr) = obj_from_bits(dict_bits).as_ptr() {
                    if object_type_id(dict_ptr) == TYPE_ID_DICT {
                        if let Some(val_bits) = dict_get_in_place(dict_ptr, annotations_bits) {
                            inc_ref_bits(val_bits);
                            class_set_annotations_bits(obj_ptr, val_bits);
                            return Some(val_bits);
                        }
                    }
                }
                let cached = class_annotations_bits(obj_ptr);
                if cached != 0 {
                    inc_ref_bits(cached);
                    return Some(cached);
                }
                let annotate_bits = class_annotate_bits(obj_ptr);
                let res_bits = if annotate_bits != 0 && !obj_from_bits(annotate_bits).is_none() {
                    let format_bits = MoltObject::from_int(1).bits();
                    let res_bits = call_callable1(annotate_bits, format_bits);
                    if exception_pending() {
                        return None;
                    }
                    let res_obj = obj_from_bits(res_bits);
                    let Some(res_ptr) = res_obj.as_ptr() else {
                        let msg = format!(
                            "__annotate__ returned non-dict of type '{}'",
                            type_name(res_obj)
                        );
                        dec_ref_bits(res_bits);
                        return raise_exception::<_>("TypeError", &msg);
                    };
                    if object_type_id(res_ptr) != TYPE_ID_DICT {
                        let msg = format!(
                            "__annotate__ returned non-dict of type '{}'",
                            type_name(res_obj)
                        );
                        dec_ref_bits(res_bits);
                        return raise_exception::<_>("TypeError", &msg);
                    }
                    res_bits
                } else {
                    let dict_ptr = alloc_dict_with_pairs(&[]);
                    if dict_ptr.is_null() {
                        return None;
                    }
                    MoltObject::from_ptr(dict_ptr).bits()
                };
                class_set_annotations_bits(obj_ptr, res_bits);
                if let Some(dict_ptr) = obj_from_bits(dict_bits).as_ptr() {
                    if object_type_id(dict_ptr) == TYPE_ID_DICT {
                        dict_set_in_place(dict_ptr, annotations_bits, res_bits);
                    }
                }
                return Some(res_bits);
            }
            if name == "__name__" {
                let bits = class_name_bits(obj_ptr);
                inc_ref_bits(bits);
                return Some(bits);
            }
            if name == "__base__" {
                let bases_bits = class_bases_bits(obj_ptr);
                let bases = class_bases_vec(bases_bits);
                if bases.is_empty() {
                    let none_bits = MoltObject::none().bits();
                    inc_ref_bits(none_bits);
                    return Some(none_bits);
                }
                let base_bits = bases[0];
                inc_ref_bits(base_bits);
                return Some(base_bits);
            }
            if name == "__bases__" {
                let bases_bits = class_bases_bits(obj_ptr);
                let bases_obj = obj_from_bits(bases_bits);
                if bases_obj.is_none() || bases_bits == 0 {
                    let tuple_ptr = alloc_tuple(&[]);
                    if tuple_ptr.is_null() {
                        return None;
                    }
                    return Some(MoltObject::from_ptr(tuple_ptr).bits());
                }
                if let Some(bases_ptr) = bases_obj.as_ptr() {
                    let bases_type = object_type_id(bases_ptr);
                    if bases_type == TYPE_ID_TUPLE {
                        inc_ref_bits(bases_bits);
                        return Some(bases_bits);
                    }
                    if bases_type == TYPE_ID_TYPE {
                        let tuple_ptr = alloc_tuple(&[bases_bits]);
                        if tuple_ptr.is_null() {
                            return None;
                        }
                        return Some(MoltObject::from_ptr(tuple_ptr).bits());
                    }
                }
                return None;
            }
            let class_bits = MoltObject::from_ptr(obj_ptr).bits();
            if name == "fromkeys" {
                let builtins = builtin_classes();
                if issubclass_bits(class_bits, builtins.dict) {
                    if let Some(func_bits) = dict_method_bits(name.as_str()) {
                        let bound_bits = molt_bound_method_new(func_bits, class_bits);
                        return Some(bound_bits);
                    }
                }
            }
            if is_builtin_class_bits(class_bits) {
                if let Some(func_bits) = builtin_class_method_bits(class_bits, name.as_str()) {
                    return descriptor_bind(func_bits, obj_ptr, None);
                }
            }
        }
        return class_attr_lookup(obj_ptr, obj_ptr, None, attr_bits);
    }
    if type_id == TYPE_ID_SUPER {
        let attr_name = string_obj_to_owned(obj_from_bits(attr_bits));
        let start_bits = super_type_bits(obj_ptr);
        let target_bits = super_obj_bits(obj_ptr);
        let target_ptr = maybe_ptr_from_bits(target_bits);
        let obj_type_bits = if let Some(raw_ptr) = target_ptr {
            if object_type_id(raw_ptr) == TYPE_ID_TYPE {
                target_bits
            } else {
                type_of_bits(target_bits)
            }
        } else {
            type_of_bits(target_bits)
        };
        let obj_type_ptr = obj_from_bits(obj_type_bits).as_ptr()?;
        if object_type_id(obj_type_ptr) != TYPE_ID_TYPE {
            return None;
        }
        let mro_storage: Cow<'_, [u64]> = if let Some(mro) = class_mro_ref(obj_type_ptr) {
            Cow::Borrowed(mro.as_slice())
        } else {
            Cow::Owned(class_mro_vec(obj_type_bits))
        };
        let mut instance_ptr = None;
        let mut owner_ptr = obj_type_ptr;
        if let Some(raw_ptr) = target_ptr {
            if object_type_id(raw_ptr) == TYPE_ID_TYPE {
                owner_ptr = raw_ptr;
            } else {
                instance_ptr = Some(raw_ptr);
            }
        }
        let mut found_start = false;
        for class_bits in mro_storage.iter() {
            if !found_start {
                if *class_bits == start_bits {
                    found_start = true;
                }
                continue;
            }
            let class_obj = obj_from_bits(*class_bits);
            let Some(class_ptr) = class_obj.as_ptr() else {
                continue;
            };
            if object_type_id(class_ptr) != TYPE_ID_TYPE {
                continue;
            }
            let dict_bits = class_dict_bits(class_ptr);
            let dict_obj = obj_from_bits(dict_bits);
            let Some(dict_ptr) = dict_obj.as_ptr() else {
                continue;
            };
            if object_type_id(dict_ptr) != TYPE_ID_DICT {
                continue;
            }
            if let Some(val_bits) = dict_get_in_place(dict_ptr, attr_bits) {
                return descriptor_bind(val_bits, owner_ptr, instance_ptr);
            }
            if let Some(name) = attr_name.as_deref() {
                if is_builtin_class_bits(*class_bits) {
                    if let Some(func_bits) = builtin_class_method_bits(*class_bits, name) {
                        return descriptor_bind(func_bits, owner_ptr, instance_ptr);
                    }
                }
            }
        }
        return None;
    }
    if type_id == TYPE_ID_FUNCTION {
        if let Some(name) = string_obj_to_owned(obj_from_bits(attr_bits)) {
            if name == "__code__" {
                let code_bits = ensure_function_code_bits(obj_ptr);
                if !obj_from_bits(code_bits).is_none() {
                    inc_ref_bits(code_bits);
                    return Some(code_bits);
                }
                return None;
            }
        }
        let annotate_name_bits =
            intern_static_name(&runtime_state().interned.annotate_name, b"__annotate__");
        if obj_eq(obj_from_bits(attr_bits), obj_from_bits(annotate_name_bits)) {
            let mut annotate_bits = function_annotate_bits(obj_ptr);
            if annotate_bits == 0 {
                annotate_bits = MoltObject::none().bits();
            }
            inc_ref_bits(annotate_bits);
            return Some(annotate_bits);
        }
        let annotations_bits = intern_static_name(
            &runtime_state().interned.annotations_name,
            b"__annotations__",
        );
        if obj_eq(obj_from_bits(attr_bits), obj_from_bits(annotations_bits)) {
            let cached = function_annotations_bits(obj_ptr);
            if cached != 0 {
                inc_ref_bits(cached);
                return Some(cached);
            }
            let annotate_bits = function_annotate_bits(obj_ptr);
            let res_bits = if annotate_bits != 0 && !obj_from_bits(annotate_bits).is_none() {
                let format_bits = MoltObject::from_int(1).bits();
                let res_bits = call_callable1(annotate_bits, format_bits);
                if exception_pending() {
                    return None;
                }
                let res_obj = obj_from_bits(res_bits);
                let Some(res_ptr) = res_obj.as_ptr() else {
                    let msg = format!(
                        "__annotate__ returned non-dict of type '{}'",
                        type_name(res_obj)
                    );
                    dec_ref_bits(res_bits);
                    return raise_exception::<_>("TypeError", &msg);
                };
                if object_type_id(res_ptr) != TYPE_ID_DICT {
                    let msg = format!(
                        "__annotate__ returned non-dict of type '{}'",
                        type_name(res_obj)
                    );
                    dec_ref_bits(res_bits);
                    return raise_exception::<_>("TypeError", &msg);
                }
                res_bits
            } else {
                let dict_ptr = alloc_dict_with_pairs(&[]);
                if dict_ptr.is_null() {
                    return None;
                }
                MoltObject::from_ptr(dict_ptr).bits()
            };
            function_set_annotations_bits(obj_ptr, res_bits);
            return Some(res_bits);
        }
        let dict_name_bits = intern_static_name(&runtime_state().interned.dict_name, b"__dict__");
        if obj_eq(obj_from_bits(attr_bits), obj_from_bits(dict_name_bits)) {
            let mut dict_bits = function_dict_bits(obj_ptr);
            if dict_bits == 0 {
                let dict_ptr = alloc_dict_with_pairs(&[]);
                if dict_ptr.is_null() {
                    return None;
                }
                dict_bits = MoltObject::from_ptr(dict_ptr).bits();
                function_set_dict_bits(obj_ptr, dict_bits);
            }
            inc_ref_bits(dict_bits);
            return Some(dict_bits);
        }
        let dict_bits = function_dict_bits(obj_ptr);
        if dict_bits != 0 {
            if let Some(dict_ptr) = obj_from_bits(dict_bits).as_ptr() {
                if object_type_id(dict_ptr) == TYPE_ID_DICT {
                    if let Some(val) = dict_get_in_place(dict_ptr, attr_bits) {
                        inc_ref_bits(val);
                        return Some(val);
                    }
                }
            }
        }
        return None;
    }
    if type_id == TYPE_ID_CODE {
        // TODO(introspection, owner:runtime, milestone:TC2, priority:P2, status:partial):
        // fill out code object fields (co_varnames, arg counts, co_linetable) for parity.
        let name = string_obj_to_owned(obj_from_bits(attr_bits))?;
        match name.as_str() {
            "co_filename" => {
                let bits = code_filename_bits(obj_ptr);
                inc_ref_bits(bits);
                return Some(bits);
            }
            "co_name" => {
                let bits = code_name_bits(obj_ptr);
                inc_ref_bits(bits);
                return Some(bits);
            }
            "co_firstlineno" => {
                return Some(MoltObject::from_int(code_firstlineno(obj_ptr)).bits());
            }
            "co_linetable" => {
                let bits = code_linetable_bits(obj_ptr);
                if bits != 0 {
                    inc_ref_bits(bits);
                    return Some(bits);
                }
                return Some(MoltObject::none().bits());
            }
            "co_varnames" => {
                let tuple_ptr = alloc_tuple(&[]);
                if tuple_ptr.is_null() {
                    return Some(MoltObject::none().bits());
                }
                return Some(MoltObject::from_ptr(tuple_ptr).bits());
            }
            _ => {}
        }
        return None;
    }
    if type_id == TYPE_ID_DATACLASS {
        let desc_ptr = dataclass_desc_ptr(obj_ptr);
        if !desc_ptr.is_null() {
            let slots = (*desc_ptr).slots;
            let class_bits = (*desc_ptr).class_bits;
            if class_bits != 0 {
                if let Some(class_ptr) = obj_from_bits(class_bits).as_ptr() {
                    if object_type_id(class_ptr) == TYPE_ID_TYPE {
                        let getattribute_bits = intern_static_name(
                            &runtime_state().interned.getattribute_name,
                            b"__getattribute__",
                        );
                        if !obj_eq(obj_from_bits(attr_bits), obj_from_bits(getattribute_bits)) {
                            if let Some(call_bits) = class_attr_lookup(
                                class_ptr,
                                class_ptr,
                                Some(obj_ptr),
                                getattribute_bits,
                            ) {
                                exception_stack_push();
                                let res_bits = call_callable1(call_bits, attr_bits);
                                if exception_pending() {
                                    let exc_bits = molt_exception_last();
                                    let kind_bits = molt_exception_kind(exc_bits);
                                    let kind = string_obj_to_owned(obj_from_bits(kind_bits));
                                    dec_ref_bits(kind_bits);
                                    if kind.as_deref() == Some("AttributeError") {
                                        let getattr_bits = intern_static_name(
                                            &runtime_state().interned.getattr_name,
                                            b"__getattr__",
                                        );
                                        if !obj_eq(
                                            obj_from_bits(attr_bits),
                                            obj_from_bits(getattr_bits),
                                        ) && class_attr_lookup_raw_mro(class_ptr, getattr_bits)
                                            .is_some()
                                        {
                                            molt_exception_clear();
                                            dec_ref_bits(exc_bits);
                                            exception_stack_pop();
                                            if let Some(getattr_call_bits) = class_attr_lookup(
                                                class_ptr,
                                                class_ptr,
                                                Some(obj_ptr),
                                                getattr_bits,
                                            ) {
                                                let getattr_res =
                                                    call_callable1(getattr_call_bits, attr_bits);
                                                if exception_pending() {
                                                    return None;
                                                }
                                                return Some(getattr_res);
                                            }
                                        }
                                    }
                                    dec_ref_bits(exc_bits);
                                    exception_stack_pop();
                                    return None;
                                }
                                exception_stack_pop();
                                return Some(res_bits);
                            }
                        }
                        if let Some(val_bits) = class_attr_lookup_raw_mro(class_ptr, attr_bits) {
                            if descriptor_is_data(val_bits) {
                                if let Some(bound) =
                                    descriptor_bind(val_bits, class_ptr, Some(obj_ptr))
                                {
                                    return Some(bound);
                                }
                                if exception_pending() {
                                    return None;
                                }
                            }
                        }
                    }
                }
            }
            let class_name_bits =
                intern_static_name(&runtime_state().interned.class_name, b"__class__");
            if obj_eq(obj_from_bits(attr_bits), obj_from_bits(class_name_bits)) {
                if class_bits != 0 {
                    inc_ref_bits(class_bits);
                    return Some(class_bits);
                }
                return None;
            }
            let dict_name_bits =
                intern_static_name(&runtime_state().interned.dict_name, b"__dict__");
            if obj_eq(obj_from_bits(attr_bits), obj_from_bits(dict_name_bits)) {
                if !slots {
                    let mut dict_bits = dataclass_dict_bits(obj_ptr);
                    if dict_bits == 0 {
                        let dict_ptr = alloc_dict_with_pairs(&[]);
                        if !dict_ptr.is_null() {
                            dict_bits = MoltObject::from_ptr(dict_ptr).bits();
                            dataclass_set_dict_bits(obj_ptr, dict_bits);
                        }
                    }
                    if dict_bits != 0 {
                        inc_ref_bits(dict_bits);
                        return Some(dict_bits);
                    }
                }
                return None;
            }
            if !slots {
                let dict_bits = dataclass_dict_bits(obj_ptr);
                if let Some(dict_ptr) = obj_from_bits(dict_bits).as_ptr() {
                    if object_type_id(dict_ptr) == TYPE_ID_DICT {
                        if let Some(val) = dict_get_in_place(dict_ptr, attr_bits) {
                            inc_ref_bits(val);
                            return Some(val);
                        }
                    }
                }
            }
            if class_bits != 0 {
                if let Some(class_ptr) = obj_from_bits(class_bits).as_ptr() {
                    if object_type_id(class_ptr) == TYPE_ID_TYPE {
                        if let Some(val_bits) = class_attr_lookup_raw_mro(class_ptr, attr_bits) {
                            if let Some(bound) = descriptor_bind(val_bits, class_ptr, Some(obj_ptr))
                            {
                                return Some(bound);
                            }
                            if exception_pending() {
                                return None;
                            }
                        }
                    }
                }
            }
            if class_bits != 0 {
                if let Some(class_ptr) = obj_from_bits(class_bits).as_ptr() {
                    if object_type_id(class_ptr) == TYPE_ID_TYPE {
                        let getattr_bits = intern_static_name(
                            &runtime_state().interned.getattr_name,
                            b"__getattr__",
                        );
                        if !obj_eq(obj_from_bits(attr_bits), obj_from_bits(getattr_bits)) {
                            if let Some(call_bits) =
                                class_attr_lookup(class_ptr, class_ptr, Some(obj_ptr), getattr_bits)
                            {
                                let res_bits = call_callable1(call_bits, attr_bits);
                                return Some(res_bits);
                            }
                        }
                    }
                }
            }
        }
        return None;
    }
    if type_id == TYPE_ID_OBJECT {
        let class_bits = object_class_bits(obj_ptr);
        let mut cached_attr_bits: Option<u64> = None;
        if class_bits != 0 {
            if let Some(class_ptr) = obj_from_bits(class_bits).as_ptr() {
                if object_type_id(class_ptr) == TYPE_ID_TYPE {
                    let getattribute_bits = intern_static_name(
                        &runtime_state().interned.getattribute_name,
                        b"__getattribute__",
                    );
                    if !obj_eq(obj_from_bits(attr_bits), obj_from_bits(getattribute_bits)) {
                        if let Some(call_bits) = class_attr_lookup(
                            class_ptr,
                            class_ptr,
                            Some(obj_ptr),
                            getattribute_bits,
                        ) {
                            exception_stack_push();
                            let res_bits = call_callable1(call_bits, attr_bits);
                            if exception_pending() {
                                let exc_bits = molt_exception_last();
                                let kind_bits = molt_exception_kind(exc_bits);
                                let kind = string_obj_to_owned(obj_from_bits(kind_bits));
                                dec_ref_bits(kind_bits);
                                if kind.as_deref() == Some("AttributeError") {
                                    let getattr_bits = intern_static_name(
                                        &runtime_state().interned.getattr_name,
                                        b"__getattr__",
                                    );
                                    if !obj_eq(
                                        obj_from_bits(attr_bits),
                                        obj_from_bits(getattr_bits),
                                    ) && class_attr_lookup_raw_mro(class_ptr, getattr_bits)
                                        .is_some()
                                    {
                                        molt_exception_clear();
                                        dec_ref_bits(exc_bits);
                                        exception_stack_pop();
                                        if let Some(getattr_call_bits) = class_attr_lookup(
                                            class_ptr,
                                            class_ptr,
                                            Some(obj_ptr),
                                            getattr_bits,
                                        ) {
                                            let getattr_res =
                                                call_callable1(getattr_call_bits, attr_bits);
                                            if exception_pending() {
                                                return None;
                                            }
                                            return Some(getattr_res);
                                        }
                                    }
                                }
                                dec_ref_bits(exc_bits);
                                exception_stack_pop();
                                return None;
                            }
                            exception_stack_pop();
                            return Some(res_bits);
                        }
                    }
                    let class_version = class_layout_version_bits(class_ptr);
                    if let Some(entry) =
                        descriptor_cache_lookup(class_bits, attr_bits, class_version)
                    {
                        if let Some(bits) = entry.data_desc_bits {
                            if let Some(bound) = descriptor_bind(bits, class_ptr, Some(obj_ptr)) {
                                return Some(bound);
                            }
                            if exception_pending() {
                                return None;
                            }
                        }
                        cached_attr_bits = entry.class_attr_bits;
                    }
                    if cached_attr_bits.is_none() {
                        if let Some(val_bits) = class_attr_lookup_raw_mro(class_ptr, attr_bits) {
                            if descriptor_is_data(val_bits) {
                                descriptor_cache_store(
                                    class_bits,
                                    attr_bits,
                                    class_version,
                                    Some(val_bits),
                                    None,
                                );
                                if let Some(bound) =
                                    descriptor_bind(val_bits, class_ptr, Some(obj_ptr))
                                {
                                    return Some(bound);
                                }
                                if exception_pending() {
                                    return None;
                                }
                            }
                            cached_attr_bits = Some(val_bits);
                            descriptor_cache_store(
                                class_bits,
                                attr_bits,
                                class_version,
                                None,
                                Some(val_bits),
                            );
                        } else {
                            descriptor_cache_store(
                                class_bits,
                                attr_bits,
                                class_version,
                                None,
                                None,
                            );
                        }
                    }
                    if let Some(offset) = class_field_offset(class_ptr, attr_bits) {
                        let bits = object_field_get_ptr_raw(obj_ptr, offset);
                        return Some(bits);
                    }
                }
            }
        }
        let class_name_bits =
            intern_static_name(&runtime_state().interned.class_name, b"__class__");
        if obj_eq(obj_from_bits(attr_bits), obj_from_bits(class_name_bits)) {
            if class_bits != 0 {
                inc_ref_bits(class_bits);
                return Some(class_bits);
            }
            return None;
        }
        let dict_name_bits = intern_static_name(&runtime_state().interned.dict_name, b"__dict__");
        if obj_eq(obj_from_bits(attr_bits), obj_from_bits(dict_name_bits)) {
            let mut dict_bits = instance_dict_bits(obj_ptr);
            if dict_bits == 0 {
                let dict_ptr = alloc_dict_with_pairs(&[]);
                if !dict_ptr.is_null() {
                    dict_bits = MoltObject::from_ptr(dict_ptr).bits();
                    instance_set_dict_bits(obj_ptr, dict_bits);
                }
            }
            if dict_bits != 0 {
                inc_ref_bits(dict_bits);
                return Some(dict_bits);
            }
            return None;
        }
        let dict_bits = instance_dict_bits(obj_ptr);
        if dict_bits != 0 {
            if let Some(dict_ptr) = obj_from_bits(dict_bits).as_ptr() {
                if object_type_id(dict_ptr) == TYPE_ID_DICT {
                    if let Some(val) = dict_get_in_place(dict_ptr, attr_bits) {
                        inc_ref_bits(val);
                        return Some(val);
                    }
                }
            }
        }
        if class_bits != 0 {
            if let Some(class_ptr) = obj_from_bits(class_bits).as_ptr() {
                if object_type_id(class_ptr) == TYPE_ID_TYPE {
                    if cached_attr_bits.is_none() {
                        if let Some(val_bits) = class_attr_lookup_raw_mro(class_ptr, attr_bits) {
                            cached_attr_bits = Some(val_bits);
                            let class_version = class_layout_version_bits(class_ptr);
                            descriptor_cache_store(
                                class_bits,
                                attr_bits,
                                class_version,
                                None,
                                Some(val_bits),
                            );
                        }
                    }
                    if let Some(val_bits) = cached_attr_bits {
                        if let Some(bound) = descriptor_bind(val_bits, class_ptr, Some(obj_ptr)) {
                            return Some(bound);
                        }
                        if exception_pending() {
                            return None;
                        }
                    }
                }
            }
        }
        if class_bits != 0 {
            if let Some(class_ptr) = obj_from_bits(class_bits).as_ptr() {
                if object_type_id(class_ptr) == TYPE_ID_TYPE {
                    let getattr_bits =
                        intern_static_name(&runtime_state().interned.getattr_name, b"__getattr__");
                    if !obj_eq(obj_from_bits(attr_bits), obj_from_bits(getattr_bits)) {
                        if let Some(call_bits) =
                            class_attr_lookup(class_ptr, class_ptr, Some(obj_ptr), getattr_bits)
                        {
                            let res_bits = call_callable1(call_bits, attr_bits);
                            return Some(res_bits);
                        }
                    }
                }
            }
        }
        return None;
    }
    None
}

/// # Safety
/// Dereferences raw pointers. Caller must ensure attr_name_ptr is valid UTF-8.
#[no_mangle]
pub unsafe extern "C" fn molt_get_attr_generic(
    obj_ptr: *mut u8,
    attr_name_ptr: *const u8,
    attr_name_len_bits: u64,
) -> i64 {
    let attr_name_len = usize_from_bits(attr_name_len_bits);
    if obj_ptr.is_null() {
        return raise_exception::<_>("AttributeError", "object has no attribute");
    }
    let slice = std::slice::from_raw_parts(attr_name_ptr, attr_name_len);
    let attr_name = std::str::from_utf8(slice).unwrap_or("<attr>");
    let Some(attr_bits) = attr_name_bits_from_bytes(slice) else {
        return MoltObject::none().bits() as i64;
    };
    let found = attr_lookup_ptr(obj_ptr, attr_bits);
    dec_ref_bits(attr_bits);
    if let Some(val) = found {
        return val as i64;
    }
    if exception_pending() {
        let exc_bits = molt_exception_last();
        molt_exception_clear();
        let _ = molt_raise(exc_bits);
        dec_ref_bits(exc_bits);
        return MoltObject::none().bits() as i64;
    }
    let type_id = object_type_id(obj_ptr);
    if type_id == TYPE_ID_DATACLASS {
        let desc_ptr = dataclass_desc_ptr(obj_ptr);
        if !desc_ptr.is_null() && (*desc_ptr).slots {
            let name = &(*desc_ptr).name;
            let type_label = if name.is_empty() {
                "dataclass"
            } else {
                name.as_str()
            };
            return attr_error(type_label, attr_name);
        }
        let type_label = if !desc_ptr.is_null() {
            let name = &(*desc_ptr).name;
            if name.is_empty() {
                "dataclass"
            } else {
                name.as_str()
            }
        } else {
            "dataclass"
        };
        return attr_error(type_label, attr_name);
    }
    if type_id == TYPE_ID_TYPE {
        let class_name =
            string_obj_to_owned(obj_from_bits(class_name_bits(obj_ptr))).unwrap_or_default();
        let msg = format!("type object '{class_name}' has no attribute '{attr_name}'");
        return raise_exception::<_>("AttributeError", &msg);
    }
    attr_error(type_name(MoltObject::from_ptr(obj_ptr)), attr_name)
}

/// # Safety
/// Dereferences raw pointers. Caller must ensure attr_name_ptr is valid UTF-8.
#[no_mangle]
pub unsafe extern "C" fn molt_get_attr_ptr(
    obj_ptr: *mut u8,
    attr_name_ptr: *const u8,
    attr_name_len_bits: u64,
) -> i64 {
    molt_get_attr_generic(obj_ptr, attr_name_ptr, attr_name_len_bits)
}

/// # Safety
/// Dereferences raw pointers. Caller must ensure attr_name_ptr is valid UTF-8.
#[no_mangle]
pub unsafe extern "C" fn molt_set_attr_generic(
    obj_ptr: *mut u8,
    attr_name_ptr: *const u8,
    attr_name_len_bits: u64,
    val_bits: u64,
) -> i64 {
    let attr_name_len = usize_from_bits(attr_name_len_bits);
    if obj_ptr.is_null() {
        return raise_exception::<_>("AttributeError", "object has no attribute");
    }
    let slice = std::slice::from_raw_parts(attr_name_ptr, attr_name_len);
    let attr_name = std::str::from_utf8(slice).unwrap_or("<attr>");
    let type_id = object_type_id(obj_ptr);
    if type_id == TYPE_ID_MODULE {
        let Some(attr_bits) = attr_name_bits_from_bytes(slice) else {
            return MoltObject::none().bits() as i64;
        };
        let module_bits = MoltObject::from_ptr(obj_ptr).bits();
        let res = molt_module_set_attr(module_bits, attr_bits, val_bits);
        dec_ref_bits(attr_bits);
        return res as i64;
    }
    if type_id == TYPE_ID_TYPE {
        let class_bits = MoltObject::from_ptr(obj_ptr).bits();
        if is_builtin_class_bits(class_bits) {
            return raise_exception::<_>("TypeError", "cannot set attributes on builtin type");
        }
        if attr_name == "__annotate__" {
            let val_obj = obj_from_bits(val_bits);
            if !val_obj.is_none() {
                let callable_ok = is_truthy(obj_from_bits(molt_is_callable(val_bits)));
                if !callable_ok {
                    return raise_exception::<_>(
                        "TypeError",
                        "__annotate__ must be callable or None",
                    );
                }
                class_set_annotations_bits(obj_ptr, 0);
            }
            let dict_bits = class_dict_bits(obj_ptr);
            if let Some(dict_ptr) = obj_from_bits(dict_bits).as_ptr() {
                if object_type_id(dict_ptr) == TYPE_ID_DICT {
                    let annotate_bits = intern_static_name(
                        &runtime_state().interned.annotate_name,
                        b"__annotate__",
                    );
                    dict_set_in_place(dict_ptr, annotate_bits, val_bits);
                    if !val_obj.is_none() {
                        let annotations_bits = intern_static_name(
                            &runtime_state().interned.annotations_name,
                            b"__annotations__",
                        );
                        dict_del_in_place(dict_ptr, annotations_bits);
                    }
                }
            }
            class_set_annotate_bits(obj_ptr, val_bits);
            class_bump_layout_version(obj_ptr);
            return MoltObject::none().bits() as i64;
        }
        if attr_name == "__annotations__" {
            let dict_bits = class_dict_bits(obj_ptr);
            if let Some(dict_ptr) = obj_from_bits(dict_bits).as_ptr() {
                if object_type_id(dict_ptr) == TYPE_ID_DICT {
                    let annotations_bits = intern_static_name(
                        &runtime_state().interned.annotations_name,
                        b"__annotations__",
                    );
                    dict_set_in_place(dict_ptr, annotations_bits, val_bits);
                    let annotate_bits = intern_static_name(
                        &runtime_state().interned.annotate_name,
                        b"__annotate__",
                    );
                    let none_bits = MoltObject::none().bits();
                    dict_set_in_place(dict_ptr, annotate_bits, none_bits);
                }
            }
            class_set_annotations_bits(obj_ptr, val_bits);
            class_set_annotate_bits(obj_ptr, MoltObject::none().bits());
            class_bump_layout_version(obj_ptr);
            return MoltObject::none().bits() as i64;
        }
        let Some(attr_bits) = attr_name_bits_from_bytes(slice) else {
            return MoltObject::none().bits() as i64;
        };
        let dict_bits = class_dict_bits(obj_ptr);
        if let Some(dict_ptr) = obj_from_bits(dict_bits).as_ptr() {
            if object_type_id(dict_ptr) == TYPE_ID_DICT {
                dict_set_in_place(dict_ptr, attr_bits, val_bits);
                class_bump_layout_version(obj_ptr);
                dec_ref_bits(attr_bits);
                return MoltObject::none().bits() as i64;
            }
        }
        dec_ref_bits(attr_bits);
        return attr_error("type", attr_name);
    }
    if type_id == TYPE_ID_EXCEPTION {
        let Some(attr_bits) = attr_name_bits_from_bytes(slice) else {
            return MoltObject::none().bits() as i64;
        };
        let name = string_obj_to_owned(obj_from_bits(attr_bits)).unwrap_or_default();
        if name == "__cause__" || name == "__context__" {
            let val_obj = obj_from_bits(val_bits);
            if !val_obj.is_none() {
                let Some(val_ptr) = val_obj.as_ptr() else {
                    return raise_exception::<_>(
                        "TypeError",
                        if name == "__cause__" {
                            "exception cause must be an exception or None"
                        } else {
                            "exception context must be an exception or None"
                        },
                    );
                };
                unsafe {
                    if object_type_id(val_ptr) != TYPE_ID_EXCEPTION {
                        return raise_exception::<_>(
                            "TypeError",
                            if name == "__cause__" {
                                "exception cause must be an exception or None"
                            } else {
                                "exception context must be an exception or None"
                            },
                        );
                    }
                }
            }
            unsafe {
                let slot = if name == "__cause__" {
                    obj_ptr.add(2 * std::mem::size_of::<u64>())
                } else {
                    obj_ptr.add(3 * std::mem::size_of::<u64>())
                } as *mut u64;
                let old_bits = *slot;
                if old_bits != val_bits {
                    dec_ref_bits(old_bits);
                    inc_ref_bits(val_bits);
                    *slot = val_bits;
                }
                if name == "__cause__" {
                    let suppress_bits = MoltObject::from_bool(true).bits();
                    let suppress_slot = obj_ptr.add(4 * std::mem::size_of::<u64>()) as *mut u64;
                    let old_bits = *suppress_slot;
                    if old_bits != suppress_bits {
                        dec_ref_bits(old_bits);
                        inc_ref_bits(suppress_bits);
                        *suppress_slot = suppress_bits;
                    }
                }
            }
            dec_ref_bits(attr_bits);
            return MoltObject::none().bits() as i64;
        }
        if name == "args" {
            let args_bits = exception_args_from_iterable(val_bits);
            if obj_from_bits(args_bits).is_none() {
                dec_ref_bits(attr_bits);
                return MoltObject::none().bits() as i64;
            }
            let msg_bits = exception_message_from_args(args_bits);
            if obj_from_bits(msg_bits).is_none() {
                dec_ref_bits(args_bits);
                dec_ref_bits(attr_bits);
                return MoltObject::none().bits() as i64;
            }
            unsafe {
                exception_store_args_and_message(obj_ptr, args_bits, msg_bits);
                exception_set_stop_iteration_value(obj_ptr, args_bits);
            }
            dec_ref_bits(attr_bits);
            return MoltObject::none().bits() as i64;
        }
        if name == "__suppress_context__" {
            let suppress = is_truthy(obj_from_bits(val_bits));
            let suppress_bits = MoltObject::from_bool(suppress).bits();
            unsafe {
                let slot = obj_ptr.add(4 * std::mem::size_of::<u64>()) as *mut u64;
                let old_bits = *slot;
                if old_bits != suppress_bits {
                    dec_ref_bits(old_bits);
                    inc_ref_bits(suppress_bits);
                    *slot = suppress_bits;
                }
            }
            dec_ref_bits(attr_bits);
            return MoltObject::none().bits() as i64;
        }
        if name == "__dict__" {
            let val_obj = obj_from_bits(val_bits);
            let Some(val_ptr) = val_obj.as_ptr() else {
                let msg = format!(
                    "__dict__ must be set to a dictionary, not a '{}'",
                    type_name(val_obj)
                );
                dec_ref_bits(attr_bits);
                return raise_exception::<_>("TypeError", &msg);
            };
            if object_type_id(val_ptr) != TYPE_ID_DICT {
                let msg = format!(
                    "__dict__ must be set to a dictionary, not a '{}'",
                    type_name(val_obj)
                );
                dec_ref_bits(attr_bits);
                return raise_exception::<_>("TypeError", &msg);
            }
            unsafe {
                let slot = obj_ptr.add(9 * std::mem::size_of::<u64>()) as *mut u64;
                let old_bits = *slot;
                if old_bits != val_bits {
                    dec_ref_bits(old_bits);
                    inc_ref_bits(val_bits);
                    *slot = val_bits;
                }
            }
            dec_ref_bits(attr_bits);
            return MoltObject::none().bits() as i64;
        }
        if name == "value" {
            let kind = string_obj_to_owned(obj_from_bits(exception_kind_bits(obj_ptr)))
                .unwrap_or_default();
            if kind != "StopIteration" {
                dec_ref_bits(attr_bits);
                return attr_error("exception", attr_name);
            }
            unsafe {
                let slot = obj_ptr.add(6 * std::mem::size_of::<u64>()) as *mut u64;
                let old_bits = *slot;
                if old_bits != val_bits {
                    dec_ref_bits(old_bits);
                    inc_ref_bits(val_bits);
                    *slot = val_bits;
                }
            }
            dec_ref_bits(attr_bits);
            return MoltObject::none().bits() as i64;
        }
        let mut dict_bits = exception_dict_bits(obj_ptr);
        if obj_from_bits(dict_bits).is_none() || dict_bits == 0 {
            let dict_ptr = alloc_dict_with_pairs(&[]);
            if !dict_ptr.is_null() {
                dict_bits = MoltObject::from_ptr(dict_ptr).bits();
                let slot = obj_ptr.add(9 * std::mem::size_of::<u64>()) as *mut u64;
                let old_bits = *slot;
                if old_bits != dict_bits {
                    dec_ref_bits(old_bits);
                    *slot = dict_bits;
                }
            }
        }
        if !obj_from_bits(dict_bits).is_none() && dict_bits != 0 {
            if let Some(dict_ptr) = obj_from_bits(dict_bits).as_ptr() {
                if object_type_id(dict_ptr) == TYPE_ID_DICT {
                    dict_set_in_place(dict_ptr, attr_bits, val_bits);
                    dec_ref_bits(attr_bits);
                    return MoltObject::none().bits() as i64;
                }
            }
        }
        dec_ref_bits(attr_bits);
        return attr_error("exception", attr_name);
    }
    if type_id == TYPE_ID_FUNCTION {
        if attr_name == "__code__" {
            let val_obj = obj_from_bits(val_bits);
            let Some(val_ptr) = val_obj.as_ptr() else {
                return raise_exception::<_>(
                    "TypeError",
                    "function __code__ must be a code object",
                );
            };
            unsafe {
                if object_type_id(val_ptr) != TYPE_ID_CODE {
                    return raise_exception::<_>(
                        "TypeError",
                        "function __code__ must be a code object",
                    );
                }
                function_set_code_bits(obj_ptr, val_bits);
            }
            return MoltObject::none().bits() as i64;
        }
        if attr_name == "__annotate__" {
            let val_obj = obj_from_bits(val_bits);
            if !val_obj.is_none() {
                let callable_ok = is_truthy(obj_from_bits(molt_is_callable(val_bits)));
                if !callable_ok {
                    return raise_exception::<_>(
                        "TypeError",
                        "__annotate__ must be callable or None",
                    );
                }
                function_set_annotations_bits(obj_ptr, 0);
            }
            function_set_annotate_bits(obj_ptr, val_bits);
            return MoltObject::none().bits() as i64;
        }
        if attr_name == "__annotations__" {
            let val_obj = obj_from_bits(val_bits);
            let ann_bits = if val_obj.is_none() {
                let dict_ptr = alloc_dict_with_pairs(&[]);
                if dict_ptr.is_null() {
                    return MoltObject::none().bits() as i64;
                }
                MoltObject::from_ptr(dict_ptr).bits()
            } else {
                let Some(val_ptr) = val_obj.as_ptr() else {
                    return raise_exception::<_>(
                        "TypeError",
                        "__annotations__ must be set to a dict object",
                    );
                };
                if object_type_id(val_ptr) != TYPE_ID_DICT {
                    return raise_exception::<_>(
                        "TypeError",
                        "__annotations__ must be set to a dict object",
                    );
                }
                val_bits
            };
            function_set_annotations_bits(obj_ptr, ann_bits);
            function_set_annotate_bits(obj_ptr, MoltObject::none().bits());
            return MoltObject::none().bits() as i64;
        }
        let Some(attr_bits) = attr_name_bits_from_bytes(slice) else {
            return MoltObject::none().bits() as i64;
        };
        let mut dict_bits = function_dict_bits(obj_ptr);
        if dict_bits == 0 {
            let dict_ptr = alloc_dict_with_pairs(&[]);
            if dict_ptr.is_null() {
                dec_ref_bits(attr_bits);
                return MoltObject::none().bits() as i64;
            }
            dict_bits = MoltObject::from_ptr(dict_ptr).bits();
            function_set_dict_bits(obj_ptr, dict_bits);
        }
        if let Some(dict_ptr) = obj_from_bits(dict_bits).as_ptr() {
            if object_type_id(dict_ptr) == TYPE_ID_DICT {
                dict_set_in_place(dict_ptr, attr_bits, val_bits);
                dec_ref_bits(attr_bits);
                return MoltObject::none().bits() as i64;
            }
        }
        dec_ref_bits(attr_bits);
        return attr_error("function", attr_name);
    }
    if type_id == TYPE_ID_CODE {
        return attr_error("code", attr_name);
    }
    if type_id == TYPE_ID_DATACLASS {
        let desc_ptr = dataclass_desc_ptr(obj_ptr);
        if !desc_ptr.is_null() && (*desc_ptr).frozen {
            return raise_exception::<_>("TypeError", "cannot assign to frozen dataclass field");
        }
        let Some(attr_bits) = attr_name_bits_from_bytes(slice) else {
            return MoltObject::none().bits() as i64;
        };
        if !desc_ptr.is_null() {
            let class_bits = (*desc_ptr).class_bits;
            if class_bits != 0 {
                if let Some(class_ptr) = obj_from_bits(class_bits).as_ptr() {
                    if object_type_id(class_ptr) == TYPE_ID_TYPE {
                        let setattr_bits = intern_static_name(
                            &runtime_state().interned.setattr_name,
                            b"__setattr__",
                        );
                        if let Some(call_bits) =
                            class_attr_lookup(class_ptr, class_ptr, Some(obj_ptr), setattr_bits)
                        {
                            let _ = call_callable2(call_bits, attr_bits, val_bits);
                            dec_ref_bits(attr_bits);
                            return MoltObject::none().bits() as i64;
                        }
                        if let Some(desc_bits) = class_attr_lookup_raw_mro(class_ptr, attr_bits) {
                            if descriptor_is_data(desc_bits) {
                                let desc_obj = obj_from_bits(desc_bits);
                                if let Some(desc_ptr) = desc_obj.as_ptr() {
                                    if object_type_id(desc_ptr) == TYPE_ID_PROPERTY {
                                        let set_bits = property_set_bits(desc_ptr);
                                        if obj_from_bits(set_bits).is_none() {
                                            dec_ref_bits(attr_bits);
                                            return property_no_setter(attr_name, class_ptr);
                                        }
                                        let inst_bits = instance_bits_for_call(obj_ptr);
                                        let _ = call_function_obj2(set_bits, inst_bits, val_bits);
                                        dec_ref_bits(attr_bits);
                                        return MoltObject::none().bits() as i64;
                                    }
                                }
                                let set_bits = intern_static_name(
                                    &runtime_state().interned.set_name,
                                    b"__set__",
                                );
                                if let Some(method_bits) =
                                    descriptor_method_bits(desc_bits, set_bits)
                                {
                                    let self_bits = desc_bits;
                                    let inst_bits = instance_bits_for_call(obj_ptr);
                                    let method_obj = obj_from_bits(method_bits);
                                    if let Some(method_ptr) = method_obj.as_ptr() {
                                        if object_type_id(method_ptr) == TYPE_ID_FUNCTION {
                                            let _ = call_function_obj3(
                                                method_bits,
                                                self_bits,
                                                inst_bits,
                                                val_bits,
                                            );
                                        } else {
                                            let _ =
                                                call_callable2(method_bits, inst_bits, val_bits);
                                        }
                                    } else {
                                        let _ = call_callable2(method_bits, inst_bits, val_bits);
                                    }
                                    dec_ref_bits(attr_bits);
                                    return MoltObject::none().bits() as i64;
                                }
                                dec_ref_bits(attr_bits);
                                return descriptor_no_setter(attr_name, class_ptr);
                            }
                        }
                    }
                }
            }
            if (*desc_ptr).slots {
                dec_ref_bits(attr_bits);
                let name = &(*desc_ptr).name;
                let type_label = if name.is_empty() {
                    "dataclass"
                } else {
                    name.as_str()
                };
                return attr_error(type_label, attr_name);
            }
        }
        let mut dict_bits = dataclass_dict_bits(obj_ptr);
        if dict_bits == 0 {
            let dict_ptr = alloc_dict_with_pairs(&[]);
            if dict_ptr.is_null() {
                dec_ref_bits(attr_bits);
                return MoltObject::none().bits() as i64;
            }
            dict_bits = MoltObject::from_ptr(dict_ptr).bits();
            dataclass_set_dict_bits(obj_ptr, dict_bits);
        }
        if let Some(dict_ptr) = obj_from_bits(dict_bits).as_ptr() {
            if object_type_id(dict_ptr) == TYPE_ID_DICT {
                dict_set_in_place(dict_ptr, attr_bits, val_bits);
                dec_ref_bits(attr_bits);
                return MoltObject::none().bits() as i64;
            }
        }
        dec_ref_bits(attr_bits);
        let type_label = if !desc_ptr.is_null() {
            let name = &(*desc_ptr).name;
            if name.is_empty() {
                "dataclass"
            } else {
                name.as_str()
            }
        } else {
            "dataclass"
        };
        return attr_error(type_label, attr_name);
    }
    if type_id == TYPE_ID_OBJECT {
        let header = header_from_obj_ptr(obj_ptr);
        if (*header).poll_fn != 0 {
            return attr_error("object", attr_name);
        }
        let payload = object_payload_size(obj_ptr);
        if payload < std::mem::size_of::<u64>() {
            return attr_error("object", attr_name);
        }
        let Some(attr_bits) = attr_name_bits_from_bytes(slice) else {
            return MoltObject::none().bits() as i64;
        };
        let class_bits = object_class_bits(obj_ptr);
        if class_bits != 0 {
            if let Some(class_ptr) = obj_from_bits(class_bits).as_ptr() {
                if object_type_id(class_ptr) == TYPE_ID_TYPE {
                    let setattr_bits =
                        intern_static_name(&runtime_state().interned.setattr_name, b"__setattr__");
                    if let Some(call_bits) =
                        class_attr_lookup(class_ptr, class_ptr, Some(obj_ptr), setattr_bits)
                    {
                        let _ = call_callable2(call_bits, attr_bits, val_bits);
                        dec_ref_bits(attr_bits);
                        return MoltObject::none().bits() as i64;
                    }
                    if let Some(desc_bits) = class_attr_lookup_raw_mro(class_ptr, attr_bits) {
                        if descriptor_is_data(desc_bits) {
                            let desc_obj = obj_from_bits(desc_bits);
                            if let Some(desc_ptr) = desc_obj.as_ptr() {
                                if object_type_id(desc_ptr) == TYPE_ID_PROPERTY {
                                    let set_bits = property_set_bits(desc_ptr);
                                    if obj_from_bits(set_bits).is_none() {
                                        dec_ref_bits(attr_bits);
                                        return property_no_setter(attr_name, class_ptr);
                                    }
                                    let inst_bits = instance_bits_for_call(obj_ptr);
                                    let _ = call_function_obj2(set_bits, inst_bits, val_bits);
                                    dec_ref_bits(attr_bits);
                                    return MoltObject::none().bits() as i64;
                                }
                            }
                            let set_bits =
                                intern_static_name(&runtime_state().interned.set_name, b"__set__");
                            if let Some(method_bits) = descriptor_method_bits(desc_bits, set_bits) {
                                let self_bits = desc_bits;
                                let inst_bits = instance_bits_for_call(obj_ptr);
                                let method_obj = obj_from_bits(method_bits);
                                if let Some(method_ptr) = method_obj.as_ptr() {
                                    if object_type_id(method_ptr) == TYPE_ID_FUNCTION {
                                        let _ = call_function_obj3(
                                            method_bits,
                                            self_bits,
                                            inst_bits,
                                            val_bits,
                                        );
                                    } else {
                                        let _ = call_callable2(method_bits, inst_bits, val_bits);
                                    }
                                } else {
                                    let _ = call_callable2(method_bits, inst_bits, val_bits);
                                }
                                dec_ref_bits(attr_bits);
                                return MoltObject::none().bits() as i64;
                            }
                            dec_ref_bits(attr_bits);
                            return descriptor_no_setter(attr_name, class_ptr);
                        }
                    }
                    if let Some(offset) = class_field_offset(class_ptr, attr_bits) {
                        dec_ref_bits(attr_bits);
                        return object_field_set_ptr_raw(obj_ptr, offset, val_bits) as i64;
                    }
                }
            }
        }
        let mut dict_bits = instance_dict_bits(obj_ptr);
        if dict_bits == 0 {
            let dict_ptr = alloc_dict_with_pairs(&[]);
            if dict_ptr.is_null() {
                dec_ref_bits(attr_bits);
                return MoltObject::none().bits() as i64;
            }
            dict_bits = MoltObject::from_ptr(dict_ptr).bits();
            instance_set_dict_bits(obj_ptr, dict_bits);
        }
        if let Some(dict_ptr) = obj_from_bits(dict_bits).as_ptr() {
            if object_type_id(dict_ptr) == TYPE_ID_DICT {
                dict_set_in_place(dict_ptr, attr_bits, val_bits);
                dec_ref_bits(attr_bits);
                return MoltObject::none().bits() as i64;
            }
        }
        dec_ref_bits(attr_bits);
        return attr_error("object", attr_name);
    }
    attr_error(type_name(MoltObject::from_ptr(obj_ptr)), attr_name)
}

unsafe fn del_attr_ptr(obj_ptr: *mut u8, attr_bits: u64, attr_name: &str) -> i64 {
    let type_id = object_type_id(obj_ptr);
    if type_id == TYPE_ID_MODULE {
        let dict_bits = module_dict_bits(obj_ptr);
        if let Some(dict_ptr) = obj_from_bits(dict_bits).as_ptr() {
            if object_type_id(dict_ptr) == TYPE_ID_DICT {
                let annotations_bits = intern_static_name(
                    &runtime_state().interned.annotations_name,
                    b"__annotations__",
                );
                if obj_eq(obj_from_bits(attr_bits), obj_from_bits(annotations_bits)) {
                    if dict_del_in_place(dict_ptr, annotations_bits) {
                        let annotate_bits = intern_static_name(
                            &runtime_state().interned.annotate_name,
                            b"__annotate__",
                        );
                        let none_bits = MoltObject::none().bits();
                        dict_set_in_place(dict_ptr, annotate_bits, none_bits);
                        return MoltObject::none().bits() as i64;
                    }
                    let module_name = string_obj_to_owned(obj_from_bits(module_name_bits(obj_ptr)))
                        .unwrap_or_default();
                    let msg = format!("module '{module_name}' has no attribute '{attr_name}'");
                    return raise_exception::<_>("AttributeError", &msg);
                }
                let annotate_bits =
                    intern_static_name(&runtime_state().interned.annotate_name, b"__annotate__");
                if obj_eq(obj_from_bits(attr_bits), obj_from_bits(annotate_bits)) {
                    return raise_exception::<_>(
                        "TypeError",
                        "cannot delete __annotate__ attribute",
                    );
                }
                if dict_del_in_place(dict_ptr, attr_bits) {
                    return MoltObject::none().bits() as i64;
                }
            }
        }
        let module_name =
            string_obj_to_owned(obj_from_bits(module_name_bits(obj_ptr))).unwrap_or_default();
        let msg = format!("module '{module_name}' has no attribute '{attr_name}'");
        return raise_exception::<_>("AttributeError", &msg);
    }
    if type_id == TYPE_ID_TYPE {
        let class_bits = MoltObject::from_ptr(obj_ptr).bits();
        if is_builtin_class_bits(class_bits) {
            return raise_exception::<_>("TypeError", "cannot delete attributes on builtin type");
        }
        if attr_name == "__annotate__" {
            return raise_exception::<_>("TypeError", "cannot delete __annotate__ attribute");
        }
        if attr_name == "__annotations__" {
            let dict_bits = class_dict_bits(obj_ptr);
            let mut removed = false;
            if let Some(dict_ptr) = obj_from_bits(dict_bits).as_ptr() {
                if object_type_id(dict_ptr) == TYPE_ID_DICT {
                    let annotations_bits = intern_static_name(
                        &runtime_state().interned.annotations_name,
                        b"__annotations__",
                    );
                    if dict_del_in_place(dict_ptr, annotations_bits) {
                        removed = true;
                    }
                    if removed {
                        let annotate_bits = intern_static_name(
                            &runtime_state().interned.annotate_name,
                            b"__annotate__",
                        );
                        let none_bits = MoltObject::none().bits();
                        dict_set_in_place(dict_ptr, annotate_bits, none_bits);
                    }
                }
            }
            if !removed && class_annotations_bits(obj_ptr) != 0 {
                removed = true;
            }
            if removed {
                class_set_annotations_bits(obj_ptr, 0);
                class_set_annotate_bits(obj_ptr, MoltObject::none().bits());
                class_bump_layout_version(obj_ptr);
                return MoltObject::none().bits() as i64;
            }
            let class_name =
                string_obj_to_owned(obj_from_bits(class_name_bits(obj_ptr))).unwrap_or_default();
            let msg = format!("type object '{class_name}' has no attribute '{attr_name}'");
            return raise_exception::<_>("AttributeError", &msg);
        }
        let dict_bits = class_dict_bits(obj_ptr);
        if let Some(dict_ptr) = obj_from_bits(dict_bits).as_ptr() {
            if object_type_id(dict_ptr) == TYPE_ID_DICT && dict_del_in_place(dict_ptr, attr_bits) {
                class_bump_layout_version(obj_ptr);
                return MoltObject::none().bits() as i64;
            }
        }
        let class_name =
            string_obj_to_owned(obj_from_bits(class_name_bits(obj_ptr))).unwrap_or_default();
        let msg = format!("type object '{class_name}' has no attribute '{attr_name}'");
        return raise_exception::<_>("AttributeError", &msg);
    }
    if type_id == TYPE_ID_EXCEPTION {
        if attr_name == "__cause__" || attr_name == "__context__" {
            unsafe {
                let slot = if attr_name == "__cause__" {
                    obj_ptr.add(2 * std::mem::size_of::<u64>())
                } else {
                    obj_ptr.add(3 * std::mem::size_of::<u64>())
                } as *mut u64;
                let old_bits = *slot;
                if !obj_from_bits(old_bits).is_none() {
                    dec_ref_bits(old_bits);
                    let none_bits = MoltObject::none().bits();
                    inc_ref_bits(none_bits);
                    *slot = none_bits;
                }
                if attr_name == "__cause__" {
                    let suppress_bits = MoltObject::from_bool(false).bits();
                    let suppress_slot = obj_ptr.add(4 * std::mem::size_of::<u64>()) as *mut u64;
                    let old_bits = *suppress_slot;
                    if old_bits != suppress_bits {
                        dec_ref_bits(old_bits);
                        inc_ref_bits(suppress_bits);
                        *suppress_slot = suppress_bits;
                    }
                }
            }
            return MoltObject::none().bits() as i64;
        }
        if attr_name == "__suppress_context__" {
            unsafe {
                let suppress_bits = MoltObject::from_bool(false).bits();
                let slot = obj_ptr.add(4 * std::mem::size_of::<u64>()) as *mut u64;
                let old_bits = *slot;
                if old_bits != suppress_bits {
                    dec_ref_bits(old_bits);
                    inc_ref_bits(suppress_bits);
                    *slot = suppress_bits;
                }
            }
            return MoltObject::none().bits() as i64;
        }
        let dict_bits = exception_dict_bits(obj_ptr);
        if !obj_from_bits(dict_bits).is_none() && dict_bits != 0 {
            if let Some(dict_ptr) = obj_from_bits(dict_bits).as_ptr() {
                if object_type_id(dict_ptr) == TYPE_ID_DICT
                    && dict_del_in_place(dict_ptr, attr_bits)
                {
                    return MoltObject::none().bits() as i64;
                }
            }
        }
        return attr_error("exception", attr_name);
    }
    if type_id == TYPE_ID_FUNCTION {
        if attr_name == "__annotate__" {
            return raise_exception::<_>("TypeError", "cannot delete __annotate__ attribute");
        }
        if attr_name == "__annotations__" {
            function_set_annotations_bits(obj_ptr, 0);
            function_set_annotate_bits(obj_ptr, MoltObject::none().bits());
            return MoltObject::none().bits() as i64;
        }
        let dict_bits = function_dict_bits(obj_ptr);
        if dict_bits != 0 {
            if let Some(dict_ptr) = obj_from_bits(dict_bits).as_ptr() {
                if object_type_id(dict_ptr) == TYPE_ID_DICT
                    && dict_del_in_place(dict_ptr, attr_bits)
                {
                    return MoltObject::none().bits() as i64;
                }
            }
        }
        return attr_error("function", attr_name);
    }
    if type_id == TYPE_ID_DATACLASS {
        let desc_ptr = dataclass_desc_ptr(obj_ptr);
        if !desc_ptr.is_null() {
            let class_bits = (*desc_ptr).class_bits;
            if class_bits != 0 {
                if let Some(class_ptr) = obj_from_bits(class_bits).as_ptr() {
                    if object_type_id(class_ptr) == TYPE_ID_TYPE {
                        let delattr_bits = intern_static_name(
                            &runtime_state().interned.delattr_name,
                            b"__delattr__",
                        );
                        if let Some(call_bits) =
                            class_attr_lookup(class_ptr, class_ptr, Some(obj_ptr), delattr_bits)
                        {
                            let _ = call_callable1(call_bits, attr_bits);
                            return MoltObject::none().bits() as i64;
                        }
                        if let Some(desc_bits) = class_attr_lookup_raw_mro(class_ptr, attr_bits) {
                            if let Some(desc_ptr) = maybe_ptr_from_bits(desc_bits) {
                                if object_type_id(desc_ptr) == TYPE_ID_PROPERTY {
                                    let del_bits = property_del_bits(desc_ptr);
                                    if obj_from_bits(del_bits).is_none() {
                                        return property_no_deleter(attr_name, class_ptr);
                                    }
                                    let inst_bits = instance_bits_for_call(obj_ptr);
                                    let _ = call_function_obj1(del_bits, inst_bits);
                                    return MoltObject::none().bits() as i64;
                                }
                            }
                            let del_bits = intern_static_name(
                                &runtime_state().interned.delete_name,
                                b"__delete__",
                            );
                            if let Some(method_bits) = descriptor_method_bits(desc_bits, del_bits) {
                                let self_bits = desc_bits;
                                let inst_bits = instance_bits_for_call(obj_ptr);
                                let method_obj = obj_from_bits(method_bits);
                                if let Some(method_ptr) = method_obj.as_ptr() {
                                    if object_type_id(method_ptr) == TYPE_ID_FUNCTION {
                                        let _ =
                                            call_function_obj2(method_bits, self_bits, inst_bits);
                                    } else {
                                        let _ = call_callable1(method_bits, inst_bits);
                                    }
                                } else {
                                    let _ = call_callable1(method_bits, inst_bits);
                                }
                                return MoltObject::none().bits() as i64;
                            }
                            let set_bits =
                                intern_static_name(&runtime_state().interned.set_name, b"__set__");
                            if descriptor_method_bits(desc_bits, set_bits).is_some() {
                                return descriptor_no_deleter(attr_name, class_ptr);
                            }
                        }
                    }
                }
            }
            if (*desc_ptr).frozen {
                return raise_exception::<_>("TypeError", "cannot delete frozen dataclass field");
            }
            if (*desc_ptr).slots {
                let name = &(*desc_ptr).name;
                let type_label = if name.is_empty() {
                    "dataclass"
                } else {
                    name.as_str()
                };
                return attr_error(type_label, attr_name);
            }
        }
        let dict_bits = dataclass_dict_bits(obj_ptr);
        if dict_bits != 0 {
            if let Some(dict_ptr) = obj_from_bits(dict_bits).as_ptr() {
                if object_type_id(dict_ptr) == TYPE_ID_DICT
                    && dict_del_in_place(dict_ptr, attr_bits)
                {
                    return MoltObject::none().bits() as i64;
                }
            }
        }
        let type_label = if !desc_ptr.is_null() {
            let name = &(*desc_ptr).name;
            if name.is_empty() {
                "dataclass"
            } else {
                name.as_str()
            }
        } else {
            "dataclass"
        };
        return attr_error(type_label, attr_name);
    }
    if type_id == TYPE_ID_OBJECT {
        let header = header_from_obj_ptr(obj_ptr);
        if (*header).poll_fn != 0 {
            return attr_error("object", attr_name);
        }
        let payload = object_payload_size(obj_ptr);
        if payload < std::mem::size_of::<u64>() {
            return attr_error("object", attr_name);
        }
        let class_bits = object_class_bits(obj_ptr);
        if class_bits != 0 {
            if let Some(class_ptr) = obj_from_bits(class_bits).as_ptr() {
                if object_type_id(class_ptr) == TYPE_ID_TYPE {
                    let delattr_bits =
                        intern_static_name(&runtime_state().interned.delattr_name, b"__delattr__");
                    if let Some(call_bits) =
                        class_attr_lookup(class_ptr, class_ptr, Some(obj_ptr), delattr_bits)
                    {
                        let _ = call_callable1(call_bits, attr_bits);
                        return MoltObject::none().bits() as i64;
                    }
                    if let Some(desc_bits) = class_attr_lookup_raw_mro(class_ptr, attr_bits) {
                        if let Some(desc_ptr) = maybe_ptr_from_bits(desc_bits) {
                            if object_type_id(desc_ptr) == TYPE_ID_PROPERTY {
                                let del_bits = property_del_bits(desc_ptr);
                                if obj_from_bits(del_bits).is_none() {
                                    return property_no_deleter(attr_name, class_ptr);
                                }
                                let inst_bits = instance_bits_for_call(obj_ptr);
                                let _ = call_function_obj1(del_bits, inst_bits);
                                return MoltObject::none().bits() as i64;
                            }
                        }
                        let del_bits = intern_static_name(
                            &runtime_state().interned.delete_name,
                            b"__delete__",
                        );
                        if let Some(method_bits) = descriptor_method_bits(desc_bits, del_bits) {
                            let self_bits = desc_bits;
                            let inst_bits = instance_bits_for_call(obj_ptr);
                            let method_obj = obj_from_bits(method_bits);
                            if let Some(method_ptr) = method_obj.as_ptr() {
                                if object_type_id(method_ptr) == TYPE_ID_FUNCTION {
                                    let _ = call_function_obj2(method_bits, self_bits, inst_bits);
                                } else {
                                    let _ = call_callable1(method_bits, inst_bits);
                                }
                            } else {
                                let _ = call_callable1(method_bits, inst_bits);
                            }
                            return MoltObject::none().bits() as i64;
                        }
                        let set_bits =
                            intern_static_name(&runtime_state().interned.set_name, b"__set__");
                        if descriptor_method_bits(desc_bits, set_bits).is_some() {
                            return descriptor_no_deleter(attr_name, class_ptr);
                        }
                    }
                }
            }
        }
        let dict_bits = instance_dict_bits(obj_ptr);
        if dict_bits != 0 {
            if let Some(dict_ptr) = obj_from_bits(dict_bits).as_ptr() {
                if object_type_id(dict_ptr) == TYPE_ID_DICT
                    && dict_del_in_place(dict_ptr, attr_bits)
                {
                    return MoltObject::none().bits() as i64;
                }
            }
        }
        return attr_error("object", attr_name);
    }
    attr_error(type_name(MoltObject::from_ptr(obj_ptr)), attr_name)
}

unsafe fn object_setattr_raw(
    obj_ptr: *mut u8,
    attr_bits: u64,
    attr_name: &str,
    val_bits: u64,
) -> i64 {
    let header = header_from_obj_ptr(obj_ptr);
    if (*header).poll_fn != 0 {
        return attr_error("object", attr_name);
    }
    let payload = object_payload_size(obj_ptr);
    if payload < std::mem::size_of::<u64>() {
        return attr_error("object", attr_name);
    }
    let class_bits = object_class_bits(obj_ptr);
    if class_bits != 0 {
        if let Some(class_ptr) = obj_from_bits(class_bits).as_ptr() {
            if object_type_id(class_ptr) == TYPE_ID_TYPE {
                if let Some(desc_bits) = class_attr_lookup_raw_mro(class_ptr, attr_bits) {
                    if descriptor_is_data(desc_bits) {
                        let desc_obj = obj_from_bits(desc_bits);
                        if let Some(desc_ptr) = desc_obj.as_ptr() {
                            if object_type_id(desc_ptr) == TYPE_ID_PROPERTY {
                                let set_bits = property_set_bits(desc_ptr);
                                if obj_from_bits(set_bits).is_none() {
                                    return property_no_setter(attr_name, class_ptr);
                                }
                                let inst_bits = instance_bits_for_call(obj_ptr);
                                let _ = call_function_obj2(set_bits, inst_bits, val_bits);
                                return MoltObject::none().bits() as i64;
                            }
                        }
                        let set_bits =
                            intern_static_name(&runtime_state().interned.set_name, b"__set__");
                        if let Some(method_bits) = descriptor_method_bits(desc_bits, set_bits) {
                            let inst_bits = instance_bits_for_call(obj_ptr);
                            let method_obj = obj_from_bits(method_bits);
                            if let Some(method_ptr) = method_obj.as_ptr() {
                                if object_type_id(method_ptr) == TYPE_ID_FUNCTION {
                                    let _ = call_function_obj3(
                                        method_bits,
                                        desc_bits,
                                        inst_bits,
                                        val_bits,
                                    );
                                } else {
                                    let _ = call_callable2(method_bits, inst_bits, val_bits);
                                }
                            } else {
                                let _ = call_callable2(method_bits, inst_bits, val_bits);
                            }
                            return MoltObject::none().bits() as i64;
                        }
                        return descriptor_no_setter(attr_name, class_ptr);
                    }
                }
                if let Some(offset) = class_field_offset(class_ptr, attr_bits) {
                    return object_field_set_ptr_raw(obj_ptr, offset, val_bits) as i64;
                }
            }
        }
    }
    let mut dict_bits = instance_dict_bits(obj_ptr);
    if dict_bits == 0 {
        let dict_ptr = alloc_dict_with_pairs(&[]);
        if dict_ptr.is_null() {
            return MoltObject::none().bits() as i64;
        }
        dict_bits = MoltObject::from_ptr(dict_ptr).bits();
        instance_set_dict_bits(obj_ptr, dict_bits);
    }
    if let Some(dict_ptr) = obj_from_bits(dict_bits).as_ptr() {
        if object_type_id(dict_ptr) == TYPE_ID_DICT {
            dict_set_in_place(dict_ptr, attr_bits, val_bits);
            return MoltObject::none().bits() as i64;
        }
    }
    attr_error("object", attr_name)
}

unsafe fn dataclass_setattr_raw(
    obj_ptr: *mut u8,
    attr_bits: u64,
    attr_name: &str,
    val_bits: u64,
) -> i64 {
    let desc_ptr = dataclass_desc_ptr(obj_ptr);
    if !desc_ptr.is_null() && (*desc_ptr).frozen {
        return raise_exception::<_>("TypeError", "cannot assign to frozen dataclass field");
    }
    if !desc_ptr.is_null() {
        let class_bits = (*desc_ptr).class_bits;
        if class_bits != 0 {
            if let Some(class_ptr) = obj_from_bits(class_bits).as_ptr() {
                if object_type_id(class_ptr) == TYPE_ID_TYPE {
                    if let Some(desc_bits) = class_attr_lookup_raw_mro(class_ptr, attr_bits) {
                        if descriptor_is_data(desc_bits) {
                            let desc_obj = obj_from_bits(desc_bits);
                            if let Some(desc_ptr) = desc_obj.as_ptr() {
                                if object_type_id(desc_ptr) == TYPE_ID_PROPERTY {
                                    let set_bits = property_set_bits(desc_ptr);
                                    if obj_from_bits(set_bits).is_none() {
                                        return property_no_setter(attr_name, class_ptr);
                                    }
                                    let inst_bits = instance_bits_for_call(obj_ptr);
                                    let _ = call_function_obj2(set_bits, inst_bits, val_bits);
                                    return MoltObject::none().bits() as i64;
                                }
                            }
                            let set_bits =
                                intern_static_name(&runtime_state().interned.set_name, b"__set__");
                            if let Some(method_bits) = descriptor_method_bits(desc_bits, set_bits) {
                                let inst_bits = instance_bits_for_call(obj_ptr);
                                let method_obj = obj_from_bits(method_bits);
                                if let Some(method_ptr) = method_obj.as_ptr() {
                                    if object_type_id(method_ptr) == TYPE_ID_FUNCTION {
                                        let _ = call_function_obj3(
                                            method_bits,
                                            desc_bits,
                                            inst_bits,
                                            val_bits,
                                        );
                                    } else {
                                        let _ = call_callable2(method_bits, inst_bits, val_bits);
                                    }
                                } else {
                                    let _ = call_callable2(method_bits, inst_bits, val_bits);
                                }
                                return MoltObject::none().bits() as i64;
                            }
                            return descriptor_no_setter(attr_name, class_ptr);
                        }
                    }
                }
            }
        }
        if (*desc_ptr).slots {
            let name = &(*desc_ptr).name;
            let type_label = if name.is_empty() {
                "dataclass"
            } else {
                name.as_str()
            };
            return attr_error(type_label, attr_name);
        }
    }
    let mut dict_bits = dataclass_dict_bits(obj_ptr);
    if dict_bits == 0 {
        let dict_ptr = alloc_dict_with_pairs(&[]);
        if dict_ptr.is_null() {
            return MoltObject::none().bits() as i64;
        }
        dict_bits = MoltObject::from_ptr(dict_ptr).bits();
        dataclass_set_dict_bits(obj_ptr, dict_bits);
    }
    if let Some(dict_ptr) = obj_from_bits(dict_bits).as_ptr() {
        if object_type_id(dict_ptr) == TYPE_ID_DICT {
            dict_set_in_place(dict_ptr, attr_bits, val_bits);
            return MoltObject::none().bits() as i64;
        }
    }
    let type_label = if !desc_ptr.is_null() {
        let name = &(*desc_ptr).name;
        if name.is_empty() {
            "dataclass"
        } else {
            name.as_str()
        }
    } else {
        "dataclass"
    };
    attr_error(type_label, attr_name)
}

unsafe fn object_delattr_raw(obj_ptr: *mut u8, attr_bits: u64, attr_name: &str) -> i64 {
    let header = header_from_obj_ptr(obj_ptr);
    if (*header).poll_fn != 0 {
        return attr_error("object", attr_name);
    }
    let payload = object_payload_size(obj_ptr);
    if payload < std::mem::size_of::<u64>() {
        return attr_error("object", attr_name);
    }
    let class_bits = object_class_bits(obj_ptr);
    if class_bits != 0 {
        if let Some(class_ptr) = obj_from_bits(class_bits).as_ptr() {
            if object_type_id(class_ptr) == TYPE_ID_TYPE {
                if let Some(desc_bits) = class_attr_lookup_raw_mro(class_ptr, attr_bits) {
                    if let Some(desc_ptr) = maybe_ptr_from_bits(desc_bits) {
                        if object_type_id(desc_ptr) == TYPE_ID_PROPERTY {
                            let del_bits = property_del_bits(desc_ptr);
                            if obj_from_bits(del_bits).is_none() {
                                return property_no_deleter(attr_name, class_ptr);
                            }
                            let inst_bits = instance_bits_for_call(obj_ptr);
                            let _ = call_function_obj1(del_bits, inst_bits);
                            return MoltObject::none().bits() as i64;
                        }
                    }
                    let del_bits =
                        intern_static_name(&runtime_state().interned.delete_name, b"__delete__");
                    if let Some(method_bits) = descriptor_method_bits(desc_bits, del_bits) {
                        let inst_bits = instance_bits_for_call(obj_ptr);
                        let method_obj = obj_from_bits(method_bits);
                        if let Some(method_ptr) = method_obj.as_ptr() {
                            if object_type_id(method_ptr) == TYPE_ID_FUNCTION {
                                let _ = call_function_obj2(method_bits, desc_bits, inst_bits);
                            } else {
                                let _ = call_callable1(method_bits, inst_bits);
                            }
                        } else {
                            let _ = call_callable1(method_bits, inst_bits);
                        }
                        return MoltObject::none().bits() as i64;
                    }
                    let set_bits =
                        intern_static_name(&runtime_state().interned.set_name, b"__set__");
                    if descriptor_method_bits(desc_bits, set_bits).is_some() {
                        return descriptor_no_deleter(attr_name, class_ptr);
                    }
                }
            }
        }
    }
    let dict_bits = instance_dict_bits(obj_ptr);
    if dict_bits != 0 {
        if let Some(dict_ptr) = obj_from_bits(dict_bits).as_ptr() {
            if object_type_id(dict_ptr) == TYPE_ID_DICT && dict_del_in_place(dict_ptr, attr_bits) {
                return MoltObject::none().bits() as i64;
            }
        }
    }
    attr_error("object", attr_name)
}

unsafe fn dataclass_delattr_raw(obj_ptr: *mut u8, attr_bits: u64, attr_name: &str) -> i64 {
    let desc_ptr = dataclass_desc_ptr(obj_ptr);
    if !desc_ptr.is_null() {
        let class_bits = (*desc_ptr).class_bits;
        if class_bits != 0 {
            if let Some(class_ptr) = obj_from_bits(class_bits).as_ptr() {
                if object_type_id(class_ptr) == TYPE_ID_TYPE {
                    if let Some(desc_bits) = class_attr_lookup_raw_mro(class_ptr, attr_bits) {
                        if let Some(desc_ptr) = maybe_ptr_from_bits(desc_bits) {
                            if object_type_id(desc_ptr) == TYPE_ID_PROPERTY {
                                let del_bits = property_del_bits(desc_ptr);
                                if obj_from_bits(del_bits).is_none() {
                                    return property_no_deleter(attr_name, class_ptr);
                                }
                                let inst_bits = instance_bits_for_call(obj_ptr);
                                let _ = call_function_obj1(del_bits, inst_bits);
                                return MoltObject::none().bits() as i64;
                            }
                        }
                        let del_bits = intern_static_name(
                            &runtime_state().interned.delete_name,
                            b"__delete__",
                        );
                        if let Some(method_bits) = descriptor_method_bits(desc_bits, del_bits) {
                            let inst_bits = instance_bits_for_call(obj_ptr);
                            let method_obj = obj_from_bits(method_bits);
                            if let Some(method_ptr) = method_obj.as_ptr() {
                                if object_type_id(method_ptr) == TYPE_ID_FUNCTION {
                                    let _ = call_function_obj2(method_bits, desc_bits, inst_bits);
                                } else {
                                    let _ = call_callable1(method_bits, inst_bits);
                                }
                            } else {
                                let _ = call_callable1(method_bits, inst_bits);
                            }
                            return MoltObject::none().bits() as i64;
                        }
                        let set_bits =
                            intern_static_name(&runtime_state().interned.set_name, b"__set__");
                        if descriptor_method_bits(desc_bits, set_bits).is_some() {
                            return descriptor_no_deleter(attr_name, class_ptr);
                        }
                    }
                }
            }
        }
        if (*desc_ptr).frozen {
            return raise_exception::<_>("TypeError", "cannot delete frozen dataclass field");
        }
        if (*desc_ptr).slots {
            let name = &(*desc_ptr).name;
            let type_label = if name.is_empty() {
                "dataclass"
            } else {
                name.as_str()
            };
            return attr_error(type_label, attr_name);
        }
    }
    let dict_bits = dataclass_dict_bits(obj_ptr);
    if dict_bits != 0 {
        if let Some(dict_ptr) = obj_from_bits(dict_bits).as_ptr() {
            if object_type_id(dict_ptr) == TYPE_ID_DICT && dict_del_in_place(dict_ptr, attr_bits) {
                return MoltObject::none().bits() as i64;
            }
        }
    }
    let type_label = if !desc_ptr.is_null() {
        let name = &(*desc_ptr).name;
        if name.is_empty() {
            "dataclass"
        } else {
            name.as_str()
        }
    } else {
        "dataclass"
    };
    attr_error(type_label, attr_name)
}

/// # Safety
/// Dereferences raw pointers. Caller must ensure attr_name_ptr is valid UTF-8.
#[no_mangle]
pub unsafe extern "C" fn molt_set_attr_ptr(
    obj_ptr: *mut u8,
    attr_name_ptr: *const u8,
    attr_name_len_bits: u64,
    val_bits: u64,
) -> i64 {
    molt_set_attr_generic(obj_ptr, attr_name_ptr, attr_name_len_bits, val_bits)
}

/// # Safety
/// Dereferences raw pointers. Caller must ensure attr_name_ptr is valid UTF-8.
#[no_mangle]
pub unsafe extern "C" fn molt_del_attr_generic(
    obj_ptr: *mut u8,
    attr_name_ptr: *const u8,
    attr_name_len_bits: u64,
) -> i64 {
    let attr_name_len = usize_from_bits(attr_name_len_bits);
    if obj_ptr.is_null() {
        return raise_exception::<_>("AttributeError", "object has no attribute");
    }
    let slice = std::slice::from_raw_parts(attr_name_ptr, attr_name_len);
    let attr_name = std::str::from_utf8(slice).unwrap_or("<attr>");
    let Some(attr_bits) = attr_name_bits_from_bytes(slice) else {
        return MoltObject::none().bits() as i64;
    };
    let res = del_attr_ptr(obj_ptr, attr_bits, attr_name);
    dec_ref_bits(attr_bits);
    res
}

/// # Safety
/// Dereferences raw pointers. Caller must ensure attr_name_ptr is valid UTF-8.
#[no_mangle]
pub unsafe extern "C" fn molt_del_attr_ptr(
    obj_ptr: *mut u8,
    attr_name_ptr: *const u8,
    attr_name_len_bits: u64,
) -> i64 {
    molt_del_attr_generic(obj_ptr, attr_name_ptr, attr_name_len_bits)
}

/// # Safety
/// Dereferences raw pointers. Caller must ensure attr_name_ptr is valid UTF-8.
#[no_mangle]
pub unsafe extern "C" fn molt_get_attr_object(
    obj_bits: u64,
    attr_name_ptr: *const u8,
    attr_name_len_bits: u64,
) -> i64 {
    let attr_name_len = usize_from_bits(attr_name_len_bits);
    if let Some(ptr) = maybe_ptr_from_bits(obj_bits) {
        return molt_get_attr_generic(ptr, attr_name_ptr, attr_name_len_bits);
    }
    let obj = obj_from_bits(obj_bits);
    let slice = std::slice::from_raw_parts(attr_name_ptr, attr_name_len);
    let attr_name = std::str::from_utf8(slice).unwrap_or("<attr>");
    attr_error(type_name(obj), attr_name)
}

/// # Safety
/// Dereferences raw pointers. Caller must ensure attr_name_ptr is valid UTF-8.
#[no_mangle]
pub unsafe extern "C" fn molt_get_attr_special(
    obj_bits: u64,
    attr_name_ptr: *const u8,
    attr_name_len_bits: u64,
) -> i64 {
    let attr_name_len = usize_from_bits(attr_name_len_bits);
    let obj = obj_from_bits(obj_bits);
    let slice = std::slice::from_raw_parts(attr_name_ptr, attr_name_len);
    let attr_name = std::str::from_utf8(slice).unwrap_or("<attr>");
    let Some(obj_ptr) = maybe_ptr_from_bits(obj_bits) else {
        return attr_error(type_name(obj), attr_name);
    };
    let name_ptr = alloc_string(slice);
    if name_ptr.is_null() {
        return MoltObject::none().bits() as i64;
    }
    let name_bits = MoltObject::from_ptr(name_ptr).bits();
    let class_bits = object_class_bits(obj_ptr);
    let class_ptr = obj_from_bits(class_bits).as_ptr();
    let res = if let Some(class_ptr) = class_ptr {
        if object_type_id(class_ptr) == TYPE_ID_TYPE {
            class_attr_lookup(class_ptr, class_ptr, Some(obj_ptr), name_bits)
        } else {
            None
        }
    } else {
        None
    };
    dec_ref_bits(name_bits);
    if let Some(bits) = res {
        return bits as i64;
    }
    attr_error(type_name(obj), attr_name)
}

/// # Safety
/// Dereferences raw pointers. Caller must ensure attr_name_ptr is valid UTF-8.
#[no_mangle]
pub unsafe extern "C" fn molt_set_attr_object(
    obj_bits: u64,
    attr_name_ptr: *const u8,
    attr_name_len_bits: u64,
    val_bits: u64,
) -> i64 {
    let attr_name_len = usize_from_bits(attr_name_len_bits);
    if let Some(ptr) = maybe_ptr_from_bits(obj_bits) {
        return molt_set_attr_generic(ptr, attr_name_ptr, attr_name_len_bits, val_bits);
    }
    let obj = obj_from_bits(obj_bits);
    let slice = std::slice::from_raw_parts(attr_name_ptr, attr_name_len);
    let attr_name = std::str::from_utf8(slice).unwrap_or("<attr>");
    attr_error(type_name(obj), attr_name)
}

/// # Safety
/// Dereferences raw pointers. Caller must ensure attr_name_ptr is valid UTF-8.
#[no_mangle]
pub unsafe extern "C" fn molt_del_attr_object(
    obj_bits: u64,
    attr_name_ptr: *const u8,
    attr_name_len_bits: u64,
) -> i64 {
    let attr_name_len = usize_from_bits(attr_name_len_bits);
    if let Some(ptr) = maybe_ptr_from_bits(obj_bits) {
        return molt_del_attr_generic(ptr, attr_name_ptr, attr_name_len_bits);
    }
    let obj = obj_from_bits(obj_bits);
    let slice = std::slice::from_raw_parts(attr_name_ptr, attr_name_len);
    let attr_name = std::str::from_utf8(slice).unwrap_or("<attr>");
    attr_error(type_name(obj), attr_name)
}

#[no_mangle]
pub extern "C" fn molt_get_attr_name(obj_bits: u64, name_bits: u64) -> u64 {
    let name_obj = obj_from_bits(name_bits);
    let Some(name_ptr) = name_obj.as_ptr() else {
        return raise_attr_name_type_error(name_bits);
    };
    unsafe {
        if object_type_id(name_ptr) != TYPE_ID_STRING {
            return raise_attr_name_type_error(name_bits);
        }
        let attr_name =
            string_obj_to_owned(obj_from_bits(name_bits)).unwrap_or_else(|| "<attr>".to_string());
        if let Some(obj_ptr) = maybe_ptr_from_bits(obj_bits) {
            if let Some(val) = attr_lookup_ptr(obj_ptr, name_bits) {
                return val;
            }
            if exception_pending() {
                let exc_bits = molt_exception_last();
                molt_exception_clear();
                let _ = molt_raise(exc_bits);
                dec_ref_bits(exc_bits);
                return MoltObject::none().bits();
            }
            let type_id = object_type_id(obj_ptr);
            if type_id == TYPE_ID_DATACLASS {
                let desc_ptr = dataclass_desc_ptr(obj_ptr);
                if !desc_ptr.is_null() && (*desc_ptr).slots {
                    let name = &(*desc_ptr).name;
                    let type_label = if name.is_empty() {
                        "dataclass"
                    } else {
                        name.as_str()
                    };
                    return attr_error(type_label, &attr_name) as u64;
                }
                let type_label = if !desc_ptr.is_null() {
                    let name = &(*desc_ptr).name;
                    if name.is_empty() {
                        "dataclass"
                    } else {
                        name.as_str()
                    }
                } else {
                    "dataclass"
                };
                return attr_error(type_label, &attr_name) as u64;
            }
            if type_id == TYPE_ID_TYPE {
                let class_name = string_obj_to_owned(obj_from_bits(class_name_bits(obj_ptr)))
                    .unwrap_or_default();
                let msg = format!("type object '{class_name}' has no attribute '{attr_name}'");
                return raise_exception::<_>("AttributeError", &msg);
            }
            return attr_error(type_name(MoltObject::from_ptr(obj_ptr)), &attr_name) as u64;
        }
        let obj = obj_from_bits(obj_bits);
        attr_error(type_name(obj), &attr_name) as u64
    }
}

#[no_mangle]
pub extern "C" fn molt_get_attr_name_default(
    obj_bits: u64,
    name_bits: u64,
    default_bits: u64,
) -> u64 {
    let name_obj = obj_from_bits(name_bits);
    let Some(name_ptr) = name_obj.as_ptr() else {
        return raise_attr_name_type_error(name_bits);
    };
    unsafe {
        if object_type_id(name_ptr) != TYPE_ID_STRING {
            return raise_attr_name_type_error(name_bits);
        }
        if let Some(obj_ptr) = maybe_ptr_from_bits(obj_bits) {
            if let Some(val) = attr_lookup_ptr(obj_ptr, name_bits) {
                return val;
            }
            if exception_pending() {
                let exc_bits = molt_exception_last();
                let kind_bits = molt_exception_kind(exc_bits);
                let kind = string_obj_to_owned(obj_from_bits(kind_bits));
                dec_ref_bits(kind_bits);
                if kind.as_deref() == Some("AttributeError") {
                    molt_exception_clear();
                    dec_ref_bits(exc_bits);
                    return default_bits;
                }
                molt_exception_clear();
                let _ = molt_raise(exc_bits);
                dec_ref_bits(exc_bits);
                return MoltObject::none().bits();
            }
            return default_bits;
        }
    }
    default_bits
}

#[no_mangle]
pub extern "C" fn molt_has_attr_name(obj_bits: u64, name_bits: u64) -> u64 {
    let name_obj = obj_from_bits(name_bits);
    let Some(name_ptr) = name_obj.as_ptr() else {
        return raise_attr_name_type_error(name_bits);
    };
    unsafe {
        if object_type_id(name_ptr) != TYPE_ID_STRING {
            return raise_attr_name_type_error(name_bits);
        }
        if let Some(obj_ptr) = maybe_ptr_from_bits(obj_bits) {
            if attr_lookup_ptr(obj_ptr, name_bits).is_some() {
                return MoltObject::from_bool(true).bits();
            }
            if exception_pending() {
                let exc_bits = molt_exception_last();
                let kind_bits = molt_exception_kind(exc_bits);
                let kind = string_obj_to_owned(obj_from_bits(kind_bits));
                dec_ref_bits(kind_bits);
                if kind.as_deref() == Some("AttributeError") {
                    molt_exception_clear();
                    dec_ref_bits(exc_bits);
                    return MoltObject::from_bool(false).bits();
                }
                molt_exception_clear();
                let _ = molt_raise(exc_bits);
                dec_ref_bits(exc_bits);
                return MoltObject::from_bool(false).bits();
            }
            return MoltObject::from_bool(false).bits();
        }
    }
    MoltObject::from_bool(false).bits()
}

#[no_mangle]
pub extern "C" fn molt_set_attr_name(obj_bits: u64, name_bits: u64, val_bits: u64) -> u64 {
    let name_obj = obj_from_bits(name_bits);
    let Some(name_ptr) = name_obj.as_ptr() else {
        return raise_attr_name_type_error(name_bits);
    };
    unsafe {
        if object_type_id(name_ptr) != TYPE_ID_STRING {
            return raise_attr_name_type_error(name_bits);
        }
        if let Some(obj_ptr) = maybe_ptr_from_bits(obj_bits) {
            let bytes = string_bytes(name_ptr);
            let len = string_len(name_ptr);
            return molt_set_attr_generic(obj_ptr, bytes, len as u64, val_bits) as u64;
        }
    }
    let obj = obj_from_bits(obj_bits);
    let name =
        string_obj_to_owned(obj_from_bits(name_bits)).unwrap_or_else(|| "<attr>".to_string());
    attr_error(type_name(obj), &name) as u64
}

#[no_mangle]
pub extern "C" fn molt_del_attr_name(obj_bits: u64, name_bits: u64) -> u64 {
    let name_obj = obj_from_bits(name_bits);
    let Some(name_ptr) = name_obj.as_ptr() else {
        return raise_attr_name_type_error(name_bits);
    };
    unsafe {
        if object_type_id(name_ptr) != TYPE_ID_STRING {
            return raise_attr_name_type_error(name_bits);
        }
        let attr_name =
            string_obj_to_owned(obj_from_bits(name_bits)).unwrap_or_else(|| "<attr>".to_string());
        if let Some(obj_ptr) = maybe_ptr_from_bits(obj_bits) {
            return del_attr_ptr(obj_ptr, name_bits, &attr_name) as u64;
        }
        let obj = obj_from_bits(obj_bits);
        attr_error(type_name(obj), &attr_name) as u64
    }
}
mod arena;
use arena::TempArena;
