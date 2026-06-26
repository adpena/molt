//! Shared asyncio Python-object call, exception, waiter, and slot helpers.
//!
//! These helpers are used by ready queues, task groups, combinators,
//! event-loop glue, process futures, and socket/stream I/O. Keeping them
//! separate prevents any one async primitive family from owning the bridge.

use super::*;

pub(crate) unsafe fn asyncio_drop_slot_ref(_py: &PyToken<'_>, payload_ptr: *mut u64, idx: usize) {
    unsafe {
        let bits = *payload_ptr.add(idx);
        if bits != 0 && !obj_from_bits(bits).is_none() {
            dec_ref_bits(_py, bits);
        }
        *payload_ptr.add(idx) = MoltObject::none().bits();
    }
}

pub(crate) unsafe fn asyncio_clear_pending_exception(_py: &PyToken<'_>) {
    if !exception_pending(_py) {
        return;
    }
    let exc_bits = molt_exception_last();
    dec_ref_bits(_py, exc_bits);
    molt_exception_clear();
}

pub(crate) unsafe fn asyncio_exception_kind_is(
    _py: &PyToken<'_>,
    exc_bits: u64,
    expected: &str,
) -> bool {
    unsafe {
        let kind_bits = molt_exception_kind(exc_bits);
        if exception_pending(_py) {
            asyncio_clear_pending_exception(_py);
            return false;
        }
        let kind = string_obj_to_owned(obj_from_bits(kind_bits));
        if !obj_from_bits(kind_bits).is_none() {
            dec_ref_bits(_py, kind_bits);
        }
        kind.as_deref() == Some(expected)
    }
}

pub(crate) unsafe fn asyncio_exception_is_fatal_base(_py: &PyToken<'_>, exc_bits: u64) -> bool {
    unsafe {
        let kind_bits = molt_exception_kind(exc_bits);
        if exception_pending(_py) {
            asyncio_clear_pending_exception(_py);
            return false;
        }
        let kind = string_obj_to_owned(obj_from_bits(kind_bits));
        if !obj_from_bits(kind_bits).is_none() {
            dec_ref_bits(_py, kind_bits);
        }
        matches!(
            kind.as_deref(),
            Some("KeyboardInterrupt")
                | Some("SystemExit")
                | Some("GeneratorExit")
                | Some("BaseExceptionGroup")
        )
    }
}

pub(crate) unsafe fn asyncio_call_method0(_py: &PyToken<'_>, obj_bits: u64, method: &[u8]) -> u64 {
    unsafe {
        let Some(obj_ptr) = obj_from_bits(obj_bits).as_ptr() else {
            return raise_exception::<u64>(_py, "TypeError", "object is not awaitable");
        };
        let Some(method_name_bits) = attr_name_bits_from_bytes(_py, method) else {
            return MoltObject::none().bits();
        };
        let Some(method_bits) = attr_lookup_ptr_allow_missing(_py, obj_ptr, method_name_bits)
        else {
            return raise_exception::<u64>(_py, "TypeError", "object is not awaitable");
        };
        let out = call_callable0(_py, method_bits);
        dec_ref_bits(_py, method_bits);
        out
    }
}

pub(crate) unsafe fn asyncio_call_method1(
    _py: &PyToken<'_>,
    obj_bits: u64,
    method: &[u8],
    arg_bits: u64,
) -> u64 {
    unsafe {
        let Some(obj_ptr) = obj_from_bits(obj_bits).as_ptr() else {
            return raise_exception::<u64>(_py, "TypeError", "object is not awaitable");
        };
        let Some(method_name_bits) = attr_name_bits_from_bytes(_py, method) else {
            return MoltObject::none().bits();
        };
        let Some(method_bits) = attr_lookup_ptr_allow_missing(_py, obj_ptr, method_name_bits)
        else {
            return raise_exception::<u64>(_py, "TypeError", "object is not awaitable");
        };
        let out = call_callable1(_py, method_bits, arg_bits);
        dec_ref_bits(_py, method_bits);
        out
    }
}

pub(crate) unsafe fn asyncio_call_method2(
    _py: &PyToken<'_>,
    obj_bits: u64,
    method: &[u8],
    arg0_bits: u64,
    arg1_bits: u64,
) -> u64 {
    unsafe {
        let Some(obj_ptr) = obj_from_bits(obj_bits).as_ptr() else {
            return raise_exception::<u64>(_py, "TypeError", "object is not awaitable");
        };
        let Some(method_name_bits) = attr_name_bits_from_bytes(_py, method) else {
            return MoltObject::none().bits();
        };
        let Some(method_bits) = attr_lookup_ptr_allow_missing(_py, obj_ptr, method_name_bits)
        else {
            return raise_exception::<u64>(_py, "TypeError", "object is not awaitable");
        };
        let out = call_callable2(_py, method_bits, arg0_bits, arg1_bits);
        dec_ref_bits(_py, method_bits);
        out
    }
}

pub(crate) unsafe fn asyncio_call_method3(
    _py: &PyToken<'_>,
    obj_bits: u64,
    method: &[u8],
    arg0_bits: u64,
    arg1_bits: u64,
    arg2_bits: u64,
) -> u64 {
    unsafe {
        let Some(obj_ptr) = obj_from_bits(obj_bits).as_ptr() else {
            return raise_exception::<u64>(_py, "TypeError", "object is not awaitable");
        };
        let Some(method_name_bits) = attr_name_bits_from_bytes(_py, method) else {
            return MoltObject::none().bits();
        };
        let Some(method_bits) = attr_lookup_ptr_allow_missing(_py, obj_ptr, method_name_bits)
        else {
            return raise_exception::<u64>(_py, "TypeError", "object is not awaitable");
        };
        let out = call_callable3(_py, method_bits, arg0_bits, arg1_bits, arg2_bits);
        dec_ref_bits(_py, method_bits);
        out
    }
}

pub(crate) unsafe fn asyncio_call_with_args(
    _py: &PyToken<'_>,
    callable_bits: u64,
    args_bits: u64,
) -> u64 {
    unsafe {
        let builder_bits = molt_callargs_new(0, 0);
        if obj_from_bits(builder_bits).is_none() {
            return builder_bits;
        }
        let _ = molt_callargs_expand_star(builder_bits, args_bits);
        if exception_pending(_py) {
            dec_ref_bits(_py, builder_bits);
            return MoltObject::none().bits();
        }
        molt_call_bind(callable_bits, builder_bits)
    }
}

pub(crate) unsafe fn asyncio_call_method0_allow_missing(
    _py: &PyToken<'_>,
    obj_bits: u64,
    method: &[u8],
) -> Option<u64> {
    unsafe {
        let obj_ptr = obj_from_bits(obj_bits).as_ptr()?;
        let method_name_bits = attr_name_bits_from_bytes(_py, method)?;
        let method_bits = attr_lookup_ptr_allow_missing(_py, obj_ptr, method_name_bits)?;
        let out = call_callable0(_py, method_bits);
        dec_ref_bits(_py, method_bits);
        Some(out)
    }
}

pub(crate) unsafe fn asyncio_attr_lookup_allow_missing(
    _py: &PyToken<'_>,
    obj_bits: u64,
    name: &[u8],
) -> Option<u64> {
    unsafe {
        let obj_ptr = obj_from_bits(obj_bits).as_ptr()?;
        let name_bits = attr_name_bits_from_bytes(_py, name)?;
        attr_lookup_ptr_allow_missing(_py, obj_ptr, name_bits)
    }
}

pub(crate) unsafe fn asyncio_take_pending_exception_bits(_py: &PyToken<'_>) -> u64 {
    let exc_bits = molt_exception_last();
    molt_exception_clear();
    exc_bits
}

pub(crate) unsafe fn asyncio_method_truthy(
    _py: &PyToken<'_>,
    obj_bits: u64,
    method: &[u8],
) -> Option<bool> {
    unsafe {
        let bits = asyncio_call_method0(_py, obj_bits, method);
        if exception_pending(_py) {
            return None;
        }
        let truthy = is_truthy(_py, obj_from_bits(bits));
        dec_ref_bits(_py, bits);
        Some(truthy)
    }
}

pub(crate) unsafe fn asyncio_waiters_pop_front(_py: &PyToken<'_>, waiters_bits: u64) -> u64 {
    unsafe {
        if let Some(bits) = asyncio_call_method0_allow_missing(_py, waiters_bits, b"popleft") {
            return bits;
        }
        asyncio_call_method1(_py, waiters_bits, b"pop", MoltObject::from_int(0).bits())
    }
}

unsafe fn asyncio_taskgroup_append_error(
    _py: &PyToken<'_>,
    errors_bits: u64,
    err_bits: u64,
) -> Option<()> {
    unsafe {
        let append_bits = asyncio_call_method1(_py, errors_bits, b"append", err_bits);
        if exception_pending(_py) {
            return None;
        }
        if !obj_from_bits(append_bits).is_none() {
            dec_ref_bits(_py, append_bits);
        }
        Some(())
    }
}

pub(crate) unsafe fn asyncio_taskgroup_collect_task_error(
    _py: &PyToken<'_>,
    errors_bits: u64,
    task_bits: u64,
) -> Option<bool> {
    unsafe {
        let exc_bits = asyncio_call_method0(_py, task_bits, b"exception");
        if exception_pending(_py) {
            let pending_exc_bits = asyncio_take_pending_exception_bits(_py);
            let cancelled = asyncio_exception_kind_is(_py, pending_exc_bits, "CancelledError");
            if cancelled {
                dec_ref_bits(_py, pending_exc_bits);
                return Some(false);
            }
            asyncio_taskgroup_append_error(_py, errors_bits, pending_exc_bits)?;
            dec_ref_bits(_py, pending_exc_bits);
            return Some(true);
        }
        if obj_from_bits(exc_bits).is_none() {
            return Some(false);
        }
        let cancelled = asyncio_exception_kind_is(_py, exc_bits, "CancelledError");
        if cancelled {
            dec_ref_bits(_py, exc_bits);
            return Some(false);
        }
        asyncio_taskgroup_append_error(_py, errors_bits, exc_bits)?;
        dec_ref_bits(_py, exc_bits);
        Some(true)
    }
}
