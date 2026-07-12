//! CPython-compatible ASCII integer literal scanning.
//!
//! This module deliberately owns lexical validation only.  Callers choose the
//! magnitude representation (the runtime uses `BigInt`; the CPython ABI folds
//! through runtime hooks), while sharing sign/base/prefix/underscore and end
//! pointer semantics.

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum IntLiteralErrorKind {
    InvalidBase,
    InvalidLiteral,
    TooManyDigits,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct IntLiteralError {
    pub kind: IntLiteralErrorKind,
    /// First byte not accepted by the scanner, matching `PyLong_FromString`
    /// `pend` semantics. Invalid bases have offset zero. For `TooManyDigits`,
    /// this carries the actual digit count because CPython leaves `pend`
    /// untouched on that pre-conversion failure.
    pub offset: usize,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ScannedIntLiteral {
    pub negative: bool,
    pub base: u32,
    /// Numeric digit values with prefixes and separators removed.
    pub digits: Vec<u8>,
    /// End of the accepted literal, including trailing ASCII whitespace.
    pub end: usize,
}

#[inline]
fn ascii_space(byte: u8) -> bool {
    matches!(byte, b' ' | b'\t' | b'\n' | 0x0b | 0x0c | b'\r')
}

#[inline]
fn digit_value(byte: u8) -> Option<u8> {
    match byte {
        b'0'..=b'9' => Some(byte - b'0'),
        b'a'..=b'z' => Some(byte - b'a' + 10),
        b'A'..=b'Z' => Some(byte - b'A' + 10),
        _ => None,
    }
}

/// Scan the valid integer prefix consumed by CPython `PyLong_FromString`.
///
/// The accepted base is zero or 2..=36.  Base-zero decimal syntax accepts only
/// a sequence whose numeric digits are all zero, preserving CPython's rejection
/// of legacy spellings such as `010`.  A single underscore immediately after a
/// recognized `0x`/`0o`/`0b` prefix is accepted.
pub fn scan_int_literal(bytes: &[u8], base_arg: i32) -> Result<ScannedIntLiteral, IntLiteralError> {
    scan_int_literal_with_limit(bytes, base_arg, 0)
}

/// Scan an integer literal while enforcing CPython's configurable conversion
/// limit. `max_str_digits == 0` disables the limit; power-of-two bases are
/// exempt because their conversion is linear rather than quadratic.
pub fn scan_int_literal_with_limit(
    bytes: &[u8],
    base_arg: i32,
    max_str_digits: usize,
) -> Result<ScannedIntLiteral, IntLiteralError> {
    if base_arg != 0 && !(2..=36).contains(&base_arg) {
        return Err(IntLiteralError {
            kind: IntLiteralErrorKind::InvalidBase,
            offset: 0,
        });
    }

    let mut pos = 0usize;
    while pos < bytes.len() && ascii_space(bytes[pos]) {
        pos += 1;
    }
    let negative = match bytes.get(pos).copied() {
        Some(b'+') => {
            pos += 1;
            false
        }
        Some(b'-') => {
            pos += 1;
            true
        }
        _ => false,
    };

    let mut base = base_arg as u32;
    let mut prefixed = false;
    if pos + 1 < bytes.len() && bytes[pos] == b'0' {
        let prefix_base = match bytes[pos + 1] {
            b'x' | b'X' => Some(16),
            b'o' | b'O' => Some(8),
            b'b' | b'B' => Some(2),
            _ => None,
        };
        if let Some(prefix_base) = prefix_base
            && (base == 0 || base == prefix_base)
        {
            base = prefix_base;
            prefixed = true;
            pos += 2;
        }
    }
    let base_zero_decimal = base_arg == 0 && !prefixed;
    if base == 0 {
        base = 10;
    }

    let digit_start = pos;
    let mut digits = Vec::with_capacity(bytes.len().saturating_sub(pos));
    if prefixed && bytes.get(pos) == Some(&b'_') {
        pos += 1;
    }
    while pos < bytes.len() {
        let byte = bytes[pos];
        if byte == b'_' {
            let next = bytes.get(pos + 1).and_then(|byte| digit_value(*byte));
            if digits.is_empty() || next.is_none_or(|digit| digit as u32 >= base) {
                return Err(IntLiteralError {
                    kind: IntLiteralErrorKind::InvalidLiteral,
                    offset: pos,
                });
            }
            pos += 1;
            continue;
        }
        let Some(digit) = digit_value(byte) else {
            break;
        };
        if digit as u32 >= base {
            break;
        }
        digits.push(digit);
        pos += 1;
    }

    if digits.is_empty() {
        return Err(IntLiteralError {
            kind: IntLiteralErrorKind::InvalidLiteral,
            offset: pos.max(digit_start),
        });
    }

    let token_end = pos;
    while pos < bytes.len() && ascii_space(bytes[pos]) {
        pos += 1;
    }
    if pos != bytes.len() || (base_zero_decimal && digits.iter().any(|digit| *digit != 0)) {
        return Err(IntLiteralError {
            kind: IntLiteralErrorKind::InvalidLiteral,
            offset: if pos != bytes.len() { pos } else { token_end },
        });
    }
    if max_str_digits != 0 && !base.is_power_of_two() && digits.len() > max_str_digits {
        return Err(IntLiteralError {
            kind: IntLiteralErrorKind::TooManyDigits,
            offset: digits.len(),
        });
    }

    Ok(ScannedIntLiteral {
        negative,
        base,
        digits,
        end: pos,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn prefix_underscore_and_base_zero_rules() {
        let value = scan_int_literal(b"  -0x_Ff  ", 0).unwrap();
        assert!(value.negative);
        assert_eq!(value.base, 16);
        assert_eq!(value.digits, [15, 15]);
        assert_eq!(value.end, 10);

        assert_eq!(scan_int_literal(b"00_0", 0).unwrap().digits, [0, 0, 0]);
        assert_eq!(
            scan_int_literal(b"010", 0).unwrap_err(),
            IntLiteralError {
                kind: IntLiteralErrorKind::InvalidLiteral,
                offset: 3,
            }
        );
    }

    #[test]
    fn reports_first_unaccepted_byte() {
        assert_eq!(scan_int_literal(b"1__2", 10).unwrap_err().offset, 1);
        assert_eq!(scan_int_literal(b"123  junk", 10).unwrap_err().offset, 5);
        assert_eq!(scan_int_literal(b"0x_", 0).unwrap_err().offset, 3);
    }

    #[test]
    fn configurable_limit_exempts_power_of_two_bases() {
        let decimal = "9".repeat(4301);
        assert_eq!(
            scan_int_literal_with_limit(decimal.as_bytes(), 10, 4300)
                .unwrap_err()
                .kind,
            IntLiteralErrorKind::TooManyDigits
        );
        let binary = format!("0b1{}", "0".repeat(5000));
        assert!(scan_int_literal_with_limit(binary.as_bytes(), 0, 4300).is_ok());
    }
}
