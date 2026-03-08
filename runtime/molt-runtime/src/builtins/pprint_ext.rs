use std::collections::HashSet;

use crate::*;

// ─── helpers ────────────────────────────────────────────────────────────────

fn i64_from_bits_default(bits: u64, default: i64) -> i64 {
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

fn alloc_string_result(_py: &PyToken<'_>, s: &str) -> u64 {
    let ptr = alloc_string(_py, s.as_bytes());
    if ptr.is_null() {
        return raise_exception::<_>(_py, "MemoryError", "out of memory");
    }
    MoltObject::from_ptr(ptr).bits()
}

// ─── repr formatting engine ─────────────────────────────────────────────────

/// Internal recursive repr generator that tracks object IDs to detect cycles
/// and respects max_depth and max_width constraints.
fn safe_repr_inner(
    _py: &PyToken<'_>,
    bits: u64,
    seen: &mut HashSet<u64>,
    depth: i64,
    max_depth: i64,
    max_width: i64,
) -> (String, bool, bool) {
    // readable = true if the repr can be eval'd back
    // recursive = true if we detected a cycle

    let obj = obj_from_bits(bits);

    // None and immediates
    if obj.is_none() {
        return ("None".to_string(), true, false);
    }
    if let Some(f) = obj.as_float() {
        return (format!("{}", f), true, false);
    }
    if let Some(i) = to_i64(obj) {
        return (format!("{}", i), true, false);
    }

    let Some(ptr) = obj.as_ptr() else {
        return ("None".to_string(), true, false);
    };

    let type_id = unsafe { object_type_id(ptr) };

    // Strings, bytes — use runtime repr
    // Note: None, bool, int, float are NaN-boxed and handled before as_ptr().
    match type_id {
        TYPE_ID_STRING | TYPE_ID_BYTES => {
            let repr = format_obj(_py, obj);
            return (repr, true, false);
        }
        _ => {}
    }

    // Check depth limit
    if max_depth > 0 && depth >= max_depth {
        match type_id {
            TYPE_ID_LIST => return ("[...]".to_string(), false, false),
            TYPE_ID_TUPLE => return ("(...)".to_string(), false, false),
            TYPE_ID_DICT => return ("{...}".to_string(), false, false),
            TYPE_ID_SET => return ("{...}".to_string(), false, false),
            TYPE_ID_FROZENSET => return ("frozenset({...})".to_string(), false, false),
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
            let type_label = type_name(_py, obj);
            return (
                format!("<Recursion on {type_label} with id={bits}>"),
                false,
                true,
            );
        }
        seen.insert(bits);
    }

    let result = match type_id {
        TYPE_ID_LIST => {
            let src = unsafe { seq_vec_ref(ptr) };
            let len = src.len();
            if len == 0 {
                ("[]".to_string(), true, false)
            } else {
                let mut readable = true;
                let mut recursive = false;
                let mut parts = Vec::with_capacity(len);
                let display_len = if max_width > 0 && (len as i64) > max_width {
                    max_width as usize
                } else {
                    len
                };
                for &item_bits in src.iter().take(display_len) {
                    let (s, r, rec) =
                        safe_repr_inner(_py, item_bits, seen, depth + 1, max_depth, max_width);
                    if !r {
                        readable = false;
                    }
                    if rec {
                        recursive = true;
                    }
                    parts.push(s);
                }
                if display_len < len {
                    parts.push("...".to_string());
                    readable = false;
                }
                (format!("[{}]", parts.join(", ")), readable, recursive)
            }
        }
        TYPE_ID_TUPLE => {
            let src = unsafe { seq_vec_ref(ptr) };
            let len = src.len();
            if len == 0 {
                ("()".to_string(), true, false)
            } else {
                let mut readable = true;
                let mut recursive = false;
                let mut parts = Vec::with_capacity(len);
                let display_len = if max_width > 0 && (len as i64) > max_width {
                    max_width as usize
                } else {
                    len
                };
                for &item_bits in src.iter().take(display_len) {
                    let (s, r, rec) =
                        safe_repr_inner(_py, item_bits, seen, depth + 1, max_depth, max_width);
                    if !r {
                        readable = false;
                    }
                    if rec {
                        recursive = true;
                    }
                    parts.push(s);
                }
                if display_len < len {
                    parts.push("...".to_string());
                    readable = false;
                }
                if len == 1 && display_len == 1 {
                    (format!("({},)", parts[0]), readable, recursive)
                } else {
                    (format!("({})", parts.join(", ")), readable, recursive)
                }
            }
        }
        TYPE_ID_DICT => {
            let order = unsafe { dict_order(ptr) };
            let num_pairs = order.len() / 2;
            if num_pairs == 0 {
                ("{}".to_string(), true, false)
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
                    let ka = format_obj(_py, obj_from_bits(a.0));
                    let kb = format_obj(_py, obj_from_bits(b.0));
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
                    parts.push(format!("{}: {}", ks, vs));
                }
                if display_len < num_pairs {
                    parts.push("...".to_string());
                    readable = false;
                }
                (format!("{{{}}}", parts.join(", ")), readable, recursive)
            }
        }
        TYPE_ID_SET => {
            let order = unsafe { set_order(ptr) };
            if order.is_empty() {
                ("set()".to_string(), true, false)
            } else {
                let readable = true;
                let recursive = false;
                let mut repr_elems: Vec<String> = order
                    .iter()
                    .map(|&e| {
                        let (s, _, _) =
                            safe_repr_inner(_py, e, seen, depth + 1, max_depth, max_width);
                        s
                    })
                    .collect();
                repr_elems.sort();
                (
                    format!("{{{}}}", repr_elems.join(", ")),
                    readable,
                    recursive,
                )
            }
        }
        TYPE_ID_FROZENSET => {
            let order = unsafe { set_order(ptr) };
            if order.is_empty() {
                ("frozenset()".to_string(), true, false)
            } else {
                let readable = true;
                let recursive = false;
                let mut repr_elems: Vec<String> = order
                    .iter()
                    .map(|&e| {
                        let (s, _, _) =
                            safe_repr_inner(_py, e, seen, depth + 1, max_depth, max_width);
                        s
                    })
                    .collect();
                repr_elems.sort();
                (
                    format!("frozenset({{{}}})", repr_elems.join(", ")),
                    readable,
                    recursive,
                )
            }
        }
        _ => {
            // Fall back to the runtime repr for other types
            let repr = format_obj(_py, obj);
            let readable = !repr.is_empty() && !repr.starts_with('<');
            (repr, readable, false)
        }
    };

    if is_container {
        seen.remove(&bits);
    }

    result
}

// ─── pformat engine ─────────────────────────────────────────────────────────

/// Full pformat implementation matching CPython's pprint.pformat behavior.
#[allow(clippy::too_many_arguments)]
fn pformat_impl(
    _py: &PyToken<'_>,
    bits: u64,
    indent: i64,
    width: i64,
    depth: i64,
    compact: bool,
    sort_dicts: bool,
    underscore_numbers: bool,
) -> String {
    let mut seen = HashSet::new();
    pformat_recursive(
        _py,
        bits,
        &mut seen,
        0,
        indent,
        width,
        depth,
        0,
        compact,
        sort_dicts,
        underscore_numbers,
    )
}

#[allow(clippy::too_many_arguments)]
fn pformat_recursive(
    _py: &PyToken<'_>,
    bits: u64,
    seen: &mut HashSet<u64>,
    current_indent: i64,
    indent_per_level: i64,
    width: i64,
    max_depth: i64,
    level: i64,
    compact: bool,
    sort_dicts: bool,
    underscore_numbers: bool,
) -> String {
    let obj = obj_from_bits(bits);

    // Simple scalars
    if obj.is_none() {
        return "None".to_string();
    }
    if let Some(f) = obj.as_float() {
        return format!("{}", f);
    }
    if let Some(i) = to_i64(obj) {
        if underscore_numbers {
            return format_int_underscored(i);
        }
        return format!("{}", i);
    }

    let Some(ptr) = obj.as_ptr() else {
        return "None".to_string();
    };

    let type_id = unsafe { object_type_id(ptr) };

    // Scalars — Note: None, bool, int, float are NaN-boxed and handled before as_ptr().
    match type_id {
        TYPE_ID_STRING | TYPE_ID_BYTES => {
            return format_obj(_py, obj);
        }
        _ => {}
    }

    // Depth check
    if max_depth > 0 && level >= max_depth {
        match type_id {
            TYPE_ID_LIST => return "[...]".to_string(),
            TYPE_ID_TUPLE => return "(...)".to_string(),
            TYPE_ID_DICT => return "{...}".to_string(),
            _ => {}
        }
    }

    // Cycle check
    let is_container = matches!(
        type_id,
        TYPE_ID_LIST | TYPE_ID_TUPLE | TYPE_ID_DICT | TYPE_ID_SET | TYPE_ID_FROZENSET
    );
    if is_container && seen.contains(&bits) {
        let type_label = type_name(_py, obj);
        return format!("<Recursion on {type_label} with id={bits}>");
    }
    if is_container {
        seen.insert(bits);
    }

    // Try simple single-line repr first
    let simple = {
        let mut temp_seen = seen.clone();
        let (s, _, _) = safe_repr_inner(_py, bits, &mut temp_seen, level, max_depth, -1);
        s
    };

    let available = width - current_indent;
    if (simple.len() as i64) <= available {
        if is_container {
            seen.remove(&bits);
        }
        return simple;
    }

    // Multi-line formatting for containers
    let result = match type_id {
        TYPE_ID_DICT => {
            let order = unsafe { dict_order(ptr) };
            let num_pairs = order.len() / 2;
            if num_pairs == 0 {
                "{}".to_string()
            } else {
                let child_indent = current_indent + indent_per_level;
                let indent_str = " ".repeat(child_indent as usize);
                // Collect pairs
                let mut pairs: Vec<(u64, u64)> = Vec::with_capacity(num_pairs);
                let mut i = 0;
                while i + 1 < order.len() {
                    pairs.push((order[i], order[i + 1]));
                    i += 2;
                }
                if sort_dicts {
                    pairs.sort_by(|a, b| {
                        let ka = format_obj(_py, obj_from_bits(a.0));
                        let kb = format_obj(_py, obj_from_bits(b.0));
                        ka.cmp(&kb)
                    });
                }
                let mut parts = Vec::with_capacity(pairs.len());
                for (key_bits, val_bits) in &pairs {
                    let key_repr = pformat_recursive(
                        _py,
                        *key_bits,
                        seen,
                        child_indent,
                        indent_per_level,
                        width,
                        max_depth,
                        level + 1,
                        compact,
                        sort_dicts,
                        underscore_numbers,
                    );
                    let val_repr = pformat_recursive(
                        _py,
                        *val_bits,
                        seen,
                        child_indent + key_repr.len() as i64 + 2,
                        indent_per_level,
                        width,
                        max_depth,
                        level + 1,
                        compact,
                        sort_dicts,
                        underscore_numbers,
                    );
                    parts.push(format!("{key_repr}: {val_repr}"));
                }
                let prefix = if indent_per_level > 1 {
                    format!("{{{}", " ".repeat((indent_per_level - 1) as usize))
                } else {
                    "{".to_string()
                };
                let sep = format!(",\n{indent_str}");
                format!("{}{}}}", prefix, parts.join(&sep))
            }
        }
        TYPE_ID_LIST => {
            let src = unsafe { seq_vec_ref(ptr) };
            if src.is_empty() {
                "[]".to_string()
            } else {
                format_sequence_pformat(
                    _py,
                    src,
                    seen,
                    current_indent,
                    indent_per_level,
                    width,
                    max_depth,
                    level,
                    compact,
                    sort_dicts,
                    underscore_numbers,
                    "[",
                    "]",
                )
            }
        }
        TYPE_ID_TUPLE => {
            let src = unsafe { seq_vec_ref(ptr) };
            if src.is_empty() {
                "()".to_string()
            } else {
                let end = if src.len() == 1 { ",)" } else { ")" };
                format_sequence_pformat(
                    _py,
                    src,
                    seen,
                    current_indent,
                    indent_per_level,
                    width,
                    max_depth,
                    level,
                    compact,
                    sort_dicts,
                    underscore_numbers,
                    "(",
                    end,
                )
            }
        }
        _ => simple,
    };

    if is_container {
        seen.remove(&bits);
    }

    result
}

#[allow(clippy::too_many_arguments)]
fn format_sequence_pformat(
    _py: &PyToken<'_>,
    elems: &[u64],
    seen: &mut HashSet<u64>,
    current_indent: i64,
    indent_per_level: i64,
    width: i64,
    max_depth: i64,
    level: i64,
    compact: bool,
    sort_dicts: bool,
    underscore_numbers: bool,
    open: &str,
    close: &str,
) -> String {
    let child_indent = current_indent + indent_per_level;
    let indent_str = " ".repeat(child_indent as usize);

    if compact {
        // In compact mode, try to fit multiple items on each line
        let mut lines: Vec<String> = Vec::new();
        let mut current_line = String::new();
        let max_line = width - child_indent;

        for (i, &elem) in elems.iter().enumerate() {
            let repr = pformat_recursive(
                _py,
                elem,
                seen,
                child_indent,
                indent_per_level,
                width,
                max_depth,
                level + 1,
                compact,
                sort_dicts,
                underscore_numbers,
            );
            let candidate = if current_line.is_empty() {
                repr.clone()
            } else {
                format!("{}, {}", current_line, repr)
            };
            let extra = if i == elems.len() - 1 {
                close.len() as i64
            } else {
                2 // ", "
            };
            if !current_line.is_empty() && (candidate.len() as i64 + extra) > max_line {
                lines.push(current_line);
                current_line = repr;
            } else {
                current_line = candidate;
            }
        }
        if !current_line.is_empty() {
            lines.push(current_line);
        }

        let prefix = if indent_per_level > 1 {
            format!("{}{}", open, " ".repeat((indent_per_level - 1) as usize))
        } else {
            open.to_string()
        };
        let sep = format!(",\n{indent_str}");
        format!("{}{}{}", prefix, lines.join(&sep), close)
    } else {
        let prefix = if indent_per_level > 1 {
            format!("{}{}", open, " ".repeat((indent_per_level - 1) as usize))
        } else {
            open.to_string()
        };
        let mut parts = Vec::with_capacity(elems.len());
        for &elem in elems {
            let repr = pformat_recursive(
                _py,
                elem,
                seen,
                child_indent,
                indent_per_level,
                width,
                max_depth,
                level + 1,
                compact,
                sort_dicts,
                underscore_numbers,
            );
            parts.push(repr);
        }
        let sep = format!(",\n{indent_str}");
        format!("{}{}{}", prefix, parts.join(&sep), close)
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
    crate::with_gil_entry!(_py, {
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
    crate::with_gil_entry!(_py, {
        let indent_val = i64_from_bits_default(indent, 1);
        let width_val = i64_from_bits_default(width, 80);
        let depth_val = i64_from_bits_default(depth, -1);
        let compact_val = bool_from_bits_default(compact, false);
        let result = pformat_impl(
            _py,
            obj_bits,
            indent_val,
            width_val,
            depth_val,
            compact_val,
            true,
            false,
        );
        alloc_string_result(_py, &result)
    })
}

/// Check if repr of an object is readable (can be eval'd back). Returns a boolean.
#[unsafe(no_mangle)]
pub extern "C" fn molt_pprint_isreadable(obj_bits: u64) -> u64 {
    crate::with_gil_entry!(_py, {
        let mut seen = HashSet::new();
        let (_repr, readable, recursive) = safe_repr_inner(_py, obj_bits, &mut seen, 0, -1, -1);
        let result = readable && !recursive;
        MoltObject::from_int(if result { 1 } else { 0 }).bits()
    })
}

/// Check if an object contains recursive references. Returns a boolean.
#[unsafe(no_mangle)]
pub extern "C" fn molt_pprint_isrecursive(obj_bits: u64) -> u64 {
    crate::with_gil_entry!(_py, {
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
    crate::with_gil_entry!(_py, {
        let indent_val = i64_from_bits_default(indent, 1);
        let width_val = i64_from_bits_default(width, 80);
        let depth_val = i64_from_bits_default(depth, -1);
        let compact_val = bool_from_bits_default(compact, false);
        let sort_dicts_val = bool_from_bits_default(sort_dicts, true);
        let underscore_numbers_val = bool_from_bits_default(underscore_numbers, false);
        let result = pformat_impl(
            _py,
            obj_bits,
            indent_val,
            width_val,
            depth_val,
            compact_val,
            sort_dicts_val,
            underscore_numbers_val,
        );
        alloc_string_result(_py, &result)
    })
}
