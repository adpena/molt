use crate::{
    bound_method_func_bits, bound_method_self_bits, call_builtin_type_if_needed,
    call_class_init_with_args, call_function_obj0, call_function_obj1, call_function_obj2,
    call_function_obj3, call_function_obj4, function_arity, lookup_call_attr, obj_from_bits,
    object_type_id, raise_not_callable, try_call_generator, TYPE_ID_BOUND_METHOD,
    TYPE_ID_DATACLASS, TYPE_ID_FUNCTION, TYPE_ID_OBJECT, TYPE_ID_TYPE,
};

pub(crate) unsafe fn call_callable0(call_bits: u64) -> u64 {
    let call_obj = obj_from_bits(call_bits);
    let Some(call_ptr) = call_obj.as_ptr() else {
        return raise_not_callable(call_obj);
    };
    if let Some(bits) = call_builtin_type_if_needed(call_bits, call_ptr, &[]) {
        return bits;
    }
    match object_type_id(call_ptr) {
        TYPE_ID_FUNCTION => {
            if let Some(bits) = try_call_generator(call_bits, &[]) {
                return bits;
            }
            call_function_obj0(call_bits)
        }
        TYPE_ID_BOUND_METHOD => {
            let func_bits = bound_method_func_bits(call_ptr);
            let self_bits = bound_method_self_bits(call_ptr);
            if let Some(bits) = try_call_generator(func_bits, &[self_bits]) {
                return bits;
            }
            call_function_obj1(func_bits, self_bits)
        }
        TYPE_ID_TYPE => call_class_init_with_args(call_ptr, &[]),
        TYPE_ID_OBJECT | TYPE_ID_DATACLASS => {
            let Some(call_attr_bits) = lookup_call_attr(call_ptr) else {
                return raise_not_callable(call_obj);
            };
            call_callable0(call_attr_bits)
        }
        _ => raise_not_callable(call_obj),
    }
}

pub(crate) unsafe fn call_callable1(call_bits: u64, arg0_bits: u64) -> u64 {
    let call_obj = obj_from_bits(call_bits);
    let Some(call_ptr) = call_obj.as_ptr() else {
        return raise_not_callable(call_obj);
    };
    if let Some(bits) = call_builtin_type_if_needed(call_bits, call_ptr, &[arg0_bits]) {
        return bits;
    }
    match object_type_id(call_ptr) {
        TYPE_ID_FUNCTION => {
            if let Some(bits) = try_call_generator(call_bits, &[arg0_bits]) {
                return bits;
            }
            call_function_obj1(call_bits, arg0_bits)
        }
        TYPE_ID_BOUND_METHOD => {
            let func_bits = bound_method_func_bits(call_ptr);
            let self_bits = bound_method_self_bits(call_ptr);
            if let Some(bits) = try_call_generator(func_bits, &[self_bits, arg0_bits]) {
                return bits;
            }
            call_function_obj2(func_bits, self_bits, arg0_bits)
        }
        TYPE_ID_TYPE => call_class_init_with_args(call_ptr, &[arg0_bits]),
        TYPE_ID_OBJECT | TYPE_ID_DATACLASS => {
            let Some(call_attr_bits) = lookup_call_attr(call_ptr) else {
                return raise_not_callable(call_obj);
            };
            call_callable1(call_attr_bits, arg0_bits)
        }
        _ => raise_not_callable(call_obj),
    }
}

pub(crate) unsafe fn callable_arity(call_bits: u64) -> Option<usize> {
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
        TYPE_ID_OBJECT | TYPE_ID_DATACLASS => {
            let call_attr_bits = lookup_call_attr(call_ptr)?;
            callable_arity(call_attr_bits)
        }
        _ => None,
    }
}

pub(crate) unsafe fn call_callable2(call_bits: u64, arg0_bits: u64, arg1_bits: u64) -> u64 {
    let call_obj = obj_from_bits(call_bits);
    let Some(call_ptr) = call_obj.as_ptr() else {
        return raise_not_callable(call_obj);
    };
    if let Some(bits) = call_builtin_type_if_needed(call_bits, call_ptr, &[arg0_bits, arg1_bits]) {
        return bits;
    }
    match object_type_id(call_ptr) {
        TYPE_ID_FUNCTION => {
            if let Some(bits) = try_call_generator(call_bits, &[arg0_bits, arg1_bits]) {
                return bits;
            }
            call_function_obj2(call_bits, arg0_bits, arg1_bits)
        }
        TYPE_ID_BOUND_METHOD => {
            let func_bits = bound_method_func_bits(call_ptr);
            let self_bits = bound_method_self_bits(call_ptr);
            if let Some(bits) = try_call_generator(func_bits, &[self_bits, arg0_bits, arg1_bits]) {
                return bits;
            }
            call_function_obj3(func_bits, self_bits, arg0_bits, arg1_bits)
        }
        TYPE_ID_TYPE => call_class_init_with_args(call_ptr, &[arg0_bits, arg1_bits]),
        TYPE_ID_OBJECT | TYPE_ID_DATACLASS => {
            let Some(call_attr_bits) = lookup_call_attr(call_ptr) else {
                return raise_not_callable(call_obj);
            };
            call_callable2(call_attr_bits, arg0_bits, arg1_bits)
        }
        _ => raise_not_callable(call_obj),
    }
}

pub(crate) unsafe fn call_callable3(
    call_bits: u64,
    arg0_bits: u64,
    arg1_bits: u64,
    arg2_bits: u64,
) -> u64 {
    let call_obj = obj_from_bits(call_bits);
    let Some(call_ptr) = call_obj.as_ptr() else {
        return raise_not_callable(call_obj);
    };
    if let Some(bits) =
        call_builtin_type_if_needed(call_bits, call_ptr, &[arg0_bits, arg1_bits, arg2_bits])
    {
        return bits;
    }
    match object_type_id(call_ptr) {
        TYPE_ID_FUNCTION => {
            if let Some(bits) = try_call_generator(call_bits, &[arg0_bits, arg1_bits, arg2_bits]) {
                return bits;
            }
            call_function_obj3(call_bits, arg0_bits, arg1_bits, arg2_bits)
        }
        TYPE_ID_BOUND_METHOD => {
            let func_bits = bound_method_func_bits(call_ptr);
            let self_bits = bound_method_self_bits(call_ptr);
            if let Some(bits) =
                try_call_generator(func_bits, &[self_bits, arg0_bits, arg1_bits, arg2_bits])
            {
                return bits;
            }
            call_function_obj4(func_bits, self_bits, arg0_bits, arg1_bits, arg2_bits)
        }
        TYPE_ID_TYPE => call_class_init_with_args(call_ptr, &[arg0_bits, arg1_bits, arg2_bits]),
        TYPE_ID_OBJECT | TYPE_ID_DATACLASS => {
            let Some(call_attr_bits) = lookup_call_attr(call_ptr) else {
                return raise_not_callable(call_obj);
            };
            call_callable3(call_attr_bits, arg0_bits, arg1_bits, arg2_bits)
        }
        _ => raise_not_callable(call_obj),
    }
}
