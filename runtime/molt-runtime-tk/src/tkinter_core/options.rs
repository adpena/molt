use molt_runtime_core::prelude::*;

use crate::bridge::{decode_value_list_bits, dict_order, object_type_id};

use super::common::{
    alloc_list_bits, alloc_str_bits, bits_as_f64, bits_as_i64, bits_is_none, bits_to_string,
    cleanup_list,
};

/// Flatten nested list/tuple args into a flat list for Tcl command building.
///
/// Recursively descends into list/tuple elements, collecting non-None leaf
/// values into a flat list. None values are skipped.
#[unsafe(no_mangle)]
pub extern "C" fn molt_tk_flatten_args(args_bits: u64) -> u64 {
    molt_runtime_core::with_gil_entry!(_py, {
        let mut out: Vec<u64> = Vec::new();
        flatten_recursive(args_bits, &mut out);
        if rt_exception_pending() {
            return MoltObject::none().bits();
        }
        match alloc_list_bits(&out) {
            Ok(bits) => bits,
            Err(bits) => bits,
        }
    })
}

/// Recursive helper for flatten_args.
fn flatten_recursive(bits: u64, out: &mut Vec<u64>) {
    if bits_is_none(bits) {
        return;
    }
    if let Some(elems) = decode_value_list_bits(bits) {
        for elem_bits in elems {
            flatten_recursive(elem_bits, out);
        }
    } else {
        // Leaf value — include as-is
        out.push(bits);
    }
}

/// Merge config dict and keyword dict, filter None values, return flat list
/// of "-key", "value" pairs for Tcl command building.
///
/// `cnf` is either a dict (NaN-boxed pointer to dict) or None.
/// `kw` is either a dict or None.
///
/// For each key-value pair in the merged dict (cnf first, then kw):
///   - Skip pairs where value is None
///   - Normalize key: ensure leading "-", convert underscores to nothing
///   - Append "-key" and value to output list
///
/// Returns a Molt list of alternating "-key", value elements.
#[unsafe(no_mangle)]
pub extern "C" fn molt_tk_cnfmerge(cnf_bits: u64, kw_bits: u64) -> u64 {
    molt_runtime_core::with_gil_entry!(_py, {
        let mut out: Vec<u64> = Vec::new();

        // Process cnf dict
        if !bits_is_none(cnf_bits)
            && let Some(ptr) = obj_from_bits(cnf_bits).as_ptr()
        {
            let type_id = object_type_id(ptr);
            if type_id == TYPE_ID_DICT {
                let order_snapshot = dict_order(ptr);
                let mut i = 0;
                while i + 1 < order_snapshot.len() {
                    let key_bits = order_snapshot[i];
                    let value_bits = order_snapshot[i + 1];
                    if !bits_is_none(value_bits)
                        && let Err(bits) = emit_option_pair(key_bits, value_bits, &mut out)
                    {
                        cleanup_list(&out);
                        return bits;
                    }
                    i += 2;
                }
            }
        }

        // Process kw dict
        if !bits_is_none(kw_bits)
            && let Some(ptr) = obj_from_bits(kw_bits).as_ptr()
        {
            let type_id = object_type_id(ptr);
            if type_id == TYPE_ID_DICT {
                let order_snapshot = dict_order(ptr);
                let mut i = 0;
                while i + 1 < order_snapshot.len() {
                    let key_bits = order_snapshot[i];
                    let value_bits = order_snapshot[i + 1];
                    if !bits_is_none(value_bits)
                        && let Err(bits) = emit_option_pair(key_bits, value_bits, &mut out)
                    {
                        cleanup_list(&out);
                        return bits;
                    }
                    i += 2;
                }
            }
        }

        match alloc_list_bits(&out) {
            Ok(list_bits) => {
                // alloc_list inc-refs each element; release our local string refs
                for &elem_bits in &out {
                    rt_dec_ref(elem_bits);
                }
                list_bits
            }
            Err(bits) => {
                cleanup_list(&out);
                bits
            }
        }
    })
}

/// Emit a normalized "-key", value pair into the output vec.
/// The key is converted to string, normalized with leading "-", and
/// underscores in the key name are preserved (matching CPython tkinter behavior).
fn emit_option_pair(key_bits: u64, value_bits: u64, out: &mut Vec<u64>) -> Result<(), u64> {
    let key_str = bits_to_string(key_bits).unwrap_or_else(|| {
        // If key is not a string, try int/float rendering
        if let Some(i) = bits_as_i64(key_bits) {
            i.to_string()
        } else if let Some(f) = bits_as_f64(key_bits) {
            f.to_string()
        } else {
            String::from("?")
        }
    });

    let normalized = normalize_option_str(&key_str);
    let key_str_bits = alloc_str_bits(&normalized)?;
    out.push(key_str_bits);
    // Push the value as-is (could be string, int, etc.)
    out.push(value_bits);
    // Inc-ref value since we're going to pass it to alloc_list later which
    // will also inc-ref it, but we dec-ref everything in out after alloc.
    // Actually, the caller pattern is: alloc_list(out) which inc-refs,
    // then we dec-ref out elements. For the key_str_bits, that's correct.
    // For value_bits we need to inc-ref here so the dec-ref after alloc_list
    // doesn't drop the caller's reference.
    rt_inc_ref(value_bits);
    Ok(())
}

/// Normalize a tkinter option name: ensure it has a leading "-".
fn normalize_option_str(name: &str) -> String {
    if name.starts_with('-') {
        name.to_string()
    } else {
        format!("-{name}")
    }
}

/// Normalize a tkinter option name: ensure leading "-", convert underscores.
///
/// Input is a Molt string. Returns a new Molt string with:
///   - Leading "-" prepended if missing
///   - (Underscores preserved — tkinter convention)
#[unsafe(no_mangle)]
pub extern "C" fn molt_tk_normalize_option(name_bits: u64) -> u64 {
    molt_runtime_core::with_gil_entry!(_py, {
        let Some(name) = bits_to_string(name_bits) else {
            // Not a string — return as-is
            return name_bits;
        };
        let normalized = normalize_option_str(&name);
        // If already normalized, return the original to avoid allocation
        if normalized == name {
            return name_bits;
        }
        match alloc_str_bits(&normalized) {
            Ok(bits) => bits,
            Err(bits) => bits,
        }
    })
}
