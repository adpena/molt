use super::field_runtime::dc_getattr_default_bits;
use super::*;
use crate::BuiltinExc;
use crate::object::seq_access::{locked_len, pin_item};

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
    let fields_bits =
        dc_getattr_default_bits(_py, cls_bits, crate::NameId::DunderDataclassFields, missing)?;
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

struct SelectedDataclassFields<'a, 'py> {
    entries: crate::object::seq_access::PinnedSequenceSnapshot<'a, 'py>,
    indices: Vec<usize>,
}
impl SelectedDataclassFields<'_, '_> {
    fn len(&self) -> usize {
        self.indices.len()
    }
    fn iter(&self) -> impl Iterator<Item = u64> + '_ {
        self.indices.iter().map(|&index| self.entries[index])
    }
}

fn dataclasses_collect_fields_by_tag<'a, 'py>(
    _py: &'a PyToken<'py>,
    fields_dict_bits: u64,
    field_tag_bits: u64,
) -> Option<SelectedDataclassFields<'a, 'py>> {
    let fields_dict_ptr = obj_from_bits(fields_dict_bits).as_ptr()?;
    let missing = missing_bits(_py);
    let mut indices = Vec::new();
    let order = unsafe {
        crate::object::ops_dict::dict_snapshot(
            _py,
            fields_dict_ptr,
            crate::object::ops_dict::DictSnapshotKind::Entries,
        )
    }?;
    for (index, pair) in order.as_chunks::<2>().0.iter().enumerate() {
        let field_obj_bits = pair[1];
        let tag_bits = dc_getattr_default_bits(
            _py,
            field_obj_bits,
            crate::NameId::PrivateFieldType,
            missing,
        )?;
        if exception_pending(_py) {
            return None;
        }
        if tag_bits == field_tag_bits {
            indices.push(index * 2 + 1);
        }
    }
    Some(SelectedDataclassFields {
        entries: order,
        indices,
    })
}

fn dataclasses_is_dataclass_instance(_py: &PyToken<'_>, obj_bits: u64, missing: u64) -> bool {
    let Some(obj_ptr) = obj_from_bits(obj_bits).as_ptr() else {
        return false;
    };
    unsafe {
        if object_type_id(obj_ptr) == TYPE_ID_TYPE {
            return false;
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
    let Some(deepcopy_name_bits) = crate::NameId::DunderDeepcopy.owned_bits(_py) else {
        dec_ref_bits(_py, memo_bits);
        return MoltObject::none().bits();
    };
    let result = crate::lookup_attribute(_py, value_bits, deepcopy_name_bits);
    dec_ref_bits(_py, deepcopy_name_bits);
    let deepcopy_bits = match result {
        crate::AttributeLookup::Found(bits) => bits,
        crate::AttributeLookup::Missing => {
            dec_ref_bits(_py, memo_bits);
            inc_ref_bits(_py, value_bits);
            return value_bits;
        }
        crate::AttributeLookup::Error => {
            dec_ref_bits(_py, memo_bits);
            return MoltObject::none().bits();
        }
    };
    let out_bits = unsafe { call_callable1(_py, deepcopy_bits, memo_bits) };
    dec_ref_bits(_py, deepcopy_bits);
    dec_ref_bits(_py, memo_bits);
    out_bits
}

#[derive(Clone, Copy)]
enum ProjectionKind {
    Dict,
    Tuple,
}

fn dataclasses_project_sequence(
    _py: &PyToken<'_>,
    value_bits: u64,
    value_ptr: *mut u8,
    factory_bits: u64,
    field_tag_bits: u64,
    projection: ProjectionKind,
) -> u64 {
    let mut copied: Vec<u64> = Vec::with_capacity(unsafe { locked_len(value_ptr) });
    let mut index = 0usize;
    loop {
        if index >= unsafe { locked_len(value_ptr) } {
            break;
        }
        let Some(elem) = (unsafe { pin_item(_py, value_ptr, index) }) else {
            continue;
        };
        index += 1;
        let inner_bits = match projection {
            ProjectionKind::Dict => {
                dataclasses_asdict_inner(_py, elem.bits(), factory_bits, field_tag_bits)
            }
            ProjectionKind::Tuple => {
                dataclasses_astuple_inner(_py, elem.bits(), factory_bits, field_tag_bits)
            }
        };
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
    let out_bits = unsafe { call_callable1(_py, cls_bits, copied_list_bits) };
    dec_ref_bits(_py, copied_list_bits);
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
        let Some(name_name_bits) = crate::NameId::Name.owned_bits(_py) else {
            return MoltObject::none().bits();
        };
        let mut item_bits: Vec<u64> = Vec::with_capacity(field_objs.len());
        for field_obj_bits in field_objs.iter() {
            let name_bits =
                molt_getattr_builtin_attached_body(_py, field_obj_bits, name_name_bits, missing);
            if exception_pending(_py) || name_bits == missing {
                dec_ref_bits(_py, name_name_bits);
                for bits in item_bits {
                    dec_ref_bits(_py, bits);
                }
                return MoltObject::none().bits();
            }
            let field_val_bits =
                molt_getattr_builtin_attached_body(_py, value_bits, name_bits, missing);
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
            TYPE_ID_LIST | TYPE_ID_TUPLE => {
                return dataclasses_project_sequence(
                    _py,
                    value_bits,
                    value_ptr,
                    dict_factory_bits,
                    field_tag_bits,
                    ProjectionKind::Dict,
                );
            }
            TYPE_ID_DICT => {
                let mut pair_bits: Vec<u64> = Vec::new();
                let Some(order) = crate::object::ops_dict::dict_snapshot(
                    _py,
                    value_ptr,
                    crate::object::ops_dict::DictSnapshotKind::Entries,
                ) else {
                    return MoltObject::none().bits();
                };
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
        let Some(name_name_bits) = crate::NameId::Name.owned_bits(_py) else {
            return MoltObject::none().bits();
        };
        let mut values: Vec<u64> = Vec::with_capacity(field_objs.len());
        for field_obj_bits in field_objs.iter() {
            let name_bits =
                molt_getattr_builtin_attached_body(_py, field_obj_bits, name_name_bits, missing);
            if exception_pending(_py) || name_bits == missing {
                dec_ref_bits(_py, name_name_bits);
                for bits in values {
                    dec_ref_bits(_py, bits);
                }
                return MoltObject::none().bits();
            }
            let field_val_bits =
                molt_getattr_builtin_attached_body(_py, value_bits, name_bits, missing);
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
            TYPE_ID_LIST | TYPE_ID_TUPLE => {
                return dataclasses_project_sequence(
                    _py,
                    value_bits,
                    value_ptr,
                    tuple_factory_bits,
                    field_tag_bits,
                    ProjectionKind::Tuple,
                );
            }
            TYPE_ID_DICT => {
                let mut pair_bits: Vec<u64> = Vec::new();
                let Some(order) = crate::object::ops_dict::dict_snapshot(
                    _py,
                    value_ptr,
                    crate::object::ops_dict::DictSnapshotKind::Entries,
                ) else {
                    return MoltObject::none().bits();
                };
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

pub(crate) fn molt_dataclasses_is_dataclass_attached_body(
    _py: &crate::PyToken<'_>,
    obj_bits: u64,
) -> u64 {
    crate::with_attached_entry_nopanic!(_py, _py, {
        let cls_bits = dataclasses_class_bits(_py, obj_bits);
        let Some(cls_ptr) = obj_from_bits(cls_bits).as_ptr() else {
            return MoltObject::from_bool(false).bits();
        };
        unsafe {
            if object_type_id(cls_ptr) != TYPE_ID_TYPE {
                return MoltObject::from_bool(false).bits();
            }
        }
        let Some(fields_name_bits) = crate::NameId::DunderDataclassFields.owned_bits(_py) else {
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
                match dict_get_in_place(_py, dict_ptr, fields_name_bits) {
                    Ok(Some(_)) => {
                        has_fields = true;
                        break;
                    }
                    Ok(None) => {}
                    Err(()) => {
                        dec_ref_bits(_py, fields_name_bits);
                        return MoltObject::none().bits();
                    }
                }
            }
        }
        dec_ref_bits(_py, fields_name_bits);
        MoltObject::from_bool(has_fields).bits()
    })
}

pub(crate) fn molt_dataclasses_fields_attached_body(
    _py: &crate::PyToken<'_>,
    class_or_instance_bits: u64,
    field_tag_bits: u64,
) -> u64 {
    crate::with_attached_entry_nopanic!(_py, _py, {
        let missing = missing_bits(_py);
        let cls_bits = dataclasses_class_bits(_py, class_or_instance_bits);
        let Some(fields_dict_bits) = dataclasses_fields_dict_bits(_py, cls_bits, missing) else {
            return raise_exception::<_>(
                _py,
                BuiltinExc::TypeError,
                "must be called with a dataclass type or instance",
            );
        };
        let Some(field_objs) =
            dataclasses_collect_fields_by_tag(_py, fields_dict_bits, field_tag_bits)
        else {
            return MoltObject::none().bits();
        };
        let fields: Vec<u64> = field_objs.iter().collect();
        let out_ptr = alloc_tuple(_py, &fields);
        if out_ptr.is_null() {
            return MoltObject::none().bits();
        }
        MoltObject::from_ptr(out_ptr).bits()
    })
}

pub(crate) fn molt_dataclasses_asdict_attached_body(
    _py: &crate::PyToken<'_>,
    obj_bits: u64,
    dict_factory_bits: u64,
    field_tag_bits: u64,
) -> u64 {
    crate::with_attached_entry_nopanic!(_py, _py, {
        let missing = missing_bits(_py);
        if !dataclasses_is_dataclass_instance(_py, obj_bits, missing) {
            return raise_exception::<_>(
                _py,
                BuiltinExc::TypeError,
                "asdict() should be called on dataclass instances",
            );
        }
        dataclasses_asdict_inner(_py, obj_bits, dict_factory_bits, field_tag_bits)
    })
}

pub(crate) fn molt_dataclasses_astuple_attached_body(
    _py: &crate::PyToken<'_>,
    obj_bits: u64,
    tuple_factory_bits: u64,
    field_tag_bits: u64,
) -> u64 {
    crate::with_attached_entry_nopanic!(_py, _py, {
        let missing = missing_bits(_py);
        if !dataclasses_is_dataclass_instance(_py, obj_bits, missing) {
            return raise_exception::<_>(
                _py,
                BuiltinExc::TypeError,
                "astuple() should be called on dataclass instances",
            );
        }
        dataclasses_astuple_inner(_py, obj_bits, tuple_factory_bits, field_tag_bits)
    })
}

// CPython Lib/dataclasses.py: replace (3.12), _replace (3.13+).
// Keep one algorithm for the public function and the generated class method.
fn dataclasses_replace_impl(
    py: &PyToken<'_>,
    obj_bits: u64,
    changes_bits: u64,
    field_tag_bits: u64,
    initvar_tag_bits: u64,
    missing_default_bits: u64,
) -> Result<u64, ()> {
    use super::field_runtime::{
        DcValue, dc_attr, dc_dict_fields, dc_repr_str, dc_result, dc_truth,
    };
    use crate::builtins::compatibility_error::CompatibilityError;
    let missing = missing_bits(py);
    if !dataclasses_is_dataclass_instance(py, obj_bits, missing) {
        raise_exception::<()>(
            py,
            BuiltinExc::TypeError,
            "replace() should be called on dataclass instances",
        );
        return Err(());
    }
    let fields = dc_attr(py, obj_bits, crate::NameId::DunderDataclassFields)?;
    let fields = dc_dict_fields(py, fields.bits)?;
    let changes = dc_result(py, molt_dict_from_obj_attached_body(py, changes_bits))?;
    let changes_ptr = obj_from_bits(changes.bits).as_ptr().ok_or(())?;
    for pair in fields.as_chunks::<2>().0 {
        let field = pair[1];
        let tag = dc_attr(py, field, crate::NameId::PrivateFieldType)?;
        if tag.bits != field_tag_bits && tag.bits != initvar_tag_bits {
            continue;
        }
        let name = dc_attr(py, field, crate::NameId::Name)?;
        let init = dc_attr(py, field, crate::NameId::Init)?;
        let changed = unsafe { dict_get_in_place(py, changes_ptr, name.bits)? }.is_some();
        if !dc_truth(py, init.bits)? {
            if changed {
                let field = string_obj_to_owned(obj_from_bits(name.bits)).ok_or(())?;
                CompatibilityError::DataclassReplaceInitFalse { field: &field }.raise::<()>(py);
                return Err(());
            }
            continue;
        }
        if changed {
            continue;
        }
        if tag.bits == initvar_tag_bits {
            let default = dc_attr(py, field, crate::NameId::Default)?;
            // The compiled caller owns the canonical sentinel binding even
            // when sys.modules no longer contains its dataclasses module.
            if default.bits == missing_default_bits {
                let field_repr = dc_repr_str(py, name.bits)?;
                CompatibilityError::DataclassReplaceMissingInitVar {
                    field_repr: &field_repr,
                }
                .raise::<()>(py);
                return Err(());
            }
        }
        let value = dc_result(
            py,
            crate::builtins::attributes::molt_get_attr_name_attached_body(py, obj_bits, name.bits),
        )?;
        if !unsafe { dict_set_in_place(py, changes_ptr, name.bits, value.bits) } {
            return Err(());
        }
    }
    let cls = dc_attr(py, obj_bits, crate::NameId::DunderClass)?;
    dc_result(py, call_with_kwargs(py, cls.bits, &[], changes.bits)).map(DcValue::into_bits)
}

pub(crate) fn molt_dataclasses_replace_attached_body(
    _py: &crate::PyToken<'_>,
    obj_bits: u64,
    changes_bits: u64,
    field_tag_bits: u64,
    initvar_tag_bits: u64,
    missing_default_bits: u64,
) -> u64 {
    crate::with_attached_entry_nopanic!(_py, py, {
        dataclasses_replace_impl(
            py,
            obj_bits,
            changes_bits,
            field_tag_bits,
            initvar_tag_bits,
            missing_default_bits,
        )
        .unwrap_or_else(|()| MoltObject::none().bits())
    })
}

include!("../../../attached_exports_generated/builtins/types/dataclasses/projection.rs");
