use super::*;
use std::collections::HashMap;
use std::sync::Arc;

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
        let Some(compiled) = pattern_handle_arc(handle_bits) else {
            return raise_exception::<_>(_py, "ValueError", "invalid regex handle");
        };
        let text_obj = obj_from_bits(text_bits);
        let Some(maxsplit) = to_i64(obj_from_bits(maxsplit_bits)) else {
            return raise_exception::<_>(_py, "TypeError", "maxsplit must be int");
        };

        let group_count = compiled.compiled.group_count;
        let is_bytes = compiled.compiled.is_bytes;

        let text = if is_bytes {
            None
        } else {
            string_obj_to_owned(text_obj)
        };
        if !is_bytes && text.is_none() {
            return raise_exception::<_>(_py, "TypeError", "expected string or bytes-like object");
        }
        let bytes = if is_bytes {
            unsafe { bytes_obj_to_borrowed(text_obj) }
        } else {
            None
        };
        if is_bytes && bytes.is_none() {
            return raise_exception::<_>(
                _py,
                "TypeError",
                "cannot use a bytes pattern on a string-like object",
            );
        }
        let chars: Vec<char> = text.as_deref().unwrap_or_default().chars().collect();
        let text_len = bytes.map_or(chars.len(), <[u8]>::len);
        let limit = if maxsplit <= 0 {
            None
        } else {
            Some(maxsplit as usize)
        };

        let mut result_parts: Vec<u64> = Vec::new();
        let mut last: usize = 0;
        let mut splits: usize = 0;
        let subject = if let Some(bytes) = bytes {
            SubjectStorage::Bytes(bytes)
        } else {
            SubjectStorage::Text(&chars)
        };
        let mut scanner = ScannerHandle::new(Arc::clone(&compiled), 0, text_len, None);
        while limit.is_none_or(|lim| splits < lim) {
            let Some(result) =
                super::execute_api::advance_scanner(&compiled.compiled, &mut scanner, subject)
            else {
                break;
            };
            let m_start = result.start;
            let m_end = result.end;

            // Append text[last..m_start]
            let seg_ptr = if let Some(bytes) = bytes {
                alloc_bytes(_py, &bytes[last..m_start])
            } else {
                let segment: String = chars[last..m_start].iter().collect();
                alloc_string(_py, segment.as_bytes())
            };
            if !seg_ptr.is_null() {
                result_parts.push(MoltObject::from_ptr(seg_ptr).bits());
            }

            // Append capturing group values (CPython includes them in split output).
            for i in 1..=(group_count as usize) {
                if i < result.groups.len() {
                    match result.groups[i] {
                        Some((gs, ge)) => {
                            let gptr = if let Some(bytes) = bytes {
                                alloc_bytes(_py, &bytes[gs..ge])
                            } else {
                                let group_text: String = chars[gs..ge].iter().collect();
                                alloc_string(_py, group_text.as_bytes())
                            };
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
        }

        // Append the remaining text.
        let tail_ptr = if let Some(bytes) = bytes {
            alloc_bytes(_py, &bytes[last..])
        } else {
            let tail: String = chars[last..].iter().collect();
            alloc_string(_py, tail.as_bytes())
        };
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
        let Some(compiled) = pattern_handle_arc(handle_bits) else {
            return raise_exception::<_>(_py, "ValueError", "invalid regex handle");
        };
        let Some(count) = to_i64(obj_from_bits(count_bits)) else {
            return raise_exception::<_>(_py, "TypeError", "count must be int");
        };

        let group_count = compiled.compiled.group_count;
        let is_bytes = compiled.compiled.is_bytes;

        if is_bytes {
            return molt_re_sub_bytes(_py, repl_bits, text_bits, count, &compiled);
        }

        let Some(repl) = string_obj_to_owned(obj_from_bits(repl_bits)) else {
            return raise_exception::<_>(_py, "TypeError", "replacement must be str");
        };
        let Some(text) = string_obj_to_owned(obj_from_bits(text_bits)) else {
            return raise_exception::<_>(
                _py,
                "TypeError",
                "cannot use a string pattern on a bytes-like object",
            );
        };

        let chars: Vec<char> = text.chars().collect();
        let text_len = chars.len();
        let limit = if count <= 0 {
            None
        } else {
            Some(count as usize)
        };

        // Check if repl contains backreferences (backslash).
        let repl_units: Vec<u32> = repl.chars().map(u32::from).collect();

        let mut out = String::with_capacity(text.len());
        let mut last: usize = 0;
        let mut replaced: usize = 0;
        let mut scanner = ScannerHandle::new(Arc::clone(&compiled), 0, text_len, None);
        while limit.is_none_or(|lim| replaced < lim) {
            let Some(result) = super::execute_api::advance_scanner(
                &compiled.compiled,
                &mut scanner,
                SubjectStorage::Text(&chars),
            ) else {
                break;
            };
            let m_start = result.start;
            let m_end = result.end;

            // Append text[last..m_start].
            let segment: String = chars[last..m_start].iter().collect();
            out.push_str(&segment);

            // Expand the replacement template.
            let expanded = match expand_match_replacement(
                &repl_units,
                SubjectStorage::Text(&chars),
                &result,
                group_count,
                &compiled.compiled.group_names,
            ) {
                Ok(units) => match units
                    .into_iter()
                    .map(char::from_u32)
                    .collect::<Option<String>>()
                {
                    Some(value) => value,
                    None => {
                        return raise_exception::<_>(
                            _py,
                            "ValueError",
                            "invalid Unicode replacement",
                        );
                    }
                },
                Err(error) => {
                    return super::functions_re::replacement_error_result(_py, error);
                }
            };
            out.push_str(&expanded);

            last = m_end;
            replaced += 1;
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

fn molt_re_sub_bytes(
    py: &CoreGilToken,
    repl_bits: u64,
    text_bits: u64,
    count: i64,
    compiled: &Arc<PatternHandle>,
) -> u64 {
    let Some(repl) = (unsafe { bytes_obj_to_borrowed(obj_from_bits(repl_bits)) }) else {
        return raise_exception::<_>(
            py,
            "TypeError",
            "a bytes-like object is required for the replacement",
        );
    };
    let Some(text) = (unsafe { bytes_obj_to_borrowed(obj_from_bits(text_bits)) }) else {
        return raise_exception::<_>(
            py,
            "TypeError",
            "cannot use a bytes pattern on a string-like object",
        );
    };
    let mut out = Vec::with_capacity(text.len());
    let repl_units: Vec<u32> = repl.iter().map(|byte| u32::from(*byte)).collect();
    let limit = if count <= 0 {
        None
    } else {
        Some(count as usize)
    };
    let mut last = 0usize;
    let mut replaced = 0usize;
    let text_len = text.len();
    let mut scanner = ScannerHandle::new(Arc::clone(&compiled), 0, text_len, None);
    while limit.is_none_or(|n| replaced < n) {
        let Some(result) = super::execute_api::advance_scanner(
            &compiled.compiled,
            &mut scanner,
            SubjectStorage::Bytes(text),
        ) else {
            break;
        };
        let (start, end) = (result.start, result.end);
        out.extend_from_slice(&text[last..start]);
        let expanded = match expand_match_replacement(
            &repl_units,
            SubjectStorage::Bytes(text),
            &result,
            compiled.compiled.group_count,
            &compiled.compiled.group_names,
        ) {
            Ok(value) => value,
            Err(error) => return super::functions_re::replacement_error_result(py, error),
        };
        if expanded.iter().any(|unit| *unit > 255) {
            return raise_exception::<_>(
                py,
                "ValueError",
                "byte replacement produced a non-byte value",
            );
        }
        out.extend(expanded.into_iter().map(|unit| unit as u8));
        last = end;
        replaced += 1;
    }
    out.extend_from_slice(&text[last..]);
    let result_ptr = alloc_bytes(py, &out);
    let result_bits = if result_ptr.is_null() {
        MoltObject::none().bits()
    } else {
        MoltObject::from_ptr(result_ptr).bits()
    };
    let count_bits = MoltObject::from_int(replaced as i64).bits();
    let tuple_ptr = alloc_tuple(py, &[result_bits, count_bits]);
    if tuple_ptr.is_null() {
        MoltObject::none().bits()
    } else {
        MoltObject::from_ptr(tuple_ptr).bits()
    }
}

fn expand_match_replacement(
    repl: &[u32],
    subject: SubjectStorage<'_>,
    result: &MatchResult,
    group_count: u32,
    names: &HashMap<String, u32>,
) -> Result<Vec<u32>, super::functions_re::ReplacementError> {
    let mut groups = Vec::with_capacity(group_count as usize + 1);
    let spans =
        std::iter::once(Some((result.start, result.end))).chain(result.groups.iter().copied());
    for span in spans.take(group_count as usize + 1) {
        groups.push(span.map(|(start, end)| {
            (start..end)
                .map(|index| match subject {
                    SubjectStorage::Text(chars) => u32::from(chars[index]),
                    SubjectStorage::Bytes(bytes) => u32::from(bytes[index]),
                })
                .collect()
        }));
    }
    super::functions_re::re_expand_typed(repl, &groups, names)
}

#[cfg(test)]
pub(super) fn expand_repl_with_groups(
    repl: &str,
    _text: &str,
    chars: &[char],
    result: &MatchResult,
    group_count: u32,
    names: &HashMap<String, u32>,
) -> String {
    let units: Vec<u32> = repl.chars().map(u32::from).collect();
    let groups = std::iter::once(Some((result.start, result.end)))
        .chain(result.groups.iter().copied())
        .take(group_count as usize + 1)
        .map(|span| {
            span.map(|(start, end)| chars[start..end].iter().copied().map(u32::from).collect())
        })
        .collect::<Vec<_>>();
    let expanded = super::functions_re::re_expand_typed(&units, &groups, names).unwrap();
    expanded
        .into_iter()
        .map(char::from_u32)
        .map(Option::unwrap)
        .collect()
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
