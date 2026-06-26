use super::*;

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
pub(super) fn re_negative_lookahead_impl(
    text: &str,
    pos: i64,
    end: i64,
    pattern: &str,
    flags: i64,
) -> i64 {
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
pub(super) fn re_positive_lookahead_impl(
    text: &str,
    pos: i64,
    end: i64,
    pattern: &str,
    flags: i64,
) -> i64 {
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
pub(super) fn re_negative_lookbehind_impl(
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
pub(super) fn re_positive_lookbehind_impl(
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
