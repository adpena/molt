//! String operations — extracted from ops.rs for tree-shaking.
//!
//! Each `pub extern "C" fn molt_string_*` / `molt_str_*` is a separate
//! linker symbol so that `wasm-ld --gc-sections` can drop unused entries.

use crate::object::utf8_cache::{
    UTF8_CACHE_BLOCK, UTF8_CACHE_MIN_LEN, UTF8_COUNT_CACHE_SHARDS, UTF8_COUNT_PREFIX_MIN_LEN,
    UTF8_COUNT_TLS, Utf8CountCache, Utf8CountCacheEntry, Utf8IndexCache,
};
use crate::*;
use memchr::memmem;
use molt_obj_model::MoltObject;
use num_bigint::{BigInt, Sign};
use num_traits::ToPrimitive;
use std::collections::HashSet;
use unicode_casefold::{Locale, UnicodeCaseFold, Variant};
use unicode_ident::{is_xid_continue, is_xid_start};
use wtf8::{CodePoint, Wtf8};

use std::sync::Arc;

use super::ops::{
    bytes_ascii_capitalize, bytes_ascii_swapcase, bytes_ascii_title,
    dict_like_bits_from_ptr, format_with_spec, parse_codec_arg,
    parse_format_spec, repeat_sequence, simd_has_any_ascii_lower, simd_has_any_ascii_upper,
    simd_is_all_ascii_alnum, simd_is_all_ascii_alpha, simd_is_all_ascii_digit,
    simd_is_all_ascii_printable, simd_is_all_ascii_whitespace, slice_bounds_from_args, slice_match,
};

#[unsafe(no_mangle)]
pub extern "C" fn molt_string_find(hay_bits: u64, needle_bits: u64) -> u64 {
    crate::with_gil_entry_nopanic!(_py, {
        // ASCII fast path: skip all char-to-byte conversion overhead when both
        // haystack and needle are pure ASCII (byte index == char index).
        if let Some(hay_ptr) = obj_from_bits(hay_bits).as_ptr() {
            unsafe {
                if object_type_id(hay_ptr) == TYPE_ID_STRING {
                    let hay_len = string_len(hay_ptr);
                    let hay_bytes = std::slice::from_raw_parts(string_bytes(hay_ptr), hay_len);
                    if hay_bytes.is_ascii()
                        && let Some(needle_ptr) = obj_from_bits(needle_bits).as_ptr()
                        && object_type_id(needle_ptr) == TYPE_ID_STRING
                    {
                        let needle_bytes = std::slice::from_raw_parts(
                            string_bytes(needle_ptr),
                            string_len(needle_ptr),
                        );
                        if needle_bytes.is_ascii() {
                            let idx = bytes_find_impl(hay_bytes, needle_bytes);
                            return MoltObject::from_int(idx).bits();
                        }
                    }
                }
            }
        }
        let none_bits = MoltObject::none().bits();
        let false_bits = MoltObject::from_bool(false).bits();
        molt_string_find_slice(
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
pub extern "C" fn molt_string_rfind(hay_bits: u64, needle_bits: u64) -> u64 {
    crate::with_gil_entry_nopanic!(_py, {
        // ASCII fast path: skip all char-to-byte conversion overhead.
        if let Some(hay_ptr) = obj_from_bits(hay_bits).as_ptr() {
            unsafe {
                if object_type_id(hay_ptr) == TYPE_ID_STRING {
                    let hay_len = string_len(hay_ptr);
                    let hay_bytes = std::slice::from_raw_parts(string_bytes(hay_ptr), hay_len);
                    if hay_bytes.is_ascii()
                        && let Some(needle_ptr) = obj_from_bits(needle_bits).as_ptr()
                        && object_type_id(needle_ptr) == TYPE_ID_STRING
                    {
                        let needle_bytes = std::slice::from_raw_parts(
                            string_bytes(needle_ptr),
                            string_len(needle_ptr),
                        );
                        if needle_bytes.is_ascii() {
                            let idx = bytes_rfind_impl(hay_bytes, needle_bytes);
                            return MoltObject::from_int(idx).bits();
                        }
                    }
                }
            }
        }
        let none_bits = MoltObject::none().bits();
        let false_bits = MoltObject::from_bool(false).bits();
        molt_string_rfind_slice(
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
pub extern "C" fn molt_string_index(hay_bits: u64, needle_bits: u64) -> u64 {
    crate::with_gil_entry_nopanic!(_py, {
        let none_bits = MoltObject::none().bits();
        let false_bits = MoltObject::from_bool(false).bits();
        molt_string_index_slice(
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
pub extern "C" fn molt_string_rindex(hay_bits: u64, needle_bits: u64) -> u64 {
    crate::with_gil_entry_nopanic!(_py, {
        let none_bits = MoltObject::none().bits();
        let false_bits = MoltObject::from_bool(false).bits();
        molt_string_rindex_slice(
            hay_bits,
            needle_bits,
            none_bits,
            none_bits,
            false_bits,
            false_bits,
        )
    })
}

// ── 4-arg method-dispatch wrappers ──────────────────────────────────
// These accept (self, sub, start=None, end=None) and convert the None
// sentinel into has_start/has_end bools before delegating to the 6-arg
// _slice functions.  Used by dynamic method resolution with __defaults__
// tuples; the backends continue to call the _slice variants directly.

/// str.find(sub, start=None, end=None) — method dispatch entry point.
#[unsafe(no_mangle)]
pub extern "C" fn molt_string_find_method(
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
    molt_string_find_slice(
        hay_bits,
        needle_bits,
        start,
        end,
        MoltObject::from_int(has_start as i64).bits(),
        MoltObject::from_int(has_end as i64).bits(),
    )
}

/// str.rfind(sub, start=None, end=None)
#[unsafe(no_mangle)]
pub extern "C" fn molt_string_rfind_method(
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
    molt_string_rfind_slice(
        hay_bits,
        needle_bits,
        start,
        end,
        MoltObject::from_int(has_start as i64).bits(),
        MoltObject::from_int(has_end as i64).bits(),
    )
}

/// str.index(sub, start=None, end=None)
#[unsafe(no_mangle)]
pub extern "C" fn molt_string_index_method(
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
    molt_string_index_slice(
        hay_bits,
        needle_bits,
        start,
        end,
        MoltObject::from_int(has_start as i64).bits(),
        MoltObject::from_int(has_end as i64).bits(),
    )
}

/// str.rindex(sub, start=None, end=None)
#[unsafe(no_mangle)]
pub extern "C" fn molt_string_rindex_method(
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
    molt_string_rindex_slice(
        hay_bits,
        needle_bits,
        start,
        end,
        MoltObject::from_int(has_start as i64).bits(),
        MoltObject::from_int(has_end as i64).bits(),
    )
}

/// str.count(sub, start=None, end=None)
#[unsafe(no_mangle)]
pub extern "C" fn molt_string_count_method(
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
    molt_string_count_slice(
        hay_bits,
        needle_bits,
        start,
        end,
        MoltObject::from_int(has_start as i64).bits(),
        MoltObject::from_int(has_end as i64).bits(),
    )
}

/// str.startswith(prefix, start=None, end=None)
#[unsafe(no_mangle)]
pub extern "C" fn molt_string_startswith_method(
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
    molt_string_startswith_slice(
        hay_bits,
        needle_bits,
        start,
        end,
        MoltObject::from_int(has_start as i64).bits(),
        MoltObject::from_int(has_end as i64).bits(),
    )
}

/// str.endswith(suffix, start=None, end=None)
#[unsafe(no_mangle)]
pub extern "C" fn molt_string_endswith_method(
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
    molt_string_endswith_slice(
        hay_bits,
        needle_bits,
        start,
        end,
        MoltObject::from_int(has_start as i64).bits(),
        MoltObject::from_int(has_end as i64).bits(),
    )
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_string_find_slice(
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
                if object_type_id(hay_ptr) != TYPE_ID_STRING {
                    return MoltObject::none().bits();
                }
                let needle_ptr = match needle.as_ptr() {
                    Some(ptr) => ptr,
                    None => {
                        let msg = format!("must be str, not {}", type_name(_py, needle));
                        return raise_exception::<_>(_py, "TypeError", &msg);
                    }
                };
                if object_type_id(needle_ptr) != TYPE_ID_STRING {
                    let msg = format!("must be str, not {}", type_name(_py, needle));
                    return raise_exception::<_>(_py, "TypeError", &msg);
                }
                let hay_len = string_len(hay_ptr);
                let needle_len = string_len(needle_ptr);
                let hay_bytes = std::slice::from_raw_parts(string_bytes(hay_ptr), hay_len);
                let needle_bytes = std::slice::from_raw_parts(string_bytes(needle_ptr), needle_len);
                // Compute is_ascii() ONCE to avoid redundant full-buffer scans.
                let hay_is_ascii = hay_bytes.is_ascii();
                let total_chars = if hay_is_ascii {
                    hay_bytes.len() as i64
                } else {
                    utf8_codepoint_count_cached(_py, hay_bytes, Some(hay_ptr as usize))
                };
                let (start, end, start_raw) = slice_bounds_from_args(
                    _py,
                    start_bits,
                    end_bits,
                    has_start,
                    has_end,
                    total_chars,
                );
                if end < start {
                    return MoltObject::from_int(-1).bits();
                }
                if needle_bytes.is_empty() {
                    if start_raw > total_chars {
                        return MoltObject::from_int(-1).bits();
                    }
                    return MoltObject::from_int(start).bits();
                }
                if hay_is_ascii {
                    // ASCII fast path: byte index == char index, skip all
                    // utf8_char_to_byte_index_cached calls.
                    let start_byte = (start as usize).min(hay_bytes.len());
                    let end_byte = (end as usize).min(hay_bytes.len());
                    let slice = &hay_bytes[start_byte..end_byte];
                    let idx = bytes_find_impl(slice, needle_bytes);
                    if idx < 0 {
                        return MoltObject::from_int(-1).bits();
                    }
                    return MoltObject::from_int(start + idx).bits();
                }
                let start_byte =
                    utf8_char_to_byte_index_cached(_py, hay_bytes, start, Some(hay_ptr as usize));
                let end_byte =
                    utf8_char_to_byte_index_cached(_py, hay_bytes, end, Some(hay_ptr as usize))
                        .min(hay_bytes.len());
                let slice = &hay_bytes[start_byte..end_byte];
                let idx = bytes_find_impl(slice, needle_bytes);
                if idx < 0 {
                    return MoltObject::from_int(-1).bits();
                }
                let byte_idx = start_byte + idx as usize;
                let char_idx = utf8_byte_to_char_index_cached(
                    _py,
                    hay_bytes,
                    byte_idx,
                    Some(hay_ptr as usize),
                );
                MoltObject::from_int(char_idx).bits()
            }
        } else {
            MoltObject::none().bits()
        }
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_string_rfind_slice(
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
                if object_type_id(hay_ptr) != TYPE_ID_STRING {
                    return MoltObject::none().bits();
                }
                let needle_ptr = match needle.as_ptr() {
                    Some(ptr) => ptr,
                    None => {
                        let msg = format!("must be str, not {}", type_name(_py, needle));
                        return raise_exception::<_>(_py, "TypeError", &msg);
                    }
                };
                if object_type_id(needle_ptr) != TYPE_ID_STRING {
                    let msg = format!("must be str, not {}", type_name(_py, needle));
                    return raise_exception::<_>(_py, "TypeError", &msg);
                }
                let hay_len = string_len(hay_ptr);
                let needle_len = string_len(needle_ptr);
                let hay_bytes = std::slice::from_raw_parts(string_bytes(hay_ptr), hay_len);
                let needle_bytes = std::slice::from_raw_parts(string_bytes(needle_ptr), needle_len);
                // Compute is_ascii() ONCE to avoid redundant full-buffer scans.
                let hay_is_ascii = hay_bytes.is_ascii();
                let total_chars = if hay_is_ascii {
                    hay_bytes.len() as i64
                } else {
                    utf8_codepoint_count_cached(_py, hay_bytes, Some(hay_ptr as usize))
                };
                let (start, end, start_raw) = slice_bounds_from_args(
                    _py,
                    start_bits,
                    end_bits,
                    has_start,
                    has_end,
                    total_chars,
                );
                if end < start {
                    return MoltObject::from_int(-1).bits();
                }
                if needle_bytes.is_empty() {
                    if start_raw > total_chars {
                        return MoltObject::from_int(-1).bits();
                    }
                    return MoltObject::from_int(end).bits();
                }
                if hay_is_ascii {
                    // ASCII fast path: byte index == char index.
                    let start_byte = (start as usize).min(hay_bytes.len());
                    let end_byte = (end as usize).min(hay_bytes.len());
                    let slice = &hay_bytes[start_byte..end_byte];
                    let idx = bytes_rfind_impl(slice, needle_bytes);
                    if idx < 0 {
                        return MoltObject::from_int(-1).bits();
                    }
                    return MoltObject::from_int(start + idx).bits();
                }
                let start_byte =
                    utf8_char_to_byte_index_cached(_py, hay_bytes, start, Some(hay_ptr as usize));
                let end_byte =
                    utf8_char_to_byte_index_cached(_py, hay_bytes, end, Some(hay_ptr as usize))
                        .min(hay_bytes.len());
                let slice = &hay_bytes[start_byte..end_byte];
                let idx = bytes_rfind_impl(slice, needle_bytes);
                if idx < 0 {
                    return MoltObject::from_int(-1).bits();
                }
                let byte_idx = start_byte + idx as usize;
                let char_idx = utf8_byte_to_char_index_cached(
                    _py,
                    hay_bytes,
                    byte_idx,
                    Some(hay_ptr as usize),
                );
                MoltObject::from_int(char_idx).bits()
            }
        } else {
            MoltObject::none().bits()
        }
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_string_index_slice(
    hay_bits: u64,
    needle_bits: u64,
    start_bits: u64,
    end_bits: u64,
    has_start_bits: u64,
    has_end_bits: u64,
) -> u64 {
    crate::with_gil_entry_nopanic!(_py, {
        let out_bits = molt_string_find_slice(
            hay_bits,
            needle_bits,
            start_bits,
            end_bits,
            has_start_bits,
            has_end_bits,
        );
        match to_i64(obj_from_bits(out_bits)) {
            Some(idx) if idx >= 0 => out_bits,
            Some(_) => raise_exception::<_>(_py, "ValueError", "substring not found"),
            None => out_bits,
        }
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_string_rindex_slice(
    hay_bits: u64,
    needle_bits: u64,
    start_bits: u64,
    end_bits: u64,
    has_start_bits: u64,
    has_end_bits: u64,
) -> u64 {
    crate::with_gil_entry_nopanic!(_py, {
        let out_bits = molt_string_rfind_slice(
            hay_bits,
            needle_bits,
            start_bits,
            end_bits,
            has_start_bits,
            has_end_bits,
        );
        match to_i64(obj_from_bits(out_bits)) {
            Some(idx) if idx >= 0 => out_bits,
            Some(_) => raise_exception::<_>(_py, "ValueError", "substring not found"),
            None => out_bits,
        }
    })
}

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
pub extern "C" fn molt_string_startswith(hay_bits: u64, needle_bits: u64) -> u64 {
    crate::with_gil_entry_nopanic!(_py, {
        let hay = obj_from_bits(hay_bits);
        let needle = obj_from_bits(needle_bits);
        let Some(hay_ptr) = hay.as_ptr() else {
            return MoltObject::from_bool(false).bits();
        };
        unsafe {
            if object_type_id(hay_ptr) != TYPE_ID_STRING {
                return MoltObject::from_bool(false).bits();
            }
            let hay_bytes = std::slice::from_raw_parts(string_bytes(hay_ptr), string_len(hay_ptr));
            // Single-prefix fast path (most common case)
            if let Some(needle_ptr) = needle.as_ptr() {
                let needle_type = object_type_id(needle_ptr);
                if needle_type == TYPE_ID_STRING {
                    let needle_bytes =
                        std::slice::from_raw_parts(string_bytes(needle_ptr), string_len(needle_ptr));
                    return MoltObject::from_bool(hay_bytes.starts_with(needle_bytes)).bits();
                }
                if needle_type == TYPE_ID_TUPLE {
                    let elems = seq_vec_ref(needle_ptr);
                    for &elem_bits in elems.iter() {
                        let elem = obj_from_bits(elem_bits);
                        if let Some(elem_ptr) = elem.as_ptr()
                            && object_type_id(elem_ptr) == TYPE_ID_STRING
                        {
                            let elem_bytes = std::slice::from_raw_parts(
                                string_bytes(elem_ptr),
                                string_len(elem_ptr),
                            );
                            if hay_bytes.starts_with(elem_bytes) {
                                return MoltObject::from_bool(true).bits();
                            }
                        }
                    }
                    return MoltObject::from_bool(false).bits();
                }
            }
            // Non-str, non-tuple needle: delegate to slice path for error handling
            let none_bits = MoltObject::none().bits();
            let false_bits = MoltObject::from_bool(false).bits();
            molt_string_startswith_slice(
                hay_bits,
                needle_bits,
                none_bits,
                none_bits,
                false_bits,
                false_bits,
            )
        }
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_string_endswith(hay_bits: u64, needle_bits: u64) -> u64 {
    crate::with_gil_entry_nopanic!(_py, {
        let hay = obj_from_bits(hay_bits);
        let needle = obj_from_bits(needle_bits);
        let Some(hay_ptr) = hay.as_ptr() else {
            return MoltObject::from_bool(false).bits();
        };
        unsafe {
            if object_type_id(hay_ptr) != TYPE_ID_STRING {
                return MoltObject::from_bool(false).bits();
            }
            let hay_bytes = std::slice::from_raw_parts(string_bytes(hay_ptr), string_len(hay_ptr));
            if let Some(needle_ptr) = needle.as_ptr() {
                let needle_type = object_type_id(needle_ptr);
                if needle_type == TYPE_ID_STRING {
                    let needle_bytes =
                        std::slice::from_raw_parts(string_bytes(needle_ptr), string_len(needle_ptr));
                    return MoltObject::from_bool(hay_bytes.ends_with(needle_bytes)).bits();
                }
                if needle_type == TYPE_ID_TUPLE {
                    let elems = seq_vec_ref(needle_ptr);
                    for &elem_bits in elems.iter() {
                        let elem = obj_from_bits(elem_bits);
                        if let Some(elem_ptr) = elem.as_ptr()
                            && object_type_id(elem_ptr) == TYPE_ID_STRING
                        {
                            let elem_bytes = std::slice::from_raw_parts(
                                string_bytes(elem_ptr),
                                string_len(elem_ptr),
                            );
                            if hay_bytes.ends_with(elem_bytes) {
                                return MoltObject::from_bool(true).bits();
                            }
                        }
                    }
                    return MoltObject::from_bool(false).bits();
                }
            }
            let none_bits = MoltObject::none().bits();
            let false_bits = MoltObject::from_bool(false).bits();
            molt_string_endswith_slice(
                hay_bits,
                needle_bits,
                none_bits,
                none_bits,
                false_bits,
                false_bits,
            )
        }
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_string_startswith_slice(
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
            if object_type_id(hay_ptr) != TYPE_ID_STRING {
                return MoltObject::none().bits();
            }
            let hay_bytes = std::slice::from_raw_parts(string_bytes(hay_ptr), string_len(hay_ptr));
            // Compute is_ascii() ONCE to avoid redundant full-buffer scans.
            let hay_is_ascii = hay_bytes.is_ascii();
            let total_chars = if hay_is_ascii {
                hay_bytes.len() as i64
            } else {
                utf8_codepoint_count_cached(_py, hay_bytes, Some(hay_ptr as usize))
            };
            let (start, end, start_raw) =
                slice_bounds_from_args(_py, start_bits, end_bits, has_start, has_end, total_chars);
            if end < start {
                return MoltObject::from_bool(false).bits();
            }
            let (start_byte, end_byte) = if hay_is_ascii {
                (
                    (start as usize).min(hay_bytes.len()),
                    (end as usize).min(hay_bytes.len()),
                )
            } else {
                (
                    utf8_char_to_byte_index_cached(_py, hay_bytes, start, Some(hay_ptr as usize)),
                    utf8_char_to_byte_index_cached(_py, hay_bytes, end, Some(hay_ptr as usize))
                        .min(hay_bytes.len()),
                )
            };
            let slice = &hay_bytes[start_byte..end_byte];
            if let Some(needle_ptr) = needle.as_ptr() {
                let needle_type = object_type_id(needle_ptr);
                if needle_type == TYPE_ID_STRING {
                    let needle_bytes = std::slice::from_raw_parts(
                        string_bytes(needle_ptr),
                        string_len(needle_ptr),
                    );
                    let ok = slice_match(slice, needle_bytes, start_raw, total_chars, false);
                    return MoltObject::from_bool(ok).bits();
                }
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
                                    "tuple for startswith must only contain str, not {}",
                                    type_name(_py, elem)
                                );
                                return raise_exception::<_>(_py, "TypeError", &msg);
                            }
                        };
                        if object_type_id(elem_ptr) != TYPE_ID_STRING {
                            let msg = format!(
                                "tuple for startswith must only contain str, not {}",
                                type_name(_py, elem)
                            );
                            return raise_exception::<_>(_py, "TypeError", &msg);
                        }
                        let needle_bytes = std::slice::from_raw_parts(
                            string_bytes(elem_ptr),
                            string_len(elem_ptr),
                        );
                        if slice_match(slice, needle_bytes, start_raw, total_chars, false) {
                            return MoltObject::from_bool(true).bits();
                        }
                    }
                    return MoltObject::from_bool(false).bits();
                }
            }
            let msg = format!(
                "startswith first arg must be str or a tuple of str, not {}",
                type_name(_py, needle)
            );
            raise_exception::<_>(_py, "TypeError", &msg)
        }
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_string_endswith_slice(
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
            if object_type_id(hay_ptr) != TYPE_ID_STRING {
                return MoltObject::none().bits();
            }
            let hay_bytes = std::slice::from_raw_parts(string_bytes(hay_ptr), string_len(hay_ptr));
            // Compute is_ascii() ONCE to avoid redundant full-buffer scans.
            let hay_is_ascii = hay_bytes.is_ascii();
            let total_chars = if hay_is_ascii {
                hay_bytes.len() as i64
            } else {
                utf8_codepoint_count_cached(_py, hay_bytes, Some(hay_ptr as usize))
            };
            let (start, end, start_raw) =
                slice_bounds_from_args(_py, start_bits, end_bits, has_start, has_end, total_chars);
            if end < start {
                return MoltObject::from_bool(false).bits();
            }
            let (start_byte, end_byte) = if hay_is_ascii {
                (
                    (start as usize).min(hay_bytes.len()),
                    (end as usize).min(hay_bytes.len()),
                )
            } else {
                (
                    utf8_char_to_byte_index_cached(_py, hay_bytes, start, Some(hay_ptr as usize)),
                    utf8_char_to_byte_index_cached(_py, hay_bytes, end, Some(hay_ptr as usize))
                        .min(hay_bytes.len()),
                )
            };
            let slice = &hay_bytes[start_byte..end_byte];
            if let Some(needle_ptr) = needle.as_ptr() {
                let needle_type = object_type_id(needle_ptr);
                if needle_type == TYPE_ID_STRING {
                    let needle_bytes = std::slice::from_raw_parts(
                        string_bytes(needle_ptr),
                        string_len(needle_ptr),
                    );
                    let ok = slice_match(slice, needle_bytes, start_raw, total_chars, true);
                    return MoltObject::from_bool(ok).bits();
                }
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
                                    "tuple for endswith must only contain str, not {}",
                                    type_name(_py, elem)
                                );
                                return raise_exception::<_>(_py, "TypeError", &msg);
                            }
                        };
                        if object_type_id(elem_ptr) != TYPE_ID_STRING {
                            let msg = format!(
                                "tuple for endswith must only contain str, not {}",
                                type_name(_py, elem)
                            );
                            return raise_exception::<_>(_py, "TypeError", &msg);
                        }
                        let needle_bytes = std::slice::from_raw_parts(
                            string_bytes(elem_ptr),
                            string_len(elem_ptr),
                        );
                        if slice_match(slice, needle_bytes, start_raw, total_chars, true) {
                            return MoltObject::from_bool(true).bits();
                        }
                    }
                    return MoltObject::from_bool(false).bits();
                }
            }
            let msg = format!(
                "endswith first arg must be str or a tuple of str, not {}",
                type_name(_py, needle)
            );
            raise_exception::<_>(_py, "TypeError", &msg)
        }
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_string_count(hay_bits: u64, needle_bits: u64) -> u64 {
    crate::with_gil_entry_nopanic!(_py, {
        let hay = obj_from_bits(hay_bits);
        let needle = obj_from_bits(needle_bits);
        let Some(hay_ptr) = hay.as_ptr() else {
            return MoltObject::none().bits();
        };
        unsafe {
            if object_type_id(hay_ptr) != TYPE_ID_STRING {
                return MoltObject::none().bits();
            }
            let needle_ptr = match needle.as_ptr() {
                Some(ptr) => ptr,
                None => {
                    let msg = format!(
                        "count() argument 1 must be str, not {}",
                        type_name(_py, needle)
                    );
                    return raise_exception::<_>(_py, "TypeError", &msg);
                }
            };
            if object_type_id(needle_ptr) != TYPE_ID_STRING {
                let msg = format!(
                    "count() argument 1 must be str, not {}",
                    type_name(_py, needle)
                );
                return raise_exception::<_>(_py, "TypeError", &msg);
            }
            let hay_bytes = std::slice::from_raw_parts(string_bytes(hay_ptr), string_len(hay_ptr));
            let needle_bytes =
                std::slice::from_raw_parts(string_bytes(needle_ptr), string_len(needle_ptr));
            let count = if needle_bytes.is_empty() {
                // For empty needle, count == len(str) + 1. Use len directly for ASCII.
                if hay_bytes.is_ascii() {
                    hay_bytes.len() as i64 + 1
                } else {
                    utf8_codepoint_count_cached(_py, hay_bytes, Some(hay_ptr as usize)) + 1
                }
            } else if let Some(cache) = utf8_count_cache_lookup(_py, hay_ptr as usize, needle_bytes)
            {
                cache.count
            } else {
                profile_hit(_py, &runtime_state(_py).string_count_cache_miss);
                let count = bytes_count_impl(hay_bytes, needle_bytes);
                utf8_count_cache_store(
                    _py,
                    hay_ptr as usize,
                    hay_bytes,
                    needle_bytes,
                    count,
                    Vec::new(),
                );
                count
            };
            MoltObject::from_int(count).bits()
        }
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_string_count_slice(
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
            if object_type_id(hay_ptr) != TYPE_ID_STRING {
                return MoltObject::none().bits();
            }
            let needle_ptr = match needle.as_ptr() {
                Some(ptr) => ptr,
                None => {
                    let msg = format!("must be str, not {}", type_name(_py, needle));
                    return raise_exception::<_>(_py, "TypeError", &msg);
                }
            };
            if object_type_id(needle_ptr) != TYPE_ID_STRING {
                let msg = format!("must be str, not {}", type_name(_py, needle));
                return raise_exception::<_>(_py, "TypeError", &msg);
            }
            let hay_bytes = std::slice::from_raw_parts(string_bytes(hay_ptr), string_len(hay_ptr));
            let needle_bytes =
                std::slice::from_raw_parts(string_bytes(needle_ptr), string_len(needle_ptr));
            // Compute is_ascii() ONCE to avoid redundant full-buffer scans.
            let hay_is_ascii = hay_bytes.is_ascii();
            let total_chars = if hay_is_ascii {
                hay_bytes.len() as i64
            } else {
                utf8_codepoint_count_cached(_py, hay_bytes, Some(hay_ptr as usize))
            };
            let (start, end, start_raw) =
                slice_bounds_from_args(_py, start_bits, end_bits, has_start, has_end, total_chars);
            if end < start {
                return MoltObject::from_int(0).bits();
            }
            if needle_bytes.is_empty() {
                if start_raw > total_chars {
                    return MoltObject::from_int(0).bits();
                }
                let count = end - start + 1;
                return MoltObject::from_int(count).bits();
            }
            let (start_byte, end_byte) = if hay_is_ascii {
                (
                    (start as usize).min(hay_bytes.len()),
                    (end as usize).min(hay_bytes.len()),
                )
            } else {
                (
                    utf8_char_to_byte_index_cached(_py, hay_bytes, start, Some(hay_ptr as usize)),
                    utf8_char_to_byte_index_cached(_py, hay_bytes, end, Some(hay_ptr as usize))
                        .min(hay_bytes.len()),
                )
            };
            if let Some(cache) = utf8_count_cache_lookup(_py, hay_ptr as usize, needle_bytes) {
                let cache =
                    utf8_count_cache_upgrade_prefix(_py, hay_ptr as usize, &cache, hay_bytes);
                let count = utf8_count_cache_count_slice(&cache, hay_bytes, start_byte, end_byte);
                return MoltObject::from_int(count).bits();
            }
            let slice = &hay_bytes[start_byte..end_byte];
            let count = bytes_count_impl(slice, needle_bytes);
            MoltObject::from_int(count).bits()
        }
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_string_join(sep_bits: u64, items_bits: u64) -> u64 {
    crate::with_gil_entry_nopanic!(_py, {
        let sep = obj_from_bits(sep_bits);
        let items = obj_from_bits(items_bits);
        let sep_ptr = match sep.as_ptr() {
            Some(ptr) => ptr,
            None => return MoltObject::none().bits(),
        };
        unsafe {
            if object_type_id(sep_ptr) != TYPE_ID_STRING {
                return raise_exception::<_>(_py, "TypeError", "join expects a str separator");
            }
            let sep_bytes = std::slice::from_raw_parts(string_bytes(sep_ptr), string_len(sep_ptr));
            let mut total_len = 0usize;
            struct StringPart {
                bits: u64,
                data: *const u8,
                len: usize,
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
                                    "sequence item {idx}: expected str instance, {} found",
                                    type_name(_py, elem_obj)
                                );
                                return raise_exception::<_>(_py, "TypeError", &msg);
                            }
                        };
                        if object_type_id(elem_ptr) != TYPE_ID_STRING {
                            let msg = format!(
                                "sequence item {idx}: expected str instance, {} found",
                                type_name(_py, elem_obj)
                            );
                            return raise_exception::<_>(_py, "TypeError", &msg);
                        }
                        let len = string_len(elem_ptr);
                        total_len += len;
                        let data = string_bytes(elem_ptr);
                        if idx == 0 {
                            first_bits = elem_bits;
                            first_data = data;
                            first_len = len;
                        } else if elem_bits != first_bits {
                            all_same = false;
                        }
                        parts.push(StringPart {
                            bits: elem_bits,
                            data,
                            len,
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
                                "sequence item {idx}: expected str instance, {} found",
                                type_name(_py, elem_obj)
                            );
                            return raise_exception::<_>(_py, "TypeError", &msg);
                        }
                    };
                    if object_type_id(elem_ptr) != TYPE_ID_STRING {
                        for bits in owned_bits.iter().copied() {
                            dec_ref_bits(_py, bits);
                        }
                        let msg = format!(
                            "sequence item {idx}: expected str instance, {} found",
                            type_name(_py, elem_obj)
                        );
                        return raise_exception::<_>(_py, "TypeError", &msg);
                    }
                    let len = string_len(elem_ptr);
                    total_len += len;
                    let data = string_bytes(elem_ptr);
                    if idx == 0 {
                        first_bits = elem_bits;
                        first_data = data;
                        first_len = len;
                    } else if elem_bits != first_bits {
                        all_same = false;
                    }
                    parts.push(StringPart {
                        bits: elem_bits,
                        data,
                        len,
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
            let mut result_bits = None;
            if parts.len() == 1 && !iter_owned {
                inc_ref_bits(_py, parts[0].bits);
                result_bits = Some(parts[0].bits);
            }
            if let Some(bits) = result_bits {
                return bits;
            }
            let out_ptr = alloc_bytes_like_with_len(_py, total_len, TYPE_ID_STRING);
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

#[derive(Copy, Clone)]
enum FormatContext {
    FormatString,
    FormatSpec,
}

struct FormatState {
    next_auto: usize,
    used_auto: bool,
    used_manual: bool,
    allow_positional: bool,
    mapping_mode: bool,
}

struct FormatField<'a> {
    field_name: &'a str,
    conversion: Option<char>,
    format_spec: &'a str,
}

fn format_raise_value_error_str(_py: &PyToken<'_>, msg: &str) -> Option<String> {
    raise_exception::<_>(_py, "ValueError", msg)
}

fn format_raise_value_error_bits(_py: &PyToken<'_>, msg: &str) -> Option<u64> {
    raise_exception::<_>(_py, "ValueError", msg)
}

fn format_raise_index_error_bits(_py: &PyToken<'_>, msg: &str) -> Option<u64> {
    raise_exception::<_>(_py, "IndexError", msg)
}

fn parse_format_field<'a>(
    _py: &PyToken<'_>,
    text: &'a str,
    start: usize,
    context: FormatContext,
) -> Option<(FormatField<'a>, usize)> {
    let bytes = text.as_bytes();
    let len = bytes.len();
    if start >= len {
        let msg = match context {
            FormatContext::FormatSpec => "unmatched '{' in format spec",
            FormatContext::FormatString => "Single '{' encountered in format string",
        };
        return raise_exception::<_>(_py, "ValueError", msg);
    }
    let mut idx = start;
    while idx < len {
        let b = bytes[idx];
        if b == b'!' || b == b':' || b == b'}' {
            break;
        }
        idx += 1;
    }
    let field_name = &text[start..idx];
    let mut conversion = None;
    if idx < len && bytes[idx] == b'!' {
        idx += 1;
        if idx >= len {
            let msg = match context {
                FormatContext::FormatSpec => "unmatched '{' in format spec",
                FormatContext::FormatString => "expected '}' before end of string",
            };
            return raise_exception::<_>(_py, "ValueError", msg);
        }
        let conv = bytes[idx] as char;
        if conv != 'r' && conv != 's' && conv != 'a' {
            if conv == '}' {
                return raise_exception::<_>(_py, "ValueError", "unmatched '{' in format spec");
            }
            let msg = format!("Unknown conversion specifier {conv}");
            return raise_exception::<_>(_py, "ValueError", &msg);
        }
        conversion = Some(conv);
        idx += 1;
    }
    let mut format_spec = "";
    if idx < len && bytes[idx] == b':' {
        idx += 1;
        let spec_start = idx;
        while idx < len {
            let b = bytes[idx];
            if b == b'{' {
                if idx + 1 < len && bytes[idx + 1] == b'{' {
                    idx += 2;
                    continue;
                }
                let (_, next_idx) =
                    parse_format_field(_py, text, idx + 1, FormatContext::FormatSpec)?;
                idx = next_idx;
                continue;
            }
            if b == b'}' {
                if idx + 1 < len && bytes[idx + 1] == b'}' {
                    idx += 2;
                    continue;
                }
                break;
            }
            idx += 1;
        }
        if idx >= len {
            let msg = match context {
                FormatContext::FormatSpec => "unmatched '{' in format spec",
                FormatContext::FormatString => "expected '}' before end of string",
            };
            return raise_exception::<_>(_py, "ValueError", msg);
        }
        format_spec = &text[spec_start..idx];
    }
    if idx >= len || bytes[idx] != b'}' {
        let msg = match context {
            FormatContext::FormatSpec => "unmatched '{' in format spec",
            FormatContext::FormatString => "expected '}' before end of string",
        };
        return raise_exception::<_>(_py, "ValueError", msg);
    }
    let next_idx = idx + 1;
    Some((
        FormatField {
            field_name,
            conversion,
            format_spec,
        },
        next_idx,
    ))
}

fn format_string_impl(
    _py: &PyToken<'_>,
    text: &str,
    args: &[u64],
    kwargs_bits: u64,
    state: &mut FormatState,
    context: FormatContext,
) -> Option<String> {
    let bytes = text.as_bytes();
    let len = bytes.len();
    let mut out = String::with_capacity(text.len());
    let mut idx = 0usize;
    while idx < len {
        let b = bytes[idx];
        if b == b'{' {
            if idx + 1 < len && bytes[idx + 1] == b'{' {
                out.push('{');
                idx += 2;
                continue;
            }
            let (field, next_idx) = parse_format_field(_py, text, idx + 1, context)?;
            let rendered = format_field(_py, field, args, kwargs_bits, state)?;
            out.push_str(&rendered);
            idx = next_idx;
            continue;
        }
        if b == b'}' {
            if idx + 1 < len && bytes[idx + 1] == b'}' {
                out.push('}');
                idx += 2;
                continue;
            }
            return format_raise_value_error_str(_py, "Single '}' encountered in format string");
        }
        let start = idx;
        idx += 1;
        while idx < len && bytes[idx] != b'{' && bytes[idx] != b'}' {
            idx += 1;
        }
        out.push_str(&text[start..idx]);
    }
    Some(out)
}

fn resolve_format_field(
    _py: &PyToken<'_>,
    field_name: &str,
    args: &[u64],
    kwargs_bits: u64,
    state: &mut FormatState,
) -> Option<u64> {
    let bytes = field_name.as_bytes();
    let len = bytes.len();
    let mut idx = 0usize;
    while idx < len && bytes[idx] != b'.' && bytes[idx] != b'[' {
        idx += 1;
    }
    let base = &field_name[..idx];
    let base_bits = if base.is_empty() {
        if !state.allow_positional {
            return format_raise_value_error_bits(_py, "Format string contains positional fields");
        }
        if state.used_manual {
            return format_raise_value_error_bits(
                _py,
                "cannot switch from manual field specification to automatic field numbering",
            );
        }
        state.used_auto = true;
        let index = state.next_auto;
        state.next_auto += 1;
        if index >= args.len() {
            let msg = format!("Replacement index {index} out of range for positional args tuple");
            return format_raise_index_error_bits(_py, &msg);
        }
        args[index]
    } else if base.as_bytes().iter().all(|b| b.is_ascii_digit()) {
        if !state.allow_positional {
            return format_raise_value_error_bits(_py, "Format string contains positional fields");
        }
        if state.used_auto {
            return format_raise_value_error_bits(
                _py,
                "cannot switch from automatic field numbering to manual field specification",
            );
        }
        state.used_manual = true;
        let index = match base.parse::<usize>() {
            Ok(val) => val,
            Err(_) => {
                return format_raise_value_error_bits(
                    _py,
                    "Too many decimal digits in format string",
                );
            }
        };
        if index >= args.len() {
            let msg = format!("Replacement index {base} out of range for positional args tuple");
            return format_raise_index_error_bits(_py, &msg);
        }
        args[index]
    } else {
        if state.used_auto {
            return format_raise_value_error_bits(
                _py,
                "cannot switch from automatic field numbering to manual field specification",
            );
        }
        state.used_manual = true;
        let key_ptr = alloc_string(_py, base.as_bytes());
        if key_ptr.is_null() {
            return None;
        }
        let key_bits = MoltObject::from_ptr(key_ptr).bits();
        let val_bits = if state.mapping_mode {
            let looked_up = molt_index(kwargs_bits, key_bits);
            if exception_pending(_py) {
                dec_ref_bits(_py, key_bits);
                return None;
            }
            Some(looked_up)
        } else {
            let kwargs_obj = obj_from_bits(kwargs_bits);
            let mut looked_up = None;
            if let Some(dict_ptr) = kwargs_obj.as_ptr() {
                unsafe {
                    if object_type_id(dict_ptr) == TYPE_ID_DICT {
                        looked_up = dict_get_in_place(_py, dict_ptr, key_bits);
                    }
                }
            }
            if looked_up.is_none() {
                raise_key_error_with_key::<()>(_py, key_bits);
                dec_ref_bits(_py, key_bits);
                return None;
            }
            looked_up
        };
        dec_ref_bits(_py, key_bits);
        val_bits.unwrap()
    };
    let mut current_bits = base_bits;
    while idx < len {
        if bytes[idx] == b'.' {
            idx += 1;
            if idx >= len {
                return format_raise_value_error_bits(_py, "Empty attribute in format string");
            }
            let start = idx;
            while idx < len && bytes[idx] != b'.' && bytes[idx] != b'[' {
                idx += 1;
            }
            let attr = &field_name[start..idx];
            if attr.is_empty() {
                return format_raise_value_error_bits(_py, "Empty attribute in format string");
            }
            let attr_ptr = alloc_string(_py, attr.as_bytes());
            if attr_ptr.is_null() {
                return None;
            }
            let attr_bits = MoltObject::from_ptr(attr_ptr).bits();
            current_bits = molt_get_attr_name(current_bits, attr_bits);
            dec_ref_bits(_py, attr_bits);
            if exception_pending(_py) {
                return None;
            }
            continue;
        }
        if bytes[idx] == b'[' {
            idx += 1;
            if idx >= len {
                return format_raise_value_error_bits(_py, "expected '}' before end of string");
            }
            let start = idx;
            while idx < len && bytes[idx] != b']' {
                idx += 1;
            }
            if idx >= len {
                return format_raise_value_error_bits(_py, "expected '}' before end of string");
            }
            let key = &field_name[start..idx];
            if key.is_empty() {
                return format_raise_value_error_bits(_py, "Empty attribute in format string");
            }
            idx += 1;
            if idx < len && bytes[idx] != b'.' && bytes[idx] != b'[' {
                return format_raise_value_error_bits(
                    _py,
                    "Only '.' or '[' may follow ']' in format field specifier",
                );
            }
            let (key_bits, drop_key) = if key.as_bytes().iter().all(|b| b.is_ascii_digit()) {
                let val = match key.parse::<i64>() {
                    Ok(num) => num,
                    Err(_) => {
                        return format_raise_value_error_bits(
                            _py,
                            "Too many decimal digits in format string",
                        );
                    }
                };
                (MoltObject::from_int(val).bits(), false)
            } else {
                let key_ptr = alloc_string(_py, key.as_bytes());
                if key_ptr.is_null() {
                    return None;
                }
                (MoltObject::from_ptr(key_ptr).bits(), true)
            };
            current_bits = molt_index(current_bits, key_bits);
            if drop_key {
                dec_ref_bits(_py, key_bits);
            }
            if exception_pending(_py) {
                return None;
            }
            continue;
        }
        break;
    }
    Some(current_bits)
}

fn format_field(
    _py: &PyToken<'_>,
    field: FormatField,
    args: &[u64],
    kwargs_bits: u64,
    state: &mut FormatState,
) -> Option<String> {
    let mut value_bits = resolve_format_field(_py, field.field_name, args, kwargs_bits, state)?;
    if exception_pending(_py) {
        return None;
    }
    let mut drop_value = false;
    if let Some(conv) = field.conversion {
        value_bits = match conv {
            'r' => {
                drop_value = true;
                molt_repr_from_obj(value_bits)
            }
            's' => {
                drop_value = true;
                molt_str_from_obj(value_bits)
            }
            'a' => {
                drop_value = true;
                molt_ascii_from_obj(value_bits)
            }
            _ => value_bits,
        };
        if exception_pending(_py) {
            return None;
        }
    }
    let spec_text = if field.format_spec.is_empty() {
        String::new()
    } else {
        format_string_impl(
            _py,
            field.format_spec,
            args,
            kwargs_bits,
            state,
            FormatContext::FormatSpec,
        )?
    };
    let spec_ptr = alloc_string(_py, spec_text.as_bytes());
    if spec_ptr.is_null() {
        return None;
    }
    let spec_bits = MoltObject::from_ptr(spec_ptr).bits();
    let formatted_bits = molt_format_builtin(value_bits, spec_bits);
    dec_ref_bits(_py, spec_bits);
    if drop_value {
        dec_ref_bits(_py, value_bits);
    }
    if exception_pending(_py) {
        return None;
    }
    let formatted_obj = obj_from_bits(formatted_bits);
    let rendered =
        string_obj_to_owned(formatted_obj).unwrap_or_else(|| format_obj_str(_py, formatted_obj));
    dec_ref_bits(_py, formatted_bits);
    Some(rendered)
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_string_format_method(
    self_bits: u64,
    args_bits: u64,
    kwargs_bits: u64,
) -> u64 {
    crate::with_gil_entry_nopanic!(_py, {
        let self_obj = obj_from_bits(self_bits);
        let Some(self_ptr) = self_obj.as_ptr() else {
            return MoltObject::none().bits();
        };
        unsafe {
            if object_type_id(self_ptr) != TYPE_ID_STRING {
                return raise_exception::<_>(_py, "TypeError", "format requires a string");
            }
            let text = string_obj_to_owned(self_obj).unwrap_or_default();
            let args_obj = obj_from_bits(args_bits);
            let Some(args_ptr) = args_obj.as_ptr() else {
                return raise_exception::<_>(_py, "TypeError", "format arguments must be a tuple");
            };
            if object_type_id(args_ptr) != TYPE_ID_TUPLE {
                return raise_exception::<_>(_py, "TypeError", "format arguments must be a tuple");
            }
            let args_vec = seq_vec_ref(args_ptr);
            let mut state = FormatState {
                next_auto: 0,
                used_auto: false,
                used_manual: false,
                allow_positional: true,
                mapping_mode: false,
            };
            let Some(rendered) = format_string_impl(
                _py,
                &text,
                args_vec.as_slice(),
                kwargs_bits,
                &mut state,
                FormatContext::FormatString,
            ) else {
                return MoltObject::none().bits();
            };
            let out_ptr = alloc_string(_py, rendered.as_bytes());
            if out_ptr.is_null() {
                return MoltObject::none().bits();
            }
            MoltObject::from_ptr(out_ptr).bits()
        }
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_string_format_map(self_bits: u64, mapping_bits: u64) -> u64 {
    crate::with_gil_entry_nopanic!(_py, {
        let self_obj = obj_from_bits(self_bits);
        let Some(self_ptr) = self_obj.as_ptr() else {
            return MoltObject::none().bits();
        };
        unsafe {
            if object_type_id(self_ptr) != TYPE_ID_STRING {
                return raise_exception::<_>(_py, "TypeError", "format_map requires a string");
            }
            let text = string_obj_to_owned(self_obj).unwrap_or_default();
            let args_vec: [u64; 0] = [];
            let mut state = FormatState {
                next_auto: 0,
                used_auto: false,
                used_manual: false,
                allow_positional: false,
                mapping_mode: true,
            };
            let Some(rendered) = format_string_impl(
                _py,
                &text,
                args_vec.as_slice(),
                mapping_bits,
                &mut state,
                FormatContext::FormatString,
            ) else {
                return MoltObject::none().bits();
            };
            let out_ptr = alloc_string(_py, rendered.as_bytes());
            if out_ptr.is_null() {
                return MoltObject::none().bits();
            }
            MoltObject::from_ptr(out_ptr).bits()
        }
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_string_format(val_bits: u64, spec_bits: u64) -> u64 {
    crate::with_gil_entry_nopanic!(_py, {
        let spec_obj = obj_from_bits(spec_bits);
        let spec_ptr = match spec_obj.as_ptr() {
            Some(ptr) => ptr,
            None => return raise_exception::<_>(_py, "TypeError", "format spec must be a str"),
        };
        unsafe {
            if object_type_id(spec_ptr) != TYPE_ID_STRING {
                return raise_exception::<_>(_py, "TypeError", "format spec must be a str");
            }
            let spec_bytes =
                std::slice::from_raw_parts(string_bytes(spec_ptr), string_len(spec_ptr));
            let spec_text = match std::str::from_utf8(spec_bytes) {
                Ok(val) => val,
                Err(_) => {
                    return raise_exception::<_>(
                        _py,
                        "ValueError",
                        "format spec must be valid UTF-8",
                    );
                }
            };
            let spec = match parse_format_spec(spec_text) {
                Ok(val) => val,
                Err(msg) => return raise_exception::<_>(_py, "ValueError", msg),
            };
            let obj = obj_from_bits(val_bits);
            let rendered = match format_with_spec(_py, obj, &spec) {
                Ok(val) => val,
                Err((kind, msg)) => return raise_exception::<_>(_py, kind, msg.as_ref()),
            };
            let out_ptr = alloc_string(_py, rendered.as_bytes());
            if out_ptr.is_null() {
                return MoltObject::none().bits();
            }
            MoltObject::from_ptr(out_ptr).bits()
        }
    })
}

fn build_utf8_cache(bytes: &[u8]) -> Utf8IndexCache {
    let mut offsets = Vec::new();
    let mut prefix = Vec::new();
    let mut total = 0i64;
    let mut idx = 0usize;
    offsets.push(0);
    prefix.push(0);
    while idx < bytes.len() {
        let mut end = (idx + UTF8_CACHE_BLOCK).min(bytes.len());
        while end < bytes.len() && (bytes[end] & 0b1100_0000) == 0b1000_0000 {
            end += 1;
        }
        total += count_utf8_bytes(&bytes[idx..end]);
        offsets.push(end);
        prefix.push(total);
        idx = end;
    }
    Utf8IndexCache { offsets, prefix }
}

fn utf8_cache_get_or_build(
    _py: &PyToken<'_>,
    key: usize,
    bytes: &[u8],
) -> Option<Arc<Utf8IndexCache>> {
    if bytes.len() < UTF8_CACHE_MIN_LEN || bytes.is_ascii() {
        return None;
    }
    if let Ok(store) = runtime_state(_py).utf8_index_cache.lock()
        && let Some(cache) = store.get(key)
    {
        return Some(cache);
    }
    let cache = Arc::new(build_utf8_cache(bytes));
    if let Ok(mut store) = runtime_state(_py).utf8_index_cache.lock() {
        if let Some(existing) = store.get(key) {
            return Some(existing);
        }
        store.insert(key, cache.clone());
    }
    Some(cache)
}

pub(crate) fn utf8_cache_remove(_py: &PyToken<'_>, key: usize) {
    if let Ok(mut store) = runtime_state(_py).utf8_index_cache.lock() {
        store.remove(key);
    }
    utf8_count_cache_remove(_py, key);
    utf8_count_cache_tls_remove(key);
}

fn utf8_count_cache_shard(key: usize) -> usize {
    let mut x = key as u64;
    x ^= x >> 33;
    x = x.wrapping_mul(0xff51afd7ed558ccd);
    x ^= x >> 33;
    (x as usize) & (UTF8_COUNT_CACHE_SHARDS - 1)
}

fn utf8_count_cache_remove(_py: &PyToken<'_>, key: usize) {
    let shard = utf8_count_cache_shard(key);
    if let Some(store) = runtime_state(_py).utf8_count_cache.get(shard)
        && let Ok(mut guard) = store.lock()
    {
        guard.remove(key);
    }
}

fn utf8_count_cache_lookup(
    _py: &PyToken<'_>,
    key: usize,
    needle: &[u8],
) -> Option<Arc<Utf8CountCache>> {
    if let Some(cache) = UTF8_COUNT_TLS.with(|cell| {
        cell.borrow().as_ref().and_then(|entry| {
            if entry.key == key && entry.cache.needle == needle {
                Some(entry.cache.clone())
            } else {
                None
            }
        })
    }) {
        profile_hit(_py, &runtime_state(_py).string_count_cache_hit);
        return Some(cache);
    }
    let shard = utf8_count_cache_shard(key);
    let store = runtime_state(_py)
        .utf8_count_cache
        .get(shard)?
        .lock()
        .ok()?;
    let cache = store.get(key)?;
    if cache.needle == needle {
        profile_hit(_py, &runtime_state(_py).string_count_cache_hit);
        return Some(cache);
    }
    None
}

fn build_utf8_count_prefix(hay_bytes: &[u8], needle: &[u8]) -> Vec<i64> {
    if hay_bytes.len() < UTF8_COUNT_PREFIX_MIN_LEN || needle.is_empty() {
        return Vec::new();
    }
    let blocks = hay_bytes.len().div_ceil(UTF8_CACHE_BLOCK);
    let mut prefix = vec![0i64; blocks + 1];
    let mut count = 0i64;
    let mut idx = 1usize;
    let mut next_boundary = UTF8_CACHE_BLOCK.min(hay_bytes.len());
    let finder = memmem::Finder::new(needle);
    for pos in finder.find_iter(hay_bytes) {
        while pos >= next_boundary && idx < prefix.len() {
            prefix[idx] = count;
            idx += 1;
            next_boundary = (next_boundary + UTF8_CACHE_BLOCK).min(hay_bytes.len());
        }
        count += 1;
    }
    while idx < prefix.len() {
        prefix[idx] = count;
        idx += 1;
    }
    prefix
}

fn utf8_count_cache_store(
    _py: &PyToken<'_>,
    key: usize,
    hay_bytes: &[u8],
    needle: &[u8],
    count: i64,
    prefix: Vec<i64>,
) {
    let cache = Arc::new(Utf8CountCache {
        needle: needle.to_vec(),
        count,
        prefix,
        hay_len: hay_bytes.len(),
    });
    let shard = utf8_count_cache_shard(key);
    if let Some(store) = runtime_state(_py).utf8_count_cache.get(shard)
        && let Ok(mut guard) = store.lock()
    {
        guard.insert(key, cache.clone());
    }
    UTF8_COUNT_TLS.with(|cell| {
        *cell.borrow_mut() = Some(Utf8CountCacheEntry { key, cache });
    });
}

fn utf8_count_cache_upgrade_prefix(
    _py: &PyToken<'_>,
    key: usize,
    cache: &Arc<Utf8CountCache>,
    hay_bytes: &[u8],
) -> Arc<Utf8CountCache> {
    if !cache.prefix.is_empty()
        || cache.hay_len != hay_bytes.len()
        || hay_bytes.len() < UTF8_COUNT_PREFIX_MIN_LEN
        || cache.needle.is_empty()
    {
        return cache.clone();
    }
    let prefix = build_utf8_count_prefix(hay_bytes, &cache.needle);
    if prefix.is_empty() {
        return cache.clone();
    }
    let upgraded = Arc::new(Utf8CountCache {
        needle: cache.needle.clone(),
        count: cache.count,
        prefix,
        hay_len: cache.hay_len,
    });
    let shard = utf8_count_cache_shard(key);
    if let Some(store) = runtime_state(_py).utf8_count_cache.get(shard)
        && let Ok(mut guard) = store.lock()
    {
        guard.insert(key, upgraded.clone());
    }
    UTF8_COUNT_TLS.with(|cell| {
        *cell.borrow_mut() = Some(Utf8CountCacheEntry {
            key,
            cache: upgraded.clone(),
        });
    });
    upgraded
}

fn utf8_count_cache_tls_remove(key: usize) {
    // Use try_with to avoid panicking during TLS destruction.
    // When thread-locals are being torn down (e.g., in ThreadLocalGuard::drop),
    // dec_ref on cached strings can reach this function after UTF8_COUNT_TLS
    // is already destroyed.
    let _ = UTF8_COUNT_TLS.try_with(|cell| {
        let mut guard = cell.borrow_mut();
        if guard.as_ref().is_some_and(|entry| entry.key == key) {
            *guard = None;
        }
    });
}

fn count_matches_range(
    hay_bytes: &[u8],
    needle: &[u8],
    window_start: usize,
    window_end: usize,
    start_min: usize,
    start_max: usize,
) -> i64 {
    if window_end <= window_start || start_min > start_max {
        return 0;
    }
    let finder = memmem::Finder::new(needle);
    let mut count = 0i64;
    for pos in finder.find_iter(&hay_bytes[window_start..window_end]) {
        let abs = window_start + pos;
        if abs < start_min {
            continue;
        }
        if abs > start_max {
            break;
        }
        count += 1;
    }
    count
}

fn utf8_count_cache_count_slice(
    cache: &Utf8CountCache,
    hay_bytes: &[u8],
    start: usize,
    end: usize,
) -> i64 {
    let needle = &cache.needle;
    let needle_len = needle.len();
    if needle_len == 0 || end <= start {
        return 0;
    }
    if end - start < needle_len {
        return 0;
    }
    if cache.prefix.is_empty() || cache.hay_len != hay_bytes.len() {
        return bytes_count_impl(&hay_bytes[start..end], needle);
    }
    let end_limit = end - needle_len;
    let block = UTF8_CACHE_BLOCK;
    let start_block = start / block;
    let end_block = end_limit / block;
    if start_block == end_block {
        return bytes_count_impl(&hay_bytes[start..end], needle);
    }
    let mut total = 0i64;
    let block_end = ((start_block + 1) * block).min(hay_bytes.len());
    let left_scan_end = (block_end + needle_len - 1).min(end);
    let left_max = (block_end.saturating_sub(1)).min(end_limit);
    total += count_matches_range(hay_bytes, needle, start, left_scan_end, start, left_max);
    if end_block > start_block + 1 {
        total += cache.prefix[end_block] - cache.prefix[start_block + 1];
    }
    let right_block_start = (end_block * block).min(hay_bytes.len());
    if right_block_start <= end_limit {
        total += count_matches_range(
            hay_bytes,
            needle,
            right_block_start,
            end,
            right_block_start,
            end_limit,
        );
    }
    total
}

fn utf8_count_prefix_cached(bytes: &[u8], cache: &Utf8IndexCache, prefix_len: usize) -> i64 {
    let prefix_len = prefix_len.min(bytes.len());
    let block_idx = match cache.offsets.binary_search(&prefix_len) {
        Ok(idx) => idx,
        Err(idx) => idx.saturating_sub(1),
    };
    let mut total = *cache.prefix.get(block_idx).unwrap_or(&0);
    let start = *cache.offsets.get(block_idx).unwrap_or(&0);
    if start < prefix_len {
        total += count_utf8_bytes(&bytes[start..prefix_len]);
    }
    total
}

pub(crate) fn utf8_codepoint_count_cached(
    _py: &PyToken<'_>,
    bytes: &[u8],
    cache_key: Option<usize>,
) -> i64 {
    if bytes.is_ascii() {
        return bytes.len() as i64;
    }
    if let Some(key) = cache_key
        && let Some(cache) = utf8_cache_get_or_build(_py, key, bytes)
    {
        return *cache.prefix.last().unwrap_or(&0);
    }
    utf8_count_prefix_blocked(bytes, bytes.len())
}

fn utf8_byte_to_char_index_cached(
    _py: &PyToken<'_>,
    bytes: &[u8],
    byte_idx: usize,
    cache_key: Option<usize>,
) -> i64 {
    if byte_idx == 0 {
        return 0;
    }
    if bytes.is_ascii() {
        return byte_idx.min(bytes.len()) as i64;
    }
    let prefix_len = byte_idx.min(bytes.len());
    if let Some(key) = cache_key
        && let Some(cache) = utf8_cache_get_or_build(_py, key, bytes)
    {
        return utf8_count_prefix_cached(bytes, &cache, prefix_len);
    }
    utf8_count_prefix_blocked(bytes, prefix_len)
}

pub(super) fn wtf8_from_bytes(bytes: &[u8]) -> &Wtf8 {
    // SAFETY: Molt string bytes are constructed as well-formed WTF-8.
    unsafe { &*(bytes as *const [u8] as *const Wtf8) }
}

pub(super) fn wtf8_codepoint_at(bytes: &[u8], idx: usize) -> Option<CodePoint> {
    wtf8_from_bytes(bytes).code_points().nth(idx)
}

#[allow(dead_code)]
fn wtf8_codepoint_count_scan(bytes: &[u8]) -> i64 {
    let mut idx = 0usize;
    let mut count = 0i64;
    while idx < bytes.len() {
        let width = utf8_char_width(bytes[idx]);
        if width == 0 {
            idx = idx.saturating_add(1);
        } else {
            idx = idx.saturating_add(width);
        }
        count += 1;
    }
    count
}

pub(super) fn wtf8_has_surrogates(bytes: &[u8]) -> bool {
    wtf8_from_bytes(bytes).as_str().is_none()
}

pub(super) fn push_wtf8_codepoint(out: &mut Vec<u8>, code: u32) {
    if code <= 0x7F {
        out.push(code as u8);
    } else if code <= 0x7FF {
        out.push(0xC0 | ((code >> 6) as u8));
        out.push(0x80 | (code as u8 & 0x3F));
    } else if code <= 0xFFFF {
        out.push(0xE0 | ((code >> 12) as u8));
        out.push(0x80 | (((code >> 6) as u8) & 0x3F));
        out.push(0x80 | (code as u8 & 0x3F));
    } else {
        out.push(0xF0 | ((code >> 18) as u8));
        out.push(0x80 | (((code >> 12) as u8) & 0x3F));
        out.push(0x80 | (((code >> 6) as u8) & 0x3F));
        out.push(0x80 | (code as u8 & 0x3F));
    }
}

fn utf8_char_width(first: u8) -> usize {
    if first < 0xC0 {
        1
    } else if first < 0xE0 {
        2
    } else if first < 0xF0 {
        3
    } else if first < 0xF8 {
        4
    } else {
        1
    }
}

fn utf8_char_to_byte_index_scan(bytes: &[u8], target: usize) -> usize {
    let mut idx = 0usize;
    let mut count = 0usize;
    while idx < bytes.len() && count < target {
        let width = utf8_char_width(bytes[idx]);
        idx = idx.saturating_add(width);
        count = count.saturating_add(1);
    }
    idx.min(bytes.len())
}

pub(super) fn utf8_char_to_byte_index_cached(
    _py: &PyToken<'_>,
    bytes: &[u8],
    char_idx: i64,
    cache_key: Option<usize>,
) -> usize {
    if char_idx <= 0 {
        return 0;
    }
    if bytes.is_ascii() {
        return (char_idx as usize).min(bytes.len());
    }
    let total = utf8_codepoint_count_cached(_py, bytes, cache_key);
    if char_idx >= total {
        return bytes.len();
    }
    let target = char_idx as usize;
    if let Some(key) = cache_key
        && let Some(cache) = utf8_cache_get_or_build(_py, key, bytes)
    {
        let mut lo = 0usize;
        let mut hi = cache.prefix.len().saturating_sub(1);
        while lo < hi {
            let mid = (lo + hi).div_ceil(2);
            if (cache.prefix.get(mid).copied().unwrap_or(0) as usize) <= target {
                lo = mid;
            } else {
                hi = mid.saturating_sub(1);
            }
        }
        let mut count = cache.prefix.get(lo).copied().unwrap_or(0) as usize;
        let mut idx = cache.offsets.get(lo).copied().unwrap_or(0);
        while idx < bytes.len() && count < target {
            let width = utf8_char_width(bytes[idx]);
            idx = idx.saturating_add(width);
            count = count.saturating_add(1);
        }
        return idx.min(bytes.len());
    }
    utf8_char_to_byte_index_scan(bytes, target)
}

fn utf8_count_prefix_blocked(bytes: &[u8], prefix_len: usize) -> i64 {
    const BLOCK: usize = 4096;
    let mut total = 0i64;
    let mut idx = 0usize;
    while idx + BLOCK <= prefix_len {
        total += count_utf8_bytes(&bytes[idx..idx + BLOCK]);
        idx += BLOCK;
    }
    if idx < prefix_len {
        total += count_utf8_bytes(&bytes[idx..prefix_len]);
    }
    total
}

#[cfg(not(target_arch = "wasm32"))]
fn count_utf8_bytes(bytes: &[u8]) -> i64 {
    // simdutf::count_utf8 counts non-continuation bytes, which works
    // correctly on both valid UTF-8 and WTF-8. No validation needed.
    #[cfg(feature = "simdutf")]
    {
        simdutf::count_utf8(bytes) as i64
    }
    #[cfg(not(feature = "simdutf"))]
    {
        bytes.iter().filter(|&&b| (b & 0xC0) != 0x80).count() as i64
    }
}

#[cfg(target_arch = "wasm32")]
fn count_utf8_bytes(bytes: &[u8]) -> i64 {
    // WASM SIMD fast path: count non-continuation bytes directly.
    // Works on valid UTF-8 and WTF-8 alike — no validation needed.
    unsafe { count_utf8_codepoints_wasm_simd(bytes) }
}

#[cfg(target_arch = "wasm32")]
unsafe fn count_utf8_codepoints_wasm_simd(bytes: &[u8]) -> i64 {
    unsafe {
        use std::arch::wasm32::*;
        let mut count = 0i64;
        let mut i = 0usize;
        let cont_mask = u8x16_splat(0xC0);
        let cont_pat = u8x16_splat(0x80);
        while i + 16 <= bytes.len() {
            let chunk = v128_load(bytes.as_ptr().add(i) as *const v128);
            // Isolate top 2 bits, compare to 0x80 → continuation bytes
            let masked = v128_and(chunk, cont_mask);
            let is_cont = u8x16_eq(masked, cont_pat);
            // Bitmask: bit set for each continuation byte
            let mask = u8x16_bitmask(is_cont);
            // 16 minus number of continuation bytes = number of codepoint-starting bytes
            count += (16 - mask.count_ones()) as i64;
            i += 16;
        }
        // Scalar tail
        for &b in &bytes[i..] {
            if (b & 0xC0) != 0x80 {
                count += 1;
            }
        }
        count
    }
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

#[unsafe(no_mangle)]
pub extern "C" fn molt_string_replace(
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
                if object_type_id(hay_ptr) != TYPE_ID_STRING {
                    return MoltObject::none().bits();
                }
                let needle_ptr = match needle.as_ptr() {
                    Some(ptr) => ptr,
                    None => {
                        let msg = format!(
                            "replace() argument 1 must be str, not {}",
                            type_name(_py, needle)
                        );
                        return raise_exception::<_>(_py, "TypeError", &msg);
                    }
                };
                if object_type_id(needle_ptr) != TYPE_ID_STRING {
                    let msg = format!(
                        "replace() argument 1 must be str, not {}",
                        type_name(_py, needle)
                    );
                    return raise_exception::<_>(_py, "TypeError", &msg);
                }
                let repl_ptr = match replacement.as_ptr() {
                    Some(ptr) => ptr,
                    None => {
                        let msg = format!(
                            "replace() argument 2 must be str, not {}",
                            type_name(_py, replacement)
                        );
                        return raise_exception::<_>(_py, "TypeError", &msg);
                    }
                };
                if object_type_id(repl_ptr) != TYPE_ID_STRING {
                    let msg = format!(
                        "replace() argument 2 must be str, not {}",
                        type_name(_py, replacement)
                    );
                    return raise_exception::<_>(_py, "TypeError", &msg);
                }
                let hay_bytes =
                    std::slice::from_raw_parts(string_bytes(hay_ptr), string_len(hay_ptr));
                let needle_bytes =
                    std::slice::from_raw_parts(string_bytes(needle_ptr), string_len(needle_ptr));
                let repl_bytes =
                    std::slice::from_raw_parts(string_bytes(repl_ptr), string_len(repl_ptr));
                let out = match replace_string_impl(
                    _py,
                    hay_ptr,
                    hay_bytes,
                    needle_bytes,
                    repl_bytes,
                    count,
                ) {
                    Some(out) => out,
                    None => return MoltObject::none().bits(),
                };
                let ptr = alloc_string(_py, &out);
                if ptr.is_null() {
                    return MoltObject::none().bits();
                }
                return MoltObject::from_ptr(ptr).bits();
            }
        }
        MoltObject::none().bits()
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_string_encode(hay_bits: u64, encoding_bits: u64, errors_bits: u64) -> u64 {
    crate::with_gil_entry_nopanic!(_py, {
        let hay = obj_from_bits(hay_bits);
        let Some(hay_ptr) = hay.as_ptr() else {
            return MoltObject::none().bits();
        };
        unsafe {
            if object_type_id(hay_ptr) != TYPE_ID_STRING {
                return MoltObject::none().bits();
            }
            let encoding = match parse_codec_arg(_py, encoding_bits, "encode", "encoding", "utf-8")
            {
                Some(val) => val,
                None => return MoltObject::none().bits(),
            };
            let errors = match parse_codec_arg(_py, errors_bits, "encode", "errors", "strict") {
                Some(val) => val,
                None => return MoltObject::none().bits(),
            };
            let bytes = std::slice::from_raw_parts(string_bytes(hay_ptr), string_len(hay_ptr));
            let out = match encode_string_with_errors(bytes, &encoding, Some(&errors)) {
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
                    return raise_unicode_encode_error::<_>(
                        _py,
                        encoding,
                        hay_bits,
                        pos,
                        pos + 1,
                        &reason,
                    );
                }
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
pub extern "C" fn molt_string_lower(hay_bits: u64) -> u64 {
    crate::with_gil_entry_nopanic!(_py, {
        let hay = obj_from_bits(hay_bits);
        let Some(hay_ptr) = hay.as_ptr() else {
            return MoltObject::none().bits();
        };
        unsafe {
            if object_type_id(hay_ptr) != TYPE_ID_STRING {
                return MoltObject::none().bits();
            }
            let hay_bytes = std::slice::from_raw_parts(string_bytes(hay_ptr), string_len(hay_ptr));
            // Single-pass ASCII check + already-lowercase detection.
            // If all bytes are ASCII and already lowercase, return the
            // input with an inc_ref instead of allocating a copy.
            let mut is_ascii = true;
            let mut already_lower = true;
            for &b in hay_bytes {
                if b >= 0x80 {
                    is_ascii = false;
                    break;
                }
                if b.is_ascii_uppercase() {
                    already_lower = false;
                }
            }
            if is_ascii {
                if already_lower {
                    inc_ref_bits(_py, hay_bits);
                    return hay_bits;
                }
                // Allocate string object directly, then write SIMD-lowered
                // bytes into the data buffer -- avoids intermediate Vec alloc.
                let ptr = alloc_bytes_like_with_len(_py, hay_bytes.len(), TYPE_ID_STRING);
                if ptr.is_null() {
                    return MoltObject::none().bits();
                }
                let data_ptr = ptr.add(std::mem::size_of::<usize>());
                let out = std::slice::from_raw_parts_mut(data_ptr, hay_bytes.len());
                ascii_lower_into(hay_bytes, out);
                return MoltObject::from_ptr(ptr).bits();
            }
            let Ok(hay_str) = std::str::from_utf8(hay_bytes) else {
                return MoltObject::none().bits();
            };
            let lowered = hay_str.to_lowercase();
            let ptr = alloc_string_nointern(_py, lowered.as_bytes());
            if ptr.is_null() {
                return MoltObject::none().bits();
            }
            MoltObject::from_ptr(ptr).bits()
        }
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_string_casefold(hay_bits: u64) -> u64 {
    crate::with_gil_entry_nopanic!(_py, {
        let hay = obj_from_bits(hay_bits);
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
            let folded: String = hay_str
                .case_fold_with(Variant::Full, Locale::NonTurkic)
                .collect();
            let ptr = alloc_string(_py, folded.as_bytes());
            if ptr.is_null() {
                return MoltObject::none().bits();
            }
            MoltObject::from_ptr(ptr).bits()
        }
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_string_upper(hay_bits: u64) -> u64 {
    crate::with_gil_entry_nopanic!(_py, {
        let hay = obj_from_bits(hay_bits);
        let Some(hay_ptr) = hay.as_ptr() else {
            return MoltObject::none().bits();
        };
        unsafe {
            if object_type_id(hay_ptr) != TYPE_ID_STRING {
                return MoltObject::none().bits();
            }
            let hay_bytes = std::slice::from_raw_parts(string_bytes(hay_ptr), string_len(hay_ptr));
            // Single-pass ASCII check + already-uppercase detection.
            let mut is_ascii = true;
            let mut already_upper = true;
            for &b in hay_bytes {
                if b >= 0x80 {
                    is_ascii = false;
                    break;
                }
                if b.is_ascii_lowercase() {
                    already_upper = false;
                }
            }
            if is_ascii {
                if already_upper {
                    inc_ref_bits(_py, hay_bits);
                    return hay_bits;
                }
                // Allocate string object directly, then write SIMD-uppered
                // bytes into the data buffer -- avoids intermediate Vec alloc.
                let ptr = alloc_bytes_like_with_len(_py, hay_bytes.len(), TYPE_ID_STRING);
                if ptr.is_null() {
                    return MoltObject::none().bits();
                }
                let data_ptr = ptr.add(std::mem::size_of::<usize>());
                let out = std::slice::from_raw_parts_mut(data_ptr, hay_bytes.len());
                ascii_upper_into(hay_bytes, out);
                return MoltObject::from_ptr(ptr).bits();
            }
            let Ok(hay_str) = std::str::from_utf8(hay_bytes) else {
                return MoltObject::none().bits();
            };
            let uppered = hay_str.to_uppercase();
            let ptr = alloc_string_nointern(_py, uppered.as_bytes());
            if ptr.is_null() {
                return MoltObject::none().bits();
            }
            MoltObject::from_ptr(ptr).bits()
        }
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_string_isidentifier(hay_bits: u64) -> u64 {
    crate::with_gil_entry_nopanic!(_py, {
        let hay = obj_from_bits(hay_bits);
        let Some(hay_ptr) = hay.as_ptr() else {
            return MoltObject::none().bits();
        };
        unsafe {
            if object_type_id(hay_ptr) != TYPE_ID_STRING {
                return MoltObject::none().bits();
            }
            let hay_bytes = std::slice::from_raw_parts(string_bytes(hay_ptr), string_len(hay_ptr));
            let Ok(hay_str) = std::str::from_utf8(hay_bytes) else {
                return MoltObject::from_bool(false).bits();
            };
            let mut chars = hay_str.chars();
            let Some(first) = chars.next() else {
                return MoltObject::from_bool(false).bits();
            };
            if !(first == '_' || is_xid_start(first)) {
                return MoltObject::from_bool(false).bits();
            }
            for ch in chars {
                if ch == '_' || is_xid_continue(ch) {
                    continue;
                }
                return MoltObject::from_bool(false).bits();
            }
            MoltObject::from_bool(true).bits()
        }
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_string_isdigit(hay_bits: u64) -> u64 {
    crate::with_gil_entry_nopanic!(_py, {
        let hay = obj_from_bits(hay_bits);
        let Some(hay_ptr) = hay.as_ptr() else {
            return MoltObject::none().bits();
        };
        unsafe {
            if object_type_id(hay_ptr) != TYPE_ID_STRING {
                return MoltObject::none().bits();
            }
            let hay_bytes = std::slice::from_raw_parts(string_bytes(hay_ptr), string_len(hay_ptr));
            // SIMD fast path: pure-ASCII strings use bulk digit range check
            if hay_bytes.is_ascii() {
                return MoltObject::from_bool(simd_is_all_ascii_digit(hay_bytes)).bits();
            }
            let Ok(hay_str) = std::str::from_utf8(hay_bytes) else {
                return MoltObject::from_bool(false).bits();
            };
            let mut seen = false;
            for ch in hay_str.chars() {
                if unicode_digit_table::is_digit(ch as u32) {
                    seen = true;
                    continue;
                }
                return MoltObject::from_bool(false).bits();
            }
            MoltObject::from_bool(seen).bits()
        }
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_string_isdecimal(hay_bits: u64) -> u64 {
    crate::with_gil_entry_nopanic!(_py, {
        let hay = obj_from_bits(hay_bits);
        let Some(hay_ptr) = hay.as_ptr() else {
            return MoltObject::none().bits();
        };
        unsafe {
            if object_type_id(hay_ptr) != TYPE_ID_STRING {
                return MoltObject::none().bits();
            }
            let hay_bytes = std::slice::from_raw_parts(string_bytes(hay_ptr), string_len(hay_ptr));
            // SIMD fast path: ASCII decimals are exactly '0'-'9'
            if hay_bytes.is_ascii() {
                return MoltObject::from_bool(simd_is_all_ascii_digit(hay_bytes)).bits();
            }
            let Ok(hay_str) = std::str::from_utf8(hay_bytes) else {
                return MoltObject::from_bool(false).bits();
            };
            let mut seen = false;
            for ch in hay_str.chars() {
                if unicode_decimal_table::is_decimal(ch as u32) {
                    seen = true;
                    continue;
                }
                return MoltObject::from_bool(false).bits();
            }
            MoltObject::from_bool(seen).bits()
        }
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_string_isnumeric(hay_bits: u64) -> u64 {
    crate::with_gil_entry_nopanic!(_py, {
        let hay = obj_from_bits(hay_bits);
        let Some(hay_ptr) = hay.as_ptr() else {
            return MoltObject::none().bits();
        };
        unsafe {
            if object_type_id(hay_ptr) != TYPE_ID_STRING {
                return MoltObject::none().bits();
            }
            let hay_bytes = std::slice::from_raw_parts(string_bytes(hay_ptr), string_len(hay_ptr));
            let Ok(hay_str) = std::str::from_utf8(hay_bytes) else {
                return MoltObject::from_bool(false).bits();
            };
            let mut seen = false;
            for ch in hay_str.chars() {
                if unicode_numeric_table::is_numeric(ch as u32) {
                    seen = true;
                    continue;
                }
                return MoltObject::from_bool(false).bits();
            }
            MoltObject::from_bool(seen).bits()
        }
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_string_isspace(hay_bits: u64) -> u64 {
    crate::with_gil_entry_nopanic!(_py, {
        let hay = obj_from_bits(hay_bits);
        let Some(hay_ptr) = hay.as_ptr() else {
            return MoltObject::none().bits();
        };
        unsafe {
            if object_type_id(hay_ptr) != TYPE_ID_STRING {
                return MoltObject::none().bits();
            }
            let hay_bytes = std::slice::from_raw_parts(string_bytes(hay_ptr), string_len(hay_ptr));
            // SIMD fast path: pure-ASCII strings use bulk whitespace check
            if hay_bytes.is_ascii() {
                return MoltObject::from_bool(simd_is_all_ascii_whitespace(hay_bytes)).bits();
            }
            let Ok(hay_str) = std::str::from_utf8(hay_bytes) else {
                return MoltObject::from_bool(false).bits();
            };
            let mut seen = false;
            for ch in hay_str.chars() {
                if unicode_space_table::is_space(ch as u32) {
                    seen = true;
                    continue;
                }
                return MoltObject::from_bool(false).bits();
            }
            MoltObject::from_bool(seen).bits()
        }
    })
}

#[inline]
fn string_char_is_cased(ch: char) -> bool {
    let lower: String = ch.to_lowercase().collect();
    let upper: String = ch.to_uppercase().collect();
    lower != upper
}

#[inline]
fn string_push_titlecase(out: &mut String, ch: char) {
    if let Some(mapped) = unicode_titlecase_table::titlecase(ch as u32) {
        out.push_str(mapped);
    } else {
        out.extend(ch.to_uppercase());
    }
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_string_isalpha(hay_bits: u64) -> u64 {
    crate::with_gil_entry_nopanic!(_py, {
        let hay = obj_from_bits(hay_bits);
        let Some(hay_ptr) = hay.as_ptr() else {
            return MoltObject::none().bits();
        };
        unsafe {
            if object_type_id(hay_ptr) != TYPE_ID_STRING {
                return MoltObject::none().bits();
            }
            let hay_bytes = std::slice::from_raw_parts(string_bytes(hay_ptr), string_len(hay_ptr));
            // SIMD fast path: pure-ASCII strings use bulk alpha range check
            if hay_bytes.is_ascii() {
                return MoltObject::from_bool(simd_is_all_ascii_alpha(hay_bytes)).bits();
            }
            let Ok(hay_str) = std::str::from_utf8(hay_bytes) else {
                return MoltObject::from_bool(false).bits();
            };
            let mut seen = false;
            for ch in hay_str.chars() {
                if ch.is_alphabetic() {
                    seen = true;
                    continue;
                }
                return MoltObject::from_bool(false).bits();
            }
            MoltObject::from_bool(seen).bits()
        }
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_string_isalnum(hay_bits: u64) -> u64 {
    crate::with_gil_entry_nopanic!(_py, {
        let hay = obj_from_bits(hay_bits);
        let Some(hay_ptr) = hay.as_ptr() else {
            return MoltObject::none().bits();
        };
        unsafe {
            if object_type_id(hay_ptr) != TYPE_ID_STRING {
                return MoltObject::none().bits();
            }
            let hay_bytes = std::slice::from_raw_parts(string_bytes(hay_ptr), string_len(hay_ptr));
            // SIMD fast path: pure-ASCII strings use bulk alnum range check
            if hay_bytes.is_ascii() {
                return MoltObject::from_bool(simd_is_all_ascii_alnum(hay_bytes)).bits();
            }
            let Ok(hay_str) = std::str::from_utf8(hay_bytes) else {
                return MoltObject::from_bool(false).bits();
            };
            let mut seen = false;
            for ch in hay_str.chars() {
                if ch.is_alphanumeric() {
                    seen = true;
                    continue;
                }
                return MoltObject::from_bool(false).bits();
            }
            MoltObject::from_bool(seen).bits()
        }
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_string_islower(hay_bits: u64) -> u64 {
    crate::with_gil_entry_nopanic!(_py, {
        let hay = obj_from_bits(hay_bits);
        let Some(hay_ptr) = hay.as_ptr() else {
            return MoltObject::none().bits();
        };
        unsafe {
            if object_type_id(hay_ptr) != TYPE_ID_STRING {
                return MoltObject::none().bits();
            }
            let hay_bytes = std::slice::from_raw_parts(string_bytes(hay_ptr), string_len(hay_ptr));
            // SIMD fast path for pure-ASCII: no uppercase letters + has lowercase
            if hay_bytes.is_ascii() {
                let has_lower = hay_bytes.iter().any(|b| b.is_ascii_lowercase());
                let has_upper = simd_has_any_ascii_upper(hay_bytes);
                return MoltObject::from_bool(has_lower && !has_upper).bits();
            }
            let Ok(hay_str) = std::str::from_utf8(hay_bytes) else {
                return MoltObject::from_bool(false).bits();
            };
            let mut seen = false;
            for ch in hay_str.chars() {
                if ch.is_lowercase() {
                    seen = true;
                    continue;
                }
                if ch.is_uppercase() || string_char_is_cased(ch) {
                    return MoltObject::from_bool(false).bits();
                }
            }
            MoltObject::from_bool(seen).bits()
        }
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_string_isupper(hay_bits: u64) -> u64 {
    crate::with_gil_entry_nopanic!(_py, {
        let hay = obj_from_bits(hay_bits);
        let Some(hay_ptr) = hay.as_ptr() else {
            return MoltObject::none().bits();
        };
        unsafe {
            if object_type_id(hay_ptr) != TYPE_ID_STRING {
                return MoltObject::none().bits();
            }
            let hay_bytes = std::slice::from_raw_parts(string_bytes(hay_ptr), string_len(hay_ptr));
            // SIMD fast path for pure-ASCII: no lowercase letters + has uppercase
            if hay_bytes.is_ascii() {
                let has_upper = hay_bytes.iter().any(|b| b.is_ascii_uppercase());
                let has_lower = simd_has_any_ascii_lower(hay_bytes);
                return MoltObject::from_bool(has_upper && !has_lower).bits();
            }
            let Ok(hay_str) = std::str::from_utf8(hay_bytes) else {
                return MoltObject::from_bool(false).bits();
            };
            let mut seen = false;
            for ch in hay_str.chars() {
                if ch.is_uppercase() {
                    seen = true;
                    continue;
                }
                if ch.is_lowercase() || string_char_is_cased(ch) {
                    return MoltObject::from_bool(false).bits();
                }
            }
            MoltObject::from_bool(seen).bits()
        }
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_string_isascii(hay_bits: u64) -> u64 {
    crate::with_gil_entry_nopanic!(_py, {
        let hay = obj_from_bits(hay_bits);
        let Some(hay_ptr) = hay.as_ptr() else {
            return MoltObject::none().bits();
        };
        unsafe {
            if object_type_id(hay_ptr) != TYPE_ID_STRING {
                return MoltObject::none().bits();
            }
            let hay_bytes = std::slice::from_raw_parts(string_bytes(hay_ptr), string_len(hay_ptr));
            MoltObject::from_bool(hay_bytes.is_ascii()).bits()
        }
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_string_istitle(hay_bits: u64) -> u64 {
    crate::with_gil_entry_nopanic!(_py, {
        let hay = obj_from_bits(hay_bits);
        let Some(hay_ptr) = hay.as_ptr() else {
            return MoltObject::none().bits();
        };
        unsafe {
            if object_type_id(hay_ptr) != TYPE_ID_STRING {
                return MoltObject::none().bits();
            }
            let hay_bytes = std::slice::from_raw_parts(string_bytes(hay_ptr), string_len(hay_ptr));
            let Ok(hay_str) = std::str::from_utf8(hay_bytes) else {
                return MoltObject::from_bool(false).bits();
            };
            let mut seen_cased = false;
            let mut prev_cased = false;
            for ch in hay_str.chars() {
                if !string_char_is_cased(ch) {
                    prev_cased = false;
                    continue;
                }
                if !prev_cased {
                    if ch.is_lowercase() {
                        return MoltObject::from_bool(false).bits();
                    }
                    seen_cased = true;
                    prev_cased = true;
                    continue;
                }
                if !ch.is_lowercase() {
                    return MoltObject::from_bool(false).bits();
                }
            }
            MoltObject::from_bool(seen_cased).bits()
        }
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_string_isprintable(hay_bits: u64) -> u64 {
    crate::with_gil_entry_nopanic!(_py, {
        let hay = obj_from_bits(hay_bits);
        let Some(hay_ptr) = hay.as_ptr() else {
            return MoltObject::none().bits();
        };
        unsafe {
            if object_type_id(hay_ptr) != TYPE_ID_STRING {
                return MoltObject::none().bits();
            }
            let hay_bytes = std::slice::from_raw_parts(string_bytes(hay_ptr), string_len(hay_ptr));
            // SIMD fast path: for ASCII, printable is [0x20..0x7E]
            if hay_bytes.is_ascii() {
                return MoltObject::from_bool(simd_is_all_ascii_printable(hay_bytes)).bits();
            }
            let Ok(hay_str) = std::str::from_utf8(hay_bytes) else {
                return MoltObject::from_bool(false).bits();
            };
            for ch in hay_str.chars() {
                if !unicode_printable_table::is_printable(ch as u32) {
                    return MoltObject::from_bool(false).bits();
                }
            }
            MoltObject::from_bool(true).bits()
        }
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_string_swapcase(hay_bits: u64) -> u64 {
    crate::with_gil_entry_nopanic!(_py, {
        let hay = obj_from_bits(hay_bits);
        let Some(hay_ptr) = hay.as_ptr() else {
            return MoltObject::none().bits();
        };
        unsafe {
            if object_type_id(hay_ptr) != TYPE_ID_STRING {
                return MoltObject::none().bits();
            }
            let hay_bytes = std::slice::from_raw_parts(string_bytes(hay_ptr), string_len(hay_ptr));
            // SIMD fast path: pure-ASCII strings use bulk XOR bit-5 swapcase
            if hay_bytes.is_ascii() {
                let buf = bytes_ascii_swapcase(hay_bytes);
                let ptr = alloc_string(_py, &buf);
                if ptr.is_null() {
                    return MoltObject::none().bits();
                }
                return MoltObject::from_ptr(ptr).bits();
            }
            let Ok(hay_str) = std::str::from_utf8(hay_bytes) else {
                return MoltObject::none().bits();
            };
            let mut out = String::with_capacity(hay_str.len());
            for ch in hay_str.chars() {
                if ch.is_lowercase() {
                    out.extend(ch.to_uppercase());
                } else if ch.is_uppercase() {
                    out.extend(ch.to_lowercase());
                } else {
                    out.push(ch);
                }
            }
            let ptr = alloc_string(_py, out.as_bytes());
            if ptr.is_null() {
                return MoltObject::none().bits();
            }
            MoltObject::from_ptr(ptr).bits()
        }
    })
}

/// Intrinsic for `str.__mul__` / `str * int`.
/// Avoids the generic `molt_mul` dispatch path (int check, bigint check, float
/// check, dunder lookup) when the compiler knows the LHS is a string.
#[unsafe(no_mangle)]
pub extern "C" fn molt_str_repeat(str_bits: u64, count_bits: u64) -> u64 {
    crate::with_gil_entry_nopanic!(_py, {
        let str_obj = obj_from_bits(str_bits);
        let count_obj = obj_from_bits(count_bits);
        let Some(ptr) = str_obj.as_ptr() else {
            return raise_exception::<_>(
                _py,
                "TypeError",
                "can't multiply sequence by non-int of type 'NoneType'",
            );
        };
        unsafe {
            if object_type_id(ptr) != TYPE_ID_STRING {
                return raise_exception::<_>(
                    _py,
                    "TypeError",
                    &format!(
                        "can't multiply sequence by non-int of type '{}'",
                        type_of_bits(_py, str_bits)
                    ),
                );
            }
        }
        let Some(count) = to_i64(count_obj) else {
            return raise_exception::<_>(
                _py,
                "TypeError",
                &format!(
                    "can't multiply sequence by non-int of type '{}'",
                    type_of_bits(_py, count_bits)
                ),
            );
        };
        match repeat_sequence(_py, ptr, count) {
            Some(bits) => bits,
            None => raise_exception::<_>(_py, "TypeError", "unsupported operand type(s) for *"),
        }
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_string_capitalize(hay_bits: u64) -> u64 {
    crate::with_gil_entry_nopanic!(_py, {
        let hay = obj_from_bits(hay_bits);
        let Some(hay_ptr) = hay.as_ptr() else {
            return MoltObject::none().bits();
        };
        unsafe {
            if object_type_id(hay_ptr) != TYPE_ID_STRING {
                return MoltObject::none().bits();
            }
            let hay_bytes = std::slice::from_raw_parts(string_bytes(hay_ptr), string_len(hay_ptr));
            // SIMD fast path: pure-ASCII capitalize uses bytes_ascii_capitalize
            if hay_bytes.is_ascii() {
                let buf = bytes_ascii_capitalize(hay_bytes);
                let ptr = alloc_string(_py, &buf);
                if ptr.is_null() {
                    return MoltObject::none().bits();
                }
                return MoltObject::from_ptr(ptr).bits();
            }
            let Ok(hay_str) = std::str::from_utf8(hay_bytes) else {
                return MoltObject::none().bits();
            };
            let mut out = String::with_capacity(hay_str.len());
            let mut chars = hay_str.chars();
            if let Some(first) = chars.next() {
                string_push_titlecase(&mut out, first);
                for ch in chars {
                    out.extend(ch.to_lowercase());
                }
            }
            let ptr = alloc_string(_py, out.as_bytes());
            if ptr.is_null() {
                return MoltObject::none().bits();
            }
            MoltObject::from_ptr(ptr).bits()
        }
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_string_title(hay_bits: u64) -> u64 {
    crate::with_gil_entry_nopanic!(_py, {
        let hay = obj_from_bits(hay_bits);
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
            // ASCII fast path: if all bytes < 0x80, use SIMD-accelerated bytes_ascii_title
            if hay_bytes.iter().all(|&b| b < 0x80) {
                let titled = bytes_ascii_title(hay_bytes);
                let ptr = alloc_string(_py, &titled);
                if ptr.is_null() {
                    return MoltObject::none().bits();
                }
                return MoltObject::from_ptr(ptr).bits();
            }
            let mut out = String::with_capacity(hay_str.len());
            let mut prev_cased = false;
            for ch in hay_str.chars() {
                if string_char_is_cased(ch) {
                    if prev_cased {
                        out.extend(ch.to_lowercase());
                    } else {
                        string_push_titlecase(&mut out, ch);
                    }
                    prev_cased = true;
                } else {
                    out.push(ch);
                    prev_cased = false;
                }
            }
            let ptr = alloc_string(_py, out.as_bytes());
            if ptr.is_null() {
                return MoltObject::none().bits();
            }
            MoltObject::from_ptr(ptr).bits()
        }
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_string_strip(hay_bits: u64, chars_bits: u64) -> u64 {
    crate::with_gil_entry_nopanic!(_py, {
        let hay = obj_from_bits(hay_bits);
        let chars = obj_from_bits(chars_bits);
        let Some(hay_ptr) = hay.as_ptr() else {
            return MoltObject::none().bits();
        };
        unsafe {
            if object_type_id(hay_ptr) != TYPE_ID_STRING {
                return MoltObject::none().bits();
            }
            let hay_bytes = std::slice::from_raw_parts(string_bytes(hay_ptr), string_len(hay_ptr));

            if chars.is_none() {
                // Default strip (whitespace) -- ASCII fast path avoids from_utf8.
                // ASCII whitespace: 0x09..=0x0D, 0x20
                let mut start = 0usize;
                let mut end = hay_bytes.len();
                let is_ascii = hay_bytes.iter().all(|&b| b < 0x80);
                if is_ascii {
                    while start < end && is_ascii_whitespace(hay_bytes[start]) {
                        start += 1;
                    }
                    while end > start && is_ascii_whitespace(hay_bytes[end - 1]) {
                        end -= 1;
                    }
                    if start == 0 && end == hay_bytes.len() {
                        // No whitespace to strip -- return same object.
                        inc_ref_bits(_py, hay_bits);
                        return hay_bits;
                    }
                    let trimmed = &hay_bytes[start..end];
                    let ptr = alloc_string_nointern(_py, trimmed);
                    if ptr.is_null() {
                        return MoltObject::none().bits();
                    }
                    return MoltObject::from_ptr(ptr).bits();
                }
                // Non-ASCII: fall through to str::trim.
                let Ok(hay_str) = std::str::from_utf8(hay_bytes) else {
                    return MoltObject::none().bits();
                };
                let trimmed = hay_str.trim();
                if trimmed.len() == hay_bytes.len() {
                    inc_ref_bits(_py, hay_bits);
                    return hay_bits;
                }
                let ptr = alloc_string_nointern(_py, trimmed.as_bytes());
                if ptr.is_null() {
                    return MoltObject::none().bits();
                }
                return MoltObject::from_ptr(ptr).bits();
            }

            // Custom chars strip.
            let Some(chars_ptr) = chars.as_ptr() else {
                return raise_exception::<_>(_py, "TypeError", "strip arg must be None or str");
            };
            if object_type_id(chars_ptr) != TYPE_ID_STRING {
                return raise_exception::<_>(_py, "TypeError", "strip arg must be None or str");
            }
            let chars_bytes =
                std::slice::from_raw_parts(string_bytes(chars_ptr), string_len(chars_ptr));
            let Ok(hay_str) = std::str::from_utf8(hay_bytes) else {
                return MoltObject::none().bits();
            };
            let Ok(chars_str) = std::str::from_utf8(chars_bytes) else {
                return MoltObject::none().bits();
            };
            let trimmed = if chars_str.is_empty() {
                hay_str
            } else {
                let mut strip_chars = HashSet::new();
                for ch in chars_str.chars() {
                    strip_chars.insert(ch);
                }
                let mut start = None;
                for (idx, ch) in hay_str.char_indices() {
                    if !strip_chars.contains(&ch) {
                        start = Some(idx);
                        break;
                    }
                }
                match start {
                    None => "",
                    Some(start_idx) => {
                        let mut end = None;
                        for (idx, ch) in hay_str.char_indices().rev() {
                            if !strip_chars.contains(&ch) {
                                end = Some(idx + ch.len_utf8());
                                break;
                            }
                        }
                        let end_idx = end.unwrap_or(start_idx);
                        &hay_str[start_idx..end_idx]
                    }
                }
            };
            let ptr = alloc_string_nointern(_py, trimmed.as_bytes());
            if ptr.is_null() {
                return MoltObject::none().bits();
            }
            MoltObject::from_ptr(ptr).bits()
        }
    })
}

/// Fast inline ASCII whitespace check matching Python's definition.
#[inline(always)]
fn is_ascii_whitespace(b: u8) -> bool {
    b == b' ' || (0x09..=0x0D).contains(&b)
}

/// Write ASCII-lowered bytes from `src` into `dst` using SIMD when available.
/// `dst` must have the same length as `src`. All bytes in `src` must be ASCII.
#[inline]
fn ascii_lower_into(src: &[u8], dst: &mut [u8]) {
    debug_assert_eq!(src.len(), dst.len());
    let mut i = 0usize;
    #[cfg(target_arch = "aarch64")]
    {
        if src.len() >= 16 && std::arch::is_aarch64_feature_detected!("neon") {
            unsafe {
                use std::arch::aarch64::*;
                let upper_a = vdupq_n_u8(b'A');
                let upper_z = vdupq_n_u8(b'Z');
                let case_bit = vdupq_n_u8(0x20);
                while i + 16 <= src.len() {
                    let v = vld1q_u8(src.as_ptr().add(i));
                    let is_upper = vandq_u8(vcgeq_u8(v, upper_a), vcleq_u8(v, upper_z));
                    let to_lower = vandq_u8(is_upper, case_bit);
                    let result = vorrq_u8(v, to_lower);
                    vst1q_u8(dst.as_mut_ptr().add(i), result);
                    i += 16;
                }
            }
        }
    }
    #[cfg(target_arch = "x86_64")]
    {
        if src.len() >= 16 && std::arch::is_x86_feature_detected!("sse2") {
            unsafe {
                use std::arch::x86_64::*;
                let case_bit = _mm_set1_epi8(0x20);
                while i + 16 <= src.len() {
                    let v = _mm_loadu_si128(src.as_ptr().add(i) as *const __m128i);
                    let ge_a = _mm_cmpgt_epi8(v, _mm_set1_epi8(b'A' as i8 - 1));
                    let le_z = _mm_cmpgt_epi8(_mm_set1_epi8(b'Z' as i8 + 1), v);
                    let is_upper = _mm_and_si128(ge_a, le_z);
                    let to_lower = _mm_and_si128(is_upper, case_bit);
                    let result = _mm_or_si128(v, to_lower);
                    _mm_storeu_si128(dst.as_mut_ptr().add(i) as *mut __m128i, result);
                    i += 16;
                }
            }
        }
    }
    for j in i..src.len() {
        dst[j] = if src[j].is_ascii_uppercase() {
            src[j].to_ascii_lowercase()
        } else {
            src[j]
        };
    }
}

/// Write ASCII-uppered bytes from `src` into `dst` using SIMD when available.
/// `dst` must have the same length as `src`. All bytes in `src` must be ASCII.
#[inline]
fn ascii_upper_into(src: &[u8], dst: &mut [u8]) {
    debug_assert_eq!(src.len(), dst.len());
    let mut i = 0usize;
    #[cfg(target_arch = "aarch64")]
    {
        if src.len() >= 16 && std::arch::is_aarch64_feature_detected!("neon") {
            unsafe {
                use std::arch::aarch64::*;
                let lower_a = vdupq_n_u8(b'a');
                let lower_z = vdupq_n_u8(b'z');
                let case_bit = vdupq_n_u8(0x20);
                while i + 16 <= src.len() {
                    let v = vld1q_u8(src.as_ptr().add(i));
                    let is_lower = vandq_u8(vcgeq_u8(v, lower_a), vcleq_u8(v, lower_z));
                    let clear = vandq_u8(is_lower, case_bit);
                    let result = veorq_u8(v, clear);
                    vst1q_u8(dst.as_mut_ptr().add(i), result);
                    i += 16;
                }
            }
        }
    }
    #[cfg(target_arch = "x86_64")]
    {
        if src.len() >= 16 && std::arch::is_x86_feature_detected!("sse2") {
            unsafe {
                use std::arch::x86_64::*;
                let case_bit = _mm_set1_epi8(0x20);
                while i + 16 <= src.len() {
                    let v = _mm_loadu_si128(src.as_ptr().add(i) as *const __m128i);
                    let ge_a = _mm_cmpgt_epi8(v, _mm_set1_epi8(b'a' as i8 - 1));
                    let le_z = _mm_cmpgt_epi8(_mm_set1_epi8(b'z' as i8 + 1), v);
                    let is_lower = _mm_and_si128(ge_a, le_z);
                    let clear = _mm_and_si128(is_lower, case_bit);
                    let result = _mm_xor_si128(v, clear);
                    _mm_storeu_si128(dst.as_mut_ptr().add(i) as *mut __m128i, result);
                    i += 16;
                }
            }
        }
    }
    for j in i..src.len() {
        dst[j] = if src[j].is_ascii_lowercase() {
            src[j].to_ascii_uppercase()
        } else {
            src[j]
        };
    }
}

fn string_lstrip_chars<'a>(hay_str: &'a str, chars_str: &str) -> &'a str {
    if chars_str.is_empty() {
        return hay_str;
    }
    let mut strip_chars = HashSet::new();
    for ch in chars_str.chars() {
        strip_chars.insert(ch);
    }
    for (idx, ch) in hay_str.char_indices() {
        if !strip_chars.contains(&ch) {
            return &hay_str[idx..];
        }
    }
    ""
}

fn string_rstrip_chars<'a>(hay_str: &'a str, chars_str: &str) -> &'a str {
    if chars_str.is_empty() {
        return hay_str;
    }
    let mut strip_chars = HashSet::new();
    for ch in chars_str.chars() {
        strip_chars.insert(ch);
    }
    for (idx, ch) in hay_str.char_indices().rev() {
        if !strip_chars.contains(&ch) {
            let end = idx + ch.len_utf8();
            return &hay_str[..end];
        }
    }
    ""
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_string_lstrip(hay_bits: u64, chars_bits: u64) -> u64 {
    crate::with_gil_entry_nopanic!(_py, {
        let hay = obj_from_bits(hay_bits);
        let chars = obj_from_bits(chars_bits);
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
            let trimmed = if chars.is_none() {
                hay_str.trim_start()
            } else {
                let Some(chars_ptr) = chars.as_ptr() else {
                    return raise_exception::<_>(
                        _py,
                        "TypeError",
                        "lstrip arg must be None or str",
                    );
                };
                if object_type_id(chars_ptr) != TYPE_ID_STRING {
                    return raise_exception::<_>(
                        _py,
                        "TypeError",
                        "lstrip arg must be None or str",
                    );
                }
                let chars_bytes =
                    std::slice::from_raw_parts(string_bytes(chars_ptr), string_len(chars_ptr));
                let Ok(chars_str) = std::str::from_utf8(chars_bytes) else {
                    return MoltObject::none().bits();
                };
                string_lstrip_chars(hay_str, chars_str)
            };
            let ptr = alloc_string_nointern(_py, trimmed.as_bytes());
            if ptr.is_null() {
                return MoltObject::none().bits();
            }
            MoltObject::from_ptr(ptr).bits()
        }
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_string_rstrip(hay_bits: u64, chars_bits: u64) -> u64 {
    crate::with_gil_entry_nopanic!(_py, {
        let hay = obj_from_bits(hay_bits);
        let chars = obj_from_bits(chars_bits);
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
            let trimmed = if chars.is_none() {
                hay_str.trim_end()
            } else {
                let Some(chars_ptr) = chars.as_ptr() else {
                    return raise_exception::<_>(
                        _py,
                        "TypeError",
                        "rstrip arg must be None or str",
                    );
                };
                if object_type_id(chars_ptr) != TYPE_ID_STRING {
                    return raise_exception::<_>(
                        _py,
                        "TypeError",
                        "rstrip arg must be None or str",
                    );
                }
                let chars_bytes =
                    std::slice::from_raw_parts(string_bytes(chars_ptr), string_len(chars_ptr));
                let Ok(chars_str) = std::str::from_utf8(chars_bytes) else {
                    return MoltObject::none().bits();
                };
                string_rstrip_chars(hay_str, chars_str)
            };
            let ptr = alloc_string_nointern(_py, trimmed.as_bytes());
            if ptr.is_null() {
                return MoltObject::none().bits();
            }
            MoltObject::from_ptr(ptr).bits()
        }
    })
}

fn parse_string_fillchar_arg(_py: &PyToken<'_>, fill_bits: u64) -> Result<char, u64> {
    if fill_bits == missing_bits(_py) {
        return Ok(' ');
    }
    let fill_obj = obj_from_bits(fill_bits);
    let Some(fill_ptr) = fill_obj.as_ptr() else {
        let msg = format!(
            "The fill character must be a unicode character, not {}",
            type_name(_py, fill_obj)
        );
        return Err(raise_exception::<_>(_py, "TypeError", &msg));
    };
    unsafe {
        if object_type_id(fill_ptr) != TYPE_ID_STRING {
            let msg = format!(
                "The fill character must be a unicode character, not {}",
                type_name(_py, fill_obj)
            );
            return Err(raise_exception::<_>(_py, "TypeError", &msg));
        }
        let fill_bytes = std::slice::from_raw_parts(string_bytes(fill_ptr), string_len(fill_ptr));
        let Ok(fill_str) = std::str::from_utf8(fill_bytes) else {
            return Err(MoltObject::none().bits());
        };
        let mut chars = fill_str.chars();
        let Some(fill_char) = chars.next() else {
            return Err(raise_exception::<_>(
                _py,
                "TypeError",
                "The fill character must be exactly one character long",
            ));
        };
        if chars.next().is_some() {
            return Err(raise_exception::<_>(
                _py,
                "TypeError",
                "The fill character must be exactly one character long",
            ));
        }
        Ok(fill_char)
    }
}

fn align_string_with_fill(
    hay_str: &str,
    fill_char: char,
    left_pad: usize,
    right_pad: usize,
) -> String {
    let extra = (left_pad.saturating_add(right_pad)).saturating_mul(fill_char.len_utf8());
    let mut out = String::with_capacity(hay_str.len().saturating_add(extra));
    for _ in 0..left_pad {
        out.push(fill_char);
    }
    out.push_str(hay_str);
    for _ in 0..right_pad {
        out.push(fill_char);
    }
    out
}

fn string_char_count(text: &str) -> usize {
    text.chars().count()
}

fn exception_is_lookup_error(_py: &PyToken<'_>, exc_bits: u64) -> bool {
    let exc_obj = obj_from_bits(exc_bits);
    let Some(exc_ptr) = exc_obj.as_ptr() else {
        return false;
    };
    unsafe {
        if object_type_id(exc_ptr) != TYPE_ID_EXCEPTION {
            return false;
        }
        let class_bits = exception_class_bits(exc_ptr);
        if class_bits != 0 {
            let lookup_error_bits = exception_type_bits_from_name(_py, "LookupError");
            if lookup_error_bits != 0 && issubclass_bits(class_bits, lookup_error_bits) {
                return true;
            }
        }
        let kind_bits = exception_kind_bits(exc_ptr);
        let kind = string_obj_to_owned(obj_from_bits(kind_bits));
        matches!(
            kind.as_deref(),
            Some("LookupError") | Some("IndexError") | Some("KeyError")
        )
    }
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_string_removeprefix(hay_bits: u64, prefix_bits: u64) -> u64 {
    crate::with_gil_entry_nopanic!(_py, {
        let hay = obj_from_bits(hay_bits);
        let prefix = obj_from_bits(prefix_bits);
        let Some(hay_ptr) = hay.as_ptr() else {
            return MoltObject::none().bits();
        };
        unsafe {
            if object_type_id(hay_ptr) != TYPE_ID_STRING {
                return MoltObject::none().bits();
            }
            let prefix_ptr = match prefix.as_ptr() {
                Some(ptr) => ptr,
                None => {
                    let msg = format!(
                        "removeprefix() argument must be str, not {}",
                        type_name(_py, prefix)
                    );
                    return raise_exception::<_>(_py, "TypeError", &msg);
                }
            };
            if object_type_id(prefix_ptr) != TYPE_ID_STRING {
                let msg = format!(
                    "removeprefix() argument must be str, not {}",
                    type_name(_py, prefix)
                );
                return raise_exception::<_>(_py, "TypeError", &msg);
            }
            let hay_bytes = std::slice::from_raw_parts(string_bytes(hay_ptr), string_len(hay_ptr));
            let prefix_bytes =
                std::slice::from_raw_parts(string_bytes(prefix_ptr), string_len(prefix_ptr));
            let Ok(hay_str) = std::str::from_utf8(hay_bytes) else {
                return MoltObject::none().bits();
            };
            let Ok(prefix_str) = std::str::from_utf8(prefix_bytes) else {
                return MoltObject::none().bits();
            };
            let out = if let Some(stripped) = hay_str.strip_prefix(prefix_str) {
                stripped
            } else {
                hay_str
            };
            let ptr = alloc_string(_py, out.as_bytes());
            if ptr.is_null() {
                return MoltObject::none().bits();
            }
            MoltObject::from_ptr(ptr).bits()
        }
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_string_removesuffix(hay_bits: u64, suffix_bits: u64) -> u64 {
    crate::with_gil_entry_nopanic!(_py, {
        let hay = obj_from_bits(hay_bits);
        let suffix = obj_from_bits(suffix_bits);
        let Some(hay_ptr) = hay.as_ptr() else {
            return MoltObject::none().bits();
        };
        unsafe {
            if object_type_id(hay_ptr) != TYPE_ID_STRING {
                return MoltObject::none().bits();
            }
            let suffix_ptr = match suffix.as_ptr() {
                Some(ptr) => ptr,
                None => {
                    let msg = format!(
                        "removesuffix() argument must be str, not {}",
                        type_name(_py, suffix)
                    );
                    return raise_exception::<_>(_py, "TypeError", &msg);
                }
            };
            if object_type_id(suffix_ptr) != TYPE_ID_STRING {
                let msg = format!(
                    "removesuffix() argument must be str, not {}",
                    type_name(_py, suffix)
                );
                return raise_exception::<_>(_py, "TypeError", &msg);
            }
            let hay_bytes = std::slice::from_raw_parts(string_bytes(hay_ptr), string_len(hay_ptr));
            let suffix_bytes =
                std::slice::from_raw_parts(string_bytes(suffix_ptr), string_len(suffix_ptr));
            let Ok(hay_str) = std::str::from_utf8(hay_bytes) else {
                return MoltObject::none().bits();
            };
            let Ok(suffix_str) = std::str::from_utf8(suffix_bytes) else {
                return MoltObject::none().bits();
            };
            let out = hay_str.strip_suffix(suffix_str).unwrap_or(hay_str);
            let ptr = alloc_string(_py, out.as_bytes());
            if ptr.is_null() {
                return MoltObject::none().bits();
            }
            MoltObject::from_ptr(ptr).bits()
        }
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_string_zfill(hay_bits: u64, width_bits: u64) -> u64 {
    crate::with_gil_entry_nopanic!(_py, {
        let hay = obj_from_bits(hay_bits);
        let width_err = format!(
            "'{}' object cannot be interpreted as an integer",
            type_name(_py, obj_from_bits(width_bits))
        );
        let width = index_i64_from_obj(_py, width_bits, &width_err);
        if exception_pending(_py) {
            return MoltObject::none().bits();
        }
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
            let hay_len = string_char_count(hay_str) as i64;
            let out = if width <= hay_len {
                hay_str.to_string()
            } else {
                let width_usize = match usize::try_from(width) {
                    Ok(val) => val,
                    Err(_) => usize::MAX,
                };
                let pad = width_usize.saturating_sub(string_char_count(hay_str));
                let (sign, rest) = match hay_str.chars().next() {
                    Some('+') | Some('-') => (&hay_str[..1], &hay_str[1..]),
                    _ => ("", hay_str),
                };
                let mut out = String::with_capacity(hay_str.len().saturating_add(pad));
                out.push_str(sign);
                for _ in 0..pad {
                    out.push('0');
                }
                out.push_str(rest);
                out
            };
            let ptr = alloc_string(_py, out.as_bytes());
            if ptr.is_null() {
                return MoltObject::none().bits();
            }
            MoltObject::from_ptr(ptr).bits()
        }
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_string_center(hay_bits: u64, width_bits: u64, fill_bits: u64) -> u64 {
    crate::with_gil_entry_nopanic!(_py, {
        let hay = obj_from_bits(hay_bits);
        let width_err = format!(
            "'{}' object cannot be interpreted as an integer",
            type_name(_py, obj_from_bits(width_bits))
        );
        let width = index_i64_from_obj(_py, width_bits, &width_err);
        if exception_pending(_py) {
            return MoltObject::none().bits();
        }
        let fill_char = match parse_string_fillchar_arg(_py, fill_bits) {
            Ok(ch) => ch,
            Err(err_bits) => return err_bits,
        };
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
            let hay_len = string_char_count(hay_str) as i64;
            let out = if width <= hay_len {
                hay_str.to_string()
            } else {
                let width_usize = match usize::try_from(width) {
                    Ok(val) => val,
                    Err(_) => usize::MAX,
                };
                let pad = width_usize.saturating_sub(string_char_count(hay_str));
                let left = pad.div_ceil(2);
                let right = pad - left;
                align_string_with_fill(hay_str, fill_char, left, right)
            };
            let ptr = alloc_string(_py, out.as_bytes());
            if ptr.is_null() {
                return MoltObject::none().bits();
            }
            MoltObject::from_ptr(ptr).bits()
        }
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_string_ljust(hay_bits: u64, width_bits: u64, fill_bits: u64) -> u64 {
    crate::with_gil_entry_nopanic!(_py, {
        let hay = obj_from_bits(hay_bits);
        let width_err = format!(
            "'{}' object cannot be interpreted as an integer",
            type_name(_py, obj_from_bits(width_bits))
        );
        let width = index_i64_from_obj(_py, width_bits, &width_err);
        if exception_pending(_py) {
            return MoltObject::none().bits();
        }
        let fill_char = match parse_string_fillchar_arg(_py, fill_bits) {
            Ok(ch) => ch,
            Err(err_bits) => return err_bits,
        };
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
            let hay_len = string_char_count(hay_str) as i64;
            let out = if width <= hay_len {
                hay_str.to_string()
            } else {
                let width_usize = match usize::try_from(width) {
                    Ok(val) => val,
                    Err(_) => usize::MAX,
                };
                let pad = width_usize.saturating_sub(string_char_count(hay_str));
                align_string_with_fill(hay_str, fill_char, 0, pad)
            };
            let ptr = alloc_string(_py, out.as_bytes());
            if ptr.is_null() {
                return MoltObject::none().bits();
            }
            MoltObject::from_ptr(ptr).bits()
        }
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_string_rjust(hay_bits: u64, width_bits: u64, fill_bits: u64) -> u64 {
    crate::with_gil_entry_nopanic!(_py, {
        let hay = obj_from_bits(hay_bits);
        let width_err = format!(
            "'{}' object cannot be interpreted as an integer",
            type_name(_py, obj_from_bits(width_bits))
        );
        let width = index_i64_from_obj(_py, width_bits, &width_err);
        if exception_pending(_py) {
            return MoltObject::none().bits();
        }
        let fill_char = match parse_string_fillchar_arg(_py, fill_bits) {
            Ok(ch) => ch,
            Err(err_bits) => return err_bits,
        };
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
            let hay_len = string_char_count(hay_str) as i64;
            let out = if width <= hay_len {
                hay_str.to_string()
            } else {
                let width_usize = match usize::try_from(width) {
                    Ok(val) => val,
                    Err(_) => usize::MAX,
                };
                let pad = width_usize.saturating_sub(string_char_count(hay_str));
                align_string_with_fill(hay_str, fill_char, pad, 0)
            };
            let ptr = alloc_string(_py, out.as_bytes());
            if ptr.is_null() {
                return MoltObject::none().bits();
            }
            MoltObject::from_ptr(ptr).bits()
        }
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_string_expandtabs(hay_bits: u64, tabsize_bits: u64) -> u64 {
    crate::with_gil_entry_nopanic!(_py, {
        let hay = obj_from_bits(hay_bits);
        let tabsize = if tabsize_bits == missing_bits(_py) {
            8
        } else {
            let tab_err = format!(
                "'{}' object cannot be interpreted as an integer",
                type_name(_py, obj_from_bits(tabsize_bits))
            );
            index_i64_from_obj(_py, tabsize_bits, &tab_err)
        };
        if exception_pending(_py) {
            return MoltObject::none().bits();
        }
        let tabsize = tabsize.max(0) as usize;
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
            let mut out = String::with_capacity(hay_str.len());
            let mut col = 0usize;
            for ch in hay_str.chars() {
                match ch {
                    '\t' => {
                        if tabsize == 0 {
                            continue;
                        }
                        let spaces = tabsize - (col % tabsize);
                        for _ in 0..spaces {
                            out.push(' ');
                        }
                        col = col.saturating_add(spaces);
                    }
                    '\n' | '\r' => {
                        out.push(ch);
                        col = 0;
                    }
                    _ => {
                        out.push(ch);
                        col = col.saturating_add(1);
                    }
                }
            }
            let ptr = alloc_string(_py, out.as_bytes());
            if ptr.is_null() {
                return MoltObject::none().bits();
            }
            MoltObject::from_ptr(ptr).bits()
        }
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_string_translate(hay_bits: u64, table_bits: u64) -> u64 {
    crate::with_gil_entry_nopanic!(_py, {
        let hay = obj_from_bits(hay_bits);
        let table = obj_from_bits(table_bits);
        let Some(hay_ptr) = hay.as_ptr() else {
            return MoltObject::none().bits();
        };
        unsafe {
            if object_type_id(hay_ptr) != TYPE_ID_STRING {
                return MoltObject::none().bits();
            }
            let dict_ptr_opt = table
                .as_ptr()
                .and_then(|ptr| dict_like_bits_from_ptr(_py, ptr))
                .and_then(|bits| obj_from_bits(bits).as_ptr())
                .filter(|ptr| object_type_id(*ptr) == TYPE_ID_DICT);
            let hay_bytes = std::slice::from_raw_parts(string_bytes(hay_ptr), string_len(hay_ptr));
            let Ok(hay_str) = std::str::from_utf8(hay_bytes) else {
                return MoltObject::none().bits();
            };
            let mut out = String::with_capacity(hay_str.len());
            for ch in hay_str.chars() {
                let key_bits = MoltObject::from_int(ch as i64).bits();
                let (mapped, mapped_owned) = if let Some(dict_ptr) = dict_ptr_opt {
                    (dict_get_in_place(_py, dict_ptr, key_bits), false)
                } else {
                    let mapped_bits = molt_getitem_method(table_bits, key_bits);
                    if exception_pending(_py) {
                        let exc_bits = molt_exception_last();
                        let is_lookup = exception_is_lookup_error(_py, exc_bits);
                        dec_ref_bits(_py, exc_bits);
                        if is_lookup {
                            clear_exception(_py);
                            out.push(ch);
                            continue;
                        }
                        return MoltObject::none().bits();
                    }
                    (Some(mapped_bits), true)
                };
                let Some(mapped_bits) = mapped else {
                    out.push(ch);
                    continue;
                };
                let mapped_obj = obj_from_bits(mapped_bits);
                if mapped_obj.is_none() {
                    if mapped_owned {
                        dec_ref_bits(_py, mapped_bits);
                    }
                    continue;
                }
                // Handle inline integers (codepoints) directly — they have
                // no heap pointer, so check before the as_ptr() gate.
                if mapped_obj.is_int() {
                    let code = mapped_obj.as_int_unchecked();
                    if !(0..=0x10FFFF).contains(&code) {
                        if mapped_owned {
                            dec_ref_bits(_py, mapped_bits);
                        }
                        return raise_exception::<_>(
                            _py,
                            "ValueError",
                            "character mapping must be in range(0x110000)",
                        );
                    }
                    if let Some(c) = char::from_u32(code as u32) {
                        out.push(c);
                    }
                    if mapped_owned {
                        dec_ref_bits(_py, mapped_bits);
                    }
                    continue;
                }
                let Some(mapped_ptr) = mapped_obj.as_ptr() else {
                    if mapped_owned {
                        dec_ref_bits(_py, mapped_bits);
                    }
                    return raise_exception::<_>(
                        _py,
                        "TypeError",
                        "character mapping must return integer, None or str",
                    );
                };
                if object_type_id(mapped_ptr) == TYPE_ID_STRING {
                    let mapped_bytes = std::slice::from_raw_parts(
                        string_bytes(mapped_ptr),
                        string_len(mapped_ptr),
                    );
                    let Ok(mapped_str) = std::str::from_utf8(mapped_bytes) else {
                        if mapped_owned {
                            dec_ref_bits(_py, mapped_bits);
                        }
                        return MoltObject::none().bits();
                    };
                    out.push_str(mapped_str);
                    if mapped_owned {
                        dec_ref_bits(_py, mapped_bits);
                    }
                    continue;
                }
                let Some(mapped_int) = to_bigint(mapped_obj) else {
                    if mapped_owned {
                        dec_ref_bits(_py, mapped_bits);
                    }
                    return raise_exception::<_>(
                        _py,
                        "TypeError",
                        "character mapping must return integer, None or str",
                    );
                };
                let max_code = BigInt::from(0x110000u32);
                if mapped_int.sign() == Sign::Minus || mapped_int >= max_code {
                    if mapped_owned {
                        dec_ref_bits(_py, mapped_bits);
                    }
                    return raise_exception::<_>(
                        _py,
                        "ValueError",
                        "character mapping must be in range(0x110000)",
                    );
                }
                let Some(code) = mapped_int.to_u32() else {
                    if mapped_owned {
                        dec_ref_bits(_py, mapped_bits);
                    }
                    return raise_exception::<_>(
                        _py,
                        "ValueError",
                        "character mapping must be in range(0x110000)",
                    );
                };
                let Some(mapped_ch) = char::from_u32(code) else {
                    if mapped_owned {
                        dec_ref_bits(_py, mapped_bits);
                    }
                    return raise_exception::<_>(
                        _py,
                        "ValueError",
                        "character mapping must be in range(0x110000)",
                    );
                };
                out.push(mapped_ch);
                if mapped_owned {
                    dec_ref_bits(_py, mapped_bits);
                }
            }
            let ptr = alloc_string(_py, out.as_bytes());
            if ptr.is_null() {
                return MoltObject::none().bits();
            }
            MoltObject::from_ptr(ptr).bits()
        }
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_string_maketrans(x_bits: u64, y_bits: u64, z_bits: u64) -> u64 {
    crate::with_gil_entry_nopanic!(_py, {
        let x_obj = obj_from_bits(x_bits);
        let y_obj = obj_from_bits(y_bits);
        let z_obj = obj_from_bits(z_bits);

        if y_obj.is_none() && z_obj.is_none() {
            let Some(x_ptr) = x_obj.as_ptr() else {
                return raise_exception::<_>(
                    _py,
                    "TypeError",
                    "if you give only one argument to maketrans it must be a dict",
                );
            };
            unsafe {
                if object_type_id(x_ptr) != TYPE_ID_DICT {
                    return raise_exception::<_>(
                        _py,
                        "TypeError",
                        "if you give only one argument to maketrans it must be a dict",
                    );
                }
                let out_ptr = alloc_dict_with_pairs(_py, &[]);
                if out_ptr.is_null() {
                    return MoltObject::none().bits();
                }
                let out_bits = MoltObject::from_ptr(out_ptr).bits();
                let pairs = dict_order(x_ptr);
                for pair in pairs.chunks_exact(2) {
                    let key_bits = pair[0];
                    let value_bits = pair[1];
                    let key_obj = obj_from_bits(key_bits);
                    let mapped_key_bits = if let Some(key_ptr) = key_obj.as_ptr() {
                        if object_type_id(key_ptr) == TYPE_ID_STRING {
                            let key_bytes = std::slice::from_raw_parts(
                                string_bytes(key_ptr),
                                string_len(key_ptr),
                            );
                            let Ok(key_str) = std::str::from_utf8(key_bytes) else {
                                dec_ref_bits(_py, out_bits);
                                return MoltObject::none().bits();
                            };
                            let mut chars = key_str.chars();
                            let Some(ch) = chars.next() else {
                                dec_ref_bits(_py, out_bits);
                                return raise_exception::<_>(
                                    _py,
                                    "ValueError",
                                    "string keys in translate table must be of length 1",
                                );
                            };
                            if chars.next().is_some() {
                                dec_ref_bits(_py, out_bits);
                                return raise_exception::<_>(
                                    _py,
                                    "ValueError",
                                    "string keys in translate table must be of length 1",
                                );
                            }
                            MoltObject::from_int(ch as i64).bits()
                        } else if to_bigint(key_obj).is_some() {
                            key_bits
                        } else {
                            dec_ref_bits(_py, out_bits);
                            return raise_exception::<_>(
                                _py,
                                "TypeError",
                                "keys in translate table must be strings or integers",
                            );
                        }
                    } else if to_bigint(key_obj).is_some() {
                        key_bits
                    } else {
                        dec_ref_bits(_py, out_bits);
                        return raise_exception::<_>(
                            _py,
                            "TypeError",
                            "keys in translate table must be strings or integers",
                        );
                    };
                    dict_set_in_place(_py, out_ptr, mapped_key_bits, value_bits);
                    if exception_pending(_py) {
                        dec_ref_bits(_py, out_bits);
                        return MoltObject::none().bits();
                    }
                }
                return out_bits;
            }
        }

        let x_ptr = match x_obj.as_ptr() {
            Some(ptr) => ptr,
            None => {
                let msg = format!(
                    "maketrans() argument 1 must be str, not {}",
                    type_name(_py, x_obj)
                );
                return raise_exception::<_>(_py, "TypeError", &msg);
            }
        };
        let y_ptr = match y_obj.as_ptr() {
            Some(ptr) => ptr,
            None => {
                let msg = format!(
                    "maketrans() argument 2 must be str, not {}",
                    type_name(_py, y_obj)
                );
                return raise_exception::<_>(_py, "TypeError", &msg);
            }
        };
        let z_ptr_opt = z_obj.as_ptr();
        unsafe {
            if object_type_id(x_ptr) != TYPE_ID_STRING {
                let msg = format!(
                    "maketrans() argument 1 must be str, not {}",
                    type_name(_py, x_obj)
                );
                return raise_exception::<_>(_py, "TypeError", &msg);
            }
            if object_type_id(y_ptr) != TYPE_ID_STRING {
                let msg = format!(
                    "maketrans() argument 2 must be str, not {}",
                    type_name(_py, y_obj)
                );
                return raise_exception::<_>(_py, "TypeError", &msg);
            }
            if !z_obj.is_none() {
                let Some(z_ptr) = z_ptr_opt else {
                    let msg = format!(
                        "maketrans() argument 3 must be str, not {}",
                        type_name(_py, z_obj)
                    );
                    return raise_exception::<_>(_py, "TypeError", &msg);
                };
                if object_type_id(z_ptr) != TYPE_ID_STRING {
                    let msg = format!(
                        "maketrans() argument 3 must be str, not {}",
                        type_name(_py, z_obj)
                    );
                    return raise_exception::<_>(_py, "TypeError", &msg);
                }
            }
            let x_bytes = std::slice::from_raw_parts(string_bytes(x_ptr), string_len(x_ptr));
            let y_bytes = std::slice::from_raw_parts(string_bytes(y_ptr), string_len(y_ptr));
            let Ok(x_str) = std::str::from_utf8(x_bytes) else {
                return MoltObject::none().bits();
            };
            let Ok(y_str) = std::str::from_utf8(y_bytes) else {
                return MoltObject::none().bits();
            };
            let x_len = string_char_count(x_str);
            let y_len = string_char_count(y_str);
            if x_len != y_len {
                return raise_exception::<_>(
                    _py,
                    "ValueError",
                    "the first two maketrans arguments must have equal length",
                );
            }
            let out_ptr = alloc_dict_with_pairs(_py, &[]);
            if out_ptr.is_null() {
                return MoltObject::none().bits();
            }
            let out_bits = MoltObject::from_ptr(out_ptr).bits();
            for (from_ch, to_ch) in x_str.chars().zip(y_str.chars()) {
                let key_bits = MoltObject::from_int(from_ch as i64).bits();
                let value_bits = MoltObject::from_int(to_ch as i64).bits();
                dict_set_in_place(_py, out_ptr, key_bits, value_bits);
                if exception_pending(_py) {
                    dec_ref_bits(_py, out_bits);
                    return MoltObject::none().bits();
                }
            }
            if !z_obj.is_none() {
                let z_ptr = z_ptr_opt.unwrap_or(std::ptr::null_mut());
                if z_ptr.is_null() {
                    dec_ref_bits(_py, out_bits);
                    let msg = format!(
                        "maketrans() argument 3 must be str, not {}",
                        type_name(_py, z_obj)
                    );
                    return raise_exception::<_>(_py, "TypeError", &msg);
                }
                let z_bytes = std::slice::from_raw_parts(string_bytes(z_ptr), string_len(z_ptr));
                let Ok(z_str) = std::str::from_utf8(z_bytes) else {
                    dec_ref_bits(_py, out_bits);
                    return MoltObject::none().bits();
                };
                let none_bits = MoltObject::none().bits();
                for ch in z_str.chars() {
                    let key_bits = MoltObject::from_int(ch as i64).bits();
                    dict_set_in_place(_py, out_ptr, key_bits, none_bits);
                    if exception_pending(_py) {
                        dec_ref_bits(_py, out_bits);
                        return MoltObject::none().bits();
                    }
                }
            }
            out_bits
        }
    })
}
