// Ancillary sendmsg/recvmsg protocol authority for native and WASM sockets.
// This module owns ancillary payload parsing, host/native control-message encoding,
// non-Unix peer ancillary queues, and recvmsg result materialization.

#[cfg(any(molt_has_net_io, target_arch = "wasm32"))]
use super::*;

#[cfg(any(molt_has_net_io, target_arch = "wasm32"))]
pub(super) fn collect_sendmsg_payload(
    _py: &PyToken<'_>,
    buffers_bits: u64,
) -> Result<Vec<Vec<u8>>, u64> {
    let values = iter_values_from_bits(_py, buffers_bits)?;
    let mut out: Vec<Vec<u8>> = Vec::with_capacity(values.len());
    for value_bits in values {
        let send_data = match send_data_from_bits(value_bits) {
            Ok(val) => val,
            Err(msg) => return Err(raise_exception::<u64>(_py, "TypeError", &msg)),
        };
        match send_data {
            SendData::Borrowed(ptr, len) => {
                let bytes = unsafe { std::slice::from_raw_parts(ptr, len) };
                out.push(bytes.to_vec());
            }
            SendData::Owned(vec) => out.push(vec),
        }
    }
    Ok(out)
}

#[cfg(any(molt_has_net_io, target_arch = "wasm32"))]
pub(super) type AncillaryItem = (i32, i32, Vec<u8>);

#[cfg(all(molt_has_net_io, not(unix)))]
pub(super) fn socket_clip_ancillary_for_bufsize(
    items: Vec<AncillaryItem>,
    ancbufsize: i64,
) -> (Vec<AncillaryItem>, bool) {
    if items.is_empty() {
        return (Vec::new(), false);
    }
    if ancbufsize <= 0 {
        return (Vec::new(), true);
    }
    let cap = ancbufsize as usize;
    let mut used = 4usize;
    let mut out: Vec<AncillaryItem> = Vec::new();
    let mut truncated = false;
    for (level, kind, data) in items {
        let entry_size = 12usize.saturating_add(data.len());
        if used.saturating_add(entry_size) > cap {
            truncated = true;
            break;
        }
        used = used.saturating_add(entry_size);
        out.push((level, kind, data));
    }
    (out, truncated)
}

#[cfg(any(molt_has_net_io, target_arch = "wasm32"))]
pub(super) fn parse_sendmsg_ancillary_items(
    _py: &PyToken<'_>,
    ancdata_bits: u64,
) -> Result<Vec<AncillaryItem>, u64> {
    if obj_from_bits(ancdata_bits).is_none() {
        return Ok(Vec::new());
    }
    let entries = iter_values_from_bits(_py, ancdata_bits)?;
    let mut out: Vec<AncillaryItem> = Vec::with_capacity(entries.len());
    for entry_bits in entries {
        let Some(entry_ptr) = maybe_ptr_from_bits(entry_bits) else {
            return Err(raise_exception::<u64>(
                _py,
                "TypeError",
                "sendmsg ancillary data must be iterable of 3-item tuples",
            ));
        };
        let entry_type = unsafe { object_type_id(entry_ptr) };
        if entry_type != TYPE_ID_TUPLE && entry_type != TYPE_ID_LIST {
            return Err(raise_exception::<u64>(
                _py,
                "TypeError",
                "sendmsg ancillary data must be iterable of 3-item tuples",
            ));
        }
        let parts = unsafe { seq_vec_ref(entry_ptr) };
        if parts.len() != 3 {
            return Err(raise_exception::<u64>(
                _py,
                "TypeError",
                "sendmsg ancillary data must be iterable of 3-item tuples",
            ));
        }
        let level = match to_i64(obj_from_bits(parts[0])) {
            Some(val) => val as i32,
            None => {
                return Err(raise_exception::<u64>(
                    _py,
                    "TypeError",
                    "sendmsg ancillary level must be int",
                ));
            }
        };
        let kind = match to_i64(obj_from_bits(parts[1])) {
            Some(val) => val as i32,
            None => {
                return Err(raise_exception::<u64>(
                    _py,
                    "TypeError",
                    "sendmsg ancillary type must be int",
                ));
            }
        };
        let payload = match send_data_from_bits(parts[2]) {
            Ok(SendData::Borrowed(ptr, len)) => {
                unsafe { std::slice::from_raw_parts(ptr, len) }.to_vec()
            }
            Ok(SendData::Owned(vec)) => vec,
            Err(msg) => return Err(raise_exception::<u64>(_py, "TypeError", &msg)),
        };
        out.push((level, kind, payload));
    }
    Ok(out)
}

#[cfg(all(molt_has_net_io, unix))]
pub(super) fn encode_sendmsg_ancillary_buffer(items: &[AncillaryItem]) -> Result<Vec<u8>, String> {
    if items.is_empty() {
        return Ok(Vec::new());
    }
    let mut total = 0usize;
    for (_, _, data) in items {
        let len_u32 = u32::try_from(data.len()).map_err(|_| "ancillary payload too large")?;
        let space = unsafe { libc::CMSG_SPACE(len_u32) as usize };
        total = total
            .checked_add(space)
            .ok_or_else(|| "ancillary payload too large".to_string())?;
    }
    let mut control = vec![0u8; total];
    let mut msg: libc::msghdr = unsafe { std::mem::zeroed() };
    msg.msg_control = control.as_mut_ptr() as *mut c_void;
    msg.msg_controllen = control
        .len()
        .try_into()
        .map_err(|_| "ancillary payload too large".to_string())?;
    let mut cmsg = unsafe { libc::CMSG_FIRSTHDR(&msg as *const _) };
    for (level, kind, data) in items {
        if cmsg.is_null() {
            return Err("ancillary header overflow".to_string());
        }
        let len_u32 = u32::try_from(data.len()).map_err(|_| "ancillary payload too large")?;
        let cmsg_len = unsafe { libc::CMSG_LEN(len_u32) as usize };
        unsafe {
            (*cmsg).cmsg_level = *level;
            (*cmsg).cmsg_type = *kind;
            (*cmsg).cmsg_len = cmsg_len as _;
            let dst = libc::CMSG_DATA(cmsg as *const _);
            std::ptr::copy_nonoverlapping(data.as_ptr(), dst, data.len());
            cmsg = libc::CMSG_NXTHDR(&msg as *const _, cmsg as *const _);
        }
    }
    Ok(control)
}

#[cfg(target_arch = "wasm32")]
pub(super) fn encode_host_sendmsg_ancillary_buffer(
    items: &[AncillaryItem],
) -> Result<Vec<u8>, String> {
    let count_u32 =
        u32::try_from(items.len()).map_err(|_| "ancillary item count too large".to_string())?;
    let mut total = 4usize;
    for (_, _, data) in items {
        total = total
            .checked_add(12)
            .and_then(|v| v.checked_add(data.len()))
            .ok_or_else(|| "ancillary payload too large".to_string())?;
    }
    let mut out = Vec::with_capacity(total);
    out.extend_from_slice(&count_u32.to_le_bytes());
    for (level, kind, data) in items {
        let len_u32 =
            u32::try_from(data.len()).map_err(|_| "ancillary payload too large".to_string())?;
        out.extend_from_slice(&level.to_le_bytes());
        out.extend_from_slice(&kind.to_le_bytes());
        out.extend_from_slice(&len_u32.to_le_bytes());
        out.extend_from_slice(data);
    }
    Ok(out)
}

#[cfg(target_arch = "wasm32")]
pub(super) fn decode_host_recvmsg_ancillary_buffer(
    buf: &[u8],
) -> Result<Vec<AncillaryItem>, String> {
    if buf.len() < 4 {
        return Err("recvmsg ancillary payload too short".to_string());
    }
    let count = u32::from_le_bytes([buf[0], buf[1], buf[2], buf[3]]) as usize;
    let mut offset = 4usize;
    let mut out = Vec::with_capacity(count);
    for _ in 0..count {
        let header_end = match offset.checked_add(12) {
            Some(next) => next,
            None => return Err("recvmsg ancillary payload too large".to_string()),
        };
        if header_end > buf.len() {
            return Err("recvmsg ancillary payload truncated".to_string());
        }
        let level = i32::from_le_bytes([
            buf[offset],
            buf[offset + 1],
            buf[offset + 2],
            buf[offset + 3],
        ]);
        offset += 4;
        let kind = i32::from_le_bytes([
            buf[offset],
            buf[offset + 1],
            buf[offset + 2],
            buf[offset + 3],
        ]);
        offset += 4;
        let data_len = u32::from_le_bytes([
            buf[offset],
            buf[offset + 1],
            buf[offset + 2],
            buf[offset + 3],
        ]) as usize;
        offset += 4;
        let data_end = match offset.checked_add(data_len) {
            Some(end) => end,
            None => return Err("recvmsg ancillary payload too large".to_string()),
        };
        if data_end > buf.len() {
            return Err("recvmsg ancillary payload truncated".to_string());
        }
        out.push((level, kind, buf[offset..data_end].to_vec()));
        offset = data_end;
    }
    if offset != buf.len() {
        return Err("recvmsg ancillary payload has trailing bytes".to_string());
    }
    Ok(out)
}

#[cfg(all(molt_has_net_io, unix))]
pub(super) fn parse_recvmsg_ancillary_items(msg: &libc::msghdr) -> Vec<AncillaryItem> {
    let mut out: Vec<AncillaryItem> = Vec::new();
    let mut cmsg = unsafe { libc::CMSG_FIRSTHDR(msg as *const _) };
    while !cmsg.is_null() {
        let cmsg_len = unsafe { (*cmsg).cmsg_len as usize };
        let header_len = unsafe { libc::CMSG_LEN(0) as usize };
        if cmsg_len >= header_len {
            let data_len = cmsg_len - header_len;
            let data_ptr = unsafe { libc::CMSG_DATA(cmsg as *const _) } as *const u8;
            let data = unsafe { std::slice::from_raw_parts(data_ptr, data_len) }.to_vec();
            let level = unsafe { (*cmsg).cmsg_level };
            let kind = unsafe { (*cmsg).cmsg_type };
            out.push((level, kind, data));
        }
        cmsg = unsafe { libc::CMSG_NXTHDR(msg as *const _, cmsg as *const _) };
    }
    out
}

#[cfg(any(molt_has_net_io, target_arch = "wasm32"))]
pub(super) fn build_ancillary_list_bits(
    _py: &PyToken<'_>,
    items: &[(i32, i32, Vec<u8>)],
) -> Result<u64, u64> {
    let mut item_bits: Vec<u64> = Vec::with_capacity(items.len());
    for (level, kind, data) in items {
        let bytes_ptr = alloc_bytes(_py, data);
        if bytes_ptr.is_null() {
            for bits in item_bits {
                dec_ref_bits(_py, bits);
            }
            return Err(MoltObject::none().bits());
        }
        let bytes_bits = MoltObject::from_ptr(bytes_ptr).bits();
        let level_bits = MoltObject::from_int(*level as i64).bits();
        let kind_bits = MoltObject::from_int(*kind as i64).bits();
        let tuple_ptr = alloc_tuple(_py, &[level_bits, kind_bits, bytes_bits]);
        dec_ref_bits(_py, bytes_bits);
        if tuple_ptr.is_null() {
            for bits in item_bits {
                dec_ref_bits(_py, bits);
            }
            return Err(MoltObject::none().bits());
        }
        item_bits.push(MoltObject::from_ptr(tuple_ptr).bits());
    }
    let list_ptr = alloc_list(_py, item_bits.as_slice());
    for bits in item_bits {
        dec_ref_bits(_py, bits);
    }
    if list_ptr.is_null() {
        return Err(MoltObject::none().bits());
    }
    Ok(MoltObject::from_ptr(list_ptr).bits())
}

#[cfg(any(molt_has_net_io, target_arch = "wasm32"))]
pub(super) fn build_recvmsg_result_with_anc(
    _py: &PyToken<'_>,
    data: &[u8],
    msg_flags: i32,
    addr_bits: u64,
    anc_bits: u64,
) -> u64 {
    let data_ptr = alloc_bytes(_py, data);
    if data_ptr.is_null() {
        dec_ref_bits(_py, anc_bits);
        dec_ref_bits(_py, addr_bits);
        return MoltObject::none().bits();
    }
    let data_bits = MoltObject::from_ptr(data_ptr).bits();
    let flags_bits = MoltObject::from_int(msg_flags as i64).bits();
    let tuple_ptr = alloc_tuple(_py, &[data_bits, anc_bits, flags_bits, addr_bits]);
    dec_ref_bits(_py, data_bits);
    dec_ref_bits(_py, anc_bits);
    dec_ref_bits(_py, addr_bits);
    if tuple_ptr.is_null() {
        return MoltObject::none().bits();
    }
    MoltObject::from_ptr(tuple_ptr).bits()
}

#[cfg(any(molt_has_net_io, target_arch = "wasm32"))]
pub(super) struct RecvmsgIntoTarget {
    ptr: *mut u8,
    len: usize,
    is_memoryview: bool,
}

#[cfg(any(molt_has_net_io, target_arch = "wasm32"))]
pub(super) fn collect_recvmsg_into_targets(
    _py: &PyToken<'_>,
    buffers_bits: u64,
) -> Result<Vec<RecvmsgIntoTarget>, u64> {
    let values = iter_values_from_bits(_py, buffers_bits)?;
    if values.is_empty() {
        return Err(raise_exception::<u64>(
            _py,
            "ValueError",
            "recvmsg_into() requires at least one buffer",
        ));
    }
    let mut out: Vec<RecvmsgIntoTarget> = Vec::with_capacity(values.len());
    for value_bits in values {
        let Some(ptr) = maybe_ptr_from_bits(value_bits) else {
            return Err(raise_exception::<u64>(
                _py,
                "TypeError",
                "recvmsg_into() argument must be an iterable of writable buffers",
            ));
        };
        unsafe {
            let type_id = object_type_id(ptr);
            if type_id == TYPE_ID_BYTEARRAY {
                out.push(RecvmsgIntoTarget {
                    ptr,
                    len: bytearray_len(ptr),
                    is_memoryview: false,
                });
                continue;
            }
            if type_id == TYPE_ID_MEMORYVIEW {
                if memoryview_readonly(ptr) {
                    return Err(raise_exception::<u64>(
                        _py,
                        "TypeError",
                        "recvmsg_into() argument must be writable buffers",
                    ));
                }
                out.push(RecvmsgIntoTarget {
                    ptr,
                    len: memoryview_len(ptr),
                    is_memoryview: true,
                });
                continue;
            }
        }
        return Err(raise_exception::<u64>(
            _py,
            "TypeError",
            "recvmsg_into() argument must be an iterable of writable buffers",
        ));
    }
    Ok(out)
}

#[cfg(any(molt_has_net_io, target_arch = "wasm32"))]
pub(super) fn write_recvmsg_into_targets(
    _py: &PyToken<'_>,
    targets: &[RecvmsgIntoTarget],
    data: &[u8],
) -> Result<(), u64> {
    let mut offset = 0usize;
    for target in targets {
        if offset >= data.len() {
            break;
        }
        let count = (data.len() - offset).min(target.len);
        if count == 0 {
            continue;
        }
        let chunk = &data[offset..offset + count];
        if target.is_memoryview {
            if let Some(slice) = unsafe { memoryview_bytes_slice_mut(target.ptr) } {
                let n = chunk.len().min(slice.len());
                slice[..n].copy_from_slice(&chunk[..n]);
            } else if let Err(msg) = unsafe { memoryview_write_bytes(target.ptr, chunk) } {
                return Err(raise_exception::<u64>(_py, "TypeError", &msg));
            }
        } else {
            let dst = unsafe { bytearray_vec(target.ptr) };
            let n = chunk.len().min(dst.len());
            dst[..n].copy_from_slice(&chunk[..n]);
        }
        offset += count;
    }
    Ok(())
}
