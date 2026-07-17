//! Asyncio stream reader/writer wrapper futures.
//!
//! Owns stream payload layouts, polling state machines, buffer helpers,
//! and drop hooks for stream-reader/readline/send-all futures.

use super::*;

const ASYNCIO_STREAM_READER_READ_SLOT_READER: usize = 0;
const ASYNCIO_STREAM_READER_READ_SLOT_N: usize = 1;
const ASYNCIO_STREAM_READER_READ_SLOT_WAIT: usize = 2;
const ASYNCIO_STREAM_READER_READLINE_SLOT_READER: usize = 0;
const ASYNCIO_STREAM_READER_READLINE_SLOT_WAIT: usize = 1;
const ASYNCIO_STREAM_SEND_ALL_SLOT_STREAM: usize = 0;
const ASYNCIO_STREAM_SEND_ALL_SLOT_DATA: usize = 1;
const ASYNCIO_STREAM_SEND_ALL_SLOT_WAIT: usize = 2;

#[unsafe(no_mangle)]
pub extern "C" fn molt_asyncio_stream_reader_read_new(reader_bits: u64, n_bits: u64) -> u64 {
    crate::with_gil_entry_nopanic!(_py, {
        let obj_bits = molt_future_new(
            asyncio_stream_reader_read_poll_fn_addr(),
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
            *payload_ptr.add(ASYNCIO_STREAM_READER_READ_SLOT_READER) = reader_bits;
            *payload_ptr.add(ASYNCIO_STREAM_READER_READ_SLOT_N) = n_bits;
            *payload_ptr.add(ASYNCIO_STREAM_READER_READ_SLOT_WAIT) = MoltObject::none().bits();
        }
        inc_ref_bits(_py, reader_bits);
        inc_ref_bits(_py, n_bits);
        obj_bits
    })
}

/// # Safety
/// - `obj_bits` must be a valid pointer to a stream-reader read wrapper future.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn molt_asyncio_stream_reader_read_poll(obj_bits: u64) -> i64 {
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
                    "invalid stream_reader_read payload",
                );
            }
            let payload_ptr = obj_ptr as *mut u64;
            let reader_bits = *payload_ptr.add(ASYNCIO_STREAM_READER_READ_SLOT_READER);
            let n_bits = *payload_ptr.add(ASYNCIO_STREAM_READER_READ_SLOT_N);
            let out_bits = molt_stream_reader_read(reader_bits, n_bits);
            if out_bits as i64 != pending_bits_i64() {
                asyncio_drop_payload_slots(_py, payload_ptr, 3);
                return out_bits as i64;
            }
            asyncio_pending_with_wait(
                _py,
                payload_ptr,
                ASYNCIO_STREAM_READER_READ_SLOT_WAIT,
                MoltObject::none().bits(),
                ASYNCIO_SOCKET_IO_EVENT_READ,
            )
        })
    }
}

/// # Safety
/// - `reader_bits` must be a valid stream-reader handle.
#[unsafe(no_mangle)]
pub extern "C" fn molt_asyncio_stream_reader_readline_new(reader_bits: u64) -> u64 {
    crate::with_gil_entry_nopanic!(_py, {
        let obj_bits = molt_future_new(
            asyncio_stream_reader_readline_poll_fn_addr(),
            (2 * std::mem::size_of::<u64>()) as u64,
        );
        if obj_from_bits(obj_bits).is_none() {
            return obj_bits;
        }
        let Some(obj_ptr) = resolve_obj_ptr(obj_bits) else {
            return MoltObject::none().bits();
        };
        let payload_ptr = obj_ptr as *mut u64;
        unsafe {
            *payload_ptr.add(ASYNCIO_STREAM_READER_READLINE_SLOT_READER) = reader_bits;
            *payload_ptr.add(ASYNCIO_STREAM_READER_READLINE_SLOT_WAIT) = MoltObject::none().bits();
        }
        inc_ref_bits(_py, reader_bits);
        obj_bits
    })
}

/// # Safety
/// - `obj_bits` must be a valid pointer to a stream-reader readline wrapper future.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn molt_asyncio_stream_reader_readline_poll(obj_bits: u64) -> i64 {
    unsafe {
        crate::with_gil_entry_nopanic!(_py, {
            let obj_ptr = ptr_from_bits(obj_bits);
            if obj_ptr.is_null() {
                return MoltObject::none().bits() as i64;
            }
            let _header = header_from_obj_ptr(obj_ptr);
            let payload_bytes = crate::object::object_payload_size(obj_ptr);
            if payload_bytes < 2 * std::mem::size_of::<u64>() {
                return raise_exception::<i64>(
                    _py,
                    "RuntimeError",
                    "invalid stream_reader_readline payload",
                );
            }
            let payload_ptr = obj_ptr as *mut u64;
            let reader_bits = *payload_ptr.add(ASYNCIO_STREAM_READER_READLINE_SLOT_READER);
            let out_bits = molt_stream_reader_readline(reader_bits);
            if out_bits as i64 != pending_bits_i64() {
                asyncio_drop_payload_slots(_py, payload_ptr, 2);
                return out_bits as i64;
            }
            asyncio_pending_with_wait(
                _py,
                payload_ptr,
                ASYNCIO_STREAM_READER_READLINE_SLOT_WAIT,
                MoltObject::none().bits(),
                ASYNCIO_SOCKET_IO_EVENT_READ,
            )
        })
    }
}

/// # Safety
/// - `stream_bits` must be a valid stream handle.
#[unsafe(no_mangle)]
pub extern "C" fn molt_asyncio_stream_send_all_new(stream_bits: u64, data_bits: u64) -> u64 {
    crate::with_gil_entry_nopanic!(_py, {
        let obj_bits = molt_future_new(
            asyncio_stream_send_all_poll_fn_addr(),
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
            *payload_ptr.add(ASYNCIO_STREAM_SEND_ALL_SLOT_STREAM) = stream_bits;
            *payload_ptr.add(ASYNCIO_STREAM_SEND_ALL_SLOT_DATA) = data_bits;
            *payload_ptr.add(ASYNCIO_STREAM_SEND_ALL_SLOT_WAIT) = MoltObject::none().bits();
        }
        inc_ref_bits(_py, stream_bits);
        inc_ref_bits(_py, data_bits);
        obj_bits
    })
}

/// # Safety
/// - `obj_bits` must be a valid pointer to a stream-send wrapper future.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn molt_asyncio_stream_send_all_poll(obj_bits: u64) -> i64 {
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
                    "invalid stream_send_all payload",
                );
            }
            let payload_ptr = obj_ptr as *mut u64;
            let stream_bits = *payload_ptr.add(ASYNCIO_STREAM_SEND_ALL_SLOT_STREAM);
            let data_bits = *payload_ptr.add(ASYNCIO_STREAM_SEND_ALL_SLOT_DATA);
            let out_bits = molt_stream_send_obj(stream_bits, data_bits);
            if out_bits as i64 == pending_bits_i64() {
                return asyncio_pending_with_wait(
                    _py,
                    payload_ptr,
                    ASYNCIO_STREAM_SEND_ALL_SLOT_WAIT,
                    MoltObject::none().bits(),
                    ASYNCIO_SOCKET_IO_EVENT_READ,
                );
            }
            if exception_pending(_py) {
                asyncio_drop_payload_slots(_py, payload_ptr, 3);
                return out_bits as i64;
            }
            let sent = to_i64(obj_from_bits(out_bits)).unwrap_or(-1);
            if sent == 0 {
                asyncio_drop_payload_slots(_py, payload_ptr, 3);
                return MoltObject::none().bits() as i64;
            }
            asyncio_pending_with_wait(
                _py,
                payload_ptr,
                ASYNCIO_STREAM_SEND_ALL_SLOT_WAIT,
                MoltObject::none().bits(),
                ASYNCIO_SOCKET_IO_EVENT_READ,
            )
        })
    }
}

/// # Safety
/// - `buffer_bits` must be bytes-like.
#[unsafe(no_mangle)]
pub extern "C" fn molt_asyncio_stream_buffer_snapshot(buffer_bits: u64) -> u64 {
    crate::with_gil_entry_nopanic!(_py, {
        let out_bits = molt_bytes_from_obj(buffer_bits);
        if exception_pending(_py) {
            return out_bits;
        }
        out_bits
    })
}

/// # Safety
/// - `buffer_bits` must be a mutable bytearray-like object.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn molt_asyncio_stream_buffer_consume(
    buffer_bits: u64,
    count_bits: u64,
) -> u64 {
    unsafe {
        crate::with_gil_entry_nopanic!(_py, {
            let Some(mut count) = to_i64(obj_from_bits(count_bits)) else {
                return raise_exception::<u64>(
                    _py,
                    "TypeError",
                    "stream consume count must be int",
                );
            };
            if count <= 0 {
                return MoltObject::from_int(0).bits();
            }
            let len_bits = molt_len(buffer_bits);
            if exception_pending(_py) {
                return MoltObject::none().bits();
            }
            let Some(buf_len) = to_i64(obj_from_bits(len_bits)) else {
                if !obj_from_bits(len_bits).is_none() {
                    dec_ref_bits(_py, len_bits);
                }
                return raise_exception::<u64>(_py, "TypeError", "stream buffer must be sized");
            };
            if !obj_from_bits(len_bits).is_none() {
                dec_ref_bits(_py, len_bits);
            }
            if buf_len <= 0 {
                return MoltObject::from_int(0).bits();
            }
            count = count.min(buf_len);

            if count == buf_len {
                let clear_bits = asyncio_call_method0(_py, buffer_bits, b"clear");
                if exception_pending(_py) {
                    return clear_bits;
                }
                if !obj_from_bits(clear_bits).is_none() {
                    dec_ref_bits(_py, clear_bits);
                }
                return MoltObject::from_int(count).bits();
            }

            let slice_bits = molt_slice_new(
                MoltObject::from_int(0).bits(),
                MoltObject::from_int(count).bits(),
                MoltObject::none().bits(),
            );
            if obj_from_bits(slice_bits).is_none() {
                return slice_bits;
            }
            let del_bits = asyncio_call_method1(_py, buffer_bits, b"__delitem__", slice_bits);
            if !obj_from_bits(slice_bits).is_none() {
                dec_ref_bits(_py, slice_bits);
            }
            if exception_pending(_py) {
                return del_bits;
            }
            if !obj_from_bits(del_bits).is_none() {
                dec_ref_bits(_py, del_bits);
            }
            MoltObject::from_int(count).bits()
        })
    }
}
