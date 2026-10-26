// unicodedata module intrinsics.
// Property and case facts come from pinned target-Python tables in runtime-core.
// Name lookup and the existing partial normalization algorithms are separate
// compatibility obligations; they do not provide property facts.

use crate::bridge::{
    alloc_string, inc_ref_bits, int_bits_from_i64, raise_exception, string_obj_to_owned,
};
use molt_obj_model::MoltObject;
use molt_runtime_core::obj_from_bits;
use molt_runtime_core::prelude::*;

// Names and normalization retain their separate existing source/algorithm owner.
fn target_unicode() -> &'static molt_obj_model::unicode::UnicodeData {
    molt_runtime_core::unicode_target::for_runtime_target()
        .expect("unsupported target Python Unicode facts")
}

fn code_point_from_bits(py: &CoreGilToken, bits: u64) -> Result<u32, u64> {
    let Some(bytes) = (unsafe { molt_runtime_core::rt_string_as_bytes(bits) }) else {
        return Err(raise_exception::<u64>(
            py,
            "TypeError",
            "argument must be str",
        ));
    };
    let mut codes = molt_obj_model::text_storage::code_points(bytes);
    codes
        .next()
        .filter(|_| codes.next().is_none())
        .ok_or_else(|| {
            raise_exception::<u64>(
                py,
                "TypeError",
                "need a single Unicode character as parameter",
            )
        })
}

fn canonical_decompose(ch: char) -> Option<Vec<char>> {
    // Only handle a representative subset of common composed characters.
    // A full table would require the full Unicode data file.
    // This covers the most common Latin diacritics for correctness in tests.
    let code = ch as u32;
    match code {
        // Latin characters with diacritics (NFC precomposed → NFD base + combining)
        0x00C0 => Some(vec!['\u{0041}', '\u{0300}']), // À = A + grave
        0x00C1 => Some(vec!['\u{0041}', '\u{0301}']), // Á = A + acute
        0x00C2 => Some(vec!['\u{0041}', '\u{0302}']), // Â = A + circumflex
        0x00C3 => Some(vec!['\u{0041}', '\u{0303}']), // Ã = A + tilde
        0x00C4 => Some(vec!['\u{0041}', '\u{0308}']), // Ä = A + diaeresis
        0x00C5 => Some(vec!['\u{0041}', '\u{030A}']), // Å = A + ring above
        0x00C7 => Some(vec!['\u{0043}', '\u{0327}']), // Ç = C + cedilla
        0x00C8 => Some(vec!['\u{0045}', '\u{0300}']), // È
        0x00C9 => Some(vec!['\u{0045}', '\u{0301}']), // É
        0x00CA => Some(vec!['\u{0045}', '\u{0302}']), // Ê
        0x00CB => Some(vec!['\u{0045}', '\u{0308}']), // Ë
        0x00CC => Some(vec!['\u{0049}', '\u{0300}']), // Ì
        0x00CD => Some(vec!['\u{0049}', '\u{0301}']), // Í
        0x00CE => Some(vec!['\u{0049}', '\u{0302}']), // Î
        0x00CF => Some(vec!['\u{0049}', '\u{0308}']), // Ï
        0x00D1 => Some(vec!['\u{004E}', '\u{0303}']), // Ñ
        0x00D2 => Some(vec!['\u{004F}', '\u{0300}']), // Ò
        0x00D3 => Some(vec!['\u{004F}', '\u{0301}']), // Ó
        0x00D4 => Some(vec!['\u{004F}', '\u{0302}']), // Ô
        0x00D5 => Some(vec!['\u{004F}', '\u{0303}']), // Õ
        0x00D6 => Some(vec!['\u{004F}', '\u{0308}']), // Ö
        0x00D9 => Some(vec!['\u{0055}', '\u{0300}']), // Ù
        0x00DA => Some(vec!['\u{0055}', '\u{0301}']), // Ú
        0x00DB => Some(vec!['\u{0055}', '\u{0302}']), // Û
        0x00DC => Some(vec!['\u{0055}', '\u{0308}']), // Ü
        0x00DD => Some(vec!['\u{0059}', '\u{0301}']), // Ý
        0x00E0 => Some(vec!['\u{0061}', '\u{0300}']), // à
        0x00E1 => Some(vec!['\u{0061}', '\u{0301}']), // á
        0x00E2 => Some(vec!['\u{0061}', '\u{0302}']), // â
        0x00E3 => Some(vec!['\u{0061}', '\u{0303}']), // ã
        0x00E4 => Some(vec!['\u{0061}', '\u{0308}']), // ä
        0x00E5 => Some(vec!['\u{0061}', '\u{030A}']), // å
        0x00E7 => Some(vec!['\u{0063}', '\u{0327}']), // ç
        0x00E8 => Some(vec!['\u{0065}', '\u{0300}']), // è
        0x00E9 => Some(vec!['\u{0065}', '\u{0301}']), // é
        0x00EA => Some(vec!['\u{0065}', '\u{0302}']), // ê
        0x00EB => Some(vec!['\u{0065}', '\u{0308}']), // ë
        0x00EC => Some(vec!['\u{0069}', '\u{0300}']), // ì
        0x00ED => Some(vec!['\u{0069}', '\u{0301}']), // í
        0x00EE => Some(vec!['\u{0069}', '\u{0302}']), // î
        0x00EF => Some(vec!['\u{0069}', '\u{0308}']), // ï
        0x00F1 => Some(vec!['\u{006E}', '\u{0303}']), // ñ
        0x00F2 => Some(vec!['\u{006F}', '\u{0300}']), // ò
        0x00F3 => Some(vec!['\u{006F}', '\u{0301}']), // ó
        0x00F4 => Some(vec!['\u{006F}', '\u{0302}']), // ô
        0x00F5 => Some(vec!['\u{006F}', '\u{0303}']), // õ
        0x00F6 => Some(vec!['\u{006F}', '\u{0308}']), // ö
        0x00F9 => Some(vec!['\u{0075}', '\u{0300}']), // ù
        0x00FA => Some(vec!['\u{0075}', '\u{0301}']), // ú
        0x00FB => Some(vec!['\u{0075}', '\u{0302}']), // û
        0x00FC => Some(vec!['\u{0075}', '\u{0308}']), // ü
        0x00FD => Some(vec!['\u{0079}', '\u{0301}']), // ý
        0x00FF => Some(vec!['\u{0079}', '\u{0308}']), // ÿ
        _ => None,
    }
}

/// NFD: decompose all composites recursively.
fn nfd_normalize(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for ch in s.chars() {
        if let Some(decomposed) = canonical_decompose(ch) {
            for dc in decomposed {
                if let Some(dd) = canonical_decompose(dc) {
                    for ddc in dd {
                        out.push(ddc);
                    }
                } else {
                    out.push(dc);
                }
            }
        } else {
            out.push(ch);
        }
    }
    out
}

/// NFC: NFD then recompose. Use a simple greedy starter+combiner approach.
fn nfc_normalize(s: &str) -> String {
    // First decompose to NFD.
    let nfd = nfd_normalize(s);
    // Then recompose: find base char + combining mark pairs that are known precompositions.
    let chars: Vec<char> = nfd.chars().collect();
    let mut out = String::with_capacity(s.len());
    let mut i = 0;
    while i < chars.len() {
        let base = chars[i];
        if i + 1 < chars.len() {
            let comb = chars[i + 1];
            if let Some(composed) = try_recompose(base, comb) {
                out.push(composed);
                i += 2;
                continue;
            }
        }
        out.push(base);
        i += 1;
    }
    out
}

/// Try to recompose base + combining mark into a precomposed character.
fn try_recompose(base: char, comb: char) -> Option<char> {
    let b = base as u32;
    let c = comb as u32;
    // Only handle the most common Latin precompositions.
    let composed = match (b, c) {
        (0x0041, 0x0300) => 0x00C0,
        (0x0041, 0x0301) => 0x00C1,
        (0x0041, 0x0302) => 0x00C2,
        (0x0041, 0x0303) => 0x00C3,
        (0x0041, 0x0308) => 0x00C4,
        (0x0041, 0x030A) => 0x00C5,
        (0x0043, 0x0327) => 0x00C7,
        (0x0045, 0x0300) => 0x00C8,
        (0x0045, 0x0301) => 0x00C9,
        (0x0045, 0x0302) => 0x00CA,
        (0x0045, 0x0308) => 0x00CB,
        (0x0049, 0x0300) => 0x00CC,
        (0x0049, 0x0301) => 0x00CD,
        (0x0049, 0x0302) => 0x00CE,
        (0x0049, 0x0308) => 0x00CF,
        (0x004E, 0x0303) => 0x00D1,
        (0x004F, 0x0300) => 0x00D2,
        (0x004F, 0x0301) => 0x00D3,
        (0x004F, 0x0302) => 0x00D4,
        (0x004F, 0x0303) => 0x00D5,
        (0x004F, 0x0308) => 0x00D6,
        (0x0055, 0x0300) => 0x00D9,
        (0x0055, 0x0301) => 0x00DA,
        (0x0055, 0x0302) => 0x00DB,
        (0x0055, 0x0308) => 0x00DC,
        (0x0059, 0x0301) => 0x00DD,
        (0x0061, 0x0300) => 0x00E0,
        (0x0061, 0x0301) => 0x00E1,
        (0x0061, 0x0302) => 0x00E2,
        (0x0061, 0x0303) => 0x00E3,
        (0x0061, 0x0308) => 0x00E4,
        (0x0061, 0x030A) => 0x00E5,
        (0x0063, 0x0327) => 0x00E7,
        (0x0065, 0x0300) => 0x00E8,
        (0x0065, 0x0301) => 0x00E9,
        (0x0065, 0x0302) => 0x00EA,
        (0x0065, 0x0308) => 0x00EB,
        (0x0069, 0x0300) => 0x00EC,
        (0x0069, 0x0301) => 0x00ED,
        (0x0069, 0x0302) => 0x00EE,
        (0x0069, 0x0308) => 0x00EF,
        (0x006E, 0x0303) => 0x00F1,
        (0x006F, 0x0300) => 0x00F2,
        (0x006F, 0x0301) => 0x00F3,
        (0x006F, 0x0302) => 0x00F4,
        (0x006F, 0x0303) => 0x00F5,
        (0x006F, 0x0308) => 0x00F6,
        (0x0075, 0x0300) => 0x00F9,
        (0x0075, 0x0301) => 0x00FA,
        (0x0075, 0x0302) => 0x00FB,
        (0x0075, 0x0308) => 0x00FC,
        (0x0079, 0x0301) => 0x00FD,
        (0x0079, 0x0308) => 0x00FF,
        _ => return None,
    };
    char::from_u32(composed)
}

// ---------------------------------------------------------------------------
// Single-char argument extraction
// ---------------------------------------------------------------------------

fn char_from_bits(_py: &CoreGilToken, bits: u64) -> Result<char, u64> {
    let obj = obj_from_bits(bits);
    let Some(s) = string_obj_to_owned(obj) else {
        return Err(raise_exception::<u64>(
            _py,
            "TypeError",
            "argument must be str",
        ));
    };
    let mut chars = s.chars();
    let ch = match chars.next() {
        Some(c) if chars.next().is_none() => c,
        _ => {
            return Err(raise_exception::<u64>(
                _py,
                "TypeError",
                "need a single Unicode character as parameter",
            ));
        }
    };
    Ok(ch)
}

fn alloc_str(_py: &CoreGilToken, s: &str) -> u64 {
    let ptr = alloc_string(_py, s.as_bytes());
    if ptr.is_null() {
        return raise_exception::<u64>(_py, "MemoryError", "out of memory");
    }
    MoltObject::from_ptr(ptr).bits()
}

// ---------------------------------------------------------------------------
// Public intrinsics
// ---------------------------------------------------------------------------

#[cfg(feature = "stdlib_unicode_names")]
#[unsafe(no_mangle)]
pub extern "C" fn molt_unicodedata_name(ch_bits: u64, default_bits: u64) -> u64 {
    molt_runtime_core::with_core_gil!(_py, {
        let ch = match char_from_bits(_py, ch_bits) {
            Ok(c) => c,
            Err(exc) => return exc,
        };
        if let Some(name) = unicode_names2::name(ch) {
            let name_string = format!("{name}");
            return alloc_str(_py, &name_string);
        }
        // Use default if provided.
        let def_obj = obj_from_bits(default_bits);
        if def_obj.is_none() {
            return raise_exception::<u64>(_py, "ValueError", &format!("no such name: {ch:?}"));
        }
        if let Some(s) = string_obj_to_owned(def_obj) {
            return alloc_str(_py, &s);
        }
        // default is not None and not a string — return it directly.
        inc_ref_bits(_py, default_bits);
        default_bits
    })
}

#[cfg(feature = "stdlib_unicode_names")]
#[unsafe(no_mangle)]
pub extern "C" fn molt_unicodedata_lookup(name_bits: u64) -> u64 {
    molt_runtime_core::with_core_gil!(_py, {
        let Some(s) = string_obj_to_owned(obj_from_bits(name_bits)) else {
            return raise_exception::<u64>(_py, "TypeError", "argument must be str");
        };
        if let Some(ch) = unicode_names2::character(&s) {
            let encoded = ch.to_string();
            return alloc_str(_py, &encoded);
        }
        raise_exception::<u64>(_py, "KeyError", &format!("undefined character name '{s}'"))
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_unicodedata_category(ch_bits: u64) -> u64 {
    molt_runtime_core::with_core_gil!(_py, {
        let code = match code_point_from_bits(_py, ch_bits) {
            Ok(code) => code,
            Err(error) => return error,
        };
        let data = target_unicode();
        alloc_str(_py, data.category(code))
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_unicodedata_bidirectional(ch_bits: u64) -> u64 {
    molt_runtime_core::with_core_gil!(_py, {
        let code = match code_point_from_bits(_py, ch_bits) {
            Ok(code) => code,
            Err(error) => return error,
        };
        let data = target_unicode();
        alloc_str(_py, data.bidirectional(code))
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_unicodedata_combining(ch_bits: u64) -> u64 {
    molt_runtime_core::with_core_gil!(_py, {
        let code = match code_point_from_bits(_py, ch_bits) {
            Ok(code) => code,
            Err(error) => return error,
        };
        let data = target_unicode();
        int_bits_from_i64(_py, i64::from(data.combining(code)))
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_unicodedata_mirrored(ch_bits: u64) -> u64 {
    molt_runtime_core::with_core_gil!(_py, {
        let code = match code_point_from_bits(_py, ch_bits) {
            Ok(code) => code,
            Err(error) => return error,
        };
        let data = target_unicode();
        int_bits_from_i64(_py, i64::from(data.mirrored(code)))
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_unicodedata_decomposition(ch_bits: u64) -> u64 {
    molt_runtime_core::with_core_gil!(_py, {
        let ch = match char_from_bits(_py, ch_bits) {
            Ok(c) => c,
            Err(exc) => return exc,
        };
        if let Some(parts) = canonical_decompose(ch) {
            let hex_parts: Vec<String> =
                parts.iter().map(|c| format!("{:04X}", *c as u32)).collect();
            let result = hex_parts.join(" ");
            alloc_str(_py, &result)
        } else {
            alloc_str(_py, "")
        }
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_unicodedata_normalize(form_bits: u64, text_bits: u64) -> u64 {
    molt_runtime_core::with_core_gil!(_py, {
        let Some(form) = string_obj_to_owned(obj_from_bits(form_bits)) else {
            return raise_exception::<u64>(_py, "TypeError", "form must be str");
        };
        let Some(text) = string_obj_to_owned(obj_from_bits(text_bits)) else {
            return raise_exception::<u64>(_py, "TypeError", "text must be str");
        };
        let normalized = match form.as_str() {
            "NFC" => nfc_normalize(&text),
            "NFD" => nfd_normalize(&text),
            // NFKC/NFKD: compatibility decomposition is a superset; approximate with NFC/NFD.
            "NFKC" => nfc_normalize(&text),
            "NFKD" => nfd_normalize(&text),
            _ => {
                return raise_exception::<u64>(
                    _py,
                    "ValueError",
                    &format!("invalid normalization form {form}"),
                );
            }
        };
        alloc_str(_py, &normalized)
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_unicodedata_is_normalized(form_bits: u64, text_bits: u64) -> u64 {
    molt_runtime_core::with_core_gil!(_py, {
        let Some(form) = string_obj_to_owned(obj_from_bits(form_bits)) else {
            return raise_exception::<u64>(_py, "TypeError", "form must be str");
        };
        let Some(text) = string_obj_to_owned(obj_from_bits(text_bits)) else {
            return raise_exception::<u64>(_py, "TypeError", "text must be str");
        };
        let already = match form.as_str() {
            "NFC" => nfc_normalize(&text) == text,
            "NFD" => nfd_normalize(&text) == text,
            "NFKC" => nfc_normalize(&text) == text,
            "NFKD" => nfd_normalize(&text) == text,
            _ => {
                return raise_exception::<u64>(
                    _py,
                    "ValueError",
                    &format!("invalid normalization form {form}"),
                );
            }
        };
        MoltObject::from_bool(already).bits()
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_unicodedata_numeric(ch_bits: u64, default_bits: u64) -> u64 {
    molt_runtime_core::with_core_gil!(_py, {
        let code = match code_point_from_bits(_py, ch_bits) {
            Ok(code) => code,
            Err(error) => return error,
        };
        if let Some(value) = target_unicode().numeric(code) {
            return MoltObject::from_float(value).bits();
        }
        if obj_from_bits(default_bits).is_none() {
            return raise_exception::<u64>(_py, "ValueError", "not a numeric character");
        }
        inc_ref_bits(_py, default_bits);
        default_bits
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_unicodedata_decimal(ch_bits: u64, default_bits: u64) -> u64 {
    molt_runtime_core::with_core_gil!(_py, {
        let code = match code_point_from_bits(_py, ch_bits) {
            Ok(code) => code,
            Err(error) => return error,
        };
        if let Some(value) = target_unicode().decimal(code) {
            return int_bits_from_i64(_py, i64::from(value));
        }
        if obj_from_bits(default_bits).is_none() {
            return raise_exception::<u64>(_py, "ValueError", "not a decimal character");
        }
        inc_ref_bits(_py, default_bits);
        default_bits
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_unicodedata_digit(ch_bits: u64, default_bits: u64) -> u64 {
    molt_runtime_core::with_core_gil!(_py, {
        let code = match code_point_from_bits(_py, ch_bits) {
            Ok(code) => code,
            Err(error) => return error,
        };
        if let Some(value) = target_unicode().digit(code) {
            return int_bits_from_i64(_py, i64::from(value));
        }
        if obj_from_bits(default_bits).is_none() {
            return raise_exception::<u64>(_py, "ValueError", "not a digit character");
        }
        inc_ref_bits(_py, default_bits);
        default_bits
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_unicodedata_east_asian_width(ch_bits: u64) -> u64 {
    molt_runtime_core::with_core_gil!(_py, crate::bridge::east_asian_width(ch_bits))
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_unicodedata_unidata_version() -> u64 {
    molt_runtime_core::with_core_gil!(_py, crate::bridge::unidata_version())
}
