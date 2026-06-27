use super::*;
use num_bigint::{BigInt, Sign};
use num_traits::ToPrimitive;
use std::collections::HashSet;

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
pub(super) fn ascii_lower_into(src: &[u8], dst: &mut [u8]) {
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
pub(super) fn ascii_upper_into(src: &[u8], dst: &mut [u8]) {
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
                // CPython `str.center` (Objects/stringlib/transmogrify.h,
                // stringlib_center): `left = marg / 2 + (marg & width & 1)`,
                // i.e. the extra fill goes on the right unless BOTH the total
                // padding and the target width are odd.
                let left = pad / 2 + (pad & width_usize & 1);
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
