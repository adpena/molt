use crate::{
    TYPE_ID_BOUND_METHOD, TYPE_ID_FUNCTION, bound_method_func_bits, function_fn_ptr,
    molt_object_init, molt_object_new_bound, obj_from_bits, object_type_id,
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
pub(crate) unsafe fn resolved_new_is_default_object_new(new_bits: Option<u64>) -> bool {
    unsafe { callable_function_addr(new_bits) == Some(fn_addr!(molt_object_new_bound)) }
}

#[inline]
pub(crate) unsafe fn resolved_constructor_init_policy(
    new_bits: Option<u64>,
    init_bits: Option<u64>,
) -> InitArgPolicy {
    unsafe {
        let init_is_object = callable_function_addr(init_bits) == Some(fn_addr!(molt_object_init));
        if !init_is_object {
            return InitArgPolicy::ForwardArgs;
        }
        if resolved_new_is_default_object_new(new_bits) {
            InitArgPolicy::RejectConstructorArgs
        } else {
            InitArgPolicy::SkipObjectInit
        }
    }
}
