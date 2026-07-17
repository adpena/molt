use crate::call::type_policy::{InitArgPolicy, resolved_constructor_init_policy};
use crate::{
    MoltObject, PyToken, TYPE_ID_BOUND_METHOD, TYPE_ID_DATACLASS, TYPE_ID_FUNCTION,
    TYPE_ID_GENERIC_ALIAS, TYPE_ID_OBJECT, TYPE_ID_TYPE, bound_method_func_bits,
    call_builtin_type_if_needed, call_function_obj_vec, class_attr_lookup_raw_mro,
    class_name_for_error, dec_ref_bits, exception_pending, exception_stack_baseline_get,
    exception_stack_baseline_set, function_arity, generic_alias_origin_bits, intern_static_name,
    lookup_call_attr, molt_call_bind, molt_callargs_new, molt_callargs_push_pos, obj_from_bits,
    object_type_id, raise_exception, raise_not_callable, runtime_state, try_call_generator,
};

struct ExceptionBaselineGuard {
    prev: usize,
}

impl ExceptionBaselineGuard {
    fn new() -> Self {
        Self {
            prev: exception_stack_baseline_get(),
        }
    }
}

impl Drop for ExceptionBaselineGuard {
    fn drop(&mut self) {
        exception_stack_baseline_set(self.prev);
    }
}

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
    crate::with_gil_entry_nopanic!(_py, {
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
                crate::builtins::functions::python_builtin_function_bits(_py, name)
            {
                return bind_owned_callable(_py, func_bits, builder_bits);
            }
            if let Some(func_bits) =
                crate::intrinsics::registry::try_resolve_intrinsic_func(_py, name, true)
            {
                return bind_owned_callable(_py, func_bits, builder_bits);
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
            bind_owned_callable(_py, callable_bits, builder_bits)
        }
    })
}

unsafe fn bind_owned_callable(_py: &PyToken<'_>, callable_bits: u64, builder_bits: u64) -> u64 {
    let result = molt_call_bind(callable_bits, builder_bits);
    dec_ref_bits(_py, callable_bits);
    result
}

unsafe fn call_generic_alias_via_bind(_py: &PyToken<'_>, alias_ptr: *mut u8, args: &[u64]) -> u64 {
    unsafe {
        let origin_bits = generic_alias_origin_bits(alias_ptr);
        call_type_via_bind(_py, origin_bits, args)
    }
}

pub(crate) unsafe fn call_callable0(_py: &PyToken<'_>, call_bits: u64) -> u64 {
    unsafe {
        let _baseline_guard = ExceptionBaselineGuard::new();
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
                call_function_obj_vec(_py, call_bits, &[])
            }
            TYPE_ID_BOUND_METHOD => call_type_via_bind(_py, call_bits, &[]),
            TYPE_ID_TYPE => call_type_via_bind(_py, call_bits, &[]),
            crate::TYPE_ID_FOREIGN => call_type_via_bind(_py, call_bits, &[]),
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
        let _baseline_guard = ExceptionBaselineGuard::new();
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
                call_function_obj_vec(_py, call_bits, &[arg0_bits])
            }
            TYPE_ID_BOUND_METHOD => call_type_via_bind(_py, call_bits, &[arg0_bits]),
            TYPE_ID_TYPE => call_type_via_bind(_py, call_bits, &[arg0_bits]),
            crate::TYPE_ID_FOREIGN => call_type_via_bind(_py, call_bits, &[arg0_bits]),
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
        let _baseline_guard = ExceptionBaselineGuard::new();
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
                call_function_obj_vec(_py, call_bits, &[arg0_bits, arg1_bits])
            }
            TYPE_ID_BOUND_METHOD => call_type_via_bind(_py, call_bits, &[arg0_bits, arg1_bits]),
            TYPE_ID_TYPE => call_type_via_bind(_py, call_bits, &[arg0_bits, arg1_bits]),
            crate::TYPE_ID_FOREIGN => call_type_via_bind(_py, call_bits, &[arg0_bits, arg1_bits]),
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
        let _baseline_guard = ExceptionBaselineGuard::new();
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
                call_function_obj_vec(_py, call_bits, &[arg0_bits, arg1_bits, arg2_bits])
            }
            TYPE_ID_BOUND_METHOD => {
                call_type_via_bind(_py, call_bits, &[arg0_bits, arg1_bits, arg2_bits])
            }
            TYPE_ID_TYPE => call_type_via_bind(_py, call_bits, &[arg0_bits, arg1_bits, arg2_bits]),
            crate::TYPE_ID_FOREIGN => {
                call_type_via_bind(_py, call_bits, &[arg0_bits, arg1_bits, arg2_bits])
            }
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

#[cfg(test)]
mod tests {
    use super::*;
    use core::sync::atomic::Ordering as AtomicOrdering;

    fn populated_python_builtin_function_slots(_py: &PyToken<'_>) -> usize {
        crate::runtime_state(_py)
            .python_builtin_function_slots
            .get()
            .map(|slots| {
                slots
                    .iter()
                    .filter(|slot| slot.load(AtomicOrdering::Acquire) != 0)
                    .count()
            })
            .unwrap_or(0)
    }

    fn single_cached_python_builtin_bits(_py: &PyToken<'_>) -> u64 {
        let slots = crate::runtime_state(_py)
            .python_builtin_function_slots
            .get()
            .expect("python builtin cache should be initialized");
        let cached = slots
            .iter()
            .filter_map(|slot| {
                let bits = slot.load(AtomicOrdering::Acquire);
                (bits != 0).then_some(bits)
            })
            .collect::<Vec<_>>();
        assert_eq!(cached.len(), 1);
        cached[0]
    }

    fn object_ref_count(bits: u64) -> u32 {
        let ptr = obj_from_bits(bits)
            .as_ptr()
            .expect("cached builtin must be an object");
        unsafe {
            (*crate::header_from_obj_ptr(ptr))
                .ref_count
                .load(AtomicOrdering::Acquire)
        }
    }

    #[test]
    fn call_builtin_prefers_generated_builtin_cache_over_intrinsic_alias() {
        let _guard = crate::test_mutex_guard();
        crate::with_gil_entry_nopanic!(_py, {
            crate::builtins::functions::python_builtin_functions_clear_runtime_state(
                _py,
                crate::runtime_state(_py),
            );
            assert_eq!(populated_python_builtin_function_slots(_py), 0);

            let name_ptr = crate::alloc_string(_py, b"len");
            assert!(!name_ptr.is_null());
            let name_bits = MoltObject::from_ptr(name_ptr).bits();
            let arg_ptr = crate::alloc_tuple(_py, &[]);
            assert!(!arg_ptr.is_null());
            let arg_bits = MoltObject::from_ptr(arg_ptr).bits();
            let builder_bits = molt_callargs_new(1, 0);
            assert!(!obj_from_bits(builder_bits).is_none());
            let push_result = unsafe { molt_callargs_push_pos(builder_bits, arg_bits) };
            assert!(obj_from_bits(push_result).is_none());

            let result_bits = molt_call_builtin(name_bits, builder_bits);
            assert!(
                !exception_pending(_py),
                "generated builtin call path must not raise"
            );
            assert_eq!(crate::to_i64(obj_from_bits(result_bits)), Some(0));
            assert_eq!(
                populated_python_builtin_function_slots(_py),
                1,
                "direct builtin calls must populate the generated builtin callable cache"
            );
            let cached_bits = single_cached_python_builtin_bits(_py);
            assert_eq!(
                object_ref_count(cached_bits),
                1,
                "molt_call_builtin must release the owned generated callable reference after binding"
            );

            crate::dec_ref_bits(_py, result_bits);
            crate::dec_ref_bits(_py, arg_bits);
            crate::dec_ref_bits(_py, name_bits);
        });
    }
}
