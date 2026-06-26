// Buffered socket reader authority for socket.makefile() and asyncio stream readers.
// Owns reader handle lifetime, refill, compaction, EOF, and line slicing semantics.

#[cfg(any(molt_has_net_io, target_arch = "wasm32"))]
use super::*;

#[cfg(any(molt_has_net_io, target_arch = "wasm32"))]
struct MoltSocketReader {
    socket_bits: u64,
    buffer: Vec<u8>,
    buffer_start: usize,
    scan_cursor: usize,
    eof: bool,
}

#[cfg(any(molt_has_net_io, target_arch = "wasm32"))]
enum SocketReaderPull {
    Eof,
    Data,
}

#[cfg(any(molt_has_net_io, target_arch = "wasm32"))]
const SOCKET_READER_COMPACT_PREFIX_MIN: usize = 4096;

#[cfg(any(molt_has_net_io, target_arch = "wasm32"))]
unsafe fn socket_reader_pull(
    _py: &PyToken<'_>,
    reader: &mut MoltSocketReader,
) -> Result<SocketReaderPull, u64> {
    unsafe {
        if reader.eof {
            return Ok(SocketReaderPull::Eof);
        }
        let recv_bits = molt_socket_recv(
            reader.socket_bits,
            MoltObject::from_int(4096).bits(),
            MoltObject::from_int(0).bits(),
        );
        if exception_pending(_py) {
            return Err(MoltObject::none().bits());
        }
        let recv_obj = obj_from_bits(recv_bits);
        let Some(recv_ptr) = recv_obj.as_ptr() else {
            return Err(raise_exception::<u64>(
                _py,
                "RuntimeError",
                "socket recv intrinsic returned invalid value",
            ));
        };
        if object_type_id(recv_ptr) != TYPE_ID_BYTES {
            dec_ref_bits(_py, recv_bits);
            return Err(raise_exception::<u64>(
                _py,
                "RuntimeError",
                "socket recv intrinsic returned invalid value",
            ));
        }
        let n = bytes_len(recv_ptr);
        if n == 0 {
            reader.eof = true;
            dec_ref_bits(_py, recv_bits);
            return Ok(SocketReaderPull::Eof);
        }
        let bytes = std::slice::from_raw_parts(bytes_data(recv_ptr), n);
        reader.buffer.extend_from_slice(bytes);
        dec_ref_bits(_py, recv_bits);
        Ok(SocketReaderPull::Data)
    }
}

#[cfg(any(molt_has_net_io, target_arch = "wasm32"))]
#[inline]
fn socket_reader_unread_len(reader: &MoltSocketReader) -> usize {
    reader.buffer.len().saturating_sub(reader.buffer_start)
}

#[cfg(any(molt_has_net_io, target_arch = "wasm32"))]
#[inline]
fn socket_reader_unread_is_empty(reader: &MoltSocketReader) -> bool {
    socket_reader_unread_len(reader) == 0
}

#[cfg(any(molt_has_net_io, target_arch = "wasm32"))]
#[inline]
fn socket_reader_unread_slice(reader: &MoltSocketReader) -> &[u8] {
    &reader.buffer[reader.buffer_start..]
}

#[cfg(any(molt_has_net_io, target_arch = "wasm32"))]
fn socket_reader_maybe_compact(reader: &mut MoltSocketReader) {
    let consumed = reader.buffer_start;
    if consumed == 0 {
        return;
    }
    if consumed >= reader.buffer.len() {
        reader.buffer.clear();
        reader.buffer_start = 0;
        reader.scan_cursor = 0;
        return;
    }
    if consumed < SOCKET_READER_COMPACT_PREFIX_MIN
        || consumed.saturating_mul(2) < reader.buffer.len()
    {
        return;
    }
    let remaining = reader.buffer.len() - consumed;
    reader.buffer.copy_within(consumed.., 0);
    reader.buffer.truncate(remaining);
    reader.buffer_start = 0;
    reader.scan_cursor = reader.scan_cursor.saturating_sub(consumed);
}

#[cfg(any(molt_has_net_io, target_arch = "wasm32"))]
fn socket_reader_find_newline_up_to(
    reader: &mut MoltSocketReader,
    max_bytes: Option<usize>,
) -> Option<usize> {
    let unread_start = reader.buffer_start;
    let unread_end = reader.buffer.len();
    let search_end = match max_bytes {
        Some(limit) => unread_start.saturating_add(limit).min(unread_end),
        None => unread_end,
    };
    let search_start = reader.scan_cursor.max(unread_start).min(search_end);
    if search_start == search_end {
        reader.scan_cursor = reader.scan_cursor.max(search_end);
        return None;
    }
    match reader.buffer[search_start..search_end]
        .iter()
        .position(|&b| b == b'\n')
    {
        Some(rel_idx) => {
            let idx = search_start + rel_idx;
            reader.scan_cursor = idx.saturating_add(1);
            Some(idx - unread_start)
        }
        None => {
            reader.scan_cursor = reader.scan_cursor.max(search_end);
            None
        }
    }
}

#[cfg(any(molt_has_net_io, target_arch = "wasm32"))]
fn socket_reader_take(_py: &PyToken<'_>, reader: &mut MoltSocketReader, count: usize) -> u64 {
    let n = count.min(socket_reader_unread_len(reader));
    let unread = socket_reader_unread_slice(reader);
    let ptr = alloc_bytes(_py, &unread[..n]);
    if ptr.is_null() {
        reader.scan_cursor = reader.buffer_start;
        return MoltObject::none().bits();
    }
    reader.buffer_start += n;
    reader.scan_cursor = reader.scan_cursor.max(reader.buffer_start);
    socket_reader_maybe_compact(reader);
    MoltObject::from_ptr(ptr).bits()
}

#[cfg(any(molt_has_net_io, target_arch = "wasm32"))]
#[unsafe(no_mangle)]
/// # Safety
/// Caller must pass a valid socket handle from `molt_socket_new`/`molt_socket_clone`.
pub unsafe extern "C" fn molt_socket_reader_new(sock_bits: u64) -> u64 {
    crate::with_gil_entry_nopanic!(_py, {
        let clone_bits = unsafe { molt_socket_clone(sock_bits) };
        if obj_from_bits(clone_bits).is_none() {
            return MoltObject::none().bits();
        }
        let reader = Box::new(MoltSocketReader {
            socket_bits: clone_bits,
            buffer: Vec::new(),
            buffer_start: 0,
            scan_cursor: 0,
            eof: false,
        });
        opaque_handle_bits(Box::into_raw(reader) as *mut u8)
    })
}

#[cfg(any(molt_has_net_io, target_arch = "wasm32"))]
#[unsafe(no_mangle)]
/// # Safety
/// Caller must pass a valid socket reader handle from `molt_socket_reader_new`.
pub unsafe extern "C" fn molt_socket_reader_drop(reader_bits: u64) {
    unsafe {
        crate::with_gil_entry_nopanic!(_py, {
            let reader_ptr = ptr_from_bits(reader_bits);
            if reader_ptr.is_null() {
                return;
            }
            let reader = Box::from_raw(reader_ptr as *mut MoltSocketReader);
            molt_socket_drop(reader.socket_bits);
        })
    }
}

#[cfg(any(molt_has_net_io, target_arch = "wasm32"))]
#[unsafe(no_mangle)]
/// # Safety
/// Caller must pass a valid socket reader handle from `molt_socket_reader_new`.
pub unsafe extern "C" fn molt_socket_reader_at_eof(reader_bits: u64) -> u64 {
    unsafe {
        crate::with_gil_entry_nopanic!(_py, {
            let reader_ptr = ptr_from_bits(reader_bits);
            if reader_ptr.is_null() {
                return MoltObject::from_bool(true).bits();
            }
            let reader = &*(reader_ptr as *mut MoltSocketReader);
            MoltObject::from_bool(reader.eof && socket_reader_unread_is_empty(reader)).bits()
        })
    }
}

#[cfg(any(molt_has_net_io, target_arch = "wasm32"))]
#[unsafe(no_mangle)]
/// # Safety
/// Caller must pass a valid socket reader handle from `molt_socket_reader_new`.
pub unsafe extern "C" fn molt_socket_reader_read(reader_bits: u64, n_bits: u64) -> u64 {
    unsafe {
        crate::with_gil_entry_nopanic!(_py, {
            let reader_ptr = ptr_from_bits(reader_bits);
            if reader_ptr.is_null() {
                return MoltObject::none().bits();
            }
            let reader = &mut *(reader_ptr as *mut MoltSocketReader);
            let n = to_i64(obj_from_bits(n_bits)).unwrap_or(-1);
            if n == 0 {
                let ptr = alloc_bytes(_py, &[]);
                if ptr.is_null() {
                    return MoltObject::none().bits();
                }
                return MoltObject::from_ptr(ptr).bits();
            }
            if n < 0 {
                loop {
                    if reader.eof {
                        return socket_reader_take(_py, reader, socket_reader_unread_len(reader));
                    }
                    match socket_reader_pull(_py, reader) {
                        Ok(SocketReaderPull::Eof) | Ok(SocketReaderPull::Data) => {}
                        Err(bits) => return bits,
                    }
                }
            }
            if !socket_reader_unread_is_empty(reader) {
                return socket_reader_take(_py, reader, n as usize);
            }
            if reader.eof {
                let ptr = alloc_bytes(_py, &[]);
                if ptr.is_null() {
                    return MoltObject::none().bits();
                }
                return MoltObject::from_ptr(ptr).bits();
            }
            loop {
                match socket_reader_pull(_py, reader) {
                    Ok(SocketReaderPull::Eof) => {
                        if socket_reader_unread_is_empty(reader) {
                            let ptr = alloc_bytes(_py, &[]);
                            if ptr.is_null() {
                                return MoltObject::none().bits();
                            }
                            return MoltObject::from_ptr(ptr).bits();
                        }
                        return socket_reader_take(_py, reader, n as usize);
                    }
                    Ok(SocketReaderPull::Data) => {
                        if !socket_reader_unread_is_empty(reader) {
                            return socket_reader_take(_py, reader, n as usize);
                        }
                    }
                    Err(bits) => return bits,
                }
            }
        })
    }
}

#[cfg(any(molt_has_net_io, target_arch = "wasm32"))]
#[unsafe(no_mangle)]
/// # Safety
/// Caller must pass a valid socket reader handle from `molt_socket_reader_new`.
pub unsafe extern "C" fn molt_socket_reader_readline(reader_bits: u64) -> u64 {
    unsafe {
        crate::with_gil_entry_nopanic!(_py, {
            let reader_ptr = ptr_from_bits(reader_bits);
            if reader_ptr.is_null() {
                return MoltObject::none().bits();
            }
            let reader = &mut *(reader_ptr as *mut MoltSocketReader);
            loop {
                if let Some(idx) = socket_reader_find_newline_up_to(reader, None) {
                    return socket_reader_take(_py, reader, idx + 1);
                }
                if reader.eof {
                    return socket_reader_take(_py, reader, socket_reader_unread_len(reader));
                }
                match socket_reader_pull(_py, reader) {
                    Ok(SocketReaderPull::Eof) | Ok(SocketReaderPull::Data) => {}
                    Err(bits) => return bits,
                }
            }
        })
    }
}

#[cfg(any(molt_has_net_io, target_arch = "wasm32"))]
#[unsafe(no_mangle)]
/// # Safety
/// Caller must pass a valid socket reader handle from `molt_socket_reader_new`.
pub unsafe extern "C" fn molt_socket_reader_readline_limit(
    reader_bits: u64,
    limit_bits: u64,
) -> u64 {
    unsafe {
        crate::with_gil_entry_nopanic!(_py, {
            let reader_ptr = ptr_from_bits(reader_bits);
            if reader_ptr.is_null() {
                return MoltObject::none().bits();
            }
            let Some(limit_raw) = to_i64(obj_from_bits(limit_bits)) else {
                return raise_exception::<u64>(_py, "TypeError", "size must be an integer");
            };
            let reader = &mut *(reader_ptr as *mut MoltSocketReader);
            if limit_raw == 0 {
                let ptr = alloc_bytes(_py, &[]);
                if ptr.is_null() {
                    return MoltObject::none().bits();
                }
                return MoltObject::from_ptr(ptr).bits();
            }
            if limit_raw < 0 {
                loop {
                    if let Some(idx) = socket_reader_find_newline_up_to(reader, None) {
                        return socket_reader_take(_py, reader, idx + 1);
                    }
                    if reader.eof {
                        return socket_reader_take(_py, reader, socket_reader_unread_len(reader));
                    }
                    match socket_reader_pull(_py, reader) {
                        Ok(SocketReaderPull::Eof) | Ok(SocketReaderPull::Data) => {}
                        Err(bits) => return bits,
                    }
                }
            }
            let limit = usize::try_from(limit_raw).unwrap_or(usize::MAX);
            loop {
                if let Some(idx) = socket_reader_find_newline_up_to(reader, Some(limit)) {
                    return socket_reader_take(_py, reader, idx + 1);
                }
                if socket_reader_unread_len(reader) >= limit {
                    return socket_reader_take(_py, reader, limit);
                }
                if reader.eof {
                    return socket_reader_take(
                        _py,
                        reader,
                        socket_reader_unread_len(reader).min(limit),
                    );
                }
                match socket_reader_pull(_py, reader) {
                    Ok(SocketReaderPull::Eof) | Ok(SocketReaderPull::Data) => {}
                    Err(bits) => return bits,
                }
            }
        })
    }
}
