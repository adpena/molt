//! Molt Runtime Core
//! Handles memory management, task scheduling, channels, and FFI boundaries.
//!
//! Locking contract (contributor guidance):
//! - Runtime mutation is serialized by the GIL-like lock.
//! - The GIL is the outermost lock; do not acquire it while holding other runtime locks.
//! - Provenance registry locks live in `molt-obj-model` and are sharded; keep their
//!   critical sections small and avoid taking them while holding the GIL for long paths.
//! - Avoid blocking host I/O while holding the GIL; release or schedule work instead.
#![cfg_attr(target_arch = "wasm32", allow(unused))]

#[cfg(all(target_arch = "wasm32", feature = "cext_loader"))]
compile_error!("feature `cext_loader` is unsupported on wasm32 targets");

macro_rules! fn_addr {
    ($func:path) => {
        $crate::builtins::functions::runtime_fn_addr(stringify!($func), $func as *const ())
    };
}

#[cfg(all(not(target_arch = "wasm32"), not(miri)))]
#[global_allocator]
static GLOBAL: mimalloc::MiMalloc = mimalloc::MiMalloc;

#[cfg(test)]
pub(crate) static TEST_MUTEX: std::sync::Mutex<()> = std::sync::Mutex::new(());

// Direct-link test and fuzz builds do not have compiler-emitted isolate
// entrypoints. Provide fallback symbols for those harnesses only while
// production binaries keep using generated symbols.
#[cfg(any(test, fuzzing))]
#[unsafe(no_mangle)]
pub extern "C" fn molt_isolate_bootstrap() -> u64 {
    molt_obj_model::MoltObject::none().bits()
}

#[cfg(any(test, fuzzing))]
#[unsafe(no_mangle)]
pub extern "C" fn molt_isolate_import(_name_bits: u64) -> u64 {
    // Test/fuzz fallback only. Production binaries must provide the
    // app-owned symbol emitted by the compiler or by the embedding host.
    molt_obj_model::MoltObject::none().bits()
}

mod async_rt;
mod builtins;
mod c_api;
mod call;
#[cfg(feature = "stdlib_collections")]
mod collections_bridge;
mod concurrency;
mod constants;
#[cfg(feature = "stdlib_crypto")]
mod crypto_bridge;
#[cfg(feature = "stdlib_difflib")]
mod difflib_bridge;
#[cfg(feature = "stdlib_http")]
mod http_bridge;
#[cfg(feature = "stdlib_ipaddress")]
mod ipaddress_bridge;
#[cfg(feature = "stdlib_itertools")]
mod itertools_bridge;
#[cfg(feature = "stdlib_logging_ext")]
mod logging_bridge;
#[cfg(feature = "stdlib_math")]
mod math_bridge;
#[cfg(feature = "stdlib_path")]
mod path_bridge;
#[cfg(feature = "stdlib_regex")]
mod regex_bridge;
#[cfg(feature = "stdlib_serial")]
mod serial_bridge;
#[cfg(feature = "stdlib_stringprep")]
mod stringprep_bridge;
#[cfg(feature = "stdlib_text")]
mod text_bridge;
#[cfg(feature = "stdlib_xml")]
mod xml_bridge;
#[cfg(feature = "stdlib_zoneinfo")]
mod zoneinfo_bridge;
// Re-export extracted crates so their symbols are available at link time.
#[cfg(feature = "stdlib_collections")]
pub use molt_runtime_collections;
#[cfg(feature = "stdlib_compression")]
pub use molt_runtime_compression;
#[cfg(feature = "stdlib_crypto")]
pub use molt_runtime_crypto;
#[cfg(feature = "stdlib_difflib")]
pub use molt_runtime_difflib;
#[cfg(feature = "stdlib_http")]
pub use molt_runtime_http;
#[cfg(feature = "stdlib_ipaddress")]
pub use molt_runtime_ipaddress;
#[cfg(feature = "stdlib_itertools")]
pub use molt_runtime_itertools;
#[cfg(feature = "stdlib_logging_ext")]
pub use molt_runtime_logging;
#[cfg(feature = "stdlib_math")]
pub use molt_runtime_math;
#[cfg(feature = "stdlib_path")]
pub use molt_runtime_path;
#[cfg(feature = "stdlib_regex")]
pub use molt_runtime_regex;
#[cfg(feature = "stdlib_serial")]
pub use molt_runtime_serial;
#[cfg(feature = "stdlib_stringprep")]
pub use molt_runtime_stringprep;
#[cfg(feature = "stdlib_text")]
pub use molt_runtime_text;
#[cfg(feature = "stdlib_xml")]
pub use molt_runtime_xml;
#[cfg(feature = "stdlib_zoneinfo")]
pub use molt_runtime_zoneinfo;
#[cfg(feature = "stdlib_tk")]
mod gui;
#[cfg(feature = "stdlib_tk")]
mod tk_bridge;
#[cfg(feature = "stdlib_tk")]
pub use molt_runtime_tk;
pub mod audit;
#[cfg(all(feature = "cext_loader", not(target_arch = "wasm32")))]
pub mod cpython_abi_hooks;
mod intrinsics;
#[cfg(target_arch = "wasm32")]
mod libc_compat;
mod object;
mod provenance;
mod randomness;
pub mod refcount_verify;
pub mod resource;
mod state;
mod utils;
pub mod vfs;
mod wasm_abi_exports;

/// Public Rust API for runtime lifecycle, used by `molt-ffi` and other
/// dependent crates that need to call init/shutdown without going through
/// C ABI extern blocks.
pub mod lifecycle {
    use crate::state::runtime_state::{molt_runtime_init, molt_runtime_shutdown};

    /// Initialize the runtime.  Returns 1 on success, 0 if already shut down.
    /// Idempotent: repeated calls after the first return 1 immediately.
    ///
    /// # Safety
    /// Must be called from the main thread before spawning worker threads.
    #[inline]
    pub fn init() -> u64 {
        molt_runtime_init()
    }

    /// Shut down the runtime. Returns 1 on success, 0 if not initialized.
    ///
    /// # Safety
    /// No runtime calls may be made after this returns.
    #[inline]
    pub fn shutdown() -> u64 {
        molt_runtime_shutdown()
    }
}

/// Bridge module exposing runtime intrinsics for the `molt-ffi` crate.
///
/// Functions that are already `pub` via wildcard re-exports (e.g. `molt_len`,
/// `molt_str_from_obj`, `molt_repr_from_obj`, `molt_math_sqrt`) can be called
/// directly through `molt_runtime::…`.  This module exposes intrinsics that are
/// otherwise `pub(crate)` or require GIL-internal helpers.
pub mod ffi_bridge {
    use crate::concurrency::GilGuard;
    use crate::object::type_ids::TYPE_ID_STRING;
    use crate::object::{object_type_id, string_bytes, string_len};

    /// Check whether a runtime capability is granted.
    ///
    /// `cap_name_bits` must be a NaN-boxed Molt string containing the
    /// capability name (e.g. `"net"`, `"fs"`, `"env"`).
    ///
    /// Returns `1` if the capability is granted, `0` otherwise.
    pub fn has_capability(cap_name_bits: u64) -> u64 {
        let _guard = GilGuard::new();
        let py = _guard.token();
        let py = &py;

        let obj = crate::obj_from_bits(cap_name_bits);
        let Some(ptr) = obj.as_ptr() else {
            return 0;
        };

        let type_id = unsafe { object_type_id(ptr) };
        if type_id != TYPE_ID_STRING {
            return 0;
        }

        let bytes = unsafe {
            let len = string_len(ptr);
            let data = string_bytes(ptr);
            std::slice::from_raw_parts(data, len)
        };

        let Ok(name) = std::str::from_utf8(bytes) else {
            return 0;
        };

        if crate::has_capability(py, name) {
            1
        } else {
            0
        }
    }
}

#[allow(unused_imports)]
pub(crate) use crate::async_rt::*;
pub use crate::builtins::gpu::molt_gpu_broadcast_binary_contiguous;
pub use crate::builtins::gpu::molt_gpu_buffer_to_list;
pub use crate::builtins::gpu::molt_gpu_interop__load_safetensors;
pub use crate::builtins::gpu::molt_gpu_interop_decode_bf16_bytes_to_f32;
pub use crate::builtins::gpu::molt_gpu_interop_decode_f16_bytes_to_f32;
pub use crate::builtins::gpu::molt_gpu_linear_contiguous;
pub use crate::builtins::gpu::molt_gpu_linear_split_last_dim_contiguous;
pub use crate::builtins::gpu::molt_gpu_linear_squared_relu_gate_interleaved_contiguous;
pub use crate::builtins::gpu::molt_gpu_matmul_contiguous;
pub use crate::builtins::gpu::molt_gpu_permute_contiguous;
pub use crate::builtins::gpu::molt_gpu_repeat_axis_contiguous;
pub use crate::builtins::gpu::molt_gpu_rms_norm_last_axis_contiguous;
pub use crate::builtins::gpu::molt_gpu_rope_apply_contiguous;
pub use crate::builtins::gpu::molt_gpu_softmax_last_axis_contiguous;
pub use crate::builtins::gpu::molt_gpu_squared_relu_gate_interleaved_contiguous;
pub use crate::builtins::gpu::molt_gpu_tensor__tensor_concat_first_dim;
pub use crate::builtins::gpu::molt_gpu_tensor__tensor_data_list;
pub use crate::builtins::gpu::molt_gpu_tensor__tensor_linear;
pub use crate::builtins::gpu::molt_gpu_tensor__tensor_linear_split_last_dim;
pub use crate::builtins::gpu::molt_gpu_tensor__tensor_linear_squared_relu_gate_interleaved;
pub use crate::builtins::gpu::molt_gpu_tensor__tensor_permute_dims;
pub use crate::builtins::gpu::molt_gpu_tensor__tensor_reshape_view;
pub use crate::builtins::gpu::molt_gpu_tensor__tensor_scaled_dot_product_attention;
pub use crate::builtins::gpu::molt_gpu_tensor__tensor_scatter_rows;
pub use crate::builtins::gpu::molt_gpu_tensor__tensor_softmax_last_axis;
pub use crate::builtins::gpu::molt_gpu_tensor__tensor_take_rows;
pub use crate::builtins::gpu::molt_gpu_tensor__zeros;
pub use crate::builtins::gpu::molt_gpu_tensor_from_buffer;
pub use crate::builtins::gpu::molt_gpu_tensor_from_parts;
pub use crate::builtins::gpu::molt_gpu_turboquant_attention_packed;
#[cfg(feature = "molt_gpu_primitives")]
pub use crate::builtins::gpu_primitives::{
    molt_gpu_prim_binary, molt_gpu_prim_create_tensor, molt_gpu_prim_device, molt_gpu_prim_realize,
    molt_gpu_prim_reduce, molt_gpu_prim_unary, molt_gpu_prim_zeros,
};
pub use crate::builtins::strings::molt_string_from_bytes;
pub use crate::concurrency::isolates::*;
pub(crate) use crate::concurrency::locks::{
    molt_barrier_abort, molt_barrier_broken, molt_barrier_drop, molt_barrier_n_waiting,
    molt_barrier_new, molt_barrier_parties, molt_barrier_reset, molt_barrier_wait,
    molt_condition_drop, molt_condition_new, molt_condition_notify, molt_condition_wait,
    molt_condition_wait_for, molt_event_clear, molt_event_drop, molt_event_is_set, molt_event_new,
    molt_event_set, molt_event_wait, molt_local_drop, molt_local_get_dict, molt_local_new,
    molt_lock_acquire, molt_lock_drop, molt_lock_locked, molt_lock_new, molt_lock_release,
    molt_queue_drop, molt_queue_empty, molt_queue_full, molt_queue_get, molt_queue_is_shutdown,
    molt_queue_join, molt_queue_lifo_new, molt_queue_new, molt_queue_priority_new, molt_queue_put,
    molt_queue_qsize, molt_queue_shutdown, molt_queue_task_done, molt_rlock_acquire,
    molt_rlock_acquire_restore, molt_rlock_drop, molt_rlock_is_owned, molt_rlock_locked,
    molt_rlock_new, molt_rlock_release, molt_rlock_release_save, molt_semaphore_acquire,
    molt_semaphore_drop, molt_semaphore_new, molt_semaphore_release,
};
#[allow(unused_imports)]
pub(crate) use crate::concurrency::{
    GilGuard, GilReleaseGuard, PyToken, gil_assert, gil_held, with_gil,
};
#[allow(unused_imports)]
pub(crate) use crate::state::RuntimeState;
pub use crate::wasm_abi_exports::{
    molt_dict_getitem, molt_dict_setitem, molt_fast_dict_get, molt_fast_list_append,
    molt_fast_str_join, molt_fast_str_lower, molt_fast_str_startswith, molt_fast_str_strip,
    molt_fast_str_upper, molt_resource_on_allocate, molt_resource_on_free, molt_scratch_alloc,
    molt_scratch_free, molt_tuple_getitem, molt_type_tag_of_bits,
};
#[allow(unused_imports)]
pub(crate) use molt_obj_model::MoltObject;

pub use crate::async_rt::cancellation::*;
pub use crate::async_rt::channels::*;
pub(crate) use crate::async_rt::channels::{
    capability_fix_hint, has_capability, raise_capability_denied,
};
pub use crate::async_rt::event_loop::*;
#[allow(unused_imports)]
pub use crate::async_rt::generators::*;
#[cfg(any(molt_has_net_io, target_arch = "wasm32"))]
pub(crate) use crate::async_rt::io_poller::IoPoller;
#[cfg(any(molt_has_net_io, target_arch = "wasm32"))]
pub use crate::async_rt::io_poller::*;
#[cfg(not(target_arch = "wasm32"))]
pub(crate) use crate::async_rt::is_block_on_task;
#[cfg(not(any(molt_has_net_io, target_arch = "wasm32")))]
pub(crate) use crate::async_rt::net_stubs::IoPoller;
#[cfg(not(any(molt_has_net_io, target_arch = "wasm32")))]
pub(crate) use crate::async_rt::net_stubs::io_wait_release_socket;
#[cfg(not(any(molt_has_net_io, target_arch = "wasm32")))]
pub use crate::async_rt::net_stubs::*;
pub use crate::async_rt::process::*;
pub(crate) use crate::async_rt::scheduler::BLOCK_ON_TASK;
#[cfg(any(molt_has_net_io, target_arch = "wasm32"))]
pub(crate) use crate::async_rt::sockets::io_wait_release_socket;
pub use crate::async_rt::sockets::*;
// Socket utilities from sockets_net.rs: only available when networking is compiled in.
#[cfg(not(any(molt_has_net_io, target_arch = "wasm32")))]
pub use crate::async_rt::net_stubs::{
    molt_socket_cmsg_len, molt_socket_cmsg_space, molt_socket_if_indextoname,
    molt_socket_if_nameindex, molt_socket_if_nametoindex, molt_socket_recv_fds,
    molt_socket_send_fds, molt_socket_sendmsg_afalg, molt_socket_sethostname,
};
#[cfg(molt_has_net_io)]
pub use crate::async_rt::sockets::{
    molt_socket_cmsg_len, molt_socket_cmsg_space, molt_socket_if_indextoname,
    molt_socket_if_nameindex, molt_socket_if_nametoindex, molt_socket_recv_fds,
    molt_socket_send_fds, molt_socket_sendmsg_afalg, molt_socket_sethostname,
};

#[cfg(not(any(molt_has_net_io, target_arch = "wasm32")))]
pub use crate::async_rt::net_stubs::{
    molt_socket_getfqdn, molt_socket_gethostbyaddr, molt_socket_gethostbyname,
    molt_socket_gethostbyname_ex, molt_socket_htonl, molt_socket_htons, molt_socket_ntohl,
    molt_socket_ntohs,
};

#[cfg(not(target_arch = "wasm32"))]
pub(crate) use crate::async_rt::sockets::{
    argv_from_bits, env_from_bits, require_net_capability, require_process_capability,
    require_time_wall_capability,
};
pub use crate::async_rt::threads::*;
pub(crate) use crate::async_rt::{
    anext_default_poll_fn_addr, async_sleep_poll_fn_addr, asyncgen_poll_fn_addr,
    asyncio_fd_watcher_poll_fn_addr, asyncio_gather_poll_fn_addr,
    asyncio_ready_runner_poll_fn_addr, asyncio_server_accept_loop_poll_fn_addr,
    asyncio_sock_accept_poll_fn_addr, asyncio_sock_connect_poll_fn_addr,
    asyncio_sock_recv_into_poll_fn_addr, asyncio_sock_recv_poll_fn_addr,
    asyncio_sock_recvfrom_into_poll_fn_addr, asyncio_sock_recvfrom_poll_fn_addr,
    asyncio_sock_sendall_poll_fn_addr, asyncio_sock_sendto_poll_fn_addr,
    asyncio_socket_reader_read_poll_fn_addr, asyncio_socket_reader_readline_poll_fn_addr,
    asyncio_stream_reader_read_poll_fn_addr, asyncio_stream_reader_readline_poll_fn_addr,
    asyncio_stream_send_all_poll_fn_addr, asyncio_timer_handle_poll_fn_addr,
    asyncio_wait_for_poll_fn_addr, asyncio_wait_poll_fn_addr, call_poll_fn,
    contextlib_async_exitstack_enter_context_poll_fn_addr,
    contextlib_async_exitstack_exit_poll_fn_addr, contextlib_asyncgen_enter_poll_fn_addr,
    contextlib_asyncgen_exit_poll_fn_addr, io_wait_poll_fn_addr, molt_block_on,
    poll_future_with_task_stack, process_poll_fn_addr, resolve_task_ptr, thread_poll_fn_addr,
};
pub(crate) use crate::async_rt::{
    molt_asyncio_task_last_exception_clear, molt_asyncio_task_registry_live,
    molt_asyncio_task_registry_live_set,
};
pub use crate::builtins::abc::*;
#[cfg(not(feature = "stdlib_collections"))]
pub use crate::builtins::argparse::*;
pub use crate::builtins::array_mod::*;
#[cfg(feature = "stdlib_ast")]
pub use crate::builtins::ast::*;
pub use crate::builtins::asyncio_core::*;
pub use crate::builtins::asyncio_queue::*;
pub use crate::builtins::atexit::*;
pub(crate) use crate::builtins::attr::{
    apply_class_slots_layout, attr_error, attr_error_with_message, attr_error_with_obj,
    attr_error_with_obj_message, attr_lookup_ptr_allow_missing, attr_name_bits_from_bytes,
    class_attr_lookup, class_attr_lookup_raw_mro, class_field_offset, dataclass_attr_lookup_raw,
    descriptor_bind, descriptor_cache_lookup, descriptor_cache_store, descriptor_is_data,
    descriptor_method_bits, descriptor_no_deleter, descriptor_no_setter,
    dir_collect_from_class_bits, dir_collect_from_instance, instance_bits_for_call,
    is_iterator_bits, module_attr_lookup, object_attr_lookup_raw, property_no_deleter,
    property_no_setter, raise_attr_name_type_error,
};
pub use crate::builtins::attributes::*;
#[cfg(not(feature = "stdlib_serial"))]
pub use crate::builtins::base64_mod::*;
#[cfg(not(feature = "stdlib_serial"))]
pub use crate::builtins::binascii::*;
#[cfg(feature = "stdlib_compression")]
pub use crate::builtins::bz2::*;
pub use crate::builtins::callable::*;
pub(crate) use crate::builtins::classes::{
    BuiltinClasses, builtin_classes, builtin_classes_if_initialized, builtin_classes_shutdown,
    builtin_type_bits, class_name_for_error, is_builtin_class_bits, molt_builtin_class_lookup,
};
#[cfg(not(feature = "stdlib_math"))]
pub use crate::builtins::cmath_mod::*;
pub use crate::builtins::codecs::*;
pub use crate::builtins::codecs_ext::*;
#[cfg(not(feature = "stdlib_collections"))]
pub use crate::builtins::collections_ext::*;
#[cfg(not(feature = "stdlib_math"))]
pub use crate::builtins::colorsys::*;
#[cfg(feature = "stdlib_compression")]
pub use crate::builtins::compression_common::*;
pub use crate::builtins::concurrent::*;
#[cfg(not(feature = "stdlib_serial"))]
pub use crate::builtins::configparser::*;
pub(crate) use crate::builtins::containers::{
    dict_len, dict_method_bits, dict_order, dict_order_ptr, dict_table, dict_table_ptr,
    dict_view_as_set_bits, dict_view_dict_bits, dict_view_entry, dict_view_len,
    frozenset_method_bits, is_set_inplace_rhs_type, is_set_like_type, is_set_view_type, list_len,
    list_method_bits, set_len, set_method_bits, set_order, set_order_ptr, set_table, set_table_ptr,
    tuple_len,
};
pub(crate) use crate::builtins::containers_alloc::{DictSeqError, dict_pair_from_item};
pub use crate::builtins::containers_alloc::{
    molt_dict_from_obj, molt_dict_new, molt_frozenset_new, molt_set_new,
};
pub use crate::builtins::context::*;
pub(crate) use crate::builtins::context::{
    context_payload_bits, context_stack_store, context_stack_take, context_stack_unwind,
    generator_context_stack_drop, generator_context_stack_store, generator_context_stack_take,
};
pub use crate::builtins::contextlib::*;
pub(crate) use crate::builtins::contextlib::{
    contextlib_async_exitstack_enter_context_task_drop, contextlib_async_exitstack_exit_task_drop,
    contextlib_asyncgen_enter_task_drop, contextlib_asyncgen_exit_task_drop,
};
pub use crate::builtins::copy_mod::*;
#[cfg(not(feature = "stdlib_serial"))]
pub use crate::builtins::csv::*;
#[cfg(not(feature = "stdlib_serial"))]
pub use crate::builtins::datetime::*;
pub use crate::builtins::dbm_dumb::*;
#[cfg(not(feature = "stdlib_serial"))]
pub use crate::builtins::decimal::*;
#[cfg(not(feature = "stdlib_difflib"))]
pub use crate::builtins::difflib::*;
pub use crate::builtins::enum_ext::*;
pub(crate) use crate::builtins::exceptions::{
    ACTIVE_EXCEPTION_FALLBACK, ACTIVE_EXCEPTION_STACK, EXCEPTION_STACK, ExceptionSentinel,
    GENERATOR_EXCEPTION_STACKS, GENERATOR_RAISE, TASK_RAISE_ACTIVE, alloc_exception,
    alloc_exception_from_class_bits, clear_exception, clear_exception_state,
    clear_exception_type_cache, exception_args_bits, exception_args_from_iterable,
    exception_cause_bits, exception_class_bits, exception_clear_reason_set,
    exception_context_align_depth, exception_context_bits, exception_context_fallback_pop,
    exception_context_fallback_push, exception_dict_bits, exception_group_method_bits,
    exception_handler_active, exception_kind_bits, exception_last_bits_noinc,
    exception_message_from_args, exception_method_bits, exception_msg_bits, exception_pending,
    exception_set_stop_iteration_value, exception_stack_baseline_get, exception_stack_baseline_set,
    exception_stack_depth, exception_stack_pop, exception_stack_push, exception_stack_set_depth,
    exception_store_args_and_message, exception_suppress_bits, exception_trace_bits,
    exception_type_bits, exception_type_bits_from_name, exception_value_bits, format_exception,
    format_exception_message, format_exception_with_traceback, frame_stack_pop, frame_stack_push,
    frame_stack_set_line, generator_exception_stack_drop, generator_exception_stack_store,
    generator_exception_stack_take, generator_raise_active, handle_system_exit,
    molt_exception_active, molt_exception_clear, molt_exception_kind, molt_exception_last,
    molt_exception_pending, molt_exception_set_last, molt_getframe, molt_globals_builtin,
    molt_locals_builtin, molt_raise, raise_exception, raise_key_error_with_key, raise_not_iterable,
    raise_unicode_decode_error, raise_unicode_encode_error, raise_unsupported_inplace,
    record_exception, set_generator_raise, set_task_raise_active, task_exception_baseline_drop,
    task_exception_baseline_store, task_exception_baseline_take, task_exception_depth_drop,
    task_exception_depth_store, task_exception_depth_take, task_exception_handler_stack_drop,
    task_exception_handler_stack_store, task_exception_handler_stack_take,
    task_exception_stack_drop, task_exception_stack_store, task_exception_stack_take,
    task_last_exception_drop, task_raise_active,
};
pub(crate) use crate::builtins::exceptions::{raise_os_error, raise_os_error_errno};
pub use crate::builtins::fcntl::*;
pub use crate::builtins::fnmatch::*;
#[cfg(not(feature = "stdlib_math"))]
pub use crate::builtins::fractions::*;
pub use crate::builtins::functions::*;
#[cfg(not(feature = "stdlib_serial"))]
pub use crate::builtins::functions_email::*;
#[cfg(not(feature = "stdlib_http"))]
pub use crate::builtins::functions_http::*;
#[cfg(not(feature = "stdlib_http"))]
pub use crate::builtins::functions_logging::*;
pub use crate::builtins::functions_pickle::*;
pub use crate::builtins::functions_re::*;
pub use crate::builtins::functions_stat::*;
#[cfg(not(feature = "stdlib_serial"))]
pub use crate::builtins::functions_zipfile::*;
pub use crate::builtins::functools::*;
#[cfg(feature = "stdlib_fs_extra")]
pub use crate::builtins::glob_mod::*;
pub use crate::builtins::graphlib::*;
#[cfg(feature = "stdlib_compression")]
pub use crate::builtins::gzip::*;
#[cfg(feature = "stdlib_crypto")]
pub use crate::builtins::hashlib::*;
#[cfg(feature = "stdlib_crypto")]
pub use crate::builtins::hmac::*;
#[cfg(not(feature = "stdlib_text"))]
pub use crate::builtins::html::*;
pub use crate::builtins::inspect::*;
pub use crate::builtins::io::*;
pub(crate) use crate::builtins::io::{
    DecodeFailure, close_payload, file_handle_detached_message, file_handle_enter,
    file_handle_exit, file_handle_is_closed, path_from_bits,
};
#[cfg(not(feature = "stdlib_ipaddress"))]
pub use crate::builtins::ipaddress::*;
#[cfg(not(feature = "stdlib_itertools"))]
pub use crate::builtins::itertools::*;
pub use crate::builtins::json::*;
#[cfg(not(feature = "stdlib_logging_ext"))]
pub use crate::builtins::logging_ext::*;
#[cfg(feature = "stdlib_compression")]
pub use crate::builtins::lzma::*;
#[cfg(not(feature = "stdlib_math"))]
pub use crate::builtins::math::*;
pub(crate) use crate::builtins::methods::*;
pub use crate::builtins::modules::*;
pub(crate) use crate::builtins::numbers::{
    ComplexParts, bigint_bits, bigint_from_f64_trunc, bigint_ptr_from_bits, bigint_ref,
    bigint_to_inline, compare_numbers, complex_bits, complex_from_obj_lossy,
    complex_from_obj_strict, complex_ptr_from_bits, complex_ref, float_pair_from_obj,
    index_bigint_from_obj, index_i64_from_obj, index_i64_with_overflow, inline_int_from_i128,
    int_bits_from_bigint, int_bits_from_i64, int_bits_from_i128, int_subclass_value_bits_raw,
    round_float_ndigits, round_half_even, split_maxsplit_from_obj, to_bigint, to_f64, to_i64,
};
pub use crate::builtins::operator::*;
#[cfg(not(feature = "stdlib_path"))]
pub use crate::builtins::os_ext::*;
#[cfg(not(feature = "stdlib_path"))]
pub use crate::builtins::pathlib::*;
pub use crate::builtins::platform::*;
pub use crate::builtins::platform_mod::*;
pub use crate::builtins::pprint_ext::*;
pub use crate::builtins::punycode::*;
#[cfg(not(feature = "stdlib_math"))]
pub use crate::builtins::random_mod::*;
#[cfg(not(feature = "stdlib_regex"))]
pub use crate::builtins::regex::*;
#[cfg(feature = "stdlib_crypto")]
pub use crate::builtins::secrets::*;
pub use crate::builtins::select::*;
pub use crate::builtins::shutil::*;
pub use crate::builtins::signal_ext::*;
pub use crate::builtins::sitebuiltins::*;
#[cfg(feature = "sqlite")]
pub use crate::builtins::sqlite3::*;
#[cfg(feature = "stdlib_net")]
pub use crate::builtins::ssl::*;
pub use crate::builtins::string_ext::*;
#[cfg(not(feature = "stdlib_stringprep"))]
pub use crate::builtins::stringprep::*;
pub(crate) use crate::builtins::strings::{
    bytes_count_impl, bytes_find_impl, bytes_rfind_impl, bytes_strip_range, replace_bytes_impl,
    replace_bytes_impl_limit, replace_string_impl, rsplit_bytes_to_list_maxsplit,
    rsplit_bytes_whitespace_to_list_maxsplit, rsplit_string_bytes_to_list_maxsplit,
    rsplit_string_whitespace_to_list_maxsplit, split_bytes_to_list_maxsplit,
    split_bytes_whitespace_to_list_maxsplit, split_string_bytes_to_list_maxsplit,
    split_string_whitespace_to_list_maxsplit, splitlines_bytes_to_list, splitlines_string_to_list,
};
#[cfg(not(feature = "stdlib_serial"))]
pub use crate::builtins::structs::*;
pub use crate::builtins::subprocess_ext::*;
pub use crate::builtins::sys_ext::*;
#[cfg(feature = "stdlib_compression")]
pub use crate::builtins::tarfile::*;
#[cfg(feature = "stdlib_fs_extra")]
pub use crate::builtins::tempfile_mod::*;
#[cfg(feature = "stdlib_tk")]
pub use crate::builtins::tkinter_core::*;
pub(crate) use crate::builtins::type_ops::{
    class_bases_vec, class_mro_ref, class_mro_vec, isinstance_bits, isinstance_runtime,
    issubclass_bits, issubclass_runtime, type_of_bits,
};
pub use crate::builtins::types::*;
#[cfg(not(feature = "stdlib_text"))]
pub use crate::builtins::unicodedata_mod::*;
pub use crate::builtins::warnings_ext::*;
#[cfg(feature = "stdlib_compression")]
pub use crate::builtins::zlib::*;
#[cfg(not(feature = "stdlib_zoneinfo"))]
pub use crate::builtins::zoneinfo::*;
#[allow(unused_imports)]
pub(crate) use crate::call::bind::molt_callargs_push_kw;
pub(crate) use crate::call::bind::{
    callargs_dec_ref_all, callargs_ptr, molt_call_bind, molt_callargs_expand_kwstar,
    molt_callargs_expand_star, molt_callargs_new, molt_callargs_push_pos,
};
pub(crate) use crate::call::class_init::{
    alloc_instance_for_class, alloc_instance_for_class_no_pool,
    alloc_instance_for_default_object_new, call_builtin_type_if_needed, call_class_init_with_args,
    function_attr_bits, function_set_attr_bits, raise_not_callable, try_call_generator,
};
pub(crate) use crate::call::dispatch::{
    call_callable0, call_callable1, call_callable2, call_callable3, callable_arity,
};
pub(crate) use crate::call::function::{
    call_function_obj_vec, call_function_obj0, call_function_obj1, call_function_obj2,
    call_function_obj3, refresh_function_task_trampoline_cache,
};
pub(crate) use crate::call::lookup_call_attr;
pub(crate) use crate::constants::*;
#[cfg(feature = "stdlib_tk")]
pub use crate::gui::tk::*;
pub use crate::intrinsics::capabilities::*;
pub(crate) use crate::object::accessors::{
    object_field_get_ptr_raw, object_field_set_ptr_raw, resolve_obj_ptr,
};
pub use crate::object::buffer2d::*;
pub use crate::object::builders::*;
pub(crate) use crate::object::builders::{PtrDropGuard, alloc_dict_with_pairs};
#[allow(unused_imports)]
pub(crate) use crate::object::layout::{
    bound_method_func_bits, bound_method_self_bits, bytearray_data, bytearray_len, bytearray_vec,
    bytearray_vec_ptr, bytearray_vec_ref, call_iter_callable_bits, call_iter_sentinel_bits,
    class_annotate_bits, class_annotations_bits, class_bases_bits, class_bump_layout_version,
    class_dict_bits, class_layout_version_bits, class_mro_bits, class_name_bits,
    class_qualname_bits, class_set_annotate_bits, class_set_annotations_bits, class_set_bases_bits,
    class_set_layout_version_bits, class_set_mro_bits, class_set_name_bits,
    class_set_qualname_bits, classmethod_func_bits, code_argcount, code_filename_bits,
    code_firstlineno, code_kwonlyargcount, code_linetable_bits, code_name_bits,
    code_posonlyargcount, code_varnames_bits, ensure_function_code_bits,
    CALL_ITER_PAYLOAD_SIZE, ENUMERATE_PAYLOAD_SIZE, MAP_PAYLOAD_SIZE, call_iter_cached_tuple,
    call_iter_set_cached_tuple, enumerate_cached_inner, enumerate_cached_outer,
    enumerate_index_bits, enumerate_set_cached_inner, enumerate_set_cached_outer,
    enumerate_set_index_bits, enumerate_target_bits, filter_func_bits, filter_iter_bits,
    function_annotate_bits, function_annotations_bits, function_arity, function_closure_bits,
    function_code_bits, function_dict_bits, function_fn_ptr, function_globals_bits,
    function_name_bits, function_set_annotate_bits, function_set_annotations_bits,
    function_set_closure_bits, function_set_code_bits, function_set_dict_bits,
    function_set_globals_bits, function_set_trampoline_ptr, function_trampoline_ptr,
    generic_alias_args_bits, generic_alias_origin_bits, iter_cached_tuple, iter_index,
    iter_set_cached_tuple, iter_set_index, iter_target_bits, map_cached_tuple, map_func_bits,
    map_iters_ptr, map_set_cached_tuple,
    module_dict_bits, module_name_bits, property_del_bits, property_get_bits, property_set_bits,
    range_len_i64, range_start_bits, range_step_bits, range_stop_bits, reversed_index,
    reversed_set_index, reversed_target_bits, seq_vec, seq_vec_ptr, seq_vec_ref, slice_start_bits,
    slice_step_bits, slice_stop_bits, staticmethod_func_bits, super_obj_bits, super_type_bits,
    union_type_args_bits, zip_iters_ptr, zip_set_strict_bits, zip_strict_bits,
};
pub(crate) use crate::object::memoryview::{
    bytes_like_slice, bytes_like_slice_raw, memoryview_bytes_slice, memoryview_bytes_slice_mut,
    memoryview_collect_bytes, memoryview_format_from_bits, memoryview_format_from_str,
    memoryview_is_c_contiguous_view, memoryview_nbytes, memoryview_nbytes_big,
    memoryview_read_scalar, memoryview_shape_product, memoryview_write_bytes,
    memoryview_write_scalar,
};
pub(crate) use crate::object::ops::HashSecret;
pub use crate::object::ops::*;
#[allow(unused_imports)]
pub(crate) use crate::object::ops::{
    DecodeTextError, class_break_cycles, decode_bytes_text, decode_string_list, decode_value_list,
    dict_clear_in_place, dict_clear_method, dict_copy_method, dict_del_in_place, dict_find_entry,
    dict_find_entry_kv_in_place, dict_fromkeys_method, dict_get_in_place, dict_get_method,
    dict_items_method, dict_keys_method, dict_pop_method, dict_popitem_method, dict_set_in_place,
    dict_setdefault_method, dict_table_capacity, dict_update_method, dict_update_set_via_store,
    dict_values_method, format_obj, format_obj_str, frozenset_from_iter_bits, hash_slice_bits,
    is_truthy, list_from_iter_bits, obj_eq, set_add_in_place, set_del_in_place, set_find_entry,
    set_replace_entries, set_table_capacity, tuple_from_isize_slice, tuple_from_iter_bits,
    type_name,
};
pub use crate::object::ops_arith::*;
pub use crate::object::ops_builtins::*;
pub use crate::object::ops_bytes::*;
pub use crate::object::ops_compare::*;
pub use crate::object::ops_convert::*;
pub use crate::object::ops_dict::*;
#[allow(unused_imports)]
pub(crate) use crate::object::ops_dict::{dict_update_apply, dict_update_set_in_place};
pub use crate::object::ops_heapq::*;
pub use crate::object::ops_iter::*;
pub use crate::object::ops_list::*;
pub use crate::object::ops_memoryview::*;
pub use crate::object::ops_set::*;
pub use crate::object::ops_slice::*;
pub use crate::object::ops_string::*;
#[allow(unused_imports)]
pub(crate) use crate::object::ops_string::{utf8_cache_remove, utf8_codepoint_count_cached};
pub use crate::object::ops_vec::*;
pub(crate) use crate::object::type_ids::*;
pub(crate) use crate::object::weakref::weakref_clear_for_ptr;
pub use crate::object::weakref::{
    molt_weakkeydict_clear, molt_weakkeydict_contains, molt_weakkeydict_del, molt_weakkeydict_get,
    molt_weakkeydict_items, molt_weakkeydict_keyrefs, molt_weakkeydict_len,
    molt_weakkeydict_popitem, molt_weakkeydict_set, molt_weakref_callback, molt_weakref_collect,
    molt_weakref_count, molt_weakref_drop, molt_weakref_finalize_track,
    molt_weakref_finalize_untrack, molt_weakref_find_nocallback, molt_weakref_get,
    molt_weakref_peek, molt_weakref_refs, molt_weakref_register, molt_weakset_add,
    molt_weakset_clear, molt_weakset_contains, molt_weakset_discard, molt_weakset_items,
    molt_weakset_len, molt_weakset_pop, molt_weakset_remove, molt_weakvaluedict_clear,
    molt_weakvaluedict_contains, molt_weakvaluedict_del, molt_weakvaluedict_get,
    molt_weakvaluedict_items, molt_weakvaluedict_len, molt_weakvaluedict_popitem,
    molt_weakvaluedict_set, molt_weakvaluedict_valuerefs,
};
pub(crate) use crate::object::{
    Buffer2D, DataclassDesc, HEADER_FLAG_BLOCK_ON, HEADER_FLAG_CANCEL_PENDING,
    HEADER_FLAG_FUNC_TASK_TRAMPOLINE_KNOWN, HEADER_FLAG_FUNC_TASK_TRAMPOLINE_NEEDED,
    HEADER_FLAG_GEN_RUNNING, HEADER_FLAG_GEN_STARTED, HEADER_FLAG_SKIP_CLASS_DECREF,
    HEADER_FLAG_SPAWN_RETAIN, HEADER_FLAG_TASK_DONE, HEADER_FLAG_TASK_QUEUED,
    HEADER_FLAG_TASK_RUNNING, HEADER_FLAG_TASK_WAKE_PENDING, HEADER_FLAG_TRACEBACK_SUPPRESSED,
    MemoryView, MemoryViewFormat, MemoryViewFormatKind, MoltFileHandle, MoltFileState,
    OBJECT_POOL_BUCKETS, OBJECT_POOL_TLS, PtrSlot, alloc_object, alloc_object_zeroed,
    alloc_object_zeroed_with_pool, bits_from_ptr, buffer2d_ptr, bytes_data, bytes_len,
    dataclass_desc_ptr, dataclass_dict_bits, dataclass_fields_mut, dataclass_fields_ref,
    dataclass_set_dict_bits, dec_ref_bits, file_handle_ptr, header_from_obj_ptr, inc_ref_bits,
    init_atomic_bits, instance_dict_bits, instance_set_dict_bits, intarray_len, intarray_slice,
    maybe_ptr_from_bits, memoryview_format_bits, memoryview_itemsize, memoryview_len,
    memoryview_ndim, memoryview_offset, memoryview_owner_bits, memoryview_ptr, memoryview_readonly,
    memoryview_shape, memoryview_stride, memoryview_strides, obj_from_bits, object_class_bits,
    object_mark_has_ptrs, object_payload_size, object_set_class_bits, object_type_id,
    pending_bits_i64, ptr_from_bits, string_bytes, string_len,
};
pub use crate::object::{
    MoltHeader, bump_type_version, global_type_version, molt_dec_ref, molt_inc_ref,
};
#[allow(unused_imports)]
pub(crate) use crate::provenance::{register_ptr, release_ptr, reset_ptr_registry, resolve_ptr};
pub(crate) use crate::state::cache::{InternedNames, MethodCache, intern_static_name};
pub(crate) use crate::state::runtime_state::{runtime_state, runtime_state_for_gil};
#[allow(unused_imports)]
pub(crate) use crate::state::{
    CONTEXT_STACK, DEFAULT_RECURSION_LIMIT, FRAME_STACK, GIL_DEPTH, PARSE_ARENA, RECURSION_DEPTH,
    RECURSION_LIMIT, REPR_DEPTH, REPR_SET, REPR_STACK, TRACEBACK_SUPPRESS, current_rss_bytes,
    profile_enabled, profile_hit, profile_hit_bytes, profile_hit_unchecked, recursion_guard_enter,
    recursion_guard_exit, recursion_limit_get, recursion_limit_set, sample_peak_rss,
    traceback_suppress_enter, traceback_suppress_exit, traceback_suppressed,
};
#[cfg(feature = "stdlib_collections")]
pub use molt_runtime_collections::argparse::*;
#[cfg(feature = "stdlib_collections")]
pub use molt_runtime_collections::collections_ext::*;
#[cfg(feature = "stdlib_difflib")]
pub use molt_runtime_difflib::difflib::*;
#[cfg(feature = "stdlib_http")]
pub use molt_runtime_http::functions_http::*;
#[cfg(feature = "stdlib_http")]
pub use molt_runtime_http::functions_logging::*;
#[cfg(feature = "stdlib_ipaddress")]
pub use molt_runtime_ipaddress::ipaddress::*;
#[cfg(feature = "stdlib_itertools")]
pub use molt_runtime_itertools::itertools::*;
#[cfg(feature = "stdlib_logging_ext")]
pub use molt_runtime_logging::logging_ext::*;
#[cfg(feature = "stdlib_math")]
pub use molt_runtime_math::cmath_mod::*;
#[cfg(feature = "stdlib_math")]
pub use molt_runtime_math::colorsys::*;
#[cfg(feature = "stdlib_math")]
pub use molt_runtime_math::fractions::*;
#[cfg(feature = "stdlib_math")]
pub use molt_runtime_math::math::*;
#[cfg(feature = "stdlib_math")]
pub use molt_runtime_math::random_mod::*;
#[cfg(feature = "stdlib_path")]
pub use molt_runtime_path::os_ext::*;
#[cfg(feature = "stdlib_path")]
pub use molt_runtime_path::pathlib::*;
#[cfg(feature = "stdlib_regex")]
pub use molt_runtime_regex::regex::*;
#[cfg(feature = "stdlib_serial")]
pub use molt_runtime_serial::base64_mod::*;
#[cfg(feature = "stdlib_serial")]
pub use molt_runtime_serial::binascii::*;
#[cfg(feature = "stdlib_serial")]
pub use molt_runtime_serial::configparser::*;
#[cfg(feature = "stdlib_csv")]
pub use molt_runtime_serial::csv::*;
#[cfg(feature = "stdlib_serial")]
pub use molt_runtime_serial::datetime::*;
#[cfg(feature = "stdlib_serial")]
pub use molt_runtime_serial::decimal::*;
#[cfg(feature = "stdlib_serial")]
pub use molt_runtime_serial::email::*;
#[cfg(feature = "stdlib_serial")]
pub use molt_runtime_serial::structs::*;
#[cfg(feature = "stdlib_serial")]
pub use molt_runtime_serial::zipfile::*;
#[cfg(feature = "stdlib_stringprep")]
pub use molt_runtime_stringprep::stringprep::*;
#[cfg(feature = "stdlib_text")]
pub use molt_runtime_text::html::*;
#[cfg(feature = "stdlib_text")]
pub use molt_runtime_text::unicodedata_mod::*;
#[cfg(feature = "stdlib_xml")]
pub use molt_runtime_xml::xml_etree::*;
#[cfg(feature = "stdlib_xml")]
pub use molt_runtime_xml::xml_sax::*;
// When stdlib_xml is OFF the in-crate `builtins::xml_etree` / `builtins::xml_sax`
// modules are compiled instead.  Re-export their `extern "C"` intrinsics at the
// crate root so the generated symbol resolver in `intrinsics::generated` can
// reach them via `crate::molt_xml_*` paths.
#[cfg(not(feature = "stdlib_xml"))]
pub use crate::builtins::xml_etree::*;
#[cfg(not(feature = "stdlib_xml"))]
pub use crate::builtins::xml_sax::*;
#[cfg(feature = "stdlib_zoneinfo")]
pub use molt_runtime_zoneinfo::zoneinfo::*;
// The extern "C" profiling entrypoints only exist on non-wasm32 targets.
#[cfg(not(target_arch = "wasm32"))]
#[allow(unused_imports)]
pub(crate) use crate::state::{
    molt_profile_enabled, molt_profile_handle_resolve, molt_profile_snapshot,
    molt_profile_struct_field_store,
};
pub(crate) use crate::utils::usize_from_bits;

#[cfg(target_arch = "wasm32")]
#[link(wasm_import_module = "env")]
unsafe extern "C" {
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
    #[link_name = "molt_db_host_poll"]
    fn molt_db_host_poll() -> i32;
    #[link_name = "molt_getpid_host"]
    fn molt_getpid_host() -> i64;
    #[link_name = "molt_time_timezone_host"]
    pub(crate) fn molt_time_timezone_host() -> i64;
    #[link_name = "molt_time_local_offset_host"]
    pub(crate) fn molt_time_local_offset_host(secs: i64) -> i64;
    #[link_name = "molt_time_tzname_host"]
    pub(crate) fn molt_time_tzname_host(
        which: i32,
        buf_ptr: u32,
        buf_cap: u32,
        out_len_ptr: u32,
    ) -> i32;
    #[link_name = "molt_os_close_host"]
    pub(crate) fn molt_os_close_host(fd: i64) -> i32;
    #[link_name = "molt_socket_new_host"]
    pub(crate) fn molt_socket_new_host(family: i32, sock_type: i32, proto: i32, fileno: i64)
    -> i64;
    #[link_name = "molt_socket_close_host"]
    pub(crate) fn molt_socket_close_host(handle: i64) -> i32;
    #[link_name = "molt_socket_clone_host"]
    pub(crate) fn molt_socket_clone_host(handle: i64) -> i64;
    #[link_name = "molt_socket_bind_host"]
    pub(crate) fn molt_socket_bind_host(handle: i64, addr_ptr: u32, addr_len: u32) -> i32;
    #[link_name = "molt_socket_listen_host"]
    pub(crate) fn molt_socket_listen_host(handle: i64, backlog: i32) -> i32;
    #[link_name = "molt_socket_accept_host"]
    pub(crate) fn molt_socket_accept_host(
        handle: i64,
        addr_ptr: u32,
        addr_cap: u32,
        out_len_ptr: u32,
    ) -> i64;
    #[link_name = "molt_socket_connect_host"]
    pub(crate) fn molt_socket_connect_host(handle: i64, addr_ptr: u32, addr_len: u32) -> i32;
    #[link_name = "molt_socket_connect_ex_host"]
    pub(crate) fn molt_socket_connect_ex_host(handle: i64) -> i32;
    #[link_name = "molt_socket_recv_host"]
    pub(crate) fn molt_socket_recv_host(handle: i64, buf_ptr: u32, buf_len: u32, flags: i32)
    -> i32;
    #[link_name = "molt_socket_send_host"]
    pub(crate) fn molt_socket_send_host(handle: i64, buf_ptr: u32, buf_len: u32, flags: i32)
    -> i32;
    #[link_name = "molt_socket_sendto_host"]
    pub(crate) fn molt_socket_sendto_host(
        handle: i64,
        buf_ptr: u32,
        buf_len: u32,
        flags: i32,
        addr_ptr: u32,
        addr_len: u32,
    ) -> i32;
    #[link_name = "molt_socket_sendmsg_host"]
    pub(crate) fn molt_socket_sendmsg_host(
        handle: i64,
        buf_ptr: u32,
        buf_len: u32,
        flags: i32,
        addr_ptr: u32,
        addr_len: u32,
        anc_ptr: u32,
        anc_len: u32,
    ) -> i32;
    #[link_name = "molt_socket_recvfrom_host"]
    pub(crate) fn molt_socket_recvfrom_host(
        handle: i64,
        buf_ptr: u32,
        buf_len: u32,
        flags: i32,
        addr_ptr: u32,
        addr_cap: u32,
        out_len_ptr: u32,
    ) -> i32;
    #[link_name = "molt_socket_recvmsg_host"]
    pub(crate) fn molt_socket_recvmsg_host(
        handle: i64,
        buf_ptr: u32,
        buf_len: u32,
        flags: i32,
        addr_ptr: u32,
        addr_cap: u32,
        out_addr_len_ptr: u32,
        anc_ptr: u32,
        anc_cap: u32,
        out_anc_len_ptr: u32,
        out_msg_flags_ptr: u32,
    ) -> i32;
    #[link_name = "molt_socket_shutdown_host"]
    pub(crate) fn molt_socket_shutdown_host(handle: i64, how: i32) -> i32;
    #[link_name = "molt_socket_getsockname_host"]
    pub(crate) fn molt_socket_getsockname_host(
        handle: i64,
        addr_ptr: u32,
        addr_cap: u32,
        out_len_ptr: u32,
    ) -> i32;
    #[link_name = "molt_socket_getpeername_host"]
    pub(crate) fn molt_socket_getpeername_host(
        handle: i64,
        addr_ptr: u32,
        addr_cap: u32,
        out_len_ptr: u32,
    ) -> i32;
    #[link_name = "molt_socket_setsockopt_host"]
    pub(crate) fn molt_socket_setsockopt_host(
        handle: i64,
        level: i32,
        optname: i32,
        val_ptr: u32,
        val_len: u32,
    ) -> i32;
    #[link_name = "molt_socket_getsockopt_host"]
    pub(crate) fn molt_socket_getsockopt_host(
        handle: i64,
        level: i32,
        optname: i32,
        val_ptr: u32,
        val_len: u32,
        out_len_ptr: u32,
    ) -> i32;
    #[link_name = "molt_socket_detach_host"]
    pub(crate) fn molt_socket_detach_host(handle: i64) -> i64;
    #[link_name = "molt_socket_socketpair_host"]
    pub(crate) fn molt_socket_socketpair_host(
        family: i32,
        sock_type: i32,
        proto: i32,
        out_left_ptr: u32,
        out_right_ptr: u32,
    ) -> i32;
    #[link_name = "molt_socket_getaddrinfo_host"]
    pub(crate) fn molt_socket_getaddrinfo_host(
        host_ptr: u32,
        host_len: u32,
        serv_ptr: u32,
        serv_len: u32,
        family: i32,
        sock_type: i32,
        proto: i32,
        flags: i32,
        out_ptr: u32,
        out_cap: u32,
        out_len_ptr: u32,
    ) -> i32;
    #[link_name = "molt_socket_gethostname_host"]
    pub(crate) fn molt_socket_gethostname_host(buf_ptr: u32, buf_cap: u32, out_len_ptr: u32)
    -> i32;
    #[link_name = "molt_socket_getservbyname_host"]
    pub(crate) fn molt_socket_getservbyname_host(
        name_ptr: u32,
        name_len: u32,
        proto_ptr: u32,
        proto_len: u32,
    ) -> i32;
    #[link_name = "molt_socket_getservbyport_host"]
    pub(crate) fn molt_socket_getservbyport_host(
        port: i32,
        proto_ptr: u32,
        proto_len: u32,
        buf_ptr: u32,
        buf_cap: u32,
        out_len_ptr: u32,
    ) -> i32;
    #[link_name = "molt_socket_poll_host"]
    pub(crate) fn molt_socket_poll_host(handle: i64, events: u32) -> i32;
    #[link_name = "molt_socket_wait_host"]
    pub(crate) fn molt_socket_wait_host(handle: i64, events: u32, timeout_ms: i64) -> i32;
    #[link_name = "molt_socket_has_ipv6_host"]
    pub(crate) fn molt_socket_has_ipv6_host() -> i32;
    #[link_name = "molt_ws_connect_host"]
    pub(crate) fn molt_ws_connect_host(url_ptr: u32, url_len: u64, out_handle: *mut i64) -> i32;
    #[link_name = "molt_ws_poll_host"]
    pub(crate) fn molt_ws_poll_host(handle: i64, events: u32) -> i32;
    #[link_name = "molt_ws_send_host"]
    pub(crate) fn molt_ws_send_host(handle: i64, data_ptr: *const u8, len: u64) -> i32;
    #[link_name = "molt_ws_recv_host"]
    pub(crate) fn molt_ws_recv_host(
        handle: i64,
        buf_ptr: *mut u8,
        buf_cap: u32,
        out_len_ptr: *mut u32,
    ) -> i32;
    #[link_name = "molt_ws_close_host"]
    pub(crate) fn molt_ws_close_host(handle: i64) -> i32;
    #[link_name = "molt_process_spawn_host"]
    pub(crate) fn molt_process_spawn_host(
        args_ptr: u32,
        args_len: u32,
        env_ptr: u32,
        env_len: u32,
        cwd_ptr: u32,
        cwd_len: u32,
        stdin_mode: i32,
        stdout_mode: i32,
        stderr_mode: i32,
        out_handle: *mut i64,
    ) -> i32;
    #[link_name = "molt_process_wait_host"]
    pub(crate) fn molt_process_wait_host(handle: i64, timeout_ms: i64, out_code: *mut i32) -> i32;
    #[link_name = "molt_process_kill_host"]
    pub(crate) fn molt_process_kill_host(handle: i64) -> i32;
    #[link_name = "molt_process_terminate_host"]
    pub(crate) fn molt_process_terminate_host(handle: i64) -> i32;
    #[link_name = "molt_process_write_host"]
    pub(crate) fn molt_process_write_host(handle: i64, data_ptr: *const u8, len: u64) -> i32;
    #[link_name = "molt_process_close_stdin_host"]
    pub(crate) fn molt_process_close_stdin_host(handle: i64) -> i32;
    #[link_name = "molt_process_stdio_host"]
    pub(crate) fn molt_process_stdio_host(handle: i64, which: i32, out_stream: *mut u64) -> i32;
    #[link_name = "molt_process_host_poll"]
    pub(crate) fn molt_process_host_poll() -> i32;
    #[link_name = "molt_gpu_webgpu_dispatch_host"]
    pub(crate) fn molt_gpu_webgpu_dispatch_host(
        source_ptr: u32,
        source_len: u32,
        entry_ptr: u32,
        entry_len: u32,
        bindings_ptr: u32,
        bindings_len: u32,
        grid: u32,
        workgroup_size: u32,
        err_ptr: u32,
        err_cap: u32,
        out_err_len_ptr: *mut u32,
    ) -> i32;
}

// (file handle helpers moved to runtime/molt-runtime/src/builtins/io.rs)

// Builtin method helpers moved to runtime/molt-runtime/src/builtins/methods.rs.

// Function/object constructors moved to runtime/molt-runtime/src/builtins/functions.rs.

// Module helpers moved to runtime/molt-runtime/src/builtins/modules.rs.

// Class/type/super/property helpers moved to runtime/molt-runtime/src/builtins/types.rs.

// Object field helpers moved to runtime/molt-runtime/src/object/accessors.rs.

// Module helpers moved to runtime/molt-runtime/src/builtins/modules.rs.

// Closure helpers moved to runtime/molt-runtime/src/builtins/functions.rs.

// Generator/task helpers moved to runtime/molt-runtime/src/async_rt/generators.rs.

// Callable helpers moved to runtime/molt-runtime/src/builtins/callable.rs.

// Generator/future helpers moved to runtime/molt-runtime/src/async_rt/generators.rs.

// Context manager FFI moved to runtime/molt-runtime/src/builtins/context.rs.

// --- File I/O ---
// (moved to runtime/molt-runtime/src/builtins/io.rs)

// --- Buffer2D ---
// (moved to runtime/molt-runtime/src/object/buffer2d.rs)
// --- Container alloc ---
// (moved to runtime/molt-runtime/src/builtins/containers_alloc.rs)

// --- Channels ---
// (moved to runtime/molt-runtime/src/async_rt/channels.rs)
// --- Sockets ---
// (moved to runtime/molt-runtime/src/async_rt/sockets.rs)
// --- Process ---
// (moved to runtime/molt-runtime/src/async_rt/process.rs)

// --- IO Poller ---
// (moved to runtime/molt-runtime/src/async_rt/io_poller.rs)

// --- Thread/Process Tasks ---
// (moved to runtime/molt-runtime/src/async_rt/threads.rs)

// Cancel token FFI moved to runtime/molt-runtime/src/async_rt/cancellation.rs.

// Spawn/block_on FFI moved to runtime/molt-runtime/src/async_rt/scheduler.rs.

// String/bytes FFI moved to runtime/molt-runtime/src/builtins/strings.rs.

// errno/socket/env helpers moved to runtime/molt-runtime/src/builtins/io.rs.

// Class construction helpers moved to runtime/molt-runtime/src/call/class_init.rs.

// Attribute accessors moved to builtins/attributes.rs.

mod arena;
