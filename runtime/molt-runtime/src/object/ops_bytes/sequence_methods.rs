use super::*;
use memchr::memchr;

#[unsafe(no_mangle)]
pub extern "C" fn molt_bytes_join(sep_bits: u64, items_bits: u64) -> u64 {
    crate::with_gil_entry_nopanic!(_py, {
        let sep = obj_from_bits(sep_bits);
        let items = obj_from_bits(items_bits);
        let sep_ptr = match sep.as_ptr() {
            Some(ptr) => ptr,
            None => return MoltObject::none().bits(),
        };
        unsafe {
            if object_type_id(sep_ptr) != TYPE_ID_BYTES {
                return raise_exception::<_>(_py, "TypeError", "join expects a bytes separator");
            }
            let sep_bytes = bytes_like_slice(sep_ptr).unwrap_or(&[]);
            let mut total_len = 0usize;
            struct BytesPart {
                bits: u64,
                data: *const u8,
                len: usize,
                type_id: u32,
            }
            let mut parts = Vec::new();
            let mut all_same = true;
            let mut first_bits = 0u64;
            let mut first_data = std::ptr::null();
            let mut first_len = 0usize;
            let mut owned_bits = Vec::new();
            let mut iter_owned = false;
            if let Some(ptr) = items.as_ptr() {
                let type_id = object_type_id(ptr);
                if type_id == TYPE_ID_LIST || type_id == TYPE_ID_TUPLE {
                    let elems = seq_vec_ref(ptr);
                    parts.reserve(elems.len());
                    for (idx, &elem_bits) in elems.iter().enumerate() {
                        let elem_obj = obj_from_bits(elem_bits);
                        let elem_ptr = match elem_obj.as_ptr() {
                            Some(ptr) => ptr,
                            None => {
                                let msg = format!(
                                    "sequence item {idx}: expected a bytes-like object, {} found",
                                    type_name(_py, elem_obj)
                                );
                                return raise_exception::<_>(_py, "TypeError", &msg);
                            }
                        };
                        let elem_bytes = match bytes_join_part_or_type_error(_py, elem_ptr, || {
                            format!(
                                "sequence item {idx}: expected a bytes-like object, {} found",
                                type_name(_py, elem_obj)
                            )
                        }) {
                            Ok(slice) => slice,
                            Err(bits) => return bits,
                        };
                        let len = elem_bytes.len();
                        total_len = total_len.saturating_add(len);
                        let data = elem_bytes.as_ptr();
                        if idx == 0 {
                            first_bits = elem_bits;
                            first_data = data;
                            first_len = len;
                        } else if elem_bits != first_bits {
                            all_same = false;
                        }
                        parts.push(BytesPart {
                            bits: elem_bits,
                            data,
                            len,
                            type_id: object_type_id(elem_ptr),
                        });
                    }
                }
            }
            if parts.is_empty() {
                let iter_bits = molt_iter(items_bits);
                if obj_from_bits(iter_bits).is_none() {
                    if exception_pending(_py) {
                        return MoltObject::none().bits();
                    }
                    return raise_exception::<_>(_py, "TypeError", "can only join an iterable");
                }
                iter_owned = true;
                let mut idx = 0usize;
                loop {
                    let pair_bits = molt_iter_next(iter_bits);
                    if exception_pending(_py) {
                        for bits in owned_bits.iter().copied() {
                            dec_ref_bits(_py, bits);
                        }
                        return MoltObject::none().bits();
                    }
                    let pair_obj = obj_from_bits(pair_bits);
                    let Some(pair_ptr) = pair_obj.as_ptr() else {
                        for bits in owned_bits.iter().copied() {
                            dec_ref_bits(_py, bits);
                        }
                        return MoltObject::none().bits();
                    };
                    if object_type_id(pair_ptr) != TYPE_ID_TUPLE {
                        for bits in owned_bits.iter().copied() {
                            dec_ref_bits(_py, bits);
                        }
                        return MoltObject::none().bits();
                    }
                    let pair_elems = seq_vec_ref(pair_ptr);
                    if pair_elems.len() < 2 {
                        for bits in owned_bits.iter().copied() {
                            dec_ref_bits(_py, bits);
                        }
                        return MoltObject::none().bits();
                    }
                    let done_bits = pair_elems[1];
                    if is_truthy(_py, obj_from_bits(done_bits)) {
                        break;
                    }
                    let elem_bits = pair_elems[0];
                    let elem_obj = obj_from_bits(elem_bits);
                    let elem_ptr = match elem_obj.as_ptr() {
                        Some(ptr) => ptr,
                        None => {
                            for bits in owned_bits.iter().copied() {
                                dec_ref_bits(_py, bits);
                            }
                            let msg = format!(
                                "sequence item {idx}: expected a bytes-like object, {} found",
                                type_name(_py, elem_obj)
                            );
                            return raise_exception::<_>(_py, "TypeError", &msg);
                        }
                    };
                    let elem_bytes = match bytes_join_part_or_type_error(_py, elem_ptr, || {
                        format!(
                            "sequence item {idx}: expected a bytes-like object, {} found",
                            type_name(_py, elem_obj)
                        )
                    }) {
                        Ok(slice) => slice,
                        Err(bits) => {
                            for bits in owned_bits.iter().copied() {
                                dec_ref_bits(_py, bits);
                            }
                            return bits;
                        }
                    };
                    let len = elem_bytes.len();
                    total_len = total_len.saturating_add(len);
                    let data = elem_bytes.as_ptr();
                    if idx == 0 {
                        first_bits = elem_bits;
                        first_data = data;
                        first_len = len;
                    } else if elem_bits != first_bits {
                        all_same = false;
                    }
                    parts.push(BytesPart {
                        bits: elem_bits,
                        data,
                        len,
                        type_id: object_type_id(elem_ptr),
                    });
                    inc_ref_bits(_py, elem_bits);
                    owned_bits.push(elem_bits);
                    idx += 1;
                }
            }
            if !parts.is_empty() {
                let sep_total = sep_bytes
                    .len()
                    .saturating_mul(parts.len().saturating_sub(1));
                total_len = total_len.saturating_add(sep_total);
            }
            if parts.len() == 1 && !iter_owned && parts[0].type_id == TYPE_ID_BYTES {
                inc_ref_bits(_py, parts[0].bits);
                return parts[0].bits;
            }
            let out_ptr = alloc_bytes_like_with_len(_py, total_len, TYPE_ID_BYTES);
            if out_ptr.is_null() {
                if iter_owned {
                    for bits in owned_bits.iter().copied() {
                        dec_ref_bits(_py, bits);
                    }
                }
                return MoltObject::none().bits();
            }
            let mut cursor = out_ptr.add(std::mem::size_of::<usize>());
            if all_same && parts.len() > 1 {
                let sep_len = sep_bytes.len();
                let elem_len = first_len;
                if elem_len > 0 {
                    std::ptr::copy_nonoverlapping(first_data, cursor, elem_len);
                    cursor = cursor.add(elem_len);
                }
                let pattern_len = sep_len.saturating_add(elem_len);
                let total_pattern_bytes = pattern_len.saturating_mul(parts.len() - 1);
                if total_pattern_bytes > 0 {
                    if sep_len > 0 {
                        std::ptr::copy_nonoverlapping(sep_bytes.as_ptr(), cursor, sep_len);
                    }
                    if elem_len > 0 {
                        std::ptr::copy_nonoverlapping(first_data, cursor.add(sep_len), elem_len);
                    }
                    let pattern_start = cursor;
                    let mut filled = pattern_len;
                    while filled < total_pattern_bytes {
                        let copy_len = (total_pattern_bytes - filled).min(filled);
                        std::ptr::copy_nonoverlapping(
                            pattern_start,
                            pattern_start.add(filled),
                            copy_len,
                        );
                        filled += copy_len;
                    }
                }
                let out_bits = MoltObject::from_ptr(out_ptr).bits();
                if iter_owned {
                    for bits in owned_bits.iter().copied() {
                        dec_ref_bits(_py, bits);
                    }
                }
                return out_bits;
            }
            for (idx, part) in parts.iter().enumerate() {
                if idx > 0 {
                    std::ptr::copy_nonoverlapping(sep_bytes.as_ptr(), cursor, sep_bytes.len());
                    cursor = cursor.add(sep_bytes.len());
                }
                std::ptr::copy_nonoverlapping(part.data, cursor, part.len);
                cursor = cursor.add(part.len);
            }
            let out_bits = MoltObject::from_ptr(out_ptr).bits();
            if iter_owned {
                for bits in owned_bits.iter().copied() {
                    dec_ref_bits(_py, bits);
                }
            }
            out_bits
        }
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_bytearray_join(sep_bits: u64, items_bits: u64) -> u64 {
    crate::with_gil_entry_nopanic!(_py, {
        let sep_obj = obj_from_bits(sep_bits);
        let Some(sep_ptr) = sep_obj.as_ptr() else {
            return raise_exception::<_>(_py, "TypeError", "join expects a bytearray separator");
        };
        unsafe {
            if object_type_id(sep_ptr) != TYPE_ID_BYTEARRAY {
                return raise_exception::<_>(
                    _py,
                    "TypeError",
                    "join expects a bytearray separator",
                );
            }
            let sep_bytes = bytes_like_slice(sep_ptr).unwrap_or(&[]);
            let tmp_sep_ptr = alloc_bytes(_py, sep_bytes);
            if tmp_sep_ptr.is_null() {
                return MoltObject::none().bits();
            }
            let tmp_sep_bits = MoltObject::from_ptr(tmp_sep_ptr).bits();
            let joined_bits = molt_bytes_join(tmp_sep_bits, items_bits);
            dec_ref_bits(_py, tmp_sep_bits);
            if exception_pending(_py) {
                return MoltObject::none().bits();
            }
            let joined_obj = obj_from_bits(joined_bits);
            let Some(joined_ptr) = joined_obj.as_ptr() else {
                return MoltObject::none().bits();
            };
            let joined_bytes = bytes_like_slice(joined_ptr).unwrap_or(&[]);
            let out_ptr = alloc_bytearray(_py, joined_bytes);
            dec_ref_bits(_py, joined_bits);
            if out_ptr.is_null() {
                return MoltObject::none().bits();
            }
            MoltObject::from_ptr(out_ptr).bits()
        }
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_bytes_find(hay_bits: u64, needle_bits: u64) -> u64 {
    crate::with_gil_entry_nopanic!(_py, {
        let none_bits = MoltObject::none().bits();
        let false_bits = MoltObject::from_bool(false).bits();
        molt_bytes_find_slice(
            hay_bits,
            needle_bits,
            none_bits,
            none_bits,
            false_bits,
            false_bits,
        )
    })
}

// ── 4-arg method-dispatch wrappers for bytes/bytearray ──────────────
// Mirror the str wrappers: (self, sub, start=None, end=None) with None
// sentinel → has_start/has_end conversion.

macro_rules! bytes_slice_method_wrapper {
    ($name:ident, $delegate:ident) => {
        #[unsafe(no_mangle)]
        pub extern "C" fn $name(
            hay_bits: u64,
            needle_bits: u64,
            start_bits: u64,
            end_bits: u64,
        ) -> u64 {
            let has_start = obj_from_bits(start_bits).is_none() as u64 ^ 1;
            let has_end = obj_from_bits(end_bits).is_none() as u64 ^ 1;
            let start = if has_start != 0 {
                start_bits
            } else {
                MoltObject::from_int(0).bits()
            };
            let end = if has_end != 0 {
                end_bits
            } else {
                MoltObject::from_int(0).bits()
            };
            $delegate(
                hay_bits,
                needle_bits,
                start,
                end,
                MoltObject::from_int(has_start as i64).bits(),
                MoltObject::from_int(has_end as i64).bits(),
            )
        }
    };
}

bytes_slice_method_wrapper!(molt_bytes_find_method, molt_bytes_find_slice);
bytes_slice_method_wrapper!(molt_bytes_rfind_method, molt_bytes_rfind_slice);
bytes_slice_method_wrapper!(molt_bytes_index_method, molt_bytes_index_slice);
bytes_slice_method_wrapper!(molt_bytes_rindex_method, molt_bytes_rindex_slice);
bytes_slice_method_wrapper!(molt_bytes_count_method, molt_bytes_count_slice);
bytes_slice_method_wrapper!(molt_bytes_startswith_method, molt_bytes_startswith_slice);
bytes_slice_method_wrapper!(molt_bytes_endswith_method, molt_bytes_endswith_slice);

// bytearray reuses the same runtime functions as bytes, so these wrappers
// delegate to the bytes _slice variants directly.
bytes_slice_method_wrapper!(molt_bytearray_find_method, molt_bytearray_find_slice);
bytes_slice_method_wrapper!(molt_bytearray_rfind_method, molt_bytearray_rfind_slice);
bytes_slice_method_wrapper!(molt_bytearray_index_method, molt_bytearray_index_slice);
bytes_slice_method_wrapper!(molt_bytearray_rindex_method, molt_bytearray_rindex_slice);
bytes_slice_method_wrapper!(molt_bytearray_count_method, molt_bytearray_count_slice);
bytes_slice_method_wrapper!(
    molt_bytearray_startswith_method,
    molt_bytearray_startswith_slice
);
bytes_slice_method_wrapper!(
    molt_bytearray_endswith_method,
    molt_bytearray_endswith_slice
);

#[unsafe(no_mangle)]
pub extern "C" fn molt_bytes_find_slice(
    hay_bits: u64,
    needle_bits: u64,
    start_bits: u64,
    end_bits: u64,
    has_start_bits: u64,
    has_end_bits: u64,
) -> u64 {
    crate::with_gil_entry_nopanic!(_py, {
        let hay = obj_from_bits(hay_bits);
        let needle = obj_from_bits(needle_bits);
        let has_start = to_i64(obj_from_bits(has_start_bits)).unwrap_or(0) != 0;
        let has_end = to_i64(obj_from_bits(has_end_bits)).unwrap_or(0) != 0;
        if let Some(hay_ptr) = hay.as_ptr() {
            unsafe {
                if object_type_id(hay_ptr) != TYPE_ID_BYTES {
                    return MoltObject::none().bits();
                }
                let hay_bytes = bytes_like_slice(hay_ptr).unwrap_or(&[]);
                let total = hay_bytes.len() as i64;
                let (start, end, start_raw) =
                    slice_bounds_from_args(_py, start_bits, end_bits, has_start, has_end, total);
                if end < start {
                    return MoltObject::from_int(-1).bits();
                }
                let slice = &hay_bytes[start as usize..end as usize];
                if let Some(byte) = needle.as_int() {
                    if !(0..=255).contains(&byte) {
                        return raise_exception::<_>(
                            _py,
                            "ValueError",
                            "byte must be in range(0, 256)",
                        );
                    }
                    let idx = match memchr(byte as u8, slice) {
                        Some(pos) => start + pos as i64,
                        None => -1,
                    };
                    return MoltObject::from_int(idx).bits();
                }
                let needle_ptr = match needle.as_ptr() {
                    Some(ptr) => ptr,
                    None => {
                        let msg = format!(
                            "argument should be integer or bytes-like object, not '{}'",
                            type_name(_py, needle)
                        );
                        return raise_exception::<_>(_py, "TypeError", &msg);
                    }
                };
                let needle_bytes = match bytes_like_arg_or_type_error(_py, needle_ptr, || {
                    format!(
                        "argument should be integer or bytes-like object, not '{}'",
                        type_name(_py, needle)
                    )
                }) {
                    Ok(slice) => slice,
                    Err(bits) => return bits,
                };
                if needle_bytes.is_empty() {
                    if start_raw > total {
                        return MoltObject::from_int(-1).bits();
                    }
                    return MoltObject::from_int(start).bits();
                }
                let idx = bytes_find_impl(slice, needle_bytes);
                let out = if idx < 0 { -1 } else { start + idx };
                MoltObject::from_int(out).bits()
            }
        } else {
            MoltObject::none().bits()
        }
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_bytes_rfind(hay_bits: u64, needle_bits: u64) -> u64 {
    crate::with_gil_entry_nopanic!(_py, {
        let none_bits = MoltObject::none().bits();
        let false_bits = MoltObject::from_bool(false).bits();
        molt_bytes_rfind_slice(
            hay_bits,
            needle_bits,
            none_bits,
            none_bits,
            false_bits,
            false_bits,
        )
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_bytes_rfind_slice(
    hay_bits: u64,
    needle_bits: u64,
    start_bits: u64,
    end_bits: u64,
    has_start_bits: u64,
    has_end_bits: u64,
) -> u64 {
    crate::with_gil_entry_nopanic!(_py, {
        let hay = obj_from_bits(hay_bits);
        let needle = obj_from_bits(needle_bits);
        let has_start = to_i64(obj_from_bits(has_start_bits)).unwrap_or(0) != 0;
        let has_end = to_i64(obj_from_bits(has_end_bits)).unwrap_or(0) != 0;
        if let Some(hay_ptr) = hay.as_ptr() {
            unsafe {
                if object_type_id(hay_ptr) != TYPE_ID_BYTES {
                    return MoltObject::none().bits();
                }
                let hay_bytes = bytes_like_slice(hay_ptr).unwrap_or(&[]);
                let total = hay_bytes.len() as i64;
                let (start, end, start_raw) =
                    slice_bounds_from_args(_py, start_bits, end_bits, has_start, has_end, total);
                if end < start {
                    return MoltObject::from_int(-1).bits();
                }
                let slice = &hay_bytes[start as usize..end as usize];
                if let Some(byte) = needle.as_int() {
                    if !(0..=255).contains(&byte) {
                        return raise_exception::<_>(
                            _py,
                            "ValueError",
                            "byte must be in range(0, 256)",
                        );
                    }
                    let idx = match memchr::memrchr(byte as u8, slice) {
                        Some(pos) => start + pos as i64,
                        None => -1,
                    };
                    return MoltObject::from_int(idx).bits();
                }
                let needle_ptr = match needle.as_ptr() {
                    Some(ptr) => ptr,
                    None => {
                        let msg = format!(
                            "argument should be integer or bytes-like object, not '{}'",
                            type_name(_py, needle)
                        );
                        return raise_exception::<_>(_py, "TypeError", &msg);
                    }
                };
                let needle_bytes = match bytes_like_arg_or_type_error(_py, needle_ptr, || {
                    format!(
                        "argument should be integer or bytes-like object, not '{}'",
                        type_name(_py, needle)
                    )
                }) {
                    Ok(slice) => slice,
                    Err(bits) => return bits,
                };
                if needle_bytes.is_empty() {
                    if start_raw > total {
                        return MoltObject::from_int(-1).bits();
                    }
                    return MoltObject::from_int(end).bits();
                }
                let idx = bytes_rfind_impl(slice, needle_bytes);
                let out = if idx < 0 { -1 } else { start + idx };
                MoltObject::from_int(out).bits()
            }
        } else {
            MoltObject::none().bits()
        }
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_bytes_startswith(hay_bits: u64, needle_bits: u64) -> u64 {
    crate::with_gil_entry_nopanic!(_py, {
        let none_bits = MoltObject::none().bits();
        let false_bits = MoltObject::from_bool(false).bits();
        molt_bytes_startswith_slice(
            hay_bits,
            needle_bits,
            none_bits,
            none_bits,
            false_bits,
            false_bits,
        )
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_bytes_endswith(hay_bits: u64, needle_bits: u64) -> u64 {
    crate::with_gil_entry_nopanic!(_py, {
        let none_bits = MoltObject::none().bits();
        let false_bits = MoltObject::from_bool(false).bits();
        molt_bytes_endswith_slice(
            hay_bits,
            needle_bits,
            none_bits,
            none_bits,
            false_bits,
            false_bits,
        )
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_bytes_startswith_slice(
    hay_bits: u64,
    needle_bits: u64,
    start_bits: u64,
    end_bits: u64,
    has_start_bits: u64,
    has_end_bits: u64,
) -> u64 {
    crate::with_gil_entry_nopanic!(_py, {
        let hay = obj_from_bits(hay_bits);
        let needle = obj_from_bits(needle_bits);
        let has_start = to_i64(obj_from_bits(has_start_bits)).unwrap_or(0) != 0;
        let has_end = to_i64(obj_from_bits(has_end_bits)).unwrap_or(0) != 0;
        let Some(hay_ptr) = hay.as_ptr() else {
            return MoltObject::none().bits();
        };
        unsafe {
            if object_type_id(hay_ptr) != TYPE_ID_BYTES {
                return MoltObject::none().bits();
            }
            let hay_bytes = bytes_like_slice(hay_ptr).unwrap_or(&[]);
            let total = hay_bytes.len() as i64;
            let (start, end, start_raw) =
                slice_bounds_from_args(_py, start_bits, end_bits, has_start, has_end, total);
            if end < start {
                return MoltObject::from_bool(false).bits();
            }
            let slice = &hay_bytes[start as usize..end as usize];
            if let Some(needle_ptr) = needle.as_ptr() {
                let needle_type = object_type_id(needle_ptr);
                if needle_type == TYPE_ID_TUPLE {
                    let elems = seq_vec_ref(needle_ptr);
                    if elems.is_empty() {
                        return MoltObject::from_bool(false).bits();
                    }
                    for &elem_bits in elems.iter() {
                        let elem = obj_from_bits(elem_bits);
                        let elem_ptr = match elem.as_ptr() {
                            Some(ptr) => ptr,
                            None => {
                                let msg = format!(
                                    "a bytes-like object is required, not '{}'",
                                    type_name(_py, elem)
                                );
                                return raise_exception::<_>(_py, "TypeError", &msg);
                            }
                        };
                        let needle_bytes = match bytes_like_arg_or_type_error(_py, elem_ptr, || {
                            format!(
                                "a bytes-like object is required, not '{}'",
                                type_name(_py, elem)
                            )
                        }) {
                            Ok(slice) => slice,
                            Err(bits) => return bits,
                        };
                        if slice_match(slice, needle_bytes, start_raw, total, false) {
                            return MoltObject::from_bool(true).bits();
                        }
                    }
                    return MoltObject::from_bool(false).bits();
                }
                let needle_bytes = match bytes_like_arg_or_type_error(_py, needle_ptr, || {
                    format!(
                        "startswith first arg must be bytes or a tuple of bytes, not {}",
                        type_name(_py, needle)
                    )
                }) {
                    Ok(slice) => slice,
                    Err(bits) => return bits,
                };
                let ok = slice_match(slice, needle_bytes, start_raw, total, false);
                return MoltObject::from_bool(ok).bits();
            }
            let msg = format!(
                "startswith first arg must be bytes or a tuple of bytes, not {}",
                type_name(_py, needle)
            );
            raise_exception::<_>(_py, "TypeError", &msg)
        }
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_bytes_endswith_slice(
    hay_bits: u64,
    needle_bits: u64,
    start_bits: u64,
    end_bits: u64,
    has_start_bits: u64,
    has_end_bits: u64,
) -> u64 {
    crate::with_gil_entry_nopanic!(_py, {
        let hay = obj_from_bits(hay_bits);
        let needle = obj_from_bits(needle_bits);
        let has_start = to_i64(obj_from_bits(has_start_bits)).unwrap_or(0) != 0;
        let has_end = to_i64(obj_from_bits(has_end_bits)).unwrap_or(0) != 0;
        let Some(hay_ptr) = hay.as_ptr() else {
            return MoltObject::none().bits();
        };
        unsafe {
            if object_type_id(hay_ptr) != TYPE_ID_BYTES {
                return MoltObject::none().bits();
            }
            let hay_bytes = bytes_like_slice(hay_ptr).unwrap_or(&[]);
            let total = hay_bytes.len() as i64;
            let (start, end, start_raw) =
                slice_bounds_from_args(_py, start_bits, end_bits, has_start, has_end, total);
            if end < start {
                return MoltObject::from_bool(false).bits();
            }
            let slice = &hay_bytes[start as usize..end as usize];
            if let Some(needle_ptr) = needle.as_ptr() {
                let needle_type = object_type_id(needle_ptr);
                if needle_type == TYPE_ID_TUPLE {
                    let elems = seq_vec_ref(needle_ptr);
                    if elems.is_empty() {
                        return MoltObject::from_bool(false).bits();
                    }
                    for &elem_bits in elems.iter() {
                        let elem = obj_from_bits(elem_bits);
                        let elem_ptr = match elem.as_ptr() {
                            Some(ptr) => ptr,
                            None => {
                                let msg = format!(
                                    "a bytes-like object is required, not '{}'",
                                    type_name(_py, elem)
                                );
                                return raise_exception::<_>(_py, "TypeError", &msg);
                            }
                        };
                        let needle_bytes = match bytes_like_arg_or_type_error(_py, elem_ptr, || {
                            format!(
                                "a bytes-like object is required, not '{}'",
                                type_name(_py, elem)
                            )
                        }) {
                            Ok(slice) => slice,
                            Err(bits) => return bits,
                        };
                        if slice_match(slice, needle_bytes, start_raw, total, true) {
                            return MoltObject::from_bool(true).bits();
                        }
                    }
                    return MoltObject::from_bool(false).bits();
                }
                let needle_bytes = match bytes_like_arg_or_type_error(_py, needle_ptr, || {
                    format!(
                        "endswith first arg must be bytes or a tuple of bytes, not {}",
                        type_name(_py, needle)
                    )
                }) {
                    Ok(slice) => slice,
                    Err(bits) => return bits,
                };
                let ok = slice_match(slice, needle_bytes, start_raw, total, true);
                return MoltObject::from_bool(ok).bits();
            }
            let msg = format!(
                "endswith first arg must be bytes or a tuple of bytes, not {}",
                type_name(_py, needle)
            );
            raise_exception::<_>(_py, "TypeError", &msg)
        }
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_bytes_count(hay_bits: u64, needle_bits: u64) -> u64 {
    crate::with_gil_entry_nopanic!(_py, {
        let hay = obj_from_bits(hay_bits);
        let needle = obj_from_bits(needle_bits);
        let Some(hay_ptr) = hay.as_ptr() else {
            return MoltObject::none().bits();
        };
        unsafe {
            if object_type_id(hay_ptr) != TYPE_ID_BYTES {
                return MoltObject::none().bits();
            }
            let hay_bytes = bytes_like_slice(hay_ptr).unwrap_or(&[]);
            if let Some(byte) = needle.as_int() {
                if !(0..=255).contains(&byte) {
                    return raise_exception::<_>(
                        _py,
                        "ValueError",
                        "byte must be in range(0, 256)",
                    );
                }
                let count = memchr::memchr_iter(byte as u8, hay_bytes).count() as i64;
                return MoltObject::from_int(count).bits();
            }
            let needle_ptr = match needle.as_ptr() {
                Some(ptr) => ptr,
                None => {
                    let msg = format!(
                        "argument should be integer or bytes-like object, not '{}'",
                        type_name(_py, needle)
                    );
                    return raise_exception::<_>(_py, "TypeError", &msg);
                }
            };
            let needle_bytes = match bytes_like_arg_or_type_error(_py, needle_ptr, || {
                format!(
                    "argument should be integer or bytes-like object, not '{}'",
                    type_name(_py, needle)
                )
            }) {
                Ok(slice) => slice,
                Err(bits) => return bits,
            };
            let count = bytes_count_impl(hay_bytes, needle_bytes);
            MoltObject::from_int(count).bits()
        }
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_bytes_count_slice(
    hay_bits: u64,
    needle_bits: u64,
    start_bits: u64,
    end_bits: u64,
    has_start_bits: u64,
    has_end_bits: u64,
) -> u64 {
    crate::with_gil_entry_nopanic!(_py, {
        let hay = obj_from_bits(hay_bits);
        let needle = obj_from_bits(needle_bits);
        let has_start = to_i64(obj_from_bits(has_start_bits)).unwrap_or(0) != 0;
        let has_end = to_i64(obj_from_bits(has_end_bits)).unwrap_or(0) != 0;
        let Some(hay_ptr) = hay.as_ptr() else {
            return MoltObject::none().bits();
        };
        unsafe {
            if object_type_id(hay_ptr) != TYPE_ID_BYTES {
                return MoltObject::none().bits();
            }
            let hay_bytes = bytes_like_slice(hay_ptr).unwrap_or(&[]);
            let total = hay_bytes.len() as i64;
            let (start, end, start_raw) =
                slice_bounds_from_args(_py, start_bits, end_bits, has_start, has_end, total);
            if end < start {
                return MoltObject::from_int(0).bits();
            }
            if let Some(byte) = needle.as_int() {
                if !(0..=255).contains(&byte) {
                    return raise_exception::<_>(
                        _py,
                        "ValueError",
                        "byte must be in range(0, 256)",
                    );
                }
                let slice = &hay_bytes[start as usize..end as usize];
                let count = memchr::memchr_iter(byte as u8, slice).count() as i64;
                return MoltObject::from_int(count).bits();
            }
            let needle_ptr = match needle.as_ptr() {
                Some(ptr) => ptr,
                None => {
                    let msg = format!(
                        "argument should be integer or bytes-like object, not '{}'",
                        type_name(_py, needle)
                    );
                    return raise_exception::<_>(_py, "TypeError", &msg);
                }
            };
            let needle_bytes = match bytes_like_arg_or_type_error(_py, needle_ptr, || {
                format!(
                    "argument should be integer or bytes-like object, not '{}'",
                    type_name(_py, needle)
                )
            }) {
                Ok(slice) => slice,
                Err(bits) => return bits,
            };
            if needle_bytes.is_empty() {
                if start_raw > total {
                    return MoltObject::from_int(0).bits();
                }
                let count = end - start + 1;
                return MoltObject::from_int(count).bits();
            }
            let slice = &hay_bytes[start as usize..end as usize];
            let count = bytes_count_impl(slice, needle_bytes);
            MoltObject::from_int(count).bits()
        }
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_bytearray_find(hay_bits: u64, needle_bits: u64) -> u64 {
    crate::with_gil_entry_nopanic!(_py, {
        let none_bits = MoltObject::none().bits();
        let false_bits = MoltObject::from_bool(false).bits();
        molt_bytearray_find_slice(
            hay_bits,
            needle_bits,
            none_bits,
            none_bits,
            false_bits,
            false_bits,
        )
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_bytearray_rfind(hay_bits: u64, needle_bits: u64) -> u64 {
    crate::with_gil_entry_nopanic!(_py, {
        let none_bits = MoltObject::none().bits();
        let false_bits = MoltObject::from_bool(false).bits();
        molt_bytearray_rfind_slice(
            hay_bits,
            needle_bits,
            none_bits,
            none_bits,
            false_bits,
            false_bits,
        )
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_bytearray_startswith(hay_bits: u64, needle_bits: u64) -> u64 {
    crate::with_gil_entry_nopanic!(_py, {
        let none_bits = MoltObject::none().bits();
        let false_bits = MoltObject::from_bool(false).bits();
        molt_bytearray_startswith_slice(
            hay_bits,
            needle_bits,
            none_bits,
            none_bits,
            false_bits,
            false_bits,
        )
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_bytearray_endswith(hay_bits: u64, needle_bits: u64) -> u64 {
    crate::with_gil_entry_nopanic!(_py, {
        let none_bits = MoltObject::none().bits();
        let false_bits = MoltObject::from_bool(false).bits();
        molt_bytearray_endswith_slice(
            hay_bits,
            needle_bits,
            none_bits,
            none_bits,
            false_bits,
            false_bits,
        )
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_bytearray_find_slice(
    hay_bits: u64,
    needle_bits: u64,
    start_bits: u64,
    end_bits: u64,
    has_start_bits: u64,
    has_end_bits: u64,
) -> u64 {
    crate::with_gil_entry_nopanic!(_py, {
        let hay = obj_from_bits(hay_bits);
        let needle = obj_from_bits(needle_bits);
        let has_start = to_i64(obj_from_bits(has_start_bits)).unwrap_or(0) != 0;
        let has_end = to_i64(obj_from_bits(has_end_bits)).unwrap_or(0) != 0;
        if let Some(hay_ptr) = hay.as_ptr() {
            unsafe {
                if object_type_id(hay_ptr) != TYPE_ID_BYTEARRAY {
                    return MoltObject::none().bits();
                }
                let hay_bytes = bytes_like_slice(hay_ptr).unwrap_or(&[]);
                let total = hay_bytes.len() as i64;
                let (start, end, start_raw) =
                    slice_bounds_from_args(_py, start_bits, end_bits, has_start, has_end, total);
                if end < start {
                    return MoltObject::from_int(-1).bits();
                }
                let slice = &hay_bytes[start as usize..end as usize];
                if let Some(byte) = needle.as_int() {
                    if !(0..=255).contains(&byte) {
                        return raise_exception::<_>(
                            _py,
                            "ValueError",
                            "byte must be in range(0, 256)",
                        );
                    }
                    let idx = match memchr(byte as u8, slice) {
                        Some(pos) => start + pos as i64,
                        None => -1,
                    };
                    return MoltObject::from_int(idx).bits();
                }
                let needle_ptr = match needle.as_ptr() {
                    Some(ptr) => ptr,
                    None => {
                        let msg = format!(
                            "argument should be integer or bytes-like object, not '{}'",
                            type_name(_py, needle)
                        );
                        return raise_exception::<_>(_py, "TypeError", &msg);
                    }
                };
                let needle_bytes = match bytes_like_arg_or_type_error(_py, needle_ptr, || {
                    format!(
                        "argument should be integer or bytes-like object, not '{}'",
                        type_name(_py, needle)
                    )
                }) {
                    Ok(slice) => slice,
                    Err(bits) => return bits,
                };
                if needle_bytes.is_empty() {
                    if start_raw > total {
                        return MoltObject::from_int(-1).bits();
                    }
                    return MoltObject::from_int(start).bits();
                }
                let idx = bytes_find_impl(slice, needle_bytes);
                let out = if idx < 0 { -1 } else { start + idx };
                MoltObject::from_int(out).bits()
            }
        } else {
            MoltObject::none().bits()
        }
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_bytearray_rfind_slice(
    hay_bits: u64,
    needle_bits: u64,
    start_bits: u64,
    end_bits: u64,
    has_start_bits: u64,
    has_end_bits: u64,
) -> u64 {
    crate::with_gil_entry_nopanic!(_py, {
        let hay = obj_from_bits(hay_bits);
        let needle = obj_from_bits(needle_bits);
        let has_start = to_i64(obj_from_bits(has_start_bits)).unwrap_or(0) != 0;
        let has_end = to_i64(obj_from_bits(has_end_bits)).unwrap_or(0) != 0;
        if let Some(hay_ptr) = hay.as_ptr() {
            unsafe {
                if object_type_id(hay_ptr) != TYPE_ID_BYTEARRAY {
                    return MoltObject::none().bits();
                }
                let hay_bytes = bytes_like_slice(hay_ptr).unwrap_or(&[]);
                let total = hay_bytes.len() as i64;
                let (start, end, start_raw) =
                    slice_bounds_from_args(_py, start_bits, end_bits, has_start, has_end, total);
                if end < start {
                    return MoltObject::from_int(-1).bits();
                }
                let slice = &hay_bytes[start as usize..end as usize];
                if let Some(byte) = needle.as_int() {
                    if !(0..=255).contains(&byte) {
                        return raise_exception::<_>(
                            _py,
                            "ValueError",
                            "byte must be in range(0, 256)",
                        );
                    }
                    let idx = match memchr::memrchr(byte as u8, slice) {
                        Some(pos) => start + pos as i64,
                        None => -1,
                    };
                    return MoltObject::from_int(idx).bits();
                }
                let needle_ptr = match needle.as_ptr() {
                    Some(ptr) => ptr,
                    None => {
                        let msg = format!(
                            "argument should be integer or bytes-like object, not '{}'",
                            type_name(_py, needle)
                        );
                        return raise_exception::<_>(_py, "TypeError", &msg);
                    }
                };
                let needle_bytes = match bytes_like_arg_or_type_error(_py, needle_ptr, || {
                    format!(
                        "argument should be integer or bytes-like object, not '{}'",
                        type_name(_py, needle)
                    )
                }) {
                    Ok(slice) => slice,
                    Err(bits) => return bits,
                };
                if needle_bytes.is_empty() {
                    if start_raw > total {
                        return MoltObject::from_int(-1).bits();
                    }
                    return MoltObject::from_int(end).bits();
                }
                let idx = bytes_rfind_impl(slice, needle_bytes);
                let out = if idx < 0 { -1 } else { start + idx };
                MoltObject::from_int(out).bits()
            }
        } else {
            MoltObject::none().bits()
        }
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_bytearray_startswith_slice(
    hay_bits: u64,
    needle_bits: u64,
    start_bits: u64,
    end_bits: u64,
    has_start_bits: u64,
    has_end_bits: u64,
) -> u64 {
    crate::with_gil_entry_nopanic!(_py, {
        let hay = obj_from_bits(hay_bits);
        let needle = obj_from_bits(needle_bits);
        let has_start = to_i64(obj_from_bits(has_start_bits)).unwrap_or(0) != 0;
        let has_end = to_i64(obj_from_bits(has_end_bits)).unwrap_or(0) != 0;
        let Some(hay_ptr) = hay.as_ptr() else {
            return MoltObject::none().bits();
        };
        unsafe {
            if object_type_id(hay_ptr) != TYPE_ID_BYTEARRAY {
                return MoltObject::none().bits();
            }
            let hay_bytes = bytes_like_slice(hay_ptr).unwrap_or(&[]);
            let total = hay_bytes.len() as i64;
            let (start, end, start_raw) =
                slice_bounds_from_args(_py, start_bits, end_bits, has_start, has_end, total);
            if end < start {
                return MoltObject::from_bool(false).bits();
            }
            let slice = &hay_bytes[start as usize..end as usize];
            if let Some(needle_ptr) = needle.as_ptr() {
                let needle_type = object_type_id(needle_ptr);
                if needle_type == TYPE_ID_TUPLE {
                    let elems = seq_vec_ref(needle_ptr);
                    if elems.is_empty() {
                        return MoltObject::from_bool(false).bits();
                    }
                    for &elem_bits in elems.iter() {
                        let elem = obj_from_bits(elem_bits);
                        let elem_ptr = match elem.as_ptr() {
                            Some(ptr) => ptr,
                            None => {
                                let msg = format!(
                                    "a bytes-like object is required, not '{}'",
                                    type_name(_py, elem)
                                );
                                return raise_exception::<_>(_py, "TypeError", &msg);
                            }
                        };
                        let needle_bytes = match bytes_like_arg_or_type_error(_py, elem_ptr, || {
                            format!(
                                "a bytes-like object is required, not '{}'",
                                type_name(_py, elem)
                            )
                        }) {
                            Ok(slice) => slice,
                            Err(bits) => return bits,
                        };
                        if slice_match(slice, needle_bytes, start_raw, total, false) {
                            return MoltObject::from_bool(true).bits();
                        }
                    }
                    return MoltObject::from_bool(false).bits();
                }
                let needle_bytes = match bytes_like_arg_or_type_error(_py, needle_ptr, || {
                    format!(
                        "startswith first arg must be bytes or a tuple of bytes, not {}",
                        type_name(_py, needle)
                    )
                }) {
                    Ok(slice) => slice,
                    Err(bits) => return bits,
                };
                let ok = slice_match(slice, needle_bytes, start_raw, total, false);
                return MoltObject::from_bool(ok).bits();
            }
            let msg = format!(
                "startswith first arg must be bytes or a tuple of bytes, not {}",
                type_name(_py, needle)
            );
            raise_exception::<_>(_py, "TypeError", &msg)
        }
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_bytearray_endswith_slice(
    hay_bits: u64,
    needle_bits: u64,
    start_bits: u64,
    end_bits: u64,
    has_start_bits: u64,
    has_end_bits: u64,
) -> u64 {
    crate::with_gil_entry_nopanic!(_py, {
        let hay = obj_from_bits(hay_bits);
        let needle = obj_from_bits(needle_bits);
        let has_start = to_i64(obj_from_bits(has_start_bits)).unwrap_or(0) != 0;
        let has_end = to_i64(obj_from_bits(has_end_bits)).unwrap_or(0) != 0;
        let Some(hay_ptr) = hay.as_ptr() else {
            return MoltObject::none().bits();
        };
        unsafe {
            if object_type_id(hay_ptr) != TYPE_ID_BYTEARRAY {
                return MoltObject::none().bits();
            }
            let hay_bytes = bytes_like_slice(hay_ptr).unwrap_or(&[]);
            let total = hay_bytes.len() as i64;
            let (start, end, start_raw) =
                slice_bounds_from_args(_py, start_bits, end_bits, has_start, has_end, total);
            if end < start {
                return MoltObject::from_bool(false).bits();
            }
            let slice = &hay_bytes[start as usize..end as usize];
            if let Some(needle_ptr) = needle.as_ptr() {
                let needle_type = object_type_id(needle_ptr);
                if needle_type == TYPE_ID_TUPLE {
                    let elems = seq_vec_ref(needle_ptr);
                    if elems.is_empty() {
                        return MoltObject::from_bool(false).bits();
                    }
                    for &elem_bits in elems.iter() {
                        let elem = obj_from_bits(elem_bits);
                        let elem_ptr = match elem.as_ptr() {
                            Some(ptr) => ptr,
                            None => {
                                let msg = format!(
                                    "a bytes-like object is required, not '{}'",
                                    type_name(_py, elem)
                                );
                                return raise_exception::<_>(_py, "TypeError", &msg);
                            }
                        };
                        let needle_bytes = match bytes_like_arg_or_type_error(_py, elem_ptr, || {
                            format!(
                                "a bytes-like object is required, not '{}'",
                                type_name(_py, elem)
                            )
                        }) {
                            Ok(slice) => slice,
                            Err(bits) => return bits,
                        };
                        if slice_match(slice, needle_bytes, start_raw, total, true) {
                            return MoltObject::from_bool(true).bits();
                        }
                    }
                    return MoltObject::from_bool(false).bits();
                }
                let needle_bytes = match bytes_like_arg_or_type_error(_py, needle_ptr, || {
                    format!(
                        "endswith first arg must be bytes or a tuple of bytes, not {}",
                        type_name(_py, needle)
                    )
                }) {
                    Ok(slice) => slice,
                    Err(bits) => return bits,
                };
                let ok = slice_match(slice, needle_bytes, start_raw, total, true);
                return MoltObject::from_bool(ok).bits();
            }
            let msg = format!(
                "endswith first arg must be bytes or a tuple of bytes, not {}",
                type_name(_py, needle)
            );
            raise_exception::<_>(_py, "TypeError", &msg)
        }
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_bytearray_count(hay_bits: u64, needle_bits: u64) -> u64 {
    crate::with_gil_entry_nopanic!(_py, {
        let hay = obj_from_bits(hay_bits);
        let needle = obj_from_bits(needle_bits);
        let Some(hay_ptr) = hay.as_ptr() else {
            return MoltObject::none().bits();
        };
        unsafe {
            if object_type_id(hay_ptr) != TYPE_ID_BYTEARRAY {
                return MoltObject::none().bits();
            }
            let hay_bytes = bytes_like_slice(hay_ptr).unwrap_or(&[]);
            if let Some(byte) = needle.as_int() {
                if !(0..=255).contains(&byte) {
                    return raise_exception::<_>(
                        _py,
                        "ValueError",
                        "byte must be in range(0, 256)",
                    );
                }
                let count = memchr::memchr_iter(byte as u8, hay_bytes).count() as i64;
                return MoltObject::from_int(count).bits();
            }
            let needle_ptr = match needle.as_ptr() {
                Some(ptr) => ptr,
                None => {
                    let msg = format!(
                        "argument should be integer or bytes-like object, not '{}'",
                        type_name(_py, needle)
                    );
                    return raise_exception::<_>(_py, "TypeError", &msg);
                }
            };
            let needle_bytes = match bytes_like_arg_or_type_error(_py, needle_ptr, || {
                format!(
                    "argument should be integer or bytes-like object, not '{}'",
                    type_name(_py, needle)
                )
            }) {
                Ok(slice) => slice,
                Err(bits) => return bits,
            };
            let count = bytes_count_impl(hay_bytes, needle_bytes);
            MoltObject::from_int(count).bits()
        }
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_bytearray_count_slice(
    hay_bits: u64,
    needle_bits: u64,
    start_bits: u64,
    end_bits: u64,
    has_start_bits: u64,
    has_end_bits: u64,
) -> u64 {
    crate::with_gil_entry_nopanic!(_py, {
        let hay = obj_from_bits(hay_bits);
        let needle = obj_from_bits(needle_bits);
        let has_start = to_i64(obj_from_bits(has_start_bits)).unwrap_or(0) != 0;
        let has_end = to_i64(obj_from_bits(has_end_bits)).unwrap_or(0) != 0;
        let Some(hay_ptr) = hay.as_ptr() else {
            return MoltObject::none().bits();
        };
        unsafe {
            if object_type_id(hay_ptr) != TYPE_ID_BYTEARRAY {
                return MoltObject::none().bits();
            }
            let hay_bytes = bytes_like_slice(hay_ptr).unwrap_or(&[]);
            let total = hay_bytes.len() as i64;
            let (start, end, start_raw) =
                slice_bounds_from_args(_py, start_bits, end_bits, has_start, has_end, total);
            if end < start {
                return MoltObject::from_int(0).bits();
            }
            if let Some(byte) = needle.as_int() {
                if !(0..=255).contains(&byte) {
                    return raise_exception::<_>(
                        _py,
                        "ValueError",
                        "byte must be in range(0, 256)",
                    );
                }
                let slice = &hay_bytes[start as usize..end as usize];
                let count = memchr::memchr_iter(byte as u8, slice).count() as i64;
                return MoltObject::from_int(count).bits();
            }
            let needle_ptr = match needle.as_ptr() {
                Some(ptr) => ptr,
                None => {
                    let msg = format!(
                        "argument should be integer or bytes-like object, not '{}'",
                        type_name(_py, needle)
                    );
                    return raise_exception::<_>(_py, "TypeError", &msg);
                }
            };
            let needle_bytes = match bytes_like_arg_or_type_error(_py, needle_ptr, || {
                format!(
                    "argument should be integer or bytes-like object, not '{}'",
                    type_name(_py, needle)
                )
            }) {
                Ok(slice) => slice,
                Err(bits) => return bits,
            };
            if needle_bytes.is_empty() {
                if start_raw > total {
                    return MoltObject::from_int(0).bits();
                }
                let count = end - start + 1;
                return MoltObject::from_int(count).bits();
            }
            let slice = &hay_bytes[start as usize..end as usize];
            let count = bytes_count_impl(slice, needle_bytes);
            MoltObject::from_int(count).bits()
        }
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_bytes_splitlines(hay_bits: u64, keepends_bits: u64) -> u64 {
    crate::with_gil_entry_nopanic!(_py, {
        let hay = obj_from_bits(hay_bits);
        let keepends = is_truthy(_py, obj_from_bits(keepends_bits));
        let Some(hay_ptr) = hay.as_ptr() else {
            return MoltObject::none().bits();
        };
        unsafe {
            if object_type_id(hay_ptr) != TYPE_ID_BYTES {
                return MoltObject::none().bits();
            }
            let hay_bytes = bytes_like_slice(hay_ptr).unwrap_or(&[]);
            let list_bits =
                splitlines_bytes_to_list(_py, hay_bytes, keepends, |bytes| alloc_bytes(_py, bytes));
            list_bits.unwrap_or_else(|| MoltObject::none().bits())
        }
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_bytearray_splitlines(hay_bits: u64, keepends_bits: u64) -> u64 {
    crate::with_gil_entry_nopanic!(_py, {
        let hay = obj_from_bits(hay_bits);
        let keepends = is_truthy(_py, obj_from_bits(keepends_bits));
        let Some(hay_ptr) = hay.as_ptr() else {
            return MoltObject::none().bits();
        };
        unsafe {
            if object_type_id(hay_ptr) != TYPE_ID_BYTEARRAY {
                return MoltObject::none().bits();
            }
            let hay_bytes = bytes_like_slice(hay_ptr).unwrap_or(&[]);
            let list_bits = splitlines_bytes_to_list(_py, hay_bytes, keepends, |bytes| {
                alloc_bytearray(_py, bytes)
            });
            list_bits.unwrap_or_else(|| MoltObject::none().bits())
        }
    })
}

fn partition_bytes_to_tuple<F>(
    _py: &PyToken<'_>,
    hay_bytes: &[u8],
    sep_bytes: &[u8],
    from_right: bool,
    mut alloc: F,
) -> Option<u64>
where
    F: FnMut(&[u8]) -> *mut u8,
{
    let idx = if from_right {
        bytes_rfind_impl(hay_bytes, sep_bytes)
    } else {
        bytes_find_impl(hay_bytes, sep_bytes)
    };
    let (head_bytes, sep_bytes, tail_bytes) = if idx < 0 {
        if from_right {
            (&[][..], &[][..], hay_bytes)
        } else {
            (hay_bytes, &[][..], &[][..])
        }
    } else {
        let idx = idx as usize;
        let end = idx + sep_bytes.len();
        (&hay_bytes[..idx], sep_bytes, &hay_bytes[end..])
    };
    let head_ptr = alloc(head_bytes);
    if head_ptr.is_null() {
        return None;
    }
    let head_bits = MoltObject::from_ptr(head_ptr).bits();
    let sep_ptr = alloc(sep_bytes);
    if sep_ptr.is_null() {
        dec_ref_bits(_py, head_bits);
        return None;
    }
    let sep_bits = MoltObject::from_ptr(sep_ptr).bits();
    let tail_ptr = alloc(tail_bytes);
    if tail_ptr.is_null() {
        dec_ref_bits(_py, head_bits);
        dec_ref_bits(_py, sep_bits);
        return None;
    }
    let tail_bits = MoltObject::from_ptr(tail_ptr).bits();
    let tuple_ptr = alloc_tuple(_py, &[head_bits, sep_bits, tail_bits]);
    if tuple_ptr.is_null() {
        dec_ref_bits(_py, head_bits);
        dec_ref_bits(_py, sep_bits);
        dec_ref_bits(_py, tail_bits);
        return None;
    }
    Some(MoltObject::from_ptr(tuple_ptr).bits())
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_bytes_partition(hay_bits: u64, sep_bits: u64) -> u64 {
    crate::with_gil_entry_nopanic!(_py, {
        let hay = obj_from_bits(hay_bits);
        let sep = obj_from_bits(sep_bits);
        let Some(hay_ptr) = hay.as_ptr() else {
            return MoltObject::none().bits();
        };
        unsafe {
            if object_type_id(hay_ptr) != TYPE_ID_BYTES {
                return MoltObject::none().bits();
            }
            let sep_ptr = match sep.as_ptr() {
                Some(ptr) => ptr,
                None => {
                    let msg = format!(
                        "a bytes-like object is required, not '{}'",
                        type_name(_py, sep)
                    );
                    return raise_exception::<_>(_py, "TypeError", &msg);
                }
            };
            let sep_bytes = match bytes_like_arg_or_type_error(_py, sep_ptr, || {
                format!(
                    "a bytes-like object is required, not '{}'",
                    type_name(_py, sep)
                )
            }) {
                Ok(slice) => slice,
                Err(bits) => return bits,
            };
            if sep_bytes.is_empty() {
                return raise_exception::<_>(_py, "ValueError", "empty separator");
            }
            let hay_bytes = bytes_like_slice(hay_ptr).unwrap_or(&[]);
            let tuple_bits = partition_bytes_to_tuple(_py, hay_bytes, sep_bytes, false, |bytes| {
                alloc_bytes(_py, bytes)
            });
            tuple_bits.unwrap_or_else(|| MoltObject::none().bits())
        }
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_bytes_rpartition(hay_bits: u64, sep_bits: u64) -> u64 {
    crate::with_gil_entry_nopanic!(_py, {
        let hay = obj_from_bits(hay_bits);
        let sep = obj_from_bits(sep_bits);
        let Some(hay_ptr) = hay.as_ptr() else {
            return MoltObject::none().bits();
        };
        unsafe {
            if object_type_id(hay_ptr) != TYPE_ID_BYTES {
                return MoltObject::none().bits();
            }
            let sep_ptr = match sep.as_ptr() {
                Some(ptr) => ptr,
                None => {
                    let msg = format!(
                        "a bytes-like object is required, not '{}'",
                        type_name(_py, sep)
                    );
                    return raise_exception::<_>(_py, "TypeError", &msg);
                }
            };
            let sep_bytes = match bytes_like_arg_or_type_error(_py, sep_ptr, || {
                format!(
                    "a bytes-like object is required, not '{}'",
                    type_name(_py, sep)
                )
            }) {
                Ok(slice) => slice,
                Err(bits) => return bits,
            };
            if sep_bytes.is_empty() {
                return raise_exception::<_>(_py, "ValueError", "empty separator");
            }
            let hay_bytes = bytes_like_slice(hay_ptr).unwrap_or(&[]);
            let tuple_bits = partition_bytes_to_tuple(_py, hay_bytes, sep_bytes, true, |bytes| {
                alloc_bytes(_py, bytes)
            });
            tuple_bits.unwrap_or_else(|| MoltObject::none().bits())
        }
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_bytearray_partition(hay_bits: u64, sep_bits: u64) -> u64 {
    crate::with_gil_entry_nopanic!(_py, {
        let hay = obj_from_bits(hay_bits);
        let sep = obj_from_bits(sep_bits);
        let Some(hay_ptr) = hay.as_ptr() else {
            return MoltObject::none().bits();
        };
        unsafe {
            if object_type_id(hay_ptr) != TYPE_ID_BYTEARRAY {
                return MoltObject::none().bits();
            }
            let sep_ptr = match sep.as_ptr() {
                Some(ptr) => ptr,
                None => {
                    let msg = format!(
                        "a bytes-like object is required, not '{}'",
                        type_name(_py, sep)
                    );
                    return raise_exception::<_>(_py, "TypeError", &msg);
                }
            };
            let sep_bytes = match bytes_like_arg_or_type_error(_py, sep_ptr, || {
                format!(
                    "a bytes-like object is required, not '{}'",
                    type_name(_py, sep)
                )
            }) {
                Ok(slice) => slice,
                Err(bits) => return bits,
            };
            if sep_bytes.is_empty() {
                return raise_exception::<_>(_py, "ValueError", "empty separator");
            }
            let hay_bytes = bytes_like_slice(hay_ptr).unwrap_or(&[]);
            let tuple_bits = partition_bytes_to_tuple(_py, hay_bytes, sep_bytes, false, |bytes| {
                alloc_bytearray(_py, bytes)
            });
            tuple_bits.unwrap_or_else(|| MoltObject::none().bits())
        }
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_bytearray_rpartition(hay_bits: u64, sep_bits: u64) -> u64 {
    crate::with_gil_entry_nopanic!(_py, {
        let hay = obj_from_bits(hay_bits);
        let sep = obj_from_bits(sep_bits);
        let Some(hay_ptr) = hay.as_ptr() else {
            return MoltObject::none().bits();
        };
        unsafe {
            if object_type_id(hay_ptr) != TYPE_ID_BYTEARRAY {
                return MoltObject::none().bits();
            }
            let sep_ptr = match sep.as_ptr() {
                Some(ptr) => ptr,
                None => {
                    let msg = format!(
                        "a bytes-like object is required, not '{}'",
                        type_name(_py, sep)
                    );
                    return raise_exception::<_>(_py, "TypeError", &msg);
                }
            };
            let sep_bytes = match bytes_like_arg_or_type_error(_py, sep_ptr, || {
                format!(
                    "a bytes-like object is required, not '{}'",
                    type_name(_py, sep)
                )
            }) {
                Ok(slice) => slice,
                Err(bits) => return bits,
            };
            if sep_bytes.is_empty() {
                return raise_exception::<_>(_py, "ValueError", "empty separator");
            }
            let hay_bytes = bytes_like_slice(hay_ptr).unwrap_or(&[]);
            let tuple_bits = partition_bytes_to_tuple(_py, hay_bytes, sep_bytes, true, |bytes| {
                alloc_bytearray(_py, bytes)
            });
            tuple_bits.unwrap_or_else(|| MoltObject::none().bits())
        }
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_bytes_split(hay_bits: u64, needle_bits: u64) -> u64 {
    crate::with_gil_entry_nopanic!(_py, {
        let maxsplit_bits = MoltObject::from_int(-1).bits();
        molt_bytes_split_max(hay_bits, needle_bits, maxsplit_bits)
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_bytes_split_max(hay_bits: u64, needle_bits: u64, maxsplit_bits: u64) -> u64 {
    crate::with_gil_entry_nopanic!(_py, {
        let hay = obj_from_bits(hay_bits);
        let needle = obj_from_bits(needle_bits);
        let maxsplit = split_maxsplit_from_obj(_py, maxsplit_bits);
        if exception_pending(_py) {
            return MoltObject::none().bits();
        }
        if let Some(hay_ptr) = hay.as_ptr() {
            unsafe {
                if object_type_id(hay_ptr) != TYPE_ID_BYTES {
                    return MoltObject::none().bits();
                }
                let hay_bytes = bytes_like_slice(hay_ptr).unwrap_or(&[]);
                if needle.is_none() {
                    let list_bits = split_bytes_whitespace_to_list_maxsplit(
                        _py,
                        hay_bytes,
                        maxsplit,
                        |bytes| alloc_bytes(_py, bytes),
                    );
                    return list_bits.unwrap_or_else(|| MoltObject::none().bits());
                }
                let needle_ptr = match needle.as_ptr() {
                    Some(ptr) => ptr,
                    None => {
                        let msg = format!(
                            "a bytes-like object is required, not '{}'",
                            type_name(_py, needle)
                        );
                        return raise_exception::<_>(_py, "TypeError", &msg);
                    }
                };
                let needle_bytes = match bytes_like_arg_or_type_error(_py, needle_ptr, || {
                    format!(
                        "a bytes-like object is required, not '{}'",
                        type_name(_py, needle)
                    )
                }) {
                    Ok(slice) => slice,
                    Err(bits) => return bits,
                };
                if needle_bytes.is_empty() {
                    return raise_exception::<_>(_py, "ValueError", "empty separator");
                }
                let list_bits = match split_bytes_to_list_maxsplit(
                    _py,
                    hay_bytes,
                    needle_bytes,
                    maxsplit,
                    |bytes| alloc_bytes(_py, bytes),
                ) {
                    Some(val) => val,
                    None => return MoltObject::none().bits(),
                };
                return list_bits;
            }
        }
        MoltObject::none().bits()
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_bytes_rsplit(hay_bits: u64, needle_bits: u64) -> u64 {
    crate::with_gil_entry_nopanic!(_py, {
        let maxsplit_bits = MoltObject::from_int(-1).bits();
        molt_bytes_rsplit_max(hay_bits, needle_bits, maxsplit_bits)
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_bytes_rsplit_max(
    hay_bits: u64,
    needle_bits: u64,
    maxsplit_bits: u64,
) -> u64 {
    crate::with_gil_entry_nopanic!(_py, {
        let hay = obj_from_bits(hay_bits);
        let needle = obj_from_bits(needle_bits);
        let maxsplit = split_maxsplit_from_obj(_py, maxsplit_bits);
        if exception_pending(_py) {
            return MoltObject::none().bits();
        }
        if let Some(hay_ptr) = hay.as_ptr() {
            unsafe {
                if object_type_id(hay_ptr) != TYPE_ID_BYTES {
                    return MoltObject::none().bits();
                }
                let hay_bytes = bytes_like_slice(hay_ptr).unwrap_or(&[]);
                if needle.is_none() {
                    let list_bits = rsplit_bytes_whitespace_to_list_maxsplit(
                        _py,
                        hay_bytes,
                        maxsplit,
                        |bytes| alloc_bytes(_py, bytes),
                    );
                    return list_bits.unwrap_or_else(|| MoltObject::none().bits());
                }
                let needle_ptr = match needle.as_ptr() {
                    Some(ptr) => ptr,
                    None => {
                        let msg = format!(
                            "a bytes-like object is required, not '{}'",
                            type_name(_py, needle)
                        );
                        return raise_exception::<_>(_py, "TypeError", &msg);
                    }
                };
                let needle_bytes = match bytes_like_arg_or_type_error(_py, needle_ptr, || {
                    format!(
                        "a bytes-like object is required, not '{}'",
                        type_name(_py, needle)
                    )
                }) {
                    Ok(slice) => slice,
                    Err(bits) => return bits,
                };
                if needle_bytes.is_empty() {
                    return raise_exception::<_>(_py, "ValueError", "empty separator");
                }
                let list_bits = rsplit_bytes_to_list_maxsplit(
                    _py,
                    hay_bytes,
                    needle_bytes,
                    maxsplit,
                    |bytes| alloc_bytes(_py, bytes),
                );
                let list_bits = match list_bits {
                    Some(val) => val,
                    None => return MoltObject::none().bits(),
                };
                return list_bits;
            }
        }
        MoltObject::none().bits()
    })
}

fn trace_bytes_strip() -> bool {
    static TRACE: OnceLock<bool> = OnceLock::new();
    *TRACE.get_or_init(|| {
        matches!(
            std::env::var("MOLT_TRACE_BYTES_STRIP").ok().as_deref(),
            Some("1")
        )
    })
}

fn bytes_strip_impl<F>(
    _py: &PyToken<'_>,
    hay_bits: u64,
    chars_bits: u64,
    type_id: u32,
    mut alloc: F,
    left: bool,
    right: bool,
) -> u64
where
    F: FnMut(&[u8]) -> *mut u8,
{
    const ASCII_WHITESPACE: [u8; 6] = [b' ', b'\t', b'\n', b'\r', 0x0b, 0x0c];
    let trace = trace_bytes_strip();
    let hay = obj_from_bits(hay_bits);
    let chars = obj_from_bits(chars_bits);
    let Some(hay_ptr) = hay.as_ptr() else {
        return MoltObject::none().bits();
    };
    unsafe {
        if object_type_id(hay_ptr) != type_id {
            return MoltObject::none().bits();
        }
        let hay_bytes = bytes_like_slice(hay_ptr).unwrap_or(&[]);
        let strip_bytes = if chars.is_none() {
            &ASCII_WHITESPACE[..]
        } else {
            let Some(chars_ptr) = chars.as_ptr() else {
                let msg = format!(
                    "a bytes-like object is required, not '{}'",
                    type_name(_py, chars)
                );
                return raise_exception::<_>(_py, "TypeError", &msg);
            };
            match bytes_like_arg_or_type_error(_py, chars_ptr, || {
                format!(
                    "a bytes-like object is required, not '{}'",
                    type_name(_py, chars)
                )
            }) {
                Ok(slice) => slice,
                Err(bits) => return bits,
            }
        };
        let (start, end) = bytes_strip_range(hay_bytes, strip_bytes, left, right);
        let ptr = alloc(&hay_bytes[start..end]);
        if trace {
            eprintln!(
                "bytes_strip_impl start={start} end={end} out_len={} ptr_null={}",
                end.saturating_sub(start),
                ptr.is_null()
            );
        }
        if ptr.is_null() {
            return MoltObject::none().bits();
        }
        MoltObject::from_ptr(ptr).bits()
    }
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_bytes_strip(hay_bits: u64, chars_bits: u64) -> u64 {
    crate::with_gil_entry_nopanic!(_py, {
        if trace_bytes_strip() {
            let hay = obj_from_bits(hay_bits);
            let info = hay
                .as_ptr()
                .map(|ptr| unsafe { (object_type_id(ptr), bytes_len(ptr)) });
            eprintln!("molt_bytes_strip hay_bits={hay_bits} info={info:?}");
        }
        bytes_strip_impl(
            _py,
            hay_bits,
            chars_bits,
            TYPE_ID_BYTES,
            |bytes| alloc_bytes(_py, bytes),
            true,
            true,
        )
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_bytes_lstrip(hay_bits: u64, chars_bits: u64) -> u64 {
    crate::with_gil_entry_nopanic!(_py, {
        bytes_strip_impl(
            _py,
            hay_bits,
            chars_bits,
            TYPE_ID_BYTES,
            |bytes| alloc_bytes(_py, bytes),
            true,
            false,
        )
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_bytes_rstrip(hay_bits: u64, chars_bits: u64) -> u64 {
    crate::with_gil_entry_nopanic!(_py, {
        bytes_strip_impl(
            _py,
            hay_bits,
            chars_bits,
            TYPE_ID_BYTES,
            |bytes| alloc_bytes(_py, bytes),
            false,
            true,
        )
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_bytearray_split(hay_bits: u64, needle_bits: u64) -> u64 {
    crate::with_gil_entry_nopanic!(_py, {
        let maxsplit_bits = MoltObject::from_int(-1).bits();
        molt_bytearray_split_max(hay_bits, needle_bits, maxsplit_bits)
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_bytearray_split_max(
    hay_bits: u64,
    needle_bits: u64,
    maxsplit_bits: u64,
) -> u64 {
    crate::with_gil_entry_nopanic!(_py, {
        let hay = obj_from_bits(hay_bits);
        let needle = obj_from_bits(needle_bits);
        let maxsplit = split_maxsplit_from_obj(_py, maxsplit_bits);
        if exception_pending(_py) {
            return MoltObject::none().bits();
        }
        if let Some(hay_ptr) = hay.as_ptr() {
            unsafe {
                if object_type_id(hay_ptr) != TYPE_ID_BYTEARRAY {
                    return MoltObject::none().bits();
                }
                let hay_bytes = bytes_like_slice(hay_ptr).unwrap_or(&[]);
                if needle.is_none() {
                    let list_bits = split_bytes_whitespace_to_list_maxsplit(
                        _py,
                        hay_bytes,
                        maxsplit,
                        |bytes| alloc_bytearray(_py, bytes),
                    );
                    return list_bits.unwrap_or_else(|| MoltObject::none().bits());
                }
                let needle_ptr = match needle.as_ptr() {
                    Some(ptr) => ptr,
                    None => {
                        let msg = format!(
                            "a bytes-like object is required, not '{}'",
                            type_name(_py, needle)
                        );
                        return raise_exception::<_>(_py, "TypeError", &msg);
                    }
                };
                let needle_bytes = match bytes_like_arg_or_type_error(_py, needle_ptr, || {
                    format!(
                        "a bytes-like object is required, not '{}'",
                        type_name(_py, needle)
                    )
                }) {
                    Ok(slice) => slice,
                    Err(bits) => return bits,
                };
                if needle_bytes.is_empty() {
                    return raise_exception::<_>(_py, "ValueError", "empty separator");
                }
                let list_bits = match split_bytes_to_list_maxsplit(
                    _py,
                    hay_bytes,
                    needle_bytes,
                    maxsplit,
                    |bytes| alloc_bytearray(_py, bytes),
                ) {
                    Some(val) => val,
                    None => return MoltObject::none().bits(),
                };
                return list_bits;
            }
        }
        MoltObject::none().bits()
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_bytearray_rsplit(hay_bits: u64, needle_bits: u64) -> u64 {
    crate::with_gil_entry_nopanic!(_py, {
        let maxsplit_bits = MoltObject::from_int(-1).bits();
        molt_bytearray_rsplit_max(hay_bits, needle_bits, maxsplit_bits)
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_bytearray_rsplit_max(
    hay_bits: u64,
    needle_bits: u64,
    maxsplit_bits: u64,
) -> u64 {
    crate::with_gil_entry_nopanic!(_py, {
        let hay = obj_from_bits(hay_bits);
        let needle = obj_from_bits(needle_bits);
        let maxsplit = split_maxsplit_from_obj(_py, maxsplit_bits);
        if exception_pending(_py) {
            return MoltObject::none().bits();
        }
        if let Some(hay_ptr) = hay.as_ptr() {
            unsafe {
                if object_type_id(hay_ptr) != TYPE_ID_BYTEARRAY {
                    return MoltObject::none().bits();
                }
                let hay_bytes = bytes_like_slice(hay_ptr).unwrap_or(&[]);
                if needle.is_none() {
                    let list_bits = rsplit_bytes_whitespace_to_list_maxsplit(
                        _py,
                        hay_bytes,
                        maxsplit,
                        |bytes| alloc_bytearray(_py, bytes),
                    );
                    return list_bits.unwrap_or_else(|| MoltObject::none().bits());
                }
                let needle_ptr = match needle.as_ptr() {
                    Some(ptr) => ptr,
                    None => {
                        let msg = format!(
                            "a bytes-like object is required, not '{}'",
                            type_name(_py, needle)
                        );
                        return raise_exception::<_>(_py, "TypeError", &msg);
                    }
                };
                let needle_bytes = match bytes_like_arg_or_type_error(_py, needle_ptr, || {
                    format!(
                        "a bytes-like object is required, not '{}'",
                        type_name(_py, needle)
                    )
                }) {
                    Ok(slice) => slice,
                    Err(bits) => return bits,
                };
                if needle_bytes.is_empty() {
                    return raise_exception::<_>(_py, "ValueError", "empty separator");
                }
                let list_bits = rsplit_bytes_to_list_maxsplit(
                    _py,
                    hay_bytes,
                    needle_bytes,
                    maxsplit,
                    |bytes| alloc_bytearray(_py, bytes),
                );
                let list_bits = match list_bits {
                    Some(val) => val,
                    None => return MoltObject::none().bits(),
                };
                return list_bits;
            }
        }
        MoltObject::none().bits()
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_bytearray_strip(hay_bits: u64, chars_bits: u64) -> u64 {
    crate::with_gil_entry_nopanic!(_py, {
        bytes_strip_impl(
            _py,
            hay_bits,
            chars_bits,
            TYPE_ID_BYTEARRAY,
            |bytes| alloc_bytearray(_py, bytes),
            true,
            true,
        )
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_bytearray_lstrip(hay_bits: u64, chars_bits: u64) -> u64 {
    crate::with_gil_entry_nopanic!(_py, {
        bytes_strip_impl(
            _py,
            hay_bits,
            chars_bits,
            TYPE_ID_BYTEARRAY,
            |bytes| alloc_bytearray(_py, bytes),
            true,
            false,
        )
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_bytearray_rstrip(hay_bits: u64, chars_bits: u64) -> u64 {
    crate::with_gil_entry_nopanic!(_py, {
        bytes_strip_impl(
            _py,
            hay_bits,
            chars_bits,
            TYPE_ID_BYTEARRAY,
            |bytes| alloc_bytearray(_py, bytes),
            false,
            true,
        )
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_bytes_index_slice(
    hay_bits: u64,
    needle_bits: u64,
    start_bits: u64,
    end_bits: u64,
    has_start_bits: u64,
    has_end_bits: u64,
) -> u64 {
    crate::with_gil_entry_nopanic!(_py, {
        let idx_bits = molt_bytes_find_slice(
            hay_bits,
            needle_bits,
            start_bits,
            end_bits,
            has_start_bits,
            has_end_bits,
        );
        if exception_pending(_py) {
            return MoltObject::none().bits();
        }
        if to_i64(obj_from_bits(idx_bits)).unwrap_or(-1) < 0 {
            return raise_exception::<_>(_py, "ValueError", "subsection not found");
        }
        idx_bits
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_bytes_rindex_slice(
    hay_bits: u64,
    needle_bits: u64,
    start_bits: u64,
    end_bits: u64,
    has_start_bits: u64,
    has_end_bits: u64,
) -> u64 {
    crate::with_gil_entry_nopanic!(_py, {
        let idx_bits = molt_bytes_rfind_slice(
            hay_bits,
            needle_bits,
            start_bits,
            end_bits,
            has_start_bits,
            has_end_bits,
        );
        if exception_pending(_py) {
            return MoltObject::none().bits();
        }
        if to_i64(obj_from_bits(idx_bits)).unwrap_or(-1) < 0 {
            return raise_exception::<_>(_py, "ValueError", "subsection not found");
        }
        idx_bits
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_bytearray_index_slice(
    hay_bits: u64,
    needle_bits: u64,
    start_bits: u64,
    end_bits: u64,
    has_start_bits: u64,
    has_end_bits: u64,
) -> u64 {
    crate::with_gil_entry_nopanic!(_py, {
        let idx_bits = molt_bytearray_find_slice(
            hay_bits,
            needle_bits,
            start_bits,
            end_bits,
            has_start_bits,
            has_end_bits,
        );
        if exception_pending(_py) {
            return MoltObject::none().bits();
        }
        if to_i64(obj_from_bits(idx_bits)).unwrap_or(-1) < 0 {
            return raise_exception::<_>(_py, "ValueError", "subsection not found");
        }
        idx_bits
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_bytearray_rindex_slice(
    hay_bits: u64,
    needle_bits: u64,
    start_bits: u64,
    end_bits: u64,
    has_start_bits: u64,
    has_end_bits: u64,
) -> u64 {
    crate::with_gil_entry_nopanic!(_py, {
        let idx_bits = molt_bytearray_rfind_slice(
            hay_bits,
            needle_bits,
            start_bits,
            end_bits,
            has_start_bits,
            has_end_bits,
        );
        if exception_pending(_py) {
            return MoltObject::none().bits();
        }
        if to_i64(obj_from_bits(idx_bits)).unwrap_or(-1) < 0 {
            return raise_exception::<_>(_py, "ValueError", "subsection not found");
        }
        idx_bits
    })
}
