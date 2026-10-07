//! CRC-32/ISO-HDLC, the checksum behind `zlib.crc32`, `binascii.crc32` and
//! zip archives.
//!
//! One function serves every caller. `crc32fast` picks the fastest kernel the
//! host proves at runtime: PCLMULQDQ folding on x86, the CRC extension on
//! aarch64, and a sliced table elsewhere. x86's SSE4.2 `crc32` instruction
//! computes CRC-32C, a different polynomial, so it can never serve zlib.

/// Continue a CRC-32 from `initial` over `data`, as `zlib.crc32(data, value)`
/// does: `crc32(b, crc32(a, 0)) == crc32(a + b, 0)`.
pub fn crc32(data: &[u8], initial: u32) -> u32 {
    let mut hasher = crc32fast::Hasher::new_with_initial(initial);
    hasher.update(data);
    hasher.finalize()
}

#[cfg(test)]
mod tests {
    use super::crc32;

    // Oracle: the CRC catalogue check value for CRC-32/ISO-HDLC.
    const CHECK_INPUT: &[u8] = b"123456789";
    const CHECK_VALUE: u32 = 0xCBF4_3926;

    /// The bitwise definition, independent of every accelerated kernel.
    fn reference(data: &[u8], initial: u32) -> u32 {
        let mut crc = !initial;
        for &byte in data {
            crc ^= u32::from(byte);
            for _ in 0..8 {
                crc = if crc & 1 != 0 {
                    (crc >> 1) ^ 0xEDB8_8320
                } else {
                    crc >> 1
                };
            }
        }
        !crc
    }

    #[test]
    fn matches_the_catalogue_check_value() {
        assert_eq!(crc32(CHECK_INPUT, 0), CHECK_VALUE);
        assert_eq!(reference(CHECK_INPUT, 0), CHECK_VALUE);
        assert_eq!(crc32(b"", 0), 0);
    }

    #[test]
    fn continues_from_an_initial_value_like_zlib() {
        let (head, rest) = CHECK_INPUT.split_at(5);
        assert_eq!(crc32(rest, crc32(head, 0)), CHECK_VALUE);
    }

    #[test]
    fn agrees_with_the_definition_on_every_length_and_initial_value() {
        // Long enough to reach every kernel's folding loop and its tails.
        let data: Vec<u8> = (0..=255u8).cycle().take(4096 + 7).collect();
        for initial in [0, 1, 0x1234_5678, u32::MAX] {
            for len in (0..300).chain([1024, 2048, 4096 + 7]) {
                assert_eq!(
                    crc32(&data[..len], initial),
                    reference(&data[..len], initial),
                    "len={len} initial={initial:#x}"
                );
            }
        }
    }
}
