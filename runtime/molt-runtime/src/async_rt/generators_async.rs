//! Asyncio primitives: futures, promises, sleep, timers, stream/socket I/O,
//! gather, wait, wait_for.
//!
//! Split from generators.rs to reduce file size.

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
use crate::{is_block_on_task, process_task_state, thread_task_state};

use super::generators::{
    asyncio_connect_trace_enabled, debug_current_task, promise_trace_enabled, resolve_sleep_target,
    sleep_trace_enabled,
};
use super::scheduler::trace_task_result;

#[path = "generators_async_future.rs"]
mod generators_async_future;
pub(crate) use generators_async_future::*;

#[path = "generators_async_pyops.rs"]
mod generators_async_pyops;
pub(crate) use generators_async_pyops::*;

#[path = "generators_async_combinators.rs"]
mod generators_async_combinators;
pub(crate) use generators_async_combinators::*;

#[path = "generators_async_io.rs"]
mod generators_async_io;
pub(crate) use generators_async_io::*;

unsafe fn asyncio_ready_batch_run_tuple(_py: &PyToken<'_>, handle_tuple_bits: u64) -> Option<i64> {
    unsafe {
        let Some(handle_tuple_ptr) = obj_from_bits(handle_tuple_bits).as_ptr() else {
            let _ = raise_exception::<u64>(
                _py,
                "TypeError",
                "ready-handle collection must be iterable",
            );
            return None;
        };
        let handle_count = crate::object::seq_access::len(handle_tuple_ptr);
        let mut ran_count = 0i64;
        for idx in 0..handle_count {
            let handle = crate::object::seq_access::pin_item(_py, handle_tuple_ptr, idx)?;
            let handle_bits = handle.bits();
            let cancelled = asyncio_method_truthy(_py, handle_bits, b"cancelled")?;
            if cancelled {
                continue;
            }
            let run_bits = asyncio_call_method0(_py, handle_bits, b"_run");
            if exception_pending(_py) {
                return None;
            }
            if !obj_from_bits(run_bits).is_none() {
                dec_ref_bits(_py, run_bits);
            }
            ran_count += 1;
        }
        Some(ran_count)
    }
}

/// # Safety
/// - `handles_bits` must be iterable and contain asyncio Handle-compatible objects.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn molt_asyncio_ready_batch_run(handles_bits: u64) -> u64 {
    unsafe {
        crate::with_gil_entry_nopanic!(_py, {
            let Some(handle_tuple_bits) = tuple_from_iter_bits(_py, handles_bits) else {
                return MoltObject::none().bits();
            };
            let Some(ran_count) = asyncio_ready_batch_run_tuple(_py, handle_tuple_bits) else {
                dec_ref_bits(_py, handle_tuple_bits);
                return MoltObject::none().bits();
            };
            dec_ref_bits(_py, handle_tuple_bits);
            MoltObject::from_int(ran_count).bits()
        })
    }
}

unsafe fn asyncio_loop_enqueue_handle_inner(
    _py: &PyToken<'_>,
    loop_bits: u64,
    ready_lock_bits: u64,
    ready_bits: u64,
    handle_bits: u64,
) -> Option<()> {
    unsafe {
        let acquire_bits = asyncio_call_method0(_py, ready_lock_bits, b"acquire");
        if exception_pending(_py) {
            return None;
        }
        let acquired = is_truthy(_py, obj_from_bits(acquire_bits));
        if !obj_from_bits(acquire_bits).is_none() {
            dec_ref_bits(_py, acquire_bits);
        }
        if !acquired {
            let _ = raise_exception::<u64>(_py, "RuntimeError", "ready queue lock acquire failed");
            return None;
        }

        let append_bits = asyncio_call_method1(_py, ready_bits, b"append", handle_bits);
        let append_failed = exception_pending(_py);
        if !obj_from_bits(append_bits).is_none() {
            dec_ref_bits(_py, append_bits);
        }

        let release_bits = asyncio_call_method0(_py, ready_lock_bits, b"release");
        let release_failed = exception_pending(_py);
        if !obj_from_bits(release_bits).is_none() {
            dec_ref_bits(_py, release_bits);
        }
        if append_failed || release_failed {
            return None;
        }

        let running = asyncio_method_truthy(_py, loop_bits, b"is_running")?;
        if running {
            let ensure_bits = asyncio_call_method0(_py, loop_bits, b"_ensure_ready_runner");
            if exception_pending(_py) {
                return None;
            }
            if !obj_from_bits(ensure_bits).is_none() {
                dec_ref_bits(_py, ensure_bits);
            }
        }
        Some(())
    }
}

/// # Safety
/// - `loop_bits` must expose `is_running()` and `_ensure_ready_runner()`.
/// - `ready_lock_bits` must expose `acquire()`/`release()`.
/// - `ready_bits` must expose `append()`.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn molt_asyncio_loop_enqueue_handle(
    loop_bits: u64,
    ready_lock_bits: u64,
    ready_bits: u64,
    handle_bits: u64,
) -> u64 {
    unsafe {
        crate::with_gil_entry_nopanic!(_py, {
            if asyncio_loop_enqueue_handle_inner(
                _py,
                loop_bits,
                ready_lock_bits,
                ready_bits,
                handle_bits,
            )
            .is_none()
            {
                return MoltObject::none().bits();
            }
            MoltObject::from_int(1).bits()
        })
    }
}

/// # Safety
/// - `ready_lock_bits` must be a lock-like object with `acquire()`/`release()`.
/// - `ready_bits` must be a mutable ready-handle queue.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn molt_asyncio_ready_queue_drain(
    ready_lock_bits: u64,
    ready_bits: u64,
) -> u64 {
    unsafe {
        crate::with_gil_entry_nopanic!(_py, {
            let mut total_ran = 0i64;
            loop {
                let acquire_bits = asyncio_call_method0(_py, ready_lock_bits, b"acquire");
                if exception_pending(_py) {
                    return MoltObject::none().bits();
                }
                let acquired = is_truthy(_py, obj_from_bits(acquire_bits));
                if !obj_from_bits(acquire_bits).is_none() {
                    dec_ref_bits(_py, acquire_bits);
                }
                if !acquired {
                    return raise_exception::<u64>(
                        _py,
                        "RuntimeError",
                        "ready queue lock acquire failed",
                    );
                }

                let len_bits = molt_len(ready_bits);
                if exception_pending(_py) {
                    let release_bits = asyncio_call_method0(_py, ready_lock_bits, b"release");
                    if !obj_from_bits(release_bits).is_none() {
                        dec_ref_bits(_py, release_bits);
                    }
                    return MoltObject::none().bits();
                }
                let Some(ready_len) = to_i64(obj_from_bits(len_bits)) else {
                    if !obj_from_bits(len_bits).is_none() {
                        dec_ref_bits(_py, len_bits);
                    }
                    let release_bits = asyncio_call_method0(_py, ready_lock_bits, b"release");
                    if exception_pending(_py) {
                        if !obj_from_bits(release_bits).is_none() {
                            dec_ref_bits(_py, release_bits);
                        }
                        return MoltObject::none().bits();
                    }
                    if !obj_from_bits(release_bits).is_none() {
                        dec_ref_bits(_py, release_bits);
                    }
                    return raise_exception::<u64>(_py, "TypeError", "ready queue must be sized");
                };
                if !obj_from_bits(len_bits).is_none() {
                    dec_ref_bits(_py, len_bits);
                }
                if ready_len <= 0 {
                    let release_bits = asyncio_call_method0(_py, ready_lock_bits, b"release");
                    if exception_pending(_py) {
                        if !obj_from_bits(release_bits).is_none() {
                            dec_ref_bits(_py, release_bits);
                        }
                        return MoltObject::none().bits();
                    }
                    if !obj_from_bits(release_bits).is_none() {
                        dec_ref_bits(_py, release_bits);
                    }
                    break;
                }

                let Some(handle_tuple_bits) = tuple_from_iter_bits(_py, ready_bits) else {
                    let release_bits = asyncio_call_method0(_py, ready_lock_bits, b"release");
                    if !obj_from_bits(release_bits).is_none() {
                        dec_ref_bits(_py, release_bits);
                    }
                    return MoltObject::none().bits();
                };
                let clear_bits = asyncio_call_method0(_py, ready_bits, b"clear");
                let release_bits = asyncio_call_method0(_py, ready_lock_bits, b"release");
                if exception_pending(_py) {
                    if !obj_from_bits(clear_bits).is_none() {
                        dec_ref_bits(_py, clear_bits);
                    }
                    if !obj_from_bits(release_bits).is_none() {
                        dec_ref_bits(_py, release_bits);
                    }
                    dec_ref_bits(_py, handle_tuple_bits);
                    return MoltObject::none().bits();
                }
                if !obj_from_bits(clear_bits).is_none() {
                    dec_ref_bits(_py, clear_bits);
                }
                if !obj_from_bits(release_bits).is_none() {
                    dec_ref_bits(_py, release_bits);
                }

                let Some(batch_ran) = asyncio_ready_batch_run_tuple(_py, handle_tuple_bits) else {
                    dec_ref_bits(_py, handle_tuple_bits);
                    return MoltObject::none().bits();
                };
                dec_ref_bits(_py, handle_tuple_bits);
                total_ran += batch_ran;
            }
            MoltObject::from_int(total_ran).bits()
        })
    }
}

/// # Safety
/// - `waiters_bits` must be a deque/list-like object supporting pop-front semantics.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn molt_asyncio_waiters_notify(
    waiters_bits: u64,
    count_bits: u64,
    result_bits: u64,
) -> u64 {
    unsafe {
        crate::with_gil_entry_nopanic!(_py, {
            let Some(mut count) = to_i64(obj_from_bits(count_bits)) else {
                return raise_exception::<u64>(_py, "TypeError", "waiter notify count must be int");
            };
            if count <= 0 {
                return MoltObject::from_int(0).bits();
            }
            let len_bits = molt_len(waiters_bits);
            if exception_pending(_py) {
                return MoltObject::none().bits();
            }
            let Some(waiters_len) = to_i64(obj_from_bits(len_bits)) else {
                if !obj_from_bits(len_bits).is_none() {
                    dec_ref_bits(_py, len_bits);
                }
                return raise_exception::<u64>(_py, "TypeError", "waiter collection must be sized");
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
                let Some(done) = asyncio_method_truthy(_py, waiter_bits, b"done") else {
                    if !obj_from_bits(waiter_bits).is_none() {
                        dec_ref_bits(_py, waiter_bits);
                    }
                    return MoltObject::none().bits();
                };
                if !done {
                    let out_bits =
                        asyncio_call_method1(_py, waiter_bits, b"set_result", result_bits);
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
#[unsafe(no_mangle)]
pub unsafe extern "C" fn molt_asyncio_waiters_notify_exception(
    waiters_bits: u64,
    count_bits: u64,
    exc_bits: u64,
) -> u64 {
    unsafe {
        crate::with_gil_entry_nopanic!(_py, {
            let Some(mut count) = to_i64(obj_from_bits(count_bits)) else {
                return raise_exception::<u64>(
                    _py,
                    "TypeError",
                    "waiter notify-exception count must be int",
                );
            };
            if count <= 0 {
                return MoltObject::from_int(0).bits();
            }
            let len_bits = molt_len(waiters_bits);
            if exception_pending(_py) {
                return MoltObject::none().bits();
            }
            let Some(waiters_len) = to_i64(obj_from_bits(len_bits)) else {
                if !obj_from_bits(len_bits).is_none() {
                    dec_ref_bits(_py, len_bits);
                }
                return raise_exception::<u64>(_py, "TypeError", "waiter collection must be sized");
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
                let Some(done) = asyncio_method_truthy(_py, waiter_bits, b"done") else {
                    if !obj_from_bits(waiter_bits).is_none() {
                        dec_ref_bits(_py, waiter_bits);
                    }
                    return MoltObject::none().bits();
                };
                if !done {
                    let out_bits =
                        asyncio_call_method1(_py, waiter_bits, b"set_exception", exc_bits);
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
#[unsafe(no_mangle)]
pub unsafe extern "C" fn molt_asyncio_waiters_remove(waiters_bits: u64, waiter_bits: u64) -> u64 {
    unsafe {
        crate::with_gil_entry_nopanic!(_py, {
            let out_bits = asyncio_call_method1(_py, waiters_bits, b"remove", waiter_bits);
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
#[unsafe(no_mangle)]
pub unsafe extern "C" fn molt_asyncio_condition_wait_for_step(
    condition_bits: u64,
    predicate_bits: u64,
) -> u64 {
    unsafe {
        crate::with_gil_entry_nopanic!(_py, {
            let callable_bits = molt_is_callable(predicate_bits);
            let is_callable = is_truthy(_py, obj_from_bits(callable_bits));
            if !obj_from_bits(callable_bits).is_none() {
                dec_ref_bits(_py, callable_bits);
            }
            if !is_callable {
                return raise_exception::<u64>(_py, "TypeError", "predicate must be callable");
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

            let wait_bits = asyncio_call_method0(_py, condition_bits, b"wait");
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
#[unsafe(no_mangle)]
pub unsafe extern "C" fn molt_asyncio_barrier_release(waiters_bits: u64) -> u64 {
    unsafe {
        crate::with_gil_entry_nopanic!(_py, {
            let Some(waiter_tuple_bits) = tuple_from_iter_bits(_py, waiters_bits) else {
                return MoltObject::none().bits();
            };
            let clear_bits = asyncio_call_method0(_py, waiters_bits, b"clear");
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
                    "TypeError",
                    "barrier waiter collection must be iterable",
                );
            };
            let waiter_count = crate::object::seq_access::len(waiter_tuple_ptr);
            let mut released_count = 0i64;
            for idx in 0..waiter_count {
                let Some(waiter) = crate::object::seq_access::pin_item(_py, waiter_tuple_ptr, idx)
                else {
                    dec_ref_bits(_py, waiter_tuple_bits);
                    return raise_exception::<u64>(_py, "RuntimeError", "invalid waiter state");
                };
                let waiter_bits = waiter.bits();
                let Some(done) = asyncio_method_truthy(_py, waiter_bits, b"done") else {
                    dec_ref_bits(_py, waiter_tuple_bits);
                    return MoltObject::none().bits();
                };
                if done {
                    continue;
                }
                let out_bits = asyncio_call_method1(
                    _py,
                    waiter_bits,
                    b"set_result",
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
        let out_bits = asyncio_call_method1(_py, target_bits, b"set_exception", exc_bits);
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
#[unsafe(no_mangle)]
pub unsafe extern "C" fn molt_asyncio_future_transfer(source_bits: u64, target_bits: u64) -> u64 {
    unsafe {
        crate::with_gil_entry_nopanic!(_py, {
            let Some(target_done) = asyncio_method_truthy(_py, target_bits, b"done") else {
                asyncio_clear_pending_exception(_py);
                return MoltObject::from_bool(false).bits();
            };
            if target_done {
                return MoltObject::from_bool(false).bits();
            }

            let Some(source_cancelled) = asyncio_method_truthy(_py, source_bits, b"cancelled")
            else {
                asyncio_clear_pending_exception(_py);
                return MoltObject::from_bool(false).bits();
            };
            if source_cancelled {
                let cancel_msg_ref =
                    asyncio_attr_lookup_allow_missing(_py, source_bits, b"_cancel_message");
                let cancel_msg_bits = cancel_msg_ref.unwrap_or_else(|| MoltObject::none().bits());
                let out_bits = asyncio_call_method1(_py, target_bits, b"cancel", cancel_msg_bits);
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

            let source_exc_bits = asyncio_call_method0(_py, source_bits, b"exception");
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

            let result_bits = asyncio_call_method0(_py, source_bits, b"result");
            if exception_pending(_py) {
                asyncio_clear_pending_exception(_py);
                return MoltObject::from_bool(false).bits();
            }
            let out_bits = asyncio_call_method1(_py, target_bits, b"set_result", result_bits);
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
#[unsafe(no_mangle)]
pub unsafe extern "C" fn molt_asyncio_event_waiters_cleanup(waiters_bits: u64) -> u64 {
    unsafe {
        crate::with_gil_entry_nopanic!(_py, {
            let Some(waiter_tuple_bits) = tuple_from_iter_bits(_py, waiters_bits) else {
                return MoltObject::none().bits();
            };
            let Some(waiter_tuple_ptr) = obj_from_bits(waiter_tuple_bits).as_ptr() else {
                dec_ref_bits(_py, waiter_tuple_bits);
                return raise_exception::<u64>(_py, "TypeError", "event waiters must be iterable");
            };
            let waiter_count = crate::object::seq_access::len(waiter_tuple_ptr);
            let mut cleaned = 0i64;
            for idx in 0..waiter_count {
                let Some(waiter) = crate::object::seq_access::pin_item(_py, waiter_tuple_ptr, idx)
                else {
                    dec_ref_bits(_py, waiter_tuple_bits);
                    return raise_exception::<u64>(_py, "RuntimeError", "invalid waiter state");
                };
                let waiter_bits = waiter.bits();
                let Some(owner_bits) =
                    asyncio_attr_lookup_allow_missing(_py, waiter_bits, b"_molt_event_owner")
                else {
                    continue;
                };
                if obj_from_bits(owner_bits).is_none() {
                    dec_ref_bits(_py, owner_bits);
                    continue;
                }
                let Some(owner_waiters_bits) =
                    asyncio_attr_lookup_allow_missing(_py, owner_bits, b"_waiters")
                else {
                    dec_ref_bits(_py, owner_bits);
                    continue;
                };
                let out_bits =
                    asyncio_call_method1(_py, owner_waiters_bits, b"remove", waiter_bits);
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
/// - `tasks_bits` must be a mutable task set.
/// - `errors_bits` must be an appendable error list.
/// - `task_bits` must be a task/future object.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn molt_asyncio_taskgroup_on_task_done(
    tasks_bits: u64,
    errors_bits: u64,
    task_bits: u64,
) -> u64 {
    unsafe {
        crate::with_gil_entry_nopanic!(_py, {
            let contains_bits = asyncio_call_method1(_py, tasks_bits, b"__contains__", task_bits);
            if exception_pending(_py) {
                return MoltObject::none().bits();
            }
            let in_group = is_truthy(_py, obj_from_bits(contains_bits));
            if !obj_from_bits(contains_bits).is_none() {
                dec_ref_bits(_py, contains_bits);
            }
            if !in_group {
                return MoltObject::from_bool(false).bits();
            }

            let discard_bits = asyncio_call_method1(_py, tasks_bits, b"discard", task_bits);
            if exception_pending(_py) {
                return MoltObject::none().bits();
            }
            if !obj_from_bits(discard_bits).is_none() {
                dec_ref_bits(_py, discard_bits);
            }

            let Some(mut should_cancel) =
                asyncio_taskgroup_collect_task_error(_py, errors_bits, task_bits)
            else {
                return MoltObject::none().bits();
            };
            if !should_cancel {
                return MoltObject::from_bool(false).bits();
            }

            let Some(task_tuple_bits) = tuple_from_iter_bits(_py, tasks_bits) else {
                return MoltObject::none().bits();
            };
            let Some(task_tuple_ptr) = obj_from_bits(task_tuple_bits).as_ptr() else {
                dec_ref_bits(_py, task_tuple_bits);
                return raise_exception::<u64>(_py, "TypeError", "task group must be iterable");
            };
            let task_count = crate::object::seq_access::len(task_tuple_ptr);
            for idx in 0..task_count {
                let Some(other_task) =
                    crate::object::seq_access::pin_item(_py, task_tuple_ptr, idx)
                else {
                    dec_ref_bits(_py, task_tuple_bits);
                    return raise_exception::<u64>(_py, "RuntimeError", "invalid task group state");
                };
                let other_task_bits = other_task.bits();
                let Some(done) = asyncio_method_truthy(_py, other_task_bits, b"done") else {
                    dec_ref_bits(_py, task_tuple_bits);
                    return MoltObject::none().bits();
                };
                if !done {
                    continue;
                }
                let discard_bits =
                    asyncio_call_method1(_py, tasks_bits, b"discard", other_task_bits);
                if exception_pending(_py) {
                    dec_ref_bits(_py, task_tuple_bits);
                    return MoltObject::none().bits();
                }
                if !obj_from_bits(discard_bits).is_none() {
                    dec_ref_bits(_py, discard_bits);
                }
                let Some(collected_error) =
                    asyncio_taskgroup_collect_task_error(_py, errors_bits, other_task_bits)
                else {
                    dec_ref_bits(_py, task_tuple_bits);
                    return MoltObject::none().bits();
                };
                should_cancel |= collected_error;
            }
            dec_ref_bits(_py, task_tuple_bits);
            MoltObject::from_bool(should_cancel).bits()
        })
    }
}

/// # Safety
/// - `cancel_callback_bits` must be callable.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn molt_asyncio_taskgroup_request_cancel(
    loop_bits: u64,
    cancel_callback_bits: u64,
    cancel_handle_bits: u64,
) -> u64 {
    unsafe {
        crate::with_gil_entry_nopanic!(_py, {
            if !obj_from_bits(cancel_handle_bits).is_none() {
                return cancel_handle_bits;
            }
            if obj_from_bits(loop_bits).is_none() {
                let out_bits = call_callable0(_py, cancel_callback_bits);
                if exception_pending(_py) {
                    return out_bits;
                }
                if !obj_from_bits(out_bits).is_none() {
                    dec_ref_bits(_py, out_bits);
                }
                return MoltObject::none().bits();
            }
            let out_bits = asyncio_call_method1(_py, loop_bits, b"call_soon", cancel_callback_bits);
            if exception_pending(_py) {
                return out_bits;
            }
            out_bits
        })
    }
}

/// # Safety
/// - `tasks_bits` must be iterable and contain Future-like objects.
/// - `callback_bits` must be callable.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn molt_asyncio_tasks_add_done_callback(
    tasks_bits: u64,
    callback_bits: u64,
) -> u64 {
    unsafe {
        crate::with_gil_entry_nopanic!(_py, {
            let callable_bits = molt_is_callable(callback_bits);
            let is_callable = is_truthy(_py, obj_from_bits(callable_bits));
            if !obj_from_bits(callable_bits).is_none() {
                dec_ref_bits(_py, callable_bits);
            }
            if !is_callable {
                return raise_exception::<u64>(_py, "TypeError", "callback must be callable");
            }
            let Some(task_tuple_bits) = tuple_from_iter_bits(_py, tasks_bits) else {
                return MoltObject::none().bits();
            };
            let Some(task_tuple_ptr) = obj_from_bits(task_tuple_bits).as_ptr() else {
                dec_ref_bits(_py, task_tuple_bits);
                return raise_exception::<u64>(_py, "TypeError", "tasks must be iterable");
            };
            let task_count = crate::object::seq_access::len(task_tuple_ptr);
            let mut attached = 0i64;
            for idx in 0..task_count {
                let Some(task) = crate::object::seq_access::pin_item(_py, task_tuple_ptr, idx)
                else {
                    dec_ref_bits(_py, task_tuple_bits);
                    return raise_exception::<u64>(_py, "RuntimeError", "invalid task state");
                };
                let task_bits = task.bits();
                let out_bits =
                    asyncio_call_method1(_py, task_bits, b"add_done_callback", callback_bits);
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
#[unsafe(no_mangle)]
pub unsafe extern "C" fn molt_asyncio_task_uncancel_apply(future_bits: u64) -> u64 {
    crate::with_gil_entry_nopanic!(_py, {
        let Some(task_ptr) = resolve_task_ptr(future_bits) else {
            return raise_exception::<_>(_py, "TypeError", "object is not awaitable");
        };
        task_cancel_message_clear(_py, task_ptr);
        let _ = task_take_cancel_pending(task_ptr);
        MoltObject::none().bits()
    })
}

/// # Safety
/// - `future_bits` must be a Future-like object exposing `_run_callback`.
/// - `callbacks_bits` must be iterable of `(callback, context)` pairs.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn molt_asyncio_future_invoke_callbacks(
    future_bits: u64,
    callbacks_bits: u64,
) -> u64 {
    unsafe {
        crate::with_gil_entry_nopanic!(_py, {
            let trace = matches!(
                std::env::var("MOLT_TRACE_ASYNCIO_CALLBACKS")
                    .ok()
                    .as_deref(),
                Some("1")
            );
            let Some(callback_tuple_bits) = tuple_from_iter_bits(_py, callbacks_bits) else {
                return MoltObject::none().bits();
            };
            let Some(callback_tuple_ptr) = obj_from_bits(callback_tuple_bits).as_ptr() else {
                dec_ref_bits(_py, callback_tuple_bits);
                return raise_exception::<u64>(
                    _py,
                    "TypeError",
                    "future callbacks must be iterable",
                );
            };
            let callback_count = crate::object::seq_access::len(callback_tuple_ptr);
            if trace {
                eprintln!(
                    "molt asyncio callbacks future=0x{:x} count={}",
                    future_bits, callback_count
                );
            }
            let idx0 = MoltObject::from_int(0).bits();
            let idx1 = MoltObject::from_int(1).bits();
            let mut called = 0i64;
            for idx in 0..callback_count {
                let Some(entry) = crate::object::seq_access::pin_item(_py, callback_tuple_ptr, idx)
                else {
                    dec_ref_bits(_py, callback_tuple_bits);
                    return raise_exception::<u64>(_py, "RuntimeError", "invalid callback state");
                };
                let entry_bits = entry.bits();
                let fn_bits = molt_getitem_method(entry_bits, idx0);
                if exception_pending(_py) {
                    dec_ref_bits(_py, callback_tuple_bits);
                    return MoltObject::none().bits();
                }
                let ctx_bits = molt_getitem_method(entry_bits, idx1);
                if exception_pending(_py) {
                    if !obj_from_bits(fn_bits).is_none() {
                        dec_ref_bits(_py, fn_bits);
                    }
                    dec_ref_bits(_py, callback_tuple_bits);
                    return MoltObject::none().bits();
                }
                if trace {
                    eprintln!(
                        "molt asyncio callback entry fn_type={} ctx_type={}",
                        crate::type_name(_py, obj_from_bits(fn_bits)),
                        crate::type_name(_py, obj_from_bits(ctx_bits))
                    );
                }
                let out_bits =
                    asyncio_call_method2(_py, future_bits, b"_run_callback", fn_bits, ctx_bits);
                if !obj_from_bits(fn_bits).is_none() {
                    dec_ref_bits(_py, fn_bits);
                }
                if !obj_from_bits(ctx_bits).is_none() {
                    dec_ref_bits(_py, ctx_bits);
                }
                if exception_pending(_py) {
                    dec_ref_bits(_py, callback_tuple_bits);
                    return out_bits;
                }
                if !obj_from_bits(out_bits).is_none() {
                    dec_ref_bits(_py, out_bits);
                }
                called += 1;
            }
            dec_ref_bits(_py, callback_tuple_bits);
            MoltObject::from_int(called).bits()
        })
    }
}

/// # Safety
/// - `waiters_bits` must be iterable of Event waiters.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn molt_asyncio_event_set_waiters(
    waiters_bits: u64,
    result_bits: u64,
) -> u64 {
    unsafe {
        crate::with_gil_entry_nopanic!(_py, {
            let Some(waiter_tuple_bits) = tuple_from_iter_bits(_py, waiters_bits) else {
                return MoltObject::none().bits();
            };
            let Some(waiter_tuple_ptr) = obj_from_bits(waiter_tuple_bits).as_ptr() else {
                dec_ref_bits(_py, waiter_tuple_bits);
                return raise_exception::<u64>(_py, "TypeError", "event waiters must be iterable");
            };
            let waiter_count = crate::object::seq_access::len(waiter_tuple_ptr);
            let mut woke = 0i64;
            for idx in 0..waiter_count {
                let Some(waiter) = crate::object::seq_access::pin_item(_py, waiter_tuple_ptr, idx)
                else {
                    dec_ref_bits(_py, waiter_tuple_bits);
                    return raise_exception::<u64>(_py, "RuntimeError", "invalid waiter state");
                };
                let waiter_bits = waiter.bits();
                if let Some(token_bits) =
                    asyncio_attr_lookup_allow_missing(_py, waiter_bits, b"_molt_event_token_id")
                {
                    if to_i64(obj_from_bits(token_bits)).is_some() {
                        let out =
                            crate::molt_asyncio_event_waiters_unregister(token_bits, waiter_bits);
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
                let out_bits = asyncio_call_method1(_py, waiter_bits, b"set_result", result_bits);
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

#[cfg(test)]
mod tests {
    use super::{molt_asyncgen_new, molt_generator_new};
    use crate::{GEN_CONTROL_SIZE, asyncgen_registry, dec_ref_bits, obj_from_bits};

    #[test]
    fn asyncgen_registry_removes_on_drop() {
        let _guard = crate::test_support::RuntimeTestTransaction::new();
        crate::with_gil_entry_nopanic!(_py, {
            {
                let mut guard = asyncgen_registry(_py).lock().unwrap();
                guard.clear();
            }
            let gen_bits = molt_generator_new(0, GEN_CONTROL_SIZE as u64);
            assert!(
                !obj_from_bits(gen_bits).is_none(),
                "generator allocation failed"
            );
            let asyncgen_bits = molt_asyncgen_new(gen_bits);
            assert!(
                !obj_from_bits(asyncgen_bits).is_none(),
                "async generator allocation failed"
            );
            let len = asyncgen_registry(_py).lock().unwrap().len();
            assert_eq!(len, 1);
            dec_ref_bits(_py, asyncgen_bits);
            let len_after = asyncgen_registry(_py).lock().unwrap().len();
            assert_eq!(len_after, 0);
            dec_ref_bits(_py, gen_bits);
        });
    }
}
