use super::*;
use std::sync::atomic::{AtomicU64, Ordering};

static HOOK_A: AtomicU64 = AtomicU64::new(0);
static HOOK_B: AtomicU64 = AtomicU64::new(0);
static HOOK_C: AtomicU64 = AtomicU64::new(0);
static BIND_A: AtomicU64 = AtomicU64::new(0);
static BIND_B: AtomicU64 = AtomicU64::new(0);
static BIND_C: AtomicU64 = AtomicU64::new(0);
static INDIRECT_RETURN_CALLABLE: AtomicU64 = AtomicU64::new(0);
static STATICMETHOD_OWNER: AtomicU64 = AtomicU64::new(0);
static FALLBACK_CLASS: AtomicU64 = AtomicU64::new(0);
static FALLBACK_REPLACEMENT: AtomicU64 = AtomicU64::new(0);
static FALLBACK_ORIGINAL_CALLS: AtomicU64 = AtomicU64::new(0);
static FALLBACK_FRESH_CALLS: AtomicU64 = AtomicU64::new(0);

fn reset_hook_args() {
    HOOK_A.store(0, Ordering::SeqCst);
    HOOK_B.store(0, Ordering::SeqCst);
    HOOK_C.store(0, Ordering::SeqCst);
    BIND_A.store(0, Ordering::SeqCst);
    BIND_B.store(0, Ordering::SeqCst);
    BIND_C.store(0, Ordering::SeqCst);
}

extern "C" fn protocol_static_get(
    descriptor_bits: u64,
    instance_bits: u64,
    owner_bits: u64,
) -> u64 {
    HOOK_A.store(descriptor_bits, Ordering::SeqCst);
    HOOK_B.store(instance_bits, Ordering::SeqCst);
    HOOK_C.store(owner_bits, Ordering::SeqCst);
    MoltObject::from_int(101).bits()
}

extern "C" fn protocol_static_set(instance_bits: u64, value_bits: u64) -> u64 {
    HOOK_A.store(instance_bits, Ordering::SeqCst);
    HOOK_B.store(value_bits, Ordering::SeqCst);
    MoltObject::none().bits()
}

extern "C" fn protocol_static_delete(instance_bits: u64) -> u64 {
    HOOK_A.store(instance_bits, Ordering::SeqCst);
    MoltObject::none().bits()
}

extern "C" fn protocol_class_get(class_bits: u64, instance_bits: u64, owner_bits: u64) -> u64 {
    HOOK_A.store(class_bits, Ordering::SeqCst);
    HOOK_B.store(instance_bits, Ordering::SeqCst);
    HOOK_C.store(owner_bits, Ordering::SeqCst);
    MoltObject::from_int(102).bits()
}

extern "C" fn protocol_class_set(class_bits: u64, instance_bits: u64, value_bits: u64) -> u64 {
    HOOK_A.store(class_bits, Ordering::SeqCst);
    HOOK_B.store(instance_bits, Ordering::SeqCst);
    HOOK_C.store(value_bits, Ordering::SeqCst);
    MoltObject::none().bits()
}

extern "C" fn protocol_class_delete(class_bits: u64, instance_bits: u64) -> u64 {
    HOOK_A.store(class_bits, Ordering::SeqCst);
    HOOK_B.store(instance_bits, Ordering::SeqCst);
    MoltObject::none().bits()
}

extern "C" fn protocol_property_returns_callable(self_bits: u64) -> u64 {
    crate::with_gil_entry_nopanic!(_py, {
        BIND_A.store(self_bits, Ordering::SeqCst);
        let callable_bits = INDIRECT_RETURN_CALLABLE.load(Ordering::SeqCst);
        if callable_bits == 0 {
            return raise_exception::<u64>(_py, "RuntimeError", "missing protocol callable");
        }
        inc_ref_bits(_py, callable_bits);
        callable_bits
    })
}

extern "C" fn protocol_custom_hook_get(self_bits: u64, instance_bits: u64, owner_bits: u64) -> u64 {
    crate::with_gil_entry_nopanic!(_py, {
        BIND_A.store(self_bits, Ordering::SeqCst);
        BIND_B.store(instance_bits, Ordering::SeqCst);
        BIND_C.store(owner_bits, Ordering::SeqCst);
        let callable_bits = INDIRECT_RETURN_CALLABLE.load(Ordering::SeqCst);
        if callable_bits == 0 {
            return raise_exception::<u64>(_py, "RuntimeError", "missing protocol callable");
        }
        inc_ref_bits(_py, callable_bits);
        callable_bits
    })
}

extern "C" fn protocol_indirect_set(instance_bits: u64, value_bits: u64) -> u64 {
    HOOK_A.store(instance_bits, Ordering::SeqCst);
    HOOK_B.store(value_bits, Ordering::SeqCst);
    MoltObject::none().bits()
}

extern "C" fn protocol_indirect_delete(instance_bits: u64) -> u64 {
    HOOK_A.store(instance_bits, Ordering::SeqCst);
    MoltObject::none().bits()
}

extern "C" fn protocol_standalone_call0() -> u64 {
    MoltObject::from_int(200).bits()
}

extern "C" fn protocol_standalone_call1(arg_bits: u64) -> u64 {
    HOOK_A.store(arg_bits, Ordering::SeqCst);
    MoltObject::from_int(201).bits()
}

extern "C" fn protocol_standalone_call2(first_bits: u64, second_bits: u64) -> u64 {
    HOOK_A.store(first_bits, Ordering::SeqCst);
    HOOK_B.store(second_bits, Ordering::SeqCst);
    MoltObject::from_int(202).bits()
}

extern "C" fn protocol_standalone_call3(first_bits: u64, second_bits: u64, third_bits: u64) -> u64 {
    HOOK_A.store(first_bits, Ordering::SeqCst);
    HOOK_B.store(second_bits, Ordering::SeqCst);
    HOOK_C.store(third_bits, Ordering::SeqCst);
    MoltObject::from_int(203).bits()
}

extern "C" fn protocol_standalone_rebind_owner(arg_bits: u64) -> u64 {
    crate::with_gil_entry_nopanic!(_py, {
        HOOK_A.store(arg_bits, Ordering::SeqCst);
        let owner_bits = STATICMETHOD_OWNER.load(Ordering::SeqCst);
        let name_bits = string_bits(_py, b"wrapped");
        let result = crate::molt_set_attr_name(owner_bits, name_bits, MoltObject::none().bits());
        dec_ref_bits(_py, name_bits);
        if exception_pending(_py) {
            result
        } else {
            MoltObject::from_int(204).bits()
        }
    })
}

extern "C" fn protocol_metaclass_getattribute(class_bits: u64, name_bits: u64) -> u64 {
    crate::with_gil_entry_nopanic!(_py, {
        match crate::string_obj_to_owned(obj_from_bits(name_bits)).as_deref() {
            Some("controlled") => {
                HOOK_A.store(class_bits, Ordering::SeqCst);
                HOOK_B.store(name_bits, Ordering::SeqCst);
                MoltObject::from_int(103).bits()
            }
            Some("__getattribute__") => MoltObject::from_int(104).bits(),
            Some("__getattr__") => MoltObject::from_int(105).bits(),
            _ => crate::molt_type_getattribute(class_bits, name_bits),
        }
    })
}

extern "C" fn protocol_mutating_getattribute(_self_bits: u64, _name_bits: u64) -> u64 {
    crate::with_gil_entry_nopanic!(_py, {
        let class_bits = FALLBACK_CLASS.load(Ordering::SeqCst);
        let replacement_bits = FALLBACK_REPLACEMENT.load(Ordering::SeqCst);
        let getattr_name = string_bits(_py, b"__getattr__");
        let _ = crate::molt_set_attr_name(class_bits, getattr_name, replacement_bits);
        dec_ref_bits(_py, getattr_name);
        if exception_pending(_py) {
            MoltObject::none().bits()
        } else {
            raise_exception::<u64>(_py, "AttributeError", "primary miss")
        }
    })
}

extern "C" fn protocol_original_getattr(self_bits: u64, name_bits: u64) -> u64 {
    HOOK_A.store(self_bits, Ordering::SeqCst);
    HOOK_B.store(name_bits, Ordering::SeqCst);
    FALLBACK_ORIGINAL_CALLS.fetch_add(1, Ordering::SeqCst);
    MoltObject::from_int(301).bits()
}

extern "C" fn protocol_fresh_getattr(_self_bits: u64, _name_bits: u64) -> u64 {
    FALLBACK_FRESH_CALLS.fetch_add(1, Ordering::SeqCst);
    MoltObject::from_int(302).bits()
}

extern "C" fn protocol_literal_getattribute(_self_bits: u64, name_bits: u64) -> u64 {
    crate::with_gil_entry_nopanic!(_py, {
        match crate::string_obj_to_owned(obj_from_bits(name_bits)).as_deref() {
            Some("__getattribute__") => MoltObject::from_int(303).bits(),
            Some("__getattr__") => MoltObject::from_int(304).bits(),
            _ => raise_exception::<u64>(_py, "AttributeError", "literal test miss"),
        }
    })
}

extern "C" fn protocol_call_property_attribute_error(_self_bits: u64) -> u64 {
    crate::with_gil_entry_nopanic!(_py, {
        raise_exception::<u64>(_py, "AttributeError", "property call bind failure")
    })
}

extern "C" fn protocol_call_property_runtime_error(_self_bits: u64) -> u64 {
    crate::with_gil_entry_nopanic!(_py, {
        raise_exception::<u64>(_py, "RuntimeError", "property call bind failure")
    })
}

extern "C" fn protocol_call_descriptor_attribute_error(
    _self_bits: u64,
    _instance_bits: u64,
    _owner_bits: u64,
) -> u64 {
    crate::with_gil_entry_nopanic!(_py, {
        raise_exception::<u64>(_py, "AttributeError", "descriptor call bind failure")
    })
}

extern "C" fn protocol_call_descriptor_runtime_error(
    _self_bits: u64,
    _instance_bits: u64,
    _owner_bits: u64,
) -> u64 {
    crate::with_gil_entry_nopanic!(_py, {
        raise_exception::<u64>(_py, "RuntimeError", "descriptor call bind failure")
    })
}

extern "C" fn protocol_error_set(_self_bits: u64, _instance_bits: u64, _value_bits: u64) -> u64 {
    crate::with_gil_entry_nopanic!(_py, {
        raise_exception::<u64>(_py, "RuntimeError", "protocol set failure")
    })
}

extern "C" fn protocol_self_replacing_set(
    self_bits: u64,
    instance_bits: u64,
    value_bits: u64,
) -> u64 {
    crate::with_gil_entry_nopanic!(_py, {
        HOOK_A.store(self_bits, Ordering::SeqCst);
        HOOK_B.store(instance_bits, Ordering::SeqCst);
        HOOK_C.store(value_bits, Ordering::SeqCst);
        let class_bits = type_of_bits(_py, self_bits);
        let name_bits = string_bits(_py, b"__set__");
        let _ = crate::molt_set_attr_name(class_bits, name_bits, MoltObject::none().bits());
        dec_ref_bits(_py, name_bits);
        MoltObject::none().bits()
    })
}

fn wrapped_bits(_py: &PyToken<'_>, function_bits: u64, classmethod: bool) -> u64 {
    let ptr = if classmethod {
        crate::alloc_classmethod_obj(_py, function_bits)
    } else {
        crate::alloc_staticmethod_obj(_py, function_bits)
    };
    assert!(!ptr.is_null());
    MoltObject::from_ptr(ptr).bits()
}

fn descriptor_instance_bits(_py: &PyToken<'_>, name: &[u8], attrs: &[(&[u8], u64)]) -> (u64, u64) {
    let class_bits = test_class_bits(_py, name, attrs);
    let class_ptr = obj_from_bits(class_bits)
        .as_ptr()
        .expect("descriptor class");
    let instance_bits = unsafe { crate::alloc_instance_for_class(_py, class_ptr) };
    assert!(!obj_from_bits(instance_bits).is_none());
    (class_bits, instance_bits)
}

fn test_type_bits(
    _py: &PyToken<'_>,
    metaclass_bits: u64,
    name: &[u8],
    base_bits: u64,
    attrs: &[(&[u8], u64)],
) -> u64 {
    let name_bits = string_bits(_py, name);
    let namespace_bits = crate::molt_dict_new(attrs.len() as u64);
    assert!(!obj_from_bits(namespace_bits).is_none());
    for &(attr_name, value_bits) in attrs {
        let attr_bits = string_bits(_py, attr_name);
        assert_eq!(
            crate::c_api::molt_mapping_setitem(namespace_bits, attr_bits, value_bits),
            0
        );
        dec_ref_bits(_py, attr_bits);
    }
    let class_bits = crate::builtins::types::molt_type_new(
        metaclass_bits,
        name_bits,
        base_bits,
        namespace_bits,
        MoltObject::none().bits(),
    );
    assert!(!obj_from_bits(class_bits).is_none());
    assert!(!exception_pending(_py));
    dec_ref_bits(_py, namespace_bits);
    dec_ref_bits(_py, name_bits);
    class_bits
}

fn assert_and_clear_error(_py: &PyToken<'_>, expected: &str) {
    assert!(exception_pending(_py));
    let error_bits = crate::builtins::exceptions::molt_exception_last_pending();
    assert!(crate::builtins::exceptions::exception_matches_builtin_name(
        _py, error_bits, expected
    ));
    crate::molt_exception_clear();
    dec_ref_bits(_py, error_bits);
}

fn set_runtime_arg_names(_py: &PyToken<'_>, function_bits: u64, names: &[&[u8]]) {
    let name_bits: Vec<u64> = names.iter().map(|name| string_bits(_py, name)).collect();
    let names_ptr = crate::alloc_tuple(_py, name_bits.as_slice());
    assert!(!names_ptr.is_null());
    let names_bits = MoltObject::from_ptr(names_ptr).bits();
    let metadata_name = string_bits(_py, b"__molt_arg_names__");
    let function_ptr = obj_from_bits(function_bits)
        .as_ptr()
        .expect("runtime function");
    unsafe {
        assert!(crate::call::class_init::function_set_attr_bits(
            _py,
            function_ptr,
            metadata_name,
            names_bits,
        ));
    }
    assert!(!exception_pending(_py));
    dec_ref_bits(_py, metadata_name);
    dec_ref_bits(_py, names_bits);
    for bits in name_bits {
        dec_ref_bits(_py, bits);
    }
}

fn assert_call_binding_error(_py: &PyToken<'_>, callable_bits: u64, expected: &str) {
    assert!(crate::builtins::callable::is_callable_impl(
        _py,
        callable_bits
    ));
    assert_eq!(
        unsafe { crate::call_callable0(_py, callable_bits) },
        MoltObject::none().bits()
    );
    assert_and_clear_error(_py, expected);

    let keyword_name = string_bits(_py, b"probe");
    let builder_bits = crate::molt_callargs_new(0, 1);
    let _ = unsafe {
        crate::molt_callargs_push_kw(builder_bits, keyword_name, MoltObject::from_int(1).bits())
    };
    assert!(!exception_pending(_py));
    assert_eq!(
        crate::molt_call_bind(callable_bits, builder_bits),
        MoltObject::none().bits()
    );
    assert_and_clear_error(_py, expected);
    dec_ref_bits(_py, keyword_name);
}

fn assert_mutation_binding_errors(_py: &PyToken<'_>, hook_bits: u64, expected: &str) {
    let target_name = string_bits(_py, b"target");
    let setter_class = test_class_bits(
        _py,
        b"SetattrBindingError",
        &[
            (b"__setattr__", hook_bits),
            (b"target", MoltObject::from_int(88).bits()),
        ],
    );
    let setter_class_ptr = obj_from_bits(setter_class)
        .as_ptr()
        .expect("setter binding class");
    let setter_bits = unsafe { crate::alloc_instance_for_class(_py, setter_class_ptr) };
    assert!(!obj_from_bits(setter_bits).is_none());
    assert_eq!(
        crate::molt_set_attr_name(setter_bits, target_name, MoltObject::from_int(99).bits(),),
        MoltObject::none().bits()
    );
    assert_and_clear_error(_py, expected);
    let unshadowed = crate::molt_get_attr_name(setter_bits, target_name);
    assert_eq!(unshadowed, MoltObject::from_int(88).bits());
    dec_ref_bits(_py, unshadowed);
    dec_ref_bits(_py, setter_bits);
    dec_ref_bits(_py, setter_class);

    let deleter_class =
        test_class_bits(_py, b"DelattrBindingError", &[(b"__delattr__", hook_bits)]);
    let deleter_class_ptr = obj_from_bits(deleter_class)
        .as_ptr()
        .expect("deleter binding class");
    let deleter_bits = unsafe { crate::alloc_instance_for_class(_py, deleter_class_ptr) };
    assert!(!obj_from_bits(deleter_bits).is_none());
    assert_eq!(
        crate::molt_set_attr_name(deleter_bits, target_name, MoltObject::from_int(77).bits(),),
        MoltObject::none().bits()
    );
    assert!(!exception_pending(_py));
    assert_eq!(
        crate::molt_del_attr_name(deleter_bits, target_name),
        MoltObject::none().bits()
    );
    assert_and_clear_error(_py, expected);
    let preserved = crate::molt_get_attr_name(deleter_bits, target_name);
    assert_eq!(preserved, MoltObject::from_int(77).bits());
    dec_ref_bits(_py, preserved);
    dec_ref_bits(_py, deleter_bits);
    dec_ref_bits(_py, deleter_class);
    dec_ref_bits(_py, target_name);
}

#[test]
fn wrapped_descriptor_get_is_raw_while_mutation_hooks_bind() {
    let _transaction = crate::test_support::RuntimeTestTransaction::new();
    crate::with_gil_entry_nopanic!(_py, {
        let builtins = builtin_classes(_py);
        let owner_ptr = obj_from_bits(builtins.int).as_ptr().expect("owner class");
        let receiver_bits = MoltObject::from_float(-0.0).bits();
        let value_bits = MoltObject::from_int(77).bits();

        for classmethod in [false, true] {
            let prefix = if classmethod { "class" } else { "static" };
            let get_function = runtime_function_bits(
                _py,
                if classmethod {
                    "protocol_class_get"
                } else {
                    "protocol_static_get"
                },
                if classmethod {
                    protocol_class_get as *const ()
                } else {
                    protocol_static_get as *const ()
                },
                3,
            );
            let set_function = runtime_function_bits(
                _py,
                if classmethod {
                    "protocol_class_set"
                } else {
                    "protocol_static_set"
                },
                if classmethod {
                    protocol_class_set as *const ()
                } else {
                    protocol_static_set as *const ()
                },
                if classmethod { 3 } else { 2 },
            );
            let delete_function = runtime_function_bits(
                _py,
                if classmethod {
                    "protocol_class_delete"
                } else {
                    "protocol_static_delete"
                },
                if classmethod {
                    protocol_class_delete as *const ()
                } else {
                    protocol_static_delete as *const ()
                },
                if classmethod { 2 } else { 1 },
            );
            let get_hook = wrapped_bits(_py, get_function, classmethod);
            let set_hook = wrapped_bits(_py, set_function, classmethod);
            let delete_hook = wrapped_bits(_py, delete_function, classmethod);
            let (descriptor_class, descriptor_bits) = descriptor_instance_bits(
                _py,
                if classmethod {
                    b"ClassMethodProtocolDescriptor"
                } else {
                    b"StaticMethodProtocolDescriptor"
                },
                &[
                    (b"__get__", get_hook),
                    (b"__set__", set_hook),
                    (b"__delete__", delete_hook),
                ],
            );

            reset_hook_args();
            let get_result = unsafe {
                descriptor_bind(
                    _py,
                    descriptor_bits,
                    Some(MoltObject::from_ptr(owner_ptr).bits()),
                    Some(receiver_bits),
                )
            };
            if classmethod {
                assert_eq!(get_result, Some(MoltObject::none().bits()));
                assert_and_clear_error(_py, "TypeError");
                assert_eq!(HOOK_A.load(Ordering::SeqCst), 0);
            } else {
                let get_result =
                    get_result.unwrap_or_else(|| panic!("{prefix} __get__ must invoke"));
                assert_eq!(get_result, MoltObject::from_int(101).bits());
                assert_eq!(HOOK_A.load(Ordering::SeqCst), descriptor_bits);
                assert_eq!(HOOK_B.load(Ordering::SeqCst), receiver_bits);
                assert_eq!(HOOK_C.load(Ordering::SeqCst), builtins.int);
                dec_ref_bits(_py, get_result);
            }

            reset_hook_args();
            assert_eq!(
                unsafe {
                    descriptor_mutate(
                        _py,
                        descriptor_bits,
                        receiver_bits,
                        DescriptorMutation::Set(value_bits),
                    )
                },
                DescriptorMutationOutcome::Applied
            );
            if classmethod {
                assert_eq!(HOOK_A.load(Ordering::SeqCst), descriptor_class);
                assert_eq!(HOOK_B.load(Ordering::SeqCst), receiver_bits);
                assert_eq!(HOOK_C.load(Ordering::SeqCst), value_bits);
            } else {
                assert_eq!(HOOK_A.load(Ordering::SeqCst), receiver_bits);
                assert_eq!(HOOK_B.load(Ordering::SeqCst), value_bits);
            }

            reset_hook_args();
            assert_eq!(
                unsafe {
                    descriptor_mutate(
                        _py,
                        descriptor_bits,
                        receiver_bits,
                        DescriptorMutation::Delete,
                    )
                },
                DescriptorMutationOutcome::Applied
            );
            if classmethod {
                assert_eq!(HOOK_A.load(Ordering::SeqCst), descriptor_class);
                assert_eq!(HOOK_B.load(Ordering::SeqCst), receiver_bits);
            } else {
                assert_eq!(HOOK_A.load(Ordering::SeqCst), receiver_bits);
            }
            assert!(!exception_pending(_py));

            dec_ref_bits(_py, descriptor_bits);
            dec_ref_bits(_py, descriptor_class);
            for bits in [delete_hook, set_hook, get_hook] {
                dec_ref_bits(_py, bits);
            }
            for bits in [delete_function, set_function, get_function] {
                dec_ref_bits(_py, bits);
            }
        }
    });
}

#[test]
fn standalone_staticmethod_callability_forwards_every_call_lane() {
    let _transaction = crate::test_support::RuntimeTestTransaction::new();
    crate::with_gil_entry_nopanic!(_py, {
        let function0 = runtime_function_bits(
            _py,
            "protocol_standalone_call0",
            protocol_standalone_call0 as *const (),
            0,
        );
        let function1 = runtime_function_bits(
            _py,
            "protocol_standalone_call1",
            protocol_standalone_call1 as *const (),
            1,
        );
        let function2 = runtime_function_bits(
            _py,
            "protocol_standalone_call2",
            protocol_standalone_call2 as *const (),
            2,
        );
        let function3 = runtime_function_bits(
            _py,
            "protocol_standalone_call3",
            protocol_standalone_call3 as *const (),
            3,
        );
        set_runtime_arg_names(_py, function2, &[b"first", b"second"]);

        let wrapper0 = wrapped_bits(_py, function0, false);
        let wrapper1 = wrapped_bits(_py, function1, false);
        let wrapper2 = wrapped_bits(_py, function2, false);
        let wrapper3 = wrapped_bits(_py, function3, false);
        for wrapper in [wrapper0, wrapper1, wrapper2, wrapper3] {
            assert!(crate::builtins::callable::is_callable_impl(_py, wrapper));
        }

        let classmethod = wrapped_bits(_py, function0, true);
        assert!(!crate::builtins::callable::is_callable_impl(
            _py,
            classmethod
        ));
        assert_eq!(
            unsafe { crate::call_callable0(_py, classmethod) },
            MoltObject::none().bits()
        );
        assert_and_clear_error(_py, "TypeError");
        let wrapped_classmethod = wrapped_bits(_py, classmethod, false);
        assert!(crate::builtins::callable::is_callable_impl(
            _py,
            wrapped_classmethod
        ));
        assert_eq!(
            unsafe { crate::call_callable0(_py, wrapped_classmethod) },
            MoltObject::none().bits()
        );
        assert_and_clear_error(_py, "TypeError");

        let noncallable = wrapped_bits(_py, MoltObject::from_int(9).bits(), false);
        assert!(crate::builtins::callable::is_callable_impl(
            _py,
            noncallable
        ));
        assert_eq!(
            unsafe { crate::call_callable0(_py, noncallable) },
            MoltObject::none().bits()
        );
        assert_and_clear_error(_py, "TypeError");

        assert_eq!(
            unsafe { crate::call_callable0(_py, wrapper0) },
            MoltObject::from_int(200).bits()
        );
        reset_hook_args();
        assert_eq!(
            unsafe { crate::call_callable1(_py, wrapper1, MoltObject::from_int(11).bits()) },
            MoltObject::from_int(201).bits()
        );
        assert_eq!(
            HOOK_A.load(Ordering::SeqCst),
            MoltObject::from_int(11).bits()
        );
        reset_hook_args();
        assert_eq!(
            unsafe {
                crate::call_callable2(
                    _py,
                    wrapper2,
                    MoltObject::from_int(21).bits(),
                    MoltObject::from_int(22).bits(),
                )
            },
            MoltObject::from_int(202).bits()
        );
        assert_eq!(
            HOOK_A.load(Ordering::SeqCst),
            MoltObject::from_int(21).bits()
        );
        assert_eq!(
            HOOK_B.load(Ordering::SeqCst),
            MoltObject::from_int(22).bits()
        );
        reset_hook_args();
        assert_eq!(
            unsafe {
                crate::call_callable3(
                    _py,
                    wrapper3,
                    MoltObject::from_int(31).bits(),
                    MoltObject::from_int(32).bits(),
                    MoltObject::from_int(33).bits(),
                )
            },
            MoltObject::from_int(203).bits()
        );
        assert_eq!(
            HOOK_A.load(Ordering::SeqCst),
            MoltObject::from_int(31).bits()
        );
        assert_eq!(
            HOOK_B.load(Ordering::SeqCst),
            MoltObject::from_int(32).bits()
        );
        assert_eq!(
            HOOK_C.load(Ordering::SeqCst),
            MoltObject::from_int(33).bits()
        );

        reset_hook_args();
        let second_name = string_bits(_py, b"second");
        let builder_bits = crate::molt_callargs_new(1, 1);
        assert_ne!(builder_bits, 0);
        let _ =
            unsafe { crate::molt_callargs_push_pos(builder_bits, MoltObject::from_int(41).bits()) };
        let _ = unsafe {
            crate::molt_callargs_push_kw(builder_bits, second_name, MoltObject::from_int(42).bits())
        };
        assert!(!exception_pending(_py));
        assert_eq!(
            crate::molt_call_bind(wrapper2, builder_bits),
            MoltObject::from_int(202).bits()
        );
        assert_eq!(
            HOOK_A.load(Ordering::SeqCst),
            MoltObject::from_int(41).bits()
        );
        assert_eq!(
            HOOK_B.load(Ordering::SeqCst),
            MoltObject::from_int(42).bits()
        );
        dec_ref_bits(_py, second_name);

        reset_hook_args();
        let unexpected_name = string_bits(_py, b"unexpected");
        let unknown_builder = crate::molt_callargs_new(2, 1);
        let _ = unsafe {
            crate::molt_callargs_push_pos(unknown_builder, MoltObject::from_int(43).bits())
        };
        let _ = unsafe {
            crate::molt_callargs_push_pos(unknown_builder, MoltObject::from_int(44).bits())
        };
        let _ = unsafe {
            crate::molt_callargs_push_kw(
                unknown_builder,
                unexpected_name,
                MoltObject::from_int(45).bits(),
            )
        };
        assert_eq!(
            crate::molt_call_bind(wrapper2, unknown_builder),
            MoltObject::none().bits()
        );
        assert_and_clear_error(_py, "TypeError");
        assert_eq!(HOOK_A.load(Ordering::SeqCst), 0);
        dec_ref_bits(_py, unexpected_name);

        let first_name = string_bits(_py, b"first");
        let duplicate_builder = crate::molt_callargs_new(1, 1);
        let _ = unsafe {
            crate::molt_callargs_push_pos(duplicate_builder, MoltObject::from_int(46).bits())
        };
        let _ = unsafe {
            crate::molt_callargs_push_kw(
                duplicate_builder,
                first_name,
                MoltObject::from_int(47).bits(),
            )
        };
        assert_eq!(
            crate::molt_call_bind(wrapper2, duplicate_builder),
            MoltObject::none().bits()
        );
        assert_and_clear_error(_py, "TypeError");
        assert_eq!(HOOK_A.load(Ordering::SeqCst), 0);
        dec_ref_bits(_py, first_name);

        let nested = wrapped_bits(_py, wrapper1, false);
        reset_hook_args();
        assert!(crate::builtins::callable::is_callable_impl(_py, nested));
        assert_eq!(
            unsafe { crate::call_callable1(_py, nested, MoltObject::from_int(51).bits()) },
            MoltObject::from_int(201).bits()
        );
        assert_eq!(
            HOOK_A.load(Ordering::SeqCst),
            MoltObject::from_int(51).bits()
        );

        let mut deep_wrappers = Vec::with_capacity(crate::DEFAULT_RECURSION_LIMIT + 1);
        let mut deep_target = function0;
        for _ in 0..=crate::DEFAULT_RECURSION_LIMIT {
            let wrapper = wrapped_bits(_py, deep_target, false);
            deep_wrappers.push(wrapper);
            deep_target = wrapper;
        }
        let observed_wrapper = deep_wrappers[crate::DEFAULT_RECURSION_LIMIT / 2];
        let observed_refcount = heap_refcount(observed_wrapper);
        assert_eq!(
            unsafe { crate::call_callable0(_py, deep_target) },
            MoltObject::none().bits()
        );
        assert_and_clear_error(_py, "RecursionError");
        assert_eq!(heap_refcount(observed_wrapper), observed_refcount);
        for wrapper in deep_wrappers.into_iter().rev() {
            dec_ref_bits(_py, wrapper);
        }

        let lifetime_function = runtime_function_bits(
            _py,
            "protocol_standalone_rebind_owner",
            protocol_standalone_rebind_owner as *const (),
            1,
        );
        let lifetime_wrapper = wrapped_bits(_py, lifetime_function, false);
        let owner_bits = test_class_bits(
            _py,
            b"StandaloneStaticmethodOwner",
            &[(b"wrapped", lifetime_wrapper)],
        );
        STATICMETHOD_OWNER.store(owner_bits, Ordering::SeqCst);
        // The class namespace is now the sole owner of both wrapper and target.
        dec_ref_bits(_py, lifetime_wrapper);
        dec_ref_bits(_py, lifetime_function);
        reset_hook_args();
        assert_eq!(
            unsafe {
                crate::call_callable1(_py, lifetime_wrapper, MoltObject::from_int(61).bits())
            },
            MoltObject::from_int(204).bits()
        );
        assert_eq!(
            HOOK_A.load(Ordering::SeqCst),
            MoltObject::from_int(61).bits()
        );
        assert!(!exception_pending(_py));
        let wrapped_name = string_bits(_py, b"wrapped");
        let replacement = crate::molt_get_attr_name(owner_bits, wrapped_name);
        assert_eq!(replacement, MoltObject::none().bits());
        dec_ref_bits(_py, replacement);
        dec_ref_bits(_py, wrapped_name);
        STATICMETHOD_OWNER.store(0, Ordering::SeqCst);
        dec_ref_bits(_py, owner_bits);

        dec_ref_bits(_py, nested);
        dec_ref_bits(_py, noncallable);
        dec_ref_bits(_py, wrapped_classmethod);
        dec_ref_bits(_py, classmethod);
        for wrapper in [wrapper3, wrapper2, wrapper1, wrapper0] {
            dec_ref_bits(_py, wrapper);
        }
        for function in [function3, function2, function1, function0] {
            dec_ref_bits(_py, function);
        }
    });
}

#[test]
fn descriptor_binding_errors_survive_call_and_mutation_consumers() {
    let _transaction = crate::test_support::RuntimeTestTransaction::new();
    crate::with_gil_entry_nopanic!(_py, {
        let none_bits = MoltObject::none().bits();
        for (name, target, expected) in [
            (
                "protocol_call_property_attribute_error",
                protocol_call_property_attribute_error as *const (),
                "AttributeError",
            ),
            (
                "protocol_call_property_runtime_error",
                protocol_call_property_runtime_error as *const (),
                "RuntimeError",
            ),
        ] {
            let getter_bits = runtime_function_bits(_py, name, target, 1);
            let property_ptr = crate::alloc_property_obj(_py, getter_bits, none_bits, none_bits);
            assert!(!property_ptr.is_null());
            let property_bits = MoltObject::from_ptr(property_ptr).bits();
            let callable_class = test_class_bits(
                _py,
                b"PropertyCallBindingError",
                &[(b"__call__", property_bits)],
            );
            let callable_class_ptr = obj_from_bits(callable_class)
                .as_ptr()
                .expect("property callable class");
            let callable_bits = unsafe { crate::alloc_instance_for_class(_py, callable_class_ptr) };
            assert!(!obj_from_bits(callable_bits).is_none());

            assert_call_binding_error(_py, callable_bits, expected);
            assert_mutation_binding_errors(_py, property_bits, expected);

            dec_ref_bits(_py, callable_bits);
            dec_ref_bits(_py, callable_class);
            dec_ref_bits(_py, property_bits);
            dec_ref_bits(_py, getter_bits);
        }

        for (name, target, expected) in [
            (
                "protocol_call_descriptor_attribute_error",
                protocol_call_descriptor_attribute_error as *const (),
                "AttributeError",
            ),
            (
                "protocol_call_descriptor_runtime_error",
                protocol_call_descriptor_runtime_error as *const (),
                "RuntimeError",
            ),
        ] {
            let get_bits = runtime_function_bits(_py, name, target, 3);
            let (descriptor_class, descriptor_bits) = descriptor_instance_bits(
                _py,
                b"CallBindingErrorDescriptor",
                &[(b"__get__", get_bits)],
            );
            let callable_class = test_class_bits(
                _py,
                b"CustomDescriptorCallBindingError",
                &[(b"__call__", descriptor_bits)],
            );
            let callable_class_ptr = obj_from_bits(callable_class)
                .as_ptr()
                .expect("descriptor callable class");
            let callable_bits = unsafe { crate::alloc_instance_for_class(_py, callable_class_ptr) };
            assert!(!obj_from_bits(callable_bits).is_none());

            assert_call_binding_error(_py, callable_bits, expected);
            assert_mutation_binding_errors(_py, descriptor_bits, expected);

            dec_ref_bits(_py, callable_bits);
            dec_ref_bits(_py, callable_class);
            dec_ref_bits(_py, descriptor_bits);
            dec_ref_bits(_py, descriptor_class);
            dec_ref_bits(_py, get_bits);
        }
    });
}

#[test]
fn descriptor_call_policies_are_versioned_only_for_binding_errors() {
    let _transaction = crate::test_support::RuntimeTestTransaction::new();
    crate::with_gil_entry_nopanic!(_py, {
        let state = crate::runtime_state(_py);
        let version_snapshot = state.sys_version_info.lock().unwrap().clone();
        let owner_ptr = obj_from_bits(builtin_classes(_py).object)
            .as_ptr()
            .expect("object class");
        let receiver_bits = MoltObject::from_int(41).bits();
        let argument_bits = MoltObject::from_int(42).bits();

        for minor in [12, 13, 14] {
            *state.sys_version_info.lock().unwrap() =
                Some(crate::state::runtime_state::PythonVersionInfo {
                    major: 3,
                    minor,
                    micro: 0,
                    releaselevel: "final".to_string(),
                    serial: 0,
                });

            for (kind, binding_target, body_target) in [
                (
                    "AttributeError",
                    protocol_call_descriptor_attribute_error as *const (),
                    protocol_call_property_attribute_error as *const (),
                ),
                (
                    "RuntimeError",
                    protocol_call_descriptor_runtime_error as *const (),
                    protocol_call_property_runtime_error as *const (),
                ),
            ] {
                let binding_hook =
                    runtime_function_bits(_py, "protocol_policy_binding_error", binding_target, 3);
                let (descriptor_class, descriptor_bits) = descriptor_instance_bits(
                    _py,
                    b"PolicyBindingErrorDescriptor",
                    &[(b"__get__", binding_hook)],
                );
                let body_hook =
                    runtime_function_bits(_py, "protocol_policy_body_error", body_target, 1);

                for (policy, binding_error_propagates) in [
                    (DescriptorCallPolicy::Required, true),
                    (
                        DescriptorCallPolicy::Optional,
                        minor < 14 || kind == "RuntimeError",
                    ),
                    (
                        DescriptorCallPolicy::RichComparison,
                        minor >= 14 && kind == "RuntimeError",
                    ),
                ] {
                    let binding_result = unsafe {
                        descriptor_special_call1(
                            _py,
                            descriptor_bits,
                            owner_ptr,
                            Some(receiver_bits),
                            argument_bits,
                            policy,
                        )
                    };
                    if binding_error_propagates {
                        assert_eq!(binding_result, Some(MoltObject::none().bits()));
                        assert_and_clear_error(_py, kind);
                    } else {
                        assert_eq!(binding_result, None);
                        assert!(!exception_pending(_py));
                    }

                    let body_result = unsafe {
                        descriptor_special_call1(
                            _py,
                            body_hook,
                            owner_ptr,
                            None,
                            argument_bits,
                            policy,
                        )
                    };
                    assert_eq!(body_result, Some(MoltObject::none().bits()));
                    assert_and_clear_error(_py, kind);
                }

                dec_ref_bits(_py, body_hook);
                dec_ref_bits(_py, descriptor_bits);
                dec_ref_bits(_py, descriptor_class);
                dec_ref_bits(_py, binding_hook);
            }
        }

        *state.sys_version_info.lock().unwrap() = version_snapshot;
    });
}

#[test]
fn descriptor_valued_get_is_raw_while_mutation_hooks_bind() {
    let _transaction = crate::test_support::RuntimeTestTransaction::new();
    crate::with_gil_entry_nopanic!(_py, {
        let receiver_bits = MoltObject::from_int(31).bits();
        let value_bits = MoltObject::from_int(32).bits();
        let none_bits = MoltObject::none().bits();

        let setter_bits = runtime_function_bits(
            _py,
            "protocol_indirect_set",
            protocol_indirect_set as *const (),
            2,
        );
        let property_getter_bits = runtime_function_bits(
            _py,
            "protocol_property_returns_callable",
            protocol_property_returns_callable as *const (),
            1,
        );
        let property_ptr =
            crate::alloc_property_obj(_py, property_getter_bits, none_bits, none_bits);
        assert!(!property_ptr.is_null());
        let property_bits = MoltObject::from_ptr(property_ptr).bits();
        let (property_descriptor_class, property_descriptor_bits) = descriptor_instance_bits(
            _py,
            b"PropertyHookValueDescriptor",
            &[(b"__get__", property_bits), (b"__set__", property_bits)],
        );
        reset_hook_args();
        assert_eq!(
            unsafe {
                descriptor_bind(
                    _py,
                    property_descriptor_bits,
                    Some(builtin_classes(_py).int),
                    Some(receiver_bits),
                )
            },
            Some(none_bits)
        );
        assert_and_clear_error(_py, "TypeError");
        assert_eq!(BIND_A.load(Ordering::SeqCst), 0);
        INDIRECT_RETURN_CALLABLE.store(setter_bits, Ordering::SeqCst);
        reset_hook_args();
        assert_eq!(
            unsafe {
                descriptor_mutate(
                    _py,
                    property_descriptor_bits,
                    receiver_bits,
                    DescriptorMutation::Set(value_bits),
                )
            },
            DescriptorMutationOutcome::Applied
        );
        assert_eq!(BIND_A.load(Ordering::SeqCst), property_descriptor_bits);
        assert_eq!(HOOK_A.load(Ordering::SeqCst), receiver_bits);
        assert_eq!(HOOK_B.load(Ordering::SeqCst), value_bits);
        assert!(!exception_pending(_py));

        let delete_callable_bits = runtime_function_bits(
            _py,
            "protocol_indirect_delete",
            protocol_indirect_delete as *const (),
            1,
        );
        let hook_get_bits = runtime_function_bits(
            _py,
            "protocol_custom_hook_get",
            protocol_custom_hook_get as *const (),
            3,
        );
        let (hook_class_bits, hook_bits) =
            descriptor_instance_bits(_py, b"ProtocolHookValue", &[(b"__get__", hook_get_bits)]);
        let (custom_descriptor_class, custom_descriptor_bits) = descriptor_instance_bits(
            _py,
            b"CustomHookValueDescriptor",
            &[(b"__get__", hook_bits), (b"__delete__", hook_bits)],
        );
        reset_hook_args();
        assert_eq!(
            unsafe {
                descriptor_bind(
                    _py,
                    custom_descriptor_bits,
                    Some(builtin_classes(_py).int),
                    Some(receiver_bits),
                )
            },
            Some(none_bits)
        );
        assert_and_clear_error(_py, "TypeError");
        assert_eq!(BIND_A.load(Ordering::SeqCst), 0);
        INDIRECT_RETURN_CALLABLE.store(delete_callable_bits, Ordering::SeqCst);
        reset_hook_args();
        assert_eq!(
            unsafe {
                descriptor_mutate(
                    _py,
                    custom_descriptor_bits,
                    receiver_bits,
                    DescriptorMutation::Delete,
                )
            },
            DescriptorMutationOutcome::Applied
        );
        assert_eq!(BIND_A.load(Ordering::SeqCst), hook_bits);
        assert_eq!(BIND_B.load(Ordering::SeqCst), custom_descriptor_bits);
        assert_eq!(BIND_C.load(Ordering::SeqCst), custom_descriptor_class);
        assert_eq!(HOOK_A.load(Ordering::SeqCst), receiver_bits);
        assert!(!exception_pending(_py));

        INDIRECT_RETURN_CALLABLE.store(0, Ordering::SeqCst);
        dec_ref_bits(_py, custom_descriptor_bits);
        dec_ref_bits(_py, custom_descriptor_class);
        dec_ref_bits(_py, hook_bits);
        dec_ref_bits(_py, hook_class_bits);
        dec_ref_bits(_py, hook_get_bits);
        dec_ref_bits(_py, delete_callable_bits);
        dec_ref_bits(_py, property_descriptor_bits);
        dec_ref_bits(_py, property_descriptor_class);
        dec_ref_bits(_py, property_bits);
        dec_ref_bits(_py, property_getter_bits);
        dec_ref_bits(_py, setter_bits);
    });
}

#[test]
fn class_objects_use_metaclass_descriptor_hooks_and_ignore_class_local_lookalikes() {
    let _transaction = crate::test_support::RuntimeTestTransaction::new();
    crate::with_gil_entry_nopanic!(_py, {
        let builtins = builtin_classes(_py);
        let receiver_bits = MoltObject::from_int(41).bits();
        let value_bits = MoltObject::from_int(42).bits();
        let owner_ptr = obj_from_bits(builtins.int).as_ptr().expect("owner class");

        let get_bits = runtime_function_bits(
            _py,
            "protocol_metaclass_get",
            protocol_class_get as *const (),
            3,
        );
        let set_bits = runtime_function_bits(
            _py,
            "protocol_metaclass_set",
            protocol_class_set as *const (),
            3,
        );
        let delete_bits = runtime_function_bits(
            _py,
            "protocol_metaclass_delete",
            protocol_class_delete as *const (),
            2,
        );
        let (managed_descriptor_class, managed_descriptor_bits) = descriptor_instance_bits(
            _py,
            b"ManagedClassAttributeDescriptor",
            &[
                (b"__get__", get_bits),
                (b"__set__", set_bits),
                (b"__delete__", delete_bits),
            ],
        );
        let metaclass_bits = test_type_bits(
            _py,
            builtins.type_obj,
            b"ProtocolMeta",
            builtins.type_obj,
            &[
                (b"__get__", get_bits),
                (b"__set__", set_bits),
                (b"__delete__", delete_bits),
                (b"controlled", managed_descriptor_bits),
            ],
        );
        let class_descriptor_bits = test_type_bits(
            _py,
            metaclass_bits,
            b"ProtocolClassDescriptor",
            builtins.object,
            &[(b"controlled", MoltObject::from_int(-1).bits())],
        );

        reset_hook_args();
        let get_result = unsafe {
            descriptor_bind(
                _py,
                class_descriptor_bits,
                Some(MoltObject::from_ptr(owner_ptr).bits()),
                Some(receiver_bits),
            )
        }
        .expect("metaclass __get__ must bind class object");
        assert_eq!(get_result, MoltObject::from_int(102).bits());
        assert_eq!(HOOK_A.load(Ordering::SeqCst), class_descriptor_bits);
        assert_eq!(HOOK_B.load(Ordering::SeqCst), receiver_bits);
        assert_eq!(HOOK_C.load(Ordering::SeqCst), builtins.int);
        dec_ref_bits(_py, get_result);

        reset_hook_args();
        assert_eq!(
            unsafe {
                descriptor_mutate(
                    _py,
                    class_descriptor_bits,
                    receiver_bits,
                    DescriptorMutation::Set(value_bits),
                )
            },
            DescriptorMutationOutcome::Applied
        );
        assert_eq!(HOOK_A.load(Ordering::SeqCst), class_descriptor_bits);
        assert_eq!(HOOK_B.load(Ordering::SeqCst), receiver_bits);
        assert_eq!(HOOK_C.load(Ordering::SeqCst), value_bits);

        reset_hook_args();
        assert_eq!(
            unsafe {
                descriptor_mutate(
                    _py,
                    class_descriptor_bits,
                    receiver_bits,
                    DescriptorMutation::Delete,
                )
            },
            DescriptorMutationOutcome::Applied
        );
        assert_eq!(HOOK_A.load(Ordering::SeqCst), class_descriptor_bits);
        assert_eq!(HOOK_B.load(Ordering::SeqCst), receiver_bits);

        let controlled_name = string_bits(_py, b"controlled");
        reset_hook_args();
        let class_read = crate::molt_get_attr_name(class_descriptor_bits, controlled_name);
        assert_eq!(class_read, MoltObject::from_int(102).bits());
        assert_eq!(HOOK_A.load(Ordering::SeqCst), managed_descriptor_bits);
        assert_eq!(HOOK_B.load(Ordering::SeqCst), class_descriptor_bits);
        assert_eq!(HOOK_C.load(Ordering::SeqCst), metaclass_bits);
        dec_ref_bits(_py, class_read);

        reset_hook_args();
        let _ = crate::molt_set_attr_name(class_descriptor_bits, controlled_name, value_bits);
        assert!(!exception_pending(_py));
        assert_eq!(HOOK_A.load(Ordering::SeqCst), managed_descriptor_bits);
        assert_eq!(HOOK_B.load(Ordering::SeqCst), class_descriptor_bits);
        assert_eq!(HOOK_C.load(Ordering::SeqCst), value_bits);

        reset_hook_args();
        let _ = crate::molt_del_attr_name(class_descriptor_bits, controlled_name);
        assert!(!exception_pending(_py));
        assert_eq!(HOOK_A.load(Ordering::SeqCst), managed_descriptor_bits);
        assert_eq!(HOOK_B.load(Ordering::SeqCst), class_descriptor_bits);
        dec_ref_bits(_py, controlled_name);

        let misleading_class = test_class_bits(
            _py,
            b"ClassLocalDescriptorLookalike",
            &[
                (b"__get__", get_bits),
                (b"__set__", set_bits),
                (b"__delete__", delete_bits),
            ],
        );
        reset_hook_args();
        let unchanged = unsafe {
            descriptor_bind(
                _py,
                misleading_class,
                Some(MoltObject::from_ptr(owner_ptr).bits()),
                Some(receiver_bits),
            )
        }
        .expect("plain class value must be returned");
        assert_eq!(unchanged, misleading_class);
        assert_eq!(HOOK_A.load(Ordering::SeqCst), 0);
        dec_ref_bits(_py, unchanged);
        assert_eq!(
            unsafe {
                descriptor_mutate(
                    _py,
                    misleading_class,
                    receiver_bits,
                    DescriptorMutation::Set(value_bits),
                )
            },
            DescriptorMutationOutcome::NotDescriptor
        );

        dec_ref_bits(_py, misleading_class);
        dec_ref_bits(_py, class_descriptor_bits);
        dec_ref_bits(_py, metaclass_bits);
        dec_ref_bits(_py, managed_descriptor_bits);
        dec_ref_bits(_py, managed_descriptor_class);
        for bits in [delete_bits, set_bits, get_bits] {
            dec_ref_bits(_py, bits);
        }
    });
}

#[test]
fn class_access_obeys_descriptor_and_metaclass_getattribute_authority() {
    let _transaction = crate::test_support::RuntimeTestTransaction::new();
    crate::with_gil_entry_nopanic!(_py, {
        let builtins = builtin_classes(_py);
        let get_bits = runtime_function_bits(
            _py,
            "protocol_class_access_get",
            protocol_class_get as *const (),
            3,
        );
        let (descriptor_class, descriptor_bits) =
            descriptor_instance_bits(_py, b"ClassAccessDescriptor", &[(b"__get__", get_bits)]);
        let owner_bits = test_class_bits(
            _py,
            b"ClassAccessOwner",
            &[(b"controlled", descriptor_bits)],
        );
        let controlled_name = string_bits(_py, b"controlled");

        reset_hook_args();
        let result = crate::molt_get_attr_name(owner_bits, controlled_name);
        assert_eq!(result, MoltObject::from_int(102).bits());
        assert_eq!(HOOK_A.load(Ordering::SeqCst), descriptor_bits);
        assert_eq!(HOOK_B.load(Ordering::SeqCst), MoltObject::none().bits());
        assert_eq!(HOOK_C.load(Ordering::SeqCst), owner_bits);
        dec_ref_bits(_py, result);
        dec_ref_bits(_py, owner_bits);
        dec_ref_bits(_py, descriptor_bits);
        dec_ref_bits(_py, descriptor_class);
        dec_ref_bits(_py, get_bits);

        let getattribute_bits = runtime_function_bits(
            _py,
            "protocol_metaclass_getattribute",
            protocol_metaclass_getattribute as *const (),
            2,
        );
        let metaclass_bits = test_type_bits(
            _py,
            builtins.type_obj,
            b"GetattributeAuthorityMeta",
            builtins.type_obj,
            &[(b"__getattribute__", getattribute_bits)],
        );
        let managed_bits = test_type_bits(
            _py,
            metaclass_bits,
            b"GetattributeAuthorityClass",
            builtins.object,
            &[(b"controlled", MoltObject::from_int(-1).bits())],
        );

        reset_hook_args();
        let overridden = crate::molt_get_attr_name(managed_bits, controlled_name);
        assert_eq!(overridden, MoltObject::from_int(103).bits());
        assert_eq!(HOOK_A.load(Ordering::SeqCst), managed_bits);
        assert_eq!(HOOK_B.load(Ordering::SeqCst), controlled_name);
        dec_ref_bits(_py, overridden);
        let getattribute_name = string_bits(_py, b"__getattribute__");
        let getattr_name = string_bits(_py, b"__getattr__");
        assert_eq!(
            crate::molt_get_attr_name(managed_bits, getattribute_name),
            MoltObject::from_int(104).bits()
        );
        assert_eq!(
            crate::molt_get_attr_name(managed_bits, getattr_name),
            MoltObject::from_int(105).bits()
        );
        dec_ref_bits(_py, getattr_name);
        dec_ref_bits(_py, getattribute_name);
        dec_ref_bits(_py, managed_bits);
        dec_ref_bits(_py, metaclass_bits);
        dec_ref_bits(_py, getattribute_bits);
        dec_ref_bits(_py, controlled_name);
    });
}

#[test]
fn custom_getattribute_captures_fallback_and_observes_literal_hook_names() {
    let _transaction = crate::test_support::RuntimeTestTransaction::new();
    crate::with_gil_entry_nopanic!(_py, {
        let primary_bits = runtime_function_bits(
            _py,
            "protocol_mutating_getattribute",
            protocol_mutating_getattribute as *const (),
            2,
        );
        let original_bits = runtime_function_bits(
            _py,
            "protocol_original_getattr",
            protocol_original_getattr as *const (),
            2,
        );
        let fresh_bits = runtime_function_bits(
            _py,
            "protocol_fresh_getattr",
            protocol_fresh_getattr as *const (),
            2,
        );
        let class_bits = test_class_bits(
            _py,
            b"CapturedFallbackOwner",
            &[
                (b"__getattribute__", primary_bits),
                (b"__getattr__", original_bits),
            ],
        );
        let class_ptr = obj_from_bits(class_bits).as_ptr().expect("fallback class");
        let instance_bits = unsafe { crate::alloc_instance_for_class(_py, class_ptr) };
        assert!(!obj_from_bits(instance_bits).is_none());
        FALLBACK_CLASS.store(class_bits, Ordering::SeqCst);
        FALLBACK_REPLACEMENT.store(fresh_bits, Ordering::SeqCst);
        FALLBACK_ORIGINAL_CALLS.store(0, Ordering::SeqCst);
        FALLBACK_FRESH_CALLS.store(0, Ordering::SeqCst);
        // The class namespace is the sole owner of both callbacks captured by lookup.
        dec_ref_bits(_py, primary_bits);
        dec_ref_bits(_py, original_bits);

        reset_hook_args();
        let missing_name = string_bits(_py, b"missing");
        let result = crate::molt_get_attr_name(instance_bits, missing_name);
        assert_eq!(result, MoltObject::from_int(301).bits());
        assert_eq!(FALLBACK_ORIGINAL_CALLS.load(Ordering::SeqCst), 1);
        assert_eq!(FALLBACK_FRESH_CALLS.load(Ordering::SeqCst), 0);
        assert_eq!(HOOK_A.load(Ordering::SeqCst), instance_bits);
        assert_eq!(HOOK_B.load(Ordering::SeqCst), missing_name);
        assert!(!exception_pending(_py));
        let getattr_name = string_bits(_py, b"__getattr__");
        assert_eq!(
            unsafe { class_attr_lookup_raw_mro(_py, class_ptr, getattr_name) },
            Some(fresh_bits)
        );
        dec_ref_bits(_py, getattr_name);
        dec_ref_bits(_py, result);
        dec_ref_bits(_py, missing_name);
        FALLBACK_CLASS.store(0, Ordering::SeqCst);
        FALLBACK_REPLACEMENT.store(0, Ordering::SeqCst);
        dec_ref_bits(_py, instance_bits);
        dec_ref_bits(_py, class_bits);
        dec_ref_bits(_py, fresh_bits);

        let literal_bits = runtime_function_bits(
            _py,
            "protocol_literal_getattribute",
            protocol_literal_getattribute as *const (),
            2,
        );
        let literal_class = test_class_bits(
            _py,
            b"LiteralHookNameOwner",
            &[(b"__getattribute__", literal_bits)],
        );
        let literal_class_ptr = obj_from_bits(literal_class)
            .as_ptr()
            .expect("literal hook class");
        let literal_instance = unsafe { crate::alloc_instance_for_class(_py, literal_class_ptr) };
        assert!(!obj_from_bits(literal_instance).is_none());
        let getattribute_name = string_bits(_py, b"__getattribute__");
        let getattr_name = string_bits(_py, b"__getattr__");
        assert_eq!(
            crate::molt_get_attr_name(literal_instance, getattribute_name),
            MoltObject::from_int(303).bits()
        );
        assert_eq!(
            crate::molt_get_attr_name(literal_instance, getattr_name),
            MoltObject::from_int(304).bits()
        );
        assert!(!exception_pending(_py));
        dec_ref_bits(_py, getattr_name);
        dec_ref_bits(_py, getattribute_name);
        dec_ref_bits(_py, literal_instance);
        dec_ref_bits(_py, literal_class);
        dec_ref_bits(_py, literal_bits);
    });
}

#[test]
fn mutation_outcomes_and_self_replacing_hook_are_stable() {
    let _transaction = crate::test_support::RuntimeTestTransaction::new();
    crate::with_gil_entry_nopanic!(_py, {
        let receiver_bits = MoltObject::from_int(51).bits();
        let value_bits = MoltObject::from_int(52).bits();
        let none_bits = MoltObject::none().bits();
        let identity_bits = runtime_function_bits(
            _py,
            "protocol_property_identity",
            scalar_attr_identity as *const (),
            1,
        );
        let property_ptr = crate::alloc_property_obj(_py, identity_bits, none_bits, none_bits);
        assert!(!property_ptr.is_null());
        let property_bits = MoltObject::from_ptr(property_ptr).bits();
        for mutation in [
            DescriptorMutation::Set(value_bits),
            DescriptorMutation::Delete,
        ] {
            assert_eq!(
                unsafe { descriptor_mutate(_py, property_bits, receiver_bits, mutation) },
                DescriptorMutationOutcome::Error
            );
            assert!(clear_attribute_error_if_pending(_py));
        }

        let set_bits = runtime_function_bits(
            _py,
            "protocol_missing_delete_set",
            protocol_class_set as *const (),
            3,
        );
        let delete_bits = runtime_function_bits(
            _py,
            "protocol_missing_set_delete",
            protocol_class_delete as *const (),
            2,
        );
        let (set_only_class, set_only_bits) =
            descriptor_instance_bits(_py, b"SetOnlyProtocolDescriptor", &[(b"__set__", set_bits)]);
        let (delete_only_class, delete_only_bits) = descriptor_instance_bits(
            _py,
            b"DeleteOnlyProtocolDescriptor",
            &[(b"__delete__", delete_bits)],
        );
        assert_eq!(
            unsafe {
                descriptor_mutate(
                    _py,
                    set_only_bits,
                    receiver_bits,
                    DescriptorMutation::Delete,
                )
            },
            DescriptorMutationOutcome::Error
        );
        assert_and_clear_error(_py, "AttributeError");
        assert_eq!(
            unsafe {
                descriptor_mutate(
                    _py,
                    delete_only_bits,
                    receiver_bits,
                    DescriptorMutation::Set(value_bits),
                )
            },
            DescriptorMutationOutcome::Error
        );
        assert_and_clear_error(_py, "AttributeError");
        assert_eq!(
            unsafe {
                descriptor_mutate(
                    _py,
                    MoltObject::from_int(1).bits(),
                    receiver_bits,
                    DescriptorMutation::Set(value_bits),
                )
            },
            DescriptorMutationOutcome::NotDescriptor
        );

        let error_bits = runtime_function_bits(
            _py,
            "protocol_error_set",
            protocol_error_set as *const (),
            3,
        );
        let (error_class, error_descriptor) =
            descriptor_instance_bits(_py, b"ErrorProtocolDescriptor", &[(b"__set__", error_bits)]);
        assert_eq!(
            unsafe {
                descriptor_mutate(
                    _py,
                    error_descriptor,
                    receiver_bits,
                    DescriptorMutation::Set(value_bits),
                )
            },
            DescriptorMutationOutcome::Error
        );
        assert_and_clear_error(_py, "RuntimeError");

        let replacing_bits = runtime_function_bits(
            _py,
            "protocol_self_replacing_set",
            protocol_self_replacing_set as *const (),
            3,
        );
        let (replacing_class, replacing_descriptor) = descriptor_instance_bits(
            _py,
            b"SelfReplacingSetDescriptor",
            &[(b"__set__", replacing_bits)],
        );
        reset_hook_args();
        // Leave the descriptor class as the sole owner of the hook callable.
        dec_ref_bits(_py, replacing_bits);
        assert_eq!(
            unsafe {
                descriptor_mutate(
                    _py,
                    replacing_descriptor,
                    receiver_bits,
                    DescriptorMutation::Set(value_bits),
                )
            },
            DescriptorMutationOutcome::Applied
        );
        assert_eq!(HOOK_A.load(Ordering::SeqCst), replacing_descriptor);
        assert_eq!(HOOK_B.load(Ordering::SeqCst), receiver_bits);
        assert_eq!(HOOK_C.load(Ordering::SeqCst), value_bits);
        let set_name = string_bits(_py, b"__set__");
        let replacing_class_ptr = obj_from_bits(replacing_class).as_ptr().unwrap();
        assert_eq!(
            unsafe { class_attr_lookup_raw_mro(_py, replacing_class_ptr, set_name) },
            Some(none_bits)
        );
        dec_ref_bits(_py, set_name);
        assert!(!exception_pending(_py));

        dec_ref_bits(_py, replacing_descriptor);
        dec_ref_bits(_py, replacing_class);
        dec_ref_bits(_py, error_descriptor);
        dec_ref_bits(_py, error_class);
        dec_ref_bits(_py, error_bits);
        dec_ref_bits(_py, delete_only_bits);
        dec_ref_bits(_py, delete_only_class);
        dec_ref_bits(_py, set_only_bits);
        dec_ref_bits(_py, set_only_class);
        dec_ref_bits(_py, delete_bits);
        dec_ref_bits(_py, set_bits);
        dec_ref_bits(_py, property_bits);
        dec_ref_bits(_py, identity_bits);
    });
}
