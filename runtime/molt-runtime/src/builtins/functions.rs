use molt_obj_model::MoltObject;

use crate::{
    alloc_bound_method_obj, alloc_code_obj, alloc_function_obj, dec_ref_bits,
    builtin_classes, function_set_closure_bits, function_set_trampoline_ptr, inc_ref_bits,
    obj_from_bits, object_class_bits, object_set_class_bits, object_type_id, raise_exception,
    to_i64, TYPE_ID_FUNCTION, TYPE_ID_STRING, TYPE_ID_TUPLE,
};

#[no_mangle]
pub extern "C" fn molt_func_new(fn_ptr: u64, trampoline_ptr: u64, arity: u64) -> u64 {
    crate::with_gil_entry!(_py, {
        let ptr = alloc_function_obj(_py, fn_ptr, arity);
        if ptr.is_null() {
            MoltObject::none().bits()
        } else {
            unsafe {
                function_set_trampoline_ptr(ptr, trampoline_ptr);
            }
            MoltObject::from_ptr(ptr).bits()
        }
    })
}

#[no_mangle]
pub extern "C" fn molt_func_new_builtin(fn_ptr: u64, trampoline_ptr: u64, arity: u64) -> u64 {
    crate::with_gil_entry!(_py, {
        let ptr = alloc_function_obj(_py, fn_ptr, arity);
        if ptr.is_null() {
            return MoltObject::none().bits();
        }
        unsafe {
            function_set_trampoline_ptr(ptr, trampoline_ptr);
            let builtin_bits = builtin_classes(_py).builtin_function_or_method;
            object_set_class_bits(_py, ptr, builtin_bits);
            inc_ref_bits(_py, builtin_bits);
        }
        MoltObject::from_ptr(ptr).bits()
    })
}

#[no_mangle]
pub extern "C" fn molt_func_new_closure(
    fn_ptr: u64,
    trampoline_ptr: u64,
    arity: u64,
    closure_bits: u64,
) -> u64 {
    crate::with_gil_entry!(_py, {
        let ptr = alloc_function_obj(_py, fn_ptr, arity);
        if ptr.is_null() {
            return MoltObject::none().bits();
        }
        unsafe {
            function_set_closure_bits(_py, ptr, closure_bits);
            function_set_trampoline_ptr(ptr, trampoline_ptr);
        }
        MoltObject::from_ptr(ptr).bits()
    })
}

#[no_mangle]
pub extern "C" fn molt_function_set_builtin(func_bits: u64) -> u64 {
    crate::with_gil_entry!(_py, {
        let Some(func_ptr) = obj_from_bits(func_bits).as_ptr() else {
            return raise_exception::<_>(_py, "TypeError", "expected function");
        };
        unsafe {
            if object_type_id(func_ptr) != TYPE_ID_FUNCTION {
                return raise_exception::<_>(_py, "TypeError", "expected function");
            }
            let builtin_bits = builtin_classes(_py).builtin_function_or_method;
            let old_bits = object_class_bits(func_ptr);
            if old_bits != builtin_bits {
                if old_bits != 0 {
                    dec_ref_bits(_py, old_bits);
                }
                object_set_class_bits(_py, func_ptr, builtin_bits);
                inc_ref_bits(_py, builtin_bits);
            }
        }
        MoltObject::none().bits()
    })
}

#[no_mangle]
pub extern "C" fn molt_code_new(
    filename_bits: u64,
    name_bits: u64,
    firstlineno_bits: u64,
    linetable_bits: u64,
) -> u64 {
    crate::with_gil_entry!(_py, {
        let filename_obj = obj_from_bits(filename_bits);
        let Some(filename_ptr) = filename_obj.as_ptr() else {
            return raise_exception::<_>(_py, "TypeError", "code filename must be str");
        };
        unsafe {
            if object_type_id(filename_ptr) != TYPE_ID_STRING {
                return raise_exception::<_>(_py, "TypeError", "code filename must be str");
            }
        }
        let name_obj = obj_from_bits(name_bits);
        let Some(name_ptr) = name_obj.as_ptr() else {
            return raise_exception::<_>(_py, "TypeError", "code name must be str");
        };
        unsafe {
            if object_type_id(name_ptr) != TYPE_ID_STRING {
                return raise_exception::<_>(_py, "TypeError", "code name must be str");
            }
        }
        if !obj_from_bits(linetable_bits).is_none() {
            let Some(table_ptr) = obj_from_bits(linetable_bits).as_ptr() else {
                return raise_exception::<_>(
                    _py,
                    "TypeError",
                    "code linetable must be tuple or None",
                );
            };
            unsafe {
                if object_type_id(table_ptr) != TYPE_ID_TUPLE {
                    return raise_exception::<_>(
                        _py,
                        "TypeError",
                        "code linetable must be tuple or None",
                    );
                }
            }
        }
        let firstlineno = to_i64(obj_from_bits(firstlineno_bits)).unwrap_or(0);
        let ptr = alloc_code_obj(_py, filename_bits, name_bits, firstlineno, linetable_bits);
        if ptr.is_null() {
            MoltObject::none().bits()
        } else {
            MoltObject::from_ptr(ptr).bits()
        }
    })
}

#[no_mangle]
pub extern "C" fn molt_bound_method_new(func_bits: u64, self_bits: u64) -> u64 {
    crate::with_gil_entry!(_py, {
        let func_obj = obj_from_bits(func_bits);
        let Some(func_ptr) = func_obj.as_ptr() else {
            return raise_exception::<_>(_py, "TypeError", "bound method expects function object");
        };
        unsafe {
            if object_type_id(func_ptr) != TYPE_ID_FUNCTION {
                return raise_exception::<_>(
                    _py,
                    "TypeError",
                    "bound method expects function object",
                );
            }
        }
        let ptr = alloc_bound_method_obj(_py, func_bits, self_bits);
        if ptr.is_null() {
            MoltObject::none().bits()
        } else {
            MoltObject::from_ptr(ptr).bits()
        }
    })
}

/// # Safety
/// `self_ptr` must point to a valid closure storage region and `offset` must be
/// within the allocated payload.
#[no_mangle]
pub unsafe extern "C" fn molt_closure_load(self_ptr: *mut u8, offset: u64) -> u64 {
    crate::with_gil_entry!(_py, {
        if self_ptr.is_null() {
            return MoltObject::none().bits();
        }
        let slot = self_ptr.add(offset as usize) as *mut u64;
        let bits = *slot;
        inc_ref_bits(_py, bits);
        bits
    })
}

/// # Safety
/// `self_ptr` must point to a valid closure storage region and `offset` must be
/// within the allocated payload.
#[no_mangle]
pub unsafe extern "C" fn molt_closure_store(self_ptr: *mut u8, offset: u64, bits: u64) -> u64 {
    crate::with_gil_entry!(_py, {
        if self_ptr.is_null() {
            return MoltObject::none().bits();
        }
        let slot = self_ptr.add(offset as usize) as *mut u64;
        let old_bits = *slot;
        dec_ref_bits(_py, old_bits);
        inc_ref_bits(_py, bits);
        *slot = bits;
        MoltObject::none().bits()
    })
}
