use molt_obj_model::MoltObject;

use crate::{
    TYPE_ID_BOUND_METHOD, TYPE_ID_DATACLASS, TYPE_ID_DICT, TYPE_ID_FUNCTION, TYPE_ID_OBJECT,
    TYPE_ID_TYPE, class_attr_lookup_raw_mro, dataclass_desc_ptr, dataclass_dict_bits,
    dict_get_in_place, function_attr_bits, function_closure_bits, function_dict_bits,
    instance_dict_bits, intern_static_name, is_truthy, maybe_ptr_from_bits, obj_from_bits,
    object_class_bits, object_type_id, raise_exception, runtime_state,
};

#[unsafe(no_mangle)]
pub extern "C" fn molt_is_bound_method(obj_bits: u64) -> u64 {
    crate::with_gil_entry!(_py, {
        let is_bound = maybe_ptr_from_bits(obj_bits)
            .is_some_and(|ptr| unsafe { object_type_id(ptr) == TYPE_ID_BOUND_METHOD });
        MoltObject::from_bool(is_bound).bits()
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_is_function_obj(obj_bits: u64) -> u64 {
    crate::with_gil_entry!(_py, {
        let trace_mode = std::env::var("MOLT_TRACE_IS_FUNCTION").ok();
        let log_all = matches!(trace_mode.as_deref(), Some("all"));
        let log_none = matches!(trace_mode.as_deref(), Some("1"));
        let ptr = maybe_ptr_from_bits(obj_bits);
        let is_func = ptr.is_some_and(|ptr| unsafe { object_type_id(ptr) == TYPE_ID_FUNCTION });
        if log_all || (log_none && obj_bits == MoltObject::none().bits()) {
            let type_id = ptr.map(|ptr| unsafe { object_type_id(ptr) });
            eprintln!(
                "molt is_function_obj bits=0x{obj_bits:x} ptr={:?} type_id={:?} is_func={}",
                ptr, type_id, is_func
            );
        }
        MoltObject::from_bool(is_func).bits()
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_function_is_generator(func_bits: u64) -> u64 {
    crate::with_gil_entry!(_py, {
        let obj = obj_from_bits(func_bits);
        let Some(ptr) = obj.as_ptr() else {
            return MoltObject::from_bool(false).bits();
        };
        unsafe {
            if object_type_id(ptr) != TYPE_ID_FUNCTION {
                return MoltObject::from_bool(false).bits();
            }
            let name_bits = intern_static_name(
                _py,
                &runtime_state(_py).interned.molt_is_generator,
                b"__molt_is_generator__",
            );
            let Some(bits) = function_attr_bits(_py, ptr, name_bits) else {
                return MoltObject::from_bool(false).bits();
            };
            MoltObject::from_bool(is_truthy(_py, obj_from_bits(bits))).bits()
        }
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_function_is_coroutine(func_bits: u64) -> u64 {
    crate::with_gil_entry!(_py, {
        let obj = obj_from_bits(func_bits);
        let Some(ptr) = obj.as_ptr() else {
            return MoltObject::from_bool(false).bits();
        };
        unsafe {
            if object_type_id(ptr) != TYPE_ID_FUNCTION {
                return MoltObject::from_bool(false).bits();
            }
            let name_bits = intern_static_name(
                _py,
                &runtime_state(_py).interned.molt_is_coroutine,
                b"__molt_is_coroutine__",
            );
            let Some(bits) = function_attr_bits(_py, ptr, name_bits) else {
                return MoltObject::from_bool(false).bits();
            };
            MoltObject::from_bool(is_truthy(_py, obj_from_bits(bits))).bits()
        }
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_is_callable(obj_bits: u64) -> u64 {
    crate::with_gil_entry!(_py, {
        let is_callable = maybe_ptr_from_bits(obj_bits).is_some_and(|ptr| unsafe {
            match object_type_id(ptr) {
                TYPE_ID_FUNCTION | TYPE_ID_BOUND_METHOD | TYPE_ID_TYPE => true,
                TYPE_ID_OBJECT => {
                    let call_bits = intern_static_name(
                        _py,
                        &runtime_state(_py).interned.call_name,
                        b"__call__",
                    );
                    let dict_bits = instance_dict_bits(ptr);
                    if dict_bits != 0 {
                        if let Some(dict_ptr) = obj_from_bits(dict_bits).as_ptr() {
                            if object_type_id(dict_ptr) == TYPE_ID_DICT {
                                if let Some(found_bits) =
                                    dict_get_in_place(_py, dict_ptr, call_bits)
                                {
                                    if !obj_from_bits(found_bits).is_none() {
                                        return true;
                                    }
                                }
                            }
                        }
                    }
                    let class_bits = object_class_bits(ptr);
                    if class_bits != 0 {
                        if let Some(class_ptr) = obj_from_bits(class_bits).as_ptr() {
                            if object_type_id(class_ptr) == TYPE_ID_TYPE {
                                if let Some(found_bits) =
                                    class_attr_lookup_raw_mro(_py, class_ptr, call_bits)
                                {
                                    return !obj_from_bits(found_bits).is_none();
                                }
                                return false;
                            }
                        }
                    }
                    false
                }
                TYPE_ID_DATACLASS => {
                    let call_bits = intern_static_name(
                        _py,
                        &runtime_state(_py).interned.call_name,
                        b"__call__",
                    );
                    let desc_ptr = dataclass_desc_ptr(ptr);
                    if !desc_ptr.is_null() && !(*desc_ptr).slots {
                        let dict_bits = dataclass_dict_bits(ptr);
                        if dict_bits != 0 {
                            if let Some(dict_ptr) = obj_from_bits(dict_bits).as_ptr() {
                                if object_type_id(dict_ptr) == TYPE_ID_DICT {
                                    if let Some(found_bits) =
                                        dict_get_in_place(_py, dict_ptr, call_bits)
                                    {
                                        if !obj_from_bits(found_bits).is_none() {
                                            return true;
                                        }
                                    }
                                }
                            }
                        }
                    }
                    if !desc_ptr.is_null() {
                        let class_bits = (*desc_ptr).class_bits;
                        if class_bits != 0 {
                            if let Some(class_ptr) = obj_from_bits(class_bits).as_ptr() {
                                if object_type_id(class_ptr) == TYPE_ID_TYPE {
                                    if let Some(found_bits) =
                                        class_attr_lookup_raw_mro(_py, class_ptr, call_bits)
                                    {
                                        return !obj_from_bits(found_bits).is_none();
                                    }
                                    return false;
                                }
                            }
                        }
                    }
                    false
                }
                _ => false,
            }
        });
        MoltObject::from_bool(is_callable).bits()
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_function_default_kind(func_bits: u64) -> i64 {
    crate::with_gil_entry!(_py, {
        let obj = obj_from_bits(func_bits);
        let Some(ptr) = obj.as_ptr() else {
            return 0;
        };
        unsafe {
            if object_type_id(ptr) != TYPE_ID_FUNCTION {
                return 0;
            }
            let dict_bits = function_dict_bits(ptr);
            if dict_bits == 0 {
                return 0;
            }
            obj_from_bits(dict_bits).as_int().unwrap_or(0)
        }
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_function_closure_bits(func_bits: u64) -> u64 {
    crate::with_gil_entry!(_py, {
        let obj = obj_from_bits(func_bits);
        let Some(ptr) = obj.as_ptr() else {
            return 0;
        };
        unsafe {
            if object_type_id(ptr) != TYPE_ID_FUNCTION {
                return 0;
            }
            function_closure_bits(ptr)
        }
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_call_arity_error(expected: i64, got: i64) -> u64 {
    crate::with_gil_entry!(_py, {
        let msg = format!("call arity mismatch (expected {expected}, got {got})");
        raise_exception::<_>(_py, "TypeError", &msg)
    })
}
