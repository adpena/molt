use super::*;

// ---------------------------------------------------------------------------
// Re flag constants (kept in sync with functions.rs and re/__init__.py)
// ---------------------------------------------------------------------------

pub(super) const RE_IGNORECASE: i64 = 2;
pub(super) const RE_VERBOSE: i64 = 64;
pub(super) const RE_ASCII: i64 = 256;

// Sentinel returned when the intrinsic cannot evaluate the sub-pattern and
// Python must fall back to its own engine.
pub(super) const SENTINEL_FALLBACK: i64 = -2;

// ---------------------------------------------------------------------------
// Internal helpers
// ---------------------------------------------------------------------------

/// Collect character positions of a single Unicode codepoint at logical index
/// `idx` within a `chars` slice.  Returns `None` if out of range.
#[inline]
pub(super) fn char_at(chars: &[char], idx: i64) -> Option<char> {
    let i = usize::try_from(idx).ok()?;
    chars.get(i).copied()
}

/// Try to match a literal string starting at char-index `pos` in `chars`,
/// honouring the IGNORECASE flag.  Returns `true` on a match.
pub(super) fn literal_matches_at(chars: &[char], pos: usize, literal: &[char], flags: i64) -> bool {
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
pub(super) fn simple_category_matches(ch: char, category: &str, flags: i64) -> Option<bool> {
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
