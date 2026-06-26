use super::*;

pub(super) unsafe fn memory_backend_vec_from_bits(
    _py: &PyToken<'_>,
    mem_bits: u64,
) -> Result<&'static mut Vec<u8>, u64> {
    unsafe {
        if mem_bits == 0 || obj_from_bits(mem_bits).is_none() {
            return Err(raise_exception::<_>(
                _py,
                "RuntimeError",
                "memory backend missing",
            ));
        }
        let Some(ptr) = obj_from_bits(mem_bits).as_ptr() else {
            return Err(raise_exception::<_>(
                _py,
                "RuntimeError",
                "memory backend missing",
            ));
        };
        if object_type_id(ptr) != TYPE_ID_BYTEARRAY {
            return Err(raise_exception::<_>(
                _py,
                "RuntimeError",
                "memory backend is not bytearray",
            ));
        }
        Ok(bytearray_vec(ptr))
    }
}

pub(super) unsafe fn memory_backend_vec(
    _py: &PyToken<'_>,
    handle: &mut MoltFileHandle,
) -> Result<&'static mut Vec<u8>, u64> {
    unsafe { memory_backend_vec_from_bits(_py, handle.mem_bits) }
}

pub(super) unsafe fn memory_backend_vec_ref_from_bits(
    _py: &PyToken<'_>,
    mem_bits: u64,
) -> Result<&'static Vec<u8>, u64> {
    unsafe {
        if mem_bits == 0 || obj_from_bits(mem_bits).is_none() {
            return Err(raise_exception::<_>(
                _py,
                "RuntimeError",
                "memory backend missing",
            ));
        }
        let Some(ptr) = obj_from_bits(mem_bits).as_ptr() else {
            return Err(raise_exception::<_>(
                _py,
                "RuntimeError",
                "memory backend missing",
            ));
        };
        if object_type_id(ptr) != TYPE_ID_BYTEARRAY {
            return Err(raise_exception::<_>(
                _py,
                "RuntimeError",
                "memory backend is not bytearray",
            ));
        }
        Ok(bytearray_vec_ref(ptr))
    }
}

pub(super) unsafe fn memory_backend_vec_ref(
    _py: &PyToken<'_>,
    handle: &mut MoltFileHandle,
) -> Result<&'static Vec<u8>, u64> {
    unsafe { memory_backend_vec_ref_from_bits(_py, handle.mem_bits) }
}

pub(crate) unsafe fn collect_bytes_like(_py: &PyToken<'_>, bits: u64) -> Result<Vec<u8>, u64> {
    unsafe {
        let obj = obj_from_bits(bits);
        if obj.is_none() {
            return Ok(Vec::new());
        }
        let Some(ptr) = obj.as_ptr() else {
            return Err(raise_exception::<_>(
                _py,
                "TypeError",
                "a bytes-like object is required",
            ));
        };
        match object_type_id(ptr) {
            TYPE_ID_BYTES | TYPE_ID_BYTEARRAY => {
                let len = bytes_len(ptr);
                let raw = std::slice::from_raw_parts(bytes_data(ptr), len);
                Ok(raw.to_vec())
            }
            TYPE_ID_MEMORYVIEW => {
                if let Some(out) = memoryview_collect_bytes(ptr) {
                    Ok(out)
                } else {
                    Err(raise_exception::<_>(
                        _py,
                        "TypeError",
                        "a bytes-like object is required",
                    ))
                }
            }
            _ => Err(raise_exception::<_>(
                _py,
                "TypeError",
                "a bytes-like object is required",
            )),
        }
    }
}

pub(super) unsafe fn backend_read_bytes(
    _py: &PyToken<'_>,
    mem_bits: u64,
    backend: &mut MoltFileBackend,
    buf: &mut [u8],
) -> Result<usize, u64> {
    unsafe {
        match backend {
            MoltFileBackend::File(file) => match file.read(buf) {
                Ok(n) => Ok(n),
                Err(_) => Err(raise_exception::<_>(_py, "OSError", "read failed")),
            },
            MoltFileBackend::Memory(mem) => {
                let data = memory_backend_vec_ref_from_bits(_py, mem_bits)?;
                if mem.pos >= data.len() {
                    return Ok(0);
                }
                let available = data.len().saturating_sub(mem.pos);
                let n = available.min(buf.len());
                buf[..n].copy_from_slice(&data[mem.pos..mem.pos + n]);
                mem.pos = mem.pos.saturating_add(n);
                Ok(n)
            }
            MoltFileBackend::Text(_) => Err(raise_exception::<_>(
                _py,
                "UnsupportedOperation",
                "binary read on text backend",
            )),
        }
    }
}

pub(super) fn file_remaining_bytes_hint(file: &mut std::fs::File) -> Option<usize> {
    let pos = file.stream_position().ok()?;
    let len = file.metadata().ok()?.len();
    usize::try_from(len.saturating_sub(pos)).ok()
}

pub(super) unsafe fn buffered_read_reserve_hint(
    _py: &PyToken<'_>,
    handle: &mut MoltFileHandle,
    backend: &mut MoltFileBackend,
    size: Option<usize>,
) -> Option<usize> {
    unsafe {
        if let Some(size) = size {
            return Some(size.saturating_add(unread_bytes(handle)));
        }
        let unread = unread_bytes(handle);
        match backend {
            MoltFileBackend::File(file) => {
                file_remaining_bytes_hint(file).map(|n| n.saturating_add(unread))
            }
            MoltFileBackend::Memory(mem) => memory_backend_vec_ref_from_bits(_py, handle.mem_bits)
                .ok()
                .map(|data| data.len().saturating_sub(mem.pos).saturating_add(unread)),
            MoltFileBackend::Text(_) => None,
        }
    }
}

pub(super) unsafe fn backend_write_bytes(
    _py: &PyToken<'_>,
    mem_bits: u64,
    backend: &mut MoltFileBackend,
    bytes: &[u8],
) -> Result<usize, u64> {
    unsafe {
        match backend {
            MoltFileBackend::File(file) => match file.write(bytes) {
                Ok(n) => Ok(n),
                Err(_) => Err(raise_exception::<_>(_py, "OSError", "write failed")),
            },
            MoltFileBackend::Memory(mem) => {
                let data = memory_backend_vec_from_bits(_py, mem_bits)?;
                if mem.pos > data.len() {
                    data.resize(mem.pos, 0);
                }
                let end = mem.pos.saturating_add(bytes.len());
                if end > data.len() {
                    data.resize(end, 0);
                }
                data[mem.pos..end].copy_from_slice(bytes);
                mem.pos = end;
                Ok(bytes.len())
            }
            MoltFileBackend::Text(_) => Err(raise_exception::<_>(
                _py,
                "UnsupportedOperation",
                "binary write on text backend",
            )),
        }
    }
}

pub(super) unsafe fn backend_flush(
    _py: &PyToken<'_>,
    backend: &mut MoltFileBackend,
) -> Result<(), u64> {
    match backend {
        MoltFileBackend::File(file) => match file.flush() {
            Ok(()) => Ok(()),
            Err(_) => Err(raise_exception::<_>(_py, "OSError", "flush failed")),
        },
        MoltFileBackend::Memory(_) | MoltFileBackend::Text(_) => Ok(()),
    }
}

pub(super) unsafe fn backend_seek(
    _py: &PyToken<'_>,
    handle: &mut MoltFileHandle,
    backend: &mut MoltFileBackend,
    from: std::io::SeekFrom,
) -> Result<u64, u64> {
    unsafe {
        match backend {
            MoltFileBackend::File(file) => match file.seek(from) {
                Ok(pos) => Ok(pos),
                Err(_) => Err(raise_exception::<_>(_py, "OSError", "seek failed")),
            },
            MoltFileBackend::Memory(mem) => {
                let pos = match from {
                    std::io::SeekFrom::Start(pos) => pos as i64,
                    std::io::SeekFrom::Current(delta) => mem.pos as i64 + delta,
                    std::io::SeekFrom::End(delta) => {
                        let len = memory_backend_vec_ref(_py, handle)?.len() as i64;
                        len + delta
                    }
                };
                if pos < 0 {
                    return Err(raise_exception::<_>(
                        _py,
                        "ValueError",
                        "negative seek position",
                    ));
                }
                mem.pos = pos as usize;
                Ok(mem.pos as u64)
            }
            MoltFileBackend::Text(_) => Err(raise_exception::<_>(
                _py,
                "UnsupportedOperation",
                "seek on text backend",
            )),
        }
    }
}

pub(super) unsafe fn backend_tell(
    _py: &PyToken<'_>,
    backend: &mut MoltFileBackend,
) -> Result<u64, u64> {
    match backend {
        MoltFileBackend::File(file) => match file.stream_position() {
            Ok(pos) => Ok(pos),
            Err(_) => Err(raise_exception::<_>(_py, "OSError", "tell failed")),
        },
        MoltFileBackend::Memory(mem) => Ok(mem.pos as u64),
        MoltFileBackend::Text(_) => Err(raise_exception::<_>(
            _py,
            "UnsupportedOperation",
            "tell on text backend",
        )),
    }
}

pub(super) unsafe fn backend_truncate(
    _py: &PyToken<'_>,
    handle: &mut MoltFileHandle,
    backend: &mut MoltFileBackend,
    size: u64,
) -> Result<(), u64> {
    unsafe {
        match backend {
            MoltFileBackend::File(file) => match file.set_len(size) {
                Ok(()) => Ok(()),
                Err(_) => Err(raise_exception::<_>(_py, "OSError", "truncate failed")),
            },
            MoltFileBackend::Memory(mem) => {
                let data = memory_backend_vec(_py, handle)?;
                let size_usize = size as usize;
                if size_usize < data.len() {
                    data.truncate(size_usize);
                } else if size_usize > data.len() {
                    data.resize(size_usize, 0);
                }
                if mem.pos > data.len() {
                    mem.pos = data.len();
                }
                Ok(())
            }
            MoltFileBackend::Text(_) => Err(raise_exception::<_>(
                _py,
                "UnsupportedOperation",
                "truncate on text backend",
            )),
        }
    }
}

pub(super) fn clear_read_buffer(handle: &mut MoltFileHandle) {
    handle.read_buf.clear();
    handle.read_pos = 0;
}

pub(super) fn prepend_read_buffer(handle: &mut MoltFileHandle, prefix: &[u8]) {
    if prefix.is_empty() {
        return;
    }
    let unread = if handle.read_pos < handle.read_buf.len() {
        handle.read_buf[handle.read_pos..].to_vec()
    } else {
        Vec::new()
    };
    let mut buf = Vec::with_capacity(prefix.len() + unread.len());
    buf.extend_from_slice(prefix);
    buf.extend_from_slice(&unread);
    handle.read_buf = buf;
    handle.read_pos = 0;
}

pub(super) fn clear_write_buffer(handle: &mut MoltFileHandle) {
    handle.write_buf.clear();
}

pub(super) fn unread_bytes(handle: &MoltFileHandle) -> usize {
    handle.read_buf.len().saturating_sub(handle.read_pos)
}

pub(super) unsafe fn rewind_read_buffer(
    _py: &PyToken<'_>,
    handle: &mut MoltFileHandle,
    backend: &mut MoltFileBackend,
) -> Result<(), u64> {
    let unread = unread_bytes(handle);
    if unread == 0 {
        clear_read_buffer(handle);
        return Ok(());
    }
    match backend {
        MoltFileBackend::File(file) => {
            let offset = -(unread as i64);
            match file.seek(std::io::SeekFrom::Current(offset)) {
                Ok(_) => {
                    clear_read_buffer(handle);
                    Ok(())
                }
                Err(_) => Err(raise_exception::<_>(_py, "OSError", "seek failed")),
            }
        }
        MoltFileBackend::Memory(mem) => {
            mem.pos = mem.pos.saturating_sub(unread);
            clear_read_buffer(handle);
            Ok(())
        }
        MoltFileBackend::Text(_) => {
            clear_read_buffer(handle);
            Ok(())
        }
    }
}

pub(super) unsafe fn flush_write_buffer(
    _py: &PyToken<'_>,
    handle: &mut MoltFileHandle,
    backend: &mut MoltFileBackend,
) -> Result<(), u64> {
    unsafe {
        if handle.write_buf.is_empty() {
            return Ok(());
        }
        let bytes = handle.write_buf.clone();
        handle.write_buf.clear();
        let mut written = 0usize;
        while written < bytes.len() {
            let n = backend_write_bytes(_py, handle.mem_bits, backend, &bytes[written..])?;
            if n == 0 {
                return Err(raise_exception::<_>(_py, "OSError", "write failed"));
            }
            written += n;
        }
        backend_flush(_py, backend)?;
        Ok(())
    }
}

pub(super) unsafe fn buffered_read_bytes(
    _py: &PyToken<'_>,
    handle: &mut MoltFileHandle,
    backend: &mut MoltFileBackend,
    size: Option<usize>,
) -> Result<(Vec<u8>, bool), u64> {
    unsafe {
        if !handle.write_buf.is_empty() {
            flush_write_buffer(_py, handle, backend)?;
        }
        if size.is_none() {
            let unread = unread_bytes(handle);
            if let MoltFileBackend::File(file) = backend {
                let reserve = file_remaining_bytes_hint(file)
                    .unwrap_or(0)
                    .saturating_add(unread);
                let mut out = Vec::with_capacity(reserve);
                if unread > 0 {
                    let start = handle.read_pos;
                    out.extend_from_slice(&handle.read_buf[start..]);
                    clear_read_buffer(handle);
                }
                match file.read_to_end(&mut out) {
                    Ok(_) => return Ok((out, true)),
                    Err(_) => return Err(raise_exception::<_>(_py, "OSError", "read failed")),
                }
            }
        }

        if handle.buffer_size == 0 {
            let mut buf = Vec::with_capacity(
                buffered_read_reserve_hint(_py, handle, backend, size).unwrap_or(0),
            );
            let mut tmp = [0u8; 8192];
            let mut at_eof = false;
            match size {
                Some(0) => return Ok((Vec::new(), false)),
                Some(mut remaining) => {
                    while remaining > 0 {
                        let to_read = remaining.min(tmp.len());
                        let n =
                            backend_read_bytes(_py, handle.mem_bits, backend, &mut tmp[..to_read])?;
                        if n == 0 {
                            at_eof = true;
                            break;
                        }
                        buf.extend_from_slice(&tmp[..n]);
                        remaining -= n;
                    }
                }
                None => loop {
                    let n = backend_read_bytes(_py, handle.mem_bits, backend, &mut tmp)?;
                    if n == 0 {
                        at_eof = true;
                        break;
                    }
                    buf.extend_from_slice(&tmp[..n]);
                },
            }
            return Ok((buf, at_eof));
        }

        let mut out =
            Vec::with_capacity(buffered_read_reserve_hint(_py, handle, backend, size).unwrap_or(0));
        let mut at_eof = false;
        let mut remaining = size;
        if let Some(rem) = remaining
            && rem == 0
        {
            return Ok((out, false));
        }

        while remaining.map(|r| r > 0).unwrap_or(true) {
            let avail = unread_bytes(handle);
            if avail > 0 {
                let take = remaining.map(|r| r.min(avail)).unwrap_or(avail);
                let start = handle.read_pos;
                let end = start + take;
                out.extend_from_slice(&handle.read_buf[start..end]);
                handle.read_pos = end;
                if handle.read_pos >= handle.read_buf.len() {
                    clear_read_buffer(handle);
                }
                if let Some(rem) = remaining {
                    let new_rem = rem.saturating_sub(take);
                    remaining = Some(new_rem);
                    if new_rem == 0 {
                        break;
                    }
                }
                continue;
            }
            let buf_size = handle.buffer_size.max(1) as usize;
            let mut buf = std::mem::take(&mut handle.read_buf);
            buf.resize(buf_size, 0);
            let n = backend_read_bytes(_py, handle.mem_bits, backend, &mut buf)?;
            if n == 0 {
                at_eof = true;
                handle.read_buf = buf;
                clear_read_buffer(handle);
                break;
            }
            buf.truncate(n);
            handle.read_buf = buf;
            handle.read_pos = 0;
        }
        Ok((out, at_eof))
    }
}

pub(super) unsafe fn buffered_read_into(
    _py: &PyToken<'_>,
    handle: &mut MoltFileHandle,
    backend: &mut MoltFileBackend,
    buf: &mut [u8],
) -> Result<usize, u64> {
    unsafe {
        if buf.is_empty() {
            return Ok(0);
        }
        if !handle.write_buf.is_empty() {
            flush_write_buffer(_py, handle, backend)?;
        }
        let mut written = 0usize;
        let avail = unread_bytes(handle);
        if avail > 0 {
            let take = avail.min(buf.len());
            let start = handle.read_pos;
            let end = start + take;
            buf[..take].copy_from_slice(&handle.read_buf[start..end]);
            handle.read_pos = end;
            if handle.read_pos >= handle.read_buf.len() {
                clear_read_buffer(handle);
            }
            written += take;
        }
        if written >= buf.len() {
            return Ok(written);
        }
        let n = backend_read_bytes(_py, handle.mem_bits, backend, &mut buf[written..])?;
        written += n;
        Ok(written)
    }
}

pub(super) unsafe fn file_read1_bytes(
    _py: &PyToken<'_>,
    handle: &mut MoltFileHandle,
    backend: &mut MoltFileBackend,
    size: Option<usize>,
) -> Result<(Vec<u8>, bool), u64> {
    unsafe {
        if let Some(0) = size {
            return Ok((Vec::new(), false));
        }
        if !handle.write_buf.is_empty() {
            flush_write_buffer(_py, handle, backend)?;
        }
        let avail = unread_bytes(handle);
        if avail > 0 {
            let take = size.unwrap_or(avail).min(avail);
            let start = handle.read_pos;
            let end = start + take;
            let out = handle.read_buf[start..end].to_vec();
            handle.read_pos = end;
            if handle.read_pos >= handle.read_buf.len() {
                clear_read_buffer(handle);
            }
            return Ok((out, false));
        }
        let read_size = size.unwrap_or({
            if handle.buffer_size > 0 {
                handle.buffer_size as usize
            } else {
                8192
            }
        });
        if read_size == 0 {
            return Ok((Vec::new(), false));
        }
        let mut buf = vec![0u8; read_size];
        let n = backend_read_bytes(_py, handle.mem_bits, backend, &mut buf)?;
        buf.truncate(n);
        Ok((buf, n == 0))
    }
}

pub(super) unsafe fn handle_read_byte(
    _py: &PyToken<'_>,
    handle: &mut MoltFileHandle,
    backend: &mut MoltFileBackend,
) -> Result<Option<u8>, u64> {
    unsafe {
        if let Some(pending) = handle.pending_byte.take() {
            return Ok(Some(pending));
        }
        if !handle.text_pending_bytes.is_empty() {
            let byte = handle.text_pending_bytes.remove(0);
            return Ok(Some(byte));
        }
        if handle.buffer_size > 0 {
            if unread_bytes(handle) == 0 {
                let buf_size = handle.buffer_size.max(1) as usize;
                let mut buf = std::mem::take(&mut handle.read_buf);
                buf.resize(buf_size, 0);
                let n = backend_read_bytes(_py, handle.mem_bits, backend, &mut buf)?;
                if n == 0 {
                    handle.read_buf = buf;
                    clear_read_buffer(handle);
                    return Ok(None);
                }
                buf.truncate(n);
                handle.read_buf = buf;
                handle.read_pos = 0;
            }
            if unread_bytes(handle) == 0 {
                return Ok(None);
            }
            let byte = handle.read_buf[handle.read_pos];
            handle.read_pos += 1;
            if handle.read_pos >= handle.read_buf.len() {
                clear_read_buffer(handle);
            }
            Ok(Some(byte))
        } else {
            let mut buf = [0u8; 1];
            let n = backend_read_bytes(_py, handle.mem_bits, backend, &mut buf)?;
            if n == 0 { Ok(None) } else { Ok(Some(buf[0])) }
        }
    }
}
