//! String operations — extracted from ops.rs for tree-shaking.
//!
//! Each `pub extern "C" fn molt_string_*` / `molt_str_*` is a separate
//! linker symbol so that `wasm-ld --gc-sections` can drop unused entries.

use crate::*;
use molt_obj_model::MoltObject;
use unicode_casefold::{Locale, UnicodeCaseFold, Variant};
use unicode_ident::{is_xid_continue, is_xid_start};

use super::ops::{
    bytes_ascii_capitalize, bytes_ascii_swapcase, bytes_ascii_title, dict_like_bits_from_ptr,
    format_with_spec, parse_codec_arg, parse_format_spec, repeat_sequence,
    simd_has_any_ascii_lower, simd_has_any_ascii_upper, simd_is_all_ascii_alnum,
    simd_is_all_ascii_alpha, simd_is_all_ascii_digit, simd_is_all_ascii_printable,
    simd_is_all_ascii_whitespace, slice_bounds_from_args, slice_match,
};

#[path = "ops_string_affix.rs"]
mod ops_string_affix;
use ops_string_affix::{ascii_lower_into, ascii_upper_into};
pub use ops_string_affix::{
    molt_string_center, molt_string_expandtabs, molt_string_ljust, molt_string_lstrip,
    molt_string_maketrans, molt_string_removeprefix, molt_string_removesuffix, molt_string_rjust,
    molt_string_rstrip, molt_string_strip, molt_string_translate, molt_string_zfill,
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
                        let msg = str_sub_arg_type_msg(_py, "find", needle);
                        return raise_exception::<_>(_py, "TypeError", &msg);
                    }
                };
                if object_type_id(needle_ptr) != TYPE_ID_STRING {
                    let msg = str_sub_arg_type_msg(_py, "find", needle);
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
                        let msg = str_sub_arg_type_msg(_py, "rfind", needle);
                        return raise_exception::<_>(_py, "TypeError", &msg);
                    }
                };
                if object_type_id(needle_ptr) != TYPE_ID_STRING {
                    let msg = str_sub_arg_type_msg(_py, "rfind", needle);
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
        // Validate the needle HERE so a non-str needle yields the "index()"-named
        // TypeError; index delegates to find_slice, whose own check would
        // otherwise report "find" (the message differs by method on 3.13+).
        let needle = obj_from_bits(needle_bits);
        let needle_is_str = needle
            .as_ptr()
            .is_some_and(|p| unsafe { object_type_id(p) == TYPE_ID_STRING });
        if !needle_is_str {
            let msg = str_sub_arg_type_msg(_py, "index", needle);
            return raise_exception::<_>(_py, "TypeError", &msg);
        }
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
        // Validate the needle HERE so a non-str needle yields the "rindex()"-named
        // TypeError; rindex delegates to rfind_slice, whose own check would
        // otherwise report "rfind" (the message differs by method on 3.13+).
        let needle = obj_from_bits(needle_bits);
        let needle_is_str = needle
            .as_ptr()
            .is_some_and(|p| unsafe { object_type_id(p) == TYPE_ID_STRING });
        if !needle_is_str {
            let msg = str_sub_arg_type_msg(_py, "rindex", needle);
            return raise_exception::<_>(_py, "TypeError", &msg);
        }
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
                    let needle_bytes = std::slice::from_raw_parts(
                        string_bytes(needle_ptr),
                        string_len(needle_ptr),
                    );
                    return MoltObject::from_bool(hay_bytes.starts_with(needle_bytes)).bits();
                }
            }
            // Tuple or non-str needle: delegate to the slice path, the single
            // source of truth for tuple-element validation (CPython raises
            // "tuple for startswith must only contain str, not <type>" on any
            // non-str element) and the non-str-needle TypeError. The fast loop
            // this replaced silently skipped non-str tuple elements, so
            // `"hi".startswith(("x", 1))` returned False instead of raising.
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
                    let needle_bytes = std::slice::from_raw_parts(
                        string_bytes(needle_ptr),
                        string_len(needle_ptr),
                    );
                    return MoltObject::from_bool(hay_bytes.ends_with(needle_bytes)).bits();
                }
            }
            // Tuple or non-str needle: delegate to the slice path, the single
            // source of truth for tuple-element validation (CPython raises
            // "tuple for endswith must only contain str, not <type>" on any
            // non-str element) and the non-str-needle TypeError. The fast loop
            // this replaced silently skipped non-str tuple elements, so
            // `"hi".endswith(("x", 1))` returned False instead of raising.
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
                    let len = crate::object::seq_access::len(needle_ptr);
                    if len == 0 {
                        return MoltObject::from_bool(false).bits();
                    }
                    for idx in 0..len {
                        let Some(elem_bits) = crate::object::seq_access::item(needle_ptr, idx)
                        else {
                            return MoltObject::from_bool(false).bits();
                        };
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
                    let len = crate::object::seq_access::len(needle_ptr);
                    if len == 0 {
                        return MoltObject::from_bool(false).bits();
                    }
                    for idx in 0..len {
                        let Some(elem_bits) = crate::object::seq_access::item(needle_ptr, idx)
                        else {
                            return MoltObject::from_bool(false).bits();
                        };
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

/// Argument-type TypeError message shared by the str substring methods
/// (count/find/index/rfind/rindex). CPython 3.13 prefixed the bare
/// "must be str, not <type>" form with "<method>() argument 1 "; 3.12 used the
/// bare form. Gated on the configured target version (default 3.12) so the
/// message matches the emulated CPython across 3.12/3.13/3.14 on every arch/OS.
fn str_sub_arg_type_msg(_py: &PyToken<'_>, method: &str, needle: MoltObject) -> String {
    if crate::object::ops_sys::runtime_target_at_least(_py, 3, 13) {
        // CPython 3.13+ renders None as the value "None" rather than the type
        // name "NoneType" in this argument-type message; every other type uses
        // its name. (3.12's bare form keeps the type name "NoneType".)
        let rendered: String = if needle.is_none() {
            "None".to_string()
        } else {
            type_name(_py, needle).into_owned()
        };
        format!("{method}() argument 1 must be str, not {rendered}")
    } else {
        format!("must be str, not {}", type_name(_py, needle))
    }
}

fn str_count_arg_type_msg(_py: &PyToken<'_>, needle: MoltObject) -> String {
    str_sub_arg_type_msg(_py, "count", needle)
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
                    return raise_exception::<_>(
                        _py,
                        "TypeError",
                        &str_count_arg_type_msg(_py, needle),
                    );
                }
            };
            if object_type_id(needle_ptr) != TYPE_ID_STRING {
                return raise_exception::<_>(
                    _py,
                    "TypeError",
                    &str_count_arg_type_msg(_py, needle),
                );
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
                profile_hit(_py, &STRING_COUNT_CACHE_MISS_COUNT);
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
            let mut _sequence_snapshot = None;
            if let Some(ptr) = items.as_ptr() {
                let type_id = object_type_id(ptr);
                if type_id == TYPE_ID_LIST || type_id == TYPE_ID_TUPLE {
                    let Some(elems) = crate::object::seq_access::snapshot(
                        _py,
                        ptr,
                        "string join snapshot allocation failed",
                    ) else {
                        return MoltObject::none().bits();
                    };
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
                        parts.push(StringPart { data, len });
                    }
                    _sequence_snapshot = Some(elems);
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
                    let Some((elem_bits, done_bits)) =
                        crate::object::seq_access::tuple_pair(pair_ptr)
                    else {
                        for bits in owned_bits.iter().copied() {
                            dec_ref_bits(_py, bits);
                        }
                        return MoltObject::none().bits();
                    };
                    if is_truthy(_py, obj_from_bits(done_bits)) {
                        break;
                    }
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
                    parts.push(StringPart { data, len });
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

#[path = "ops_string_format.rs"]
mod ops_string_format;
pub use ops_string_format::{
    molt_string_format, molt_string_format_map, molt_string_format_method,
};

#[path = "ops_string_utf8.rs"]
mod ops_string_utf8;
pub(super) use ops_string_utf8::{
    push_wtf8_codepoint, utf8_char_to_byte_index_cached, wtf8_codepoint_at, wtf8_from_bytes,
    wtf8_has_surrogates,
};
use ops_string_utf8::{
    utf8_byte_to_char_index_cached, utf8_count_cache_count_slice, utf8_count_cache_lookup,
    utf8_count_cache_store, utf8_count_cache_upgrade_prefix,
};
pub(crate) use ops_string_utf8::{utf8_cache_remove, utf8_codepoint_count_cached};

#[path = "ops_string_split.rs"]
mod ops_string_split;
pub(crate) use ops_string_split::{explicit_split_field_args, split_field_bounds_at_index};
pub use ops_string_split::{
    molt_string_partition, molt_string_rpartition, molt_string_rsplit, molt_string_rsplit_max,
    molt_string_split, molt_string_split_field, molt_string_split_field_end,
    molt_string_split_field_eq, molt_string_split_field_is_ascii, molt_string_split_field_len,
    molt_string_split_field_len_from_bounds, molt_string_split_field_ord_at_bounds,
    molt_string_split_field_start, molt_string_split_max, molt_string_split_validate,
    molt_string_splitlines,
};

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
