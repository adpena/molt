use super::*;

pub(crate) fn bytes_ascii_upper(bytes: &[u8]) -> Vec<u8> {
    let mut out = vec![0u8; bytes.len()];
    let mut i = 0usize;
    // SIMD: clear bit 5 on lowercase bytes [a-z] → [A-Z]
    #[cfg(target_arch = "aarch64")]
    {
        if bytes.len() >= 16 && std::arch::is_aarch64_feature_detected!("neon") {
            unsafe {
                use std::arch::aarch64::*;
                let lower_a = vdupq_n_u8(b'a');
                let lower_z = vdupq_n_u8(b'z');
                let case_bit = vdupq_n_u8(0x20);
                while i + 16 <= bytes.len() {
                    let v = vld1q_u8(bytes.as_ptr().add(i));
                    let is_lower = vandq_u8(vcgeq_u8(v, lower_a), vcleq_u8(v, lower_z));
                    let clear = vandq_u8(is_lower, case_bit);
                    let result = veorq_u8(v, clear); // XOR clears bit 5 on lowercase
                    vst1q_u8(out.as_mut_ptr().add(i), result);
                    i += 16;
                }
            }
        }
    }
    #[cfg(target_arch = "x86_64")]
    {
        if bytes.len() >= 16 && std::arch::is_x86_feature_detected!("sse2") {
            unsafe {
                use std::arch::x86_64::*;
                let case_bit = _mm_set1_epi8(0x20);
                while i + 16 <= bytes.len() {
                    let v = _mm_loadu_si128(bytes.as_ptr().add(i) as *const __m128i);
                    let ge_a = _mm_cmpgt_epi8(v, _mm_set1_epi8(b'a' as i8 - 1));
                    let le_z = _mm_cmpgt_epi8(_mm_set1_epi8(b'z' as i8 + 1), v);
                    let is_lower = _mm_and_si128(ge_a, le_z);
                    let clear = _mm_and_si128(is_lower, case_bit);
                    let result = _mm_xor_si128(v, clear);
                    _mm_storeu_si128(out.as_mut_ptr().add(i) as *mut __m128i, result);
                    i += 16;
                }
            }
        }
    }
    #[cfg(target_arch = "wasm32")]
    {
        unsafe {
            use std::arch::wasm32::*;
            let lower_a = u8x16_splat(b'a');
            let lower_z = u8x16_splat(b'z');
            let case_bit = u8x16_splat(0x20);
            while i + 16 <= bytes.len() {
                let v = v128_load(bytes.as_ptr().add(i) as *const v128);
                let ge_a = u8x16_ge(v, lower_a);
                let le_z = u8x16_le(v, lower_z);
                let is_lower = v128_and(ge_a, le_z);
                let clear = v128_and(is_lower, case_bit);
                let result = v128_xor(v, clear);
                v128_store(out.as_mut_ptr().add(i) as *mut v128, result);
                i += 16;
            }
        }
    }
    for j in i..bytes.len() {
        out[j] = if bytes[j].is_ascii_lowercase() {
            bytes[j].to_ascii_uppercase()
        } else {
            bytes[j]
        };
    }
    out
}

#[inline]
pub(crate) fn bytes_ascii_lower(bytes: &[u8]) -> Vec<u8> {
    let mut out = vec![0u8; bytes.len()];
    let mut i = 0usize;
    // SIMD: set bit 5 on uppercase bytes [A-Z] → [a-z]
    #[cfg(target_arch = "aarch64")]
    {
        if bytes.len() >= 16 && std::arch::is_aarch64_feature_detected!("neon") {
            unsafe {
                use std::arch::aarch64::*;
                let upper_a = vdupq_n_u8(b'A');
                let upper_z = vdupq_n_u8(b'Z');
                let case_bit = vdupq_n_u8(0x20);
                while i + 16 <= bytes.len() {
                    let v = vld1q_u8(bytes.as_ptr().add(i));
                    let is_upper = vandq_u8(vcgeq_u8(v, upper_a), vcleq_u8(v, upper_z));
                    let to_lower = vandq_u8(is_upper, case_bit);
                    let result = vorrq_u8(v, to_lower);
                    vst1q_u8(out.as_mut_ptr().add(i), result);
                    i += 16;
                }
            }
        }
    }
    #[cfg(target_arch = "x86_64")]
    {
        if bytes.len() >= 16 && std::arch::is_x86_feature_detected!("sse2") {
            unsafe {
                use std::arch::x86_64::*;
                let case_bit = _mm_set1_epi8(0x20);
                while i + 16 <= bytes.len() {
                    let v = _mm_loadu_si128(bytes.as_ptr().add(i) as *const __m128i);
                    let ge_a = _mm_cmpgt_epi8(v, _mm_set1_epi8(b'A' as i8 - 1));
                    let le_z = _mm_cmpgt_epi8(_mm_set1_epi8(b'Z' as i8 + 1), v);
                    let is_upper = _mm_and_si128(ge_a, le_z);
                    let to_lower = _mm_and_si128(is_upper, case_bit);
                    let result = _mm_or_si128(v, to_lower);
                    _mm_storeu_si128(out.as_mut_ptr().add(i) as *mut __m128i, result);
                    i += 16;
                }
            }
        }
    }
    #[cfg(target_arch = "wasm32")]
    {
        unsafe {
            use std::arch::wasm32::*;
            let upper_a = u8x16_splat(b'A');
            let upper_z = u8x16_splat(b'Z');
            let case_bit = u8x16_splat(0x20);
            while i + 16 <= bytes.len() {
                let v = v128_load(bytes.as_ptr().add(i) as *const v128);
                let ge_a = u8x16_ge(v, upper_a);
                let le_z = u8x16_le(v, upper_z);
                let is_upper = v128_and(ge_a, le_z);
                let to_lower = v128_and(is_upper, case_bit);
                let result = v128_or(v, to_lower);
                v128_store(out.as_mut_ptr().add(i) as *mut v128, result);
                i += 16;
            }
        }
    }
    for j in i..bytes.len() {
        out[j] = if bytes[j].is_ascii_uppercase() {
            bytes[j].to_ascii_lowercase()
        } else {
            bytes[j]
        };
    }
    out
}

pub(crate) fn simd_is_all_ascii_whitespace(bytes: &[u8]) -> bool {
    if bytes.is_empty() {
        return false;
    }
    let mut i = 0usize;
    let ptr = bytes.as_ptr();

    #[cfg(target_arch = "aarch64")]
    {
        unsafe {
            use std::arch::aarch64::*;
            let space = vdupq_n_u8(b' ');
            let tab = vdupq_n_u8(b'\t');
            let nl = vdupq_n_u8(b'\n');
            let cr = vdupq_n_u8(b'\r');
            let vt = vdupq_n_u8(0x0b);
            let ff = vdupq_n_u8(0x0c);
            while i + 16 <= bytes.len() {
                let chunk = vld1q_u8(ptr.add(i));
                let is_ws = vorrq_u8(
                    vorrq_u8(
                        vorrq_u8(vceqq_u8(chunk, space), vceqq_u8(chunk, tab)),
                        vceqq_u8(chunk, nl),
                    ),
                    vorrq_u8(
                        vceqq_u8(chunk, cr),
                        vorrq_u8(vceqq_u8(chunk, vt), vceqq_u8(chunk, ff)),
                    ),
                );
                // If any byte is NOT whitespace, vminvq will be 0
                if vminvq_u8(is_ws) == 0 {
                    return false;
                }
                i += 16;
            }
        }
    }

    #[cfg(target_arch = "x86_64")]
    {
        unsafe {
            use std::arch::x86_64::*;
            let space = _mm_set1_epi8(b' ' as i8);
            let tab = _mm_set1_epi8(b'\t' as i8);
            let nl = _mm_set1_epi8(b'\n' as i8);
            let cr = _mm_set1_epi8(b'\r' as i8);
            let vt = _mm_set1_epi8(0x0b);
            let ff = _mm_set1_epi8(0x0c);
            while i + 16 <= bytes.len() {
                let chunk = _mm_loadu_si128(ptr.add(i) as *const __m128i);
                let is_ws = _mm_or_si128(
                    _mm_or_si128(
                        _mm_or_si128(_mm_cmpeq_epi8(chunk, space), _mm_cmpeq_epi8(chunk, tab)),
                        _mm_cmpeq_epi8(chunk, nl),
                    ),
                    _mm_or_si128(
                        _mm_cmpeq_epi8(chunk, cr),
                        _mm_or_si128(_mm_cmpeq_epi8(chunk, vt), _mm_cmpeq_epi8(chunk, ff)),
                    ),
                );
                // All bytes must be whitespace → all mask bits must be set
                if _mm_movemask_epi8(is_ws) != 0xFFFF {
                    return false;
                }
                i += 16;
            }
        }
    }

    #[cfg(target_arch = "wasm32")]
    {
        unsafe {
            use std::arch::wasm32::*;
            let space = u8x16_splat(b' ');
            let tab = u8x16_splat(b'\t');
            let nl = u8x16_splat(b'\n');
            let cr = u8x16_splat(b'\r');
            let vt = u8x16_splat(0x0b);
            let ff = u8x16_splat(0x0c);
            while i + 16 <= bytes.len() {
                let chunk = v128_load(ptr.add(i) as *const v128);
                let is_ws = v128_or(
                    v128_or(
                        v128_or(u8x16_eq(chunk, space), u8x16_eq(chunk, tab)),
                        u8x16_eq(chunk, nl),
                    ),
                    v128_or(
                        u8x16_eq(chunk, cr),
                        v128_or(u8x16_eq(chunk, vt), u8x16_eq(chunk, ff)),
                    ),
                );
                // All bytes must be whitespace → all bitmask bits set
                if u8x16_bitmask(is_ws) != 0xFFFF {
                    return false;
                }
                i += 16;
            }
        }
    }

    // Scalar tail
    while i < bytes.len() {
        if !bytes_ascii_space(bytes[i]) {
            return false;
        }
        i += 1;
    }
    true
}

/// SIMD-accelerated check: are ALL bytes ASCII alphabetic [A-Za-z]?
pub(crate) fn simd_is_all_ascii_alpha(bytes: &[u8]) -> bool {
    if bytes.is_empty() {
        return false;
    }
    let mut i = 0usize;

    #[cfg(target_arch = "aarch64")]
    {
        unsafe {
            use std::arch::aarch64::*;
            let case_bit = vdupq_n_u8(0x20); // bit 5 forces lowercase
            let a_lower = vdupq_n_u8(b'a');
            let z_lower = vdupq_n_u8(b'z');
            while i + 16 <= bytes.len() {
                let chunk = vld1q_u8(bytes.as_ptr().add(i));
                // Force lowercase via OR with 0x20, then range check 'a'-'z'
                let lowered = vorrq_u8(chunk, case_bit);
                let is_alpha = vandq_u8(vcgeq_u8(lowered, a_lower), vcleq_u8(lowered, z_lower));
                if vminvq_u8(is_alpha) == 0 {
                    return false;
                }
                i += 16;
            }
        }
    }

    #[cfg(target_arch = "x86_64")]
    {
        unsafe {
            use std::arch::x86_64::*;
            let case_bit = _mm_set1_epi8(0x20);
            while i + 16 <= bytes.len() {
                let chunk = _mm_loadu_si128(bytes.as_ptr().add(i) as *const __m128i);
                let lowered = _mm_or_si128(chunk, case_bit);
                let ge_a = _mm_cmpgt_epi8(lowered, _mm_set1_epi8((b'a' - 1) as i8));
                let le_z = _mm_cmpgt_epi8(_mm_set1_epi8((b'z' + 1) as i8), lowered);
                let is_alpha = _mm_and_si128(ge_a, le_z);
                if _mm_movemask_epi8(is_alpha) != 0xFFFF {
                    return false;
                }
                i += 16;
            }
        }
    }

    #[cfg(target_arch = "wasm32")]
    {
        unsafe {
            use std::arch::wasm32::*;
            let case_bit = u8x16_splat(0x20);
            let a_lower = u8x16_splat(b'a');
            let z_lower = u8x16_splat(b'z');
            while i + 16 <= bytes.len() {
                let chunk = v128_load(bytes.as_ptr().add(i) as *const v128);
                let lowered = v128_or(chunk, case_bit);
                // Range check: a <= lowered <= z
                // lowered >= a: use unsigned saturating sub; if (lowered - a) didn't underflow, >= a
                let ge_a = u8x16_ge(lowered, a_lower);
                let le_z = u8x16_le(lowered, z_lower);
                let is_alpha = v128_and(ge_a, le_z);
                if u8x16_bitmask(is_alpha) != 0xFFFF {
                    return false;
                }
                i += 16;
            }
        }
    }

    while i < bytes.len() {
        if !bytes[i].is_ascii_alphabetic() {
            return false;
        }
        i += 1;
    }
    true
}

/// SIMD-accelerated check: are ALL bytes ASCII digits [0-9]?
pub(crate) fn simd_is_all_ascii_digit(bytes: &[u8]) -> bool {
    if bytes.is_empty() {
        return false;
    }
    let mut i = 0usize;

    #[cfg(target_arch = "aarch64")]
    {
        unsafe {
            use std::arch::aarch64::*;
            let zero = vdupq_n_u8(b'0');
            let nine = vdupq_n_u8(b'9');
            while i + 16 <= bytes.len() {
                let chunk = vld1q_u8(bytes.as_ptr().add(i));
                let is_digit = vandq_u8(vcgeq_u8(chunk, zero), vcleq_u8(chunk, nine));
                if vminvq_u8(is_digit) == 0 {
                    return false;
                }
                i += 16;
            }
        }
    }

    #[cfg(target_arch = "x86_64")]
    {
        unsafe {
            use std::arch::x86_64::*;
            while i + 16 <= bytes.len() {
                let chunk = _mm_loadu_si128(bytes.as_ptr().add(i) as *const __m128i);
                let ge_0 = _mm_cmpgt_epi8(chunk, _mm_set1_epi8((b'0' - 1) as i8));
                let le_9 = _mm_cmpgt_epi8(_mm_set1_epi8((b'9' + 1) as i8), chunk);
                let is_digit = _mm_and_si128(ge_0, le_9);
                if _mm_movemask_epi8(is_digit) != 0xFFFF {
                    return false;
                }
                i += 16;
            }
        }
    }

    #[cfg(target_arch = "wasm32")]
    {
        unsafe {
            use std::arch::wasm32::*;
            let zero = u8x16_splat(b'0');
            let nine = u8x16_splat(b'9');
            while i + 16 <= bytes.len() {
                let chunk = v128_load(bytes.as_ptr().add(i) as *const v128);
                let ge_0 = u8x16_ge(chunk, zero);
                let le_9 = u8x16_le(chunk, nine);
                let is_digit = v128_and(ge_0, le_9);
                if u8x16_bitmask(is_digit) != 0xFFFF {
                    return false;
                }
                i += 16;
            }
        }
    }

    while i < bytes.len() {
        if !bytes[i].is_ascii_digit() {
            return false;
        }
        i += 1;
    }
    true
}

/// SIMD-accelerated check: are ALL bytes ASCII alphanumeric [A-Za-z0-9]?
pub(crate) fn simd_is_all_ascii_alnum(bytes: &[u8]) -> bool {
    if bytes.is_empty() {
        return false;
    }
    let mut i = 0usize;

    #[cfg(target_arch = "aarch64")]
    {
        unsafe {
            use std::arch::aarch64::*;
            let case_bit = vdupq_n_u8(0x20);
            let a_lower = vdupq_n_u8(b'a');
            let z_lower = vdupq_n_u8(b'z');
            let zero = vdupq_n_u8(b'0');
            let nine = vdupq_n_u8(b'9');
            while i + 16 <= bytes.len() {
                let chunk = vld1q_u8(bytes.as_ptr().add(i));
                let lowered = vorrq_u8(chunk, case_bit);
                let is_alpha = vandq_u8(vcgeq_u8(lowered, a_lower), vcleq_u8(lowered, z_lower));
                let is_digit = vandq_u8(vcgeq_u8(chunk, zero), vcleq_u8(chunk, nine));
                let is_alnum = vorrq_u8(is_alpha, is_digit);
                if vminvq_u8(is_alnum) == 0 {
                    return false;
                }
                i += 16;
            }
        }
    }

    #[cfg(target_arch = "x86_64")]
    {
        unsafe {
            use std::arch::x86_64::*;
            let case_bit = _mm_set1_epi8(0x20);
            while i + 16 <= bytes.len() {
                let chunk = _mm_loadu_si128(bytes.as_ptr().add(i) as *const __m128i);
                let lowered = _mm_or_si128(chunk, case_bit);
                let ge_a = _mm_cmpgt_epi8(lowered, _mm_set1_epi8((b'a' - 1) as i8));
                let le_z = _mm_cmpgt_epi8(_mm_set1_epi8((b'z' + 1) as i8), lowered);
                let is_alpha = _mm_and_si128(ge_a, le_z);
                let ge_0 = _mm_cmpgt_epi8(chunk, _mm_set1_epi8((b'0' - 1) as i8));
                let le_9 = _mm_cmpgt_epi8(_mm_set1_epi8((b'9' + 1) as i8), chunk);
                let is_digit = _mm_and_si128(ge_0, le_9);
                let is_alnum = _mm_or_si128(is_alpha, is_digit);
                if _mm_movemask_epi8(is_alnum) != 0xFFFF {
                    return false;
                }
                i += 16;
            }
        }
    }

    #[cfg(target_arch = "wasm32")]
    {
        unsafe {
            use std::arch::wasm32::*;
            let case_bit = u8x16_splat(0x20);
            let a_lower = u8x16_splat(b'a');
            let z_lower = u8x16_splat(b'z');
            let zero = u8x16_splat(b'0');
            let nine = u8x16_splat(b'9');
            while i + 16 <= bytes.len() {
                let chunk = v128_load(bytes.as_ptr().add(i) as *const v128);
                let lowered = v128_or(chunk, case_bit);
                let is_alpha = v128_and(u8x16_ge(lowered, a_lower), u8x16_le(lowered, z_lower));
                let is_digit = v128_and(u8x16_ge(chunk, zero), u8x16_le(chunk, nine));
                let is_alnum = v128_or(is_alpha, is_digit);
                if u8x16_bitmask(is_alnum) != 0xFFFF {
                    return false;
                }
                i += 16;
            }
        }
    }

    while i < bytes.len() {
        if !bytes[i].is_ascii_alphanumeric() {
            return false;
        }
        i += 1;
    }
    true
}

/// SIMD-accelerated check: are ALL bytes ASCII printable [0x20..0x7E]?
pub(crate) fn simd_is_all_ascii_printable(bytes: &[u8]) -> bool {
    // Empty string is "printable" per Python semantics
    let mut i = 0usize;

    #[cfg(target_arch = "aarch64")]
    {
        unsafe {
            use std::arch::aarch64::*;
            let lo = vdupq_n_u8(0x20);
            let hi = vdupq_n_u8(0x7E);
            while i + 16 <= bytes.len() {
                let chunk = vld1q_u8(bytes.as_ptr().add(i));
                let is_print = vandq_u8(vcgeq_u8(chunk, lo), vcleq_u8(chunk, hi));
                if vminvq_u8(is_print) == 0 {
                    return false;
                }
                i += 16;
            }
        }
    }

    #[cfg(target_arch = "x86_64")]
    {
        unsafe {
            use std::arch::x86_64::*;
            while i + 16 <= bytes.len() {
                let chunk = _mm_loadu_si128(bytes.as_ptr().add(i) as *const __m128i);
                let ge_lo = _mm_cmpgt_epi8(chunk, _mm_set1_epi8(0x1F));
                let le_hi = _mm_cmpgt_epi8(_mm_set1_epi8(0x7F_u8 as i8), chunk);
                let is_print = _mm_and_si128(ge_lo, le_hi);
                if _mm_movemask_epi8(is_print) != 0xFFFF {
                    return false;
                }
                i += 16;
            }
        }
    }

    #[cfg(target_arch = "wasm32")]
    {
        unsafe {
            use std::arch::wasm32::*;
            let lo = u8x16_splat(0x20);
            let hi = u8x16_splat(0x7E);
            while i + 16 <= bytes.len() {
                let chunk = v128_load(bytes.as_ptr().add(i) as *const v128);
                let is_print = v128_and(u8x16_ge(chunk, lo), u8x16_le(chunk, hi));
                if u8x16_bitmask(is_print) != 0xFFFF {
                    return false;
                }
                i += 16;
            }
        }
    }

    while i < bytes.len() {
        let b = bytes[i];
        if !(0x20..=0x7E).contains(&b) {
            return false;
        }
        i += 1;
    }
    true
}

/// SIMD check: does the buffer contain ANY uppercase ASCII letter [A-Z]?
pub(crate) fn simd_has_any_ascii_upper(bytes: &[u8]) -> bool {
    let mut i = 0usize;

    #[cfg(target_arch = "aarch64")]
    {
        unsafe {
            use std::arch::aarch64::*;
            let a_upper = vdupq_n_u8(b'A');
            let z_upper = vdupq_n_u8(b'Z');
            while i + 16 <= bytes.len() {
                let chunk = vld1q_u8(bytes.as_ptr().add(i));
                let is_upper = vandq_u8(vcgeq_u8(chunk, a_upper), vcleq_u8(chunk, z_upper));
                if vmaxvq_u8(is_upper) != 0 {
                    return true;
                }
                i += 16;
            }
        }
    }

    #[cfg(target_arch = "x86_64")]
    {
        unsafe {
            use std::arch::x86_64::*;
            while i + 16 <= bytes.len() {
                let chunk = _mm_loadu_si128(bytes.as_ptr().add(i) as *const __m128i);
                let ge_a = _mm_cmpgt_epi8(chunk, _mm_set1_epi8((b'A' - 1) as i8));
                let le_z = _mm_cmpgt_epi8(_mm_set1_epi8((b'Z' + 1) as i8), chunk);
                let is_upper = _mm_and_si128(ge_a, le_z);
                if _mm_movemask_epi8(is_upper) != 0 {
                    return true;
                }
                i += 16;
            }
        }
    }

    #[cfg(target_arch = "wasm32")]
    {
        unsafe {
            use std::arch::wasm32::*;
            let a_upper = u8x16_splat(b'A');
            let z_upper = u8x16_splat(b'Z');
            while i + 16 <= bytes.len() {
                let chunk = v128_load(bytes.as_ptr().add(i) as *const v128);
                let is_upper = v128_and(u8x16_ge(chunk, a_upper), u8x16_le(chunk, z_upper));
                if u8x16_bitmask(is_upper) != 0 {
                    return true;
                }
                i += 16;
            }
        }
    }

    while i < bytes.len() {
        if bytes[i].is_ascii_uppercase() {
            return true;
        }
        i += 1;
    }
    false
}

/// SIMD check: does the buffer contain ANY lowercase ASCII letter [a-z]?
pub(crate) fn simd_has_any_ascii_lower(bytes: &[u8]) -> bool {
    let mut i = 0usize;

    #[cfg(target_arch = "aarch64")]
    {
        unsafe {
            use std::arch::aarch64::*;
            let a_lower = vdupq_n_u8(b'a');
            let z_lower = vdupq_n_u8(b'z');
            while i + 16 <= bytes.len() {
                let chunk = vld1q_u8(bytes.as_ptr().add(i));
                let is_lower = vandq_u8(vcgeq_u8(chunk, a_lower), vcleq_u8(chunk, z_lower));
                if vmaxvq_u8(is_lower) != 0 {
                    return true;
                }
                i += 16;
            }
        }
    }

    #[cfg(target_arch = "x86_64")]
    {
        unsafe {
            use std::arch::x86_64::*;
            while i + 16 <= bytes.len() {
                let chunk = _mm_loadu_si128(bytes.as_ptr().add(i) as *const __m128i);
                let ge_a = _mm_cmpgt_epi8(chunk, _mm_set1_epi8((b'a' - 1) as i8));
                let le_z = _mm_cmpgt_epi8(_mm_set1_epi8((b'z' + 1) as i8), chunk);
                let is_lower = _mm_and_si128(ge_a, le_z);
                if _mm_movemask_epi8(is_lower) != 0 {
                    return true;
                }
                i += 16;
            }
        }
    }

    #[cfg(target_arch = "wasm32")]
    {
        unsafe {
            use std::arch::wasm32::*;
            let a_lower = u8x16_splat(b'a');
            let z_lower = u8x16_splat(b'z');
            while i + 16 <= bytes.len() {
                let chunk = v128_load(bytes.as_ptr().add(i) as *const v128);
                let is_lower = v128_and(u8x16_ge(chunk, a_lower), u8x16_le(chunk, z_lower));
                if u8x16_bitmask(is_lower) != 0 {
                    return true;
                }
                i += 16;
            }
        }
    }

    while i < bytes.len() {
        if bytes[i].is_ascii_lowercase() {
            return true;
        }
        i += 1;
    }
    false
}

pub(crate) fn bytes_ascii_capitalize(bytes: &[u8]) -> Vec<u8> {
    if bytes.is_empty() {
        return Vec::new();
    }
    let mut out = vec![0u8; bytes.len()];
    // First byte: capitalize
    out[0] = if bytes[0].is_ascii_lowercase() {
        bytes[0].to_ascii_uppercase()
    } else {
        bytes[0]
    };
    // Rest: SIMD-accelerated lowercasing (set bit 5 on uppercase bytes)
    let rest = &bytes[1..];
    let mut i = 0usize;
    #[cfg(target_arch = "aarch64")]
    {
        if rest.len() >= 16 && std::arch::is_aarch64_feature_detected!("neon") {
            unsafe {
                use std::arch::aarch64::*;
                let upper_a = vdupq_n_u8(b'A');
                let upper_z = vdupq_n_u8(b'Z');
                let case_bit = vdupq_n_u8(0x20);
                while i + 16 <= rest.len() {
                    let v = vld1q_u8(rest.as_ptr().add(i));
                    let is_upper = vandq_u8(vcgeq_u8(v, upper_a), vcleq_u8(v, upper_z));
                    let to_lower = vandq_u8(is_upper, case_bit);
                    let result = vorrq_u8(v, to_lower);
                    vst1q_u8(out.as_mut_ptr().add(1 + i), result);
                    i += 16;
                }
            }
        }
    }
    #[cfg(target_arch = "x86_64")]
    {
        if rest.len() >= 16 && std::arch::is_x86_feature_detected!("sse2") {
            unsafe {
                use std::arch::x86_64::*;
                let case_bit = _mm_set1_epi8(0x20);
                while i + 16 <= rest.len() {
                    let v = _mm_loadu_si128(rest.as_ptr().add(i) as *const __m128i);
                    let ge_a = _mm_cmpgt_epi8(v, _mm_set1_epi8(b'A' as i8 - 1));
                    let le_z = _mm_cmpgt_epi8(_mm_set1_epi8(b'Z' as i8 + 1), v);
                    let is_upper = _mm_and_si128(ge_a, le_z);
                    let to_lower = _mm_and_si128(is_upper, case_bit);
                    let result = _mm_or_si128(v, to_lower);
                    _mm_storeu_si128(out.as_mut_ptr().add(1 + i) as *mut __m128i, result);
                    i += 16;
                }
            }
        }
    }
    // Scalar tail
    for j in i..rest.len() {
        out[1 + j] = if rest[j].is_ascii_uppercase() {
            rest[j].to_ascii_lowercase()
        } else {
            rest[j]
        };
    }
    out
}

pub(crate) fn bytes_ascii_swapcase(bytes: &[u8]) -> Vec<u8> {
    let mut out = vec![0u8; bytes.len()];
    let mut i = 0usize;
    // SIMD fast path: toggle bit 5 on alphabetic bytes (16 bytes at a time)
    #[cfg(target_arch = "aarch64")]
    {
        if bytes.len() >= 16 && std::arch::is_aarch64_feature_detected!("neon") {
            unsafe {
                use std::arch::aarch64::*;
                let lower_a = vdupq_n_u8(b'a');
                let lower_z = vdupq_n_u8(b'z');
                let upper_a = vdupq_n_u8(b'A');
                let upper_z = vdupq_n_u8(b'Z');
                let case_bit = vdupq_n_u8(0x20);
                while i + 16 <= bytes.len() {
                    let v = vld1q_u8(bytes.as_ptr().add(i));
                    let is_lower = vandq_u8(vcgeq_u8(v, lower_a), vcleq_u8(v, lower_z));
                    let is_upper = vandq_u8(vcgeq_u8(v, upper_a), vcleq_u8(v, upper_z));
                    let is_alpha = vorrq_u8(is_lower, is_upper);
                    let flip = vandq_u8(is_alpha, case_bit);
                    let result = veorq_u8(v, flip);
                    vst1q_u8(out.as_mut_ptr().add(i), result);
                    i += 16;
                }
            }
        }
    }
    #[cfg(target_arch = "x86_64")]
    {
        if bytes.len() >= 16 && std::arch::is_x86_feature_detected!("sse2") {
            unsafe {
                use std::arch::x86_64::*;
                let case_bit = _mm_set1_epi8(0x20);
                while i + 16 <= bytes.len() {
                    let v = _mm_loadu_si128(bytes.as_ptr().add(i) as *const __m128i);
                    // Check lower: a <= v <= z (use unsigned saturation trick)
                    let shifted = _mm_or_si128(v, case_bit); // force to lowercase
                    let ge_a = _mm_cmpgt_epi8(shifted, _mm_set1_epi8(b'a' as i8 - 1));
                    let le_z = _mm_cmpgt_epi8(_mm_set1_epi8(b'z' as i8 + 1), shifted);
                    let is_alpha = _mm_and_si128(ge_a, le_z);
                    let flip = _mm_and_si128(is_alpha, case_bit);
                    let result = _mm_xor_si128(v, flip);
                    _mm_storeu_si128(out.as_mut_ptr().add(i) as *mut __m128i, result);
                    i += 16;
                }
            }
        }
    }
    // Scalar tail
    for j in i..bytes.len() {
        let b = bytes[j];
        out[j] = if b.is_ascii_lowercase() {
            b.to_ascii_uppercase()
        } else if b.is_ascii_uppercase() {
            b.to_ascii_lowercase()
        } else {
            b
        };
    }
    out
}

pub(crate) fn bytes_ascii_title(bytes: &[u8]) -> Vec<u8> {
    let mut out = vec![0u8; bytes.len()];
    let mut i = 0usize;
    let mut at_word_start = true;

    // SIMD fast path: process 16 bytes at a time.
    // For each chunk, classify bytes as alpha/non-alpha, then compute word-start
    // boundaries based on the at_word_start carry from the previous chunk.
    // Title case = uppercase at word start, lowercase otherwise, for alpha bytes.
    #[cfg(target_arch = "aarch64")]
    {
        if bytes.len() >= 16 && std::arch::is_aarch64_feature_detected!("neon") {
            unsafe {
                use std::arch::aarch64::*;
                let lower_a = vdupq_n_u8(b'a');
                let lower_z = vdupq_n_u8(b'z');
                let upper_a = vdupq_n_u8(b'A');
                let upper_z = vdupq_n_u8(b'Z');

                while i + 16 <= bytes.len() {
                    let v = vld1q_u8(bytes.as_ptr().add(i));
                    let is_lower = vandq_u8(vcgeq_u8(v, lower_a), vcleq_u8(v, lower_z));
                    let is_upper = vandq_u8(vcgeq_u8(v, upper_a), vcleq_u8(v, upper_z));
                    let is_alpha = vorrq_u8(is_lower, is_upper);

                    // Extract alpha mask to do sequential word-boundary tracking
                    let mut alpha_bytes = [0u8; 16];
                    vst1q_u8(alpha_bytes.as_mut_ptr(), is_alpha);
                    let mut src_bytes = [0u8; 16];
                    vst1q_u8(src_bytes.as_mut_ptr(), v);
                    let mut result_bytes = [0u8; 16];

                    for j in 0..16 {
                        let b = src_bytes[j];
                        if alpha_bytes[j] != 0 {
                            if at_word_start {
                                result_bytes[j] = b & !0x20; // to_ascii_uppercase
                                at_word_start = false;
                            } else {
                                result_bytes[j] = b | 0x20; // to_ascii_lowercase
                            }
                        } else {
                            result_bytes[j] = b;
                            at_word_start = true;
                        }
                    }

                    let result = vld1q_u8(result_bytes.as_ptr());
                    vst1q_u8(out.as_mut_ptr().add(i), result);
                    i += 16;
                }
            }
        }
    }
    #[cfg(target_arch = "x86_64")]
    {
        if bytes.len() >= 16 && std::arch::is_x86_feature_detected!("sse2") {
            unsafe {
                use std::arch::x86_64::*;
                let case_bit = _mm_set1_epi8(0x20);

                while i + 16 <= bytes.len() {
                    let v = _mm_loadu_si128(bytes.as_ptr().add(i) as *const __m128i);
                    let shifted = _mm_or_si128(v, case_bit);
                    let ge_a = _mm_cmpgt_epi8(shifted, _mm_set1_epi8(b'a' as i8 - 1));
                    let le_z = _mm_cmpgt_epi8(_mm_set1_epi8(b'z' as i8 + 1), shifted);
                    let is_alpha = _mm_and_si128(ge_a, le_z);
                    let alpha_mask = _mm_movemask_epi8(is_alpha) as u32;

                    let mut src_bytes = [0u8; 16];
                    _mm_storeu_si128(src_bytes.as_mut_ptr() as *mut __m128i, v);
                    let mut result_bytes = [0u8; 16];

                    for j in 0..16 {
                        let b = src_bytes[j];
                        if alpha_mask & (1 << j) != 0 {
                            if at_word_start {
                                result_bytes[j] = b & !0x20;
                                at_word_start = false;
                            } else {
                                result_bytes[j] = b | 0x20;
                            }
                        } else {
                            result_bytes[j] = b;
                            at_word_start = true;
                        }
                    }

                    let result = _mm_loadu_si128(result_bytes.as_ptr() as *const __m128i);
                    _mm_storeu_si128(out.as_mut_ptr().add(i) as *mut __m128i, result);
                    i += 16;
                }
            }
        }
    }

    // Scalar tail
    for j in i..bytes.len() {
        let b = bytes[j];
        if b.is_ascii_alphabetic() {
            if at_word_start {
                out[j] = b.to_ascii_uppercase();
                at_word_start = false;
            } else {
                out[j] = b.to_ascii_lowercase();
            }
        } else {
            out[j] = b;
            at_word_start = true;
        }
    }
    out
}
