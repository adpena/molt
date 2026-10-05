//! Shared throw argument transport and exception normalization.
//!
//! Public variadic calls own a tuple of one to three arguments. Internal
//! cancellation/close paths already own a canonical exception instance. The
//! tuple is kept intact during delegation and normalized only at its receiver.

use crate::builtins::exceptions::{
    ExceptionFieldSlot, ExceptionValue, exception_class, exception_class_is_subtype,
    exception_field, exception_is_class, exception_is_instance, exception_matches_type,
};
use crate::*;

struct ThrowArguments {
    values: [u64; 3],
    len: usize,
}

impl ThrowArguments {
    fn as_slice(&self) -> &[u64] {
        &self.values[..self.len]
    }
}

impl std::ops::Deref for ThrowArguments {
    type Target = [u64];
    fn deref(&self) -> &[u64] {
        self.as_slice()
    }
}

fn arguments(py: &PyToken<'_>, carrier: u64) -> Option<ThrowArguments> {
    if let Some(ptr) = maybe_ptr_from_bits(carrier)
        && unsafe { object_type_id(ptr) == TYPE_ID_TUPLE }
    {
        let copied = unsafe {
            crate::object::seq_access::with_immutable_tuple_slice(ptr, |args| {
                if !(1..=3).contains(&args.len()) {
                    return None;
                }
                let mut values = [MoltObject::none().bits(); 3];
                values[..args.len()].copy_from_slice(args);
                Some(ThrowArguments {
                    values,
                    len: args.len(),
                })
            })
        }
        .flatten();
        if copied.is_none() {
            return raise_exception::<_>(py, "SystemError", "invalid throw argument carrier");
        }
        copied
    } else {
        Some(ThrowArguments {
            values: [
                carrier,
                MoltObject::none().bits(),
                MoltObject::none().bits(),
            ],
            len: 1,
        })
    }
}

/// Return a borrowed receiver and owned argument carrier; binder inputs remain
/// alive until the entry returns, including through a warnings callback.
pub(crate) fn parse_throw_call(py: &PyToken<'_>, args: u64, kwargs: u64) -> Option<(u64, u64)> {
    let args = crate::builtins::types::call_vararg_args(py, "throw", args)?;
    let (_, keywords) = crate::builtins::types::call_vararg_kwargs(py, "throw", kwargs)?;
    let Some((&receiver, args)) = args.split_first() else {
        return raise_exception::<_>(py, "TypeError", "descriptor 'throw' needs an argument");
    };
    if !keywords.is_empty() {
        return raise_exception::<_>(py, "TypeError", "throw() takes no keyword arguments");
    }
    if !(1..=3).contains(&args.len()) {
        return raise_exception::<_>(
            py,
            "TypeError",
            &format!("throw expected 1 to 3 arguments, got {}", args.len()),
        );
    }
    if args.len() > 1
        && !crate::builtins::warnings_ext::emit_deprecation_warning(
            py,
            "the (type, exc, tb) signature of throw() is deprecated, use the single-arg signature instead.",
        )
    {
        return None;
    }
    let ptr = crate::alloc_tuple(py, args);
    if ptr.is_null() {
        None
    } else {
        Some((receiver, MoltObject::from_ptr(ptr).bits()))
    }
}

/// Shape the value exactly as _PyErr_CreateException does and invoke the
/// ordinary class-call authority, including metaclass hooks. Result is owned.
fn construct_throw_exception(py: &PyToken<'_>, class: u64, value: u64) -> Option<u64> {
    let instance = if obj_from_bits(value).is_none() {
        unsafe { crate::call_callable0(py, class) }
    } else if maybe_ptr_from_bits(value)
        .is_some_and(|ptr| unsafe { object_type_id(ptr) == TYPE_ID_TUPLE })
    {
        let parameters = unsafe {
            crate::object::seq_access::snapshot(
                py,
                ptr_from_bits(value),
                "throw value allocation failed",
            )
        }?;
        crate::builtins::types::call_with_kwargs(py, class, &parameters, MoltObject::none().bits())
    } else {
        unsafe { crate::call_callable1(py, class, value) }
    };
    let instance = ExceptionValue::adopt(py, instance);
    if exception_pending(py) {
        return None;
    }
    if !exception_is_instance(py, instance.bits()) {
        return raise_exception::<_>(
            py,
            "TypeError",
            "calling exception class did not return an exception instance",
        );
    }
    Some(instance.into_bits())
}

/// A valid exception-class argument admits the throw. Constructor or subclass
/// hook failure becomes its injected exception, rather than rejecting the
/// throw before resumption. Normalization retains the old traceback only when
/// the new failure has none; restore-stage failure has no fallback traceback.
fn take_throw_normalization_failure(
    py: &PyToken<'_>,
    fallback_traceback: Option<u64>,
) -> Option<u64> {
    if !exception_pending(py) {
        return None;
    }
    let exception = ExceptionValue::adopt(py, molt_exception_last());
    clear_exception(py);
    if let Some(traceback) = fallback_traceback {
        let current = exception_field(py, exception.bits(), ExceptionFieldSlot::Traceback)?;
        if obj_from_bits(current.bits()).is_none()
            && let Err(message) = crate::builtins::exceptions::exception_replace_field_bits(
                py,
                exception.bits(),
                ExceptionFieldSlot::Traceback,
                traceback,
            )
        {
            return if exception_pending(py) {
                None
            } else {
                raise_exception::<_>(py, "TypeError", message)
            };
        }
    }
    Some(exception.into_bits())
}

pub(crate) fn normalize_throw_argument(py: &PyToken<'_>, carrier: u64) -> Option<u64> {
    let _carrier = ExceptionValue::pin(py, carrier);
    let args = arguments(py, carrier)?;
    let exception = args[0];
    let value = args.get(1).copied().unwrap_or(MoltObject::none().bits());
    let traceback = args
        .get(2)
        .copied()
        .filter(|&bits| !obj_from_bits(bits).is_none());
    if let Some(traceback) = traceback {
        let ty = crate::builtin_classes(py).traceback;
        if !unsafe { crate::object::class_layout::is_real_instance(py, traceback, ty) } {
            return raise_exception::<_>(
                py,
                "TypeError",
                "throw() third argument must be a traceback object",
            );
        }
    }
    let is_class = exception_is_class(py, exception);
    let normalized = if exception_is_instance(py, exception) {
        if !obj_from_bits(value).is_none() {
            return raise_exception::<_>(
                py,
                "TypeError",
                "instance exception may not have a separate value",
            );
        }
        inc_ref_bits(py, exception);
        exception
    } else if is_class {
        let matching_instance = if exception_is_instance(py, value) {
            let Some(actual_class) = exception_class(py, value) else {
                return take_throw_normalization_failure(py, traceback);
            };
            let matched =
                ExceptionValue::adopt(py, crate::molt_issubclass(actual_class.bits(), exception));
            if exception_pending(py) {
                return take_throw_normalization_failure(py, traceback);
            }
            let matches = is_truthy(py, obj_from_bits(matched.bits()));
            if exception_pending(py) {
                return take_throw_normalization_failure(py, traceback);
            }
            matches
        } else {
            false
        };
        if matching_instance {
            // NormalizeException trusts an existing matching subclass and
            // updates the exception type to that instance's actual class.
            inc_ref_bits(py, value);
            value
        } else {
            let Some(instance) = construct_throw_exception(py, exception, value) else {
                return take_throw_normalization_failure(py, traceback);
            };
            // A freshly constructed result does not replace the requested
            // type in NormalizeException. The following PyErr_Restore step
            // accepts an exact class only, constructing once more otherwise.
            // This includes a constructed subclass, but not a supplied one.
            let instance = ExceptionValue::adopt(py, instance);
            let Some(actual_class) = exception_class(py, instance.bits()) else {
                return take_throw_normalization_failure(py, traceback);
            };
            if actual_class.bits() != exception {
                let restored = construct_throw_exception(py, exception, instance.bits());
                match restored {
                    Some(restored) => restored,
                    None => return take_throw_normalization_failure(py, None),
                }
            } else {
                instance.into_bits()
            }
        }
    } else {
        return raise_exception::<_>(
            py,
            "TypeError",
            "exceptions must be classes or instances deriving from BaseException",
        );
    };
    // Class/value restoration replaces the traceback even when it is absent;
    // the single-instance form preserves its existing traceback when omitted.
    let normalized = ExceptionValue::adopt(py, normalized);
    let restored_traceback = traceback.or_else(|| is_class.then(|| MoltObject::none().bits()));
    if let Some(traceback) = restored_traceback {
        if let Err(message) = crate::builtins::exceptions::exception_replace_field_bits(
            py,
            normalized.bits(),
            crate::builtins::exceptions::ExceptionFieldSlot::Traceback,
            traceback,
        ) {
            return if exception_pending(py) {
                None
            } else {
                raise_exception::<_>(py, "TypeError", message)
            };
        }
    }
    Some(normalized.into_bits())
}

pub(crate) fn raise_throw_argument(py: &PyToken<'_>, carrier: u64) -> u64 {
    let Some(exception) = normalize_throw_argument(py, carrier) else {
        return MoltObject::none().bits();
    };
    crate::molt_exception_trace_prepend(exception);
    let result = crate::molt_raise(exception);
    dec_ref_bits(py, exception);
    result
}

pub(crate) fn throw_is_generator_exit(py: &PyToken<'_>, carrier: u64) -> bool {
    let Some(args) = arguments(py, carrier) else {
        return false;
    };
    let target = crate::exception_type_bits_from_name(py, "GeneratorExit");
    if exception_is_class(py, args[0]) {
        exception_class_is_subtype(py, args[0], target)
    } else {
        exception_matches_type(py, args[0], target)
    }
}

pub(crate) unsafe fn call_throw_method(py: &PyToken<'_>, method: u64, carrier: u64) -> u64 {
    let Some(args) = arguments(py, carrier) else {
        return MoltObject::none().bits();
    };
    unsafe {
        match args.as_slice() {
            [ty] => crate::call_callable1(py, method, *ty),
            [ty, value] => crate::call_callable2(py, method, *ty, *value),
            [ty, value, traceback] => crate::call_callable3(py, method, *ty, *value, *traceback),
            _ => unreachable!(),
        }
    }
}

/// Convert only exceptions escaping a resumed Python body. Delegated iterator
/// exhaustion and exceptions thrown before first execution never enter here.
/// The incoming exception is owned and consumed on every path.
pub(crate) unsafe fn raise_body_exception(
    py: &PyToken<'_>,
    task_ptr: *mut u8,
    exception: u64,
) -> u64 {
    use crate::builtins::exceptions::{
        ExceptionFieldSlot, exception_matches_builtin_name, exception_replace_field_bits,
    };
    use crate::object::layout::{CodeExecutionKind, code_execution_kind};

    let kind = unsafe {
        let code = crate::object::aux_header::object_frame_code_bits(task_ptr);
        if let Some(code) = maybe_ptr_from_bits(code)
            && object_type_id(code) == TYPE_ID_CODE
        {
            code_execution_kind(code)
        } else if object_type_id(task_ptr) == TYPE_ID_GENERATOR {
            CodeExecutionKind::Generator
        } else if crate::async_rt::generators::is_native_coroutine_bits(
            MoltObject::from_ptr(task_ptr).bits(),
        ) {
            CodeExecutionKind::Coroutine
        } else {
            CodeExecutionKind::Direct
        }
    };
    let message = if kind != CodeExecutionKind::Direct
        && exception_matches_builtin_name(py, exception, "StopIteration")
    {
        Some(match kind {
            CodeExecutionKind::AsyncGenerator => "async generator raised StopIteration",
            CodeExecutionKind::Coroutine => "coroutine raised StopIteration",
            CodeExecutionKind::Generator => "generator raised StopIteration",
            CodeExecutionKind::Direct => unreachable!(),
        })
    } else if kind == CodeExecutionKind::AsyncGenerator
        && exception_matches_builtin_name(py, exception, "StopAsyncIteration")
    {
        Some("async generator raised StopAsyncIteration")
    } else {
        None
    };
    let Some(message) = message else {
        let result = molt_raise(exception);
        dec_ref_bits(py, exception);
        return result;
    };
    let converted = alloc_exception(py, "RuntimeError", message);
    if converted.is_null() {
        dec_ref_bits(py, exception);
        if !exception_pending(py) {
            return raise_exception::<_>(
                py,
                "MemoryError",
                "exception conversion allocation failed",
            );
        }
        return MoltObject::none().bits();
    }
    let converted = MoltObject::from_ptr(converted).bits();
    for field in [ExceptionFieldSlot::Cause, ExceptionFieldSlot::Context] {
        if let Err(message) = exception_replace_field_bits(py, converted, field, exception) {
            dec_ref_bits(py, converted);
            dec_ref_bits(py, exception);
            if !exception_pending(py) {
                return raise_exception::<_>(py, "SystemError", message);
            }
            return MoltObject::none().bits();
        }
    }
    dec_ref_bits(py, exception);
    let result = molt_raise(converted);
    dec_ref_bits(py, converted);
    result
}
