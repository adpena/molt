use molt_runtime_core::prelude::{
    MoltObject, OwnedBridgeHandleSnapshot, TYPE_ID_DICT, obj_from_bits, rt_dec_ref,
    rt_exception_pending, rt_inc_ref,
};

use crate::bridge::{decode_value_list_bits, dict_snapshot, object_type_id};

use super::common::{
    alloc_list_bits, alloc_str_bits, bits_as_f64, bits_as_i64, bits_is_none, bits_to_string,
    cleanup_list,
};

/// Flatten nested list/tuple args into a flat list for Tcl command building.
///
/// Recursively descends into list/tuple elements, collecting non-None leaf
/// values into a flat list. None values are skipped.

pub fn molt_tk_flatten_args_attached_body(_py: &molt_runtime_core::PyToken, args_bits: u64) -> u64 {
    molt_runtime_core::with_attached_entry!(_py, _py, {
        let mut out: Vec<u64> = Vec::new();
        let mut snapshots = Vec::new();
        flatten_recursive(_py, args_bits, &mut out, &mut snapshots);
        if rt_exception_pending(_py) {
            return MoltObject::none().bits();
        }
        match alloc_list_bits(_py, &out) {
            Ok(bits) => bits,
            Err(bits) => bits,
        }
    })
}

/// Recursive helper for flatten_args.
fn flatten_recursive<'py>(
    py: &'py molt_runtime_core::PyToken,
    bits: u64,
    out: &mut Vec<u64>,
    snapshots: &mut Vec<OwnedBridgeHandleSnapshot<'py>>,
) {
    if bits_is_none(bits) {
        return;
    }
    if let Some(elems) = decode_value_list_bits(py, bits) {
        for &elem_bits in elems.iter() {
            flatten_recursive(py, elem_bits, out, snapshots);
        }
        // Keep nested leaves pinned until allocation adopts the flattened list.
        snapshots.push(elems);
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

pub fn molt_tk_cnfmerge_attached_body(
    _py: &molt_runtime_core::PyToken,
    cnf_bits: u64,
    kw_bits: u64,
) -> u64 {
    molt_runtime_core::with_attached_entry!(_py, _py, {
        let mut out: Vec<u64> = Vec::new();

        // Process cnf dict
        if !bits_is_none(cnf_bits)
            && let Some(ptr) = obj_from_bits(cnf_bits).as_ptr()
        {
            let type_id = object_type_id(_py, ptr);
            if type_id == TYPE_ID_DICT {
                let Some(order_snapshot) = dict_snapshot(_py, ptr) else {
                    cleanup_list(_py, &out);
                    return MoltObject::none().bits();
                };
                let mut i = 0;
                while i + 1 < order_snapshot.len() {
                    let key_bits = order_snapshot[i];
                    let value_bits = order_snapshot[i + 1];
                    if !bits_is_none(value_bits)
                        && let Err(bits) = emit_option_pair(_py, key_bits, value_bits, &mut out)
                    {
                        cleanup_list(_py, &out);
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
            let type_id = object_type_id(_py, ptr);
            if type_id == TYPE_ID_DICT {
                let Some(order_snapshot) = dict_snapshot(_py, ptr) else {
                    cleanup_list(_py, &out);
                    return MoltObject::none().bits();
                };
                let mut i = 0;
                while i + 1 < order_snapshot.len() {
                    let key_bits = order_snapshot[i];
                    let value_bits = order_snapshot[i + 1];
                    if !bits_is_none(value_bits)
                        && let Err(bits) = emit_option_pair(_py, key_bits, value_bits, &mut out)
                    {
                        cleanup_list(_py, &out);
                        return bits;
                    }
                    i += 2;
                }
            }
        }

        match alloc_list_bits(_py, &out) {
            Ok(list_bits) => {
                // alloc_list inc-refs each element; release our local string refs
                for &elem_bits in &out {
                    rt_dec_ref(_py, elem_bits);
                }
                list_bits
            }
            Err(bits) => {
                cleanup_list(_py, &out);
                bits
            }
        }
    })
}

/// Emit a normalized "-key", value pair into the output vec.
/// The key is converted to string, normalized with leading "-", and
/// underscores in the key name are preserved (matching CPython tkinter behavior).
fn emit_option_pair(
    py: &molt_runtime_core::PyToken,
    key_bits: u64,
    value_bits: u64,
    out: &mut Vec<u64>,
) -> Result<(), u64> {
    let key_str = bits_to_string(py, key_bits).unwrap_or_else(|| {
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
    let key_str_bits = alloc_str_bits(py, &normalized)?;
    out.push(key_str_bits);
    // Push the value as-is (could be string, int, etc.)
    out.push(value_bits);
    // Inc-ref value since we're going to pass it to alloc_list later which
    // will also inc-ref it, but we dec-ref everything in out after alloc.
    // Actually, the caller pattern is: alloc_list(out) which inc-refs,
    // then we dec-ref out elements. For the key_str_bits, that's correct.
    // For value_bits we need to inc-ref here so the dec-ref after alloc_list
    // doesn't drop the caller's reference.
    rt_inc_ref(py, value_bits);
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

include!("../attached_exports_generated/tkinter_core/options.rs");
