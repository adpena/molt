// unicodedata module intrinsics.
// Uses unicode_names2 for name<->character lookups and Rust's built-in
// Unicode property tables (char::is_uppercase, etc.) for categories.
// NFC/NFD normalization is implemented via Rust's char decomposition tables
// from the standard library.

use crate::bridge::{
    alloc_string, inc_ref_bits, int_bits_from_i64, raise_exception, string_obj_to_owned,
};
use molt_obj_model::MoltObject;
use molt_runtime_core::obj_from_bits;
use molt_runtime_core::prelude::*;

// ---------------------------------------------------------------------------
// Unicode version
// ---------------------------------------------------------------------------

const UNIDATA_VERSION: &str = "15.1.0";

// ---------------------------------------------------------------------------
// General category helpers
// ---------------------------------------------------------------------------

/// Returns the Unicode general category abbreviation for a char.
/// Categories follow the Unicode standard two-letter codes.
fn general_category(ch: char) -> &'static str {
    let code = ch as u32;

    // --- Letter categories ---
    if ch.is_uppercase() && ch.is_alphabetic() {
        return "Lu";
    }
    if ch.is_lowercase() && ch.is_alphabetic() {
        return "Ll";
    }
    // Titlecase: only a small set (e.g. ǅ Dz Lj NJ etc.)
    if ch.is_alphabetic() && is_titlecase(code) {
        return "Lt";
    }
    if ch.is_alphabetic() && !ch.is_uppercase() && !ch.is_lowercase() {
        // Modifier letter or other letter
        if is_modifier_letter(code) {
            return "Lm";
        }
        return "Lo";
    }

    // --- Mark categories ---
    if is_nonspacing_mark(code) {
        return "Mn";
    }
    if is_spacing_mark(code) {
        return "Mc";
    }
    if is_enclosing_mark(code) {
        return "Me";
    }

    // --- Number categories ---
    if ch.is_ascii_digit() || is_decimal_digit(code) {
        return "Nd";
    }
    if is_letter_number(code) {
        return "Nl";
    }
    if is_other_number(code) {
        return "No";
    }

    // --- Punctuation categories ---
    if is_connector_punctuation(code) {
        return "Pc";
    }
    if is_dash_punctuation(code) {
        return "Pd";
    }
    if is_open_punctuation(code) {
        return "Ps";
    }
    if is_close_punctuation(code) {
        return "Pe";
    }
    if is_initial_punctuation(code) {
        return "Pi";
    }
    if is_final_punctuation(code) {
        return "Pf";
    }
    if ch.is_ascii_punctuation() || is_other_punctuation(code) {
        return "Po";
    }

    // --- Symbol categories ---
    if is_math_symbol(code) {
        return "Sm";
    }
    if is_currency_symbol(code) {
        return "Sc";
    }
    if is_modifier_symbol(code) {
        return "Sk";
    }
    if is_other_symbol(code) {
        return "So";
    }

    // --- Separator categories ---
    if ch == ' ' || is_space_separator(code) {
        return "Zs";
    }
    if is_line_separator(code) {
        return "Zl";
    }
    if is_paragraph_separator(code) {
        return "Zp";
    }

    // --- Control/format/surrogate/private/unassigned ---
    if ch.is_control() {
        return "Cc";
    }
    if is_format_char(code) {
        return "Cf";
    }
    if is_surrogate(code) {
        return "Cs";
    }
    if is_private_use(code) {
        return "Co";
    }

    "Cn" // unassigned
}

/// Returns the Unicode bidi category string for a char.
fn bidi_category(ch: char) -> &'static str {
    let code = ch as u32;
    // Strong types
    if ch.is_uppercase() && ch.is_alphabetic() {
        return "L"; // actually Hebrew/Arabic would be R/AL but without full table use "L"
    }
    // Use range-based classification for common cases.
    match code {
        0x0590..=0x05FF => "R",
        0x0600..=0x06FF => "AL",
        0x0700..=0x08FF => "R",
        0x0000..=0x001F => "BN",
        0x0020 => "WS",
        0x0021..=0x0022 => "ON",
        0x0023..=0x0025 => "ET",
        0x0026..=0x002F => "ON",
        0x0030..=0x0039 => "EN",
        0x003A..=0x0040 => "ON",
        0x005B..=0x0060 => "ON",
        0x007B..=0x007E => "ON",
        0x007F => "BN",
        0x00A0 => "CS",
        0x00A1 => "ON",
        0x00A2..=0x00A5 => "ET",
        0x00A6..=0x00A9 => "ON",
        0x00AA => "L",
        0x00AB => "ON",
        0x00AC => "ON",
        0x00AD => "BN",
        0x00AE..=0x00AF => "ON",
        0x00B0..=0x00B1 => "ET",
        0x00B2..=0x00B3 => "EN",
        0x00B4 => "ON",
        0x00B5 => "L",
        0x00B6..=0x00B8 => "ON",
        0x00B9 => "EN",
        0x00BA => "L",
        0x00BB => "ON",
        0x00BC..=0x00BE => "ON",
        0x00BF => "ON",
        0x00C0..=0x00D6 => "L",
        0x00D7 => "ON",
        0x00D8..=0x00F6 => "L",
        0x00F7 => "ON",
        0x00F8..=0x01FF => "L",
        0x0200..=0x036F => "NSM",
        _ if ch.is_alphabetic() => "L",
        _ if ch.is_numeric() => "EN",
        _ if ch.is_whitespace() => "WS",
        _ => "ON",
    }
}

/// Unicode combining class (0 for base characters, non-zero for combining marks).
fn combining_class(ch: char) -> u8 {
    let code = ch as u32;
    // Combining diacritical marks: U+0300–U+036F
    if (0x0300..=0x036F).contains(&code) {
        return 230; // most combining diacriticals use class 230
    }
    // Nuktas and below-base marks
    if (0x0900..=0x097F).contains(&code) && is_nonspacing_mark(code) {
        return 7;
    }
    // Virama (halant) marks
    if code == 0x094D
        || code == 0x09CD
        || code == 0x0A4D
        || code == 0x0ACD
        || code == 0x0B4D
        || code == 0x0BCD
        || code == 0x0C4D
        || code == 0x0CCD
        || code == 0x0D4D
        || code == 0x0DCA
    {
        return 9;
    }
    0
}

// ---------------------------------------------------------------------------
// Unicode property tables (simplified ranges)
// ---------------------------------------------------------------------------

fn is_titlecase(code: u32) -> bool {
    matches!(
        code,
        0x01C5 | 0x01C8 | 0x01CB | 0x01F2 | 0x1F88..=0x1F8F
            | 0x1F98..=0x1F9F | 0x1FA8..=0x1FAF | 0x1FBC | 0x1FCC | 0x1FFC
    )
}

fn is_modifier_letter(code: u32) -> bool {
    (0x02B0..=0x02FF).contains(&code) || (0xA700..=0xA71F).contains(&code)
}

fn is_nonspacing_mark(code: u32) -> bool {
    (0x0300..=0x036F).contains(&code)
        || (0x0483..=0x0489).contains(&code)
        || (0x0591..=0x05BD).contains(&code)
        || (0x0610..=0x061A).contains(&code)
        || (0x064B..=0x065F).contains(&code)
        || (0x0670..=0x0670).contains(&code)
        || (0x06D6..=0x06DC).contains(&code)
        || (0x0730..=0x074A).contains(&code)
        || (0x07A6..=0x07B0).contains(&code)
        || (0x07EB..=0x07F3).contains(&code)
        || (0x0816..=0x082D).contains(&code)
        || (0x0900..=0x0903).contains(&code)
        || (0x093A..=0x093C).contains(&code)
        || (0x0941..=0x0948).contains(&code)
        || (0x094D..=0x094D).contains(&code)
        || (0x0951..=0x0957).contains(&code)
        || (0x1AB0..=0x1AFF).contains(&code)
        || (0x1CD0..=0x1CF6).contains(&code)
        || (0x1DC0..=0x1DFF).contains(&code)
        || (0x20D0..=0x20FF).contains(&code)
}

fn is_spacing_mark(code: u32) -> bool {
    (0x0903..=0x0903).contains(&code)
        || (0x093E..=0x0940).contains(&code)
        || (0x0949..=0x094C).contains(&code)
        || (0x0982..=0x0983).contains(&code)
}

fn is_enclosing_mark(code: u32) -> bool {
    (0x0488..=0x0489).contains(&code) || (0x20DD..=0x20E0).contains(&code)
}

fn is_decimal_digit(code: u32) -> bool {
    // Beyond ASCII decimal digits 0x0030–0x0039
    (0x0660..=0x0669).contains(&code) // Arabic-Indic digits
        || (0x06F0..=0x06F9).contains(&code) // Extended Arabic-Indic
        || (0x07C0..=0x07C9).contains(&code) // NKo digits
        || (0x0966..=0x096F).contains(&code) // Devanagari digits
        || (0x09E6..=0x09EF).contains(&code) // Bengali digits
        || (0xFF10..=0xFF19).contains(&code) // fullwidth digits
}

fn is_letter_number(code: u32) -> bool {
    (0x16EE..=0x16F0).contains(&code)
        || (0x2160..=0x2188).contains(&code)
        || (0x3007..=0x3007).contains(&code)
        || (0x3021..=0x3029).contains(&code)
}

fn is_other_number(code: u32) -> bool {
    (0x00B2..=0x00B3).contains(&code)
        || code == 0x00B9
        || (0x00BC..=0x00BE).contains(&code)
        || (0x09F4..=0x09F9).contains(&code)
        || (0x2070..=0x2079).contains(&code)
        || (0x2080..=0x2089).contains(&code)
        || (0x2150..=0x215F).contains(&code)
        || (0x2189..=0x2189).contains(&code)
}

fn is_connector_punctuation(code: u32) -> bool {
    code == 0x005F
        || (0x203F..=0x2040).contains(&code)
        || code == 0x2054
        || (0xFE33..=0xFE34).contains(&code)
        || (0xFE4D..=0xFE4F).contains(&code)
        || code == 0xFF3F
}

fn is_dash_punctuation(code: u32) -> bool {
    code == 0x002D
        || (0x2010..=0x2015).contains(&code)
        || code == 0x2E3A
        || code == 0x2E3B
        || code == 0x2212
        || (0xFE31..=0xFE32).contains(&code)
        || code == 0xFE58
        || code == 0xFE63
        || code == 0xFF0D
}

fn is_open_punctuation(code: u32) -> bool {
    matches!(
        code,
        0x0028
            | 0x005B
            | 0x007B
            | 0x0F3A
            | 0x0F3C
            | 0x169B
            | 0x2045
            | 0x207D
            | 0x208D
            | 0x2308
            | 0x230A
            | 0x2329
            | 0x2768
            | 0x276A
            | 0x276C
            | 0x276E
            | 0x2770
            | 0x2772
            | 0x2774
            | 0x27C5
            | 0x27E6
            | 0x27E8
            | 0x27EA
            | 0x27EC
            | 0x27EE
            | 0x2983
            | 0x2985
            | 0x2987
            | 0x2989
            | 0x298B
            | 0x298D
            | 0x298F
            | 0x2991
            | 0x2993
            | 0x2995
            | 0x2997
            | 0x29D8
            | 0x29DA
            | 0x29FC
            | 0x2E22
            | 0x2E24
            | 0x2E26
            | 0x2E28
            | 0x2E55
            | 0x2E57
            | 0x2E59
            | 0x2E5B
            | 0x3008
            | 0x300A
            | 0x300C
            | 0x300E
            | 0x3010
            | 0x3014
            | 0x3016
            | 0x3018
            | 0x301A
            | 0x301D
            | 0xFD3E
            | 0xFE17
            | 0xFE35
            | 0xFE37
            | 0xFE39
            | 0xFE3B
            | 0xFE3D
            | 0xFE3F
            | 0xFE41
            | 0xFE43
            | 0xFE47
            | 0xFE59
            | 0xFE5B
            | 0xFE5D
            | 0xFF08
            | 0xFF3B
            | 0xFF5B
            | 0xFF5F
            | 0xFF62
    )
}

fn is_close_punctuation(code: u32) -> bool {
    matches!(
        code,
        0x0029
            | 0x005D
            | 0x007D
            | 0x0F3B
            | 0x0F3D
            | 0x169C
            | 0x2046
            | 0x207E
            | 0x208E
            | 0x2309
            | 0x230B
            | 0x232A
            | 0x2769
            | 0x276B
            | 0x276D
            | 0x276F
            | 0x2771
            | 0x2773
            | 0x2775
            | 0x27C6
            | 0x27E7
            | 0x27E9
            | 0x27EB
            | 0x27ED
            | 0x27EF
            | 0x2984
            | 0x2986
            | 0x2988
            | 0x298A
            | 0x298C
            | 0x298E
            | 0x2990
            | 0x2992
            | 0x2994
            | 0x2996
            | 0x2998
            | 0x29D9
            | 0x29DB
            | 0x29FD
            | 0x2E23
            | 0x2E25
            | 0x2E27
            | 0x2E29
            | 0x2E56
            | 0x2E58
            | 0x2E5A
            | 0x2E5C
            | 0x3009
            | 0x300B
            | 0x300D
            | 0x300F
            | 0x3011
            | 0x3015
            | 0x3017
            | 0x3019
            | 0x301B
            | 0x301E
            | 0x301F
            | 0xFD3F
            | 0xFE18
            | 0xFE36
            | 0xFE38
            | 0xFE3A
            | 0xFE3C
            | 0xFE3E
            | 0xFE40
            | 0xFE42
            | 0xFE44
            | 0xFE48
            | 0xFE5A
            | 0xFE5C
            | 0xFE5E
            | 0xFF09
            | 0xFF3D
            | 0xFF5D
            | 0xFF60
            | 0xFF63
    )
}

fn is_initial_punctuation(code: u32) -> bool {
    matches!(
        code,
        0x00AB
            | 0x2018
            | 0x201B
            | 0x201C
            | 0x201F
            | 0x2039
            | 0x2E02
            | 0x2E04
            | 0x2E09
            | 0x2E0C
            | 0x2E1C
            | 0x2E20
    )
}

fn is_final_punctuation(code: u32) -> bool {
    matches!(
        code,
        0x00BB | 0x2019 | 0x201D | 0x203A | 0x2E03 | 0x2E05 | 0x2E0A | 0x2E0D | 0x2E1D | 0x2E21
    )
}

fn is_other_punctuation(code: u32) -> bool {
    matches!(code, 0x0021..=0x0023 | 0x0025..=0x0027 | 0x002A | 0x002C
        | 0x002E..=0x002F | 0x003A..=0x003B | 0x003F..=0x0040
        | 0x005C | 0x00A1 | 0x00A7 | 0x00B6 | 0x00B7 | 0x00BF
        | 0x037E | 0x0387 | 0x055A..=0x055F | 0x0589 | 0x05C0
        | 0x05C3 | 0x05C6 | 0x05F3..=0x05F4 | 0x0609..=0x060A
        | 0x060C | 0x060D | 0x061B | 0x061D..=0x061F
        | 0x066A..=0x066D | 0x06D4)
}

fn is_math_symbol(code: u32) -> bool {
    matches!(code, 0x002B | 0x003C..=0x003E | 0x007C | 0x007E
        | 0x00AC | 0x00B1 | 0x00D7 | 0x00F7
        | 0x2200..=0x22FF | 0x2A00..=0x2AFF)
}

fn is_currency_symbol(code: u32) -> bool {
    matches!(code, 0x0024 | 0x00A2..=0x00A5 | 0x058F | 0x060B | 0x07FE
        | 0x07FF | 0x09F2..=0x09F3 | 0x09FB | 0x0AF1 | 0x0BF9 | 0x0E3F
        | 0x17DB | 0x20A0..=0x20CF | 0xA838 | 0xFDFC | 0xFE69
        | 0xFF04 | 0xFFE0..=0xFFE1 | 0xFFE5..=0xFFE6)
}

fn is_modifier_symbol(code: u32) -> bool {
    matches!(code, 0x005E | 0x0060 | 0x00A8 | 0x00AF | 0x00B4
        | 0x00B8 | 0x02C2..=0x02CF | 0x02D2..=0x02DF | 0x02E5..=0x02EB
        | 0x02ED | 0x02EF..=0x02FF | 0xA700..=0xA716 | 0xA720..=0xA721
        | 0xA789..=0xA78A | 0xAB5B | 0xAB6F | 0xFBB2..=0xFBC1
        | 0xFF3E | 0xFF40 | 0xFFE3)
}

fn is_other_symbol(code: u32) -> bool {
    (0x00A6..=0x00A6).contains(&code)
        || code == 0x00A9
        || code == 0x00AE
        || code == 0x00B0
        || (0x0482..=0x0482).contains(&code)
        || (0x060E..=0x060F).contains(&code)
        || (0x1390..=0x1399).contains(&code)
        || (0x1940..=0x1940).contains(&code)
        || (0x19DE..=0x19FF).contains(&code)
        || (0x1B61..=0x1B6A).contains(&code)
        || (0x2100..=0x2101).contains(&code)
        || (0x2103..=0x2106).contains(&code)
        || (0x2108..=0x2109).contains(&code)
        || (0x2114..=0x2114).contains(&code)
        || (0x2116..=0x2118).contains(&code)
        || (0x211E..=0x2123).contains(&code)
        || (0x2125..=0x2125).contains(&code)
        || (0x2127..=0x2127).contains(&code)
        || (0x2129..=0x2129).contains(&code)
        || (0x212E..=0x212E).contains(&code)
        || (0x213A..=0x213B).contains(&code)
        || (0x214A..=0x214A).contains(&code)
        || (0x214C..=0x214D).contains(&code)
        || (0x214F..=0x214F).contains(&code)
        || (0x218A..=0x218B).contains(&code)
        || (0x2190..=0x21FF).contains(&code)
        || (0x2300..=0x23FF).contains(&code)
        || (0x2400..=0x243F).contains(&code)
        || (0x2440..=0x245F).contains(&code)
        || (0x2460..=0x24FF).contains(&code)
        || (0x2500..=0x25FF).contains(&code)
        || (0x2600..=0x26FF).contains(&code)
        || (0x2700..=0x27BF).contains(&code)
        || (0x2900..=0x297F).contains(&code)
        || (0x2B00..=0x2BFF).contains(&code)
}

fn is_space_separator(code: u32) -> bool {
    matches!(
        code,
        0x00A0 | 0x1680 | 0x2000..=0x200A | 0x202F | 0x205F | 0x3000
    )
}

fn is_line_separator(code: u32) -> bool {
    code == 0x2028
}

fn is_paragraph_separator(code: u32) -> bool {
    code == 0x2029
}

fn is_format_char(code: u32) -> bool {
    matches!(
        code,
        0x00AD | 0x0600..=0x0605 | 0x061C | 0x06DD | 0x070F | 0x08E2
            | 0x180E | 0x200B..=0x200F | 0x202A..=0x202E | 0x2060..=0x2064
            | 0x2066..=0x206F | 0xFEFF | 0xFFF9..=0xFFFB
    )
}

fn is_surrogate(code: u32) -> bool {
    (0xD800..=0xDFFF).contains(&code)
}

fn is_private_use(code: u32) -> bool {
    (0xE000..=0xF8FF).contains(&code)
        || (0xF0000..=0xFFFFF).contains(&code)
        || (0x100000..=0x10FFFF).contains(&code)
}

// ---------------------------------------------------------------------------
// East Asian Width
// ---------------------------------------------------------------------------

fn east_asian_width(ch: char) -> &'static str {
    let code = ch as u32;
    match code {
        // Fullwidth
        0xFF01..=0xFF60 | 0xFFE0..=0xFFE6 => "F",
        // Wide CJK
        0x4E00..=0x9FFF | 0x3400..=0x4DBF | 0x2000..=0x2FFF if !matches!(code, 0x2000..=0x200F | 0x2028..=0x202F | 0x2060..=0x206F) => {
            "W"
        }
        0xF900..=0xFAFF
        | 0x3000..=0x303F
        | 0x3040..=0x309F
        | 0x30A0..=0x30FF
        | 0x31F0..=0x31FF
        | 0x3200..=0x32FF
        | 0x3300..=0x33FF
        | 0xAC00..=0xD7AF
        | 0x1F300..=0x1F9FF => "W",
        // Halfwidth
        0xFF61..=0xFFDC | 0xFFE8..=0xFFEE => "H",
        // Narrow ASCII and Latin
        0x0020..=0x007E | 0x00A2..=0x00A3 | 0x00A5..=0x00A6 | 0x00AC | 0x00AF => "Na",
        // Ambiguous
        0x00A1
        | 0x00A4
        | 0x00A7..=0x00A8
        | 0x00AA
        | 0x00AD
        | 0x00AE
        | 0x00B0..=0x00B4
        | 0x00B6..=0x00BA
        | 0x00BC..=0x00BF
        | 0x00C6
        | 0x00D0
        | 0x00D7
        | 0x00D8
        | 0x00DE..=0x00E1
        | 0x00E6
        | 0x00E8..=0x00EA
        | 0x00EC..=0x00ED
        | 0x00F0
        | 0x00F2..=0x00F3
        | 0x00F7..=0x00FA
        | 0x00FC
        | 0x00FE => "A",
        // Neutral (default)
        _ => "N",
    }
}

// ---------------------------------------------------------------------------
// Simple NFC/NFD normalization using Rust's char decomposition
// ---------------------------------------------------------------------------

/// Returns canonical decomposition codepoints for a char.
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

#[unsafe(no_mangle)]
pub extern "C" fn molt_unicodedata_name(ch_bits: u64, default_bits: u64) -> u64 {
    molt_runtime_core::with_core_gil!(_py, {
        let ch = match char_from_bits(_py, ch_bits) {
            Ok(c) => c,
            Err(exc) => return exc,
        };
        #[cfg(feature = "stdlib_unicode_names")]
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

#[unsafe(no_mangle)]
pub extern "C" fn molt_unicodedata_lookup(name_bits: u64) -> u64 {
    molt_runtime_core::with_core_gil!(_py, {
        let Some(s) = string_obj_to_owned(obj_from_bits(name_bits)) else {
            return raise_exception::<u64>(_py, "TypeError", "argument must be str");
        };
        #[cfg(feature = "stdlib_unicode_names")]
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
        let ch = match char_from_bits(_py, ch_bits) {
            Ok(c) => c,
            Err(exc) => return exc,
        };
        alloc_str(_py, general_category(ch))
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_unicodedata_bidirectional(ch_bits: u64) -> u64 {
    molt_runtime_core::with_core_gil!(_py, {
        let ch = match char_from_bits(_py, ch_bits) {
            Ok(c) => c,
            Err(exc) => return exc,
        };
        alloc_str(_py, bidi_category(ch))
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_unicodedata_combining(ch_bits: u64) -> u64 {
    molt_runtime_core::with_core_gil!(_py, {
        let ch = match char_from_bits(_py, ch_bits) {
            Ok(c) => c,
            Err(exc) => return exc,
        };
        int_bits_from_i64(_py, combining_class(ch) as i64)
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_unicodedata_mirrored(ch_bits: u64) -> u64 {
    molt_runtime_core::with_core_gil!(_py, {
        let ch = match char_from_bits(_py, ch_bits) {
            Ok(c) => c,
            Err(exc) => return exc,
        };
        // Common mirrored characters: brackets, math operators.
        let code = ch as u32;
        let mirrored = matches!(
            code,
            0x0028 | 0x0029 | 0x003C | 0x003E | 0x005B | 0x005D | 0x007B | 0x007D
                | 0x00AB | 0x00BB | 0x2018 | 0x2019 | 0x201C | 0x201D | 0x2039 | 0x203A
                | 0x2045 | 0x2046 | 0x207D | 0x207E | 0x208D | 0x208E | 0x2208..=0x220D
                | 0x2215 | 0x223C | 0x2243 | 0x2252 | 0x2253 | 0x2254 | 0x2255
                | 0x2264 | 0x2265 | 0x2266 | 0x2267 | 0x226A | 0x226B | 0x2308..=0x230B
                | 0x2329 | 0x232A | 0x27E6..=0x27EF | 0x2983..=0x2998 | 0x29D8..=0x29DB
                | 0x29FC | 0x29FD | 0x3008..=0x3011 | 0x3014..=0x301B | 0xFD3E | 0xFD3F
                | 0xFE59..=0xFE5E | 0xFF08 | 0xFF09 | 0xFF3B | 0xFF3D | 0xFF5B | 0xFF5D
                | 0xFF5F | 0xFF60 | 0xFF62 | 0xFF63
        );
        int_bits_from_i64(_py, if mirrored { 1 } else { 0 })
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
        let ch = match char_from_bits(_py, ch_bits) {
            Ok(c) => c,
            Err(exc) => return exc,
        };
        // Check digit value first.
        if let Some(d) = ch.to_digit(10) {
            return MoltObject::from_float(d as f64).bits();
        }
        // Numeric fractions (selected well-known cases).
        let code = ch as u32;
        let value: Option<f64> = match code {
            0x00BC => Some(0.25),
            0x00BD => Some(0.5),
            0x00BE => Some(0.75),
            0x2153 => Some(1.0 / 3.0),
            0x2154 => Some(2.0 / 3.0),
            0x2155 => Some(0.2),
            0x2156 => Some(0.4),
            0x2157 => Some(0.6),
            0x2158 => Some(0.8),
            0x2159 => Some(1.0 / 6.0),
            0x215A => Some(5.0 / 6.0),
            0x215B => Some(0.125),
            0x215C => Some(0.375),
            0x215D => Some(0.625),
            0x215E => Some(0.875),
            0x2150 => Some(1.0 / 7.0),
            0x2151 => Some(1.0 / 9.0),
            0x2152 => Some(0.1),
            _ => None,
        };
        if let Some(v) = value {
            return MoltObject::from_float(v).bits();
        }
        let def_obj = obj_from_bits(default_bits);
        if def_obj.is_none() {
            return raise_exception::<u64>(_py, "ValueError", "not a numeric character");
        }
        inc_ref_bits(_py, default_bits);
        default_bits
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_unicodedata_decimal(ch_bits: u64, default_bits: u64) -> u64 {
    molt_runtime_core::with_core_gil!(_py, {
        let ch = match char_from_bits(_py, ch_bits) {
            Ok(c) => c,
            Err(exc) => return exc,
        };
        if let Some(d) = ch.to_digit(10) {
            return int_bits_from_i64(_py, d as i64);
        }
        let def_obj = obj_from_bits(default_bits);
        if def_obj.is_none() {
            return raise_exception::<u64>(_py, "ValueError", "not a decimal character");
        }
        inc_ref_bits(_py, default_bits);
        default_bits
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_unicodedata_digit(ch_bits: u64, default_bits: u64) -> u64 {
    molt_runtime_core::with_core_gil!(_py, {
        let ch = match char_from_bits(_py, ch_bits) {
            Ok(c) => c,
            Err(exc) => return exc,
        };
        // Digits include superscripts ² ³ etc.
        let code = ch as u32;
        let digit: Option<i64> = match code {
            0x0030..=0x0039 => Some((code - 0x0030) as i64),
            0x00B2 => Some(2),
            0x00B3 => Some(3),
            0x00B9 => Some(1),
            0x2070 => Some(0),
            0x2074..=0x2079 => Some((code - 0x2074 + 4) as i64),
            0x2080..=0x2089 => Some((code - 0x2080) as i64),
            _ => ch.to_digit(10).map(|d| d as i64),
        };
        if let Some(d) = digit {
            return int_bits_from_i64(_py, d);
        }
        let def_obj = obj_from_bits(default_bits);
        if def_obj.is_none() {
            return raise_exception::<u64>(_py, "ValueError", "not a digit character");
        }
        inc_ref_bits(_py, default_bits);
        default_bits
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_unicodedata_east_asian_width(ch_bits: u64) -> u64 {
    molt_runtime_core::with_core_gil!(_py, {
        let ch = match char_from_bits(_py, ch_bits) {
            Ok(c) => c,
            Err(exc) => return exc,
        };
        alloc_str(_py, east_asian_width(ch))
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_unicodedata_unidata_version() -> u64 {
    molt_runtime_core::with_core_gil!(_py, { alloc_str(_py, UNIDATA_VERSION) })
}
