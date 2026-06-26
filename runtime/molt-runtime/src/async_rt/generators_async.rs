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

#[path = "generators_async_io.rs"]
mod generators_async_io;
pub(crate) use generators_async_io::*;

const ASYNC_SLEEP_YIELD_SECS: f64 = 0.000_001;
const ASYNC_SLEEP_YIELD_SENTINEL: f64 = -1.0;
const ASYNCIO_WAIT_RETURN_ALL_COMPLETED: i64 = 0;
const ASYNCIO_WAIT_RETURN_FIRST_COMPLETED: i64 = 1;
const ASYNCIO_WAIT_RETURN_FIRST_EXCEPTION: i64 = 2;
const ASYNCIO_WAIT_FLAG_HAS_TIMER: i64 = 1;
const ASYNCIO_WAIT_FLAG_TIMEOUT_READY: i64 = 2;
const ASYNCIO_WAIT_FLAG_TIMEOUT_DEFERRED: i64 = 4;
const ASYNCIO_WAIT_FLAG_TIMEOUT_DEFERRED_2: i64 = 8;
const ASYNCIO_GATHER_RESULT_OFFSET: usize = 4;
const WAIT_FOR_STATE_PENDING: i64 = 1;
const WAIT_FOR_STATE_CANCEL_WAIT: i64 = 2;
const WAIT_FOR_FLAG_HAS_TIMER: i64 = 1;
const WAIT_FOR_FLAG_FORCE_TIMEOUT: i64 = 2;

#[unsafe(no_mangle)]
pub extern "C" fn molt_future_poll_fn(future_bits: u64) -> u64 {
    crate::with_gil_entry_nopanic!(_py, {
        let obj = obj_from_bits(future_bits);
        let Some(ptr) = obj.as_ptr() else {
            if std::env::var("MOLT_DEBUG_AWAITABLE").is_ok() {
                eprintln!(
                    "Molt awaitable debug: bits=0x{:x} type={}",
                    future_bits,
                    type_name(_py, obj)
                );
            }
            raise_exception::<()>(_py, "TypeError", "object is not awaitable");
            return 0;
        };
        unsafe {
            let _gil = GilGuard::new();
            let header = header_from_obj_ptr(ptr);
            let poll_fn_addr = crate::object::object_poll_fn(ptr);
            if poll_fn_addr == 0 {
                if std::env::var("MOLT_DEBUG_AWAITABLE").is_ok() {
                    let mut class_name = None;
                    if object_type_id(ptr) == TYPE_ID_OBJECT {
                        let class_bits = object_class_bits(ptr);
                        if class_bits != 0 {
                            class_name = Some(class_name_for_error(class_bits));
                        }
                    }
                    eprintln!(
                        "Molt awaitable debug: bits=0x{:x} type={} class={} poll=0x0 state={} size={}",
                        future_bits,
                        type_name(_py, obj),
                        class_name.as_deref().unwrap_or("-"),
                        crate::object::object_state(ptr),
                        crate::object::total_size_from_header(&*header, ptr)
                    );
                }
                raise_exception::<()>(_py, "TypeError", "object is not awaitable");
                return 0;
            }
            poll_fn_addr
        }
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_future_poll(future_bits: u64) -> i64 {
    crate::with_gil_entry_nopanic!(_py, {
        let obj = obj_from_bits(future_bits);
        let Some(ptr) = obj.as_ptr() else {
            if std::env::var("MOLT_DEBUG_AWAITABLE").is_ok() {
                eprintln!(
                    "Molt awaitable debug: poll bits=0x{:x} type={}",
                    future_bits,
                    type_name(_py, obj)
                );
            }
            raise_exception::<i64>(_py, "TypeError", "object is not awaitable");
            return 0;
        };
        unsafe {
            let header = header_from_obj_ptr(ptr);
            let poll_fn_addr = crate::object::object_poll_fn(ptr);
            if poll_fn_addr == 0 {
                if std::env::var("MOLT_DEBUG_AWAITABLE").is_ok() {
                    let mut class_name = None;
                    if object_type_id(ptr) == TYPE_ID_OBJECT {
                        let class_bits = object_class_bits(ptr);
                        if class_bits != 0 {
                            class_name = Some(class_name_for_error(class_bits));
                        }
                    }
                    eprintln!(
                        "Molt awaitable debug: poll bits=0x{:x} type={} class={} poll=0x0 state={} size={}",
                        future_bits,
                        type_name(_py, obj),
                        class_name.as_deref().unwrap_or("-"),
                        crate::object::object_state(ptr),
                        crate::object::total_size_from_header(&*header, ptr)
                    );
                }
                raise_exception::<i64>(_py, "TypeError", "object is not awaitable");
                return 0;
            }
            if ((*header).flags & HEADER_FLAG_TASK_DONE) != 0 {
                if let Some(result_bits) = task_result_get(_py, ptr) {
                    return result_bits as i64;
                }
                let cached_exception = {
                    let guard = task_last_exceptions(_py).lock().unwrap();
                    guard.get(&PtrSlot(ptr)).copied()
                };
                if let Some(exc_ptr) = cached_exception {
                    let exc_bits = MoltObject::from_ptr(exc_ptr.0).bits();
                    inc_ref_bits(_py, exc_bits);
                    let raised = molt_raise(exc_bits);
                    dec_ref_bits(_py, exc_bits);
                    return raised as i64;
                }
                return MoltObject::none().bits() as i64;
            }
            if ((*header).flags & HEADER_FLAG_COROUTINE) != 0
                && crate::object::object_state(ptr) == 0
                && task_cancel_pending(ptr)
            {
                task_take_cancel_pending(ptr);
                task_mark_done(_py, ptr);
                return raise_cancelled_with_message::<i64>(_py, ptr);
            }
            let res = crate::poll_future_with_task_stack(_py, ptr, poll_fn_addr);
            if trace_task_result() {
                eprintln!(
                    "molt task_result poll ptr=0x{:x} res=0x{:x} pending={} done_before=false",
                    ptr as usize,
                    res as u64,
                    res == pending_bits_i64()
                );
            }
            if promise_trace_enabled() && poll_fn_addr == promise_poll_fn_addr() {
                let state = crate::object::object_state(ptr);
                eprintln!(
                    "molt async trace: promise_poll task=0x{:x} state={} res=0x{:x}",
                    ptr as usize, state, res as u64
                );
            }
            if task_cancel_pending(ptr) {
                task_take_cancel_pending(ptr);
                return raise_cancelled_with_message::<i64>(_py, ptr);
            }
            let current_task = current_task_ptr();
            if res == pending_bits_i64() {
                if !current_task.is_null() && ptr != current_task {
                    await_waiter_register(_py, current_task, ptr);
                    let current_header = header_from_obj_ptr(current_task);
                    let is_block_on = ((*current_header).flags & HEADER_FLAG_BLOCK_ON) != 0;
                    let is_spawned = ((*current_header).flags & HEADER_FLAG_SPAWN_RETAIN) != 0;
                    if is_block_on || is_spawned {
                        let sleep_target = resolve_sleep_target(_py, ptr);
                        let _ = sleep_register_impl(_py, current_task, sleep_target);
                    }
                }
            } else if !current_task.is_null() {
                await_waiter_clear(_py, current_task);
            }
            if !current_task.is_null() {
                let current_cancelled = task_cancel_pending(current_task);
                if current_cancelled {
                    task_take_cancel_pending(current_task);
                    return raise_cancelled_with_message::<i64>(_py, current_task);
                }
            }
            let awaited_exception =
                if res != pending_bits_i64() && !current_task.is_null() && ptr != current_task {
                    let guard = task_last_exceptions(_py).lock().unwrap();
                    guard.get(&PtrSlot(ptr)).copied()
                } else {
                    None
                };
            let poll_pending = exception_pending(_py) || awaited_exception.is_some();
            if res != pending_bits_i64() {
                if !poll_pending {
                    crate::task_last_exception_drop(_py, ptr);
                    task_result_store(_py, ptr, res as u64);
                } else {
                    task_result_drop(_py, ptr);
                }
                task_mark_done(_py, ptr);
            }
            if res != pending_bits_i64()
                && poll_pending
                && !current_task.is_null()
                && ptr != current_task
            {
                if let Some(exc_ptr) = awaited_exception {
                    let exc_bits = MoltObject::from_ptr(exc_ptr.0).bits();
                    inc_ref_bits(_py, exc_bits);
                    let raised = molt_raise(exc_bits);
                    dec_ref_bits(_py, exc_bits);
                    return raised as i64;
                } else {
                    let prev_task = crate::CURRENT_TASK.with(|cell| {
                        let prev = cell.get();
                        cell.set(ptr);
                        prev
                    });
                    let exc_bits = if exception_pending(_py) {
                        molt_exception_last()
                    } else {
                        MoltObject::none().bits()
                    };
                    if debug_current_task() && prev_task.is_null() {
                        let current = crate::CURRENT_TASK.with(|cell| cell.get());
                        if !current.is_null() {
                            eprintln!(
                                "molt task trace: generators restore null current=0x{:x} task=0x{:x}",
                                current as usize, ptr as usize
                            );
                        }
                    }
                    crate::CURRENT_TASK.with(|cell| cell.set(prev_task));
                    if obj_from_bits(exc_bits).is_none() {
                        if let Some(exc_bits) = crate::global_last_exception_bits_noinc(_py) {
                            inc_ref_bits(_py, exc_bits);
                            let raised = molt_raise(exc_bits);
                            dec_ref_bits(_py, exc_bits);
                            clear_exception_state(_py);
                            return raised as i64;
                        }
                    } else {
                        let raised = molt_raise(exc_bits);
                        dec_ref_bits(_py, exc_bits);
                        return raised as i64;
                    }
                }
            }
            if res != pending_bits_i64() && !task_has_token(_py, ptr) {
                task_exception_stack_drop(_py, ptr);
                task_exception_depth_drop(_py, ptr);
                task_exception_baseline_drop(_py, ptr);
            }
            res
        }
    })
}

pub(crate) fn cancel_future_task(_py: &PyToken<'_>, task_ptr: *mut u8, msg_bits: Option<u64>) {
    if task_ptr.is_null() {
        return;
    }
    if async_trace_enabled() {
        eprintln!(
            "molt async trace: cancel_future task=0x{:x}",
            task_ptr as usize
        );
    }
    match msg_bits {
        Some(bits) => task_cancel_message_set(_py, task_ptr, bits),
        None => task_cancel_message_clear(_py, task_ptr),
    }
    task_set_cancel_pending(task_ptr);
    let awaited_ptr = {
        let waiting_map = task_waiting_on(_py).lock().unwrap();
        waiting_map.get(&PtrSlot(task_ptr)).map(|val| val.0)
    };
    if let Some(awaited_ptr) = awaited_ptr {
        if async_trace_enabled() {
            eprintln!(
                "molt async trace: cancel_future_waiting task=0x{:x} awaited=0x{:x}",
                task_ptr as usize, awaited_ptr as usize
            );
        }
        if !awaited_ptr.is_null() {
            let sleep_target = resolve_sleep_target(_py, awaited_ptr);
            if !sleep_target.is_null() {
                let poll_fn = crate::object::object_poll_fn(sleep_target);
                if poll_fn == io_wait_poll_fn_addr() {
                    #[cfg(not(target_arch = "wasm32"))]
                    runtime_state(_py).io_poller().cancel_waiter(sleep_target);
                }
            }
        }
    }
    await_waiter_clear(_py, task_ptr);
    unsafe {
        let _header = header_from_obj_ptr(task_ptr);
        let poll_fn = crate::object::object_poll_fn(task_ptr);
        if poll_fn == thread_poll_fn_addr() {
            #[cfg(not(target_arch = "wasm32"))]
            if let Some(state) = thread_task_state(_py, task_ptr) {
                state.cancelled.store(true, AtomicOrdering::Release);
                state.condvar.notify_all();
            }
        }
        if poll_fn == process_poll_fn_addr() {
            #[cfg(not(target_arch = "wasm32"))]
            if let Some(state) = process_task_state(_py, task_ptr) {
                state.cancelled.store(true, AtomicOrdering::Release);
                state.process.condvar.notify_all();
            }
        }
        if poll_fn == io_wait_poll_fn_addr() {
            #[cfg(not(target_arch = "wasm32"))]
            runtime_state(_py).io_poller().cancel_waiter(task_ptr);
        }
    }
    let waiter_count = wake_await_waiters(_py, task_ptr);
    if async_trace_enabled() {
        eprintln!(
            "molt async trace: cancel_future_waiters task=0x{:x} count={}",
            task_ptr as usize, waiter_count
        );
    }
    wake_task_ptr(_py, task_ptr);
}

fn sleep_register_impl(_py: &PyToken<'_>, task_ptr: *mut u8, future_ptr: *mut u8) -> bool {
    if async_trace_enabled() || sleep_trace_enabled() {
        eprintln!(
            "molt async trace: sleep_register_impl_enter task=0x{:x} future=0x{:x}",
            task_ptr as usize, future_ptr as usize
        );
    }
    if future_ptr.is_null() {
        if async_trace_enabled() || sleep_trace_enabled() {
            eprintln!("molt async trace: sleep_register_impl_fail future_null");
        }
        return false;
    }
    let mut resolved_task = task_ptr;
    if resolved_task.is_null() {
        resolved_task = await_waiters(_py)
            .lock()
            .unwrap()
            .get(&PtrSlot(future_ptr))
            .and_then(|list| list.first().copied())
            .map(|waiter| waiter.0)
            .unwrap_or(std::ptr::null_mut());
    }
    if resolved_task.is_null() {
        if async_trace_enabled() || sleep_trace_enabled() {
            eprintln!(
                "molt async trace: sleep_register_impl_fail task_null task=0x{:x} future=0x{:x}",
                task_ptr as usize, future_ptr as usize
            );
        }
        return false;
    }
    let task_ptr = resolved_task;
    let _header = unsafe { header_from_obj_ptr(future_ptr) };
    let poll_fn = crate::object::object_poll_fn(future_ptr);
    if poll_fn != async_sleep_poll_fn_addr() && poll_fn != io_wait_poll_fn_addr() {
        if async_trace_enabled() || sleep_trace_enabled() {
            eprintln!(
                "molt async trace: sleep_register_impl_fail poll_fn=0x{:x}",
                poll_fn
            );
        }
        return false;
    }
    if crate::object::object_state(future_ptr) == 0 {
        if async_trace_enabled() || sleep_trace_enabled() {
            eprintln!("molt async trace: sleep_register_impl_fail state=0");
        }
        return false;
    }
    let payload_bytes = unsafe { crate::object::object_payload_size(future_ptr) };
    let payload_ptr = future_ptr as *mut u64;
    let deadline_obj = if poll_fn == async_sleep_poll_fn_addr() {
        if payload_bytes < std::mem::size_of::<u64>() {
            return false;
        }
        obj_from_bits(unsafe { *payload_ptr })
    } else {
        if payload_bytes < 3 * std::mem::size_of::<u64>() {
            return false;
        }
        obj_from_bits(unsafe { *payload_ptr.add(2) })
    };
    if poll_fn == io_wait_poll_fn_addr() && deadline_obj.is_none() {
        // I/O waits without a timeout rely on the poller to wake the task.
        return true;
    }
    let Some(deadline_secs) = to_f64(deadline_obj) else {
        if async_trace_enabled() || sleep_trace_enabled() {
            eprintln!("molt async trace: sleep_register_impl_fail deadline_nan");
        }
        return false;
    };
    if !deadline_secs.is_finite() {
        if async_trace_enabled() || sleep_trace_enabled() {
            eprintln!(
                "molt async trace: sleep_register_impl_fail deadline_secs={}",
                deadline_secs
            );
        }
        return false;
    }
    if poll_fn == async_sleep_poll_fn_addr() && deadline_secs < 0.0 {
        if async_trace_enabled() || sleep_trace_enabled() {
            eprintln!(
                "molt async trace: sleep_register_yield task=0x{:x}",
                task_ptr as usize
            );
        }
        let task_header = unsafe { header_from_obj_ptr(task_ptr) };
        if unsafe { ((*task_header).flags & HEADER_FLAG_BLOCK_ON) != 0 } {
            let deadline =
                Instant::now() + Duration::from_secs_f64(ASYNC_SLEEP_YIELD_SECS.max(0.0));
            runtime_state(_py)
                .sleep_queue()
                .register_blocking(_py, task_ptr, deadline);
            return true;
        }
        runtime_state(_py).scheduler().defer_task_ptr(task_ptr);
        return true;
    }
    if deadline_secs <= 0.0 {
        if async_trace_enabled() || sleep_trace_enabled() {
            eprintln!(
                "molt async trace: sleep_register_immediate task=0x{:x} deadline_secs={}",
                task_ptr as usize, deadline_secs
            );
        }
        if poll_fn == async_sleep_poll_fn_addr() {
            let deadline =
                Instant::now() + Duration::from_secs_f64(ASYNC_SLEEP_YIELD_SECS.max(0.0));
            let task_header = unsafe { header_from_obj_ptr(task_ptr) };
            if unsafe { ((*task_header).flags & HEADER_FLAG_BLOCK_ON) != 0 } {
                runtime_state(_py)
                    .sleep_queue()
                    .register_blocking(_py, task_ptr, deadline);
                return true;
            }
            #[cfg(target_arch = "wasm32")]
            {
                runtime_state(_py)
                    .sleep_queue()
                    .register_blocking(_py, task_ptr, deadline);
                return true;
            }
            #[cfg(not(target_arch = "wasm32"))]
            {
                if is_block_on_task(task_ptr) {
                    runtime_state(_py)
                        .sleep_queue()
                        .register_blocking(_py, task_ptr, deadline);
                } else {
                    runtime_state(_py)
                        .sleep_queue()
                        .register_scheduler(_py, task_ptr, deadline);
                }
                return true;
            }
        }
        wake_task_ptr(_py, task_ptr);
        return true;
    }
    let deadline = instant_from_monotonic_secs(_py, deadline_secs);
    if deadline <= Instant::now() {
        if async_trace_enabled() || sleep_trace_enabled() {
            eprintln!(
                "molt async trace: sleep_register_immediate_elapsed task=0x{:x}",
                task_ptr as usize
            );
        }
        if poll_fn == async_sleep_poll_fn_addr() {
            let deadline =
                Instant::now() + Duration::from_secs_f64(ASYNC_SLEEP_YIELD_SECS.max(0.0));
            let task_header = unsafe { header_from_obj_ptr(task_ptr) };
            if unsafe { ((*task_header).flags & HEADER_FLAG_BLOCK_ON) != 0 } {
                runtime_state(_py)
                    .sleep_queue()
                    .register_blocking(_py, task_ptr, deadline);
                return true;
            }
            #[cfg(target_arch = "wasm32")]
            {
                runtime_state(_py)
                    .sleep_queue()
                    .register_blocking(_py, task_ptr, deadline);
                return true;
            }
            #[cfg(not(target_arch = "wasm32"))]
            {
                if is_block_on_task(task_ptr) {
                    runtime_state(_py)
                        .sleep_queue()
                        .register_blocking(_py, task_ptr, deadline);
                } else {
                    runtime_state(_py)
                        .sleep_queue()
                        .register_scheduler(_py, task_ptr, deadline);
                }
                return true;
            }
        }
        wake_task_ptr(_py, task_ptr);
        return true;
    }
    let task_header = unsafe { header_from_obj_ptr(task_ptr) };
    if unsafe { ((*task_header).flags & HEADER_FLAG_BLOCK_ON) != 0 } {
        runtime_state(_py)
            .sleep_queue()
            .register_blocking(_py, task_ptr, deadline);
        return true;
    }
    #[cfg(target_arch = "wasm32")]
    {
        runtime_state(_py)
            .sleep_queue()
            .register_blocking(_py, task_ptr, deadline);
        true
    }
    #[cfg(not(target_arch = "wasm32"))]
    {
        if is_block_on_task(task_ptr) {
            runtime_state(_py)
                .sleep_queue()
                .register_blocking(_py, task_ptr, deadline);
        } else {
            runtime_state(_py)
                .sleep_queue()
                .register_scheduler(_py, task_ptr, deadline);
        }
        if async_trace_enabled() {
            let delay = deadline.saturating_duration_since(Instant::now());
            eprintln!(
                "molt async trace: sleep_register_request task=0x{:x} deadline_secs={} delay_ms={}",
                task_ptr as usize,
                deadline_secs,
                delay.as_secs_f64() * 1000.0
            );
        }
        true
    }
}

/// # Safety
/// - `future_bits` must be a valid pointer to a Molt future.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn molt_future_cancel(future_bits: u64) -> u64 {
    crate::with_gil_entry_nopanic!(_py, {
        let Some(task_ptr) = resolve_task_ptr(future_bits) else {
            return raise_exception::<_>(_py, "TypeError", "object is not awaitable");
        };
        cancel_future_task(_py, task_ptr, None);
        MoltObject::none().bits()
    })
}

/// # Safety
/// - `future_bits` must be a valid pointer to a Molt future.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn molt_future_cancel_msg(future_bits: u64, msg_bits: u64) -> u64 {
    crate::with_gil_entry_nopanic!(_py, {
        let Some(task_ptr) = resolve_task_ptr(future_bits) else {
            return raise_exception::<_>(_py, "TypeError", "object is not awaitable");
        };
        cancel_future_task(_py, task_ptr, Some(msg_bits));
        MoltObject::none().bits()
    })
}

/// # Safety
/// - `future_bits` must be a valid pointer to a Molt future.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn molt_future_cancel_clear(future_bits: u64) -> u64 {
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
/// - `future_bits` must be a valid pointer to a Molt future.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn molt_asyncio_task_cancel_apply(future_bits: u64, msg_bits: u64) -> u64 {
    crate::with_gil_entry_nopanic!(_py, {
        let Some(task_ptr) = resolve_task_ptr(future_bits) else {
            return raise_exception::<_>(_py, "TypeError", "object is not awaitable");
        };
        if obj_from_bits(msg_bits).is_none() {
            cancel_future_task(_py, task_ptr, None);
        } else {
            cancel_future_task(_py, task_ptr, Some(msg_bits));
        }
        MoltObject::from_bool(true).bits()
    })
}

/// # Safety
/// - `tasks_bits` must be iterable and contain awaitables with `done()`/`cancel()`.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn molt_asyncio_cancel_pending(tasks_bits: u64) -> u64 {
    unsafe {
        crate::with_gil_entry_nopanic!(_py, {
            let Some(task_tuple_bits) = tuple_from_iter_bits(_py, tasks_bits) else {
                return MoltObject::none().bits();
            };
            let Some(task_tuple_ptr) = obj_from_bits(task_tuple_bits).as_ptr() else {
                dec_ref_bits(_py, task_tuple_bits);
                return raise_exception::<u64>(
                    _py,
                    "TypeError",
                    "task collection must be awaitables",
                );
            };
            let tasks = seq_vec_ref(task_tuple_ptr);
            let mut cancelled_count = 0i64;
            for &task_bits in tasks {
                let Some(done) = asyncio_method_truthy(_py, task_bits, b"done") else {
                    dec_ref_bits(_py, task_tuple_bits);
                    return MoltObject::none().bits();
                };
                if done {
                    continue;
                }
                let out_bits = asyncio_call_method0(_py, task_bits, b"cancel");
                if exception_pending(_py) {
                    dec_ref_bits(_py, task_tuple_bits);
                    return MoltObject::none().bits();
                }
                let did_cancel = is_truthy(_py, obj_from_bits(out_bits));
                if !obj_from_bits(out_bits).is_none() {
                    dec_ref_bits(_py, out_bits);
                }
                if did_cancel {
                    cancelled_count += 1;
                }
            }
            dec_ref_bits(_py, task_tuple_bits);
            MoltObject::from_int(cancelled_count).bits()
        })
    }
}

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
        let handles = seq_vec_ref(handle_tuple_ptr);
        let mut ran_count = 0i64;
        for &handle_bits in handles {
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
            let waiters = seq_vec_ref(waiter_tuple_ptr);
            let mut released_count = 0i64;
            for (idx, &waiter_bits) in waiters.iter().enumerate() {
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
            let waiters = seq_vec_ref(waiter_tuple_ptr);
            let mut cleaned = 0i64;
            for &waiter_bits in waiters {
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
            let tasks = seq_vec_ref(task_tuple_ptr);
            for &other_task_bits in tasks {
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
            let tasks = seq_vec_ref(task_tuple_ptr);
            let mut attached = 0i64;
            for &task_bits in tasks {
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
            let callbacks = seq_vec_ref(callback_tuple_ptr);
            if trace {
                eprintln!(
                    "molt asyncio callbacks future=0x{:x} count={}",
                    future_bits,
                    callbacks.len()
                );
            }
            let idx0 = MoltObject::from_int(0).bits();
            let idx1 = MoltObject::from_int(1).bits();
            let mut called = 0i64;
            for &entry_bits in callbacks {
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
            let waiters = seq_vec_ref(waiter_tuple_ptr);
            let mut woke = 0i64;
            for &waiter_bits in waiters {
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

#[unsafe(no_mangle)]
pub extern "C" fn molt_future_new(poll_fn_addr: u64, closure_size: u64) -> u64 {
    crate::with_gil_entry_nopanic!(_py, {
        let obj_bits = molt_task_new(poll_fn_addr, closure_size, TASK_KIND_FUTURE);
        if std::env::var("MOLT_DEBUG_AWAITABLE").is_ok()
            && let Some(obj_ptr) = resolve_obj_ptr(obj_bits)
        {
            unsafe {
                let header = header_from_obj_ptr(obj_ptr);
                eprintln!(
                    "Molt future init debug: bits=0x{:x} poll=0x{:x} size={}",
                    obj_bits,
                    poll_fn_addr,
                    crate::object::total_size_from_header(&*header, obj_ptr)
                );
            }
        }
        obj_bits
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_promise_new() -> u64 {
    crate::with_gil_entry_nopanic!(_py, {
        let obj_bits = molt_future_new(promise_poll_fn_addr(), std::mem::size_of::<u64>() as u64);
        if promise_trace_enabled() {
            eprintln!("molt async trace: promise_new bits=0x{:x}", obj_bits);
        }
        obj_bits
    })
}

/// # Safety
/// - `obj_bits` must be a valid pointer to a Molt promise future.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn molt_promise_poll(obj_bits: u64) -> i64 {
    unsafe {
        crate::with_gil_entry_nopanic!(_py, {
            let ptr = ptr_from_bits(obj_bits);
            if ptr.is_null() {
                return MoltObject::none().bits() as i64;
            }
            let _header = header_from_obj_ptr(ptr);
            if async_trace_enabled() || promise_trace_enabled() {
                let current = current_task_ptr();
                eprintln!(
                    "molt async trace: promise_poll task=0x{:x} state={} current=0x{:x}",
                    ptr as usize,
                    crate::object::object_state(ptr),
                    current as usize
                );
            }
            match crate::object::object_state(ptr) {
                0 => pending_bits_i64(),
                1 => {
                    let payload_ptr = ptr as *mut u64;
                    let res_bits = *payload_ptr;
                    inc_ref_bits(_py, res_bits);
                    res_bits as i64
                }
                2 => {
                    let payload_ptr = ptr as *mut u64;
                    let exc_bits = *payload_ptr;
                    let _ = molt_raise(exc_bits);
                    MoltObject::none().bits() as i64
                }
                _ => MoltObject::none().bits() as i64,
            }
        })
    }
}

/// # Safety
/// - `future_bits` must be a valid pointer to a Molt promise future.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn molt_promise_set_result(future_bits: u64, result_bits: u64) -> u64 {
    unsafe {
        crate::with_gil_entry_nopanic!(_py, {
            if async_trace_enabled() || promise_trace_enabled() {
                eprintln!(
                    "molt async trace: promise_set_result_enter bits=0x{:x}",
                    future_bits
                );
            }
            let Some(task_ptr) = resolve_task_ptr(future_bits) else {
                if async_trace_enabled() || promise_trace_enabled() {
                    eprintln!("molt async trace: promise_set_result_fail reason=resolve");
                }
                return raise_exception::<_>(_py, "TypeError", "object is not awaitable");
            };
            let _header = header_from_obj_ptr(task_ptr);
            if crate::object::object_poll_fn(task_ptr) != promise_poll_fn_addr() {
                if async_trace_enabled() || promise_trace_enabled() {
                    eprintln!(
                        "molt async trace: promise_set_result_fail reason=poll_fn poll=0x{:x}",
                        crate::object::object_poll_fn(task_ptr)
                    );
                }
                return raise_exception::<_>(_py, "TypeError", "object is not a promise");
            }
            if crate::object::object_state(task_ptr) != 0 {
                if async_trace_enabled() || promise_trace_enabled() {
                    eprintln!(
                        "molt async trace: promise_set_result_skip state={}",
                        crate::object::object_state(task_ptr)
                    );
                }
                return MoltObject::none().bits();
            }
            let payload_ptr = task_ptr as *mut u64;
            *payload_ptr = result_bits;
            inc_ref_bits(_py, result_bits);
            crate::object::object_set_state(task_ptr, 1);
            if async_trace_enabled() || promise_trace_enabled() {
                eprintln!(
                    "molt async trace: promise_set_result task=0x{:x}",
                    task_ptr as usize
                );
            }
            let waiter_count = wake_await_waiters(_py, task_ptr);
            if async_trace_enabled() || promise_trace_enabled() {
                eprintln!(
                    "molt async trace: promise_wake task=0x{:x} waiters={}",
                    task_ptr as usize, waiter_count
                );
            }
            MoltObject::none().bits()
        })
    }
}

/// # Safety
/// - `future_bits` must be a valid pointer to a Molt promise future.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn molt_promise_set_exception(future_bits: u64, exc_bits: u64) -> u64 {
    unsafe {
        crate::with_gil_entry_nopanic!(_py, {
            let Some(task_ptr) = resolve_task_ptr(future_bits) else {
                return raise_exception::<_>(_py, "TypeError", "object is not awaitable");
            };
            let _header = header_from_obj_ptr(task_ptr);
            if crate::object::object_poll_fn(task_ptr) != promise_poll_fn_addr() {
                return raise_exception::<_>(_py, "TypeError", "object is not a promise");
            }
            if crate::object::object_state(task_ptr) != 0 {
                return MoltObject::none().bits();
            }
            let payload_ptr = task_ptr as *mut u64;
            *payload_ptr = exc_bits;
            inc_ref_bits(_py, exc_bits);
            crate::object::object_set_state(task_ptr, 2);
            if async_trace_enabled() || promise_trace_enabled() {
                eprintln!(
                    "molt async trace: promise_set_exception task=0x{:x}",
                    task_ptr as usize
                );
            }
            let waiter_count = wake_await_waiters(_py, task_ptr);
            if async_trace_enabled() || promise_trace_enabled() {
                eprintln!(
                    "molt async trace: promise_wake task=0x{:x} waiters={}",
                    task_ptr as usize, waiter_count
                );
            }
            MoltObject::none().bits()
        })
    }
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_async_sleep(delay_bits: u64, result_bits: u64) -> u64 {
    crate::with_gil_entry_nopanic!(_py, {
        let obj_bits = molt_future_new(
            async_sleep_poll_fn_addr(),
            (2 * std::mem::size_of::<u64>()) as u64,
        );
        let Some(obj_ptr) = resolve_obj_ptr(obj_bits) else {
            return MoltObject::none().bits();
        };
        unsafe {
            let payload_ptr = obj_ptr as *mut u64;
            *payload_ptr = delay_bits;
            *payload_ptr.add(1) = result_bits;
            inc_ref_bits(_py, delay_bits);
            inc_ref_bits(_py, result_bits);
        }
        obj_bits
    })
}

/// # Safety
/// - `obj_bits` must be a valid pointer if the runtime associates a future with it.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn molt_async_sleep_poll(obj_bits: u64) -> i64 {
    unsafe {
        crate::with_gil_entry_nopanic!(_py, {
            let _obj_ptr = ptr_from_bits(obj_bits);
            if _obj_ptr.is_null() {
                return MoltObject::none().bits() as i64;
            }
            let task_ptr = current_task_ptr();
            if !task_ptr.is_null() && task_cancel_pending(task_ptr) {
                task_take_cancel_pending(task_ptr);
                return raise_cancelled_with_message::<i64>(_py, task_ptr);
            }
            let _header = header_from_obj_ptr(_obj_ptr);
            let payload_bytes = crate::object::object_payload_size(_obj_ptr);
            let payload_len = payload_bytes / std::mem::size_of::<u64>();
            let payload_ptr = _obj_ptr as *mut u64;
            if crate::object::object_state(_obj_ptr) == 0 {
                let delay_secs = if payload_len >= 1 {
                    let delay_bits = *payload_ptr;
                    let float_bits = molt_float_from_obj(delay_bits);
                    let delay_obj = obj_from_bits(float_bits);
                    delay_obj.as_float().unwrap_or(0.0)
                } else {
                    0.0
                };
                let delay_secs = if delay_secs.is_finite() && delay_secs > 0.0 {
                    delay_secs
                } else {
                    0.0
                };
                let immediate = delay_secs <= 0.0;
                if payload_len >= 1 {
                    let deadline = if immediate {
                        ASYNC_SLEEP_YIELD_SENTINEL
                    } else {
                        crate::monotonic_now_secs(_py) + delay_secs
                    };
                    *payload_ptr = MoltObject::from_float(deadline).bits();
                }
                crate::object::object_set_state(_obj_ptr, 1);
                if async_trace_enabled() || sleep_trace_enabled() {
                    eprintln!(
                        "molt async trace: async_sleep_init task=0x{:x} delay={} immediate={}",
                        task_ptr as usize, delay_secs, immediate
                    );
                }
                return pending_bits_i64();
            }

            if payload_len >= 1 {
                let deadline_obj = obj_from_bits(*payload_ptr);
                if let Some(deadline) = to_f64(deadline_obj)
                    && deadline.is_finite()
                    && deadline > 0.0
                    && crate::monotonic_now_secs(_py) < deadline
                {
                    return pending_bits_i64();
                }
            }

            let result_bits = if payload_len >= 2 {
                *payload_ptr.add(1)
            } else {
                MoltObject::none().bits()
            };
            inc_ref_bits(_py, result_bits);
            if async_trace_enabled() || sleep_trace_enabled() {
                eprintln!(
                    "molt async trace: async_sleep_ready task=0x{:x}",
                    task_ptr as usize
                );
            }
            result_bits as i64
        })
    }
}

unsafe fn asyncio_drop_slot_ref(_py: &PyToken<'_>, payload_ptr: *mut u64, idx: usize) {
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

unsafe fn asyncio_exception_kind_is(_py: &PyToken<'_>, exc_bits: u64, expected: &str) -> bool {
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

unsafe fn asyncio_exception_is_fatal_base(_py: &PyToken<'_>, exc_bits: u64) -> bool {
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

unsafe fn asyncio_call_method1(
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

unsafe fn asyncio_call_method2(
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

unsafe fn asyncio_call_method3(
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

unsafe fn asyncio_call_with_args(_py: &PyToken<'_>, callable_bits: u64, args_bits: u64) -> u64 {
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

unsafe fn asyncio_call_method0_allow_missing(
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

unsafe fn asyncio_attr_lookup_allow_missing(
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

unsafe fn asyncio_take_pending_exception_bits(_py: &PyToken<'_>) -> u64 {
    let exc_bits = molt_exception_last();
    molt_exception_clear();
    exc_bits
}

unsafe fn asyncio_method_truthy(_py: &PyToken<'_>, obj_bits: u64, method: &[u8]) -> Option<bool> {
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

unsafe fn asyncio_waiters_pop_front(_py: &PyToken<'_>, waiters_bits: u64) -> u64 {
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

unsafe fn asyncio_taskgroup_collect_task_error(
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

unsafe fn asyncio_wait_scan(
    _py: &PyToken<'_>,
    tasks_bits: u64,
    return_when: i64,
) -> Option<(Vec<bool>, bool)> {
    unsafe {
        let Some(tasks_ptr) = obj_from_bits(tasks_bits).as_ptr() else {
            let _ = raise_exception::<u64>(_py, "TypeError", "wait tasks must be awaitables");
            return None;
        };
        let tasks = seq_vec_ref(tasks_ptr);
        let mut done_flags = Vec::with_capacity(tasks.len());
        let mut pending_count = 0usize;
        let mut triggered = false;
        for &task_bits in tasks {
            let done = asyncio_method_truthy(_py, task_bits, b"done")?;
            done_flags.push(done);
            if !done {
                pending_count += 1;
                continue;
            }
            if return_when == ASYNCIO_WAIT_RETURN_FIRST_COMPLETED {
                triggered = true;
                continue;
            }
            if return_when == ASYNCIO_WAIT_RETURN_FIRST_EXCEPTION {
                let cancelled = asyncio_method_truthy(_py, task_bits, b"cancelled")?;
                if cancelled {
                    triggered = true;
                    continue;
                }
                let exc_bits = asyncio_call_method0(_py, task_bits, b"exception");
                if exception_pending(_py) {
                    return None;
                }
                let has_exc = !obj_from_bits(exc_bits).is_none();
                dec_ref_bits(_py, exc_bits);
                if has_exc {
                    triggered = true;
                }
            }
        }
        if return_when == ASYNCIO_WAIT_RETURN_ALL_COMPLETED && pending_count == 0 {
            triggered = true;
        }
        Some((done_flags, triggered))
    }
}

unsafe fn asyncio_wait_build_result(
    _py: &PyToken<'_>,
    tasks_bits: u64,
    done_flags: &[bool],
) -> i64 {
    unsafe {
        let Some(tasks_ptr) = obj_from_bits(tasks_bits).as_ptr() else {
            return raise_exception::<i64>(_py, "TypeError", "wait tasks must be awaitables");
        };
        let tasks = seq_vec_ref(tasks_ptr);
        if tasks.len() != done_flags.len() {
            return raise_exception::<i64>(_py, "RuntimeError", "invalid wait state");
        }
        let done_bits = molt_set_new(tasks.len() as u64);
        if obj_from_bits(done_bits).is_none() {
            return MoltObject::none().bits() as i64;
        }
        let pending_bits = molt_set_new(tasks.len() as u64);
        if obj_from_bits(pending_bits).is_none() {
            dec_ref_bits(_py, done_bits);
            return MoltObject::none().bits() as i64;
        }
        for (idx, &task_bits) in tasks.iter().enumerate() {
            let target_set = if done_flags[idx] {
                done_bits
            } else {
                pending_bits
            };
            let _ = molt_set_add(target_set, task_bits);
            if exception_pending(_py) {
                dec_ref_bits(_py, done_bits);
                dec_ref_bits(_py, pending_bits);
                return MoltObject::none().bits() as i64;
            }
        }
        let out_ptr = alloc_tuple(_py, &[done_bits, pending_bits]);
        dec_ref_bits(_py, done_bits);
        dec_ref_bits(_py, pending_bits);
        if out_ptr.is_null() {
            return MoltObject::none().bits() as i64;
        }
        MoltObject::from_ptr(out_ptr).bits() as i64
    }
}

unsafe fn asyncio_cancel_task(_py: &PyToken<'_>, task_bits: u64) {
    unsafe {
        let out_bits = asyncio_call_method0(_py, task_bits, b"cancel");
        if exception_pending(_py) {
            asyncio_clear_pending_exception(_py);
            return;
        }
        if !obj_from_bits(out_bits).is_none() {
            dec_ref_bits(_py, out_bits);
        }
    }
}

unsafe fn asyncio_gather_store_result(
    _py: &PyToken<'_>,
    payload_ptr: *mut u64,
    idx: usize,
    value_bits: u64,
) {
    unsafe {
        let slot = payload_ptr.add(ASYNCIO_GATHER_RESULT_OFFSET + idx);
        let old_bits = *slot;
        if old_bits != 0 && !obj_from_bits(old_bits).is_none() {
            dec_ref_bits(_py, old_bits);
        }
        *slot = value_bits;
        inc_ref_bits(_py, value_bits);
    }
}

unsafe fn asyncio_gather_cancel_pending(
    _py: &PyToken<'_>,
    tasks_bits: u64,
    payload_ptr: *mut u64,
    results_len: usize,
    missing: u64,
) {
    unsafe {
        let Some(tasks_ptr) = obj_from_bits(tasks_bits).as_ptr() else {
            return;
        };
        let tasks = seq_vec_ref(tasks_ptr);
        let limit = results_len.min(tasks.len());
        for (idx, &task_bits) in tasks.iter().take(limit).enumerate() {
            if *payload_ptr.add(ASYNCIO_GATHER_RESULT_OFFSET + idx) == missing {
                asyncio_cancel_task(_py, task_bits);
            }
        }
    }
}

unsafe fn asyncio_gather_build_list(_py: &PyToken<'_>, payload_ptr: *mut u64, len: usize) -> u64 {
    unsafe {
        let mut elems = Vec::with_capacity(len);
        for idx in 0..len {
            elems.push(*payload_ptr.add(ASYNCIO_GATHER_RESULT_OFFSET + idx));
        }
        let out_ptr = alloc_list(_py, elems.as_slice());
        if out_ptr.is_null() {
            return MoltObject::none().bits();
        }
        MoltObject::from_ptr(out_ptr).bits()
    }
}

/// # Safety
/// - `tasks_bits` must be iterable; items must implement asyncio Future methods.
#[unsafe(no_mangle)]
pub extern "C" fn molt_asyncio_wait_new(
    tasks_bits: u64,
    timeout_bits: u64,
    return_when_bits: u64,
) -> u64 {
    crate::with_gil_entry_nopanic!(_py, {
        let Some(task_tuple_bits) = (unsafe { tuple_from_iter_bits(_py, tasks_bits) }) else {
            return MoltObject::none().bits();
        };
        let Some(task_tuple_ptr) = obj_from_bits(task_tuple_bits).as_ptr() else {
            dec_ref_bits(_py, task_tuple_bits);
            return raise_exception::<u64>(_py, "TypeError", "wait tasks must be awaitables");
        };
        if unsafe { seq_vec_ref(task_tuple_ptr) }.is_empty() {
            dec_ref_bits(_py, task_tuple_bits);
            return raise_exception::<u64>(
                _py,
                "ValueError",
                "asyncio.wait() requires at least one awaitable",
            );
        }
        let return_when =
            to_i64(obj_from_bits(return_when_bits)).unwrap_or(ASYNCIO_WAIT_RETURN_ALL_COMPLETED);
        if !matches!(
            return_when,
            ASYNCIO_WAIT_RETURN_ALL_COMPLETED
                | ASYNCIO_WAIT_RETURN_FIRST_COMPLETED
                | ASYNCIO_WAIT_RETURN_FIRST_EXCEPTION
        ) {
            dec_ref_bits(_py, task_tuple_bits);
            return raise_exception::<u64>(_py, "ValueError", "Invalid return_when value");
        }

        let mut wait_flags = 0i64;
        let mut timer_bits = MoltObject::none().bits();
        if !obj_from_bits(timeout_bits).is_none() {
            let timeout_obj = obj_from_bits(molt_float_from_obj(timeout_bits));
            if exception_pending(_py) {
                dec_ref_bits(_py, task_tuple_bits);
                return MoltObject::none().bits();
            }
            let timeout = timeout_obj.as_float().unwrap_or(0.0);
            if timeout.is_finite() && timeout > 0.0 {
                timer_bits = molt_async_sleep(
                    MoltObject::from_float(timeout).bits(),
                    MoltObject::none().bits(),
                );
                if obj_from_bits(timer_bits).is_none() {
                    dec_ref_bits(_py, task_tuple_bits);
                    return MoltObject::none().bits();
                }
                wait_flags |= ASYNCIO_WAIT_FLAG_HAS_TIMER;
            } else {
                // Match CPython: timeout<=0 still gives scheduled tasks one loop turn
                // before timing out.
                wait_flags |= ASYNCIO_WAIT_FLAG_TIMEOUT_DEFERRED_2;
            }
        }

        let obj_bits = molt_future_new(
            asyncio_wait_poll_fn_addr(),
            (4 * std::mem::size_of::<u64>()) as u64,
        );
        if obj_from_bits(obj_bits).is_none() {
            dec_ref_bits(_py, task_tuple_bits);
            if !obj_from_bits(timer_bits).is_none() {
                dec_ref_bits(_py, timer_bits);
            }
            return obj_bits;
        }
        let Some(obj_ptr) = resolve_obj_ptr(obj_bits) else {
            dec_ref_bits(_py, task_tuple_bits);
            if !obj_from_bits(timer_bits).is_none() {
                dec_ref_bits(_py, timer_bits);
            }
            return MoltObject::none().bits();
        };
        unsafe {
            let payload_ptr = obj_ptr as *mut u64;
            *payload_ptr = task_tuple_bits;
            *payload_ptr.add(1) = timer_bits;
            *payload_ptr.add(2) = MoltObject::from_int(return_when).bits();
            *payload_ptr.add(3) = MoltObject::from_int(wait_flags).bits();
        }
        obj_bits
    })
}

/// # Safety
/// - `obj_bits` must be a valid pointer to an asyncio wait wrapper future.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn molt_asyncio_wait_poll(obj_bits: u64) -> i64 {
    unsafe {
        crate::with_gil_entry_nopanic!(_py, {
            let obj_ptr = ptr_from_bits(obj_bits);
            if obj_ptr.is_null() {
                return MoltObject::none().bits() as i64;
            }
            let _header = header_from_obj_ptr(obj_ptr);
            let payload_bytes = crate::object::object_payload_size(obj_ptr);
            if payload_bytes < 4 * std::mem::size_of::<u64>() {
                return raise_exception::<i64>(_py, "RuntimeError", "invalid wait payload");
            }
            let payload_ptr = obj_ptr as *mut u64;
            let wrapper_ptr = current_task_ptr();
            if !wrapper_ptr.is_null() && wrapper_ptr == obj_ptr && task_cancel_pending(wrapper_ptr)
            {
                task_take_cancel_pending(wrapper_ptr);
                return raise_cancelled_with_message::<i64>(_py, wrapper_ptr);
            }
            let tasks_bits = *payload_ptr;
            let return_when = to_i64(obj_from_bits(*payload_ptr.add(2)))
                .unwrap_or(ASYNCIO_WAIT_RETURN_ALL_COMPLETED);
            let Some((done_flags, mut triggered)) = asyncio_wait_scan(_py, tasks_bits, return_when)
            else {
                return MoltObject::none().bits() as i64;
            };
            let mut wait_flags = to_i64(obj_from_bits(*payload_ptr.add(3))).unwrap_or(0);
            if !triggered {
                if (wait_flags & ASYNCIO_WAIT_FLAG_TIMEOUT_DEFERRED_2) != 0 {
                    wait_flags &= !ASYNCIO_WAIT_FLAG_TIMEOUT_DEFERRED_2;
                    wait_flags |= ASYNCIO_WAIT_FLAG_TIMEOUT_DEFERRED;
                    *payload_ptr.add(3) = MoltObject::from_int(wait_flags).bits();
                    return pending_bits_i64();
                }
                if (wait_flags & ASYNCIO_WAIT_FLAG_TIMEOUT_DEFERRED) != 0 {
                    wait_flags &= !ASYNCIO_WAIT_FLAG_TIMEOUT_DEFERRED;
                    wait_flags |= ASYNCIO_WAIT_FLAG_TIMEOUT_READY;
                    *payload_ptr.add(3) = MoltObject::from_int(wait_flags).bits();
                    return pending_bits_i64();
                }
                if (wait_flags & ASYNCIO_WAIT_FLAG_TIMEOUT_READY) != 0 {
                    triggered = true;
                } else if (wait_flags & ASYNCIO_WAIT_FLAG_HAS_TIMER) != 0 {
                    let timer_bits = *payload_ptr.add(1);
                    if !obj_from_bits(timer_bits).is_none() {
                        let timer_res = molt_future_poll(timer_bits);
                        if timer_res == pending_bits_i64() {
                            return pending_bits_i64();
                        }
                        if exception_pending(_py) {
                            return timer_res;
                        }
                    }
                    wait_flags |= ASYNCIO_WAIT_FLAG_TIMEOUT_READY;
                    *payload_ptr.add(3) = MoltObject::from_int(wait_flags).bits();
                    triggered = true;
                }
            }
            if !triggered {
                return pending_bits_i64();
            }
            let out = asyncio_wait_build_result(_py, tasks_bits, done_flags.as_slice());
            if exception_pending(_py) {
                return out;
            }
            asyncio_drop_slot_ref(_py, payload_ptr, 0);
            asyncio_drop_slot_ref(_py, payload_ptr, 1);
            asyncio_drop_slot_ref(_py, payload_ptr, 2);
            asyncio_drop_slot_ref(_py, payload_ptr, 3);
            out
        })
    }
}

/// # Safety
/// - `tasks_bits` must be iterable; items must implement asyncio Future methods.
#[unsafe(no_mangle)]
pub extern "C" fn molt_asyncio_gather_new(tasks_bits: u64, return_exceptions_bits: u64) -> u64 {
    crate::with_gil_entry_nopanic!(_py, {
        let Some(task_tuple_bits) = (unsafe { tuple_from_iter_bits(_py, tasks_bits) }) else {
            return MoltObject::none().bits();
        };
        let Some(task_tuple_ptr) = obj_from_bits(task_tuple_bits).as_ptr() else {
            dec_ref_bits(_py, task_tuple_bits);
            return raise_exception::<u64>(_py, "TypeError", "gather tasks must be awaitables");
        };
        let results_len = unsafe { seq_vec_ref(task_tuple_ptr) }.len();
        let payload_slots = ASYNCIO_GATHER_RESULT_OFFSET + results_len;
        let payload_bytes = (payload_slots * std::mem::size_of::<u64>()) as u64;
        let obj_bits = molt_future_new(asyncio_gather_poll_fn_addr(), payload_bytes);
        if obj_from_bits(obj_bits).is_none() {
            dec_ref_bits(_py, task_tuple_bits);
            return obj_bits;
        }
        let Some(obj_ptr) = resolve_obj_ptr(obj_bits) else {
            dec_ref_bits(_py, task_tuple_bits);
            return MoltObject::none().bits();
        };
        let return_exceptions = is_truthy(_py, obj_from_bits(return_exceptions_bits));
        let missing = missing_bits(_py);
        unsafe {
            let payload_ptr = obj_ptr as *mut u64;
            *payload_ptr = task_tuple_bits;
            *payload_ptr.add(1) = MoltObject::from_bool(return_exceptions).bits();
            *payload_ptr.add(2) = MoltObject::from_int(0).bits();
            *payload_ptr.add(3) = MoltObject::from_int(results_len as i64).bits();
            for idx in 0..results_len {
                let slot = payload_ptr.add(ASYNCIO_GATHER_RESULT_OFFSET + idx);
                *slot = missing;
                inc_ref_bits(_py, missing);
            }
        }
        obj_bits
    })
}

/// # Safety
/// - `obj_bits` must be a valid pointer to an asyncio gather wrapper future.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn molt_asyncio_gather_poll(obj_bits: u64) -> i64 {
    unsafe {
        crate::with_gil_entry_nopanic!(_py, {
            let obj_ptr = ptr_from_bits(obj_bits);
            if obj_ptr.is_null() {
                return MoltObject::none().bits() as i64;
            }
            let _header = header_from_obj_ptr(obj_ptr);
            let payload_bytes = crate::object::object_payload_size(obj_ptr);
            if payload_bytes < ASYNCIO_GATHER_RESULT_OFFSET * std::mem::size_of::<u64>() {
                return raise_exception::<i64>(_py, "RuntimeError", "invalid gather payload");
            }
            let payload_ptr = obj_ptr as *mut u64;
            let payload_slots = payload_bytes / std::mem::size_of::<u64>();
            let results_len = payload_slots.saturating_sub(ASYNCIO_GATHER_RESULT_OFFSET);
            let tasks_bits = *payload_ptr;
            let Some(tasks_ptr) = obj_from_bits(tasks_bits).as_ptr() else {
                return raise_exception::<i64>(_py, "TypeError", "gather tasks must be awaitables");
            };
            let tasks = seq_vec_ref(tasks_ptr);
            if tasks.len() != results_len {
                return raise_exception::<i64>(_py, "RuntimeError", "invalid gather payload state");
            }
            let wrapper_ptr = current_task_ptr();
            if !wrapper_ptr.is_null() && wrapper_ptr == obj_ptr && task_cancel_pending(wrapper_ptr)
            {
                let missing = missing_bits(_py);
                asyncio_gather_cancel_pending(_py, tasks_bits, payload_ptr, results_len, missing);
                task_take_cancel_pending(wrapper_ptr);
                return raise_cancelled_with_message::<i64>(_py, wrapper_ptr);
            }

            let missing = missing_bits(_py);
            let return_exceptions = is_truthy(_py, obj_from_bits(*payload_ptr.add(1)));
            for (idx, &task_bits) in tasks.iter().enumerate() {
                if *payload_ptr.add(ASYNCIO_GATHER_RESULT_OFFSET + idx) != missing {
                    continue;
                }
                let done = match asyncio_method_truthy(_py, task_bits, b"done") {
                    Some(value) => value,
                    None => return MoltObject::none().bits() as i64,
                };
                if !done {
                    continue;
                }
                let result_bits = asyncio_call_method0(_py, task_bits, b"result");
                if exception_pending(_py) {
                    if return_exceptions {
                        let exc_bits = molt_exception_last();
                        molt_exception_clear();
                        asyncio_gather_store_result(_py, payload_ptr, idx, exc_bits);
                        dec_ref_bits(_py, exc_bits);
                        if exception_pending(_py) {
                            return MoltObject::none().bits() as i64;
                        }
                        continue;
                    }
                    let exc_bits = molt_exception_last();
                    molt_exception_clear();
                    asyncio_gather_cancel_pending(
                        _py,
                        tasks_bits,
                        payload_ptr,
                        results_len,
                        missing,
                    );
                    let raised = molt_raise(exc_bits);
                    dec_ref_bits(_py, exc_bits);
                    return raised as i64;
                }
                asyncio_gather_store_result(_py, payload_ptr, idx, result_bits);
                dec_ref_bits(_py, result_bits);
                if exception_pending(_py) {
                    return MoltObject::none().bits() as i64;
                }
            }

            for idx in 0..results_len {
                if *payload_ptr.add(ASYNCIO_GATHER_RESULT_OFFSET + idx) == missing {
                    return pending_bits_i64();
                }
            }
            let out_bits = asyncio_gather_build_list(_py, payload_ptr, results_len);
            if obj_from_bits(out_bits).is_none() {
                return MoltObject::none().bits() as i64;
            }
            for idx in 0..payload_slots {
                asyncio_drop_slot_ref(_py, payload_ptr, idx);
            }
            out_bits as i64
        })
    }
}

/// # Safety
/// - `future_ptr` must be a valid Molt wait future pointer.
pub(crate) unsafe fn asyncio_wait_task_drop(_py: &PyToken<'_>, future_ptr: *mut u8) {
    unsafe {
        if future_ptr.is_null() {
            return;
        }
        let _header = header_from_obj_ptr(future_ptr);
        let payload_bytes = crate::object::object_payload_size(future_ptr);
        if payload_bytes < 4 * std::mem::size_of::<u64>() {
            return;
        }
        let payload_ptr = future_ptr as *mut u64;
        for idx in 0..4usize {
            asyncio_drop_slot_ref(_py, payload_ptr, idx);
        }
    }
}

/// # Safety
/// - `future_ptr` must be a valid Molt gather future pointer.
pub(crate) unsafe fn asyncio_gather_task_drop(_py: &PyToken<'_>, future_ptr: *mut u8) {
    unsafe {
        if future_ptr.is_null() {
            return;
        }
        let _header = header_from_obj_ptr(future_ptr);
        let payload_bytes = crate::object::object_payload_size(future_ptr);
        let payload_slots = payload_bytes / std::mem::size_of::<u64>();
        if payload_slots < ASYNCIO_GATHER_RESULT_OFFSET {
            return;
        }
        let payload_ptr = future_ptr as *mut u64;
        for idx in 0..payload_slots {
            asyncio_drop_slot_ref(_py, payload_ptr, idx);
        }
    }
}

/// # Safety
/// - `future_ptr` must be a valid Molt wait_for future pointer.
pub(crate) unsafe fn asyncio_wait_for_task_drop(_py: &PyToken<'_>, future_ptr: *mut u8) {
    unsafe {
        if future_ptr.is_null() {
            return;
        }
        let _header = header_from_obj_ptr(future_ptr);
        let payload_bytes = crate::object::object_payload_size(future_ptr);
        if payload_bytes < 4 * std::mem::size_of::<u64>() {
            return;
        }
        let payload_ptr = future_ptr as *mut u64;
        for idx in 0..3usize {
            let bits = *payload_ptr.add(idx);
            if bits != 0 && !obj_from_bits(bits).is_none() {
                dec_ref_bits(_py, bits);
                *payload_ptr.add(idx) = MoltObject::none().bits();
            }
        }
    }
}

/// # Safety
/// - `future_ptr` must be a valid Molt stream-reader read future pointer.

fn wait_for_raise_timeout(_py: &PyToken<'_>) -> i64 {
    raise_exception::<i64>(_py, "TimeoutError", "")
}

unsafe fn wait_for_flags(payload_ptr: *mut u64) -> i64 {
    unsafe { to_i64(obj_from_bits(*payload_ptr.add(3))).unwrap_or(0) }
}

unsafe fn wait_for_set_flags(payload_ptr: *mut u64, flags: i64) {
    unsafe {
        *payload_ptr.add(3) = MoltObject::from_int(flags).bits();
    }
}

unsafe fn wait_for_drop_slot_ref(_py: &PyToken<'_>, payload_ptr: *mut u64, idx: usize) {
    unsafe {
        let bits = *payload_ptr.add(idx);
        if bits != 0 && !obj_from_bits(bits).is_none() {
            dec_ref_bits(_py, bits);
        }
        *payload_ptr.add(idx) = MoltObject::none().bits();
    }
}

unsafe fn wait_for_clear_pending_exception(_py: &PyToken<'_>) {
    if !exception_pending(_py) {
        return;
    }
    let exc_bits = molt_exception_last();
    dec_ref_bits(_py, exc_bits);
    molt_exception_clear();
}

unsafe fn wait_for_has_method(_py: &PyToken<'_>, obj_bits: u64, method: &[u8]) -> bool {
    unsafe {
        let Some(obj_ptr) = obj_from_bits(obj_bits).as_ptr() else {
            return false;
        };
        let Some(method_name_bits) = attr_name_bits_from_bytes(_py, method) else {
            return false;
        };
        let Some(method_bits) = attr_lookup_ptr_allow_missing(_py, obj_ptr, method_name_bits)
        else {
            return false;
        };
        dec_ref_bits(_py, method_bits);
        true
    }
}

unsafe fn wait_for_is_supported_target(_py: &PyToken<'_>, target_bits: u64) -> bool {
    unsafe {
        if resolve_task_ptr(target_bits).is_some() {
            return true;
        }
        wait_for_has_method(_py, target_bits, b"done")
            && wait_for_has_method(_py, target_bits, b"cancel")
            && wait_for_has_method(_py, target_bits, b"result")
    }
}

unsafe fn wait_for_poll_target(_py: &PyToken<'_>, target_bits: u64) -> i64 {
    unsafe {
        if resolve_task_ptr(target_bits).is_some() {
            return molt_future_poll(target_bits);
        }
        let Some(done) = asyncio_method_truthy(_py, target_bits, b"done") else {
            return MoltObject::none().bits() as i64;
        };
        if !done {
            return pending_bits_i64();
        }
        asyncio_call_method0(_py, target_bits, b"result") as i64
    }
}

unsafe fn wait_for_cancel_target(_py: &PyToken<'_>, target_bits: u64) {
    unsafe {
        if let Some(task_ptr) = resolve_task_ptr(target_bits) {
            cancel_future_task(_py, task_ptr, None);
            return;
        }
        asyncio_cancel_task(_py, target_bits);
    }
}

/// # Safety
/// - `future_bits` must reference an awaitable future/task.
#[unsafe(no_mangle)]
pub extern "C" fn molt_asyncio_wait_for_new(future_bits: u64, timeout_bits: u64) -> u64 {
    crate::with_gil_entry_nopanic!(_py, {
        let supported = unsafe { wait_for_is_supported_target(_py, future_bits) };
        if exception_pending(_py) {
            return MoltObject::none().bits();
        }
        if !supported {
            return raise_exception::<u64>(_py, "TypeError", "object is not awaitable");
        }
        let payload = (4 * std::mem::size_of::<u64>()) as u64;
        let obj_bits = molt_future_new(asyncio_wait_for_poll_fn_addr(), payload);
        if obj_from_bits(obj_bits).is_none() {
            return obj_bits;
        }
        let Some(obj_ptr) = resolve_obj_ptr(obj_bits) else {
            return MoltObject::none().bits();
        };
        unsafe {
            let payload_ptr = obj_ptr as *mut u64;
            *payload_ptr = future_bits;
            *payload_ptr.add(1) = timeout_bits;
            *payload_ptr.add(2) = MoltObject::none().bits();
            *payload_ptr.add(3) = MoltObject::from_int(0).bits();
            inc_ref_bits(_py, future_bits);
            inc_ref_bits(_py, timeout_bits);
        }
        obj_bits
    })
}

/// # Safety
/// - `obj_bits` must be a valid pointer to a wait_for wrapper future.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn molt_asyncio_wait_for_poll(obj_bits: u64) -> i64 {
    unsafe {
        crate::with_gil_entry_nopanic!(_py, {
            let obj_ptr = ptr_from_bits(obj_bits);
            if obj_ptr.is_null() {
                return MoltObject::none().bits() as i64;
            }
            let _header = header_from_obj_ptr(obj_ptr);
            let payload_bytes = crate::object::object_payload_size(obj_ptr);
            if payload_bytes < 4 * std::mem::size_of::<u64>() {
                return raise_exception::<i64>(_py, "RuntimeError", "invalid wait_for payload");
            }
            let payload_ptr = obj_ptr as *mut u64;
            let target_bits = *payload_ptr;
            let wrapper_ptr = current_task_ptr();
            if !wrapper_ptr.is_null() && wrapper_ptr == obj_ptr && task_cancel_pending(wrapper_ptr)
            {
                let timer_bits = *payload_ptr.add(2);
                if let Some(timer_ptr) = resolve_task_ptr(timer_bits) {
                    cancel_future_task(_py, timer_ptr, None);
                }
                wait_for_cancel_target(_py, target_bits);
                task_take_cancel_pending(wrapper_ptr);
                return raise_cancelled_with_message::<i64>(_py, wrapper_ptr);
            }

            if crate::object::object_state(obj_ptr) == 0 {
                let mut flags = wait_for_flags(payload_ptr);
                let timeout_bits = *payload_ptr.add(1);
                if obj_from_bits(timeout_bits).is_none() {
                    wait_for_drop_slot_ref(_py, payload_ptr, 1);
                } else {
                    let timeout_obj = obj_from_bits(molt_float_from_obj(timeout_bits));
                    if exception_pending(_py) {
                        return MoltObject::none().bits() as i64;
                    }
                    let timeout = timeout_obj.as_float().unwrap_or(0.0);
                    let immediate = !timeout.is_finite() || timeout <= 0.0;
                    if immediate {
                        flags |= WAIT_FOR_FLAG_FORCE_TIMEOUT;
                        wait_for_set_flags(payload_ptr, flags);
                        wait_for_drop_slot_ref(_py, payload_ptr, 1);
                        let Some(done_now) = asyncio_method_truthy(_py, target_bits, b"done")
                        else {
                            return MoltObject::none().bits() as i64;
                        };
                        if !done_now {
                            wait_for_cancel_target(_py, target_bits);
                            crate::object::object_set_state(obj_ptr, WAIT_FOR_STATE_CANCEL_WAIT);
                            // Fast-path immediate timeout cancellation: if the target settles
                            // synchronously (common when it has not started yet), resolve now
                            // instead of yielding another scheduler turn.
                            let target_res = wait_for_poll_target(_py, target_bits);
                            if target_res == pending_bits_i64() {
                                return pending_bits_i64();
                            }
                            let flags = wait_for_flags(payload_ptr);
                            if (flags & WAIT_FOR_FLAG_FORCE_TIMEOUT) != 0 {
                                wait_for_clear_pending_exception(_py);
                                return wait_for_raise_timeout(_py);
                            }
                            if exception_pending(_py) {
                                let exc_bits = molt_exception_last();
                                let kind_bits = molt_exception_kind(exc_bits);
                                let kind = string_obj_to_owned(obj_from_bits(kind_bits));
                                dec_ref_bits(_py, kind_bits);
                                dec_ref_bits(_py, exc_bits);
                                if kind.as_deref() == Some("CancelledError") {
                                    molt_exception_clear();
                                    return wait_for_raise_timeout(_py);
                                }
                            }
                            return target_res;
                        }
                    } else {
                        let timer_bits = molt_async_sleep(
                            MoltObject::from_float(timeout).bits(),
                            MoltObject::none().bits(),
                        );
                        if obj_from_bits(timer_bits).is_none() {
                            return timer_bits as i64;
                        }
                        *payload_ptr.add(2) = timer_bits;
                        inc_ref_bits(_py, timer_bits);
                        flags |= WAIT_FOR_FLAG_HAS_TIMER;
                        wait_for_set_flags(payload_ptr, flags);
                        wait_for_drop_slot_ref(_py, payload_ptr, 1);
                    }
                }
                crate::object::object_set_state(obj_ptr, WAIT_FOR_STATE_PENDING);
            }

            if crate::object::object_state(obj_ptr) == WAIT_FOR_STATE_CANCEL_WAIT {
                let target_res = wait_for_poll_target(_py, target_bits);
                if target_res == pending_bits_i64() {
                    return pending_bits_i64();
                }
                let flags = wait_for_flags(payload_ptr);
                if (flags & WAIT_FOR_FLAG_FORCE_TIMEOUT) != 0 {
                    wait_for_clear_pending_exception(_py);
                    return wait_for_raise_timeout(_py);
                }
                if exception_pending(_py) {
                    let exc_bits = molt_exception_last();
                    let kind_bits = molt_exception_kind(exc_bits);
                    let kind = string_obj_to_owned(obj_from_bits(kind_bits));
                    dec_ref_bits(_py, kind_bits);
                    dec_ref_bits(_py, exc_bits);
                    if kind.as_deref() == Some("CancelledError") {
                        molt_exception_clear();
                        return wait_for_raise_timeout(_py);
                    }
                }
                return target_res;
            }

            let target_res = wait_for_poll_target(_py, target_bits);
            if target_res != pending_bits_i64() {
                wait_for_drop_slot_ref(_py, payload_ptr, 2);
                return target_res;
            }

            let flags = wait_for_flags(payload_ptr);
            if (flags & WAIT_FOR_FLAG_HAS_TIMER) == 0 {
                return pending_bits_i64();
            }
            let timer_bits = *payload_ptr.add(2);
            if obj_from_bits(timer_bits).is_none() {
                return pending_bits_i64();
            }
            let timer_res = molt_future_poll(timer_bits);
            if timer_res == pending_bits_i64() {
                return pending_bits_i64();
            }
            if exception_pending(_py) {
                return timer_res;
            }
            wait_for_cancel_target(_py, target_bits);
            wait_for_drop_slot_ref(_py, payload_ptr, 2);
            crate::object::object_set_state(obj_ptr, WAIT_FOR_STATE_CANCEL_WAIT);
            pending_bits_i64()
        })
    }
}

/// # Safety
/// - `obj_bits` must be a valid pointer to a Molt future allocated with payload slots.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn molt_anext_default_poll(obj_bits: u64) -> i64 {
    unsafe {
        crate::with_gil_entry_nopanic!(_py, {
            let _obj_ptr = ptr_from_bits(obj_bits);
            if _obj_ptr.is_null() {
                return MoltObject::none().bits() as i64;
            }
            let _header = header_from_obj_ptr(_obj_ptr);
            let payload_bytes = crate::object::object_payload_size(_obj_ptr);
            if payload_bytes < 3 * std::mem::size_of::<u64>() {
                return MoltObject::none().bits() as i64;
            }
            let payload_ptr = _obj_ptr as *mut u64;
            let iter_bits = *payload_ptr;
            let default_bits = *payload_ptr.add(1);
            if crate::object::object_state(_obj_ptr) == 0 {
                let await_bits = molt_anext(iter_bits);
                inc_ref_bits(_py, await_bits);
                *payload_ptr.add(2) = await_bits;
                crate::object::object_set_state(_obj_ptr, 1);
            }
            let await_bits = *payload_ptr.add(2);
            let Some(await_ptr) = maybe_ptr_from_bits(await_bits) else {
                return MoltObject::none().bits() as i64;
            };
            let poll_fn_addr = crate::object::object_poll_fn(await_ptr);
            if poll_fn_addr == 0 {
                return MoltObject::none().bits() as i64;
            }
            let res = molt_future_poll(await_bits);
            if res == pending_bits_i64() {
                return res;
            }
            if exception_pending(_py) {
                let exc_bits = molt_exception_last();
                let kind_bits = molt_exception_kind(exc_bits);
                let kind = string_obj_to_owned(obj_from_bits(kind_bits));
                dec_ref_bits(_py, kind_bits);
                if kind.as_deref() == Some("StopAsyncIteration") {
                    exception_clear_reason_set("anext_default_stopasync");
                    molt_exception_clear();
                    dec_ref_bits(_py, exc_bits);
                    inc_ref_bits(_py, default_bits);
                    return default_bits as i64;
                }
                dec_ref_bits(_py, exc_bits);
            }
            res
        })
    }
}

/// # Safety
/// - `task_ptr_bits` must encode a valid Molt task pointer.
/// - `future_ptr_bits` must encode a valid Molt future pointer.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn molt_sleep_register(task_ptr_bits: u64, future_ptr_bits: u64) -> u64 {
    unsafe {
        crate::with_gil_entry_nopanic!(_py, {
            let task_ptr = task_ptr_bits as usize as *mut u8;
            let future_ptr = future_ptr_bits as usize as *mut u8;
            if task_ptr.is_null() || future_ptr.is_null() {
                return 0;
            }
            let header = header_from_obj_ptr(task_ptr);
            let flags = (*header).flags;
            let is_block_on = (flags & HEADER_FLAG_BLOCK_ON) != 0;
            let is_spawned = (flags & HEADER_FLAG_SPAWN_RETAIN) != 0;
            if !is_block_on && !is_spawned {
                return 0;
            }
            let sleep_target = resolve_sleep_target(_py, future_ptr);
            if sleep_register_impl(_py, task_ptr, sleep_target) {
                1
            } else {
                0
            }
        })
    }
}

#[cfg(test)]
mod tests {
    use super::{molt_asyncgen_new, molt_generator_new};
    use crate::{GEN_CONTROL_SIZE, asyncgen_registry, dec_ref_bits, obj_from_bits};

    #[test]
    fn asyncgen_registry_removes_on_drop() {
        let _guard = crate::TEST_MUTEX.lock().unwrap();
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
