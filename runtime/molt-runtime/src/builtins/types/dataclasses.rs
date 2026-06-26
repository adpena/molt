use super::*;

mod field_runtime;
mod projection;

pub use field_runtime::{
    molt_dataclasses_check_default_order, molt_dataclasses_eq, molt_dataclasses_field_flags,
    molt_dataclasses_hash_fn, molt_dataclasses_repr,
};
pub use projection::{
    molt_dataclasses_asdict, molt_dataclasses_astuple, molt_dataclasses_fields,
    molt_dataclasses_is_dataclass, molt_dataclasses_replace,
};

fn validate_make_dataclass_field_name(_py: &PyToken<'_>, name_bits: u64) -> Option<String> {
    if !isinstance_runtime(_py, name_bits, builtin_classes(_py).str) {
        let _ = raise_exception::<u64>(_py, "TypeError", "Field names must be strings");
        return None;
    }

    let name_str_bits = molt_str_from_obj(name_bits);
    if obj_from_bits(name_str_bits).is_none() {
        return None;
    }

    let ident_bits = molt_string_isidentifier(name_str_bits);
    let is_ident = is_truthy(_py, obj_from_bits(ident_bits));
    if obj_from_bits(ident_bits).as_ptr().is_some() {
        dec_ref_bits(_py, ident_bits);
    }
    if !is_ident || keyword_contains(name_str_bits, HARD_KEYWORDS) {
        let repr_bits = molt_repr_from_obj(name_str_bits);
        let repr = string_obj_to_owned(obj_from_bits(repr_bits)).unwrap_or_default();
        dec_ref_bits(_py, repr_bits);
        dec_ref_bits(_py, name_str_bits);
        let msg = format!("Field names must be valid identifiers: {repr}");
        let _ = raise_exception::<u64>(_py, "TypeError", &msg);
        return None;
    }
    let out = string_obj_to_owned(obj_from_bits(name_str_bits));
    dec_ref_bits(_py, name_str_bits);
    out
}

#[allow(clippy::too_many_arguments)]
#[unsafe(no_mangle)]
pub extern "C" fn molt_dataclasses_make_dataclass(
    cls_name_bits: u64,
    fields_bits: u64,
    bases_bits: u64,
    namespace_bits: u64,
    module_bits: u64,
    default_field_type_bits: u64,
    _field_class_bits: u64,
) -> u64 {
    crate::with_gil_entry_nopanic!(_py, {
        let mut result_bits = MoltObject::none().bits();
        let mut bases_tuple_bits = 0u64;
        let mut body_bits = 0u64;
        let mut annotations_bits = 0u64;
        let mut fields_iter_bits = 0u64;

        'compute: {
            let Some(cls_name_ptr) = obj_from_bits(cls_name_bits).as_ptr() else {
                result_bits = raise_exception::<_>(_py, "TypeError", "cls_name must be a string");
                break 'compute;
            };
            unsafe {
                if object_type_id(cls_name_ptr) != TYPE_ID_STRING {
                    result_bits =
                        raise_exception::<_>(_py, "TypeError", "cls_name must be a string");
                    break 'compute;
                }
            }

            bases_tuple_bits = if obj_from_bits(bases_bits)
                .as_ptr()
                .is_some_and(|ptr| unsafe { object_type_id(ptr) == TYPE_ID_TUPLE })
            {
                inc_ref_bits(_py, bases_bits);
                bases_bits
            } else {
                let Some(bits) = (unsafe { tuple_from_iter_bits(_py, bases_bits) }) else {
                    result_bits = MoltObject::none().bits();
                    break 'compute;
                };
                bits
            };

            if obj_from_bits(namespace_bits).is_none() {
                let dict_ptr = alloc_dict_with_pairs(_py, &[]);
                if dict_ptr.is_null() {
                    result_bits = MoltObject::none().bits();
                    break 'compute;
                }
                body_bits = MoltObject::from_ptr(dict_ptr).bits();
            } else {
                body_bits = molt_dict_from_obj(namespace_bits);
                if obj_from_bits(body_bits).is_none() {
                    result_bits = MoltObject::none().bits();
                    break 'compute;
                }
            }
            let Some(body_ptr) = obj_from_bits(body_bits).as_ptr() else {
                result_bits = MoltObject::none().bits();
                break 'compute;
            };
            unsafe {
                if object_type_id(body_ptr) != TYPE_ID_DICT {
                    result_bits = MoltObject::none().bits();
                    break 'compute;
                }
            }
            let Some(init_name_bits) = attr_name_bits_from_bytes(_py, b"__init__") else {
                result_bits = MoltObject::none().bits();
                break 'compute;
            };
            let has_user_init =
                unsafe { dict_get_in_place(_py, body_ptr, init_name_bits) }.is_some();
            dec_ref_bits(_py, init_name_bits);
            let Some(user_init_marker_bits) =
                attr_name_bits_from_bytes(_py, b"__molt_dataclass_user_init__")
            else {
                result_bits = MoltObject::none().bits();
                break 'compute;
            };
            unsafe {
                dict_set_in_place(
                    _py,
                    body_ptr,
                    user_init_marker_bits,
                    MoltObject::from_bool(has_user_init).bits(),
                );
            }
            dec_ref_bits(_py, user_init_marker_bits);
            if exception_pending(_py) {
                result_bits = MoltObject::none().bits();
                break 'compute;
            }
            let Some(make_dataclass_marker_bits) =
                attr_name_bits_from_bytes(_py, b"__molt_make_dataclass__")
            else {
                result_bits = MoltObject::none().bits();
                break 'compute;
            };
            unsafe {
                dict_set_in_place(
                    _py,
                    body_ptr,
                    make_dataclass_marker_bits,
                    MoltObject::from_bool(true).bits(),
                );
            }
            dec_ref_bits(_py, make_dataclass_marker_bits);
            if exception_pending(_py) {
                result_bits = MoltObject::none().bits();
                break 'compute;
            }

            let Some(annotations_name_bits) = attr_name_bits_from_bytes(_py, b"__annotations__")
            else {
                result_bits = MoltObject::none().bits();
                break 'compute;
            };
            let existing_annotations_bits =
                unsafe { dict_get_in_place(_py, body_ptr, annotations_name_bits) };
            dec_ref_bits(_py, annotations_name_bits);
            annotations_bits = if let Some(bits) = existing_annotations_bits {
                let Some(existing_ptr) = obj_from_bits(bits).as_ptr() else {
                    result_bits =
                        raise_exception::<_>(_py, "TypeError", "__annotations__ must be a dict");
                    break 'compute;
                };
                unsafe {
                    if object_type_id(existing_ptr) != TYPE_ID_DICT {
                        result_bits = raise_exception::<_>(
                            _py,
                            "TypeError",
                            "__annotations__ must be a dict",
                        );
                        break 'compute;
                    }
                }
                let copied_bits = molt_dict_from_obj(bits);
                if obj_from_bits(copied_bits).is_none() {
                    result_bits = MoltObject::none().bits();
                    break 'compute;
                }
                copied_bits
            } else {
                let dict_ptr = alloc_dict_with_pairs(_py, &[]);
                if dict_ptr.is_null() {
                    result_bits = MoltObject::none().bits();
                    break 'compute;
                }
                MoltObject::from_ptr(dict_ptr).bits()
            };
            let Some(annotations_ptr) = obj_from_bits(annotations_bits).as_ptr() else {
                result_bits = MoltObject::none().bits();
                break 'compute;
            };
            unsafe {
                if object_type_id(annotations_ptr) != TYPE_ID_DICT {
                    result_bits = MoltObject::none().bits();
                    break 'compute;
                }
            }

            let mut seen: HashSet<String> = HashSet::new();
            let annotation_order = unsafe { dict_order(annotations_ptr) }.clone();
            for pair in annotation_order.chunks(2) {
                if pair.len() != 2 {
                    continue;
                }
                if let Some(name) = string_obj_to_owned(obj_from_bits(pair[0])) {
                    seen.insert(name);
                }
            }

            fields_iter_bits = molt_iter(fields_bits);
            if exception_pending(_py) {
                result_bits = MoltObject::none().bits();
                break 'compute;
            }
            loop {
                let Some((field_spec_bits, done)) = iter_next_pair(_py, fields_iter_bits) else {
                    result_bits = MoltObject::none().bits();
                    break 'compute;
                };
                if done {
                    break;
                }

                let mut raw_name_bits = field_spec_bits;
                let mut field_type_bits = default_field_type_bits;
                let mut default_value_bits = 0u64;
                let mut has_default_value = false;
                let invalid_spec_msg = "Invalid field specification: must be name, (name, type), or (name, type, Field)";

                let Some(field_spec_ptr) = obj_from_bits(field_spec_bits).as_ptr() else {
                    result_bits = raise_exception::<_>(_py, "TypeError", invalid_spec_msg);
                    break 'compute;
                };

                unsafe {
                    match object_type_id(field_spec_ptr) {
                        TYPE_ID_STRING => {}
                        TYPE_ID_TUPLE | TYPE_ID_LIST => {
                            let parts = seq_vec_ref(field_spec_ptr).clone();
                            if parts.len() == 2 {
                                raw_name_bits = parts[0];
                                field_type_bits = parts[1];
                            } else if parts.len() == 3 {
                                raw_name_bits = parts[0];
                                field_type_bits = parts[1];
                                default_value_bits = parts[2];
                                has_default_value = true;
                            } else {
                                result_bits =
                                    raise_exception::<_>(_py, "TypeError", invalid_spec_msg);
                                break 'compute;
                            }
                        }
                        _ => {
                            result_bits = raise_exception::<_>(_py, "TypeError", invalid_spec_msg);
                            break 'compute;
                        }
                    }
                }

                let Some(field_name) = validate_make_dataclass_field_name(_py, raw_name_bits)
                else {
                    result_bits = MoltObject::none().bits();
                    break 'compute;
                };

                if seen.contains(field_name.as_str()) {
                    let field_name_repr_bits = molt_repr_from_obj(raw_name_bits);
                    let field_name_repr = string_obj_to_owned(obj_from_bits(field_name_repr_bits))
                        .unwrap_or_default();
                    dec_ref_bits(_py, field_name_repr_bits);
                    let msg = format!("Field name duplicated: {field_name_repr}");
                    result_bits = raise_exception::<_>(_py, "TypeError", &msg);
                    break 'compute;
                }
                seen.insert(field_name.clone());

                let key_ptr = alloc_string(_py, field_name.as_bytes());
                if key_ptr.is_null() {
                    result_bits = MoltObject::none().bits();
                    break 'compute;
                }
                let key_bits = MoltObject::from_ptr(key_ptr).bits();
                unsafe {
                    dict_set_in_place(_py, annotations_ptr, key_bits, field_type_bits);
                }
                if exception_pending(_py) {
                    dec_ref_bits(_py, key_bits);
                    result_bits = MoltObject::none().bits();
                    break 'compute;
                }
                if has_default_value {
                    unsafe {
                        dict_set_in_place(_py, body_ptr, key_bits, default_value_bits);
                    }
                    if exception_pending(_py) {
                        dec_ref_bits(_py, key_bits);
                        result_bits = MoltObject::none().bits();
                        break 'compute;
                    }
                }
                dec_ref_bits(_py, key_bits);
            }
            if exception_pending(_py) {
                break 'compute;
            }

            let Some(annotations_key_bits) = attr_name_bits_from_bytes(_py, b"__annotations__")
            else {
                result_bits = MoltObject::none().bits();
                break 'compute;
            };
            unsafe {
                dict_set_in_place(_py, body_ptr, annotations_key_bits, annotations_bits);
            }
            dec_ref_bits(_py, annotations_key_bits);
            if exception_pending(_py) {
                result_bits = MoltObject::none().bits();
                break 'compute;
            }

            let Some(molt_dataclass_name_bits) =
                attr_name_bits_from_bytes(_py, b"__molt_dataclass__")
            else {
                result_bits = MoltObject::none().bits();
                break 'compute;
            };
            let has_molt_dataclass =
                unsafe { dict_get_in_place(_py, body_ptr, molt_dataclass_name_bits) }.is_some();
            if !has_molt_dataclass {
                unsafe {
                    dict_set_in_place(
                        _py,
                        body_ptr,
                        molt_dataclass_name_bits,
                        MoltObject::from_bool(true).bits(),
                    );
                }
            }
            dec_ref_bits(_py, molt_dataclass_name_bits);
            if exception_pending(_py) {
                result_bits = MoltObject::none().bits();
                break 'compute;
            }

            let Some(module_name_bits) = attr_name_bits_from_bytes(_py, b"__module__") else {
                result_bits = MoltObject::none().bits();
                break 'compute;
            };
            let has_module =
                unsafe { dict_get_in_place(_py, body_ptr, module_name_bits) }.is_some();
            if !has_module {
                unsafe {
                    dict_set_in_place(_py, body_ptr, module_name_bits, module_bits);
                }
            }
            dec_ref_bits(_py, module_name_bits);
            if exception_pending(_py) {
                result_bits = MoltObject::none().bits();
                break 'compute;
            }

            let out_ptr = alloc_tuple(_py, &[bases_tuple_bits, body_bits]);
            if out_ptr.is_null() {
                result_bits = MoltObject::none().bits();
                break 'compute;
            }
            result_bits = MoltObject::from_ptr(out_ptr).bits();
        }

        if !obj_from_bits(fields_iter_bits).is_none() {
            dec_ref_bits(_py, fields_iter_bits);
        }
        if !obj_from_bits(annotations_bits).is_none() {
            dec_ref_bits(_py, annotations_bits);
        }
        if !obj_from_bits(body_bits).is_none() {
            dec_ref_bits(_py, body_bits);
        }
        if !obj_from_bits(bases_tuple_bits).is_none() {
            dec_ref_bits(_py, bases_tuple_bits);
        }

        result_bits
    })
}

// ─────────────────────────────────────────────────────────────────────────────
// __post_init__ support
// ─────────────────────────────────────────────────────────────────────────────

/// `molt_dataclasses_post_init(instance, *initvar_values) -> None`
///
/// Calls `instance.__post_init__(*initvar_values)` if it exists.  This is
/// invoked at the end of the generated `__init__` for dataclasses that define
/// a `__post_init__` method.
///
/// `initvar_values_bits` is a tuple of the InitVar field values in declaration
/// order.  If the instance has no `__post_init__` method, this is a no-op.
#[unsafe(no_mangle)]
pub extern "C" fn molt_dataclasses_post_init(instance_bits: u64, initvar_values_bits: u64) -> u64 {
    crate::with_gil_entry_nopanic!(_py, {
        let missing = missing_bits(_py);

        // Look up __post_init__ on the instance.
        let Some(post_init_name_bits) = attr_name_bits_from_bytes(_py, b"__post_init__") else {
            return MoltObject::none().bits();
        };
        let method_bits = crate::builtins::attributes::molt_get_attr_name_default(
            instance_bits,
            post_init_name_bits,
            missing,
        );
        dec_ref_bits(_py, post_init_name_bits);

        if exception_pending(_py) {
            // __post_init__ not found or attribute error — clear and return.
            if !crate::builtins::attr::clear_attribute_error_if_pending(_py) {
                return MoltObject::none().bits();
            }
            return MoltObject::none().bits();
        }
        if method_bits == missing {
            // No __post_init__ method — nothing to do.
            return MoltObject::none().bits();
        }

        // Call __post_init__ with the InitVar values.
        // initvar_values_bits may be a tuple (possibly empty) or None.
        let has_args = obj_from_bits(initvar_values_bits)
            .as_ptr()
            .is_some_and(|ptr| unsafe {
                let ty = object_type_id(ptr);
                (ty == TYPE_ID_TUPLE || ty == TYPE_ID_LIST) && !seq_vec_ref(ptr).is_empty()
            });

        let result_bits = if has_args {
            // Build a call with positional args from the tuple.
            let args_ptr = obj_from_bits(initvar_values_bits).as_ptr().unwrap();
            let args = unsafe { seq_vec_ref(args_ptr) };
            // Use the CallArgs builder to push positional args.
            let builder_bits = crate::molt_callargs_new(args.len() as u64, 0);
            for &arg_bits in args.iter() {
                unsafe {
                    let _ = crate::molt_callargs_push_pos(builder_bits, arg_bits);
                }
            }
            crate::molt_call_bind(method_bits, builder_bits)
        } else {
            // No initvar args — call with zero args.
            unsafe { call_callable0(_py, method_bits) }
        };

        dec_ref_bits(_py, method_bits);

        if exception_pending(_py) {
            return MoltObject::none().bits();
        }

        // __post_init__ return value is discarded.
        if obj_from_bits(result_bits).as_ptr().is_some() {
            dec_ref_bits(_py, result_bits);
        }

        MoltObject::none().bits()
    })
}

// ─────────────────────────────────────────────────────────────────────────────
// field() metadata support
// ─────────────────────────────────────────────────────────────────────────────

/// `molt_dataclasses_field_metadata(field_obj) -> MappingProxy | empty dict`
///
/// Returns the `metadata` attribute of a Field object.  If the field has no
/// metadata or metadata is None, returns an empty dict (matching CPython's
/// behaviour where metadata defaults to `types.MappingProxyType({})`).
#[unsafe(no_mangle)]
pub extern "C" fn molt_dataclasses_field_metadata(field_bits: u64) -> u64 {
    crate::with_gil_entry_nopanic!(_py, {
        let missing = missing_bits(_py);
        let Some(meta_name_bits) = attr_name_bits_from_bytes(_py, b"metadata") else {
            // Allocation failure — return empty dict.
            let ptr = alloc_dict_with_pairs(_py, &[]);
            return if ptr.is_null() {
                MoltObject::none().bits()
            } else {
                MoltObject::from_ptr(ptr).bits()
            };
        };
        let meta_bits = molt_getattr_builtin(field_bits, meta_name_bits, missing);
        dec_ref_bits(_py, meta_name_bits);

        if exception_pending(_py) {
            clear_exception(_py);
            let ptr = alloc_dict_with_pairs(_py, &[]);
            return if ptr.is_null() {
                MoltObject::none().bits()
            } else {
                MoltObject::from_ptr(ptr).bits()
            };
        }
        if meta_bits == missing || obj_from_bits(meta_bits).is_none() {
            let ptr = alloc_dict_with_pairs(_py, &[]);
            return if ptr.is_null() {
                MoltObject::none().bits()
            } else {
                MoltObject::from_ptr(ptr).bits()
            };
        }
        // Return the metadata value as-is (should already be a MappingProxy or dict).
        meta_bits
    })
}

/// `molt_dataclasses_set_field_metadata(field_obj, metadata_dict) -> None`
///
/// Sets the `metadata` attribute on a Field object, wrapping the given dict
/// in a `types.MappingProxyType` if it isn't already one.  If `metadata_dict`
/// is None or empty, sets an empty MappingProxy.
#[unsafe(no_mangle)]
pub extern "C" fn molt_dataclasses_set_field_metadata(field_bits: u64, metadata_bits: u64) -> u64 {
    crate::with_gil_entry_nopanic!(_py, {
        let Some(meta_name_bits) = attr_name_bits_from_bytes(_py, b"metadata") else {
            return MoltObject::none().bits();
        };

        // If metadata is None, set an empty dict.
        let val_bits = if obj_from_bits(metadata_bits).is_none() {
            let ptr = alloc_dict_with_pairs(_py, &[]);
            if ptr.is_null() {
                dec_ref_bits(_py, meta_name_bits);
                return MoltObject::none().bits();
            }
            MoltObject::from_ptr(ptr).bits()
        } else {
            metadata_bits
        };

        let _ = crate::molt_object_setattr(field_bits, meta_name_bits, val_bits);
        dec_ref_bits(_py, meta_name_bits);

        MoltObject::none().bits()
    })
}

// ─────────────────────────────────────────────────────────────────────────────
// InitVar / KW_ONLY sentinel support
// ─────────────────────────────────────────────────────────────────────────────

/// `molt_dataclasses_is_initvar(obj) -> bool`
///
/// Checks if `obj` is an InitVar descriptor (has `__molt_initvar__` marker or
/// its class name is "InitVar").
#[unsafe(no_mangle)]
pub extern "C" fn molt_dataclasses_is_initvar(obj_bits: u64) -> u64 {
    crate::with_gil_entry_nopanic!(_py, {
        let missing = missing_bits(_py);

        // Check for __molt_initvar__ marker attribute.
        if let Some(marker_bits) = attr_name_bits_from_bytes(_py, b"__molt_initvar__") {
            let val = crate::builtins::attributes::molt_get_attr_name_default(
                obj_bits,
                marker_bits,
                missing,
            );
            dec_ref_bits(_py, marker_bits);
            if exception_pending(_py) {
                clear_exception(_py);
            } else if val != missing && is_truthy(_py, obj_from_bits(val)) {
                return MoltObject::from_bool(true).bits();
            }
        }

        // Fall back: check class name.
        let cls_bits = type_of_bits(_py, obj_bits);
        if let Some(name_bits) = attr_name_bits_from_bytes(_py, b"__name__") {
            let name_val = crate::builtins::attributes::molt_get_attr_name_default(
                cls_bits, name_bits, missing,
            );
            dec_ref_bits(_py, name_bits);
            if !exception_pending(_py)
                && name_val != missing
                && let Some(name_str) = string_obj_to_owned(obj_from_bits(name_val))
                && name_str == "InitVar"
            {
                return MoltObject::from_bool(true).bits();
            }
            if exception_pending(_py) {
                clear_exception(_py);
            }
        }

        MoltObject::from_bool(false).bits()
    })
}

/// `molt_dataclasses_is_kw_only_sentinel(obj) -> bool`
///
/// Checks if `obj` is the KW_ONLY sentinel (has `__molt_kw_only__` marker or
/// its class name is "KW_ONLY" in the `dataclasses` module).
#[unsafe(no_mangle)]
pub extern "C" fn molt_dataclasses_is_kw_only_sentinel(obj_bits: u64) -> u64 {
    crate::with_gil_entry_nopanic!(_py, {
        let missing = missing_bits(_py);

        // Check for __molt_kw_only__ marker attribute.
        if let Some(marker_bits) = attr_name_bits_from_bytes(_py, b"__molt_kw_only__") {
            let val = crate::builtins::attributes::molt_get_attr_name_default(
                obj_bits,
                marker_bits,
                missing,
            );
            dec_ref_bits(_py, marker_bits);
            if exception_pending(_py) {
                clear_exception(_py);
            } else if val != missing && is_truthy(_py, obj_from_bits(val)) {
                return MoltObject::from_bool(true).bits();
            }
        }

        // Fall back: check class name.
        let cls_bits = type_of_bits(_py, obj_bits);
        if let Some(name_bits) = attr_name_bits_from_bytes(_py, b"__name__") {
            let name_val = crate::builtins::attributes::molt_get_attr_name_default(
                cls_bits, name_bits, missing,
            );
            dec_ref_bits(_py, name_bits);
            if !exception_pending(_py)
                && name_val != missing
                && let Some(name_str) = string_obj_to_owned(obj_from_bits(name_val))
                && (name_str == "KW_ONLY" || name_str == "_KW_ONLY_TYPE")
            {
                return MoltObject::from_bool(true).bits();
            }
            if exception_pending(_py) {
                clear_exception(_py);
            }
        }

        MoltObject::from_bool(false).bits()
    })
}
