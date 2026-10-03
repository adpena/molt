//! Shared asyncio Python-object call, exception, waiter, and slot helpers.
//!
//! These helpers are used by ready queues, task groups, combinators,
//! event-loop glue, process futures, and socket/stream I/O. Keeping them
//! separate prevents any one async primitive family from owning the bridge.

use super::*;

pub(crate) unsafe fn asyncio_drop_slot_ref(_py: &PyToken<'_>, payload_ptr: *mut u64, idx: usize) {
    unsafe {
        crate::object::payload_refs::store_owned(
            _py,
            payload_ptr.cast(),
            idx * std::mem::size_of::<u64>(),
            MoltObject::none().bits(),
        );
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

pub(crate) fn asyncio_exception_is_cancelled(py: &PyToken<'_>, exception: u64) -> bool {
    // The runtime class is also the identity published by asyncio.exceptions.
    // A same-named user exception must never be mistaken for cancellation.
    crate::builtins::exceptions::exception_matches_builtin_name(py, exception, "CancelledError")
}

pub(crate) fn asyncio_exception_escapes_callback(py: &PyToken<'_>, exception: u64) -> bool {
    crate::builtins::exceptions::exception_matches_builtin_name(py, exception, "SystemExit")
        || crate::builtins::exceptions::exception_matches_builtin_name(
            py,
            exception,
            "KeyboardInterrupt",
        )
}

unsafe fn asyncio_required_method(py: &PyToken<'_>, object: u64, name: &[u8]) -> Option<u64> {
    unsafe {
        let Some(ptr) = obj_from_bits(object).as_ptr() else {
            return raise_exception::<Option<u64>>(py, "TypeError", "object is not awaitable");
        };
        let name = attr_name_bits_from_bytes(py, name)?;
        // Required lookup must preserve descriptor failures, including an
        // AttributeError deliberately raised by the descriptor itself.
        let method = crate::builtins::attributes::attr_lookup_ptr(py, ptr, name);
        dec_ref_bits(py, name);
        if exception_pending(py) {
            if let Some(method) = method {
                dec_ref_bits(py, method);
            }
            return None;
        }
        if method.is_none() {
            return raise_exception::<Option<u64>>(py, "TypeError", "object is not awaitable");
        }
        method
    }
}

pub(crate) unsafe fn asyncio_call_method0(_py: &PyToken<'_>, obj_bits: u64, method: &[u8]) -> u64 {
    unsafe {
        let Some(method_bits) = asyncio_required_method(_py, obj_bits, method) else {
            return MoltObject::none().bits();
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
        let Some(method_bits) = asyncio_required_method(_py, obj_bits, method) else {
            return MoltObject::none().bits();
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
        let Some(method_bits) = asyncio_required_method(_py, obj_bits, method) else {
            return MoltObject::none().bits();
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
        let Some(method_bits) = asyncio_required_method(_py, obj_bits, method) else {
            return MoltObject::none().bits();
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
        let method_bits = attr_lookup_ptr_allow_missing(_py, obj_ptr, method_name_bits);
        dec_ref_bits(_py, method_name_bits);
        let method_bits = method_bits?;
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
        let result = attr_lookup_ptr_allow_missing(_py, obj_ptr, name_bits);
        dec_ref_bits(_py, name_bits);
        result
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
        if exception_pending(_py) {
            None
        } else {
            Some(truthy)
        }
    }
}

pub(crate) unsafe fn asyncio_waiters_pop_front(_py: &PyToken<'_>, waiters_bits: u64) -> u64 {
    unsafe {
        if let Some(bits) = asyncio_call_method0_allow_missing(_py, waiters_bits, b"popleft") {
            return bits;
        }
        if exception_pending(_py) {
            return MoltObject::none().bits();
        }
        asyncio_call_method1(_py, waiters_bits, b"pop", MoltObject::from_int(0).bits())
    }
}

#[cfg(test)]
mod exception_dispatch_tests {
    use super::*;

    fn class(py: &PyToken<'_>, name: &[u8], base: u64) -> u64 {
        let name = MoltObject::from_ptr(alloc_string(py, name)).bits();
        let bases = MoltObject::from_ptr(alloc_tuple(py, &[base])).bits();
        let namespace = MoltObject::from_ptr(alloc_dict_with_pairs(py, &[])).bits();
        let result = crate::builtins::types::molt_type_new(
            builtin_classes(py).type_obj,
            name,
            bases,
            namespace,
            MoltObject::none().bits(),
        );
        for bits in [name, bases, namespace] {
            dec_ref_bits(py, bits);
        }
        assert!(!exception_pending(py));
        result
    }

    #[test]
    fn async_exception_dispatch_uses_class_identity_and_callback_rules() {
        let _transaction = crate::test_support::RuntimeTestTransaction::new();
        crate::with_gil_entry_nopanic!(py, {
            for (name, base, cancelled, fatal) in [
                (
                    b"DerivedCancellation".as_slice(),
                    "CancelledError",
                    true,
                    false,
                ),
                (b"CancelledError".as_slice(), "Exception", false, false),
                (
                    b"DerivedInterrupt".as_slice(),
                    "KeyboardInterrupt",
                    false,
                    true,
                ),
                (b"SystemExit".as_slice(), "Exception", false, false),
                (b"DerivedExit".as_slice(), "GeneratorExit", false, false),
            ] {
                let class = class(py, name, exception_type_bits_from_name(py, base));
                let args = MoltObject::from_ptr(alloc_tuple(py, &[])).bits();
                let exception =
                    MoltObject::from_ptr(alloc_exception_from_class_bits(py, class, args)).bits();
                assert_eq!(asyncio_exception_is_cancelled(py, exception), cancelled);
                assert_eq!(asyncio_exception_escapes_callback(py, exception), fatal);
                assert!(!exception_pending(py));
                for bits in [exception, args, class] {
                    dec_ref_bits(py, bits);
                }
            }
        });
    }

    extern "C" fn failing_descriptor(_instance: u64) -> u64 {
        crate::with_gil_entry_nopanic!(py, {
            raise_exception::<u64>(py, "AttributeError", "descriptor-origin")
        })
    }

    #[test]
    fn required_async_methods_preserve_descriptor_failure_at_every_arity() {
        let _transaction = crate::test_support::RuntimeTestTransaction::new();
        crate::with_gil_entry_nopanic!(py, {
            let class = class(py, b"DescriptorOwner", builtin_classes(py).object);
            let getter =
                MoltObject::from_ptr(crate::builtins::functions::alloc_runtime_function_obj(
                    py,
                    crate::provenance::abi::expose_function_address(
                        failing_descriptor as *const (),
                    ),
                    1,
                ))
                .bits();
            let none = MoltObject::none().bits();
            let descriptor = crate::builtins::types::molt_property_new(getter, none, none);
            let name = attr_name_bits_from_bytes(py, b"probe").unwrap();
            crate::molt_set_attr_name(class, name, descriptor);
            assert!(!exception_pending(py));
            let instance = unsafe {
                crate::alloc_instance_for_class(py, obj_from_bits(class).as_ptr().unwrap())
            };
            for arity in 0..=3 {
                let result = unsafe {
                    match arity {
                        0 => asyncio_call_method0(py, instance, b"probe"),
                        1 => asyncio_call_method1(py, instance, b"probe", none),
                        2 => asyncio_call_method2(py, instance, b"probe", none, none),
                        _ => asyncio_call_method3(py, instance, b"probe", none, none, none),
                    }
                };
                assert_eq!(result, none);
                assert!(exception_pending(py));
                let exception = molt_exception_last_pending();
                assert!(crate::builtins::exceptions::exception_matches_builtin_name(
                    py,
                    exception,
                    "AttributeError"
                ));
                clear_exception(py);
                dec_ref_bits(py, exception);
            }
            for bits in [instance, name, descriptor, getter, class] {
                dec_ref_bits(py, bits);
            }
        });
    }
}
