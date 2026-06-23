//! RFC 3454 StringPrep table membership intrinsics.
//!
//! Implements all `in_table_*` and `map_table_b*` functions from Python's
//! `stringprep` module using generated CPython parity data.

use crate::bridge::{alloc_string, raise_exception, string_obj_to_owned};
use crate::tables;
use molt_obj_model::MoltObject;
use molt_runtime_core::prelude::*;

fn in_ranges(ranges: &[(u32, u32)], cp: u32) -> bool {
    ranges
        .binary_search_by(|(start, end)| {
            if cp < *start {
                std::cmp::Ordering::Greater
            } else if cp > *end {
                std::cmp::Ordering::Less
            } else {
                std::cmp::Ordering::Equal
            }
        })
        .is_ok()
}

fn mapping_for(mappings: &'static [(u32, &'static str)], cp: u32) -> Option<&'static str> {
    mappings
        .binary_search_by_key(&cp, |(mapped_cp, _)| *mapped_cp)
        .ok()
        .map(|idx| mappings[idx].1)
}

fn char_len(value: &str) -> usize {
    value.chars().count()
}

fn single_char_or_raise_ord(_py: &CoreGilToken, value: &str, context: &str) -> Result<char, u64> {
    let mut chars = value.chars();
    match (chars.next(), chars.next()) {
        (Some(ch), None) => Ok(ch),
        _ => Err(raise_exception::<_>(
            _py,
            "TypeError",
            &format!(
                "{context}: ord() expected a character, but string of length {} found",
                char_len(value),
            ),
        )),
    }
}

fn single_char_or_raise_unicodedata(
    _py: &CoreGilToken,
    value: &str,
    function_name: &str,
) -> Result<char, u64> {
    let mut chars = value.chars();
    match (chars.next(), chars.next()) {
        (Some(ch), None) => Ok(ch),
        _ => Err(raise_exception::<_>(
            _py,
            "TypeError",
            &format!("{function_name}() argument must be a unicode character, not str"),
        )),
    }
}

fn single_char_or_false(value: &str) -> Option<char> {
    let mut chars = value.chars();
    match (chars.next(), chars.next()) {
        (Some(ch), None) => Some(ch),
        _ => None,
    }
}

// ---------------------------------------------------------------------------
// Table B.1 — Commonly mapped to nothing
// ---------------------------------------------------------------------------

fn in_table_b1(cp: u32) -> bool {
    in_ranges(tables::B1_RANGES, cp)
}

// ---------------------------------------------------------------------------
// Table C.1.1 — ASCII space characters
// ---------------------------------------------------------------------------

fn in_table_c11(cp: u32) -> bool {
    cp == 0x0020
}

// ---------------------------------------------------------------------------
// Table C.1.2 — Non-ASCII space characters
// ---------------------------------------------------------------------------

fn in_table_c12(cp: u32) -> bool {
    matches!(
        cp,
        0x00A0 | 0x1680 | 0x2000..=0x200B | 0x202F | 0x205F | 0x3000
    )
}

// ---------------------------------------------------------------------------
// Table C.2.1 — ASCII control characters
// ---------------------------------------------------------------------------

fn in_table_c21(cp: u32) -> bool {
    matches!(cp, 0x0000..=0x001F | 0x007F)
}

// ---------------------------------------------------------------------------
// Table C.2.2 — Non-ASCII control characters
// ---------------------------------------------------------------------------

fn in_table_c22(cp: u32) -> bool {
    matches!(
        cp,
        0x0080..=0x009F
            | 0x06DD
            | 0x070F
            | 0x180E
            | 0x200C..=0x200D
            | 0x2028..=0x2029
            | 0x2060..=0x2063
            | 0x206A..=0x206F
            | 0xFEFF
            | 0xFFF9..=0xFFFC
            | 0x1D173..=0x1D17A
    )
}

// ---------------------------------------------------------------------------
// Table C.3 — Private use
// ---------------------------------------------------------------------------

fn in_table_c3(cp: u32) -> bool {
    matches!(cp, 0xE000..=0xF8FF | 0xF0000..=0xFFFFD | 0x100000..=0x10FFFD)
}

// ---------------------------------------------------------------------------
// Table C.4 — Non-character code points
// ---------------------------------------------------------------------------

fn in_table_c4(cp: u32) -> bool {
    if (0xFDD0..=0xFDEF).contains(&cp) {
        return true;
    }
    // Plane-final non-characters: xxFFFE and xxFFFF for planes 0-16
    matches!(cp & 0xFFFF, 0xFFFE | 0xFFFF) && cp <= 0x10FFFF
}

// ---------------------------------------------------------------------------
// Table C.5 — Surrogate codes
// ---------------------------------------------------------------------------

fn in_table_c5(cp: u32) -> bool {
    (0xD800..=0xDFFF).contains(&cp)
}

// ---------------------------------------------------------------------------
// Table C.6 — Inappropriate for plain text
// ---------------------------------------------------------------------------

fn in_table_c6(cp: u32) -> bool {
    (0xFFF9..=0xFFFD).contains(&cp)
}

// ---------------------------------------------------------------------------
// Table C.7 — Inappropriate for canonical representation
// ---------------------------------------------------------------------------

fn in_table_c7(cp: u32) -> bool {
    (0x2FF0..=0x2FFB).contains(&cp)
}

// ---------------------------------------------------------------------------
// Table C.8 — Change display properties or deprecated
// ---------------------------------------------------------------------------

fn in_table_c8(cp: u32) -> bool {
    matches!(
        cp,
        0x0340..=0x0341 | 0x200E..=0x200F | 0x202A..=0x202E | 0x206A..=0x206F
    )
}

// ---------------------------------------------------------------------------
// Table C.9 — Tagging characters
// ---------------------------------------------------------------------------

fn in_table_c9(cp: u32) -> bool {
    matches!(cp, 0xE0001 | 0xE0020..=0xE007F)
}

// ---------------------------------------------------------------------------
// Table D.1 — Characters with bidirectional property "R" or "AL"
// ---------------------------------------------------------------------------

fn in_table_d1(cp: u32) -> bool {
    in_ranges(tables::D1_RANGES, cp)
}

// ---------------------------------------------------------------------------
// Table D.2 — Characters with bidirectional property "L"
// ---------------------------------------------------------------------------

fn in_table_d2(cp: u32) -> bool {
    in_ranges(tables::D2_RANGES, cp)
}

// ---------------------------------------------------------------------------
// Table A.1 — Unassigned code points in Unicode 3.2
// ---------------------------------------------------------------------------

fn in_table_a1(cp: u32) -> bool {
    in_ranges(tables::A1_RANGES, cp)
}

// ---------------------------------------------------------------------------
// Intrinsic entry points
// ---------------------------------------------------------------------------

/// `molt_stringprep_in_table(table_name, char) -> bool`
#[unsafe(no_mangle)]
pub extern "C" fn molt_stringprep_in_table(table_bits: u64, char_bits: u64) -> u64 {
    with_core_gil!(_py, {
        let Some(table_name) = string_obj_to_owned(obj_from_bits(table_bits)) else {
            return raise_exception::<_>(_py, "TypeError", "stringprep: expected str table name");
        };
        let Some(ch_str) = string_obj_to_owned(obj_from_bits(char_bits)) else {
            return raise_exception::<_>(_py, "TypeError", "stringprep: expected str char");
        };
        macro_rules! cp_ord {
            ($context:expr) => {
                match single_char_or_raise_ord(_py, &ch_str, $context) {
                    Ok(ch) => ch as u32,
                    Err(bits) => return bits,
                }
            };
        }
        macro_rules! cp_unicodedata {
            ($function_name:expr) => {
                match single_char_or_raise_unicodedata(_py, &ch_str, $function_name) {
                    Ok(ch) => ch as u32,
                    Err(bits) => return bits,
                }
            };
        }
        let result = match table_name.as_str() {
            "a1" => in_table_a1(cp_unicodedata!("category")),
            "b1" => in_table_b1(cp_ord!("in_table_b1")),
            "c11" => single_char_or_false(&ch_str).is_some_and(|ch| in_table_c11(ch as u32)),
            "c12" => in_table_c12(cp_unicodedata!("category")),
            "c11_c12" => {
                if single_char_or_false(&ch_str).is_some_and(|ch| in_table_c11(ch as u32)) {
                    true
                } else {
                    in_table_c12(cp_unicodedata!("category"))
                }
            }
            "c21" => in_table_c21(cp_ord!("in_table_c21")),
            "c22" => in_table_c22(cp_ord!("in_table_c22")),
            "c21_c22" => {
                let cp = cp_unicodedata!("category");
                in_table_c21(cp) || in_table_c22(cp)
            }
            "c3" => in_table_c3(cp_unicodedata!("category")),
            "c4" => in_table_c4(cp_ord!("in_table_c4")),
            "c5" => in_table_c5(cp_unicodedata!("category")),
            "c6" => in_table_c6(cp_ord!("in_table_c6")),
            "c7" => in_table_c7(cp_ord!("in_table_c7")),
            "c8" => in_table_c8(cp_ord!("in_table_c8")),
            "c9" => in_table_c9(cp_ord!("in_table_c9")),
            "d1" => in_table_d1(cp_unicodedata!("bidirectional")),
            "d2" => in_table_d2(cp_unicodedata!("bidirectional")),
            _ => {
                return raise_exception::<_>(
                    _py,
                    "ValueError",
                    &format!("Unknown stringprep table: {table_name}"),
                );
            }
        };
        MoltObject::from_bool(result).bits()
    })
}

fn alloc_mapping(_py: &CoreGilToken, mapped: &str, context: &str) -> u64 {
    let ptr = alloc_string(_py, mapped.as_bytes());
    if ptr.is_null() {
        return raise_exception::<_>(_py, "MemoryError", &format!("{context}: OOM"));
    }
    MoltObject::from_ptr(ptr).bits()
}

fn map_stringprep_table(
    _py: &CoreGilToken,
    char_bits: u64,
    mappings: &'static [(u32, &'static str)],
    context: &str,
) -> u64 {
    let Some(ch_str) = string_obj_to_owned(obj_from_bits(char_bits)) else {
        return raise_exception::<_>(_py, "TypeError", &format!("{context}: expected str"));
    };
    let ch = match single_char_or_raise_ord(_py, &ch_str, context) {
        Ok(ch) => ch,
        Err(bits) => return bits,
    };
    if let Some(mapped) = mapping_for(mappings, ch as u32) {
        return alloc_mapping(_py, mapped, context);
    }
    let mut encoded = [0_u8; 4];
    alloc_mapping(_py, ch.encode_utf8(&mut encoded), context)
}

/// `molt_stringprep_map_table_b2(char) -> str`
#[unsafe(no_mangle)]
pub extern "C" fn molt_stringprep_map_table_b2(char_bits: u64) -> u64 {
    with_core_gil!(_py, {
        map_stringprep_table(_py, char_bits, tables::B2_MAPPINGS, "map_table_b2")
    })
}

/// `molt_stringprep_map_table_b3(char) -> str`
#[unsafe(no_mangle)]
pub extern "C" fn molt_stringprep_map_table_b3(char_bits: u64) -> u64 {
    with_core_gil!(_py, {
        map_stringprep_table(_py, char_bits, tables::B3_MAPPINGS, "map_table_b3")
    })
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_table_b1() {
        assert!(in_table_b1(0x00AD)); // SOFT HYPHEN
        assert!(in_table_b1(0x200B)); // ZERO WIDTH SPACE
        assert!(in_table_b1(0xFEFF)); // BOM
        assert!(!in_table_b1(0x0041)); // 'A'
    }

    #[test]
    fn test_table_c11() {
        assert!(in_table_c11(0x0020)); // SPACE
        assert!(!in_table_c11(0x00A0)); // NO-BREAK SPACE
    }

    #[test]
    fn test_table_c12() {
        assert!(in_table_c12(0x00A0)); // NO-BREAK SPACE
        assert!(in_table_c12(0x2003)); // EM SPACE
        assert!(in_table_c12(0x3000)); // IDEOGRAPHIC SPACE
        assert!(!in_table_c12(0x0020)); // regular SPACE
    }

    #[test]
    fn test_table_c21() {
        assert!(in_table_c21(0x0000)); // NULL
        assert!(in_table_c21(0x001F)); // UNIT SEPARATOR
        assert!(in_table_c21(0x007F)); // DELETE
        assert!(!in_table_c21(0x0020)); // SPACE
    }

    #[test]
    fn test_table_c22() {
        assert!(in_table_c22(0x0080));
        assert!(in_table_c22(0x06DD));
        assert!(in_table_c22(0xFEFF));
        assert!(!in_table_c22(0x0041));
    }

    #[test]
    fn test_table_c3() {
        assert!(in_table_c3(0xE000));
        assert!(in_table_c3(0xF8FF));
        assert!(in_table_c3(0xF0000));
        assert!(!in_table_c3(0x0041));
    }

    #[test]
    fn test_table_c4() {
        assert!(in_table_c4(0xFDD0));
        assert!(in_table_c4(0xFDEF));
        assert!(in_table_c4(0xFFFE));
        assert!(in_table_c4(0xFFFF));
        assert!(in_table_c4(0x1FFFE));
        assert!(in_table_c4(0x10FFFF));
        assert!(!in_table_c4(0x0041));
    }

    #[test]
    fn test_table_c5() {
        assert!(in_table_c5(0xD800));
        assert!(in_table_c5(0xDFFF));
        assert!(!in_table_c5(0x0041));
    }

    #[test]
    fn test_table_c8() {
        assert!(in_table_c8(0x0340));
        assert!(in_table_c8(0x200E));
        assert!(in_table_c8(0x202A));
        assert!(!in_table_c8(0x0041));
    }

    #[test]
    fn test_table_d1() {
        assert!(in_table_d1(0x05D0)); // Hebrew ALEF
        assert!(in_table_d1(0x0621)); // Arabic HAMZA
        assert!(in_table_d1(0x200F)); // RIGHT-TO-LEFT MARK
        assert!(in_table_d1(0xFB1D)); // HEBREW LETTER YOD WITH HIRIQ
        assert!(!in_table_d1(0x0041)); // 'A'
    }

    #[test]
    fn test_table_d2() {
        assert!(in_table_d2(0x0041)); // 'A'
        assert!(in_table_d2(0x0061)); // 'a'
        assert!(in_table_d2(0x3041)); // HIRAGANA
        assert!(in_table_d2(0x4E00)); // CJK
        assert!(!in_table_d2(0x0621)); // Arabic (RandAL)
    }
}
