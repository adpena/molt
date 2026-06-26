use super::projection::dataclasses_fields_dict_bits;
use super::*;

// ─────────────────────────────────────────────────────────────────────────────
// Dataclass runtime method intrinsics
//
// These replace the Python-side _dataclass_init, _dataclass_repr,
// _dataclass_eq, _dataclass_hash, and _dataclass_order functions with
// single intrinsic calls, eliminating per-call field iteration in Python.
// ─────────────────────────────────────────────────────────────────────────────

/// Helper: get the repr() of a value as a Rust String.
fn dc_repr_str(_py: &PyToken<'_>, val_bits: u64) -> String {
    let repr_bits = molt_repr_from_obj(val_bits);
    if exception_pending(_py) {
        clear_exception(_py);
        return "?".to_string();
    }
    string_obj_to_owned(obj_from_bits(repr_bits)).unwrap_or_else(|| "?".to_string())
}

/// Helper: read `__dataclass_fields__` from the class of `self_bits` and
/// return an ordered vec of (field_name_bits, field_obj_bits) pairs.
fn dc_fields_ordered(_py: &PyToken<'_>, self_bits: u64) -> Option<Vec<(u64, u64)>> {
    let cls_bits = type_of_bits(_py, self_bits);
    let missing = missing_bits(_py);
    let fields_dict_bits = dataclasses_fields_dict_bits(_py, cls_bits, missing)?;
    let fields_dict_ptr = obj_from_bits(fields_dict_bits).as_ptr()?;
    let order = unsafe { dict_order(fields_dict_ptr) }.clone();
    let mut result = Vec::new();
    for pair in order.chunks(2) {
        if pair.len() == 2 {
            result.push((pair[0], pair[1]));
        }
    }
    Some(result)
}

pub(in crate::builtins::types::dataclasses) fn dc_getattr_default_bits(
    _py: &PyToken<'_>,
    obj_bits: u64,
    attr: &[u8],
    default_bits: u64,
) -> Option<u64> {
    let name_bits = attr_name_bits_from_bytes(_py, attr)?;
    let val =
        crate::builtins::attributes::molt_get_attr_name_default(obj_bits, name_bits, default_bits);
    dec_ref_bits(_py, name_bits);
    Some(val)
}

/// Helper: read the `_field_type` attribute from a field object and check
/// whether it matches a given tag.
fn dc_field_has_tag(_py: &PyToken<'_>, field_bits: u64, tag_name: &[u8]) -> bool {
    let missing = missing_bits(_py);
    let Some(ft_bits) = dc_getattr_default_bits(_py, field_bits, b"_field_type", missing) else {
        return false;
    };
    if exception_pending(_py) {
        clear_exception(_py);
        return false;
    }
    if ft_bits == missing {
        return false;
    }
    // Compare the field_type's name attribute to the tag_name string
    let Some(name_bits) = dc_getattr_default_bits(_py, ft_bits, b"name", missing) else {
        return false;
    };
    if exception_pending(_py) {
        clear_exception(_py);
        return false;
    }
    if name_bits == missing {
        return false;
    }
    let Some(name_str) = string_obj_to_owned(obj_from_bits(name_bits)) else {
        return false;
    };
    name_str.as_bytes() == tag_name
}

/// Helper: check if a field_obj is a _FIELD (not classvar or initvar).
fn dc_is_field(_py: &PyToken<'_>, field_bits: u64) -> bool {
    dc_field_has_tag(_py, field_bits, b"_FIELD")
}

/// Helper: read a bool-ish attribute from a field object.
fn dc_field_bool_attr(_py: &PyToken<'_>, field_bits: u64, attr: &[u8]) -> Option<bool> {
    let missing = missing_bits(_py);
    let val = dc_getattr_default_bits(_py, field_bits, attr, missing)?;
    if exception_pending(_py) {
        clear_exception(_py);
        return None;
    }
    if val == missing {
        return None;
    }
    Some(is_truthy(_py, obj_from_bits(val)))
}

/// Helper: read the "name" string attribute from a field object.
fn dc_field_name_str(_py: &PyToken<'_>, field_bits: u64) -> Option<String> {
    let missing = missing_bits(_py);
    let val = dc_getattr_default_bits(_py, field_bits, b"name", missing)?;
    if exception_pending(_py) {
        clear_exception(_py);
        return None;
    }
    if val == missing {
        return None;
    }
    string_obj_to_owned(obj_from_bits(val))
}

/// `molt_dataclasses_repr(self)` → str
///
/// Replaces `_dataclass_repr` in Python. Iterates over dataclass fields
/// that have repr=True and builds `ClassName(field=value, ...)`.
#[unsafe(no_mangle)]
pub extern "C" fn molt_dataclasses_repr(self_bits: u64) -> u64 {
    crate::with_gil_entry_nopanic!(_py, {
        let Some(fields) = dc_fields_ordered(_py, self_bits) else {
            let ptr = alloc_string(_py, b"<dataclass>");
            if ptr.is_null() {
                return MoltObject::none().bits();
            }
            return MoltObject::from_ptr(ptr).bits();
        };
        let cls_bits = type_of_bits(_py, self_bits);
        let missing = missing_bits(_py);

        // Get class qualname or name
        let cls_name = {
            let qname_key = attr_name_bits_from_bytes(_py, b"__qualname__");
            let mut name = String::new();
            if let Some(qk) = qname_key {
                let qv =
                    crate::builtins::attributes::molt_get_attr_name_default(cls_bits, qk, missing);
                dec_ref_bits(_py, qk);
                if !exception_pending(_py) && qv != missing {
                    if let Some(s) = string_obj_to_owned(obj_from_bits(qv)) {
                        name = s;
                    }
                } else if exception_pending(_py) {
                    clear_exception(_py);
                }
            }
            if name.is_empty()
                && let Some(nk) = attr_name_bits_from_bytes(_py, b"__name__")
            {
                let nv =
                    crate::builtins::attributes::molt_get_attr_name_default(cls_bits, nk, missing);
                dec_ref_bits(_py, nk);
                if !exception_pending(_py) && nv != missing {
                    if let Some(s) = string_obj_to_owned(obj_from_bits(nv)) {
                        name = s;
                    }
                } else if exception_pending(_py) {
                    clear_exception(_py);
                }
            }
            if name.is_empty() {
                "?".to_string()
            } else {
                name
            }
        };

        let mut parts: Vec<String> = Vec::new();
        for (_name_bits, field_bits) in &fields {
            if !dc_is_field(_py, *field_bits) {
                continue;
            }
            if !dc_field_bool_attr(_py, *field_bits, b"repr").unwrap_or(true) {
                continue;
            }
            let Some(fname) = dc_field_name_str(_py, *field_bits) else {
                continue;
            };
            // Get the field value from self
            let Some(fname_key) = attr_name_bits_from_bytes(_py, fname.as_bytes()) else {
                continue;
            };
            let val = crate::builtins::attributes::molt_get_attr_name_default(
                self_bits, fname_key, missing,
            );
            dec_ref_bits(_py, fname_key);
            if exception_pending(_py) {
                clear_exception(_py);
                continue;
            }
            if val == missing {
                continue;
            }
            // Get repr of value
            let repr_str = dc_repr_str(_py, val);
            parts.push(format!("{fname}={repr_str}"));
        }
        let result = format!("{cls_name}({})", parts.join(", "));
        let ptr = alloc_string(_py, result.as_bytes());
        if ptr.is_null() {
            return MoltObject::none().bits();
        }
        MoltObject::from_ptr(ptr).bits()
    })
}

/// `molt_dataclasses_eq(self, other)` → bool|NotImplemented
///
/// Replaces `_dataclass_eq` in Python. Returns True if all compare-enabled
/// fields are equal. Returns None (used as NotImplemented signal) if classes differ.
#[unsafe(no_mangle)]
pub extern "C" fn molt_dataclasses_eq(self_bits: u64, other_bits: u64) -> u64 {
    crate::with_gil_entry_nopanic!(_py, {
        let self_cls = type_of_bits(_py, self_bits);
        let other_cls = type_of_bits(_py, other_bits);
        if self_cls != other_cls {
            // Return None to signal NotImplemented
            return MoltObject::none().bits();
        }
        let Some(fields) = dc_fields_ordered(_py, self_bits) else {
            return MoltObject::from_bool(true).bits();
        };
        let missing = missing_bits(_py);
        for (_name_bits, field_bits) in &fields {
            if !dc_is_field(_py, *field_bits) {
                continue;
            }
            if !dc_field_bool_attr(_py, *field_bits, b"compare").unwrap_or(true) {
                continue;
            }
            let Some(fname) = dc_field_name_str(_py, *field_bits) else {
                continue;
            };
            let Some(fname_key) = attr_name_bits_from_bytes(_py, fname.as_bytes()) else {
                continue;
            };
            let self_val = crate::builtins::attributes::molt_get_attr_name_default(
                self_bits, fname_key, missing,
            );
            let other_val = crate::builtins::attributes::molt_get_attr_name_default(
                other_bits, fname_key, missing,
            );
            dec_ref_bits(_py, fname_key);
            if exception_pending(_py) {
                clear_exception(_py);
                return MoltObject::from_bool(false).bits();
            }
            if !obj_eq(_py, obj_from_bits(self_val), obj_from_bits(other_val)) {
                return MoltObject::from_bool(false).bits();
            }
        }
        MoltObject::from_bool(true).bits()
    })
}

/// `molt_dataclasses_hash_fn(self)` → int
///
/// Replaces `_dataclass_hash` in Python. Hashes the tuple of all
/// hash-enabled fields (hash=True or hash=None and compare=True).
#[unsafe(no_mangle)]
pub extern "C" fn molt_dataclasses_hash_fn(self_bits: u64) -> u64 {
    crate::with_gil_entry_nopanic!(_py, {
        let Some(fields) = dc_fields_ordered(_py, self_bits) else {
            return MoltObject::from_int(0).bits();
        };
        let missing = missing_bits(_py);
        let mut values: Vec<u64> = Vec::new();
        for (_name_bits, field_bits) in &fields {
            if !dc_is_field(_py, *field_bits) {
                continue;
            }
            // Check hash attribute: if None, use compare; if explicit bool, use that
            let hash_flag = {
                let Some(hash_attr_name) = attr_name_bits_from_bytes(_py, b"hash") else {
                    continue;
                };
                let hash_val = crate::builtins::attributes::molt_get_attr_name_default(
                    *field_bits,
                    hash_attr_name,
                    missing,
                );
                dec_ref_bits(_py, hash_attr_name);
                if exception_pending(_py) {
                    clear_exception(_py);
                    continue;
                }
                if hash_val == missing || obj_from_bits(hash_val).is_none() {
                    // hash=None → use compare flag
                    dc_field_bool_attr(_py, *field_bits, b"compare").unwrap_or(true)
                } else {
                    is_truthy(_py, obj_from_bits(hash_val))
                }
            };
            if !hash_flag {
                continue;
            }
            let Some(fname) = dc_field_name_str(_py, *field_bits) else {
                continue;
            };
            let Some(fname_key) = attr_name_bits_from_bytes(_py, fname.as_bytes()) else {
                continue;
            };
            let val = crate::builtins::attributes::molt_get_attr_name_default(
                self_bits, fname_key, missing,
            );
            dec_ref_bits(_py, fname_key);
            if exception_pending(_py) {
                clear_exception(_py);
                continue;
            }
            values.push(val);
        }
        // Build a tuple and hash it
        let tuple_ptr = alloc_tuple(_py, &values);
        if tuple_ptr.is_null() {
            return MoltObject::from_int(0).bits();
        }
        let tuple_bits = MoltObject::from_ptr(tuple_ptr).bits();
        let hash_result = molt_hash_builtin(tuple_bits);
        dec_ref_bits(_py, tuple_bits);
        hash_result
    })
}

/// `molt_dataclasses_check_default_order(fields_dict)` → None or raises TypeError
///
/// Replaces `_check_default_order` in Python. Validates that no positional
/// field without a default follows one that has a default.
#[unsafe(no_mangle)]
pub extern "C" fn molt_dataclasses_check_default_order(fields_dict_bits: u64) -> u64 {
    crate::with_gil_entry_nopanic!(_py, {
        let Some(fields_ptr) = obj_from_bits(fields_dict_bits).as_ptr() else {
            return MoltObject::none().bits();
        };
        let order = unsafe { dict_order(fields_ptr) }.clone();
        let missing = missing_bits(_py);
        let mut seen_default_name: Option<String> = None;

        let missing_type_str = b"MISSING";

        for pair in order.chunks(2) {
            if pair.len() != 2 {
                continue;
            }
            let field_bits = pair[1];
            // Check _field_type is _FIELD or _FIELD_INITVAR
            if !dc_field_has_tag(_py, field_bits, b"_FIELD")
                && !dc_field_has_tag(_py, field_bits, b"_FIELD_INITVAR")
            {
                continue;
            }
            // Skip if not init or kw_only
            if !dc_field_bool_attr(_py, field_bits, b"init").unwrap_or(true) {
                continue;
            }
            if dc_field_bool_attr(_py, field_bits, b"kw_only").unwrap_or(false) {
                continue;
            }
            // Check if field has default or default_factory
            let has_default = {
                let mut has = false;
                for attr_name in &[b"default" as &[u8], b"default_factory"] {
                    if let Some(ak) = attr_name_bits_from_bytes(_py, attr_name) {
                        let val = crate::builtins::attributes::molt_get_attr_name_default(
                            field_bits, ak, missing,
                        );
                        dec_ref_bits(_py, ak);
                        if exception_pending(_py) {
                            clear_exception(_py);
                            continue;
                        }
                        if val != missing {
                            // Check if the value is MISSING sentinel
                            let repr = dc_repr_str(_py, val);
                            if repr.as_bytes() != missing_type_str {
                                has = true;
                                break;
                            }
                        }
                    }
                }
                has
            };
            let field_name = dc_field_name_str(_py, field_bits).unwrap_or_default();
            if has_default {
                seen_default_name = Some(field_name);
                continue;
            }
            if let Some(ref prev) = seen_default_name {
                let msg = format!(
                    "non-default argument {field_name:?} follows default argument {prev:?}"
                );
                return raise_exception::<_>(_py, "TypeError", &msg);
            }
        }
        MoltObject::none().bits()
    })
}

/// `molt_dataclasses_field_flags(fields_dict)` → tuple[int, ...]
///
/// Replaces `_dataclass_field_flags` in Python. Returns a tuple of integer
/// flag values for each _FIELD: bit 0 = repr, bit 1 = compare, bit 2 = hash.
#[unsafe(no_mangle)]
pub extern "C" fn molt_dataclasses_field_flags(fields_dict_bits: u64) -> u64 {
    crate::with_gil_entry_nopanic!(_py, {
        let Some(fields_ptr) = obj_from_bits(fields_dict_bits).as_ptr() else {
            let ptr = alloc_tuple(_py, &[]);
            return if ptr.is_null() {
                MoltObject::none().bits()
            } else {
                MoltObject::from_ptr(ptr).bits()
            };
        };
        let order = unsafe { dict_order(fields_ptr) }.clone();
        let mut flags: Vec<u64> = Vec::new();

        for pair in order.chunks(2) {
            if pair.len() != 2 {
                continue;
            }
            let field_bits = pair[1];
            if !dc_is_field(_py, field_bits) {
                continue;
            }
            let mut flag: i64 = 0;
            if dc_field_bool_attr(_py, field_bits, b"repr").unwrap_or(true) {
                flag |= 0x1;
            }
            let compare = dc_field_bool_attr(_py, field_bits, b"compare").unwrap_or(true);
            if compare {
                flag |= 0x2;
            }
            // hash: if None → use compare, otherwise use the explicit bool
            let hash_flag = {
                let missing = missing_bits(_py);
                if let Some(ak) = attr_name_bits_from_bytes(_py, b"hash") {
                    let val = crate::builtins::attributes::molt_get_attr_name_default(
                        field_bits, ak, missing,
                    );
                    dec_ref_bits(_py, ak);
                    if exception_pending(_py) {
                        clear_exception(_py);
                        compare
                    } else if val == missing || obj_from_bits(val).is_none() {
                        compare
                    } else {
                        is_truthy(_py, obj_from_bits(val))
                    }
                } else {
                    compare
                }
            };
            if hash_flag {
                flag |= 0x4;
            }
            flags.push(MoltObject::from_int(flag).bits());
        }
        let ptr = alloc_tuple(_py, &flags);
        if ptr.is_null() {
            return MoltObject::none().bits();
        }
        MoltObject::from_ptr(ptr).bits()
    })
}
