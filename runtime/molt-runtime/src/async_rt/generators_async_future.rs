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
        let caller = current_task_ptr();
        // An exception injected at an await continuation belongs to its caller.
        // Do not execute or terminalize the child merely to deliver that error.
        let injected = maybe_ptr_from_bits(future_bits)
            .and_then(|child| crate::async_rt::awaitable::direct_exception_before_poll(_py, child));
        let result = injected.unwrap_or_else(|| poll_future_value(_py, future_bits));
        if !caller.is_null()
            && let Some(child) = maybe_ptr_from_bits(future_bits)
            && child != caller
        {
            unsafe {
                if result == pending_bits_i64() && !exception_pending(_py) {
                    if crate::object::aux_header::object_frame_awaited_bits(caller) != future_bits {
                        inc_ref_bits(_py, future_bits);
                        crate::object::aux_header::object_replace_frame_awaited_owned(
                            _py,
                            caller,
                            future_bits,
                        );
                    }
                    await_waiter_register(_py, caller, child);
                    let flags = (*header_from_obj_ptr(caller)).load_synchronized_flags();
                    if flags & (HEADER_FLAG_BLOCK_ON | HEADER_FLAG_SPAWN_RETAIN) != 0 {
                        let target = resolve_sleep_target(_py, child);
                        let _ = sleep_register_impl(_py, caller, target);
                    }
                } else {
                    // Completion, including an early host-stream completion,
                    // retires only the deadline owned by this awaited edge.
                    // The owned edge survives removal of the wake subscription
                    // by a completion published inside this poll. Another
                    // child's wait must survive an unrelated poll.
                    if crate::object::aux_header::object_frame_awaited_bits(caller) == future_bits {
                        crate::async_rt::scheduler::cancel_task_sleep(_py, caller);
                        crate::object::aux_header::object_replace_frame_awaited_owned(
                            _py, caller, 0,
                        );
                        await_waiter_clear(_py, caller);
                    }
                }
            }
        }
        result
    })
}

// All completion/cache/error paths return through the caller's continuation
// publication above. Scheduler wake subscriptions are separate from ownership.
fn poll_future_value(_py: &PyToken<'_>, future_bits: u64) -> i64 {
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
        if ((*header).load_synchronized_flags() & HEADER_FLAG_TASK_DONE) != 0
            && !crate::async_rt::awaitable::is_coroutine_wrapper_bits(future_bits)
        {
            if crate::async_rt::generators::is_native_coroutine_bits(future_bits) {
                return raise_exception::<i64>(
                    _py,
                    "RuntimeError",
                    "cannot reuse already awaited coroutine",
                );
            }
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
        if ((*header).load_metadata_flags() & HEADER_FLAG_COROUTINE) != 0
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
        if !current_task.is_null() {
            let current_cancelled = task_cancel_pending(current_task);
            if current_cancelled {
                task_take_cancel_pending(current_task);
                return raise_cancelled_with_message::<i64>(_py, current_task);
            }
        }
        let awaited_exception = if res != pending_bits_i64() && ptr != current_task {
            let guard = task_last_exceptions(_py).lock().unwrap();
            guard.get(&PtrSlot(ptr)).map(|exc_ptr| {
                let bits = MoltObject::from_ptr(exc_ptr.0).bits();
                inc_ref_bits(_py, bits);
                bits
            })
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
        if res != pending_bits_i64() && poll_pending && ptr != current_task {
            if let Some(exc_bits) = awaited_exception {
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
        if poll_fn == process_poll_fn_addr()
            && let Some(state) = process_task_state(_py, task_ptr)
        {
            state.cancel_wait();
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
    #[cfg(target_arch = "wasm32")]
    if poll_fn == process_poll_fn_addr() {
        // A host may publish exit only when polled. A scheduled process future
        // owns its own retry; an inline process future borrows its scheduled
        // awaiter. Never give both the child and its waiter a retry timer for
        // the same host operation. Native process workers publish wakes.
        let flags = unsafe { (*_header).load_synchronized_flags() };
        let owner = if flags & (HEADER_FLAG_BLOCK_ON | HEADER_FLAG_SPAWN_RETAIN) != 0 {
            future_ptr
        } else {
            task_ptr
        };
        return crate::async_rt::io_poller::register_host_progress_retry(_py, owner);
    }
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
        if unsafe { ((*task_header).load_synchronized_flags() & HEADER_FLAG_BLOCK_ON) != 0 } {
            let deadline =
                Instant::now() + Duration::from_secs_f64(ASYNC_SLEEP_YIELD_SECS.max(0.0));
            runtime_state(_py)
                .sleep_queue()
                .register_blocking(_py, task_ptr, deadline);
            return true;
        }
        runtime_state(_py).scheduler().defer_task_ptr(_py, task_ptr);
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
            if unsafe { ((*task_header).load_synchronized_flags() & HEADER_FLAG_BLOCK_ON) != 0 } {
                runtime_state(_py)
                    .sleep_queue()
                    .register_blocking(_py, task_ptr, deadline);
                return true;
            }
            crate::async_rt::scheduler::register_task_sleep(_py, task_ptr, deadline);
            return true;
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
            if unsafe { ((*task_header).load_synchronized_flags() & HEADER_FLAG_BLOCK_ON) != 0 } {
                runtime_state(_py)
                    .sleep_queue()
                    .register_blocking(_py, task_ptr, deadline);
                return true;
            }
            crate::async_rt::scheduler::register_task_sleep(_py, task_ptr, deadline);
            return true;
        }
        wake_task_ptr(_py, task_ptr);
        return true;
    }
    let task_header = unsafe { header_from_obj_ptr(task_ptr) };
    if unsafe { ((*task_header).load_synchronized_flags() & HEADER_FLAG_BLOCK_ON) != 0 } {
        runtime_state(_py)
            .sleep_queue()
            .register_blocking(_py, task_ptr, deadline);
        return true;
    }
    crate::async_rt::scheduler::register_task_sleep(_py, task_ptr, deadline);
    true
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
                    if exception_pending(_py) {
                        dec_ref_bits(_py, float_bits);
                        return MoltObject::none().bits() as i64;
                    }
                    let delay_secs =
                        crate::object::ops::as_float_extended(obj_from_bits(float_bits))
                            .expect("float conversion returned a float without an exception");
                    dec_ref_bits(_py, float_bits);
                    delay_secs
                } else {
                    0.0
                };
                let delay_secs = if delay_secs.is_finite() && delay_secs > 0.0 {
                    delay_secs
                } else {
                    0.0
                };
                let immediate = delay_secs <= 0.0;
                let displaced_delay = if payload_len >= 1 {
                    let deadline = if immediate {
                        ASYNC_SLEEP_YIELD_SENTINEL
                    } else {
                        crate::monotonic_now_secs(_py) + delay_secs
                    };
                    crate::object::payload_refs::exchange_owned(
                        _py,
                        _obj_ptr,
                        0,
                        MoltObject::from_float(deadline).bits(),
                    )
                } else {
                    MoltObject::none().bits()
                };
                // A delay finalizer may reenter this future. Publish both the
                // deadline and poll state before releasing its displaced owner.
                crate::object::object_set_state(_obj_ptr, 1);
                dec_ref_bits(_py, displaced_delay);
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
                let raw = molt_anext(iter_bits);
                let await_bits = if exception_pending(_py) {
                    MoltObject::none().bits()
                } else {
                    crate::molt_get_awaitable(raw)
                };
                dec_ref_bits(_py, raw);
                *payload_ptr.add(2) = await_bits;
                crate::object::object_set_state(_obj_ptr, 1);
            }
            let mut result = if exception_pending(_py) {
                MoltObject::none().bits() as i64
            } else {
                molt_future_poll(*payload_ptr.add(2))
            };
            if result == pending_bits_i64() && !exception_pending(_py) {
                return result;
            }
            if exception_pending(_py) {
                let exception = molt_exception_last();
                if crate::builtins::exceptions::exception_matches_builtin_name(
                    _py,
                    exception,
                    "StopAsyncIteration",
                ) {
                    molt_exception_clear();
                    dec_ref_bits(_py, result as u64);
                    inc_ref_bits(_py, default_bits);
                    result = default_bits as i64;
                }
                dec_ref_bits(_py, exception);
            }
            crate::object::payload_refs::clear_prefix::<3>(_py, _obj_ptr);
            result
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
            let flags = (*header).load_synchronized_flags();
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
mod sleep_payload_tests {
    use super::*;
    use std::sync::atomic::{AtomicU64, Ordering};

    static OWNER: AtomicU64 = AtomicU64::new(0);
    static CONVERSION_ERROR: AtomicU64 = AtomicU64::new(0);
    static RETIRED_PREFIX: AtomicU64 = AtomicU64::new(0);
    static FINALIZER_CALLS: AtomicU64 = AtomicU64::new(0);
    static OBSERVED_STATE: AtomicU64 = AtomicU64::new(0);
    static OBSERVED_SLOTS: [AtomicU64; 3] = [const { AtomicU64::new(0) }; 3];
    static REENTRY_RESULT: AtomicU64 = AtomicU64::new(0);

    // RuntimeTestTransaction serializes these callbacks. Disarm their borrowed
    // pointers before releasing the transaction, including on a Rust assertion.
    struct CallbackScope;

    impl CallbackScope {
        fn new() -> Self {
            let scope = Self;
            scope.reset();
            scope
        }

        fn reset(&self) {
            OWNER.store(0, Ordering::Relaxed);
            CONVERSION_ERROR.store(0, Ordering::Relaxed);
            RETIRED_PREFIX.store(0, Ordering::Relaxed);
            FINALIZER_CALLS.store(0, Ordering::Relaxed);
            OBSERVED_STATE.store(0, Ordering::Relaxed);
            for slot in &OBSERVED_SLOTS {
                slot.store(0, Ordering::Relaxed);
            }
            REENTRY_RESULT.store(0, Ordering::Relaxed);
        }
    }

    impl Drop for CallbackScope {
        fn drop(&mut self) {
            self.reset();
        }
    }

    extern "C" fn delay_float(_self: u64) -> u64 {
        let error = CONVERSION_ERROR.load(Ordering::Relaxed);
        if error != 0 {
            return crate::molt_exception_set_last(error);
        }
        MoltObject::from_float(0.0).bits()
    }

    extern "C" fn delay_finalizer(_self: u64) -> u64 {
        crate::with_gil_entry_nopanic!(py, {
            // Consume the observation before any reentrant runtime call. The
            // callback records facts; assertions belong on the Rust test side
            // of the non-unwinding C ABI boundary.
            let owner = OWNER.swap(0, Ordering::Relaxed);
            if owner != 0 {
                let ptr = ptr_from_bits(owner);
                unsafe {
                    OBSERVED_STATE
                        .store(crate::object::object_state(ptr) as u64, Ordering::Relaxed);
                    let prefix = RETIRED_PREFIX.load(Ordering::Relaxed);
                    if prefix == 0 {
                        OBSERVED_SLOTS[0].store(*ptr.cast::<u64>(), Ordering::Relaxed);
                        // Reenter the same poll before replacing the deadline.
                        let result = molt_async_sleep_poll(owner) as u64;
                        REENTRY_RESULT.store(result, Ordering::Relaxed);
                        dec_ref_bits(py, result);
                    } else {
                        for (offset, observed) in OBSERVED_SLOTS.iter().enumerate() {
                            observed.store(*ptr.cast::<u64>().add(offset), Ordering::Relaxed);
                        }
                    }
                    crate::object::payload_refs::store_borrowed(
                        py,
                        ptr,
                        0,
                        MoltObject::from_float(-2.0).bits(),
                    );
                }
                FINALIZER_CALLS.fetch_add(1, Ordering::Relaxed);
            }
            MoltObject::none().bits()
        })
    }

    fn delay_class(py: &PyToken<'_>) -> u64 {
        let name = attr_name_bits_from_bytes(py, b"SleepPayloadDelay").unwrap();
        let class = crate::molt_class_new(name);
        dec_ref_bits(py, name);
        crate::molt_class_set_base(class, builtin_classes(py).object);
        for (name, address) in [
            (b"__float__".as_slice(), delay_float as *const ()),
            (b"__del__".as_slice(), delay_finalizer as *const ()),
        ] {
            let name = attr_name_bits_from_bytes(py, name).unwrap();
            let function = crate::object::builders::alloc_function_obj(
                py,
                crate::provenance::abi::expose_function_address(address),
                1,
            );
            assert!(!function.is_null());
            unsafe { crate::object::layout::function_set_call_target_ptr(function, address) };
            let function = MoltObject::from_ptr(function).bits();
            crate::molt_set_attr_name(class, name, function);
            dec_ref_bits(py, function);
            dec_ref_bits(py, name);
        }
        unsafe {
            crate::object::class_finish_definition(py, ptr_from_bits(class)).unwrap();
        }
        assert!(!exception_pending(py));
        class
    }

    fn delay_instance(py: &PyToken<'_>, class: u64) -> u64 {
        let size = unsafe {
            crate::object::layout::class_cached_layout_size(ptr_from_bits(class)).unwrap()
        };
        let instance = crate::object::builders::alloc_class_instance(py, size, class);
        assert!(!obj_from_bits(instance).is_none());
        unsafe { crate::object::gc::gc_publish_initialized(py, ptr_from_bits(instance)) };
        instance
    }

    fn owners(bits: u64) -> u64 {
        unsafe { (*header_from_obj_ptr(ptr_from_bits(bits))).ref_count_snapshot() as u64 }
    }

    #[test]
    fn payload_reference_sleep_conversion_releases_delay_and_heap_float_result_owners() {
        let _transaction = crate::test_support::RuntimeTestTransaction::new();
        crate::with_gil_entry_nopanic!(py, {
            for delay in [
                MoltObject::from_ptr(crate::object::builders::alloc_string_nointern(py, b"0"))
                    .bits(),
                crate::object::ops::float_result_bits(py, f64::NAN),
            ] {
                let before = owners(delay);
                assert_eq!(
                    before, 1,
                    "ownership regression requires a mortal allocation"
                );
                let future = molt_async_sleep(delay, MoltObject::from_int(41).bits());
                assert_eq!(owners(delay), before + 1);
                assert_eq!(unsafe { molt_async_sleep_poll(future) }, pending_bits_i64());
                assert_eq!(owners(delay), before);
                assert_eq!(
                    unsafe { molt_async_sleep_poll(future) } as u64,
                    MoltObject::from_int(41).bits()
                );
                dec_ref_bits(py, future);
                assert_eq!(owners(delay), before);
                dec_ref_bits(py, delay);
                assert!(!exception_pending(py));
            }
        });
    }

    #[test]
    fn payload_reference_sleep_conversion_preserves_exact_failure_and_unpublished_payload() {
        let _transaction = crate::test_support::RuntimeTestTransaction::new();
        crate::with_gil_entry_nopanic!(py, {
            let _callbacks = CallbackScope::new();
            let class = delay_class(py);
            let delay = delay_instance(py, class);
            let future = molt_async_sleep(delay, MoltObject::from_int(41).bits());
            let before = owners(delay);
            let raised = raise_exception::<u64>(py, "ValueError", "delay conversion failed");
            dec_ref_bits(py, raised);
            let original = crate::builtins::exceptions::molt_exception_last_pending();
            crate::molt_exception_clear();
            CONVERSION_ERROR.store(original, Ordering::Relaxed);
            let result = unsafe { molt_async_sleep_poll(future) } as u64;
            assert!(obj_from_bits(result).is_none());
            let observed = crate::builtins::exceptions::molt_exception_last_pending();
            assert_eq!(observed, original);
            assert_eq!(owners(delay), before);
            unsafe {
                assert_eq!(crate::object::object_state(ptr_from_bits(future)), 0);
                assert_eq!(*ptr_from_bits(future).cast::<u64>(), delay);
            }
            CONVERSION_ERROR.store(0, Ordering::Relaxed);
            crate::molt_exception_clear();
            for bits in [result, observed, original, future, delay, class] {
                dec_ref_bits(py, bits);
            }
            assert!(!exception_pending(py));
        });
    }

    #[test]
    fn payload_reference_sleep_publication_and_anext_retirement_precede_finalizer_reentry() {
        let _transaction = crate::test_support::RuntimeTestTransaction::new();
        crate::with_gil_entry_nopanic!(py, {
            let class = delay_class(py);
            for retired_prefix in [0, 3] {
                let _callbacks = CallbackScope::new();
                RETIRED_PREFIX.store(retired_prefix, Ordering::Relaxed);
                let delay = delay_instance(py, class);
                let future = if retired_prefix == 0 {
                    molt_async_sleep(delay, MoltObject::from_int(41).bits())
                } else {
                    let ready = molt_promise_new();
                    assert!(!ptr_from_bits(ready).is_null());
                    unsafe { molt_promise_set_result(ready, MoltObject::from_int(41).bits()) };
                    assert!(!exception_pending(py));
                    let wrapper = molt_future_new(
                        anext_default_poll_fn_addr(),
                        (3 * std::mem::size_of::<u64>()) as u64,
                    );
                    let ptr = ptr_from_bits(wrapper);
                    assert!(!ptr.is_null());
                    unsafe {
                        crate::object::payload_refs::store_borrowed(py, ptr, 0, delay);
                        crate::object::payload_refs::store_borrowed(
                            py,
                            ptr,
                            std::mem::size_of::<u64>(),
                            MoltObject::from_int(99).bits(),
                        );
                        crate::object::payload_refs::store_owned(
                            py,
                            ptr,
                            2 * std::mem::size_of::<u64>(),
                            ready,
                        );
                        crate::object::object_set_state(ptr, 1);
                    }
                    wrapper
                };
                dec_ref_bits(py, delay);
                OWNER.store(future, Ordering::Relaxed);
                let result = unsafe {
                    if retired_prefix == 0 {
                        molt_async_sleep_poll(future)
                    } else {
                        molt_anext_default_poll(future)
                    }
                };
                assert_eq!(
                    result,
                    if retired_prefix == 0 {
                        pending_bits_i64()
                    } else {
                        MoltObject::from_int(41).bits() as i64
                    }
                );
                assert_eq!(FINALIZER_CALLS.load(Ordering::Relaxed), 1);
                assert_eq!(OBSERVED_STATE.load(Ordering::Relaxed), 1);
                if retired_prefix == 0 {
                    assert_eq!(
                        OBSERVED_SLOTS[0].load(Ordering::Relaxed),
                        MoltObject::from_float(ASYNC_SLEEP_YIELD_SENTINEL).bits()
                    );
                    assert_eq!(
                        REENTRY_RESULT.load(Ordering::Relaxed),
                        MoltObject::from_int(41).bits()
                    );
                } else {
                    for observed in &OBSERVED_SLOTS {
                        assert_eq!(observed.load(Ordering::Relaxed), MoltObject::none().bits());
                    }
                }
                assert_eq!(
                    unsafe { *ptr_from_bits(future).cast::<u64>() },
                    MoltObject::from_float(-2.0).bits()
                );
                OWNER.store(0, Ordering::Relaxed);
                dec_ref_bits(py, future);
                assert!(!exception_pending(py));
            }
            RETIRED_PREFIX.store(0, Ordering::Relaxed);
            dec_ref_bits(py, class);
        });
    }

    #[test]
    fn sleep_callback_observations_survive_bad_publication_and_disarm_after_unwind() {
        let _transaction = crate::test_support::RuntimeTestTransaction::new();
        crate::with_gil_entry_nopanic!(py, {
            let class = delay_class(py);
            let delay = delay_instance(py, class);
            let future = molt_future_new(
                anext_default_poll_fn_addr(),
                (3 * std::mem::size_of::<u64>()) as u64,
            );
            let ptr = ptr_from_bits(future);
            assert!(!ptr.is_null());
            let unexpected = MoltObject::from_int(73).bits();
            unsafe { crate::object::payload_refs::store_borrowed(py, ptr, 0, unexpected) };
            let failure = crate::test_support::catch_expected_unwind(|| {
                let _callbacks = CallbackScope::new();
                RETIRED_PREFIX.store(3, Ordering::Relaxed);
                OWNER.store(future, Ordering::Relaxed);
                let result = delay_finalizer(delay);
                dec_ref_bits(py, result);
                assert_eq!(OBSERVED_STATE.load(Ordering::Relaxed), 0);
                assert_eq!(OBSERVED_SLOTS[0].load(Ordering::Relaxed), unexpected);
                assert_eq!(OWNER.load(Ordering::Relaxed), 0);
                assert_eq!(FINALIZER_CALLS.load(Ordering::Relaxed), 1);
                // Leave both borrowed callback inputs armed when Rust unwinds.
                OWNER.store(future, Ordering::Relaxed);
                CONVERSION_ERROR.store(delay, Ordering::Relaxed);
                panic!("callback scope rollback control");
            });
            assert_eq!(
                failure
                    .expect_err("rollback control must unwind")
                    .downcast_ref::<&str>(),
                Some(&"callback scope rollback control")
            );
            assert_eq!(OWNER.load(Ordering::Relaxed), 0);
            assert_eq!(CONVERSION_ERROR.load(Ordering::Relaxed), 0);
            assert_eq!(RETIRED_PREFIX.load(Ordering::Relaxed), 0);
            for bits in [future, delay, class] {
                dec_ref_bits(py, bits);
            }
            assert_eq!(FINALIZER_CALLS.load(Ordering::Relaxed), 0);
            assert!(!exception_pending(py));
        });
    }
}
