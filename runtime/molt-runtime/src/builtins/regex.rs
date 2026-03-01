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
    TYPE_ID_DICT, TYPE_ID_LIST, TYPE_ID_TUPLE, alloc_dict_with_pairs, alloc_string, alloc_tuple,
    dec_ref_bits, exception_pending, is_truthy, obj_from_bits, object_type_id, raise_exception,
    seq_vec_ref, string_obj_to_owned, to_i64,
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
use std::sync::{LazyLock, Mutex, atomic::{AtomicI64, Ordering}};

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
            if let ReNode::Literal(ref new_text) = node {
                if let Some(ReNode::Literal(prev_text)) = nodes.last_mut() {
                    let combined = prev_text.clone() + new_text;
                    *prev_text = combined;
                    continue;
                }
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
                if let Some(max) = max_count {
                    if max < min_count {
                        return Err("invalid quantifier range".to_string());
                    }
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
        digits.parse::<u64>().map_err(|_| "number overflow".to_string())
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
            c if ")*+?{}|".contains(c) => {
                Err(format!("unexpected character '{c}'"))
            }
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
        // Capturing group.
        let node = self.parse_expr()?;
        if self.peek() != Some(')') {
            return Err("missing )".to_string());
        }
        self.next_ch()?;
        self.group_count += 1;
        let idx = self.group_count;
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
                        let width = fixed_width(&node, Some(&self.group_widths))
                            .ok_or_else(|| "look-behind requires fixed-width pattern".to_string())?;
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
                        let width = fixed_width(&node, Some(&self.group_widths))
                            .ok_or_else(|| "look-behind requires fixed-width pattern".to_string())?;
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
                let group_index = digits.parse::<u32>().map_err(|_| "group index overflow".to_string())?;
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
                        let idx = self.group_names.get(&name)
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
            Some(c) if "imsxaLu-".contains(c) || c == ')' => {
                self.parse_inline_flags()
            }
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
        self.group_count += 1;
        let idx = self.group_count;
        self.group_names.insert(name, idx);
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
                Some(c @ ('i' | 'm' | 's' | 'x' | 'a' | 'L' | 'u' | 'I' | 'M' | 'S' | 'X' | 'A' | 'U')) => {
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
                let idx = digits.parse::<u32>().map_err(|_| "backref index overflow".to_string())?;
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
        ReNode::Backref(idx) => {
            group_widths?.get(idx).copied().flatten()
        }
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
        ReNode::Repeat { node, min_count, max_count, .. } => {
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
    crate::with_gil_entry!(_py, {
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
            Err(msg) => {
                raise_exception::<_>(_py, "ValueError", &msg)
            }
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
    crate::with_gil_entry!(_py, {
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
// molt_re_execute stub (Phase-1b placeholder)
// ---------------------------------------------------------------------------

/// `molt_re_execute(handle, text, pos, end, mode) -> None`
///
/// Phase-1b stub.  Always returns None, signalling Python to fall back to its
/// own match engine.  The full Rust NFA/backtracking engine will replace this
/// in a future phase.
#[unsafe(no_mangle)]
pub extern "C" fn molt_re_execute(
    _handle_bits: u64,
    _text_bits: u64,
    _pos_bits: u64,
    _end_bits: u64,
    _mode_bits: u64,
) -> u64 {
    crate::with_gil_entry!(_py, {
        MoltObject::none().bits()
    })
}

// ---------------------------------------------------------------------------
// molt_re_finditer_collect stub (Phase-1b placeholder)
// ---------------------------------------------------------------------------

/// `molt_re_finditer_collect(handle, text, pos, end) -> None`
///
/// Phase-1b stub.  Always returns None, signalling Python to fall back to its
/// own finditer engine.
#[unsafe(no_mangle)]
pub extern "C" fn molt_re_finditer_collect(
    _handle_bits: u64,
    _text_bits: u64,
    _pos_bits: u64,
    _end_bits: u64,
) -> u64 {
    crate::with_gil_entry!(_py, {
        MoltObject::none().bits()
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
            ReNode::Repeat { min_count, max_count, greedy, .. } => {
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
            ReNode::Repeat { min_count, max_count, greedy, .. } => {
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
            ReNode::Repeat { min_count, max_count, .. } => {
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
            ReNode::CharClass { negated, categories, .. } => {
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
            ReNode::CharClass { negated, categories, .. } => {
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
            ReNode::Look { behind, positive, .. } => {
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
            ReNode::Look { behind, positive, .. } => {
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
            ReNode::Look { behind, positive, width, .. } => {
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
            ReNode::Look { behind, positive, width, .. } => {
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
            ReNode::ScopedFlags { add_flags, clear_flags, .. } => {
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
        assert_eq!(fixed_width(&ReNode::Literal("hello".to_string()), None), Some(5));
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
                assert!(chars.contains(&"A".to_string()), "expected 'A' in chars, got {chars:?}");
            }
            other => panic!("expected CharClass, got {other:?}"),
        }
    }
}
