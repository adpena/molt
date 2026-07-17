//! Asyncio socket wrapper futures.
//!
//! Owns socket reader/sock_* future payload layouts, polling state machines,
//! and drop hooks. Shared IO wait/error helpers stay in generators_async_io.rs.

use super::*;
const ASYNCIO_SOCKET_READER_READ_SLOT_READER: usize = 0;
const ASYNCIO_SOCKET_READER_READ_SLOT_N: usize = 1;
const ASYNCIO_SOCKET_READER_READ_SLOT_FD: usize = 2;
const ASYNCIO_SOCKET_READER_READ_SLOT_WAIT: usize = 3;
const ASYNCIO_SOCKET_READER_READLINE_SLOT_READER: usize = 0;
const ASYNCIO_SOCKET_READER_READLINE_SLOT_FD: usize = 1;
const ASYNCIO_SOCKET_READER_READLINE_SLOT_WAIT: usize = 2;
const ASYNCIO_SOCK_RECV_SLOT_SOCK: usize = 0;
const ASYNCIO_SOCK_RECV_SLOT_SIZE: usize = 1;
const ASYNCIO_SOCK_RECV_SLOT_FD: usize = 2;
const ASYNCIO_SOCK_RECV_SLOT_WAIT: usize = 3;
const ASYNCIO_SOCK_CONNECT_SLOT_SOCK: usize = 0;
const ASYNCIO_SOCK_CONNECT_SLOT_ADDR: usize = 1;
const ASYNCIO_SOCK_CONNECT_SLOT_FD: usize = 2;
const ASYNCIO_SOCK_CONNECT_SLOT_WAIT: usize = 3;
const ASYNCIO_SOCK_ACCEPT_SLOT_SOCK: usize = 0;
const ASYNCIO_SOCK_ACCEPT_SLOT_FD: usize = 1;
const ASYNCIO_SOCK_ACCEPT_SLOT_WAIT: usize = 2;
const ASYNCIO_SOCK_RECV_INTO_SLOT_SOCK: usize = 0;
const ASYNCIO_SOCK_RECV_INTO_SLOT_BUF: usize = 1;
const ASYNCIO_SOCK_RECV_INTO_SLOT_NBYTES: usize = 2;
const ASYNCIO_SOCK_RECV_INTO_SLOT_FD: usize = 3;
const ASYNCIO_SOCK_RECV_INTO_SLOT_WAIT: usize = 4;
const ASYNCIO_SOCK_SENDALL_SLOT_SOCK: usize = 0;
const ASYNCIO_SOCK_SENDALL_SLOT_DATA: usize = 1;
const ASYNCIO_SOCK_SENDALL_SLOT_TOTAL: usize = 2;
const ASYNCIO_SOCK_SENDALL_SLOT_DLEN: usize = 3;
const ASYNCIO_SOCK_SENDALL_SLOT_FD: usize = 4;
const ASYNCIO_SOCK_SENDALL_SLOT_WAIT: usize = 5;
const ASYNCIO_SOCK_RECVFROM_SLOT_SOCK: usize = 0;
const ASYNCIO_SOCK_RECVFROM_SLOT_SIZE: usize = 1;
const ASYNCIO_SOCK_RECVFROM_SLOT_FD: usize = 2;
const ASYNCIO_SOCK_RECVFROM_SLOT_WAIT: usize = 3;
const ASYNCIO_SOCK_RECVFROM_INTO_SLOT_SOCK: usize = 0;
const ASYNCIO_SOCK_RECVFROM_INTO_SLOT_BUF: usize = 1;
const ASYNCIO_SOCK_RECVFROM_INTO_SLOT_NBYTES: usize = 2;
const ASYNCIO_SOCK_RECVFROM_INTO_SLOT_FD: usize = 3;
const ASYNCIO_SOCK_RECVFROM_INTO_SLOT_WAIT: usize = 4;
const ASYNCIO_SOCK_SENDTO_SLOT_SOCK: usize = 0;
const ASYNCIO_SOCK_SENDTO_SLOT_DATA: usize = 1;
const ASYNCIO_SOCK_SENDTO_SLOT_ADDR: usize = 2;
const ASYNCIO_SOCK_SENDTO_SLOT_FD: usize = 3;
const ASYNCIO_SOCK_SENDTO_SLOT_WAIT: usize = 4;

/// # Safety
/// - `reader_bits` must be a valid socket-reader handle.
#[unsafe(no_mangle)]
pub extern "C" fn molt_asyncio_socket_reader_read_new(
    reader_bits: u64,
    n_bits: u64,
    fd_bits: u64,
) -> u64 {
    crate::with_gil_entry_nopanic!(_py, {
        let obj_bits = molt_future_new(
            asyncio_socket_reader_read_poll_fn_addr(),
            (4 * std::mem::size_of::<u64>()) as u64,
        );
        if obj_from_bits(obj_bits).is_none() {
            return obj_bits;
        }
        let Some(obj_ptr) = resolve_obj_ptr(obj_bits) else {
            return MoltObject::none().bits();
        };
        let payload_ptr = obj_ptr as *mut u64;
        unsafe {
            *payload_ptr.add(ASYNCIO_SOCKET_READER_READ_SLOT_READER) = reader_bits;
            *payload_ptr.add(ASYNCIO_SOCKET_READER_READ_SLOT_N) = n_bits;
            *payload_ptr.add(ASYNCIO_SOCKET_READER_READ_SLOT_FD) = fd_bits;
            *payload_ptr.add(ASYNCIO_SOCKET_READER_READ_SLOT_WAIT) = MoltObject::none().bits();
        }
        inc_ref_bits(_py, reader_bits);
        inc_ref_bits(_py, n_bits);
        inc_ref_bits(_py, fd_bits);
        obj_bits
    })
}

/// # Safety
/// - `obj_bits` must be a valid pointer to a socket-reader read wrapper future.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn molt_asyncio_socket_reader_read_poll(obj_bits: u64) -> i64 {
    unsafe {
        crate::with_gil_entry_nopanic!(_py, {
            let obj_ptr = ptr_from_bits(obj_bits);
            if obj_ptr.is_null() {
                return MoltObject::none().bits() as i64;
            }
            let _header = header_from_obj_ptr(obj_ptr);
            let payload_bytes = crate::object::object_payload_size(obj_ptr);
            if payload_bytes < 4 * std::mem::size_of::<u64>() {
                return raise_exception::<i64>(
                    _py,
                    "RuntimeError",
                    "invalid socket_reader_read payload",
                );
            }
            let payload_ptr = obj_ptr as *mut u64;
            let reader_bits = *payload_ptr.add(ASYNCIO_SOCKET_READER_READ_SLOT_READER);
            let n_bits = *payload_ptr.add(ASYNCIO_SOCKET_READER_READ_SLOT_N);
            let out_bits = molt_socket_reader_read(reader_bits, n_bits);
            if out_bits as i64 != pending_bits_i64() {
                asyncio_drop_payload_slots(_py, payload_ptr, 4);
                return out_bits as i64;
            }
            let fd_bits = *payload_ptr.add(ASYNCIO_SOCKET_READER_READ_SLOT_FD);
            asyncio_pending_with_wait(
                _py,
                payload_ptr,
                ASYNCIO_SOCKET_READER_READ_SLOT_WAIT,
                fd_bits,
                ASYNCIO_SOCKET_IO_EVENT_READ,
            )
        })
    }
}

/// # Safety
/// - `reader_bits` must be a valid socket-reader handle.
#[unsafe(no_mangle)]
pub extern "C" fn molt_asyncio_socket_reader_readline_new(reader_bits: u64, fd_bits: u64) -> u64 {
    crate::with_gil_entry_nopanic!(_py, {
        let obj_bits = molt_future_new(
            asyncio_socket_reader_readline_poll_fn_addr(),
            (3 * std::mem::size_of::<u64>()) as u64,
        );
        if obj_from_bits(obj_bits).is_none() {
            return obj_bits;
        }
        let Some(obj_ptr) = resolve_obj_ptr(obj_bits) else {
            return MoltObject::none().bits();
        };
        let payload_ptr = obj_ptr as *mut u64;
        unsafe {
            *payload_ptr.add(ASYNCIO_SOCKET_READER_READLINE_SLOT_READER) = reader_bits;
            *payload_ptr.add(ASYNCIO_SOCKET_READER_READLINE_SLOT_FD) = fd_bits;
            *payload_ptr.add(ASYNCIO_SOCKET_READER_READLINE_SLOT_WAIT) = MoltObject::none().bits();
        }
        inc_ref_bits(_py, reader_bits);
        inc_ref_bits(_py, fd_bits);
        obj_bits
    })
}

/// # Safety
/// - `obj_bits` must be a valid pointer to a socket-reader readline wrapper future.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn molt_asyncio_socket_reader_readline_poll(obj_bits: u64) -> i64 {
    unsafe {
        crate::with_gil_entry_nopanic!(_py, {
            let obj_ptr = ptr_from_bits(obj_bits);
            if obj_ptr.is_null() {
                return MoltObject::none().bits() as i64;
            }
            let _header = header_from_obj_ptr(obj_ptr);
            let payload_bytes = crate::object::object_payload_size(obj_ptr);
            if payload_bytes < 3 * std::mem::size_of::<u64>() {
                return raise_exception::<i64>(
                    _py,
                    "RuntimeError",
                    "invalid socket_reader_readline payload",
                );
            }
            let payload_ptr = obj_ptr as *mut u64;
            let reader_bits = *payload_ptr.add(ASYNCIO_SOCKET_READER_READLINE_SLOT_READER);
            let out_bits = molt_socket_reader_readline(reader_bits);
            if out_bits as i64 != pending_bits_i64() {
                asyncio_drop_payload_slots(_py, payload_ptr, 3);
                return out_bits as i64;
            }
            let fd_bits = *payload_ptr.add(ASYNCIO_SOCKET_READER_READLINE_SLOT_FD);
            asyncio_pending_with_wait(
                _py,
                payload_ptr,
                ASYNCIO_SOCKET_READER_READLINE_SLOT_WAIT,
                fd_bits,
                ASYNCIO_SOCKET_IO_EVENT_READ,
            )
        })
    }
}

/// # Safety
/// - `sock_bits` must be a valid socket object.
#[unsafe(no_mangle)]
pub extern "C" fn molt_asyncio_sock_recv_new(sock_bits: u64, size_bits: u64, fd_bits: u64) -> u64 {
    crate::with_gil_entry_nopanic!(_py, {
        let obj_bits = molt_future_new(
            asyncio_sock_recv_poll_fn_addr(),
            (4 * std::mem::size_of::<u64>()) as u64,
        );
        if obj_from_bits(obj_bits).is_none() {
            return obj_bits;
        }
        let Some(obj_ptr) = resolve_obj_ptr(obj_bits) else {
            return MoltObject::none().bits();
        };
        let payload_ptr = obj_ptr as *mut u64;
        unsafe {
            *payload_ptr.add(ASYNCIO_SOCK_RECV_SLOT_SOCK) = sock_bits;
            *payload_ptr.add(ASYNCIO_SOCK_RECV_SLOT_SIZE) = size_bits;
            *payload_ptr.add(ASYNCIO_SOCK_RECV_SLOT_FD) = fd_bits;
            *payload_ptr.add(ASYNCIO_SOCK_RECV_SLOT_WAIT) = MoltObject::none().bits();
        }
        inc_ref_bits(_py, sock_bits);
        inc_ref_bits(_py, size_bits);
        inc_ref_bits(_py, fd_bits);
        obj_bits
    })
}

/// # Safety
/// - `obj_bits` must be a valid pointer to a socket recv wrapper future.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn molt_asyncio_sock_recv_poll(obj_bits: u64) -> i64 {
    unsafe {
        crate::with_gil_entry_nopanic!(_py, {
            let obj_ptr = ptr_from_bits(obj_bits);
            if obj_ptr.is_null() {
                return MoltObject::none().bits() as i64;
            }
            let _header = header_from_obj_ptr(obj_ptr);
            let payload_bytes = crate::object::object_payload_size(obj_ptr);
            if payload_bytes < 4 * std::mem::size_of::<u64>() {
                return raise_exception::<i64>(
                    _py,
                    "RuntimeError",
                    "invalid asyncio sock_recv payload",
                );
            }
            let payload_ptr = obj_ptr as *mut u64;
            let sock_bits = *payload_ptr.add(ASYNCIO_SOCK_RECV_SLOT_SOCK);
            let size_bits = *payload_ptr.add(ASYNCIO_SOCK_RECV_SLOT_SIZE);
            let flags_bits = MoltObject::from_int(asyncio_msg_dontwait()).bits();
            let out_bits = asyncio_call_method2(_py, sock_bits, b"recv", size_bits, flags_bits);
            if exception_pending(_py) {
                let exc_bits = asyncio_take_pending_exception_bits(_py);
                let errno = asyncio_oserror_errno_from_exception(_py, exc_bits).unwrap_or(i64::MIN);
                if errno != i64::MIN && asyncio_retryable_socket_errno(errno) {
                    dec_ref_bits(_py, exc_bits);
                    let fd_bits = *payload_ptr.add(ASYNCIO_SOCK_RECV_SLOT_FD);
                    return asyncio_pending_with_wait(
                        _py,
                        payload_ptr,
                        ASYNCIO_SOCK_RECV_SLOT_WAIT,
                        fd_bits,
                        ASYNCIO_SOCKET_IO_EVENT_READ,
                    );
                }
                let raised = molt_raise(exc_bits);
                dec_ref_bits(_py, exc_bits);
                return raised as i64;
            }
            asyncio_drop_payload_slots(_py, payload_ptr, 4);
            out_bits as i64
        })
    }
}

/// # Safety
/// - `sock_bits` must be a valid socket object.
#[unsafe(no_mangle)]
pub extern "C" fn molt_asyncio_sock_recv_into_new(
    sock_bits: u64,
    buf_bits: u64,
    nbytes_bits: u64,
    fd_bits: u64,
) -> u64 {
    crate::with_gil_entry_nopanic!(_py, {
        let obj_bits = molt_future_new(
            asyncio_sock_recv_into_poll_fn_addr(),
            (5 * std::mem::size_of::<u64>()) as u64,
        );
        if obj_from_bits(obj_bits).is_none() {
            return obj_bits;
        }
        let Some(obj_ptr) = resolve_obj_ptr(obj_bits) else {
            return MoltObject::none().bits();
        };
        let payload_ptr = obj_ptr as *mut u64;
        unsafe {
            *payload_ptr.add(ASYNCIO_SOCK_RECV_INTO_SLOT_SOCK) = sock_bits;
            *payload_ptr.add(ASYNCIO_SOCK_RECV_INTO_SLOT_BUF) = buf_bits;
            *payload_ptr.add(ASYNCIO_SOCK_RECV_INTO_SLOT_NBYTES) = nbytes_bits;
            *payload_ptr.add(ASYNCIO_SOCK_RECV_INTO_SLOT_FD) = fd_bits;
            *payload_ptr.add(ASYNCIO_SOCK_RECV_INTO_SLOT_WAIT) = MoltObject::none().bits();
        }
        inc_ref_bits(_py, sock_bits);
        inc_ref_bits(_py, buf_bits);
        inc_ref_bits(_py, nbytes_bits);
        inc_ref_bits(_py, fd_bits);
        obj_bits
    })
}

/// # Safety
/// - `obj_bits` must be a valid pointer to a socket recv_into wrapper future.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn molt_asyncio_sock_recv_into_poll(obj_bits: u64) -> i64 {
    unsafe {
        crate::with_gil_entry_nopanic!(_py, {
            let obj_ptr = ptr_from_bits(obj_bits);
            if obj_ptr.is_null() {
                return MoltObject::none().bits() as i64;
            }
            let _header = header_from_obj_ptr(obj_ptr);
            let payload_bytes = crate::object::object_payload_size(obj_ptr);
            if payload_bytes < 5 * std::mem::size_of::<u64>() {
                return raise_exception::<i64>(
                    _py,
                    "RuntimeError",
                    "invalid asyncio sock_recv_into payload",
                );
            }
            let payload_ptr = obj_ptr as *mut u64;
            let sock_bits = *payload_ptr.add(ASYNCIO_SOCK_RECV_INTO_SLOT_SOCK);
            let buf_bits = *payload_ptr.add(ASYNCIO_SOCK_RECV_INTO_SLOT_BUF);
            let nbytes_bits = *payload_ptr.add(ASYNCIO_SOCK_RECV_INTO_SLOT_NBYTES);
            let flags_bits = MoltObject::from_int(asyncio_msg_dontwait()).bits();
            let out_bits = asyncio_call_method3(
                _py,
                sock_bits,
                b"recv_into",
                buf_bits,
                nbytes_bits,
                flags_bits,
            );
            if exception_pending(_py) {
                let exc_bits = asyncio_take_pending_exception_bits(_py);
                let errno = asyncio_oserror_errno_from_exception(_py, exc_bits).unwrap_or(i64::MIN);
                if errno != i64::MIN && asyncio_retryable_socket_errno(errno) {
                    dec_ref_bits(_py, exc_bits);
                    let fd_bits = *payload_ptr.add(ASYNCIO_SOCK_RECV_INTO_SLOT_FD);
                    return asyncio_pending_with_wait(
                        _py,
                        payload_ptr,
                        ASYNCIO_SOCK_RECV_INTO_SLOT_WAIT,
                        fd_bits,
                        ASYNCIO_SOCKET_IO_EVENT_READ,
                    );
                }
                let raised = molt_raise(exc_bits);
                dec_ref_bits(_py, exc_bits);
                return raised as i64;
            }
            asyncio_drop_payload_slots(_py, payload_ptr, 5);
            out_bits as i64
        })
    }
}

/// # Safety
/// - `sock_bits` must be a valid socket object.
#[unsafe(no_mangle)]
pub extern "C" fn molt_asyncio_sock_sendall_new(
    sock_bits: u64,
    data_bits: u64,
    fd_bits: u64,
) -> u64 {
    crate::with_gil_entry_nopanic!(_py, {
        let data_len_bits = molt_len(data_bits);
        if exception_pending(_py) {
            return data_len_bits;
        }
        let data_len = to_i64(obj_from_bits(data_len_bits)).unwrap_or(-1);
        if data_len < 0 {
            if !obj_from_bits(data_len_bits).is_none() {
                dec_ref_bits(_py, data_len_bits);
            }
            return raise_exception::<u64>(_py, "TypeError", "invalid sendall payload");
        }
        let obj_bits = molt_future_new(
            asyncio_sock_sendall_poll_fn_addr(),
            (6 * std::mem::size_of::<u64>()) as u64,
        );
        if obj_from_bits(obj_bits).is_none() {
            if !obj_from_bits(data_len_bits).is_none() {
                dec_ref_bits(_py, data_len_bits);
            }
            return obj_bits;
        }
        let Some(obj_ptr) = resolve_obj_ptr(obj_bits) else {
            if !obj_from_bits(data_len_bits).is_none() {
                dec_ref_bits(_py, data_len_bits);
            }
            return MoltObject::none().bits();
        };
        let total_bits = MoltObject::from_int(0).bits();
        let payload_ptr = obj_ptr as *mut u64;
        unsafe {
            *payload_ptr.add(ASYNCIO_SOCK_SENDALL_SLOT_SOCK) = sock_bits;
            *payload_ptr.add(ASYNCIO_SOCK_SENDALL_SLOT_DATA) = data_bits;
            *payload_ptr.add(ASYNCIO_SOCK_SENDALL_SLOT_TOTAL) = total_bits;
            *payload_ptr.add(ASYNCIO_SOCK_SENDALL_SLOT_DLEN) = data_len_bits;
            *payload_ptr.add(ASYNCIO_SOCK_SENDALL_SLOT_FD) = fd_bits;
            *payload_ptr.add(ASYNCIO_SOCK_SENDALL_SLOT_WAIT) = MoltObject::none().bits();
        }
        inc_ref_bits(_py, sock_bits);
        inc_ref_bits(_py, data_bits);
        inc_ref_bits(_py, total_bits);
        inc_ref_bits(_py, data_len_bits);
        inc_ref_bits(_py, fd_bits);
        dec_ref_bits(_py, data_len_bits);
        obj_bits
    })
}

/// # Safety
/// - `obj_bits` must be a valid pointer to a socket sendall wrapper future.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn molt_asyncio_sock_sendall_poll(obj_bits: u64) -> i64 {
    unsafe {
        crate::with_gil_entry_nopanic!(_py, {
            let obj_ptr = ptr_from_bits(obj_bits);
            if obj_ptr.is_null() {
                return MoltObject::none().bits() as i64;
            }
            let _header = header_from_obj_ptr(obj_ptr);
            let payload_bytes = crate::object::object_payload_size(obj_ptr);
            if payload_bytes < 6 * std::mem::size_of::<u64>() {
                return raise_exception::<i64>(
                    _py,
                    "RuntimeError",
                    "invalid asyncio sock_sendall payload",
                );
            }
            let payload_ptr = obj_ptr as *mut u64;
            let sock_bits = *payload_ptr.add(ASYNCIO_SOCK_SENDALL_SLOT_SOCK);
            let data_bits = *payload_ptr.add(ASYNCIO_SOCK_SENDALL_SLOT_DATA);
            let data_len = to_i64(obj_from_bits(
                *payload_ptr.add(ASYNCIO_SOCK_SENDALL_SLOT_DLEN),
            ))
            .unwrap_or(0);

            for _ in 0..8 {
                let total_bits = *payload_ptr.add(ASYNCIO_SOCK_SENDALL_SLOT_TOTAL);
                let total = to_i64(obj_from_bits(total_bits)).unwrap_or(0);
                if total >= data_len {
                    asyncio_drop_payload_slots(_py, payload_ptr, 6);
                    return MoltObject::none().bits() as i64;
                }

                let slice_bits = molt_slice_new(
                    total_bits,
                    MoltObject::none().bits(),
                    MoltObject::none().bits(),
                );
                if obj_from_bits(slice_bits).is_none() {
                    return slice_bits as i64;
                }
                let tail_bits = molt_getitem_method(data_bits, slice_bits);
                dec_ref_bits(_py, slice_bits);
                if exception_pending(_py) {
                    if !obj_from_bits(tail_bits).is_none() {
                        dec_ref_bits(_py, tail_bits);
                    }
                    return tail_bits as i64;
                }

                let flags_bits = MoltObject::from_int(asyncio_msg_dontwait()).bits();
                let out_bits = asyncio_call_method2(_py, sock_bits, b"send", tail_bits, flags_bits);
                dec_ref_bits(_py, tail_bits);
                if exception_pending(_py) {
                    let exc_bits = asyncio_take_pending_exception_bits(_py);
                    let errno =
                        asyncio_oserror_errno_from_exception(_py, exc_bits).unwrap_or(i64::MIN);
                    if errno != i64::MIN && asyncio_retryable_socket_errno(errno) {
                        dec_ref_bits(_py, exc_bits);
                        let fd_bits = *payload_ptr.add(ASYNCIO_SOCK_SENDALL_SLOT_FD);
                        return asyncio_pending_with_wait(
                            _py,
                            payload_ptr,
                            ASYNCIO_SOCK_SENDALL_SLOT_WAIT,
                            fd_bits,
                            ASYNCIO_SOCKET_IO_EVENT_WRITE,
                        );
                    }
                    let raised = molt_raise(exc_bits);
                    dec_ref_bits(_py, exc_bits);
                    return raised as i64;
                }

                let sent = to_i64(obj_from_bits(out_bits)).unwrap_or(-1);
                dec_ref_bits(_py, out_bits);
                if sent <= 0 {
                    let fd_bits = *payload_ptr.add(ASYNCIO_SOCK_SENDALL_SLOT_FD);
                    return asyncio_pending_with_wait(
                        _py,
                        payload_ptr,
                        ASYNCIO_SOCK_SENDALL_SLOT_WAIT,
                        fd_bits,
                        ASYNCIO_SOCKET_IO_EVENT_WRITE,
                    );
                }

                let old_total_bits = *payload_ptr.add(ASYNCIO_SOCK_SENDALL_SLOT_TOTAL);
                let new_total = total.saturating_add(sent);
                let new_total_bits = MoltObject::from_int(new_total).bits();
                if old_total_bits != 0 && !obj_from_bits(old_total_bits).is_none() {
                    dec_ref_bits(_py, old_total_bits);
                }
                *payload_ptr.add(ASYNCIO_SOCK_SENDALL_SLOT_TOTAL) = new_total_bits;
                inc_ref_bits(_py, new_total_bits);
                if new_total >= data_len {
                    asyncio_drop_payload_slots(_py, payload_ptr, 6);
                    return MoltObject::none().bits() as i64;
                }
            }

            let fd_bits = *payload_ptr.add(ASYNCIO_SOCK_SENDALL_SLOT_FD);
            asyncio_pending_with_wait(
                _py,
                payload_ptr,
                ASYNCIO_SOCK_SENDALL_SLOT_WAIT,
                fd_bits,
                ASYNCIO_SOCKET_IO_EVENT_WRITE,
            )
        })
    }
}

/// # Safety
/// - `sock_bits` must be a valid socket object.
#[unsafe(no_mangle)]
pub extern "C" fn molt_asyncio_sock_recvfrom_new(
    sock_bits: u64,
    size_bits: u64,
    fd_bits: u64,
) -> u64 {
    crate::with_gil_entry_nopanic!(_py, {
        let obj_bits = molt_future_new(
            asyncio_sock_recvfrom_poll_fn_addr(),
            (4 * std::mem::size_of::<u64>()) as u64,
        );
        if obj_from_bits(obj_bits).is_none() {
            return obj_bits;
        }
        let Some(obj_ptr) = resolve_obj_ptr(obj_bits) else {
            return MoltObject::none().bits();
        };
        let payload_ptr = obj_ptr as *mut u64;
        unsafe {
            *payload_ptr.add(ASYNCIO_SOCK_RECVFROM_SLOT_SOCK) = sock_bits;
            *payload_ptr.add(ASYNCIO_SOCK_RECVFROM_SLOT_SIZE) = size_bits;
            *payload_ptr.add(ASYNCIO_SOCK_RECVFROM_SLOT_FD) = fd_bits;
            *payload_ptr.add(ASYNCIO_SOCK_RECVFROM_SLOT_WAIT) = MoltObject::none().bits();
        }
        inc_ref_bits(_py, sock_bits);
        inc_ref_bits(_py, size_bits);
        inc_ref_bits(_py, fd_bits);
        obj_bits
    })
}

/// # Safety
/// - `obj_bits` must be a valid pointer to a socket recvfrom wrapper future.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn molt_asyncio_sock_recvfrom_poll(obj_bits: u64) -> i64 {
    unsafe {
        crate::with_gil_entry_nopanic!(_py, {
            let obj_ptr = ptr_from_bits(obj_bits);
            if obj_ptr.is_null() {
                return MoltObject::none().bits() as i64;
            }
            let _header = header_from_obj_ptr(obj_ptr);
            let payload_bytes = crate::object::object_payload_size(obj_ptr);
            if payload_bytes < 4 * std::mem::size_of::<u64>() {
                return raise_exception::<i64>(
                    _py,
                    "RuntimeError",
                    "invalid asyncio sock_recvfrom payload",
                );
            }
            let payload_ptr = obj_ptr as *mut u64;
            let sock_bits = *payload_ptr.add(ASYNCIO_SOCK_RECVFROM_SLOT_SOCK);
            let size_bits = *payload_ptr.add(ASYNCIO_SOCK_RECVFROM_SLOT_SIZE);
            let flags_bits = MoltObject::from_int(asyncio_msg_dontwait()).bits();
            let out_bits = asyncio_call_method2(_py, sock_bits, b"recvfrom", size_bits, flags_bits);
            if exception_pending(_py) {
                let exc_bits = asyncio_take_pending_exception_bits(_py);
                let errno = asyncio_oserror_errno_from_exception(_py, exc_bits).unwrap_or(i64::MIN);
                if errno != i64::MIN && asyncio_retryable_socket_errno(errno) {
                    dec_ref_bits(_py, exc_bits);
                    let fd_bits = *payload_ptr.add(ASYNCIO_SOCK_RECVFROM_SLOT_FD);
                    return asyncio_pending_with_wait(
                        _py,
                        payload_ptr,
                        ASYNCIO_SOCK_RECVFROM_SLOT_WAIT,
                        fd_bits,
                        ASYNCIO_SOCKET_IO_EVENT_READ,
                    );
                }
                let raised = molt_raise(exc_bits);
                dec_ref_bits(_py, exc_bits);
                return raised as i64;
            }
            asyncio_drop_payload_slots(_py, payload_ptr, 4);
            out_bits as i64
        })
    }
}

/// # Safety
/// - `sock_bits` must be a valid socket object.
#[unsafe(no_mangle)]
pub extern "C" fn molt_asyncio_sock_recvfrom_into_new(
    sock_bits: u64,
    buf_bits: u64,
    nbytes_bits: u64,
    fd_bits: u64,
) -> u64 {
    crate::with_gil_entry_nopanic!(_py, {
        let obj_bits = molt_future_new(
            asyncio_sock_recvfrom_into_poll_fn_addr(),
            (5 * std::mem::size_of::<u64>()) as u64,
        );
        if obj_from_bits(obj_bits).is_none() {
            return obj_bits;
        }
        let Some(obj_ptr) = resolve_obj_ptr(obj_bits) else {
            return MoltObject::none().bits();
        };
        let payload_ptr = obj_ptr as *mut u64;
        unsafe {
            *payload_ptr.add(ASYNCIO_SOCK_RECVFROM_INTO_SLOT_SOCK) = sock_bits;
            *payload_ptr.add(ASYNCIO_SOCK_RECVFROM_INTO_SLOT_BUF) = buf_bits;
            *payload_ptr.add(ASYNCIO_SOCK_RECVFROM_INTO_SLOT_NBYTES) = nbytes_bits;
            *payload_ptr.add(ASYNCIO_SOCK_RECVFROM_INTO_SLOT_FD) = fd_bits;
            *payload_ptr.add(ASYNCIO_SOCK_RECVFROM_INTO_SLOT_WAIT) = MoltObject::none().bits();
        }
        inc_ref_bits(_py, sock_bits);
        inc_ref_bits(_py, buf_bits);
        inc_ref_bits(_py, nbytes_bits);
        inc_ref_bits(_py, fd_bits);
        obj_bits
    })
}

/// # Safety
/// - `obj_bits` must be a valid pointer to a socket recvfrom_into wrapper future.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn molt_asyncio_sock_recvfrom_into_poll(obj_bits: u64) -> i64 {
    unsafe {
        crate::with_gil_entry_nopanic!(_py, {
            let obj_ptr = ptr_from_bits(obj_bits);
            if obj_ptr.is_null() {
                return MoltObject::none().bits() as i64;
            }
            let _header = header_from_obj_ptr(obj_ptr);
            let payload_bytes = crate::object::object_payload_size(obj_ptr);
            if payload_bytes < 5 * std::mem::size_of::<u64>() {
                return raise_exception::<i64>(
                    _py,
                    "RuntimeError",
                    "invalid asyncio sock_recvfrom_into payload",
                );
            }
            let payload_ptr = obj_ptr as *mut u64;
            let sock_bits = *payload_ptr.add(ASYNCIO_SOCK_RECVFROM_INTO_SLOT_SOCK);
            let buf_bits = *payload_ptr.add(ASYNCIO_SOCK_RECVFROM_INTO_SLOT_BUF);
            let nbytes_bits = *payload_ptr.add(ASYNCIO_SOCK_RECVFROM_INTO_SLOT_NBYTES);
            let flags_bits = MoltObject::from_int(asyncio_msg_dontwait()).bits();
            let out_bits = asyncio_call_method3(
                _py,
                sock_bits,
                b"recvfrom_into",
                buf_bits,
                nbytes_bits,
                flags_bits,
            );
            if exception_pending(_py) {
                let exc_bits = asyncio_take_pending_exception_bits(_py);
                let errno = asyncio_oserror_errno_from_exception(_py, exc_bits).unwrap_or(i64::MIN);
                if errno != i64::MIN && asyncio_retryable_socket_errno(errno) {
                    dec_ref_bits(_py, exc_bits);
                    let fd_bits = *payload_ptr.add(ASYNCIO_SOCK_RECVFROM_INTO_SLOT_FD);
                    return asyncio_pending_with_wait(
                        _py,
                        payload_ptr,
                        ASYNCIO_SOCK_RECVFROM_INTO_SLOT_WAIT,
                        fd_bits,
                        ASYNCIO_SOCKET_IO_EVENT_READ,
                    );
                }
                let raised = molt_raise(exc_bits);
                dec_ref_bits(_py, exc_bits);
                return raised as i64;
            }
            asyncio_drop_payload_slots(_py, payload_ptr, 5);
            out_bits as i64
        })
    }
}

/// # Safety
/// - `sock_bits` must be a valid socket object.
#[unsafe(no_mangle)]
pub extern "C" fn molt_asyncio_sock_sendto_new(
    sock_bits: u64,
    data_bits: u64,
    addr_bits: u64,
    fd_bits: u64,
) -> u64 {
    crate::with_gil_entry_nopanic!(_py, {
        let obj_bits = molt_future_new(
            asyncio_sock_sendto_poll_fn_addr(),
            (5 * std::mem::size_of::<u64>()) as u64,
        );
        if obj_from_bits(obj_bits).is_none() {
            return obj_bits;
        }
        let Some(obj_ptr) = resolve_obj_ptr(obj_bits) else {
            return MoltObject::none().bits();
        };
        let payload_ptr = obj_ptr as *mut u64;
        unsafe {
            *payload_ptr.add(ASYNCIO_SOCK_SENDTO_SLOT_SOCK) = sock_bits;
            *payload_ptr.add(ASYNCIO_SOCK_SENDTO_SLOT_DATA) = data_bits;
            *payload_ptr.add(ASYNCIO_SOCK_SENDTO_SLOT_ADDR) = addr_bits;
            *payload_ptr.add(ASYNCIO_SOCK_SENDTO_SLOT_FD) = fd_bits;
            *payload_ptr.add(ASYNCIO_SOCK_SENDTO_SLOT_WAIT) = MoltObject::none().bits();
        }
        inc_ref_bits(_py, sock_bits);
        inc_ref_bits(_py, data_bits);
        inc_ref_bits(_py, addr_bits);
        inc_ref_bits(_py, fd_bits);
        obj_bits
    })
}

/// # Safety
/// - `obj_bits` must be a valid pointer to a socket sendto wrapper future.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn molt_asyncio_sock_sendto_poll(obj_bits: u64) -> i64 {
    unsafe {
        crate::with_gil_entry_nopanic!(_py, {
            let obj_ptr = ptr_from_bits(obj_bits);
            if obj_ptr.is_null() {
                return MoltObject::none().bits() as i64;
            }
            let _header = header_from_obj_ptr(obj_ptr);
            let payload_bytes = crate::object::object_payload_size(obj_ptr);
            if payload_bytes < 5 * std::mem::size_of::<u64>() {
                return raise_exception::<i64>(
                    _py,
                    "RuntimeError",
                    "invalid asyncio sock_sendto payload",
                );
            }
            let payload_ptr = obj_ptr as *mut u64;
            let sock_bits = *payload_ptr.add(ASYNCIO_SOCK_SENDTO_SLOT_SOCK);
            let data_bits = *payload_ptr.add(ASYNCIO_SOCK_SENDTO_SLOT_DATA);
            let addr_bits = *payload_ptr.add(ASYNCIO_SOCK_SENDTO_SLOT_ADDR);
            let flags_bits = MoltObject::from_int(asyncio_msg_dontwait()).bits();
            let out_bits =
                asyncio_call_method3(_py, sock_bits, b"sendto", data_bits, flags_bits, addr_bits);
            if exception_pending(_py) {
                let exc_bits = asyncio_take_pending_exception_bits(_py);
                let errno = asyncio_oserror_errno_from_exception(_py, exc_bits).unwrap_or(i64::MIN);
                if errno != i64::MIN && asyncio_retryable_socket_errno(errno) {
                    dec_ref_bits(_py, exc_bits);
                    let fd_bits = *payload_ptr.add(ASYNCIO_SOCK_SENDTO_SLOT_FD);
                    return asyncio_pending_with_wait(
                        _py,
                        payload_ptr,
                        ASYNCIO_SOCK_SENDTO_SLOT_WAIT,
                        fd_bits,
                        ASYNCIO_SOCKET_IO_EVENT_WRITE,
                    );
                }
                let raised = molt_raise(exc_bits);
                dec_ref_bits(_py, exc_bits);
                return raised as i64;
            }
            let sent = to_i64(obj_from_bits(out_bits)).unwrap_or(-1);
            if sent <= 0 {
                dec_ref_bits(_py, out_bits);
                let fd_bits = *payload_ptr.add(ASYNCIO_SOCK_SENDTO_SLOT_FD);
                return asyncio_pending_with_wait(
                    _py,
                    payload_ptr,
                    ASYNCIO_SOCK_SENDTO_SLOT_WAIT,
                    fd_bits,
                    ASYNCIO_SOCKET_IO_EVENT_WRITE,
                );
            }
            asyncio_drop_payload_slots(_py, payload_ptr, 5);
            out_bits as i64
        })
    }
}

/// # Safety
/// - `sock_bits` must be a valid socket object.
#[unsafe(no_mangle)]
pub extern "C" fn molt_asyncio_sock_connect_new(
    sock_bits: u64,
    addr_bits: u64,
    fd_bits: u64,
) -> u64 {
    crate::with_gil_entry_nopanic!(_py, {
        let obj_bits = molt_future_new(
            asyncio_sock_connect_poll_fn_addr(),
            (4 * std::mem::size_of::<u64>()) as u64,
        );
        if obj_from_bits(obj_bits).is_none() {
            return obj_bits;
        }
        let Some(obj_ptr) = resolve_obj_ptr(obj_bits) else {
            return MoltObject::none().bits();
        };
        let payload_ptr = obj_ptr as *mut u64;
        unsafe {
            *payload_ptr.add(ASYNCIO_SOCK_CONNECT_SLOT_SOCK) = sock_bits;
            *payload_ptr.add(ASYNCIO_SOCK_CONNECT_SLOT_ADDR) = addr_bits;
            *payload_ptr.add(ASYNCIO_SOCK_CONNECT_SLOT_FD) = fd_bits;
            *payload_ptr.add(ASYNCIO_SOCK_CONNECT_SLOT_WAIT) = MoltObject::none().bits();
        }
        inc_ref_bits(_py, sock_bits);
        inc_ref_bits(_py, addr_bits);
        inc_ref_bits(_py, fd_bits);
        obj_bits
    })
}

/// # Safety
/// - `obj_bits` must be a valid pointer to a socket connect wrapper future.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn molt_asyncio_sock_connect_poll(obj_bits: u64) -> i64 {
    unsafe {
        crate::with_gil_entry_nopanic!(_py, {
            let obj_ptr = ptr_from_bits(obj_bits);
            if obj_ptr.is_null() {
                return MoltObject::none().bits() as i64;
            }
            let _header = header_from_obj_ptr(obj_ptr);
            let payload_bytes = crate::object::object_payload_size(obj_ptr);
            if payload_bytes < 4 * std::mem::size_of::<u64>() {
                return raise_exception::<i64>(
                    _py,
                    "RuntimeError",
                    "invalid asyncio sock_connect payload",
                );
            }
            let payload_ptr = obj_ptr as *mut u64;
            let sock_bits = *payload_ptr.add(ASYNCIO_SOCK_CONNECT_SLOT_SOCK);
            let addr_bits = *payload_ptr.add(ASYNCIO_SOCK_CONNECT_SLOT_ADDR);
            let mut allow_immediate_retry = true;
            loop {
                let rc_bits = asyncio_call_method1(_py, sock_bits, b"connect_ex", addr_bits);
                if exception_pending(_py) {
                    let exc_bits = asyncio_take_pending_exception_bits(_py);
                    let errno =
                        asyncio_oserror_errno_from_exception(_py, exc_bits).unwrap_or(i64::MIN);
                    if asyncio_connect_trace_enabled() {
                        eprintln!(
                            "molt async connect: exception errno={} sock=0x{:x}",
                            errno, sock_bits
                        );
                    }
                    if errno != i64::MIN && asyncio_retryable_socket_errno(errno) {
                        dec_ref_bits(_py, exc_bits);
                        if asyncio_socket_is_connected(_py, sock_bits) {
                            if asyncio_connect_trace_enabled() {
                                eprintln!(
                                    "molt async connect: connected-via-getpeername sock=0x{:x}",
                                    sock_bits
                                );
                            }
                            asyncio_drop_payload_slots(_py, payload_ptr, 4);
                            return MoltObject::none().bits() as i64;
                        }
                        let pending = asyncio_pending_with_connect_retry(
                            _py,
                            payload_ptr,
                            ASYNCIO_SOCK_CONNECT_SLOT_WAIT,
                            *payload_ptr.add(ASYNCIO_SOCK_CONNECT_SLOT_FD),
                        );
                        if pending == pending_bits_i64()
                            && allow_immediate_retry
                            && obj_from_bits(*payload_ptr.add(ASYNCIO_SOCK_CONNECT_SLOT_WAIT))
                                .is_none()
                        {
                            allow_immediate_retry = false;
                            continue;
                        }
                        if asyncio_connect_trace_enabled() {
                            eprintln!(
                                "molt async connect: waiting errno={} wait_slot_none={}",
                                errno,
                                obj_from_bits(*payload_ptr.add(ASYNCIO_SOCK_CONNECT_SLOT_WAIT))
                                    .is_none()
                            );
                        }
                        return pending;
                    }
                    let raised = molt_raise(exc_bits);
                    dec_ref_bits(_py, exc_bits);
                    return raised as i64;
                }
                let rc = to_i64(obj_from_bits(rc_bits)).unwrap_or(libc::EINVAL as i64);
                dec_ref_bits(_py, rc_bits);
                if asyncio_connect_trace_enabled() {
                    eprintln!("molt async connect: rc={} sock=0x{:x}", rc, sock_bits);
                }
                if rc == 0 || rc == libc::EISCONN as i64 {
                    asyncio_drop_payload_slots(_py, payload_ptr, 4);
                    return MoltObject::none().bits() as i64;
                }
                if asyncio_retryable_socket_errno(rc) {
                    if asyncio_socket_is_connected(_py, sock_bits) {
                        if asyncio_connect_trace_enabled() {
                            eprintln!(
                                "molt async connect: connected-after-retry sock=0x{:x}",
                                sock_bits
                            );
                        }
                        asyncio_drop_payload_slots(_py, payload_ptr, 4);
                        return MoltObject::none().bits() as i64;
                    }
                    let pending = asyncio_pending_with_connect_retry(
                        _py,
                        payload_ptr,
                        ASYNCIO_SOCK_CONNECT_SLOT_WAIT,
                        *payload_ptr.add(ASYNCIO_SOCK_CONNECT_SLOT_FD),
                    );
                    if pending == pending_bits_i64()
                        && allow_immediate_retry
                        && obj_from_bits(*payload_ptr.add(ASYNCIO_SOCK_CONNECT_SLOT_WAIT)).is_none()
                    {
                        allow_immediate_retry = false;
                        continue;
                    }
                    if asyncio_connect_trace_enabled() {
                        eprintln!(
                            "molt async connect: pending rc={} wait_slot_none={}",
                            rc,
                            obj_from_bits(*payload_ptr.add(ASYNCIO_SOCK_CONNECT_SLOT_WAIT))
                                .is_none()
                        );
                    }
                    return pending;
                }
                asyncio_drop_payload_slots(_py, payload_ptr, 4);
                return raise_os_error_errno::<i64>(_py, rc, "connect");
            }
        })
    }
}

/// # Safety
/// - `sock_bits` must be a valid socket object.
#[unsafe(no_mangle)]
pub extern "C" fn molt_asyncio_sock_accept_new(sock_bits: u64, fd_bits: u64) -> u64 {
    crate::with_gil_entry_nopanic!(_py, {
        let obj_bits = molt_future_new(
            asyncio_sock_accept_poll_fn_addr(),
            (3 * std::mem::size_of::<u64>()) as u64,
        );
        if obj_from_bits(obj_bits).is_none() {
            return obj_bits;
        }
        let Some(obj_ptr) = resolve_obj_ptr(obj_bits) else {
            return MoltObject::none().bits();
        };
        let payload_ptr = obj_ptr as *mut u64;
        unsafe {
            *payload_ptr.add(ASYNCIO_SOCK_ACCEPT_SLOT_SOCK) = sock_bits;
            *payload_ptr.add(ASYNCIO_SOCK_ACCEPT_SLOT_FD) = fd_bits;
            *payload_ptr.add(ASYNCIO_SOCK_ACCEPT_SLOT_WAIT) = MoltObject::none().bits();
        }
        inc_ref_bits(_py, sock_bits);
        inc_ref_bits(_py, fd_bits);
        obj_bits
    })
}

/// # Safety
/// - `obj_bits` must be a valid pointer to a socket accept wrapper future.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn molt_asyncio_sock_accept_poll(obj_bits: u64) -> i64 {
    unsafe {
        crate::with_gil_entry_nopanic!(_py, {
            let obj_ptr = ptr_from_bits(obj_bits);
            if obj_ptr.is_null() {
                return MoltObject::none().bits() as i64;
            }
            let _header = header_from_obj_ptr(obj_ptr);
            let payload_bytes = crate::object::object_payload_size(obj_ptr);
            if payload_bytes < 3 * std::mem::size_of::<u64>() {
                return raise_exception::<i64>(
                    _py,
                    "RuntimeError",
                    "invalid asyncio sock_accept payload",
                );
            }
            let payload_ptr = obj_ptr as *mut u64;
            let sock_bits = *payload_ptr.add(ASYNCIO_SOCK_ACCEPT_SLOT_SOCK);
            let out_bits = asyncio_call_method0(_py, sock_bits, b"accept");
            if exception_pending(_py) {
                let exc_bits = asyncio_take_pending_exception_bits(_py);
                let errno = asyncio_oserror_errno_from_exception(_py, exc_bits).unwrap_or(i64::MIN);
                if errno != i64::MIN && asyncio_retryable_socket_errno(errno) {
                    dec_ref_bits(_py, exc_bits);
                    let fd_bits = *payload_ptr.add(ASYNCIO_SOCK_ACCEPT_SLOT_FD);
                    return asyncio_pending_with_wait(
                        _py,
                        payload_ptr,
                        ASYNCIO_SOCK_ACCEPT_SLOT_WAIT,
                        fd_bits,
                        ASYNCIO_SOCKET_IO_EVENT_READ,
                    );
                }
                let raised = molt_raise(exc_bits);
                dec_ref_bits(_py, exc_bits);
                return raised as i64;
            }
            asyncio_drop_payload_slots(_py, payload_ptr, 3);
            out_bits as i64
        })
    }
}
