// === FILE: runtime/molt-runtime/src/builtins/string_ext.rs ===
//
// Intrinsics for Python `string` module: Template $-substitution scanning,
// Formatter parse, and field name splitting.
//
// These are pure string-processing operations — no Python callbacks.
// The Python wrapper handles mapping lookups and method dispatch.

use crate::*;

// ─────────────────────────────────────────────────────────────────────────────
// Helpers
// ─────────────────────────────────────────────────────────────────────────────

#[inline]
fn is_identifier_start(b: u8) -> bool {
    b == b'_' || b.is_ascii_alphabetic()
}

#[inline]
fn is_identifier_continue(b: u8) -> bool {
    is_identifier_start(b) || b.is_ascii_digit()
}

fn scan_identifier(text: &[u8], start: usize) -> Option<(usize, usize)> {
    if start >= text.len() || !is_identifier_start(text[start]) {
        return None;
    }
    let mut end = start + 1;
    while end < text.len() && is_identifier_continue(text[end]) {
        end += 1;
    }
    Some((start, end))
}

// ─────────────────────────────────────────────────────────────────────────────
// Template scanning
// ─────────────────────────────────────────────────────────────────────────────

/// Scan a Template string and return a list of segments.
///
/// Each segment is a 3-tuple: (literal_text: str, var_name: str|None, original: str|None)
/// - `literal_text`: text before the variable (always present)
/// - `var_name`: the variable name if a $-variable was found, or None for the final segment
/// - `original`: the original $var or ${var} text for safe_substitute fallback
///
/// `template_bits` must be a str, `delimiter_bits` must be a str (usually "$").
#[unsafe(no_mangle)]
pub extern "C" fn molt_string_template_scan(template_bits: u64, delimiter_bits: u64) -> u64 {
    crate::with_gil_entry_nopanic!(_py, {
        let Some(template) = string_obj_to_owned(obj_from_bits(template_bits)) else {
            return raise_exception::<_>(_py, "TypeError", "template must be str");
        };
        let Some(delimiter) = string_obj_to_owned(obj_from_bits(delimiter_bits)) else {
            return raise_exception::<_>(_py, "TypeError", "delimiter must be str");
        };
        let text = template.as_bytes();
        let delim = delimiter.as_bytes();
        if delim.is_empty() {
            // No delimiter → entire string is literal.
            let lit_ptr = alloc_string(_py, text);
            let none = MoltObject::none().bits();
            let tup = alloc_tuple(_py, &[MoltObject::from_ptr(lit_ptr).bits(), none, none]);
            let list = alloc_list(_py, &[MoltObject::from_ptr(tup).bits()]);
            return MoltObject::from_ptr(list).bits();
        }

        let delim_len = delim.len();
        let length = text.len();
        let mut segments: Vec<u64> = Vec::new();
        let mut idx = 0usize;

        while idx < length {
            // Find next delimiter.
            let next_idx = text[idx..]
                .windows(delim_len)
                .position(|w| w == delim)
                .map(|p| p + idx);
            let Some(di) = next_idx else {
                // No more delimiters — emit final literal segment.
                let lit = &text[idx..];
                let lit_ptr = alloc_string(_py, lit);
                let none = MoltObject::none().bits();
                let tup = alloc_tuple(_py, &[MoltObject::from_ptr(lit_ptr).bits(), none, none]);
                segments.push(MoltObject::from_ptr(tup).bits());
                break;
            };
            // Literal before delimiter.
            let literal = &text[idx..di];

            // Check what follows the delimiter.
            let after = di + delim_len;
            if after > length - 1 {
                // Delimiter at very end — emit as literal.
                let lit = &text[idx..];
                let lit_ptr = alloc_string(_py, lit);
                let none = MoltObject::none().bits();
                let tup = alloc_tuple(_py, &[MoltObject::from_ptr(lit_ptr).bits(), none, none]);
                segments.push(MoltObject::from_ptr(tup).bits());
                break;
            }

            // Escaped delimiter: $$
            if text[after..].starts_with(delim) {
                let mut combined = literal.to_vec();
                combined.extend_from_slice(delim);
                let lit_ptr = alloc_string(_py, &combined);
                let none = MoltObject::none().bits();
                let tup = alloc_tuple(_py, &[MoltObject::from_ptr(lit_ptr).bits(), none, none]);
                segments.push(MoltObject::from_ptr(tup).bits());
                idx = after + delim_len;
                continue;
            }

            // ${name} form.
            if text[after] == b'{' {
                let brace_start = after + 1;
                if let Some(brace_end_rel) = text[brace_start..].iter().position(|&b| b == b'}') {
                    let brace_end = brace_start + brace_end_rel;
                    let name_bytes = &text[brace_start..brace_end];
                    if !name_bytes.is_empty() && scan_identifier(name_bytes, 0).is_some() {
                        let lit_ptr = alloc_string(_py, literal);
                        let name_ptr = alloc_string(_py, name_bytes);
                        let orig = &text[di..brace_end + 1];
                        let orig_ptr = alloc_string(_py, orig);
                        let tup = alloc_tuple(
                            _py,
                            &[
                                MoltObject::from_ptr(lit_ptr).bits(),
                                MoltObject::from_ptr(name_ptr).bits(),
                                MoltObject::from_ptr(orig_ptr).bits(),
                            ],
                        );
                        segments.push(MoltObject::from_ptr(tup).bits());
                        idx = brace_end + 1;
                        continue;
                    }
                }
                // Invalid brace pattern — emit delimiter as literal.
                let lit = &text[idx..after];
                let lit_ptr = alloc_string(_py, lit);
                let none = MoltObject::none().bits();
                let tup = alloc_tuple(_py, &[MoltObject::from_ptr(lit_ptr).bits(), none, none]);
                segments.push(MoltObject::from_ptr(tup).bits());
                idx = after;
                continue;
            }

            // $name form.
            if let Some((start, end)) = scan_identifier(text, after) {
                let lit_ptr = alloc_string(_py, literal);
                let name_ptr = alloc_string(_py, &text[start..end]);
                let orig = &text[di..end];
                let orig_ptr = alloc_string(_py, orig);
                let tup = alloc_tuple(
                    _py,
                    &[
                        MoltObject::from_ptr(lit_ptr).bits(),
                        MoltObject::from_ptr(name_ptr).bits(),
                        MoltObject::from_ptr(orig_ptr).bits(),
                    ],
                );
                segments.push(MoltObject::from_ptr(tup).bits());
                idx = end;
                continue;
            }

            // Not a valid variable — emit delimiter as literal.
            let lit = &text[idx..after];
            let lit_ptr = alloc_string(_py, lit);
            let none = MoltObject::none().bits();
            let tup = alloc_tuple(_py, &[MoltObject::from_ptr(lit_ptr).bits(), none, none]);
            segments.push(MoltObject::from_ptr(tup).bits());
            idx = after;
        }

        if segments.is_empty() {
            // Empty template.
            let lit_ptr = alloc_string(_py, b"");
            let none = MoltObject::none().bits();
            let tup = alloc_tuple(_py, &[MoltObject::from_ptr(lit_ptr).bits(), none, none]);
            segments.push(MoltObject::from_ptr(tup).bits());
        }

        let list_ptr = alloc_list(_py, &segments);
        MoltObject::from_ptr(list_ptr).bits()
    })
}

/// Check whether a template string is valid (all $-placeholders are well-formed).
#[unsafe(no_mangle)]
pub extern "C" fn molt_string_template_is_valid(template_bits: u64, delimiter_bits: u64) -> u64 {
    crate::with_gil_entry_nopanic!(_py, {
        let Some(template) = string_obj_to_owned(obj_from_bits(template_bits)) else {
            return raise_exception::<_>(_py, "TypeError", "template must be str");
        };
        let Some(delimiter) = string_obj_to_owned(obj_from_bits(delimiter_bits)) else {
            return raise_exception::<_>(_py, "TypeError", "delimiter must be str");
        };
        let text = template.as_bytes();
        let delim = delimiter.as_bytes();
        if delim.is_empty() {
            return MoltObject::from_bool(true).bits();
        }
        let delim_len = delim.len();
        let length = text.len();
        let mut idx = 0usize;
        while idx < length {
            let next_idx = text[idx..]
                .windows(delim_len)
                .position(|w| w == delim)
                .map(|p| p + idx);
            let Some(di) = next_idx else {
                return MoltObject::from_bool(true).bits();
            };
            let after = di + delim_len;
            if after > length - 1 {
                return MoltObject::from_bool(false).bits();
            }
            if text[after..].starts_with(delim) {
                idx = after + delim_len;
                continue;
            }
            if text[after] == b'{' {
                let brace_start = after + 1;
                let Some(brace_end_rel) = text[brace_start..].iter().position(|&b| b == b'}')
                else {
                    return MoltObject::from_bool(false).bits();
                };
                let brace_end = brace_start + brace_end_rel;
                let name_bytes = &text[brace_start..brace_end];
                if name_bytes.is_empty() || scan_identifier(name_bytes, 0).is_none() {
                    return MoltObject::from_bool(false).bits();
                }
                idx = brace_end + 1;
                continue;
            }
            if scan_identifier(text, after).is_none() {
                return MoltObject::from_bool(false).bits();
            }
            let (_, end) = scan_identifier(text, after).unwrap();
            idx = end;
        }
        MoltObject::from_bool(true).bits()
    })
}

/// Extract all $-variable identifiers from a template string.
/// Returns a list of unique identifier strings in order of first appearance.
#[unsafe(no_mangle)]
pub extern "C" fn molt_string_template_get_identifiers(
    template_bits: u64,
    delimiter_bits: u64,
) -> u64 {
    crate::with_gil_entry_nopanic!(_py, {
        let Some(template) = string_obj_to_owned(obj_from_bits(template_bits)) else {
            return raise_exception::<_>(_py, "TypeError", "template must be str");
        };
        let Some(delimiter) = string_obj_to_owned(obj_from_bits(delimiter_bits)) else {
            return raise_exception::<_>(_py, "TypeError", "delimiter must be str");
        };
        let text = template.as_bytes();
        let delim = delimiter.as_bytes();
        if delim.is_empty() {
            let list_ptr = alloc_list(_py, &[]);
            return MoltObject::from_ptr(list_ptr).bits();
        }
        let delim_len = delim.len();
        let length = text.len();
        let mut seen: Vec<Vec<u8>> = Vec::new();
        let mut result_bits: Vec<u64> = Vec::new();
        let mut idx = 0usize;
        while idx < length {
            let next_idx = text[idx..]
                .windows(delim_len)
                .position(|w| w == delim)
                .map(|p| p + idx);
            let Some(di) = next_idx else { break };
            let after = di + delim_len;
            if after > length - 1 {
                break;
            }
            if text[after..].starts_with(delim) {
                idx = after + delim_len;
                continue;
            }
            if text[after] == b'{' {
                let brace_start = after + 1;
                if let Some(brace_end_rel) = text[brace_start..].iter().position(|&b| b == b'}') {
                    let brace_end = brace_start + brace_end_rel;
                    let name = &text[brace_start..brace_end];
                    if !name.is_empty()
                        && scan_identifier(name, 0).is_some()
                        && !seen.iter().any(|s| s == name)
                    {
                        seen.push(name.to_vec());
                        let ptr = alloc_string(_py, name);
                        result_bits.push(MoltObject::from_ptr(ptr).bits());
                    }
                    idx = brace_end + 1;
                    continue;
                }
                idx = after;
                continue;
            }
            if let Some((start, end)) = scan_identifier(text, after) {
                let name = &text[start..end];
                if !seen.iter().any(|s| s == name) {
                    seen.push(name.to_vec());
                    let ptr = alloc_string(_py, name);
                    result_bits.push(MoltObject::from_ptr(ptr).bits());
                }
                idx = end;
                continue;
            }
            idx = after;
        }
        let list_ptr = alloc_list(_py, &result_bits);
        MoltObject::from_ptr(list_ptr).bits()
    })
}

// ─────────────────────────────────────────────────────────────────────────────
// Formatter parse / field name split
// ─────────────────────────────────────────────────────────────────────────────

/// Parse a format string into a list of 4-tuples:
/// (literal_text: str, field_name: str|None, format_spec: str|None, conversion: str|None)
///
/// This is equivalent to Python's `string.Formatter.parse()` / `_formatter_parser()`.
#[unsafe(no_mangle)]
pub extern "C" fn molt_string_formatter_parse(format_string_bits: u64) -> u64 {
    crate::with_gil_entry_nopanic!(_py, {
        let Some(format_string) = string_obj_to_owned(obj_from_bits(format_string_bits)) else {
            return raise_exception::<_>(_py, "TypeError", "format_string must be str");
        };
        let text = format_string.as_bytes();
        let length = text.len();
        let mut results: Vec<u64> = Vec::new();
        let mut idx = 0usize;
        let mut literal: Vec<u8> = Vec::new();
        let none = MoltObject::none().bits();

        while idx < length {
            let ch = text[idx];
            if ch == b'{' {
                if idx + 1 < length && text[idx + 1] == b'{' {
                    literal.push(b'{');
                    idx += 2;
                    continue;
                }
                // Emit literal + parse field.
                let lit_ptr = alloc_string(_py, &literal);
                literal.clear();
                idx += 1;
                if idx >= length {
                    return raise_exception::<_>(
                        _py,
                        "ValueError",
                        "Single '{' encountered in format string",
                    );
                }
                // Parse field: field_name, format_spec, conversion.
                let field_start = idx;
                let mut bracket_depth = 0i32;
                while idx < length {
                    let c = text[idx];
                    if c == b'[' {
                        bracket_depth += 1;
                        idx += 1;
                        continue;
                    }
                    if c == b']' && bracket_depth > 0 {
                        bracket_depth -= 1;
                        idx += 1;
                        continue;
                    }
                    if bracket_depth == 0 && (c == b'!' || c == b':' || c == b'}') {
                        break;
                    }
                    idx += 1;
                }
                if idx >= length {
                    return raise_exception::<_>(
                        _py,
                        "ValueError",
                        "expected '}' before end of string",
                    );
                }
                let field_name = &text[field_start..idx];
                let field_name_ptr = alloc_string(_py, field_name);

                // Conversion.
                let mut conversion_bits = none;
                if text[idx] == b'!' {
                    if idx + 1 >= length {
                        return raise_exception::<_>(
                            _py,
                            "ValueError",
                            "unmatched '{' in format spec",
                        );
                    }
                    let conv = &text[idx + 1..idx + 2];
                    let conv_ptr = alloc_string(_py, conv);
                    conversion_bits = MoltObject::from_ptr(conv_ptr).bits();
                    idx += 2;
                    if idx >= length || (text[idx] != b':' && text[idx] != b'}') {
                        return raise_exception::<_>(
                            _py,
                            "ValueError",
                            "expected ':' after conversion specifier",
                        );
                    }
                }

                // Format spec.
                let format_spec_bits = if text[idx] == b':' {
                    idx += 1;
                    let spec_start = idx;
                    let mut nested = 0i32;
                    while idx < length {
                        let c = text[idx];
                        if c == b'{' {
                            if idx + 1 < length && text[idx + 1] == b'{' {
                                idx += 2;
                                continue;
                            }
                            nested += 1;
                            idx += 1;
                            continue;
                        }
                        if c == b'}' {
                            if idx + 1 < length && text[idx + 1] == b'}' {
                                idx += 2;
                                continue;
                            }
                            if nested == 0 {
                                break;
                            }
                            nested -= 1;
                            idx += 1;
                            continue;
                        }
                        idx += 1;
                    }
                    if idx >= length {
                        return raise_exception::<_>(
                            _py,
                            "ValueError",
                            "unmatched '{' in format spec",
                        );
                    }
                    let spec = &text[spec_start..idx];
                    let spec_ptr = alloc_string(_py, spec);
                    MoltObject::from_ptr(spec_ptr).bits()
                } else {
                    let empty_ptr = alloc_string(_py, b"");
                    MoltObject::from_ptr(empty_ptr).bits()
                };

                if idx >= length || text[idx] != b'}' {
                    return raise_exception::<_>(
                        _py,
                        "ValueError",
                        "expected '}' before end of string",
                    );
                }
                idx += 1;

                let tup = alloc_tuple(
                    _py,
                    &[
                        MoltObject::from_ptr(lit_ptr).bits(),
                        MoltObject::from_ptr(field_name_ptr).bits(),
                        format_spec_bits,
                        conversion_bits,
                    ],
                );
                results.push(MoltObject::from_ptr(tup).bits());
                continue;
            }

            if ch == b'}' {
                if idx + 1 < length && text[idx + 1] == b'}' {
                    literal.push(b'}');
                    idx += 2;
                    continue;
                }
                return raise_exception::<_>(
                    _py,
                    "ValueError",
                    "Single '}' encountered in format string",
                );
            }

            literal.push(ch);
            idx += 1;
        }

        // Final literal segment.
        if !literal.is_empty() || results.is_empty() {
            let lit_ptr = alloc_string(_py, &literal);
            let tup = alloc_tuple(
                _py,
                &[MoltObject::from_ptr(lit_ptr).bits(), none, none, none],
            );
            results.push(MoltObject::from_ptr(tup).bits());
        }

        let list_ptr = alloc_list(_py, &results);
        MoltObject::from_ptr(list_ptr).bits()
    })
}

/// Split a format field name into (first, rest).
///
/// `first` is either a str (attribute name) or int (positional index).
/// `rest` is a list of (is_attr: bool, key: str|int) tuples.
///
/// Example: "0.name[2]" → (0, [(True, "name"), (False, 2)])
#[unsafe(no_mangle)]
pub extern "C" fn molt_string_formatter_field_name_split(field_name_bits: u64) -> u64 {
    crate::with_gil_entry_nopanic!(_py, {
        let Some(field_name) = string_obj_to_owned(obj_from_bits(field_name_bits)) else {
            return raise_exception::<_>(_py, "TypeError", "field_name must be str");
        };
        let text = field_name.as_bytes();
        if text.is_empty() {
            let empty_ptr = alloc_string(_py, b"");
            let rest_ptr = alloc_list(_py, &[]);
            let tup = alloc_tuple(
                _py,
                &[
                    MoltObject::from_ptr(empty_ptr).bits(),
                    MoltObject::from_ptr(rest_ptr).bits(),
                ],
            );
            return MoltObject::from_ptr(tup).bits();
        }

        // Parse first component (up to '.' or '[').
        let mut end = 0usize;
        while end < text.len() && text[end] != b'.' && text[end] != b'[' {
            end += 1;
        }
        let first_bytes = &text[..end];
        let first_bits =
            if first_bytes.iter().all(|b| b.is_ascii_digit()) && !first_bytes.is_empty() {
                let val: i64 = std::str::from_utf8(first_bytes)
                    .unwrap_or("0")
                    .parse()
                    .unwrap_or(0);
                MoltObject::from_int(val).bits()
            } else {
                let ptr = alloc_string(_py, first_bytes);
                MoltObject::from_ptr(ptr).bits()
            };

        // Parse rest.
        let mut rest_items: Vec<u64> = Vec::new();
        let mut idx = end;
        while idx < text.len() {
            if text[idx] == b'.' {
                idx += 1;
                let start = idx;
                while idx < text.len() && text[idx] != b'.' && text[idx] != b'[' {
                    idx += 1;
                }
                let attr_name = &text[start..idx];
                let attr_ptr = alloc_string(_py, attr_name);
                let tup = alloc_tuple(
                    _py,
                    &[
                        MoltObject::from_bool(true).bits(),
                        MoltObject::from_ptr(attr_ptr).bits(),
                    ],
                );
                rest_items.push(MoltObject::from_ptr(tup).bits());
                continue;
            }
            if text[idx] == b'[' {
                idx += 1;
                let start = idx;
                while idx < text.len() && text[idx] != b']' {
                    idx += 1;
                }
                if idx >= text.len() {
                    return raise_exception::<_>(
                        _py,
                        "ValueError",
                        "expected ']' before end of string",
                    );
                }
                let key_bytes = &text[start..idx];
                let key_bits =
                    if key_bytes.iter().all(|b| b.is_ascii_digit()) && !key_bytes.is_empty() {
                        let val: i64 = std::str::from_utf8(key_bytes)
                            .unwrap_or("0")
                            .parse()
                            .unwrap_or(0);
                        MoltObject::from_int(val).bits()
                    } else {
                        let ptr = alloc_string(_py, key_bytes);
                        MoltObject::from_ptr(ptr).bits()
                    };
                let tup = alloc_tuple(_py, &[MoltObject::from_bool(false).bits(), key_bits]);
                rest_items.push(MoltObject::from_ptr(tup).bits());
                idx += 1; // skip ']'
                continue;
            }
            break;
        }

        let rest_ptr = alloc_list(_py, &rest_items);
        let result = alloc_tuple(_py, &[first_bits, MoltObject::from_ptr(rest_ptr).bits()]);
        MoltObject::from_ptr(result).bits()
    })
}
