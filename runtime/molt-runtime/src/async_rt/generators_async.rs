//! Asyncio primitives: futures, promises, sleep, timers, and stream/socket I/O.
//!
//! Split from generators.rs to reduce file size.

use crate::BuiltinExc;
use crate::PyToken;
use std::sync::atomic::Ordering as AtomicOrdering;
use std::time::{Duration, Instant};

use molt_obj_model::MoltObject;

use crate::concurrency::GilGuard;
#[cfg(target_arch = "wasm32")]
use crate::libc_compat as libc;
use crate::object::accessors::resolve_obj_ptr;
use crate::object::{HEADER_FLAG_COROUTINE, HEADER_FLAG_TASK_DONE};
use crate::*;

#[cfg(not(target_arch = "wasm32"))]
use crate::{process_task_state, thread_task_state};

#[cfg(molt_net)]
use super::generators::asyncio_connect_trace_enabled;
use super::generators::{
    debug_current_task, promise_trace_enabled, resolve_sleep_target, sleep_trace_enabled,
};
use super::scheduler::trace_task_result;

#[path = "generators_async_future.rs"]
mod generators_async_future;
pub(crate) use generators_async_future::*;

#[path = "generators_async_pyops.rs"]
mod generators_async_pyops;
pub(crate) use generators_async_pyops::*;

// Socket, stream, fd-watcher and accept-loop futures wait on the networking
// I/O driver, so they exist only with a networking provider (HF-487).
#[cfg(molt_net)]
#[path = "generators_async_io.rs"]
mod generators_async_io;
#[cfg(molt_net)]
pub(crate) use generators_async_io::*;

/// # Safety
/// - `waiters_bits` must be a deque/list-like object supporting pop-front semantics.

pub(crate) unsafe fn molt_asyncio_waiters_notify_attached_body(
    _py: &crate::PyToken<'_>,
    waiters_bits: u64,
    count_bits: u64,
    result_bits: u64,
) -> u64 {
    unsafe {
        crate::with_attached_entry_nopanic!(_py, _py, {
            let Some(mut count) = to_i64(obj_from_bits(count_bits)) else {
                return raise_exception::<u64>(
                    _py,
                    BuiltinExc::TypeError,
                    "waiter notify count must be int",
                );
            };
            if count <= 0 {
                return MoltObject::from_int(0).bits();
            }
            let len_bits = molt_len_attached_body(_py, waiters_bits);
            if exception_pending(_py) {
                return MoltObject::none().bits();
            }
            let Some(waiters_len) = to_i64(obj_from_bits(len_bits)) else {
                if !obj_from_bits(len_bits).is_none() {
                    dec_ref_bits(_py, len_bits);
                }
                return raise_exception::<u64>(
                    _py,
                    BuiltinExc::TypeError,
                    "waiter collection must be sized",
                );
            };
            if !obj_from_bits(len_bits).is_none() {
                dec_ref_bits(_py, len_bits);
            }
            if waiters_len <= 0 {
                return MoltObject::from_int(0).bits();
            }
            count = count.min(waiters_len);
            let mut woken_count = 0i64;
            for _ in 0..count {
                let waiter_bits = asyncio_waiters_pop_front(_py, waiters_bits);
                if exception_pending(_py) {
                    return MoltObject::none().bits();
                }
                let Some(done) = asyncio_method_truthy(_py, waiter_bits, crate::NameId::Done)
                else {
                    if !obj_from_bits(waiter_bits).is_none() {
                        dec_ref_bits(_py, waiter_bits);
                    }
                    return MoltObject::none().bits();
                };
                if !done {
                    let out_bits = asyncio_call_method1(
                        _py,
                        waiter_bits,
                        crate::NameId::SetResult,
                        result_bits,
                    );
                    if exception_pending(_py) {
                        if !obj_from_bits(waiter_bits).is_none() {
                            dec_ref_bits(_py, waiter_bits);
                        }
                        return MoltObject::none().bits();
                    }
                    if !obj_from_bits(out_bits).is_none() {
                        dec_ref_bits(_py, out_bits);
                    }
                }
                if !obj_from_bits(waiter_bits).is_none() {
                    dec_ref_bits(_py, waiter_bits);
                }
                woken_count += 1;
            }
            MoltObject::from_int(woken_count).bits()
        })
    }
}

/// # Safety
/// - `waiters_bits` must be a deque/list-like object supporting pop-front semantics.

pub(crate) unsafe fn molt_asyncio_waiters_notify_exception_attached_body(
    _py: &crate::PyToken<'_>,
    waiters_bits: u64,
    count_bits: u64,
    exc_bits: u64,
) -> u64 {
    unsafe {
        crate::with_attached_entry_nopanic!(_py, _py, {
            let Some(mut count) = to_i64(obj_from_bits(count_bits)) else {
                return raise_exception::<u64>(
                    _py,
                    BuiltinExc::TypeError,
                    "waiter notify-exception count must be int",
                );
            };
            if count <= 0 {
                return MoltObject::from_int(0).bits();
            }
            let len_bits = molt_len_attached_body(_py, waiters_bits);
            if exception_pending(_py) {
                return MoltObject::none().bits();
            }
            let Some(waiters_len) = to_i64(obj_from_bits(len_bits)) else {
                if !obj_from_bits(len_bits).is_none() {
                    dec_ref_bits(_py, len_bits);
                }
                return raise_exception::<u64>(
                    _py,
                    BuiltinExc::TypeError,
                    "waiter collection must be sized",
                );
            };
            if !obj_from_bits(len_bits).is_none() {
                dec_ref_bits(_py, len_bits);
            }
            if waiters_len <= 0 {
                return MoltObject::from_int(0).bits();
            }
            count = count.min(waiters_len);
            let mut woken_count = 0i64;
            for _ in 0..count {
                let waiter_bits = asyncio_waiters_pop_front(_py, waiters_bits);
                if exception_pending(_py) {
                    return MoltObject::none().bits();
                }
                let Some(done) = asyncio_method_truthy(_py, waiter_bits, crate::NameId::Done)
                else {
                    if !obj_from_bits(waiter_bits).is_none() {
                        dec_ref_bits(_py, waiter_bits);
                    }
                    return MoltObject::none().bits();
                };
                if !done {
                    let out_bits = asyncio_call_method1(
                        _py,
                        waiter_bits,
                        crate::NameId::SetException,
                        exc_bits,
                    );
                    if exception_pending(_py) {
                        if !obj_from_bits(waiter_bits).is_none() {
                            dec_ref_bits(_py, waiter_bits);
                        }
                        return MoltObject::none().bits();
                    }
                    if !obj_from_bits(out_bits).is_none() {
                        dec_ref_bits(_py, out_bits);
                    }
                }
                if !obj_from_bits(waiter_bits).is_none() {
                    dec_ref_bits(_py, waiter_bits);
                }
                woken_count += 1;
            }
            MoltObject::from_int(woken_count).bits()
        })
    }
}

/// # Safety
/// - `waiters_bits` must support `remove(waiter)` semantics.

pub(crate) unsafe fn molt_asyncio_waiters_remove_attached_body(
    _py: &crate::PyToken<'_>,
    waiters_bits: u64,
    waiter_bits: u64,
) -> u64 {
    unsafe {
        crate::with_attached_entry_nopanic!(_py, _py, {
            let out_bits =
                asyncio_call_method1(_py, waiters_bits, crate::NameId::Remove, waiter_bits);
            if exception_pending(_py) {
                asyncio_clear_pending_exception(_py);
                return MoltObject::from_bool(false).bits();
            }
            if !obj_from_bits(out_bits).is_none() {
                dec_ref_bits(_py, out_bits);
            }
            MoltObject::from_bool(true).bits()
        })
    }
}

/// # Safety
/// - `condition_bits` must be an asyncio.Condition-like object.
/// - `predicate_bits` must be callable.

pub(crate) unsafe fn molt_asyncio_condition_wait_for_step_attached_body(
    _py: &crate::PyToken<'_>,
    condition_bits: u64,
    predicate_bits: u64,
) -> u64 {
    unsafe {
        crate::with_attached_entry_nopanic!(_py, _py, {
            let callable_bits = molt_is_callable_attached_body(_py, predicate_bits);
            let is_callable = is_truthy(_py, obj_from_bits(callable_bits));
            if !obj_from_bits(callable_bits).is_none() {
                dec_ref_bits(_py, callable_bits);
            }
            if !is_callable {
                return raise_exception::<u64>(
                    _py,
                    BuiltinExc::TypeError,
                    "predicate must be callable",
                );
            }

            let predicate_out = call_callable0(_py, predicate_bits);
            if exception_pending(_py) {
                return MoltObject::none().bits();
            }
            let done = is_truthy(_py, obj_from_bits(predicate_out));
            let done_bits = MoltObject::from_bool(done).bits();
            if done {
                let out_ptr = alloc_tuple(_py, &[done_bits, predicate_out]);
                if out_ptr.is_null() {
                    if !obj_from_bits(predicate_out).is_none() {
                        dec_ref_bits(_py, predicate_out);
                    }
                    return MoltObject::none().bits();
                }
                if !obj_from_bits(predicate_out).is_none() {
                    dec_ref_bits(_py, predicate_out);
                }
                return MoltObject::from_ptr(out_ptr).bits();
            }
            if !obj_from_bits(predicate_out).is_none() {
                dec_ref_bits(_py, predicate_out);
            }

            let wait_bits = asyncio_call_method0(_py, condition_bits, crate::NameId::Wait);
            if exception_pending(_py) {
                return wait_bits;
            }
            let out_ptr = alloc_tuple(_py, &[done_bits, wait_bits]);
            if out_ptr.is_null() {
                if !obj_from_bits(wait_bits).is_none() {
                    dec_ref_bits(_py, wait_bits);
                }
                return MoltObject::none().bits();
            }
            if !obj_from_bits(wait_bits).is_none() {
                dec_ref_bits(_py, wait_bits);
            }
            MoltObject::from_ptr(out_ptr).bits()
        })
    }
}

/// # Safety
/// - `waiters_bits` must be iterable and contain asyncio Future-compatible waiters.

pub(crate) unsafe fn molt_asyncio_barrier_release_attached_body(
    _py: &crate::PyToken<'_>,
    waiters_bits: u64,
) -> u64 {
    unsafe {
        crate::with_attached_entry_nopanic!(_py, _py, {
            let Some(waiter_tuple_bits) = tuple_from_iter_bits(_py, waiters_bits) else {
                return MoltObject::none().bits();
            };
            let clear_bits = asyncio_call_method0(_py, waiters_bits, crate::NameId::Clear);
            if exception_pending(_py) {
                dec_ref_bits(_py, waiter_tuple_bits);
                return MoltObject::none().bits();
            }
            if !obj_from_bits(clear_bits).is_none() {
                dec_ref_bits(_py, clear_bits);
            }
            let Some(waiter_tuple_ptr) = obj_from_bits(waiter_tuple_bits).as_ptr() else {
                dec_ref_bits(_py, waiter_tuple_bits);
                return raise_exception::<u64>(
                    _py,
                    BuiltinExc::TypeError,
                    "barrier waiter collection must be iterable",
                );
            };
            let waiter_count = crate::object::seq_access::len(waiter_tuple_ptr);
            let mut released_count = 0i64;
            for idx in 0..waiter_count {
                let Some(waiter) = crate::object::seq_access::pin_item(_py, waiter_tuple_ptr, idx)
                else {
                    dec_ref_bits(_py, waiter_tuple_bits);
                    return raise_exception::<u64>(
                        _py,
                        BuiltinExc::RuntimeError,
                        "invalid waiter state",
                    );
                };
                let waiter_bits = waiter.bits();
                let Some(done) = asyncio_method_truthy(_py, waiter_bits, crate::NameId::Done)
                else {
                    dec_ref_bits(_py, waiter_tuple_bits);
                    return MoltObject::none().bits();
                };
                if done {
                    continue;
                }
                let out_bits = asyncio_call_method1(
                    _py,
                    waiter_bits,
                    crate::NameId::SetResult,
                    MoltObject::from_int(idx as i64).bits(),
                );
                if exception_pending(_py) {
                    dec_ref_bits(_py, waiter_tuple_bits);
                    return MoltObject::none().bits();
                }
                if !obj_from_bits(out_bits).is_none() {
                    dec_ref_bits(_py, out_bits);
                }
                released_count += 1;
            }
            dec_ref_bits(_py, waiter_tuple_bits);
            MoltObject::from_int(released_count).bits()
        })
    }
}

unsafe fn asyncio_transfer_set_target_exception(
    _py: &PyToken<'_>,
    target_bits: u64,
    exc_bits: u64,
) {
    unsafe {
        let out_bits =
            asyncio_call_method1(_py, target_bits, crate::NameId::SetException, exc_bits);
        if !obj_from_bits(out_bits).is_none() {
            dec_ref_bits(_py, out_bits);
        }
        if exception_pending(_py) {
            asyncio_clear_pending_exception(_py);
        }
    }
}

/// # Safety
/// - `source_bits`/`target_bits` must be Future-compatible objects.

pub(crate) unsafe fn molt_asyncio_future_transfer_attached_body(
    _py: &crate::PyToken<'_>,
    source_bits: u64,
    target_bits: u64,
) -> u64 {
    unsafe {
        crate::with_attached_entry_nopanic!(_py, _py, {
            let Some(target_done) = asyncio_method_truthy(_py, target_bits, crate::NameId::Done)
            else {
                asyncio_clear_pending_exception(_py);
                return MoltObject::from_bool(false).bits();
            };
            if target_done {
                return MoltObject::from_bool(false).bits();
            }

            let Some(source_cancelled) =
                asyncio_method_truthy(_py, source_bits, crate::NameId::Cancelled)
            else {
                asyncio_clear_pending_exception(_py);
                return MoltObject::from_bool(false).bits();
            };
            if source_cancelled {
                let cancel_msg_ref = match asyncio_attr_lookup_allow_missing(
                    _py,
                    source_bits,
                    crate::NameId::PrivateCancelMessage,
                ) {
                    Ok(value) => value,
                    Err(()) => {
                        asyncio_clear_pending_exception(_py);
                        return MoltObject::from_bool(false).bits();
                    }
                };
                let cancel_msg_bits = cancel_msg_ref.unwrap_or_else(|| MoltObject::none().bits());
                let out_bits =
                    asyncio_call_method1(_py, target_bits, crate::NameId::Cancel, cancel_msg_bits);
                if let Some(found_bits) = cancel_msg_ref
                    && !obj_from_bits(found_bits).is_none()
                {
                    dec_ref_bits(_py, found_bits);
                }
                if !obj_from_bits(out_bits).is_none() {
                    dec_ref_bits(_py, out_bits);
                }
                if exception_pending(_py) {
                    asyncio_clear_pending_exception(_py);
                    return MoltObject::from_bool(false).bits();
                }
                return MoltObject::from_bool(true).bits();
            }

            let source_exc_bits = asyncio_call_method0(_py, source_bits, crate::NameId::Exception);
            if exception_pending(_py) {
                asyncio_clear_pending_exception(_py);
                return MoltObject::from_bool(false).bits();
            }
            let source_has_exc = !obj_from_bits(source_exc_bits).is_none();
            if source_has_exc {
                asyncio_transfer_set_target_exception(_py, target_bits, source_exc_bits);
                dec_ref_bits(_py, source_exc_bits);
                if exception_pending(_py) {
                    asyncio_clear_pending_exception(_py);
                    return MoltObject::from_bool(false).bits();
                }
                return MoltObject::from_bool(true).bits();
            }
            if !obj_from_bits(source_exc_bits).is_none() {
                dec_ref_bits(_py, source_exc_bits);
            }

            let result_bits = asyncio_call_method0(_py, source_bits, crate::NameId::Result);
            if exception_pending(_py) {
                asyncio_clear_pending_exception(_py);
                return MoltObject::from_bool(false).bits();
            }
            let out_bits =
                asyncio_call_method1(_py, target_bits, crate::NameId::SetResult, result_bits);
            if !obj_from_bits(result_bits).is_none() {
                dec_ref_bits(_py, result_bits);
            }
            if !obj_from_bits(out_bits).is_none() {
                dec_ref_bits(_py, out_bits);
            }
            if exception_pending(_py) {
                asyncio_clear_pending_exception(_py);
                return MoltObject::from_bool(false).bits();
            }
            MoltObject::from_bool(true).bits()
        })
    }
}

/// # Safety
/// - `waiters_bits` must be iterable and contain Event waiter futures.

pub(crate) unsafe fn molt_asyncio_event_waiters_cleanup_attached_body(
    _py: &crate::PyToken<'_>,
    waiters_bits: u64,
) -> u64 {
    unsafe {
        crate::with_attached_entry_nopanic!(_py, _py, {
            let Some(waiter_tuple_bits) = tuple_from_iter_bits(_py, waiters_bits) else {
                return MoltObject::none().bits();
            };
            let Some(waiter_tuple_ptr) = obj_from_bits(waiter_tuple_bits).as_ptr() else {
                dec_ref_bits(_py, waiter_tuple_bits);
                return raise_exception::<u64>(
                    _py,
                    BuiltinExc::TypeError,
                    "event waiters must be iterable",
                );
            };
            let waiter_count = crate::object::seq_access::len(waiter_tuple_ptr);
            let mut cleaned = 0i64;
            for idx in 0..waiter_count {
                let Some(waiter) = crate::object::seq_access::pin_item(_py, waiter_tuple_ptr, idx)
                else {
                    dec_ref_bits(_py, waiter_tuple_bits);
                    return raise_exception::<u64>(
                        _py,
                        BuiltinExc::RuntimeError,
                        "invalid waiter state",
                    );
                };
                let waiter_bits = waiter.bits();
                let owner_bits = match asyncio_attr_lookup_allow_missing(
                    _py,
                    waiter_bits,
                    crate::NameId::PrivateMoltEventOwner,
                ) {
                    Ok(Some(bits)) => bits,
                    Ok(None) => continue,
                    Err(()) => {
                        dec_ref_bits(_py, waiter_tuple_bits);
                        return MoltObject::none().bits();
                    }
                };
                if obj_from_bits(owner_bits).is_none() {
                    dec_ref_bits(_py, owner_bits);
                    continue;
                }
                let owner_waiters_bits = match asyncio_attr_lookup_allow_missing(
                    _py,
                    owner_bits,
                    crate::NameId::PrivateWaiters,
                ) {
                    Ok(Some(bits)) => bits,
                    Ok(None) => {
                        dec_ref_bits(_py, owner_bits);
                        continue;
                    }
                    Err(()) => {
                        dec_ref_bits(_py, owner_bits);
                        dec_ref_bits(_py, waiter_tuple_bits);
                        return MoltObject::none().bits();
                    }
                };
                let out_bits = asyncio_call_method1(
                    _py,
                    owner_waiters_bits,
                    crate::NameId::Remove,
                    waiter_bits,
                );
                if exception_pending(_py) {
                    asyncio_clear_pending_exception(_py);
                } else {
                    cleaned += 1;
                }
                if !obj_from_bits(out_bits).is_none() {
                    dec_ref_bits(_py, out_bits);
                }
                if !obj_from_bits(owner_waiters_bits).is_none() {
                    dec_ref_bits(_py, owner_waiters_bits);
                }
                if !obj_from_bits(owner_bits).is_none() {
                    dec_ref_bits(_py, owner_bits);
                }
            }
            dec_ref_bits(_py, waiter_tuple_bits);
            MoltObject::from_int(cleaned).bits()
        })
    }
}

/// # Safety
/// - `tasks_bits` must be iterable and contain Future-like objects.
/// - `callback_bits` must be callable.

pub(crate) unsafe fn molt_asyncio_tasks_add_done_callback_attached_body(
    _py: &crate::PyToken<'_>,
    tasks_bits: u64,
    callback_bits: u64,
) -> u64 {
    unsafe {
        crate::with_attached_entry_nopanic!(_py, _py, {
            let callable_bits = molt_is_callable_attached_body(_py, callback_bits);
            let is_callable = is_truthy(_py, obj_from_bits(callable_bits));
            if !obj_from_bits(callable_bits).is_none() {
                dec_ref_bits(_py, callable_bits);
            }
            if !is_callable {
                return raise_exception::<u64>(
                    _py,
                    BuiltinExc::TypeError,
                    "callback must be callable",
                );
            }
            let Some(task_tuple_bits) = tuple_from_iter_bits(_py, tasks_bits) else {
                return MoltObject::none().bits();
            };
            let Some(task_tuple_ptr) = obj_from_bits(task_tuple_bits).as_ptr() else {
                dec_ref_bits(_py, task_tuple_bits);
                return raise_exception::<u64>(
                    _py,
                    BuiltinExc::TypeError,
                    "tasks must be iterable",
                );
            };
            let task_count = crate::object::seq_access::len(task_tuple_ptr);
            let mut attached = 0i64;
            for idx in 0..task_count {
                let Some(task) = crate::object::seq_access::pin_item(_py, task_tuple_ptr, idx)
                else {
                    dec_ref_bits(_py, task_tuple_bits);
                    return raise_exception::<u64>(
                        _py,
                        BuiltinExc::RuntimeError,
                        "invalid task state",
                    );
                };
                let task_bits = task.bits();
                let out_bits = asyncio_call_method1(
                    _py,
                    task_bits,
                    crate::NameId::AddDoneCallback,
                    callback_bits,
                );
                if exception_pending(_py) {
                    dec_ref_bits(_py, task_tuple_bits);
                    return MoltObject::none().bits();
                }
                if !obj_from_bits(out_bits).is_none() {
                    dec_ref_bits(_py, out_bits);
                }
                attached += 1;
            }
            dec_ref_bits(_py, task_tuple_bits);
            MoltObject::from_int(attached).bits()
        })
    }
}

/// # Safety
/// - `future_bits` must be a valid pointer to a Molt future.

pub(crate) unsafe fn molt_asyncio_task_uncancel_apply_attached_body(
    _py: &crate::PyToken<'_>,
    future_bits: u64,
) -> u64 {
    crate::with_attached_entry_nopanic!(_py, _py, {
        let Some(task_ptr) = resolve_task_ptr(future_bits) else {
            return raise_exception::<_>(_py, BuiltinExc::TypeError, "object is not awaitable");
        };
        task_cancel_message_clear(_py, task_ptr);
        let _ = task_take_cancel_pending(task_ptr);
        MoltObject::none().bits()
    })
}

/// # Safety
/// - `waiters_bits` must be iterable of Event waiters.

pub(crate) unsafe fn molt_asyncio_event_set_waiters_attached_body(
    _py: &crate::PyToken<'_>,
    waiters_bits: u64,
    result_bits: u64,
) -> u64 {
    unsafe {
        crate::with_attached_entry_nopanic!(_py, _py, {
            let Some(waiter_tuple_bits) = tuple_from_iter_bits(_py, waiters_bits) else {
                return MoltObject::none().bits();
            };
            let Some(waiter_tuple_ptr) = obj_from_bits(waiter_tuple_bits).as_ptr() else {
                dec_ref_bits(_py, waiter_tuple_bits);
                return raise_exception::<u64>(
                    _py,
                    BuiltinExc::TypeError,
                    "event waiters must be iterable",
                );
            };
            let waiter_count = crate::object::seq_access::len(waiter_tuple_ptr);
            let mut woke = 0i64;
            for idx in 0..waiter_count {
                let Some(waiter) = crate::object::seq_access::pin_item(_py, waiter_tuple_ptr, idx)
                else {
                    dec_ref_bits(_py, waiter_tuple_bits);
                    return raise_exception::<u64>(
                        _py,
                        BuiltinExc::RuntimeError,
                        "invalid waiter state",
                    );
                };
                let waiter_bits = waiter.bits();
                let token = match asyncio_attr_lookup_allow_missing(
                    _py,
                    waiter_bits,
                    crate::NameId::PrivateMoltEventTokenId,
                ) {
                    Ok(value) => value,
                    Err(()) => {
                        dec_ref_bits(_py, waiter_tuple_bits);
                        return MoltObject::none().bits();
                    }
                };
                if let Some(token_bits) = token {
                    if to_i64(obj_from_bits(token_bits)).is_some() {
                        let out = crate::molt_asyncio_event_waiters_unregister_attached_body(
                            _py,
                            token_bits,
                            waiter_bits,
                        );
                        if !obj_from_bits(out).is_none() {
                            dec_ref_bits(_py, out);
                        }
                        if exception_pending(_py) {
                            dec_ref_bits(_py, token_bits);
                            dec_ref_bits(_py, waiter_tuple_bits);
                            return MoltObject::none().bits();
                        }
                    }
                    if !obj_from_bits(token_bits).is_none() {
                        dec_ref_bits(_py, token_bits);
                    }
                }
                let out_bits =
                    asyncio_call_method1(_py, waiter_bits, crate::NameId::SetResult, result_bits);
                if exception_pending(_py) {
                    dec_ref_bits(_py, waiter_tuple_bits);
                    return out_bits;
                }
                if !obj_from_bits(out_bits).is_none() {
                    dec_ref_bits(_py, out_bits);
                }
                woke += 1;
            }
            dec_ref_bits(_py, waiter_tuple_bits);
            MoltObject::from_int(woke).bits()
        })
    }
}

// StreamReader buffer edits are pure bytearray operations; they need no I/O
// driver, unlike the stream futures in `generators_async_io`.

/// # Safety
/// - `buffer_bits` must be bytes-like.

pub(crate) fn molt_asyncio_stream_buffer_snapshot_attached_body(
    _py: &crate::PyToken<'_>,
    buffer_bits: u64,
) -> u64 {
    crate::with_attached_entry_nopanic!(_py, _py, {
        let out_bits = molt_bytes_from_obj_attached_body(_py, buffer_bits);
        if exception_pending(_py) {
            return out_bits;
        }
        out_bits
    })
}

/// # Safety
/// - `buffer_bits` must be a mutable bytearray-like object.

pub(crate) unsafe fn molt_asyncio_stream_buffer_consume_attached_body(
    _py: &crate::PyToken<'_>,
    buffer_bits: u64,
    count_bits: u64,
) -> u64 {
    unsafe {
        crate::with_attached_entry_nopanic!(_py, _py, {
            let Some(mut count) = to_i64(obj_from_bits(count_bits)) else {
                return raise_exception::<u64>(
                    _py,
                    BuiltinExc::TypeError,
                    "stream consume count must be int",
                );
            };
            if count <= 0 {
                return MoltObject::from_int(0).bits();
            }
            let len_bits = molt_len_attached_body(_py, buffer_bits);
            if exception_pending(_py) {
                return MoltObject::none().bits();
            }
            let Some(buf_len) = to_i64(obj_from_bits(len_bits)) else {
                if !obj_from_bits(len_bits).is_none() {
                    dec_ref_bits(_py, len_bits);
                }
                return raise_exception::<u64>(
                    _py,
                    BuiltinExc::TypeError,
                    "stream buffer must be sized",
                );
            };
            if !obj_from_bits(len_bits).is_none() {
                dec_ref_bits(_py, len_bits);
            }
            if buf_len <= 0 {
                return MoltObject::from_int(0).bits();
            }
            count = count.min(buf_len);

            if count == buf_len {
                let clear_bits = asyncio_call_method0(_py, buffer_bits, crate::NameId::Clear);
                if exception_pending(_py) {
                    return clear_bits;
                }
                if !obj_from_bits(clear_bits).is_none() {
                    dec_ref_bits(_py, clear_bits);
                }
                return MoltObject::from_int(count).bits();
            }

            let slice_bits = molt_slice_new_attached_body(
                _py,
                MoltObject::from_int(0).bits(),
                MoltObject::from_int(count).bits(),
                MoltObject::none().bits(),
            );
            if obj_from_bits(slice_bits).is_none() {
                return slice_bits;
            }
            let del_bits =
                asyncio_call_method1(_py, buffer_bits, crate::NameId::DunderDelitem, slice_bits);
            if !obj_from_bits(slice_bits).is_none() {
                dec_ref_bits(_py, slice_bits);
            }
            if exception_pending(_py) {
                return del_bits;
            }
            if !obj_from_bits(del_bits).is_none() {
                dec_ref_bits(_py, del_bits);
            }
            MoltObject::from_int(count).bits()
        })
    }
}

include!("../attached_exports_generated/async_rt/generators_async.rs");
