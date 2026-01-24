use crate::{
    alloc_object, attr_lookup_ptr_allow_missing, call_callable0, call_callable3, close_payload,
    dec_ref_bits, exception_pending, exception_trace_bits, file_handle_enter, file_handle_exit,
    inc_ref_bits, intern_static_name, obj_from_bits, object_type_id, raise_exception,
    runtime_state, to_i64, type_of_bits, MoltHeader, MoltObject, PyToken, CONTEXT_STACK,
    TYPE_ID_CONTEXT_MANAGER, TYPE_ID_FILE_HANDLE,
};

unsafe fn context_enter_fn(ptr: *mut u8) -> *const () {
    *(ptr as *const *const ())
}

unsafe fn context_exit_fn(ptr: *mut u8) -> *const () {
    *(ptr.add(std::mem::size_of::<*const ()>()) as *const *const ())
}

pub(crate) unsafe fn context_payload_bits(ptr: *mut u8) -> u64 {
    *(ptr.add(2 * std::mem::size_of::<*const ()>()) as *const u64)
}

fn alloc_context_manager(
    _py: &PyToken<'_>,
    enter_fn: *const (),
    exit_fn: *const (),
    payload_bits: u64,
) -> *mut u8 {
    let total = std::mem::size_of::<MoltHeader>()
        + 2 * std::mem::size_of::<*const ()>()
        + std::mem::size_of::<u64>();
    let ptr = alloc_object(_py, total, TYPE_ID_CONTEXT_MANAGER);
    if ptr.is_null() {
        return ptr;
    }
    unsafe {
        *(ptr as *mut *const ()) = enter_fn;
        *(ptr.add(std::mem::size_of::<*const ()>()) as *mut *const ()) = exit_fn;
        *(ptr.add(2 * std::mem::size_of::<*const ()>()) as *mut u64) = payload_bits;
        inc_ref_bits(_py, payload_bits);
    }
    ptr
}

extern "C" fn context_null_enter(payload_bits: u64) -> u64 {
    crate::with_gil_entry!(_py, {
        inc_ref_bits(_py, payload_bits);
        payload_bits
    })
}

extern "C" fn context_null_exit(_payload_bits: u64, _exc_bits: u64) -> u64 {
    crate::with_gil_entry!(_py, { MoltObject::from_bool(false).bits() })
}

extern "C" fn context_closing_enter(payload_bits: u64) -> u64 {
    crate::with_gil_entry!(_py, {
        inc_ref_bits(_py, payload_bits);
        payload_bits
    })
}

extern "C" fn context_closing_exit(payload_bits: u64, _exc_bits: u64) -> u64 {
    crate::with_gil_entry!(_py, {
        close_payload(_py, payload_bits);
        MoltObject::from_bool(false).bits()
    })
}

fn context_stack_push(_py: &PyToken<'_>, ctx_bits: u64) {
    crate::gil_assert();
    CONTEXT_STACK.with(|stack| {
        stack.borrow_mut().push(ctx_bits);
    });
    inc_ref_bits(_py, ctx_bits);
}

fn context_stack_pop(_py: &PyToken<'_>, expected_bits: u64) {
    crate::gil_assert();
    let result = CONTEXT_STACK.with(|stack| {
        let mut stack = stack.borrow_mut();
        let Some(bits) = stack.pop() else {
            return Err("context manager stack underflow");
        };
        if bits != expected_bits {
            return Err("context manager stack mismatch");
        }
        Ok(bits)
    });
    match result {
        Ok(bits) => dec_ref_bits(_py, bits),
        Err(msg) => return raise_exception::<_>(_py, "RuntimeError", msg),
    }
}

unsafe fn context_exit_unchecked(_py: &PyToken<'_>, ctx_bits: u64, exc_bits: u64) {
    crate::gil_assert();
    let ctx_obj = obj_from_bits(ctx_bits);
    let Some(ptr) = ctx_obj.as_ptr() else {
        return;
    };
    let type_id = object_type_id(ptr);
    if type_id == TYPE_ID_CONTEXT_MANAGER {
        let exit_fn_addr = context_exit_fn(ptr);
        if exit_fn_addr.is_null() {
            return;
        }
        let exit_fn =
            std::mem::transmute::<*const (), extern "C" fn(u64, u64) -> u64>(exit_fn_addr);
        exit_fn(context_payload_bits(ptr), exc_bits);
        return;
    }
    if type_id == TYPE_ID_FILE_HANDLE {
        file_handle_exit(_py, ptr, exc_bits);
        return;
    }
    let exit_name_bits =
        intern_static_name(_py, &runtime_state(_py).interned.exit_name, b"__exit__");
    let Some(exit_bits) = attr_lookup_ptr_allow_missing(_py, ptr, exit_name_bits) else {
        return;
    };
    let none_bits = MoltObject::none().bits();
    let exc_obj = obj_from_bits(exc_bits);
    let (exc_type_bits, exc_val_bits, tb_bits) = if exc_obj.is_none() {
        (none_bits, none_bits, none_bits)
    } else {
        let tb_bits = exc_obj
            .as_ptr()
            .map(|ptr| exception_trace_bits(ptr))
            .unwrap_or(none_bits);
        (type_of_bits(_py, exc_bits), exc_bits, tb_bits)
    };
    let _ = call_callable3(_py, exit_bits, exc_type_bits, exc_val_bits, tb_bits);
    dec_ref_bits(_py, exit_bits);
}

fn context_stack_depth() -> usize {
    CONTEXT_STACK.with(|stack| stack.borrow().len())
}

fn context_stack_unwind_to(_py: &PyToken<'_>, depth: usize, exc_bits: u64) {
    crate::gil_assert();
    let contexts = CONTEXT_STACK.with(|stack| {
        let mut stack = stack.borrow_mut();
        if depth > stack.len() {
            return Err("context manager stack underflow");
        }
        let tail = stack.split_off(depth);
        Ok(tail)
    });
    match contexts {
        Ok(contexts) => {
            for bits in contexts.into_iter().rev() {
                unsafe { context_exit_unchecked(_py, bits, exc_bits) };
                dec_ref_bits(_py, bits);
            }
        }
        Err(msg) => return raise_exception::<_>(_py, "RuntimeError", msg),
    }
}

pub(crate) fn context_stack_unwind(_py: &PyToken<'_>, exc_bits: u64) {
    context_stack_unwind_to(_py, 0, exc_bits);
}

#[no_mangle]
pub extern "C" fn molt_context_new(
    enter_fn: *const (),
    exit_fn: *const (),
    payload_bits: u64,
) -> u64 {
    crate::with_gil_entry!(_py, {
        if enter_fn.is_null() || exit_fn.is_null() {
            return raise_exception::<_>(
                _py,
                "TypeError",
                "context manager hooks must be non-null",
            );
        }
        let ptr = alloc_context_manager(_py, enter_fn, exit_fn, payload_bits);
        if ptr.is_null() {
            MoltObject::none().bits()
        } else {
            MoltObject::from_ptr(ptr).bits()
        }
    })
}

#[no_mangle]
pub extern "C" fn molt_context_enter(ctx_bits: u64) -> u64 {
    crate::with_gil_entry!(_py, {
        let ctx_obj = obj_from_bits(ctx_bits);
        let Some(ptr) = ctx_obj.as_ptr() else {
            return raise_exception::<_>(_py, "TypeError", "context manager must be an object");
        };
        unsafe {
            let type_id = object_type_id(ptr);
            match type_id {
                TYPE_ID_CONTEXT_MANAGER => {
                    let enter_fn_addr = context_enter_fn(ptr);
                    if enter_fn_addr.is_null() {
                        return raise_exception::<_>(
                            _py,
                            "TypeError",
                            "context manager missing __enter__",
                        );
                    }
                    let enter_fn =
                        std::mem::transmute::<*const (), extern "C" fn(u64) -> u64>(enter_fn_addr);
                    let res = enter_fn(context_payload_bits(ptr));
                    if exception_pending(_py) {
                        return MoltObject::none().bits();
                    }
                    context_stack_push(_py, ctx_bits);
                    res
                }
                TYPE_ID_FILE_HANDLE => {
                    let res = file_handle_enter(_py, ptr);
                    if exception_pending(_py) {
                        return MoltObject::none().bits();
                    }
                    context_stack_push(_py, ctx_bits);
                    res
                }
                _ => {
                    let enter_name_bits = intern_static_name(
                        _py,
                        &runtime_state(_py).interned.enter_name,
                        b"__enter__",
                    );
                    let Some(enter_bits) = attr_lookup_ptr_allow_missing(_py, ptr, enter_name_bits)
                    else {
                        return raise_exception::<_>(
                            _py,
                            "TypeError",
                            "context manager missing __enter__",
                        );
                    };
                    let res = call_callable0(_py, enter_bits);
                    dec_ref_bits(_py, enter_bits);
                    if exception_pending(_py) {
                        return MoltObject::none().bits();
                    }
                    context_stack_push(_py, ctx_bits);
                    res
                }
            }
        }
    })
}

#[no_mangle]
pub extern "C" fn molt_context_exit(ctx_bits: u64, exc_bits: u64) -> u64 {
    crate::with_gil_entry!(_py, {
        let ctx_obj = obj_from_bits(ctx_bits);
        let Some(ptr) = ctx_obj.as_ptr() else {
            return raise_exception::<_>(_py, "TypeError", "context manager must be an object");
        };
        unsafe {
            let type_id = object_type_id(ptr);
            match type_id {
                TYPE_ID_CONTEXT_MANAGER => {
                    let exit_fn_addr = context_exit_fn(ptr);
                    if exit_fn_addr.is_null() {
                        return raise_exception::<_>(
                            _py,
                            "TypeError",
                            "context manager missing __exit__",
                        );
                    }
                    let exit_fn = std::mem::transmute::<*const (), extern "C" fn(u64, u64) -> u64>(
                        exit_fn_addr,
                    );
                    context_stack_pop(_py, ctx_bits);
                    exit_fn(context_payload_bits(ptr), exc_bits)
                }
                TYPE_ID_FILE_HANDLE => {
                    let res = file_handle_exit(_py, ptr, exc_bits);
                    context_stack_pop(_py, ctx_bits);
                    res
                }
                _ => {
                    let exit_name_bits = intern_static_name(
                        _py,
                        &runtime_state(_py).interned.exit_name,
                        b"__exit__",
                    );
                    let Some(exit_bits) = attr_lookup_ptr_allow_missing(_py, ptr, exit_name_bits)
                    else {
                        context_stack_pop(_py, ctx_bits);
                        return raise_exception::<_>(
                            _py,
                            "TypeError",
                            "context manager missing __exit__",
                        );
                    };
                    let none_bits = MoltObject::none().bits();
                    let exc_obj = obj_from_bits(exc_bits);
                    let (exc_type_bits, exc_val_bits, tb_bits) = if exc_obj.is_none() {
                        (none_bits, none_bits, none_bits)
                    } else {
                        let tb_bits = exc_obj
                            .as_ptr()
                            .map(|ptr| exception_trace_bits(ptr))
                            .unwrap_or(none_bits);
                        (type_of_bits(_py, exc_bits), exc_bits, tb_bits)
                    };
                    let res = call_callable3(_py, exit_bits, exc_type_bits, exc_val_bits, tb_bits);
                    dec_ref_bits(_py, exit_bits);
                    context_stack_pop(_py, ctx_bits);
                    res
                }
            }
        }
    })
}

#[no_mangle]
pub extern "C" fn molt_context_unwind(exc_bits: u64) -> u64 {
    crate::with_gil_entry!(_py, {
        context_stack_unwind(_py, exc_bits);
        MoltObject::none().bits()
    })
}

#[no_mangle]
pub extern "C" fn molt_context_depth() -> u64 {
    crate::with_gil_entry!(_py, {
        MoltObject::from_int(context_stack_depth() as i64).bits()
    })
}

#[no_mangle]
pub extern "C" fn molt_context_unwind_to(depth_bits: u64, exc_bits: u64) -> u64 {
    crate::with_gil_entry!(_py, {
        let depth = match to_i64(obj_from_bits(depth_bits)) {
            Some(val) if val >= 0 => val as usize,
            _ => {
                return raise_exception::<_>(
                    _py,
                    "TypeError",
                    "context depth must be a non-negative int",
                )
            }
        };
        context_stack_unwind_to(_py, depth, exc_bits);
        MoltObject::none().bits()
    })
}

#[no_mangle]
pub extern "C" fn molt_context_null(payload_bits: u64) -> u64 {
    crate::with_gil_entry!(_py, {
        let enter_fn = context_null_enter as *const ();
        let exit_fn = context_null_exit as *const ();
        let ptr = alloc_context_manager(_py, enter_fn, exit_fn, payload_bits);
        if ptr.is_null() {
            MoltObject::none().bits()
        } else {
            MoltObject::from_ptr(ptr).bits()
        }
    })
}

#[no_mangle]
pub extern "C" fn molt_context_closing(payload_bits: u64) -> u64 {
    crate::with_gil_entry!(_py, {
        let enter_fn = context_closing_enter as *const ();
        let exit_fn = context_closing_exit as *const ();
        let ptr = alloc_context_manager(_py, enter_fn, exit_fn, payload_bits);
        if ptr.is_null() {
            MoltObject::none().bits()
        } else {
            MoltObject::from_ptr(ptr).bits()
        }
    })
}
