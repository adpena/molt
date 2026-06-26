use super::*;
use std::collections::HashMap;

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
        let guard = regex_state(_py)
            .patterns
            .lock()
            .unwrap_or_else(|e| e.into_inner());
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
        let guard = regex_state(_py)
            .patterns
            .lock()
            .unwrap_or_else(|e| e.into_inner());
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
pub(super) fn expand_repl_with_groups(
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
pub(super) const RE_SPECIAL_CHARS: &[char] = &[
    '\\', '.', '^', '$', '*', '+', '?', '{', '}', '[', ']', '|', '(', ')',
];

/// Pure-Rust implementation of `re.escape()`.
///
/// Prefixes every character in `pattern` that has special regex meaning with
/// a backslash.  NUL characters are also escaped as `\000`.
pub(super) fn re_escape_impl(pattern: &str) -> String {
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
        let guard = regex_state(_py)
            .patterns
            .lock()
            .unwrap_or_else(|e| e.into_inner());
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
                    let match_tuple_bits = build_match_result_bits(_py, &result, group_count);
                    let repl_result_bits =
                        call_callable1(_py, repl_callable_bits, match_tuple_bits);
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
