//! Shared byte lexicographic authority for strings, bytes, and bytearrays.

use std::cmp::Ordering;

/// Compare unsigned byte spans. UTF-8/WTF-8 byte order also preserves Unicode
/// code-point order. No callback runs while either borrowed span is in use.
/// The SIMD implementation is shared by runtime and C ABI storage readers.
pub fn compare_bytes(left: &[u8], right: &[u8]) -> Ordering {
    let common = left.len().min(right.len());
    if common < 32 {
        return left.cmp(right);
    }
    let diff = unsafe { simd_find_first_byte_diff(left.as_ptr(), right.as_ptr(), common) };
    if diff == common {
        left.len().cmp(&right.len())
    } else {
        left[diff].cmp(&right[diff])
    }
}
/// Find the first byte index where `a` and `b` differ, within `len` bytes.
/// Returns `len` if the prefixes are identical.
#[inline]
unsafe fn simd_find_first_byte_diff(a: *const u8, b: *const u8, len: usize) -> usize {
    #[cfg(target_arch = "x86_64")]
    {
        if std::arch::is_x86_feature_detected!("avx2") {
            unsafe { simd_find_first_byte_diff_avx2(a, b, len) }
        } else {
            unsafe { simd_find_first_byte_diff_sse2(a, b, len) }
        }
    }
    #[cfg(target_arch = "aarch64")]
    {
        unsafe { simd_find_first_byte_diff_neon(a, b, len) }
    }
    #[cfg(all(target_arch = "wasm32", target_feature = "simd128"))]
    {
        unsafe { simd_find_first_byte_diff_wasm(a, b, len) }
    }
    #[cfg(not(any(
        target_arch = "x86_64",
        target_arch = "aarch64",
        all(target_arch = "wasm32", target_feature = "simd128")
    )))]
    {
        for i in 0..len {
            if unsafe { *a.add(i) != *b.add(i) } {
                return i;
            }
        }
        len
    }
}

#[cfg(all(target_arch = "wasm32", target_feature = "simd128"))]
#[inline]
unsafe fn simd_find_first_byte_diff_wasm(a: *const u8, b: *const u8, len: usize) -> usize {
    use std::arch::wasm32::*;
    let mut i = 0usize;
    while i + 16 <= len {
        let va = unsafe { v128_load(a.add(i) as *const v128) };
        let vb = unsafe { v128_load(b.add(i) as *const v128) };
        let eq = u8x16_eq(va, vb);
        let mask = u8x16_bitmask(eq) as u32;
        if mask != 0xFFFF {
            // Not all equal — find first differing byte
            return i + (!mask).trailing_zeros() as usize;
        }
        i += 16;
    }
    // Scalar tail
    while i < len {
        if unsafe { *a.add(i) != *b.add(i) } {
            return i;
        }
        i += 1;
    }
    len
}

#[cfg(target_arch = "x86_64")]
#[inline]
unsafe fn simd_find_first_byte_diff_sse2(a: *const u8, b: *const u8, len: usize) -> usize {
    unsafe {
        use std::arch::x86_64::*;
        let mut i = 0usize;
        while i + 16 <= len {
            let va = _mm_loadu_si128(a.add(i) as *const __m128i);
            let vb = _mm_loadu_si128(b.add(i) as *const __m128i);
            let cmp = _mm_cmpeq_epi8(va, vb);
            let mask = _mm_movemask_epi8(cmp) as u32;
            if mask != 0xFFFF {
                // Find first differing byte via trailing zeros of negated mask
                return i + (!mask).trailing_zeros() as usize;
            }
            i += 16;
        }
        for j in i..len {
            if *a.add(j) != *b.add(j) {
                return j;
            }
        }
        len
    }
}

#[cfg(target_arch = "x86_64")]
#[inline]
unsafe fn simd_find_first_byte_diff_avx2(a: *const u8, b: *const u8, len: usize) -> usize {
    unsafe {
        use std::arch::x86_64::*;
        let mut i = 0usize;
        while i + 32 <= len {
            let va = _mm256_loadu_si256(a.add(i) as *const __m256i);
            let vb = _mm256_loadu_si256(b.add(i) as *const __m256i);
            let cmp = _mm256_cmpeq_epi8(va, vb);
            let mask = _mm256_movemask_epi8(cmp) as u32;
            if mask != 0xFFFFFFFF {
                return i + (!mask).trailing_zeros() as usize;
            }
            i += 32;
        }
        // SSE2 tail
        if i + 16 <= len {
            let va = _mm_loadu_si128(a.add(i) as *const __m128i);
            let vb = _mm_loadu_si128(b.add(i) as *const __m128i);
            let cmp = _mm_cmpeq_epi8(va, vb);
            let mask = _mm_movemask_epi8(cmp) as u32;
            if mask != 0xFFFF {
                return i + (!mask).trailing_zeros() as usize;
            }
            i += 16;
        }
        for j in i..len {
            if *a.add(j) != *b.add(j) {
                return j;
            }
        }
        len
    }
}

#[cfg(target_arch = "aarch64")]
#[inline]
unsafe fn simd_find_first_byte_diff_neon(a: *const u8, b: *const u8, len: usize) -> usize {
    unsafe {
        use std::arch::aarch64::*;
        let mut i = 0usize;
        while i + 16 <= len {
            let va = vld1q_u8(a.add(i));
            let vb = vld1q_u8(b.add(i));
            let cmp = vceqq_u8(va, vb);
            if vminvq_u8(cmp) != 0xFF {
                // Find the exact byte — check 8-byte halves first
                let low = vget_low_u8(cmp);
                let _high = vget_high_u8(cmp);
                if vminv_u8(low) != 0xFF {
                    for j in 0..8 {
                        if *a.add(i + j) != *b.add(i + j) {
                            return i + j;
                        }
                    }
                }
                for j in 8..16 {
                    if *a.add(i + j) != *b.add(i + j) {
                        return i + j;
                    }
                }
            }
            i += 16;
        }
        for j in i..len {
            if *a.add(j) != *b.add(j) {
                return j;
            }
        }
        len
    }
}
