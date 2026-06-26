use super::field_runtime::dc_getattr_default_bits;
use super::*;

fn dataclasses_class_bits(_py: &PyToken<'_>, obj_bits: u64) -> u64 {
    if obj_from_bits(obj_bits)
        .as_ptr()
        .is_some_and(|ptr| unsafe { object_type_id(ptr) == TYPE_ID_TYPE })
    {
        obj_bits
    } else {
        type_of_bits(_py, obj_bits)
    }
}

pub(in crate::builtins::types::dataclasses) fn dataclasses_fields_dict_bits(
    _py: &PyToken<'_>,
    cls_bits: u64,
    missing: u64,
) -> Option<u64> {
    let fields_bits = dc_getattr_default_bits(_py, cls_bits, b"__dataclass_fields__", missing)?;
    if exception_pending(_py) {
        clear_exception(_py);
        return None;
    }
    if fields_bits == missing {
        return None;
    }
    let fields_ptr = obj_from_bits(fields_bits).as_ptr()?;
    unsafe {
        if object_type_id(fields_ptr) != TYPE_ID_DICT {
            return None;
        }
    }
    Some(fields_bits)
}

fn dataclasses_collect_fields_by_tag(
    _py: &PyToken<'_>,
    fields_dict_bits: u64,
    field_tag_bits: u64,
) -> Option<Vec<u64>> {
    let fields_dict_ptr = obj_from_bits(fields_dict_bits).as_ptr()?;
    let missing = missing_bits(_py);
    let mut out: Vec<u64> = Vec::new();
    let order = unsafe { dict_order(fields_dict_ptr) }.clone();
    for pair in order.chunks(2) {
        if pair.len() != 2 {
            continue;
        }
        let field_obj_bits = pair[1];
        let tag_bits = dc_getattr_default_bits(_py, field_obj_bits, b"_field_type", missing)?;
        if exception_pending(_py) {
            return None;
        }
        if tag_bits == field_tag_bits {
            out.push(field_obj_bits);
        }
    }
    Some(out)
}

fn dataclasses_is_dataclass_instance(_py: &PyToken<'_>, obj_bits: u64, missing: u64) -> bool {
    let Some(obj_ptr) = obj_from_bits(obj_bits).as_ptr() else {
        return false;
    };
    unsafe {
        match object_type_id(obj_ptr) {
            TYPE_ID_TYPE | TYPE_ID_LIST | TYPE_ID_TUPLE | TYPE_ID_DICT => return false,
            _ => {}
        }
    }
    let cls_bits = type_of_bits(_py, obj_bits);
    dataclasses_fields_dict_bits(_py, cls_bits, missing).is_some()
}

fn dataclasses_deepcopy(_py: &PyToken<'_>, value_bits: u64) -> u64 {
    // Immediate scalar values are immutable and represented without object pointers.
    if obj_from_bits(value_bits).as_ptr().is_none() {
        inc_ref_bits(_py, value_bits);
        return value_bits;
    }
    let Some(value_ptr) = obj_from_bits(value_bits).as_ptr() else {
        return MoltObject::none().bits();
    };
    unsafe {
        let ty = object_type_id(value_ptr);
        if matches!(
            ty,
            TYPE_ID_STRING
                | TYPE_ID_BYTES
                | TYPE_ID_RANGE
                | TYPE_ID_TYPE
                | TYPE_ID_NOT_IMPLEMENTED
                | TYPE_ID_ELLIPSIS
                | TYPE_ID_COMPLEX
        ) {
            inc_ref_bits(_py, value_bits);
            return value_bits;
        }
    }

    let memo_ptr = alloc_dict_with_pairs(_py, &[]);
    if memo_ptr.is_null() {
        return MoltObject::none().bits();
    }
    let memo_bits = MoltObject::from_ptr(memo_ptr).bits();
    let Some(deepcopy_name_bits) = attr_name_bits_from_bytes(_py, b"__deepcopy__") else {
        dec_ref_bits(_py, memo_bits);
        return MoltObject::none().bits();
    };
    let missing = missing_bits(_py);
    let deepcopy_bits = molt_getattr_builtin(value_bits, deepcopy_name_bits, missing);
    dec_ref_bits(_py, deepcopy_name_bits);
    if exception_pending(_py) {
        if !crate::builtins::attr::clear_attribute_error_if_pending(_py) {
            dec_ref_bits(_py, memo_bits);
            return MoltObject::none().bits();
        }
        dec_ref_bits(_py, memo_bits);
        inc_ref_bits(_py, value_bits);
        return value_bits;
    }
    if deepcopy_bits == missing {
        dec_ref_bits(_py, memo_bits);
        inc_ref_bits(_py, value_bits);
        return value_bits;
    }
    let out_bits = unsafe { call_callable1(_py, deepcopy_bits, memo_bits) };
    dec_ref_bits(_py, deepcopy_bits);
    dec_ref_bits(_py, memo_bits);
    out_bits
}

fn dataclasses_asdict_inner(
    _py: &PyToken<'_>,
    value_bits: u64,
    dict_factory_bits: u64,
    field_tag_bits: u64,
) -> u64 {
    let missing = missing_bits(_py);
    if dataclasses_is_dataclass_instance(_py, value_bits, missing) {
        let cls_bits = type_of_bits(_py, value_bits);
        let Some(fields_dict_bits) = dataclasses_fields_dict_bits(_py, cls_bits, missing) else {
            return MoltObject::none().bits();
        };
        let Some(field_objs) =
            dataclasses_collect_fields_by_tag(_py, fields_dict_bits, field_tag_bits)
        else {
            return MoltObject::none().bits();
        };
        let Some(name_name_bits) = attr_name_bits_from_bytes(_py, b"name") else {
            return MoltObject::none().bits();
        };
        let mut item_bits: Vec<u64> = Vec::with_capacity(field_objs.len());
        for field_obj_bits in field_objs {
            let name_bits = molt_getattr_builtin(field_obj_bits, name_name_bits, missing);
            if exception_pending(_py) || name_bits == missing {
                dec_ref_bits(_py, name_name_bits);
                for bits in item_bits {
                    dec_ref_bits(_py, bits);
                }
                return MoltObject::none().bits();
            }
            let field_val_bits = molt_getattr_builtin(value_bits, name_bits, missing);
            if exception_pending(_py) || field_val_bits == missing {
                dec_ref_bits(_py, name_name_bits);
                for bits in item_bits {
                    dec_ref_bits(_py, bits);
                }
                return MoltObject::none().bits();
            }
            let copied_bits =
                dataclasses_asdict_inner(_py, field_val_bits, dict_factory_bits, field_tag_bits);
            if obj_from_bits(copied_bits).is_none() {
                dec_ref_bits(_py, name_name_bits);
                for bits in item_bits {
                    dec_ref_bits(_py, bits);
                }
                return MoltObject::none().bits();
            }
            let pair_ptr = alloc_tuple(_py, &[name_bits, copied_bits]);
            dec_ref_bits(_py, copied_bits);
            if pair_ptr.is_null() {
                dec_ref_bits(_py, name_name_bits);
                for bits in item_bits {
                    dec_ref_bits(_py, bits);
                }
                return MoltObject::none().bits();
            }
            item_bits.push(MoltObject::from_ptr(pair_ptr).bits());
        }
        dec_ref_bits(_py, name_name_bits);
        let items_list_ptr = alloc_list(_py, item_bits.as_slice());
        for bits in item_bits {
            dec_ref_bits(_py, bits);
        }
        if items_list_ptr.is_null() {
            return MoltObject::none().bits();
        }
        let items_list_bits = MoltObject::from_ptr(items_list_ptr).bits();
        let out_bits = unsafe { call_callable1(_py, dict_factory_bits, items_list_bits) };
        dec_ref_bits(_py, items_list_bits);
        return out_bits;
    }

    let Some(value_ptr) = obj_from_bits(value_bits).as_ptr() else {
        return dataclasses_deepcopy(_py, value_bits);
    };
    unsafe {
        match object_type_id(value_ptr) {
            TYPE_ID_LIST => {
                let elems = seq_vec_ref(value_ptr).clone();
                let mut copied: Vec<u64> = Vec::with_capacity(elems.len());
                for elem_bits in elems {
                    let inner_bits =
                        dataclasses_asdict_inner(_py, elem_bits, dict_factory_bits, field_tag_bits);
                    if obj_from_bits(inner_bits).is_none() {
                        for bits in copied {
                            dec_ref_bits(_py, bits);
                        }
                        return MoltObject::none().bits();
                    }
                    copied.push(inner_bits);
                }
                let copied_list_ptr = alloc_list(_py, copied.as_slice());
                for bits in copied {
                    dec_ref_bits(_py, bits);
                }
                if copied_list_ptr.is_null() {
                    return MoltObject::none().bits();
                }
                let copied_list_bits = MoltObject::from_ptr(copied_list_ptr).bits();
                let cls_bits = type_of_bits(_py, value_bits);
                let out_bits = call_callable1(_py, cls_bits, copied_list_bits);
                dec_ref_bits(_py, copied_list_bits);
                return out_bits;
            }
            TYPE_ID_TUPLE => {
                let elems = seq_vec_ref(value_ptr).clone();
                let mut copied: Vec<u64> = Vec::with_capacity(elems.len());
                for elem_bits in elems {
                    let inner_bits =
                        dataclasses_asdict_inner(_py, elem_bits, dict_factory_bits, field_tag_bits);
                    if obj_from_bits(inner_bits).is_none() {
                        for bits in copied {
                            dec_ref_bits(_py, bits);
                        }
                        return MoltObject::none().bits();
                    }
                    copied.push(inner_bits);
                }
                let copied_list_ptr = alloc_list(_py, copied.as_slice());
                for bits in copied {
                    dec_ref_bits(_py, bits);
                }
                if copied_list_ptr.is_null() {
                    return MoltObject::none().bits();
                }
                let copied_list_bits = MoltObject::from_ptr(copied_list_ptr).bits();
                let cls_bits = type_of_bits(_py, value_bits);
                let out_bits = call_callable1(_py, cls_bits, copied_list_bits);
                dec_ref_bits(_py, copied_list_bits);
                return out_bits;
            }
            TYPE_ID_DICT => {
                let mut pair_bits: Vec<u64> = Vec::new();
                let order = dict_order(value_ptr).clone();
                for pair in order.chunks(2) {
                    if pair.len() != 2 {
                        continue;
                    }
                    let key_bits =
                        dataclasses_asdict_inner(_py, pair[0], dict_factory_bits, field_tag_bits);
                    if obj_from_bits(key_bits).is_none() {
                        for bits in pair_bits {
                            dec_ref_bits(_py, bits);
                        }
                        return MoltObject::none().bits();
                    }
                    let val_bits =
                        dataclasses_asdict_inner(_py, pair[1], dict_factory_bits, field_tag_bits);
                    if obj_from_bits(val_bits).is_none() {
                        dec_ref_bits(_py, key_bits);
                        for bits in pair_bits {
                            dec_ref_bits(_py, bits);
                        }
                        return MoltObject::none().bits();
                    }
                    let tuple_ptr = alloc_tuple(_py, &[key_bits, val_bits]);
                    dec_ref_bits(_py, key_bits);
                    dec_ref_bits(_py, val_bits);
                    if tuple_ptr.is_null() {
                        for bits in pair_bits {
                            dec_ref_bits(_py, bits);
                        }
                        return MoltObject::none().bits();
                    }
                    pair_bits.push(MoltObject::from_ptr(tuple_ptr).bits());
                }
                let pairs_list_ptr = alloc_list(_py, pair_bits.as_slice());
                for bits in pair_bits {
                    dec_ref_bits(_py, bits);
                }
                if pairs_list_ptr.is_null() {
                    return MoltObject::none().bits();
                }
                let pairs_list_bits = MoltObject::from_ptr(pairs_list_ptr).bits();
                let cls_bits = type_of_bits(_py, value_bits);
                let out_bits = call_callable1(_py, cls_bits, pairs_list_bits);
                dec_ref_bits(_py, pairs_list_bits);
                return out_bits;
            }
            _ => {}
        }
    }
    dataclasses_deepcopy(_py, value_bits)
}

fn dataclasses_astuple_inner(
    _py: &PyToken<'_>,
    value_bits: u64,
    tuple_factory_bits: u64,
    field_tag_bits: u64,
) -> u64 {
    let missing = missing_bits(_py);
    if dataclasses_is_dataclass_instance(_py, value_bits, missing) {
        let cls_bits = type_of_bits(_py, value_bits);
        let Some(fields_dict_bits) = dataclasses_fields_dict_bits(_py, cls_bits, missing) else {
            return MoltObject::none().bits();
        };
        let Some(field_objs) =
            dataclasses_collect_fields_by_tag(_py, fields_dict_bits, field_tag_bits)
        else {
            return MoltObject::none().bits();
        };
        let Some(name_name_bits) = attr_name_bits_from_bytes(_py, b"name") else {
            return MoltObject::none().bits();
        };
        let mut values: Vec<u64> = Vec::with_capacity(field_objs.len());
        for field_obj_bits in field_objs {
            let name_bits = molt_getattr_builtin(field_obj_bits, name_name_bits, missing);
            if exception_pending(_py) || name_bits == missing {
                dec_ref_bits(_py, name_name_bits);
                for bits in values {
                    dec_ref_bits(_py, bits);
                }
                return MoltObject::none().bits();
            }
            let field_val_bits = molt_getattr_builtin(value_bits, name_bits, missing);
            if exception_pending(_py) || field_val_bits == missing {
                dec_ref_bits(_py, name_name_bits);
                for bits in values {
                    dec_ref_bits(_py, bits);
                }
                return MoltObject::none().bits();
            }
            let copied_bits =
                dataclasses_astuple_inner(_py, field_val_bits, tuple_factory_bits, field_tag_bits);
            if obj_from_bits(copied_bits).is_none() {
                dec_ref_bits(_py, name_name_bits);
                for bits in values {
                    dec_ref_bits(_py, bits);
                }
                return MoltObject::none().bits();
            }
            values.push(copied_bits);
        }
        dec_ref_bits(_py, name_name_bits);
        let values_list_ptr = alloc_list(_py, values.as_slice());
        for bits in values {
            dec_ref_bits(_py, bits);
        }
        if values_list_ptr.is_null() {
            return MoltObject::none().bits();
        }
        let values_list_bits = MoltObject::from_ptr(values_list_ptr).bits();
        let out_bits = unsafe { call_callable1(_py, tuple_factory_bits, values_list_bits) };
        dec_ref_bits(_py, values_list_bits);
        return out_bits;
    }

    let Some(value_ptr) = obj_from_bits(value_bits).as_ptr() else {
        return dataclasses_deepcopy(_py, value_bits);
    };
    unsafe {
        match object_type_id(value_ptr) {
            TYPE_ID_LIST => {
                let elems = seq_vec_ref(value_ptr).clone();
                let mut copied: Vec<u64> = Vec::with_capacity(elems.len());
                for elem_bits in elems {
                    let inner_bits = dataclasses_astuple_inner(
                        _py,
                        elem_bits,
                        tuple_factory_bits,
                        field_tag_bits,
                    );
                    if obj_from_bits(inner_bits).is_none() {
                        for bits in copied {
                            dec_ref_bits(_py, bits);
                        }
                        return MoltObject::none().bits();
                    }
                    copied.push(inner_bits);
                }
                let copied_list_ptr = alloc_list(_py, copied.as_slice());
                for bits in copied {
                    dec_ref_bits(_py, bits);
                }
                if copied_list_ptr.is_null() {
                    return MoltObject::none().bits();
                }
                let copied_list_bits = MoltObject::from_ptr(copied_list_ptr).bits();
                let cls_bits = type_of_bits(_py, value_bits);
                let out_bits = call_callable1(_py, cls_bits, copied_list_bits);
                dec_ref_bits(_py, copied_list_bits);
                return out_bits;
            }
            TYPE_ID_TUPLE => {
                let elems = seq_vec_ref(value_ptr).clone();
                let mut copied: Vec<u64> = Vec::with_capacity(elems.len());
                for elem_bits in elems {
                    let inner_bits = dataclasses_astuple_inner(
                        _py,
                        elem_bits,
                        tuple_factory_bits,
                        field_tag_bits,
                    );
                    if obj_from_bits(inner_bits).is_none() {
                        for bits in copied {
                            dec_ref_bits(_py, bits);
                        }
                        return MoltObject::none().bits();
                    }
                    copied.push(inner_bits);
                }
                let copied_list_ptr = alloc_list(_py, copied.as_slice());
                for bits in copied {
                    dec_ref_bits(_py, bits);
                }
                if copied_list_ptr.is_null() {
                    return MoltObject::none().bits();
                }
                let copied_list_bits = MoltObject::from_ptr(copied_list_ptr).bits();
                let cls_bits = type_of_bits(_py, value_bits);
                let out_bits = call_callable1(_py, cls_bits, copied_list_bits);
                dec_ref_bits(_py, copied_list_bits);
                return out_bits;
            }
            TYPE_ID_DICT => {
                let mut pair_bits: Vec<u64> = Vec::new();
                let order = dict_order(value_ptr).clone();
                for pair in order.chunks(2) {
                    if pair.len() != 2 {
                        continue;
                    }
                    let key_bits =
                        dataclasses_astuple_inner(_py, pair[0], tuple_factory_bits, field_tag_bits);
                    if obj_from_bits(key_bits).is_none() {
                        for bits in pair_bits {
                            dec_ref_bits(_py, bits);
                        }
                        return MoltObject::none().bits();
                    }
                    let val_bits =
                        dataclasses_astuple_inner(_py, pair[1], tuple_factory_bits, field_tag_bits);
                    if obj_from_bits(val_bits).is_none() {
                        dec_ref_bits(_py, key_bits);
                        for bits in pair_bits {
                            dec_ref_bits(_py, bits);
                        }
                        return MoltObject::none().bits();
                    }
                    let tuple_ptr = alloc_tuple(_py, &[key_bits, val_bits]);
                    dec_ref_bits(_py, key_bits);
                    dec_ref_bits(_py, val_bits);
                    if tuple_ptr.is_null() {
                        for bits in pair_bits {
                            dec_ref_bits(_py, bits);
                        }
                        return MoltObject::none().bits();
                    }
                    pair_bits.push(MoltObject::from_ptr(tuple_ptr).bits());
                }
                let pairs_list_ptr = alloc_list(_py, pair_bits.as_slice());
                for bits in pair_bits {
                    dec_ref_bits(_py, bits);
                }
                if pairs_list_ptr.is_null() {
                    return MoltObject::none().bits();
                }
                let pairs_list_bits = MoltObject::from_ptr(pairs_list_ptr).bits();
                let cls_bits = type_of_bits(_py, value_bits);
                let out_bits = call_callable1(_py, cls_bits, pairs_list_bits);
                dec_ref_bits(_py, pairs_list_bits);
                return out_bits;
            }
            _ => {}
        }
    }
    dataclasses_deepcopy(_py, value_bits)
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_dataclasses_is_dataclass(obj_bits: u64) -> u64 {
    crate::with_gil_entry_nopanic!(_py, {
        let cls_bits = dataclasses_class_bits(_py, obj_bits);
        let Some(cls_ptr) = obj_from_bits(cls_bits).as_ptr() else {
            return MoltObject::from_bool(false).bits();
        };
        unsafe {
            if object_type_id(cls_ptr) != TYPE_ID_TYPE {
                return MoltObject::from_bool(false).bits();
            }
        }
        let Some(fields_name_bits) = attr_name_bits_from_bytes(_py, b"__dataclass_fields__") else {
            return MoltObject::from_bool(false).bits();
        };
        let mut has_fields = false;
        for base_bits in class_mro_vec(cls_bits) {
            let Some(base_ptr) = obj_from_bits(base_bits).as_ptr() else {
                continue;
            };
            unsafe {
                if object_type_id(base_ptr) != TYPE_ID_TYPE {
                    continue;
                }
                let dict_bits = class_dict_bits(base_ptr);
                let Some(dict_ptr) = obj_from_bits(dict_bits).as_ptr() else {
                    continue;
                };
                if object_type_id(dict_ptr) != TYPE_ID_DICT {
                    continue;
                }
                if dict_get_in_place(_py, dict_ptr, fields_name_bits).is_some() {
                    has_fields = true;
                    break;
                }
            }
        }
        dec_ref_bits(_py, fields_name_bits);
        MoltObject::from_bool(has_fields).bits()
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_dataclasses_fields(class_or_instance_bits: u64, field_tag_bits: u64) -> u64 {
    crate::with_gil_entry_nopanic!(_py, {
        let missing = missing_bits(_py);
        let cls_bits = dataclasses_class_bits(_py, class_or_instance_bits);
        let Some(fields_dict_bits) = dataclasses_fields_dict_bits(_py, cls_bits, missing) else {
            return raise_exception::<_>(
                _py,
                "TypeError",
                "must be called with a dataclass type or instance",
            );
        };
        let Some(field_objs) =
            dataclasses_collect_fields_by_tag(_py, fields_dict_bits, field_tag_bits)
        else {
            return MoltObject::none().bits();
        };
        let out_ptr = alloc_tuple(_py, field_objs.as_slice());
        if out_ptr.is_null() {
            return MoltObject::none().bits();
        }
        MoltObject::from_ptr(out_ptr).bits()
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_dataclasses_asdict(
    obj_bits: u64,
    dict_factory_bits: u64,
    field_tag_bits: u64,
) -> u64 {
    crate::with_gil_entry_nopanic!(_py, {
        let missing = missing_bits(_py);
        if !dataclasses_is_dataclass_instance(_py, obj_bits, missing) {
            return raise_exception::<_>(
                _py,
                "TypeError",
                "asdict() should be called on dataclass instances",
            );
        }
        dataclasses_asdict_inner(_py, obj_bits, dict_factory_bits, field_tag_bits)
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_dataclasses_astuple(
    obj_bits: u64,
    tuple_factory_bits: u64,
    field_tag_bits: u64,
) -> u64 {
    crate::with_gil_entry_nopanic!(_py, {
        let missing = missing_bits(_py);
        if !dataclasses_is_dataclass_instance(_py, obj_bits, missing) {
            return raise_exception::<_>(
                _py,
                "TypeError",
                "astuple() should be called on dataclass instances",
            );
        }
        dataclasses_astuple_inner(_py, obj_bits, tuple_factory_bits, field_tag_bits)
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_dataclasses_replace(
    obj_bits: u64,
    changes_bits: u64,
    field_tag_bits: u64,
    initvar_tag_bits: u64,
) -> u64 {
    crate::with_gil_entry_nopanic!(_py, {
        let missing = missing_bits(_py);
        if !dataclasses_is_dataclass_instance(_py, obj_bits, missing) {
            return raise_exception::<_>(
                _py,
                "TypeError",
                "replace() should be called on dataclass instances",
            );
        }
        let cls_bits = type_of_bits(_py, obj_bits);
        let changes_copy_bits = molt_dict_from_obj(changes_bits);
        if obj_from_bits(changes_copy_bits).is_none() {
            return MoltObject::none().bits();
        }
        let Some(changes_ptr) = obj_from_bits(changes_copy_bits).as_ptr() else {
            dec_ref_bits(_py, changes_copy_bits);
            return MoltObject::none().bits();
        };
        let values_ptr = alloc_dict_with_pairs(_py, &[]);
        if values_ptr.is_null() {
            dec_ref_bits(_py, changes_copy_bits);
            return MoltObject::none().bits();
        }
        let values_bits = MoltObject::from_ptr(values_ptr).bits();
        let Some(fields_dict_bits) = dataclasses_fields_dict_bits(_py, cls_bits, missing) else {
            dec_ref_bits(_py, changes_copy_bits);
            dec_ref_bits(_py, values_bits);
            return raise_exception::<_>(
                _py,
                "TypeError",
                "replace() should be called on dataclass instances",
            );
        };
        let Some(fields_ptr) = obj_from_bits(fields_dict_bits).as_ptr() else {
            dec_ref_bits(_py, changes_copy_bits);
            dec_ref_bits(_py, values_bits);
            return MoltObject::none().bits();
        };
        let Some(field_type_name_bits) = attr_name_bits_from_bytes(_py, b"_field_type") else {
            dec_ref_bits(_py, changes_copy_bits);
            dec_ref_bits(_py, values_bits);
            return MoltObject::none().bits();
        };
        let Some(name_name_bits) = attr_name_bits_from_bytes(_py, b"name") else {
            dec_ref_bits(_py, field_type_name_bits);
            dec_ref_bits(_py, changes_copy_bits);
            dec_ref_bits(_py, values_bits);
            return MoltObject::none().bits();
        };
        let Some(init_name_bits) = attr_name_bits_from_bytes(_py, b"init") else {
            dec_ref_bits(_py, name_name_bits);
            dec_ref_bits(_py, field_type_name_bits);
            dec_ref_bits(_py, changes_copy_bits);
            dec_ref_bits(_py, values_bits);
            return MoltObject::none().bits();
        };

        let order = unsafe { dict_order(fields_ptr) }.clone();
        for pair in order.chunks(2) {
            if pair.len() != 2 {
                continue;
            }
            let field_obj_bits = pair[1];
            let ftype_bits = molt_getattr_builtin(field_obj_bits, field_type_name_bits, missing);
            if exception_pending(_py) || ftype_bits == missing {
                dec_ref_bits(_py, init_name_bits);
                dec_ref_bits(_py, name_name_bits);
                dec_ref_bits(_py, field_type_name_bits);
                dec_ref_bits(_py, changes_copy_bits);
                dec_ref_bits(_py, values_bits);
                return MoltObject::none().bits();
            }
            let name_bits = molt_getattr_builtin(field_obj_bits, name_name_bits, missing);
            if exception_pending(_py) || name_bits == missing {
                dec_ref_bits(_py, init_name_bits);
                dec_ref_bits(_py, name_name_bits);
                dec_ref_bits(_py, field_type_name_bits);
                dec_ref_bits(_py, changes_copy_bits);
                dec_ref_bits(_py, values_bits);
                return MoltObject::none().bits();
            }

            if ftype_bits == initvar_tag_bits {
                let ch_val = unsafe { dict_get_in_place(_py, changes_ptr, name_bits) };
                if let Some(bits) = ch_val {
                    unsafe {
                        dict_set_in_place(_py, values_ptr, name_bits, bits);
                        dict_del_in_place(_py, changes_ptr, name_bits);
                    }
                    if exception_pending(_py) {
                        dec_ref_bits(_py, init_name_bits);
                        dec_ref_bits(_py, name_name_bits);
                        dec_ref_bits(_py, field_type_name_bits);
                        dec_ref_bits(_py, changes_copy_bits);
                        dec_ref_bits(_py, values_bits);
                        return MoltObject::none().bits();
                    }
                } else {
                    let name_repr_bits = molt_repr_from_obj(name_bits);
                    let name_repr =
                        string_obj_to_owned(obj_from_bits(name_repr_bits)).unwrap_or_default();
                    dec_ref_bits(_py, name_repr_bits);
                    dec_ref_bits(_py, init_name_bits);
                    dec_ref_bits(_py, name_name_bits);
                    dec_ref_bits(_py, field_type_name_bits);
                    dec_ref_bits(_py, changes_copy_bits);
                    dec_ref_bits(_py, values_bits);
                    let msg = format!("InitVar {name_repr} must be specified with replace()");
                    return raise_exception::<_>(_py, "TypeError", &msg);
                }
                continue;
            }
            if ftype_bits != field_tag_bits {
                continue;
            }

            let init_flag_bits = molt_getattr_builtin(field_obj_bits, init_name_bits, missing);
            if exception_pending(_py) || init_flag_bits == missing {
                dec_ref_bits(_py, init_name_bits);
                dec_ref_bits(_py, name_name_bits);
                dec_ref_bits(_py, field_type_name_bits);
                dec_ref_bits(_py, changes_copy_bits);
                dec_ref_bits(_py, values_bits);
                return MoltObject::none().bits();
            }
            let init_enabled = is_truthy(_py, obj_from_bits(init_flag_bits));
            if !init_enabled {
                if unsafe { dict_get_in_place(_py, changes_ptr, name_bits) }.is_some() {
                    let field_name =
                        string_obj_to_owned(obj_from_bits(name_bits)).unwrap_or_default();
                    dec_ref_bits(_py, init_name_bits);
                    dec_ref_bits(_py, name_name_bits);
                    dec_ref_bits(_py, field_type_name_bits);
                    dec_ref_bits(_py, changes_copy_bits);
                    dec_ref_bits(_py, values_bits);
                    let msg = format!(
                        "field {field_name} is declared with init=False, it cannot be specified with replace()"
                    );
                    return raise_exception::<_>(_py, "TypeError", &msg);
                }
                continue;
            }

            if let Some(changed_bits) = unsafe { dict_get_in_place(_py, changes_ptr, name_bits) } {
                unsafe {
                    dict_set_in_place(_py, values_ptr, name_bits, changed_bits);
                    dict_del_in_place(_py, changes_ptr, name_bits);
                }
                if exception_pending(_py) {
                    dec_ref_bits(_py, init_name_bits);
                    dec_ref_bits(_py, name_name_bits);
                    dec_ref_bits(_py, field_type_name_bits);
                    dec_ref_bits(_py, changes_copy_bits);
                    dec_ref_bits(_py, values_bits);
                    return MoltObject::none().bits();
                }
            } else {
                let current_bits = molt_getattr_builtin(obj_bits, name_bits, missing);
                if exception_pending(_py) || current_bits == missing {
                    dec_ref_bits(_py, init_name_bits);
                    dec_ref_bits(_py, name_name_bits);
                    dec_ref_bits(_py, field_type_name_bits);
                    dec_ref_bits(_py, changes_copy_bits);
                    dec_ref_bits(_py, values_bits);
                    return MoltObject::none().bits();
                }
                unsafe {
                    dict_set_in_place(_py, values_ptr, name_bits, current_bits);
                }
                if exception_pending(_py) {
                    dec_ref_bits(_py, init_name_bits);
                    dec_ref_bits(_py, name_name_bits);
                    dec_ref_bits(_py, field_type_name_bits);
                    dec_ref_bits(_py, changes_copy_bits);
                    dec_ref_bits(_py, values_bits);
                    return MoltObject::none().bits();
                }
            }
        }

        dec_ref_bits(_py, init_name_bits);
        dec_ref_bits(_py, name_name_bits);
        dec_ref_bits(_py, field_type_name_bits);

        unsafe {
            let _ = dict_update_apply(
                _py,
                values_bits,
                dict_update_set_in_place,
                changes_copy_bits,
            );
        }
        dec_ref_bits(_py, changes_copy_bits);
        if exception_pending(_py) {
            dec_ref_bits(_py, values_bits);
            return MoltObject::none().bits();
        }
        let out_bits = call_with_kwargs(_py, cls_bits, &[], values_bits);
        dec_ref_bits(_py, values_bits);
        out_bits
    })
}
