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

use core::ffi::{c_long, c_longlong, c_uint, c_ulong, c_ulonglong};

const _: () = {
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
