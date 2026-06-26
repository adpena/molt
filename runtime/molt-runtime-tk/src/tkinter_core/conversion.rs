use molt_runtime_core::prelude::*;

use crate::bridge::{string_obj_to_owned, to_i64};

use super::common::{bits_as_f64, bits_as_i64, bits_to_string};

// ─── Delay/value parsing ─────────────────────────────────────────────────────

/// Convert a delay value to integer milliseconds.
///
/// Accepts:
///   - int: returned as-is
///   - float: truncated to int
///   - string of digits: parsed as int
///
/// Returns None if the value cannot be interpreted as a delay.
pub extern "C" fn molt_tk_normalize_delay_ms(delay_bits: u64) -> u64 {
    molt_runtime_core::with_gil_entry!(_py, {
        // Already an integer?
        if let Some(i) = bits_as_i64(delay_bits) {
            return MoltObject::from_int(i).bits();
        }
        // Float?
        if let Some(f) = bits_as_f64(delay_bits)
            && f.is_finite()
        {
            return MoltObject::from_int(f as i64).bits();
        }
        // String of digits?
        if let Some(text) = bits_to_string(delay_bits) {
            let trimmed = text.trim();
            if !trimmed.is_empty()
                && trimmed.chars().all(|c| c.is_ascii_digit())
                && let Ok(i) = trimmed.parse::<i64>()
            {
                return MoltObject::from_int(i).bits();
            }
            // Also handle negative digit strings
            if trimmed.starts_with('-')
                && trimmed.len() > 1
                && trimmed[1..].chars().all(|c| c.is_ascii_digit())
                && let Ok(i) = trimmed.parse::<i64>()
            {
                return MoltObject::from_int(i).bits();
            }
        }
        // Cannot convert
        MoltObject::none().bits()
    })
}

/// Convert a Tcl string value to the best Python type.
///
/// Conversion priority:
///   1. Try int (with optional sign)
///   2. Try float (if the result is finite)
///   3. Keep as string
///
/// This mirrors the tkinter `_convert_stringval` / `getint`/`getdouble`
/// cascade used when reading widget configuration values from Tcl.
pub extern "C" fn molt_tk_convert_stringval(text_bits: u64) -> u64 {
    molt_runtime_core::with_gil_entry!(_py, {
        // If already int or float, return as-is
        let obj = obj_from_bits(text_bits);
        if obj.is_none() {
            return text_bits;
        }
        if to_i64(obj).is_some() {
            return text_bits;
        }
        if obj.as_float().is_some() {
            return text_bits;
        }

        let Some(text) = string_obj_to_owned(obj) else {
            return text_bits;
        };
        let trimmed = text.trim();

        if trimmed.is_empty() {
            return text_bits;
        }

        // Try integer parsing (with Tcl hex/octal support)
        if let Some(int_val) = try_parse_tcl_int(trimmed) {
            return MoltObject::from_int(int_val).bits();
        }

        // Try float parsing
        if let Ok(float_val) = trimmed.parse::<f64>()
            && float_val.is_finite()
        {
            return MoltObject::from_float(float_val).bits();
        }

        // Keep as original string
        text_bits
    })
}

/// Try to parse a Tcl integer value, supporting:
///   - Decimal: "123", "-456"
///   - Hex: "0x1A", "0X1a"
///   - Octal: "0o17", "0O17"
///   - Binary: "0b1010", "0B1010"
fn try_parse_tcl_int(text: &str) -> Option<i64> {
    let trimmed = text.trim();
    if trimmed.is_empty() {
        return None;
    }

    // Handle sign
    let (negative, unsigned) = if let Some(rest) = trimmed.strip_prefix('-') {
        (true, rest)
    } else if let Some(rest) = trimmed.strip_prefix('+') {
        (false, rest)
    } else {
        (false, trimmed)
    };

    if unsigned.is_empty() {
        return None;
    }

    let value = if let Some(hex) = unsigned
        .strip_prefix("0x")
        .or_else(|| unsigned.strip_prefix("0X"))
    {
        // Hex
        if hex.is_empty() || !hex.chars().all(|c| c.is_ascii_hexdigit()) {
            return None;
        }
        i64::from_str_radix(hex, 16).ok()?
    } else if let Some(oct) = unsigned
        .strip_prefix("0o")
        .or_else(|| unsigned.strip_prefix("0O"))
    {
        // Octal
        if oct.is_empty() || !oct.chars().all(|c| matches!(c, '0'..='7')) {
            return None;
        }
        i64::from_str_radix(oct, 8).ok()?
    } else if let Some(bin) = unsigned
        .strip_prefix("0b")
        .or_else(|| unsigned.strip_prefix("0B"))
    {
        // Binary
        if bin.is_empty() || !bin.chars().all(|c| c == '0' || c == '1') {
            return None;
        }
        i64::from_str_radix(bin, 2).ok()?
    } else {
        // Decimal
        if !unsigned.chars().all(|c| c.is_ascii_digit()) {
            return None;
        }
        unsigned.parse::<i64>().ok()?
    };

    Some(if negative { -value } else { value })
}
