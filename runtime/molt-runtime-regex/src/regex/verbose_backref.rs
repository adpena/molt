use super::*;

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
pub(super) fn re_strip_verbose_impl(pattern: &str, flags: i64) -> String {
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
