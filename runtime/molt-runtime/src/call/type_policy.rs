use crate::{
    PyToken, TYPE_ID_BOUND_METHOD, TYPE_ID_FUNCTION, bound_method_func_bits,
    class_attr_lookup_raw_mro, exception_pending, function_fn_ptr, function_trampoline_ptr,
    intern_static_name, molt_object_init, molt_object_new_bound, obj_from_bits, object_type_id,
    runtime_state,
};

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum InitArgPolicy {
    ForwardArgs,
    RejectConstructorArgs,
    SkipObjectInit,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum ObjectConstructorCall {
    New,
    Init,
}

#[allow(dead_code)]
#[inline]
pub(crate) unsafe fn callable_function_addr(bits: Option<u64>) -> Option<u64> {
    unsafe {
        let bits = bits?;
        let mut func_ptr = obj_from_bits(bits).as_ptr()?;
        if object_type_id(func_ptr) == TYPE_ID_BOUND_METHOD {
            let inner_bits = bound_method_func_bits(func_ptr);
            func_ptr = obj_from_bits(inner_bits).as_ptr()?;
        }
        if object_type_id(func_ptr) != TYPE_ID_FUNCTION {
            return None;
        }
        Some(function_fn_ptr(func_ptr))
    }
}

#[inline]
pub(crate) unsafe fn callable_matches_runtime_symbol(
    bits: Option<u64>,
    symbol_fn_ptr: u64,
) -> bool {
    unsafe {
        let bits = bits.unwrap_or(0);
        let mut func_ptr = match obj_from_bits(bits).as_ptr() {
            Some(ptr) => ptr,
            None => return false,
        };
        if object_type_id(func_ptr) == TYPE_ID_BOUND_METHOD {
            let inner_bits = bound_method_func_bits(func_ptr);
            func_ptr = match obj_from_bits(inner_bits).as_ptr() {
                Some(ptr) => ptr,
                None => return false,
            };
        }
        if object_type_id(func_ptr) != TYPE_ID_FUNCTION {
            return false;
        }
        crate::builtins::functions::runtime_callable_represents_symbol(
            function_fn_ptr(func_ptr),
            function_trampoline_ptr(func_ptr),
            symbol_fn_ptr,
        )
    }
}

#[inline]
pub(crate) unsafe fn resolved_new_is_default_object_new(new_bits: Option<u64>) -> bool {
    unsafe { callable_matches_runtime_symbol(new_bits, fn_addr!(molt_object_new_bound)) }
}

/// Whether CPython permits extra arguments when `object.__new__` or
/// `object.__init__` is invoked for `class_bits`.
///
/// The allowance is deliberately asymmetric. The inherited `object` half may
/// ignore arguments only when the complementary constructor half is custom and
/// consumes them. Generic builtin binding must not truncate arguments outside
/// this constructor policy.
#[inline]
pub(crate) unsafe fn object_constructor_extra_args_allowed(
    _py: &PyToken<'_>,
    class_bits: u64,
    call: ObjectConstructorCall,
) -> bool {
    unsafe {
        let Some(class_ptr) = obj_from_bits(class_bits).as_ptr() else {
            return false;
        };
        if object_type_id(class_ptr) != crate::TYPE_ID_TYPE {
            return false;
        }

        let new_name_bits =
            intern_static_name(_py, &runtime_state(_py).interned.new_name, b"__new__");
        let init_name_bits =
            intern_static_name(_py, &runtime_state(_py).interned.init_name, b"__init__");
        let Some(resolved_new) = class_attr_lookup_raw_mro(_py, class_ptr, new_name_bits) else {
            return false;
        };
        if exception_pending(_py) {
            return false;
        }
        let Some(resolved_init) = class_attr_lookup_raw_mro(_py, class_ptr, init_name_bits) else {
            return false;
        };
        if exception_pending(_py) {
            return false;
        }
        let new_is_object =
            callable_matches_runtime_symbol(Some(resolved_new), fn_addr!(molt_object_new_bound));
        let init_is_object =
            callable_matches_runtime_symbol(Some(resolved_init), fn_addr!(molt_object_init));

        match call {
            ObjectConstructorCall::New => new_is_object && !init_is_object,
            ObjectConstructorCall::Init => init_is_object && !new_is_object,
        }
    }
}

#[inline]
pub(crate) unsafe fn resolved_constructor_init_policy(
    new_bits: Option<u64>,
    init_bits: Option<u64>,
) -> InitArgPolicy {
    unsafe {
        let init_is_object = callable_matches_runtime_symbol(init_bits, fn_addr!(molt_object_init));
        if !init_is_object {
            return InitArgPolicy::ForwardArgs;
        }
        let new_is_object = resolved_new_is_default_object_new(new_bits);
        if new_is_object {
            // Both __init__ and __new__ are inherited from object.
            // CPython rejects extra args: "X() takes no arguments".
            return InitArgPolicy::RejectConstructorArgs;
        }
        // __init__ is object.__init__ but __new__ is overridden —
        // CPython 3.12+ accepts and ignores extra args in __init__
        // when __new__ is custom (the custom __new__ consumes them).
        InitArgPolicy::SkipObjectInit
    }
}

#[cfg(test)]
mod tests {
    use super::{
        InitArgPolicy, ObjectConstructorCall, callable_matches_runtime_symbol,
        object_constructor_extra_args_allowed, resolved_constructor_init_policy,
    };
    use crate::builtins::methods::{object_method_bits, type_method_bits};
    use crate::{MoltObject, dec_ref_bits, obj_from_bits};

    extern "C" fn custom_new_policy_probe(_cls_bits: u64) -> i64 {
        0
    }

    extern "C" fn custom_init_policy_probe(_self_bits: u64) -> i64 {
        0
    }

    unsafe fn empty_policy_type(_py: &crate::PyToken<'_>, name: &[u8]) -> u64 {
        unsafe {
            let name_ptr = crate::alloc_string(_py, name);
            let namespace_ptr = crate::alloc_dict_with_pairs(_py, &[]);
            let bases_ptr = crate::alloc_tuple(_py, &[]);
            assert!(!name_ptr.is_null() && !namespace_ptr.is_null() && !bases_ptr.is_null());
            let class_bits = crate::molt_type_new(
                crate::builtin_classes(_py).type_obj,
                MoltObject::from_ptr(name_ptr).bits(),
                MoltObject::from_ptr(bases_ptr).bits(),
                MoltObject::from_ptr(namespace_ptr).bits(),
                MoltObject::none().bits(),
            );
            for ptr in [name_ptr, namespace_ptr, bases_ptr] {
                dec_ref_bits(_py, MoltObject::from_ptr(ptr).bits());
            }
            assert!(!crate::exception_pending(_py));
            assert_eq!(
                crate::object_type_id(obj_from_bits(class_bits).as_ptr().unwrap()),
                crate::TYPE_ID_TYPE
            );
            class_bits
        }
    }

    unsafe fn install_policy_method(
        _py: &crate::PyToken<'_>,
        class_bits: u64,
        name: &[u8],
        function: *const (),
    ) {
        unsafe {
            let function_ptr = crate::builtins::functions::alloc_runtime_function_obj(
                _py,
                crate::provenance::abi::expose_function_address(function),
                1,
            );
            assert!(!function_ptr.is_null());
            let function_bits = MoltObject::from_ptr(function_ptr).bits();
            let name_bits = crate::attr_name_bits_from_bytes(_py, name).unwrap();
            let class_ptr = obj_from_bits(class_bits).as_ptr().unwrap();
            let dictionary_bits = crate::class_dict_bits(class_ptr);
            let dictionary_ptr = obj_from_bits(dictionary_bits).as_ptr().unwrap();
            crate::dict_set_in_place(_py, dictionary_ptr, name_bits, function_bits);
            dec_ref_bits(_py, name_bits);
            dec_ref_bits(_py, function_bits);
        }
    }

    #[test]
    fn object_builtin_methods_match_runtime_symbols() {
        crate::with_gil_entry_nopanic!(_py, {
            let new_bits = object_method_bits(_py, "__new__");
            let init_bits = object_method_bits(_py, "__init__");
            assert!(unsafe {
                callable_matches_runtime_symbol(new_bits, fn_addr!(crate::molt_object_new_bound))
            });
            assert!(unsafe {
                callable_matches_runtime_symbol(init_bits, fn_addr!(crate::molt_object_init))
            });
        });
    }

    #[test]
    fn type_call_matches_runtime_symbol() {
        crate::with_gil_entry_nopanic!(_py, {
            let call_bits = type_method_bits(_py, "__call__");
            assert!(unsafe {
                callable_matches_runtime_symbol(call_bits, fn_addr!(crate::molt_type_call))
            });
        });
    }

    #[test]
    fn exception_builtin_methods_match_runtime_symbols() {
        crate::with_gil_entry_nopanic!(_py, {
            let new_bits = crate::builtins::exceptions::exception_method_bits(_py, "__new__");
            let init_bits = crate::builtins::exceptions::exception_method_bits(_py, "__init__");
            assert!(unsafe {
                callable_matches_runtime_symbol(
                    new_bits,
                    fn_addr!(crate::builtins::exceptions::molt_exception_new_bound),
                )
            });
            assert!(unsafe {
                callable_matches_runtime_symbol(
                    init_bits,
                    fn_addr!(crate::builtins::exceptions::molt_exception_init_owned),
                )
            });
        });
    }

    #[test]
    fn constructor_policy_rejects_args_for_default_object_constructor() {
        crate::with_gil_entry_nopanic!(_py, {
            let new_bits = object_method_bits(_py, "__new__");
            let init_bits = object_method_bits(_py, "__init__");
            assert_eq!(
                unsafe { resolved_constructor_init_policy(new_bits, init_bits) },
                InitArgPolicy::RejectConstructorArgs
            );
        });
    }

    #[test]
    fn constructor_policy_skips_object_init_for_custom_new() {
        crate::with_gil_entry_nopanic!(_py, {
            let new_ptr = crate::builtins::functions::alloc_runtime_function_obj(
                _py,
                custom_new_policy_probe as *const () as usize as u64,
                1,
            );
            assert!(!new_ptr.is_null());
            let new_bits = MoltObject::from_ptr(new_ptr).bits();
            let init_bits = object_method_bits(_py, "__init__");
            assert_eq!(
                unsafe { resolved_constructor_init_policy(Some(new_bits), init_bits) },
                InitArgPolicy::SkipObjectInit
            );
            dec_ref_bits(_py, new_bits);
        });
    }

    #[test]
    fn constructor_policy_forwards_args_to_custom_init_even_with_custom_new() {
        crate::with_gil_entry_nopanic!(_py, {
            let new_ptr = crate::builtins::functions::alloc_runtime_function_obj(
                _py,
                custom_new_policy_probe as *const () as usize as u64,
                1,
            );
            let init_ptr = crate::builtins::functions::alloc_runtime_function_obj(
                _py,
                custom_init_policy_probe as *const () as usize as u64,
                1,
            );
            assert!(!new_ptr.is_null());
            assert!(!init_ptr.is_null());
            let new_bits = MoltObject::from_ptr(new_ptr).bits();
            let init_bits = MoltObject::from_ptr(init_ptr).bits();
            assert_eq!(
                unsafe { resolved_constructor_init_policy(Some(new_bits), Some(init_bits)) },
                InitArgPolicy::ForwardArgs
            );
            dec_ref_bits(_py, init_bits);
            dec_ref_bits(_py, new_bits);
        });
    }

    #[test]
    fn object_constructor_extra_args_follow_complementary_override_policy() {
        let _transaction = crate::test_support::RuntimeTestTransaction::new();
        crate::with_gil_entry_nopanic!(_py, {
            unsafe {
                let neither = empty_policy_type(_py, b"PolicyNeither");
                let init_only = empty_policy_type(_py, b"PolicyInitOnly");
                let new_only = empty_policy_type(_py, b"PolicyNewOnly");
                let both = empty_policy_type(_py, b"PolicyBoth");

                install_policy_method(
                    _py,
                    init_only,
                    b"__init__",
                    custom_init_policy_probe as *const (),
                );
                install_policy_method(
                    _py,
                    new_only,
                    b"__new__",
                    custom_new_policy_probe as *const (),
                );
                install_policy_method(_py, both, b"__new__", custom_new_policy_probe as *const ());
                install_policy_method(
                    _py,
                    both,
                    b"__init__",
                    custom_init_policy_probe as *const (),
                );

                let allowed =
                    |class_bits, call| object_constructor_extra_args_allowed(_py, class_bits, call);
                assert!(!allowed(neither, ObjectConstructorCall::New));
                assert!(!allowed(neither, ObjectConstructorCall::Init));
                assert!(allowed(init_only, ObjectConstructorCall::New));
                assert!(!allowed(init_only, ObjectConstructorCall::Init));
                assert!(!allowed(new_only, ObjectConstructorCall::New));
                assert!(allowed(new_only, ObjectConstructorCall::Init));
                assert!(!allowed(both, ObjectConstructorCall::New));
                assert!(!allowed(both, ObjectConstructorCall::Init));
                assert!(!allowed(
                    MoltObject::none().bits(),
                    ObjectConstructorCall::New
                ));

                for class_bits in [neither, init_only, new_only, both] {
                    dec_ref_bits(_py, class_bits);
                }
            }
        });
    }
}
