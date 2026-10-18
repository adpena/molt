// Regex group-value and replacement-expansion entry points (re module support).
//
// `molt_re_group_values` turns captured spans into group strings and
// `molt_re_expand_replacement` expands a replacement template against them.
// The bridge supplies the runtime primitives (`crate::bridge::*` via
// `super::*` and `molt_runtime_core::prelude`).

use super::*;

// Re flag constants are provided by `common.rs` (re-exported through the
// parent `regex` module) — `RE_IGNORECASE`, `RE_DOTALL`, `RE_MULTILINE`,
// `RE_ASCII` — so they are not redeclared here.

/// Replacement errors carry exactly the data required by their exception role.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) enum ReplacementError {
    Syntax(String),
    Index(String),
    InvalidGroupName { name: Vec<u32>, position: usize },
}

/// Expand both text and byte replacement templates through one parser. Units
/// are Unicode code points (including surrogates) for text and byte values for
/// byte patterns.
pub(super) fn re_expand_typed(
    repl: &[u32],
    group_values: &[Option<Vec<u32>>],
    group_names: &NamedGroups,
) -> Result<Vec<u32>, ReplacementError> {
    let mut out = Vec::with_capacity(repl.len());
    let mut i = 0;
    while i < repl.len() {
        if repl[i] != u32::from(b'\\') {
            out.push(repl[i]);
            i += 1;
            continue;
        }
        if i + 1 >= repl.len() {
            return Err(ReplacementError::Syntax(format!(
                "bad escape (end of pattern) at position {i}"
            )));
        }
        let ch = repl[i + 1];
        if ch == u32::from(b'g') && repl.get(i + 2) == Some(&u32::from(b'<')) {
            let Some(end_rel) = repl[i + 3..]
                .iter()
                .position(|unit| *unit == u32::from(b'>'))
            else {
                return Err(ReplacementError::Syntax(format!(
                    "missing >, unterminated name at position {i}"
                )));
            };
            let end = i + 3 + end_rel;
            let name_units = &repl[i + 3..end];
            if name_units
                .iter()
                .any(|unit| (0xD800..=0xDFFF).contains(unit))
            {
                return Err(ReplacementError::InvalidGroupName {
                    name: name_units.to_vec(),
                    position: i + 3,
                });
            }
            let name: String = name_units
                .iter()
                .map(|unit| char::from_u32(*unit).expect("validated replacement code point"))
                .collect();
            let index = if !name.is_empty() && name.bytes().all(|byte| byte.is_ascii_digit()) {
                name.parse::<usize>().map_err(|_| {
                    ReplacementError::Syntax(format!(
                        "invalid group reference {name} at position {}",
                        i + 3
                    ))
                })?
            } else {
                *group_names.get(&name).ok_or_else(|| {
                    ReplacementError::Index(format!("unknown group name '{name}'"))
                })? as usize
            };
            append_group(&mut out, group_values, index, i + 3)?;
            i = end + 1;
            continue;
        }
        if (u32::from(b'0')..=u32::from(b'9')).contains(&ch) {
            if ch == u32::from(b'0')
                || (i + 3 < repl.len()
                    && repl[i + 1..i + 4]
                        .iter()
                        .all(|unit| (u32::from(b'0')..=u32::from(b'7')).contains(unit)))
            {
                let end = if ch == u32::from(b'0') {
                    let mut end = i + 2;
                    while end < (i + 4).min(repl.len())
                        && (u32::from(b'0')..=u32::from(b'7')).contains(&repl[end])
                    {
                        end += 1;
                    }
                    end
                } else {
                    i + 4
                };
                let mut value = 0u32;
                for unit in &repl[i + 1..end] {
                    value = value * 8 + (*unit - u32::from(b'0'));
                }
                if value > 0o377 {
                    let digits: String = repl[i + 1..end]
                        .iter()
                        .map(|unit| char::from_u32(*unit).unwrap())
                        .collect();
                    return Err(ReplacementError::Syntax(format!(
                        "octal escape value \\{digits} outside of range 0-0o377 at position {i}"
                    )));
                }
                out.push(value);
                i = end;
                continue;
            }
            let mut end = i + 2;
            if end < repl.len() && (u32::from(b'0')..=u32::from(b'9')).contains(&repl[end]) {
                end += 1;
            }
            let digits: String = repl[i + 1..end]
                .iter()
                .map(|unit| char::from_u32(*unit).unwrap_or('\u{fffd}'))
                .collect();
            let index = digits.parse::<usize>().map_err(|_| {
                ReplacementError::Syntax(format!(
                    "invalid group reference {digits} at position {}",
                    i + 1
                ))
            })?;
            append_group(&mut out, group_values, index, i + 1)?;
            i = end;
            continue;
        }
        let escaped = match ch {
            x if x == u32::from(b'a') => Some(7),
            x if x == u32::from(b'b') => Some(8),
            x if x == u32::from(b'f') => Some(12),
            x if x == u32::from(b'n') => Some(10),
            x if x == u32::from(b'r') => Some(13),
            x if x == u32::from(b't') => Some(9),
            x if x == u32::from(b'v') => Some(11),
            x if x == u32::from(b'\\') => Some(u32::from(b'\\')),
            _ => None,
        };
        if let Some(value) = escaped {
            out.push(value);
        } else if char::from_u32(ch).is_some_and(|c| c.is_ascii_alphabetic()) {
            return Err(ReplacementError::Syntax(format!(
                "bad escape \\{} at position {i}",
                char::from_u32(ch).unwrap()
            )));
        } else {
            out.extend([u32::from(b'\\'), ch]);
        }
        i += 2;
    }
    Ok(out)
}

pub(super) fn string_codepoints(_py: &CoreGilToken, bits: u64) -> Option<Vec<u32>> {
    // SAFETY: the caller holds the core GIL and the original runtime string is
    // retained by the argument/sequence snapshot for this entire operation.
    let bytes = unsafe { rt_string_as_bytes(bits) }?;
    // SAFETY: runtime strings are admitted only after Molt string storage has
    // validated its UTF-8/WTF-8 encoding.
    Some(
        unsafe { molt_stdlib_text::wtf8::from_validated_bytes(bytes) }
            .code_points()
            .map(|point| point.to_u32())
            .collect(),
    )
}

fn string_name(_py: &CoreGilToken, bits: u64) -> Option<String> {
    // Group names are parser-validated identifiers and therefore have a strict
    // UTF-8 view even though ordinary runtime strings can contain surrogates.
    // SAFETY: the caller holds the core GIL and the dictionary snapshot keeps
    // each key live during this read.
    let bytes = unsafe { rt_string_as_bytes(bits) }?;
    // SAFETY: these bytes are runtime string storage; group-name validation
    // additionally guarantees there are no surrogate code points.
    unsafe { molt_stdlib_text::wtf8::from_validated_bytes(bytes) }
        .as_str()
        .map(str::to_owned)
}

/// Return a private `(None, None, message)` marker so the Python facade raises
/// its actual `re.error` class. Builtin IndexError remains in Rust's authority.
pub(super) fn replacement_error_result(py: &CoreGilToken, error: ReplacementError) -> u64 {
    let message = match error {
        ReplacementError::Index(message) => {
            return raise_exception::<u64>(py, "IndexError", &message);
        }
        ReplacementError::InvalidGroupName { name, position } => {
            let mut encoded_name = Vec::with_capacity(name.len().saturating_mul(3));
            for unit in name {
                molt_stdlib_text::wtf8::push_wtf8_codepoint(&mut encoded_name, unit);
            }
            let name_ptr = alloc_string(py, &encoded_name);
            if name_ptr.is_null() {
                return MoltObject::none().bits();
            }
            let name = unsafe {
                OwnedRuntimeValue::from_owned_bits(py, MoltObject::from_ptr(name_ptr).bits())
            };
            let repr_bits = rt_repr(name.bits());
            if rt_exception_pending() {
                return MoltObject::none().bits();
            }
            let repr = unsafe { OwnedRuntimeValue::from_owned_bits(py, repr_bits) };
            let Some(repr_bytes) = repr.string_bytes(py) else {
                return raise_exception::<u64>(py, "SystemError", "repr returned a non-string");
            };
            let Ok(repr_text) = std::str::from_utf8(repr_bytes) else {
                return raise_exception::<u64>(py, "SystemError", "repr returned invalid UTF-8");
            };
            format!("bad character in group name {repr_text} at position {position}")
        }
        ReplacementError::Syntax(message) => message,
    };
    let message_ptr = alloc_string(py, message.as_bytes());
    if message_ptr.is_null() {
        return MoltObject::none().bits();
    }
    let message_bits = MoltObject::from_ptr(message_ptr).bits();
    let tuple_ptr = alloc_tuple(
        py,
        &[
            MoltObject::none().bits(),
            MoltObject::none().bits(),
            message_bits,
        ],
    );
    dec_ref_bits(py, message_bits);
    if tuple_ptr.is_null() {
        MoltObject::none().bits()
    } else {
        MoltObject::from_ptr(tuple_ptr).bits()
    }
}

pub(super) fn syntax_error_result(py: &CoreGilToken, message: &str) -> u64 {
    replacement_error_result(py, ReplacementError::Syntax(message.to_owned()))
}

fn append_group(
    out: &mut Vec<u32>,
    groups: &[Option<Vec<u32>>],
    index: usize,
    position: usize,
) -> Result<(), ReplacementError> {
    let Some(value) = groups.get(index) else {
        return Err(ReplacementError::Syntax(format!(
            "invalid group reference {index} at position {position}"
        )));
    };
    if let Some(value) = value {
        out.extend_from_slice(value);
    }
    Ok(())
}

pub(super) fn re_group_spans_from_sequence(
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
    let elems = unsafe { seq_snapshot(groups_ptr) };
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
                "group span must be tuple[int, int] or None",
            ));
        };
        let elem_ty = unsafe { object_type_id(elem_ptr) };
        if elem_ty != TYPE_ID_LIST && elem_ty != TYPE_ID_TUPLE {
            return Err(raise_exception::<u64>(
                _py,
                "TypeError",
                "group span must be tuple[int, int] or None",
            ));
        }
        let span = unsafe { seq_snapshot(elem_ptr) };
        if span.len() < 2 {
            return Err(raise_exception::<u64>(
                _py,
                "TypeError",
                "group span must contain start/end",
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

// Runtime regex ABI entrypoints.
#[unsafe(no_mangle)]
pub extern "C" fn molt_re_group_values(text_bits: u64, groups_bits: u64) -> u64 {
    molt_runtime_core::with_core_gil!(_py, {
        let spans = match re_group_spans_from_sequence(_py, groups_bits) {
            Ok(value) => value,
            Err(err_bits) => return err_bits,
        };
        let mut values = Vec::with_capacity(spans.len());
        for span in spans {
            let value_bits = if let Some((start, end)) = span {
                if start < 0 || end < start {
                    return raise_exception::<_>(_py, "IndexError", "invalid group span");
                }
                let bits = subject_slice_to_object(text_bits, start as usize, end as usize);
                if obj_from_bits(bits).is_none() {
                    // The root bridge owns the admission/error authority.
                    // Preserve exporter and allocation failures unchanged.
                    return bits;
                }
                unsafe { molt_runtime_core::OwnedRuntimeValue::from_owned_bits(_py, bits) }
            } else {
                unsafe {
                    molt_runtime_core::OwnedRuntimeValue::from_owned_bits(
                        _py,
                        MoltObject::none().bits(),
                    )
                }
            };
            values.push(value_bits);
        }
        let ptr = alloc_tuple(
            _py,
            molt_runtime_core::OwnedRuntimeValue::as_bits_slice(&values),
        );
        if ptr.is_null() {
            MoltObject::none().bits()
        } else {
            MoltObject::from_ptr(ptr).bits()
        }
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_re_expand_replacement(
    repl_bits: u64,
    group_values_bits: u64,
    group_names_bits: u64,
) -> u64 {
    molt_runtime_core::with_core_gil!(_py, {
        let repl_obj = obj_from_bits(repl_bits);
        let Some(repl_ptr) = repl_obj.as_ptr() else {
            return raise_exception::<_>(_py, "TypeError", "replacement must be str or bytes");
        };
        let is_bytes = unsafe { object_type_id(repl_ptr) } == TYPE_ID_BYTES;
        let repl_units: Vec<u32> = if is_bytes {
            let Some(bytes) = (unsafe { bytes_obj_to_borrowed(repl_obj) }) else {
                return raise_exception::<_>(_py, "TypeError", "replacement must be bytes");
            };
            bytes.iter().map(|byte| u32::from(*byte)).collect()
        } else {
            let Some(units) = string_codepoints(_py, repl_bits) else {
                return raise_exception::<_>(_py, "TypeError", "replacement must be str or bytes");
            };
            units
        };
        let values_obj = obj_from_bits(group_values_bits);
        let Some(values_ptr) = values_obj.as_ptr() else {
            return raise_exception::<_>(_py, "TypeError", "group_values must be a sequence");
        };
        let values_ty = unsafe { object_type_id(values_ptr) };
        if values_ty != TYPE_ID_LIST && values_ty != TYPE_ID_TUPLE {
            return raise_exception::<_>(_py, "TypeError", "group_values must be a sequence");
        }
        let mut group_values = Vec::new();
        for &value_bits in unsafe { seq_snapshot(values_ptr) }.iter() {
            let value = obj_from_bits(value_bits);
            if value.is_none() {
                group_values.push(None);
            } else if is_bytes {
                let Some(bytes) = (unsafe { bytes_obj_to_borrowed(value) }) else {
                    return raise_exception::<_>(
                        _py,
                        "TypeError",
                        "group_values must match replacement type",
                    );
                };
                group_values.push(Some(bytes.iter().map(|byte| u32::from(*byte)).collect()));
            } else {
                let Some(units) = string_codepoints(_py, value_bits) else {
                    return raise_exception::<_>(
                        _py,
                        "TypeError",
                        "group_values must match replacement type",
                    );
                };
                group_values.push(Some(units));
            }
        }
        let mut group_names = NamedGroups::new();
        if let Some(names_ptr) = obj_from_bits(group_names_bits).as_ptr() {
            if unsafe { object_type_id(names_ptr) } == TYPE_ID_DICT {
                if let Some(pairs) = unsafe { dict_snapshot(_py, names_ptr) } {
                    for pair in pairs.chunks(2) {
                        if pair.len() != 2 {
                            continue;
                        }
                        let Some(name) = string_name(_py, pair[0]) else {
                            continue;
                        };
                        let Some(index) = to_i64(obj_from_bits(pair[1])) else {
                            continue;
                        };
                        if let Ok(index) = u32::try_from(index) {
                            group_names.insert(name, index);
                        }
                    }
                }
            }
        }
        let expanded = match re_expand_typed(&repl_units, &group_values, &group_names) {
            Ok(value) => value,
            Err(error) => return replacement_error_result(_py, error),
        };
        let out_ptr = if is_bytes {
            if expanded.iter().any(|unit| *unit > 255) {
                return raise_exception::<_>(
                    _py,
                    "ValueError",
                    "byte replacement produced a non-byte value",
                );
            }
            let bytes: Vec<u8> = expanded.into_iter().map(|unit| unit as u8).collect();
            alloc_bytes(_py, &bytes)
        } else {
            let mut bytes = Vec::with_capacity(expanded.len().saturating_mul(2));
            for unit in expanded {
                molt_stdlib_text::wtf8::push_wtf8_codepoint(&mut bytes, unit);
            }
            alloc_string(_py, &bytes)
        };
        if out_ptr.is_null() {
            MoltObject::none().bits()
        } else {
            MoltObject::from_ptr(out_ptr).bits()
        }
    })
}
