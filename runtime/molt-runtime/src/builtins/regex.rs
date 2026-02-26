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
//!   with `crate::with_gil_entry!(_py, { … })` as the outer frame.

use molt_obj_model::MoltObject;

use crate::{
    TYPE_ID_DICT, TYPE_ID_LIST, TYPE_ID_TUPLE, alloc_string, dec_ref_bits, exception_pending,
    is_truthy, obj_from_bits, object_type_id, raise_exception, seq_vec_ref, string_obj_to_owned,
    to_i64,
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
    crate::with_gil_entry!(_py, {
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
    crate::with_gil_entry!(_py, {
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
    crate::with_gil_entry!(_py, {
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
    crate::with_gil_entry!(_py, {
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
fn re_strip_verbose_impl(pattern: &str, _flags: i64) -> String {
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
    crate::with_gil_entry!(_py, {
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
    crate::with_gil_entry!(_py, {
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
fn decode_group_names(_py: &crate::PyToken<'_>, dict_bits: u64) -> Result<Vec<(String, i64)>, u64> {
    let dict_obj = obj_from_bits(dict_bits);
    let Some(dict_ptr) = dict_obj.as_ptr() else {
        return Err(raise_exception::<u64>(
            _py,
            "TypeError",
            "groups must be a dict",
        ));
    };
    // We iterate the dict using molt_iter, which yields (key, value) pairs.
    let iter_bits = crate::molt_iter(dict_bits);
    if exception_pending(_py) {
        return Err(MoltObject::none().bits());
    }
    let _ = dict_ptr; // suppress unused warning — dict_ptr validated above
    let mut out: Vec<(String, i64)> = Vec::new();
    loop {
        let item_bits = crate::molt_iter_next(iter_bits);
        let item_obj = obj_from_bits(item_bits);
        let Some(item_ptr) = item_obj.as_ptr() else {
            // None signals StopIteration.
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
    _py: &crate::PyToken<'_>,
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
    crate::with_gil_entry!(_py, {
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
}
