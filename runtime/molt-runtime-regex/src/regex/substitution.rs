use super::*;
use std::collections::HashMap;
use std::sync::Arc;

/// Python's match-count convention: zero is unlimited, negative is no work.
fn match_limit(value: i64) -> Option<usize> {
    if value == 0 {
        None
    } else {
        Some(if value < 0 {
            0
        } else {
            usize::try_from(value).unwrap_or(usize::MAX)
        })
    }
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
        let limit = match_limit(maxsplit);

        let mut result_parts = Vec::new();
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
            if seg_ptr.is_null() {
                return MoltObject::none().bits();
            }
            result_parts.push(unsafe {
                OwnedRuntimeValue::from_owned_bits(_py, MoltObject::from_ptr(seg_ptr).bits())
            });

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
                            if gptr.is_null() {
                                return MoltObject::none().bits();
                            }
                            result_parts.push(unsafe {
                                OwnedRuntimeValue::from_owned_bits(
                                    _py,
                                    MoltObject::from_ptr(gptr).bits(),
                                )
                            });
                        }
                        None => {
                            result_parts.push(unsafe {
                                OwnedRuntimeValue::from_owned_bits(_py, MoltObject::none().bits())
                            });
                        }
                    }
                } else {
                    result_parts.push(unsafe {
                        OwnedRuntimeValue::from_owned_bits(_py, MoltObject::none().bits())
                    });
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
        if tail_ptr.is_null() {
            return MoltObject::none().bits();
        }
        result_parts.push(unsafe {
            OwnedRuntimeValue::from_owned_bits(_py, MoltObject::from_ptr(tail_ptr).bits())
        });

        let list_ptr = alloc_list(_py, OwnedRuntimeValue::as_bits_slice(&result_parts));
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
        let limit = match_limit(count);

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
        if result_str_ptr.is_null() {
            return MoltObject::none().bits();
        }
        let result_string = unsafe {
            OwnedRuntimeValue::from_owned_bits(_py, MoltObject::from_ptr(result_str_ptr).bits())
        };
        let count_bits_out = MoltObject::from_int(replaced as i64).bits();

        let tuple_ptr = alloc_tuple(_py, &[result_string.bits(), count_bits_out]);
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
    let limit = match_limit(count);
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
    if result_ptr.is_null() {
        return MoltObject::none().bits();
    }
    let result =
        unsafe { OwnedRuntimeValue::from_owned_bits(py, MoltObject::from_ptr(result_ptr).bits()) };
    let count_bits = MoltObject::from_int(replaced as i64).bits();
    let tuple_ptr = alloc_tuple(py, &[result.bits(), count_bits]);
    if tuple_ptr.is_null() {
        MoltObject::none().bits()
    } else {
        MoltObject::from_ptr(tuple_ptr).bits()
    }
}

pub(super) fn expand_match_replacement(
    repl: &[u32],
    subject: SubjectStorage<'_>,
    result: &MatchResult,
    group_count: u32,
    names: &HashMap<String, u32>,
) -> Result<Vec<u32>, super::functions_re::ReplacementError> {
    let mut groups = Vec::with_capacity(group_count as usize + 1);
    let spans = std::iter::once(Some((result.start, result.end)))
        .chain(result.groups.iter().skip(1).copied());
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

/// `molt_re_escape(pattern: str) -> str`
///
/// Escape all special regex characters in `pattern` so it can be used as a
/// literal string in a regex.
#[unsafe(no_mangle)]
pub extern "C" fn molt_re_escape(pattern_bits: u64) -> u64 {
    molt_runtime_core::with_core_gil!(_py, {
        let object = obj_from_bits(pattern_bits);
        let is_bytes = object
            .as_ptr()
            .is_some_and(|ptr| unsafe { object_type_id(ptr) } == TYPE_ID_BYTES);
        let escaped = if is_bytes {
            let Some(pattern) = (unsafe { bytes_obj_to_borrowed(object) }) else {
                return raise_exception::<_>(_py, "TypeError", "pattern must be str or bytes");
            };
            re_escape_impl(pattern)
        } else {
            let Some(pattern) = string_obj_to_owned(object) else {
                return raise_exception::<_>(_py, "TypeError", "pattern must be str or bytes");
            };
            re_escape_impl(pattern.as_bytes())
        };
        let out_ptr = if is_bytes {
            alloc_bytes(_py, &escaped)
        } else {
            alloc_string(_py, &escaped)
        };
        if out_ptr.is_null() {
            MoltObject::none().bits()
        } else {
            MoltObject::from_ptr(out_ptr).bits()
        }
    })
}

/// CPython's ASCII special-character map applies unchanged to UTF-8 text and
/// raw byte patterns. Non-ASCII and NUL units pass through without escaping.
pub(super) fn re_escape_impl(pattern: &[u8]) -> Vec<u8> {
    let mut escaped = Vec::with_capacity(pattern.len());
    for &byte in pattern {
        if matches!(
            byte,
            b'(' | b')'
                | b'['
                | b']'
                | b'{'
                | b'}'
                | b'?'
                | b'*'
                | b'+'
                | b'-'
                | b'|'
                | b'^'
                | b'$'
                | b'\\'
                | b'.'
                | b'&'
                | b'~'
                | b'#'
                | b' '
                | b'\t'
                | b'\n'
                | b'\r'
                | 0x0b
                | 0x0c
        ) {
            escaped.push(b'\\');
        }
        escaped.push(byte);
    }
    escaped
}
