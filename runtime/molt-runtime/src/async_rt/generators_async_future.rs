//! Async future, promise, cancellation, and sleep primitives.
//!
//! This module owns the shared awaitable core used by asyncio combinators,
//! I/O futures, threads, processes, and async generators.

use super::*;

const ASYNC_SLEEP_YIELD_SECS: f64 = 0.000_001;
const ASYNC_SLEEP_YIELD_SENTINEL: f64 = -1.0;

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
                    let task_scope = crate::CurrentTaskScope::enter(_py, ptr);
                    let prev_task = task_scope.previous();
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
                    drop(task_scope);
                    if !obj_from_bits(exc_bits).is_none() {
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
            let task_count = crate::object::seq_access::len(task_tuple_ptr);
            let mut cancelled_count = 0i64;
            for idx in 0..task_count {
                let Some(task) = crate::object::seq_access::pin_item(_py, task_tuple_ptr, idx)
                else {
                    dec_ref_bits(_py, task_tuple_bits);
                    return raise_exception::<u64>(_py, "RuntimeError", "invalid task state");
                };
                let task_bits = task.bits();
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
/// - `task_ptr` must be a valid Molt task pointer.
/// - `future_ptr` must be a valid Molt future pointer.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn molt_sleep_register(task_ptr: *mut u8, future_ptr: *mut u8) -> u64 {
    unsafe {
        crate::with_gil_entry_nopanic!(_py, {
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
