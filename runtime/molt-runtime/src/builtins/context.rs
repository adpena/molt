use crate::BuiltinExc;
use crate::builtins::attr::lookup_special_method_bits;
use crate::builtins::exceptions::{
    ExceptionValue, exception_class, exception_is_instance, exception_traceback,
};
use crate::runtime_flags::{self, RuntimeFlag};
use crate::{
    CONTEXT_STACK, MoltHeader, MoltObject, PyToken, TYPE_ID_CONTEXT_MANAGER, alloc_object,
    call_callable0, call_callable3, dec_ref_bits, exception_pending, inc_ref_bits,
    intern_static_name, obj_from_bits, object_type_id, raise_exception, runtime_state, to_i64,
};

fn context_debug_enabled() -> bool {
    runtime_flags::enabled(RuntimeFlag::MOLT_DEBUG_CONTEXT_STACK)
}

fn context_debug_log(msg: &str) {
    if context_debug_enabled() {
        eprintln!("[molt-context] {msg}");
    }
}

fn context_stack_len() -> usize {
    CONTEXT_STACK.with(|stack| stack.borrow().len())
}

/// Install `values` as the thread's context-manager stack; return the one it held.
pub(crate) fn context_stack_replace(values: Vec<u64>) -> Vec<u64> {
    CONTEXT_STACK.with(|stack| stack.replace(values))
}

unsafe fn context_enter_fn(ptr: *mut u8) -> *const () {
    unsafe { *(ptr as *const *const ()) }
}

unsafe fn context_exit_fn(ptr: *mut u8) -> *const () {
    unsafe { *(ptr.add(std::mem::size_of::<*const ()>()) as *const *const ()) }
}

pub(crate) unsafe fn context_payload_bits(ptr: *mut u8) -> u64 {
    unsafe { *(ptr.add(2 * std::mem::size_of::<*const ()>()) as *const u64) }
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
    crate::with_gil_entry_nopanic!(_py, {
        inc_ref_bits(_py, payload_bits);
        payload_bits
    })
}

extern "C" fn context_null_exit(_payload_bits: u64, _exc_bits: u64) -> u64 {
    crate::with_gil_entry_nopanic!(_py, { MoltObject::from_bool(false).bits() })
}

fn context_stack_push(_py: &PyToken<'_>, ctx_bits: u64) {
    crate::gil_assert();
    CONTEXT_STACK.with(|stack| {
        stack.borrow_mut().push(ctx_bits);
    });
    inc_ref_bits(_py, ctx_bits);
    if context_debug_enabled() {
        context_debug_log(&format!(
            "push depth={} ctx_bits={}",
            context_stack_len(),
            ctx_bits
        ));
    }
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
        Ok(bits) => {
            dec_ref_bits(_py, bits);
            if context_debug_enabled() {
                context_debug_log(&format!(
                    "pop depth={} ctx_bits={}",
                    context_stack_len(),
                    expected_bits
                ));
            }
        }
        Err(msg) => raise_exception::<_>(_py, BuiltinExc::RuntimeError, msg),
    }
}

/// Keep the complete exception triple alive if __exit__ changes its fields or
/// drops the exception owner while the callback is running.
unsafe fn call_context_exit(py: &PyToken<'_>, method: u64, exception: u64) -> u64 {
    let value = ExceptionValue::pin(py, exception);
    let (class, traceback) = if obj_from_bits(exception).is_none() {
        (
            ExceptionValue::pin(py, exception),
            ExceptionValue::pin(py, exception),
        )
    } else {
        if !exception_is_instance(py, exception) {
            return raise_exception(
                py,
                BuiltinExc::TypeError,
                "value must be an exception instance",
            );
        }
        let Some(class) = exception_class(py, exception) else {
            return MoltObject::none().bits();
        };
        if exception_pending(py) {
            return MoltObject::none().bits();
        }
        let Some(traceback) = exception_traceback(py, exception) else {
            return MoltObject::none().bits();
        };
        if exception_pending(py) {
            return MoltObject::none().bits();
        }
        (class, traceback)
    };
    unsafe { call_callable3(py, method, class.bits(), value.bits(), traceback.bits()) }
}

unsafe fn context_exit_unchecked(_py: &PyToken<'_>, ctx_bits: u64, exc_bits: u64) {
    unsafe {
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
            // SAFETY: exit_fn_addr was stored by alloc_context_manager from a
            // function pointer provided by the caller of molt_context_new. That
            // caller (compiled Python `with` statement lowering) guarantees the
            // pointer is a valid `extern "C" fn(u64, u64) -> u64`. The null check
            // above rules out zero. A dangling pointer here causes a segfault.
            let exit_fn =
                std::mem::transmute::<*const (), extern "C" fn(u64, u64) -> u64>(exit_fn_addr);
            exit_fn(context_payload_bits(ptr), exc_bits);
            return;
        }
        let exit_name_bits =
            intern_static_name(_py, &runtime_state(_py).interned.exit_name, b"__exit__");
        let Ok(Some(exit_bits)) =
            lookup_special_method_bits(_py, MoltObject::from_ptr(ptr).bits(), exit_name_bits)
        else {
            return;
        };
        crate::call::discard_owned_call_result(_py, call_context_exit(_py, exit_bits, exc_bits));
        dec_ref_bits(_py, exit_bits);
    }
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
            if context_debug_enabled() {
                context_debug_log(&format!(
                    "unwind depth={} remaining={}",
                    depth,
                    context_stack_len()
                ));
            }
            for bits in contexts.into_iter().rev() {
                unsafe { context_exit_unchecked(_py, bits, exc_bits) };
                dec_ref_bits(_py, bits);
            }
        }
        Err(msg) => raise_exception::<_>(_py, BuiltinExc::RuntimeError, msg),
    }
}

pub(crate) fn context_stack_unwind(_py: &PyToken<'_>, exc_bits: u64) {
    context_stack_unwind_to(_py, 0, exc_bits);
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_context_new(
    enter_fn: *const (),
    exit_fn: *const (),
    payload_bits: u64,
) -> u64 {
    crate::with_gil_entry_nopanic!(_py, {
        if enter_fn.is_null() || exit_fn.is_null() {
            return raise_exception::<_>(
                _py,
                BuiltinExc::TypeError,
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

#[unsafe(no_mangle)]
pub extern "C" fn molt_context_enter(ctx_bits: u64) -> u64 {
    crate::with_gil_entry_nopanic!(_py, {
        let ctx_obj = obj_from_bits(ctx_bits);
        let Some(ptr) = ctx_obj.as_ptr() else {
            return raise_exception::<_>(
                _py,
                BuiltinExc::TypeError,
                "context manager must be an object",
            );
        };
        unsafe {
            let type_id = object_type_id(ptr);
            match type_id {
                TYPE_ID_CONTEXT_MANAGER => {
                    let enter_fn_addr = context_enter_fn(ptr);
                    if enter_fn_addr.is_null() {
                        return raise_exception::<_>(
                            _py,
                            BuiltinExc::TypeError,
                            "context manager missing __enter__",
                        );
                    }
                    // SAFETY: enter_fn_addr was stored by alloc_context_manager from
                    // a function pointer provided by molt_context_new. The caller
                    // guarantees it is a valid `extern "C" fn(u64) -> u64`. The null
                    // check above rules out zero. Dangling pointer causes a segfault.
                    let enter_fn =
                        std::mem::transmute::<*const (), extern "C" fn(u64) -> u64>(enter_fn_addr);
                    let res = enter_fn(context_payload_bits(ptr));
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
                    let enter_method = lookup_special_method_bits(
                        _py,
                        MoltObject::from_ptr(ptr).bits(),
                        enter_name_bits,
                    );
                    let enter_method = match enter_method {
                        Ok(method) => method,
                        Err(()) => return MoltObject::none().bits(),
                    };
                    let Some(enter_bits) = enter_method else {
                        return raise_exception::<_>(
                            _py,
                            BuiltinExc::TypeError,
                            "context manager missing __enter__",
                        );
                    };
                    let res = call_callable0(_py, enter_bits);
                    let failed = exception_pending(_py);
                    dec_ref_bits(_py, enter_bits);
                    if failed {
                        dec_ref_bits(_py, res);
                        return MoltObject::none().bits();
                    }
                    context_stack_push(_py, ctx_bits);
                    res
                }
            }
        }
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_context_exit(ctx_bits: u64, exc_bits: u64) -> u64 {
    crate::with_gil_entry_nopanic!(_py, {
        let ctx_obj = obj_from_bits(ctx_bits);
        let Some(ptr) = ctx_obj.as_ptr() else {
            return raise_exception::<_>(
                _py,
                BuiltinExc::TypeError,
                "context manager must be an object",
            );
        };
        unsafe {
            let type_id = object_type_id(ptr);
            match type_id {
                TYPE_ID_CONTEXT_MANAGER => {
                    let exit_fn_addr = context_exit_fn(ptr);
                    if exit_fn_addr.is_null() {
                        return raise_exception::<_>(
                            _py,
                            BuiltinExc::TypeError,
                            "context manager missing __exit__",
                        );
                    }
                    // SAFETY: exit_fn_addr was stored by alloc_context_manager
                    // from a function pointer provided by molt_context_new. The
                    // caller guarantees it is a valid `extern "C" fn(u64, u64) -> u64`.
                    // The null check above rules out zero. Dangling pointer causes
                    // a segfault.
                    let exit_fn = std::mem::transmute::<*const (), extern "C" fn(u64, u64) -> u64>(
                        exit_fn_addr,
                    );
                    context_stack_pop(_py, ctx_bits);
                    exit_fn(context_payload_bits(ptr), exc_bits)
                }
                _ => {
                    let exit_name_bits = intern_static_name(
                        _py,
                        &runtime_state(_py).interned.exit_name,
                        b"__exit__",
                    );
                    let exit_method = lookup_special_method_bits(
                        _py,
                        MoltObject::from_ptr(ptr).bits(),
                        exit_name_bits,
                    );
                    context_stack_pop(_py, ctx_bits);
                    let exit_method = match exit_method {
                        Ok(method) => method,
                        Err(()) => return MoltObject::none().bits(),
                    };
                    let Some(exit_bits) = exit_method else {
                        return raise_exception::<_>(
                            _py,
                            BuiltinExc::TypeError,
                            "context manager missing __exit__",
                        );
                    };
                    let res = call_context_exit(_py, exit_bits, exc_bits);
                    dec_ref_bits(_py, exit_bits);
                    res
                }
            }
        }
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_context_unwind(exc_bits: u64) -> u64 {
    crate::with_gil_entry_nopanic!(_py, {
        context_stack_unwind(_py, exc_bits);
        MoltObject::none().bits()
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_context_depth() -> u64 {
    crate::with_gil_entry_nopanic!(_py, {
        MoltObject::from_int(context_stack_depth() as i64).bits()
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_context_unwind_to(depth_bits: u64, exc_bits: u64) -> u64 {
    crate::with_gil_entry_nopanic!(_py, {
        let depth = match to_i64(obj_from_bits(depth_bits)) {
            Some(val) if val >= 0 => val as usize,
            _ => {
                return raise_exception::<_>(
                    _py,
                    BuiltinExc::TypeError,
                    "context depth must be a non-negative int",
                );
            }
        };
        context_stack_unwind_to(_py, depth, exc_bits);
        MoltObject::none().bits()
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_context_null(payload_bits: u64) -> u64 {
    crate::with_gil_entry_nopanic!(_py, {
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

#[unsafe(no_mangle)]
pub extern "C" fn molt_context_closing(payload_bits: u64) -> u64 {
    // Keep legacy lowering wired to contextlib semantics.
    crate::molt_contextlib_closing(payload_bits)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicUsize, Ordering};

    static EXIT_CALLS: AtomicUsize = AtomicUsize::new(0);

    extern "C" fn enter(_payload: u64) -> u64 {
        MoltObject::none().bits()
    }
    extern "C" fn exit(_payload: u64, _exception: u64) -> u64 {
        EXIT_CALLS.fetch_add(1, Ordering::SeqCst);
        MoltObject::from_bool(false).bits()
    }

    #[test]
    fn source_raise_keeps_contexts_until_lexical_or_terminal_owner_exits() {
        let _transaction = crate::test_support::RuntimeTestTransaction::new();
        crate::with_gil_entry_nopanic!(py, {
            assert_eq!(context_stack_depth(), 0);
            assert!(!crate::exception_handler_active());
            for policy in 0..3 {
                EXIT_CALLS.store(0, Ordering::SeqCst);
                let outer = MoltObject::from_ptr(alloc_context_manager(
                    py,
                    enter as *const (),
                    exit as *const (),
                    MoltObject::none().bits(),
                ))
                .bits();
                let inner = MoltObject::from_ptr(alloc_context_manager(
                    py,
                    enter as *const (),
                    exit as *const (),
                    MoltObject::none().bits(),
                ))
                .bits();
                molt_context_enter(outer);
                molt_context_enter(inner);
                assert!(!exception_pending(py));
                let exception = ExceptionValue::adopt(
                    py,
                    MoltObject::from_ptr(crate::alloc_exception(
                        py,
                        BuiltinExc::ValueError,
                        "nested body failure",
                    ))
                    .bits(),
                );
                crate::builtins::exceptions::molt_raise(exception.bits());
                assert!(exception_pending(py));
                let pending = ExceptionValue::adopt(py, crate::molt_exception_last_pending());
                assert_eq!(
                    pending.bits(),
                    exception.bits(),
                    "raise preserves the actual exception"
                );
                assert_eq!(
                    EXIT_CALLS.load(Ordering::SeqCst),
                    0,
                    "raising does not run exits"
                );
                assert_eq!(
                    context_stack_depth(),
                    2,
                    "caller and callee contexts remain live"
                );
                crate::clear_exception(py);
                match policy {
                    0 => {
                        molt_context_exit(inner, MoltObject::none().bits());
                    }
                    1 => context_stack_unwind_to(py, 1, exception.bits()),
                    _ => context_stack_unwind(py, exception.bits()),
                }
                assert!(!exception_pending(py));
                if policy != 2 {
                    assert_eq!(
                        context_stack_depth(),
                        1,
                        "lexical cleanup cannot unwind its caller"
                    );
                    assert_eq!(EXIT_CALLS.load(Ordering::SeqCst), 1);
                    molt_context_exit(outer, MoltObject::none().bits());
                }
                assert!(!exception_pending(py));
                assert_eq!(context_stack_depth(), 0);
                assert_eq!(
                    EXIT_CALLS.load(Ordering::SeqCst),
                    2,
                    "each owner exits exactly once"
                );
                dec_ref_bits(py, inner);
                dec_ref_bits(py, outer);
            }
        });
    }
}
