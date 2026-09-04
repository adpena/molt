use super::*;

#[cfg(molt_has_net_io)]
/// # Safety
/// Caller must pass a valid io-wait awaitable object bits value and ensure the
/// runtime is initialized. The function enters the GIL-guarded runtime state.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn molt_io_wait(obj_bits: u64) -> i64 {
    unsafe {
        crate::with_gil_entry_nopanic!(_py, {
            let obj_ptr = ptr_from_bits(obj_bits);
            if obj_ptr.is_null() {
                return MoltObject::none().bits() as i64;
            }
            let _header = header_from_obj_ptr(obj_ptr);
            let payload_bytes = crate::object::object_payload_size(obj_ptr);
            let payload_len = payload_bytes / std::mem::size_of::<u64>();
            if payload_len < 2 {
                return raise_exception::<i64>(_py, "TypeError", "io wait payload too small");
            }
            let payload_ptr = obj_ptr as *mut u64;
            let socket_bits = *payload_ptr;
            let events_bits = *payload_ptr.add(1);
            let socket_ptr = socket_ptr_from_bits_or_fd(socket_bits);
            if socket_ptr.is_null() {
                if trace_io_wait_errors() {
                    eprintln!(
                        "molt io_wait error: invalid socket bits=0x{:x} state={}",
                        socket_bits,
                        crate::object::object_state(obj_ptr)
                    );
                }
                return raise_exception::<i64>(_py, "TypeError", "invalid socket");
            }
            let events = to_i64(obj_from_bits(events_bits)).unwrap_or(0) as u32;
            if events == 0 {
                return raise_exception::<i64>(_py, "ValueError", "events must be non-zero");
            }
            if crate::object::object_state(obj_ptr) == 0 {
                let mut timeout: Option<f64> = None;
                if payload_len >= 3 {
                    let timeout_bits = *payload_ptr.add(2);
                    let timeout_obj = obj_from_bits(timeout_bits);
                    if !timeout_obj.is_none() {
                        if let Some(val) = to_f64(timeout_obj) {
                            if !val.is_finite() || val < 0.0 {
                                return raise_exception::<i64>(
                                    _py,
                                    "ValueError",
                                    "timeout must be non-negative",
                                );
                            }
                            timeout = Some(val);
                        } else {
                            return raise_exception::<i64>(
                                _py,
                                "TypeError",
                                "timeout must be float or None",
                            );
                        }
                    }
                }
                if let Some(val) = timeout {
                    if val == 0.0 {
                        match runtime_state(_py).io_poller().wait_blocking(
                            socket_ptr,
                            events,
                            Some(Duration::from_millis(5)),
                        ) {
                            Ok(mask) => {
                                let res_bits = MoltObject::from_int(mask as i64).bits();
                                return res_bits as i64;
                            }
                            Err(err) => return raise_os_error::<i64>(_py, err, "io_wait"),
                        }
                    }
                    let deadline = monotonic_now_secs(_py) + val;
                    let deadline_bits = MoltObject::from_float(deadline).bits();
                    if payload_len >= 3 {
                        dec_ref_bits(_py, *payload_ptr.add(2));
                        *payload_ptr.add(2) = deadline_bits;
                        inc_ref_bits(_py, deadline_bits);
                    }
                }
                if let Err(err) = runtime_state(_py)
                    .io_poller()
                    .register_wait(obj_ptr, socket_ptr, events)
                {
                    if trace_io_wait_errors() {
                        eprintln!(
                            "molt io_wait error: register_wait failed fd={} err={}",
                            socket_debug_fd(socket_ptr).unwrap_or(-1),
                            err
                        );
                    }
                    return raise_os_error::<i64>(_py, err, "io_wait");
                }
                crate::object::object_set_state(obj_ptr, 1);
                return pending_bits_i64();
            }
            if let Some(mask) = runtime_state(_py).io_poller().take_ready(obj_ptr) {
                let res_bits = MoltObject::from_int(mask as i64).bits();
                return res_bits as i64;
            }
            if payload_len >= 3 {
                let deadline_obj = obj_from_bits(*payload_ptr.add(2));
                if let Some(deadline) = to_f64(deadline_obj)
                    && deadline.is_finite()
                    && monotonic_now_secs(_py) >= deadline
                {
                    runtime_state(_py).io_poller().cancel_waiter(obj_ptr);
                    return raise_exception::<i64>(_py, "TimeoutError", "timed out");
                }
            }
            pending_bits_i64()
        })
    }
}

#[cfg(molt_has_net_io)]
#[unsafe(no_mangle)]
pub extern "C" fn molt_io_wait_new(socket_bits: u64, events_bits: u64, timeout_bits: u64) -> u64 {
    crate::with_gil_entry_nopanic!(_py, {
        if require_net_capability::<u64>(_py, crate::OperationId::NetPoll).is_err() {
            return MoltObject::none().bits();
        }
        let socket_ptr = socket_ptr_from_bits_or_fd(socket_bits);
        if socket_ptr.is_null() {
            return raise_exception::<_>(_py, "TypeError", "invalid socket");
        }
        let events = match to_i64(obj_from_bits(events_bits)) {
            Some(val) => val,
            None => return raise_exception::<_>(_py, "TypeError", "events must be int"),
        };
        if events == 0 {
            return raise_exception::<_>(_py, "ValueError", "events must be non-zero");
        }
        let obj_bits = molt_future_new(
            io_wait_poll_fn_addr(),
            (3 * std::mem::size_of::<u64>()) as u64,
        );
        let Some(obj_ptr) = resolve_obj_ptr(obj_bits) else {
            return MoltObject::none().bits();
        };
        unsafe {
            let payload_ptr = obj_ptr as *mut u64;
            *payload_ptr = socket_bits;
            *payload_ptr.add(1) = events_bits;
            *payload_ptr.add(2) = timeout_bits;
            inc_ref_bits(_py, events_bits);
            inc_ref_bits(_py, timeout_bits);
        }
        socket_ref_inc(socket_ptr);
        obj_bits
    })
}

#[cfg(target_arch = "wasm32")]
#[unsafe(no_mangle)]
pub extern "C" fn molt_io_wait_new(socket_bits: u64, events_bits: u64, timeout_bits: u64) -> u64 {
    crate::with_gil_entry_nopanic!(_py, {
        if require_net_capability::<u64>(_py, crate::OperationId::NetPoll).is_err() {
            return MoltObject::none().bits();
        }
        let socket_obj = obj_from_bits(socket_bits);
        let Some(handle) = to_i64(socket_obj) else {
            return raise_exception::<_>(_py, "TypeError", "invalid socket");
        };
        if handle < 0 {
            return raise_exception::<_>(_py, "TypeError", "invalid socket");
        }
        let events = match to_i64(obj_from_bits(events_bits)) {
            Some(val) => val,
            None => return raise_exception::<_>(_py, "TypeError", "events must be int"),
        };
        if events == 0 {
            return raise_exception::<_>(_py, "ValueError", "events must be non-zero");
        }
        let obj_bits = molt_future_new(
            io_wait_poll_fn_addr(),
            (3 * std::mem::size_of::<u64>()) as u64,
        );
        let Some(obj_ptr) = resolve_obj_ptr(obj_bits) else {
            return MoltObject::none().bits();
        };
        unsafe {
            let payload_ptr = obj_ptr as *mut u64;
            *payload_ptr = socket_bits;
            *payload_ptr.add(1) = events_bits;
            *payload_ptr.add(2) = timeout_bits;
            inc_ref_bits(_py, events_bits);
            inc_ref_bits(_py, timeout_bits);
        }
        obj_bits
    })
}

#[cfg(target_arch = "wasm32")]
#[unsafe(no_mangle)]
/// # Safety
/// Caller must ensure `obj_bits` is a valid I/O object pointer.
pub unsafe extern "C" fn molt_io_wait(obj_bits: u64) -> i64 {
    unsafe {
        crate::with_gil_entry_nopanic!(_py, {
            let obj_ptr = ptr_from_bits(obj_bits);
            if obj_ptr.is_null() {
                return MoltObject::none().bits() as i64;
            }
            let header = header_from_obj_ptr(obj_ptr);
            let payload_bytes = crate::object::object_payload_size(obj_ptr);
            let payload_len = payload_bytes / std::mem::size_of::<u64>();
            if payload_len < 2 {
                return raise_exception::<i64>(_py, "TypeError", "io wait payload too small");
            }
            let payload_ptr = obj_ptr as *mut u64;
            let socket_bits = *payload_ptr;
            let socket_obj = obj_from_bits(socket_bits);
            let Some(handle) = to_i64(socket_obj) else {
                return raise_exception::<i64>(_py, "TypeError", "invalid socket");
            };
            if handle < 0 {
                return raise_exception::<i64>(_py, "TypeError", "invalid socket");
            }
            let events_bits = *payload_ptr.add(1);
            let events = to_i64(obj_from_bits(events_bits)).unwrap_or(0) as u32;
            if events == 0 {
                return raise_exception::<i64>(_py, "ValueError", "events must be non-zero");
            }
            if crate::object::object_state(obj_ptr) == 0 {
                let mut timeout: Option<f64> = None;
                if payload_len >= 3 {
                    let timeout_bits = *payload_ptr.add(2);
                    let timeout_obj = obj_from_bits(timeout_bits);
                    if !timeout_obj.is_none() {
                        if let Some(val) = to_f64(timeout_obj) {
                            if !val.is_finite() || val < 0.0 {
                                return raise_exception::<i64>(
                                    _py,
                                    "ValueError",
                                    "timeout must be non-negative",
                                );
                            }
                            timeout = Some(val);
                        } else {
                            return raise_exception::<i64>(
                                _py,
                                "TypeError",
                                "timeout must be float or None",
                            );
                        }
                    }
                }
                if let Some(val) = timeout {
                    if val == 0.0 {
                        return raise_exception::<i64>(_py, "TimeoutError", "timed out");
                    }
                    let deadline = monotonic_now_secs(_py) + val;
                    let deadline_bits = MoltObject::from_float(deadline).bits();
                    if payload_len >= 3 {
                        dec_ref_bits(_py, *payload_ptr.add(2));
                        *payload_ptr.add(2) = deadline_bits;
                        inc_ref_bits(_py, deadline_bits);
                    }
                }
                if let Err(err) = runtime_state(_py)
                    .io_poller()
                    .register_wait(obj_ptr, handle, events)
                {
                    return raise_exception::<i64>(_py, "RuntimeError", &err.to_string());
                }
                crate::object::object_set_state(obj_ptr, 1);
                return pending_bits_i64();
            }
            if let Some(mask) = runtime_state(_py).io_poller().take_ready(obj_ptr) {
                let res_bits = MoltObject::from_int(mask as i64).bits();
                return res_bits as i64;
            }
            if payload_len >= 3 {
                let deadline_obj = obj_from_bits(*payload_ptr.add(2));
                if let Some(deadline) = to_f64(deadline_obj)
                    && deadline.is_finite()
                    && monotonic_now_secs(_py) >= deadline
                {
                    runtime_state(_py).io_poller().cancel_waiter(obj_ptr);
                    return raise_exception::<i64>(_py, "TimeoutError", "timed out");
                }
            }
            pending_bits_i64()
        })
    }
}
