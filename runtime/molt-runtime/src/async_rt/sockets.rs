use crate::PyToken;
use crate::audit::AuditArgs;
use crate::*;

// Re-export network utilities so that `sockets::*` includes them
#[cfg(not(any(molt_has_net_io, target_arch = "wasm32")))]
#[allow(unused_imports)]
pub use super::net_stubs::{
    molt_socket_reader_at_eof, molt_socket_reader_drop, molt_socket_reader_new,
    molt_socket_reader_read, molt_socket_reader_readline, molt_socket_reader_readline_limit,
};

#[cfg(any(molt_has_net_io, target_arch = "wasm32"))]
mod target_capability;
#[cfg(target_arch = "wasm32")]
mod wasm;
#[allow(unused_imports)]
#[cfg(any(molt_has_net_io, target_arch = "wasm32"))]
pub use super::sockets_net::*;
#[cfg(target_arch = "wasm32")]
pub use wasm::*;

#[cfg(target_arch = "wasm32")]
use crate::libc_compat as libc;
#[cfg(molt_has_net_io)]
use socket2::{Domain, Protocol, SockAddr, Socket, Type};
use std::ffi::OsString;
#[cfg(any(molt_has_net_io, target_arch = "wasm32"))]
use std::io::ErrorKind;
#[cfg(all(molt_has_net_io, unix))]
use std::os::raw::c_int;
#[cfg(molt_has_net_io)]
use std::os::raw::c_void;
#[cfg(all(molt_has_net_io, unix))]
use std::os::unix::io::{AsRawFd, FromRawFd, IntoRawFd, RawFd};
#[cfg(all(molt_has_net_io, windows))]
use std::os::windows::io::{AsRawSocket, FromRawSocket, IntoRawSocket, RawSocket};
#[cfg(molt_has_net_io)]
use std::sync::atomic::Ordering as AtomicOrdering;
#[cfg(any(molt_has_net_io, target_arch = "wasm32"))]
use std::time::Duration;

#[cfg(any(molt_has_net_io, target_arch = "wasm32"))]
mod ancillary;
#[cfg(all(molt_has_net_io, not(unix)))]
use ancillary::socket_clip_ancillary_for_bufsize;
#[cfg(any(molt_has_net_io, target_arch = "wasm32"))]
use ancillary::{
    build_ancillary_list_bits, build_recvmsg_result_with_anc, collect_recvmsg_into_targets,
    collect_sendmsg_payload, parse_sendmsg_ancillary_items, write_recvmsg_into_targets,
};
#[cfg(target_arch = "wasm32")]
use ancillary::{decode_host_recvmsg_ancillary_buffer, encode_host_sendmsg_ancillary_buffer};
#[cfg(all(molt_has_net_io, unix))]
use ancillary::{encode_sendmsg_ancillary_buffer, parse_recvmsg_ancillary_items};

#[cfg(any(molt_has_net_io, target_arch = "wasm32"))]
mod address;
#[cfg(target_arch = "wasm32")]
pub(crate) use address::decode_sockaddr;
#[cfg(target_arch = "wasm32")]
use address::encode_sockaddr;
#[cfg(any(molt_has_net_io, target_arch = "wasm32"))]
pub(crate) use address::{host_from_bits, port_from_bits, service_from_bits};
#[cfg(molt_has_net_io)]
pub(crate) use address::{sockaddr_from_bits, sockaddr_to_bits};

#[cfg(any(molt_has_net_io, target_arch = "wasm32"))]
mod state;
#[cfg(all(molt_has_net_io, not(unix)))]
pub(crate) use state::socket_register_peer_pair;
#[cfg(molt_has_net_io)]
use state::{
    MoltSocket, MoltSocketKind, socket_alloc, socket_close_ptr, socket_debug_fd, socket_detach_raw,
    socket_ref_dec, socket_set_timeout, trace_socket_recv, trace_socket_send,
};
#[cfg(any(molt_has_net_io, target_arch = "wasm32"))]
pub(crate) use state::{SocketRuntimeState, socket_runtime_state_clear};
#[cfg(target_arch = "wasm32")]
pub(crate) use state::{WasmSocketMeta, socket_timeout, wasm_socket_meta_insert};
#[cfg(target_arch = "wasm32")]
use state::{
    socket_connect_pending, socket_set_connect_pending, socket_set_timeout, wasm_socket_family,
    wasm_socket_meta_clone, wasm_socket_meta_remove,
};
#[cfg(all(molt_has_net_io, not(unix)))]
use state::{
    socket_enqueue_stream_ancillary, socket_peer_available, socket_take_stream_ancillary,
    socket_unregister_peer_state,
};
#[cfg(molt_has_net_io)]
pub(crate) use state::{
    socket_ptr_from_bits_or_fd, socket_ref_inc, socket_timeout, with_socket_mut,
};

#[cfg(molt_has_net_io)]
mod raw;
#[cfg(molt_has_net_io)]
use raw::{connect_raw_socket, socket_is_acceptor, socket_relisten, take_error_mio, with_sockref};
#[cfg(molt_has_net_io)]
pub(crate) use raw::{libc_socket, sock_addr_from_storage};

#[cfg(any(molt_has_net_io, target_arch = "wasm32"))]
mod wait;
#[cfg(all(molt_has_net_io, windows))]
pub(crate) use raw::{socket_close_raw_windows, socketpair_windows_loopback_raw};
#[cfg(target_arch = "wasm32")]
pub(crate) use wait::errno_from_rc;
#[cfg(any(molt_has_net_io, target_arch = "wasm32"))]
pub(crate) use wait::socket_wait_ready;
#[cfg(target_arch = "wasm32")]
use wait::would_block_errno;

#[cfg(any(molt_has_net_io, target_arch = "wasm32"))]
mod reader;
#[cfg(any(molt_has_net_io, target_arch = "wasm32"))]
pub use reader::{
    molt_socket_reader_at_eof, molt_socket_reader_drop, molt_socket_reader_new,
    molt_socket_reader_read, molt_socket_reader_readline, molt_socket_reader_readline_limit,
};
// --- Sockets ---

pub(crate) enum SendData {
    Borrowed(*const u8, usize),
    Owned(Vec<u8>),
}

#[cfg(molt_has_net_io)]
pub(crate) fn io_wait_detach_resource(future_ptr: *mut u8) -> u64 {
    if future_ptr.is_null() {
        return MoltObject::none().bits();
    }
    let _header = unsafe { header_from_obj_ptr(future_ptr) };
    let payload_bytes = unsafe { crate::object::object_payload_size(future_ptr) };
    if payload_bytes < std::mem::size_of::<u64>() {
        return MoltObject::none().bits();
    }
    let payload_ptr = future_ptr as *mut u64;
    unsafe { payload_ptr.replace(MoltObject::none().bits()) }
}

#[cfg(molt_has_net_io)]
pub(crate) fn io_wait_release_detached_resource(_py: &PyToken<'_>, socket_bits: u64) {
    let socket_ptr = socket_ptr_from_bits_or_fd(socket_bits);
    if !socket_ptr.is_null() {
        socket_ref_dec(_py, socket_ptr);
    }
}

#[cfg(target_arch = "wasm32")]
pub(crate) fn io_wait_detach_resource(future_ptr: *mut u8) -> u64 {
    if future_ptr.is_null() {
        return MoltObject::none().bits();
    }
    let payload_bytes = unsafe { crate::object::object_payload_size(future_ptr) };
    if payload_bytes < std::mem::size_of::<u64>() {
        return MoltObject::none().bits();
    }
    let payload_ptr = future_ptr as *mut u64;
    unsafe { payload_ptr.replace(MoltObject::none().bits()) }
}

#[cfg(target_arch = "wasm32")]
pub(crate) fn io_wait_release_detached_resource(_py: &PyToken<'_>, _resource_bits: u64) {}

pub(crate) fn send_data_from_bits(bits: u64) -> Result<SendData, String> {
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
            if memoryview_released(ptr) {
                return Err(RELEASED_MEMORYVIEW_ERROR.to_string());
            }
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

#[cfg(any(test, molt_has_net_io, target_arch = "wasm32"))]
pub(crate) fn iter_values_from_bits<'a, 'py>(
    _py: &'a PyToken<'py>,
    iterable_bits: u64,
) -> Result<crate::object::seq_access::PinnedSequenceSnapshot<'a, 'py>, u64> {
    // The shared unboxed iterator authority owns the iterator and each item,
    // including unwind after a later __next__ callback fails. Socket consumers
    // must not reconstruct its boxed-pair protocol or return borrowed values.
    let values = crate::object::iterable::collect(
        _py,
        iterable_bits,
        crate::object::iterable::LengthHint::Skip,
    )
    .ok_or_else(|| MoltObject::none().bits())?;
    let Some(owned) = crate::object::backing::tracked_vec_box_from_slice(&values, values.len())
    else {
        for bits in values {
            dec_ref_bits(_py, bits);
        }
        return Err(raise_exception::<u64>(
            _py,
            "MemoryError",
            "socket iterable snapshot allocation failed",
        ));
    };
    // Transfer, rather than duplicate, the collected references. Dropping the
    // temporary Vec only frees its scalar storage; the snapshot owns each item.
    Ok(
        crate::object::seq_access::PinnedSequenceSnapshot::from_owned_values(_py, unsafe {
            crate::object::backing::tracked_vec_box_from_raw(owned)
        }),
    )
}

#[cfg(target_arch = "wasm32")]
fn socket_handle_from_bits(_py: &PyToken<'_>, bits: u64) -> Result<i64, String> {
    let obj = obj_from_bits(bits);
    if obj.is_none() {
        return Err("invalid socket".to_string());
    }
    if let Some(val) = to_i64(obj) {
        if val < 0 {
            return Err("invalid socket".to_string());
        }
        return Ok(val);
    }
    let obj_type = class_name_for_error(type_of_bits(_py, bits));
    Err(format!("socket handle must be int, not {obj_type}"))
}

pub(crate) fn require_time_wall_capability<T: ExceptionSentinel>(
    _py: &PyToken<'_>,
    operation: OperationId,
) -> Result<(), T> {
    require_operation(_py, operation, AuditArgs::None)
}

// Native no-net keeps this symbol for the existing crate-root helper surface;
// net/wasm feature lanes call it directly from socket/channel operations.
#[allow(dead_code)]
pub(crate) fn require_net_capability<T: ExceptionSentinel>(
    _py: &PyToken<'_>,
    operation: OperationId,
) -> Result<(), T> {
    require_operation(_py, operation, AuditArgs::None)
}

pub(crate) fn require_process_capability<T: ExceptionSentinel>(
    _py: &PyToken<'_>,
    operation: OperationId,
) -> Result<(), T> {
    require_operation(_py, operation, AuditArgs::None)
}

#[cfg(not(target_arch = "wasm32"))]
fn os_string_from_bits(_py: &PyToken<'_>, bits: u64) -> Result<OsString, String> {
    let path = path_from_bits(_py, bits)?;
    Ok(path.into_os_string())
}

#[cfg(not(target_arch = "wasm32"))]
pub(crate) fn argv_from_bits(_py: &PyToken<'_>, args_bits: u64) -> Result<Vec<OsString>, String> {
    let obj = obj_from_bits(args_bits);
    if obj.is_none() {
        return Err("args must be a sequence".to_string());
    }
    if let Some(ptr) = obj.as_ptr() {
        let type_id = unsafe { object_type_id(ptr) };
        if type_id == TYPE_ID_LIST || type_id == TYPE_ID_TUPLE {
            let Some(elems) = (unsafe {
                crate::object::seq_access::snapshot(
                    _py,
                    ptr,
                    "socket argument snapshot allocation failed",
                )
            }) else {
                return Err("socket argument snapshot allocation failed".to_string());
            };
            let mut args = Vec::with_capacity(elems.len());
            for &elem in elems.iter() {
                args.push(os_string_from_bits(_py, elem)?);
            }
            return Ok(args);
        }
    }
    Ok(vec![os_string_from_bits(_py, args_bits)?])
}

#[cfg(not(target_arch = "wasm32"))]
pub(crate) fn env_from_bits(
    _py: &PyToken<'_>,
    env_bits: u64,
) -> Result<Option<Vec<(OsString, OsString)>>, String> {
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
                os_string_from_bits(_py, key_bits)?,
                os_string_from_bits(_py, val_bits)?,
            ));
            idx += 2;
        }
        Ok(Some(out))
    }
}

#[cfg(molt_has_net_io)]
mod io_ops;
#[cfg(molt_has_net_io)]
pub use io_ops::*;

#[cfg(molt_has_net_io)]
mod ops;
#[cfg(molt_has_net_io)]
pub use ops::*;

#[cfg(test)]
mod iterable_ownership_tests {
    use super::*;
    use std::sync::atomic::{AtomicBool, AtomicU64, AtomicUsize, Ordering};

    static ITEM: AtomicU64 = AtomicU64::new(0);
    static SOURCE: AtomicU64 = AtomicU64::new(0);
    static CALLS: AtomicUsize = AtomicUsize::new(0);
    static FAIL: AtomicBool = AtomicBool::new(false);

    fn refcount(bits: u64) -> u32 {
        unsafe {
            (*header_from_obj_ptr(obj_from_bits(bits).as_ptr().unwrap())).ref_count_snapshot()
        }
    }

    extern "C" fn yield_then_mutate_source() -> u64 {
        crate::with_gil_entry_nopanic!(py, {
            let item = ITEM.load(Ordering::Relaxed);
            if CALLS.fetch_add(1, Ordering::Relaxed) == 0 {
                inc_ref_bits(py, item);
                return item;
            }
            crate::molt_list_clear(SOURCE.load(Ordering::Relaxed));
            // Local test reference plus the collected item survive the later
            // callback dropping the original source's edge.
            assert!(refcount(item) >= 2);
            if FAIL.load(Ordering::Relaxed) {
                raise_exception::<u64>(py, "LookupError", "injected socket iterator failure")
            } else {
                MoltObject::from_int(777).bits()
            }
        })
    }

    #[test]
    fn buffer_export_contract_socket_iterable_pins_items_and_unwinds_callback_failure() {
        let _transaction = crate::test_support::RuntimeTestTransaction::new();
        crate::with_gil_entry_nopanic!(py, {
            for fail in [false, true] {
                let item = MoltObject::from_ptr(alloc_bytearray(py, b"payload")).bits();
                let source = MoltObject::from_ptr(alloc_list(py, &[item])).bits();
                let callback = crate::object::builders::alloc_function_obj(
                    py,
                    crate::provenance::abi::expose_function_address(
                        yield_then_mutate_source as *const (),
                    ),
                    0,
                );
                assert!(!callback.is_null());
                unsafe {
                    crate::object::layout::function_set_call_target_ptr(
                        callback,
                        yield_then_mutate_source as *const (),
                    );
                }
                let callback = MoltObject::from_ptr(callback).bits();
                let iterator =
                    crate::molt_iter_sentinel(callback, MoltObject::from_int(777).bits());
                assert!(!exception_pending(py));
                ITEM.store(item, Ordering::Relaxed);
                SOURCE.store(source, Ordering::Relaxed);
                CALLS.store(0, Ordering::Relaxed);
                FAIL.store(fail, Ordering::Relaxed);
                let iterator_refs = refcount(iterator);
                let values = iter_values_from_bits(py, iterator);
                assert_eq!(CALLS.load(Ordering::Relaxed), 2);
                assert_eq!(
                    refcount(iterator),
                    iterator_refs,
                    "iterator owner must be balanced"
                );
                assert!(
                    unsafe {
                        crate::object::layout::call_iter_cached_tuple(
                            obj_from_bits(iterator).as_ptr().unwrap(),
                        )
                    }
                    .is_null()
                );
                if fail {
                    assert!(values.is_err());
                    let error = crate::builtins::exceptions::molt_exception_last_pending();
                    assert!(crate::builtins::exceptions::exception_matches_builtin_name(
                        py,
                        error,
                        "LookupError"
                    ));
                    clear_exception(py);
                    dec_ref_bits(py, error);
                    assert_eq!(
                        refcount(item),
                        1,
                        "yielded item must unwind on a later failure"
                    );
                } else {
                    let values = values.expect("owned socket iterable");
                    assert_eq!(&*values, &[item]);
                    assert_eq!(refcount(item), 2, "snapshot owns exactly one item edge");
                    dec_ref_bits(py, source);
                    assert_eq!(
                        unsafe {
                            bytes_like_slice_raw(obj_from_bits(values[0]).as_ptr().unwrap())
                                .unwrap()
                        },
                        b"payload"
                    );
                    drop(values);
                    assert_eq!(refcount(item), 1, "snapshot drop must release its item");
                }
                if fail {
                    dec_ref_bits(py, source);
                }
                for bits in [iterator, callback, item] {
                    dec_ref_bits(py, bits);
                }
                assert!(!exception_pending(py));
            }
            ITEM.store(0, Ordering::Relaxed);
            SOURCE.store(0, Ordering::Relaxed);
        });
    }

    #[test]
    fn buffer_export_contract_socket_iterable_snapshot_allocation_failure_unwinds() {
        let _transaction = crate::test_support::RuntimeTestTransaction::new();
        crate::with_gil_entry_nopanic!(py, {
            use crate::resource::{LimitedTracker, ResourceLimits, UnlimitedTracker, set_tracker};
            let item = MoltObject::from_ptr(alloc_bytearray(py, b"payload")).bits();
            let source = MoltObject::from_ptr(alloc_list(py, &[item])).bits();
            let iterator = crate::molt_iter(source);
            assert!(!exception_pending(py));
            let iterator_refs = refcount(iterator);
            set_tracker(Box::new(LimitedTracker::new(&ResourceLimits {
                max_allocations: Some(0),
                ..Default::default()
            })));
            let values = iter_values_from_bits(py, iterator);
            set_tracker(Box::new(UnlimitedTracker));
            assert!(values.is_err());
            clear_exception(py);
            assert_eq!(refcount(iterator), iterator_refs);
            assert_eq!(
                refcount(source),
                1,
                "exhaustion releases the iterator target"
            );
            assert_eq!(
                refcount(item),
                2,
                "failed snapshot releases collected item refs"
            );
            crate::molt_list_clear(source);
            assert_eq!(refcount(item), 1);
            for bits in [iterator, source, item] {
                dec_ref_bits(py, bits);
            }
            assert!(!exception_pending(py));
        });
    }
}
