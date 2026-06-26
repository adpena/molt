//! Asyncio wait/gather/wait_for task combinator wrapper futures.
//!
//! Owns the wait-family payload layouts, polling state machines,
//! cancellation propagation, result assembly, and drop hooks.

use super::*;

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
