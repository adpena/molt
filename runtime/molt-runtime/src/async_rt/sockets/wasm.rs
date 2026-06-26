use crate::PyToken;
use crate::libc_compat as libc;
use crate::*;
use std::io::ErrorKind;
use std::time::Duration;

use super::address::{decode_sockaddr, encode_sockaddr};
use super::ancillary::{
    build_ancillary_list_bits, build_recvmsg_result_with_anc, collect_recvmsg_into_targets,
    collect_sendmsg_payload, decode_host_recvmsg_ancillary_buffer,
    encode_host_sendmsg_ancillary_buffer, parse_sendmsg_ancillary_items,
    write_recvmsg_into_targets,
};
use super::state::{
    WasmSocketMeta, socket_connect_pending, socket_set_connect_pending, socket_set_timeout,
    socket_timeout, wasm_socket_family, wasm_socket_meta_clone, wasm_socket_meta_insert,
    wasm_socket_meta_remove,
};
use super::wait::{errno_from_rc, socket_wait_ready, would_block_errno};
use super::{SendData, require_net_capability, send_data_from_bits, socket_handle_from_bits};

#[cfg(target_arch = "wasm32")]
#[unsafe(no_mangle)]
pub extern "C" fn molt_socket_clone(_sock_bits: u64) -> u64 {
    crate::with_gil_entry_nopanic!(_py, {
        let handle = match socket_handle_from_bits(_py, _sock_bits) {
            Ok(val) => val,
            Err(msg) => return raise_exception::<_>(_py, "TypeError", &msg),
        };
        let new_handle = unsafe { crate::molt_socket_clone_host(handle) };
        if new_handle < 0 {
            return raise_os_error_errno::<u64>(_py, (-new_handle) as i64, "socket.clone");
        }
        let meta = wasm_socket_meta_clone(handle);
        if let Some(meta) = meta {
            wasm_socket_meta_insert(new_handle, meta);
        }
        MoltObject::from_int(new_handle).bits()
    })
}

#[cfg(target_arch = "wasm32")]
#[allow(dead_code)]
fn wasm_socket_unavailable<T: ExceptionSentinel>(_py: &PyToken<'_>) -> T {
    raise_exception(_py, "RuntimeError", "socket unsupported on wasm")
}

#[cfg(target_arch = "wasm32")]
#[unsafe(no_mangle)]
pub extern "C" fn molt_socket_new(
    _family_bits: u64,
    _type_bits: u64,
    _proto_bits: u64,
    _fileno_bits: u64,
) -> u64 {
    crate::with_gil_entry_nopanic!(_py, {
        if require_net_capability::<u64>(_py, &["net", "net.connect", "net.listen", "net.bind"])
            .is_err()
        {
            return MoltObject::none().bits();
        }
        let family = match to_i64(obj_from_bits(_family_bits)) {
            Some(val) => val as i32,
            None => return raise_exception::<_>(_py, "TypeError", "family must be int"),
        };
        let sock_type = if obj_from_bits(_type_bits).is_none() {
            libc::SOCK_STREAM
        } else {
            match to_i64(obj_from_bits(_type_bits)) {
                Some(val) => val as i32,
                None => return raise_exception::<_>(_py, "TypeError", "type must be int"),
            }
        };
        let proto = if obj_from_bits(_proto_bits).is_none() {
            0
        } else {
            match to_i64(obj_from_bits(_proto_bits)) {
                Some(val) => val as i32,
                None => return raise_exception::<_>(_py, "TypeError", "proto must be int"),
            }
        };
        let fileno = if obj_from_bits(_fileno_bits).is_none() {
            -1
        } else {
            match to_i64(obj_from_bits(_fileno_bits)) {
                Some(val) => val,
                None => {
                    return raise_exception::<_>(_py, "TypeError", "fileno must be int or None");
                }
            }
        };
        #[cfg(unix)]
        let base_type = sock_type & !(SOCK_NONBLOCK_FLAG | SOCK_CLOEXEC_FLAG);
        #[cfg(not(unix))]
        let base_type = sock_type;
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
        let handle = unsafe { crate::molt_socket_new_host(family, base_type, proto, fileno) };
        if handle < 0 {
            return raise_os_error_errno::<u64>(_py, (-handle) as i64, "socket");
        }
        wasm_socket_meta_insert(
            handle,
            WasmSocketMeta::new(family, base_type, proto, timeout),
        );
        MoltObject::from_int(handle).bits()
    })
}

#[cfg(target_arch = "wasm32")]
#[unsafe(no_mangle)]
pub extern "C" fn molt_socket_close(_sock_bits: u64) -> u64 {
    crate::with_gil_entry_nopanic!(_py, {
        let handle = match socket_handle_from_bits(_py, _sock_bits) {
            Ok(val) => val,
            Err(msg) => return raise_exception::<_>(_py, "TypeError", &msg),
        };
        let rc = unsafe { crate::molt_socket_close_host(handle) };
        wasm_socket_meta_remove(handle);
        if rc < 0 {
            return raise_os_error_errno::<u64>(_py, (-rc) as i64, "close");
        }
        MoltObject::none().bits()
    })
}

#[cfg(target_arch = "wasm32")]
#[unsafe(no_mangle)]
pub extern "C" fn molt_socket_drop(_sock_bits: u64) {
    crate::with_gil_entry_nopanic!(_py, {
        let handle = match socket_handle_from_bits(_py, _sock_bits) {
            Ok(val) => val,
            Err(_) => return,
        };
        let _ = unsafe { crate::molt_socket_close_host(handle) };
        wasm_socket_meta_remove(handle);
    })
}

#[cfg(target_arch = "wasm32")]
#[unsafe(no_mangle)]
pub extern "C" fn molt_socket_fileno(_sock_bits: u64) -> u64 {
    crate::with_gil_entry_nopanic!(_py, {
        let handle = match socket_handle_from_bits(_py, _sock_bits) {
            Ok(val) => val,
            Err(_) => return MoltObject::from_int(-1).bits(),
        };
        MoltObject::from_int(handle).bits()
    })
}

#[cfg(target_arch = "wasm32")]
#[unsafe(no_mangle)]
pub extern "C" fn molt_socket_gettimeout(_sock_bits: u64) -> u64 {
    crate::with_gil_entry_nopanic!(_py, {
        let handle = match socket_handle_from_bits(_py, _sock_bits) {
            Ok(val) => val,
            Err(_) => return MoltObject::none().bits(),
        };
        match socket_timeout(handle) {
            None => MoltObject::none().bits(),
            Some(val) => MoltObject::from_float(val.as_secs_f64()).bits(),
        }
    })
}

#[cfg(target_arch = "wasm32")]
#[unsafe(no_mangle)]
pub extern "C" fn molt_socket_settimeout(_sock_bits: u64, _timeout_bits: u64) -> u64 {
    crate::with_gil_entry_nopanic!(_py, {
        let handle = match socket_handle_from_bits(_py, _sock_bits) {
            Ok(val) => val,
            Err(msg) => return raise_exception::<_>(_py, "TypeError", &msg),
        };
        let obj = obj_from_bits(_timeout_bits);
        if obj.is_none() {
            let _ = socket_set_timeout(handle, None);
            return MoltObject::none().bits();
        }
        let Some(timeout) = to_f64(obj) else {
            return raise_exception::<_>(_py, "TypeError", "timeout must be float or None");
        };
        if !timeout.is_finite() || timeout < 0.0 {
            return raise_exception::<_>(_py, "ValueError", "timeout must be non-negative");
        }
        let duration = if timeout == 0.0 {
            Duration::ZERO
        } else {
            Duration::from_secs_f64(timeout)
        };
        if let Err(msg) = socket_set_timeout(handle, Some(duration)) {
            return raise_exception::<_>(_py, "RuntimeError", &msg);
        }
        MoltObject::none().bits()
    })
}

#[cfg(target_arch = "wasm32")]
#[unsafe(no_mangle)]
pub extern "C" fn molt_socket_setblocking(_sock_bits: u64, _flag_bits: u64) -> u64 {
    crate::with_gil_entry_nopanic!(_py, {
        let handle = match socket_handle_from_bits(_py, _sock_bits) {
            Ok(val) => val,
            Err(msg) => return raise_exception::<_>(_py, "TypeError", &msg),
        };
        let flag = obj_from_bits(_flag_bits).as_bool().unwrap_or(false);
        if flag {
            let _ = socket_set_timeout(handle, None);
        } else {
            let _ = socket_set_timeout(handle, Some(Duration::ZERO));
        }
        MoltObject::none().bits()
    })
}

#[cfg(target_arch = "wasm32")]
#[unsafe(no_mangle)]
pub extern "C" fn molt_socket_getblocking(_sock_bits: u64) -> u64 {
    crate::with_gil_entry_nopanic!(_py, {
        let handle = match socket_handle_from_bits(_py, _sock_bits) {
            Ok(val) => val,
            Err(_) => return MoltObject::from_bool(false).bits(),
        };
        let timeout = socket_timeout(handle);
        let blocking = match timeout {
            None => true,
            Some(val) if val == Duration::ZERO => false,
            Some(_) => true,
        };
        MoltObject::from_bool(blocking).bits()
    })
}

#[cfg(target_arch = "wasm32")]
#[unsafe(no_mangle)]
pub extern "C" fn molt_socket_bind(_sock_bits: u64, _addr_bits: u64) -> u64 {
    crate::with_gil_entry_nopanic!(_py, {
        if require_net_capability::<u64>(_py, &["net", "net.bind", "net"]).is_err() {
            return MoltObject::none().bits();
        }
        let handle = match socket_handle_from_bits(_py, _sock_bits) {
            Ok(val) => val,
            Err(msg) => return raise_exception::<_>(_py, "TypeError", &msg),
        };
        let family = match wasm_socket_family(handle) {
            Ok(val) => val,
            Err(msg) => return raise_exception::<_>(_py, "TypeError", &msg),
        };
        let addr = match encode_sockaddr(_py, _addr_bits, family) {
            Ok(val) => val,
            Err(msg) => return raise_exception::<_>(_py, "TypeError", &msg),
        };
        let rc = unsafe {
            crate::molt_socket_bind_host(handle, addr.as_ptr() as u32, addr.len() as u32)
        };
        if rc < 0 {
            return raise_os_error_errno::<u64>(_py, errno_from_rc(rc) as i64, "bind");
        }
        MoltObject::none().bits()
    })
}

#[cfg(target_arch = "wasm32")]
#[unsafe(no_mangle)]
pub extern "C" fn molt_socket_listen(_sock_bits: u64, _backlog_bits: u64) -> u64 {
    crate::with_gil_entry_nopanic!(_py, {
        if require_net_capability::<u64>(_py, &["net", "net.listen", "net"]).is_err() {
            return MoltObject::none().bits();
        }
        let handle = match socket_handle_from_bits(_py, _sock_bits) {
            Ok(val) => val,
            Err(msg) => return raise_exception::<_>(_py, "TypeError", &msg),
        };
        let backlog = to_i64(obj_from_bits(_backlog_bits)).unwrap_or(0) as i32;
        let rc = unsafe { crate::molt_socket_listen_host(handle, backlog) };
        if rc < 0 {
            return raise_os_error_errno::<u64>(_py, errno_from_rc(rc) as i64, "listen");
        }
        MoltObject::none().bits()
    })
}

#[cfg(target_arch = "wasm32")]
#[unsafe(no_mangle)]
pub extern "C" fn molt_socket_accept(_sock_bits: u64) -> u64 {
    crate::with_gil_entry_nopanic!(_py, {
        if require_net_capability::<u64>(_py, &["net", "net.listen", "net"]).is_err() {
            return MoltObject::none().bits();
        }
        let handle = match socket_handle_from_bits(_py, _sock_bits) {
            Ok(val) => val,
            Err(msg) => return raise_exception::<_>(_py, "TypeError", &msg),
        };
        let family = match wasm_socket_family(handle) {
            Ok(val) => val,
            Err(msg) => return raise_exception::<_>(_py, "TypeError", &msg),
        };
        let timeout = socket_timeout(handle);
        let mut addr_buf = vec![0u8; 128];
        let mut addr_len: u32 = 0;
        loop {
            let rc = unsafe {
                crate::molt_socket_accept_host(
                    handle,
                    addr_buf.as_mut_ptr() as u32,
                    addr_buf.len() as u32,
                    (&mut addr_len) as *mut u32 as u32,
                )
            };
            if rc >= 0 {
                let new_handle = rc;
                wasm_socket_meta_insert(
                    new_handle,
                    WasmSocketMeta::new(family, libc::SOCK_STREAM, 0, timeout),
                );
                let addr_bits = match decode_sockaddr(_py, &addr_buf[..addr_len as usize]) {
                    Ok(bits) => bits,
                    Err(msg) => return raise_exception::<_>(_py, "TypeError", &msg),
                };
                let handle_bits = MoltObject::from_int(new_handle).bits();
                let tuple_ptr = alloc_tuple(_py, &[handle_bits, addr_bits]);
                if tuple_ptr.is_null() {
                    return MoltObject::none().bits();
                }
                return MoltObject::from_ptr(tuple_ptr).bits();
            }
            let errno = errno_from_rc(rc as i32);
            if would_block_errno(errno) {
                if let Some(val) = timeout
                    && val == Duration::ZERO
                {
                    return raise_os_error_errno::<u64>(
                        _py,
                        libc::EWOULDBLOCK as i64,
                        "accept would block",
                    );
                }
                if let Err(wait_err) = socket_wait_ready(_py, handle, IO_EVENT_READ) {
                    if wait_err.kind() == ErrorKind::TimedOut {
                        return raise_exception::<u64>(_py, "TimeoutError", "timed out");
                    }
                    if wait_err.kind() == ErrorKind::WouldBlock {
                        continue;
                    }
                    return raise_os_error::<u64>(_py, wait_err, "accept");
                }
                continue;
            }
            return raise_os_error_errno::<u64>(_py, errno as i64, "accept");
        }
    })
}

#[cfg(target_arch = "wasm32")]
#[unsafe(no_mangle)]
pub extern "C" fn molt_socket_connect(_sock_bits: u64, _addr_bits: u64) -> u64 {
    crate::with_gil_entry_nopanic!(_py, {
        if require_net_capability::<u64>(_py, &["net", "net.connect", "net"]).is_err() {
            return MoltObject::none().bits();
        }
        let handle = match socket_handle_from_bits(_py, _sock_bits) {
            Ok(val) => val,
            Err(msg) => return raise_exception::<_>(_py, "TypeError", &msg),
        };
        let family = match wasm_socket_family(handle) {
            Ok(val) => val,
            Err(msg) => return raise_exception::<_>(_py, "TypeError", &msg),
        };
        let addr = match encode_sockaddr(_py, _addr_bits, family) {
            Ok(val) => val,
            Err(msg) => return raise_exception::<_>(_py, "TypeError", &msg),
        };
        let timeout = socket_timeout(handle);
        let rc = unsafe {
            crate::molt_socket_connect_host(handle, addr.as_ptr() as u32, addr.len() as u32)
        };
        if rc == 0 {
            let _ = socket_set_connect_pending(handle, false);
            return MoltObject::none().bits();
        }
        let errno = errno_from_rc(rc);
        if errno == libc::EINPROGRESS || errno == libc::EWOULDBLOCK {
            let _ = socket_set_connect_pending(handle, true);
            if matches!(timeout, Some(val) if val == Duration::ZERO) {
                return raise_os_error_errno::<u64>(
                    _py,
                    libc::EINPROGRESS as i64,
                    "operation in progress",
                );
            }
            if let Err(wait_err) = socket_wait_ready(_py, handle, IO_EVENT_WRITE) {
                if wait_err.kind() == ErrorKind::TimedOut {
                    return raise_exception::<u64>(_py, "TimeoutError", "timed out");
                }
                return raise_os_error::<u64>(_py, wait_err, "connect");
            }
            let rc = unsafe { crate::molt_socket_connect_ex_host(handle) };
            if rc == 0 {
                let _ = socket_set_connect_pending(handle, false);
                return MoltObject::none().bits();
            }
            let err = errno_from_rc(rc);
            let _ = socket_set_connect_pending(handle, false);
            return raise_os_error_errno::<u64>(_py, err as i64, "connect");
        }
        raise_os_error_errno::<u64>(_py, errno as i64, "connect")
    })
}

#[cfg(target_arch = "wasm32")]
#[unsafe(no_mangle)]
pub extern "C" fn molt_socket_connect_ex(_sock_bits: u64, _addr_bits: u64) -> u64 {
    crate::with_gil_entry_nopanic!(_py, {
        if require_net_capability::<u64>(_py, &["net", "net.connect", "net"]).is_err() {
            return MoltObject::none().bits();
        }
        let handle = match socket_handle_from_bits(_py, _sock_bits) {
            Ok(val) => val,
            Err(_) => return MoltObject::from_int(libc::EBADF as i64).bits(),
        };
        let timeout = socket_timeout(handle);
        if socket_connect_pending(handle) {
            let rc = unsafe { crate::molt_socket_connect_ex_host(handle) };
            let errno = errno_from_rc(rc);
            if errno == 0 {
                let _ = socket_set_connect_pending(handle, false);
            }
            return MoltObject::from_int(errno as i64).bits();
        }
        let family = match wasm_socket_family(handle) {
            Ok(val) => val,
            Err(_) => return MoltObject::from_int(libc::EAFNOSUPPORT as i64).bits(),
        };
        let addr = match encode_sockaddr(_py, _addr_bits, family) {
            Ok(val) => val,
            Err(_) => return MoltObject::from_int(libc::EAFNOSUPPORT as i64).bits(),
        };
        let rc = unsafe {
            crate::molt_socket_connect_host(handle, addr.as_ptr() as u32, addr.len() as u32)
        };
        if rc == 0 {
            let _ = socket_set_connect_pending(handle, false);
            return MoltObject::from_int(0).bits();
        }
        let errno = errno_from_rc(rc);
        if errno == libc::EINPROGRESS || errno == libc::EWOULDBLOCK {
            let _ = socket_set_connect_pending(handle, true);
            if matches!(timeout, Some(val) if val == Duration::ZERO) {
                return MoltObject::from_int(errno as i64).bits();
            }
            if let Err(wait_err) = socket_wait_ready(_py, handle, IO_EVENT_WRITE) {
                if wait_err.kind() == ErrorKind::TimedOut {
                    return MoltObject::from_int(libc::ETIMEDOUT as i64).bits();
                }
                if wait_err.kind() == ErrorKind::WouldBlock {
                    return MoltObject::from_int(libc::EINPROGRESS as i64).bits();
                }
                return MoltObject::from_int(wait_err.raw_os_error().unwrap_or(libc::EIO) as i64)
                    .bits();
            }
            let rc = unsafe { crate::molt_socket_connect_ex_host(handle) };
            let err = errno_from_rc(rc);
            let _ = socket_set_connect_pending(handle, false);
            return MoltObject::from_int(err as i64).bits();
        }
        MoltObject::from_int(errno as i64).bits()
    })
}

#[cfg(target_arch = "wasm32")]
#[unsafe(no_mangle)]
pub extern "C" fn molt_socket_recv(_sock_bits: u64, _size_bits: u64, _flags_bits: u64) -> u64 {
    crate::with_gil_entry_nopanic!(_py, {
        let handle = match socket_handle_from_bits(_py, _sock_bits) {
            Ok(val) => val,
            Err(_) => return MoltObject::none().bits(),
        };
        let size = to_i64(obj_from_bits(_size_bits)).unwrap_or(0).max(0) as usize;
        let flags = to_i64(obj_from_bits(_flags_bits)).unwrap_or(0) as i32;
        #[cfg(unix)]
        let dontwait = (flags & libc::MSG_DONTWAIT) != 0;
        #[cfg(not(unix))]
        let dontwait = false;
        if size == 0 {
            let ptr = alloc_bytes(_py, &[]);
            if ptr.is_null() {
                return MoltObject::none().bits();
            }
            return MoltObject::from_ptr(ptr).bits();
        }
        let nonblocking = matches!(socket_timeout(handle), Some(val) if val == Duration::ZERO);
        let mut buf = vec![0u8; size];
        loop {
            let rc = unsafe {
                crate::molt_socket_recv_host(
                    handle,
                    buf.as_mut_ptr() as u32,
                    buf.len() as u32,
                    flags,
                )
            };
            if rc >= 0 {
                let n = rc as usize;
                let ptr = alloc_bytes(_py, &buf[..n]);
                if ptr.is_null() {
                    return MoltObject::none().bits();
                }
                return MoltObject::from_ptr(ptr).bits();
            }
            let errno = errno_from_rc(rc);
            if would_block_errno(errno) {
                if dontwait || nonblocking {
                    return raise_os_error_errno::<u64>(_py, errno as i64, "recv: would block");
                }
                if let Err(wait_err) = socket_wait_ready(_py, handle, IO_EVENT_READ) {
                    if wait_err.kind() == ErrorKind::TimedOut {
                        return raise_exception::<u64>(_py, "TimeoutError", "timed out");
                    }
                    if wait_err.kind() == ErrorKind::WouldBlock {
                        continue;
                    }
                    return raise_os_error::<u64>(_py, wait_err, "recv");
                }
                continue;
            }
            return raise_os_error_errno::<u64>(_py, errno as i64, "recv");
        }
    })
}

#[cfg(target_arch = "wasm32")]
#[unsafe(no_mangle)]
pub extern "C" fn molt_socket_recv_into(
    _sock_bits: u64,
    _buffer_bits: u64,
    _size_bits: u64,
    _flags_bits: u64,
) -> u64 {
    crate::with_gil_entry_nopanic!(_py, {
        let handle = match socket_handle_from_bits(_py, _sock_bits) {
            Ok(val) => val,
            Err(_) => return MoltObject::from_int(0).bits(),
        };
        let buffer_obj = obj_from_bits(_buffer_bits);
        let buffer_ptr = buffer_obj.as_ptr();
        if buffer_ptr.is_none() {
            return raise_exception::<_>(_py, "TypeError", "recv_into requires a writable buffer");
        }
        let buffer_ptr = buffer_ptr.unwrap();
        let size = to_i64(obj_from_bits(_size_bits)).unwrap_or(-1);
        let target_len;
        let mut use_memoryview = false;
        let type_id = unsafe { object_type_id(buffer_ptr) };
        if type_id == TYPE_ID_BYTEARRAY {
            target_len = unsafe { bytearray_len(buffer_ptr) };
        } else if type_id == TYPE_ID_MEMORYVIEW {
            if unsafe { memoryview_readonly(buffer_ptr) } {
                return raise_exception::<_>(
                    _py,
                    "TypeError",
                    "recv_into requires a writable buffer",
                );
            }
            target_len = unsafe { memoryview_len(buffer_ptr) };
            use_memoryview = true;
        } else {
            return raise_exception::<_>(_py, "TypeError", "recv_into requires a writable buffer");
        }
        let size = if size < 0 {
            target_len
        } else {
            (size as usize).min(target_len)
        };
        let flags = to_i64(obj_from_bits(_flags_bits)).unwrap_or(0) as i32;
        #[cfg(unix)]
        let dontwait = (flags & libc::MSG_DONTWAIT) != 0;
        #[cfg(not(unix))]
        let dontwait = false;
        loop {
            let rc = if use_memoryview {
                if let Some(slice) = unsafe { memoryview_bytes_slice_mut(buffer_ptr) } {
                    let len = size.min(slice.len());
                    unsafe {
                        crate::molt_socket_recv_host(
                            handle,
                            slice.as_mut_ptr() as u32,
                            len as u32,
                            flags,
                        )
                    }
                } else {
                    let mut tmp = vec![0u8; size];
                    let res = unsafe {
                        crate::molt_socket_recv_host(
                            handle,
                            tmp.as_mut_ptr() as u32,
                            tmp.len() as u32,
                            flags,
                        )
                    };
                    if res >= 0
                        && let Err(msg) =
                            unsafe { memoryview_write_bytes(buffer_ptr, &tmp[..res as usize]) }
                    {
                        return raise_exception::<u64>(_py, "TypeError", &msg);
                    }
                    res
                }
            } else {
                let buf = unsafe { bytearray_vec(buffer_ptr) };
                unsafe {
                    crate::molt_socket_recv_host(
                        handle,
                        buf.as_mut_ptr() as u32,
                        size as u32,
                        flags,
                    )
                }
            };
            if rc >= 0 {
                return MoltObject::from_int(rc as i64).bits();
            }
            let errno = errno_from_rc(rc);
            if would_block_errno(errno) {
                if dontwait {
                    return raise_os_error_errno::<u64>(_py, errno as i64, "recv_into");
                }
                if let Err(wait_err) = socket_wait_ready(_py, handle, IO_EVENT_READ) {
                    if wait_err.kind() == ErrorKind::TimedOut {
                        return raise_exception::<u64>(_py, "TimeoutError", "timed out");
                    }
                    if wait_err.kind() == ErrorKind::WouldBlock {
                        continue;
                    }
                    return raise_os_error::<u64>(_py, wait_err, "recv_into");
                }
                continue;
            }
            return raise_os_error_errno::<u64>(_py, errno as i64, "recv_into");
        }
    })
}

#[cfg(target_arch = "wasm32")]
#[unsafe(no_mangle)]
pub extern "C" fn molt_socket_send(_sock_bits: u64, _data_bits: u64, _flags_bits: u64) -> u64 {
    crate::with_gil_entry_nopanic!(_py, {
        let handle = match socket_handle_from_bits(_py, _sock_bits) {
            Ok(val) => val,
            Err(_) => return MoltObject::from_int(0).bits(),
        };
        let flags = to_i64(obj_from_bits(_flags_bits)).unwrap_or(0) as i32;
        #[cfg(unix)]
        let dontwait = (flags & libc::MSG_DONTWAIT) != 0;
        #[cfg(not(unix))]
        let dontwait = false;
        let nonblocking = matches!(socket_timeout(handle), Some(val) if val == Duration::ZERO);
        let send_data = match send_data_from_bits(_data_bits) {
            Ok(data) => data,
            Err(msg) => return raise_exception::<_>(_py, "TypeError", &msg),
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
            let rc = unsafe {
                crate::molt_socket_send_host(handle, data_ptr as u32, data_len as u32, flags)
            };
            if rc >= 0 {
                return MoltObject::from_int(rc as i64).bits();
            }
            let errno = errno_from_rc(rc);
            if would_block_errno(errno) {
                if dontwait || nonblocking {
                    return raise_os_error_errno::<u64>(_py, errno as i64, "send");
                }
                if let Err(wait_err) = socket_wait_ready(_py, handle, IO_EVENT_WRITE) {
                    if wait_err.kind() == ErrorKind::TimedOut {
                        return raise_exception::<u64>(_py, "TimeoutError", "timed out");
                    }
                    if wait_err.kind() == ErrorKind::WouldBlock {
                        continue;
                    }
                    return raise_os_error::<u64>(_py, wait_err, "send");
                }
                continue;
            }
            return raise_os_error_errno::<u64>(_py, errno as i64, "send");
        }
    })
}

#[cfg(target_arch = "wasm32")]
#[unsafe(no_mangle)]
pub extern "C" fn molt_socket_sendall(_sock_bits: u64, _data_bits: u64, _flags_bits: u64) -> u64 {
    crate::with_gil_entry_nopanic!(_py, {
        let handle = match socket_handle_from_bits(_py, _sock_bits) {
            Ok(val) => val,
            Err(_) => return MoltObject::none().bits(),
        };
        let flags = to_i64(obj_from_bits(_flags_bits)).unwrap_or(0) as i32;
        #[cfg(unix)]
        let dontwait = (flags & libc::MSG_DONTWAIT) != 0;
        #[cfg(not(unix))]
        let dontwait = false;
        let nonblocking = matches!(socket_timeout(handle), Some(val) if val == Duration::ZERO);
        let send_data = match send_data_from_bits(_data_bits) {
            Ok(data) => data,
            Err(msg) => return raise_exception::<_>(_py, "TypeError", &msg),
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
        let mut sent = 0usize;
        while sent < data_len {
            let rc = unsafe {
                crate::molt_socket_send_host(
                    handle,
                    data_ptr.add(sent) as u32,
                    (data_len - sent) as u32,
                    flags,
                )
            };
            if rc >= 0 {
                sent += rc as usize;
                continue;
            }
            let errno = errno_from_rc(rc);
            if would_block_errno(errno) {
                if dontwait || nonblocking {
                    return raise_os_error_errno::<u64>(_py, errno as i64, "sendall");
                }
                if let Err(wait_err) = socket_wait_ready(_py, handle, IO_EVENT_WRITE) {
                    if wait_err.kind() == ErrorKind::TimedOut {
                        return raise_exception::<u64>(_py, "TimeoutError", "timed out");
                    }
                    if wait_err.kind() == ErrorKind::WouldBlock {
                        continue;
                    }
                    return raise_os_error::<u64>(_py, wait_err, "sendall");
                }
                continue;
            }
            return raise_os_error_errno::<u64>(_py, errno as i64, "sendall");
        }
        MoltObject::none().bits()
    })
}

#[cfg(target_arch = "wasm32")]
#[unsafe(no_mangle)]
pub extern "C" fn molt_socket_sendto(
    _sock_bits: u64,
    _data_bits: u64,
    _flags_bits: u64,
    _addr_bits: u64,
) -> u64 {
    crate::with_gil_entry_nopanic!(_py, {
        let handle = match socket_handle_from_bits(_py, _sock_bits) {
            Ok(val) => val,
            Err(_) => return MoltObject::from_int(0).bits(),
        };
        let family = match wasm_socket_family(handle) {
            Ok(val) => val,
            Err(msg) => return raise_exception::<_>(_py, "TypeError", &msg),
        };
        let addr = match encode_sockaddr(_py, _addr_bits, family) {
            Ok(val) => val,
            Err(msg) => return raise_exception::<_>(_py, "TypeError", &msg),
        };
        let flags = to_i64(obj_from_bits(_flags_bits)).unwrap_or(0) as i32;
        #[cfg(unix)]
        let dontwait = (flags & libc::MSG_DONTWAIT) != 0;
        #[cfg(not(unix))]
        let dontwait = false;
        let nonblocking = matches!(socket_timeout(handle), Some(val) if val == Duration::ZERO);
        let send_data = match send_data_from_bits(_data_bits) {
            Ok(data) => data,
            Err(msg) => return raise_exception::<_>(_py, "TypeError", &msg),
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
            let rc = unsafe {
                crate::molt_socket_sendto_host(
                    handle,
                    data_ptr as u32,
                    data_len as u32,
                    flags,
                    addr.as_ptr() as u32,
                    addr.len() as u32,
                )
            };
            if rc >= 0 {
                return MoltObject::from_int(rc as i64).bits();
            }
            let errno = errno_from_rc(rc);
            if would_block_errno(errno) {
                if dontwait || nonblocking {
                    return raise_os_error_errno::<u64>(_py, errno as i64, "sendto");
                }
                if let Err(wait_err) = socket_wait_ready(_py, handle, IO_EVENT_WRITE) {
                    if wait_err.kind() == ErrorKind::TimedOut {
                        return raise_exception::<u64>(_py, "TimeoutError", "timed out");
                    }
                    if wait_err.kind() == ErrorKind::WouldBlock {
                        continue;
                    }
                    return raise_os_error::<u64>(_py, wait_err, "sendto");
                }
                continue;
            }
            return raise_os_error_errno::<u64>(_py, errno as i64, "sendto");
        }
    })
}

#[cfg(target_arch = "wasm32")]
#[unsafe(no_mangle)]
pub extern "C" fn molt_socket_recvfrom(_sock_bits: u64, _size_bits: u64, _flags_bits: u64) -> u64 {
    crate::with_gil_entry_nopanic!(_py, {
        let handle = match socket_handle_from_bits(_py, _sock_bits) {
            Ok(val) => val,
            Err(_) => return MoltObject::none().bits(),
        };
        let size = to_i64(obj_from_bits(_size_bits)).unwrap_or(0).max(0) as usize;
        let flags = to_i64(obj_from_bits(_flags_bits)).unwrap_or(0) as i32;
        #[cfg(unix)]
        let dontwait = (flags & libc::MSG_DONTWAIT) != 0;
        #[cfg(not(unix))]
        let dontwait = false;
        if size == 0 {
            let bytes_ptr = alloc_bytes(_py, &[]);
            if bytes_ptr.is_null() {
                return MoltObject::none().bits();
            }
            let tuple_ptr = alloc_tuple(
                _py,
                &[
                    MoltObject::from_ptr(bytes_ptr).bits(),
                    MoltObject::none().bits(),
                ],
            );
            if tuple_ptr.is_null() {
                return MoltObject::none().bits();
            }
            return MoltObject::from_ptr(tuple_ptr).bits();
        }
        let nonblocking = matches!(socket_timeout(handle), Some(val) if val == Duration::ZERO);
        let mut buf = vec![0u8; size];
        let mut addr_buf = vec![0u8; 128];
        let mut addr_len: u32 = 0;
        loop {
            let rc = unsafe {
                crate::molt_socket_recvfrom_host(
                    handle,
                    buf.as_mut_ptr() as u32,
                    buf.len() as u32,
                    flags,
                    addr_buf.as_mut_ptr() as u32,
                    addr_buf.len() as u32,
                    (&mut addr_len) as *mut u32 as u32,
                )
            };
            if rc >= 0 {
                let bytes_ptr = alloc_bytes(_py, &buf[..rc as usize]);
                if bytes_ptr.is_null() {
                    return MoltObject::none().bits();
                }
                let addr_bits = match decode_sockaddr(_py, &addr_buf[..addr_len as usize]) {
                    Ok(bits) => bits,
                    Err(msg) => return raise_exception::<_>(_py, "TypeError", &msg),
                };
                let bytes_bits = MoltObject::from_ptr(bytes_ptr).bits();
                let tuple_ptr = alloc_tuple(_py, &[bytes_bits, addr_bits]);
                if tuple_ptr.is_null() {
                    return MoltObject::none().bits();
                }
                return MoltObject::from_ptr(tuple_ptr).bits();
            }
            let errno = errno_from_rc(rc);
            if would_block_errno(errno) {
                if dontwait || nonblocking {
                    return raise_os_error_errno::<u64>(_py, errno as i64, "recvfrom");
                }
                if let Err(wait_err) = socket_wait_ready(_py, handle, IO_EVENT_READ) {
                    if wait_err.kind() == ErrorKind::TimedOut {
                        return raise_exception::<u64>(_py, "TimeoutError", "timed out");
                    }
                    if wait_err.kind() == ErrorKind::WouldBlock {
                        continue;
                    }
                    return raise_os_error::<u64>(_py, wait_err, "recvfrom");
                }
                continue;
            }
            return raise_os_error_errno::<u64>(_py, errno as i64, "recvfrom");
        }
    })
}

#[cfg(target_arch = "wasm32")]
#[unsafe(no_mangle)]
pub extern "C" fn molt_socket_recvfrom_into(
    _sock_bits: u64,
    _buffer_bits: u64,
    _size_bits: u64,
    _flags_bits: u64,
) -> u64 {
    crate::with_gil_entry_nopanic!(_py, {
        let handle = match socket_handle_from_bits(_py, _sock_bits) {
            Ok(val) => val,
            Err(_) => return MoltObject::none().bits(),
        };
        let buffer_obj = obj_from_bits(_buffer_bits);
        let buffer_ptr = match buffer_obj.as_ptr() {
            Some(ptr) => ptr,
            None => {
                return raise_exception::<_>(
                    _py,
                    "TypeError",
                    "recvfrom_into requires a writable buffer",
                );
            }
        };
        let size = to_i64(obj_from_bits(_size_bits)).unwrap_or(-1);
        let target_len;
        let mut use_memoryview = false;
        let type_id = unsafe { object_type_id(buffer_ptr) };
        if type_id == TYPE_ID_BYTEARRAY {
            target_len = unsafe { bytearray_len(buffer_ptr) };
        } else if type_id == TYPE_ID_MEMORYVIEW {
            if unsafe { memoryview_readonly(buffer_ptr) } {
                return raise_exception::<_>(
                    _py,
                    "TypeError",
                    "recvfrom_into requires a writable buffer",
                );
            }
            target_len = unsafe { memoryview_len(buffer_ptr) };
            use_memoryview = true;
        } else {
            return raise_exception::<_>(
                _py,
                "TypeError",
                "recvfrom_into requires a writable buffer",
            );
        }
        let size = if size < 0 {
            target_len
        } else {
            (size as usize).min(target_len)
        };
        let flags = to_i64(obj_from_bits(_flags_bits)).unwrap_or(0) as i32;
        #[cfg(unix)]
        let dontwait = (flags & libc::MSG_DONTWAIT) != 0;
        #[cfg(not(unix))]
        let dontwait = false;
        let nonblocking = matches!(socket_timeout(handle), Some(val) if val == Duration::ZERO);
        let mut addr_buf = vec![0u8; 128];
        let mut addr_len: u32 = 0;
        loop {
            let rc = if use_memoryview {
                if let Some(slice) = unsafe { memoryview_bytes_slice_mut(buffer_ptr) } {
                    let recv_len = size.min(slice.len());
                    unsafe {
                        crate::molt_socket_recvfrom_host(
                            handle,
                            slice.as_mut_ptr() as u32,
                            recv_len as u32,
                            flags,
                            addr_buf.as_mut_ptr() as u32,
                            addr_buf.len() as u32,
                            (&mut addr_len) as *mut u32 as u32,
                        )
                    }
                } else {
                    let mut tmp = vec![0u8; size];
                    let res = unsafe {
                        crate::molt_socket_recvfrom_host(
                            handle,
                            tmp.as_mut_ptr() as u32,
                            tmp.len() as u32,
                            flags,
                            addr_buf.as_mut_ptr() as u32,
                            addr_buf.len() as u32,
                            (&mut addr_len) as *mut u32 as u32,
                        )
                    };
                    if res >= 0
                        && let Err(msg) =
                            unsafe { memoryview_write_bytes(buffer_ptr, &tmp[..res as usize]) }
                    {
                        return raise_exception::<u64>(_py, "TypeError", &msg);
                    }
                    res
                }
            } else {
                let buf = unsafe { bytearray_vec(buffer_ptr) };
                let recv_len = size.min(buf.len());
                unsafe {
                    crate::molt_socket_recvfrom_host(
                        handle,
                        buf.as_mut_ptr() as u32,
                        recv_len as u32,
                        flags,
                        addr_buf.as_mut_ptr() as u32,
                        addr_buf.len() as u32,
                        (&mut addr_len) as *mut u32 as u32,
                    )
                }
            };
            if rc >= 0 {
                let n_bits = MoltObject::from_int(rc as i64).bits();
                let addr_bits = match decode_sockaddr(_py, &addr_buf[..addr_len as usize]) {
                    Ok(bits) => bits,
                    Err(msg) => return raise_exception::<_>(_py, "TypeError", &msg),
                };
                let tuple_ptr = alloc_tuple(_py, &[n_bits, addr_bits]);
                if tuple_ptr.is_null() {
                    return MoltObject::none().bits();
                }
                return MoltObject::from_ptr(tuple_ptr).bits();
            }
            let errno = errno_from_rc(rc);
            if would_block_errno(errno) {
                if dontwait || nonblocking {
                    return raise_os_error_errno::<u64>(_py, errno as i64, "recvfrom_into");
                }
                if let Err(wait_err) = socket_wait_ready(_py, handle, IO_EVENT_READ) {
                    if wait_err.kind() == ErrorKind::TimedOut {
                        return raise_exception::<u64>(_py, "TimeoutError", "timed out");
                    }
                    if wait_err.kind() == ErrorKind::WouldBlock {
                        continue;
                    }
                    return raise_os_error::<u64>(_py, wait_err, "recvfrom_into");
                }
                continue;
            }
            return raise_os_error_errno::<u64>(_py, errno as i64, "recvfrom_into");
        }
    })
}

#[cfg(target_arch = "wasm32")]
#[unsafe(no_mangle)]
pub extern "C" fn molt_socket_sendmsg(
    _sock_bits: u64,
    _buffers_bits: u64,
    _ancdata_bits: u64,
    _flags_bits: u64,
    _address_bits: u64,
) -> u64 {
    crate::with_gil_entry_nopanic!(_py, {
        let handle = match socket_handle_from_bits(_py, _sock_bits) {
            Ok(val) => val,
            Err(_) => return MoltObject::from_int(0).bits(),
        };
        let ancillary_items = match parse_sendmsg_ancillary_items(_py, _ancdata_bits) {
            Ok(val) => val,
            Err(bits) => return bits,
        };
        let ancillary_payload = match encode_host_sendmsg_ancillary_buffer(&ancillary_items) {
            Ok(val) => val,
            Err(msg) => return raise_exception::<u64>(_py, "RuntimeError", &msg),
        };
        let chunks = match collect_sendmsg_payload(_py, _buffers_bits) {
            Ok(val) => val,
            Err(bits) => return bits,
        };
        let payload: Vec<u8> = chunks.concat();
        let flags = to_i64(obj_from_bits(_flags_bits)).unwrap_or(0) as i32;
        #[cfg(unix)]
        let dontwait = (flags & libc::MSG_DONTWAIT) != 0;
        #[cfg(not(unix))]
        let dontwait = false;
        let nonblocking = matches!(socket_timeout(handle), Some(val) if val == Duration::ZERO);
        let send_addr = if obj_from_bits(_address_bits).is_none() {
            None
        } else {
            let family = match wasm_socket_family(handle) {
                Ok(val) => val,
                Err(msg) => return raise_exception::<u64>(_py, "TypeError", &msg),
            };
            match encode_sockaddr(_py, _address_bits, family) {
                Ok(val) => Some(val),
                Err(msg) => return raise_exception::<u64>(_py, "TypeError", &msg),
            }
        };
        let payload_ptr = if payload.is_empty() {
            std::ptr::null::<u8>() as u32
        } else {
            payload.as_ptr() as u32
        };
        let ancillary_ptr = if ancillary_payload.is_empty() {
            std::ptr::null::<u8>() as u32
        } else {
            ancillary_payload.as_ptr() as u32
        };
        let ancillary_len = ancillary_payload.len() as u32;
        loop {
            let (addr_ptr, addr_len) = if let Some(addr) = send_addr.as_ref() {
                (addr.as_ptr() as u32, addr.len() as u32)
            } else {
                (0, 0)
            };
            let rc = unsafe {
                crate::molt_socket_sendmsg_host(
                    handle,
                    payload_ptr,
                    payload.len() as u32,
                    flags,
                    addr_ptr,
                    addr_len,
                    ancillary_ptr,
                    ancillary_len,
                )
            };
            if rc >= 0 {
                return MoltObject::from_int(rc as i64).bits();
            }
            let errno = errno_from_rc(rc);
            if would_block_errno(errno) {
                if dontwait || nonblocking {
                    return raise_os_error_errno::<u64>(_py, errno as i64, "sendmsg");
                }
                if let Err(wait_err) = socket_wait_ready(_py, handle, IO_EVENT_WRITE) {
                    if wait_err.kind() == ErrorKind::TimedOut {
                        return raise_exception::<u64>(_py, "TimeoutError", "timed out");
                    }
                    if wait_err.kind() == ErrorKind::WouldBlock {
                        continue;
                    }
                    return raise_os_error::<u64>(_py, wait_err, "sendmsg");
                }
                continue;
            }
            return raise_os_error_errno::<u64>(_py, errno as i64, "sendmsg");
        }
    })
}

#[cfg(target_arch = "wasm32")]
#[unsafe(no_mangle)]
pub extern "C" fn molt_socket_recvmsg(
    _sock_bits: u64,
    _bufsize_bits: u64,
    _ancbufsize_bits: u64,
    _flags_bits: u64,
) -> u64 {
    crate::with_gil_entry_nopanic!(_py, {
        let handle = match socket_handle_from_bits(_py, _sock_bits) {
            Ok(val) => val,
            Err(_) => return MoltObject::none().bits(),
        };
        let bufsize = to_i64(obj_from_bits(_bufsize_bits)).unwrap_or(0);
        if bufsize < 0 {
            return raise_exception::<u64>(_py, "ValueError", "negative buffersize in recvmsg");
        }
        let ancbufsize = to_i64(obj_from_bits(_ancbufsize_bits)).unwrap_or(0);
        if ancbufsize < 0 {
            return raise_exception::<u64>(_py, "ValueError", "negative ancbufsize in recvmsg");
        }
        let flags = to_i64(obj_from_bits(_flags_bits)).unwrap_or(0) as i32;
        #[cfg(unix)]
        let dontwait = (flags & libc::MSG_DONTWAIT) != 0;
        #[cfg(not(unix))]
        let dontwait = false;
        let nonblocking = matches!(socket_timeout(handle), Some(val) if val == Duration::ZERO);
        let mut buf = vec![0u8; bufsize as usize];
        let mut anc_buf = vec![0u8; ancbufsize as usize];
        let mut addr_buf = vec![0u8; 128];
        let mut addr_len: u32 = 0;
        let mut anc_len: u32 = 0;
        let mut msg_flags: i32 = 0;
        loop {
            let rc = unsafe {
                crate::molt_socket_recvmsg_host(
                    handle,
                    if buf.is_empty() {
                        std::ptr::null_mut::<u8>() as u32
                    } else {
                        buf.as_mut_ptr() as u32
                    },
                    buf.len() as u32,
                    flags,
                    addr_buf.as_mut_ptr() as u32,
                    addr_buf.len() as u32,
                    (&mut addr_len) as *mut u32 as u32,
                    if anc_buf.is_empty() {
                        std::ptr::null_mut::<u8>() as u32
                    } else {
                        anc_buf.as_mut_ptr() as u32
                    },
                    anc_buf.len() as u32,
                    (&mut anc_len) as *mut u32 as u32,
                    (&mut msg_flags) as *mut i32 as u32,
                )
            };
            if rc >= 0 {
                let addr_bits = if addr_len > 0 {
                    match decode_sockaddr(_py, &addr_buf[..addr_len as usize]) {
                        Ok(bits) => bits,
                        Err(msg) => return raise_exception::<u64>(_py, "TypeError", &msg),
                    }
                } else {
                    MoltObject::none().bits()
                };
                if (anc_len as usize) > anc_buf.len() {
                    dec_ref_bits(_py, addr_bits);
                    return raise_os_error_errno::<u64>(_py, libc::ENOMEM as i64, "recvmsg");
                }
                let ancillary_items =
                    match decode_host_recvmsg_ancillary_buffer(&anc_buf[..anc_len as usize]) {
                        Ok(val) => val,
                        Err(msg) => {
                            dec_ref_bits(_py, addr_bits);
                            return raise_exception::<u64>(_py, "RuntimeError", &msg);
                        }
                    };
                let anc_bits = match build_ancillary_list_bits(_py, ancillary_items.as_slice()) {
                    Ok(bits) => bits,
                    Err(bits) => {
                        dec_ref_bits(_py, addr_bits);
                        return bits;
                    }
                };
                return build_recvmsg_result_with_anc(
                    _py,
                    &buf[..rc as usize],
                    msg_flags,
                    addr_bits,
                    anc_bits,
                );
            }
            let errno = errno_from_rc(rc);
            if would_block_errno(errno) {
                if dontwait || nonblocking {
                    return raise_os_error_errno::<u64>(_py, errno as i64, "recvmsg");
                }
                if let Err(wait_err) = socket_wait_ready(_py, handle, IO_EVENT_READ) {
                    if wait_err.kind() == ErrorKind::TimedOut {
                        return raise_exception::<u64>(_py, "TimeoutError", "timed out");
                    }
                    if wait_err.kind() == ErrorKind::WouldBlock {
                        continue;
                    }
                    return raise_os_error::<u64>(_py, wait_err, "recvmsg");
                }
                continue;
            }
            return raise_os_error_errno::<u64>(_py, errno as i64, "recvmsg");
        }
    })
}

#[cfg(target_arch = "wasm32")]
#[unsafe(no_mangle)]
pub extern "C" fn molt_socket_recvmsg_into(
    _sock_bits: u64,
    _buffers_bits: u64,
    _ancbufsize_bits: u64,
    _flags_bits: u64,
) -> u64 {
    crate::with_gil_entry_nopanic!(_py, {
        let handle = match socket_handle_from_bits(_py, _sock_bits) {
            Ok(val) => val,
            Err(_) => return MoltObject::none().bits(),
        };
        let ancbufsize = to_i64(obj_from_bits(_ancbufsize_bits)).unwrap_or(0);
        if ancbufsize < 0 {
            return raise_exception::<u64>(_py, "ValueError", "negative ancbufsize in recvmsg");
        }
        let targets = match collect_recvmsg_into_targets(_py, _buffers_bits) {
            Ok(val) => val,
            Err(bits) => return bits,
        };
        let total_len = targets
            .iter()
            .fold(0usize, |acc, target| acc.saturating_add(target.len()));
        let mut tmp = vec![0u8; total_len];
        let flags = to_i64(obj_from_bits(_flags_bits)).unwrap_or(0) as i32;
        #[cfg(unix)]
        let dontwait = (flags & libc::MSG_DONTWAIT) != 0;
        #[cfg(not(unix))]
        let dontwait = false;
        let nonblocking = matches!(socket_timeout(handle), Some(val) if val == Duration::ZERO);
        let mut anc_buf = vec![0u8; ancbufsize as usize];
        let mut addr_buf = vec![0u8; 128];
        let mut addr_len: u32 = 0;
        let mut anc_len: u32 = 0;
        let mut msg_flags: i32 = 0;
        loop {
            let rc = unsafe {
                crate::molt_socket_recvmsg_host(
                    handle,
                    if tmp.is_empty() {
                        std::ptr::null_mut::<u8>() as u32
                    } else {
                        tmp.as_mut_ptr() as u32
                    },
                    tmp.len() as u32,
                    flags,
                    addr_buf.as_mut_ptr() as u32,
                    addr_buf.len() as u32,
                    (&mut addr_len) as *mut u32 as u32,
                    if anc_buf.is_empty() {
                        std::ptr::null_mut::<u8>() as u32
                    } else {
                        anc_buf.as_mut_ptr() as u32
                    },
                    anc_buf.len() as u32,
                    (&mut anc_len) as *mut u32 as u32,
                    (&mut msg_flags) as *mut i32 as u32,
                )
            };
            if rc >= 0 {
                let addr_bits = if addr_len > 0 {
                    match decode_sockaddr(_py, &addr_buf[..addr_len as usize]) {
                        Ok(bits) => bits,
                        Err(msg) => return raise_exception::<u64>(_py, "TypeError", &msg),
                    }
                } else {
                    MoltObject::none().bits()
                };
                if let Err(bits) = write_recvmsg_into_targets(_py, &targets, &tmp[..rc as usize]) {
                    dec_ref_bits(_py, addr_bits);
                    return bits;
                }
                if (anc_len as usize) > anc_buf.len() {
                    dec_ref_bits(_py, addr_bits);
                    return raise_os_error_errno::<u64>(_py, libc::ENOMEM as i64, "recvmsg_into");
                }
                let ancillary_items =
                    match decode_host_recvmsg_ancillary_buffer(&anc_buf[..anc_len as usize]) {
                        Ok(val) => val,
                        Err(msg) => {
                            dec_ref_bits(_py, addr_bits);
                            return raise_exception::<u64>(_py, "RuntimeError", &msg);
                        }
                    };
                let anc_bits = match build_ancillary_list_bits(_py, ancillary_items.as_slice()) {
                    Ok(bits) => bits,
                    Err(bits) => {
                        dec_ref_bits(_py, addr_bits);
                        return bits;
                    }
                };
                let n_bits = MoltObject::from_int(rc as i64).bits();
                let msg_flags_bits = MoltObject::from_int(msg_flags as i64).bits();
                let tuple_ptr = alloc_tuple(_py, &[n_bits, anc_bits, msg_flags_bits, addr_bits]);
                dec_ref_bits(_py, anc_bits);
                dec_ref_bits(_py, addr_bits);
                if tuple_ptr.is_null() {
                    return MoltObject::none().bits();
                }
                return MoltObject::from_ptr(tuple_ptr).bits();
            }
            let errno = errno_from_rc(rc);
            if would_block_errno(errno) {
                if dontwait || nonblocking {
                    return raise_os_error_errno::<u64>(_py, errno as i64, "recvmsg_into");
                }
                if let Err(wait_err) = socket_wait_ready(_py, handle, IO_EVENT_READ) {
                    if wait_err.kind() == ErrorKind::TimedOut {
                        return raise_exception::<u64>(_py, "TimeoutError", "timed out");
                    }
                    if wait_err.kind() == ErrorKind::WouldBlock {
                        continue;
                    }
                    return raise_os_error::<u64>(_py, wait_err, "recvmsg_into");
                }
                continue;
            }
            return raise_os_error_errno::<u64>(_py, errno as i64, "recvmsg_into");
        }
    })
}

#[cfg(target_arch = "wasm32")]
#[unsafe(no_mangle)]
pub extern "C" fn molt_socket_shutdown(_sock_bits: u64, _how_bits: u64) -> u64 {
    crate::with_gil_entry_nopanic!(_py, {
        let handle = match socket_handle_from_bits(_py, _sock_bits) {
            Ok(val) => val,
            Err(_) => return MoltObject::none().bits(),
        };
        let how = to_i64(obj_from_bits(_how_bits)).unwrap_or(0) as i32;
        let rc = unsafe { crate::molt_socket_shutdown_host(handle, how) };
        if rc < 0 {
            return raise_os_error_errno::<u64>(_py, errno_from_rc(rc) as i64, "shutdown");
        }
        MoltObject::none().bits()
    })
}

#[cfg(target_arch = "wasm32")]
#[unsafe(no_mangle)]
pub extern "C" fn molt_socket_getsockname(_sock_bits: u64) -> u64 {
    crate::with_gil_entry_nopanic!(_py, {
        let handle = match socket_handle_from_bits(_py, _sock_bits) {
            Ok(val) => val,
            Err(_) => return MoltObject::none().bits(),
        };
        let mut addr_buf = vec![0u8; 128];
        let mut addr_len: u32 = 0;
        let rc = unsafe {
            crate::molt_socket_getsockname_host(
                handle,
                addr_buf.as_mut_ptr() as u32,
                addr_buf.len() as u32,
                (&mut addr_len) as *mut u32 as u32,
            )
        };
        if rc < 0 {
            return raise_os_error_errno::<u64>(_py, errno_from_rc(rc) as i64, "getsockname");
        }
        match decode_sockaddr(_py, &addr_buf[..addr_len as usize]) {
            Ok(bits) => bits,
            Err(msg) => raise_exception::<_>(_py, "TypeError", &msg),
        }
    })
}

#[cfg(target_arch = "wasm32")]
#[unsafe(no_mangle)]
pub extern "C" fn molt_socket_getpeername(_sock_bits: u64) -> u64 {
    crate::with_gil_entry_nopanic!(_py, {
        let handle = match socket_handle_from_bits(_py, _sock_bits) {
            Ok(val) => val,
            Err(_) => return MoltObject::none().bits(),
        };
        let mut addr_buf = vec![0u8; 128];
        let mut addr_len: u32 = 0;
        let rc = unsafe {
            crate::molt_socket_getpeername_host(
                handle,
                addr_buf.as_mut_ptr() as u32,
                addr_buf.len() as u32,
                (&mut addr_len) as *mut u32 as u32,
            )
        };
        if rc < 0 {
            return raise_os_error_errno::<u64>(_py, errno_from_rc(rc) as i64, "getpeername");
        }
        match decode_sockaddr(_py, &addr_buf[..addr_len as usize]) {
            Ok(bits) => bits,
            Err(msg) => raise_exception::<_>(_py, "TypeError", &msg),
        }
    })
}

#[cfg(target_arch = "wasm32")]
#[unsafe(no_mangle)]
pub extern "C" fn molt_socket_setsockopt(
    _sock_bits: u64,
    _level_bits: u64,
    _opt_bits: u64,
    _value_bits: u64,
) -> u64 {
    crate::with_gil_entry_nopanic!(_py, {
        let handle = match socket_handle_from_bits(_py, _sock_bits) {
            Ok(val) => val,
            Err(_) => return MoltObject::none().bits(),
        };
        let level = to_i64(obj_from_bits(_level_bits)).unwrap_or(0) as i32;
        let optname = to_i64(obj_from_bits(_opt_bits)).unwrap_or(0) as i32;
        let obj = obj_from_bits(_value_bits);
        let (val_buf, val_len) = if let Some(val) = to_i64(obj) {
            let bytes = (val as i32).to_ne_bytes();
            (bytes.to_vec(), bytes.len())
        } else if let Some(ptr) = obj.as_ptr() {
            let bytes = unsafe { bytes_like_slice_raw(ptr) };
            let Some(bytes) = bytes else {
                return raise_exception::<_>(_py, "TypeError", "invalid optval");
            };
            (bytes.to_vec(), bytes.len())
        } else {
            return raise_exception::<_>(_py, "TypeError", "invalid optval");
        };
        let rc = unsafe {
            crate::molt_socket_setsockopt_host(
                handle,
                level,
                optname,
                val_buf.as_ptr() as u32,
                val_len as u32,
            )
        };
        if rc < 0 {
            return raise_os_error_errno::<u64>(_py, errno_from_rc(rc) as i64, "setsockopt");
        }
        MoltObject::none().bits()
    })
}

#[cfg(target_arch = "wasm32")]
#[unsafe(no_mangle)]
pub extern "C" fn molt_socket_getsockopt(
    _sock_bits: u64,
    _level_bits: u64,
    _opt_bits: u64,
    _buflen_bits: u64,
) -> u64 {
    crate::with_gil_entry_nopanic!(_py, {
        let handle = match socket_handle_from_bits(_py, _sock_bits) {
            Ok(val) => val,
            Err(_) => return MoltObject::none().bits(),
        };
        let level = to_i64(obj_from_bits(_level_bits)).unwrap_or(0) as i32;
        let optname = to_i64(obj_from_bits(_opt_bits)).unwrap_or(0) as i32;
        let buflen = if obj_from_bits(_buflen_bits).is_none() {
            None
        } else {
            Some(to_i64(obj_from_bits(_buflen_bits)).unwrap_or(0).max(0) as usize)
        };
        let mut out_len: u32 = 0;
        if let Some(buflen) = buflen {
            let mut buf = vec![0u8; buflen];
            let rc = unsafe {
                crate::molt_socket_getsockopt_host(
                    handle,
                    level,
                    optname,
                    buf.as_mut_ptr() as u32,
                    buf.len() as u32,
                    (&mut out_len) as *mut u32 as u32,
                )
            };
            if rc < 0 {
                return raise_os_error_errno::<u64>(_py, errno_from_rc(rc) as i64, "getsockopt");
            }
            let ptr = alloc_bytes(_py, &buf[..out_len as usize]);
            if ptr.is_null() {
                return MoltObject::none().bits();
            }
            return MoltObject::from_ptr(ptr).bits();
        }
        let mut buf = vec![0u8; std::mem::size_of::<i32>()];
        let rc = unsafe {
            crate::molt_socket_getsockopt_host(
                handle,
                level,
                optname,
                buf.as_mut_ptr() as u32,
                buf.len() as u32,
                (&mut out_len) as *mut u32 as u32,
            )
        };
        if rc < 0 {
            return raise_os_error_errno::<u64>(_py, errno_from_rc(rc) as i64, "getsockopt");
        }
        if out_len as usize >= std::mem::size_of::<i32>() {
            let val = i32::from_ne_bytes([buf[0], buf[1], buf[2], buf[3]]) as i64;
            return MoltObject::from_int(val).bits();
        }
        let ptr = alloc_bytes(_py, &buf[..out_len as usize]);
        if ptr.is_null() {
            return MoltObject::none().bits();
        }
        MoltObject::from_ptr(ptr).bits()
    })
}

#[cfg(target_arch = "wasm32")]
#[unsafe(no_mangle)]
pub extern "C" fn molt_socket_detach(_sock_bits: u64) -> u64 {
    crate::with_gil_entry_nopanic!(_py, {
        let handle = match socket_handle_from_bits(_py, _sock_bits) {
            Ok(val) => val,
            Err(_) => return MoltObject::from_int(-1).bits(),
        };
        let rc = unsafe { crate::molt_socket_detach_host(handle) };
        wasm_socket_meta_remove(handle);
        if rc < 0 {
            return raise_os_error_errno::<u64>(_py, (-rc) as i64, "detach");
        }
        MoltObject::from_int(rc).bits()
    })
}
