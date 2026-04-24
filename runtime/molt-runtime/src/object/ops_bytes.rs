//! Bytes and bytearray operations — extracted from ops.rs for tree-shaking.
//!
//! Each `pub extern "C" fn molt_bytes_*` / `molt_bytearray_*` is a separate
//! linker symbol so that `wasm-ld --gc-sections` can drop unused entries.

use crate::*;
use memchr::memchr;
use molt_obj_model::MoltObject;
use num_traits::{Signed, ToPrimitive};
use std::sync::OnceLock;

use super::ops::{
    bytes_ascii_capitalize, bytes_ascii_lower, bytes_ascii_swapcase, bytes_ascii_title,
    bytes_ascii_upper, decode_error_byte, decode_error_range, parse_codec_arg,
    simd_is_all_ascii_alnum, simd_is_all_ascii_alpha, simd_is_all_ascii_digit,
    simd_is_all_ascii_whitespace, slice_bounds_from_args, slice_match,
};

pub(super) fn collect_bytearray_assign_bytes(_py: &PyToken<'_>, bits: u64) -> Option<Vec<u8>> {
    let obj = obj_from_bits(bits);
    if let Some(ptr) = obj.as_ptr() {
        unsafe {
            let type_id = object_type_id(ptr);
            if type_id == TYPE_ID_BYTES || type_id == TYPE_ID_BYTEARRAY {
                return Some(bytes_like_slice_raw(ptr).unwrap_or(&[]).to_vec());
            }
            if type_id == TYPE_ID_STRING {
                return raise_exception::<_>(
                    _py,
                    "TypeError",
                    "can assign only bytes, buffers, or iterables of ints in range(0, 256)",
                );
            }
            if type_id == TYPE_ID_MEMORYVIEW {
                if let Some(slice) = memoryview_bytes_slice(ptr) {
                    return Some(slice.to_vec());
                }
                return memoryview_collect_bytes(ptr);
            }
        }
    }
    let iter_bits = molt_iter(bits);
    if obj_from_bits(iter_bits).is_none() {
        if exception_pending(_py) {
            return None;
        }
        return raise_exception::<_>(
            _py,
            "TypeError",
            "can assign only bytes, buffers, or iterables of ints in range(0, 256)",
        );
    }
    bytes_collect_from_iter(_py, iter_bits, BytesCtorKind::Bytearray)
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_bytearray_extend(bytearray_bits: u64, other_bits: u64) -> u64 {
    crate::with_gil_entry_nopanic!(_py, {
        let bytearray_obj = obj_from_bits(bytearray_bits);
        let Some(bytearray_ptr) = bytearray_obj.as_ptr() else {
            return raise_exception::<_>(_py, "TypeError", "bytearray.extend expects bytearray");
        };
        unsafe {
            if object_type_id(bytearray_ptr) != TYPE_ID_BYTEARRAY {
                return raise_exception::<_>(
                    _py,
                    "TypeError",
                    "bytearray.extend expects bytearray",
                );
            }
        }
        let Some(payload) = collect_bytearray_assign_bytes(_py, other_bits) else {
            return MoltObject::none().bits();
        };
        unsafe {
            bytearray_vec(bytearray_ptr).extend_from_slice(&payload);
        }
        MoltObject::none().bits()
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_bytearray_append(bytearray_bits: u64, val_bits: u64) -> u64 {
    crate::with_gil_entry_nopanic!(_py, {
        let bytearray_obj = obj_from_bits(bytearray_bits);
        let Some(bytearray_ptr) = bytearray_obj.as_ptr() else {
            return raise_exception::<_>(_py, "TypeError", "bytearray.append expects bytearray");
        };
        unsafe {
            if object_type_id(bytearray_ptr) != TYPE_ID_BYTEARRAY {
                return raise_exception::<_>(
                    _py,
                    "TypeError",
                    "bytearray.append expects bytearray",
                );
            }
        }
        let Some(byte) = bytes_item_to_u8(_py, val_bits, BytesCtorKind::Bytearray) else {
            return MoltObject::none().bits();
        };
        unsafe {
            bytearray_vec(bytearray_ptr).push(byte);
        }
        MoltObject::none().bits()
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_bytearray_clear(bytearray_bits: u64) -> u64 {
    crate::with_gil_entry_nopanic!(_py, {
        let bytearray_obj = obj_from_bits(bytearray_bits);
        let Some(bytearray_ptr) = bytearray_obj.as_ptr() else {
            return raise_exception::<_>(_py, "TypeError", "bytearray.clear expects bytearray");
        };
        unsafe {
            if object_type_id(bytearray_ptr) != TYPE_ID_BYTEARRAY {
                return raise_exception::<_>(_py, "TypeError", "bytearray.clear expects bytearray");
            }
            bytearray_vec(bytearray_ptr).clear();
        }
        MoltObject::none().bits()
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_bytearray_copy(bytearray_bits: u64) -> u64 {
    crate::with_gil_entry_nopanic!(_py, {
        let bytearray_obj = obj_from_bits(bytearray_bits);
        let Some(bytearray_ptr) = bytearray_obj.as_ptr() else {
            return raise_exception::<_>(_py, "TypeError", "bytearray.copy expects bytearray");
        };
        unsafe {
            if object_type_id(bytearray_ptr) != TYPE_ID_BYTEARRAY {
                return raise_exception::<_>(_py, "TypeError", "bytearray.copy expects bytearray");
            }
            let data = bytes_like_slice(bytearray_ptr).unwrap_or(&[]);
            let ptr = alloc_bytearray(_py, data);
            if ptr.is_null() {
                return MoltObject::none().bits();
            }
            MoltObject::from_ptr(ptr).bits()
        }
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_bytearray_insert(
    bytearray_bits: u64,
    index_bits: u64,
    val_bits: u64,
) -> u64 {
    crate::with_gil_entry_nopanic!(_py, {
        let bytearray_obj = obj_from_bits(bytearray_bits);
        let Some(bytearray_ptr) = bytearray_obj.as_ptr() else {
            return raise_exception::<_>(_py, "TypeError", "bytearray.insert expects bytearray");
        };
        unsafe {
            if object_type_id(bytearray_ptr) != TYPE_ID_BYTEARRAY {
                return raise_exception::<_>(
                    _py,
                    "TypeError",
                    "bytearray.insert expects bytearray",
                );
            }
            let Some(byte) = bytes_item_to_u8(_py, val_bits, BytesCtorKind::Bytearray) else {
                return MoltObject::none().bits();
            };
            let len = bytearray_vec_ref(bytearray_ptr).len() as i64;
            let mut idx = index_i64_from_obj(
                _py,
                index_bits,
                "bytearray indices must be integers or have an __index__ method",
            );
            if exception_pending(_py) {
                return MoltObject::none().bits();
            }
            if idx < 0 {
                idx += len;
            }
            if idx < 0 {
                idx = 0;
            }
            if idx > len {
                idx = len;
            }
            bytearray_vec(bytearray_ptr).insert(idx as usize, byte);
            MoltObject::none().bits()
        }
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_bytearray_pop(bytearray_bits: u64, index_bits: u64) -> u64 {
    crate::with_gil_entry_nopanic!(_py, {
        let bytearray_obj = obj_from_bits(bytearray_bits);
        let index_obj = obj_from_bits(index_bits);
        let Some(bytearray_ptr) = bytearray_obj.as_ptr() else {
            return raise_exception::<_>(_py, "TypeError", "bytearray.pop expects bytearray");
        };
        unsafe {
            if object_type_id(bytearray_ptr) != TYPE_ID_BYTEARRAY {
                return raise_exception::<_>(_py, "TypeError", "bytearray.pop expects bytearray");
            }
            let elems = bytearray_vec(bytearray_ptr);
            let len = elems.len() as i64;
            if len == 0 {
                return raise_exception::<_>(_py, "IndexError", "pop from empty bytearray");
            }
            let mut idx = if index_obj.is_none() {
                len - 1
            } else {
                index_i64_from_obj(
                    _py,
                    index_bits,
                    "bytearray indices must be integers or have an __index__ method",
                )
            };
            if exception_pending(_py) {
                return MoltObject::none().bits();
            }
            if idx < 0 {
                idx += len;
            }
            if idx < 0 || idx >= len {
                return raise_exception::<_>(_py, "IndexError", "pop index out of range");
            }
            let out = elems.remove(idx as usize);
            MoltObject::from_int(i64::from(out)).bits()
        }
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_bytearray_remove(bytearray_bits: u64, val_bits: u64) -> u64 {
    crate::with_gil_entry_nopanic!(_py, {
        let bytearray_obj = obj_from_bits(bytearray_bits);
        let Some(bytearray_ptr) = bytearray_obj.as_ptr() else {
            return raise_exception::<_>(_py, "TypeError", "bytearray.remove expects bytearray");
        };
        unsafe {
            if object_type_id(bytearray_ptr) != TYPE_ID_BYTEARRAY {
                return raise_exception::<_>(
                    _py,
                    "TypeError",
                    "bytearray.remove expects bytearray",
                );
            }
            let Some(byte) = bytes_item_to_u8(_py, val_bits, BytesCtorKind::Bytearray) else {
                return MoltObject::none().bits();
            };
            let elems = bytearray_vec(bytearray_ptr);
            if let Some(pos) = elems.iter().position(|item| *item == byte) {
                elems.remove(pos);
                return MoltObject::none().bits();
            }
            raise_exception::<_>(_py, "ValueError", "value not found in bytearray")
        }
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_bytearray_reverse(bytearray_bits: u64) -> u64 {
    crate::with_gil_entry_nopanic!(_py, {
        let bytearray_obj = obj_from_bits(bytearray_bits);
        let Some(bytearray_ptr) = bytearray_obj.as_ptr() else {
            return raise_exception::<_>(_py, "TypeError", "bytearray.reverse expects bytearray");
        };
        unsafe {
            if object_type_id(bytearray_ptr) != TYPE_ID_BYTEARRAY {
                return raise_exception::<_>(
                    _py,
                    "TypeError",
                    "bytearray.reverse expects bytearray",
                );
            }
            bytearray_vec(bytearray_ptr).reverse();
            MoltObject::none().bits()
        }
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_bytearray_resize(bytearray_bits: u64, size_bits: u64) -> u64 {
    crate::with_gil_entry_nopanic!(_py, {
        let bytearray_obj = obj_from_bits(bytearray_bits);
        let Some(bytearray_ptr) = bytearray_obj.as_ptr() else {
            return raise_exception::<_>(_py, "TypeError", "bytearray.resize expects bytearray");
        };
        let size = index_i64_from_obj(
            _py,
            size_bits,
            "bytearray.resize() argument must be integer",
        );
        if exception_pending(_py) {
            return MoltObject::none().bits();
        }
        if size < 0 {
            let msg = format!("Can only resize to positive sizes, got {size}");
            return raise_exception::<_>(_py, "ValueError", &msg);
        }
        unsafe {
            if object_type_id(bytearray_ptr) != TYPE_ID_BYTEARRAY {
                return raise_exception::<_>(
                    _py,
                    "TypeError",
                    "bytearray.resize expects bytearray",
                );
            }
            bytearray_vec(bytearray_ptr).resize(size as usize, 0u8);
        }
        MoltObject::none().bits()
    })
}

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
                        let Some(elem_bytes) = bytes_like_slice(elem_ptr) else {
                            let msg = format!(
                                "sequence item {idx}: expected a bytes-like object, {} found",
                                type_name(_py, elem_obj)
                            );
                            return raise_exception::<_>(_py, "TypeError", &msg);
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
                    let Some(elem_bytes) = bytes_like_slice(elem_ptr) else {
                        for bits in owned_bits.iter().copied() {
                            dec_ref_bits(_py, bits);
                        }
                        let msg = format!(
                            "sequence item {idx}: expected a bytes-like object, {} found",
                            type_name(_py, elem_obj)
                        );
                        return raise_exception::<_>(_py, "TypeError", &msg);
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
                let needle_bytes = match bytes_like_slice(needle_ptr) {
                    Some(slice) => slice,
                    None => {
                        let msg = format!(
                            "argument should be integer or bytes-like object, not '{}'",
                            type_name(_py, needle)
                        );
                        return raise_exception::<_>(_py, "TypeError", &msg);
                    }
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
                let needle_bytes = match bytes_like_slice(needle_ptr) {
                    Some(slice) => slice,
                    None => {
                        let msg = format!(
                            "argument should be integer or bytes-like object, not '{}'",
                            type_name(_py, needle)
                        );
                        return raise_exception::<_>(_py, "TypeError", &msg);
                    }
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
                        let needle_bytes = match bytes_like_slice(elem_ptr) {
                            Some(slice) => slice,
                            None => {
                                let msg = format!(
                                    "a bytes-like object is required, not '{}'",
                                    type_name(_py, elem)
                                );
                                return raise_exception::<_>(_py, "TypeError", &msg);
                            }
                        };
                        if slice_match(slice, needle_bytes, start_raw, total, false) {
                            return MoltObject::from_bool(true).bits();
                        }
                    }
                    return MoltObject::from_bool(false).bits();
                }
                if let Some(needle_bytes) = bytes_like_slice(needle_ptr) {
                    let ok = slice_match(slice, needle_bytes, start_raw, total, false);
                    return MoltObject::from_bool(ok).bits();
                }
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
                        let needle_bytes = match bytes_like_slice(elem_ptr) {
                            Some(slice) => slice,
                            None => {
                                let msg = format!(
                                    "a bytes-like object is required, not '{}'",
                                    type_name(_py, elem)
                                );
                                return raise_exception::<_>(_py, "TypeError", &msg);
                            }
                        };
                        if slice_match(slice, needle_bytes, start_raw, total, true) {
                            return MoltObject::from_bool(true).bits();
                        }
                    }
                    return MoltObject::from_bool(false).bits();
                }
                if let Some(needle_bytes) = bytes_like_slice(needle_ptr) {
                    let ok = slice_match(slice, needle_bytes, start_raw, total, true);
                    return MoltObject::from_bool(ok).bits();
                }
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
            let needle_bytes = match bytes_like_slice(needle_ptr) {
                Some(slice) => slice,
                None => {
                    let msg = format!(
                        "argument should be integer or bytes-like object, not '{}'",
                        type_name(_py, needle)
                    );
                    return raise_exception::<_>(_py, "TypeError", &msg);
                }
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
            let needle_bytes = match bytes_like_slice(needle_ptr) {
                Some(slice) => slice,
                None => {
                    let msg = format!(
                        "argument should be integer or bytes-like object, not '{}'",
                        type_name(_py, needle)
                    );
                    return raise_exception::<_>(_py, "TypeError", &msg);
                }
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
                let needle_bytes = match bytes_like_slice(needle_ptr) {
                    Some(slice) => slice,
                    None => {
                        let msg = format!(
                            "argument should be integer or bytes-like object, not '{}'",
                            type_name(_py, needle)
                        );
                        return raise_exception::<_>(_py, "TypeError", &msg);
                    }
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
                let needle_bytes = match bytes_like_slice(needle_ptr) {
                    Some(slice) => slice,
                    None => {
                        let msg = format!(
                            "argument should be integer or bytes-like object, not '{}'",
                            type_name(_py, needle)
                        );
                        return raise_exception::<_>(_py, "TypeError", &msg);
                    }
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
                        let needle_bytes = match bytes_like_slice(elem_ptr) {
                            Some(slice) => slice,
                            None => {
                                let msg = format!(
                                    "a bytes-like object is required, not '{}'",
                                    type_name(_py, elem)
                                );
                                return raise_exception::<_>(_py, "TypeError", &msg);
                            }
                        };
                        if slice_match(slice, needle_bytes, start_raw, total, false) {
                            return MoltObject::from_bool(true).bits();
                        }
                    }
                    return MoltObject::from_bool(false).bits();
                }
                if let Some(needle_bytes) = bytes_like_slice(needle_ptr) {
                    let ok = slice_match(slice, needle_bytes, start_raw, total, false);
                    return MoltObject::from_bool(ok).bits();
                }
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
                        let needle_bytes = match bytes_like_slice(elem_ptr) {
                            Some(slice) => slice,
                            None => {
                                let msg = format!(
                                    "a bytes-like object is required, not '{}'",
                                    type_name(_py, elem)
                                );
                                return raise_exception::<_>(_py, "TypeError", &msg);
                            }
                        };
                        if slice_match(slice, needle_bytes, start_raw, total, true) {
                            return MoltObject::from_bool(true).bits();
                        }
                    }
                    return MoltObject::from_bool(false).bits();
                }
                if let Some(needle_bytes) = bytes_like_slice(needle_ptr) {
                    let ok = slice_match(slice, needle_bytes, start_raw, total, true);
                    return MoltObject::from_bool(ok).bits();
                }
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
            let needle_bytes = match bytes_like_slice(needle_ptr) {
                Some(slice) => slice,
                None => {
                    let msg = format!(
                        "argument should be integer or bytes-like object, not '{}'",
                        type_name(_py, needle)
                    );
                    return raise_exception::<_>(_py, "TypeError", &msg);
                }
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
            let needle_bytes = match bytes_like_slice(needle_ptr) {
                Some(slice) => slice,
                None => {
                    let msg = format!(
                        "argument should be integer or bytes-like object, not '{}'",
                        type_name(_py, needle)
                    );
                    return raise_exception::<_>(_py, "TypeError", &msg);
                }
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
            let sep_bytes = match bytes_like_slice(sep_ptr) {
                Some(slice) => slice,
                None => {
                    let msg = format!(
                        "a bytes-like object is required, not '{}'",
                        type_name(_py, sep)
                    );
                    return raise_exception::<_>(_py, "TypeError", &msg);
                }
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
            let sep_bytes = match bytes_like_slice(sep_ptr) {
                Some(slice) => slice,
                None => {
                    let msg = format!(
                        "a bytes-like object is required, not '{}'",
                        type_name(_py, sep)
                    );
                    return raise_exception::<_>(_py, "TypeError", &msg);
                }
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
            let sep_bytes = match bytes_like_slice(sep_ptr) {
                Some(slice) => slice,
                None => {
                    let msg = format!(
                        "a bytes-like object is required, not '{}'",
                        type_name(_py, sep)
                    );
                    return raise_exception::<_>(_py, "TypeError", &msg);
                }
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
            let sep_bytes = match bytes_like_slice(sep_ptr) {
                Some(slice) => slice,
                None => {
                    let msg = format!(
                        "a bytes-like object is required, not '{}'",
                        type_name(_py, sep)
                    );
                    return raise_exception::<_>(_py, "TypeError", &msg);
                }
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
                let needle_bytes = match bytes_like_slice(needle_ptr) {
                    Some(slice) => slice,
                    None => {
                        let msg = format!(
                            "a bytes-like object is required, not '{}'",
                            type_name(_py, needle)
                        );
                        return raise_exception::<_>(_py, "TypeError", &msg);
                    }
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
                let needle_bytes = match bytes_like_slice(needle_ptr) {
                    Some(slice) => slice,
                    None => {
                        let msg = format!(
                            "a bytes-like object is required, not '{}'",
                            type_name(_py, needle)
                        );
                        return raise_exception::<_>(_py, "TypeError", &msg);
                    }
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
            match bytes_like_slice(chars_ptr) {
                Some(slice) => slice,
                None => {
                    let msg = format!(
                        "a bytes-like object is required, not '{}'",
                        type_name(_py, chars)
                    );
                    return raise_exception::<_>(_py, "TypeError", &msg);
                }
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
                let needle_bytes = match bytes_like_slice(needle_ptr) {
                    Some(slice) => slice,
                    None => {
                        let msg = format!(
                            "a bytes-like object is required, not '{}'",
                            type_name(_py, needle)
                        );
                        return raise_exception::<_>(_py, "TypeError", &msg);
                    }
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
                let needle_bytes = match bytes_like_slice(needle_ptr) {
                    Some(slice) => slice,
                    None => {
                        let msg = format!(
                            "a bytes-like object is required, not '{}'",
                            type_name(_py, needle)
                        );
                        return raise_exception::<_>(_py, "TypeError", &msg);
                    }
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

fn bytes_decode_impl(
    _py: &PyToken<'_>,
    hay_bits: u64,
    encoding_bits: u64,
    errors_bits: u64,
    type_id: u32,
) -> u64 {
    let hay = obj_from_bits(hay_bits);
    let Some(hay_ptr) = hay.as_ptr() else {
        return MoltObject::none().bits();
    };
    unsafe {
        if object_type_id(hay_ptr) != type_id {
            return MoltObject::none().bits();
        }
        let encoding = match parse_codec_arg(_py, encoding_bits, "decode", "encoding", "utf-8") {
            Some(val) => val,
            None => return MoltObject::none().bits(),
        };
        let errors = match parse_codec_arg(_py, errors_bits, "decode", "errors", "strict") {
            Some(val) => val,
            None => return MoltObject::none().bits(),
        };
        let bytes = bytes_like_slice(hay_ptr).unwrap_or(&[]);

        match decode_bytes_text(&encoding, &errors, bytes) {
            Ok((text_bytes, _label)) => {
                let ptr = alloc_string(_py, &text_bytes);
                if ptr.is_null() {
                    return MoltObject::none().bits();
                }
                MoltObject::from_ptr(ptr).bits()
            }
            Err(DecodeTextError::UnknownEncoding(name)) => {
                let msg = format!("unknown encoding: {name}");
                raise_exception::<_>(_py, "LookupError", &msg)
            }
            Err(DecodeTextError::UnknownErrorHandler(name)) => {
                let msg = format!("unknown error handler name '{name}'");
                raise_exception::<_>(_py, "LookupError", &msg)
            }
            Err(DecodeTextError::Failure(DecodeFailure::Byte { pos, byte, message }, label)) => {
                let msg = decode_error_byte(&label, byte, pos, message);
                raise_exception::<_>(_py, "UnicodeDecodeError", &msg)
            }
            Err(DecodeTextError::Failure(
                DecodeFailure::Range {
                    start,
                    end,
                    message,
                },
                label,
            )) => {
                let msg = decode_error_range(&label, start, end, message);
                raise_exception::<_>(_py, "UnicodeDecodeError", &msg)
            }
            Err(DecodeTextError::Failure(DecodeFailure::UnknownErrorHandler(name), _label)) => {
                let msg = format!("unknown error handler name '{name}'");
                raise_exception::<_>(_py, "LookupError", &msg)
            }
        }
    }
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_bytes_decode(hay_bits: u64, encoding_bits: u64, errors_bits: u64) -> u64 {
    crate::with_gil_entry_nopanic!(_py, {
        bytes_decode_impl(_py, hay_bits, encoding_bits, errors_bits, TYPE_ID_BYTES)
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_bytearray_decode(
    hay_bits: u64,
    encoding_bits: u64,
    errors_bits: u64,
) -> u64 {
    crate::with_gil_entry_nopanic!(_py, {
        bytes_decode_impl(_py, hay_bits, encoding_bits, errors_bits, TYPE_ID_BYTEARRAY)
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_bytes_replace(
    hay_bits: u64,
    needle_bits: u64,
    replacement_bits: u64,
    count_bits: u64,
) -> u64 {
    crate::with_gil_entry_nopanic!(_py, {
        let hay = obj_from_bits(hay_bits);
        let needle = obj_from_bits(needle_bits);
        let replacement = obj_from_bits(replacement_bits);
        let count_err = format!(
            "'{}' object cannot be interpreted as an integer",
            type_name(_py, obj_from_bits(count_bits))
        );
        let count = index_i64_from_obj(_py, count_bits, &count_err);
        if exception_pending(_py) {
            return MoltObject::none().bits();
        }
        if let Some(hay_ptr) = hay.as_ptr() {
            unsafe {
                if object_type_id(hay_ptr) != TYPE_ID_BYTES {
                    return MoltObject::none().bits();
                }
                let hay_bytes = bytes_like_slice(hay_ptr).unwrap_or(&[]);
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
                let needle_bytes = match bytes_like_slice(needle_ptr) {
                    Some(slice) => slice,
                    None => {
                        let msg = format!(
                            "a bytes-like object is required, not '{}'",
                            type_name(_py, needle)
                        );
                        return raise_exception::<_>(_py, "TypeError", &msg);
                    }
                };
                let repl_ptr = match replacement.as_ptr() {
                    Some(ptr) => ptr,
                    None => {
                        let msg = format!(
                            "a bytes-like object is required, not '{}'",
                            type_name(_py, replacement)
                        );
                        return raise_exception::<_>(_py, "TypeError", &msg);
                    }
                };
                let repl_bytes = match bytes_like_slice(repl_ptr) {
                    Some(slice) => slice,
                    None => {
                        let msg = format!(
                            "a bytes-like object is required, not '{}'",
                            type_name(_py, replacement)
                        );
                        return raise_exception::<_>(_py, "TypeError", &msg);
                    }
                };
                let out = if count < 0 {
                    match replace_bytes_impl(hay_bytes, needle_bytes, repl_bytes) {
                        Some(out) => out,
                        None => return MoltObject::none().bits(),
                    }
                } else {
                    replace_bytes_impl_limit(hay_bytes, needle_bytes, repl_bytes, count as usize)
                };
                let ptr = alloc_bytes(_py, &out);
                if ptr.is_null() {
                    return MoltObject::none().bits();
                }
                return MoltObject::from_ptr(ptr).bits();
            }
        }
        MoltObject::none().bits()
    })
}

fn bytes_hex_sep_from_bits(_py: &PyToken<'_>, sep_bits: u64) -> Result<Option<String>, u64> {
    if sep_bits == 0 || obj_from_bits(sep_bits).is_none() {
        return Ok(None);
    }
    let sep_obj = obj_from_bits(sep_bits);
    let Some(sep_ptr) = sep_obj.as_ptr() else {
        return Err(raise_exception::<_>(
            _py,
            "TypeError",
            "sep must be str or bytes",
        ));
    };
    unsafe {
        let type_id = object_type_id(sep_ptr);
        if type_id == TYPE_ID_STRING {
            let bytes = std::slice::from_raw_parts(string_bytes(sep_ptr), string_len(sep_ptr));
            let Ok(sep_str) = std::str::from_utf8(bytes) else {
                return Err(raise_exception::<_>(
                    _py,
                    "TypeError",
                    "sep must be str or bytes",
                ));
            };
            if sep_str.chars().count() != 1 {
                return Err(raise_exception::<_>(
                    _py,
                    "ValueError",
                    "sep must be length 1",
                ));
            }
            return Ok(Some(sep_str.to_string()));
        }
        if type_id == TYPE_ID_BYTES || type_id == TYPE_ID_BYTEARRAY {
            let bytes = bytes_like_slice(sep_ptr).unwrap_or(&[]);
            if bytes.len() != 1 {
                return Err(raise_exception::<_>(
                    _py,
                    "ValueError",
                    "sep must be length 1",
                ));
            }
            let ch = char::from(bytes[0]);
            return Ok(Some(ch.to_string()));
        }
    }
    Err(raise_exception::<_>(
        _py,
        "TypeError",
        "sep must be str or bytes",
    ))
}

fn bytes_hex_string(bytes: &[u8], sep: Option<&str>, bytes_per_sep: i64) -> String {
    const HEX: &[u8; 16] = b"0123456789abcdef";
    if bytes.is_empty() {
        return String::new();
    }
    let hex_len = bytes.len() * 2;
    let Some(sep_str) = sep else {
        // SIMD fast path for no-separator hex encoding
        let mut raw: Vec<u8> = Vec::with_capacity(hex_len);
        let mut i = 0usize;
        #[cfg(target_arch = "aarch64")]
        {
            if bytes.len() >= 16 && std::arch::is_aarch64_feature_detected!("neon") {
                unsafe {
                    use std::arch::aarch64::*;
                    let hex_lut = vld1q_u8(b"0123456789abcdef".as_ptr());
                    let mask_lo = vdupq_n_u8(0x0F);
                    while i + 16 <= bytes.len() {
                        let chunk = vld1q_u8(bytes.as_ptr().add(i));
                        let hi_nibbles = vshrq_n_u8(chunk, 4);
                        let lo_nibbles = vandq_u8(chunk, mask_lo);
                        let hi_hex = vqtbl1q_u8(hex_lut, hi_nibbles);
                        let lo_hex = vqtbl1q_u8(hex_lut, lo_nibbles);
                        let zipped_lo = vzip1q_u8(hi_hex, lo_hex);
                        let zipped_hi = vzip2q_u8(hi_hex, lo_hex);
                        let len = raw.len();
                        raw.set_len(len + 32);
                        vst1q_u8(raw.as_mut_ptr().add(len), zipped_lo);
                        vst1q_u8(raw.as_mut_ptr().add(len + 16), zipped_hi);
                        i += 16;
                    }
                }
            }
        }
        #[cfg(target_arch = "x86_64")]
        {
            if bytes.len() >= 16 && std::arch::is_x86_feature_detected!("ssse3") {
                unsafe {
                    use std::arch::x86_64::*;
                    let mask_lo = _mm_set1_epi8(0x0F);
                    let hex_lut = _mm_setr_epi8(
                        b'0' as i8, b'1' as i8, b'2' as i8, b'3' as i8, b'4' as i8, b'5' as i8,
                        b'6' as i8, b'7' as i8, b'8' as i8, b'9' as i8, b'a' as i8, b'b' as i8,
                        b'c' as i8, b'd' as i8, b'e' as i8, b'f' as i8,
                    );
                    while i + 16 <= bytes.len() {
                        let chunk = _mm_loadu_si128(bytes.as_ptr().add(i) as *const __m128i);
                        let hi_nibbles = _mm_and_si128(_mm_srli_epi16(chunk, 4), mask_lo);
                        let lo_nibbles = _mm_and_si128(chunk, mask_lo);
                        let hi_hex = _mm_shuffle_epi8(hex_lut, hi_nibbles);
                        let lo_hex = _mm_shuffle_epi8(hex_lut, lo_nibbles);
                        let interleaved_lo = _mm_unpacklo_epi8(hi_hex, lo_hex);
                        let interleaved_hi = _mm_unpackhi_epi8(hi_hex, lo_hex);
                        let len = raw.len();
                        raw.set_len(len + 32);
                        _mm_storeu_si128(raw.as_mut_ptr().add(len) as *mut __m128i, interleaved_lo);
                        _mm_storeu_si128(
                            raw.as_mut_ptr().add(len + 16) as *mut __m128i,
                            interleaved_hi,
                        );
                        i += 16;
                    }
                }
            }
        }
        #[cfg(target_arch = "wasm32")]
        {
            if cfg!(target_feature = "simd128") && bytes.len() >= 16 {
                unsafe {
                    use std::arch::wasm32::*;
                    let mask_lo = u8x16_splat(0x0F);
                    let hex_lut = v128_load(b"0123456789abcdef".as_ptr() as *const v128);
                    while i + 16 <= bytes.len() {
                        let chunk = v128_load(bytes.as_ptr().add(i) as *const v128);
                        let hi_nibbles = v128_and(u16x8_shr(chunk, 4), mask_lo);
                        let lo_nibbles = v128_and(chunk, mask_lo);
                        let hi_hex = i8x16_swizzle(hex_lut, hi_nibbles);
                        let lo_hex = i8x16_swizzle(hex_lut, lo_nibbles);
                        // Interleave hi and lo hex chars
                        let interleaved_lo = i8x16_shuffle::<
                            0,
                            16,
                            1,
                            17,
                            2,
                            18,
                            3,
                            19,
                            4,
                            20,
                            5,
                            21,
                            6,
                            22,
                            7,
                            23,
                        >(hi_hex, lo_hex);
                        let interleaved_hi = i8x16_shuffle::<
                            8,
                            24,
                            9,
                            25,
                            10,
                            26,
                            11,
                            27,
                            12,
                            28,
                            13,
                            29,
                            14,
                            30,
                            15,
                            31,
                        >(hi_hex, lo_hex);
                        let len = raw.len();
                        raw.set_len(len + 32);
                        v128_store(raw.as_mut_ptr().add(len) as *mut v128, interleaved_lo);
                        v128_store(raw.as_mut_ptr().add(len + 16) as *mut v128, interleaved_hi);
                        i += 16;
                    }
                }
            }
        }
        // Scalar tail
        for &b in &bytes[i..] {
            raw.push(HEX[(b >> 4) as usize]);
            raw.push(HEX[(b & 0xF) as usize]);
        }
        // SAFETY: all bytes are valid ASCII hex characters
        return unsafe { String::from_utf8_unchecked(raw) };
    };
    let group = bytes_per_sep.unsigned_abs() as usize;
    let separators = if group == 0 {
        0
    } else {
        (bytes.len().saturating_sub(1)) / group
    };
    let mut out = String::with_capacity(hex_len + separators * sep_str.len());
    if bytes_per_sep > 0 {
        for (idx, &b) in bytes.iter().enumerate() {
            if idx > 0 && idx % group == 0 {
                out.push_str(sep_str);
            }
            out.push(char::from(HEX[(b >> 4) as usize]));
            out.push(char::from(HEX[(b & 0xF) as usize]));
        }
    } else {
        let mut first_group = bytes.len() % group;
        if first_group == 0 {
            first_group = group;
        }
        for (idx, &b) in bytes.iter().enumerate() {
            if idx == first_group
                || (idx > first_group && (idx - first_group).is_multiple_of(group))
            {
                out.push_str(sep_str);
            }
            out.push(char::from(HEX[(b >> 4) as usize]));
            out.push(char::from(HEX[(b & 0xF) as usize]));
        }
    }
    out
}

pub(crate) fn bytes_hex_from_bits(
    _py: &PyToken<'_>,
    bytes: &[u8],
    sep_bits: u64,
    bytes_per_sep_bits: u64,
) -> u64 {
    let sep_opt = if sep_bits == missing_bits(_py) {
        None
    } else {
        match bytes_hex_sep_from_bits(_py, sep_bits) {
            Ok(sep) => sep,
            Err(err_bits) => return err_bits,
        }
    };
    let bytes_per_sep = if bytes_per_sep_bits == missing_bits(_py) {
        1
    } else {
        index_i64_from_obj(_py, bytes_per_sep_bits, "bytes_per_sep must be int")
    };
    if exception_pending(_py) {
        return MoltObject::none().bits();
    }
    if bytes_per_sep == 0 {
        return raise_exception::<_>(_py, "ValueError", "bytes_per_sep must not be 0");
    }
    let text = bytes_hex_string(bytes, sep_opt.as_deref(), bytes_per_sep);
    let ptr = alloc_string(_py, text.as_bytes());
    if ptr.is_null() {
        return MoltObject::none().bits();
    }
    MoltObject::from_ptr(ptr).bits()
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_bytes_upper(hay_bits: u64) -> u64 {
    crate::with_gil_entry_nopanic!(_py, {
        let hay = obj_from_bits(hay_bits);
        let Some(hay_ptr) = hay.as_ptr() else {
            return MoltObject::none().bits();
        };
        unsafe {
            if object_type_id(hay_ptr) != TYPE_ID_BYTES {
                return MoltObject::none().bits();
            }
            let hay_bytes = bytes_like_slice(hay_ptr).unwrap_or(&[]);
            let out = bytes_ascii_upper(hay_bytes);
            let ptr = alloc_bytes(_py, &out);
            if ptr.is_null() {
                return MoltObject::none().bits();
            }
            MoltObject::from_ptr(ptr).bits()
        }
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_bytes_lower(hay_bits: u64) -> u64 {
    crate::with_gil_entry_nopanic!(_py, {
        let hay = obj_from_bits(hay_bits);
        let Some(hay_ptr) = hay.as_ptr() else {
            return MoltObject::none().bits();
        };
        unsafe {
            if object_type_id(hay_ptr) != TYPE_ID_BYTES {
                return MoltObject::none().bits();
            }
            let hay_bytes = bytes_like_slice(hay_ptr).unwrap_or(&[]);
            let out = bytes_ascii_lower(hay_bytes);
            let ptr = alloc_bytes(_py, &out);
            if ptr.is_null() {
                return MoltObject::none().bits();
            }
            MoltObject::from_ptr(ptr).bits()
        }
    })
}

#[inline]
pub(super) fn bytes_ascii_space(b: u8) -> bool {
    matches!(b, b' ' | b'\t' | b'\n' | b'\r' | 0x0b | 0x0c)
}

/// SIMD-accelerated check: are ALL bytes ASCII whitespace?
/// Uses NEON/SSE2 to test 16 bytes at a time against the 6 ASCII
/// whitespace characters (' ', '\t', '\n', '\r', 0x0b, 0x0c).
#[inline]
fn alloc_bytes_like_for_type(_py: &PyToken<'_>, type_id: u32, bytes: &[u8]) -> *mut u8 {
    if type_id == TYPE_ID_BYTEARRAY {
        alloc_bytearray(_py, bytes)
    } else {
        alloc_bytes(_py, bytes)
    }
}

fn bytes_like_ascii_transform<F>(_py: &PyToken<'_>, hay_bits: u64, type_id: u32, f: F) -> u64
where
    F: FnOnce(&[u8]) -> Vec<u8>,
{
    let hay = obj_from_bits(hay_bits);
    let Some(hay_ptr) = hay.as_ptr() else {
        return MoltObject::none().bits();
    };
    unsafe {
        if object_type_id(hay_ptr) != type_id {
            return MoltObject::none().bits();
        }
        let hay_bytes = bytes_like_slice(hay_ptr).unwrap_or(&[]);
        let out = f(hay_bytes);
        let ptr = alloc_bytes_like_for_type(_py, type_id, &out);
        if ptr.is_null() {
            return MoltObject::none().bits();
        }
        MoltObject::from_ptr(ptr).bits()
    }
}

fn bytes_like_ascii_predicate<F>(_py: &PyToken<'_>, hay_bits: u64, type_id: u32, f: F) -> u64
where
    F: FnOnce(&[u8]) -> bool,
{
    let hay = obj_from_bits(hay_bits);
    let Some(hay_ptr) = hay.as_ptr() else {
        return MoltObject::none().bits();
    };
    unsafe {
        if object_type_id(hay_ptr) != type_id {
            return MoltObject::none().bits();
        }
        let hay_bytes = bytes_like_slice(hay_ptr).unwrap_or(&[]);
        MoltObject::from_bool(f(hay_bytes)).bits()
    }
}

fn bytes_ascii_islower(bytes: &[u8]) -> bool {
    if bytes.is_empty() {
        return false;
    }
    // SIMD fast path: check if any byte is in [A-Z] range (instant false) in bulk
    #[cfg(target_arch = "aarch64")]
    {
        if bytes.len() >= 16 && std::arch::is_aarch64_feature_detected!("neon") {
            unsafe {
                use std::arch::aarch64::*;
                let upper_a = vdupq_n_u8(b'A');
                let upper_z = vdupq_n_u8(b'Z');
                let lower_a = vdupq_n_u8(b'a');
                let lower_z = vdupq_n_u8(b'z');
                let mut has_lower_vec = vdupq_n_u8(0);
                let mut i = 0usize;
                while i + 16 <= bytes.len() {
                    let v = vld1q_u8(bytes.as_ptr().add(i));
                    let is_upper = vandq_u8(vcgeq_u8(v, upper_a), vcleq_u8(v, upper_z));
                    if vmaxvq_u8(is_upper) != 0 {
                        return false;
                    }
                    let is_lower = vandq_u8(vcgeq_u8(v, lower_a), vcleq_u8(v, lower_z));
                    has_lower_vec = vorrq_u8(has_lower_vec, is_lower);
                    i += 16;
                }
                let has_lower_simd = vmaxvq_u8(has_lower_vec) != 0;
                // Scalar tail
                let mut has_lower = has_lower_simd;
                for &b in &bytes[i..] {
                    if b.is_ascii_uppercase() {
                        return false;
                    }
                    if b.is_ascii_lowercase() {
                        has_lower = true;
                    }
                }
                return has_lower;
            }
        }
    }
    #[cfg(target_arch = "x86_64")]
    {
        if bytes.len() >= 16 && std::arch::is_x86_feature_detected!("sse2") {
            unsafe {
                use std::arch::x86_64::*;
                let mut has_lower_any = false;
                let mut i = 0usize;
                while i + 16 <= bytes.len() {
                    let v = _mm_loadu_si128(bytes.as_ptr().add(i) as *const __m128i);
                    // Check for uppercase [A-Z]
                    let ge_a = _mm_cmpgt_epi8(v, _mm_set1_epi8(b'A' as i8 - 1));
                    let le_z = _mm_cmpgt_epi8(_mm_set1_epi8(b'Z' as i8 + 1), v);
                    let is_upper = _mm_and_si128(ge_a, le_z);
                    if _mm_movemask_epi8(is_upper) != 0 {
                        return false;
                    }
                    // Check for lowercase [a-z]
                    let ge_la = _mm_cmpgt_epi8(v, _mm_set1_epi8(b'a' as i8 - 1));
                    let le_lz = _mm_cmpgt_epi8(_mm_set1_epi8(b'z' as i8 + 1), v);
                    let is_lower = _mm_and_si128(ge_la, le_lz);
                    if _mm_movemask_epi8(is_lower) != 0 {
                        has_lower_any = true;
                    }
                    i += 16;
                }
                for &b in &bytes[i..] {
                    if b.is_ascii_uppercase() {
                        return false;
                    }
                    if b.is_ascii_lowercase() {
                        has_lower_any = true;
                    }
                }
                return has_lower_any;
            }
        }
    }
    let mut has_lower = false;
    for &b in bytes {
        if b.is_ascii_uppercase() {
            return false;
        }
        if b.is_ascii_lowercase() {
            has_lower = true;
        }
    }
    has_lower
}

fn bytes_ascii_isupper(bytes: &[u8]) -> bool {
    if bytes.is_empty() {
        return false;
    }
    // SIMD fast path: check if any byte is in [a-z] range (instant false) in bulk
    #[cfg(target_arch = "aarch64")]
    {
        if bytes.len() >= 16 && std::arch::is_aarch64_feature_detected!("neon") {
            unsafe {
                use std::arch::aarch64::*;
                let lower_a = vdupq_n_u8(b'a');
                let lower_z = vdupq_n_u8(b'z');
                let upper_a = vdupq_n_u8(b'A');
                let upper_z = vdupq_n_u8(b'Z');
                let mut has_upper_vec = vdupq_n_u8(0);
                let mut i = 0usize;
                while i + 16 <= bytes.len() {
                    let v = vld1q_u8(bytes.as_ptr().add(i));
                    let is_lower = vandq_u8(vcgeq_u8(v, lower_a), vcleq_u8(v, lower_z));
                    if vmaxvq_u8(is_lower) != 0 {
                        return false;
                    }
                    let is_upper = vandq_u8(vcgeq_u8(v, upper_a), vcleq_u8(v, upper_z));
                    has_upper_vec = vorrq_u8(has_upper_vec, is_upper);
                    i += 16;
                }
                let has_upper_simd = vmaxvq_u8(has_upper_vec) != 0;
                let mut has_upper = has_upper_simd;
                for &b in &bytes[i..] {
                    if b.is_ascii_lowercase() {
                        return false;
                    }
                    if b.is_ascii_uppercase() {
                        has_upper = true;
                    }
                }
                return has_upper;
            }
        }
    }
    #[cfg(target_arch = "x86_64")]
    {
        if bytes.len() >= 16 && std::arch::is_x86_feature_detected!("sse2") {
            unsafe {
                use std::arch::x86_64::*;
                let mut has_upper_any = false;
                let mut i = 0usize;
                while i + 16 <= bytes.len() {
                    let v = _mm_loadu_si128(bytes.as_ptr().add(i) as *const __m128i);
                    let ge_la = _mm_cmpgt_epi8(v, _mm_set1_epi8(b'a' as i8 - 1));
                    let le_lz = _mm_cmpgt_epi8(_mm_set1_epi8(b'z' as i8 + 1), v);
                    let is_lower = _mm_and_si128(ge_la, le_lz);
                    if _mm_movemask_epi8(is_lower) != 0 {
                        return false;
                    }
                    let ge_a = _mm_cmpgt_epi8(v, _mm_set1_epi8(b'A' as i8 - 1));
                    let le_z = _mm_cmpgt_epi8(_mm_set1_epi8(b'Z' as i8 + 1), v);
                    let is_upper = _mm_and_si128(ge_a, le_z);
                    if _mm_movemask_epi8(is_upper) != 0 {
                        has_upper_any = true;
                    }
                    i += 16;
                }
                for &b in &bytes[i..] {
                    if b.is_ascii_lowercase() {
                        return false;
                    }
                    if b.is_ascii_uppercase() {
                        has_upper_any = true;
                    }
                }
                return has_upper_any;
            }
        }
    }
    let mut has_upper = false;
    for &b in bytes {
        if b.is_ascii_lowercase() {
            return false;
        }
        if b.is_ascii_uppercase() {
            has_upper = true;
        }
    }
    has_upper
}

fn bytes_ascii_istitle(bytes: &[u8]) -> bool {
    if bytes.is_empty() {
        return false;
    }
    let mut cased = false;
    let mut prev_cased = false;
    for &b in bytes {
        if b.is_ascii_uppercase() {
            if prev_cased {
                return false;
            }
            cased = true;
            prev_cased = true;
        } else if b.is_ascii_lowercase() {
            if !prev_cased {
                return false;
            }
            cased = true;
            prev_cased = true;
        } else {
            prev_cased = false;
        }
    }
    cased
}

fn bytes_fill_byte_from_bits(_py: &PyToken<'_>, fill_bits: u64, method: &str) -> Option<u8> {
    if fill_bits == missing_bits(_py) {
        return Some(b' ');
    }
    let fill_obj = obj_from_bits(fill_bits);
    let Some(fill_ptr) = fill_obj.as_ptr() else {
        let msg = format!(
            "{method}() argument 2 must be a byte string of length 1, not {}",
            type_name(_py, fill_obj)
        );
        return raise_exception::<_>(_py, "TypeError", &msg);
    };
    unsafe {
        let Some(fill_slice) = bytes_like_slice(fill_ptr) else {
            let msg = format!(
                "{method}() argument 2 must be a byte string of length 1, not {}",
                type_name(_py, fill_obj)
            );
            return raise_exception::<_>(_py, "TypeError", &msg);
        };
        if fill_slice.len() != 1 {
            let msg = format!(
                "{method}(): argument 2 must be a byte string of length 1, not a bytes object of length {}",
                fill_slice.len()
            );
            return raise_exception::<_>(_py, "TypeError", &msg);
        }
        Some(fill_slice[0])
    }
}

enum BytesAlignKind {
    Center,
    Left,
    Right,
}

fn bytes_align_impl(
    _py: &PyToken<'_>,
    hay_bits: u64,
    width_bits: u64,
    fill_bits: u64,
    type_id: u32,
    kind: BytesAlignKind,
    method_name: &str,
) -> u64 {
    let width = index_i64_from_obj(_py, width_bits, "an integer is required");
    if exception_pending(_py) {
        return MoltObject::none().bits();
    }
    let Some(fill_byte) = bytes_fill_byte_from_bits(_py, fill_bits, method_name) else {
        return MoltObject::none().bits();
    };
    let hay = obj_from_bits(hay_bits);
    let Some(hay_ptr) = hay.as_ptr() else {
        return MoltObject::none().bits();
    };
    unsafe {
        if object_type_id(hay_ptr) != type_id {
            return MoltObject::none().bits();
        }
        let hay_bytes = bytes_like_slice(hay_ptr).unwrap_or(&[]);
        let len = hay_bytes.len() as i64;
        if width <= len {
            let ptr = alloc_bytes_like_for_type(_py, type_id, hay_bytes);
            if ptr.is_null() {
                return MoltObject::none().bits();
            }
            return MoltObject::from_ptr(ptr).bits();
        }
        let total = width as usize;
        let pad = total.saturating_sub(hay_bytes.len());
        let (left_pad, right_pad) = match kind {
            BytesAlignKind::Center => (pad / 2, pad - (pad / 2)),
            BytesAlignKind::Left => (0, pad),
            BytesAlignKind::Right => (pad, 0),
        };
        let mut out = Vec::with_capacity(total);
        out.extend(std::iter::repeat_n(fill_byte, left_pad));
        out.extend_from_slice(hay_bytes);
        out.extend(std::iter::repeat_n(fill_byte, right_pad));
        let ptr = alloc_bytes_like_for_type(_py, type_id, &out);
        if ptr.is_null() {
            return MoltObject::none().bits();
        }
        MoltObject::from_ptr(ptr).bits()
    }
}

fn bytes_zfill_impl(_py: &PyToken<'_>, hay_bits: u64, width_bits: u64, type_id: u32) -> u64 {
    let width = index_i64_from_obj(_py, width_bits, "an integer is required");
    if exception_pending(_py) {
        return MoltObject::none().bits();
    }
    let hay = obj_from_bits(hay_bits);
    let Some(hay_ptr) = hay.as_ptr() else {
        return MoltObject::none().bits();
    };
    unsafe {
        if object_type_id(hay_ptr) != type_id {
            return MoltObject::none().bits();
        }
        let hay_bytes = bytes_like_slice(hay_ptr).unwrap_or(&[]);
        let len = hay_bytes.len() as i64;
        if width <= len {
            let ptr = alloc_bytes_like_for_type(_py, type_id, hay_bytes);
            if ptr.is_null() {
                return MoltObject::none().bits();
            }
            return MoltObject::from_ptr(ptr).bits();
        }
        let pad = (width - len) as usize;
        let mut out = Vec::with_capacity(width as usize);
        if let Some(first) = hay_bytes.first().copied() {
            if first == b'+' || first == b'-' {
                out.push(first);
                out.extend(std::iter::repeat_n(b'0', pad));
                out.extend_from_slice(&hay_bytes[1..]);
            } else {
                out.extend(std::iter::repeat_n(b'0', pad));
                out.extend_from_slice(hay_bytes);
            }
        } else {
            out.extend(std::iter::repeat_n(b'0', pad));
        }
        let ptr = alloc_bytes_like_for_type(_py, type_id, &out);
        if ptr.is_null() {
            return MoltObject::none().bits();
        }
        MoltObject::from_ptr(ptr).bits()
    }
}

fn bytes_expandtabs_ascii(bytes: &[u8], tabsize: i64) -> Vec<u8> {
    let tab = tabsize.max(0) as usize;
    let mut out = Vec::with_capacity(bytes.len());
    let mut column = 0usize;
    for &b in bytes {
        if b == b'\t' {
            let spaces = if tab == 0 { 0 } else { tab - (column % tab) };
            out.extend(std::iter::repeat_n(b' ', spaces));
            column = column.saturating_add(spaces);
        } else {
            out.push(b);
            if b == b'\n' || b == b'\r' {
                column = 0;
            } else {
                column = column.saturating_add(1);
            }
        }
    }
    out
}

fn bytes_expandtabs_impl(_py: &PyToken<'_>, hay_bits: u64, tabsize_bits: u64, type_id: u32) -> u64 {
    let tabsize = if tabsize_bits == missing_bits(_py) {
        8
    } else {
        index_i64_from_obj(_py, tabsize_bits, "an integer is required")
    };
    if exception_pending(_py) {
        return MoltObject::none().bits();
    }
    bytes_like_ascii_transform(_py, hay_bits, type_id, |bytes| {
        bytes_expandtabs_ascii(bytes, tabsize)
    })
}

fn bytes_remove_affix_impl(
    _py: &PyToken<'_>,
    hay_bits: u64,
    affix_bits: u64,
    type_id: u32,
    suffix: bool,
) -> u64 {
    let hay = obj_from_bits(hay_bits);
    let affix = obj_from_bits(affix_bits);
    let Some(hay_ptr) = hay.as_ptr() else {
        return MoltObject::none().bits();
    };
    let Some(affix_ptr) = affix.as_ptr() else {
        let msg = format!(
            "a bytes-like object is required, not '{}'",
            type_name(_py, affix)
        );
        return raise_exception::<_>(_py, "TypeError", &msg);
    };
    unsafe {
        if object_type_id(hay_ptr) != type_id {
            return MoltObject::none().bits();
        }
        let Some(affix_bytes) = bytes_like_slice(affix_ptr) else {
            let msg = format!(
                "a bytes-like object is required, not '{}'",
                type_name(_py, affix)
            );
            return raise_exception::<_>(_py, "TypeError", &msg);
        };
        let hay_bytes = bytes_like_slice(hay_ptr).unwrap_or(&[]);
        let out = if suffix {
            if hay_bytes.ends_with(affix_bytes) {
                &hay_bytes[..hay_bytes.len().saturating_sub(affix_bytes.len())]
            } else {
                hay_bytes
            }
        } else if hay_bytes.starts_with(affix_bytes) {
            &hay_bytes[affix_bytes.len()..]
        } else {
            hay_bytes
        };
        let ptr = alloc_bytes_like_for_type(_py, type_id, out);
        if ptr.is_null() {
            return MoltObject::none().bits();
        }
        MoltObject::from_ptr(ptr).bits()
    }
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_bytes_capitalize(hay_bits: u64) -> u64 {
    crate::with_gil_entry_nopanic!(_py, {
        bytes_like_ascii_transform(_py, hay_bits, TYPE_ID_BYTES, bytes_ascii_capitalize)
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_bytearray_capitalize(hay_bits: u64) -> u64 {
    crate::with_gil_entry_nopanic!(_py, {
        bytes_like_ascii_transform(_py, hay_bits, TYPE_ID_BYTEARRAY, bytes_ascii_capitalize)
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_bytes_swapcase(hay_bits: u64) -> u64 {
    crate::with_gil_entry_nopanic!(_py, {
        bytes_like_ascii_transform(_py, hay_bits, TYPE_ID_BYTES, bytes_ascii_swapcase)
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_bytearray_swapcase(hay_bits: u64) -> u64 {
    crate::with_gil_entry_nopanic!(_py, {
        bytes_like_ascii_transform(_py, hay_bits, TYPE_ID_BYTEARRAY, bytes_ascii_swapcase)
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_bytes_title(hay_bits: u64) -> u64 {
    crate::with_gil_entry_nopanic!(_py, {
        bytes_like_ascii_transform(_py, hay_bits, TYPE_ID_BYTES, bytes_ascii_title)
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_bytearray_title(hay_bits: u64) -> u64 {
    crate::with_gil_entry_nopanic!(_py, {
        bytes_like_ascii_transform(_py, hay_bits, TYPE_ID_BYTEARRAY, bytes_ascii_title)
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_bytes_isalpha(hay_bits: u64) -> u64 {
    crate::with_gil_entry_nopanic!(_py, {
        bytes_like_ascii_predicate(_py, hay_bits, TYPE_ID_BYTES, simd_is_all_ascii_alpha)
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_bytearray_isalpha(hay_bits: u64) -> u64 {
    crate::with_gil_entry_nopanic!(_py, {
        bytes_like_ascii_predicate(_py, hay_bits, TYPE_ID_BYTEARRAY, simd_is_all_ascii_alpha)
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_bytes_isalnum(hay_bits: u64) -> u64 {
    crate::with_gil_entry_nopanic!(_py, {
        bytes_like_ascii_predicate(_py, hay_bits, TYPE_ID_BYTES, simd_is_all_ascii_alnum)
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_bytearray_isalnum(hay_bits: u64) -> u64 {
    crate::with_gil_entry_nopanic!(_py, {
        bytes_like_ascii_predicate(_py, hay_bits, TYPE_ID_BYTEARRAY, simd_is_all_ascii_alnum)
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_bytes_isdigit(hay_bits: u64) -> u64 {
    crate::with_gil_entry_nopanic!(_py, {
        bytes_like_ascii_predicate(_py, hay_bits, TYPE_ID_BYTES, simd_is_all_ascii_digit)
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_bytearray_isdigit(hay_bits: u64) -> u64 {
    crate::with_gil_entry_nopanic!(_py, {
        bytes_like_ascii_predicate(_py, hay_bits, TYPE_ID_BYTEARRAY, simd_is_all_ascii_digit)
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_bytes_isspace(hay_bits: u64) -> u64 {
    crate::with_gil_entry_nopanic!(_py, {
        bytes_like_ascii_predicate(_py, hay_bits, TYPE_ID_BYTES, simd_is_all_ascii_whitespace)
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_bytearray_isspace(hay_bits: u64) -> u64 {
    crate::with_gil_entry_nopanic!(_py, {
        bytes_like_ascii_predicate(
            _py,
            hay_bits,
            TYPE_ID_BYTEARRAY,
            simd_is_all_ascii_whitespace,
        )
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_bytes_islower(hay_bits: u64) -> u64 {
    crate::with_gil_entry_nopanic!(_py, {
        bytes_like_ascii_predicate(_py, hay_bits, TYPE_ID_BYTES, bytes_ascii_islower)
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_bytearray_islower(hay_bits: u64) -> u64 {
    crate::with_gil_entry_nopanic!(_py, {
        bytes_like_ascii_predicate(_py, hay_bits, TYPE_ID_BYTEARRAY, bytes_ascii_islower)
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_bytes_isupper(hay_bits: u64) -> u64 {
    crate::with_gil_entry_nopanic!(_py, {
        bytes_like_ascii_predicate(_py, hay_bits, TYPE_ID_BYTES, bytes_ascii_isupper)
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_bytearray_isupper(hay_bits: u64) -> u64 {
    crate::with_gil_entry_nopanic!(_py, {
        bytes_like_ascii_predicate(_py, hay_bits, TYPE_ID_BYTEARRAY, bytes_ascii_isupper)
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_bytes_istitle(hay_bits: u64) -> u64 {
    crate::with_gil_entry_nopanic!(_py, {
        bytes_like_ascii_predicate(_py, hay_bits, TYPE_ID_BYTES, bytes_ascii_istitle)
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_bytearray_istitle(hay_bits: u64) -> u64 {
    crate::with_gil_entry_nopanic!(_py, {
        bytes_like_ascii_predicate(_py, hay_bits, TYPE_ID_BYTEARRAY, bytes_ascii_istitle)
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_bytes_isascii(hay_bits: u64) -> u64 {
    crate::with_gil_entry_nopanic!(_py, {
        bytes_like_ascii_predicate(_py, hay_bits, TYPE_ID_BYTES, |bytes| {
            bytes.iter().all(|b| b.is_ascii())
        })
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_bytearray_isascii(hay_bits: u64) -> u64 {
    crate::with_gil_entry_nopanic!(_py, {
        bytes_like_ascii_predicate(_py, hay_bits, TYPE_ID_BYTEARRAY, |bytes| {
            bytes.iter().all(|b| b.is_ascii())
        })
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_bytearray_upper(hay_bits: u64) -> u64 {
    crate::with_gil_entry_nopanic!(_py, {
        bytes_like_ascii_transform(_py, hay_bits, TYPE_ID_BYTEARRAY, bytes_ascii_upper)
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_bytearray_lower(hay_bits: u64) -> u64 {
    crate::with_gil_entry_nopanic!(_py, {
        bytes_like_ascii_transform(_py, hay_bits, TYPE_ID_BYTEARRAY, bytes_ascii_lower)
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_bytes_center(hay_bits: u64, width_bits: u64, fill_bits: u64) -> u64 {
    crate::with_gil_entry_nopanic!(_py, {
        bytes_align_impl(
            _py,
            hay_bits,
            width_bits,
            fill_bits,
            TYPE_ID_BYTES,
            BytesAlignKind::Center,
            "center",
        )
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_bytearray_center(hay_bits: u64, width_bits: u64, fill_bits: u64) -> u64 {
    crate::with_gil_entry_nopanic!(_py, {
        bytes_align_impl(
            _py,
            hay_bits,
            width_bits,
            fill_bits,
            TYPE_ID_BYTEARRAY,
            BytesAlignKind::Center,
            "center",
        )
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_bytes_ljust(hay_bits: u64, width_bits: u64, fill_bits: u64) -> u64 {
    crate::with_gil_entry_nopanic!(_py, {
        bytes_align_impl(
            _py,
            hay_bits,
            width_bits,
            fill_bits,
            TYPE_ID_BYTES,
            BytesAlignKind::Left,
            "ljust",
        )
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_bytearray_ljust(hay_bits: u64, width_bits: u64, fill_bits: u64) -> u64 {
    crate::with_gil_entry_nopanic!(_py, {
        bytes_align_impl(
            _py,
            hay_bits,
            width_bits,
            fill_bits,
            TYPE_ID_BYTEARRAY,
            BytesAlignKind::Left,
            "ljust",
        )
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_bytes_rjust(hay_bits: u64, width_bits: u64, fill_bits: u64) -> u64 {
    crate::with_gil_entry_nopanic!(_py, {
        bytes_align_impl(
            _py,
            hay_bits,
            width_bits,
            fill_bits,
            TYPE_ID_BYTES,
            BytesAlignKind::Right,
            "rjust",
        )
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_bytearray_rjust(hay_bits: u64, width_bits: u64, fill_bits: u64) -> u64 {
    crate::with_gil_entry_nopanic!(_py, {
        bytes_align_impl(
            _py,
            hay_bits,
            width_bits,
            fill_bits,
            TYPE_ID_BYTEARRAY,
            BytesAlignKind::Right,
            "rjust",
        )
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_bytes_zfill(hay_bits: u64, width_bits: u64) -> u64 {
    crate::with_gil_entry_nopanic!(_py, {
        bytes_zfill_impl(_py, hay_bits, width_bits, TYPE_ID_BYTES)
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_bytearray_zfill(hay_bits: u64, width_bits: u64) -> u64 {
    crate::with_gil_entry_nopanic!(_py, {
        bytes_zfill_impl(_py, hay_bits, width_bits, TYPE_ID_BYTEARRAY)
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_bytes_expandtabs(hay_bits: u64, tabsize_bits: u64) -> u64 {
    crate::with_gil_entry_nopanic!(_py, {
        bytes_expandtabs_impl(_py, hay_bits, tabsize_bits, TYPE_ID_BYTES)
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_bytearray_expandtabs(hay_bits: u64, tabsize_bits: u64) -> u64 {
    crate::with_gil_entry_nopanic!(_py, {
        bytes_expandtabs_impl(_py, hay_bits, tabsize_bits, TYPE_ID_BYTEARRAY)
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_bytes_removeprefix(hay_bits: u64, prefix_bits: u64) -> u64 {
    crate::with_gil_entry_nopanic!(_py, {
        bytes_remove_affix_impl(_py, hay_bits, prefix_bits, TYPE_ID_BYTES, false)
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_bytearray_removeprefix(hay_bits: u64, prefix_bits: u64) -> u64 {
    crate::with_gil_entry_nopanic!(_py, {
        bytes_remove_affix_impl(_py, hay_bits, prefix_bits, TYPE_ID_BYTEARRAY, false)
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_bytes_removesuffix(hay_bits: u64, suffix_bits: u64) -> u64 {
    crate::with_gil_entry_nopanic!(_py, {
        bytes_remove_affix_impl(_py, hay_bits, suffix_bits, TYPE_ID_BYTES, true)
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_bytearray_removesuffix(hay_bits: u64, suffix_bits: u64) -> u64 {
    crate::with_gil_entry_nopanic!(_py, {
        bytes_remove_affix_impl(_py, hay_bits, suffix_bits, TYPE_ID_BYTEARRAY, true)
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

fn bytes_translate_impl(
    _py: &PyToken<'_>,
    hay_bytes: &[u8],
    table_bits: u64,
    delete_bits: u64,
) -> Result<Vec<u8>, u64> {
    let table_obj = obj_from_bits(table_bits);
    let table_opt = if table_obj.is_none() {
        None
    } else {
        let table_ptr = match table_obj.as_ptr() {
            Some(ptr) => ptr,
            None => {
                let msg = format!(
                    "a bytes-like object is required, not '{}'",
                    type_name(_py, table_obj)
                );
                return Err(raise_exception::<_>(_py, "TypeError", &msg));
            }
        };
        let table_bytes = match unsafe { bytes_like_slice(table_ptr) } {
            Some(slice) => slice,
            None => {
                let msg = format!(
                    "a bytes-like object is required, not '{}'",
                    type_name(_py, table_obj)
                );
                return Err(raise_exception::<_>(_py, "TypeError", &msg));
            }
        };
        if table_bytes.len() != 256 {
            return Err(raise_exception::<_>(
                _py,
                "ValueError",
                "translation table must be 256 characters long",
            ));
        }
        Some(table_bytes)
    };
    let delete_bytes = if is_missing_bits(_py, delete_bits) {
        &[]
    } else {
        let delete_obj = obj_from_bits(delete_bits);
        let delete_ptr = match delete_obj.as_ptr() {
            Some(ptr) => ptr,
            None => {
                let msg = format!(
                    "a bytes-like object is required, not '{}'",
                    type_name(_py, delete_obj)
                );
                return Err(raise_exception::<_>(_py, "TypeError", &msg));
            }
        };
        match unsafe { bytes_like_slice(delete_ptr) } {
            Some(slice) => slice,
            None => {
                let msg = format!(
                    "a bytes-like object is required, not '{}'",
                    type_name(_py, delete_obj)
                );
                return Err(raise_exception::<_>(_py, "TypeError", &msg));
            }
        }
    };
    if hay_bytes.is_empty() {
        return Ok(Vec::new());
    }
    let mut delete_map = [false; 256];
    for &b in delete_bytes {
        delete_map[b as usize] = true;
    }
    let mut out = Vec::with_capacity(hay_bytes.len());
    match table_opt {
        Some(table) => {
            for &b in hay_bytes {
                if delete_map[b as usize] {
                    continue;
                }
                out.push(table[b as usize]);
            }
        }
        None => {
            for &b in hay_bytes {
                if delete_map[b as usize] {
                    continue;
                }
                out.push(b);
            }
        }
    }
    Ok(out)
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_bytes_translate(hay_bits: u64, table_bits: u64, delete_bits: u64) -> u64 {
    crate::with_gil_entry_nopanic!(_py, {
        let hay = obj_from_bits(hay_bits);
        let Some(hay_ptr) = hay.as_ptr() else {
            return MoltObject::none().bits();
        };
        unsafe {
            if object_type_id(hay_ptr) != TYPE_ID_BYTES {
                return MoltObject::none().bits();
            }
            let hay_bytes = bytes_like_slice(hay_ptr).unwrap_or(&[]);
            let out = match bytes_translate_impl(_py, hay_bytes, table_bits, delete_bits) {
                Ok(out) => out,
                Err(err_bits) => return err_bits,
            };
            let ptr = alloc_bytes(_py, &out);
            if ptr.is_null() {
                return MoltObject::none().bits();
            }
            MoltObject::from_ptr(ptr).bits()
        }
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_bytearray_translate(
    hay_bits: u64,
    table_bits: u64,
    delete_bits: u64,
) -> u64 {
    crate::with_gil_entry_nopanic!(_py, {
        let hay = obj_from_bits(hay_bits);
        let Some(hay_ptr) = hay.as_ptr() else {
            return MoltObject::none().bits();
        };
        unsafe {
            if object_type_id(hay_ptr) != TYPE_ID_BYTEARRAY {
                return MoltObject::none().bits();
            }
            let hay_bytes = bytes_like_slice(hay_ptr).unwrap_or(&[]);
            let out = match bytes_translate_impl(_py, hay_bytes, table_bits, delete_bits) {
                Ok(out) => out,
                Err(err_bits) => return err_bits,
            };
            let ptr = alloc_bytearray(_py, &out);
            if ptr.is_null() {
                return MoltObject::none().bits();
            }
            MoltObject::from_ptr(ptr).bits()
        }
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_bytes_maketrans(from_bits: u64, to_bits: u64) -> u64 {
    crate::with_gil_entry_nopanic!(_py, {
        let from_obj = obj_from_bits(from_bits);
        let to_obj = obj_from_bits(to_bits);
        let from_ptr = match from_obj.as_ptr() {
            Some(ptr) => ptr,
            None => {
                let msg = format!(
                    "a bytes-like object is required, not '{}'",
                    type_name(_py, from_obj)
                );
                return raise_exception::<_>(_py, "TypeError", &msg);
            }
        };
        let to_ptr = match to_obj.as_ptr() {
            Some(ptr) => ptr,
            None => {
                let msg = format!(
                    "a bytes-like object is required, not '{}'",
                    type_name(_py, to_obj)
                );
                return raise_exception::<_>(_py, "TypeError", &msg);
            }
        };
        let from_bytes = match unsafe { bytes_like_slice(from_ptr) } {
            Some(slice) => slice,
            None => {
                let msg = format!(
                    "a bytes-like object is required, not '{}'",
                    type_name(_py, from_obj)
                );
                return raise_exception::<_>(_py, "TypeError", &msg);
            }
        };
        let to_bytes = match unsafe { bytes_like_slice(to_ptr) } {
            Some(slice) => slice,
            None => {
                let msg = format!(
                    "a bytes-like object is required, not '{}'",
                    type_name(_py, to_obj)
                );
                return raise_exception::<_>(_py, "TypeError", &msg);
            }
        };
        if from_bytes.len() != to_bytes.len() {
            return raise_exception::<_>(
                _py,
                "ValueError",
                "maketrans arguments must have same length",
            );
        }
        let mut table = [0u8; 256];
        for (idx, slot) in table.iter_mut().enumerate() {
            *slot = idx as u8;
        }
        for (from_byte, to_byte) in from_bytes.iter().zip(to_bytes.iter()) {
            table[*from_byte as usize] = *to_byte;
        }
        let ptr = alloc_bytes(_py, &table);
        if ptr.is_null() {
            return MoltObject::none().bits();
        }
        MoltObject::from_ptr(ptr).bits()
    })
}

fn fromhex_nibble(b: u8) -> Option<u8> {
    match b {
        b'0'..=b'9' => Some(b - b'0'),
        b'a'..=b'f' => Some(b - b'a' + 10),
        b'A'..=b'F' => Some(b - b'A' + 10),
        _ => None,
    }
}

fn bytes_fromhex_parse(_py: &PyToken<'_>, text: &[u8]) -> Result<Vec<u8>, u64> {
    let mut out: Vec<u8> = Vec::new();
    let mut idx = 0usize;
    while idx < text.len() {
        // Skip ASCII whitespace. CPython ignores whitespace between nibbles and bytes.
        while idx < text.len() && text[idx].is_ascii_whitespace() {
            idx += 1;
        }
        if idx >= text.len() {
            break;
        }
        let Some(hi) = fromhex_nibble(text[idx]) else {
            let msg = format!("non-hexadecimal number found in fromhex() arg at position {idx}");
            return Err(raise_exception::<_>(_py, "ValueError", &msg));
        };
        idx += 1;
        while idx < text.len() && text[idx].is_ascii_whitespace() {
            idx += 1;
        }
        if idx >= text.len() {
            let msg = format!("non-hexadecimal number found in fromhex() arg at position {idx}");
            return Err(raise_exception::<_>(_py, "ValueError", &msg));
        }
        let Some(lo) = fromhex_nibble(text[idx]) else {
            let msg = format!("non-hexadecimal number found in fromhex() arg at position {idx}");
            return Err(raise_exception::<_>(_py, "ValueError", &msg));
        };
        idx += 1;
        out.push((hi << 4) | lo);
    }
    Ok(out)
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_bytes_fromhex(cls_bits: u64, text_bits: u64) -> u64 {
    crate::with_gil_entry_nopanic!(_py, {
        let text_obj = obj_from_bits(text_bits);
        let Some(text_ptr) = text_obj.as_ptr() else {
            let msg = format!(
                "fromhex() argument must be str, not {}",
                type_name(_py, text_obj)
            );
            return raise_exception::<_>(_py, "TypeError", &msg);
        };
        unsafe {
            if object_type_id(text_ptr) != TYPE_ID_STRING {
                let msg = format!(
                    "fromhex() argument must be str, not {}",
                    type_name(_py, text_obj)
                );
                return raise_exception::<_>(_py, "TypeError", &msg);
            }
            let text = std::slice::from_raw_parts(string_bytes(text_ptr), string_len(text_ptr));
            let out = match bytes_fromhex_parse(_py, text) {
                Ok(out) => out,
                Err(err_bits) => return err_bits,
            };
            let bytes_ptr = alloc_bytes(_py, &out);
            if bytes_ptr.is_null() {
                return MoltObject::none().bits();
            }
            let bytes_bits = MoltObject::from_ptr(bytes_ptr).bits();
            let builtins = builtin_classes(_py);
            if cls_bits == builtins.bytes {
                return bytes_bits;
            }
            if !issubclass_bits(cls_bits, builtins.bytes) {
                dec_ref_bits(_py, bytes_bits);
                return raise_exception::<_>(
                    _py,
                    "TypeError",
                    "fromhex() requires a bytes subclass",
                );
            }
            let res_bits = call_callable1(_py, cls_bits, bytes_bits);
            dec_ref_bits(_py, bytes_bits);
            if exception_pending(_py) {
                return MoltObject::none().bits();
            }
            res_bits
        }
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_bytearray_fromhex(cls_bits: u64, text_bits: u64) -> u64 {
    crate::with_gil_entry_nopanic!(_py, {
        let text_obj = obj_from_bits(text_bits);
        let Some(text_ptr) = text_obj.as_ptr() else {
            let msg = format!(
                "fromhex() argument must be str, not {}",
                type_name(_py, text_obj)
            );
            return raise_exception::<_>(_py, "TypeError", &msg);
        };
        unsafe {
            if object_type_id(text_ptr) != TYPE_ID_STRING {
                let msg = format!(
                    "fromhex() argument must be str, not {}",
                    type_name(_py, text_obj)
                );
                return raise_exception::<_>(_py, "TypeError", &msg);
            }
            let text = std::slice::from_raw_parts(string_bytes(text_ptr), string_len(text_ptr));
            let out = match bytes_fromhex_parse(_py, text) {
                Ok(out) => out,
                Err(err_bits) => return err_bits,
            };
            let ba_ptr = alloc_bytearray(_py, &out);
            if ba_ptr.is_null() {
                return MoltObject::none().bits();
            }
            let ba_bits = MoltObject::from_ptr(ba_ptr).bits();
            let builtins = builtin_classes(_py);
            if cls_bits == builtins.bytearray {
                return ba_bits;
            }
            if !issubclass_bits(cls_bits, builtins.bytearray) {
                dec_ref_bits(_py, ba_bits);
                return raise_exception::<_>(
                    _py,
                    "TypeError",
                    "fromhex() requires a bytearray subclass",
                );
            }
            let res_bits = call_callable1(_py, cls_bits, ba_bits);
            dec_ref_bits(_py, ba_bits);
            if exception_pending(_py) {
                return MoltObject::none().bits();
            }
            res_bits
        }
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_bytes_hex(hay_bits: u64, sep_bits: u64, bytes_per_sep_bits: u64) -> u64 {
    crate::with_gil_entry_nopanic!(_py, {
        let hay = obj_from_bits(hay_bits);
        let Some(hay_ptr) = hay.as_ptr() else {
            return MoltObject::none().bits();
        };
        unsafe {
            if object_type_id(hay_ptr) != TYPE_ID_BYTES {
                return MoltObject::none().bits();
            }
            let hay_bytes = bytes_like_slice(hay_ptr).unwrap_or(&[]);
            bytes_hex_from_bits(_py, hay_bytes, sep_bits, bytes_per_sep_bits)
        }
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_bytearray_hex(hay_bits: u64, sep_bits: u64, bytes_per_sep_bits: u64) -> u64 {
    crate::with_gil_entry_nopanic!(_py, {
        let hay = obj_from_bits(hay_bits);
        let Some(hay_ptr) = hay.as_ptr() else {
            return MoltObject::none().bits();
        };
        unsafe {
            if object_type_id(hay_ptr) != TYPE_ID_BYTEARRAY {
                return MoltObject::none().bits();
            }
            let hay_bytes = bytes_like_slice(hay_ptr).unwrap_or(&[]);
            bytes_hex_from_bits(_py, hay_bytes, sep_bits, bytes_per_sep_bits)
        }
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_bytearray_replace(
    hay_bits: u64,
    needle_bits: u64,
    replacement_bits: u64,
    count_bits: u64,
) -> u64 {
    crate::with_gil_entry_nopanic!(_py, {
        let hay = obj_from_bits(hay_bits);
        let needle = obj_from_bits(needle_bits);
        let replacement = obj_from_bits(replacement_bits);
        let count_err = format!(
            "'{}' object cannot be interpreted as an integer",
            type_name(_py, obj_from_bits(count_bits))
        );
        let count = index_i64_from_obj(_py, count_bits, &count_err);
        if exception_pending(_py) {
            return MoltObject::none().bits();
        }
        if let Some(hay_ptr) = hay.as_ptr() {
            unsafe {
                if object_type_id(hay_ptr) != TYPE_ID_BYTEARRAY {
                    return MoltObject::none().bits();
                }
                let hay_bytes = bytes_like_slice(hay_ptr).unwrap_or(&[]);
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
                let needle_bytes = match bytes_like_slice(needle_ptr) {
                    Some(slice) => slice,
                    None => {
                        let msg = format!(
                            "a bytes-like object is required, not '{}'",
                            type_name(_py, needle)
                        );
                        return raise_exception::<_>(_py, "TypeError", &msg);
                    }
                };
                let repl_ptr = match replacement.as_ptr() {
                    Some(ptr) => ptr,
                    None => {
                        let msg = format!(
                            "a bytes-like object is required, not '{}'",
                            type_name(_py, replacement)
                        );
                        return raise_exception::<_>(_py, "TypeError", &msg);
                    }
                };
                let repl_bytes = match bytes_like_slice(repl_ptr) {
                    Some(slice) => slice,
                    None => {
                        let msg = format!(
                            "a bytes-like object is required, not '{}'",
                            type_name(_py, replacement)
                        );
                        return raise_exception::<_>(_py, "TypeError", &msg);
                    }
                };
                let out = if count < 0 {
                    match replace_bytes_impl(hay_bytes, needle_bytes, repl_bytes) {
                        Some(out) => out,
                        None => return MoltObject::none().bits(),
                    }
                } else {
                    replace_bytes_impl_limit(hay_bytes, needle_bytes, repl_bytes, count as usize)
                };
                let ptr = alloc_bytearray(_py, &out);
                if ptr.is_null() {
                    return MoltObject::none().bits();
                }
                return MoltObject::from_ptr(ptr).bits();
            }
        }
        MoltObject::none().bits()
    })
}

#[derive(Clone, Copy)]
pub(super) enum BytesCtorKind {
    Bytes,
    Bytearray,
}

impl BytesCtorKind {
    pub(crate) fn name(self) -> &'static str {
        match self {
            BytesCtorKind::Bytes => "bytes",
            BytesCtorKind::Bytearray => "bytearray",
        }
    }

    fn ctor_label(self) -> &'static str {
        match self {
            BytesCtorKind::Bytes => "bytes()",
            BytesCtorKind::Bytearray => "bytearray()",
        }
    }

    fn range_error(self) -> &'static str {
        match self {
            BytesCtorKind::Bytes => "bytes must be in range(0, 256)",
            BytesCtorKind::Bytearray => "byte must be in range(0, 256)",
        }
    }

    fn non_iterable_message(self, type_name: &str) -> String {
        format!("cannot convert '{}' object to {}", type_name, self.name())
    }

    fn arg_type_message(self, arg: &str, type_name: &str) -> String {
        format!(
            "{} argument '{}' must be str, not {}",
            self.ctor_label(),
            arg,
            type_name
        )
    }
}

fn bytes_from_count(_py: &PyToken<'_>, len: usize, type_id: u32) -> u64 {
    if type_id == TYPE_ID_BYTEARRAY {
        let ptr = alloc_bytearray_with_len(_py, len);
        if ptr.is_null() {
            return MoltObject::none().bits();
        }
        return MoltObject::from_ptr(ptr).bits();
    }
    let ptr = alloc_bytes_like_with_len(_py, len, type_id);
    if ptr.is_null() {
        return MoltObject::none().bits();
    }
    unsafe {
        let data_ptr = ptr.add(std::mem::size_of::<usize>());
        std::ptr::write_bytes(data_ptr, 0, len);
    }
    MoltObject::from_ptr(ptr).bits()
}

pub(super) fn bytes_item_to_u8(_py: &PyToken<'_>, bits: u64, kind: BytesCtorKind) -> Option<u8> {
    let type_name = class_name_for_error(type_of_bits(_py, bits));
    let msg = format!("'{}' object cannot be interpreted as an integer", type_name);
    let val = index_i64_from_obj(_py, bits, &msg);
    if exception_pending(_py) {
        return None;
    }
    if !(0..=255).contains(&val) {
        return raise_exception::<_>(_py, "ValueError", kind.range_error());
    }
    Some(val as u8)
}

fn bytes_collect_from_iter(
    _py: &PyToken<'_>,
    iter_bits: u64,
    kind: BytesCtorKind,
) -> Option<Vec<u8>> {
    let mut out = Vec::new();
    loop {
        let pair_bits = molt_iter_next(iter_bits);
        if exception_pending(_py) {
            return None;
        }
        let pair_ptr = obj_from_bits(pair_bits).as_ptr()?;
        unsafe {
            if object_type_id(pair_ptr) != TYPE_ID_TUPLE {
                return None;
            }
            let elems = seq_vec_ref(pair_ptr);
            if elems.len() < 2 {
                return None;
            }
            let done_bits = elems[1];
            if is_truthy(_py, obj_from_bits(done_bits)) {
                break;
            }
            let val_bits = elems[0];
            let byte = bytes_item_to_u8(_py, val_bits, kind)?;
            out.push(byte);
        }
    }
    Some(out)
}

fn bytes_from_obj_impl(_py: &PyToken<'_>, bits: u64, kind: BytesCtorKind) -> u64 {
    let obj = obj_from_bits(bits);
    if let Some(i) = to_i64(obj) {
        if i < 0 {
            return raise_exception::<_>(_py, "ValueError", "negative count");
        }
        let len = match usize::try_from(i) {
            Ok(len) => len,
            Err(_) => {
                return raise_exception::<_>(
                    _py,
                    "OverflowError",
                    "cannot fit 'int' into an index-sized integer",
                );
            }
        };
        let type_id = match kind {
            BytesCtorKind::Bytes => TYPE_ID_BYTES,
            BytesCtorKind::Bytearray => TYPE_ID_BYTEARRAY,
        };
        return bytes_from_count(_py, len, type_id);
    }
    if let Some(ptr) = obj.as_ptr() {
        unsafe {
            let type_id = object_type_id(ptr);
            if type_id == TYPE_ID_STRING {
                return raise_exception::<_>(
                    _py,
                    "TypeError",
                    "string argument without an encoding",
                );
            }
            if type_id == TYPE_ID_BYTES && matches!(kind, BytesCtorKind::Bytes) {
                inc_ref_bits(_py, bits);
                return bits;
            }
            if type_id == TYPE_ID_LIST || type_id == TYPE_ID_TUPLE {
                let elems = seq_vec_ref(ptr);
                let mut out = Vec::with_capacity(elems.len());
                for &elem in elems.iter() {
                    let Some(byte) = bytes_item_to_u8(_py, elem, kind) else {
                        return MoltObject::none().bits();
                    };
                    out.push(byte);
                }
                let out_ptr = match kind {
                    BytesCtorKind::Bytes => alloc_bytes(_py, &out),
                    BytesCtorKind::Bytearray => alloc_bytearray(_py, &out),
                };
                if out_ptr.is_null() {
                    return MoltObject::none().bits();
                }
                return MoltObject::from_ptr(out_ptr).bits();
            }
            if let Some(slice) = bytes_like_slice(ptr) {
                let out_ptr = match kind {
                    BytesCtorKind::Bytes => alloc_bytes(_py, slice),
                    BytesCtorKind::Bytearray => alloc_bytearray(_py, slice),
                };
                if out_ptr.is_null() {
                    return MoltObject::none().bits();
                }
                return MoltObject::from_ptr(out_ptr).bits();
            }
            if type_id == TYPE_ID_MEMORYVIEW
                && let Some(out) = memoryview_collect_bytes(ptr)
            {
                let out_ptr = match kind {
                    BytesCtorKind::Bytes => alloc_bytes(_py, &out),
                    BytesCtorKind::Bytearray => alloc_bytearray(_py, &out),
                };
                if out_ptr.is_null() {
                    return MoltObject::none().bits();
                }
                return MoltObject::from_ptr(out_ptr).bits();
            }
            // Check __bytes__ method (e.g. PickleBuffer, custom objects).
            if matches!(kind, BytesCtorKind::Bytes) {
                let bytes_dunder = intern_static_name(
                    _py,
                    &runtime_state(_py).interned.bytes_dunder,
                    b"__bytes__",
                );
                let call = attr_lookup_ptr(_py, ptr, bytes_dunder);
                if let Some(call_bits) = call {
                    let res_bits = call_callable0(_py, call_bits);
                    dec_ref_bits(_py, call_bits);
                    if exception_pending(_py) {
                        return MoltObject::none().bits();
                    }
                    if let Some(res_ptr) = obj_from_bits(res_bits).as_ptr()
                        && object_type_id(res_ptr) == TYPE_ID_BYTES
                    {
                        return res_bits;
                    }
                    let res_type = class_name_for_error(type_of_bits(_py, res_bits));
                    if obj_from_bits(res_bits).as_ptr().is_some() {
                        dec_ref_bits(_py, res_bits);
                    }
                    let msg = format!("__bytes__ returned non-bytes (type {res_type})");
                    return raise_exception::<_>(_py, "TypeError", &msg);
                }
                if exception_pending(_py) {
                    clear_exception(_py);
                }
            }
            if type_id == TYPE_ID_BIGINT {
                let big = bigint_ref(ptr);
                if big.is_negative() {
                    return raise_exception::<_>(_py, "ValueError", "negative count");
                }
                let Some(len) = big.to_usize() else {
                    return raise_exception::<_>(
                        _py,
                        "OverflowError",
                        "cannot fit 'int' into an index-sized integer",
                    );
                };
                let type_id = match kind {
                    BytesCtorKind::Bytes => TYPE_ID_BYTES,
                    BytesCtorKind::Bytearray => TYPE_ID_BYTEARRAY,
                };
                return bytes_from_count(_py, len, type_id);
            }
            let index_name_bits =
                intern_static_name(_py, &runtime_state(_py).interned.index_name, b"__index__");
            let call_bits = attr_lookup_ptr(_py, ptr, index_name_bits);
            dec_ref_bits(_py, index_name_bits);
            if let Some(call_bits) = call_bits {
                let res_bits = call_callable0(_py, call_bits);
                dec_ref_bits(_py, call_bits);
                if exception_pending(_py) {
                    return MoltObject::none().bits();
                }
                let res_obj = obj_from_bits(res_bits);
                if let Some(i) = to_i64(res_obj) {
                    if i < 0 {
                        return raise_exception::<_>(_py, "ValueError", "negative count");
                    }
                    let len = match usize::try_from(i) {
                        Ok(len) => len,
                        Err(_) => {
                            return raise_exception::<_>(
                                _py,
                                "OverflowError",
                                "cannot fit 'int' into an index-sized integer",
                            );
                        }
                    };
                    let type_id = match kind {
                        BytesCtorKind::Bytes => TYPE_ID_BYTES,
                        BytesCtorKind::Bytearray => TYPE_ID_BYTEARRAY,
                    };
                    return bytes_from_count(_py, len, type_id);
                }
                if let Some(big_ptr) = bigint_ptr_from_bits(res_bits) {
                    let big = bigint_ref(big_ptr);
                    if big.is_negative() {
                        return raise_exception::<_>(_py, "ValueError", "negative count");
                    }
                    let Some(len) = big.to_usize() else {
                        return raise_exception::<_>(
                            _py,
                            "OverflowError",
                            "cannot fit 'int' into an index-sized integer",
                        );
                    };
                    dec_ref_bits(_py, res_bits);
                    let type_id = match kind {
                        BytesCtorKind::Bytes => TYPE_ID_BYTES,
                        BytesCtorKind::Bytearray => TYPE_ID_BYTEARRAY,
                    };
                    return bytes_from_count(_py, len, type_id);
                }
                let res_type = class_name_for_error(type_of_bits(_py, res_bits));
                if res_obj.as_ptr().is_some() {
                    dec_ref_bits(_py, res_bits);
                }
                let msg = format!("__index__ returned non-int (type {res_type})");
                return raise_exception::<_>(_py, "TypeError", &msg);
            }
        }
    }
    let iter_bits = molt_iter(bits);
    if obj_from_bits(iter_bits).is_none() {
        let type_name = class_name_for_error(type_of_bits(_py, bits));
        let msg = kind.non_iterable_message(&type_name);
        return raise_exception::<_>(_py, "TypeError", &msg);
    }
    let Some(out) = bytes_collect_from_iter(_py, iter_bits, kind) else {
        return MoltObject::none().bits();
    };
    let out_ptr = match kind {
        BytesCtorKind::Bytes => alloc_bytes(_py, &out),
        BytesCtorKind::Bytearray => alloc_bytearray(_py, &out),
    };
    if out_ptr.is_null() {
        return MoltObject::none().bits();
    }
    MoltObject::from_ptr(out_ptr).bits()
}

fn bytes_from_str_impl(
    _py: &PyToken<'_>,
    src_bits: u64,
    encoding_bits: u64,
    errors_bits: u64,
    kind: BytesCtorKind,
) -> u64 {
    let encoding_obj = obj_from_bits(encoding_bits);
    let errors_obj = obj_from_bits(errors_bits);
    let encoding = if encoding_obj.is_none() {
        None
    } else {
        let Some(encoding) = string_obj_to_owned(encoding_obj) else {
            let type_name = class_name_for_error(type_of_bits(_py, encoding_bits));
            let msg = kind.arg_type_message("encoding", &type_name);
            return raise_exception::<_>(_py, "TypeError", &msg);
        };
        Some(encoding)
    };
    let errors = if errors_obj.is_none() {
        None
    } else {
        let Some(errors) = string_obj_to_owned(errors_obj) else {
            let type_name = class_name_for_error(type_of_bits(_py, errors_bits));
            let msg = kind.arg_type_message("errors", &type_name);
            return raise_exception::<_>(_py, "TypeError", &msg);
        };
        Some(errors)
    };
    let src_obj = obj_from_bits(src_bits);
    let Some(src_ptr) = src_obj.as_ptr() else {
        if encoding.is_some() {
            return raise_exception::<_>(_py, "TypeError", "encoding without a string argument");
        }
        if errors.is_some() {
            return raise_exception::<_>(_py, "TypeError", "errors without a string argument");
        }
        return MoltObject::none().bits();
    };
    unsafe {
        if object_type_id(src_ptr) != TYPE_ID_STRING {
            if encoding.is_some() {
                return raise_exception::<_>(
                    _py,
                    "TypeError",
                    "encoding without a string argument",
                );
            }
            if errors.is_some() {
                return raise_exception::<_>(_py, "TypeError", "errors without a string argument");
            }
            return MoltObject::none().bits();
        }
    }
    let Some(encoding) = encoding else {
        return raise_exception::<_>(_py, "TypeError", "string argument without an encoding");
    };
    let bytes = unsafe { std::slice::from_raw_parts(string_bytes(src_ptr), string_len(src_ptr)) };
    let out = match encode_string_with_errors(bytes, &encoding, errors.as_deref()) {
        Ok(bytes) => bytes,
        Err(EncodeError::UnknownEncoding(name)) => {
            let msg = format!("unknown encoding: {name}");
            return raise_exception::<_>(_py, "LookupError", &msg);
        }
        Err(EncodeError::UnknownErrorHandler(name)) => {
            let msg = format!("unknown error handler name '{name}'");
            return raise_exception::<_>(_py, "LookupError", &msg);
        }
        Err(EncodeError::InvalidChar {
            encoding,
            code,
            pos,
            limit,
        }) => {
            let reason = encode_error_reason(encoding, code, limit);
            return raise_unicode_encode_error::<_>(_py, encoding, src_bits, pos, pos + 1, &reason);
        }
    };
    let out_ptr = match kind {
        BytesCtorKind::Bytes => alloc_bytes(_py, &out),
        BytesCtorKind::Bytearray => alloc_bytearray(_py, &out),
    };
    if out_ptr.is_null() {
        return MoltObject::none().bits();
    }
    MoltObject::from_ptr(out_ptr).bits()
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_bytes_from_obj(bits: u64) -> u64 {
    crate::with_gil_entry_nopanic!(_py, {
        bytes_from_obj_impl(_py, bits, BytesCtorKind::Bytes)
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_bytearray_from_obj(bits: u64) -> u64 {
    crate::with_gil_entry_nopanic!(_py, {
        bytes_from_obj_impl(_py, bits, BytesCtorKind::Bytearray)
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_bytes_from_str(src_bits: u64, encoding_bits: u64, errors_bits: u64) -> u64 {
    crate::with_gil_entry_nopanic!(_py, {
        bytes_from_str_impl(
            _py,
            src_bits,
            encoding_bits,
            errors_bits,
            BytesCtorKind::Bytes,
        )
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_bytearray_from_str(
    src_bits: u64,
    encoding_bits: u64,
    errors_bits: u64,
) -> u64 {
    crate::with_gil_entry_nopanic!(_py, {
        bytes_from_str_impl(
            _py,
            src_bits,
            encoding_bits,
            errors_bits,
            BytesCtorKind::Bytearray,
        )
    })
}
