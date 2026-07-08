//! String split and partition operations.

use crate::*;
use memchr::memmem;
use molt_obj_model::MoltObject;

use super::ops_string_utf8::{utf8_codepoint_count_cached, wtf8_codepoint_at};
fn partition_string_bytes(
    _py: &PyToken<'_>,
    hay_bytes: &[u8],
    sep_bytes: &[u8],
    from_right: bool,
) -> Option<u64> {
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
    let head_ptr = alloc_string(_py, head_bytes);
    if head_ptr.is_null() {
        return None;
    }
    let head_bits = MoltObject::from_ptr(head_ptr).bits();
    let sep_ptr = alloc_string(_py, sep_bytes);
    if sep_ptr.is_null() {
        dec_ref_bits(_py, head_bits);
        return None;
    }
    let sep_bits = MoltObject::from_ptr(sep_ptr).bits();
    let tail_ptr = alloc_string(_py, tail_bytes);
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
pub extern "C" fn molt_string_partition(hay_bits: u64, sep_bits: u64) -> u64 {
    crate::with_gil_entry_nopanic!(_py, {
        let hay = obj_from_bits(hay_bits);
        let sep = obj_from_bits(sep_bits);
        let Some(hay_ptr) = hay.as_ptr() else {
            return MoltObject::none().bits();
        };
        unsafe {
            if object_type_id(hay_ptr) != TYPE_ID_STRING {
                return MoltObject::none().bits();
            }
            let sep_ptr = match sep.as_ptr() {
                Some(ptr) => ptr,
                None => {
                    let msg = format!("must be str, not {}", type_name(_py, sep));
                    return raise_exception::<_>(_py, "TypeError", &msg);
                }
            };
            if object_type_id(sep_ptr) != TYPE_ID_STRING {
                let msg = format!("must be str, not {}", type_name(_py, sep));
                return raise_exception::<_>(_py, "TypeError", &msg);
            }
            let hay_bytes = std::slice::from_raw_parts(string_bytes(hay_ptr), string_len(hay_ptr));
            let sep_bytes = std::slice::from_raw_parts(string_bytes(sep_ptr), string_len(sep_ptr));
            if sep_bytes.is_empty() {
                return raise_exception::<_>(_py, "ValueError", "empty separator");
            }
            let tuple_bits = partition_string_bytes(_py, hay_bytes, sep_bytes, false);
            tuple_bits.unwrap_or_else(|| MoltObject::none().bits())
        }
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_string_rpartition(hay_bits: u64, sep_bits: u64) -> u64 {
    crate::with_gil_entry_nopanic!(_py, {
        let hay = obj_from_bits(hay_bits);
        let sep = obj_from_bits(sep_bits);
        let Some(hay_ptr) = hay.as_ptr() else {
            return MoltObject::none().bits();
        };
        unsafe {
            if object_type_id(hay_ptr) != TYPE_ID_STRING {
                return MoltObject::none().bits();
            }
            let sep_ptr = match sep.as_ptr() {
                Some(ptr) => ptr,
                None => {
                    let msg = format!("must be str, not {}", type_name(_py, sep));
                    return raise_exception::<_>(_py, "TypeError", &msg);
                }
            };
            if object_type_id(sep_ptr) != TYPE_ID_STRING {
                let msg = format!("must be str, not {}", type_name(_py, sep));
                return raise_exception::<_>(_py, "TypeError", &msg);
            }
            let hay_bytes = std::slice::from_raw_parts(string_bytes(hay_ptr), string_len(hay_ptr));
            let sep_bytes = std::slice::from_raw_parts(string_bytes(sep_ptr), string_len(sep_ptr));
            if sep_bytes.is_empty() {
                return raise_exception::<_>(_py, "ValueError", "empty separator");
            }
            let tuple_bits = partition_string_bytes(_py, hay_bytes, sep_bytes, true);
            tuple_bits.unwrap_or_else(|| MoltObject::none().bits())
        }
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_string_splitlines(hay_bits: u64, keepends_bits: u64) -> u64 {
    crate::with_gil_entry_nopanic!(_py, {
        let hay = obj_from_bits(hay_bits);
        let keepends = is_truthy(_py, obj_from_bits(keepends_bits));
        let Some(hay_ptr) = hay.as_ptr() else {
            return MoltObject::none().bits();
        };
        unsafe {
            if object_type_id(hay_ptr) != TYPE_ID_STRING {
                return MoltObject::none().bits();
            }
            let hay_bytes = std::slice::from_raw_parts(string_bytes(hay_ptr), string_len(hay_ptr));
            let Ok(hay_str) = std::str::from_utf8(hay_bytes) else {
                return MoltObject::none().bits();
            };
            let list_bits = splitlines_string_to_list(_py, hay_str, keepends);
            list_bits.unwrap_or_else(|| MoltObject::none().bits())
        }
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_string_split(hay_bits: u64, needle_bits: u64) -> u64 {
    crate::with_gil_entry_nopanic!(_py, {
        let maxsplit_bits = MoltObject::from_int(-1).bits();
        molt_string_split_max(hay_bits, needle_bits, maxsplit_bits)
    })
}

/// Validate explicit `str.split`/split-field arguments and return live string pointers.
///
/// # Safety
/// Caller must hold the GIL and pass object bits whose pointees stay live for
/// the returned raw pointers. The returned pointers are only borrowed; callers
/// must not store them beyond the current GIL entry.
unsafe fn validate_explicit_string_split_args(
    _py: &PyToken<'_>,
    hay_bits: u64,
    needle_bits: u64,
) -> Option<(*mut u8, *mut u8)> {
    unsafe {
        let hay = obj_from_bits(hay_bits);
        let needle = obj_from_bits(needle_bits);
        let Some(hay_ptr) = hay.as_ptr() else {
            let msg = format!(
                "descriptor 'split' for 'str' objects doesn't apply to a '{}' object",
                type_name(_py, hay)
            );
            raise_exception::<()>(_py, "TypeError", &msg);
            return None;
        };
        if object_type_id(hay_ptr) != TYPE_ID_STRING {
            let msg = format!(
                "descriptor 'split' for 'str' objects doesn't apply to a '{}' object",
                type_name(_py, hay)
            );
            raise_exception::<()>(_py, "TypeError", &msg);
            return None;
        }
        let Some(needle_ptr) = needle.as_ptr() else {
            let msg = format!("must be str or None, not {}", type_name(_py, needle));
            raise_exception::<()>(_py, "TypeError", &msg);
            return None;
        };
        if object_type_id(needle_ptr) != TYPE_ID_STRING {
            let msg = format!("must be str or None, not {}", type_name(_py, needle));
            raise_exception::<()>(_py, "TypeError", &msg);
            return None;
        }
        if string_len(needle_ptr) == 0 {
            raise_exception::<()>(_py, "ValueError", "empty separator");
            return None;
        }
        Some((hay_ptr, needle_ptr))
    }
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_string_split_validate(hay_bits: u64, needle_bits: u64) -> u64 {
    crate::with_gil_entry_nopanic!(_py, {
        unsafe {
            if validate_explicit_string_split_args(_py, hay_bits, needle_bits).is_none() {
                return MoltObject::none().bits();
            }
        }
        MoltObject::none().bits()
    })
}

fn split_field_index_error(_py: &PyToken<'_>) -> u64 {
    raise_exception::<_>(_py, "IndexError", "list index out of range")
}

pub(crate) fn split_field_bounds_at_index(
    hay: &[u8],
    needle: &[u8],
    target_index: usize,
) -> Option<(usize, usize)> {
    let mut field_index = 0usize;
    let mut start = 0usize;
    if needle.len() == 1 {
        for idx in memchr::memchr_iter(needle[0], hay) {
            if field_index == target_index {
                return Some((start, idx));
            }
            start = idx + 1;
            field_index += 1;
        }
    } else {
        let finder = memmem::Finder::new(needle);
        for idx in finder.find_iter(hay) {
            if field_index == target_index {
                return Some((start, idx));
            }
            start = idx + needle.len();
            field_index += 1;
        }
    }
    if field_index == target_index {
        Some((start, hay.len()))
    } else {
        None
    }
}

fn alloc_split_field_at_index(
    _py: &PyToken<'_>,
    hay: &[u8],
    needle: &[u8],
    target_index: usize,
) -> u64 {
    if let Some((start, end)) = split_field_bounds_at_index(hay, needle, target_index) {
        let ptr = alloc_string(_py, &hay[start..end]);
        return if ptr.is_null() {
            MoltObject::none().bits()
        } else {
            MoltObject::from_ptr(ptr).bits()
        };
    }
    split_field_index_error(_py)
}

pub(crate) fn explicit_split_field_args(
    _py: &PyToken<'_>,
    hay_bits: u64,
    needle_bits: u64,
    index_bits: u64,
) -> Option<(*mut u8, *mut u8, usize)> {
    unsafe {
        let (hay_ptr, needle_ptr) =
            validate_explicit_string_split_args(_py, hay_bits, needle_bits)?;
        let Some(index) = to_i64(obj_from_bits(index_bits)) else {
            let msg = format!(
                "list indices must be integers or slices, not {}",
                type_name(_py, obj_from_bits(index_bits))
            );
            raise_exception::<()>(_py, "TypeError", &msg);
            return None;
        };
        let Ok(target_index) = usize::try_from(index) else {
            split_field_index_error(_py);
            return None;
        };
        Some((hay_ptr, needle_ptr, target_index))
    }
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_string_split_field(hay_bits: u64, needle_bits: u64, index_bits: u64) -> u64 {
    crate::with_gil_entry_nopanic!(_py, {
        unsafe {
            let Some((hay_ptr, needle_ptr, target_index)) =
                explicit_split_field_args(_py, hay_bits, needle_bits, index_bits)
            else {
                return MoltObject::none().bits();
            };
            let hay_bytes = std::slice::from_raw_parts(string_bytes(hay_ptr), string_len(hay_ptr));
            let needle_bytes =
                std::slice::from_raw_parts(string_bytes(needle_ptr), string_len(needle_ptr));
            alloc_split_field_at_index(_py, hay_bytes, needle_bytes, target_index)
        }
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_string_split_field_len(
    hay_bits: u64,
    needle_bits: u64,
    index_bits: u64,
) -> u64 {
    crate::with_gil_entry_nopanic!(_py, {
        unsafe {
            let Some((hay_ptr, needle_ptr, target_index)) =
                explicit_split_field_args(_py, hay_bits, needle_bits, index_bits)
            else {
                return MoltObject::none().bits();
            };
            let hay_bytes = std::slice::from_raw_parts(string_bytes(hay_ptr), string_len(hay_ptr));
            let needle_bytes =
                std::slice::from_raw_parts(string_bytes(needle_ptr), string_len(needle_ptr));
            let Some((start, end)) =
                split_field_bounds_at_index(hay_bytes, needle_bytes, target_index)
            else {
                return split_field_index_error(_py);
            };
            let field = &hay_bytes[start..end];
            let count = if field.is_ascii() {
                field.len() as i64
            } else {
                utf8_codepoint_count_cached(_py, field, None)
            };
            MoltObject::from_int(count).bits()
        }
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_string_split_field_eq(
    hay_bits: u64,
    needle_bits: u64,
    index_bits: u64,
    expected_bits: u64,
) -> u64 {
    crate::with_gil_entry_nopanic!(_py, {
        unsafe {
            let Some((hay_ptr, needle_ptr, target_index)) =
                explicit_split_field_args(_py, hay_bits, needle_bits, index_bits)
            else {
                return MoltObject::none().bits();
            };
            let expected = obj_from_bits(expected_bits);
            let Some(expected_ptr) = expected.as_ptr() else {
                return MoltObject::from_bool(false).bits();
            };
            if object_type_id(expected_ptr) != TYPE_ID_STRING {
                return MoltObject::from_bool(false).bits();
            }
            let hay_bytes = std::slice::from_raw_parts(string_bytes(hay_ptr), string_len(hay_ptr));
            let needle_bytes =
                std::slice::from_raw_parts(string_bytes(needle_ptr), string_len(needle_ptr));
            let Some((start, end)) =
                split_field_bounds_at_index(hay_bytes, needle_bytes, target_index)
            else {
                return split_field_index_error(_py);
            };
            let expected_bytes =
                std::slice::from_raw_parts(string_bytes(expected_ptr), string_len(expected_ptr));
            MoltObject::from_bool(&hay_bytes[start..end] == expected_bytes).bits()
        }
    })
}

/// Deforestation support for a non-escaping `s.split(sep)[idx]` field consumed
/// only by read-only string ops (`len` / `ord(field[i])`).
///
/// This is the keystone of the bounds-once design that AVOIDS the O(n²) trap a
/// per-char `split_field_ord_at(hay,sep,idx,cidx)` intrinsic would create
/// (re-scanning the split once per loop character). The deforestation pass emits
/// THREE of these field-property ops ONCE at the field-definition site (which
/// dominates the `while i < len(field)` char loop): `..._start`, `..._end` and
/// `..._is_ascii`. Each scans the split once to find the field's byte bounds;
/// every per-character `ord(field[i])` / `len(field)` consumer then reads from
/// those three already-computed values in O(1) (ASCII) via
/// [`molt_string_split_field_ord_at_bounds`] / [`molt_string_split_field_len_from_bounds`].
/// 3 split scans per field (no per-char rescans, no allocation) beats the
/// materializing path's 1 scan + 1 `alloc_string` + per-char heap reads.
///
/// All three return ORDINARY boxed Python ints (a field's byte offset is a
/// small, inline-boxable int for any realistically-allocatable `str`; a
/// pathologically large offset bigint-boxes, still decoded losslessly by the
/// consumers' `to_i64`), so they thread through the IR as plain int values with
/// no raw-carrier / representation plumbing. An out-of-range field index raises
/// `IndexError("list index out of range")` — byte-identical to the materializing
/// `molt_string_split_field` path's `split_field_index_error`.
#[unsafe(no_mangle)]
pub extern "C" fn molt_string_split_field_start(
    hay_bits: u64,
    needle_bits: u64,
    index_bits: u64,
) -> u64 {
    crate::with_gil_entry_nopanic!(_py, {
        unsafe {
            match split_field_bounds_bytes(_py, hay_bits, needle_bits, index_bits) {
                Some((start, _end, _hay)) => MoltObject::from_int(start as i64).bits(),
                None => MoltObject::none().bits(),
            }
        }
    })
}

/// Field byte END offset — see [`molt_string_split_field_start`].
#[unsafe(no_mangle)]
pub extern "C" fn molt_string_split_field_end(
    hay_bits: u64,
    needle_bits: u64,
    index_bits: u64,
) -> u64 {
    crate::with_gil_entry_nopanic!(_py, {
        unsafe {
            match split_field_bounds_bytes(_py, hay_bits, needle_bits, index_bits) {
                Some((_start, end, _hay)) => MoltObject::from_int(end as i64).bits(),
                None => MoltObject::none().bits(),
            }
        }
    })
}

/// Field ASCII flag (1 iff every byte of the field is < 0x80, so codepoint index
/// == byte index — the O(1) `ord`/`len` read fast path). See
/// [`molt_string_split_field_start`].
#[unsafe(no_mangle)]
pub extern "C" fn molt_string_split_field_is_ascii(
    hay_bits: u64,
    needle_bits: u64,
    index_bits: u64,
) -> u64 {
    crate::with_gil_entry_nopanic!(_py, {
        unsafe {
            match split_field_bounds_bytes(_py, hay_bits, needle_bits, index_bits) {
                Some((start, end, hay)) => {
                    MoltObject::from_int(hay[start..end].is_ascii() as i64).bits()
                }
                None => MoltObject::none().bits(),
            }
        }
    })
}

/// Shared core for the three field-property ops: validate args, scan the split
/// once, return `(start, end, hay_bytes)` or raise `IndexError` (returning
/// `None`). The returned slice borrows the (kept-alive by `string_split_validate`
/// / the live field haystack) source string.
///
/// # Safety
/// Caller is inside a GIL entry; `hay_bits`/`needle_bits` reference live objects.
#[inline]
unsafe fn split_field_bounds_bytes<'a>(
    _py: &PyToken<'a>,
    hay_bits: u64,
    needle_bits: u64,
    index_bits: u64,
) -> Option<(usize, usize, &'a [u8])> {
    unsafe {
        // The deforestation pass emits the three field-property ops back-to-back
        // (`_start`, `_end`, `_is_ascii`) with a single `check_exception` after
        // the group. On a bad field index the FIRST op raises; short-circuit the
        // rest so they neither re-raise nor read past an already-failed scan.
        if exception_pending(_py) {
            return None;
        }
        let (hay_ptr, needle_ptr, target_index) =
            explicit_split_field_args(_py, hay_bits, needle_bits, index_bits)?;
        let hay_bytes = std::slice::from_raw_parts(string_bytes(hay_ptr), string_len(hay_ptr));
        let needle_bytes =
            std::slice::from_raw_parts(string_bytes(needle_ptr), string_len(needle_ptr));
        match split_field_bounds_at_index(hay_bytes, needle_bytes, target_index) {
            Some((start, end)) => Some((start, end, hay_bytes)),
            None => {
                split_field_index_error(_py);
                None
            }
        }
    }
}

/// `len(field)` for a deforested split field, from the `(start, end, is_ascii)`
/// values produced by the field-property ops. ASCII fields: the byte span
/// `end - start` IS the codepoint count (O(1)). Non-ASCII: count codepoints over
/// `hay[start..end]` (no split re-scan), byte-identical to `len()` on the
/// materialized field. `start`/`end`/`is_ascii` arrive as boxed Python ints.
#[unsafe(no_mangle)]
pub extern "C" fn molt_string_split_field_len_from_bounds(
    hay_bits: u64,
    start_bits: u64,
    end_bits: u64,
    is_ascii_bits: u64,
) -> u64 {
    crate::with_gil_entry_nopanic!(_py, {
        let start = to_i64(obj_from_bits(start_bits)).unwrap_or(0) as usize;
        let end = to_i64(obj_from_bits(end_bits)).unwrap_or(0) as usize;
        let is_ascii = to_i64(obj_from_bits(is_ascii_bits)).unwrap_or(0) != 0;
        if is_ascii {
            return MoltObject::from_int((end - start) as i64).bits();
        }
        unsafe {
            let Some(ptr) = obj_from_bits(hay_bits).as_ptr() else {
                return MoltObject::from_int(0).bits();
            };
            let hay_bytes = std::slice::from_raw_parts(string_bytes(ptr), string_len(ptr));
            let field = &hay_bytes[start..end];
            let count = utf8_codepoint_count_cached(_py, field, None);
            MoltObject::from_int(count).bits()
        }
    })
}

/// `ord(field[idx])` for a deforested split field, from the
/// `(start, end, is_ascii)` values produced by the field-property ops. ASCII
/// fields: the codepoint at codepoint index `idx` IS the byte `hay[start + idx]`
/// (O(1), no decode). Non-ASCII: decode the `idx`-th codepoint of
/// `hay[start..end]` via `wtf8_codepoint_at` (O(idx) within the bounded field,
/// NOT a split re-scan), byte-identical to `ord(field[idx])` on the materialized
/// field — same negative-index handling and same
/// `IndexError("string index out of range")`. `start`/`end`/`is_ascii`/`idx`
/// arrive as boxed Python ints.
#[unsafe(no_mangle)]
pub extern "C" fn molt_string_split_field_ord_at_bounds(
    hay_bits: u64,
    start_bits: u64,
    end_bits: u64,
    is_ascii_bits: u64,
    index_bits: u64,
) -> u64 {
    crate::with_gil_entry_nopanic!(_py, {
        let start = to_i64(obj_from_bits(start_bits)).unwrap_or(0) as usize;
        let end = to_i64(obj_from_bits(end_bits)).unwrap_or(0) as usize;
        let is_ascii = to_i64(obj_from_bits(is_ascii_bits)).unwrap_or(0) != 0;
        let type_err = format!(
            "string indices must be integers, not '{}'",
            type_name(_py, obj_from_bits(index_bits))
        );
        let Some(idx) = index_i64_with_overflow(_py, index_bits, &type_err, None) else {
            return MoltObject::none().bits();
        };
        unsafe {
            let Some(ptr) = obj_from_bits(hay_bits).as_ptr() else {
                return MoltObject::none().bits();
            };
            let hay_bytes = std::slice::from_raw_parts(string_bytes(ptr), string_len(ptr));
            let field = &hay_bytes[start..end];
            if is_ascii {
                let len = (end - start) as i64;
                let mut i = idx;
                if i < 0 {
                    i += len;
                }
                if i < 0 || i >= len {
                    return raise_exception::<_>(_py, "IndexError", "string index out of range");
                }
                return MoltObject::from_int(field[i as usize] as i64).bits();
            }
            let len = utf8_codepoint_count_cached(_py, field, None);
            let mut i = idx;
            if i < 0 {
                i += len;
            }
            if i < 0 || i >= len {
                return raise_exception::<_>(_py, "IndexError", "string index out of range");
            }
            let Some(code) = wtf8_codepoint_at(field, i as usize) else {
                return raise_exception::<_>(_py, "IndexError", "string index out of range");
            };
            MoltObject::from_int(code.to_u32() as i64).bits()
        }
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_string_split_max(
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
                if object_type_id(hay_ptr) != TYPE_ID_STRING {
                    return MoltObject::none().bits();
                }
                let hay_bytes =
                    std::slice::from_raw_parts(string_bytes(hay_ptr), string_len(hay_ptr));
                if needle.is_none() {
                    let Ok(hay_str) = std::str::from_utf8(hay_bytes) else {
                        return MoltObject::none().bits();
                    };
                    let list_bits =
                        split_string_whitespace_to_list_maxsplit(_py, hay_str, maxsplit);
                    return list_bits.unwrap_or_else(|| MoltObject::none().bits());
                }
                let Some(needle_ptr) = needle.as_ptr() else {
                    return MoltObject::none().bits();
                };
                if object_type_id(needle_ptr) != TYPE_ID_STRING {
                    let msg = format!("must be str or None, not {}", type_name(_py, needle));
                    return raise_exception::<_>(_py, "TypeError", &msg);
                }
                let needle_bytes =
                    std::slice::from_raw_parts(string_bytes(needle_ptr), string_len(needle_ptr));
                if needle_bytes.is_empty() {
                    return raise_exception::<_>(_py, "ValueError", "empty separator");
                }
                let list_bits =
                    split_string_bytes_to_list_maxsplit(_py, hay_bytes, needle_bytes, maxsplit);
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
pub extern "C" fn molt_string_rsplit(hay_bits: u64, needle_bits: u64) -> u64 {
    crate::with_gil_entry_nopanic!(_py, {
        let maxsplit_bits = MoltObject::from_int(-1).bits();
        molt_string_rsplit_max(hay_bits, needle_bits, maxsplit_bits)
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_string_rsplit_max(
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
                if object_type_id(hay_ptr) != TYPE_ID_STRING {
                    return MoltObject::none().bits();
                }
                let hay_bytes =
                    std::slice::from_raw_parts(string_bytes(hay_ptr), string_len(hay_ptr));
                if needle.is_none() {
                    let Ok(hay_str) = std::str::from_utf8(hay_bytes) else {
                        return MoltObject::none().bits();
                    };
                    let list_bits =
                        rsplit_string_whitespace_to_list_maxsplit(_py, hay_str, maxsplit);
                    return list_bits.unwrap_or_else(|| MoltObject::none().bits());
                }
                let Some(needle_ptr) = needle.as_ptr() else {
                    return MoltObject::none().bits();
                };
                if object_type_id(needle_ptr) != TYPE_ID_STRING {
                    let msg = format!("must be str or None, not {}", type_name(_py, needle));
                    return raise_exception::<_>(_py, "TypeError", &msg);
                }
                let needle_bytes =
                    std::slice::from_raw_parts(string_bytes(needle_ptr), string_len(needle_ptr));
                if needle_bytes.is_empty() {
                    return raise_exception::<_>(_py, "ValueError", "empty separator");
                }
                let list_bits =
                    rsplit_string_bytes_to_list_maxsplit(_py, hay_bytes, needle_bytes, maxsplit);
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
