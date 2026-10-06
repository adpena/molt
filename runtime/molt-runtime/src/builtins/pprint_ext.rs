use std::collections::HashSet;

use crate::object::ops_format::{
    format_class_name_bytes, format_native_repr_override_bytes, format_obj_bytes,
    snapshot_format_inputs,
};
use crate::*;

fn text(parts: &[&[u8]]) -> Vec<u8> {
    parts.concat()
}
fn text_len(bytes: &[u8]) -> usize {
    crate::object::ops_string::wtf8_from_bytes(bytes)
        .code_points()
        .count()
}

// ─── helpers ────────────────────────────────────────────────────────────────

pub(crate) fn i64_from_bits_default(bits: u64, default: i64) -> i64 {
    let obj = obj_from_bits(bits);
    if obj.is_none() {
        return default;
    }
    if let Some(i) = to_i64(obj) {
        return i;
    }
    default
}

fn bool_from_bits_default(bits: u64, default: bool) -> bool {
    let obj = obj_from_bits(bits);
    if obj.is_none() {
        return default;
    }
    if let Some(i) = to_i64(obj) {
        return i != 0;
    }
    default
}

fn alloc_string_result(_py: &PyToken<'_>, s: &[u8]) -> u64 {
    if exception_pending(_py) {
        return MoltObject::none().bits();
    }
    let ptr = alloc_string(_py, s);
    if ptr.is_null() {
        return raise_exception::<_>(_py, "MemoryError", "out of memory");
    }
    MoltObject::from_ptr(ptr).bits()
}

// ─── repr formatting engine ─────────────────────────────────────────────────

/// Internal recursive repr generator that tracks object IDs to detect cycles
/// and respects max_depth and max_width constraints.
pub(crate) fn safe_repr_inner(
    _py: &PyToken<'_>,
    bits: u64,
    seen: &mut HashSet<u64>,
    depth: i64,
    max_depth: i64,
    max_width: i64,
) -> (Vec<u8>, bool, bool) {
    // readable = true if the repr can be eval'd back
    // recursive = true if we detected a cycle

    if exception_pending(_py) {
        return (Vec::new(), false, false);
    }
    let obj = obj_from_bits(bits);

    // None and immediates
    if obj.is_none()
        || obj.as_bool().is_some()
        || obj.as_int().is_some()
        || obj.as_float().is_some()
    {
        return (format_obj_bytes(_py, obj), true, false);
    }

    let Some(ptr) = obj.as_ptr() else {
        return ("None".as_bytes().to_vec(), true, false);
    };

    let type_id = unsafe { object_type_id(ptr) };

    if let Some(repr) = format_native_repr_override_bytes(_py, obj) {
        let readable = !repr.is_empty() && !repr.starts_with(b"<");
        return (repr, readable, false);
    }

    // Strings, bytes — use runtime repr
    // Note: None, bool, int, float are NaN-boxed and handled before as_ptr().
    match type_id {
        TYPE_ID_STRING | TYPE_ID_BYTES => {
            let repr = format_obj_bytes(_py, obj);
            return (repr, true, false);
        }
        _ => {}
    }

    // Check depth limit
    if max_depth > 0 && depth >= max_depth {
        match type_id {
            TYPE_ID_LIST => return ("[...]".as_bytes().to_vec(), false, false),
            TYPE_ID_TUPLE => return ("(...)".as_bytes().to_vec(), false, false),
            TYPE_ID_DICT => return ("{...}".as_bytes().to_vec(), false, false),
            TYPE_ID_SET => return ("{...}".as_bytes().to_vec(), false, false),
            TYPE_ID_FROZENSET => return ("frozenset({...})".as_bytes().to_vec(), false, false),
            _ => {}
        }
    }

    // Cycle detection for container types
    let is_container = matches!(
        type_id,
        TYPE_ID_LIST | TYPE_ID_TUPLE | TYPE_ID_DICT | TYPE_ID_SET | TYPE_ID_FROZENSET
    );

    if is_container {
        if seen.contains(&bits) {
            let type_label = format_class_name_bytes(type_of_bits(_py, bits));
            return (
                text(&[
                    b"<Recursion on ",
                    &type_label,
                    format!(" with id={bits}>").as_bytes(),
                ]),
                false,
                true,
            );
        }
        seen.insert(bits);
    }

    let result = match type_id {
        TYPE_ID_LIST => {
            let len = unsafe { crate::object::seq_access::locked_len(ptr) };
            if len == 0 {
                ("[]".as_bytes().to_vec(), true, false)
            } else {
                let mut readable = true;
                let mut recursive = false;
                let mut parts = Vec::with_capacity(len);
                let display_len = if max_width > 0 && (len as i64) > max_width {
                    max_width as usize
                } else {
                    len
                };
                for index in 0..display_len {
                    let Some(item) =
                        (unsafe { crate::object::seq_access::pin_item(_py, ptr, index) })
                    else {
                        break;
                    };
                    let (s, r, rec) =
                        safe_repr_inner(_py, item.bits(), seen, depth + 1, max_depth, max_width);
                    if !r {
                        readable = false;
                    }
                    if rec {
                        recursive = true;
                    }
                    parts.push(s);
                }
                if display_len < len {
                    parts.push("...".as_bytes().to_vec());
                    readable = false;
                }
                (
                    text(&[b"[", &parts.join(b", ".as_slice()), b"]"]),
                    readable,
                    recursive,
                )
            }
        }
        TYPE_ID_TUPLE => {
            let len = unsafe { crate::object::seq_access::locked_len(ptr) };
            if len == 0 {
                ("()".as_bytes().to_vec(), true, false)
            } else {
                let mut readable = true;
                let mut recursive = false;
                let mut parts = Vec::with_capacity(len);
                let display_len = if max_width > 0 && (len as i64) > max_width {
                    max_width as usize
                } else {
                    len
                };
                for index in 0..display_len {
                    let Some(item) =
                        (unsafe { crate::object::seq_access::pin_item(_py, ptr, index) })
                    else {
                        break;
                    };
                    let (s, r, rec) =
                        safe_repr_inner(_py, item.bits(), seen, depth + 1, max_depth, max_width);
                    if !r {
                        readable = false;
                    }
                    if rec {
                        recursive = true;
                    }
                    parts.push(s);
                }
                if display_len < len {
                    parts.push("...".as_bytes().to_vec());
                    readable = false;
                }
                if len == 1 && display_len == 1 {
                    (text(&[b"(", &parts[0], b",)"]), readable, recursive)
                } else {
                    (
                        text(&[b"(", &parts.join(b", ".as_slice()), b")"]),
                        readable,
                        recursive,
                    )
                }
            }
        }
        TYPE_ID_DICT => {
            let Some(order) = (unsafe {
                crate::object::ops_dict::dict_snapshot(
                    _py,
                    ptr,
                    crate::object::ops_dict::DictSnapshotKind::Entries,
                )
            }) else {
                seen.remove(&bits);
                return (Vec::new(), false, false);
            };
            let num_pairs = order.len() / 2;
            if num_pairs == 0 {
                ("{}".as_bytes().to_vec(), true, false)
            } else {
                let mut readable = true;
                let mut recursive = false;
                let mut parts = Vec::with_capacity(num_pairs);
                let display_len = if max_width > 0 && (num_pairs as i64) > max_width {
                    max_width as usize
                } else {
                    num_pairs
                };
                // Collect pairs for sorting
                let mut pairs: Vec<(u64, u64)> = Vec::with_capacity(num_pairs);
                let mut i = 0;
                while i + 1 < order.len() {
                    pairs.push((order[i], order[i + 1]));
                    i += 2;
                }
                // Sort by key repr for deterministic output
                pairs.sort_by(|a, b| {
                    if exception_pending(_py) {
                        return std::cmp::Ordering::Equal;
                    }
                    let ka = format_obj_bytes(_py, obj_from_bits(a.0));
                    if exception_pending(_py) {
                        return std::cmp::Ordering::Equal;
                    }
                    let kb = format_obj_bytes(_py, obj_from_bits(b.0));
                    ka.cmp(&kb)
                });
                for &(key_bits, val_bits) in pairs.iter().take(display_len) {
                    let (ks, kr, krec) =
                        safe_repr_inner(_py, key_bits, seen, depth + 1, max_depth, max_width);
                    let (vs, vr, vrec) =
                        safe_repr_inner(_py, val_bits, seen, depth + 1, max_depth, max_width);
                    if !kr || !vr {
                        readable = false;
                    }
                    if krec || vrec {
                        recursive = true;
                    }
                    parts.push(text(&[&ks, b": ", &vs]));
                }
                if display_len < num_pairs {
                    parts.push("...".as_bytes().to_vec());
                    readable = false;
                }
                (
                    text(&[b"{", &parts.join(b", ".as_slice()), b"}"]),
                    readable,
                    recursive,
                )
            }
        }
        TYPE_ID_SET => {
            let Some(order) = snapshot_format_inputs(_py, unsafe { set_order(ptr) }) else {
                seen.remove(&bits);
                return (Vec::new(), false, false);
            };
            if order.is_empty() {
                ("set()".as_bytes().to_vec(), true, false)
            } else {
                let readable = true;
                let recursive = false;
                let mut repr_elems: Vec<Vec<u8>> = order
                    .iter()
                    .map(|&e| {
                        let (s, _, _) =
                            safe_repr_inner(_py, e, seen, depth + 1, max_depth, max_width);
                        s
                    })
                    .collect();
                repr_elems.sort();
                (
                    text(&[b"{", &repr_elems.join(b", ".as_slice()), b"}"]),
                    readable,
                    recursive,
                )
            }
        }
        TYPE_ID_FROZENSET => {
            let Some(order) = snapshot_format_inputs(_py, unsafe { set_order(ptr) }) else {
                seen.remove(&bits);
                return (Vec::new(), false, false);
            };
            if order.is_empty() {
                ("frozenset()".as_bytes().to_vec(), true, false)
            } else {
                let readable = true;
                let recursive = false;
                let mut repr_elems: Vec<Vec<u8>> = order
                    .iter()
                    .map(|&e| {
                        let (s, _, _) =
                            safe_repr_inner(_py, e, seen, depth + 1, max_depth, max_width);
                        s
                    })
                    .collect();
                repr_elems.sort();
                (
                    text(&[b"frozenset({", &repr_elems.join(b", ".as_slice()), b"})"]),
                    readable,
                    recursive,
                )
            }
        }
        _ => {
            // Fall back to the runtime repr for other types
            let repr = format_obj_bytes(_py, obj);
            let readable = !repr.is_empty() && !repr.starts_with(b"<");
            (repr, readable, false)
        }
    };

    if is_container {
        seen.remove(&bits);
    }

    result
}

// ─── pformat engine ─────────────────────────────────────────────────────────

#[derive(Clone, Copy)]
struct PformatConfig {
    indent_per_level: i64,
    width: i64,
    max_depth: i64,
    compact: bool,
    sort_dicts: bool,
    underscore_numbers: bool,
}

/// Full pformat implementation matching CPython's pprint.pformat behavior.
fn pformat_impl(_py: &PyToken<'_>, bits: u64, config: PformatConfig) -> Vec<u8> {
    let mut seen = HashSet::new();
    pformat_recursive(_py, bits, &mut seen, 0, 0, config)
}

fn pformat_recursive(
    _py: &PyToken<'_>,
    bits: u64,
    seen: &mut HashSet<u64>,
    current_indent: i64,
    level: i64,
    config: PformatConfig,
) -> Vec<u8> {
    if exception_pending(_py) {
        return Vec::new();
    }
    let obj = obj_from_bits(bits);

    // Simple scalars
    if obj.is_none() || obj.as_bool().is_some() || obj.as_float().is_some() {
        return format_obj_bytes(_py, obj);
    }
    if let Some(i) = obj.as_int() {
        if config.underscore_numbers {
            return format_int_underscored(i).into_bytes();
        }
        return format!("{}", i).into_bytes();
    }

    let Some(ptr) = obj.as_ptr() else {
        return "None".as_bytes().to_vec();
    };

    let type_id = unsafe { object_type_id(ptr) };

    if let Some(repr) = format_native_repr_override_bytes(_py, obj) {
        return repr;
    }

    // Scalars — Note: None, bool, int, float are NaN-boxed and handled before as_ptr().
    match type_id {
        TYPE_ID_STRING | TYPE_ID_BYTES => {
            return format_obj_bytes(_py, obj);
        }
        _ => {}
    }

    // Depth check
    if config.max_depth > 0 && level >= config.max_depth {
        match type_id {
            TYPE_ID_LIST => return "[...]".as_bytes().to_vec(),
            TYPE_ID_TUPLE => return "(...)".as_bytes().to_vec(),
            TYPE_ID_DICT => return "{...}".as_bytes().to_vec(),
            _ => {}
        }
    }

    // Cycle check
    let is_container = matches!(
        type_id,
        TYPE_ID_LIST | TYPE_ID_TUPLE | TYPE_ID_DICT | TYPE_ID_SET | TYPE_ID_FROZENSET
    );
    if is_container && seen.contains(&bits) {
        let type_label = format_class_name_bytes(type_of_bits(_py, bits));
        return text(&[
            b"<Recursion on ",
            &type_label,
            format!(" with id={bits}>").as_bytes(),
        ]);
    }
    if is_container {
        seen.insert(bits);
    }

    // Try simple single-line repr first
    let simple = {
        let mut temp_seen = seen.clone();
        let (s, _, _) = safe_repr_inner(_py, bits, &mut temp_seen, level, config.max_depth, -1);
        s
    };

    let available = config.width - current_indent;
    if (text_len(&simple) as i64) <= available {
        if is_container {
            seen.remove(&bits);
        }
        return simple;
    }

    // Multi-line formatting for containers
    let result = match type_id {
        TYPE_ID_DICT => {
            let Some(order) = (unsafe {
                crate::object::ops_dict::dict_snapshot(
                    _py,
                    ptr,
                    crate::object::ops_dict::DictSnapshotKind::Entries,
                )
            }) else {
                seen.remove(&bits);
                return Vec::new();
            };
            let num_pairs = order.len() / 2;
            if num_pairs == 0 {
                "{}".as_bytes().to_vec()
            } else {
                let child_indent = current_indent + config.indent_per_level;
                let indent_str = " ".repeat(child_indent as usize);
                // Collect pairs
                let mut pairs: Vec<(u64, u64)> = Vec::with_capacity(num_pairs);
                let mut i = 0;
                while i + 1 < order.len() {
                    pairs.push((order[i], order[i + 1]));
                    i += 2;
                }
                if config.sort_dicts {
                    pairs.sort_by(|a, b| {
                        if exception_pending(_py) {
                            return std::cmp::Ordering::Equal;
                        }
                        let ka = format_obj_bytes(_py, obj_from_bits(a.0));
                        if exception_pending(_py) {
                            return std::cmp::Ordering::Equal;
                        }
                        let kb = format_obj_bytes(_py, obj_from_bits(b.0));
                        ka.cmp(&kb)
                    });
                }
                let mut parts = Vec::with_capacity(pairs.len());
                for (key_bits, val_bits) in &pairs {
                    let key_repr =
                        pformat_recursive(_py, *key_bits, seen, child_indent, level + 1, config);
                    let val_repr = pformat_recursive(
                        _py,
                        *val_bits,
                        seen,
                        child_indent + text_len(&key_repr) as i64 + 2,
                        level + 1,
                        config,
                    );
                    parts.push(text(&[&key_repr, b": ", &val_repr]));
                }
                let prefix = if config.indent_per_level > 1 {
                    format!("{{{}", " ".repeat((config.indent_per_level - 1) as usize)).into_bytes()
                } else {
                    "{".as_bytes().to_vec()
                };
                let sep = format!(",\n{indent_str}");
                text(&[&prefix, &parts.join(sep.as_bytes()), b"}"])
            }
        }
        TYPE_ID_LIST => {
            if unsafe { crate::object::seq_access::locked_len(ptr) } == 0 {
                "[]".as_bytes().to_vec()
            } else {
                format_sequence_pformat(_py, ptr, seen, current_indent, level, config, ("[", "]"))
            }
        }
        TYPE_ID_TUPLE => {
            let len = unsafe { crate::object::seq_access::locked_len(ptr) };
            if len == 0 {
                "()".as_bytes().to_vec()
            } else {
                let end = if len == 1 { ",)" } else { ")" };
                format_sequence_pformat(_py, ptr, seen, current_indent, level, config, ("(", end))
            }
        }
        _ => simple,
    };

    if is_container {
        seen.remove(&bits);
    }

    result
}

fn format_sequence_pformat(
    _py: &PyToken<'_>,
    ptr: *mut u8,
    seen: &mut HashSet<u64>,
    current_indent: i64,
    level: i64,
    config: PformatConfig,
    delimiters: (&str, &str),
) -> Vec<u8> {
    let (open, close) = delimiters;
    let child_indent = current_indent + config.indent_per_level;
    let indent_str = " ".repeat(child_indent as usize);
    let mut reprs = Vec::with_capacity(unsafe { crate::object::seq_access::locked_len(ptr) });
    let mut index = 0usize;
    loop {
        let live_len = unsafe { crate::object::seq_access::locked_len(ptr) };
        if index >= live_len {
            break;
        }
        let Some(item) = (unsafe { crate::object::seq_access::pin_item(_py, ptr, index) }) else {
            continue;
        };
        reprs.push(pformat_recursive(
            _py,
            item.bits(),
            seen,
            child_indent,
            level + 1,
            config,
        ));
        index += 1;
    }

    if config.compact {
        // In compact mode, try to fit multiple items on each line
        let mut lines: Vec<Vec<u8>> = Vec::new();
        let mut current_line = Vec::new();
        let max_line = config.width - child_indent;

        for (i, repr) in reprs.iter().enumerate() {
            let candidate = if current_line.is_empty() {
                repr.clone()
            } else {
                text(&[&current_line, b", ", repr])
            };
            let extra = if i == reprs.len() - 1 {
                close.len() as i64
            } else {
                2 // ", "
            };
            if !current_line.is_empty() && (text_len(&candidate) as i64 + extra) > max_line {
                lines.push(current_line);
                current_line = repr.clone();
            } else {
                current_line = candidate;
            }
        }
        if !current_line.is_empty() {
            lines.push(current_line);
        }

        let prefix = if config.indent_per_level > 1 {
            format!(
                "{}{}",
                open,
                " ".repeat((config.indent_per_level - 1) as usize)
            )
            .into_bytes()
        } else {
            open.as_bytes().to_vec()
        };
        let sep = format!(",\n{indent_str}");
        text(&[&prefix, &lines.join(sep.as_bytes()), close.as_bytes()])
    } else {
        let prefix = if config.indent_per_level > 1 {
            format!(
                "{}{}",
                open,
                " ".repeat((config.indent_per_level - 1) as usize)
            )
            .into_bytes()
        } else {
            open.as_bytes().to_vec()
        };
        let sep = format!(",\n{indent_str}");
        text(&[&prefix, &reprs.join(sep.as_bytes()), close.as_bytes()])
    }
}

fn format_int_underscored(i: i64) -> String {
    let s = format!("{}", i.unsigned_abs());
    let chars: Vec<char> = s.chars().collect();
    let mut result = String::with_capacity(s.len() + s.len() / 3);
    for (idx, ch) in chars.iter().enumerate() {
        if idx > 0 && (chars.len() - idx).is_multiple_of(3) {
            result.push('_');
        }
        result.push(*ch);
    }
    if i < 0 { format!("-{result}") } else { result }
}

// ─── public intrinsics ──────────────────────────────────────────────────────

/// Generate a safe repr with depth and width limits. Returns a string.
#[unsafe(no_mangle)]
pub extern "C" fn molt_pprint_safe_repr(obj_bits: u64, max_depth: u64, max_width: u64) -> u64 {
    crate::with_gil_entry_nopanic!(_py, {
        let depth = i64_from_bits_default(max_depth, -1);
        let width = i64_from_bits_default(max_width, -1);
        let mut seen = HashSet::new();
        let (repr, _, _) = safe_repr_inner(_py, obj_bits, &mut seen, 0, depth, width);
        alloc_string_result(_py, &repr)
    })
}

/// Format an object for pretty-printing. Returns a formatted string.
#[unsafe(no_mangle)]
pub extern "C" fn molt_pprint_format(
    obj_bits: u64,
    indent: u64,
    width: u64,
    depth: u64,
    compact: u64,
) -> u64 {
    crate::with_gil_entry_nopanic!(_py, {
        let indent_val = i64_from_bits_default(indent, 1);
        let width_val = i64_from_bits_default(width, 80);
        let depth_val = i64_from_bits_default(depth, -1);
        let compact_val = bool_from_bits_default(compact, false);
        let result = pformat_impl(
            _py,
            obj_bits,
            PformatConfig {
                indent_per_level: indent_val,
                width: width_val,
                max_depth: depth_val,
                compact: compact_val,
                sort_dicts: true,
                underscore_numbers: false,
            },
        );
        alloc_string_result(_py, &result)
    })
}

/// Check if repr of an object is readable (can be eval'd back). Returns a boolean.
#[unsafe(no_mangle)]
pub extern "C" fn molt_pprint_isreadable(obj_bits: u64) -> u64 {
    crate::with_gil_entry_nopanic!(_py, {
        let mut seen = HashSet::new();
        let (_repr, readable, recursive) = safe_repr_inner(_py, obj_bits, &mut seen, 0, -1, -1);
        let result = readable && !recursive;
        MoltObject::from_int(if result { 1 } else { 0 }).bits()
    })
}

/// Check if an object contains recursive references. Returns a boolean.
#[unsafe(no_mangle)]
pub extern "C" fn molt_pprint_isrecursive(obj_bits: u64) -> u64 {
    crate::with_gil_entry_nopanic!(_py, {
        let mut seen = HashSet::new();
        let (_repr, _, recursive) = safe_repr_inner(_py, obj_bits, &mut seen, 0, -1, -1);
        MoltObject::from_int(if recursive { 1 } else { 0 }).bits()
    })
}

/// Full pformat with all options. Returns a formatted string.
#[unsafe(no_mangle)]
pub extern "C" fn molt_pprint_pformat(
    obj_bits: u64,
    indent: u64,
    width: u64,
    depth: u64,
    compact: u64,
    sort_dicts: u64,
    underscore_numbers: u64,
) -> u64 {
    crate::with_gil_entry_nopanic!(_py, {
        let indent_val = i64_from_bits_default(indent, 1);
        let width_val = i64_from_bits_default(width, 80);
        let depth_val = i64_from_bits_default(depth, -1);
        let compact_val = bool_from_bits_default(compact, false);
        let sort_dicts_val = bool_from_bits_default(sort_dicts, true);
        let underscore_numbers_val = bool_from_bits_default(underscore_numbers, false);
        let result = pformat_impl(
            _py,
            obj_bits,
            PformatConfig {
                indent_per_level: indent_val,
                width: width_val,
                max_depth: depth_val,
                compact: compact_val,
                sort_dicts: sort_dicts_val,
                underscore_numbers: underscore_numbers_val,
            },
        );
        alloc_string_result(_py, &result)
    })
}
