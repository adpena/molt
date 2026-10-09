use super::*;

// Registration must succeed before changing timeout interpretation or releasing
// a float-subclass timeout whose finalizer can reenter the waiter.
// The caller owns the payload across release; Some admits reference slot two.
#[cfg(any(molt_has_net_io, target_arch = "wasm32", test))]
pub(crate) unsafe fn publish_registered_wait(
    py: &PyToken<'_>,
    owner: *mut u8,
    deadline: Option<u64>,
) {
    let previous = deadline.map(|bits| unsafe {
        crate::object::payload_refs::exchange_owned(py, owner, 2 * std::mem::size_of::<u64>(), bits)
    });
    crate::object::object_set_state(owner, 1);
    if let Some(previous) = previous {
        dec_ref_bits(py, previous);
    }
}

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
                let mut deadline_bits = None;
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
                    deadline_bits = Some(MoltObject::from_float(deadline).bits());
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
                publish_registered_wait(_py, obj_ptr, deadline_bits);
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
                let mut deadline_bits = None;
                if let Some(val) = timeout {
                    if val == 0.0 {
                        return raise_exception::<i64>(_py, "TimeoutError", "timed out");
                    }
                    let deadline = monotonic_now_secs(_py) + val;
                    deadline_bits = Some(MoltObject::from_float(deadline).bits());
                }
                if let Err(err) = runtime_state(_py)
                    .io_poller()
                    .register_wait(obj_ptr, handle, events)
                {
                    return raise_exception::<i64>(_py, "RuntimeError", &err.to_string());
                }
                publish_registered_wait(_py, obj_ptr, deadline_bits);
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

#[cfg(test)]
mod deadline_publication_tests {
    use super::*;
    use std::sync::atomic::{AtomicU64, AtomicUsize, Ordering};

    static OWNER: AtomicU64 = AtomicU64::new(0);
    static CALLS: AtomicUsize = AtomicUsize::new(0);
    static OBSERVED_STATE: AtomicU64 = AtomicU64::new(0);
    static OBSERVED_DEADLINE: AtomicU64 = AtomicU64::new(0);

    struct CallbackScope;

    impl CallbackScope {
        fn new() -> Self {
            let scope = Self;
            scope.reset();
            scope
        }

        fn reset(&self) {
            OWNER.store(0, Ordering::Relaxed);
            CALLS.store(0, Ordering::Relaxed);
            OBSERVED_STATE.store(0, Ordering::Relaxed);
            OBSERVED_DEADLINE.store(0, Ordering::Relaxed);
        }
    }

    impl Drop for CallbackScope {
        fn drop(&mut self) {
            self.reset();
        }
    }

    extern "C" fn timeout_probe(_argument: u64) -> u64 {
        MoltObject::none().bits()
    }

    extern "C" fn timeout_released(_weak: u64) -> u64 {
        crate::with_gil_entry_nopanic!(py, {
            let bits = OWNER.swap(0, Ordering::Relaxed);
            if bits == 0 {
                return MoltObject::none().bits();
            }
            let owner = ptr_from_bits(bits);
            OBSERVED_STATE.store(crate::object::object_state(owner) as u64, Ordering::Relaxed);
            unsafe {
                OBSERVED_DEADLINE.store(*owner.cast::<u64>().add(2), Ordering::Relaxed);
                crate::object::payload_refs::store_owned(
                    py,
                    owner,
                    2 * std::mem::size_of::<u64>(),
                    MoltObject::from_float(456.0).bits(),
                );
            }
            crate::object::object_set_state(owner, 7);
            CALLS.fetch_add(1, Ordering::Relaxed);
            MoltObject::none().bits()
        })
    }

    fn function(py: &PyToken<'_>, address: *const ()) -> u64 {
        let ptr = crate::object::builders::alloc_function_obj(
            py,
            crate::provenance::abi::expose_function_address(address),
            1,
        );
        assert!(!ptr.is_null());
        unsafe { crate::object::layout::function_set_call_target_ptr(ptr, address) };
        MoltObject::from_ptr(ptr).bits()
    }

    #[test]
    fn payload_reference_wait_deadline_and_registered_state_precede_timeout_release() {
        let _transaction = crate::test_support::RuntimeTestTransaction::new();
        crate::with_gil_entry_nopanic!(py, {
            let _callbacks = CallbackScope::new();
            let owner = crate::molt_alloc((3 * std::mem::size_of::<u64>()) as u64);
            let ptr = ptr_from_bits(owner);
            assert!(!ptr.is_null());
            let timeout = function(py, timeout_probe as *const ());
            let hook = function(py, timeout_released as *const ());
            let class = crate::molt_weakref_reference_type();
            let weak = crate::molt_weakref_new(class, timeout, hook);
            dec_ref_bits(py, class);
            unsafe {
                for index in 0..3 {
                    ptr.cast::<u64>()
                        .add(index)
                        .write(MoltObject::none().bits());
                }
                assert!(crate::object::object_init_state_unpublished(ptr, 0));
                crate::object::payload_refs::store_owned(
                    py,
                    ptr,
                    2 * std::mem::size_of::<u64>(),
                    timeout,
                );
            }
            crate::molt_object_publish_initialized(owner);
            OWNER.store(owner, Ordering::Relaxed);
            unsafe { publish_registered_wait(py, ptr, Some(MoltObject::from_float(123.0).bits())) };
            assert_eq!(CALLS.load(Ordering::Relaxed), 1);
            assert_eq!(
                OBSERVED_STATE.load(Ordering::Relaxed),
                1,
                "callback saw an unregistered wait"
            );
            assert_eq!(
                OBSERVED_DEADLINE.load(Ordering::Relaxed),
                MoltObject::from_float(123.0).bits()
            );
            assert_eq!(
                crate::object::object_state(ptr),
                7,
                "outer publication overwrote callback state"
            );
            assert_eq!(
                unsafe { *ptr.cast::<u64>().add(2) },
                MoltObject::from_float(456.0).bits()
            );
            assert!(obj_from_bits(crate::molt_weakref_call(weak)).is_none());
            dec_ref_bits(py, weak);
            dec_ref_bits(py, hook);
            dec_ref_bits(py, owner);
            assert!(!crate::exception_pending(py));
        });
    }

    #[test]
    fn deadline_callback_records_bad_publication_and_disarms_on_unwind() {
        let _transaction = crate::test_support::RuntimeTestTransaction::new();
        crate::with_gil_entry_nopanic!(py, {
            let owner = crate::molt_alloc(24);
            let ptr = ptr_from_bits(owner);
            assert!(!ptr.is_null());
            unsafe {
                for index in 0..3 {
                    ptr.cast::<u64>()
                        .add(index)
                        .write(MoltObject::none().bits());
                }
                assert!(crate::object::object_init_state_unpublished(ptr, 0));
            }
            crate::molt_object_publish_initialized(owner);
            let failure = crate::test_support::catch_expected_unwind(|| {
                let _callbacks = CallbackScope::new();
                OWNER.store(owner, Ordering::Relaxed);
                let result = timeout_released(0);
                dec_ref_bits(py, result);
                assert_eq!(OBSERVED_STATE.load(Ordering::Relaxed), 0);
                assert_eq!(
                    OBSERVED_DEADLINE.load(Ordering::Relaxed),
                    MoltObject::none().bits()
                );
                assert_eq!(OWNER.load(Ordering::Relaxed), 0);
                OWNER.store(owner, Ordering::Relaxed);
                panic!("deadline callback rollback control");
            });
            assert_eq!(
                failure
                    .expect_err("rollback control must unwind")
                    .downcast_ref::<&str>(),
                Some(&"deadline callback rollback control")
            );
            assert_eq!(OWNER.load(Ordering::Relaxed), 0);
            assert_eq!(CALLS.load(Ordering::Relaxed), 0);
            dec_ref_bits(py, owner);
            assert!(!crate::exception_pending(py));
        });
    }
}
