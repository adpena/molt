use super::*;
use crate::BuiltinExc;
use crate::object::seq_access::snapshot;

mod field_runtime;
mod initializer;
mod initializer_spec_generated;
mod projection;

pub(crate) use initializer::{clone_code_init_plan, drop_code_init_plan};
pub use initializer::{
    molt_dataclasses_init, molt_dataclasses_init_globals, molt_dataclasses_init_trampoline,
    molt_dataclasses_set_init_template,
};

pub use field_runtime::{molt_dataclasses_eq, molt_dataclasses_hash_fn, molt_dataclasses_repr};
pub use projection::{
    molt_dataclasses_asdict, molt_dataclasses_astuple, molt_dataclasses_fields,
    molt_dataclasses_is_dataclass, molt_dataclasses_replace,
};

fn validate_make_dataclass_field_name(_py: &PyToken<'_>, name_bits: u64) -> Option<String> {
    if !isinstance_runtime(_py, name_bits, builtin_classes(_py).str)? {
        let _ = raise_exception::<u64>(_py, BuiltinExc::TypeError, "Field names must be strings");
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
        let _ = raise_exception::<u64>(_py, BuiltinExc::TypeError, &msg);
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
        let mut fields_iterator = None;

        'compute: {
            let Some(cls_name_ptr) = obj_from_bits(cls_name_bits).as_ptr() else {
                result_bits =
                    raise_exception::<_>(_py, BuiltinExc::TypeError, "cls_name must be a string");
                break 'compute;
            };
            unsafe {
                if object_type_id(cls_name_ptr) != TYPE_ID_STRING {
                    result_bits = raise_exception::<_>(
                        _py,
                        BuiltinExc::TypeError,
                        "cls_name must be a string",
                    );
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
            let Some(annotations_name_bits) = crate::NameId::DunderAnnotations.owned_bits(_py)
            else {
                result_bits = MoltObject::none().bits();
                break 'compute;
            };
            let existing_annotations_bits =
                unsafe { dict_get_in_place(_py, body_ptr, annotations_name_bits) };
            dec_ref_bits(_py, annotations_name_bits);
            let existing_annotations_bits = match existing_annotations_bits {
                Ok(value) => value,
                Err(()) => {
                    result_bits = MoltObject::none().bits();
                    break 'compute;
                }
            };
            annotations_bits = if let Some(bits) = existing_annotations_bits {
                let Some(existing_ptr) = obj_from_bits(bits).as_ptr() else {
                    result_bits = raise_exception::<_>(
                        _py,
                        BuiltinExc::TypeError,
                        "__annotations__ must be a dict",
                    );
                    break 'compute;
                };
                unsafe {
                    if object_type_id(existing_ptr) != TYPE_ID_DICT {
                        result_bits = raise_exception::<_>(
                            _py,
                            BuiltinExc::TypeError,
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
            let Some(annotation_order) = (unsafe {
                crate::object::ops_dict::dict_snapshot(
                    _py,
                    annotations_ptr,
                    crate::object::ops_dict::DictSnapshotKind::Entries,
                )
            }) else {
                return MoltObject::none().bits();
            };
            for pair in annotation_order.chunks(2) {
                if pair.len() != 2 {
                    continue;
                }
                if let Some(name) = string_obj_to_owned(obj_from_bits(pair[0])) {
                    seen.insert(name);
                }
            }

            let fields_iter_bits = molt_iter(fields_bits);
            fields_iterator = Some(unsafe {
                molt_runtime_core::OwnedRuntimeValue::from_owned_bits(
                    _py.core_token(),
                    fields_iter_bits,
                )
            });
            if exception_pending(_py) {
                result_bits = MoltObject::none().bits();
                break 'compute;
            }
            loop {
                let field_spec_owner = match molt_runtime_core::iter_next_owned(
                    _py.core_token(),
                    fields_iterator.as_ref().expect("acquired fields iterator"),
                ) {
                    Ok(Some(value)) => value,
                    Ok(None) => break,
                    Err(_) => {
                        result_bits = MoltObject::none().bits();
                        break 'compute;
                    }
                };
                let field_spec_bits = field_spec_owner.bits();

                let mut raw_name_bits = field_spec_bits;
                let mut field_type_bits = default_field_type_bits;
                let mut default_value_bits = 0u64;
                let mut has_default_value = false;
                let mut _field_parts = None;
                let invalid_spec_msg = "Invalid field specification: must be name, (name, type), or (name, type, Field)";

                let Some(field_spec_ptr) = obj_from_bits(field_spec_bits).as_ptr() else {
                    result_bits =
                        raise_exception::<_>(_py, BuiltinExc::TypeError, invalid_spec_msg);
                    break 'compute;
                };

                unsafe {
                    match object_type_id(field_spec_ptr) {
                        TYPE_ID_STRING => {}
                        TYPE_ID_TUPLE | TYPE_ID_LIST => {
                            let Some(parts) = snapshot(
                                _py,
                                field_spec_ptr,
                                "dataclass field specification allocation failed",
                            ) else {
                                result_bits = MoltObject::none().bits();
                                break 'compute;
                            };
                            if parts.len() == 2 {
                                raw_name_bits = parts[0];
                                field_type_bits = parts[1];
                            } else if parts.len() == 3 {
                                raw_name_bits = parts[0];
                                field_type_bits = parts[1];
                                default_value_bits = parts[2];
                                has_default_value = true;
                            } else {
                                result_bits = raise_exception::<_>(
                                    _py,
                                    BuiltinExc::TypeError,
                                    invalid_spec_msg,
                                );
                                break 'compute;
                            }
                            _field_parts = Some(parts);
                        }
                        _ => {
                            result_bits =
                                raise_exception::<_>(_py, BuiltinExc::TypeError, invalid_spec_msg);
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
                    result_bits = raise_exception::<_>(_py, BuiltinExc::TypeError, &msg);
                    break 'compute;
                }
                seen.insert(field_name.clone());

                let key_ptr = alloc_string(_py, field_name.as_bytes());
                if key_ptr.is_null() {
                    result_bits = MoltObject::none().bits();
                    break 'compute;
                }
                let key_bits = MoltObject::from_ptr(key_ptr).bits();
                let completed =
                    unsafe { dict_set_in_place(_py, annotations_ptr, key_bits, field_type_bits) };
                if !completed {
                    dec_ref_bits(_py, key_bits);
                    result_bits = MoltObject::none().bits();
                    break 'compute;
                }
                if has_default_value {
                    let completed =
                        unsafe { dict_set_in_place(_py, body_ptr, key_bits, default_value_bits) };
                    if !completed {
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

            let Some(annotations_key_bits) = crate::NameId::DunderAnnotations.owned_bits(_py)
            else {
                result_bits = MoltObject::none().bits();
                break 'compute;
            };
            let completed =
                unsafe { dict_set_in_place(_py, body_ptr, annotations_key_bits, annotations_bits) };
            dec_ref_bits(_py, annotations_key_bits);
            if !completed {
                result_bits = MoltObject::none().bits();
                break 'compute;
            }

            let Some(module_name_bits) = crate::NameId::DunderModule.owned_bits(_py) else {
                result_bits = MoltObject::none().bits();
                break 'compute;
            };
            let completed = match unsafe { dict_get_in_place(_py, body_ptr, module_name_bits) } {
                Ok(Some(_)) => true,
                Ok(None) => unsafe {
                    dict_set_in_place(_py, body_ptr, module_name_bits, module_bits)
                },
                Err(()) => false,
            };
            dec_ref_bits(_py, module_name_bits);
            if !completed {
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

        drop(fields_iterator);
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

/// `molt_dataclasses_set_field_metadata(field_obj, metadata_dict) -> None`
///
/// Sets the `metadata` attribute on a Field object, wrapping the given dict
/// in a `types.MappingProxyType` if it isn't already one.  If `metadata_dict`
/// is None or empty, sets an empty MappingProxy.
#[unsafe(no_mangle)]
pub extern "C" fn molt_dataclasses_set_field_metadata(field_bits: u64, metadata_bits: u64) -> u64 {
    crate::with_gil_entry_nopanic!(_py, {
        let Some(meta_name_bits) = crate::NameId::Metadata.owned_bits(_py) else {
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

        let _ = crate::molt_set_attr_name(field_bits, meta_name_bits, val_bits);
        dec_ref_bits(_py, meta_name_bits);

        MoltObject::none().bits()
    })
}
