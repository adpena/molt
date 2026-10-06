//! CRC-32/ISO-HDLC, the checksum behind `zlib.crc32` and `binascii.crc32`.
//!
//! One kernel serves every caller. The accelerated kernels carry the
//! `#[target_feature]` their intrinsics require and run only after the host
//! proves the instructions at runtime: Apple silicon enables `crc` in the
//! aarch64-apple-darwin baseline, generic aarch64 Linux does not, and x86_64
//! has CRC32 only from SSE4.2 on. Selection is by detection, never by the
//! target's baseline, so one binary stays correct on every host of its arch.

const POLYNOMIAL: u32 = 0xEDB8_8320;

const TABLE: [u32; 256] = {
    let mut table = [0u32; 256];
    let mut i = 0u32;
    while i < 256 {
        let mut crc = i;
        let mut j = 0;
        while j < 8 {
            crc = if crc & 1 != 0 {
                (crc >> 1) ^ POLYNOMIAL
            } else {
                crc >> 1
            };
            j += 1;
        }
        table[i as usize] = crc;
        i += 1;
    }
    table
};

/// Continue a CRC-32 from `initial` over `data`, as `zlib.crc32(data, value)`
/// does: `crc32(b, crc32(a, 0)) == crc32(a + b, 0)`.
pub fn crc32(data: &[u8], initial: u32) -> u32 {
    #[cfg(target_arch = "aarch64")]
    if std::arch::is_aarch64_feature_detected!("crc") {
        // SAFETY: the host reports the CRC extension the kernel is compiled for.
        return unsafe { crc32_aarch64_crc(data, initial) };
    }
    #[cfg(target_arch = "x86_64")]
    if std::arch::is_x86_feature_detected!("sse4.2") {
        // SAFETY: the host reports SSE4.2, which the kernel is compiled for.
        return unsafe { crc32_x86_64_sse42(data, initial) };
    }
    crc32_table(data, initial)
}

fn crc32_table(data: &[u8], initial: u32) -> u32 {
    let mut crc = !initial;
    for &byte in data {
        crc = TABLE[((crc ^ u32::from(byte)) & 0xff) as usize] ^ (crc >> 8);
    }
    !crc
}

/// # Safety
///
/// The host must support the aarch64 CRC extension (`crc`); `crc32` checks
/// `is_aarch64_feature_detected!("crc")` before calling this.
#[cfg(target_arch = "aarch64")]
#[target_feature(enable = "crc")]
unsafe fn crc32_aarch64_crc(data: &[u8], initial: u32) -> u32 {
    use std::arch::aarch64::{__crc32b, __crc32d};
    let mut crc = !initial;
    let (words, tail) = data.as_chunks::<8>();
    for word in words {
        crc = __crc32d(crc, u64::from_le_bytes(*word));
    }
    for &byte in tail {
        crc = __crc32b(crc, byte);
    }
    !crc
}

/// # Safety
///
/// The host must support SSE4.2; `crc32` checks
/// `is_x86_feature_detected!("sse4.2")` before calling this.
#[cfg(target_arch = "x86_64")]
#[target_feature(enable = "sse4.2")]
unsafe fn crc32_x86_64_sse42(data: &[u8], initial: u32) -> u32 {
    use std::arch::x86_64::{_mm_crc32_u8, _mm_crc32_u64};
    let mut crc = !initial;
    let (words, tail) = data.as_chunks::<8>();
    for word in words {
        // The instruction leaves the upper 32 bits zero.
        crc = _mm_crc32_u64(u64::from(crc), u64::from_le_bytes(*word)) as u32;
    }
    for &byte in tail {
        crc = _mm_crc32_u8(crc, byte);
    }
    !crc
}

#[cfg(test)]
mod tests {
    use super::*;

    // Oracle: the CRC catalogue check value for CRC-32/ISO-HDLC.
    const CHECK_INPUT: &[u8] = b"123456789";
    const CHECK_VALUE: u32 = 0xCBF4_3926;

    #[test]
    fn matches_the_catalogue_check_value_on_every_kernel() {
        assert_eq!(crc32(CHECK_INPUT, 0), CHECK_VALUE);
        assert_eq!(crc32_table(CHECK_INPUT, 0), CHECK_VALUE);
        assert_eq!(crc32(b"", 0), 0);
    }

    #[test]
    fn continues_from_an_initial_value_like_zlib() {
        let (head, rest) = CHECK_INPUT.split_at(5);
        assert_eq!(crc32(rest, crc32(head, 0)), CHECK_VALUE);
        assert_eq!(crc32_table(rest, crc32_table(head, 0)), CHECK_VALUE);
    }

    #[test]
    fn accelerated_and_table_kernels_agree_on_every_tail_length() {
        let data: Vec<u8> = (0..=255u8).cycle().take(1024 + 7).collect();
        for len in 0..data.len() {
            assert_eq!(
                crc32(&data[..len], 0x1234_5678),
                crc32_table(&data[..len], 0x1234_5678)
            );
        }
    }
}
