use crate::object::ops_sys::runtime_target_minor;
use crate::state::runtime_state::{AtexitCallbackEntry, AtexitCallbackKind};
use crate::{
    MoltObject, PyToken, TYPE_ID_BOUND_METHOD, TYPE_ID_EXCEPTION, TYPE_ID_FUNCTION, alloc_string,
    alloc_tuple, attr_name_bits_from_bytes, bound_method_func_bits, bound_method_self_bits,
    clear_exception, clear_exception_state, dec_ref_bits, exception_class_bits, exception_pending,
    exception_trace_bits, format_exception_with_traceback, function_closure_bits, function_fn_ptr,
    inc_ref_bits, int_bits_from_i64, is_truthy, molt_call_bind, molt_callargs_expand_kwstar,
    molt_callargs_expand_star, molt_callargs_new, molt_callargs_push_pos, molt_eq,
    molt_exception_clear, molt_exception_last, molt_get_attr_name, molt_is_callable,
    molt_module_import, molt_sys_stderr, obj_from_bits, object_type_id, raise_exception,
    runtime_state,
};
use std::sync::atomic::Ordering as AtomicOrdering;

fn atexit_callback_release_refs(_py: &PyToken<'_>, callback: AtexitCallbackEntry) {
    if !obj_from_bits(callback.func_bits).is_none() {
        dec_ref_bits(_py, callback.func_bits);
    }
    if !obj_from_bits(callback.args_bits).is_none() {
        dec_ref_bits(_py, callback.args_bits);
    }
    if !obj_from_bits(callback.kwargs_bits).is_none() {
        dec_ref_bits(_py, callback.kwargs_bits);
    }
}

fn py_eq_checked(_py: &PyToken<'_>, lhs_bits: u64, rhs_bits: u64) -> Result<bool, u64> {
    let eq_bits = molt_eq(lhs_bits, rhs_bits);
    if exception_pending(_py) {
        return Err(MoltObject::none().bits());
    }
    Ok(is_truthy(_py, obj_from_bits(eq_bits)))
}

fn callable_identity_eq(lhs_bits: u64, rhs_bits: u64) -> bool {
    if lhs_bits == rhs_bits {
        return true;
    }
    let Some(lhs_ptr) = obj_from_bits(lhs_bits).as_ptr() else {
        return false;
    };
    let Some(rhs_ptr) = obj_from_bits(rhs_bits).as_ptr() else {
        return false;
    };
    let lhs_type = unsafe { object_type_id(lhs_ptr) };
    let rhs_type = unsafe { object_type_id(rhs_ptr) };
    if lhs_type != rhs_type {
        return false;
    }
    match lhs_type {
        TYPE_ID_FUNCTION => unsafe {
            function_fn_ptr(lhs_ptr) == function_fn_ptr(rhs_ptr)
                && function_closure_bits(lhs_ptr) == function_closure_bits(rhs_ptr)
        },
        TYPE_ID_BOUND_METHOD => unsafe {
            let lhs_func = bound_method_func_bits(lhs_ptr);
            let rhs_func = bound_method_func_bits(rhs_ptr);
            let lhs_self = bound_method_self_bits(lhs_ptr);
            let rhs_self = bound_method_self_bits(rhs_ptr);
            lhs_self == rhs_self && callable_identity_eq(lhs_func, rhs_func)
        },
        _ => false,
    }
}

fn atexit_clear_pending_exception_state(_py: &PyToken<'_>) {
    for _ in 0..4 {
        if exception_pending(_py) {
            let _ = molt_exception_clear();
        }
        clear_exception_state(_py);
        if !exception_pending(_py) {
            break;
        }
    }
}

fn callback_repr(_py: &PyToken<'_>, callback_bits: u64) -> String {
    if obj_from_bits(callback_bits).is_none() {
        return "<callback>".to_string();
    }
    let repr_bits = crate::molt_repr_from_obj(callback_bits);
    if exception_pending(_py) {
        atexit_clear_pending_exception_state(_py);
        return "<callback>".to_string();
    }
    let rendered = crate::object::ops::string_obj_to_owned(obj_from_bits(repr_bits))
        .unwrap_or_else(|| "<callback>".to_string());
    if !obj_from_bits(repr_bits).is_none() {
        dec_ref_bits(_py, repr_bits);
    }
    rendered
}

fn call_with_positional_args(_py: &PyToken<'_>, callable_bits: u64, args: &[u64]) -> u64 {
    let builder_bits = molt_callargs_new(args.len() as u64, 0);
    if builder_bits == 0 {
        return MoltObject::none().bits();
    }
    for &arg_bits in args {
        // Safety: builder_bits is created by `molt_callargs_new` above and remains valid here.
        let _ = unsafe { molt_callargs_push_pos(builder_bits, arg_bits) };
        if exception_pending(_py) {
            return MoltObject::none().bits();
        }
    }
    molt_call_bind(callable_bits, builder_bits)
}

fn sys_attr_bits(_py: &PyToken<'_>, name: &[u8]) -> u64 {
    let sys_name_ptr = alloc_string(_py, b"sys");
    if sys_name_ptr.is_null() {
        return MoltObject::none().bits();
    }
    let sys_name_bits = MoltObject::from_ptr(sys_name_ptr).bits();
    let sys_bits = molt_module_import(sys_name_bits);
    dec_ref_bits(_py, sys_name_bits);
    if exception_pending(_py) || obj_from_bits(sys_bits).is_none() {
        atexit_clear_pending_exception_state(_py);
        if !obj_from_bits(sys_bits).is_none() {
            dec_ref_bits(_py, sys_bits);
        }
        return MoltObject::none().bits();
    }
    let Some(name_bits) = attr_name_bits_from_bytes(_py, name) else {
        dec_ref_bits(_py, sys_bits);
        return MoltObject::none().bits();
    };
    let value_bits = molt_get_attr_name(sys_bits, name_bits);
    dec_ref_bits(_py, name_bits);
    dec_ref_bits(_py, sys_bits);
    if exception_pending(_py) || obj_from_bits(value_bits).is_none() {
        atexit_clear_pending_exception_state(_py);
        if !obj_from_bits(value_bits).is_none() {
            dec_ref_bits(_py, value_bits);
        }
        return MoltObject::none().bits();
    }
    value_bits
}

fn atexit_unraisable_message_and_object(
    _py: &PyToken<'_>,
    callback_bits: u64,
    callback_text: &str,
) -> (String, u64) {
    if runtime_target_minor(_py) >= 13 {
        (
            format!("Exception ignored in atexit callback {callback_text}"),
            MoltObject::none().bits(),
        )
    } else {
        (
            "Exception ignored in atexit callback".to_string(),
            callback_bits,
        )
    }
}

fn atexit_build_unraisablehook_args(_py: &PyToken<'_>, callback_bits: u64, exc_bits: u64) -> u64 {
    let hook_args_class_bits = sys_attr_bits(_py, b"UnraisableHookArgs");
    if obj_from_bits(hook_args_class_bits).is_none() {
        return MoltObject::none().bits();
    }
    let class_callable_bits = molt_is_callable(hook_args_class_bits);
    let class_is_callable = is_truthy(_py, obj_from_bits(class_callable_bits));
    if !class_is_callable {
        dec_ref_bits(_py, hook_args_class_bits);
        return MoltObject::none().bits();
    }

    let callback_text = callback_repr(_py, callback_bits);
    let (err_msg, object_bits) =
        atexit_unraisable_message_and_object(_py, callback_bits, &callback_text);
    let msg_ptr = alloc_string(_py, err_msg.as_bytes());
    if msg_ptr.is_null() {
        dec_ref_bits(_py, hook_args_class_bits);
        return MoltObject::none().bits();
    }
    let msg_bits = MoltObject::from_ptr(msg_ptr).bits();

    let mut exc_type_bits = MoltObject::none().bits();
    let mut trace_bits = MoltObject::none().bits();
    if let Some(exc_ptr) = obj_from_bits(exc_bits).as_ptr()
        && unsafe { object_type_id(exc_ptr) } == TYPE_ID_EXCEPTION
    {
        let class_bits = unsafe { exception_class_bits(exc_ptr) };
        if !obj_from_bits(class_bits).is_none() {
            exc_type_bits = class_bits;
        }
        let tb_bits = unsafe { exception_trace_bits(exc_ptr) };
        if !obj_from_bits(tb_bits).is_none() {
            trace_bits = tb_bits;
        }
    }

    let out_bits = call_with_positional_args(
        _py,
        hook_args_class_bits,
        &[exc_type_bits, exc_bits, trace_bits, msg_bits, object_bits],
    );
    dec_ref_bits(_py, hook_args_class_bits);
    dec_ref_bits(_py, msg_bits);
    if exception_pending(_py) {
        atexit_clear_pending_exception_state(_py);
        if !obj_from_bits(out_bits).is_none() {
            dec_ref_bits(_py, out_bits);
        }
        return MoltObject::none().bits();
    }
    out_bits
}

fn atexit_try_unraisablehook(_py: &PyToken<'_>, callback_bits: u64, exc_bits: u64) -> bool {
    let hook_bits = sys_attr_bits(_py, b"unraisablehook");
    if obj_from_bits(hook_bits).is_none() {
        return false;
    }
    let hook_callable_bits = molt_is_callable(hook_bits);
    let hook_is_callable = is_truthy(_py, obj_from_bits(hook_callable_bits));
    if !hook_is_callable {
        dec_ref_bits(_py, hook_bits);
        return false;
    }
    let hook_args_bits = atexit_build_unraisablehook_args(_py, callback_bits, exc_bits);
    if obj_from_bits(hook_args_bits).is_none() {
        dec_ref_bits(_py, hook_bits);
        return false;
    }

    let out_bits = call_with_positional_args(_py, hook_bits, &[hook_args_bits]);
    let hook_ok = !exception_pending(_py);
    if exception_pending(_py) {
        atexit_clear_pending_exception_state(_py);
    }
    if !obj_from_bits(out_bits).is_none() {
        dec_ref_bits(_py, out_bits);
    }
    dec_ref_bits(_py, hook_args_bits);
    dec_ref_bits(_py, hook_bits);
    hook_ok
}

fn atexit_report_callback_exception(_py: &PyToken<'_>, callback_bits: u64, exc_bits: u64) {
    if atexit_try_unraisablehook(_py, callback_bits, exc_bits) {
        return;
    }
    let callback_text = callback_repr(_py, callback_bits);
    let formatted = if let Some(exc_ptr) = obj_from_bits(exc_bits).as_ptr() {
        format_exception_with_traceback(_py, exc_ptr)
    } else {
        String::new()
    };
    let (err_msg, object_bits) =
        atexit_unraisable_message_and_object(_py, callback_bits, &callback_text);
    let prefix = if obj_from_bits(object_bits).is_none() {
        format!("{err_msg}:")
    } else {
        format!("{err_msg}: {callback_text}")
    };
    write_stderr_line(_py, &prefix);
    if !formatted.is_empty() {
        write_stderr_line(_py, &formatted);
    }
}

fn callback_returned_raised_exception(_py: &PyToken<'_>, out_bits: u64) -> bool {
    let Some(out_ptr) = obj_from_bits(out_bits).as_ptr() else {
        return false;
    };
    if unsafe { object_type_id(out_ptr) } != TYPE_ID_EXCEPTION {
        return false;
    }
    let Some(tb_name_bits) = attr_name_bits_from_bytes(_py, b"__traceback__") else {
        return true;
    };
    let tb_bits = molt_get_attr_name(out_bits, tb_name_bits);
    dec_ref_bits(_py, tb_name_bits);
    if exception_pending(_py) {
        atexit_clear_pending_exception_state(_py);
        return true;
    }
    let has_tb = !obj_from_bits(tb_bits).is_none();
    if !obj_from_bits(tb_bits).is_none() {
        dec_ref_bits(_py, tb_bits);
    }
    has_tb
}

fn write_stderr_line(_py: &PyToken<'_>, text: &str) {
    let text_ptr = alloc_string(_py, text.as_bytes());
    if text_ptr.is_null() {
        return;
    }
    let text_bits = MoltObject::from_ptr(text_ptr).bits();
    let args_ptr = alloc_tuple(_py, &[text_bits]);
    if args_ptr.is_null() {
        dec_ref_bits(_py, text_bits);
        return;
    }
    let args_bits = MoltObject::from_ptr(args_ptr).bits();
    let stderr_bits = current_stderr_target(_py);
    if obj_from_bits(stderr_bits).is_none() {
        dec_ref_bits(_py, args_bits);
        dec_ref_bits(_py, text_bits);
        return;
    }
    let none = MoltObject::none().bits();
    let flush = MoltObject::from_bool(true).bits();
    let out_bits = crate::molt_print_builtin(args_bits, none, none, stderr_bits, flush);
    if !obj_from_bits(out_bits).is_none() {
        dec_ref_bits(_py, out_bits);
    }
    if exception_pending(_py) {
        atexit_clear_pending_exception_state(_py);
    }
    dec_ref_bits(_py, stderr_bits);
    dec_ref_bits(_py, args_bits);
    dec_ref_bits(_py, text_bits);
}

fn current_stderr_target(_py: &PyToken<'_>) -> u64 {
    let stderr_bits = sys_attr_bits(_py, b"stderr");
    if obj_from_bits(stderr_bits).is_none() {
        return molt_sys_stderr();
    }
    stderr_bits
}

fn atexit_call_callback(_py: &PyToken<'_>, callback: &AtexitCallbackEntry) -> u64 {
    let builder_bits = molt_callargs_new(0, 0);
    if builder_bits == 0 {
        return MoltObject::none().bits();
    }
    if !obj_from_bits(callback.args_bits).is_none() {
        let _ = unsafe { molt_callargs_expand_star(builder_bits, callback.args_bits) };
        if exception_pending(_py) {
            return MoltObject::none().bits();
        }
    }
    if !obj_from_bits(callback.kwargs_bits).is_none() {
        let _ = unsafe { molt_callargs_expand_kwstar(builder_bits, callback.kwargs_bits) };
        if exception_pending(_py) {
            return MoltObject::none().bits();
        }
    }
    molt_call_bind(callback.func_bits, builder_bits)
}

fn atexit_register_impl(
    _py: &PyToken<'_>,
    func_bits: u64,
    args_bits: u64,
    kwargs_bits: u64,
) -> u64 {
    let callable_bits = molt_is_callable(func_bits);
    if !is_truthy(_py, obj_from_bits(callable_bits)) {
        return raise_exception::<u64>(_py, "TypeError", "the first argument must be callable");
    }

    if !obj_from_bits(func_bits).is_none() {
        inc_ref_bits(_py, func_bits);
    }
    if !obj_from_bits(args_bits).is_none() {
        inc_ref_bits(_py, args_bits);
    }
    if !obj_from_bits(kwargs_bits).is_none() {
        inc_ref_bits(_py, kwargs_bits);
    }

    runtime_state(_py)
        .atexit_callbacks
        .lock()
        .unwrap()
        .push(AtexitCallbackEntry {
            kind: AtexitCallbackKind::Python,
            func_bits,
            args_bits,
            kwargs_bits,
        });
    func_bits
}

pub(crate) fn atexit_register_weakref_runner_once(_py: &PyToken<'_>) {
    let state = runtime_state(_py);
    if state
        .atexit_weakref_runner_registered
        .swap(true, AtomicOrdering::AcqRel)
    {
        return;
    }
    state
        .atexit_callbacks
        .lock()
        .unwrap()
        .push(AtexitCallbackEntry {
            kind: AtexitCallbackKind::WeakrefFinalizerRunner,
            func_bits: MoltObject::none().bits(),
            args_bits: MoltObject::none().bits(),
            kwargs_bits: MoltObject::none().bits(),
        });
}

fn atexit_register_weakref_runner_if_pending(_py: &PyToken<'_>) {
    let pending = {
        let guard = runtime_state(_py).weakref_finalizers.lock().unwrap();
        !guard.is_empty()
    };
    if pending {
        atexit_register_weakref_runner_once(_py);
    }
}

fn atexit_unregister_impl(_py: &PyToken<'_>, func_bits: u64) -> u64 {
    atexit_clear_pending_exception_state(_py);
    let removed = {
        let mut guard = runtime_state(_py).atexit_callbacks.lock().unwrap();
        let mut removed = Vec::new();
        for callback in guard.iter_mut() {
            if callback.kind != AtexitCallbackKind::Python {
                continue;
            }
            if obj_from_bits(callback.func_bits).is_none() {
                continue;
            }
            match py_eq_checked(_py, func_bits, callback.func_bits) {
                Ok(true) => {
                    removed.push(callback.clone());
                    callback.func_bits = MoltObject::none().bits();
                    callback.args_bits = MoltObject::none().bits();
                    callback.kwargs_bits = MoltObject::none().bits();
                }
                Ok(false) => {
                    if callable_identity_eq(func_bits, callback.func_bits) {
                        removed.push(callback.clone());
                        callback.func_bits = MoltObject::none().bits();
                        callback.args_bits = MoltObject::none().bits();
                        callback.kwargs_bits = MoltObject::none().bits();
                    }
                }
                Err(_) => break,
            }
        }
        removed
    };
    for callback in removed {
        atexit_callback_release_refs(_py, callback);
    }
    if exception_pending(_py) {
        return MoltObject::none().bits();
    }
    MoltObject::none().bits()
}

fn atexit_clear_impl(_py: &PyToken<'_>) -> u64 {
    let state = runtime_state(_py);
    let callbacks = {
        let mut guard = state.atexit_callbacks.lock().unwrap();
        std::mem::take(&mut *guard)
    };
    state
        .atexit_weakref_runner_registered
        .store(false, AtomicOrdering::Release);
    for callback in callbacks {
        atexit_callback_release_refs(_py, callback);
    }
    // Preserve CPython ordering when finalizers were already tracked before `_clear()`.
    atexit_register_weakref_runner_if_pending(_py);
    MoltObject::none().bits()
}

fn atexit_run_exitfuncs_impl(_py: &PyToken<'_>) -> u64 {
    loop {
        let callback = {
            let mut guard = runtime_state(_py).atexit_callbacks.lock().unwrap();
            guard.pop()
        };
        let Some(callback) = callback else {
            break;
        };
        if callback.kind == AtexitCallbackKind::WeakrefFinalizerRunner {
            runtime_state(_py)
                .atexit_weakref_runner_registered
                .store(false, AtomicOrdering::Release);
            crate::object::weakref::weakref_run_atexit_finalizers(_py);
            if exception_pending(_py) {
                let exc_bits = molt_exception_last();
                atexit_clear_pending_exception_state(_py);
                atexit_report_callback_exception(_py, MoltObject::none().bits(), exc_bits);
                if !obj_from_bits(exc_bits).is_none() {
                    dec_ref_bits(_py, exc_bits);
                }
            }
            continue;
        }
        if obj_from_bits(callback.func_bits).is_none() {
            atexit_callback_release_refs(_py, callback);
            continue;
        }
        atexit_clear_pending_exception_state(_py);
        let out_bits = atexit_call_callback(_py, &callback);
        let pending = exception_pending(_py);
        let mut exc_bits = MoltObject::none().bits();
        if pending {
            exc_bits = molt_exception_last();
        }
        if obj_from_bits(exc_bits).is_none() && callback_returned_raised_exception(_py, out_bits) {
            inc_ref_bits(_py, out_bits);
            exc_bits = out_bits;
        }
        let callback_raised = pending || !obj_from_bits(exc_bits).is_none();
        if callback_raised {
            atexit_clear_pending_exception_state(_py);
            atexit_report_callback_exception(_py, callback.func_bits, exc_bits);
        }
        if !obj_from_bits(exc_bits).is_none() {
            dec_ref_bits(_py, exc_bits);
        }
        if !obj_from_bits(out_bits).is_none() {
            dec_ref_bits(_py, out_bits);
        }
        atexit_callback_release_refs(_py, callback);
    }
    MoltObject::none().bits()
}

fn atexit_ncallbacks_impl(_py: &PyToken<'_>) -> u64 {
    let count = runtime_state(_py).atexit_callbacks.lock().unwrap().len();
    let count = i64::try_from(count).unwrap_or(i64::MAX);
    int_bits_from_i64(_py, count)
}

pub(crate) fn atexit_run_exitfuncs_teardown(_py: &PyToken<'_>) {
    let _ = atexit_run_exitfuncs_impl(_py);
    if exception_pending(_py) {
        atexit_clear_pending_exception_state(_py);
    }
    if exception_pending(_py) {
        clear_exception(_py);
    }
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_atexit_register(func_bits: u64, args_bits: u64, kwargs_bits: u64) -> u64 {
    crate::with_gil_entry_nopanic!(_py, {
        atexit_register_impl(_py, func_bits, args_bits, kwargs_bits)
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_atexit_unregister(func_bits: u64) -> u64 {
    crate::with_gil_entry_nopanic!(_py, { atexit_unregister_impl(_py, func_bits) })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_atexit_clear() -> u64 {
    crate::with_gil_entry_nopanic!(_py, { atexit_clear_impl(_py) })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_atexit_run_exitfuncs() -> u64 {
    crate::with_gil_entry_nopanic!(_py, { atexit_run_exitfuncs_impl(_py) })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_atexit_ncallbacks() -> u64 {
    crate::with_gil_entry_nopanic!(_py, { atexit_ncallbacks_impl(_py) })
}
