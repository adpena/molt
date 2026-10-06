//! C integer width authority shared by every crate that crosses a C ABI.
//!
//! C `long` and `unsigned long` are 64-bit on LP64 targets (Linux, macOS) and
//! 32-bit on LLP64 (Windows) and wasm32. Widening them to Rust's fixed-width
//! integers is a same-type cast on some targets and a real widening on others,
//! and narrowing `unsigned long` to `unsigned int` is a same-type cast on
//! LLP64/wasm32 and a real narrowing on LP64. Each conversion is written once
//! here, so the lint that rejects a same-type cast is allowed only on these
//! functions and every caller spells the platform fact the same way. The
//! assertions keep every widening lossless on every target; the narrowing
//! checks its value and panics on a bit outside the destination.
//!
//! C `char` has one width everywhere but two signednesses: it is `u8` on
//! aarch64, arm, powerpc and s390x Linux and `i8` on x86_64 everywhere and on
//! aarch64 macOS and Windows. The byte pattern is the value on both, so the
//! conversion is a reinterpretation that is a same-type cast on one family
//! and a sign reinterpretation on the other; it, too, is spelled only here.

use core::ffi::{c_char, c_long, c_longlong, c_uint, c_ulong, c_ulonglong};

const _: () = {
    assert!(size_of::<c_char>() == 1);
    assert!(size_of::<c_long>() <= size_of::<i64>());
    assert!(size_of::<c_ulong>() <= size_of::<u64>());
    assert!(size_of::<c_longlong>() == size_of::<i64>());
    assert!(size_of::<c_ulonglong>() == size_of::<u64>());
    assert!(size_of::<c_uint>() <= size_of::<c_ulong>());
};

/// Widen a C `long` to `i64`, preserving the sign on every target.
#[allow(
    clippy::unnecessary_cast,
    reason = "c_long is i64 on LP64 and i32 on LLP64/wasm32"
)]
#[inline]
pub const fn c_long_to_i64(value: c_long) -> i64 {
    value as i64
}

/// Widen a C `unsigned long` to `u64` on every target.
#[allow(
    clippy::unnecessary_cast,
    reason = "c_ulong is u64 on LP64 and u32 on LLP64/wasm32"
)]
#[inline]
pub const fn c_ulong_to_u64(value: c_ulong) -> u64 {
    value as u64
}

/// Widen a C `long long` to `i64`; the two types have the same width.
#[allow(
    clippy::unnecessary_cast,
    reason = "c_longlong is the platform's i64 spelling"
)]
#[inline]
pub const fn c_longlong_to_i64(value: c_longlong) -> i64 {
    value as i64
}

/// Widen a C `unsigned long long` to `u64`; the two types have the same width.
#[allow(
    clippy::unnecessary_cast,
    reason = "c_ulonglong is the platform's u64 spelling"
)]
#[inline]
pub const fn c_ulonglong_to_u64(value: c_ulonglong) -> u64 {
    value as u64
}

/// Narrow a `u64` to a C `unsigned long`, saturating at `ULONG_MAX`: the
/// shape of a resource limit (`rlim_t`) computed in bytes.
#[allow(
    clippy::unnecessary_cast,
    reason = "c_ulong is u64 on LP64 and u32 on LLP64/wasm32"
)]
#[inline]
pub const fn c_ulong_from_u64_saturating(value: u64) -> c_ulong {
    if value > c_ulong::MAX as u64 {
        c_ulong::MAX
    } else {
        value as c_ulong
    }
}

/// Reinterpret a C `char` as the byte it stores.
#[allow(
    clippy::unnecessary_cast,
    reason = "c_char is u8 on aarch64 Linux and i8 on x86_64, macOS and Windows"
)]
#[inline]
pub const fn c_char_to_u8(value: c_char) -> u8 {
    value as u8
}

/// Store a byte in a C `char`, keeping its bit pattern.
#[allow(
    clippy::unnecessary_cast,
    reason = "c_char is u8 on aarch64 Linux and i8 on x86_64, macOS and Windows"
)]
#[inline]
pub const fn u8_to_c_char(value: u8) -> c_char {
    value as c_char
}

/// Narrow a C `unsigned long` to a C `unsigned int` without losing a bit.
///
/// C narrows this assignment implicitly and silently. Rust spells it here
/// once; a value with a bit above the destination width is a caller defect
/// and panics (at compile time when evaluated in a `const` context).
#[allow(
    clippy::unnecessary_cast,
    reason = "c_ulong is u64 on LP64 and the same u32 as c_uint on LLP64/wasm32"
)]
#[inline]
pub const fn c_ulong_to_c_uint(value: c_ulong) -> c_uint {
    assert!(
        value <= c_uint::MAX as c_ulong,
        "C unsigned long value does not fit in unsigned int"
    );
    value as c_uint
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn widening_keeps_sign_and_magnitude_on_every_target() {
        assert_eq!(c_long_to_i64(-1), -1);
        assert_eq!(c_longlong_to_i64(-1), -1);
        assert!(c_long_to_i64(c_long::MIN) <= i64::from(i32::MIN));
        assert!(c_long_to_i64(c_long::MAX) >= i64::from(i32::MAX));
        assert!(c_ulong_to_u64(c_ulong::MAX) >= u64::from(u32::MAX));
        assert_eq!(c_ulonglong_to_u64(c_ulonglong::MAX), u64::MAX);
    }

    #[test]
    fn saturating_narrowing_keeps_values_in_range_and_clamps_the_rest() {
        assert_eq!(c_ulong_from_u64_saturating(0), 0);
        assert_eq!(c_ulong_from_u64_saturating(0xffff_ffff), 0xffff_ffff);
        assert_eq!(c_ulong_from_u64_saturating(u64::MAX), c_ulong::MAX);
        assert_eq!(
            c_ulong_to_u64(c_ulong_from_u64_saturating(u64::from(u32::MAX) + 1)),
            u64::from(u32::MAX) + 1
        );
    }

    #[test]
    fn char_reinterpretation_keeps_every_bit_on_both_signednesses() {
        for byte in [0u8, 1, 0x7f, 0x80, 0xff] {
            assert_eq!(c_char_to_u8(u8_to_c_char(byte)), byte);
        }
        // MIN and MAX are 0x80/0x7f for i8 and 0x00/0xff for u8: both XOR to 0xff.
        assert_eq!(c_char_to_u8(c_char::MIN) ^ c_char_to_u8(c_char::MAX), 0xff);
    }

    #[test]
    fn narrowing_keeps_every_unsigned_int_bit() {
        assert_eq!(c_ulong_to_c_uint(0), 0);
        assert_eq!(c_ulong_to_c_uint(1 << 31), 1 << 31);
        assert_eq!(
            c_ulong_to_c_uint((1 << 10) | (1 << 14)),
            (1 << 10) | (1 << 14)
        );
    }

    #[test]
    fn narrowing_rejects_a_bit_above_unsigned_int() {
        // Only an LP64 `unsigned long` can hold such a bit; on LLP64 and
        // wasm32 the two C types share one width and the narrowing is total.
        let Some(above) = (c_uint::MAX as c_ulong).checked_add(1) else {
            return;
        };
        let narrowed = std::panic::catch_unwind(|| c_ulong_to_c_uint(above));
        assert!(
            narrowed.is_err(),
            "a bit above unsigned int must not be dropped"
        );
    }
}
