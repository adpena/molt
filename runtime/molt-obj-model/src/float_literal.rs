//! Python hexadecimal binary64 text conversion.
//!
//! The coefficient stays borrowed. Rounding retains at most 53 bits and one
//! guard/sticky decision, then constructs the IEEE encoding directly. Neither
//! target libm scaling nor an intermediate floating mantissa participates.

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum HexFloatError {
    InvalidSyntax,
    TooLong,
    Overflow,
}

impl HexFloatError {
    pub fn diagnostic(self) -> (&'static str, &'static str) {
        match self {
            Self::InvalidSyntax => ("ValueError", "invalid hexadecimal floating-point string"),
            Self::TooLong => ("ValueError", "hexadecimal string too long to convert"),
            Self::Overflow => (
                "OverflowError",
                "hexadecimal value too large to represent as a float",
            ),
        }
    }
}

fn hexadecimal_digit(byte: u8) -> Option<u8> {
    match byte {
        b'0'..=b'9' => Some(byte - b'0'),
        b'a'..=b'f' => Some(byte - b'a' + 10),
        b'A'..=b'F' => Some(byte - b'A' + 10),
        _ => None,
    }
}

fn prefix_ignoring_ascii_case(text: &[u8], prefix: &[u8]) -> bool {
    text.get(..prefix.len())
        .is_some_and(|head| head.eq_ignore_ascii_case(prefix))
}

fn python_ascii_space(byte: &u8) -> bool {
    // Py_ISSPACE also admits vertical tab, unlike Rust's ASCII predicate.
    matches!(*byte, b' ' | b'\t'..=b'\r')
}

/// CPython's float.fromhex grammar, including optional 0x/p components,
/// ASCII whitespace and signed special values. The runtime must first admit
/// the string's strict UTF-8 export; a surrogate is an encoding failure.
pub fn parse_hex_float(text: &[u8]) -> Result<f64, HexFloatError> {
    let mut cursor = 0;
    while text.get(cursor).is_some_and(python_ascii_space) {
        cursor += 1;
    }
    let negative = text.get(cursor) == Some(&b'-');
    if negative || text.get(cursor) == Some(&b'+') {
        cursor += 1;
    }
    let sign = u64::from(negative) << 63;
    let finish = |cursor: usize, magnitude: u64| {
        if text[cursor..].iter().all(python_ascii_space) {
            Ok(f64::from_bits(sign | magnitude))
        } else {
            Err(HexFloatError::InvalidSyntax)
        }
    };
    let tail = &text[cursor..];
    if prefix_ignoring_ascii_case(tail, b"inf") {
        let length = if prefix_ignoring_ascii_case(tail, b"infinity") {
            8
        } else {
            3
        };
        return finish(cursor + length, f64::INFINITY.to_bits());
    }
    if prefix_ignoring_ascii_case(tail, b"nan") {
        return finish(cursor + 3, 0x7ff8_0000_0000_0000);
    }
    if prefix_ignoring_ascii_case(tail, b"0x") {
        cursor += 2;
    }
    let integer_start = cursor;
    while text
        .get(cursor)
        .copied()
        .and_then(hexadecimal_digit)
        .is_some()
    {
        cursor += 1;
    }
    let integer = &text[integer_start..cursor];
    let fraction = if text.get(cursor) == Some(&b'.') {
        cursor += 1;
        let start = cursor;
        while text
            .get(cursor)
            .copied()
            .and_then(hexadecimal_digit)
            .is_some()
        {
            cursor += 1;
        }
        &text[start..cursor]
    } else {
        &text[cursor..cursor]
    };
    let digit_count = integer.len() + fraction.len();
    if digit_count == 0 {
        return Err(HexFloatError::InvalidSyntax);
    }
    // Match floatobject.c's target-C-long coefficient bound. Besides its
    // diagnostic, this proves exponent +/- 4*digit_count fits in i64 and
    // that saturated decimal exponents cannot be cancelled by the input.
    // C `long` is 32-bit on LLP64 Windows and 64-bit on LP64 Unix.
    let long_max = (1i64 << (std::os::raw::c_long::BITS - 1)) - 1;
    let long_min = -long_max - 1;
    let digit_limit = ((-1074 - long_min / 2).min(long_max / 2 + 1 - 1024)) / 4;
    if digit_count as u64 > digit_limit as u64 {
        return Err(HexFloatError::TooLong);
    }
    let mut exponent = 0i64;
    if matches!(text.get(cursor), Some(b'p' | b'P')) {
        cursor += 1;
        let negative_exponent = text.get(cursor) == Some(&b'-');
        if negative_exponent || text.get(cursor) == Some(&b'+') {
            cursor += 1;
        }
        let start = cursor;
        while let Some(&digit @ b'0'..=b'9') = text.get(cursor) {
            let digit = i64::from(digit - b'0');
            exponent = if negative_exponent {
                exponent.saturating_mul(10).saturating_sub(digit)
            } else {
                exponent.saturating_mul(10).saturating_add(digit)
            };
            cursor += 1;
        }
        if cursor == start {
            return Err(HexFloatError::InvalidSyntax);
        }
    }

    let digits = || integer.iter().chain(fraction).copied();
    let leading_zeroes = digits().take_while(|&digit| digit == b'0').count();
    if leading_zeroes == digit_count || exponent < long_min / 2 {
        return finish(cursor, 0);
    }
    // CPython reports numeric overflow before inspecting a trailing suffix.
    if exponent > long_max / 2 {
        return Err(HexFloatError::Overflow);
    }
    let first = hexadecimal_digit(digits().nth(leading_zeroes).unwrap()).unwrap();
    let significant_bits =
        4 * (digit_count - leading_zeroes - 1) as i64 + i64::from(u8::BITS - first.leading_zeros());
    let scale = exponent - 4 * fraction.len() as i64;
    let top = scale + significant_bits;
    if top < -1074 {
        return finish(cursor, 0);
    }
    if top > 1024 {
        return Err(HexFloatError::Overflow);
    }
    let mut unit_exponent = top.max(-1021) - 53;
    let retained_bits = (top - unit_exponent) as u32;
    let mut coefficient = 0u64;
    let mut consumed = 0u32;
    let mut guard = false;
    let mut sticky = false;
    for (index, digit) in digits().skip(leading_zeroes).enumerate() {
        let digit = hexadecimal_digit(digit).unwrap();
        let bits = if index == 0 {
            u8::BITS - digit.leading_zeros()
        } else {
            4
        };
        for bit in (0..bits).rev() {
            let set = (digit >> bit) & 1 != 0;
            if consumed < retained_bits {
                coefficient = (coefficient << 1) | u64::from(set);
            } else if consumed == retained_bits {
                guard = set;
            } else {
                sticky |= set;
            }
            // All remaining digits have already passed syntax admission.
            // Once sticky is known, only the retained coefficient matters.
            if sticky {
                break;
            }
            consumed = (consumed + 1).min(retained_bits + 2);
        }
        if sticky {
            break;
        }
    }
    if consumed < retained_bits {
        coefficient <<= retained_bits - consumed;
    }
    if guard && (sticky || coefficient & 1 != 0) {
        coefficient += 1;
    }
    if coefficient == 1u64 << 53 {
        coefficient >>= 1;
        unit_exponent += 1;
    }
    let magnitude = if coefficient < 1u64 << 52 {
        // Includes signed zero and rounding up to the smallest subnormal.
        coefficient
    } else {
        let biased_exponent = unit_exponent + 52 + 1023;
        if biased_exponent >= 0x7ff {
            return Err(HexFloatError::Overflow);
        }
        ((biased_exponent as u64) << 52) | (coefficient & ((1u64 << 52) - 1))
    };
    finish(cursor, magnitude)
}

/// Exact inverse hexadecimal presentation, independent of target libm.
pub fn format_hex_float(value: f64) -> String {
    if value.is_nan() {
        return "nan".to_string();
    }
    if value.is_infinite() {
        if value.is_sign_negative() {
            return "-inf".to_string();
        }
        return "inf".to_string();
    }
    if value == 0.0 {
        if value.is_sign_negative() {
            return "-0x0.0p+0".to_string();
        }
        return "0x0.0p+0".to_string();
    }
    let bits = value.to_bits();
    let sign = if (bits >> 63) != 0 { "-" } else { "" };
    let exp_bits = ((bits >> 52) & 0x7ff) as i32;
    let frac_bits = bits & ((1u64 << 52) - 1);
    let (lead, exponent) = if exp_bits == 0 {
        (0u8, -1022)
    } else {
        (1u8, exp_bits - 1023)
    };
    format!("{sign}0x{lead:x}.{frac_bits:013x}p{exponent:+}")
}
