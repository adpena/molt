use molt_obj_model::MoltObject;

use super::methods::is_not_implemented_bits;
use crate::{
    TYPE_ID_DICT, TYPE_ID_TUPLE, TYPE_ID_TYPE, alloc_bytearray, alloc_bytes, alloc_dict_with_pairs,
    alloc_function_obj, alloc_list, alloc_string, alloc_tuple, attr_name_bits_from_bytes,
    builtin_classes, call_callable0, call_callable1, class_bases_bits, class_bases_vec,
    class_dict_bits, class_mro_vec, dec_ref_bits, dict_get_in_place, dict_order, exception_pending,
    int_bits_from_i64, is_truthy, issubclass_bits, maybe_ptr_from_bits, obj_eq, obj_from_bits,
    object_type_id, raise_exception, runtime_state, seq_vec_ref, type_of_bits,
};

fn get_attr_default(
    _py: &crate::PyToken<'_>,
    obj_bits: u64,
    name: &[u8],
    default_bits: u64,
) -> u64 {
    if let Some(obj_ptr) = maybe_ptr_from_bits(obj_bits) {
        unsafe {
            if object_type_id(obj_ptr) == TYPE_ID_TYPE {
                if let Some(name_bits) = attr_name_bits_from_bytes(_py, name) {
                    let out = class_lookup_mro_attr(_py, obj_bits, name_bits);
                    dec_ref_bits(_py, name_bits);
                    if !obj_from_bits(out).is_none() {
                        return out;
                    }
                }
                return default_bits;
            }
        }
    }
    let Some(name_bits) = attr_name_bits_from_bytes(_py, name) else {
        return MoltObject::none().bits();
    };
    let out = crate::molt_getattr_builtin(obj_bits, name_bits, default_bits);
    dec_ref_bits(_py, name_bits);
    out
}

fn set_attr_name(
    _py: &crate::PyToken<'_>,
    obj_bits: u64,
    name: &[u8],
    value_bits: u64,
) -> Result<(), u64> {
    let name_bits =
        attr_name_bits_from_bytes(_py, name).ok_or_else(|| MoltObject::none().bits())?;
    let _ = crate::molt_set_attr_name(obj_bits, name_bits, value_bits);
    dec_ref_bits(_py, name_bits);
    if exception_pending(_py) {
        return Err(MoltObject::none().bits());
    }
    Ok(())
}

fn set_add(_py: &crate::PyToken<'_>, set_bits: u64, value_bits: u64) -> Result<(), u64> {
    let _ = crate::molt_set_add(set_bits, value_bits);
    if exception_pending(_py) {
        return Err(MoltObject::none().bits());
    }
    Ok(())
}

fn set_clear(_py: &crate::PyToken<'_>, set_bits: u64) -> Result<(), u64> {
    let _ = crate::molt_set_clear(set_bits);
    if exception_pending(_py) {
        return Err(MoltObject::none().bits());
    }
    Ok(())
}

fn dict_set_str_key(
    _py: &crate::PyToken<'_>,
    dict_bits: u64,
    key: &[u8],
    value_bits: u64,
) -> Result<(), u64> {
    let key_ptr = alloc_string(_py, key);
    if key_ptr.is_null() {
        return Err(MoltObject::none().bits());
    }
    let key_bits = MoltObject::from_ptr(key_ptr).bits();
    let Some(dict_ptr) = maybe_ptr_from_bits(dict_bits) else {
        dec_ref_bits(_py, key_bits);
        return Err(MoltObject::none().bits());
    };
    unsafe {
        if object_type_id(dict_ptr) != TYPE_ID_DICT {
            dec_ref_bits(_py, key_bits);
            return Err(MoltObject::none().bits());
        }
        crate::dict_set_in_place(_py, dict_ptr, key_bits, value_bits);
    }
    dec_ref_bits(_py, key_bits);
    Ok(())
}

fn iter_type(_py: &crate::PyToken<'_>, iterable_bits: u64) -> Result<u64, u64> {
    let iter_bits = crate::molt_iter(iterable_bits);
    if exception_pending(_py) {
        return Err(MoltObject::none().bits());
    }
    if obj_from_bits(iter_bits).is_none() {
        return Err(MoltObject::none().bits());
    }
    let iter_type_bits = type_of_bits(_py, iter_bits);
    dec_ref_bits(_py, iter_bits);
    if !is_type_object(iter_type_bits) {
        return Err(raise_exception::<_>(
            _py,
            "RuntimeError",
            "iterator type resolution did not return a type",
        ));
    }
    Ok(iter_type_bits)
}

fn set_contains(_py: &crate::PyToken<'_>, set_bits: u64, value_bits: u64) -> Result<bool, u64> {
    if obj_from_bits(set_bits).is_none() {
        return Ok(false);
    }
    for entry_bits in iter_values(_py, set_bits)? {
        if obj_eq(_py, obj_from_bits(entry_bits), obj_from_bits(value_bits)) {
            return Ok(true);
        }
    }
    Ok(false)
}

fn is_type_object(bits: u64) -> bool {
    let Some(ptr) = maybe_ptr_from_bits(bits) else {
        return false;
    };
    unsafe { object_type_id(ptr) == TYPE_ID_TYPE }
}

fn iter_values(_py: &crate::PyToken<'_>, iterable_bits: u64) -> Result<Vec<u64>, u64> {
    let iter_bits = crate::molt_iter(iterable_bits);
    if exception_pending(_py) {
        return Err(MoltObject::none().bits());
    }
    let mut out: Vec<u64> = Vec::new();
    loop {
        let pair_bits = crate::molt_iter_next(iter_bits);
        let Some(pair_ptr) = maybe_ptr_from_bits(pair_bits) else {
            return Err(MoltObject::none().bits());
        };
        unsafe {
            if object_type_id(pair_ptr) != TYPE_ID_TUPLE {
                return Err(MoltObject::none().bits());
            }
        }
        let pair = unsafe { seq_vec_ref(pair_ptr) };
        if pair.len() < 2 {
            return Err(MoltObject::none().bits());
        }
        if is_truthy(_py, obj_from_bits(pair[1])) {
            break;
        }
        out.push(pair[0]);
    }
    Ok(out)
}

fn is_abstract_value(_py: &crate::PyToken<'_>, value_bits: u64) -> Result<bool, u64> {
    if obj_from_bits(value_bits).is_none() {
        return Ok(false);
    }
    let is_abs = get_attr_default(
        _py,
        value_bits,
        b"__isabstractmethod__",
        MoltObject::none().bits(),
    );
    if exception_pending(_py) {
        return Err(MoltObject::none().bits());
    }
    if !obj_from_bits(is_abs).is_none() && is_truthy(_py, obj_from_bits(is_abs)) {
        return Ok(true);
    }
    let func_bits = get_attr_default(_py, value_bits, b"__func__", MoltObject::none().bits());
    if exception_pending(_py) {
        return Err(MoltObject::none().bits());
    }
    if !obj_from_bits(func_bits).is_none() {
        let func_abs = get_attr_default(
            _py,
            func_bits,
            b"__isabstractmethod__",
            MoltObject::none().bits(),
        );
        if exception_pending(_py) {
            return Err(MoltObject::none().bits());
        }
        if !obj_from_bits(func_abs).is_none() && is_truthy(_py, obj_from_bits(func_abs)) {
            return Ok(true);
        }
    }
    for accessor in [b"fget".as_slice(), b"fset".as_slice(), b"fdel".as_slice()] {
        let acc_bits = get_attr_default(_py, value_bits, accessor, MoltObject::none().bits());
        if exception_pending(_py) {
            return Err(MoltObject::none().bits());
        }
        if obj_from_bits(acc_bits).is_none() {
            continue;
        }
        let acc_abs = get_attr_default(
            _py,
            acc_bits,
            b"__isabstractmethod__",
            MoltObject::none().bits(),
        );
        if exception_pending(_py) {
            return Err(MoltObject::none().bits());
        }
        if !obj_from_bits(acc_abs).is_none() && is_truthy(_py, obj_from_bits(acc_abs)) {
            return Ok(true);
        }
    }
    Ok(false)
}

fn mro_contains(subclass_bits: u64, needle_bits: u64) -> bool {
    class_mro_vec(subclass_bits)
        .iter()
        .copied()
        .any(|base| base == needle_bits)
}

fn abc_counter_get(_py: &crate::PyToken<'_>) -> u64 {
    runtime_state(_py)
        .abc_invalidation_counter
        .load(std::sync::atomic::Ordering::Acquire)
}

fn abc_counter_inc(_py: &crate::PyToken<'_>) -> u64 {
    runtime_state(_py)
        .abc_invalidation_counter
        .fetch_add(1, std::sync::atomic::Ordering::AcqRel)
        + 1
}

fn abc_state_attr(_py: &crate::PyToken<'_>, cls_bits: u64, name: &[u8]) -> u64 {
    let Some(cls_ptr) = maybe_ptr_from_bits(cls_bits) else {
        return MoltObject::none().bits();
    };
    unsafe {
        if object_type_id(cls_ptr) != TYPE_ID_TYPE {
            return MoltObject::none().bits();
        }
        let dict_bits = class_dict_bits(cls_ptr);
        let Some(dict_ptr) = maybe_ptr_from_bits(dict_bits) else {
            return MoltObject::none().bits();
        };
        if object_type_id(dict_ptr) != TYPE_ID_DICT {
            return MoltObject::none().bits();
        }
        let Some(name_bits) = attr_name_bits_from_bytes(_py, name) else {
            return MoltObject::none().bits();
        };
        let out = dict_get_in_place(_py, dict_ptr, name_bits)
            .unwrap_or_else(|| MoltObject::none().bits());
        dec_ref_bits(_py, name_bits);
        out
    }
}

fn class_lookup_mro_attr(_py: &crate::PyToken<'_>, cls_bits: u64, name_bits: u64) -> u64 {
    let Some(cls_ptr) = maybe_ptr_from_bits(cls_bits) else {
        return MoltObject::none().bits();
    };
    unsafe {
        if object_type_id(cls_ptr) == TYPE_ID_TYPE {
            let dict_bits = class_dict_bits(cls_ptr);
            if let Some(dict_ptr) = maybe_ptr_from_bits(dict_bits)
                && object_type_id(dict_ptr) == TYPE_ID_DICT
                && let Some(value_bits) = dict_get_in_place(_py, dict_ptr, name_bits)
            {
                return value_bits;
            }
        }
    }

    for base_bits in class_mro_vec(cls_bits) {
        if base_bits == cls_bits {
            continue;
        }
        let Some(base_ptr) = maybe_ptr_from_bits(base_bits) else {
            continue;
        };
        unsafe {
            if object_type_id(base_ptr) != TYPE_ID_TYPE {
                continue;
            }
            let dict_bits = class_dict_bits(base_ptr);
            let Some(dict_ptr) = maybe_ptr_from_bits(dict_bits) else {
                continue;
            };
            if object_type_id(dict_ptr) != TYPE_ID_DICT {
                continue;
            }
            if let Some(value_bits) = dict_get_in_place(_py, dict_ptr, name_bits) {
                return value_bits;
            }
        }
    }
    MoltObject::none().bits()
}

fn abc_collect_abstractmethods_frozenset(
    _py: &crate::PyToken<'_>,
    cls_bits: u64,
) -> Result<u64, u64> {
    let abstracts_bits = crate::molt_set_new(0);
    if obj_from_bits(abstracts_bits).is_none() {
        return Err(MoltObject::none().bits());
    }

    let cls_ptr = maybe_ptr_from_bits(cls_bits).ok_or_else(|| MoltObject::none().bits())?;
    let bases_bits = unsafe { class_bases_bits(cls_ptr) };
    for base_bits in class_bases_vec(bases_bits) {
        let base_abstracts = get_attr_default(
            _py,
            base_bits,
            b"__abstractmethods__",
            MoltObject::none().bits(),
        );
        if exception_pending(_py) {
            return Err(MoltObject::none().bits());
        }
        if obj_from_bits(base_abstracts).is_none() {
            continue;
        }
        for name_bits in iter_values(_py, base_abstracts)? {
            let value_bits = class_lookup_mro_attr(_py, cls_bits, name_bits);
            if is_abstract_value(_py, value_bits)? {
                set_add(_py, abstracts_bits, name_bits)?;
            }
        }
    }

    let dict_bits = unsafe { class_dict_bits(cls_ptr) };
    if let Some(dict_ptr) = maybe_ptr_from_bits(dict_bits) {
        unsafe {
            if object_type_id(dict_ptr) == TYPE_ID_DICT {
                let entries = dict_order(dict_ptr);
                for pair in entries.chunks(2) {
                    if pair.len() < 2 {
                        continue;
                    }
                    if is_abstract_value(_py, pair[1])? {
                        set_add(_py, abstracts_bits, pair[0])?;
                    }
                }
            }
        }
    }

    let frozen_bits = unsafe {
        crate::frozenset_from_iter_bits(_py, abstracts_bits).unwrap_or(MoltObject::none().bits())
    };
    dec_ref_bits(_py, abstracts_bits);
    if obj_from_bits(frozen_bits).is_none() && exception_pending(_py) {
        return Err(MoltObject::none().bits());
    }
    Ok(frozen_bits)
}

fn abc_init_impl(_py: &crate::PyToken<'_>, cls_bits: u64) -> Result<(), u64> {
    if !is_type_object(cls_bits) {
        return Err(raise_exception::<_>(
            _py,
            "TypeError",
            "abc init expects class",
        ));
    }
    let frozen_bits = abc_collect_abstractmethods_frozenset(_py, cls_bits)?;
    let registry_bits = crate::molt_set_new(0);
    let cache_bits = crate::molt_set_new(0);
    let neg_cache_bits = crate::molt_set_new(0);
    if obj_from_bits(registry_bits).is_none()
        || obj_from_bits(cache_bits).is_none()
        || obj_from_bits(neg_cache_bits).is_none()
    {
        return Err(MoltObject::none().bits());
    }
    let version_bits = int_bits_from_i64(_py, abc_counter_get(_py) as i64);

    set_attr_name(_py, cls_bits, b"__abstractmethods__", frozen_bits)?;
    set_attr_name(_py, cls_bits, b"_abc_registry", registry_bits)?;
    set_attr_name(_py, cls_bits, b"_abc_cache", cache_bits)?;
    set_attr_name(_py, cls_bits, b"_abc_negative_cache", neg_cache_bits)?;
    set_attr_name(_py, cls_bits, b"_abc_negative_cache_version", version_bits)?;

    if !obj_from_bits(frozen_bits).is_none() {
        dec_ref_bits(_py, frozen_bits);
    }
    dec_ref_bits(_py, registry_bits);
    dec_ref_bits(_py, cache_bits);
    dec_ref_bits(_py, neg_cache_bits);
    Ok(())
}

fn abc_update_abstractmethods_impl(_py: &crate::PyToken<'_>, cls_bits: u64) -> Result<u64, u64> {
    if !is_type_object(cls_bits) {
        return Ok(cls_bits);
    }
    let current = get_attr_default(
        _py,
        cls_bits,
        b"__abstractmethods__",
        MoltObject::none().bits(),
    );
    if exception_pending(_py) {
        return Err(MoltObject::none().bits());
    }
    if obj_from_bits(current).is_none() {
        return Ok(cls_bits);
    }
    let frozen_bits = abc_collect_abstractmethods_frozenset(_py, cls_bits)?;
    set_attr_name(_py, cls_bits, b"__abstractmethods__", frozen_bits)?;
    if !obj_from_bits(frozen_bits).is_none() {
        dec_ref_bits(_py, frozen_bits);
    }
    Ok(cls_bits)
}

fn abc_sync_negative_cache_version(_py: &crate::PyToken<'_>, cls_bits: u64) -> Result<(), u64> {
    let neg_cache_bits = abc_state_attr(_py, cls_bits, b"_abc_negative_cache");
    if !obj_from_bits(neg_cache_bits).is_none() {
        set_clear(_py, neg_cache_bits)?;
    }
    let version_bits = int_bits_from_i64(_py, abc_counter_get(_py) as i64);
    set_attr_name(_py, cls_bits, b"_abc_negative_cache_version", version_bits)?;
    Ok(())
}

fn abc_ensure_init(_py: &crate::PyToken<'_>, cls_bits: u64) -> Result<(), u64> {
    let registry = abc_state_attr(_py, cls_bits, b"_abc_registry");
    if obj_from_bits(registry).is_none() {
        return abc_init_impl(_py, cls_bits);
    }
    Ok(())
}

fn abc_subclasscheck_impl(
    _py: &crate::PyToken<'_>,
    cls_bits: u64,
    subclass_bits: u64,
) -> Result<bool, u64> {
    if !is_type_object(subclass_bits) {
        return Err(raise_exception::<_>(
            _py,
            "TypeError",
            "issubclass() arg 1 must be a class",
        ));
    }
    abc_ensure_init(_py, cls_bits)?;

    let cache_bits = abc_state_attr(_py, cls_bits, b"_abc_cache");
    let neg_cache_bits = abc_state_attr(_py, cls_bits, b"_abc_negative_cache");
    let registry_bits = abc_state_attr(_py, cls_bits, b"_abc_registry");
    let neg_ver_bits = abc_state_attr(_py, cls_bits, b"_abc_negative_cache_version");
    if obj_from_bits(cache_bits).is_none()
        || obj_from_bits(neg_cache_bits).is_none()
        || obj_from_bits(registry_bits).is_none()
    {
        return Err(MoltObject::none().bits());
    }

    if set_contains(_py, cache_bits, subclass_bits)? {
        return Ok(true);
    }

    let current_counter = abc_counter_get(_py) as i64;
    let stored_version = crate::to_i64(obj_from_bits(neg_ver_bits)).unwrap_or(-1);
    if stored_version < current_counter {
        set_clear(_py, neg_cache_bits)?;
        let version_bits = int_bits_from_i64(_py, current_counter);
        set_attr_name(_py, cls_bits, b"_abc_negative_cache_version", version_bits)?;
    } else if set_contains(_py, neg_cache_bits, subclass_bits)? {
        return Ok(false);
    }

    let hook_bits = get_attr_default(
        _py,
        cls_bits,
        b"__subclasshook__",
        MoltObject::none().bits(),
    );
    if exception_pending(_py) {
        return Err(MoltObject::none().bits());
    }
    if !obj_from_bits(hook_bits).is_none() {
        let callable_ok = is_truthy(_py, obj_from_bits(crate::molt_is_callable(hook_bits)));
        if callable_ok {
            let hook_res = unsafe { call_callable1(_py, hook_bits, subclass_bits) };
            if exception_pending(_py) {
                return Err(MoltObject::none().bits());
            }
            let true_bits = MoltObject::from_bool(true).bits();
            let false_bits = MoltObject::from_bool(false).bits();
            if hook_res == true_bits {
                set_add(_py, cache_bits, subclass_bits)?;
                if !obj_from_bits(hook_res).is_none() {
                    dec_ref_bits(_py, hook_res);
                }
                return Ok(true);
            }
            if hook_res == false_bits {
                set_add(_py, neg_cache_bits, subclass_bits)?;
                if !obj_from_bits(hook_res).is_none() {
                    dec_ref_bits(_py, hook_res);
                }
                return Ok(false);
            }
            if !is_not_implemented_bits(_py, hook_res) {
                if !obj_from_bits(hook_res).is_none() {
                    dec_ref_bits(_py, hook_res);
                }
                return Err(raise_exception::<_>(
                    _py,
                    "AssertionError",
                    "__subclasshook__ must return either False, True, or NotImplemented",
                ));
            }
            if !obj_from_bits(hook_res).is_none() {
                dec_ref_bits(_py, hook_res);
            }
        }
    }

    if mro_contains(subclass_bits, cls_bits) {
        set_add(_py, cache_bits, subclass_bits)?;
        return Ok(true);
    }

    for rcls_bits in iter_values(_py, registry_bits)? {
        if mro_contains(subclass_bits, rcls_bits) {
            set_add(_py, cache_bits, subclass_bits)?;
            return Ok(true);
        }
    }

    let subclasses_bits =
        get_attr_default(_py, cls_bits, b"__subclasses__", MoltObject::none().bits());
    if exception_pending(_py) {
        return Err(MoltObject::none().bits());
    }
    if !obj_from_bits(subclasses_bits).is_none() {
        let callable_ok = is_truthy(_py, obj_from_bits(crate::molt_is_callable(subclasses_bits)));
        if callable_ok {
            let sub_list = unsafe { call_callable0(_py, subclasses_bits) };
            if exception_pending(_py) {
                return Err(MoltObject::none().bits());
            }
            for scls_bits in iter_values(_py, sub_list)? {
                if mro_contains(subclass_bits, scls_bits) {
                    set_add(_py, cache_bits, subclass_bits)?;
                    return Ok(true);
                }
            }
            if !obj_from_bits(sub_list).is_none() {
                dec_ref_bits(_py, sub_list);
            }
        }
    }

    set_add(_py, neg_cache_bits, subclass_bits)?;
    Ok(false)
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_collections_abc_runtime_types() -> u64 {
    crate::with_gil_entry!(_py, {
        let debug = std::env::var("MOLT_DEBUG_COLLECTIONS_ABC_TYPES").as_deref() == Ok("1");
        let trace_stage = |stage: &str| {
            if debug {
                eprintln!("molt collections_abc runtime types stage={stage}");
            }
        };
        trace_stage("start");
        let dict_ptr = alloc_dict_with_pairs(_py, &[]);
        if dict_ptr.is_null() {
            return MoltObject::none().bits();
        }
        let dict_bits = MoltObject::from_ptr(dict_ptr).bits();

        let bytes_ptr = alloc_bytes(_py, &[]);
        if bytes_ptr.is_null() {
            return MoltObject::none().bits();
        }
        let bytes_bits = MoltObject::from_ptr(bytes_ptr).bits();
        let bytes_iterator = match iter_type(_py, bytes_bits) {
            Ok(bits) => bits,
            Err(bits) => return bits,
        };
        trace_stage("bytes_iterator");
        dec_ref_bits(_py, bytes_bits);

        let bytearray_ptr = alloc_bytearray(_py, &[]);
        if bytearray_ptr.is_null() {
            return MoltObject::none().bits();
        }
        let bytearray_bits = MoltObject::from_ptr(bytearray_ptr).bits();
        let bytearray_iterator = match iter_type(_py, bytearray_bits) {
            Ok(bits) => bits,
            Err(bits) => return bits,
        };
        trace_stage("bytearray_iterator");
        dec_ref_bits(_py, bytearray_bits);

        let empty_dict_ptr = alloc_dict_with_pairs(_py, &[]);
        if empty_dict_ptr.is_null() {
            return MoltObject::none().bits();
        }
        let empty_dict_bits = MoltObject::from_ptr(empty_dict_ptr).bits();
        let dict_keys_bits = crate::molt_dict_keys(empty_dict_bits);
        if exception_pending(_py) || obj_from_bits(dict_keys_bits).is_none() {
            return MoltObject::none().bits();
        }
        let dict_values_bits = crate::molt_dict_values(empty_dict_bits);
        if exception_pending(_py) || obj_from_bits(dict_values_bits).is_none() {
            return MoltObject::none().bits();
        }
        let dict_items_bits = crate::molt_dict_items(empty_dict_bits);
        if exception_pending(_py) || obj_from_bits(dict_items_bits).is_none() {
            return MoltObject::none().bits();
        }
        let dict_keyiterator = match iter_type(_py, dict_keys_bits) {
            Ok(bits) => bits,
            Err(bits) => return bits,
        };
        let dict_valueiterator = match iter_type(_py, dict_values_bits) {
            Ok(bits) => bits,
            Err(bits) => return bits,
        };
        let dict_itemiterator = match iter_type(_py, dict_items_bits) {
            Ok(bits) => bits,
            Err(bits) => return bits,
        };
        let dict_keys = type_of_bits(_py, dict_keys_bits);
        let dict_values = type_of_bits(_py, dict_values_bits);
        let dict_items = type_of_bits(_py, dict_items_bits);
        trace_stage("dict_views");
        dec_ref_bits(_py, dict_keys_bits);
        dec_ref_bits(_py, dict_values_bits);
        dec_ref_bits(_py, dict_items_bits);
        dec_ref_bits(_py, empty_dict_bits);

        let empty_list_ptr = alloc_list(_py, &[]);
        if empty_list_ptr.is_null() {
            return MoltObject::none().bits();
        }
        let empty_list_bits = MoltObject::from_ptr(empty_list_ptr).bits();
        let list_iterator = match iter_type(_py, empty_list_bits) {
            Ok(bits) => bits,
            Err(bits) => return bits,
        };
        let reversed_bits = crate::molt_reversed_builtin(empty_list_bits);
        if exception_pending(_py) || obj_from_bits(reversed_bits).is_none() {
            return MoltObject::none().bits();
        }
        let list_reverseiterator = type_of_bits(_py, reversed_bits);
        trace_stage("list_iterators");
        dec_ref_bits(_py, reversed_bits);
        dec_ref_bits(_py, empty_list_bits);

        let zero_bits = int_bits_from_i64(_py, 0);
        let one_bits = int_bits_from_i64(_py, 1);
        let range_bits = crate::molt_range_new(zero_bits, zero_bits, one_bits);
        if exception_pending(_py) || obj_from_bits(range_bits).is_none() {
            return MoltObject::none().bits();
        }
        let range_iterator = match iter_type(_py, range_bits) {
            Ok(bits) => bits,
            Err(bits) => return bits,
        };
        trace_stage("range_iterator");
        dec_ref_bits(_py, range_bits);

        let thousand_bits = int_bits_from_i64(_py, 1000);
        let long_stop_bits = crate::molt_lshift(one_bits, thousand_bits);
        if exception_pending(_py) || obj_from_bits(long_stop_bits).is_none() {
            return MoltObject::none().bits();
        }
        let long_range_bits = crate::molt_range_new(zero_bits, long_stop_bits, one_bits);
        if exception_pending(_py) || obj_from_bits(long_range_bits).is_none() {
            if !obj_from_bits(long_stop_bits).is_none() {
                dec_ref_bits(_py, long_stop_bits);
            }
            return MoltObject::none().bits();
        }
        let longrange_iterator = match iter_type(_py, long_range_bits) {
            Ok(bits) => bits,
            Err(bits) => return bits,
        };
        trace_stage("longrange_iterator");
        dec_ref_bits(_py, long_range_bits);
        if !obj_from_bits(long_stop_bits).is_none() {
            dec_ref_bits(_py, long_stop_bits);
        }

        let empty_set_bits = crate::molt_set_new(0);
        if obj_from_bits(empty_set_bits).is_none() {
            return MoltObject::none().bits();
        }
        let set_iterator = match iter_type(_py, empty_set_bits) {
            Ok(bits) => bits,
            Err(bits) => return bits,
        };
        trace_stage("set_iterator");
        dec_ref_bits(_py, empty_set_bits);

        let empty_str_ptr = alloc_string(_py, b"");
        if empty_str_ptr.is_null() {
            return MoltObject::none().bits();
        }
        let empty_str_bits = MoltObject::from_ptr(empty_str_ptr).bits();
        let str_iterator = match iter_type(_py, empty_str_bits) {
            Ok(bits) => bits,
            Err(bits) => return bits,
        };
        trace_stage("str_iterator");
        dec_ref_bits(_py, empty_str_bits);

        let empty_tuple_ptr = alloc_tuple(_py, &[]);
        if empty_tuple_ptr.is_null() {
            return MoltObject::none().bits();
        }
        let empty_tuple_bits = MoltObject::from_ptr(empty_tuple_ptr).bits();
        let tuple_iterator = match iter_type(_py, empty_tuple_bits) {
            Ok(bits) => bits,
            Err(bits) => return bits,
        };
        let zip_bits =
            crate::molt_zip_builtin(empty_tuple_bits, MoltObject::from_bool(false).bits());
        if exception_pending(_py) || obj_from_bits(zip_bits).is_none() {
            return MoltObject::none().bits();
        }
        let zip_iterator = type_of_bits(_py, zip_bits);
        trace_stage("tuple_zip_iterators");
        dec_ref_bits(_py, zip_bits);
        dec_ref_bits(_py, empty_tuple_bits);

        let builtins = builtin_classes(_py);
        let mappingproxy = crate::builtins::types::mappingproxy_class_bits(_py);
        trace_stage("mappingproxy");
        if obj_from_bits(mappingproxy).is_none() || !is_type_object(mappingproxy) {
            return raise_exception::<_>(
                _py,
                "RuntimeError",
                "mappingproxy type is unavailable while lowering collections.abc",
            );
        }

        // Keep frame locals deterministic and bootstrap-safe. In the current
        // runtime, `frame.f_locals` exposes a dict-backed snapshot surface, so
        // collections.abc should register the concrete dict type instead of
        // reflectively probing a live frame object during bootstrap.
        let framelocalsproxy = builtins.dict;
        trace_stage("framelocalsproxy");

        let entries: [(&[u8], u64); 20] = [
            (b"bytes_iterator", bytes_iterator),
            (b"bytearray_iterator", bytearray_iterator),
            (b"dict_keyiterator", dict_keyiterator),
            (b"dict_valueiterator", dict_valueiterator),
            (b"dict_itemiterator", dict_itemiterator),
            (b"list_iterator", list_iterator),
            (b"list_reverseiterator", list_reverseiterator),
            (b"range_iterator", range_iterator),
            (b"longrange_iterator", longrange_iterator),
            (b"set_iterator", set_iterator),
            (b"str_iterator", str_iterator),
            (b"tuple_iterator", tuple_iterator),
            (b"zip_iterator", zip_iterator),
            (b"dict_keys", dict_keys),
            (b"dict_values", dict_values),
            (b"dict_items", dict_items),
            (b"mappingproxy", mappingproxy),
            (b"framelocalsproxy", framelocalsproxy),
            (b"generator", builtins.generator),
            (b"coroutine", builtins.coroutine),
        ];
        for (name, bits) in entries {
            if !is_type_object(bits) {
                return raise_exception::<_>(
                    _py,
                    "RuntimeError",
                    "collections.abc runtime type payload contained non-type value",
                );
            }
            if let Err(bits) = dict_set_str_key(_py, dict_bits, name, bits) {
                return bits;
            }
        }
        trace_stage("dict_populated");
        if !is_type_object(builtins.async_generator) {
            return raise_exception::<_>(
                _py,
                "RuntimeError",
                "collections.abc async_generator runtime type is unavailable",
            );
        }
        if let Err(bits) =
            dict_set_str_key(_py, dict_bits, b"async_generator", builtins.async_generator)
        {
            return bits;
        }
        trace_stage("done");

        dict_bits
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_abc_get_cache_token() -> u64 {
    crate::with_gil_entry!(_py, { int_bits_from_i64(_py, abc_counter_get(_py) as i64) })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_abc_init(cls_bits: u64) -> u64 {
    crate::with_gil_entry!(_py, {
        if let Err(bits) = abc_init_impl(_py, cls_bits) {
            return bits;
        }
        MoltObject::none().bits()
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_abc_register(cls_bits: u64, subclass_bits: u64) -> u64 {
    crate::with_gil_entry!(_py, {
        if !is_type_object(subclass_bits) {
            return raise_exception::<_>(_py, "TypeError", "Can only register classes");
        }
        if let Err(bits) = abc_ensure_init(_py, cls_bits) {
            return bits;
        }
        if mro_contains(subclass_bits, cls_bits) {
            return subclass_bits;
        }
        if mro_contains(cls_bits, subclass_bits) {
            return raise_exception::<_>(
                _py,
                "RuntimeError",
                "Refusing to create an inheritance cycle",
            );
        }
        let registry_bits = abc_state_attr(_py, cls_bits, b"_abc_registry");
        if let Err(bits) = set_add(_py, registry_bits, subclass_bits) {
            return bits;
        }
        let _ = abc_counter_inc(_py);
        if let Err(bits) = abc_sync_negative_cache_version(_py, cls_bits) {
            return bits;
        }
        subclass_bits
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_abc_instancecheck(cls_bits: u64, instance_bits: u64) -> u64 {
    crate::with_gil_entry!(_py, {
        let class_attr_bits =
            get_attr_default(_py, instance_bits, b"__class__", MoltObject::none().bits());
        if exception_pending(_py) {
            return MoltObject::none().bits();
        }
        if is_type_object(class_attr_bits) {
            match abc_subclasscheck_impl(_py, cls_bits, class_attr_bits) {
                Ok(true) => return MoltObject::from_bool(true).bits(),
                Ok(false) => {}
                Err(bits) => return bits,
            }
            let subtype_bits = type_of_bits(_py, instance_bits);
            if subtype_bits != class_attr_bits {
                match abc_subclasscheck_impl(_py, cls_bits, subtype_bits) {
                    Ok(value) => return MoltObject::from_bool(value).bits(),
                    Err(bits) => return bits,
                }
            }
            return MoltObject::from_bool(false).bits();
        }
        let subtype_bits = type_of_bits(_py, instance_bits);
        match abc_subclasscheck_impl(_py, cls_bits, subtype_bits) {
            Ok(value) => MoltObject::from_bool(value).bits(),
            Err(bits) => bits,
        }
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_abc_subclasscheck(cls_bits: u64, subclass_bits: u64) -> u64 {
    crate::with_gil_entry!(_py, {
        match abc_subclasscheck_impl(_py, cls_bits, subclass_bits) {
            Ok(value) => MoltObject::from_bool(value).bits(),
            Err(bits) => bits,
        }
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_abc_get_dump(cls_bits: u64) -> u64 {
    crate::with_gil_entry!(_py, {
        if let Err(bits) = abc_ensure_init(_py, cls_bits) {
            return bits;
        }
        let registry_bits = abc_state_attr(_py, cls_bits, b"_abc_registry");
        let cache_bits = abc_state_attr(_py, cls_bits, b"_abc_cache");
        let neg_cache_bits = abc_state_attr(_py, cls_bits, b"_abc_negative_cache");
        let neg_ver_bits = abc_state_attr(_py, cls_bits, b"_abc_negative_cache_version");
        let tuple_ptr = alloc_tuple(
            _py,
            &[registry_bits, cache_bits, neg_cache_bits, neg_ver_bits],
        );
        if tuple_ptr.is_null() {
            return MoltObject::none().bits();
        }
        MoltObject::from_ptr(tuple_ptr).bits()
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_abc_reset_registry(cls_bits: u64) -> u64 {
    crate::with_gil_entry!(_py, {
        if let Err(bits) = abc_ensure_init(_py, cls_bits) {
            return bits;
        }
        let registry_bits = abc_state_attr(_py, cls_bits, b"_abc_registry");
        if let Err(bits) = set_clear(_py, registry_bits) {
            return bits;
        }
        let _ = abc_counter_inc(_py);
        if let Err(bits) = abc_sync_negative_cache_version(_py, cls_bits) {
            return bits;
        }
        MoltObject::none().bits()
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_abc_reset_caches(cls_bits: u64) -> u64 {
    crate::with_gil_entry!(_py, {
        if let Err(bits) = abc_ensure_init(_py, cls_bits) {
            return bits;
        }
        let cache_bits = abc_state_attr(_py, cls_bits, b"_abc_cache");
        let neg_cache_bits = abc_state_attr(_py, cls_bits, b"_abc_negative_cache");
        if let Err(bits) = set_clear(_py, cache_bits) {
            return bits;
        }
        if let Err(bits) = set_clear(_py, neg_cache_bits) {
            return bits;
        }
        let version_bits = int_bits_from_i64(_py, abc_counter_get(_py) as i64);
        if let Err(bits) =
            set_attr_name(_py, cls_bits, b"_abc_negative_cache_version", version_bits)
        {
            return bits;
        }
        MoltObject::none().bits()
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_abc_update_abstractmethods(cls_bits: u64) -> u64 {
    crate::with_gil_entry!(_py, {
        match abc_update_abstractmethods_impl(_py, cls_bits) {
            Ok(bits) => bits,
            Err(bits) => bits,
        }
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_abc_bootstrap() -> u64 {
    crate::with_gil_entry!(_py, {
        let dict_ptr = alloc_dict_with_pairs(_py, &[]);
        if dict_ptr.is_null() {
            return MoltObject::none().bits();
        }
        let dict_bits = MoltObject::from_ptr(dict_ptr).bits();

        let entries: [(&[u8], u64, u64); 9] = [
            (
                b"get_cache_token",
                crate::molt_abc_get_cache_token as *const () as usize as u64,
                0,
            ),
            (
                b"_abc_init",
                crate::molt_abc_init as *const () as usize as u64,
                1,
            ),
            (
                b"_abc_register",
                crate::molt_abc_register as *const () as usize as u64,
                2,
            ),
            (
                b"_abc_instancecheck",
                crate::molt_abc_instancecheck as *const () as usize as u64,
                2,
            ),
            (
                b"_abc_subclasscheck",
                crate::molt_abc_subclasscheck as *const () as usize as u64,
                2,
            ),
            (
                b"_get_dump",
                crate::molt_abc_get_dump as *const () as usize as u64,
                1,
            ),
            (
                b"_reset_registry",
                crate::molt_abc_reset_registry as *const () as usize as u64,
                1,
            ),
            (
                b"_reset_caches",
                crate::molt_abc_reset_caches as *const () as usize as u64,
                1,
            ),
            (
                b"update_abstractmethods",
                crate::molt_abc_update_abstractmethods as *const () as usize as u64,
                1,
            ),
        ];

        for (name, fn_ptr, arity) in entries {
            let key_ptr = crate::alloc_string(_py, name);
            if key_ptr.is_null() {
                return MoltObject::none().bits();
            }
            let key_bits = MoltObject::from_ptr(key_ptr).bits();
            let fn_obj_ptr = alloc_function_obj(_py, fn_ptr, arity);
            if fn_obj_ptr.is_null() {
                dec_ref_bits(_py, key_bits);
                return MoltObject::none().bits();
            }
            let fn_bits = MoltObject::from_ptr(fn_obj_ptr).bits();
            let Some(dict_ptr) = obj_from_bits(dict_bits).as_ptr() else {
                dec_ref_bits(_py, fn_bits);
                dec_ref_bits(_py, key_bits);
                return MoltObject::none().bits();
            };
            unsafe {
                if object_type_id(dict_ptr) != TYPE_ID_DICT {
                    dec_ref_bits(_py, fn_bits);
                    dec_ref_bits(_py, key_bits);
                    return MoltObject::none().bits();
                }
                crate::dict_set_in_place(_py, dict_ptr, key_bits, fn_bits);
            }
            dec_ref_bits(_py, fn_bits);
            dec_ref_bits(_py, key_bits);
        }

        dict_bits
    })
}

// ---------------------------------------------------------------------------
// Protocol intrinsics – structural protocol checks for Molt AOT compilation
// ---------------------------------------------------------------------------

/// Collect the set of structural member names from a protocol class.
///
/// Walks the class `__dict__` and `__annotations__` to extract the names of
/// attributes that constitute the protocol's structural contract.  Returns a
/// frozen set of attribute name strings.
/// Collect structural members from a single class's own annotations and dict,
/// without walking the MRO.  Used as a building block by the full collector.
fn protocol_collect_own_members(
    _py: &crate::PyToken<'_>,
    cls_bits: u64,
    members_bits: u64,
    ignored: &[&[u8]],
) -> Result<(), u64> {
    // Collect names from __annotations__
    let ann_bits = get_attr_default(_py, cls_bits, b"__annotations__", MoltObject::none().bits());
    if exception_pending(_py) {
        return Err(MoltObject::none().bits());
    }
    if !obj_from_bits(ann_bits).is_none() {
        let keys_bits = crate::molt_dict_keys(ann_bits);
        if !exception_pending(_py) && !obj_from_bits(keys_bits).is_none() {
            for name_bits in iter_values(_py, keys_bits)? {
                let name_str = crate::string_obj_to_owned(obj_from_bits(name_bits));
                let skip = match &name_str {
                    Some(s) => ignored.contains(&s.as_bytes()),
                    None => false,
                };
                if !skip {
                    set_add(_py, members_bits, name_bits)?;
                }
            }
            dec_ref_bits(_py, keys_bits);
        }
    }

    // Collect names from __dict__
    let cls_ptr = match maybe_ptr_from_bits(cls_bits) {
        Some(p) => p,
        None => return Ok(()),
    };
    unsafe {
        if object_type_id(cls_ptr) != TYPE_ID_TYPE {
            return Ok(());
        }
    }
    let dict_bits = unsafe { class_dict_bits(cls_ptr) };
    if let Some(dict_ptr) = maybe_ptr_from_bits(dict_bits) {
        unsafe {
            if object_type_id(dict_ptr) == TYPE_ID_DICT {
                let entries = dict_order(dict_ptr);
                for pair in entries.chunks(2) {
                    if pair.len() < 2 {
                        continue;
                    }
                    let name_str = crate::string_obj_to_owned(obj_from_bits(pair[0]));
                    let skip = match &name_str {
                        Some(s) => {
                            let b = s.as_bytes();
                            ignored.contains(&b) || (b.starts_with(b"_") && !b.starts_with(b"__"))
                        }
                        None => false,
                    };
                    if !skip {
                        set_add(_py, members_bits, pair[0])?;
                    }
                }
            }
        }
    }
    Ok(())
}

fn protocol_collect_structural_members(
    _py: &crate::PyToken<'_>,
    proto_bits: u64,
) -> Result<u64, u64> {
    let members_bits = crate::molt_set_new(0);
    if obj_from_bits(members_bits).is_none() {
        return Err(MoltObject::none().bits());
    }

    // Attributes to ignore – internal / dunder bookkeeping that is never part
    // of the structural contract.
    let ignored: &[&[u8]] = &[
        b"__dict__",
        b"__weakref__",
        b"__module__",
        b"__doc__",
        b"__annotations__",
        b"__annotate__",
        b"_is_protocol",
        b"_is_runtime_protocol",
        b"__protocol_attrs__",
        b"__class_getitem__",
        b"__init__",
        b"__new__",
        b"__subclasshook__",
        b"__init_subclass__",
        b"__abstractmethods__",
        // Internal class attributes that are never part of the structural
        // contract — these are set by the compiler or runtime on every class.
        b"__classdictcell__",
        b"__mro__",
        b"__bases__",
        b"__firstlineno__",
        b"__static_attributes__",
        b"__qualname__",
        b"__slots__",
        b"__type_params__",
    ];

    // Collect own members from the protocol class itself.
    protocol_collect_own_members(_py, proto_bits, members_bits, ignored)?;

    // Walk the MRO and merge structural members from parent protocol classes.
    // This is essential for protocol inheritance: if NamedDrawable inherits from
    // Named and Drawable, it must include `name` and `draw` in its contract.
    let mro = class_mro_vec(proto_bits);
    for base_bits in mro {
        // Skip the class itself (already collected) and `object`.
        if base_bits == proto_bits {
            continue;
        }
        // Check if this base is a protocol (_is_protocol == True).
        let is_proto_flag = get_attr_default(
            _py,
            base_bits,
            b"_is_protocol",
            MoltObject::from_bool(false).bits(),
        );
        if exception_pending(_py) {
            dec_ref_bits(_py, members_bits);
            return Err(MoltObject::none().bits());
        }
        if !is_truthy(_py, obj_from_bits(is_proto_flag)) {
            continue;
        }
        // If the parent protocol already has __protocol_attrs__, merge them
        // directly (avoids redundant re-collection).
        let parent_attrs = get_attr_default(
            _py,
            base_bits,
            b"__protocol_attrs__",
            MoltObject::none().bits(),
        );
        if exception_pending(_py) {
            dec_ref_bits(_py, members_bits);
            return Err(MoltObject::none().bits());
        }
        if !obj_from_bits(parent_attrs).is_none() {
            for name_bits in iter_values(_py, parent_attrs)? {
                set_add(_py, members_bits, name_bits)?;
            }
        } else {
            // Parent protocol hasn't been initialized yet; collect its own
            // members inline.
            protocol_collect_own_members(_py, base_bits, members_bits, ignored)?;
        }
    }

    let frozen_bits = unsafe {
        crate::frozenset_from_iter_bits(_py, members_bits).unwrap_or(MoltObject::none().bits())
    };
    dec_ref_bits(_py, members_bits);
    if obj_from_bits(frozen_bits).is_none() && exception_pending(_py) {
        return Err(MoltObject::none().bits());
    }
    Ok(frozen_bits)
}

/// `molt_protocol_get_structural_members(proto_cls) -> frozenset[str]`
///
/// Extract the set of attribute names that define the structural contract of
/// a Protocol class.  Used by the typing shim to populate `__protocol_attrs__`.
#[unsafe(no_mangle)]
pub extern "C" fn molt_protocol_get_structural_members(proto_bits: u64) -> u64 {
    crate::with_gil_entry!(_py, {
        match protocol_collect_structural_members(_py, proto_bits) {
            Ok(bits) => bits,
            Err(bits) => bits,
        }
    })
}

/// Full isinstance/issubclass check for Protocol classes.
///
/// 1. **Nominal check** – if `obj` (or `obj`'s type for isinstance) is a
///    nominal subclass of the protocol via the MRO, return True immediately.
///    This works even for non-`@runtime_checkable` protocols, matching
///    CPython's behavior for `class Foo(SomeProtocol): ...`.
///
/// 2. **Structural check** – only attempted if the protocol is
///    `@runtime_checkable`.  Verifies that every name in
///    `proto.__protocol_attrs__` is present on `obj` via `hasattr`.
fn protocol_check_impl(
    _py: &crate::PyToken<'_>,
    proto_bits: u64,
    obj_bits: u64,
) -> Result<bool, u64> {
    // Determine the class to check against the MRO.
    // For issubclass(cls, Proto) obj_bits is already a class;
    // for isinstance(inst, Proto) we need type(inst).
    let obj_is_type = is_type_object(obj_bits);
    let obj_type_bits = if obj_is_type {
        obj_bits
    } else {
        type_of_bits(_py, obj_bits)
    };

    // Nominal subclass check – works for any Protocol, runtime_checkable or not.
    if issubclass_bits(obj_type_bits, proto_bits) {
        return Ok(true);
    }
    // For issubclass(cls, Proto) also accept the class itself.
    if obj_is_type && issubclass_bits(obj_bits, proto_bits) {
        return Ok(true);
    }

    // Structural check requires @runtime_checkable.
    let rt_flag = get_attr_default(
        _py,
        proto_bits,
        b"_is_runtime_protocol",
        MoltObject::from_bool(false).bits(),
    );
    if exception_pending(_py) {
        return Err(MoltObject::none().bits());
    }
    if !is_truthy(_py, obj_from_bits(rt_flag)) {
        return Err(raise_exception::<_>(
            _py,
            "TypeError",
            "Instance and class checks can only be used with @runtime_checkable protocols",
        ));
    }

    // Get protocol attrs
    let attrs_bits = get_attr_default(
        _py,
        proto_bits,
        b"__protocol_attrs__",
        MoltObject::none().bits(),
    );
    if exception_pending(_py) {
        return Err(MoltObject::none().bits());
    }
    if obj_from_bits(attrs_bits).is_none() {
        // No protocol attrs means everything satisfies the protocol
        return Ok(true);
    }

    // Iterate over protocol attrs and check hasattr on the object
    for name_bits in iter_values(_py, attrs_bits)? {
        let has = crate::molt_has_attr_name(obj_bits, name_bits);
        if exception_pending(_py) {
            return Err(MoltObject::none().bits());
        }
        if !is_truthy(_py, obj_from_bits(has)) {
            return Ok(false);
        }
    }

    Ok(true)
}

/// `molt_protocol_check(proto_cls, obj) -> bool`
///
/// Combined nominal + structural isinstance/issubclass check for Protocol
/// classes.  Nominal subclasses always pass; structural checks require
/// `@runtime_checkable`.
#[unsafe(no_mangle)]
pub extern "C" fn molt_protocol_check(proto_bits: u64, obj_bits: u64) -> u64 {
    crate::with_gil_entry!(_py, {
        match protocol_check_impl(_py, proto_bits, obj_bits) {
            Ok(value) => MoltObject::from_bool(value).bits(),
            Err(bits) => bits,
        }
    })
}

/// `molt_protocol_register(proto_cls, cls) -> cls`
///
/// Register a class as a virtual subclass of a Protocol, analogous to
/// `ABCMeta.register()`.  This bumps the ABC invalidation counter so cached
/// negative checks are flushed.
#[unsafe(no_mangle)]
pub extern "C" fn molt_protocol_register(proto_bits: u64, subclass_bits: u64) -> u64 {
    crate::with_gil_entry!(_py, {
        if !is_type_object(subclass_bits) {
            return raise_exception::<_>(_py, "TypeError", "Can only register classes");
        }
        // Delegate to the ABC register machinery which already handles
        // registry, cache invalidation, and cycle detection.
        molt_abc_register(proto_bits, subclass_bits)
    })
}

/// `molt_abc_abstractmethod_check(cls) -> bool`
///
/// Returns True if `cls` has any unimplemented abstract methods (i.e. its
/// `__abstractmethods__` frozenset is non-empty).  Used at class-creation
/// time to determine if instantiation should be blocked.
#[unsafe(no_mangle)]
pub extern "C" fn molt_abc_abstractmethod_check(cls_bits: u64) -> u64 {
    crate::with_gil_entry!(_py, {
        let abs_bits = get_attr_default(
            _py,
            cls_bits,
            b"__abstractmethods__",
            MoltObject::none().bits(),
        );
        if exception_pending(_py) {
            return MoltObject::none().bits();
        }
        if obj_from_bits(abs_bits).is_none() {
            return MoltObject::from_bool(false).bits();
        }
        // Check if the frozenset is non-empty by trying to get its length
        let len_bits = crate::molt_len(abs_bits);
        if exception_pending(_py) {
            return MoltObject::from_bool(false).bits();
        }
        let len_val = crate::to_i64(obj_from_bits(len_bits)).unwrap_or(0);
        MoltObject::from_bool(len_val > 0).bits()
    })
}

// ---------------------------------------------------------------------------
// typing helpers – lowered so the compiler can elide call frames
// ---------------------------------------------------------------------------

/// `typing.cast(typ, val)` → identity (returns val, ignores typ).
/// Lowered so the compiler can recognise the no-op and elide the frame.
#[unsafe(no_mangle)]
pub extern "C" fn molt_typing_cast(_typ_bits: u64, val_bits: u64) -> u64 {
    val_bits
}

/// `typing.get_origin(tp)` → `tp.__origin__` or None.
#[unsafe(no_mangle)]
pub extern "C" fn molt_typing_get_origin(tp_bits: u64) -> u64 {
    crate::with_gil_entry!(_py, {
        get_attr_default(_py, tp_bits, b"__origin__", MoltObject::none().bits())
    })
}

/// `typing.get_args(tp)` → `tp.__args__` or `()`.
#[unsafe(no_mangle)]
pub extern "C" fn molt_typing_get_args(tp_bits: u64) -> u64 {
    crate::with_gil_entry!(_py, {
        let empty_tuple = alloc_tuple(_py, &[]) as u64;
        get_attr_default(_py, tp_bits, b"__args__", empty_tuple)
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Once;

    static INIT: Once = Once::new();

    fn init_runtime() {
        INIT.call_once(|| {
            assert_ne!(crate::lifecycle::init(), 0);
        });
        let _ = crate::molt_exception_clear();
    }

    #[test]
    fn get_attr_default_reads_inherited_type_attrs_through_mro() {
        init_runtime();

        let (set_base_ok, set_base_pending, set_attr_ok, set_attr_pending, inherited) =
            crate::with_gil_entry!(_py, {
                let builtins = builtin_classes(_py);
                let none_bits = MoltObject::none().bits();

                let base_name_ptr = alloc_string(_py, b"AbcAttrBase");
                if base_name_ptr.is_null() {
                    return (
                        false,
                        exception_pending(_py),
                        false,
                        exception_pending(_py),
                        MoltObject::none().bits(),
                    );
                }
                let base_name_bits = MoltObject::from_ptr(base_name_ptr).bits();
                let base_bases_ptr = alloc_tuple(_py, &[builtins.object]);
                if base_bases_ptr.is_null() {
                    dec_ref_bits(_py, base_name_bits);
                    return (
                        false,
                        exception_pending(_py),
                        false,
                        exception_pending(_py),
                        MoltObject::none().bits(),
                    );
                }
                let base_bases_bits = MoltObject::from_ptr(base_bases_ptr).bits();
                let base_ns_ptr = alloc_dict_with_pairs(_py, &[]);
                if base_ns_ptr.is_null() {
                    dec_ref_bits(_py, base_name_bits);
                    dec_ref_bits(_py, base_bases_bits);
                    return (
                        false,
                        exception_pending(_py),
                        false,
                        exception_pending(_py),
                        MoltObject::none().bits(),
                    );
                }
                let base_ns_bits = MoltObject::from_ptr(base_ns_ptr).bits();
                let base_bits = crate::molt_type_new(
                    builtins.type_obj,
                    base_name_bits,
                    base_bases_bits,
                    base_ns_bits,
                    none_bits,
                );
                dec_ref_bits(_py, base_name_bits);
                dec_ref_bits(_py, base_bases_bits);
                dec_ref_bits(_py, base_ns_bits);

                let child_name_ptr = alloc_string(_py, b"AbcAttrChild");
                if child_name_ptr.is_null() {
                    dec_ref_bits(_py, base_bits);
                    return (
                        false,
                        exception_pending(_py),
                        false,
                        exception_pending(_py),
                        MoltObject::none().bits(),
                    );
                }
                let child_name_bits = MoltObject::from_ptr(child_name_ptr).bits();
                let child_bases_ptr = alloc_tuple(_py, &[base_bits]);
                if child_bases_ptr.is_null() {
                    dec_ref_bits(_py, base_bits);
                    dec_ref_bits(_py, child_name_bits);
                    return (
                        false,
                        exception_pending(_py),
                        false,
                        exception_pending(_py),
                        MoltObject::none().bits(),
                    );
                }
                let child_bases_bits = MoltObject::from_ptr(child_bases_ptr).bits();
                let child_ns_ptr = alloc_dict_with_pairs(_py, &[]);
                if child_ns_ptr.is_null() {
                    dec_ref_bits(_py, base_bits);
                    dec_ref_bits(_py, child_name_bits);
                    dec_ref_bits(_py, child_bases_bits);
                    return (
                        false,
                        exception_pending(_py),
                        false,
                        exception_pending(_py),
                        MoltObject::none().bits(),
                    );
                }
                let child_ns_bits = MoltObject::from_ptr(child_ns_ptr).bits();
                let child_bits = crate::molt_type_new(
                    builtins.type_obj,
                    child_name_bits,
                    child_bases_bits,
                    child_ns_bits,
                    none_bits,
                );
                dec_ref_bits(_py, child_name_bits);
                dec_ref_bits(_py, child_bases_bits);
                dec_ref_bits(_py, child_ns_bits);
                let set_base_ok = is_type_object(base_bits) && is_type_object(child_bits);
                let set_base_pending = exception_pending(_py);

                let attr_name_bits = attr_name_bits_from_bytes(_py, b"bootstrap_flag")
                    .expect("attr name allocation failed");
                let set_attr = crate::molt_set_attr_name(
                    base_bits,
                    attr_name_bits,
                    MoltObject::from_bool(true).bits(),
                );
                dec_ref_bits(_py, attr_name_bits);
                let set_attr_ok = set_attr == MoltObject::none().bits();
                let set_attr_pending = exception_pending(_py);

                let inherited = get_attr_default(
                    _py,
                    child_bits,
                    b"bootstrap_flag",
                    MoltObject::none().bits(),
                );

                dec_ref_bits(_py, child_bits);
                dec_ref_bits(_py, base_bits);
                (
                    set_base_ok,
                    set_base_pending,
                    set_attr_ok,
                    set_attr_pending,
                    inherited,
                )
            });

        assert!(set_base_ok, "class_set_base failed");
        assert!(
            !set_base_pending,
            "class_set_base left an exception pending"
        );
        assert!(set_attr_ok, "set_attr_name failed for base class attr");
        assert!(!set_attr_pending, "set_attr_name left an exception pending");
        assert_eq!(
            inherited,
            MoltObject::from_bool(true).bits(),
            "type-object attr lookup should honor inherited class attrs",
        );
    }
}
