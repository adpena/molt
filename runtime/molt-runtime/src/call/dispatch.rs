use crate::call::type_policy::{InitArgPolicy, resolved_constructor_init_policy};
use crate::{
    MoltObject, PyToken, TYPE_ID_BOUND_METHOD, TYPE_ID_DATACLASS, TYPE_ID_FUNCTION,
    TYPE_ID_GENERIC_ALIAS, TYPE_ID_OBJECT, TYPE_ID_TYPE, bound_method_func_bits,
    call_builtin_type_if_needed, call_function_obj0, call_function_obj1, call_function_obj2,
    call_function_obj3, class_attr_lookup_raw_mro, class_name_for_error, exception_pending,
    function_arity, generic_alias_origin_bits, intern_static_name, lookup_call_attr,
    molt_call_bind, molt_callargs_new, molt_callargs_push_pos, obj_from_bits, object_type_id,
    raise_exception, raise_not_callable, runtime_state, try_call_generator,
};

unsafe fn call_type_via_bind(_py: &PyToken<'_>, call_bits: u64, args: &[u64]) -> u64 {
    unsafe {
        if !args.is_empty() {
            let call_obj = obj_from_bits(call_bits);
            let Some(call_ptr) = call_obj.as_ptr() else {
                return raise_not_callable(_py, call_obj);
            };
            if object_type_id(call_ptr) == TYPE_ID_TYPE {
                let new_name_bits =
                    intern_static_name(_py, &runtime_state(_py).interned.new_name, b"__new__");
                let new_bits = class_attr_lookup_raw_mro(_py, call_ptr, new_name_bits);
                let init_name_bits =
                    intern_static_name(_py, &runtime_state(_py).interned.init_name, b"__init__");
                let init_bits = class_attr_lookup_raw_mro(_py, call_ptr, init_name_bits);
                if matches!(
                    resolved_constructor_init_policy(new_bits, init_bits),
                    InitArgPolicy::RejectConstructorArgs
                ) {
                    let class_name = class_name_for_error(call_bits);
                    let msg = format!("{class_name}() takes no arguments");
                    return raise_exception::<_>(_py, "TypeError", &msg);
                }
            }
        }
        let builder_bits = molt_callargs_new(args.len() as u64, 0);
        if builder_bits == 0 {
            return MoltObject::none().bits();
        }
        for &arg in args {
            let _ = molt_callargs_push_pos(builder_bits, arg);
        }
        molt_call_bind(call_bits, builder_bits)
    }
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_call_builtin(name_bits: u64, builder_bits: u64) -> u64 {
    crate::with_gil_entry!(_py, {
        unsafe {
            let name_obj = obj_from_bits(name_bits);
            let Some(name_ptr) = name_obj.as_ptr() else {
                return raise_exception::<_>(_py, "TypeError", "builtin name must be str");
            };
            let name = {
                if object_type_id(name_ptr) != crate::TYPE_ID_STRING {
                    return raise_exception::<_>(_py, "TypeError", "builtin name must be str");
                }
                let len = crate::string_len(name_ptr);
                let bytes = std::slice::from_raw_parts(crate::string_bytes(name_ptr), len);
                std::str::from_utf8(bytes).unwrap_or("")
            };

            if let Some(func_bits) =
                crate::intrinsics::registry::try_resolve_intrinsic_func(_py, name, true)
            {
                return molt_call_bind(func_bits, builder_bits);
            }

            let builtins_bits = {
                let cache = crate::builtins::exceptions::internals::module_cache(_py);
                let guard = cache.lock().unwrap();
                guard.get("builtins").copied()
            };
            let Some(builtins_bits) = builtins_bits else {
                return raise_exception::<_>(
                    _py,
                    "RuntimeError",
                    "builtins module cache missing during builtin call",
                );
            };
            let missing = crate::missing_bits(_py);
            let callable_bits = crate::object::ops_builtins::molt_getattr_builtin(
                builtins_bits,
                name_bits,
                missing,
            );
            if exception_pending(_py) {
                return MoltObject::none().bits();
            }
            molt_call_bind(callable_bits, builder_bits)
        }
    })
}

unsafe fn call_generic_alias_via_bind(_py: &PyToken<'_>, alias_ptr: *mut u8, args: &[u64]) -> u64 {
    unsafe {
        let origin_bits = generic_alias_origin_bits(alias_ptr);
        call_type_via_bind(_py, origin_bits, args)
    }
}

pub(crate) unsafe fn call_callable0(_py: &PyToken<'_>, call_bits: u64) -> u64 {
    unsafe {
        let call_obj = obj_from_bits(call_bits);
        let Some(call_ptr) = call_obj.as_ptr() else {
            return raise_not_callable(_py, call_obj);
        };
        if let Some(bits) = call_builtin_type_if_needed(_py, call_bits, call_ptr, &[]) {
            return bits;
        }
        match object_type_id(call_ptr) {
            TYPE_ID_FUNCTION => {
                if let Some(bits) = try_call_generator(_py, call_bits, &[]) {
                    return bits;
                }
                call_function_obj0(_py, call_bits)
            }
            TYPE_ID_BOUND_METHOD => call_type_via_bind(_py, call_bits, &[]),
            TYPE_ID_TYPE => call_type_via_bind(_py, call_bits, &[]),
            TYPE_ID_GENERIC_ALIAS => call_generic_alias_via_bind(_py, call_ptr, &[]),
            TYPE_ID_OBJECT | TYPE_ID_DATACLASS => {
                let Some(call_attr_bits) = lookup_call_attr(_py, call_ptr) else {
                    return raise_not_callable(_py, call_obj);
                };
                call_callable0(_py, call_attr_bits)
            }
            _ => raise_not_callable(_py, call_obj),
        }
    }
}

pub(crate) unsafe fn call_callable1(_py: &PyToken<'_>, call_bits: u64, arg0_bits: u64) -> u64 {
    unsafe {
        let call_obj = obj_from_bits(call_bits);
        let Some(call_ptr) = call_obj.as_ptr() else {
            return raise_not_callable(_py, call_obj);
        };
        if let Some(bits) = call_builtin_type_if_needed(_py, call_bits, call_ptr, &[arg0_bits]) {
            return bits;
        }
        match object_type_id(call_ptr) {
            TYPE_ID_FUNCTION => {
                if let Some(bits) = try_call_generator(_py, call_bits, &[arg0_bits]) {
                    return bits;
                }
                call_function_obj1(_py, call_bits, arg0_bits)
            }
            TYPE_ID_BOUND_METHOD => call_type_via_bind(_py, call_bits, &[arg0_bits]),
            TYPE_ID_TYPE => call_type_via_bind(_py, call_bits, &[arg0_bits]),
            TYPE_ID_GENERIC_ALIAS => call_generic_alias_via_bind(_py, call_ptr, &[arg0_bits]),
            TYPE_ID_OBJECT | TYPE_ID_DATACLASS => {
                let Some(call_attr_bits) = lookup_call_attr(_py, call_ptr) else {
                    return raise_not_callable(_py, call_obj);
                };
                call_callable1(_py, call_attr_bits, arg0_bits)
            }
            _ => raise_not_callable(_py, call_obj),
        }
    }
}

pub(crate) unsafe fn callable_arity(_py: &PyToken<'_>, call_bits: u64) -> Option<usize> {
    unsafe {
        let call_obj = obj_from_bits(call_bits);
        let call_ptr = call_obj.as_ptr()?;
        match object_type_id(call_ptr) {
            TYPE_ID_FUNCTION => Some(function_arity(call_ptr) as usize),
            TYPE_ID_BOUND_METHOD => {
                let func_bits = bound_method_func_bits(call_ptr);
                let func_obj = obj_from_bits(func_bits);
                let func_ptr = func_obj.as_ptr()?;
                if object_type_id(func_ptr) != TYPE_ID_FUNCTION {
                    return None;
                }
                Some(function_arity(func_ptr) as usize)
            }
            TYPE_ID_GENERIC_ALIAS => {
                let origin_bits = generic_alias_origin_bits(call_ptr);
                callable_arity(_py, origin_bits)
            }
            TYPE_ID_OBJECT | TYPE_ID_DATACLASS => {
                let call_attr_bits = lookup_call_attr(_py, call_ptr)?;
                callable_arity(_py, call_attr_bits)
            }
            _ => None,
        }
    }
}

pub(crate) unsafe fn call_callable2(
    _py: &PyToken<'_>,
    call_bits: u64,
    arg0_bits: u64,
    arg1_bits: u64,
) -> u64 {
    unsafe {
        let call_obj = obj_from_bits(call_bits);
        let Some(call_ptr) = call_obj.as_ptr() else {
            return raise_not_callable(_py, call_obj);
        };
        if let Some(bits) =
            call_builtin_type_if_needed(_py, call_bits, call_ptr, &[arg0_bits, arg1_bits])
        {
            return bits;
        }
        match object_type_id(call_ptr) {
            TYPE_ID_FUNCTION => {
                if let Some(bits) = try_call_generator(_py, call_bits, &[arg0_bits, arg1_bits]) {
                    return bits;
                }
                call_function_obj2(_py, call_bits, arg0_bits, arg1_bits)
            }
            TYPE_ID_BOUND_METHOD => call_type_via_bind(_py, call_bits, &[arg0_bits, arg1_bits]),
            TYPE_ID_TYPE => call_type_via_bind(_py, call_bits, &[arg0_bits, arg1_bits]),
            TYPE_ID_GENERIC_ALIAS => {
                call_generic_alias_via_bind(_py, call_ptr, &[arg0_bits, arg1_bits])
            }
            TYPE_ID_OBJECT | TYPE_ID_DATACLASS => {
                let Some(call_attr_bits) = lookup_call_attr(_py, call_ptr) else {
                    return raise_not_callable(_py, call_obj);
                };
                call_callable2(_py, call_attr_bits, arg0_bits, arg1_bits)
            }
            _ => raise_not_callable(_py, call_obj),
        }
    }
}

pub(crate) unsafe fn call_callable3(
    _py: &PyToken<'_>,
    call_bits: u64,
    arg0_bits: u64,
    arg1_bits: u64,
    arg2_bits: u64,
) -> u64 {
    unsafe {
        let call_obj = obj_from_bits(call_bits);
        let Some(call_ptr) = call_obj.as_ptr() else {
            return raise_not_callable(_py, call_obj);
        };
        if let Some(bits) = call_builtin_type_if_needed(
            _py,
            call_bits,
            call_ptr,
            &[arg0_bits, arg1_bits, arg2_bits],
        ) {
            return bits;
        }
        match object_type_id(call_ptr) {
            TYPE_ID_FUNCTION => {
                if let Some(bits) =
                    try_call_generator(_py, call_bits, &[arg0_bits, arg1_bits, arg2_bits])
                {
                    return bits;
                }
                call_function_obj3(_py, call_bits, arg0_bits, arg1_bits, arg2_bits)
            }
            TYPE_ID_BOUND_METHOD => {
                call_type_via_bind(_py, call_bits, &[arg0_bits, arg1_bits, arg2_bits])
            }
            TYPE_ID_TYPE => call_type_via_bind(_py, call_bits, &[arg0_bits, arg1_bits, arg2_bits]),
            TYPE_ID_GENERIC_ALIAS => {
                call_generic_alias_via_bind(_py, call_ptr, &[arg0_bits, arg1_bits, arg2_bits])
            }
            TYPE_ID_OBJECT | TYPE_ID_DATACLASS => {
                let Some(call_attr_bits) = lookup_call_attr(_py, call_ptr) else {
                    return raise_not_callable(_py, call_obj);
                };
                call_callable3(_py, call_attr_bits, arg0_bits, arg1_bits, arg2_bits)
            }
            _ => raise_not_callable(_py, call_obj),
        }
    }
}
