use super::super::ops::{
    bytes_ascii_capitalize, bytes_ascii_lower, bytes_ascii_swapcase, bytes_ascii_title,
    bytes_ascii_upper, simd_is_all_ascii_alnum, simd_is_all_ascii_alpha, simd_is_all_ascii_digit,
    simd_is_all_ascii_whitespace,
};
use super::*;
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
pub(in crate::object) fn bytes_ascii_space(b: u8) -> bool {
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
    // CPython accepts only bytes/bytearray fillchars (PyBytes_Check ||
    // PyByteArray_Check); everything else (str, int, memoryview, ...) raises the
    // short type error regardless of length. (Verified 3.12/3.13/3.14.)
    let Some(fill_ptr) = fill_obj.as_ptr() else {
        let msg = format!(
            "{method}() argument 2 must be a byte string of length 1, not {}",
            type_name(_py, fill_obj)
        );
        return raise_exception::<_>(_py, "TypeError", &msg);
    };
    unsafe {
        let type_id = object_type_id(fill_ptr);
        if type_id != TYPE_ID_BYTES && type_id != TYPE_ID_BYTEARRAY {
            let msg = format!(
                "{method}() argument 2 must be a byte string of length 1, not {}",
                type_name(_py, fill_obj)
            );
            return raise_exception::<_>(_py, "TypeError", &msg);
        }
        let fill_slice = bytes_like_slice(fill_ptr).unwrap_or(&[]);
        if fill_slice.len() != 1 {
            // 3.14 reports a long-form message naming the actual type
            // (bytes/bytearray) and the length; 3.12/3.13 use the short form.
            let msg = if crate::object::ops_sys::runtime_target_at_least(_py, 3, 14) {
                format!(
                    "{method}(): argument 2 must be a byte string of length 1, not a {} object of length {}",
                    type_name(_py, fill_obj),
                    fill_slice.len()
                )
            } else {
                format!(
                    "{method}() argument 2 must be a byte string of length 1, not {}",
                    type_name(_py, fill_obj)
                )
            };
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
            // CPython `bytes`/`bytearray.center` share `stringlib_center`
            // (Objects/stringlib/transmogrify.h): `left = marg / 2 +
            // (marg & width & 1)`, so the extra fill goes on the right unless
            // BOTH the total padding and the target width are odd.
            BytesAlignKind::Center => {
                let left = pad / 2 + (pad & total & 1);
                (left, pad - left)
            }
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
        let affix_bytes = match bytes_like_arg_or_type_error(_py, affix_ptr, || {
            format!(
                "a bytes-like object is required, not '{}'",
                type_name(_py, affix)
            )
        }) {
            Ok(slice) => slice,
            Err(bits) => return bits,
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
