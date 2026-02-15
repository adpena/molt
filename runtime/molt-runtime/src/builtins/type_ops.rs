use crate::object::HEADER_FLAG_COROUTINE;
use crate::*;

pub(crate) unsafe fn class_mro_ref(class_ptr: *mut u8) -> Option<&'static Vec<u64>> {
    unsafe {
        let mro_bits = class_mro_bits(class_ptr);
        let mro_obj = obj_from_bits(mro_bits);
        let mro_ptr = mro_obj.as_ptr()?;
        if object_type_id(mro_ptr) != TYPE_ID_TUPLE {
            return None;
        }
        Some(seq_vec_ref(mro_ptr))
    }
}

pub(crate) fn class_mro_vec(class_bits: u64) -> Vec<u64> {
    let obj = obj_from_bits(class_bits);
    let Some(ptr) = obj.as_ptr() else {
        return vec![class_bits];
    };
    unsafe {
        if object_type_id(ptr) != TYPE_ID_TYPE {
            return vec![class_bits];
        }
        if let Some(mro) = class_mro_ref(ptr) {
            return mro.clone();
        }
        let mut out = vec![class_bits];
        let bases_bits = class_bases_bits(ptr);
        let bases = class_bases_vec(bases_bits);
        for base in bases {
            out.extend(class_mro_vec(base));
        }
        out
    }
}

pub(crate) fn class_bases_vec(bits: u64) -> Vec<u64> {
    let obj = obj_from_bits(bits);
    if obj.is_none() || bits == 0 {
        return Vec::new();
    }
    if let Some(ptr) = obj.as_ptr() {
        unsafe {
            match object_type_id(ptr) {
                TYPE_ID_TYPE => return vec![bits],
                TYPE_ID_TUPLE => return seq_vec_ref(ptr).clone(),
                _ => {}
            }
        }
    }
    Vec::new()
}

pub(crate) fn type_of_bits(_py: &PyToken<'_>, val_bits: u64) -> u64 {
    let builtins = builtin_classes(_py);
    let obj = obj_from_bits(val_bits);
    if obj.is_none() {
        return builtins.none_type;
    }
    if val_bits == ellipsis_bits(_py) {
        return builtins.ellipsis_type;
    }
    if obj.is_bool() {
        return builtins.bool;
    }
    if obj.is_int() {
        return builtins.int;
    }
    if obj.is_float() {
        return builtins.float;
    }
    if let Some(ptr) = obj.as_ptr() {
        unsafe {
            let class_bits = object_class_bits(ptr);
            if class_bits != 0 {
                return class_bits;
            }
            return match object_type_id(ptr) {
                TYPE_ID_DATACLASS => {
                    let desc_ptr = dataclass_desc_ptr(ptr);
                    if !desc_ptr.is_null() {
                        let class_bits = (*desc_ptr).class_bits;
                        if class_bits != 0 {
                            return class_bits;
                        }
                    }
                    builtins.object
                }
                TYPE_ID_STRING => builtins.str,
                TYPE_ID_BYTES => builtins.bytes,
                TYPE_ID_BYTEARRAY => builtins.bytearray,
                TYPE_ID_LIST => builtins.list,
                TYPE_ID_TUPLE => builtins.tuple,
                TYPE_ID_DICT => builtins.dict,
                TYPE_ID_DICT_KEYS_VIEW => builtins.dict_keys,
                TYPE_ID_DICT_ITEMS_VIEW => builtins.dict_items,
                TYPE_ID_DICT_VALUES_VIEW => builtins.dict_values,
                TYPE_ID_SET => builtins.set,
                TYPE_ID_FROZENSET => builtins.frozenset,
                TYPE_ID_BIGINT => builtins.int,
                TYPE_ID_COMPLEX => builtins.complex,
                TYPE_ID_RANGE => builtins.range,
                TYPE_ID_SLICE => builtins.slice,
                TYPE_ID_MEMORYVIEW => builtins.memoryview,
                TYPE_ID_FILE_HANDLE => {
                    let handle_ptr = file_handle_ptr(ptr);
                    if !handle_ptr.is_null() {
                        let handle = &*handle_ptr;
                        if handle.class_bits != 0 {
                            return handle.class_bits;
                        }
                    }
                    builtins.file
                }
                TYPE_ID_NOT_IMPLEMENTED => builtins.not_implemented_type,
                TYPE_ID_ELLIPSIS => builtins.ellipsis_type,
                TYPE_ID_EXCEPTION => {
                    let class_bits = exception_class_bits(ptr);
                    if !obj_from_bits(class_bits).is_none() && class_bits != 0 {
                        class_bits
                    } else {
                        exception_type_bits(_py, exception_kind_bits(ptr))
                    }
                }
                TYPE_ID_FUNCTION => {
                    let class_bits = object_class_bits(ptr);
                    if class_bits != 0 {
                        class_bits
                    } else {
                        builtins.function
                    }
                }
                TYPE_ID_BOUND_METHOD => {
                    let func_bits = bound_method_func_bits(ptr);
                    let func_obj = obj_from_bits(func_bits);
                    let func_ptr = func_obj.as_ptr();
                    if let Some(func_ptr) = func_ptr {
                        let func_class_bits = object_class_bits(func_ptr);
                        if func_class_bits == builtins.builtin_function_or_method {
                            func_class_bits
                        } else {
                            crate::builtins::types::method_class(_py)
                        }
                    } else {
                        crate::builtins::types::method_class(_py)
                    }
                }
                TYPE_ID_GENERATOR => builtins.generator,
                TYPE_ID_ASYNC_GENERATOR => builtins.async_generator,
                TYPE_ID_ITER => {
                    // CPython exposes distinct iterator types (e.g. list_iterator,
                    // str_ascii_iterator). Our iterator object stores the target iterable, so
                    // resolve the type from the target at runtime.
                    let target_bits = iter_target_bits(ptr);
                    let target_obj = obj_from_bits(target_bits);
                    if let Some(target_ptr) = target_obj.as_ptr() {
                        match object_type_id(target_ptr) {
                            TYPE_ID_LIST => builtins.list_iterator,
                            TYPE_ID_TUPLE => builtins.tuple_iterator,
                            TYPE_ID_STRING => {
                                let bytes = std::slice::from_raw_parts(
                                    string_bytes(target_ptr),
                                    string_len(target_ptr),
                                );
                                if bytes.is_ascii() {
                                    builtins.str_ascii_iterator
                                } else {
                                    builtins.str_iterator
                                }
                            }
                            TYPE_ID_BYTES => builtins.bytes_iterator,
                            TYPE_ID_BYTEARRAY => builtins.bytearray_iterator,
                            TYPE_ID_DICT | TYPE_ID_DICT_KEYS_VIEW => builtins.dict_keyiterator,
                            TYPE_ID_DICT_VALUES_VIEW => builtins.dict_valueiterator,
                            TYPE_ID_DICT_ITEMS_VIEW => builtins.dict_itemiterator,
                            TYPE_ID_SET | TYPE_ID_FROZENSET => builtins.set_iterator,
                            TYPE_ID_RANGE => {
                                let start_bits = range_start_bits(target_ptr);
                                let stop_bits = range_stop_bits(target_ptr);
                                let step_bits = range_step_bits(target_ptr);
                                if bigint_ptr_from_bits(start_bits).is_some()
                                    || bigint_ptr_from_bits(stop_bits).is_some()
                                    || bigint_ptr_from_bits(step_bits).is_some()
                                {
                                    builtins.longrange_iterator
                                } else {
                                    builtins.range_iterator
                                }
                            }
                            _ => builtins.iterator,
                        }
                    } else {
                        builtins.iterator
                    }
                }
                TYPE_ID_ENUMERATE => builtins.enumerate,
                TYPE_ID_CALL_ITER => builtins.callable_iterator,
                TYPE_ID_REVERSED => {
                    // CPython exposes distinct reverse iterator types for some builtins
                    // (notably list_reverseiterator and the dict reverse iterators). Our
                    // reversed object stores the target, so resolve the public type name from
                    // that target.
                    let target_bits = reversed_target_bits(ptr);
                    let target_obj = obj_from_bits(target_bits);
                    if let Some(target_ptr) = target_obj.as_ptr() {
                        match object_type_id(target_ptr) {
                            TYPE_ID_LIST => builtins.list_reverseiterator,
                            TYPE_ID_DICT | TYPE_ID_DICT_KEYS_VIEW => {
                                builtins.dict_reversekeyiterator
                            }
                            TYPE_ID_DICT_VALUES_VIEW => builtins.dict_reversevalueiterator,
                            TYPE_ID_DICT_ITEMS_VIEW => builtins.dict_reverseitemiterator,
                            TYPE_ID_RANGE => {
                                let start_bits = range_start_bits(target_ptr);
                                let stop_bits = range_stop_bits(target_ptr);
                                let step_bits = range_step_bits(target_ptr);
                                if bigint_ptr_from_bits(start_bits).is_some()
                                    || bigint_ptr_from_bits(stop_bits).is_some()
                                    || bigint_ptr_from_bits(step_bits).is_some()
                                {
                                    builtins.longrange_iterator
                                } else {
                                    builtins.range_iterator
                                }
                            }
                            _ => builtins.reversed,
                        }
                    } else {
                        builtins.reversed
                    }
                }
                TYPE_ID_ZIP => builtins.zip,
                TYPE_ID_MAP => builtins.map,
                TYPE_ID_FILTER => builtins.filter,
                TYPE_ID_CODE => builtins.code,
                TYPE_ID_MODULE => builtins.module,
                TYPE_ID_TYPE => {
                    let class_bits = object_class_bits(ptr);
                    if class_bits != 0 {
                        class_bits
                    } else {
                        builtins.type_obj
                    }
                }
                TYPE_ID_GENERIC_ALIAS => builtins.generic_alias,
                TYPE_ID_UNION => builtins.union_type,
                TYPE_ID_SUPER => builtins.super_type,
                TYPE_ID_CLASSMETHOD => builtins.classmethod,
                TYPE_ID_STATICMETHOD => builtins.staticmethod,
                TYPE_ID_PROPERTY => builtins.property,
                TYPE_ID_OBJECT => {
                    let header = header_from_obj_ptr(ptr);
                    if ((*header).flags & HEADER_FLAG_COROUTINE) != 0 {
                        return builtins.coroutine;
                    }
                    let class_bits = object_class_bits(ptr);
                    if class_bits != 0 {
                        class_bits
                    } else {
                        builtins.object
                    }
                }
                _ => builtins.object,
            };
        }
    }
    if let Some(ptr) = maybe_ptr_from_bits(val_bits) {
        unsafe {
            let class_bits = object_class_bits(ptr);
            if class_bits != 0 {
                return class_bits;
            }
        }
    }
    builtins.object
}

fn collect_classinfo_isinstance(_py: &PyToken<'_>, class_bits: u64, out: &mut Vec<u64>) {
    let obj = obj_from_bits(class_bits);
    let Some(ptr) = obj.as_ptr() else {
        return raise_exception::<_>(
            _py,
            "TypeError",
            "isinstance() arg 2 must be a type or tuple of types",
        );
    };
    unsafe {
        match object_type_id(ptr) {
            TYPE_ID_TYPE => out.push(class_bits),
            TYPE_ID_TUPLE => {
                let items = seq_vec_ref(ptr);
                for item in items.iter() {
                    collect_classinfo_isinstance(_py, *item, out);
                }
            }
            _ => raise_exception::<_>(
                _py,
                "TypeError",
                "isinstance() arg 2 must be a type or tuple of types",
            ),
        }
    }
}

pub(crate) fn issubclass_bits(sub_bits: u64, class_bits: u64) -> bool {
    if sub_bits == class_bits {
        return true;
    }
    let obj = obj_from_bits(sub_bits);
    let Some(ptr) = obj.as_ptr() else {
        return false;
    };
    unsafe {
        if object_type_id(ptr) != TYPE_ID_TYPE {
            return false;
        }
        if let Some(mro) = class_mro_ref(ptr) {
            return mro.contains(&class_bits);
        }
    }
    class_mro_vec(sub_bits).contains(&class_bits)
}

pub(crate) fn issubclass_runtime(_py: &PyToken<'_>, sub_bits: u64, class_bits: u64) -> bool {
    if sub_bits == class_bits {
        return true;
    }
    let class_obj = obj_from_bits(class_bits);
    let Some(class_ptr) = class_obj.as_ptr() else {
        return false;
    };
    unsafe {
        if object_type_id(class_ptr) != TYPE_ID_TYPE {
            return false;
        }
    }
    let meta_bits = unsafe { object_class_bits(class_ptr) };
    let meta_ptr = if meta_bits != 0 {
        obj_from_bits(meta_bits).as_ptr()
    } else {
        obj_from_bits(builtin_classes(_py).type_obj).as_ptr()
    };
    if let Some(meta_ptr) = meta_ptr {
        unsafe {
            if object_type_id(meta_ptr) == TYPE_ID_TYPE {
                let name_bits = intern_static_name(
                    _py,
                    &runtime_state(_py).interned.subclasscheck_name,
                    b"__subclasscheck__",
                );
                if let Some(check_bits) =
                    class_attr_lookup(_py, meta_ptr, meta_ptr, Some(class_ptr), name_bits)
                {
                    let res_bits = call_callable1(_py, check_bits, sub_bits);
                    dec_ref_bits(_py, check_bits);
                    if exception_pending(_py) {
                        return false;
                    }
                    let res = is_truthy(_py, obj_from_bits(res_bits));
                    dec_ref_bits(_py, res_bits);
                    return res;
                }
            }
        }
    }
    issubclass_bits(sub_bits, class_bits)
}

pub(crate) fn isinstance_bits(_py: &PyToken<'_>, val_bits: u64, class_bits: u64) -> bool {
    let mut classes = Vec::new();
    collect_classinfo_isinstance(_py, class_bits, &mut classes);
    let val_type = type_of_bits(_py, val_bits);
    for class_bits in classes {
        if issubclass_bits(val_type, class_bits) {
            return true;
        }
    }
    false
}

pub(crate) fn isinstance_runtime(_py: &PyToken<'_>, val_bits: u64, class_bits: u64) -> bool {
    let saved_exc_bits = if exception_pending(_py) {
        molt_exception_last()
    } else {
        MoltObject::none().bits()
    };
    let has_saved_exc = !obj_from_bits(saved_exc_bits).is_none() && saved_exc_bits != 0;
    let skip_clear = has_saved_exc && saved_exc_bits == val_bits;
    if has_saved_exc && !skip_clear {
        molt_exception_clear();
    }
    let mut saw_new_exception = false;
    let mut matched = false;
    let mut classes = Vec::new();
    collect_classinfo_isinstance(_py, class_bits, &mut classes);
    let debug_match = std::env::var("MOLT_DEBUG_EXCEPTION_MATCH").as_deref() == Ok("1");
    if debug_match && has_saved_exc && classes.is_empty() {
        let class_type = class_name_for_error(type_of_bits(_py, class_bits));
        eprintln!(
            "molt isinstance match pending=1 classes_empty=1 class_type={}",
            class_type
        );
    }
    for class_bits in classes {
        let class_obj = obj_from_bits(class_bits);
        let Some(class_ptr) = class_obj.as_ptr() else {
            continue;
        };
        unsafe {
            if object_type_id(class_ptr) != TYPE_ID_TYPE {
                continue;
            }
        }
        if !skip_clear {
            let meta_bits = unsafe { object_class_bits(class_ptr) };
            let meta_ptr = if meta_bits != 0 {
                obj_from_bits(meta_bits).as_ptr()
            } else {
                obj_from_bits(builtin_classes(_py).type_obj).as_ptr()
            };
            if let Some(meta_ptr) = meta_ptr {
                unsafe {
                    if object_type_id(meta_ptr) == TYPE_ID_TYPE {
                        let name_bits = intern_static_name(
                            _py,
                            &runtime_state(_py).interned.instancecheck_name,
                            b"__instancecheck__",
                        );
                        if let Some(check_bits) =
                            class_attr_lookup(_py, meta_ptr, meta_ptr, Some(class_ptr), name_bits)
                        {
                            let res_bits = call_callable1(_py, check_bits, val_bits);
                            dec_ref_bits(_py, check_bits);
                            if exception_pending(_py) {
                                saw_new_exception = true;
                                break;
                            }
                            let res = is_truthy(_py, obj_from_bits(res_bits));
                            dec_ref_bits(_py, res_bits);
                            if res {
                                matched = true;
                                break;
                            }
                            continue;
                        }
                    }
                }
            }
        }
        let val_type = type_of_bits(_py, val_bits);
        if debug_match && has_saved_exc {
            let val_name = class_name_for_error(val_type);
            let class_name = class_name_for_error(class_bits);
            eprintln!(
                "molt isinstance match pending=1 val_type={} class={}",
                val_name, class_name
            );
        }
        if issubclass_bits(val_type, class_bits) {
            matched = true;
            break;
        }
    }
    if saw_new_exception {
        if has_saved_exc {
            dec_ref_bits(_py, saved_exc_bits);
        }
        return false;
    }
    if has_saved_exc {
        if !skip_clear {
            let _ = molt_exception_set_last(saved_exc_bits);
        }
        dec_ref_bits(_py, saved_exc_bits);
    }
    matched
}
