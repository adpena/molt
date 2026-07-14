// Binary pickle unpickling (load) VM: stack items, apply ops, global resolution.

use super::*;

#[derive(Clone, Debug)]
pub(crate) enum PickleVmItem {
    Value(u64),
    Global(PickleGlobal),
    Mark,
}

fn pickle_apply_dict_state(
    _py: &crate::PyToken<'_>,
    inst_bits: u64,
    dict_state_bits: u64,
) -> Result<(), u64> {
    if obj_from_bits(dict_state_bits).is_none() {
        return Ok(());
    }
    let Some(state_ptr) = obj_from_bits(dict_state_bits).as_ptr() else {
        return Err(pickle_raise(_py, "pickle.loads: BUILD state must be dict"));
    };
    if unsafe { object_type_id(state_ptr) } != TYPE_ID_DICT {
        return Err(pickle_raise(_py, "pickle.loads: BUILD state must be dict"));
    }

    // Use setattr for each state entry. This correctly routes values to typed
    // field slots (TYPE_ID_OBJECT), dataclass descriptor fields
    // (TYPE_ID_DATACLASS), or __dict__ for fully dynamic instances.
    let pairs = unsafe { crate::dict_order(state_ptr).to_vec() };
    let mut idx = 0usize;
    while idx + 1 < pairs.len() {
        let key_bits = pairs[idx];
        let value_bits = pairs[idx + 1];
        idx += 2;
        let _ = crate::molt_object_setattr(inst_bits, key_bits, value_bits);
        if exception_pending(_py) {
            return Err(MoltObject::none().bits());
        }
    }
    Ok(())
}

pub(crate) fn pickle_vm_item_to_bits(
    _py: &crate::PyToken<'_>,
    item: &PickleVmItem,
) -> Result<u64, u64> {
    match item {
        PickleVmItem::Value(bits) => Ok(*bits),
        PickleVmItem::Global(global) => pickle_global_callable_bits(_py, *global),
        PickleVmItem::Mark => Err(pickle_raise(_py, "pickle.loads: mark not found")),
    }
}

pub(crate) fn pickle_vm_pop_mark_items(
    _py: &crate::PyToken<'_>,
    stack: &mut Vec<PickleVmItem>,
) -> Result<Vec<PickleVmItem>, u64> {
    let mut out: Vec<PickleVmItem> = Vec::new();
    while let Some(item) = stack.pop() {
        if matches!(item, PickleVmItem::Mark) {
            out.reverse();
            return Ok(out);
        }
        out.push(item);
    }
    Err(pickle_raise(_py, "pickle.loads: mark not found"))
}

pub(crate) fn pickle_vm_pop_value(
    _py: &crate::PyToken<'_>,
    stack: &mut Vec<PickleVmItem>,
) -> Result<u64, u64> {
    let item = stack
        .pop()
        .ok_or_else(|| pickle_raise(_py, "pickle.loads: stack underflow"))?;
    pickle_vm_item_to_bits(_py, &item)
}

pub(crate) fn pickle_resolve_global_bits(
    _py: &crate::PyToken<'_>,
    module: &str,
    name: &str,
) -> Result<u64, u64> {
    let Some(module_bits) = alloc_string_bits(_py, module) else {
        return Err(MoltObject::none().bits());
    };
    let imported_bits = crate::molt_module_import(module_bits);
    dec_ref_bits(_py, module_bits);
    if exception_pending(_py) {
        return Err(MoltObject::none().bits());
    }
    let Some(name_bits) = alloc_string_bits(_py, name) else {
        if !obj_from_bits(imported_bits).is_none() {
            dec_ref_bits(_py, imported_bits);
        }
        return Err(MoltObject::none().bits());
    };
    let value_bits = crate::molt_object_getattribute(imported_bits, name_bits);
    dec_ref_bits(_py, name_bits);
    if !obj_from_bits(imported_bits).is_none() {
        dec_ref_bits(_py, imported_bits);
    }
    if exception_pending(_py) {
        return Err(MoltObject::none().bits());
    }
    Ok(value_bits)
}

pub(crate) fn pickle_resolve_global_with_hook(
    _py: &crate::PyToken<'_>,
    module: &str,
    name: &str,
    find_class_bits: Option<u64>,
) -> Result<u64, u64> {
    if let Some(callback_bits) = find_class_bits {
        let Some(module_bits) = alloc_string_bits(_py, module) else {
            return Err(MoltObject::none().bits());
        };
        let Some(name_bits) = alloc_string_bits(_py, name) else {
            dec_ref_bits(_py, module_bits);
            return Err(MoltObject::none().bits());
        };
        let out_bits = unsafe { call_callable2(_py, callback_bits, module_bits, name_bits) };
        dec_ref_bits(_py, module_bits);
        dec_ref_bits(_py, name_bits);
        if exception_pending(_py) {
            return Err(MoltObject::none().bits());
        }
        return Ok(out_bits);
    }
    pickle_resolve_global_bits(_py, module, name)
}

pub(crate) fn pickle_lookup_extension_bits(
    _py: &crate::PyToken<'_>,
    code: i64,
    find_class_bits: Option<u64>,
) -> Result<u64, u64> {
    let copyreg_bits = pickle_resolve_global_bits(_py, "copyreg", "_inverted_registry")?;
    let Some(dict_ptr) = obj_from_bits(copyreg_bits).as_ptr() else {
        dec_ref_bits(_py, copyreg_bits);
        return Err(pickle_raise(
            _py,
            "pickle.loads: extension registry unavailable",
        ));
    };
    if unsafe { object_type_id(dict_ptr) } != TYPE_ID_DICT {
        dec_ref_bits(_py, copyreg_bits);
        return Err(pickle_raise(
            _py,
            "pickle.loads: extension registry unavailable",
        ));
    }
    let code_bits = MoltObject::from_int(code).bits();
    let entry_bits = unsafe { dict_get_in_place(_py, dict_ptr, code_bits) };
    if exception_pending(_py) {
        dec_ref_bits(_py, copyreg_bits);
        return Err(MoltObject::none().bits());
    }
    let Some(entry_bits) = entry_bits else {
        dec_ref_bits(_py, copyreg_bits);
        return Err(pickle_raise(_py, "pickle.loads: unknown extension code"));
    };
    let Some(entry_ptr) = obj_from_bits(entry_bits).as_ptr() else {
        dec_ref_bits(_py, copyreg_bits);
        return Err(pickle_raise(_py, "pickle.loads: invalid extension entry"));
    };
    if unsafe { object_type_id(entry_ptr) } != TYPE_ID_TUPLE {
        dec_ref_bits(_py, copyreg_bits);
        return Err(pickle_raise(_py, "pickle.loads: invalid extension entry"));
    }
    let Some(fields) = (unsafe {
        crate::object::seq_access::snapshot(_py, entry_ptr, "sequence snapshot allocation failed")
    }) else {
        dec_ref_bits(_py, copyreg_bits);
        return Err(MoltObject::none().bits());
    };
    if fields.len() != 2 {
        dec_ref_bits(_py, copyreg_bits);
        return Err(pickle_raise(_py, "pickle.loads: invalid extension entry"));
    }
    let Some(module) = string_obj_to_owned(obj_from_bits(fields[0])) else {
        dec_ref_bits(_py, copyreg_bits);
        return Err(pickle_raise(_py, "pickle.loads: invalid extension entry"));
    };
    let Some(name) = string_obj_to_owned(obj_from_bits(fields[1])) else {
        dec_ref_bits(_py, copyreg_bits);
        return Err(pickle_raise(_py, "pickle.loads: invalid extension entry"));
    };
    dec_ref_bits(_py, copyreg_bits);
    pickle_resolve_global_with_hook(_py, &module, &name, find_class_bits)
}

pub(crate) fn pickle_apply_newobj(
    _py: &crate::PyToken<'_>,
    cls_bits: u64,
    args_bits: u64,
    kwargs_bits: Option<u64>,
) -> Result<u64, u64> {
    let new_bits = pickle_attr_required(_py, cls_bits, b"__new__")?;
    let Some(args_ptr) = obj_from_bits(args_bits).as_ptr() else {
        dec_ref_bits(_py, new_bits);
        return Err(pickle_raise(_py, "pickle.loads: NEWOBJ args must be tuple"));
    };
    if unsafe { object_type_id(args_ptr) } != TYPE_ID_TUPLE {
        dec_ref_bits(_py, new_bits);
        return Err(pickle_raise(_py, "pickle.loads: NEWOBJ args must be tuple"));
    }
    let Some(args) = (unsafe {
        crate::object::seq_access::snapshot(_py, args_ptr, "sequence snapshot allocation failed")
    }) else {
        dec_ref_bits(_py, new_bits);
        return Err(MoltObject::none().bits());
    };
    let kw_len = if let Some(kw_bits) = kwargs_bits {
        let Some(kw_ptr) = obj_from_bits(kw_bits).as_ptr() else {
            dec_ref_bits(_py, new_bits);
            return Err(pickle_raise(
                _py,
                "pickle.loads: NEWOBJ_EX kwargs must be dict",
            ));
        };
        if unsafe { object_type_id(kw_ptr) } != TYPE_ID_DICT {
            dec_ref_bits(_py, new_bits);
            return Err(pickle_raise(
                _py,
                "pickle.loads: NEWOBJ_EX kwargs must be dict",
            ));
        }
        unsafe { crate::dict_order(kw_ptr).len() / 2 }
    } else {
        0
    };
    let builder_bits = crate::molt_callargs_new((args.len() + 1) as u64, kw_len as u64);
    let _ = unsafe { crate::molt_callargs_push_pos(builder_bits, cls_bits) };
    if exception_pending(_py) {
        dec_ref_bits(_py, new_bits);
        return Err(MoltObject::none().bits());
    }
    for arg in args.iter().copied() {
        let _ = unsafe { crate::molt_callargs_push_pos(builder_bits, arg) };
        if exception_pending(_py) {
            dec_ref_bits(_py, new_bits);
            return Err(MoltObject::none().bits());
        }
    }
    if let Some(kw_bits) = kwargs_bits {
        let kw_ptr = obj_from_bits(kw_bits).as_ptr().expect("checked above");
        let pairs = unsafe { crate::dict_order(kw_ptr).to_vec() };
        let mut idx = 0usize;
        while idx + 1 < pairs.len() {
            let key_bits = pairs[idx];
            let val_bits = pairs[idx + 1];
            let _ = unsafe { crate::molt_callargs_push_kw(builder_bits, key_bits, val_bits) };
            if exception_pending(_py) {
                dec_ref_bits(_py, new_bits);
                return Err(MoltObject::none().bits());
            }
            idx += 2;
        }
    }
    let out_bits = crate::molt_call_bind(new_bits, builder_bits);
    dec_ref_bits(_py, new_bits);
    if exception_pending(_py) {
        return Err(MoltObject::none().bits());
    }
    // Initialize typed field slots to the missing sentinel so that
    // uninitialized fields (from __new__ without __init__) are properly
    // recognized as absent by hasattr/getattr.
    pickle_init_missing_fields(_py, out_bits);
    Ok(out_bits)
}

/// Initialize all typed field slots (and dataclass field values) to the missing
/// sentinel. Called after NEWOBJ to ensure fields not populated by BUILD are
/// correctly absent.
fn pickle_init_missing_fields(_py: &crate::PyToken<'_>, inst_bits: u64) {
    let Some(inst_ptr) = obj_from_bits(inst_bits).as_ptr() else {
        return;
    };
    let type_id = unsafe { object_type_id(inst_ptr) };
    let missing = missing_bits(_py);

    if type_id == crate::TYPE_ID_OBJECT {
        // Initialize typed field offsets to missing.
        let class_bits = unsafe { object_class_bits(inst_ptr) };
        let Some(class_ptr) = obj_from_bits(class_bits).as_ptr() else {
            return;
        };
        if unsafe { object_type_id(class_ptr) } != crate::TYPE_ID_TYPE {
            return;
        }
        let cd_bits = unsafe { crate::class_dict_bits(class_ptr) };
        let Some(cd_ptr) = obj_from_bits(cd_bits).as_ptr() else {
            return;
        };
        if unsafe { object_type_id(cd_ptr) } != TYPE_ID_DICT {
            return;
        }
        let Some(offsets_name) = attr_name_bits_from_bytes(_py, b"__molt_field_offsets__") else {
            return;
        };
        let offsets_bits = unsafe { crate::dict_get_in_place(_py, cd_ptr, offsets_name) };
        dec_ref_bits(_py, offsets_name);
        if exception_pending(_py) {
            clear_exception(_py);
            return;
        }
        let Some(offsets_bits) = offsets_bits else {
            return;
        };
        let Some(offsets_ptr) = obj_from_bits(offsets_bits).as_ptr() else {
            return;
        };
        if unsafe { object_type_id(offsets_ptr) } != TYPE_ID_DICT {
            return;
        }
        let pairs = unsafe { crate::dict_order(offsets_ptr).to_vec() };
        let mut idx = 0usize;
        while idx + 1 < pairs.len() {
            let offset_bits = pairs[idx + 1];
            idx += 2;
            if let Some(offset) = to_i64(obj_from_bits(offset_bits)).filter(|&v| v >= 0) {
                unsafe {
                    let slot = inst_ptr.add(offset as usize) as *mut u64;
                    let old = *slot;
                    if old != missing {
                        inc_ref_bits(_py, missing);
                        if obj_from_bits(old).as_ptr().is_some() {
                            dec_ref_bits(_py, old);
                        }
                        *slot = missing;
                    }
                }
            }
        }
    } else if type_id == crate::TYPE_ID_DATACLASS {
        // Initialize dataclass field values to missing.
        let desc_ptr = unsafe { crate::dataclass_desc_ptr(inst_ptr) };
        if desc_ptr.is_null() {
            return;
        }
        let fields = unsafe { crate::dataclass_fields_mut(inst_ptr) };
        for val in fields.iter_mut() {
            if *val != missing {
                inc_ref_bits(_py, missing);
                if obj_from_bits(*val).as_ptr().is_some() {
                    dec_ref_bits(_py, *val);
                }
                *val = missing;
            }
        }
    }
}

pub(crate) fn pickle_apply_build(
    _py: &crate::PyToken<'_>,
    inst_bits: u64,
    state_bits: u64,
) -> Result<u64, u64> {
    if obj_from_bits(state_bits).is_none() {
        return Ok(inst_bits);
    }
    if let Some(setstate_bits) = pickle_attr_optional(_py, inst_bits, b"__setstate__")? {
        let _ = unsafe { call_callable1(_py, setstate_bits, state_bits) };
        dec_ref_bits(_py, setstate_bits);
        if exception_pending(_py) {
            return Err(MoltObject::none().bits());
        }
        return Ok(inst_bits);
    }
    let mut dict_state_bits = state_bits;
    let mut slot_state_bits: Option<u64> = None;
    if let Some(state_ptr) = obj_from_bits(state_bits).as_ptr()
        && unsafe { object_type_id(state_ptr) } == TYPE_ID_TUPLE
    {
        if let Some((dict_bits, slots_bits)) =
            unsafe { crate::object::seq_access::tuple_pair(state_ptr) }
        {
            dict_state_bits = dict_bits;
            slot_state_bits = Some(slots_bits);
        }
    }
    pickle_apply_dict_state(_py, inst_bits, dict_state_bits)?;
    if let Some(slot_bits) = slot_state_bits
        && !obj_from_bits(slot_bits).is_none()
    {
        let Some(slot_ptr) = obj_from_bits(slot_bits).as_ptr() else {
            return Err(pickle_raise(
                _py,
                "pickle.loads: BUILD slot state must be dict",
            ));
        };
        if unsafe { object_type_id(slot_ptr) } != TYPE_ID_DICT {
            return Err(pickle_raise(
                _py,
                "pickle.loads: BUILD slot state must be dict",
            ));
        }
        let pairs = unsafe { crate::dict_order(slot_ptr).to_vec() };
        let mut idx = 0usize;
        while idx + 1 < pairs.len() {
            let key_bits = pairs[idx];
            let value_bits = pairs[idx + 1];
            let _ = crate::molt_object_setattr(inst_bits, key_bits, value_bits);
            if exception_pending(_py) {
                return Err(MoltObject::none().bits());
            }
            idx += 2;
        }
    }
    Ok(inst_bits)
}

pub(crate) fn pickle_apply_reduce_vm(
    _py: &crate::PyToken<'_>,
    callable: PickleVmItem,
    args_bits: u64,
) -> Result<u64, u64> {
    let Some(args_ptr) = obj_from_bits(args_bits).as_ptr() else {
        return Err(pickle_raise(_py, "pickle.loads: reduce args must be tuple"));
    };
    if unsafe { object_type_id(args_ptr) } != TYPE_ID_TUPLE {
        return Err(pickle_raise(_py, "pickle.loads: reduce args must be tuple"));
    }
    let Some(args) = (unsafe {
        crate::object::seq_access::snapshot(_py, args_ptr, "sequence snapshot allocation failed")
    }) else {
        return Err(MoltObject::none().bits());
    };
    let out_bits = match callable {
        PickleVmItem::Mark => {
            return Err(pickle_raise(_py, "pickle.loads: mark cannot be called"));
        }
        PickleVmItem::Global(PickleGlobal::CodecsEncode) => {
            if args.is_empty() || args.len() > 2 {
                return Err(pickle_raise(
                    _py,
                    "pickle.loads: _codecs.encode expects 1 or 2 arguments",
                ));
            }
            let Some(text) = string_obj_to_owned(obj_from_bits(args[0])) else {
                return Err(pickle_raise(
                    _py,
                    "pickle.loads: _codecs.encode text must be str",
                ));
            };
            let encoding = if args.len() == 1 {
                "utf-8".to_string()
            } else {
                let Some(enc) = string_obj_to_owned(obj_from_bits(args[1])) else {
                    return Err(pickle_raise(
                        _py,
                        "pickle.loads: _codecs.encode encoding must be str",
                    ));
                };
                enc
            };
            pickle_encode_text(_py, &text, &encoding)?
        }
        PickleVmItem::Global(global) => {
            let callable_bits = pickle_global_callable_bits(_py, global)?;
            let out_bits = pickle_call_with_args(_py, callable_bits, &args);
            if exception_pending(_py) {
                return Err(MoltObject::none().bits());
            }
            out_bits
        }
        PickleVmItem::Value(callable_bits) => {
            let out_bits = pickle_call_with_args(_py, callable_bits, &args);
            if exception_pending(_py) {
                return Err(MoltObject::none().bits());
            }
            out_bits
        }
    };
    Ok(out_bits)
}

pub(crate) fn pickle_apply_reduce_bits(
    _py: &crate::PyToken<'_>,
    callable_bits: u64,
    args_bits: u64,
) -> Result<u64, u64> {
    let Some(args_ptr) = obj_from_bits(args_bits).as_ptr() else {
        return Err(pickle_raise(_py, "pickle.loads: reduce args must be tuple"));
    };
    if unsafe { object_type_id(args_ptr) } != TYPE_ID_TUPLE {
        return Err(pickle_raise(_py, "pickle.loads: reduce args must be tuple"));
    }
    let Some(args) = (unsafe {
        crate::object::seq_access::snapshot(_py, args_ptr, "sequence snapshot allocation failed")
    }) else {
        return Err(MoltObject::none().bits());
    };
    let out_bits = pickle_call_with_args(_py, callable_bits, &args);
    if exception_pending(_py) {
        Err(MoltObject::none().bits())
    } else {
        Ok(out_bits)
    }
}

pub(crate) fn pickle_memo_set(
    _py: &crate::PyToken<'_>,
    memo: &mut Vec<Option<PickleVmItem>>,
    index: usize,
    item: PickleVmItem,
) {
    if memo.len() <= index {
        memo.resize(index + 1, None);
    }
    memo[index] = Some(item);
}

pub(crate) fn pickle_memo_get(
    _py: &crate::PyToken<'_>,
    memo: &[Option<PickleVmItem>],
    index: usize,
) -> Result<PickleVmItem, u64> {
    if let Some(Some(item)) = memo.get(index) {
        return Ok(item.clone());
    }
    let msg = format!("pickle.loads: memo key {} missing", index);
    Err(pickle_raise(_py, &msg))
}
