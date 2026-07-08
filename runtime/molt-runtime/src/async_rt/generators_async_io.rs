use super::*;
#[path = "generators_async_stream.rs"]
mod generators_async_stream;
pub(crate) use generators_async_stream::*;
#[path = "generators_async_socket.rs"]
mod generators_async_socket;
pub(crate) use generators_async_socket::*;

const ASYNCIO_SOCKET_IO_EVENT_READ: i64 = 1;
const ASYNCIO_SOCKET_IO_EVENT_WRITE: i64 = 2;
const ASYNCIO_TIMER_SLOT_HANDLE: usize = 0;
const ASYNCIO_TIMER_SLOT_DELAY: usize = 1;
const ASYNCIO_TIMER_SLOT_LOOP: usize = 2;
const ASYNCIO_TIMER_SLOT_SCHEDULED: usize = 3;
const ASYNCIO_TIMER_SLOT_READY_LOCK: usize = 4;
const ASYNCIO_TIMER_SLOT_READY: usize = 5;
const ASYNCIO_TIMER_SLOT_WAIT: usize = 6;
const ASYNCIO_FD_WATCHER_SLOT_REGISTRY: usize = 0;
const ASYNCIO_FD_WATCHER_SLOT_FILENO: usize = 1;
const ASYNCIO_FD_WATCHER_SLOT_CALLBACK: usize = 2;
const ASYNCIO_FD_WATCHER_SLOT_ARGS: usize = 3;
const ASYNCIO_FD_WATCHER_SLOT_EVENTS: usize = 4;
const ASYNCIO_FD_WATCHER_SLOT_WAIT: usize = 5;
const ASYNCIO_SERVER_ACCEPT_SLOT_SOCK: usize = 0;
const ASYNCIO_SERVER_ACCEPT_SLOT_CALLBACK: usize = 1;
const ASYNCIO_SERVER_ACCEPT_SLOT_LOOP: usize = 2;
const ASYNCIO_SERVER_ACCEPT_SLOT_READER_CTOR: usize = 3;
const ASYNCIO_SERVER_ACCEPT_SLOT_WRITER_CTOR: usize = 4;
const ASYNCIO_SERVER_ACCEPT_SLOT_CLOSED_PROBE: usize = 5;
const ASYNCIO_SERVER_ACCEPT_SLOT_FD: usize = 6;
const ASYNCIO_SERVER_ACCEPT_SLOT_WAIT: usize = 7;
const ASYNCIO_READY_RUNNER_SLOT_LOOP: usize = 0;
const ASYNCIO_READY_RUNNER_SLOT_READY_LOCK: usize = 1;
const ASYNCIO_READY_RUNNER_SLOT_READY: usize = 2;
const ASYNCIO_READY_RUNNER_SLOT_WAIT: usize = 3;

unsafe fn asyncio_fd_ready_select_once(
    _py: &PyToken<'_>,
    fileno_bits: u64,
    events_bits: u64,
) -> Option<bool> {
    let events = to_i64(obj_from_bits(events_bits)).unwrap_or(0);
    let empty_ptr = alloc_list(_py, &[]);
    if empty_ptr.is_null() {
        return None;
    }
    let empty_bits = MoltObject::from_ptr(empty_ptr).bits();
    let read_bits = if (events & ASYNCIO_SOCKET_IO_EVENT_READ) != 0 {
        let ptr = alloc_list(_py, &[fileno_bits]);
        if ptr.is_null() {
            dec_ref_bits(_py, empty_bits);
            return None;
        }
        MoltObject::from_ptr(ptr).bits()
    } else {
        inc_ref_bits(_py, empty_bits);
        empty_bits
    };
    let write_bits = if (events & ASYNCIO_SOCKET_IO_EVENT_WRITE) != 0 {
        let ptr = alloc_list(_py, &[fileno_bits]);
        if ptr.is_null() {
            dec_ref_bits(_py, read_bits);
            dec_ref_bits(_py, empty_bits);
            return None;
        }
        MoltObject::from_ptr(ptr).bits()
    } else {
        inc_ref_bits(_py, empty_bits);
        empty_bits
    };
    let timeout_bits = MoltObject::from_float(0.0).bits();
    let select_out_bits =
        crate::molt_select_select(read_bits, write_bits, empty_bits, timeout_bits);
    dec_ref_bits(_py, read_bits);
    dec_ref_bits(_py, write_bits);
    dec_ref_bits(_py, empty_bits);
    if exception_pending(_py) {
        return None;
    }
    let idx0 = MoltObject::from_int(0).bits();
    let idx1 = MoltObject::from_int(1).bits();
    let ready_read_bits = molt_getitem_method(select_out_bits, idx0);
    if exception_pending(_py) {
        if !obj_from_bits(select_out_bits).is_none() {
            dec_ref_bits(_py, select_out_bits);
        }
        return None;
    }
    let ready_write_bits = molt_getitem_method(select_out_bits, idx1);
    if exception_pending(_py) {
        if !obj_from_bits(ready_read_bits).is_none() {
            dec_ref_bits(_py, ready_read_bits);
        }
        if !obj_from_bits(select_out_bits).is_none() {
            dec_ref_bits(_py, select_out_bits);
        }
        return None;
    }
    let read_len_bits = molt_len(ready_read_bits);
    let write_len_bits = molt_len(ready_write_bits);
    let read_len = to_i64(obj_from_bits(read_len_bits)).unwrap_or(0);
    let write_len = to_i64(obj_from_bits(write_len_bits)).unwrap_or(0);
    if !obj_from_bits(read_len_bits).is_none() {
        dec_ref_bits(_py, read_len_bits);
    }
    if !obj_from_bits(write_len_bits).is_none() {
        dec_ref_bits(_py, write_len_bits);
    }
    if !obj_from_bits(ready_read_bits).is_none() {
        dec_ref_bits(_py, ready_read_bits);
    }
    if !obj_from_bits(ready_write_bits).is_none() {
        dec_ref_bits(_py, ready_write_bits);
    }
    if !obj_from_bits(select_out_bits).is_none() {
        dec_ref_bits(_py, select_out_bits);
    }
    if exception_pending(_py) {
        return None;
    }
    Some(read_len > 0 || write_len > 0)
}

unsafe fn asyncio_close_connection_best_effort(_py: &PyToken<'_>, conn_bits: u64) {
    unsafe {
        let close_bits = asyncio_call_method0(_py, conn_bits, b"close");
        if exception_pending(_py) {
            let exc_bits = molt_exception_last();
            molt_exception_clear();
            dec_ref_bits(_py, exc_bits);
        }
        if !obj_from_bits(close_bits).is_none() {
            dec_ref_bits(_py, close_bits);
        }
    }
}

unsafe fn asyncio_oserror_errno_from_exception(_py: &PyToken<'_>, exc_bits: u64) -> Option<i64> {
    unsafe {
        let exc_ptr = obj_from_bits(exc_bits).as_ptr()?;
        if object_type_id(exc_ptr) != TYPE_ID_EXCEPTION {
            return None;
        }
        let kind_bits = exception_kind_bits(exc_ptr);
        let kind = string_obj_to_owned(obj_from_bits(kind_bits)).unwrap_or_default();
        dec_ref_bits(_py, kind_bits);
        if kind == "BlockingIOError" {
            return Some(libc::EWOULDBLOCK as i64);
        }
        if kind == "InterruptedError" {
            return Some(libc::EINTR as i64);
        }
        let args_bits = exception_materialized_args_bits(_py, exc_ptr);
        let args_ptr = obj_from_bits(args_bits).as_ptr()?;
        if object_type_id(args_ptr) != TYPE_ID_TUPLE {
            return None;
        }
        let args = seq_vec_ref(args_ptr);
        args.first().and_then(|bits| to_i64(obj_from_bits(*bits)))
    }
}

fn asyncio_retryable_socket_errno(errno: i64) -> bool {
    errno == libc::EWOULDBLOCK as i64
        || errno == libc::EAGAIN as i64
        || errno == libc::EINTR as i64
        || errno == libc::EALREADY as i64
        || errno == libc::EINPROGRESS as i64
}

unsafe fn asyncio_pending_with_wait(
    _py: &PyToken<'_>,
    payload_ptr: *mut u64,
    slot_idx: usize,
    fd_bits: u64,
    io_events: i64,
) -> i64 {
    unsafe {
        let mut waiter_bits = *payload_ptr.add(slot_idx);
        if obj_from_bits(waiter_bits).is_none() {
            // Only int-like runtime objects are valid fd carriers here.
            // Raw bit reinterpretation can misread tagged sentinels (e.g. None) as fds.
            let fd = to_i64(obj_from_bits(fd_bits)).unwrap_or(-1);
            waiter_bits = if fd < 0 {
                molt_async_sleep(
                    MoltObject::from_float(0.0).bits(),
                    MoltObject::none().bits(),
                )
            } else {
                molt_io_wait_new(
                    MoltObject::from_int(fd).bits(),
                    MoltObject::from_int(io_events).bits(),
                    MoltObject::none().bits(),
                )
            };
            if obj_from_bits(waiter_bits).is_none() {
                return waiter_bits as i64;
            }
            *payload_ptr.add(slot_idx) = waiter_bits;
        }
        let wait_res = molt_future_poll(waiter_bits);
        if wait_res == pending_bits_i64() {
            return pending_bits_i64();
        }
        if exception_pending(_py) {
            return wait_res;
        }
        asyncio_drop_slot_ref(_py, payload_ptr, slot_idx);
        pending_bits_i64()
    }
}

unsafe fn asyncio_pending_with_connect_retry(
    _py: &PyToken<'_>,
    payload_ptr: *mut u64,
    slot_idx: usize,
    fd_bits: u64,
) -> i64 {
    unsafe {
        asyncio_pending_with_wait(
            _py,
            payload_ptr,
            slot_idx,
            fd_bits,
            ASYNCIO_SOCKET_IO_EVENT_WRITE,
        )
    }
}

unsafe fn asyncio_socket_is_connected(_py: &PyToken<'_>, sock_bits: u64) -> bool {
    unsafe {
        let Some(peer_bits) = asyncio_call_method0_allow_missing(_py, sock_bits, b"getpeername")
        else {
            if exception_pending(_py) {
                asyncio_clear_pending_exception(_py);
            }
            return false;
        };
        if exception_pending(_py) {
            if !obj_from_bits(peer_bits).is_none() {
                dec_ref_bits(_py, peer_bits);
            }
            asyncio_clear_pending_exception(_py);
            return false;
        }
        if !obj_from_bits(peer_bits).is_none() {
            dec_ref_bits(_py, peer_bits);
        }
        true
    }
}

fn asyncio_msg_dontwait() -> i64 {
    #[cfg(unix)]
    {
        libc::MSG_DONTWAIT as i64
    }
    #[cfg(not(unix))]
    {
        0
    }
}

unsafe fn asyncio_drop_payload_slots(_py: &PyToken<'_>, payload_ptr: *mut u64, slots: usize) {
    unsafe {
        for idx in 0..slots {
            asyncio_drop_slot_ref(_py, payload_ptr, idx);
        }
    }
}

/// # Safety
/// - All arguments must be valid runtime objects.
#[unsafe(no_mangle)]
pub extern "C" fn molt_asyncio_timer_handle_new(
    handle_bits: u64,
    delay_bits: u64,
    loop_bits: u64,
    scheduled_bits: u64,
    ready_lock_bits: u64,
    ready_bits: u64,
) -> u64 {
    crate::with_gil_entry_nopanic!(_py, {
        let obj_bits = molt_future_new(
            asyncio_timer_handle_poll_fn_addr(),
            (7 * std::mem::size_of::<u64>()) as u64,
        );
        if obj_from_bits(obj_bits).is_none() {
            return obj_bits;
        }
        let Some(obj_ptr) = resolve_obj_ptr(obj_bits) else {
            return MoltObject::none().bits();
        };
        let payload_ptr = obj_ptr as *mut u64;
        unsafe {
            *payload_ptr.add(ASYNCIO_TIMER_SLOT_HANDLE) = handle_bits;
            *payload_ptr.add(ASYNCIO_TIMER_SLOT_DELAY) = delay_bits;
            *payload_ptr.add(ASYNCIO_TIMER_SLOT_LOOP) = loop_bits;
            *payload_ptr.add(ASYNCIO_TIMER_SLOT_SCHEDULED) = scheduled_bits;
            *payload_ptr.add(ASYNCIO_TIMER_SLOT_READY_LOCK) = ready_lock_bits;
            *payload_ptr.add(ASYNCIO_TIMER_SLOT_READY) = ready_bits;
            *payload_ptr.add(ASYNCIO_TIMER_SLOT_WAIT) = MoltObject::none().bits();
        }
        inc_ref_bits(_py, handle_bits);
        inc_ref_bits(_py, delay_bits);
        inc_ref_bits(_py, loop_bits);
        inc_ref_bits(_py, scheduled_bits);
        inc_ref_bits(_py, ready_lock_bits);
        inc_ref_bits(_py, ready_bits);
        obj_bits
    })
}

/// # Safety
/// - All arguments must be valid runtime objects.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn molt_asyncio_timer_schedule(
    handle_bits: u64,
    delay_bits: u64,
    loop_bits: u64,
    scheduled_bits: u64,
    ready_lock_bits: u64,
    ready_bits: u64,
) -> u64 {
    unsafe {
        crate::with_gil_entry_nopanic!(_py, {
            let delay_obj = obj_from_bits(molt_float_from_obj(delay_bits));
            if exception_pending(_py) {
                return MoltObject::none().bits();
            }
            let delay = delay_obj.as_float().unwrap_or(0.0);
            if !delay.is_finite() || delay <= 0.0 {
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
                return MoltObject::none().bits();
            }

            let add_bits = asyncio_call_method1(_py, scheduled_bits, b"add", handle_bits);
            if exception_pending(_py) {
                return add_bits;
            }
            if !obj_from_bits(add_bits).is_none() {
                dec_ref_bits(_py, add_bits);
            }

            let timer_bits = molt_asyncio_timer_handle_new(
                handle_bits,
                delay_bits,
                loop_bits,
                scheduled_bits,
                ready_lock_bits,
                ready_bits,
            );
            if obj_from_bits(timer_bits).is_none() {
                return timer_bits;
            }
            let task_bits = asyncio_call_method1(_py, loop_bits, b"create_task", timer_bits);
            dec_ref_bits(_py, timer_bits);
            if exception_pending(_py) {
                return task_bits;
            }
            task_bits
        })
    }
}

/// # Safety
/// - All arguments must be valid runtime objects.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn molt_asyncio_timer_handle_cancel(
    scheduled_bits: u64,
    handle_bits: u64,
    timer_task_bits: u64,
) -> u64 {
    unsafe {
        crate::with_gil_entry_nopanic!(_py, {
            if !obj_from_bits(timer_task_bits).is_none() {
                let cancel_bits = asyncio_call_method0(_py, timer_task_bits, b"cancel");
                if exception_pending(_py) {
                    asyncio_clear_pending_exception(_py);
                } else if !obj_from_bits(cancel_bits).is_none() {
                    dec_ref_bits(_py, cancel_bits);
                }
            }
            let discard_bits = asyncio_call_method1(_py, scheduled_bits, b"discard", handle_bits);
            if exception_pending(_py) {
                asyncio_clear_pending_exception(_py);
            } else if !obj_from_bits(discard_bits).is_none() {
                dec_ref_bits(_py, discard_bits);
            }
            MoltObject::none().bits()
        })
    }
}

/// # Safety
/// - `obj_bits` must be a valid timer-handle wrapper future pointer.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn molt_asyncio_timer_handle_poll(obj_bits: u64) -> i64 {
    unsafe {
        crate::with_gil_entry_nopanic!(_py, {
            let obj_ptr = ptr_from_bits(obj_bits);
            if obj_ptr.is_null() {
                return MoltObject::none().bits() as i64;
            }
            let _header = header_from_obj_ptr(obj_ptr);
            let payload_bytes = crate::object::object_payload_size(obj_ptr);
            if payload_bytes < 7 * std::mem::size_of::<u64>() {
                return raise_exception::<i64>(
                    _py,
                    "RuntimeError",
                    "invalid asyncio timer payload",
                );
            }
            let payload_ptr = obj_ptr as *mut u64;
            if crate::object::object_state(obj_ptr) == 0 {
                let delay_bits = *payload_ptr.add(ASYNCIO_TIMER_SLOT_DELAY);
                let delay_obj = obj_from_bits(molt_float_from_obj(delay_bits));
                if exception_pending(_py) {
                    return MoltObject::none().bits() as i64;
                }
                let delay = delay_obj.as_float().unwrap_or(0.0);
                if delay.is_finite() && delay > 0.0 {
                    let waiter_bits = molt_async_sleep(
                        MoltObject::from_float(delay).bits(),
                        MoltObject::none().bits(),
                    );
                    if obj_from_bits(waiter_bits).is_none() {
                        return waiter_bits as i64;
                    }
                    *payload_ptr.add(ASYNCIO_TIMER_SLOT_WAIT) = waiter_bits;
                }
                crate::object::object_set_state(obj_ptr, 1);
            }

            let wait_bits = *payload_ptr.add(ASYNCIO_TIMER_SLOT_WAIT);
            if !obj_from_bits(wait_bits).is_none() {
                let wait_res = molt_future_poll(wait_bits);
                if wait_res == pending_bits_i64() {
                    return pending_bits_i64();
                }
                if exception_pending(_py) {
                    return wait_res;
                }
                asyncio_drop_slot_ref(_py, payload_ptr, ASYNCIO_TIMER_SLOT_WAIT);
            }

            let handle_bits = *payload_ptr.add(ASYNCIO_TIMER_SLOT_HANDLE);
            let scheduled_bits = *payload_ptr.add(ASYNCIO_TIMER_SLOT_SCHEDULED);
            let discard_bits = asyncio_call_method1(_py, scheduled_bits, b"discard", handle_bits);
            if exception_pending(_py) {
                return discard_bits as i64;
            }
            if !obj_from_bits(discard_bits).is_none() {
                dec_ref_bits(_py, discard_bits);
            }
            let cancelled = match asyncio_method_truthy(_py, handle_bits, b"cancelled") {
                Some(flag) => flag,
                None => return MoltObject::none().bits() as i64,
            };
            if cancelled {
                asyncio_drop_payload_slots(_py, payload_ptr, 7);
                return MoltObject::none().bits() as i64;
            }
            let run_bits = asyncio_call_method0(_py, handle_bits, b"_run");
            if exception_pending(_py) {
                return run_bits as i64;
            }
            if !obj_from_bits(run_bits).is_none() {
                dec_ref_bits(_py, run_bits);
            }
            asyncio_drop_payload_slots(_py, payload_ptr, 7);
            MoltObject::none().bits() as i64
        })
    }
}

/// # Safety
/// - All arguments must be valid runtime objects.
#[unsafe(no_mangle)]
pub extern "C" fn molt_asyncio_fd_watcher_new(
    registry_bits: u64,
    fileno_bits: u64,
    callback_bits: u64,
    args_bits: u64,
    events_bits: u64,
) -> u64 {
    crate::with_gil_entry_nopanic!(_py, {
        let obj_bits = molt_future_new(
            asyncio_fd_watcher_poll_fn_addr(),
            (6 * std::mem::size_of::<u64>()) as u64,
        );
        if obj_from_bits(obj_bits).is_none() {
            return obj_bits;
        }
        let Some(obj_ptr) = resolve_obj_ptr(obj_bits) else {
            return MoltObject::none().bits();
        };
        let payload_ptr = obj_ptr as *mut u64;
        unsafe {
            *payload_ptr.add(ASYNCIO_FD_WATCHER_SLOT_REGISTRY) = registry_bits;
            *payload_ptr.add(ASYNCIO_FD_WATCHER_SLOT_FILENO) = fileno_bits;
            *payload_ptr.add(ASYNCIO_FD_WATCHER_SLOT_CALLBACK) = callback_bits;
            *payload_ptr.add(ASYNCIO_FD_WATCHER_SLOT_ARGS) = args_bits;
            *payload_ptr.add(ASYNCIO_FD_WATCHER_SLOT_EVENTS) = events_bits;
            *payload_ptr.add(ASYNCIO_FD_WATCHER_SLOT_WAIT) = MoltObject::none().bits();
        }
        inc_ref_bits(_py, registry_bits);
        inc_ref_bits(_py, fileno_bits);
        inc_ref_bits(_py, callback_bits);
        inc_ref_bits(_py, args_bits);
        inc_ref_bits(_py, events_bits);
        obj_bits
    })
}

/// # Safety
/// - All arguments must be valid runtime objects.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn molt_asyncio_fd_watcher_register(
    loop_bits: u64,
    registry_bits: u64,
    fileno_bits: u64,
    callback_bits: u64,
    args_bits: u64,
    events_bits: u64,
) -> u64 {
    unsafe {
        crate::with_gil_entry_nopanic!(_py, {
            let old_entry_bits = asyncio_call_method2(
                _py,
                registry_bits,
                b"pop",
                fileno_bits,
                MoltObject::none().bits(),
            );
            if exception_pending(_py) {
                return old_entry_bits;
            }
            if !obj_from_bits(old_entry_bits).is_none() {
                let task_bits = molt_getitem_method(old_entry_bits, MoltObject::from_int(2).bits());
                if exception_pending(_py) {
                    asyncio_clear_pending_exception(_py);
                } else {
                    let cancel_bits = asyncio_call_method0(_py, task_bits, b"cancel");
                    if exception_pending(_py) {
                        asyncio_clear_pending_exception(_py);
                    } else if !obj_from_bits(cancel_bits).is_none() {
                        dec_ref_bits(_py, cancel_bits);
                    }
                    if !obj_from_bits(task_bits).is_none() {
                        dec_ref_bits(_py, task_bits);
                    }
                }
                dec_ref_bits(_py, old_entry_bits);
            }

            let pending_entry_ptr =
                alloc_tuple(_py, &[callback_bits, args_bits, MoltObject::none().bits()]);
            if pending_entry_ptr.is_null() {
                return MoltObject::none().bits();
            }
            let pending_entry_bits = MoltObject::from_ptr(pending_entry_ptr).bits();
            let pending_set_bits = asyncio_call_method2(
                _py,
                registry_bits,
                b"__setitem__",
                fileno_bits,
                pending_entry_bits,
            );
            if !obj_from_bits(pending_entry_bits).is_none() {
                dec_ref_bits(_py, pending_entry_bits);
            }
            if exception_pending(_py) {
                return pending_set_bits;
            }
            if !obj_from_bits(pending_set_bits).is_none() {
                dec_ref_bits(_py, pending_set_bits);
            }

            let watcher_bits = molt_asyncio_fd_watcher_new(
                registry_bits,
                fileno_bits,
                callback_bits,
                args_bits,
                events_bits,
            );
            if obj_from_bits(watcher_bits).is_none() {
                let cleanup_bits = asyncio_call_method2(
                    _py,
                    registry_bits,
                    b"pop",
                    fileno_bits,
                    MoltObject::none().bits(),
                );
                if !obj_from_bits(cleanup_bits).is_none() {
                    dec_ref_bits(_py, cleanup_bits);
                }
                return watcher_bits;
            }
            let task_bits = asyncio_call_method1(_py, loop_bits, b"create_task", watcher_bits);
            dec_ref_bits(_py, watcher_bits);
            if exception_pending(_py) {
                let cleanup_bits = asyncio_call_method2(
                    _py,
                    registry_bits,
                    b"pop",
                    fileno_bits,
                    MoltObject::none().bits(),
                );
                if !obj_from_bits(cleanup_bits).is_none() {
                    dec_ref_bits(_py, cleanup_bits);
                }
                return task_bits;
            }
            let entry_ptr = alloc_tuple(_py, &[callback_bits, args_bits, task_bits]);
            if entry_ptr.is_null() {
                let cleanup_bits = asyncio_call_method2(
                    _py,
                    registry_bits,
                    b"pop",
                    fileno_bits,
                    MoltObject::none().bits(),
                );
                if !obj_from_bits(cleanup_bits).is_none() {
                    dec_ref_bits(_py, cleanup_bits);
                }
                if !obj_from_bits(task_bits).is_none() {
                    dec_ref_bits(_py, task_bits);
                }
                return MoltObject::none().bits();
            }
            let entry_bits = MoltObject::from_ptr(entry_ptr).bits();
            let setitem_bits =
                asyncio_call_method2(_py, registry_bits, b"__setitem__", fileno_bits, entry_bits);
            if !obj_from_bits(entry_bits).is_none() {
                dec_ref_bits(_py, entry_bits);
            }
            if !obj_from_bits(task_bits).is_none() {
                dec_ref_bits(_py, task_bits);
            }
            if exception_pending(_py) {
                return setitem_bits;
            }
            if !obj_from_bits(setitem_bits).is_none() {
                dec_ref_bits(_py, setitem_bits);
            }
            MoltObject::none().bits()
        })
    }
}

/// # Safety
/// - All arguments must be valid runtime objects.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn molt_asyncio_fd_watcher_unregister(
    registry_bits: u64,
    fileno_bits: u64,
) -> u64 {
    unsafe {
        crate::with_gil_entry_nopanic!(_py, {
            let entry_bits = asyncio_call_method2(
                _py,
                registry_bits,
                b"pop",
                fileno_bits,
                MoltObject::none().bits(),
            );
            if exception_pending(_py) {
                return entry_bits;
            }
            if obj_from_bits(entry_bits).is_none() {
                return MoltObject::from_bool(false).bits();
            }

            let task_bits = molt_getitem_method(entry_bits, MoltObject::from_int(2).bits());
            if exception_pending(_py) {
                dec_ref_bits(_py, entry_bits);
                return task_bits;
            }
            if !obj_from_bits(task_bits).is_none() {
                let cancel_bits = asyncio_call_method0(_py, task_bits, b"cancel");
                if exception_pending(_py) {
                    asyncio_clear_pending_exception(_py);
                } else if !obj_from_bits(cancel_bits).is_none() {
                    dec_ref_bits(_py, cancel_bits);
                }
                dec_ref_bits(_py, task_bits);
            }
            dec_ref_bits(_py, entry_bits);
            MoltObject::from_bool(true).bits()
        })
    }
}

/// # Safety
/// - `obj_bits` must be a valid fd watcher wrapper future pointer.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn molt_asyncio_fd_watcher_poll(obj_bits: u64) -> i64 {
    unsafe {
        crate::with_gil_entry_nopanic!(_py, {
            let obj_ptr = ptr_from_bits(obj_bits);
            if obj_ptr.is_null() {
                return MoltObject::none().bits() as i64;
            }
            let _header = header_from_obj_ptr(obj_ptr);
            let payload_bytes = crate::object::object_payload_size(obj_ptr);
            if payload_bytes < 6 * std::mem::size_of::<u64>() {
                return raise_exception::<i64>(
                    _py,
                    "RuntimeError",
                    "invalid asyncio fd watcher payload",
                );
            }
            let payload_ptr = obj_ptr as *mut u64;
            let registry_bits = *payload_ptr.add(ASYNCIO_FD_WATCHER_SLOT_REGISTRY);
            let fileno_bits = *payload_ptr.add(ASYNCIO_FD_WATCHER_SLOT_FILENO);

            let contains_bits =
                asyncio_call_method1(_py, registry_bits, b"__contains__", fileno_bits);
            if exception_pending(_py) {
                return contains_bits as i64;
            }
            let still_registered = is_truthy(_py, obj_from_bits(contains_bits));
            if !obj_from_bits(contains_bits).is_none() {
                dec_ref_bits(_py, contains_bits);
            }
            if !still_registered {
                asyncio_drop_payload_slots(_py, payload_ptr, 6);
                return MoltObject::none().bits() as i64;
            }

            let mut waiter_bits = *payload_ptr.add(ASYNCIO_FD_WATCHER_SLOT_WAIT);
            if obj_from_bits(waiter_bits).is_none() {
                let events_bits = *payload_ptr.add(ASYNCIO_FD_WATCHER_SLOT_EVENTS);
                waiter_bits = molt_io_wait_new(fileno_bits, events_bits, MoltObject::none().bits());
                if obj_from_bits(waiter_bits).is_none() {
                    if exception_pending(_py) {
                        let exc_bits = asyncio_take_pending_exception_bits(_py);
                        let fatal = asyncio_exception_is_fatal_base(_py, exc_bits);
                        if fatal {
                            let raised = molt_raise(exc_bits);
                            dec_ref_bits(_py, exc_bits);
                            return raised as i64;
                        }
                        dec_ref_bits(_py, exc_bits);
                    }
                    let Some(ready_now) =
                        asyncio_fd_ready_select_once(_py, fileno_bits, events_bits)
                    else {
                        return MoltObject::none().bits() as i64;
                    };
                    if !ready_now {
                        waiter_bits = molt_async_sleep(
                            MoltObject::from_float(0.001).bits(),
                            MoltObject::none().bits(),
                        );
                        if obj_from_bits(waiter_bits).is_none() {
                            return waiter_bits as i64;
                        }
                        *payload_ptr.add(ASYNCIO_FD_WATCHER_SLOT_WAIT) = waiter_bits;
                    }
                } else {
                    *payload_ptr.add(ASYNCIO_FD_WATCHER_SLOT_WAIT) = waiter_bits;
                }
            }

            if !obj_from_bits(waiter_bits).is_none() {
                let wait_res = molt_future_poll(waiter_bits);
                if wait_res == pending_bits_i64() {
                    return pending_bits_i64();
                }
                if exception_pending(_py) {
                    let exc_bits = asyncio_take_pending_exception_bits(_py);
                    let fatal = asyncio_exception_is_fatal_base(_py, exc_bits);
                    if fatal {
                        let raised = molt_raise(exc_bits);
                        dec_ref_bits(_py, exc_bits);
                        return raised as i64;
                    }
                    dec_ref_bits(_py, exc_bits);
                    asyncio_drop_payload_slots(_py, payload_ptr, 6);
                    return MoltObject::none().bits() as i64;
                }
                asyncio_drop_slot_ref(_py, payload_ptr, ASYNCIO_FD_WATCHER_SLOT_WAIT);
            }

            let contains_bits =
                asyncio_call_method1(_py, registry_bits, b"__contains__", fileno_bits);
            if exception_pending(_py) {
                return contains_bits as i64;
            }
            let still_registered = is_truthy(_py, obj_from_bits(contains_bits));
            if !obj_from_bits(contains_bits).is_none() {
                dec_ref_bits(_py, contains_bits);
            }
            if !still_registered {
                asyncio_drop_payload_slots(_py, payload_ptr, 6);
                return MoltObject::none().bits() as i64;
            }

            let callback_bits = *payload_ptr.add(ASYNCIO_FD_WATCHER_SLOT_CALLBACK);
            let args_bits = *payload_ptr.add(ASYNCIO_FD_WATCHER_SLOT_ARGS);
            let callback_res = asyncio_call_with_args(_py, callback_bits, args_bits);
            if exception_pending(_py) {
                let exc_bits = asyncio_take_pending_exception_bits(_py);
                let fatal = asyncio_exception_is_fatal_base(_py, exc_bits);
                if fatal {
                    let raised = molt_raise(exc_bits);
                    dec_ref_bits(_py, exc_bits);
                    return raised as i64;
                }
                let errno = asyncio_oserror_errno_from_exception(_py, exc_bits).unwrap_or(i64::MIN);
                dec_ref_bits(_py, exc_bits);
                let events_bits = *payload_ptr.add(ASYNCIO_FD_WATCHER_SLOT_EVENTS);
                let default_events = ASYNCIO_SOCKET_IO_EVENT_READ | ASYNCIO_SOCKET_IO_EVENT_WRITE;
                let events = to_i64(obj_from_bits(events_bits)).unwrap_or(default_events);
                if errno != i64::MIN && asyncio_retryable_socket_errno(errno) {
                    return asyncio_pending_with_wait(
                        _py,
                        payload_ptr,
                        ASYNCIO_FD_WATCHER_SLOT_WAIT,
                        fileno_bits,
                        events,
                    );
                }
                // CPython event loops do not terminate reader/writer watchers on ordinary callback
                // exceptions; they route errors via loop exception handling and keep dispatch alive.
                // Re-arm the watcher to avoid silently dropping callbacks on non-fatal errors.
                return asyncio_pending_with_wait(
                    _py,
                    payload_ptr,
                    ASYNCIO_FD_WATCHER_SLOT_WAIT,
                    fileno_bits,
                    events,
                );
            }
            if !obj_from_bits(callback_res).is_none() {
                dec_ref_bits(_py, callback_res);
            }
            pending_bits_i64()
        })
    }
}

/// # Safety
/// - All arguments must be valid runtime objects.
#[unsafe(no_mangle)]
pub extern "C" fn molt_asyncio_server_accept_loop_new(
    sock_bits: u64,
    callback_bits: u64,
    loop_bits: u64,
    reader_ctor_bits: u64,
    writer_ctor_bits: u64,
    closed_probe_bits: u64,
) -> u64 {
    crate::with_gil_entry_nopanic!(_py, {
        let fd_bits = unsafe { asyncio_call_method0(_py, sock_bits, b"fileno") };
        if exception_pending(_py) {
            return fd_bits;
        }
        let obj_bits = molt_future_new(
            asyncio_server_accept_loop_poll_fn_addr(),
            (8 * std::mem::size_of::<u64>()) as u64,
        );
        if obj_from_bits(obj_bits).is_none() {
            if !obj_from_bits(fd_bits).is_none() {
                dec_ref_bits(_py, fd_bits);
            }
            return obj_bits;
        }
        let Some(obj_ptr) = resolve_obj_ptr(obj_bits) else {
            if !obj_from_bits(fd_bits).is_none() {
                dec_ref_bits(_py, fd_bits);
            }
            return MoltObject::none().bits();
        };
        let payload_ptr = obj_ptr as *mut u64;
        unsafe {
            *payload_ptr.add(ASYNCIO_SERVER_ACCEPT_SLOT_SOCK) = sock_bits;
            *payload_ptr.add(ASYNCIO_SERVER_ACCEPT_SLOT_CALLBACK) = callback_bits;
            *payload_ptr.add(ASYNCIO_SERVER_ACCEPT_SLOT_LOOP) = loop_bits;
            *payload_ptr.add(ASYNCIO_SERVER_ACCEPT_SLOT_READER_CTOR) = reader_ctor_bits;
            *payload_ptr.add(ASYNCIO_SERVER_ACCEPT_SLOT_WRITER_CTOR) = writer_ctor_bits;
            *payload_ptr.add(ASYNCIO_SERVER_ACCEPT_SLOT_CLOSED_PROBE) = closed_probe_bits;
            *payload_ptr.add(ASYNCIO_SERVER_ACCEPT_SLOT_FD) = fd_bits;
            *payload_ptr.add(ASYNCIO_SERVER_ACCEPT_SLOT_WAIT) = MoltObject::none().bits();
        }
        inc_ref_bits(_py, sock_bits);
        inc_ref_bits(_py, callback_bits);
        inc_ref_bits(_py, loop_bits);
        inc_ref_bits(_py, reader_ctor_bits);
        inc_ref_bits(_py, writer_ctor_bits);
        inc_ref_bits(_py, closed_probe_bits);
        obj_bits
    })
}

/// # Safety
/// - `obj_bits` must be a valid server-accept-loop wrapper future pointer.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn molt_asyncio_server_accept_loop_poll(obj_bits: u64) -> i64 {
    unsafe {
        crate::with_gil_entry_nopanic!(_py, {
            let obj_ptr = ptr_from_bits(obj_bits);
            if obj_ptr.is_null() {
                return MoltObject::none().bits() as i64;
            }
            let _header = header_from_obj_ptr(obj_ptr);
            let payload_bytes = crate::object::object_payload_size(obj_ptr);
            if payload_bytes < 8 * std::mem::size_of::<u64>() {
                return raise_exception::<i64>(
                    _py,
                    "RuntimeError",
                    "invalid asyncio server accept loop payload",
                );
            }
            let payload_ptr = obj_ptr as *mut u64;
            let closed_probe_bits = *payload_ptr.add(ASYNCIO_SERVER_ACCEPT_SLOT_CLOSED_PROBE);
            let closed_bits = call_callable0(_py, closed_probe_bits);
            if exception_pending(_py) {
                return closed_bits as i64;
            }
            let is_closed = is_truthy(_py, obj_from_bits(closed_bits));
            if !obj_from_bits(closed_bits).is_none() {
                dec_ref_bits(_py, closed_bits);
            }
            if is_closed {
                asyncio_drop_payload_slots(_py, payload_ptr, 8);
                return MoltObject::none().bits() as i64;
            }

            let mut waiter_bits = *payload_ptr.add(ASYNCIO_SERVER_ACCEPT_SLOT_WAIT);
            if obj_from_bits(waiter_bits).is_none() {
                let sock_bits = *payload_ptr.add(ASYNCIO_SERVER_ACCEPT_SLOT_SOCK);
                let fd_bits = *payload_ptr.add(ASYNCIO_SERVER_ACCEPT_SLOT_FD);
                waiter_bits = molt_asyncio_sock_accept_new(sock_bits, fd_bits);
                if obj_from_bits(waiter_bits).is_none() {
                    return waiter_bits as i64;
                }
                *payload_ptr.add(ASYNCIO_SERVER_ACCEPT_SLOT_WAIT) = waiter_bits;
            }

            let wait_res = molt_future_poll(waiter_bits);
            if wait_res == pending_bits_i64() {
                return pending_bits_i64();
            }
            if exception_pending(_py) {
                let exc_bits = asyncio_take_pending_exception_bits(_py);
                if asyncio_exception_kind_is(_py, exc_bits, "CancelledError") {
                    dec_ref_bits(_py, exc_bits);
                    asyncio_drop_payload_slots(_py, payload_ptr, 8);
                    return MoltObject::none().bits() as i64;
                }
                let fatal = asyncio_exception_is_fatal_base(_py, exc_bits);
                if fatal {
                    let raised = molt_raise(exc_bits);
                    dec_ref_bits(_py, exc_bits);
                    return raised as i64;
                }
                dec_ref_bits(_py, exc_bits);
                asyncio_drop_slot_ref(_py, payload_ptr, ASYNCIO_SERVER_ACCEPT_SLOT_WAIT);
                return pending_bits_i64();
            }

            asyncio_drop_slot_ref(_py, payload_ptr, ASYNCIO_SERVER_ACCEPT_SLOT_WAIT);
            let accepted_bits = wait_res as u64;
            let conn_bits = molt_getitem_method(accepted_bits, MoltObject::from_int(0).bits());
            if !obj_from_bits(accepted_bits).is_none() {
                dec_ref_bits(_py, accepted_bits);
            }
            if exception_pending(_py) {
                let exc_bits = asyncio_take_pending_exception_bits(_py);
                let fatal = asyncio_exception_is_fatal_base(_py, exc_bits);
                if fatal {
                    let raised = molt_raise(exc_bits);
                    dec_ref_bits(_py, exc_bits);
                    return raised as i64;
                }
                dec_ref_bits(_py, exc_bits);
                return pending_bits_i64();
            }

            let setblocking_bits = asyncio_call_method1(
                _py,
                conn_bits,
                b"setblocking",
                MoltObject::from_bool(false).bits(),
            );
            if exception_pending(_py) {
                let exc_bits = asyncio_take_pending_exception_bits(_py);
                if asyncio_exception_is_fatal_base(_py, exc_bits) {
                    let raised = molt_raise(exc_bits);
                    dec_ref_bits(_py, exc_bits);
                    dec_ref_bits(_py, conn_bits);
                    return raised as i64;
                }
                dec_ref_bits(_py, exc_bits);
            } else if !obj_from_bits(setblocking_bits).is_none() {
                dec_ref_bits(_py, setblocking_bits);
            }

            let reader_ctor_bits = *payload_ptr.add(ASYNCIO_SERVER_ACCEPT_SLOT_READER_CTOR);
            let reader_bits = call_callable1(_py, reader_ctor_bits, conn_bits);
            if exception_pending(_py) {
                let exc_bits = asyncio_take_pending_exception_bits(_py);
                let fatal = asyncio_exception_is_fatal_base(_py, exc_bits);
                asyncio_close_connection_best_effort(_py, conn_bits);
                dec_ref_bits(_py, conn_bits);
                if fatal {
                    let raised = molt_raise(exc_bits);
                    dec_ref_bits(_py, exc_bits);
                    return raised as i64;
                }
                dec_ref_bits(_py, exc_bits);
                return pending_bits_i64();
            }

            let writer_ctor_bits = *payload_ptr.add(ASYNCIO_SERVER_ACCEPT_SLOT_WRITER_CTOR);
            let writer_bits = call_callable1(_py, writer_ctor_bits, conn_bits);
            if exception_pending(_py) {
                let exc_bits = asyncio_take_pending_exception_bits(_py);
                let fatal = asyncio_exception_is_fatal_base(_py, exc_bits);
                if !obj_from_bits(reader_bits).is_none() {
                    dec_ref_bits(_py, reader_bits);
                }
                asyncio_close_connection_best_effort(_py, conn_bits);
                dec_ref_bits(_py, conn_bits);
                if fatal {
                    let raised = molt_raise(exc_bits);
                    dec_ref_bits(_py, exc_bits);
                    return raised as i64;
                }
                dec_ref_bits(_py, exc_bits);
                return pending_bits_i64();
            }

            let callback_bits = *payload_ptr.add(ASYNCIO_SERVER_ACCEPT_SLOT_CALLBACK);
            let callback_res = call_callable2(_py, callback_bits, reader_bits, writer_bits);
            if !obj_from_bits(reader_bits).is_none() {
                dec_ref_bits(_py, reader_bits);
            }
            if !obj_from_bits(writer_bits).is_none() {
                dec_ref_bits(_py, writer_bits);
            }
            if exception_pending(_py) {
                let exc_bits = asyncio_take_pending_exception_bits(_py);
                let fatal = asyncio_exception_is_fatal_base(_py, exc_bits);
                asyncio_close_connection_best_effort(_py, conn_bits);
                dec_ref_bits(_py, conn_bits);
                if fatal {
                    let raised = molt_raise(exc_bits);
                    dec_ref_bits(_py, exc_bits);
                    return raised as i64;
                }
                dec_ref_bits(_py, exc_bits);
                return pending_bits_i64();
            }

            let loop_bits = *payload_ptr.add(ASYNCIO_SERVER_ACCEPT_SLOT_LOOP);
            let spawn_bits = asyncio_call_method1(_py, loop_bits, b"create_task", callback_res);
            if !obj_from_bits(callback_res).is_none() {
                dec_ref_bits(_py, callback_res);
            }
            if exception_pending(_py) {
                let exc_bits = asyncio_take_pending_exception_bits(_py);
                let fatal = asyncio_exception_is_fatal_base(_py, exc_bits);
                asyncio_close_connection_best_effort(_py, conn_bits);
                dec_ref_bits(_py, conn_bits);
                if fatal {
                    let raised = molt_raise(exc_bits);
                    dec_ref_bits(_py, exc_bits);
                    return raised as i64;
                }
                dec_ref_bits(_py, exc_bits);
                return pending_bits_i64();
            }
            if !obj_from_bits(spawn_bits).is_none() {
                dec_ref_bits(_py, spawn_bits);
            }
            dec_ref_bits(_py, conn_bits);
            pending_bits_i64()
        })
    }
}

/// # Safety
/// - All arguments must be valid runtime objects.
#[unsafe(no_mangle)]
pub extern "C" fn molt_asyncio_ready_runner_new(
    loop_bits: u64,
    ready_lock_bits: u64,
    ready_bits: u64,
) -> u64 {
    crate::with_gil_entry_nopanic!(_py, {
        let obj_bits = molt_future_new(
            asyncio_ready_runner_poll_fn_addr(),
            (4 * std::mem::size_of::<u64>()) as u64,
        );
        if obj_from_bits(obj_bits).is_none() {
            return obj_bits;
        }
        let Some(obj_ptr) = resolve_obj_ptr(obj_bits) else {
            return MoltObject::none().bits();
        };
        let payload_ptr = obj_ptr as *mut u64;
        unsafe {
            *payload_ptr.add(ASYNCIO_READY_RUNNER_SLOT_LOOP) = loop_bits;
            *payload_ptr.add(ASYNCIO_READY_RUNNER_SLOT_READY_LOCK) = ready_lock_bits;
            *payload_ptr.add(ASYNCIO_READY_RUNNER_SLOT_READY) = ready_bits;
            *payload_ptr.add(ASYNCIO_READY_RUNNER_SLOT_WAIT) = MoltObject::none().bits();
        }
        inc_ref_bits(_py, loop_bits);
        inc_ref_bits(_py, ready_lock_bits);
        inc_ref_bits(_py, ready_bits);
        obj_bits
    })
}

/// # Safety
/// - `obj_bits` must be a valid ready-runner wrapper future pointer.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn molt_asyncio_ready_runner_poll(obj_bits: u64) -> i64 {
    unsafe {
        crate::with_gil_entry_nopanic!(_py, {
            let obj_ptr = ptr_from_bits(obj_bits);
            if obj_ptr.is_null() {
                return MoltObject::none().bits() as i64;
            }
            let _header = header_from_obj_ptr(obj_ptr);
            let payload_bytes = crate::object::object_payload_size(obj_ptr);
            if payload_bytes < 4 * std::mem::size_of::<u64>() {
                return raise_exception::<i64>(
                    _py,
                    "RuntimeError",
                    "invalid asyncio ready runner payload",
                );
            }
            let payload_ptr = obj_ptr as *mut u64;
            let loop_bits = *payload_ptr.add(ASYNCIO_READY_RUNNER_SLOT_LOOP);
            let closed = match asyncio_method_truthy(_py, loop_bits, b"is_closed") {
                Some(flag) => flag,
                None => return MoltObject::none().bits() as i64,
            };
            if closed {
                asyncio_drop_payload_slots(_py, payload_ptr, 4);
                return MoltObject::none().bits() as i64;
            }

            let ready_lock_bits = *payload_ptr.add(ASYNCIO_READY_RUNNER_SLOT_READY_LOCK);
            let ready_bits = *payload_ptr.add(ASYNCIO_READY_RUNNER_SLOT_READY);
            let drained_bits = molt_asyncio_ready_queue_drain(ready_lock_bits, ready_bits);
            if exception_pending(_py) {
                return drained_bits as i64;
            }
            if !obj_from_bits(drained_bits).is_none() {
                dec_ref_bits(_py, drained_bits);
            }

            let mut waiter_bits = *payload_ptr.add(ASYNCIO_READY_RUNNER_SLOT_WAIT);
            if obj_from_bits(waiter_bits).is_none() {
                waiter_bits = molt_async_sleep(
                    MoltObject::from_float(0.0).bits(),
                    MoltObject::none().bits(),
                );
                if obj_from_bits(waiter_bits).is_none() {
                    return waiter_bits as i64;
                }
                *payload_ptr.add(ASYNCIO_READY_RUNNER_SLOT_WAIT) = waiter_bits;
            }
            let wait_res = molt_future_poll(waiter_bits);
            if wait_res == pending_bits_i64() {
                return pending_bits_i64();
            }
            if exception_pending(_py) {
                let exc_bits = asyncio_take_pending_exception_bits(_py);
                if asyncio_exception_kind_is(_py, exc_bits, "CancelledError") {
                    dec_ref_bits(_py, exc_bits);
                    asyncio_drop_payload_slots(_py, payload_ptr, 4);
                    return MoltObject::none().bits() as i64;
                }
                let raised = molt_raise(exc_bits);
                dec_ref_bits(_py, exc_bits);
                return raised as i64;
            }
            asyncio_drop_slot_ref(_py, payload_ptr, ASYNCIO_READY_RUNNER_SLOT_WAIT);
            pending_bits_i64()
        })
    }
}

/// # Safety
/// - `future_ptr` must be a valid Molt timer-handle future pointer.
pub(crate) unsafe fn asyncio_timer_handle_task_drop(_py: &PyToken<'_>, future_ptr: *mut u8) {
    unsafe {
        if future_ptr.is_null() {
            return;
        }
        let _header = header_from_obj_ptr(future_ptr);
        let payload_bytes = crate::object::object_payload_size(future_ptr);
        if payload_bytes < 7 * std::mem::size_of::<u64>() {
            return;
        }
        let payload_ptr = future_ptr as *mut u64;
        asyncio_drop_payload_slots(_py, payload_ptr, 7);
    }
}

/// # Safety
/// - `future_ptr` must be a valid Molt fd-watcher future pointer.
pub(crate) unsafe fn asyncio_fd_watcher_task_drop(_py: &PyToken<'_>, future_ptr: *mut u8) {
    unsafe {
        if future_ptr.is_null() {
            return;
        }
        let _header = header_from_obj_ptr(future_ptr);
        let payload_bytes = crate::object::object_payload_size(future_ptr);
        if payload_bytes < 6 * std::mem::size_of::<u64>() {
            return;
        }
        let payload_ptr = future_ptr as *mut u64;
        asyncio_drop_payload_slots(_py, payload_ptr, 6);
    }
}

/// # Safety
/// - `future_ptr` must be a valid Molt server-accept-loop future pointer.
pub(crate) unsafe fn asyncio_server_accept_loop_task_drop(_py: &PyToken<'_>, future_ptr: *mut u8) {
    unsafe {
        if future_ptr.is_null() {
            return;
        }
        let _header = header_from_obj_ptr(future_ptr);
        let payload_bytes = crate::object::object_payload_size(future_ptr);
        if payload_bytes < 8 * std::mem::size_of::<u64>() {
            return;
        }
        let payload_ptr = future_ptr as *mut u64;
        asyncio_drop_payload_slots(_py, payload_ptr, 8);
    }
}

/// # Safety
/// - `future_ptr` must be a valid Molt ready-runner future pointer.
pub(crate) unsafe fn asyncio_ready_runner_task_drop(_py: &PyToken<'_>, future_ptr: *mut u8) {
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
        asyncio_drop_payload_slots(_py, payload_ptr, 4);
    }
}
