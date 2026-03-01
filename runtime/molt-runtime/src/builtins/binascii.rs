use crate::*;

const BASE64_TABLE: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";

/// Hardware-accelerated CRC32 on AArch64 using the CRC extension instructions.
/// Apple Silicon (M1/M2/M3/M4) always supports these. Processes 8 bytes per
/// iteration using `__crc32d`, falling back to per-byte `__crc32b` for remainder.
#[cfg(target_arch = "aarch64")]
unsafe fn crc32_hw_aarch64(data: &[u8]) -> u32 {
    unsafe {
        use std::arch::aarch64::*;
        let mut crc = 0xFFFF_FFFFu32;
        let mut i = 0usize;
        // Process 8 bytes at a time using CRC32 doubleword instruction
        while i + 8 <= data.len() {
            let chunk = u64::from_le_bytes([
                data[i], data[i + 1], data[i + 2], data[i + 3],
                data[i + 4], data[i + 5], data[i + 6], data[i + 7],
            ]);
            crc = __crc32d(crc, chunk);
            i += 8;
        }
        // Process remaining bytes one at a time
        while i < data.len() {
            crc = __crc32b(crc, data[i]);
            i += 1;
        }
        crc ^ 0xFFFF_FFFF
    }
}

fn bytes_like_arg(_py: &PyToken<'_>, bits: u64, func: &str) -> Result<Vec<u8>, u64> {
    let obj = obj_from_bits(bits);
    let Some(ptr) = obj.as_ptr() else {
        let msg = format!("{func}() argument 1 must be bytes-like, not None");
        return Err(raise_exception::<_>(_py, "TypeError", &msg));
    };
    let Some(raw) = (unsafe { bytes_like_slice(ptr) }) else {
        let type_name = type_name(_py, obj);
        let msg = format!("a bytes-like object is required, not '{type_name}'");
        return Err(raise_exception::<_>(_py, "TypeError", &msg));
    };
    Ok(raw.to_vec())
}

fn ascii_or_bytes_arg(_py: &PyToken<'_>, bits: u64, _func: &str) -> Result<Vec<u8>, u64> {
    let obj = obj_from_bits(bits);
    if let Some(text) = string_obj_to_owned(obj) {
        return Ok(text.into_bytes());
    }
    bytes_like_arg(_py, bits, "a2b")
}

fn alloc_bytes_or_oom(_py: &PyToken<'_>, data: &[u8], context: &str) -> u64 {
    let ptr = alloc_bytes(_py, data);
    if ptr.is_null() {
        let msg = format!("{context}: out of memory");
        return raise_exception::<_>(_py, "MemoryError", &msg);
    }
    MoltObject::from_ptr(ptr).bits()
}

/// SIMD-accelerated whitespace stripping: removes ASCII whitespace bytes
/// (\t, \n, \r, \x0c, ' ') from input using NEON/SSE2 bulk classification.
fn simd_strip_whitespace(input: &[u8]) -> Vec<u8> {
    let mut out: Vec<u8> = Vec::with_capacity(input.len());
    let mut i = 0usize;

    #[cfg(target_arch = "aarch64")]
    {
        unsafe {
            use std::arch::aarch64::*;
            let space = vdupq_n_u8(b' ');
            let tab = vdupq_n_u8(b'\t');
            let nl = vdupq_n_u8(b'\n');
            let cr = vdupq_n_u8(b'\r');
            let ff = vdupq_n_u8(0x0C); // form feed
            while i + 16 <= input.len() {
                let chunk = vld1q_u8(input.as_ptr().add(i));
                let is_ws = vorrq_u8(
                    vorrq_u8(
                        vorrq_u8(vceqq_u8(chunk, space), vceqq_u8(chunk, tab)),
                        vceqq_u8(chunk, nl),
                    ),
                    vorrq_u8(vceqq_u8(chunk, cr), vceqq_u8(chunk, ff)),
                );
                // Fast path: if no whitespace in this chunk, copy all 16 bytes
                if vmaxvq_u8(is_ws) == 0 {
                    let len = out.len();
                    out.set_len(len + 16);
                    vst1q_u8(out.as_mut_ptr().add(len), chunk);
                } else {
                    // Slow path: copy non-whitespace bytes one by one
                    for j in 0..16 {
                        let b = input[i + j];
                        if !b.is_ascii_whitespace() {
                            out.push(b);
                        }
                    }
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
            let ff = _mm_set1_epi8(0x0C);
            while i + 16 <= input.len() {
                let chunk = _mm_loadu_si128(input.as_ptr().add(i) as *const __m128i);
                let is_ws = _mm_or_si128(
                    _mm_or_si128(
                        _mm_or_si128(_mm_cmpeq_epi8(chunk, space), _mm_cmpeq_epi8(chunk, tab)),
                        _mm_cmpeq_epi8(chunk, nl),
                    ),
                    _mm_or_si128(_mm_cmpeq_epi8(chunk, cr), _mm_cmpeq_epi8(chunk, ff)),
                );
                let mask = _mm_movemask_epi8(is_ws) as u32;
                if mask == 0 {
                    let len = out.len();
                    out.set_len(len + 16);
                    _mm_storeu_si128(out.as_mut_ptr().add(len) as *mut __m128i, chunk);
                } else {
                    for j in 0..16 {
                        let b = input[i + j];
                        if !b.is_ascii_whitespace() {
                            out.push(b);
                        }
                    }
                }
                i += 16;
            }
        }
    }

    #[cfg(target_arch = "wasm32")]
    {
        if cfg!(target_feature = "simd128") {
            unsafe {
                use std::arch::wasm32::*;
                let space = u8x16_splat(b' ');
                let tab = u8x16_splat(b'\t');
                let nl = u8x16_splat(b'\n');
                let cr = u8x16_splat(b'\r');
                let ff = u8x16_splat(0x0C);
                while i + 16 <= input.len() {
                    let chunk = v128_load(input.as_ptr().add(i) as *const v128);
                    let is_ws = v128_or(
                        v128_or(
                            v128_or(u8x16_eq(chunk, space), u8x16_eq(chunk, tab)),
                            u8x16_eq(chunk, nl),
                        ),
                        v128_or(u8x16_eq(chunk, cr), u8x16_eq(chunk, ff)),
                    );
                    let mask = u8x16_bitmask(is_ws) as u32;
                    if mask == 0 {
                        let len = out.len();
                        out.set_len(len + 16);
                        v128_store(out.as_mut_ptr().add(len) as *mut v128, chunk);
                    } else {
                        for j in 0..16 {
                            let b = input[i + j];
                            if !b.is_ascii_whitespace() {
                                out.push(b);
                            }
                        }
                    }
                    i += 16;
                }
            }
        }
    }

    // Scalar tail
    while i < input.len() {
        let b = input[i];
        if !b.is_ascii_whitespace() {
            out.push(b);
        }
        i += 1;
    }
    out
}

fn base64_value(byte: u8) -> Option<u8> {
    match byte {
        b'A'..=b'Z' => Some(byte - b'A'),
        b'a'..=b'z' => Some(byte - b'a' + 26),
        b'0'..=b'9' => Some(byte - b'0' + 52),
        b'+' => Some(62),
        b'/' => Some(63),
        _ => None,
    }
}

fn base64_decode(input: &[u8]) -> Result<Vec<u8>, &'static str> {
    let compact = simd_strip_whitespace(input);
    if compact.is_empty() {
        return Ok(Vec::new());
    }
    if !compact.len().is_multiple_of(4) {
        return Err("Incorrect padding");
    }
    let mut out = Vec::with_capacity(compact.len() / 4 * 3);
    let mut idx = 0usize;
    while idx < compact.len() {
        let c0 = compact[idx];
        let c1 = compact[idx + 1];
        let c2 = compact[idx + 2];
        let c3 = compact[idx + 3];

        let Some(v0) = base64_value(c0) else {
            return Err("Non-base64 digit found");
        };
        let Some(v1) = base64_value(c1) else {
            return Err("Non-base64 digit found");
        };
        let pad2 = c2 == b'=';
        let pad3 = c3 == b'=';
        let v2 = if pad2 {
            0
        } else if let Some(v) = base64_value(c2) {
            v
        } else {
            return Err("Non-base64 digit found");
        };
        let v3 = if pad3 {
            0
        } else if let Some(v) = base64_value(c3) {
            v
        } else {
            return Err("Non-base64 digit found");
        };
        if pad2 && !pad3 {
            return Err("Incorrect padding");
        }

        out.push((v0 << 2) | (v1 >> 4));
        if !pad2 {
            out.push(((v1 & 0x0F) << 4) | (v2 >> 2));
        }
        if !pad3 {
            out.push(((v2 & 0x03) << 6) | v3);
        }
        idx += 4;
    }
    Ok(out)
}

fn base64_encode(input: &[u8]) -> Vec<u8> {
    let mut out: Vec<u8> = Vec::with_capacity(input.len().div_ceil(3) * 4 + 1);
    let mut idx = 0usize;

    // 4× unrolled encode loop: process 12 bytes → 16 base64 chars per iteration.
    // Unrolling helps LLVM auto-vectorize on both aarch64 (NEON) and x86_64 (AVX2).
    while idx + 12 <= input.len() {
        for _ in 0..4 {
            let b0 = input[idx] as u32;
            let b1 = input[idx + 1] as u32;
            let b2 = input[idx + 2] as u32;
            let n = (b0 << 16) | (b1 << 8) | b2;
            out.push(BASE64_TABLE[((n >> 18) & 0x3f) as usize]);
            out.push(BASE64_TABLE[((n >> 12) & 0x3f) as usize]);
            out.push(BASE64_TABLE[((n >> 6) & 0x3f) as usize]);
            out.push(BASE64_TABLE[(n & 0x3f) as usize]);
            idx += 3;
        }
    }

    // Scalar tail (handles remaining bytes + padding)
    while idx < input.len() {
        let b0 = input[idx];
        let b1 = if idx + 1 < input.len() {
            input[idx + 1]
        } else {
            0
        };
        let b2 = if idx + 2 < input.len() {
            input[idx + 2]
        } else {
            0
        };
        let n = ((b0 as u32) << 16) | ((b1 as u32) << 8) | (b2 as u32);
        out.push(BASE64_TABLE[((n >> 18) & 0x3f) as usize]);
        out.push(BASE64_TABLE[((n >> 12) & 0x3f) as usize]);
        if idx + 1 < input.len() {
            out.push(BASE64_TABLE[((n >> 6) & 0x3f) as usize]);
        } else {
            out.push(b'=');
        }
        if idx + 2 < input.len() {
            out.push(BASE64_TABLE[(n & 0x3f) as usize]);
        } else {
            out.push(b'=');
        }
        idx += 3;
    }
    out.push(b'\n');
    out
}

fn hex_nibble(b: u8) -> Option<u8> {
    match b {
        b'0'..=b'9' => Some(b - b'0'),
        b'a'..=b'f' => Some(b - b'a' + 10),
        b'A'..=b'F' => Some(b - b'A' + 10),
        _ => None,
    }
}

/// SIMD-accelerated hex encoding: converts bytes to hex string.
/// Processes 16 bytes at a time on NEON (aarch64) or SSE2 (x86_64),
/// producing 32 hex characters per iteration using vector table lookups.
fn simd_hex_encode(input: &[u8]) -> Vec<u8> {
    let mut out: Vec<u8> = Vec::with_capacity(input.len() * 2);
    let mut i = 0usize;

    #[cfg(target_arch = "aarch64")]
    {
        unsafe {
            use std::arch::aarch64::*;
            // Hex lookup table: "0123456789abcdef"
            let hex_lut = vld1q_u8(b"0123456789abcdef".as_ptr());
            let mask_lo = vdupq_n_u8(0x0F);
            while i + 16 <= input.len() {
                let chunk = vld1q_u8(input.as_ptr().add(i));
                // Split each byte into high and low nibbles
                let hi_nibbles = vshrq_n_u8(chunk, 4);
                let lo_nibbles = vandq_u8(chunk, mask_lo);
                // Table lookup to convert nibbles to hex chars
                let hi_hex = vqtbl1q_u8(hex_lut, hi_nibbles);
                let lo_hex = vqtbl1q_u8(hex_lut, lo_nibbles);
                // Interleave: hi0 lo0 hi1 lo1 ...
                let zipped_lo = vzip1q_u8(hi_hex, lo_hex);
                let zipped_hi = vzip2q_u8(hi_hex, lo_hex);
                // Write 32 bytes
                let len = out.len();
                out.set_len(len + 32);
                vst1q_u8(out.as_mut_ptr().add(len), zipped_lo);
                vst1q_u8(out.as_mut_ptr().add(len + 16), zipped_hi);
                i += 16;
            }
        }
    }

    #[cfg(target_arch = "x86_64")]
    {
        unsafe {
            use std::arch::x86_64::*;
            let mask_lo = _mm_set1_epi8(0x0F);
            // Build hex lookup: indices 0..15 map to '0'..'f'
            let hex_lut = _mm_setr_epi8(
                b'0' as i8, b'1' as i8, b'2' as i8, b'3' as i8,
                b'4' as i8, b'5' as i8, b'6' as i8, b'7' as i8,
                b'8' as i8, b'9' as i8, b'a' as i8, b'b' as i8,
                b'c' as i8, b'd' as i8, b'e' as i8, b'f' as i8,
            );
            while i + 16 <= input.len() {
                let chunk = _mm_loadu_si128(input.as_ptr().add(i) as *const __m128i);
                let hi_nibbles = _mm_and_si128(_mm_srli_epi16(chunk, 4), mask_lo);
                let lo_nibbles = _mm_and_si128(chunk, mask_lo);
                let hi_hex = _mm_shuffle_epi8(hex_lut, hi_nibbles);
                let lo_hex = _mm_shuffle_epi8(hex_lut, lo_nibbles);
                // Interleave
                let interleaved_lo = _mm_unpacklo_epi8(hi_hex, lo_hex);
                let interleaved_hi = _mm_unpackhi_epi8(hi_hex, lo_hex);
                let len = out.len();
                out.set_len(len + 32);
                _mm_storeu_si128(out.as_mut_ptr().add(len) as *mut __m128i, interleaved_lo);
                _mm_storeu_si128(out.as_mut_ptr().add(len + 16) as *mut __m128i, interleaved_hi);
                i += 16;
            }
        }
    }

    // Scalar tail
    const HEX: &[u8; 16] = b"0123456789abcdef";
    while i < input.len() {
        let b = input[i];
        out.push(HEX[(b >> 4) as usize]);
        out.push(HEX[(b & 0x0f) as usize]);
        i += 1;
    }
    out
}

/// SIMD-accelerated hex decoding: converts hex string to bytes.
/// Validates that all characters are valid hex digits and processes
/// 32 hex chars (16 output bytes) per SIMD iteration.
/// Returns None if any non-hex character is found.
fn simd_hex_decode(input: &[u8]) -> Option<Vec<u8>> {
    if !input.len().is_multiple_of(2) {
        return None;
    }
    let mut out: Vec<u8> = Vec::with_capacity(input.len() / 2);
    let mut i = 0usize;

    #[cfg(target_arch = "aarch64")]
    {
        unsafe {
            use std::arch::aarch64::*;
            while i + 32 <= input.len() {
                let lo_chars = vld1q_u8(input.as_ptr().add(i));
                let hi_chars = vld1q_u8(input.as_ptr().add(i + 16));
                // Deinterleave: separate hi-nibble chars and lo-nibble chars
                let pairs = vuzpq_u8(lo_chars, hi_chars);
                let hi_nibble_chars = pairs.0; // chars at even positions (hi nibbles)
                let lo_nibble_chars = pairs.1; // chars at odd positions (lo nibbles)
                // Convert ASCII hex chars to nibble values using range checks
                let hi_vals = hex_chars_to_nibbles_neon(hi_nibble_chars);
                let lo_vals = hex_chars_to_nibbles_neon(lo_nibble_chars);
                // Check for invalid (0xFF sentinel)
                let invalid_hi = vceqq_u8(hi_vals, vdupq_n_u8(0xFF));
                let invalid_lo = vceqq_u8(lo_vals, vdupq_n_u8(0xFF));
                let any_invalid = vorrq_u8(invalid_hi, invalid_lo);
                if vmaxvq_u8(any_invalid) != 0 {
                    return None; // Contains non-hex chars
                }
                // Combine nibbles: (hi << 4) | lo
                let result = vorrq_u8(vshlq_n_u8(hi_vals, 4), lo_vals);
                let len = out.len();
                out.set_len(len + 16);
                vst1q_u8(out.as_mut_ptr().add(len), result);
                i += 32;
            }
        }
    }

    #[cfg(target_arch = "x86_64")]
    {
        unsafe {
            use std::arch::x86_64::*;
            while i + 32 <= input.len() {
                let lo_block = _mm_loadu_si128(input.as_ptr().add(i) as *const __m128i);
                let hi_block = _mm_loadu_si128(input.as_ptr().add(i + 16) as *const __m128i);
                // Deinterleave: separate hi-nibble and lo-nibble chars
                let mask = _mm_set_epi8(14,12,10,8,6,4,2,0, 15,13,11,9,7,5,3,1);
                let lo_shuffled = _mm_shuffle_epi8(lo_block, mask);
                let hi_shuffled = _mm_shuffle_epi8(hi_block, mask);
                // hi_nibble_chars = bytes at even positions from both blocks
                let hi_nibble_chars = _mm_unpacklo_epi64(lo_shuffled, hi_shuffled);
                let lo_nibble_chars = _mm_unpackhi_epi64(lo_shuffled, hi_shuffled);
                let hi_vals = hex_chars_to_nibbles_sse2(hi_nibble_chars);
                let lo_vals = hex_chars_to_nibbles_sse2(lo_nibble_chars);
                // Check for 0xFF sentinel
                let sentinel = _mm_set1_epi8(0xFFu8 as i8);
                let hi_bad = _mm_cmpeq_epi8(hi_vals, sentinel);
                let lo_bad = _mm_cmpeq_epi8(lo_vals, sentinel);
                let any_bad = _mm_or_si128(hi_bad, lo_bad);
                if _mm_movemask_epi8(any_bad) != 0 {
                    return None;
                }
                let result = _mm_or_si128(_mm_slli_epi16(hi_vals, 4), lo_vals);
                // Mask off the high bits that leaked from slli_epi16
                let nibble_mask = _mm_set1_epi8(0x0F);
                let hi_shifted = _mm_and_si128(_mm_slli_epi16(hi_vals, 4), _mm_set1_epi8(0xF0u8 as i8));
                let lo_masked = _mm_and_si128(lo_vals, nibble_mask);
                let result = _mm_or_si128(hi_shifted, lo_masked);
                let len = out.len();
                out.set_len(len + 16);
                _mm_storeu_si128(out.as_mut_ptr().add(len) as *mut __m128i, result);
                i += 32;
            }
        }
    }

    // Scalar tail
    while i < input.len() {
        let hi = hex_nibble(input[i])?;
        let lo = hex_nibble(input[i + 1])?;
        out.push((hi << 4) | lo);
        i += 2;
    }
    Some(out)
}

/// NEON helper: convert hex ASCII chars to nibble values (0-15),
/// returning 0xFF for invalid characters.
#[cfg(target_arch = "aarch64")]
#[inline(always)]
unsafe fn hex_chars_to_nibbles_neon(chars: std::arch::aarch64::uint8x16_t) -> std::arch::aarch64::uint8x16_t {
    unsafe {
        use std::arch::aarch64::*;
        let zero = vdupq_n_u8(b'0');
        let nine = vdupq_n_u8(b'9');
        let a_lower = vdupq_n_u8(b'a');
        let f_lower = vdupq_n_u8(b'f');
        let a_upper = vdupq_n_u8(b'A');
        let f_upper = vdupq_n_u8(b'F');
        let invalid = vdupq_n_u8(0xFF);

        // Check digit range: '0'-'9' → subtract '0'
        let is_digit = vandq_u8(vcgeq_u8(chars, zero), vcleq_u8(chars, nine));
        let digit_val = vsubq_u8(chars, zero);

        // Check lowercase range: 'a'-'f' → subtract 'a' + 10
        let is_lower = vandq_u8(vcgeq_u8(chars, a_lower), vcleq_u8(chars, f_lower));
        let lower_val = vaddq_u8(vsubq_u8(chars, a_lower), vdupq_n_u8(10));

        // Check uppercase range: 'A'-'F' → subtract 'A' + 10
        let is_upper = vandq_u8(vcgeq_u8(chars, a_upper), vcleq_u8(chars, f_upper));
        let upper_val = vaddq_u8(vsubq_u8(chars, a_upper), vdupq_n_u8(10));

        // Select: digit → digit_val, lower → lower_val, upper → upper_val, else → 0xFF
        let result = vbslq_u8(is_digit, digit_val, invalid);
        let result = vbslq_u8(is_lower, lower_val, result);
        vbslq_u8(is_upper, upper_val, result)
    }
}

/// SSE2 helper: convert hex ASCII chars to nibble values (0-15),
/// returning 0xFF for invalid characters.
#[cfg(target_arch = "x86_64")]
#[inline(always)]
unsafe fn hex_chars_to_nibbles_sse2(chars: std::arch::x86_64::__m128i) -> std::arch::x86_64::__m128i {
    unsafe {
        use std::arch::x86_64::*;
        let zero_char = _mm_set1_epi8(b'0' as i8);
        let a_lower = _mm_set1_epi8(b'a' as i8);
        let a_upper = _mm_set1_epi8(b'A' as i8);
        let invalid = _mm_set1_epi8(0xFFu8 as i8);

        // Digit check: chars >= '0' && chars <= '9'
        let ge_zero = _mm_cmpgt_epi8(chars, _mm_set1_epi8((b'0' - 1) as i8));
        let le_nine = _mm_cmpgt_epi8(_mm_set1_epi8((b'9' + 1) as i8), chars);
        let is_digit = _mm_and_si128(ge_zero, le_nine);
        let digit_val = _mm_sub_epi8(chars, zero_char);

        // Lowercase check: 'a'-'f'
        let ge_a = _mm_cmpgt_epi8(chars, _mm_set1_epi8((b'a' - 1) as i8));
        let le_f = _mm_cmpgt_epi8(_mm_set1_epi8((b'f' + 1) as i8), chars);
        let is_lower = _mm_and_si128(ge_a, le_f);
        let lower_val = _mm_add_epi8(_mm_sub_epi8(chars, a_lower), _mm_set1_epi8(10));

        // Uppercase check: 'A'-'F'
        let ge_upper = _mm_cmpgt_epi8(chars, _mm_set1_epi8((b'A' - 1) as i8));
        let le_upper = _mm_cmpgt_epi8(_mm_set1_epi8((b'F' + 1) as i8), chars);
        let is_upper = _mm_and_si128(ge_upper, le_upper);
        let upper_val = _mm_add_epi8(_mm_sub_epi8(chars, a_upper), _mm_set1_epi8(10));

        // Blend: start with invalid, override with matches
        let result = _mm_or_si128(
            _mm_and_si128(is_digit, digit_val),
            _mm_andnot_si128(is_digit, invalid),
        );
        let result = _mm_or_si128(
            _mm_and_si128(is_lower, lower_val),
            _mm_andnot_si128(is_lower, result),
        );
        _mm_or_si128(
            _mm_and_si128(is_upper, upper_val),
            _mm_andnot_si128(is_upper, result),
        )
    }
}

fn uu_val(ch: u8) -> Option<u8> {
    if ch == b'`' {
        return Some(0);
    }
    if !(b' '..=b'_').contains(&ch) {
        return None;
    }
    Some((ch - b' ') & 0x3f)
}

fn uu_decode(input: &[u8]) -> Result<Vec<u8>, &'static str> {
    let mut line = input;
    while let Some(last) = line.last() {
        if *last == b'\n' || *last == b'\r' {
            line = &line[..line.len() - 1];
        } else {
            break;
        }
    }
    if line.is_empty() {
        return Ok(Vec::new());
    }
    let Some(want) = uu_val(line[0]) else {
        return Err("Illegal char");
    };
    let want_len = usize::from(want);
    let groups = want_len.div_ceil(3);
    if line.len() < 1 + groups * 4 {
        return Err("Truncated input");
    }
    let mut out = Vec::with_capacity(groups * 3);
    let mut idx = 1usize;
    for _ in 0..groups {
        let Some(a) = uu_val(line[idx]) else {
            return Err("Illegal char");
        };
        let Some(b) = uu_val(line[idx + 1]) else {
            return Err("Illegal char");
        };
        let Some(c) = uu_val(line[idx + 2]) else {
            return Err("Illegal char");
        };
        let Some(d) = uu_val(line[idx + 3]) else {
            return Err("Illegal char");
        };
        out.push((a << 2) | (b >> 4));
        out.push(((b & 0x0f) << 4) | (c >> 2));
        out.push(((c & 0x03) << 6) | d);
        idx += 4;
    }
    out.truncate(want_len);
    Ok(out)
}

fn uu_encode(input: &[u8]) -> Result<Vec<u8>, &'static str> {
    if input.len() > 45 {
        return Err("At most 45 bytes at once");
    }
    let mut out = Vec::with_capacity(2 + input.len().div_ceil(3) * 4);
    out.push(((input.len() as u8) & 0x3f) + b' ');
    let mut idx = 0usize;
    while idx < input.len() {
        let b0 = input[idx];
        let b1 = if idx + 1 < input.len() {
            input[idx + 1]
        } else {
            0
        };
        let b2 = if idx + 2 < input.len() {
            input[idx + 2]
        } else {
            0
        };
        let c0 = ((b0 >> 2) & 0x3f) + b' ';
        let c1 = (((b0 << 4) | (b1 >> 4)) & 0x3f) + b' ';
        let c2 = (((b1 << 2) | (b2 >> 6)) & 0x3f) + b' ';
        let c3 = (b2 & 0x3f) + b' ';
        out.extend_from_slice(&[c0, c1, c2, c3]);
        idx += 3;
    }
    out.push(b'\n');
    Ok(out)
}

fn qp_decode(input: &[u8]) -> Vec<u8> {
    let mut out = Vec::with_capacity(input.len());
    let mut idx = 0usize;
    // Use memchr (SIMD-backed) to find '=' markers, bulk-copying safe spans
    while idx < input.len() {
        if let Some(eq_pos) = memchr::memchr(b'=', &input[idx..]) {
            // Bulk copy the safe bytes before the '='
            if eq_pos > 0 {
                out.extend_from_slice(&input[idx..idx + eq_pos]);
            }
            idx += eq_pos;
            // Handle the '=' escape
            if idx + 2 < input.len() {
                if input[idx + 1] == b'\r' && input[idx + 2] == b'\n' {
                    idx += 3;
                    continue;
                }
                if input[idx + 1] == b'\n' {
                    idx += 2;
                    continue;
                }
                if let (Some(a), Some(b)) =
                    (hex_nibble(input[idx + 1]), hex_nibble(input[idx + 2]))
                {
                    out.push((a << 4) | b);
                    idx += 3;
                    continue;
                }
            } else if idx + 1 < input.len() && input[idx + 1] == b'\n' {
                idx += 2;
                continue;
            }
            // Not a valid escape — pass through the '='
            out.push(input[idx]);
            idx += 1;
        } else {
            // No more '=' in the remainder — bulk copy everything
            out.extend_from_slice(&input[idx..]);
            break;
        }
    }
    out
}

fn qp_encode(input: &[u8]) -> Vec<u8> {
    const HEX: &[u8; 16] = b"0123456789ABCDEF";
    let mut out: Vec<u8> = Vec::with_capacity(input.len() * 3 / 2);
    let mut i = 0usize;

    // SIMD fast path: scan 16 bytes at a time for passthrough characters.
    // QP passthrough: ' '..='~' (except '='), '\n', '\r'
    #[cfg(target_arch = "aarch64")]
    {
        if input.len() >= 16 && std::arch::is_aarch64_feature_detected!("neon") {
            unsafe {
                use std::arch::aarch64::*;
                let space = vdupq_n_u8(b' ');
                let tilde = vdupq_n_u8(b'~');
                let eq_char = vdupq_n_u8(b'=');
                let nl = vdupq_n_u8(b'\n');
                let cr = vdupq_n_u8(b'\r');

                while i + 16 <= input.len() {
                    let v = vld1q_u8(input.as_ptr().add(i));
                    // Printable range: space <= v <= tilde
                    let is_printable = vandq_u8(vcgeq_u8(v, space), vcleq_u8(v, tilde));
                    let is_not_eq = vmvnq_u8(vceqq_u8(v, eq_char));
                    let is_safe_printable = vandq_u8(is_printable, is_not_eq);
                    let is_nl = vceqq_u8(v, nl);
                    let is_cr = vceqq_u8(v, cr);
                    let is_passthrough = vorrq_u8(is_safe_printable, vorrq_u8(is_nl, is_cr));

                    // If all 16 bytes are passthrough, bulk copy
                    if vminvq_u8(is_passthrough) == 0xFF {
                        let len = out.len();
                        out.set_len(len + 16);
                        vst1q_u8(out.as_mut_ptr().add(len), v);
                        i += 16;
                    } else {
                        // Fall back to scalar for this chunk
                        break;
                    }
                }
            }
        }
    }
    #[cfg(target_arch = "x86_64")]
    {
        if input.len() >= 16 && std::arch::is_x86_feature_detected!("sse2") {
            unsafe {
                use std::arch::x86_64::*;
                let space = _mm_set1_epi8(b' ' as i8);
                let tilde = _mm_set1_epi8(b'~' as i8);
                let eq_char = _mm_set1_epi8(b'=' as i8);
                let nl = _mm_set1_epi8(b'\n' as i8);
                let cr = _mm_set1_epi8(b'\r' as i8);

                while i + 16 <= input.len() {
                    let v = _mm_loadu_si128(input.as_ptr().add(i) as *const __m128i);
                    // Printable: space-1 < v < tilde+1 (signed comparison)
                    let ge_space = _mm_cmpgt_epi8(v, _mm_set1_epi8(b' ' as i8 - 1));
                    let le_tilde = _mm_cmpgt_epi8(_mm_set1_epi8(b'~' as i8 + 1), v);
                    let is_printable = _mm_and_si128(ge_space, le_tilde);
                    // Not '='
                    let not_eq = _mm_andnot_si128(_mm_cmpeq_epi8(v, eq_char), _mm_set1_epi8(-1));
                    let safe_print = _mm_and_si128(is_printable, not_eq);
                    let is_nl = _mm_cmpeq_epi8(v, nl);
                    let is_cr = _mm_cmpeq_epi8(v, cr);
                    let passthrough = _mm_or_si128(safe_print, _mm_or_si128(is_nl, is_cr));

                    if _mm_movemask_epi8(passthrough) == 0xFFFF {
                        let len = out.len();
                        out.set_len(len + 16);
                        _mm_storeu_si128(out.as_mut_ptr().add(len) as *mut __m128i, v);
                        i += 16;
                    } else {
                        break;
                    }
                }
            }
        }
    }
    #[cfg(target_arch = "wasm32")]
    {
        if cfg!(target_feature = "simd128") && input.len() >= 16 {
            unsafe {
                use std::arch::wasm32::*;
                let space = u8x16_splat(b' ');
                let tilde = u8x16_splat(b'~');
                let eq_char = u8x16_splat(b'=');
                let nl = u8x16_splat(b'\n');
                let cr = u8x16_splat(b'\r');
                while i + 16 <= input.len() {
                    let v = v128_load(input.as_ptr().add(i) as *const v128);
                    let is_printable = v128_and(u8x16_ge(v, space), u8x16_le(v, tilde));
                    let is_not_eq = v128_not(u8x16_eq(v, eq_char));
                    let safe_print = v128_and(is_printable, is_not_eq);
                    let passthrough = v128_or(safe_print, v128_or(u8x16_eq(v, nl), u8x16_eq(v, cr)));
                    if u8x16_bitmask(passthrough) == 0xFFFF {
                        let len = out.len();
                        out.set_len(len + 16);
                        v128_store(out.as_mut_ptr().add(len) as *mut v128, v);
                        i += 16;
                    } else {
                        break;
                    }
                }
            }
        }
    }

    // Scalar path for remaining bytes
    for &b in &input[i..] {
        if b == b'\n' || b == b'\r' || (b' '..=b'~').contains(&b) && b != b'=' {
            out.push(b);
        } else {
            out.push(b'=');
            out.push(HEX[(b >> 4) as usize]);
            out.push(HEX[(b & 0x0f) as usize]);
        }
    }
    out
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_binascii_a2b_base64(data_bits: u64) -> u64 {
    crate::with_gil_entry!(_py, {
        let raw = match ascii_or_bytes_arg(_py, data_bits, "a2b_base64") {
            Ok(v) => v,
            Err(bits) => return bits,
        };
        let decoded = match base64_decode(&raw) {
            Ok(v) => v,
            Err(msg) => return raise_exception::<_>(_py, "ValueError", msg),
        };
        alloc_bytes_or_oom(_py, &decoded, "a2b_base64")
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_binascii_b2a_base64(data_bits: u64) -> u64 {
    crate::with_gil_entry!(_py, {
        let raw = match bytes_like_arg(_py, data_bits, "b2a_base64") {
            Ok(v) => v,
            Err(bits) => return bits,
        };
        let encoded = base64_encode(&raw);
        alloc_bytes_or_oom(_py, &encoded, "b2a_base64")
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_binascii_a2b_hex(data_bits: u64) -> u64 {
    crate::with_gil_entry!(_py, {
        let raw = match ascii_or_bytes_arg(_py, data_bits, "a2b_hex") {
            Ok(v) => v,
            Err(bits) => return bits,
        };
        if !raw.len().is_multiple_of(2) {
            return raise_exception::<_>(_py, "ValueError", "Odd-length string");
        }
        // Use SIMD-accelerated hex decode (NEON/SSE2)
        let out = match simd_hex_decode(&raw) {
            Some(v) => v,
            None => {
                return raise_exception::<_>(_py, "ValueError", "Non-hexadecimal digit found");
            }
        };
        alloc_bytes_or_oom(_py, &out, "a2b_hex")
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_binascii_b2a_hex(data_bits: u64) -> u64 {
    crate::with_gil_entry!(_py, {
        let raw = match bytes_like_arg(_py, data_bits, "b2a_hex") {
            Ok(v) => v,
            Err(bits) => return bits,
        };
        // Use SIMD-accelerated hex encode (NEON/SSE2)
        let out = simd_hex_encode(&raw);
        alloc_bytes_or_oom(_py, &out, "b2a_hex")
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_binascii_a2b_qp(data_bits: u64) -> u64 {
    crate::with_gil_entry!(_py, {
        let raw = match ascii_or_bytes_arg(_py, data_bits, "a2b_qp") {
            Ok(v) => v,
            Err(bits) => return bits,
        };
        let out = qp_decode(&raw);
        alloc_bytes_or_oom(_py, &out, "a2b_qp")
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_binascii_b2a_qp(data_bits: u64) -> u64 {
    crate::with_gil_entry!(_py, {
        let raw = match bytes_like_arg(_py, data_bits, "b2a_qp") {
            Ok(v) => v,
            Err(bits) => return bits,
        };
        let out = qp_encode(&raw);
        alloc_bytes_or_oom(_py, &out, "b2a_qp")
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_binascii_a2b_uu(data_bits: u64) -> u64 {
    crate::with_gil_entry!(_py, {
        let raw = match ascii_or_bytes_arg(_py, data_bits, "a2b_uu") {
            Ok(v) => v,
            Err(bits) => return bits,
        };
        let out = match uu_decode(&raw) {
            Ok(v) => v,
            Err(msg) => return raise_exception::<_>(_py, "ValueError", msg),
        };
        alloc_bytes_or_oom(_py, &out, "a2b_uu")
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_binascii_b2a_uu(data_bits: u64) -> u64 {
    crate::with_gil_entry!(_py, {
        let raw = match bytes_like_arg(_py, data_bits, "b2a_uu") {
            Ok(v) => v,
            Err(bits) => return bits,
        };
        let out = match uu_encode(&raw) {
            Ok(v) => v,
            Err(msg) => return raise_exception::<_>(_py, "ValueError", msg),
        };
        alloc_bytes_or_oom(_py, &out, "b2a_uu")
    })
}

/// CRC32 lookup table (IEEE polynomial 0xEDB88320). 256 entries, generated at
/// compile time. Each entry[i] = CRC32 of byte i, allowing single-lookup per byte
/// instead of 8 conditional branch iterations.
const CRC32_TABLE: [u32; 256] = {
    let mut table = [0u32; 256];
    let mut i = 0u32;
    while i < 256 {
        let mut crc = i;
        let mut j = 0;
        while j < 8 {
            if (crc & 1) != 0 {
                crc = (crc >> 1) ^ 0xEDB8_8320;
            } else {
                crc >>= 1;
            }
            j += 1;
        }
        table[i as usize] = crc;
        i += 1;
    }
    table
};

#[unsafe(no_mangle)]
pub extern "C" fn molt_binascii_crc32(data_bits: u64) -> u64 {
    crate::with_gil_entry!(_py, {
        let raw = match bytes_like_arg(_py, data_bits, "crc32") {
            Ok(v) => v,
            Err(bits) => return bits,
        };
        // Hardware CRC32 on AArch64 (Apple Silicon M1+ always has CRC32 extension)
        #[cfg(target_arch = "aarch64")]
        {
            if std::arch::is_aarch64_feature_detected!("crc") {
                let crc = unsafe { crc32_hw_aarch64(&raw) };
                return MoltObject::from_int(i64::from(crc)).bits();
            }
        }
        // Table-driven CRC32: 1 lookup per byte instead of 8 branches.
        let mut crc = 0xFFFF_FFFFu32;
        for &byte in &raw {
            let idx = ((crc ^ u32::from(byte)) & 0xFF) as usize;
            crc = (crc >> 8) ^ CRC32_TABLE[idx];
        }
        crc ^= 0xFFFF_FFFF;
        MoltObject::from_int(i64::from(crc)).bits()
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_binascii_crc_hqx(data_bits: u64, value_bits: u64) -> u64 {
    crate::with_gil_entry!(_py, {
        let raw = match bytes_like_arg(_py, data_bits, "crc_hqx") {
            Ok(v) => v,
            Err(bits) => return bits,
        };
        let value = index_i64_from_obj(_py, value_bits, "crc_hqx() arg 2 must be int");
        if exception_pending(_py) {
            return MoltObject::none().bits();
        }
        let mut crc = (value as u32) & 0xFFFF;
        for byte in raw {
            crc ^= (u32::from(byte)) << 8;
            for _ in 0..8 {
                if (crc & 0x8000) != 0 {
                    crc = ((crc << 1) ^ 0x1021) & 0xFFFF;
                } else {
                    crc = (crc << 1) & 0xFFFF;
                }
            }
        }
        MoltObject::from_int(i64::from(crc as i32)).bits()
    })
}

// ---------------------------------------------------------------------------
// UU codec-level intrinsics (full-message framing around per-line primitives)
// ---------------------------------------------------------------------------

/// `molt_uu_codec_encode(data, filename, mode)` — full UU-encoded message with
/// begin/end framing.  Reuses the internal `uu_encode` per-line function.
#[unsafe(no_mangle)]
pub extern "C" fn molt_uu_codec_encode(
    data_bits: u64,
    filename_bits: u64,
    mode_bits: u64,
) -> u64 {
    crate::with_gil_entry!(_py, {
        let raw = match bytes_like_arg(_py, data_bits, "uu_codec_encode") {
            Ok(v) => v,
            Err(bits) => return bits,
        };
        let filename = match string_obj_to_owned(obj_from_bits(filename_bits)) {
            Some(s) => s.replace('\n', "\\n").replace('\r', "\\r"),
            None => "<data>".to_string(),
        };
        let mode = to_i64(obj_from_bits(mode_bits)).unwrap_or(0o666) & 0o777;

        let mut out = Vec::with_capacity(raw.len() * 2 + 64);
        // begin header
        let header = format!("begin {mode:o} {filename}\n");
        out.extend_from_slice(header.as_bytes());

        // Encode in 45-byte chunks
        let mut offset = 0usize;
        while offset < raw.len() {
            let end = std::cmp::min(offset + 45, raw.len());
            let chunk = &raw[offset..end];
            match uu_encode(chunk) {
                Ok(encoded) => out.extend_from_slice(&encoded),
                Err(msg) => return raise_exception::<_>(_py, "ValueError", msg),
            }
            offset = end;
        }

        // Trailer
        out.extend_from_slice(b" \nend\n");

        alloc_bytes_or_oom(_py, &out, "uu_codec_encode")
    })
}

/// `molt_uu_codec_decode(data)` — decode a full UU-encoded message (find begin,
/// decode lines, expect end).
#[unsafe(no_mangle)]
pub extern "C" fn molt_uu_codec_decode(data_bits: u64) -> u64 {
    crate::with_gil_entry!(_py, {
        let raw = match bytes_like_arg(_py, data_bits, "uu_codec_decode") {
            Ok(v) => v,
            Err(bits) => return bits,
        };

        // Find start of encoded data — scan for line starting with "begin"
        let mut pos = 0usize;
        let mut found_begin = false;
        loop {
            if pos >= raw.len() {
                break;
            }
            // Find end of current line
            let line_end = raw[pos..]
                .iter()
                .position(|&b| b == b'\n')
                .map(|i| pos + i + 1)
                .unwrap_or(raw.len());
            let line = &raw[pos..line_end];
            if line.len() >= 5 && &line[..5] == b"begin" {
                pos = line_end;
                found_begin = true;
                break;
            }
            pos = line_end;
        }

        if !found_begin {
            return raise_exception::<_>(
                _py,
                "ValueError",
                "Missing \"begin\" line in input data",
            );
        }

        // Decode lines until "end" or EOF
        let mut out = Vec::with_capacity(raw.len());
        let mut found_end = false;
        loop {
            if pos >= raw.len() {
                break;
            }
            let line_end = raw[pos..]
                .iter()
                .position(|&b| b == b'\n')
                .map(|i| pos + i + 1)
                .unwrap_or(raw.len());
            let line = &raw[pos..line_end];
            pos = line_end;

            if line == b"end\n" || line == b"end" {
                found_end = true;
                break;
            }

            // Try normal decode; on error, use broken-uuencoder workaround
            match uu_decode(line) {
                Ok(decoded) => out.extend_from_slice(&decoded),
                Err(_) => {
                    // Workaround for broken uuencoders: use byte-count header
                    if !line.is_empty() {
                        let nbytes =
                            ((((line[0].wrapping_sub(32)) & 63) as usize) * 4 + 5) / 3;
                        let truncated = &line[..std::cmp::min(nbytes, line.len())];
                        if let Ok(decoded) = uu_decode(truncated) {
                            out.extend_from_slice(&decoded);
                        }
                    }
                }
            }
        }

        if !found_end {
            return raise_exception::<_>(_py, "ValueError", "Truncated input data");
        }

        alloc_bytes_or_oom(_py, &out, "uu_codec_decode")
    })
}
