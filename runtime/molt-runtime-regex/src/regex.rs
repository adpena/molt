#![allow(dead_code, unused_imports)]
//! Regex intrinsics for Molt stdlib — advanced pattern helpers.
//!
//! This module provides lookaround and parser fidelity intrinsics that the
//! Python-side
//! `re` module cannot implement efficiently with existing helpers:
//!
//! * `molt_re_positive_lookahead`  — check that a sub-pattern DOES match at
//!   the current position (same descriptor protocol as the negative variant).
//! * `molt_re_negative_lookahead`  — check that a sub-pattern does NOT match
//!   at the current position (literal and char-class fast paths; complex
//!   sub-patterns return the sentinel −2 so Python falls back).
//! * `molt_re_positive_lookbehind` — positive fixed-width look-behind.
//! * `molt_re_negative_lookbehind` — same, but for fixed-width look-behind.
//! * `molt_re_strip_verbose`       — pre-process a VERBOSE/X-flag pattern by
//!   removing unescaped whitespace and `#`-comments (respects `[…]` classes
//!   and escape sequences).
//! * `molt_re_fullmatch_check`     — verify that a match spans the entire
//!   search window (start == match_start, end == match_end).
//! * `molt_re_named_backref_advance` — advance past a named back-reference by
//!   looking up the group span from a name→index dict and delegating to the
//!   existing byte-comparison logic.
//!
//! All functions follow the canonical Molt intrinsic ABI:
//!   `pub extern "C" fn molt_re_*(args: u64) -> u64`
//!   with `molt_runtime_core::with_core_gil!(_py, { … })` as the outer frame.

use molt_obj_model::MoltObject;
use molt_runtime_core::prelude::*;
use molt_runtime_core::obj_from_bits;

use crate::bridge::{
    alloc_dict_with_pairs, alloc_list, alloc_string,
    alloc_tuple, attr_name_bits_from_bytes, dec_ref_bits, exception_pending, inc_ref_bits,
    is_truthy, object_type_id, raise_exception, seq_vec_ref,
    string_obj_to_owned, to_i64, dict_get_in_place, dict_set_in_place,
    dict_order_clone, molt_iter, molt_iter_next, call_callable1,
};

// ---------------------------------------------------------------------------
// Re flag constants (kept in sync with functions.rs and re/__init__.py)
// ---------------------------------------------------------------------------

const RE_IGNORECASE: i64 = 2;
const RE_VERBOSE: i64 = 64;
const RE_ASCII: i64 = 256;

// Sentinel returned when the intrinsic cannot evaluate the sub-pattern and
// Python must fall back to its own engine.
const SENTINEL_FALLBACK: i64 = -2;

// ---------------------------------------------------------------------------
// Internal helpers
// ---------------------------------------------------------------------------

/// Collect character positions of a single Unicode codepoint at logical index
/// `idx` within a `chars` slice.  Returns `None` if out of range.
#[inline]
fn char_at(chars: &[char], idx: i64) -> Option<char> {
    let i = usize::try_from(idx).ok()?;
    chars.get(i).copied()
}

/// Try to match a literal string starting at char-index `pos` in `chars`,
/// honouring the IGNORECASE flag.  Returns `true` on a match.
fn literal_matches_at(chars: &[char], pos: usize, literal: &[char], flags: i64) -> bool {
    if pos + literal.len() > chars.len() {
        return false;
    }
    if flags & RE_IGNORECASE != 0 {
        for (a, b) in chars[pos..pos + literal.len()].iter().zip(literal.iter()) {
            // Cheap lowercase comparison — good enough for ASCII; Unicode
            // casefold would need a separate crate and adds significant
            // binary size.  The Python-side engine already does the heavy
            // lifting for UNICODE + IGNORECASE patterns.
            let al: String = a.to_lowercase().collect();
            let bl: String = b.to_lowercase().collect();
            if al != bl {
                return false;
            }
        }
        true
    } else {
        chars[pos..pos + literal.len()] == *literal
    }
}

/// Determine whether a character belongs to a simple character class
/// expressed as a plain string token from the pattern parser.
/// Returns `None` if the token is not a recognised simple category.
fn simple_category_matches(ch: char, category: &str, flags: i64) -> Option<bool> {
    match category {
        "d" | "digit" => Some(ch.is_ascii_digit()),
        "D" => Some(!ch.is_ascii_digit()),
        "s" | "space" => Some(matches!(
            ch,
            ' ' | '\t' | '\n' | '\r' | '\u{000C}' | '\u{000B}'
        )),
        "S" => Some(!matches!(
            ch,
            ' ' | '\t' | '\n' | '\r' | '\u{000C}' | '\u{000B}'
        )),
        "w" | "word" => {
            let is_word = ch == '_'
                || ch.is_ascii_alphanumeric()
                || (flags & RE_ASCII == 0 && (ch as u32) >= 128);
            Some(is_word)
        }
        "W" => {
            let is_word = ch == '_'
                || ch.is_ascii_alphanumeric()
                || (flags & RE_ASCII == 0 && (ch as u32) >= 128);
            Some(!is_word)
        }
        _ => None,
    }
}

// ---------------------------------------------------------------------------
// Negative lookahead
// ---------------------------------------------------------------------------

/// Core logic for negative lookahead.
///
/// `pattern` is a *pre-parsed* sub-pattern descriptor coming from the Python
/// side.  We only handle two fast-path forms here:
///
/// * `"lit:<text>"` — the sub-pattern is a literal string.
/// * `"cat:<name>"` — the sub-pattern is a single character category (d, s, w
///   and their negations, upper-case variants).
/// * Any other prefix → return `SENTINEL_FALLBACK` so Python falls back.
///
/// Returns:
///   1   → lookahead succeeds (sub-pattern does NOT match at pos)
///   0   → lookahead fails   (sub-pattern DOES match at pos)
///  -2   → complex sub-pattern; Python must handle it
///  -1   → out-of-bounds / error
fn re_negative_lookahead_impl(text: &str, pos: i64, end: i64, pattern: &str, flags: i64) -> i64 {
    let text_chars: Vec<char> = text.chars().collect();
    let text_len = i64::try_from(text_chars.len()).unwrap_or(i64::MAX);

    if pos < 0 || end < 0 || pos > end || end > text_len {
        return -1;
    }

    let pos_usize = match usize::try_from(pos) {
        Ok(v) => v,
        Err(_) => return -1,
    };

    if let Some(lit) = pattern.strip_prefix("lit:") {
        // Literal fast path.
        let lit_chars: Vec<char> = lit.chars().collect();
        let matched = literal_matches_at(&text_chars, pos_usize, &lit_chars, flags);
        return if matched { 0 } else { 1 };
    }

    if let Some(cat) = pattern.strip_prefix("cat:") {
        // Single-character category fast path.
        let Some(ch) = char_at(&text_chars, pos) else {
            // No character at pos means the pattern cannot match → lookahead succeeds.
            return 1;
        };
        if let Some(matched) = simple_category_matches(ch, cat, flags) {
            return if matched { 0 } else { 1 };
        }
        // Unknown category — fall back.
        return SENTINEL_FALLBACK;
    }

    if let Some(cls) = pattern.strip_prefix("cls:") {
        // Negated/normal single-char class expressed as "cls:<negated>:<chars>"
        // where negated is "1" or "0" and chars is a flat char sequence.
        // Format: "cls:0:abc" → not negated, chars={a,b,c}
        let mut parts = cls.splitn(2, ':');
        let negated_str = parts.next().unwrap_or("0");
        let chars_str = parts.next().unwrap_or("");
        let negated = negated_str == "1";
        let Some(ch) = char_at(&text_chars, pos) else {
            // No character → pattern cannot match → lookahead succeeds.
            return 1;
        };
        let hit = if flags & RE_IGNORECASE != 0 {
            // Compare lowercased characters.
            let ch_lower: char = ch.to_lowercase().next().unwrap_or(ch);
            chars_str
                .chars()
                .any(|c| c.to_lowercase().next().unwrap_or(c) == ch_lower)
        } else {
            chars_str.chars().any(|c| c == ch)
        };
        let matched = if negated { !hit } else { hit };
        return if matched { 0 } else { 1 };
    }

    // Complex sub-pattern — signal Python to handle it.
    SENTINEL_FALLBACK
}

/// Core logic for positive lookahead.
///
/// Returns:
///   1   → lookahead succeeds (sub-pattern DOES match at pos)
///   0   → lookahead fails   (sub-pattern does NOT match at pos)
///  -2   → complex sub-pattern; Python must handle it
///  -1   → out-of-bounds / error
fn re_positive_lookahead_impl(text: &str, pos: i64, end: i64, pattern: &str, flags: i64) -> i64 {
    match re_negative_lookahead_impl(text, pos, end, pattern, flags) {
        0 => 1,
        1 => 0,
        SENTINEL_FALLBACK => SENTINEL_FALLBACK,
        _ => -1,
    }
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_re_positive_lookahead(
    text_bits: u64,
    pos_bits: u64,
    end_bits: u64,
    pattern_bits: u64,
    flags_bits: u64,
) -> u64 {
    molt_runtime_core::with_core_gil!(_py, {
        let Some(text) = string_obj_to_owned(obj_from_bits(text_bits)) else {
            return raise_exception::<_>(_py, "TypeError", "text must be str");
        };
        let Some(pos) = to_i64(obj_from_bits(pos_bits)) else {
            return raise_exception::<_>(_py, "TypeError", "pos must be int");
        };
        let Some(end) = to_i64(obj_from_bits(end_bits)) else {
            return raise_exception::<_>(_py, "TypeError", "end must be int");
        };
        let Some(pattern) = string_obj_to_owned(obj_from_bits(pattern_bits)) else {
            return raise_exception::<_>(_py, "TypeError", "pattern must be str");
        };
        let Some(flags) = to_i64(obj_from_bits(flags_bits)) else {
            return raise_exception::<_>(_py, "TypeError", "flags must be int");
        };
        let result = re_positive_lookahead_impl(&text, pos, end, &pattern, flags);
        MoltObject::from_int(result).bits()
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_re_negative_lookahead(
    text_bits: u64,
    pos_bits: u64,
    end_bits: u64,
    pattern_bits: u64,
    flags_bits: u64,
) -> u64 {
    molt_runtime_core::with_core_gil!(_py, {
        let Some(text) = string_obj_to_owned(obj_from_bits(text_bits)) else {
            return raise_exception::<_>(_py, "TypeError", "text must be str");
        };
        let Some(pos) = to_i64(obj_from_bits(pos_bits)) else {
            return raise_exception::<_>(_py, "TypeError", "pos must be int");
        };
        let Some(end) = to_i64(obj_from_bits(end_bits)) else {
            return raise_exception::<_>(_py, "TypeError", "end must be int");
        };
        let Some(pattern) = string_obj_to_owned(obj_from_bits(pattern_bits)) else {
            return raise_exception::<_>(_py, "TypeError", "pattern must be str");
        };
        let Some(flags) = to_i64(obj_from_bits(flags_bits)) else {
            return raise_exception::<_>(_py, "TypeError", "flags must be int");
        };
        let result = re_negative_lookahead_impl(&text, pos, end, &pattern, flags);
        MoltObject::from_int(result).bits()
    })
}

// ---------------------------------------------------------------------------
// Negative lookbehind
// ---------------------------------------------------------------------------

/// Core logic for negative lookbehind.
///
/// `width` is the fixed width of the sub-pattern (already validated by the
/// Python parser).  We check the substring `text[pos-width .. pos]`.
///
/// Same pattern descriptor protocol as negative lookahead.
///
/// Returns:
///   1   → lookbehind succeeds (sub-pattern does NOT match ending at pos)
///   0   → lookbehind fails   (sub-pattern DOES match ending at pos)
///  -2   → complex sub-pattern; Python must handle it
///  -1   → out-of-bounds / error
fn re_negative_lookbehind_impl(
    text: &str,
    pos: i64,
    end: i64,
    pattern: &str,
    width: i64,
    flags: i64,
) -> i64 {
    let text_chars: Vec<char> = text.chars().collect();
    let text_len = i64::try_from(text_chars.len()).unwrap_or(i64::MAX);

    if pos < 0 || end < 0 || pos > end || end > text_len || width < 0 {
        return -1;
    }

    let start = pos - width;
    if start < 0 {
        // Not enough text behind the cursor — sub-pattern cannot match →
        // lookbehind succeeds (negative).
        return 1;
    }

    let start_usize = match usize::try_from(start) {
        Ok(v) => v,
        Err(_) => return -1,
    };

    if let Some(lit) = pattern.strip_prefix("lit:") {
        let lit_chars: Vec<char> = lit.chars().collect();
        // The width of a literal is its char length.  Sanity-check.
        if lit_chars.len() as i64 != width {
            return -1;
        }
        let matched = literal_matches_at(&text_chars, start_usize, &lit_chars, flags);
        return if matched { 0 } else { 1 };
    }

    if let Some(cat) = pattern.strip_prefix("cat:") {
        // Single-char category.  Width must be 1.
        if width != 1 {
            return -1;
        }
        let Some(ch) = char_at(&text_chars, start) else {
            return 1; // no char → no match → succeeds
        };
        if let Some(matched) = simple_category_matches(ch, cat, flags) {
            return if matched { 0 } else { 1 };
        }
        return SENTINEL_FALLBACK;
    }

    if let Some(cls) = pattern.strip_prefix("cls:") {
        // Same encoding as in lookahead.
        if width != 1 {
            return -1;
        }
        let mut parts = cls.splitn(2, ':');
        let negated_str = parts.next().unwrap_or("0");
        let chars_str = parts.next().unwrap_or("");
        let negated = negated_str == "1";
        let Some(ch) = char_at(&text_chars, start) else {
            return 1;
        };
        let hit = if flags & RE_IGNORECASE != 0 {
            let ch_lower: char = ch.to_lowercase().next().unwrap_or(ch);
            chars_str
                .chars()
                .any(|c| c.to_lowercase().next().unwrap_or(c) == ch_lower)
        } else {
            chars_str.chars().any(|c| c == ch)
        };
        let matched = if negated { !hit } else { hit };
        return if matched { 0 } else { 1 };
    }

    SENTINEL_FALLBACK
}

/// Core logic for positive lookbehind.
///
/// Returns:
///   1   → lookbehind succeeds (sub-pattern DOES match ending at pos)
///   0   → lookbehind fails   (sub-pattern does NOT match ending at pos)
///  -2   → complex sub-pattern; Python must handle it
///  -1   → out-of-bounds / error
fn re_positive_lookbehind_impl(
    text: &str,
    pos: i64,
    end: i64,
    pattern: &str,
    width: i64,
    flags: i64,
) -> i64 {
    match re_negative_lookbehind_impl(text, pos, end, pattern, width, flags) {
        0 => 1,
        1 => 0,
        SENTINEL_FALLBACK => SENTINEL_FALLBACK,
        _ => -1,
    }
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_re_positive_lookbehind(
    text_bits: u64,
    pos_bits: u64,
    end_bits: u64,
    pattern_bits: u64,
    width_bits: u64,
    flags_bits: u64,
) -> u64 {
    molt_runtime_core::with_core_gil!(_py, {
        let Some(text) = string_obj_to_owned(obj_from_bits(text_bits)) else {
            return raise_exception::<_>(_py, "TypeError", "text must be str");
        };
        let Some(pos) = to_i64(obj_from_bits(pos_bits)) else {
            return raise_exception::<_>(_py, "TypeError", "pos must be int");
        };
        let Some(end) = to_i64(obj_from_bits(end_bits)) else {
            return raise_exception::<_>(_py, "TypeError", "end must be int");
        };
        let Some(pattern) = string_obj_to_owned(obj_from_bits(pattern_bits)) else {
            return raise_exception::<_>(_py, "TypeError", "pattern must be str");
        };
        let Some(width) = to_i64(obj_from_bits(width_bits)) else {
            return raise_exception::<_>(_py, "TypeError", "width must be int");
        };
        let Some(flags) = to_i64(obj_from_bits(flags_bits)) else {
            return raise_exception::<_>(_py, "TypeError", "flags must be int");
        };
        let result = re_positive_lookbehind_impl(&text, pos, end, &pattern, width, flags);
        MoltObject::from_int(result).bits()
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_re_negative_lookbehind(
    text_bits: u64,
    pos_bits: u64,
    end_bits: u64,
    pattern_bits: u64,
    width_bits: u64,
    flags_bits: u64,
) -> u64 {
    molt_runtime_core::with_core_gil!(_py, {
        let Some(text) = string_obj_to_owned(obj_from_bits(text_bits)) else {
            return raise_exception::<_>(_py, "TypeError", "text must be str");
        };
        let Some(pos) = to_i64(obj_from_bits(pos_bits)) else {
            return raise_exception::<_>(_py, "TypeError", "pos must be int");
        };
        let Some(end) = to_i64(obj_from_bits(end_bits)) else {
            return raise_exception::<_>(_py, "TypeError", "end must be int");
        };
        let Some(pattern) = string_obj_to_owned(obj_from_bits(pattern_bits)) else {
            return raise_exception::<_>(_py, "TypeError", "pattern must be str");
        };
        let Some(width) = to_i64(obj_from_bits(width_bits)) else {
            return raise_exception::<_>(_py, "TypeError", "width must be int");
        };
        let Some(flags) = to_i64(obj_from_bits(flags_bits)) else {
            return raise_exception::<_>(_py, "TypeError", "flags must be int");
        };
        let result = re_negative_lookbehind_impl(&text, pos, end, &pattern, width, flags);
        MoltObject::from_int(result).bits()
    })
}

// ---------------------------------------------------------------------------
// VERBOSE / X-flag pattern pre-processor
// ---------------------------------------------------------------------------

/// Strip whitespace and `#` comments from a VERBOSE-mode pattern string.
///
/// Rules (matching CPython's `sre_parse` behaviour):
/// * Outside a character class `[…]`:
///   - Unescaped whitespace is removed.
///   - `#` starts a comment that runs to the next `\n` (exclusive); the `\n`
///     itself is also consumed.
///   - `\ ` (backslash-space) is kept as a literal space.
///   - `\#` is kept as a literal `#`.
///   - All other escape sequences (`\n`, `\t`, `\\`, etc.) are passed through
///     verbatim (the downstream parser handles them).
/// * Inside a character class `[…]`:
///   - No stripping is performed; the entire class is copied verbatim.
///   - Nested `[` inside a class does not open another class (CPython does not
///     support true nesting, but does allow `[` literally).
///
/// The `flags` argument is accepted for symmetry (VERBOSE is already set when
/// this is called) but is not used internally.
fn re_strip_verbose_impl(pattern: &str, flags: i64) -> String {
    // Only strip verbose formatting when the VERBOSE flag is set.
    if flags & RE_VERBOSE == 0 {
        return pattern.to_string();
    }
    let chars: Vec<char> = pattern.chars().collect();
    let len = chars.len();
    let mut out = String::with_capacity(len);
    let mut i = 0usize;
    let mut in_class = false; // inside [...]

    while i < len {
        let ch = chars[i];

        if in_class {
            // Inside a character class: pass everything through verbatim,
            // tracking `]` to know when we exit (handle `\]` escape).
            if ch == '\\' && i + 1 < len {
                // Consume the escape pair as-is.
                out.push(ch);
                out.push(chars[i + 1]);
                i += 2;
                continue;
            }
            if ch == ']' {
                in_class = false;
            }
            out.push(ch);
            i += 1;
            continue;
        }

        // Outside a character class.
        match ch {
            '\\' if i + 1 < len => {
                let next = chars[i + 1];
                // `\ ` (backslash + space) → keep as-is (literal space in output).
                // `\#` → keep as-is (literal `#` in output).
                // Any other escape → pass through verbatim.
                out.push('\\');
                out.push(next);
                i += 2;
            }
            '#' => {
                // Comment: skip to end of line (or end of pattern).
                i += 1;
                while i < len && chars[i] != '\n' {
                    i += 1;
                }
                // Also consume the newline itself (CPython strips it too).
                if i < len && chars[i] == '\n' {
                    i += 1;
                }
            }
            '[' => {
                in_class = true;
                out.push(ch);
                i += 1;
            }
            c if c.is_whitespace() => {
                // Unescaped whitespace → strip.
                i += 1;
            }
            _ => {
                out.push(ch);
                i += 1;
            }
        }
    }

    out
}

/// `molt_re_strip_verbose(pattern: str, flags: int) -> str`
///
/// Pre-process a VERBOSE/X-mode regex pattern by removing unescaped
/// whitespace and `#` comments.  Returns the cleaned pattern string.
/// If the flags do not include VERBOSE (64) the pattern is returned unchanged.
#[unsafe(no_mangle)]
pub extern "C" fn molt_re_strip_verbose(pattern_bits: u64, flags_bits: u64) -> u64 {
    molt_runtime_core::with_core_gil!(_py, {
        let Some(pattern) = string_obj_to_owned(obj_from_bits(pattern_bits)) else {
            return raise_exception::<_>(_py, "TypeError", "pattern must be str");
        };
        let Some(flags) = to_i64(obj_from_bits(flags_bits)) else {
            return raise_exception::<_>(_py, "TypeError", "flags must be int");
        };

        let cleaned = if flags & RE_VERBOSE != 0 {
            re_strip_verbose_impl(&pattern, flags)
        } else {
            // Not VERBOSE — return the pattern unchanged to avoid a copy.
            pattern
        };

        let out_ptr = alloc_string(_py, cleaned.as_bytes());
        if out_ptr.is_null() {
            MoltObject::none().bits()
        } else {
            MoltObject::from_ptr(out_ptr).bits()
        }
    })
}

// ---------------------------------------------------------------------------
// fullmatch check
// ---------------------------------------------------------------------------

/// `molt_re_fullmatch_check(text: str, match_start: int, match_end: int) -> bool`
///
/// Returns `True` if the match spans the *entire* text (i.e. `match_start == 0`
/// and `match_end == len(text)`).
///
/// This is a thin helper so the Python-side `_fullmatch` loop can delegate the
/// boundary check into Rust without re-computing `len(text)` repeatedly when
/// many candidate positions are tried.
///
/// Arguments are passed as MoltObject bits following the intrinsic ABI.
/// * `text_bits`        — the subject string.
/// * `match_start_bits` — integer start position returned by the matcher.
/// * `match_end_bits`   — integer end   position returned by the matcher.
#[unsafe(no_mangle)]
pub extern "C" fn molt_re_fullmatch_check(
    text_bits: u64,
    match_start_bits: u64,
    match_end_bits: u64,
) -> u64 {
    molt_runtime_core::with_core_gil!(_py, {
        let Some(text) = string_obj_to_owned(obj_from_bits(text_bits)) else {
            return raise_exception::<_>(_py, "TypeError", "text must be str");
        };
        let Some(match_start) = to_i64(obj_from_bits(match_start_bits)) else {
            return raise_exception::<_>(_py, "TypeError", "match_start must be int");
        };
        let Some(match_end) = to_i64(obj_from_bits(match_end_bits)) else {
            return raise_exception::<_>(_py, "TypeError", "match_end must be int");
        };

        let text_len = i64::try_from(text.chars().count()).unwrap_or(i64::MAX);
        let spans_all = match_start == 0 && match_end == text_len;
        MoltObject::from_bool(spans_all).bits()
    })
}

// ---------------------------------------------------------------------------
// Named back-reference advance
// ---------------------------------------------------------------------------

/// Decode a Python dict whose keys are str group names and values are integer
/// group indices into a `Vec<(String, i64)>`.  The dict is the
/// `Pattern._group_names` mapping passed from the Python side.
fn decode_group_names(_py: &CoreGilToken, dict_bits: u64) -> Result<Vec<(String, i64)>, u64> {
    let dict_obj = obj_from_bits(dict_bits);
    let Some(dict_ptr) = dict_obj.as_ptr() else {
        return Err(raise_exception::<u64>(
            _py,
            "TypeError",
            "groups must be a dict",
        ));
    };
    // We iterate the dict using molt_iter, which yields (key, value) pairs.
    let iter_bits = molt_iter(_py,dict_bits);
    if exception_pending(_py) {
        return Err(MoltObject::none().bits());
    }
    let _ = dict_ptr; // suppress unused warning — dict_ptr validated above
    let mut out: Vec<(String, i64)> = Vec::new();
    loop {
        let Some(item_bits) = molt_iter_next(_py, iter_bits) else {
            // None signals StopIteration.
            break;
        };
        let item_obj = obj_from_bits(item_bits);
        let Some(item_ptr) = item_obj.as_ptr() else {
            break;
        };
        // Each item is a tuple (done_flag, value) in Molt's iterator protocol,
        // OR a raw (key, value) pair depending on how the dict iter is wrapped.
        // Use the same iter_next_pair convention: item is (value, done_bool).
        let item_ty = unsafe { object_type_id(item_ptr) };
        if item_ty != TYPE_ID_TUPLE && item_ty != TYPE_ID_LIST {
            dec_ref_bits(_py, item_bits);
            return Err(raise_exception::<u64>(
                _py,
                "TypeError",
                "dict iterator item must be a pair",
            ));
        }
        let elems = unsafe { seq_vec_ref(item_ptr) };
        if elems.len() < 2 {
            dec_ref_bits(_py, item_bits);
            return Err(raise_exception::<u64>(
                _py,
                "TypeError",
                "dict iterator pair too short",
            ));
        }
        // Molt iterator protocol: elems[0] = value, elems[1] = done (bool).
        let val_bits = elems[0];
        let done_bits = elems[1];
        let done = is_truthy(_py, obj_from_bits(done_bits));
        dec_ref_bits(_py, item_bits);
        if done {
            break;
        }
        // val_bits should be a (name, index) tuple from the dict items iterator.
        let pair_obj = obj_from_bits(val_bits);
        let Some(pair_ptr) = pair_obj.as_ptr() else {
            continue;
        };
        let pair_ty = unsafe { object_type_id(pair_ptr) };
        if pair_ty != TYPE_ID_TUPLE && pair_ty != TYPE_ID_LIST {
            continue;
        }
        let pair = unsafe { seq_vec_ref(pair_ptr) };
        if pair.len() < 2 {
            continue;
        }
        let Some(name) = string_obj_to_owned(obj_from_bits(pair[0])) else {
            continue;
        };
        let Some(idx) = to_i64(obj_from_bits(pair[1])) else {
            continue;
        };
        out.push((name, idx));
    }
    Ok(out)
}

/// Decode a groups sequence (list/tuple of `None | (start, end)` pairs) into
/// a `Vec<Option<(i64, i64)>>` exactly as `re_group_spans_from_sequence` in
/// functions.rs does — duplicated here to keep this module self-contained.
fn decode_group_spans(
    _py: &CoreGilToken,
    groups_bits: u64,
) -> Result<Vec<Option<(i64, i64)>>, u64> {
    let groups_obj = obj_from_bits(groups_bits);
    let Some(groups_ptr) = groups_obj.as_ptr() else {
        return Err(raise_exception::<u64>(
            _py,
            "TypeError",
            "groups must be a sequence",
        ));
    };
    let groups_ty = unsafe { object_type_id(groups_ptr) };
    if groups_ty != TYPE_ID_LIST && groups_ty != TYPE_ID_TUPLE {
        return Err(raise_exception::<u64>(
            _py,
            "TypeError",
            "groups must be a sequence",
        ));
    }
    let mut out: Vec<Option<(i64, i64)>> = Vec::new();
    let elems = unsafe { seq_vec_ref(groups_ptr) };
    for &elem_bits in elems.iter() {
        let elem_obj = obj_from_bits(elem_bits);
        if elem_obj.is_none() {
            out.push(None);
            continue;
        }
        let Some(elem_ptr) = elem_obj.as_ptr() else {
            return Err(raise_exception::<u64>(
                _py,
                "TypeError",
                "group span must be (int, int) or None",
            ));
        };
        let elem_ty = unsafe { object_type_id(elem_ptr) };
        if elem_ty != TYPE_ID_LIST && elem_ty != TYPE_ID_TUPLE {
            return Err(raise_exception::<u64>(
                _py,
                "TypeError",
                "group span must be (int, int) or None",
            ));
        }
        let span = unsafe { seq_vec_ref(elem_ptr) };
        if span.len() < 2 {
            return Err(raise_exception::<u64>(
                _py,
                "TypeError",
                "group span must contain start and end",
            ));
        }
        let Some(start) = to_i64(obj_from_bits(span[0])) else {
            return Err(raise_exception::<u64>(
                _py,
                "TypeError",
                "group span start must be int",
            ));
        };
        let Some(end) = to_i64(obj_from_bits(span[1])) else {
            return Err(raise_exception::<u64>(
                _py,
                "TypeError",
                "group span end must be int",
            ));
        };
        out.push(Some((start, end)));
    }
    Ok(out)
}

/// Core logic for named back-reference advance.
///
/// Looks up `name` in the `group_names` dict to find the group index, then
/// reads that group's captured span from `groups`, and tries to match the
/// captured text at the current position in `text`.
///
/// Returns the new position on success, or -1 on failure / no-match.
fn re_named_backref_advance_impl(
    text: &str,
    pos: i64,
    end: i64,
    group_spans: &[Option<(i64, i64)>],
    name: &str,
    group_names: &[(String, i64)],
) -> i64 {
    // Look up the group index by name.
    let group_idx = group_names.iter().find(|(n, _)| n == name).map(|(_, i)| *i);
    let Some(idx) = group_idx else {
        return -1; // unknown group name
    };
    let idx_usize = match usize::try_from(idx) {
        Ok(v) => v,
        Err(_) => return -1,
    };
    // Get the captured span for this group.
    let Some(Some((cap_start, cap_end))) = group_spans.get(idx_usize) else {
        return -1; // group not captured
    };
    let cap_start = *cap_start;
    let cap_end = *cap_end;
    if cap_start < 0 || cap_end < cap_start {
        return -1;
    }

    // Delegate to the same byte-comparison logic used by backref_advance.
    let text_chars: Vec<char> = text.chars().collect();
    let text_len = i64::try_from(text_chars.len()).unwrap_or(i64::MAX);

    if pos < 0 || end < 0 || pos > end || end > text_len {
        return -1;
    }
    if cap_end > text_len {
        return -1;
    }

    let ref_len = cap_end - cap_start;
    let Some(pos_end) = pos.checked_add(ref_len) else {
        return -1;
    };
    if pos_end > end {
        return -1;
    }

    let Some(start_idx) = usize::try_from(cap_start).ok() else {
        return -1;
    };
    let Some(pos_idx) = usize::try_from(pos).ok() else {
        return -1;
    };
    let Some(ref_len_usize) = usize::try_from(ref_len).ok() else {
        return -1;
    };

    for i in 0..ref_len_usize {
        if text_chars[start_idx + i] != text_chars[pos_idx + i] {
            return -1;
        }
    }
    pos_end
}

/// `molt_re_named_backref_advance(text, pos, end, groups, name) -> int`
///
/// Advance past a named back-reference.  `groups` is the live group-span
/// tuple/list (the same one threaded through `_match_node`).  `name` is the
/// string group name.  The function resolves the name to an index via the
/// `groups` mapping that the Python caller passes as a dict.
///
/// NOTE: Because the Python-side `_Backref` node only stores an integer index,
/// named back-references are pre-resolved to indices by the parser and stored
/// as `_Backref(index)`.  This intrinsic exists for the rare case where a
/// pattern explicitly uses `(?P=name)` syntax and the caller wants to delegate
/// the name→index lookup to Rust.  In all current Python-side code paths the
/// name is resolved to an index before the match loop; this intrinsic is
/// therefore an optional accelerator for future parser evolution.
///
/// Arguments:
///   `text_bits`   — subject string
///   `pos_bits`    — current position (int)
///   `end_bits`    — search endpoint (int)
///   `groups_bits` — live group-span sequence (tuple/list of (start,end)|None)
///   `name_bits`   — group name string
///
/// The caller must also pass the group-names dict as the *sixth* argument so
/// the name can be resolved.  To keep the ABI consistent with other 5-arg
/// intrinsics the group-names dict is encoded into `name_bits` using the
/// format `"<name>\x00<json-like dict encoding>"` — however, for simplicity
/// the Python side is expected to pre-resolve the name and pass the index
/// via `molt_re_backref_group_advance` instead.  This intrinsic therefore
/// accepts `groups` as the *group_names dict* and `name` as the literal group
/// name string, resolving internally.
///
/// Returns the new position as int, or -1 on no-match / error.
#[unsafe(no_mangle)]
pub extern "C" fn molt_re_named_backref_advance(
    text_bits: u64,
    pos_bits: u64,
    end_bits: u64,
    groups_bits: u64,
    name_bits: u64,
) -> u64 {
    molt_runtime_core::with_core_gil!(_py, {
        // Validate that text, pos, end, and name are correctly typed even if
        // the current implementation returns early before using them.
        let Some(_text) = string_obj_to_owned(obj_from_bits(text_bits)) else {
            return raise_exception::<_>(_py, "TypeError", "text must be str");
        };
        let Some(_pos) = to_i64(obj_from_bits(pos_bits)) else {
            return raise_exception::<_>(_py, "TypeError", "pos must be int");
        };
        let Some(_end) = to_i64(obj_from_bits(end_bits)) else {
            return raise_exception::<_>(_py, "TypeError", "end must be int");
        };
        let Some(_name) = string_obj_to_owned(obj_from_bits(name_bits)) else {
            return raise_exception::<_>(_py, "TypeError", "name must be str");
        };

        // `groups_bits` is expected to be a dict: {name: index, ...}
        // We check the type to decide whether we received a dict (groups_names
        // lookup path) or a sequence (span array).  If it is a dict we decode
        // the names map and use a zero-length span array (index only); if it is
        // a sequence we treat it as the span array with name pre-resolved
        // externally and try to decode it as groups + a companion name→index
        // lookup (not possible without the dict).  The canonical call pattern
        // from Python passes the groups dict.
        let groups_obj = obj_from_bits(groups_bits);
        if groups_obj.is_none() {
            return MoltObject::from_int(-1).bits();
        }
        let Some(groups_ptr) = groups_obj.as_ptr() else {
            return raise_exception::<_>(_py, "TypeError", "groups must be a dict or sequence");
        };
        let groups_ty = unsafe { object_type_id(groups_ptr) };

        if groups_ty == TYPE_ID_DICT {
            // groups_bits is the group_names dict.  We have no span array in
            // this call — return -1 (cannot resolve without captured spans).
            // The Python side must pass both the span tuple and the names dict.
            // For backward-compatibility we accept this call and return -1.
            let _ = groups_ptr;
            return MoltObject::from_int(-1).bits();
        }

        // groups_bits is the span sequence; name is the group name.  We cannot
        // resolve name→index without the group_names dict.  Signal -1.
        if groups_ty == TYPE_ID_LIST || groups_ty == TYPE_ID_TUPLE {
            let _spans = match decode_group_spans(_py, groups_bits) {
                Ok(v) => v,
                Err(e) => return e,
            };
            // No group_names dict provided in this signature variant — -1.
            return MoltObject::from_int(-1).bits();
        }

        raise_exception::<_>(_py, "TypeError", "groups must be a dict or sequence")
    })
}

// ---------------------------------------------------------------------------
// Phase-1 regex compiler: IR parser + compiled-pattern registry
// ---------------------------------------------------------------------------
//
// This section implements the Rust-side `molt_re_compile` / `molt_re_pattern_info`
// intrinsics (Phase 1).  The match engine (`molt_re_execute` /
// `molt_re_finditer_collect`) is a Phase-1b stub that always returns None /
// empty list, signalling Python to fall back to its own match engine.
//
// Design notes:
// * No `regex` crate — backreferences are required and not supported there.
// * Hand-rolled recursive-descent parser that mirrors the Python `_Parser` class
//   in `src/molt/stdlib/re/__init__.py` exactly (same quirks, same IR shape).
// * Global registry uses `LazyLock<Mutex<…>>` (not thread_local!) so that
//   compiled patterns are visible across threads.
// * Handle allocation starts at 1 so that 0 can serve as "invalid".

use std::collections::HashMap;
use std::sync::{
    LazyLock, Mutex,
    atomic::{AtomicI64, Ordering},
};

// ---------------------------------------------------------------------------
// Re-use the flag constants already defined at the top of this file.
// (RE_IGNORECASE = 2, RE_VERBOSE = 64, RE_ASCII = 256)
// Additional flags:
const RE_MULTILINE: i64 = 8;
const RE_DOTALL: i64 = 16;
const RE_UNICODE: i64 = 32;
const RE_LOCALE: i64 = 4;

// ---------------------------------------------------------------------------
// IR node enum — mirrors the Python dataclasses in re/__init__.py
// ---------------------------------------------------------------------------

#[derive(Debug, Clone)]
pub(crate) enum ReNode {
    Empty,
    Literal(String),
    Any,
    Anchor(String),
    CharClass {
        negated: bool,
        ranges: Vec<(String, String)>,
        chars: Vec<String>,
        categories: Vec<String>,
    },
    Concat(Vec<ReNode>),
    Alt(Vec<ReNode>),
    Repeat {
        node: Box<ReNode>,
        min_count: u64,
        max_count: Option<u64>,
        greedy: bool,
    },
    Group {
        node: Box<ReNode>,
        index: u32,
    },
    Backref(u32),
    Look {
        node: Box<ReNode>,
        behind: bool,
        positive: bool,
        width: Option<u64>,
    },
    ScopedFlags {
        node: Box<ReNode>,
        add_flags: i64,
        clear_flags: i64,
    },
    Conditional {
        group_index: u32,
        yes: Box<ReNode>,
        no: Box<ReNode>,
    },
}

// ---------------------------------------------------------------------------
// Compiled pattern — stored in the global registry
// ---------------------------------------------------------------------------

#[derive(Debug)]
pub(crate) struct CompiledPattern {
    pub root: ReNode,
    pub group_count: u32,
    pub group_names: HashMap<String, u32>,
    pub flags: i64,
    /// Position (char index) of a nested-set-in-charclass warning, or None.
    pub warn_pos: Option<i64>,
}

// ---------------------------------------------------------------------------
// Global pattern registry
// ---------------------------------------------------------------------------

static RE_NEXT_HANDLE: AtomicI64 = AtomicI64::new(1);
static RE_PATTERNS: LazyLock<Mutex<HashMap<i64, CompiledPattern>>> =
    LazyLock::new(|| Mutex::new(HashMap::new()));

fn re_alloc_handle() -> i64 {
    RE_NEXT_HANDLE.fetch_add(1, Ordering::Relaxed)
}

fn re_store_pattern(handle: i64, pattern: CompiledPattern) {
    let mut guard = RE_PATTERNS.lock().unwrap_or_else(|e| e.into_inner());
    guard.insert(handle, pattern);
}

// ---------------------------------------------------------------------------
// Parser state — mirrors _Parser in re/__init__.py
// ---------------------------------------------------------------------------

struct ReParser {
    chars: Vec<char>,
    pos: usize,
    group_count: u32,
    group_names: HashMap<String, u32>,
    /// Fixed widths keyed by group index — used for look-behind validation.
    group_widths: HashMap<u32, Option<u64>>,
    open_group_names: std::collections::HashSet<String>,
    flags: i64,
    inline_flags: i64,
    nested_set_warning_pos: Option<i64>,
    in_class: bool,
}

impl ReParser {
    fn new(pattern: &str, flags: i64) -> Self {
        Self {
            chars: pattern.chars().collect(),
            pos: 0,
            group_count: 0,
            group_names: HashMap::new(),
            group_widths: HashMap::new(),
            open_group_names: std::collections::HashSet::new(),
            flags,
            inline_flags: 0,
            nested_set_warning_pos: None,
            in_class: false,
        }
    }

    fn len(&self) -> usize {
        self.chars.len()
    }

    fn is_verbose(&self) -> bool {
        (self.flags | self.inline_flags) & RE_VERBOSE != 0
    }

    fn skip_verbose_whitespace(&mut self) {
        if self.in_class || !self.is_verbose() {
            return;
        }
        while self.pos < self.len() {
            let ch = self.chars[self.pos];
            if ch == '#' {
                self.pos += 1;
                while self.pos < self.len() && self.chars[self.pos] != '\n' {
                    self.pos += 1;
                }
                if self.pos < self.len() {
                    self.pos += 1; // consume '\n'
                }
            } else if matches!(ch, ' ' | '\t' | '\n' | '\r' | '\x0C' | '\x0B') {
                self.pos += 1;
            } else {
                break;
            }
        }
    }

    fn peek(&mut self) -> Option<char> {
        self.skip_verbose_whitespace();
        self.chars.get(self.pos).copied()
    }

    fn next_ch(&mut self) -> Result<char, String> {
        self.skip_verbose_whitespace();
        if self.pos >= self.len() {
            return Err("unexpected end of pattern".to_string());
        }
        let ch = self.chars[self.pos];
        self.pos += 1;
        Ok(ch)
    }

    fn raw_peek(&self) -> Option<char> {
        self.chars.get(self.pos).copied()
    }

    fn raw_next(&mut self) -> Result<char, String> {
        if self.pos >= self.len() {
            return Err("unexpected end of pattern".to_string());
        }
        let ch = self.chars[self.pos];
        self.pos += 1;
        Ok(ch)
    }

    // -----------------------------------------------------------------------
    // Top-level parse entry
    // -----------------------------------------------------------------------

    fn parse(&mut self) -> Result<ReNode, String> {
        let node = self.parse_expr()?;
        self.skip_verbose_whitespace();
        if self.pos != self.len() {
            return Err("unexpected pattern text".to_string());
        }
        Ok(node)
    }

    // -----------------------------------------------------------------------
    // Expression = alternation
    // -----------------------------------------------------------------------

    fn parse_expr(&mut self) -> Result<ReNode, String> {
        let mut terms = vec![self.parse_term()?];
        while self.peek() == Some('|') {
            self.next_ch()?;
            terms.push(self.parse_term()?);
        }
        if terms.len() == 1 {
            Ok(terms.remove(0))
        } else {
            Ok(ReNode::Alt(terms))
        }
    }

    // -----------------------------------------------------------------------
    // Term = sequence of factors
    // -----------------------------------------------------------------------

    fn parse_term(&mut self) -> Result<ReNode, String> {
        let mut nodes: Vec<ReNode> = Vec::new();
        loop {
            let ch = self.peek();
            if ch.is_none() || ch == Some(')') || ch == Some('|') {
                break;
            }
            let node = self.parse_factor()?;
            // Coalesce adjacent literals.
            if let ReNode::Literal(ref new_text) = node
                && let Some(ReNode::Literal(prev_text)) = nodes.last_mut()
            {
                let combined = prev_text.clone() + new_text;
                *prev_text = combined;
                continue;
            }
            nodes.push(node);
        }
        if nodes.is_empty() {
            return Ok(ReNode::Empty);
        }
        if nodes.len() == 1 {
            return Ok(nodes.remove(0));
        }
        Ok(ReNode::Concat(nodes))
    }

    // -----------------------------------------------------------------------
    // Factor = atom + optional quantifier
    // -----------------------------------------------------------------------

    fn parse_factor(&mut self) -> Result<ReNode, String> {
        let node = self.parse_atom()?;
        let ch = self.peek();
        if ch.is_none() {
            return Ok(node);
        }
        match ch.unwrap() {
            '*' | '+' | '?' => {
                let quant = self.next_ch()?;
                let (min_count, max_count) = match quant {
                    '*' => (0, None),
                    '+' => (1, None),
                    '?' => (0, Some(1)),
                    _ => unreachable!(),
                };
                let greedy = if self.peek() == Some('?') {
                    self.next_ch()?;
                    false
                } else {
                    true
                };
                Ok(ReNode::Repeat {
                    node: Box::new(node),
                    min_count,
                    max_count,
                    greedy,
                })
            }
            '{' => {
                let start_pos = self.pos;
                self.next_ch()?; // consume '{'
                let min_res = self.parse_number();
                if min_res.is_err() {
                    // Not a valid quantifier — backtrack.
                    self.pos = start_pos;
                    return Ok(node);
                }
                let min_count = min_res.unwrap();
                let max_count = if self.peek() == Some(',') {
                    self.next_ch()?; // consume ','
                    if self.peek() == Some('}') {
                        None // {n,} — unbounded
                    } else {
                        let max_res = self.parse_number();
                        if max_res.is_err() {
                            self.pos = start_pos;
                            return Ok(node);
                        }
                        Some(max_res.unwrap())
                    }
                } else {
                    Some(min_count)
                };
                if self.peek() != Some('}') {
                    // Backtrack — not a valid quantifier.
                    self.pos = start_pos;
                    return Ok(node);
                }
                self.next_ch()?; // consume '}'
                if let Some(max) = max_count
                    && max < min_count
                {
                    return Err("invalid quantifier range".to_string());
                }
                let greedy = if self.peek() == Some('?') {
                    self.next_ch()?;
                    false
                } else {
                    true
                };
                Ok(ReNode::Repeat {
                    node: Box::new(node),
                    min_count,
                    max_count,
                    greedy,
                })
            }
            _ => Ok(node),
        }
    }

    fn parse_number(&mut self) -> Result<u64, String> {
        let mut digits = String::new();
        loop {
            match self.peek() {
                Some(c) if c.is_ascii_digit() => {
                    self.next_ch()?;
                    digits.push(c);
                }
                _ => break,
            }
        }
        if digits.is_empty() {
            return Err("expected number".to_string());
        }
        digits
            .parse::<u64>()
            .map_err(|_| "number overflow".to_string())
    }

    // -----------------------------------------------------------------------
    // Atom
    // -----------------------------------------------------------------------

    fn parse_atom(&mut self) -> Result<ReNode, String> {
        let ch = self.next_ch()?;
        match ch {
            '.' => Ok(ReNode::Any),
            '^' => Ok(ReNode::Anchor("start".to_string())),
            '$' => Ok(ReNode::Anchor("end".to_string())),
            '(' => self.parse_group(),
            '[' => self.parse_class(),
            '\\' => self.parse_escape(),
            c if ")*+?{}|".contains(c) => Err(format!("unexpected character '{c}'")),
            c => Ok(ReNode::Literal(c.to_string())),
        }
    }

    // -----------------------------------------------------------------------
    // Group: (...) and (?...)
    // -----------------------------------------------------------------------

    fn parse_group(&mut self) -> Result<ReNode, String> {
        if self.peek() == Some('?') {
            self.next_ch()?; // consume '?'
            return self.parse_extension_group();
        }
        // Capturing group — assign index at open-paren time (CPython order).
        self.group_count += 1;
        let idx = self.group_count;
        let node = self.parse_expr()?;
        if self.peek() != Some(')') {
            return Err("missing )".to_string());
        }
        self.next_ch()?;
        let width = fixed_width(&node, Some(&self.group_widths));
        self.group_widths.insert(idx, width);
        Ok(ReNode::Group {
            node: Box::new(node),
            index: idx,
        })
    }

    fn parse_extension_group(&mut self) -> Result<ReNode, String> {
        let marker = self.peek();
        match marker {
            Some('=') => {
                self.next_ch()?;
                let node = self.parse_expr()?;
                if self.peek() != Some(')') {
                    return Err("missing )".to_string());
                }
                self.next_ch()?;
                Ok(ReNode::Look {
                    node: Box::new(node),
                    behind: false,
                    positive: true,
                    width: None,
                })
            }
            Some('!') => {
                self.next_ch()?;
                let node = self.parse_expr()?;
                if self.peek() != Some(')') {
                    return Err("missing )".to_string());
                }
                self.next_ch()?;
                Ok(ReNode::Look {
                    node: Box::new(node),
                    behind: false,
                    positive: false,
                    width: None,
                })
            }
            Some('<') => {
                self.next_ch()?; // consume '<'
                let look_kind = self.peek();
                match look_kind {
                    Some('=') => {
                        self.next_ch()?;
                        let node = self.parse_expr()?;
                        if self.peek() != Some(')') {
                            return Err("missing )".to_string());
                        }
                        self.next_ch()?;
                        let width =
                            fixed_width(&node, Some(&self.group_widths)).ok_or_else(|| {
                                "look-behind requires fixed-width pattern".to_string()
                            })?;
                        Ok(ReNode::Look {
                            node: Box::new(node),
                            behind: true,
                            positive: true,
                            width: Some(width),
                        })
                    }
                    Some('!') => {
                        self.next_ch()?;
                        let node = self.parse_expr()?;
                        if self.peek() != Some(')') {
                            return Err("missing )".to_string());
                        }
                        self.next_ch()?;
                        let width =
                            fixed_width(&node, Some(&self.group_widths)).ok_or_else(|| {
                                "look-behind requires fixed-width pattern".to_string()
                            })?;
                        Ok(ReNode::Look {
                            node: Box::new(node),
                            behind: true,
                            positive: false,
                            width: Some(width),
                        })
                    }
                    _ => {
                        // Named group: (?P<name>...) already consumed '<', so
                        // this is a raw (?<name>...) style from parse_extension_group.
                        // Actually we get here only when parse_extension_group sees
                        // marker=='<' and dispatches here.  We have already consumed '<'.
                        // So we must handle the named-group body:
                        self.parse_named_group_body()
                    }
                }
            }
            Some('(') => {
                // Conditional: (?(id)yes|no)
                self.next_ch()?; // consume '('
                let mut digits = String::new();
                loop {
                    match self.peek() {
                        Some(c) if c.is_ascii_digit() => {
                            self.next_ch()?;
                            digits.push(c);
                        }
                        _ => break,
                    }
                }
                if digits.is_empty() || self.peek() != Some(')') {
                    return Err("bad character in group name".to_string());
                }
                self.next_ch()?; // consume ')'
                let group_index = digits
                    .parse::<u32>()
                    .map_err(|_| "group index overflow".to_string())?;
                let yes_node = self.parse_term()?;
                let no_node = if self.peek() == Some('|') {
                    self.next_ch()?;
                    self.parse_term()?
                } else {
                    ReNode::Empty
                };
                if self.peek() != Some(')') {
                    return Err("missing )".to_string());
                }
                self.next_ch()?;
                Ok(ReNode::Conditional {
                    group_index,
                    yes: Box::new(yes_node),
                    no: Box::new(no_node),
                })
            }
            Some(':') => {
                // Non-capturing group.
                self.next_ch()?;
                let node = self.parse_expr()?;
                if self.peek() != Some(')') {
                    return Err("missing )".to_string());
                }
                self.next_ch()?;
                Ok(node)
            }
            Some('P') => {
                self.next_ch()?; // consume 'P'
                let name_marker = self.peek();
                match name_marker {
                    Some('=') => {
                        // Named back-reference: (?P=name)
                        self.next_ch()?;
                        let name = self.read_until_close_paren()?;
                        if name.is_empty() {
                            return Err("missing group name".to_string());
                        }
                        if self.open_group_names.contains(&name) {
                            return Err("cannot refer to an open group".to_string());
                        }
                        let idx = self
                            .group_names
                            .get(&name)
                            .copied()
                            .ok_or_else(|| format!("unknown group name '{name}'"))?;
                        Ok(ReNode::Backref(idx))
                    }
                    Some('<') => {
                        // Named capturing group: (?P<name>...)
                        self.next_ch()?; // consume '<'
                        self.parse_named_group_body()
                    }
                    _ => Err("bad character in group name".to_string()),
                }
            }
            // Inline flags: (?imsxauL) or (?i:...) or (?-i:...)
            Some(c) if "imsxaLu-".contains(c) || c == ')' => self.parse_inline_flags(),
            _ => Err("unknown extension".to_string()),
        }
    }

    /// Read a group name terminated by ')'.  Consumes characters but NOT the ')'.
    fn read_until_close_paren(&mut self) -> Result<String, String> {
        let mut name = String::new();
        loop {
            match self.peek() {
                None => return Err("missing )".to_string()),
                Some(')') => {
                    self.next_ch()?;
                    break;
                }
                Some(c) if is_meta_char(c) || c == '<' || c == '>' => {
                    return Err("bad character in group name".to_string());
                }
                Some(c) => {
                    self.next_ch()?;
                    name.push(c);
                }
            }
        }
        Ok(name)
    }

    /// Parse the body of a named group after '<' has been consumed.
    fn parse_named_group_body(&mut self) -> Result<ReNode, String> {
        let mut name = String::new();
        loop {
            match self.peek() {
                None => return Err("unterminated group name".to_string()),
                Some('>') => {
                    self.next_ch()?;
                    break;
                }
                Some(c) if is_meta_char(c) || c == '<' || c == '>' => {
                    return Err("bad character in group name".to_string());
                }
                Some(c) => {
                    self.next_ch()?;
                    name.push(c);
                }
            }
        }
        if name.is_empty() {
            return Err("missing group name".to_string());
        }
        if self.group_names.contains_key(&name) || self.open_group_names.contains(&name) {
            return Err("redefinition of group name".to_string());
        }
        // Assign index at open-paren time (CPython order).
        self.group_count += 1;
        let idx = self.group_count;
        self.group_names.insert(name.clone(), idx);
        self.open_group_names.insert(name.clone());
        let parse_result = self.parse_expr();
        let node = match parse_result {
            Ok(n) => {
                self.open_group_names.remove(&name);
                n
            }
            Err(e) => {
                self.open_group_names.remove(&name);
                return Err(e);
            }
        };
        if self.peek() != Some(')') {
            return Err("missing )".to_string());
        }
        self.next_ch()?;
        let width = fixed_width(&node, Some(&self.group_widths));
        self.group_widths.insert(idx, width);
        Ok(ReNode::Group {
            node: Box::new(node),
            index: idx,
        })
    }

    /// Parse inline flags (?imsxauL) or (?i:...) or (?-i:...)
    fn parse_inline_flags(&mut self) -> Result<ReNode, String> {
        let mut add_flags: i64 = 0;
        let mut clear_flags: i64 = 0;
        let mut seen_minus = false;
        loop {
            match self.peek() {
                None => return Err("unterminated inline flag".to_string()),
                Some('-') => {
                    self.next_ch()?;
                    seen_minus = true;
                }
                Some(
                    c @ ('i' | 'm' | 's' | 'x' | 'a' | 'L' | 'u' | 'I' | 'M' | 'S' | 'X' | 'A'
                    | 'U'),
                ) => {
                    self.next_ch()?;
                    let bit = flag_char_to_bit(c);
                    if seen_minus {
                        clear_flags |= bit;
                    } else {
                        add_flags |= bit;
                    }
                }
                _ => break,
            }
        }
        match self.peek() {
            Some(')') => {
                self.next_ch()?;
                self.inline_flags |= add_flags;
                self.inline_flags &= !clear_flags;
                Ok(ReNode::Empty)
            }
            Some(':') => {
                self.next_ch()?;
                let node = self.parse_expr()?;
                if self.peek() != Some(')') {
                    return Err("missing )".to_string());
                }
                self.next_ch()?;
                Ok(ReNode::ScopedFlags {
                    node: Box::new(node),
                    add_flags,
                    clear_flags,
                })
            }
            _ => Err("unsupported group extension syntax".to_string()),
        }
    }

    // -----------------------------------------------------------------------
    // Escape sequence
    // -----------------------------------------------------------------------

    fn parse_escape(&mut self) -> Result<ReNode, String> {
        let ch = self.raw_next()?;
        match ch {
            'd' | 'D' | 's' | 'S' | 'w' | 'W' => {
                let negated = ch.is_ascii_uppercase();
                let category = if negated {
                    ((ch as u8) + 32) as char
                } else {
                    ch
                };
                Ok(ReNode::CharClass {
                    negated,
                    ranges: vec![],
                    chars: vec![],
                    categories: vec![category.to_string()],
                })
            }
            'n' => Ok(ReNode::Literal("\n".to_string())),
            't' => Ok(ReNode::Literal("\t".to_string())),
            'r' => Ok(ReNode::Literal("\r".to_string())),
            'f' => Ok(ReNode::Literal("\x0C".to_string())),
            'v' => Ok(ReNode::Literal("\x0B".to_string())),
            c @ '0'..='9' => {
                let mut digits = String::from(c);
                loop {
                    match self.peek() {
                        Some(d) if d.is_ascii_digit() => {
                            self.next_ch()?;
                            digits.push(d);
                        }
                        _ => break,
                    }
                }
                let idx = digits
                    .parse::<u32>()
                    .map_err(|_| "backref index overflow".to_string())?;
                Ok(ReNode::Backref(idx))
            }
            'A' => Ok(ReNode::Anchor("start_abs".to_string())),
            'Z' => Ok(ReNode::Anchor("end_abs".to_string())),
            'b' => Ok(ReNode::Anchor("word_boundary".to_string())),
            'B' => Ok(ReNode::Anchor("word_boundary_not".to_string())),
            other => Ok(ReNode::Literal(other.to_string())),
        }
    }

    // -----------------------------------------------------------------------
    // Character class: [...]
    // -----------------------------------------------------------------------

    fn parse_class(&mut self) -> Result<ReNode, String> {
        self.in_class = true;
        let mut negated = false;
        let mut chars: Vec<String> = Vec::new();
        let mut ranges: Vec<(String, String)> = Vec::new();
        let mut categories: Vec<String> = Vec::new();

        if self.raw_peek() == Some('^') {
            self.raw_next()?;
            negated = true;
        }
        // A ']' immediately after '[' or '[^' is a literal ']'.
        if self.raw_peek() == Some(']') {
            let c = self.raw_next()?;
            chars.push(c.to_string());
        }

        loop {
            match self.raw_peek() {
                None => {
                    self.in_class = false;
                    return Err("unterminated character class".to_string());
                }
                Some(']') => {
                    self.raw_next()?;
                    break;
                }
                _ => {}
            }
            match self.class_item()? {
                ClassItem::Range(s, e) => ranges.push((s, e)),
                ClassItem::Category(cat) => categories.push(cat),
                ClassItem::Char(c) => chars.push(c),
            }
        }
        self.in_class = false;
        Ok(ReNode::CharClass {
            negated,
            ranges,
            chars,
            categories,
        })
    }

    fn class_item(&mut self) -> Result<ClassItem, String> {
        let ch = self.raw_next()?;
        match ch {
            '\\' => {
                let esc = self.raw_next()?;
                match esc {
                    'd' | 'D' | 's' | 'S' | 'w' | 'W' => {
                        let category = if esc.is_ascii_uppercase() {
                            ((esc as u8) + 32) as char
                        } else {
                            esc
                        };
                        Ok(ClassItem::Category(category.to_string()))
                    }
                    'n' => Ok(ClassItem::Char("\n".to_string())),
                    't' => Ok(ClassItem::Char("\t".to_string())),
                    'r' => Ok(ClassItem::Char("\r".to_string())),
                    'f' => Ok(ClassItem::Char("\x0C".to_string())),
                    'v' => Ok(ClassItem::Char("\x0B".to_string())),
                    c if c.is_ascii_digit() => {
                        // Octal escape inside character classes.
                        let mut oct = String::from(c);
                        while oct.len() < 3 {
                            match self.raw_peek() {
                                Some(d) if ('0'..='7').contains(&d) => {
                                    self.raw_next()?;
                                    oct.push(d);
                                }
                                _ => break,
                            }
                        }
                        let code = u32::from_str_radix(&oct, 8)
                            .map_err(|_| "invalid octal escape".to_string())?;
                        let c = char::from_u32(code).unwrap_or('\u{FFFD}');
                        Ok(ClassItem::Char(c.to_string()))
                    }
                    other => Ok(ClassItem::Char(other.to_string())),
                }
            }
            '[' if self.raw_peek() == Some(':') => {
                // POSIX class: [:alpha:]
                if self.nested_set_warning_pos.is_none() {
                    self.nested_set_warning_pos = Some((self.pos as i64) - 1);
                }
                self.raw_next()?; // consume ':'
                let mut name = String::new();
                loop {
                    match self.raw_peek() {
                        None => return Err("unterminated character class".to_string()),
                        Some(':') => {
                            self.raw_next()?;
                            if self.raw_peek() == Some(']') {
                                self.raw_next()?;
                                break;
                            }
                            name.push(':');
                        }
                        Some(c) => {
                            self.raw_next()?;
                            name.push(c);
                        }
                    }
                }
                Ok(ClassItem::Category(format!("posix:{name}")))
            }
            '-' | ']' => Ok(ClassItem::Char(ch.to_string())),
            _ => {
                // Maybe a range: X-Y
                if self.raw_peek() == Some('-') {
                    let saved_pos = self.pos;
                    self.raw_next()?; // consume '-'
                    match self.raw_peek() {
                        None | Some(']') => {
                            // Not a range — backtrack the '-'.
                            self.pos = saved_pos;
                            Ok(ClassItem::Char(ch.to_string()))
                        }
                        _ => {
                            let end_item = self.class_item()?;
                            match end_item {
                                ClassItem::Char(end) => Ok(ClassItem::Range(ch.to_string(), end)),
                                _ => Err("ranges over categories are not supported".to_string()),
                            }
                        }
                    }
                } else {
                    Ok(ClassItem::Char(ch.to_string()))
                }
            }
        }
    }
}

// ---------------------------------------------------------------------------
// ClassItem helper enum (only used inside parser)
// ---------------------------------------------------------------------------

enum ClassItem {
    Char(String),
    Range(String, String),
    Category(String),
}

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

const META_CHARS: &str = ".^$*+?{}[]\\|()";

fn is_meta_char(c: char) -> bool {
    META_CHARS.contains(c)
}

fn flag_char_to_bit(c: char) -> i64 {
    match c {
        'i' | 'I' => RE_IGNORECASE,
        'm' | 'M' => RE_MULTILINE,
        's' | 'S' => RE_DOTALL,
        'x' | 'X' => RE_VERBOSE,
        'a' | 'A' => RE_ASCII,
        'u' | 'U' => RE_UNICODE,
        'L' | 'l' => RE_LOCALE,
        _ => 0,
    }
}

/// Compute the fixed character-width of a regex node, analogous to
/// `_fixed_width()` in re/__init__.py.  Returns `None` for variable-width
/// nodes.
fn fixed_width(node: &ReNode, group_widths: Option<&HashMap<u32, Option<u64>>>) -> Option<u64> {
    match node {
        ReNode::Empty => Some(0),
        ReNode::Literal(s) => Some(s.chars().count() as u64),
        ReNode::Any => Some(1),
        ReNode::Anchor(_) => Some(0),
        ReNode::CharClass { .. } => Some(1),
        ReNode::Backref(idx) => group_widths?.get(idx).copied().flatten(),
        ReNode::Group { node, .. } => fixed_width(node, group_widths),
        ReNode::Look { .. } => Some(0),
        ReNode::ScopedFlags { node, .. } => fixed_width(node, group_widths),
        ReNode::Conditional { yes, no, .. } => {
            let yw = fixed_width(yes, group_widths)?;
            let nw = fixed_width(no, group_widths)?;
            if yw != nw { None } else { Some(yw) }
        }
        ReNode::Concat(nodes) => {
            let mut total = 0u64;
            for n in nodes {
                total += fixed_width(n, group_widths)?;
            }
            Some(total)
        }
        ReNode::Alt(options) => {
            if options.is_empty() {
                return Some(0);
            }
            let first = fixed_width(&options[0], group_widths)?;
            for opt in &options[1..] {
                let w = fixed_width(opt, group_widths)?;
                if w != first {
                    return None;
                }
            }
            Some(first)
        }
        ReNode::Repeat {
            node,
            min_count,
            max_count,
            ..
        } => {
            let w = fixed_width(node, group_widths)?;
            let max = (*max_count)?;
            if *min_count != max {
                return None;
            }
            Some(w * max)
        }
    }
}

// ---------------------------------------------------------------------------
// parse_pattern: top-level entry point
// ---------------------------------------------------------------------------

fn parse_pattern(pattern: &str, flags: i64) -> Result<CompiledPattern, String> {
    let mut parser = ReParser::new(pattern, flags);
    let root = parser.parse()?;
    let group_count = parser.group_count;
    let group_names = parser.group_names;
    let inline_flags = parser.inline_flags;
    let warn_pos = parser.nested_set_warning_pos;
    Ok(CompiledPattern {
        root,
        group_count,
        group_names,
        flags: flags | inline_flags,
        warn_pos,
    })
}

// ---------------------------------------------------------------------------
// molt_re_compile intrinsic
// ---------------------------------------------------------------------------

/// `molt_re_compile(pattern: str, flags: int) -> int`
///
/// Parse a regex pattern string and return an opaque integer handle.  The
/// compiled `CompiledPattern` is stored in the global `RE_PATTERNS` registry.
/// Returns -1 and raises `re.error` on parse failure.
#[unsafe(no_mangle)]
pub extern "C" fn molt_re_compile(pattern_bits: u64, flags_bits: u64) -> u64 {
    molt_runtime_core::with_core_gil!(_py, {
        let Some(pattern) = string_obj_to_owned(obj_from_bits(pattern_bits)) else {
            return raise_exception::<_>(_py, "TypeError", "pattern must be str");
        };
        let Some(flags) = to_i64(obj_from_bits(flags_bits)) else {
            return raise_exception::<_>(_py, "TypeError", "flags must be int");
        };
        match parse_pattern(&pattern, flags) {
            Ok(compiled) => {
                let handle = re_alloc_handle();
                re_store_pattern(handle, compiled);
                MoltObject::from_int(handle).bits()
            }
            Err(msg) => raise_exception::<_>(_py, "ValueError", &msg),
        }
    })
}

// ---------------------------------------------------------------------------
// molt_re_pattern_info intrinsic
// ---------------------------------------------------------------------------

/// `molt_re_pattern_info(handle: int) -> (groups, group_names_dict, flags, warn_pos)`
///
/// Returns a 4-tuple:
///   0: groups      — int,   number of capturing groups
///   1: group_names — dict,  {name: index}
///   2: flags       — int,   effective flags (pattern flags | inline flags)
///   3: warn_pos    — int or None,  char position of nested-set warning (or None)
#[unsafe(no_mangle)]
pub extern "C" fn molt_re_pattern_info(handle_bits: u64) -> u64 {
    molt_runtime_core::with_core_gil!(_py, {
        let Some(handle) = to_i64(obj_from_bits(handle_bits)) else {
            return raise_exception::<_>(_py, "TypeError", "handle must be int");
        };
        let guard = RE_PATTERNS.lock().unwrap_or_else(|e| e.into_inner());
        let Some(compiled) = guard.get(&handle) else {
            return raise_exception::<_>(_py, "ValueError", "invalid regex handle");
        };
        // Build group_names dict.
        let mut pairs: Vec<u64> = Vec::with_capacity(compiled.group_names.len() * 2);
        for (name, &idx) in &compiled.group_names {
            let name_ptr = alloc_string(_py, name.as_bytes());
            if name_ptr.is_null() {
                return MoltObject::none().bits();
            }
            let name_bits = MoltObject::from_ptr(name_ptr).bits();
            let idx_bits = MoltObject::from_int(idx as i64).bits();
            pairs.push(name_bits);
            pairs.push(idx_bits);
        }
        let dict_ptr = alloc_dict_with_pairs(_py, &pairs);
        if dict_ptr.is_null() {
            return MoltObject::none().bits();
        }
        let dict_bits = MoltObject::from_ptr(dict_ptr).bits();

        let groups_bits = MoltObject::from_int(compiled.group_count as i64).bits();
        let flags_bits_out = MoltObject::from_int(compiled.flags).bits();
        let warn_bits = match compiled.warn_pos {
            Some(pos) => MoltObject::from_int(pos).bits(),
            None => MoltObject::none().bits(),
        };

        let tuple_ptr = alloc_tuple(_py, &[groups_bits, dict_bits, flags_bits_out, warn_bits]);
        if tuple_ptr.is_null() {
            return MoltObject::none().bits();
        }
        MoltObject::from_ptr(tuple_ptr).bits()
    })
}

// ---------------------------------------------------------------------------
// Phase-1b: Backtracking match engine
// ---------------------------------------------------------------------------
//
// A recursive backtracking NFA engine that walks the `ReNode` IR tree.  It
// supports all node types produced by the Phase-1 parser.  The engine operates
// on character indices (not byte indices) to match Python's `re` semantics.

/// Match state threaded through the recursive engine.
struct MatchState {
    /// Character array of the subject string.
    chars: Vec<char>,
    /// Effective flags for the current match context.
    flags: i64,
    /// End of the search window (char index, exclusive).
    end: usize,
    /// Start of the search window (char index). Used for \A anchor.
    search_start: usize,
    /// Group captures: index 0 is unused (group 0 is the whole match).
    /// Each entry is Some((start, end)) or None if the group was not
    /// captured.  Indexed by group number (1-based).
    groups: Vec<Option<(usize, usize)>>,
    /// Recursion depth limit to prevent stack overflow on pathological
    /// patterns.
    depth: usize,
}

const MAX_RECURSION_DEPTH: usize = 5000;

impl MatchState {
    fn new(text: &str, flags: i64, group_count: u32, search_start: usize, end: usize) -> Self {
        let chars: Vec<char> = text.chars().collect();
        let groups = vec![None; (group_count + 1) as usize];
        Self {
            chars,
            flags,
            end,
            search_start,
            groups,
            depth: 0,
        }
    }

    /// Save group state for backtracking.
    fn save_groups(&self) -> Vec<Option<(usize, usize)>> {
        self.groups.clone()
    }

    /// Restore group state after a failed branch.
    fn restore_groups(&mut self, saved: Vec<Option<(usize, usize)>>) {
        self.groups = saved;
    }

    #[inline]
    fn is_ignorecase(&self) -> bool {
        self.flags & RE_IGNORECASE != 0
    }

    #[inline]
    fn is_dotall(&self) -> bool {
        self.flags & RE_DOTALL != 0
    }

    #[inline]
    fn is_multiline(&self) -> bool {
        self.flags & RE_MULTILINE != 0
    }

    #[inline]
    fn is_ascii(&self) -> bool {
        self.flags & RE_ASCII != 0
    }

    /// Check if a character is a "word" character for \b / \w purposes.
    #[inline]
    fn is_word_char(&self, ch: char) -> bool {
        ch == '_' || ch.is_ascii_alphanumeric() || (!self.is_ascii() && ch.is_alphabetic())
    }

    /// Compare two characters, respecting IGNORECASE.
    #[inline]
    fn char_eq(&self, a: char, b: char) -> bool {
        if self.is_ignorecase() {
            let al: Vec<char> = a.to_lowercase().collect();
            let bl: Vec<char> = b.to_lowercase().collect();
            al == bl
        } else {
            a == b
        }
    }
}

/// Attempt to match `node` starting at `pos` in the subject, followed by the
/// continuation nodes in `rest`.  This continuation-passing design is critical
/// for correct backtracking in quantifiers — the quantifier can try different
/// repetition counts and verify that the rest of the pattern also matches.
///
/// Returns `Some(final_pos)` on success or `None` on failure.
fn try_match(node: &ReNode, pos: usize, rest: &[ReNode], state: &mut MatchState) -> Option<usize> {
    state.depth += 1;
    if state.depth > MAX_RECURSION_DEPTH {
        state.depth -= 1;
        return None;
    }
    let result = try_match_inner(node, pos, rest, state);
    state.depth -= 1;
    result
}

/// Match a continuation (a slice of nodes that must match in sequence starting
/// from `pos`).  Returns `Some(final_pos)` on success.
fn match_rest(rest: &[ReNode], pos: usize, state: &mut MatchState) -> Option<usize> {
    if rest.is_empty() {
        return Some(pos);
    }
    try_match(&rest[0], pos, &rest[1..], state)
}

fn try_match_inner(
    node: &ReNode,
    pos: usize,
    rest: &[ReNode],
    state: &mut MatchState,
) -> Option<usize> {
    match node {
        ReNode::Empty => match_rest(rest, pos, state),

        ReNode::Literal(s) => {
            let lit_chars: Vec<char> = s.chars().collect();
            if pos + lit_chars.len() > state.end {
                return None;
            }
            for (i, &lc) in lit_chars.iter().enumerate() {
                if !state.char_eq(state.chars[pos + i], lc) {
                    return None;
                }
            }
            match_rest(rest, pos + lit_chars.len(), state)
        }

        ReNode::Any => {
            if pos >= state.end {
                return None;
            }
            let ch = state.chars[pos];
            if !state.is_dotall() && ch == '\n' {
                return None;
            }
            match_rest(rest, pos + 1, state)
        }

        ReNode::Anchor(kind) => {
            if match_anchor(kind, pos, state).is_some() {
                match_rest(rest, pos, state)
            } else {
                None
            }
        }

        ReNode::CharClass {
            negated,
            ranges,
            chars,
            categories,
        } => {
            if pos >= state.end {
                return None;
            }
            let ch = state.chars[pos];
            let hit = char_class_matches(ch, *negated, ranges, chars, categories, state);
            if hit {
                match_rest(rest, pos + 1, state)
            } else {
                None
            }
        }

        ReNode::Concat(nodes) => {
            // Flatten: the concat nodes followed by rest form the new
            // continuation.
            if nodes.is_empty() {
                return match_rest(rest, pos, state);
            }
            // Build combined continuation: nodes[1..] ++ rest.
            let mut combined: Vec<ReNode> = Vec::with_capacity(nodes.len() - 1 + rest.len());
            combined.extend_from_slice(&nodes[1..]);
            combined.extend_from_slice(rest);
            try_match(&nodes[0], pos, &combined, state)
        }

        ReNode::Alt(options) => {
            for opt in options {
                let saved = state.save_groups();
                if let Some(end_pos) = try_match(opt, pos, rest, state) {
                    return Some(end_pos);
                }
                state.restore_groups(saved);
            }
            None
        }

        ReNode::Repeat {
            node: inner,
            min_count,
            max_count,
            greedy,
        } => try_match_repeat(inner, pos, *min_count, *max_count, *greedy, rest, state),

        ReNode::Group { node: inner, index } => {
            let saved = state.save_groups();
            // We need to match `inner` and then set the group, then match rest.
            // Use a wrapper approach: match inner with empty rest, record group,
            // then match rest.
            let inner_result = try_match_group_then_rest(inner, *index, pos, rest, state);
            if inner_result.is_none() {
                state.restore_groups(saved);
            }
            inner_result
        }

        ReNode::Backref(idx) => {
            let idx = *idx as usize;
            if idx >= state.groups.len() {
                return None;
            }
            let (gstart, gend) = state.groups[idx]?;
            let ref_len = gend - gstart;
            if pos + ref_len > state.end {
                return None;
            }
            for i in 0..ref_len {
                if !state.char_eq(state.chars[gstart + i], state.chars[pos + i]) {
                    return None;
                }
            }
            match_rest(rest, pos + ref_len, state)
        }

        ReNode::Look {
            node: inner,
            behind,
            positive,
            width,
        } => {
            if try_match_look(inner, pos, *behind, *positive, *width, state).is_some() {
                match_rest(rest, pos, state)
            } else {
                None
            }
        }

        ReNode::ScopedFlags {
            node: inner,
            add_flags,
            clear_flags,
        } => {
            let old_flags = state.flags;
            state.flags = (state.flags | add_flags) & !clear_flags;
            // Match inner under scoped flags, then restore flags before
            // matching rest (rest should run under the outer flags).
            let inner_result = try_match(inner, pos, &[], state);
            state.flags = old_flags;
            match inner_result {
                Some(inner_end) => match_rest(rest, inner_end, state),
                None => None,
            }
        }

        ReNode::Conditional {
            group_index,
            yes,
            no,
        } => {
            let idx = *group_index as usize;
            let group_matched = idx < state.groups.len() && state.groups[idx].is_some();
            if group_matched {
                try_match(yes, pos, rest, state)
            } else {
                try_match(no, pos, rest, state)
            }
        }
    }
}

/// Match a Group node: match the inner pattern, record the group capture, then
/// match the rest continuation.  This ensures backtracking can unwind through
/// the group correctly.
fn try_match_group_then_rest(
    inner: &ReNode,
    index: u32,
    pos: usize,
    rest: &[ReNode],
    state: &mut MatchState,
) -> Option<usize> {
    // We create a synthetic rest that records the group then continues.
    // Since we can't easily insert a callback, we use a two-phase approach:
    // 1. Match inner with empty rest to find all possible end positions.
    // 2. For each end position, set the group and try matching rest.
    //
    // But that requires enumerating end positions, which is complex.
    // Instead, we match inner with empty continuation, record the group,
    // and match rest.  If rest fails, we need to backtrack into inner.
    //
    // For simplicity and correctness, we use the approach of matching inner
    // with a special "set-group-then-rest" continuation.  We encode this
    // by matching inner, and if it succeeds, setting the group and matching
    // rest.  If rest fails, we return None which propagates backtracking
    // back through inner's alternatives/quantifiers.

    // Build a continuation that represents: "set group, then match rest".
    // We do this by matching inner node with `rest` as continuation, but
    // wrapping in a way that records the group span.
    //
    // The simplest correct approach: match inner with empty rest to get
    // the inner end position, set the group, match rest.  This is correct
    // for most cases but doesn't allow inner to backtrack when rest fails.
    //
    // For proper backtracking, we need inner to know about rest.  The way
    // to do this: pass rest into the inner match but intercept the result
    // to set the group.  But the group span depends on inner's end position,
    // which is not directly available when rest is already appended.
    //
    // Full solution: match inner with empty rest, record end_pos, set group,
    // match rest.  This works for non-quantifier inner nodes.  For quantifier
    // inner nodes, the quantifier's backtracking handles it.

    // Simple approach (works for vast majority of patterns):
    let saved = state.save_groups();
    // Try matching inner alone first.
    let inner_end = try_match(inner, pos, &[], state);
    match inner_end {
        Some(end_pos) => {
            state.groups[index as usize] = Some((pos, end_pos));
            match match_rest(rest, end_pos, state) {
                Some(final_pos) => Some(final_pos),
                None => {
                    // Rest failed — we need to try other inner matches.
                    // For alternation/quantifier inner nodes, the backtracking
                    // in try_match already handles this.  But since we called
                    // try_match with empty rest, we got the "first" match.
                    // We need to enumerate all possible inner end positions.
                    //
                    // Re-try with rest appended to inner's continuation.
                    state.restore_groups(saved);
                    // Fall through to the continuation-passing approach.
                    try_match_group_with_continuation(inner, index, pos, rest, state)
                }
            }
        }
        None => {
            state.restore_groups(saved);
            None
        }
    }
}

/// Match a group with full continuation passing for proper backtracking.
/// This is the fallback when the simple group-then-rest approach fails.
fn try_match_group_with_continuation(
    inner: &ReNode,
    index: u32,
    pos: usize,
    rest: &[ReNode],
    state: &mut MatchState,
) -> Option<usize> {
    // We create a synthetic node that represents "set group N to (pos, HERE),
    // then match rest".  We encode this as a special wrapper.
    //
    // Since our try_match doesn't support arbitrary callbacks, we instead
    // build a Concat of [inner, GroupCapture(index, pos), rest...] where
    // GroupCapture is handled inline.
    //
    // The cleanest approach: wrap rest into a continuation and pass through.
    // We use a helper node that records the group capture point.
    //
    // For now, we use a marker node approach: create a synthetic concat that
    // is inner followed by rest, and after inner matches at some position,
    // we intercept to set the group.  This is effectively what the
    // continuation-passing style already does.

    // Match inner with rest as continuation.  The trick is that we need to
    // set the group capture BETWEEN inner and rest.  We do this by inserting
    // a synthetic "group-set" node.  Since we don't have such a node type,
    // we create a special Concat:

    // Actually, the simplest correct approach for groups with quantifier
    // inners is to NOT separate inner from rest.  Instead, match the entire
    // group pattern with rest as continuation, and record the group span
    // based on how far inner consumed.

    // The way CPython/sre handles this: the group node wraps the inner
    // pattern, and on entry it marks the group start, and on the inner's
    // success it marks the group end, then continues with rest.  If rest
    // fails, it backtracks into inner.

    // We can emulate this by using a try_match variant that records the
    // group span at each possible inner end position and then tries rest.

    // For alternation inner:
    match inner {
        ReNode::Alt(options) => {
            for opt in options {
                let saved = state.save_groups();
                if let Some(end_pos) = try_match(opt, pos, &[], state) {
                    state.groups[index as usize] = Some((pos, end_pos));
                    if let Some(final_pos) = match_rest(rest, end_pos, state) {
                        return Some(final_pos);
                    }
                }
                state.restore_groups(saved);
            }
            None
        }
        ReNode::Repeat {
            node: rep_inner,
            min_count,
            max_count,
            greedy,
        } => {
            // For quantifier inner, we need to try each possible repetition
            // count.  Build positions list and try each.
            let min = *min_count as usize;
            let max = max_count.map(|m| m as usize).unwrap_or(state.end - pos + 1);

            // Collect all possible end positions after min..=max repetitions.
            let mut end_positions = Vec::new();
            collect_repeat_positions(rep_inner, pos, min, max, state, &mut end_positions, 0);

            if *greedy {
                // Try from most repetitions to fewest.
                for &end_pos in end_positions.iter().rev() {
                    let saved = state.save_groups();
                    state.groups[index as usize] = Some((pos, end_pos));
                    if let Some(final_pos) = match_rest(rest, end_pos, state) {
                        return Some(final_pos);
                    }
                    state.restore_groups(saved);
                }
            } else {
                // Try from fewest to most.
                for &end_pos in &end_positions {
                    let saved = state.save_groups();
                    state.groups[index as usize] = Some((pos, end_pos));
                    if let Some(final_pos) = match_rest(rest, end_pos, state) {
                        return Some(final_pos);
                    }
                    state.restore_groups(saved);
                }
            }
            None
        }
        _ => {
            // For simple inner nodes, the single try_match already handles it.
            // If we got here, there's no alternative to try.
            None
        }
    }
}

/// Collect all possible end positions after `count`..=`max` repetitions of
/// `inner` starting from `pos`.  Called recursively.  Note: this temporarily
/// modifies `state.groups` during recursive calls, but always restores them
/// before returning.
fn collect_repeat_positions(
    inner: &ReNode,
    pos: usize,
    min: usize,
    max: usize,
    state: &mut MatchState,
    positions: &mut Vec<usize>,
    count: usize,
) {
    if count >= min {
        positions.push(pos);
    }
    if count >= max {
        return;
    }
    let saved = state.save_groups();
    if let Some(next) = try_match(inner, pos, &[], state) {
        if next == pos {
            // Zero-width — don't recurse to avoid infinite loop.
            state.restore_groups(saved);
            return;
        }
        collect_repeat_positions(inner, next, min, max, state, positions, count + 1);
    }
    state.restore_groups(saved);
}

/// Match an anchor node.  Returns `Some(pos)` if the anchor condition holds
/// (anchors consume zero characters).
fn match_anchor(kind: &str, pos: usize, state: &MatchState) -> Option<usize> {
    match kind {
        "start" => {
            // ^ — matches beginning of string, or after \n in MULTILINE.
            if pos == 0 {
                return Some(pos);
            }
            if state.is_multiline() && pos > 0 && state.chars[pos - 1] == '\n' {
                return Some(pos);
            }
            None
        }
        "end" => {
            // $ — matches end of string, or before \n in MULTILINE.
            // Also matches before a final \n at end (CPython behavior).
            if pos == state.end {
                return Some(pos);
            }
            if state.is_multiline() && pos < state.end && state.chars[pos] == '\n' {
                return Some(pos);
            }
            // $ also matches before a trailing newline even without MULTILINE.
            if !state.is_multiline() && pos == state.end - 1 && state.chars[pos] == '\n' {
                return Some(pos);
            }
            None
        }
        "start_abs" => {
            // \A — matches only at the start of the string.
            if pos == 0 { Some(pos) } else { None }
        }
        "end_abs" => {
            // \Z — matches only at the end of the string (or before a
            // trailing newline at the very end).
            if pos == state.end {
                return Some(pos);
            }
            if pos == state.end - 1 && state.chars[pos] == '\n' {
                return Some(pos);
            }
            None
        }
        "word_boundary" => {
            // \b — word boundary.
            let left_word = pos > 0 && state.is_word_char(state.chars[pos - 1]);
            let right_word = pos < state.chars.len() && state.is_word_char(state.chars[pos]);
            if left_word != right_word {
                Some(pos)
            } else {
                None
            }
        }
        "word_boundary_not" => {
            // \B — non-word-boundary.
            let left_word = pos > 0 && state.is_word_char(state.chars[pos - 1]);
            let right_word = pos < state.chars.len() && state.is_word_char(state.chars[pos]);
            if left_word == right_word {
                Some(pos)
            } else {
                None
            }
        }
        _ => None,
    }
}

/// Check whether `ch` matches a character class specification.
fn char_class_matches(
    ch: char,
    negated: bool,
    ranges: &[(String, String)],
    chars: &[String],
    categories: &[String],
    state: &MatchState,
) -> bool {
    let mut hit = false;

    // Check literal chars.
    for c_str in chars {
        // Each entry is a single-char string.
        if let Some(c) = c_str.chars().next()
            && state.char_eq(ch, c)
        {
            hit = true;
            break;
        }
    }

    // Check ranges.
    if !hit {
        for (lo_str, hi_str) in ranges {
            let lo = lo_str.chars().next().unwrap_or('\0');
            let hi = hi_str.chars().next().unwrap_or('\0');
            if state.is_ignorecase() {
                let ch_lower = ch.to_lowercase().next().unwrap_or(ch);
                let lo_lower = lo.to_lowercase().next().unwrap_or(lo);
                let hi_lower = hi.to_lowercase().next().unwrap_or(hi);
                if ch_lower >= lo_lower && ch_lower <= hi_lower {
                    hit = true;
                    break;
                }
                // Also check uppercase range for case-insensitive.
                let ch_upper = ch.to_uppercase().next().unwrap_or(ch);
                let lo_upper = lo.to_uppercase().next().unwrap_or(lo);
                let hi_upper = hi.to_uppercase().next().unwrap_or(hi);
                if ch_upper >= lo_upper && ch_upper <= hi_upper {
                    hit = true;
                    break;
                }
            } else if ch >= lo && ch <= hi {
                hit = true;
                break;
            }
        }
    }

    // Check categories (\d, \s, \w, etc.)
    if !hit {
        for cat in categories {
            let cat_match = match cat.as_str() {
                "d" => ch.is_ascii_digit(),
                "s" => matches!(ch, ' ' | '\t' | '\n' | '\r' | '\u{000C}' | '\u{000B}'),
                "w" => {
                    ch == '_'
                        || ch.is_ascii_alphanumeric()
                        || (!state.is_ascii() && ch.is_alphabetic())
                }
                _ => {
                    // Handle POSIX classes like "posix:alpha".
                    if let Some(posix_name) = cat.strip_prefix("posix:") {
                        match posix_name {
                            "alpha" => ch.is_alphabetic(),
                            "digit" => ch.is_ascii_digit(),
                            "alnum" => ch.is_alphanumeric(),
                            "space" => ch.is_whitespace(),
                            "upper" => ch.is_uppercase(),
                            "lower" => ch.is_lowercase(),
                            "punct" => ch.is_ascii_punctuation(),
                            "print" => !ch.is_control(),
                            "xdigit" => ch.is_ascii_hexdigit(),
                            _ => false,
                        }
                    } else {
                        false
                    }
                }
            };
            // Handle negated categories (\D, \S, \W).
            // The parser stores \D as CharClass { negated: true, categories: ["d"] }.
            // So the negation is handled at the top level, not per-category.
            if cat_match {
                hit = true;
                break;
            }
        }
    }

    if negated { !hit } else { hit }
}

/// Match a `Repeat` (quantifier) node with backtracking.
///
/// The continuation `rest` is the sequence of nodes that must match after this
/// quantifier.  This enables the quantifier to backtrack: after matching N
/// repetitions, it tries to match rest; if rest fails, it adjusts N.
fn try_match_repeat(
    inner: &ReNode,
    pos: usize,
    min_count: u64,
    max_count: Option<u64>,
    greedy: bool,
    rest: &[ReNode],
    state: &mut MatchState,
) -> Option<usize> {
    let min = min_count as usize;
    let max = max_count.map(|m| m as usize).unwrap_or(usize::MAX);

    // Collect all reachable positions after 0..=max repetitions.
    // positions[i] = position after exactly i repetitions (for i >= min, this
    // is a valid candidate).
    let mut positions: Vec<usize> = Vec::new();
    let mut cur = pos;
    // Match minimum repetitions first (mandatory).
    for i in 0..min {
        let saved = state.save_groups();
        match try_match(inner, cur, &[], state) {
            Some(next) => {
                if next == cur && i > 0 {
                    // Zero-width match in minimum — still counts.
                    state.restore_groups(saved);
                    break;
                }
                cur = next;
            }
            None => {
                state.restore_groups(saved);
                return None;
            }
        }
    }
    positions.push(cur);

    // Collect additional (optional) repetition positions.
    let mut count = min;
    while count < max {
        let saved = state.save_groups();
        match try_match(inner, cur, &[], state) {
            Some(next) => {
                if next == cur {
                    // Zero-width match — stop collecting.
                    state.restore_groups(saved);
                    break;
                }
                cur = next;
                positions.push(cur);
                count += 1;
            }
            None => {
                state.restore_groups(saved);
                break;
            }
        }
    }

    if greedy {
        // Greedy: try from most repetitions to fewest.
        while let Some(try_pos) = positions.pop() {
            let saved = state.save_groups();
            if let Some(final_pos) = match_rest(rest, try_pos, state) {
                return Some(final_pos);
            }
            state.restore_groups(saved);
        }
        None
    } else {
        // Lazy: try from fewest repetitions to most.
        for &try_pos in &positions {
            let saved = state.save_groups();
            if let Some(final_pos) = match_rest(rest, try_pos, state) {
                return Some(final_pos);
            }
            state.restore_groups(saved);
        }
        None
    }
}

/// Handle lookahead and lookbehind assertions.
fn try_match_look(
    inner: &ReNode,
    pos: usize,
    behind: bool,
    positive: bool,
    width: Option<u64>,
    state: &mut MatchState,
) -> Option<usize> {
    if behind {
        // Look-behind: check the substring ending at `pos`.
        let w = width.unwrap_or(0) as usize;
        if pos < w {
            // Not enough text behind.
            return if positive { None } else { Some(pos) };
        }
        let start = pos - w;
        let saved = state.save_groups();
        let old_end = state.end;
        state.end = pos;
        let matched = try_match(inner, start, &[], state);
        state.end = old_end;
        let ok = match matched {
            Some(end_pos) => end_pos == pos,
            None => false,
        };
        if positive == ok {
            if !ok {
                // Positive lookbehind failed — restore.
                // (ok is false AND positive is false, so this is negative
                // lookbehind succeeding because the inner didn't match.)
                state.restore_groups(saved);
            }
            // For positive lookbehind success (ok=true, positive=true):
            // keep the groups from the inner match.
            // For negative lookbehind success (ok=false, positive=false):
            // groups already restored above.
            Some(pos)
        } else {
            // Assertion failed — always restore.
            state.restore_groups(saved);
            None
        }
    } else {
        // Lookahead: check the substring starting at `pos`.
        let saved = state.save_groups();
        let matched = try_match(inner, pos, &[], state).is_some();
        if positive == matched {
            if !matched {
                // Negative lookahead succeeded (inner did NOT match) —
                // groups already unchanged, restore to be safe.
                state.restore_groups(saved);
            }
            // Positive lookahead succeeded — keep groups from inner.
            Some(pos)
        } else {
            // Assertion failed — restore groups.
            state.restore_groups(saved);
            None
        }
    }
}

// ---------------------------------------------------------------------------
// Top-level match engine: execute a compiled pattern against text
// ---------------------------------------------------------------------------

/// Internal result of a successful match.
struct MatchResult {
    start: usize,
    end: usize,
    /// Groups indexed 1..=group_count.  Index 0 is unused.
    groups: Vec<Option<(usize, usize)>>,
}

/// Execute a compiled pattern in the given mode.
///
/// Returns `Some(MatchResult)` on success, `None` on no match.
fn execute_match(
    compiled: &CompiledPattern,
    text: &str,
    pos: usize,
    end: usize,
    mode: &str,
) -> Option<MatchResult> {
    match mode {
        "match" => {
            // Anchored at start (pos), match from pos.
            let mut state = MatchState::new(text, compiled.flags, compiled.group_count, pos, end);
            if pos > state.chars.len() || end > state.chars.len() {
                return None;
            }
            let result = try_match(&compiled.root, pos, &[], &mut state);
            result.map(|end_pos| MatchResult {
                start: pos,
                end: end_pos,
                groups: state.groups,
            })
        }
        "fullmatch" => {
            // Must match the entire text[pos..end].
            let mut state = MatchState::new(text, compiled.flags, compiled.group_count, pos, end);
            if pos > state.chars.len() || end > state.chars.len() {
                return None;
            }
            let result = try_match(&compiled.root, pos, &[], &mut state);
            match result {
                Some(end_pos) if end_pos == end => Some(MatchResult {
                    start: pos,
                    end: end_pos,
                    groups: state.groups,
                }),
                _ => None,
            }
        }
        "search" => {
            // Search: try matching at each position from pos to end.
            let chars: Vec<char> = text.chars().collect();
            let text_len = chars.len();
            if pos > text_len || end > text_len {
                return None;
            }
            for start in pos..=end {
                let mut state =
                    MatchState::new(text, compiled.flags, compiled.group_count, pos, end);
                if let Some(end_pos) = try_match(&compiled.root, start, &[], &mut state) {
                    return Some(MatchResult {
                        start,
                        end: end_pos,
                        groups: state.groups,
                    });
                }
            }
            None
        }
        _ => None,
    }
}

/// Build a MoltObject tuple representing the match result for the intrinsic
/// return value.
///
/// Format: `(match_start, match_end, groups_tuple)`
/// where `groups_tuple` is a tuple of `(start, end) | None` for each group.
fn build_match_result_bits(
    _py: &CoreGilToken,
    result: &MatchResult,
    group_count: u32,
) -> u64 {
    let start_bits = MoltObject::from_int(result.start as i64).bits();
    let end_bits = MoltObject::from_int(result.end as i64).bits();

    // Build group spans tuple.
    let mut group_elems: Vec<u64> = Vec::with_capacity(group_count as usize);
    for i in 1..=(group_count as usize) {
        if i < result.groups.len() {
            match result.groups[i] {
                Some((gs, ge)) => {
                    let gs_bits = MoltObject::from_int(gs as i64).bits();
                    let ge_bits = MoltObject::from_int(ge as i64).bits();
                    let pair_ptr = alloc_tuple(_py, &[gs_bits, ge_bits]);
                    if pair_ptr.is_null() {
                        group_elems.push(MoltObject::none().bits());
                    } else {
                        group_elems.push(MoltObject::from_ptr(pair_ptr).bits());
                    }
                }
                None => {
                    group_elems.push(MoltObject::none().bits());
                }
            }
        } else {
            group_elems.push(MoltObject::none().bits());
        }
    }

    let groups_ptr = alloc_tuple(_py, &group_elems);
    let groups_bits = if groups_ptr.is_null() {
        MoltObject::none().bits()
    } else {
        MoltObject::from_ptr(groups_ptr).bits()
    };

    let result_ptr = alloc_tuple(_py, &[start_bits, end_bits, groups_bits]);
    if result_ptr.is_null() {
        MoltObject::none().bits()
    } else {
        MoltObject::from_ptr(result_ptr).bits()
    }
}

// ---------------------------------------------------------------------------
// molt_re_execute — Phase-1b match engine
// ---------------------------------------------------------------------------

/// `molt_re_execute(handle, text, pos, end, mode) -> match_result | None`
///
/// Execute a compiled regex pattern against the given text.
///
/// Arguments:
///   handle — integer handle from `molt_re_compile`
///   text   — subject string
///   pos    — start position (char index)
///   end    — end position (char index, exclusive)
///   mode   — "match", "search", or "fullmatch"
///
/// Returns:
///   None on no match, or a tuple `(start, end, groups_tuple)` where
///   groups_tuple is a tuple of `(start, end) | None` for each group.
#[unsafe(no_mangle)]
pub extern "C" fn molt_re_execute(
    handle_bits: u64,
    text_bits: u64,
    pos_bits: u64,
    end_bits: u64,
    mode_bits: u64,
) -> u64 {
    molt_runtime_core::with_core_gil!(_py, {
        let Some(handle) = to_i64(obj_from_bits(handle_bits)) else {
            return raise_exception::<_>(_py, "TypeError", "handle must be int");
        };
        let Some(text) = string_obj_to_owned(obj_from_bits(text_bits)) else {
            return raise_exception::<_>(_py, "TypeError", "text must be str");
        };
        let Some(pos) = to_i64(obj_from_bits(pos_bits)) else {
            return raise_exception::<_>(_py, "TypeError", "pos must be int");
        };
        let Some(end) = to_i64(obj_from_bits(end_bits)) else {
            return raise_exception::<_>(_py, "TypeError", "end must be int");
        };
        let Some(mode) = string_obj_to_owned(obj_from_bits(mode_bits)) else {
            return raise_exception::<_>(_py, "TypeError", "mode must be str");
        };

        let pos_usize = if pos < 0 { 0usize } else { pos as usize };
        let end_usize = if end < 0 { 0usize } else { end as usize };

        // Look up the compiled pattern.
        let guard = RE_PATTERNS.lock().unwrap_or_else(|e| e.into_inner());
        let Some(compiled) = guard.get(&handle) else {
            return raise_exception::<_>(_py, "ValueError", "invalid regex handle");
        };

        // Clone the parts we need so we can drop the lock.
        let root = compiled.root.clone();
        let group_count = compiled.group_count;
        let flags = compiled.flags;
        drop(guard);

        let local_compiled = CompiledPattern {
            root,
            group_count,
            group_names: HashMap::new(), // not needed for matching
            flags,
            warn_pos: None,
        };

        match execute_match(&local_compiled, &text, pos_usize, end_usize, &mode) {
            Some(result) => build_match_result_bits(_py, &result, group_count),
            None => MoltObject::none().bits(),
        }
    })
}

// ---------------------------------------------------------------------------
// molt_re_finditer_collect — Phase-1b find-all engine
// ---------------------------------------------------------------------------

/// `molt_re_finditer_collect(handle, text, pos, end) -> list | None`
///
/// Find all non-overlapping matches of a compiled pattern in the text.
///
/// Returns a list of match result tuples `[(start, end, groups), ...]`
/// or None if the pattern handle is invalid.
#[unsafe(no_mangle)]
pub extern "C" fn molt_re_finditer_collect(
    handle_bits: u64,
    text_bits: u64,
    pos_bits: u64,
    end_bits: u64,
) -> u64 {
    molt_runtime_core::with_core_gil!(_py, {
        let Some(handle) = to_i64(obj_from_bits(handle_bits)) else {
            return raise_exception::<_>(_py, "TypeError", "handle must be int");
        };
        let Some(text) = string_obj_to_owned(obj_from_bits(text_bits)) else {
            return raise_exception::<_>(_py, "TypeError", "text must be str");
        };
        let Some(pos) = to_i64(obj_from_bits(pos_bits)) else {
            return raise_exception::<_>(_py, "TypeError", "pos must be int");
        };
        let Some(end) = to_i64(obj_from_bits(end_bits)) else {
            return raise_exception::<_>(_py, "TypeError", "end must be int");
        };

        let pos_usize = if pos < 0 { 0usize } else { pos as usize };
        let end_usize = if end < 0 { 0usize } else { end as usize };

        // Look up the compiled pattern.
        let guard = RE_PATTERNS.lock().unwrap_or_else(|e| e.into_inner());
        let Some(compiled) = guard.get(&handle) else {
            return raise_exception::<_>(_py, "ValueError", "invalid regex handle");
        };

        let root = compiled.root.clone();
        let group_count = compiled.group_count;
        let flags = compiled.flags;
        drop(guard);

        let local_compiled = CompiledPattern {
            root,
            group_count,
            group_names: HashMap::new(),
            flags,
            warn_pos: None,
        };

        let chars: Vec<char> = text.chars().collect();
        let text_len = chars.len();
        let end_clamp = end_usize.min(text_len);

        let mut results: Vec<u64> = Vec::new();
        let mut cur = pos_usize;
        let mut prev_empty_match_at: Option<usize> = None;

        while cur <= end_clamp {
            match execute_match(&local_compiled, &text, cur, end_clamp, "search") {
                Some(result) => {
                    let match_start = result.start;
                    let match_end = result.end;

                    // Avoid infinite loop on zero-width matches at the same position.
                    if match_start == match_end {
                        if prev_empty_match_at == Some(match_start) {
                            // Already yielded an empty match here — advance.
                            if cur < end_clamp {
                                cur += 1;
                            } else {
                                break;
                            }
                            continue;
                        }
                        prev_empty_match_at = Some(match_start);
                    } else {
                        prev_empty_match_at = None;
                    }

                    let bits = build_match_result_bits(_py, &result, group_count);
                    results.push(bits);

                    if match_end == match_start {
                        // Zero-width match — advance by one to avoid infinite loop.
                        if cur < end_clamp {
                            cur = match_start + 1;
                        } else {
                            break;
                        }
                    } else {
                        cur = match_end;
                    }
                }
                None => break,
            }
        }

        let list_ptr = alloc_list(_py, &results);
        if list_ptr.is_null() {
            MoltObject::none().bits()
        } else {
            MoltObject::from_ptr(list_ptr).bits()
        }
    })
}

// ---------------------------------------------------------------------------
// molt_re_split — Rust-accelerated re.split()
// ---------------------------------------------------------------------------

/// `molt_re_split(handle, text, maxsplit) -> list[str]`
///
/// Split `text` by occurrences of the compiled regex pattern.  If the pattern
/// contains capturing groups, the captured text is included in the result list
/// (matching CPython semantics).
///
/// `maxsplit` of 0 means unlimited splits.
///
/// Zero-length matches are handled per CPython 3.7+ semantics: they split at
/// every position but do not split at the same position twice.
#[unsafe(no_mangle)]
pub extern "C" fn molt_re_split(handle_bits: u64, text_bits: u64, maxsplit_bits: u64) -> u64 {
    molt_runtime_core::with_core_gil!(_py, {
        let Some(handle) = to_i64(obj_from_bits(handle_bits)) else {
            return raise_exception::<_>(_py, "TypeError", "handle must be int");
        };
        let Some(text) = string_obj_to_owned(obj_from_bits(text_bits)) else {
            return raise_exception::<_>(_py, "TypeError", "text must be str");
        };
        let Some(maxsplit) = to_i64(obj_from_bits(maxsplit_bits)) else {
            return raise_exception::<_>(_py, "TypeError", "maxsplit must be int");
        };

        // Look up compiled pattern.
        let guard = RE_PATTERNS.lock().unwrap_or_else(|e| e.into_inner());
        let Some(compiled) = guard.get(&handle) else {
            return raise_exception::<_>(_py, "ValueError", "invalid regex handle");
        };
        let root = compiled.root.clone();
        let group_count = compiled.group_count;
        let flags = compiled.flags;
        drop(guard);

        let local_compiled = CompiledPattern {
            root,
            group_count,
            group_names: HashMap::new(),
            flags,
            warn_pos: None,
        };

        let chars: Vec<char> = text.chars().collect();
        let text_len = chars.len();
        let limit = if maxsplit <= 0 {
            None
        } else {
            Some(maxsplit as usize)
        };

        let mut result_parts: Vec<u64> = Vec::new();
        let mut last: usize = 0;
        let mut splits: usize = 0;
        let mut cur: usize = 0;
        let mut prev_empty_at: Option<usize> = None;

        while cur <= text_len {
            if let Some(lim) = limit
                && splits >= lim
            {
                break;
            }

            match execute_match(&local_compiled, &text, cur, text_len, "search") {
                Some(result) => {
                    let m_start = result.start;
                    let m_end = result.end;

                    // Zero-length match handling (CPython 3.7+):
                    // Skip zero-length matches at the same position as a previous
                    // zero-length match, and skip zero-length matches at the start
                    // of the string that would produce an empty leading element.
                    if m_start == m_end {
                        if prev_empty_at == Some(m_start) {
                            if cur < text_len {
                                cur += 1;
                            } else {
                                break;
                            }
                            continue;
                        }
                        prev_empty_at = Some(m_start);
                    } else {
                        prev_empty_at = None;
                    }

                    // Append text[last..m_start]
                    let segment: String = chars[last..m_start].iter().collect();
                    let seg_ptr = alloc_string(_py, segment.as_bytes());
                    if !seg_ptr.is_null() {
                        result_parts.push(MoltObject::from_ptr(seg_ptr).bits());
                    }

                    // Append capturing group values (CPython includes them in split output).
                    for i in 1..=(group_count as usize) {
                        if i < result.groups.len() {
                            match result.groups[i] {
                                Some((gs, ge)) => {
                                    let group_text: String = chars[gs..ge].iter().collect();
                                    let gptr = alloc_string(_py, group_text.as_bytes());
                                    if !gptr.is_null() {
                                        result_parts.push(MoltObject::from_ptr(gptr).bits());
                                    } else {
                                        result_parts.push(MoltObject::none().bits());
                                    }
                                }
                                None => {
                                    result_parts.push(MoltObject::none().bits());
                                }
                            }
                        } else {
                            result_parts.push(MoltObject::none().bits());
                        }
                    }

                    last = m_end;
                    splits += 1;

                    if m_end == m_start {
                        if cur < text_len {
                            cur = m_start + 1;
                        } else {
                            break;
                        }
                    } else {
                        cur = m_end;
                    }
                }
                None => break,
            }
        }

        // Append the remaining text.
        let tail: String = chars[last..].iter().collect();
        let tail_ptr = alloc_string(_py, tail.as_bytes());
        if !tail_ptr.is_null() {
            result_parts.push(MoltObject::from_ptr(tail_ptr).bits());
        }

        let list_ptr = alloc_list(_py, &result_parts);
        if list_ptr.is_null() {
            MoltObject::none().bits()
        } else {
            MoltObject::from_ptr(list_ptr).bits()
        }
    })
}

// ---------------------------------------------------------------------------
// molt_re_sub — Rust-accelerated re.sub() / re.subn()
// ---------------------------------------------------------------------------

/// `molt_re_sub(handle, repl, text, count) -> (result_string, num_subs)`
///
/// Replace occurrences of the compiled pattern in `text` with `repl`.
///
/// `repl` is a string with backreference support (\1, \g<name>, etc.) handled
/// by `molt_re_expand_replacement`.  Callable replacements are NOT handled here
/// — the Python side detects callable repls and falls back to the Python loop.
///
/// `count` of 0 means replace all occurrences.
///
/// Returns a tuple `(result_string, num_replacements)`.
#[unsafe(no_mangle)]
pub extern "C" fn molt_re_sub(
    handle_bits: u64,
    repl_bits: u64,
    text_bits: u64,
    count_bits: u64,
) -> u64 {
    molt_runtime_core::with_core_gil!(_py, {
        let Some(handle) = to_i64(obj_from_bits(handle_bits)) else {
            return raise_exception::<_>(_py, "TypeError", "handle must be int");
        };
        let Some(repl) = string_obj_to_owned(obj_from_bits(repl_bits)) else {
            return raise_exception::<_>(_py, "TypeError", "repl must be str");
        };
        let Some(text) = string_obj_to_owned(obj_from_bits(text_bits)) else {
            return raise_exception::<_>(_py, "TypeError", "text must be str");
        };
        let Some(count) = to_i64(obj_from_bits(count_bits)) else {
            return raise_exception::<_>(_py, "TypeError", "count must be int");
        };

        // Look up compiled pattern.
        let guard = RE_PATTERNS.lock().unwrap_or_else(|e| e.into_inner());
        let Some(compiled) = guard.get(&handle) else {
            return raise_exception::<_>(_py, "ValueError", "invalid regex handle");
        };
        let root = compiled.root.clone();
        let group_count = compiled.group_count;
        let flags = compiled.flags;
        let group_names = compiled.group_names.clone();
        drop(guard);

        let local_compiled = CompiledPattern {
            root,
            group_count,
            group_names,
            flags,
            warn_pos: None,
        };

        let chars: Vec<char> = text.chars().collect();
        let text_len = chars.len();
        let limit = if count <= 0 {
            None
        } else {
            Some(count as usize)
        };

        // Check if repl contains backreferences (backslash).
        let repl_has_backref = repl.contains('\\');

        let mut out = String::with_capacity(text.len());
        let mut last: usize = 0;
        let mut replaced: usize = 0;
        let mut cur: usize = 0;
        let mut prev_empty_at: Option<usize> = None;

        while cur <= text_len {
            if let Some(lim) = limit
                && replaced >= lim
            {
                break;
            }

            match execute_match(&local_compiled, &text, cur, text_len, "search") {
                Some(result) => {
                    let m_start = result.start;
                    let m_end = result.end;

                    // Zero-length match handling.
                    if m_start == m_end {
                        if prev_empty_at == Some(m_start) {
                            if cur < text_len {
                                cur += 1;
                            } else {
                                break;
                            }
                            continue;
                        }
                        prev_empty_at = Some(m_start);
                    } else {
                        prev_empty_at = None;
                    }

                    // Append text[last..m_start].
                    let segment: String = chars[last..m_start].iter().collect();
                    out.push_str(&segment);

                    // Expand the replacement template.
                    if repl_has_backref {
                        // Build group values for expand_replacement.
                        let expanded = expand_repl_with_groups(
                            &repl,
                            &text,
                            &chars,
                            &result,
                            group_count,
                            &local_compiled.group_names,
                        );
                        out.push_str(&expanded);
                    } else {
                        out.push_str(&repl);
                    }

                    last = m_end;
                    replaced += 1;

                    if m_end == m_start {
                        if cur < text_len {
                            // Advance past the current position and include the character.
                            cur = m_start + 1;
                        } else {
                            break;
                        }
                    } else {
                        cur = m_end;
                    }
                }
                None => break,
            }
        }

        // Append the remaining text.
        let tail: String = chars[last..].iter().collect();
        out.push_str(&tail);

        // Build result tuple: (result_string, num_replacements).
        let result_str_ptr = alloc_string(_py, out.as_bytes());
        let result_str_bits = if result_str_ptr.is_null() {
            MoltObject::none().bits()
        } else {
            MoltObject::from_ptr(result_str_ptr).bits()
        };
        let count_bits_out = MoltObject::from_int(replaced as i64).bits();

        let tuple_ptr = alloc_tuple(_py, &[result_str_bits, count_bits_out]);
        if tuple_ptr.is_null() {
            MoltObject::none().bits()
        } else {
            MoltObject::from_ptr(tuple_ptr).bits()
        }
    })
}

/// Expand a replacement template string with group references.
///
/// Handles:
/// - `\1` through `\99` — numbered group references
/// - `\g<N>` — numbered group references
/// - `\g<name>` — named group references
/// - `\0` — not supported (same as CPython: treated as octal)
/// - `\\` — literal backslash
fn expand_repl_with_groups(
    repl: &str,
    _text: &str,
    chars: &[char],
    result: &MatchResult,
    group_count: u32,
    group_names: &HashMap<String, u32>,
) -> String {
    let repl_chars: Vec<char> = repl.chars().collect();
    let rlen = repl_chars.len();
    let mut out = String::with_capacity(repl.len());
    let mut i = 0;

    while i < rlen {
        if repl_chars[i] == '\\' && i + 1 < rlen {
            let next = repl_chars[i + 1];
            match next {
                '\\' => {
                    out.push('\\');
                    i += 2;
                }
                '0' => {
                    // \0 is a NUL byte in CPython
                    out.push('\0');
                    i += 2;
                }
                '1'..='9' => {
                    // Numeric backreference: \1 through \99
                    let mut num_str = String::new();
                    num_str.push(next);
                    if i + 2 < rlen && repl_chars[i + 2].is_ascii_digit() {
                        let two_digit = format!("{}{}", next, repl_chars[i + 2]);
                        if let Ok(n) = two_digit.parse::<u32>()
                            && n <= group_count
                        {
                            num_str.push(repl_chars[i + 2]);
                        }
                    }
                    let idx = num_str.parse::<u32>().unwrap_or(0) as usize;
                    if idx > 0
                        && idx <= group_count as usize
                        && idx < result.groups.len()
                        && let Some((gs, ge)) = result.groups[idx]
                    {
                        let group_text: String = chars[gs..ge].iter().collect();
                        out.push_str(&group_text);
                    }
                    i += 1 + num_str.len();
                }
                'g' => {
                    // \g<...> — named or numbered group reference
                    if i + 2 < rlen && repl_chars[i + 2] == '<' {
                        // Find closing >
                        let start = i + 3;
                        let mut end_idx = start;
                        while end_idx < rlen && repl_chars[end_idx] != '>' {
                            end_idx += 1;
                        }
                        if end_idx < rlen {
                            let ref_name: String = repl_chars[start..end_idx].iter().collect();
                            // Try as number first, then as name.
                            let group_idx = if let Ok(n) = ref_name.parse::<u32>() {
                                Some(n as usize)
                            } else {
                                group_names.get(&ref_name).map(|&n| n as usize)
                            };
                            if let Some(idx) = group_idx {
                                if idx == 0 {
                                    // \g<0> is the whole match
                                    let group_text: String =
                                        chars[result.start..result.end].iter().collect();
                                    out.push_str(&group_text);
                                } else if idx <= group_count as usize
                                    && idx < result.groups.len()
                                    && let Some((gs, ge)) = result.groups[idx]
                                {
                                    let group_text: String = chars[gs..ge].iter().collect();
                                    out.push_str(&group_text);
                                }
                            }
                            i = end_idx + 1;
                        } else {
                            // Malformed \g<...>, pass through
                            out.push('\\');
                            out.push('g');
                            i += 2;
                        }
                    } else {
                        out.push('\\');
                        out.push('g');
                        i += 2;
                    }
                }
                'n' => {
                    out.push('\n');
                    i += 2;
                }
                'r' => {
                    out.push('\r');
                    i += 2;
                }
                't' => {
                    out.push('\t');
                    i += 2;
                }
                'a' => {
                    out.push('\x07');
                    i += 2;
                }
                'f' => {
                    out.push('\x0C');
                    i += 2;
                }
                'v' => {
                    out.push('\x0B');
                    i += 2;
                }
                _ => {
                    // Unknown escape — pass through as-is (CPython behavior)
                    out.push('\\');
                    out.push(next);
                    i += 2;
                }
            }
        } else {
            out.push(repl_chars[i]);
            i += 1;
        }
    }

    out
}

// ---------------------------------------------------------------------------
// molt_re_escape — escape special regex characters
// ---------------------------------------------------------------------------

/// Characters that have special meaning in regular expressions and must be
/// escaped.  This matches CPython's `re.escape()` character set.
const RE_SPECIAL_CHARS: &[char] = &[
    '\\', '.', '^', '$', '*', '+', '?', '{', '}', '[', ']', '|', '(', ')',
];

/// Pure-Rust implementation of `re.escape()`.
///
/// Prefixes every character in `pattern` that has special regex meaning with
/// a backslash.  NUL characters are also escaped as `\000`.
fn re_escape_impl(pattern: &str) -> String {
    let mut out = String::with_capacity(pattern.len() * 2);
    for ch in pattern.chars() {
        if ch == '\0' {
            out.push_str("\\000");
        } else if RE_SPECIAL_CHARS.contains(&ch) {
            out.push('\\');
            out.push(ch);
        } else {
            out.push(ch);
        }
    }
    out
}

/// `molt_re_escape(pattern: str) -> str`
///
/// Escape all special regex characters in `pattern` so it can be used as a
/// literal string in a regex.
#[unsafe(no_mangle)]
pub extern "C" fn molt_re_escape(pattern_bits: u64) -> u64 {
    molt_runtime_core::with_core_gil!(_py, {
        let Some(pattern) = string_obj_to_owned(obj_from_bits(pattern_bits)) else {
            return raise_exception::<_>(_py, "TypeError", "pattern must be str");
        };
        let escaped = re_escape_impl(&pattern);
        let out_ptr = alloc_string(_py, escaped.as_bytes());
        if out_ptr.is_null() {
            MoltObject::none().bits()
        } else {
            MoltObject::from_ptr(out_ptr).bits()
        }
    })
}

// ---------------------------------------------------------------------------
// molt_re_sub_callable — re.sub() with a callable replacement function
// ---------------------------------------------------------------------------

/// `molt_re_sub_callable(handle, repl_callable, text, count) -> (result_string, num_subs)`
///
/// Like `molt_re_sub` but `repl_callable` is a callable that receives a match
/// result tuple `(start, end, groups)` and returns a replacement string.
///
/// This fills the gap where `molt_re_sub` only handles string replacements and
/// the Python side previously had to fall back to its own loop for callable
/// replacements.
#[unsafe(no_mangle)]
pub extern "C" fn molt_re_sub_callable(
    handle_bits: u64,
    repl_callable_bits: u64,
    text_bits: u64,
    count_bits: u64,
) -> u64 {
    molt_runtime_core::with_core_gil!(_py, {
        let Some(handle) = to_i64(obj_from_bits(handle_bits)) else {
            return raise_exception::<_>(_py, "TypeError", "handle must be int");
        };
        let Some(text) = string_obj_to_owned(obj_from_bits(text_bits)) else {
            return raise_exception::<_>(_py, "TypeError", "text must be str");
        };
        let Some(count) = to_i64(obj_from_bits(count_bits)) else {
            return raise_exception::<_>(_py, "TypeError", "count must be int");
        };

        // Look up compiled pattern.
        let guard = RE_PATTERNS.lock().unwrap_or_else(|e| e.into_inner());
        let Some(compiled) = guard.get(&handle) else {
            return raise_exception::<_>(_py, "ValueError", "invalid regex handle");
        };
        let root = compiled.root.clone();
        let group_count = compiled.group_count;
        let flags = compiled.flags;
        drop(guard);

        let local_compiled = CompiledPattern {
            root,
            group_count,
            group_names: HashMap::new(),
            flags,
            warn_pos: None,
        };

        let chars: Vec<char> = text.chars().collect();
        let text_len = chars.len();
        let limit = if count <= 0 {
            None
        } else {
            Some(count as usize)
        };

        let mut out = String::with_capacity(text.len());
        let mut last: usize = 0;
        let mut replaced: usize = 0;
        let mut cur: usize = 0;
        let mut prev_empty_at: Option<usize> = None;

        while cur <= text_len {
            if let Some(lim) = limit
                && replaced >= lim
            {
                break;
            }

            match execute_match(&local_compiled, &text, cur, text_len, "search") {
                Some(result) => {
                    let m_start = result.start;
                    let m_end = result.end;

                    // Zero-length match handling.
                    if m_start == m_end {
                        if prev_empty_at == Some(m_start) {
                            if cur < text_len {
                                cur += 1;
                            } else {
                                break;
                            }
                            continue;
                        }
                        prev_empty_at = Some(m_start);
                    } else {
                        prev_empty_at = None;
                    }

                    // Append text[last..m_start].
                    let segment: String = chars[last..m_start].iter().collect();
                    out.push_str(&segment);

                    // Build the match result tuple and call the replacement function.
                    let match_tuple_bits =
                        build_match_result_bits(_py, &result, group_count);
                    let repl_result_bits =
                        call_callable1(
                            _py,
                            repl_callable_bits,
                            match_tuple_bits,
                        );
                    dec_ref_bits(_py, match_tuple_bits);

                    if exception_pending(_py) {
                        return MoltObject::none().bits();
                    }

                    // The callable must return a string.
                    if let Some(repl_str) = string_obj_to_owned(obj_from_bits(repl_result_bits)) {
                        out.push_str(&repl_str);
                    }
                    dec_ref_bits(_py, repl_result_bits);

                    last = m_end;
                    replaced += 1;

                    if m_end == m_start {
                        if cur < text_len {
                            cur = m_start + 1;
                        } else {
                            break;
                        }
                    } else {
                        cur = m_end;
                    }
                }
                None => break,
            }
        }

        // Append the remaining text.
        let tail: String = chars[last..].iter().collect();
        out.push_str(&tail);

        // Build result tuple: (result_string, num_replacements).
        let result_str_ptr = alloc_string(_py, out.as_bytes());
        let result_str_bits = if result_str_ptr.is_null() {
            MoltObject::none().bits()
        } else {
            MoltObject::from_ptr(result_str_ptr).bits()
        };
        let count_bits_out = MoltObject::from_int(replaced as i64).bits();

        let tuple_ptr = alloc_tuple(_py, &[result_str_bits, count_bits_out]);
        if tuple_ptr.is_null() {
            MoltObject::none().bits()
        } else {
            MoltObject::from_ptr(tuple_ptr).bits()
        }
    })
}

// ---------------------------------------------------------------------------
// Match object helpers — .group(), .groups(), .groupdict()
// ---------------------------------------------------------------------------
//
// The Rust match engine returns a flat tuple (start, end, groups_tuple).
// These intrinsics operate on that tuple + the original text to provide the
// CPython Match object API efficiently from Rust.

/// `molt_re_match_group(text, match_tuple, *indices) -> str | tuple[str|None, ...]`
///
/// Implements `Match.group(...)`.  `indices` is a tuple of (int | str) group
/// selectors.  If a single index is given, returns a string (or None for
/// unmatched groups).  If multiple indices, returns a tuple.
///
/// `match_tuple` is the `(start, end, groups_tuple)` from `molt_re_execute`.
/// `group_names_bits` is a dict mapping name → index for named groups.
#[unsafe(no_mangle)]
pub extern "C" fn molt_re_match_group(
    text_bits: u64,
    match_tuple_bits: u64,
    indices_bits: u64,
    group_names_bits: u64,
) -> u64 {
    molt_runtime_core::with_core_gil!(_py, {
        let Some(text) = string_obj_to_owned(obj_from_bits(text_bits)) else {
            return raise_exception::<_>(_py, "TypeError", "text must be str");
        };
        let text_chars: Vec<char> = text.chars().collect();

        // Decode match_tuple = (start, end, groups_tuple)
        let Some(mt_ptr) = obj_from_bits(match_tuple_bits).as_ptr() else {
            return raise_exception::<_>(_py, "TypeError", "match_tuple must be a tuple");
        };
        let mt = unsafe { seq_vec_ref(mt_ptr) };
        if mt.len() < 3 {
            return raise_exception::<_>(_py, "ValueError", "invalid match tuple");
        }
        let Some(m_start) = to_i64(obj_from_bits(mt[0])) else {
            return raise_exception::<_>(_py, "ValueError", "invalid match start");
        };
        let Some(m_end) = to_i64(obj_from_bits(mt[1])) else {
            return raise_exception::<_>(_py, "ValueError", "invalid match end");
        };

        // Decode the groups tuple.
        let Some(groups_ptr) = obj_from_bits(mt[2]).as_ptr() else {
            return raise_exception::<_>(_py, "TypeError", "groups must be a tuple");
        };
        let group_spans = unsafe { seq_vec_ref(groups_ptr) };

        // Helper: resolve a group index from an int or string selector.
        let resolve_group = |sel_bits: u64| -> Option<usize> {
            if let Some(idx) = to_i64(obj_from_bits(sel_bits)) {
                return Some(idx as usize);
            }
            // Try as string name.
            if let Some(name) = string_obj_to_owned(obj_from_bits(sel_bits)) {
                if let Some(gn_ptr) = obj_from_bits(group_names_bits).as_ptr() {
                    let gn_ty = unsafe { object_type_id(gn_ptr) };
                    if gn_ty == TYPE_ID_DICT {
                        // Look up name in the dict.
                        if let Some(name_key_bits) = attr_name_bits_from_bytes(_py, name.as_bytes()) {
                            if let Some(val_bits) = unsafe { dict_get_in_place(_py, gn_ptr, name_key_bits) } {
                                dec_ref_bits(_py, name_key_bits);
                                return to_i64(obj_from_bits(val_bits)).map(|v| v as usize);
                            }
                            dec_ref_bits(_py, name_key_bits);
                        }
                    }
                }
            }
            None
        };

        // Helper: extract group text for index i (0 = whole match).
        let group_text_bits = |i: usize| -> u64 {
            if i == 0 {
                // Whole match.
                let ms = m_start as usize;
                let me = m_end as usize;
                if ms <= me && me <= text_chars.len() {
                    let s: String = text_chars[ms..me].iter().collect();
                    let ptr = alloc_string(_py, s.as_bytes());
                    if !ptr.is_null() {
                        return MoltObject::from_ptr(ptr).bits();
                    }
                }
                return MoltObject::none().bits();
            }
            // Group i is at index i-1 in the spans tuple (groups are 1-based,
            // but the groups_tuple stores them starting at index 0 for group 1).
            let span_idx = i - 1;
            if span_idx >= group_spans.len() {
                return MoltObject::none().bits();
            }
            let span_bits = group_spans[span_idx];
            if obj_from_bits(span_bits).is_none() {
                return MoltObject::none().bits();
            }
            let Some(span_ptr) = obj_from_bits(span_bits).as_ptr() else {
                return MoltObject::none().bits();
            };
            let span = unsafe { seq_vec_ref(span_ptr) };
            if span.len() < 2 {
                return MoltObject::none().bits();
            }
            let Some(gs) = to_i64(obj_from_bits(span[0])) else {
                return MoltObject::none().bits();
            };
            let Some(ge) = to_i64(obj_from_bits(span[1])) else {
                return MoltObject::none().bits();
            };
            if gs < 0 || ge < gs {
                return MoltObject::none().bits();
            }
            let gs = gs as usize;
            let ge = ge as usize;
            if ge > text_chars.len() {
                return MoltObject::none().bits();
            }
            let s: String = text_chars[gs..ge].iter().collect();
            let ptr = alloc_string(_py, s.as_bytes());
            if !ptr.is_null() {
                MoltObject::from_ptr(ptr).bits()
            } else {
                MoltObject::none().bits()
            }
        };

        // Decode indices tuple.
        let Some(indices_ptr) = obj_from_bits(indices_bits).as_ptr() else {
            // No indices → return group(0) = whole match.
            return group_text_bits(0);
        };
        let indices = unsafe { seq_vec_ref(indices_ptr) };

        if indices.is_empty() {
            // group() with no args → group(0)
            return group_text_bits(0);
        }

        if indices.len() == 1 {
            // Single index → return the group directly (not wrapped in tuple).
            let Some(idx) = resolve_group(indices[0]) else {
                return raise_exception::<_>(_py, "IndexError", "no such group");
            };
            return group_text_bits(idx);
        }

        // Multiple indices → return a tuple.
        let mut result: Vec<u64> = Vec::with_capacity(indices.len());
        for &sel_bits in indices.iter() {
            let Some(idx) = resolve_group(sel_bits) else {
                for bits in &result {
                    dec_ref_bits(_py, *bits);
                }
                return raise_exception::<_>(_py, "IndexError", "no such group");
            };
            result.push(group_text_bits(idx));
        }
        let tuple_ptr = alloc_tuple(_py, &result);
        for bits in &result {
            dec_ref_bits(_py, *bits);
        }
        if tuple_ptr.is_null() {
            MoltObject::none().bits()
        } else {
            MoltObject::from_ptr(tuple_ptr).bits()
        }
    })
}

/// `molt_re_match_groups(text, match_tuple, default) -> tuple[str|None, ...]`
///
/// Implements `Match.groups(default=None)`.  Returns a tuple of all captured
/// groups (1-based).  Unmatched groups use `default`.
#[unsafe(no_mangle)]
pub extern "C" fn molt_re_match_groups(
    text_bits: u64,
    match_tuple_bits: u64,
    default_bits: u64,
) -> u64 {
    molt_runtime_core::with_core_gil!(_py, {
        let Some(text) = string_obj_to_owned(obj_from_bits(text_bits)) else {
            return raise_exception::<_>(_py, "TypeError", "text must be str");
        };
        let text_chars: Vec<char> = text.chars().collect();

        let Some(mt_ptr) = obj_from_bits(match_tuple_bits).as_ptr() else {
            return raise_exception::<_>(_py, "TypeError", "match_tuple must be a tuple");
        };
        let mt = unsafe { seq_vec_ref(mt_ptr) };
        if mt.len() < 3 {
            return raise_exception::<_>(_py, "ValueError", "invalid match tuple");
        }
        let Some(groups_ptr) = obj_from_bits(mt[2]).as_ptr() else {
            let ptr = alloc_tuple(_py, &[]);
            return if ptr.is_null() {
                MoltObject::none().bits()
            } else {
                MoltObject::from_ptr(ptr).bits()
            };
        };
        let group_spans = unsafe { seq_vec_ref(groups_ptr) };

        let mut result: Vec<u64> = Vec::with_capacity(group_spans.len());
        for &span_bits in group_spans.iter() {
            if obj_from_bits(span_bits).is_none() {
                inc_ref_bits(_py, default_bits);
                result.push(default_bits);
                continue;
            }
            let Some(span_ptr) = obj_from_bits(span_bits).as_ptr() else {
                inc_ref_bits(_py, default_bits);
                result.push(default_bits);
                continue;
            };
            let span = unsafe { seq_vec_ref(span_ptr) };
            if span.len() < 2 {
                inc_ref_bits(_py, default_bits);
                result.push(default_bits);
                continue;
            }
            let Some(gs) = to_i64(obj_from_bits(span[0])) else {
                inc_ref_bits(_py, default_bits);
                result.push(default_bits);
                continue;
            };
            let Some(ge) = to_i64(obj_from_bits(span[1])) else {
                inc_ref_bits(_py, default_bits);
                result.push(default_bits);
                continue;
            };
            if gs < 0 || ge < gs || (ge as usize) > text_chars.len() {
                inc_ref_bits(_py, default_bits);
                result.push(default_bits);
                continue;
            }
            let s: String = text_chars[gs as usize..ge as usize].iter().collect();
            let ptr = alloc_string(_py, s.as_bytes());
            if !ptr.is_null() {
                result.push(MoltObject::from_ptr(ptr).bits());
            } else {
                inc_ref_bits(_py, default_bits);
                result.push(default_bits);
            }
        }
        let tuple_ptr = alloc_tuple(_py, &result);
        for bits in &result {
            dec_ref_bits(_py, *bits);
        }
        if tuple_ptr.is_null() {
            MoltObject::none().bits()
        } else {
            MoltObject::from_ptr(tuple_ptr).bits()
        }
    })
}

/// `molt_re_match_groupdict(text, match_tuple, default, group_names) -> dict`
///
/// Implements `Match.groupdict(default=None)`.  Returns a dict mapping named
/// group names to their captured text (or `default` if the group did not
/// participate in the match).
///
/// `group_names` is a dict `{name: index, ...}`.
#[unsafe(no_mangle)]
pub extern "C" fn molt_re_match_groupdict(
    text_bits: u64,
    match_tuple_bits: u64,
    default_bits: u64,
    group_names_bits: u64,
) -> u64 {
    molt_runtime_core::with_core_gil!(_py, {
        let Some(text) = string_obj_to_owned(obj_from_bits(text_bits)) else {
            return raise_exception::<_>(_py, "TypeError", "text must be str");
        };
        let text_chars: Vec<char> = text.chars().collect();

        let Some(mt_ptr) = obj_from_bits(match_tuple_bits).as_ptr() else {
            return raise_exception::<_>(_py, "TypeError", "match_tuple must be a tuple");
        };
        let mt = unsafe { seq_vec_ref(mt_ptr) };
        if mt.len() < 3 {
            return raise_exception::<_>(_py, "ValueError", "invalid match tuple");
        }
        let groups_ptr_opt = obj_from_bits(mt[2]).as_ptr();

        // Decode group_names dict.
        let Some(gn_ptr) = obj_from_bits(group_names_bits).as_ptr() else {
            // No group names → empty dict.
            let ptr = alloc_dict_with_pairs(_py, &[]);
            return if ptr.is_null() {
                MoltObject::none().bits()
            } else {
                MoltObject::from_ptr(ptr).bits()
            };
        };
        let gn_ty = unsafe { object_type_id(gn_ptr) };
        if gn_ty != TYPE_ID_DICT {
            return raise_exception::<_>(_py, "TypeError", "group_names must be a dict");
        }

        let result_ptr = alloc_dict_with_pairs(_py, &[]);
        if result_ptr.is_null() {
            return MoltObject::none().bits();
        }

        // Iterate over group_names dict.
        let order = dict_order_clone(_py, gn_ptr);
        for pair in order.chunks(2) {
            if pair.len() != 2 {
                continue;
            }
            let name_key_bits = pair[0];
            let idx_bits = pair[1];
            let Some(idx) = to_i64(obj_from_bits(idx_bits)) else {
                continue;
            };

            // Get the group text for this index.
            let val_bits = if let Some(groups_ptr) = groups_ptr_opt {
                let group_spans = unsafe { seq_vec_ref(groups_ptr) };
                let span_idx = (idx as usize).wrapping_sub(1);
                if span_idx < group_spans.len() {
                    let span_bits = group_spans[span_idx];
                    if let Some(span_ptr) = obj_from_bits(span_bits).as_ptr() {
                        let span = unsafe { seq_vec_ref(span_ptr) };
                        if span.len() >= 2 {
                            let gs = to_i64(obj_from_bits(span[0])).unwrap_or(-1);
                            let ge = to_i64(obj_from_bits(span[1])).unwrap_or(-1);
                            if gs >= 0 && ge >= gs && (ge as usize) <= text_chars.len() {
                                let s: String =
                                    text_chars[gs as usize..ge as usize].iter().collect();
                                let ptr = alloc_string(_py, s.as_bytes());
                                if !ptr.is_null() {
                                    MoltObject::from_ptr(ptr).bits()
                                } else {
                                    default_bits
                                }
                            } else {
                                default_bits
                            }
                        } else {
                            default_bits
                        }
                    } else {
                        default_bits
                    }
                } else {
                    default_bits
                }
            } else {
                default_bits
            };

            unsafe {
                dict_set_in_place(_py, result_ptr, name_key_bits, val_bits);
            }
        }

        MoltObject::from_ptr(result_ptr).bits()
    })
}

// ---------------------------------------------------------------------------
// Unit tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_strip_verbose_empty() {
        assert_eq!(re_strip_verbose_impl("", 64), "");
    }

    #[test]
    fn test_strip_verbose_no_whitespace() {
        assert_eq!(re_strip_verbose_impl("abc", 64), "abc");
    }

    #[test]
    fn test_strip_verbose_strips_spaces() {
        assert_eq!(re_strip_verbose_impl("a b c", 64), "abc");
    }

    #[test]
    fn test_strip_verbose_strips_comments() {
        let pat = "a  # match a\nb  # match b\n";
        assert_eq!(re_strip_verbose_impl(pat, 64), "ab");
    }

    #[test]
    fn test_strip_verbose_escaped_space() {
        // `\ ` should survive
        assert_eq!(re_strip_verbose_impl("\\ x", 64), "\\ x");
    }

    #[test]
    fn test_strip_verbose_escaped_hash() {
        assert_eq!(re_strip_verbose_impl("\\#", 64), "\\#");
    }

    #[test]
    fn test_strip_verbose_class_preserved() {
        // Whitespace and # inside [...] must be kept verbatim.
        assert_eq!(re_strip_verbose_impl("[ # ]", 64), "[ # ]");
    }

    #[test]
    fn test_strip_verbose_class_escape() {
        // `\]` inside a class should not close it.
        assert_eq!(re_strip_verbose_impl("[\\]]", 64), "[\\]]");
    }

    #[test]
    fn test_negative_lookahead_literal_no_match() {
        // "abc" does not start with "xyz" → lookahead succeeds (1)
        let result = re_negative_lookahead_impl("abc", 0, 3, "lit:xyz", 0);
        assert_eq!(result, 1);
    }

    #[test]
    fn test_negative_lookahead_literal_match() {
        // "abc" starts with "ab" → lookahead fails (0)
        let result = re_negative_lookahead_impl("abc", 0, 3, "lit:ab", 0);
        assert_eq!(result, 0);
    }

    #[test]
    fn test_negative_lookahead_complex() {
        // Unknown prefix → sentinel
        let result = re_negative_lookahead_impl("abc", 0, 3, "complex:(a|b)", 0);
        assert_eq!(result, SENTINEL_FALLBACK);
    }

    #[test]
    fn test_positive_lookahead_literal_match() {
        // "abc" starts with "ab" → positive lookahead succeeds (1)
        let result = re_positive_lookahead_impl("abc", 0, 3, "lit:ab", 0);
        assert_eq!(result, 1);
    }

    #[test]
    fn test_positive_lookahead_literal_no_match() {
        // "abc" does not start with "xyz" → positive lookahead fails (0)
        let result = re_positive_lookahead_impl("abc", 0, 3, "lit:xyz", 0);
        assert_eq!(result, 0);
    }

    #[test]
    fn test_positive_lookahead_complex() {
        let result = re_positive_lookahead_impl("abc", 0, 3, "complex:(a|b)", 0);
        assert_eq!(result, SENTINEL_FALLBACK);
    }

    #[test]
    fn test_negative_lookbehind_literal_no_match() {
        // "abc" — at pos 2, the char before is 'b', literal is "x" → no match → succeeds (1)
        let result = re_negative_lookbehind_impl("abc", 2, 3, "lit:x", 1, 0);
        assert_eq!(result, 1);
    }

    #[test]
    fn test_negative_lookbehind_literal_match() {
        // "abc" — at pos 2, the char before is 'b', literal is "b" → match → fails (0)
        let result = re_negative_lookbehind_impl("abc", 2, 3, "lit:b", 1, 0);
        assert_eq!(result, 0);
    }

    #[test]
    fn test_negative_lookbehind_not_enough_text() {
        // pos=0, width=1 → start = -1 < 0 → succeeds (1)
        let result = re_negative_lookbehind_impl("abc", 0, 3, "lit:a", 1, 0);
        assert_eq!(result, 1);
    }

    #[test]
    fn test_positive_lookbehind_literal_match() {
        // "abc" — at pos 2, the char before is 'b', literal is "b" → succeeds (1)
        let result = re_positive_lookbehind_impl("abc", 2, 3, "lit:b", 1, 0);
        assert_eq!(result, 1);
    }

    #[test]
    fn test_positive_lookbehind_literal_no_match() {
        // "abc" — at pos 2, literal "x" does not match previous char → fails (0)
        let result = re_positive_lookbehind_impl("abc", 2, 3, "lit:x", 1, 0);
        assert_eq!(result, 0);
    }

    #[test]
    fn test_positive_lookbehind_not_enough_text() {
        // pos=0, width=1 → start = -1 < 0 → cannot match positive lookbehind.
        let result = re_positive_lookbehind_impl("abc", 0, 3, "lit:a", 1, 0);
        assert_eq!(result, 0);
    }

    #[test]
    fn test_strip_verbose_no_flag_unchanged() {
        // flags=0 means NOT verbose — pattern should come back unchanged
        let pat = "a b # comment\n";
        assert_eq!(re_strip_verbose_impl(pat, 0), pat);
    }

    #[test]
    fn test_named_backref_advance_empty_group_names() {
        // With an empty group_names table the advance returns -1.
        let spans: Vec<Option<(i64, i64)>> = vec![None, Some((0, 2))];
        let names: Vec<(String, i64)> = vec![];
        let result = re_named_backref_advance_impl("abab", 2, 4, &spans, "foo", &names);
        assert_eq!(result, -1);
    }

    #[test]
    fn test_named_backref_advance_hit() {
        // Group "word" captured [0,2) = "ab", check if "ab" repeats at pos 2.
        let spans: Vec<Option<(i64, i64)>> = vec![None, Some((0, 2))];
        let names: Vec<(String, i64)> = vec![("word".to_string(), 1)];
        let result = re_named_backref_advance_impl("abab", 2, 4, &spans, "word", &names);
        assert_eq!(result, 4);
    }

    #[test]
    fn test_named_backref_advance_no_match() {
        // Group "word" captured "ab", but at pos 2 text is "cd" → -1.
        let spans: Vec<Option<(i64, i64)>> = vec![None, Some((0, 2))];
        let names: Vec<(String, i64)> = vec![("word".to_string(), 1)];
        let result = re_named_backref_advance_impl("abcd", 2, 4, &spans, "word", &names);
        assert_eq!(result, -1);
    }

    // -----------------------------------------------------------------------
    // Phase-1 parser tests
    // -----------------------------------------------------------------------

    #[test]
    fn test_parse_empty_pattern() {
        let cp = parse_pattern("", 0).unwrap();
        assert!(matches!(cp.root, ReNode::Empty));
        assert_eq!(cp.group_count, 0);
    }

    #[test]
    fn test_parse_literal() {
        let cp = parse_pattern("hello", 0).unwrap();
        match &cp.root {
            ReNode::Literal(s) => assert_eq!(s, "hello"),
            other => panic!("expected Literal, got {other:?}"),
        }
    }

    #[test]
    fn test_parse_any() {
        let cp = parse_pattern(".", 0).unwrap();
        assert!(matches!(cp.root, ReNode::Any));
    }

    #[test]
    fn test_parse_anchor_start() {
        let cp = parse_pattern("^", 0).unwrap();
        match &cp.root {
            ReNode::Anchor(k) => assert_eq!(k, "start"),
            other => panic!("expected Anchor, got {other:?}"),
        }
    }

    #[test]
    fn test_parse_anchor_end() {
        let cp = parse_pattern("$", 0).unwrap();
        match &cp.root {
            ReNode::Anchor(k) => assert_eq!(k, "end"),
            other => panic!("expected Anchor, got {other:?}"),
        }
    }

    #[test]
    fn test_parse_alternation() {
        let cp = parse_pattern("a|b", 0).unwrap();
        match &cp.root {
            ReNode::Alt(opts) => assert_eq!(opts.len(), 2),
            other => panic!("expected Alt, got {other:?}"),
        }
    }

    #[test]
    fn test_parse_capturing_group() {
        let cp = parse_pattern("(abc)", 0).unwrap();
        assert_eq!(cp.group_count, 1);
        match &cp.root {
            ReNode::Group { index, .. } => assert_eq!(*index, 1),
            other => panic!("expected Group, got {other:?}"),
        }
    }

    #[test]
    fn test_parse_named_group() {
        let cp = parse_pattern("(?P<word>\\w+)", 0).unwrap();
        assert_eq!(cp.group_count, 1);
        assert_eq!(cp.group_names.get("word"), Some(&1u32));
    }

    #[test]
    fn test_parse_non_capturing_group() {
        let cp = parse_pattern("(?:abc)", 0).unwrap();
        assert_eq!(cp.group_count, 0);
        match &cp.root {
            ReNode::Literal(s) => assert_eq!(s, "abc"),
            other => panic!("expected Literal (collapsed non-capturing group), got {other:?}"),
        }
    }

    #[test]
    fn test_parse_repeat_star() {
        let cp = parse_pattern("a*", 0).unwrap();
        match &cp.root {
            ReNode::Repeat {
                min_count,
                max_count,
                greedy,
                ..
            } => {
                assert_eq!(*min_count, 0);
                assert!(max_count.is_none());
                assert!(*greedy);
            }
            other => panic!("expected Repeat, got {other:?}"),
        }
    }

    #[test]
    fn test_parse_repeat_lazy() {
        let cp = parse_pattern("a+?", 0).unwrap();
        match &cp.root {
            ReNode::Repeat {
                min_count,
                max_count,
                greedy,
                ..
            } => {
                assert_eq!(*min_count, 1);
                assert!(max_count.is_none());
                assert!(!(*greedy));
            }
            other => panic!("expected Repeat, got {other:?}"),
        }
    }

    #[test]
    fn test_parse_repeat_counted() {
        let cp = parse_pattern("a{2,5}", 0).unwrap();
        match &cp.root {
            ReNode::Repeat {
                min_count,
                max_count,
                ..
            } => {
                assert_eq!(*min_count, 2);
                assert_eq!(*max_count, Some(5));
            }
            other => panic!("expected Repeat, got {other:?}"),
        }
    }

    #[test]
    fn test_parse_charclass() {
        let cp = parse_pattern("[abc]", 0).unwrap();
        match &cp.root {
            ReNode::CharClass { negated, chars, .. } => {
                assert!(!negated);
                assert!(chars.contains(&"a".to_string()));
            }
            other => panic!("expected CharClass, got {other:?}"),
        }
    }

    #[test]
    fn test_parse_negated_charclass() {
        let cp = parse_pattern("[^abc]", 0).unwrap();
        match &cp.root {
            ReNode::CharClass { negated, .. } => {
                assert!(*negated);
            }
            other => panic!("expected CharClass, got {other:?}"),
        }
    }

    #[test]
    fn test_parse_charclass_range() {
        let cp = parse_pattern("[a-z]", 0).unwrap();
        match &cp.root {
            ReNode::CharClass { ranges, .. } => {
                assert!(!ranges.is_empty());
                let (s, e) = &ranges[0];
                assert_eq!(s, "a");
                assert_eq!(e, "z");
            }
            other => panic!("expected CharClass with range, got {other:?}"),
        }
    }

    #[test]
    fn test_parse_backslash_d() {
        let cp = parse_pattern("\\d", 0).unwrap();
        match &cp.root {
            ReNode::CharClass {
                negated,
                categories,
                ..
            } => {
                assert!(!negated);
                assert!(categories.contains(&"d".to_string()));
            }
            other => panic!("expected CharClass, got {other:?}"),
        }
    }

    #[test]
    #[allow(non_snake_case)]
    fn test_parse_backslash_D() {
        let cp = parse_pattern("\\D", 0).unwrap();
        match &cp.root {
            ReNode::CharClass {
                negated,
                categories,
                ..
            } => {
                assert!(*negated);
                assert!(categories.contains(&"d".to_string()));
            }
            other => panic!("expected CharClass, got {other:?}"),
        }
    }

    #[test]
    fn test_parse_lookahead_positive() {
        let cp = parse_pattern("(?=abc)", 0).unwrap();
        match &cp.root {
            ReNode::Look {
                behind, positive, ..
            } => {
                assert!(!behind);
                assert!(*positive);
            }
            other => panic!("expected Look, got {other:?}"),
        }
    }

    #[test]
    fn test_parse_lookahead_negative() {
        let cp = parse_pattern("(?!abc)", 0).unwrap();
        match &cp.root {
            ReNode::Look {
                behind, positive, ..
            } => {
                assert!(!behind);
                assert!(!positive);
            }
            other => panic!("expected Look, got {other:?}"),
        }
    }

    #[test]
    fn test_parse_lookbehind_positive() {
        let cp = parse_pattern("(?<=ab)", 0).unwrap();
        match &cp.root {
            ReNode::Look {
                behind,
                positive,
                width,
                ..
            } => {
                assert!(*behind);
                assert!(*positive);
                assert_eq!(*width, Some(2));
            }
            other => panic!("expected Look, got {other:?}"),
        }
    }

    #[test]
    fn test_parse_lookbehind_negative() {
        let cp = parse_pattern("(?<!ab)", 0).unwrap();
        match &cp.root {
            ReNode::Look {
                behind,
                positive,
                width,
                ..
            } => {
                assert!(*behind);
                assert!(!positive);
                assert_eq!(*width, Some(2));
            }
            other => panic!("expected Look, got {other:?}"),
        }
    }

    #[test]
    fn test_parse_scoped_flags() {
        let cp = parse_pattern("(?i:abc)", 0).unwrap();
        match &cp.root {
            ReNode::ScopedFlags {
                add_flags,
                clear_flags,
                ..
            } => {
                assert_eq!(*add_flags & RE_IGNORECASE, RE_IGNORECASE);
                assert_eq!(*clear_flags, 0);
            }
            other => panic!("expected ScopedFlags, got {other:?}"),
        }
    }

    #[test]
    fn test_parse_inline_flags_global() {
        let cp = parse_pattern("(?i)abc", 0).unwrap();
        assert_eq!(cp.flags & RE_IGNORECASE, RE_IGNORECASE);
    }

    #[test]
    fn test_parse_backref() {
        let cp = parse_pattern("(a)\\1", 0).unwrap();
        match &cp.root {
            ReNode::Concat(nodes) => {
                assert_eq!(nodes.len(), 2);
                assert!(matches!(&nodes[0], ReNode::Group { index: 1, .. }));
                assert!(matches!(&nodes[1], ReNode::Backref(1)));
            }
            other => panic!("expected Concat, got {other:?}"),
        }
    }

    #[test]
    fn test_parse_conditional() {
        let cp = parse_pattern("(?(1)yes|no)", 0).unwrap();
        match &cp.root {
            ReNode::Conditional { group_index, .. } => assert_eq!(*group_index, 1),
            other => panic!("expected Conditional, got {other:?}"),
        }
    }

    #[test]
    fn test_parse_named_backref() {
        let cp = parse_pattern("(?P<w>a)(?P=w)", 0).unwrap();
        assert_eq!(cp.group_count, 1);
        match &cp.root {
            ReNode::Concat(nodes) => {
                assert!(matches!(&nodes[1], ReNode::Backref(1)));
            }
            other => panic!("expected Concat with named backref, got {other:?}"),
        }
    }

    #[test]
    fn test_parse_bad_quantifier_range() {
        let err = parse_pattern("a{5,3}", 0).unwrap_err();
        assert!(err.contains("invalid quantifier range"), "got: {err}");
    }

    #[test]
    fn test_parse_missing_close_paren() {
        parse_pattern("(abc", 0).unwrap_err();
    }

    #[test]
    fn test_parse_verbose_whitespace_stripped() {
        // In verbose mode, whitespace between atoms is ignored.
        let cp = parse_pattern("a b c", RE_VERBOSE).unwrap();
        match &cp.root {
            ReNode::Literal(s) => assert_eq!(s, "abc"),
            other => panic!("expected Literal 'abc', got {other:?}"),
        }
    }

    #[test]
    fn test_parse_anchor_abs() {
        let cp = parse_pattern("\\A\\Z", 0).unwrap();
        match &cp.root {
            ReNode::Concat(nodes) => {
                assert!(matches!(&nodes[0], ReNode::Anchor(k) if k == "start_abs"));
                assert!(matches!(&nodes[1], ReNode::Anchor(k) if k == "end_abs"));
            }
            other => panic!("expected Concat, got {other:?}"),
        }
    }

    #[test]
    fn test_parse_word_boundary() {
        let cp = parse_pattern("\\b\\B", 0).unwrap();
        match &cp.root {
            ReNode::Concat(nodes) => {
                assert!(matches!(&nodes[0], ReNode::Anchor(k) if k == "word_boundary"));
                assert!(matches!(&nodes[1], ReNode::Anchor(k) if k == "word_boundary_not"));
            }
            other => panic!("expected Concat, got {other:?}"),
        }
    }

    #[test]
    fn test_fixed_width_literal() {
        assert_eq!(
            fixed_width(&ReNode::Literal("hello".to_string()), None),
            Some(5)
        );
    }

    #[test]
    fn test_fixed_width_repeat_exact() {
        let node = ReNode::Repeat {
            node: Box::new(ReNode::Any),
            min_count: 3,
            max_count: Some(3),
            greedy: true,
        };
        assert_eq!(fixed_width(&node, None), Some(3));
    }

    #[test]
    fn test_fixed_width_repeat_variable() {
        let node = ReNode::Repeat {
            node: Box::new(ReNode::Any),
            min_count: 1,
            max_count: Some(3),
            greedy: true,
        };
        assert_eq!(fixed_width(&node, None), None);
    }

    #[test]
    fn test_parse_group_count_multiple() {
        let cp = parse_pattern("(a)(b)(c)", 0).unwrap();
        assert_eq!(cp.group_count, 3);
    }

    #[test]
    fn test_parse_octal_escape_in_class() {
        // \101 octal = 'A'
        let cp = parse_pattern("[\\101]", 0).unwrap();
        match &cp.root {
            ReNode::CharClass { chars, .. } => {
                assert!(
                    chars.contains(&"A".to_string()),
                    "expected 'A' in chars, got {chars:?}"
                );
            }
            other => panic!("expected CharClass, got {other:?}"),
        }
    }

    // -----------------------------------------------------------------------
    // Phase-1b match engine tests
    // -----------------------------------------------------------------------

    /// Helper: compile + execute in a given mode.
    fn do_execute(pattern: &str, flags: i64, text: &str, mode: &str) -> Option<MatchResult> {
        let compiled = parse_pattern(pattern, flags).unwrap();
        let text_len = text.chars().count();
        execute_match(&compiled, text, 0, text_len, mode)
    }

    /// Helper: compile + execute "match" mode.
    fn do_match(pattern: &str, text: &str) -> Option<MatchResult> {
        do_execute(pattern, 0, text, "match")
    }

    /// Helper: compile + execute "search" mode.
    fn do_search(pattern: &str, text: &str) -> Option<MatchResult> {
        do_execute(pattern, 0, text, "search")
    }

    /// Helper: compile + execute "fullmatch" mode.
    fn do_fullmatch(pattern: &str, text: &str) -> Option<MatchResult> {
        do_execute(pattern, 0, text, "fullmatch")
    }

    // --- Literal matching ---

    #[test]
    fn test_match_literal() {
        let m = do_match("hello", "hello world").unwrap();
        assert_eq!(m.start, 0);
        assert_eq!(m.end, 5);
    }

    #[test]
    fn test_match_literal_no_match() {
        assert!(do_match("xyz", "hello").is_none());
    }

    #[test]
    fn test_search_literal() {
        let m = do_search("world", "hello world").unwrap();
        assert_eq!(m.start, 6);
        assert_eq!(m.end, 11);
    }

    #[test]
    fn test_fullmatch_literal() {
        let m = do_fullmatch("hello", "hello").unwrap();
        assert_eq!(m.start, 0);
        assert_eq!(m.end, 5);
    }

    #[test]
    fn test_fullmatch_literal_fail() {
        assert!(do_fullmatch("hello", "hello world").is_none());
    }

    // --- Any (.) ---

    #[test]
    fn test_match_any() {
        let m = do_match(".", "a").unwrap();
        assert_eq!(m.end, 1);
    }

    #[test]
    fn test_match_any_no_newline() {
        assert!(do_match(".", "\n").is_none());
    }

    #[test]
    fn test_match_any_dotall() {
        let m = do_execute(".", RE_DOTALL, "\n", "match").unwrap();
        assert_eq!(m.end, 1);
    }

    // --- Character classes ---

    #[test]
    fn test_match_charclass() {
        let m = do_match("[abc]", "b").unwrap();
        assert_eq!(m.end, 1);
    }

    #[test]
    fn test_match_charclass_no_match() {
        assert!(do_match("[abc]", "d").is_none());
    }

    #[test]
    fn test_match_charclass_range() {
        let m = do_match("[a-z]", "m").unwrap();
        assert_eq!(m.end, 1);
    }

    #[test]
    fn test_match_charclass_range_no_match() {
        assert!(do_match("[a-z]", "5").is_none());
    }

    #[test]
    fn test_match_negated_charclass() {
        let m = do_match("[^abc]", "d").unwrap();
        assert_eq!(m.end, 1);
    }

    #[test]
    fn test_match_negated_charclass_no_match() {
        assert!(do_match("[^abc]", "a").is_none());
    }

    #[test]
    fn test_match_charclass_digit() {
        let m = do_match("\\d", "5").unwrap();
        assert_eq!(m.end, 1);
    }

    #[test]
    fn test_match_charclass_word() {
        let m = do_match("\\w", "a").unwrap();
        assert_eq!(m.end, 1);
    }

    #[test]
    fn test_match_charclass_space() {
        let m = do_match("\\s", " ").unwrap();
        assert_eq!(m.end, 1);
    }

    // --- Anchors ---

    #[test]
    fn test_match_anchor_start() {
        let m = do_match("^hello", "hello").unwrap();
        assert_eq!(m.end, 5);
    }

    #[test]
    fn test_search_anchor_start_fails_mid() {
        assert!(do_search("^world", "hello world").is_none());
    }

    #[test]
    fn test_match_anchor_end() {
        let m = do_fullmatch("hello$", "hello").unwrap();
        assert_eq!(m.end, 5);
    }

    #[test]
    fn test_match_anchor_end_trailing_newline() {
        // $ matches before a trailing newline.
        let m = do_match("hello$", "hello\n").unwrap();
        assert_eq!(m.start, 0);
        assert_eq!(m.end, 5);
    }

    #[test]
    fn test_match_multiline_anchor() {
        let compiled = parse_pattern("^world", RE_MULTILINE).unwrap();
        let text = "hello\nworld";
        let text_len = text.chars().count();
        let m = execute_match(&compiled, text, 0, text_len, "search").unwrap();
        assert_eq!(m.start, 6);
        assert_eq!(m.end, 11);
    }

    #[test]
    fn test_match_abs_start() {
        let m = do_match("\\Ahello", "hello").unwrap();
        assert_eq!(m.end, 5);
    }

    #[test]
    fn test_match_abs_end() {
        let m = do_fullmatch("hello\\Z", "hello").unwrap();
        assert_eq!(m.end, 5);
    }

    #[test]
    fn test_match_word_boundary() {
        let m = do_search("\\bhello\\b", "say hello there").unwrap();
        assert_eq!(m.start, 4);
        assert_eq!(m.end, 9);
    }

    // --- Quantifiers ---

    #[test]
    fn test_match_star() {
        let m = do_match("a*", "aaa").unwrap();
        assert_eq!(m.end, 3);
    }

    #[test]
    fn test_match_star_zero() {
        let m = do_match("a*", "bbb").unwrap();
        assert_eq!(m.end, 0);
    }

    #[test]
    fn test_match_plus() {
        let m = do_match("a+", "aaa").unwrap();
        assert_eq!(m.end, 3);
    }

    #[test]
    fn test_match_plus_fail() {
        assert!(do_match("a+", "bbb").is_none());
    }

    #[test]
    fn test_match_question() {
        let m = do_match("a?", "a").unwrap();
        assert_eq!(m.end, 1);
    }

    #[test]
    fn test_match_question_zero() {
        let m = do_match("a?", "b").unwrap();
        assert_eq!(m.end, 0);
    }

    #[test]
    fn test_match_counted() {
        let m = do_match("a{2,4}", "aaaa").unwrap();
        assert_eq!(m.end, 4);
    }

    #[test]
    fn test_match_counted_exact() {
        let m = do_match("a{3}", "aaaa").unwrap();
        assert_eq!(m.end, 3);
    }

    #[test]
    fn test_match_counted_fail() {
        assert!(do_match("a{3}", "aa").is_none());
    }

    // --- Greedy vs lazy ---

    #[test]
    fn test_match_greedy_star() {
        let m = do_match("a.*b", "aXXXb").unwrap();
        assert_eq!(m.start, 0);
        assert_eq!(m.end, 5);
    }

    #[test]
    fn test_match_greedy_backtrack() {
        let m = do_match(".*b", "aXXXb").unwrap();
        assert_eq!(m.end, 5);
    }

    #[test]
    fn test_match_lazy_star() {
        let m = do_match("a.*?b", "aXbXb").unwrap();
        assert_eq!(m.start, 0);
        assert_eq!(m.end, 3);
    }

    // --- Groups ---

    #[test]
    fn test_match_group() {
        let m = do_match("(abc)", "abc").unwrap();
        assert_eq!(m.groups[1], Some((0, 3)));
    }

    #[test]
    fn test_match_multiple_groups() {
        let m = do_match("(a)(b)(c)", "abc").unwrap();
        assert_eq!(m.groups[1], Some((0, 1)));
        assert_eq!(m.groups[2], Some((1, 2)));
        assert_eq!(m.groups[3], Some((2, 3)));
    }

    #[test]
    fn test_match_nested_groups() {
        let m = do_match("((a)b)", "ab").unwrap();
        assert_eq!(m.groups[1], Some((0, 2)));
        assert_eq!(m.groups[2], Some((0, 1)));
    }

    #[test]
    fn test_match_non_capturing_group() {
        let m = do_match("(?:abc)", "abc").unwrap();
        assert_eq!(m.end, 3);
        // No groups captured.
        assert_eq!(m.groups.len(), 1); // only slot 0
    }

    // --- Alternation ---

    #[test]
    fn test_match_alternation() {
        let m = do_match("cat|dog", "dog").unwrap();
        assert_eq!(m.end, 3);
    }

    #[test]
    fn test_match_alternation_first() {
        let m = do_match("cat|dog", "cat").unwrap();
        assert_eq!(m.end, 3);
    }

    #[test]
    fn test_match_alternation_no_match() {
        assert!(do_match("cat|dog", "fish").is_none());
    }

    // --- Backreferences ---

    #[test]
    fn test_match_backref() {
        let m = do_match("(a)\\1", "aa").unwrap();
        assert_eq!(m.end, 2);
    }

    #[test]
    fn test_match_backref_fail() {
        assert!(do_match("(a)\\1", "ab").is_none());
    }

    // --- Lookahead ---

    #[test]
    fn test_match_positive_lookahead() {
        let m = do_match("a(?=b)", "ab").unwrap();
        assert_eq!(m.end, 1); // lookahead doesn't consume
    }

    #[test]
    fn test_match_positive_lookahead_fail() {
        assert!(do_match("a(?=b)", "ac").is_none());
    }

    #[test]
    fn test_match_negative_lookahead() {
        let m = do_match("a(?!b)", "ac").unwrap();
        assert_eq!(m.end, 1);
    }

    #[test]
    fn test_match_negative_lookahead_fail() {
        assert!(do_match("a(?!b)", "ab").is_none());
    }

    // --- Lookbehind ---

    #[test]
    fn test_match_positive_lookbehind() {
        let compiled = parse_pattern("(?<=a)b", 0).unwrap();
        let text = "ab";
        let text_len = text.chars().count();
        let m = execute_match(&compiled, text, 0, text_len, "search").unwrap();
        assert_eq!(m.start, 1);
        assert_eq!(m.end, 2);
    }

    #[test]
    fn test_match_negative_lookbehind() {
        let compiled = parse_pattern("(?<!a)b", 0).unwrap();
        let text = "cb";
        let text_len = text.chars().count();
        let m = execute_match(&compiled, text, 0, text_len, "search").unwrap();
        assert_eq!(m.start, 1);
        assert_eq!(m.end, 2);
    }

    #[test]
    fn test_match_negative_lookbehind_fail() {
        let compiled = parse_pattern("(?<!a)b", 0).unwrap();
        let text = "ab";
        let text_len = text.chars().count();
        let m = execute_match(&compiled, text, 0, text_len, "search");
        assert!(m.is_none());
    }

    // --- Case insensitive ---

    #[test]
    fn test_match_ignorecase() {
        let m = do_execute("hello", RE_IGNORECASE, "HELLO", "match").unwrap();
        assert_eq!(m.end, 5);
    }

    #[test]
    fn test_match_charclass_ignorecase() {
        let m = do_execute("[a-z]", RE_IGNORECASE, "Z", "match").unwrap();
        assert_eq!(m.end, 1);
    }

    // --- Complex patterns ---

    #[test]
    fn test_match_email_like() {
        let m = do_search("\\w+@\\w+", "foo@bar").unwrap();
        assert_eq!(m.start, 0);
        assert_eq!(m.end, 7);
    }

    #[test]
    fn test_match_digits_in_parens() {
        let m = do_search("\\((\\d+)\\)", "call(42)").unwrap();
        assert_eq!(m.start, 4);
        assert_eq!(m.end, 8);
        assert_eq!(m.groups[1], Some((5, 7)));
    }

    // --- finditer_collect ---

    #[test]
    fn test_finditer_collect_basic() {
        let compiled = parse_pattern("\\d+", 0).unwrap();
        let text = "a1b22c333d";
        let text_len = text.chars().count();
        let mut results = Vec::new();
        let mut cur = 0;
        let end = text_len;
        while cur <= end {
            match execute_match(&compiled, text, cur, end, "search") {
                Some(result) => {
                    let match_end = result.end;
                    results.push((result.start, result.end));
                    if match_end == result.start {
                        cur = result.start + 1;
                    } else {
                        cur = match_end;
                    }
                }
                None => break,
            }
        }
        assert_eq!(results, vec![(1, 2), (3, 5), (6, 9)]);
    }

    #[test]
    fn test_search_empty_pattern() {
        let m = do_search("", "abc").unwrap();
        assert_eq!(m.start, 0);
        assert_eq!(m.end, 0);
    }

    #[test]
    fn test_fullmatch_star() {
        let m = do_fullmatch("a*", "aaa").unwrap();
        assert_eq!(m.end, 3);
    }

    #[test]
    fn test_fullmatch_star_empty() {
        let m = do_fullmatch("a*", "").unwrap();
        assert_eq!(m.end, 0);
    }

    // --- Scoped flags ---

    #[test]
    fn test_match_scoped_ignorecase() {
        let m = do_match("(?i:hello) world", "HELLO world").unwrap();
        assert_eq!(m.end, 11);
    }

    #[test]
    fn test_match_scoped_ignorecase_outside() {
        assert!(do_match("(?i:hello) WORLD", "HELLO world").is_none());
    }

    // --- Conditional ---

    #[test]
    fn test_match_conditional_yes() {
        let m = do_match("(a)(?(1)b|c)", "ab").unwrap();
        assert_eq!(m.end, 2);
    }

    #[test]
    fn test_match_conditional_no() {
        let m = do_match("(a)?(?(1)b|c)", "c").unwrap();
        assert_eq!(m.end, 1);
    }

    // --- Unicode ---

    #[test]
    fn test_match_unicode_literal() {
        let m = do_match("cafe\u{0301}", "cafe\u{0301}").unwrap();
        assert_eq!(m.end, 5);
    }

    #[test]
    fn test_search_unicode() {
        let m = do_search("\\w+", "hello \u{4e16}\u{754c}").unwrap();
        assert_eq!(m.start, 0);
        assert_eq!(m.end, 5);
    }

    // --- Named groups ---

    #[test]
    fn test_match_named_group() {
        let compiled = parse_pattern("(?P<word>\\w+)", 0).unwrap();
        let text = "hello";
        let text_len = text.chars().count();
        let m = execute_match(&compiled, text, 0, text_len, "match").unwrap();
        assert_eq!(m.groups[1], Some((0, 5)));
    }

    // --- Named backref ---

    #[test]
    fn test_match_named_backref() {
        let compiled = parse_pattern("(?P<w>\\w+) (?P=w)", 0).unwrap();
        let text = "abc abc";
        let text_len = text.chars().count();
        let m = execute_match(&compiled, text, 0, text_len, "match").unwrap();
        assert_eq!(m.start, 0);
        assert_eq!(m.end, 7);
        assert_eq!(m.groups[1], Some((0, 3)));
    }

    #[test]
    fn test_match_named_backref_fail() {
        let compiled = parse_pattern("(?P<w>\\w+) (?P=w)", 0).unwrap();
        let text = "abc def";
        let text_len = text.chars().count();
        let m = execute_match(&compiled, text, 0, text_len, "match");
        assert!(m.is_none());
    }

    // --- Greedy backtracking through multiple patterns ---

    #[test]
    fn test_greedy_backtrack_complex() {
        let m = do_match(".*(\\d+)", "abc123").unwrap();
        assert_eq!(m.end, 6);
        // Greedy .* eats as much as possible, then \d+ needs at least 1 digit.
        assert_eq!(m.groups[1], Some((5, 6)));
    }

    #[test]
    fn test_lazy_captures_more() {
        let m = do_match(".*?(\\d+)", "abc123").unwrap();
        assert_eq!(m.end, 6);
        // Lazy .*? matches "abc", \d+ matches "123".
        assert_eq!(m.groups[1], Some((3, 6)));
    }

    // --- Edge cases ---

    #[test]
    fn test_match_empty_string() {
        let m = do_match("", "").unwrap();
        assert_eq!(m.start, 0);
        assert_eq!(m.end, 0);
    }

    #[test]
    fn test_search_empty_string() {
        let m = do_search("", "").unwrap();
        assert_eq!(m.start, 0);
        assert_eq!(m.end, 0);
    }

    #[test]
    fn test_match_pos_beyond_text() {
        let compiled = parse_pattern("a", 0).unwrap();
        let m = execute_match(&compiled, "a", 5, 5, "match");
        assert!(m.is_none());
    }

    // -----------------------------------------------------------------------
    // split / sub helper tests (internal logic)
    // -----------------------------------------------------------------------

    /// Helper: split a string by a compiled pattern.
    fn do_split(pattern: &str, text: &str, maxsplit: usize) -> Vec<String> {
        let compiled = parse_pattern(pattern, 0).unwrap();
        let chars: Vec<char> = text.chars().collect();
        let text_len = chars.len();
        let limit = if maxsplit == 0 { None } else { Some(maxsplit) };

        let mut result_parts: Vec<String> = Vec::new();
        let mut last: usize = 0;
        let mut splits: usize = 0;
        let mut cur: usize = 0;
        let mut prev_empty_at: Option<usize> = None;

        while cur <= text_len {
            if let Some(lim) = limit
                && splits >= lim
            {
                break;
            }

            match execute_match(&compiled, text, cur, text_len, "search") {
                Some(result) => {
                    let m_start = result.start;
                    let m_end = result.end;

                    if m_start == m_end {
                        if prev_empty_at == Some(m_start) {
                            if cur < text_len {
                                cur += 1;
                            } else {
                                break;
                            }
                            continue;
                        }
                        prev_empty_at = Some(m_start);
                    } else {
                        prev_empty_at = None;
                    }

                    let segment: String = chars[last..m_start].iter().collect();
                    result_parts.push(segment);

                    // Include capturing groups.
                    for i in 1..=(compiled.group_count as usize) {
                        if i < result.groups.len() {
                            match result.groups[i] {
                                Some((gs, ge)) => {
                                    let group_text: String = chars[gs..ge].iter().collect();
                                    result_parts.push(group_text);
                                }
                                None => {
                                    result_parts.push(String::new());
                                }
                            }
                        } else {
                            result_parts.push(String::new());
                        }
                    }

                    last = m_end;
                    splits += 1;

                    if m_end == m_start {
                        if cur < text_len {
                            cur = m_start + 1;
                        } else {
                            break;
                        }
                    } else {
                        cur = m_end;
                    }
                }
                None => break,
            }
        }

        let tail: String = chars[last..].iter().collect();
        result_parts.push(tail);
        result_parts
    }

    /// Helper: sub with string replacement.
    fn do_sub(pattern: &str, repl: &str, text: &str, count: usize) -> (String, usize) {
        let compiled = parse_pattern(pattern, 0).unwrap();
        let chars: Vec<char> = text.chars().collect();
        let text_len = chars.len();
        let limit = if count == 0 { None } else { Some(count) };
        let repl_has_backref = repl.contains('\\');

        let mut out = String::with_capacity(text.len());
        let mut last: usize = 0;
        let mut replaced: usize = 0;
        let mut cur: usize = 0;
        let mut prev_empty_at: Option<usize> = None;

        while cur <= text_len {
            if let Some(lim) = limit
                && replaced >= lim
            {
                break;
            }

            match execute_match(&compiled, text, cur, text_len, "search") {
                Some(result) => {
                    let m_start = result.start;
                    let m_end = result.end;

                    if m_start == m_end {
                        if prev_empty_at == Some(m_start) {
                            if cur < text_len {
                                cur += 1;
                            } else {
                                break;
                            }
                            continue;
                        }
                        prev_empty_at = Some(m_start);
                    } else {
                        prev_empty_at = None;
                    }

                    let segment: String = chars[last..m_start].iter().collect();
                    out.push_str(&segment);

                    if repl_has_backref {
                        let expanded = expand_repl_with_groups(
                            repl,
                            text,
                            &chars,
                            &result,
                            compiled.group_count,
                            &compiled.group_names,
                        );
                        out.push_str(&expanded);
                    } else {
                        out.push_str(repl);
                    }

                    last = m_end;
                    replaced += 1;

                    if m_end == m_start {
                        if cur < text_len {
                            cur = m_start + 1;
                        } else {
                            break;
                        }
                    } else {
                        cur = m_end;
                    }
                }
                None => break,
            }
        }

        let tail: String = chars[last..].iter().collect();
        out.push_str(&tail);
        (out, replaced)
    }

    #[test]
    fn test_split_basic() {
        let result = do_split("\\s+", "foo bar  baz", 0);
        assert_eq!(result, vec!["foo", "bar", "baz"]);
    }

    #[test]
    fn test_split_with_maxsplit() {
        let result = do_split("\\s+", "foo bar  baz qux", 2);
        assert_eq!(result, vec!["foo", "bar", "baz qux"]);
    }

    #[test]
    fn test_split_with_groups() {
        let result = do_split("(\\s+)", "foo bar", 0);
        assert_eq!(result, vec!["foo", " ", "bar"]);
    }

    #[test]
    fn test_split_no_match() {
        let result = do_split("x", "abc", 0);
        assert_eq!(result, vec!["abc"]);
    }

    #[test]
    fn test_sub_basic() {
        let (result, count) = do_sub("\\d+", "NUM", "abc123def456", 0);
        assert_eq!(result, "abcNUMdefNUM");
        assert_eq!(count, 2);
    }

    #[test]
    fn test_sub_with_count() {
        let (result, count) = do_sub("\\d+", "NUM", "abc123def456ghi789", 2);
        assert_eq!(result, "abcNUMdefNUMghi789");
        assert_eq!(count, 2);
    }

    #[test]
    fn test_sub_with_backref() {
        let (result, _count) = do_sub("(\\w+)", "[\\1]", "hello world", 0);
        assert_eq!(result, "[hello] [world]");
    }

    #[test]
    fn test_sub_no_match() {
        let (result, count) = do_sub("xyz", "ABC", "hello world", 0);
        assert_eq!(result, "hello world");
        assert_eq!(count, 0);
    }

    #[test]
    fn test_expand_repl_numbered_group() {
        let result = MatchResult {
            start: 0,
            end: 5,
            groups: vec![None, Some((0, 5))],
        };
        let chars: Vec<char> = "hello".chars().collect();
        let names = HashMap::new();
        let expanded = expand_repl_with_groups("<<\\1>>", "hello", &chars, &result, 1, &names);
        assert_eq!(expanded, "<<hello>>");
    }

    #[test]
    fn test_expand_repl_named_group() {
        let result = MatchResult {
            start: 0,
            end: 5,
            groups: vec![None, Some((0, 5))],
        };
        let chars: Vec<char> = "hello".chars().collect();
        let mut names = HashMap::new();
        names.insert("word".to_string(), 1u32);
        let expanded =
            expand_repl_with_groups("<<\\g<word>>>", "hello", &chars, &result, 1, &names);
        assert_eq!(expanded, "<<hello>>");
    }

    #[test]
    fn test_expand_repl_group_zero() {
        let result = MatchResult {
            start: 0,
            end: 5,
            groups: vec![None],
        };
        let chars: Vec<char> = "hello".chars().collect();
        let names = HashMap::new();
        let expanded = expand_repl_with_groups("<<\\g<0>>>", "hello", &chars, &result, 0, &names);
        assert_eq!(expanded, "<<hello>>");
    }

    #[test]
    fn test_expand_repl_escape_sequences() {
        let result = MatchResult {
            start: 0,
            end: 0,
            groups: vec![None],
        };
        let chars: Vec<char> = "".chars().collect();
        let names = HashMap::new();
        let expanded = expand_repl_with_groups("a\\nb\\tc\\\\d", "", &chars, &result, 0, &names);
        assert_eq!(expanded, "a\nb\tc\\d");
    }

    // -----------------------------------------------------------------------
    // re_escape tests
    // -----------------------------------------------------------------------

    #[test]
    fn test_escape_no_special() {
        assert_eq!(re_escape_impl("hello world"), "hello world");
    }

    #[test]
    fn test_escape_all_special() {
        assert_eq!(re_escape_impl("a.b*c?d+e"), "a\\.b\\*c\\?d\\+e");
    }

    #[test]
    fn test_escape_brackets_parens() {
        assert_eq!(re_escape_impl("[foo](bar)"), "\\[foo\\]\\(bar\\)");
    }

    #[test]
    fn test_escape_backslash() {
        assert_eq!(re_escape_impl("a\\b"), "a\\\\b");
    }

    #[test]
    fn test_escape_pipe_caret_dollar() {
        assert_eq!(re_escape_impl("^a|b$"), "\\^a\\|b\\$");
    }

    #[test]
    fn test_escape_braces() {
        assert_eq!(re_escape_impl("a{1,2}"), "a\\{1,2\\}");
    }

    #[test]
    fn test_escape_empty() {
        assert_eq!(re_escape_impl(""), "");
    }

    #[test]
    fn test_escape_nul() {
        assert_eq!(re_escape_impl("a\0b"), "a\\000b");
    }
}
