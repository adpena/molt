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

#[cfg(any(molt_has_net_io, target_arch = "wasm32"))]
pub(crate) fn iter_values_from_bits(
    _py: &PyToken<'_>,
    iterable_bits: u64,
) -> Result<Vec<u64>, u64> {
    let iter_bits = crate::molt_iter(iterable_bits);
    if exception_pending(_py) {
        return Err(MoltObject::none().bits());
    }
    let mut out: Vec<u64> = Vec::new();
    loop {
        let pair_bits = crate::molt_iter_next(iter_bits);
        let Some(pair_ptr) = maybe_ptr_from_bits(pair_bits) else {
            return Err(MoltObject::none().bits());
        };
        unsafe {
            if object_type_id(pair_ptr) != TYPE_ID_TUPLE {
                return Err(raise_exception::<u64>(
                    _py,
                    "RuntimeError",
                    "iterator protocol violation",
                ));
            }
        }
        let Some(pair) = (unsafe {
            crate::object::seq_access::snapshot(
                _py,
                pair_ptr,
                "iterator pair snapshot allocation failed",
            )
        }) else {
            return Err(MoltObject::none().bits());
        };
        if pair.len() < 2 {
            return Err(raise_exception::<u64>(
                _py,
                "RuntimeError",
                "iterator protocol violation",
            ));
        }
        if is_truthy(_py, obj_from_bits(pair[1])) {
            break;
        }
        out.push(pair[0]);
    }
    Ok(out)
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
