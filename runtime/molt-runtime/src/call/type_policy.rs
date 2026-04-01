use crate::{
    TYPE_ID_BOUND_METHOD, TYPE_ID_FUNCTION, bound_method_func_bits, function_fn_ptr,
    function_trampoline_ptr, molt_object_init, molt_object_new_bound, obj_from_bits,
    object_type_id,
};

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum InitArgPolicy {
    ForwardArgs,
    RejectConstructorArgs,
    SkipObjectInit,
}

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
    use super::callable_matches_runtime_symbol;
    use crate::builtins::methods::{object_method_bits, type_method_bits};

    #[test]
    fn object_builtin_methods_match_runtime_symbols() {
        crate::with_gil_entry!(_py, {
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
        crate::with_gil_entry!(_py, {
            let call_bits = type_method_bits(_py, "__call__");
            assert!(unsafe {
                callable_matches_runtime_symbol(call_bits, fn_addr!(crate::molt_type_call))
            });
        });
    }
}
