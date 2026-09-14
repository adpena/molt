use super::*;
use crate::{TYPE_ID_LIST, alloc_list, call_callable0, call_callable1, header_from_obj_ptr};
use num_bigint::BigInt;
use std::sync::atomic::{AtomicU64, Ordering};

mod protocol;
mod wrapper_integration;

static CUSTOM_GET_SELF_BITS: AtomicU64 = AtomicU64::new(0);
static CUSTOM_GET_INSTANCE_BITS: AtomicU64 = AtomicU64::new(0);
static CUSTOM_GET_OWNER_BITS: AtomicU64 = AtomicU64::new(0);
static CALL1_RECEIVER_BITS: AtomicU64 = AtomicU64::new(0);
static CALL1_ARGUMENT_BITS: AtomicU64 = AtomicU64::new(0);
static MUTATING_GET_SELF_BITS: AtomicU64 = AtomicU64::new(0);
static MUTATING_GET_OWNER_BITS: AtomicU64 = AtomicU64::new(0);
static MUTATING_GET_RETURN_CALLABLE_BITS: AtomicU64 = AtomicU64::new(0);

extern "C" fn scalar_attr_identity(value_bits: u64) -> u64 {
    crate::with_gil_entry_nopanic!(_py, {
        inc_ref_bits(_py, value_bits);
        value_bits
    })
}

extern "C" fn descriptor_test_dir(_self_bits: u64) -> u64 {
    crate::with_gil_entry_nopanic!(_py, {
        let ptr = alloc_list(_py, &[]);
        if ptr.is_null() {
            MoltObject::none().bits()
        } else {
            MoltObject::from_ptr(ptr).bits()
        }
    })
}

extern "C" fn descriptor_test_property_attribute_error(_self_bits: u64) -> u64 {
    crate::with_gil_entry_nopanic!(_py, {
        raise_exception::<u64>(_py, "AttributeError", "descriptor test attribute miss")
    })
}

extern "C" fn descriptor_test_property_runtime_error(_self_bits: u64) -> u64 {
    crate::with_gil_entry_nopanic!(_py, {
        raise_exception::<u64>(_py, "RuntimeError", "descriptor test failure")
    })
}

extern "C" fn descriptor_test_call1(receiver_bits: u64, argument_bits: u64) -> u64 {
    crate::with_gil_entry_nopanic!(_py, {
        CALL1_RECEIVER_BITS.store(receiver_bits, Ordering::SeqCst);
        CALL1_ARGUMENT_BITS.store(argument_bits, Ordering::SeqCst);
        inc_ref_bits(_py, argument_bits);
        argument_bits
    })
}

extern "C" fn descriptor_test_get_replaces_own_method(
    self_bits: u64,
    _instance_bits: u64,
    owner_bits: u64,
) -> u64 {
    crate::with_gil_entry_nopanic!(_py, {
        MUTATING_GET_SELF_BITS.store(self_bits, Ordering::SeqCst);
        MUTATING_GET_OWNER_BITS.store(owner_bits, Ordering::SeqCst);

        let descriptor_class_bits = type_of_bits(_py, self_bits);
        let get_name_bits = string_bits(_py, b"__get__");
        let _ = crate::molt_set_attr_name(
            descriptor_class_bits,
            get_name_bits,
            MoltObject::none().bits(),
        );
        dec_ref_bits(_py, get_name_bits);
        if exception_pending(_py) {
            return MoltObject::none().bits();
        }

        // The class mutation above released the raw __get__ lookup owner. The
        // method pin in descriptor_bind must keep that runtime function alive
        // until this callback returns.
        let callable_bits = MUTATING_GET_RETURN_CALLABLE_BITS.load(Ordering::SeqCst);
        if callable_bits == 0 {
            return raise_exception::<u64>(_py, "RuntimeError", "missing return callable");
        }
        inc_ref_bits(_py, callable_bits);
        callable_bits
    })
}

extern "C" fn scalar_attr_custom_get_reentrant(
    self_bits: u64,
    instance_bits: u64,
    owner_bits: u64,
) -> u64 {
    crate::with_gil_entry_nopanic!(_py, {
        CUSTOM_GET_SELF_BITS.store(self_bits, Ordering::SeqCst);
        CUSTOM_GET_INSTANCE_BITS.store(instance_bits, Ordering::SeqCst);
        CUSTOM_GET_OWNER_BITS.store(owner_bits, Ordering::SeqCst);

        let name_bits = string_bits(_py, b"probe");
        let _ = crate::molt_del_attr_name(owner_bits, name_bits);
        dec_ref_bits(_py, name_bits);
        if exception_pending(_py) {
            return MoltObject::none().bits();
        }

        // Deleting the owner-class attribute above can release its last
        // reference to this descriptor. descriptor_bind must keep self alive
        // across arbitrary __get__ reentry, while this retain creates the
        // owned result returned to the caller.
        inc_ref_bits(_py, self_bits);
        self_bits
    })
}

fn string_bits(_py: &PyToken<'_>, value: &[u8]) -> u64 {
    let ptr = alloc_string(_py, value);
    assert!(!ptr.is_null());
    MoltObject::from_ptr(ptr).bits()
}

fn heap_refcount(bits: u64) -> u32 {
    let ptr = obj_from_bits(bits).as_ptr().expect("heap object");
    unsafe { (*header_from_obj_ptr(ptr)).ref_count_snapshot() }
}

fn runtime_function_bits(
    _py: &PyToken<'_>,
    name: &'static str,
    target: *const (),
    arity: u64,
) -> u64 {
    let ptr = crate::builtins::functions::alloc_runtime_function_obj(
        _py,
        crate::builtins::functions::runtime_fn_addr(name, target),
        arity,
    );
    assert!(!ptr.is_null());
    MoltObject::from_ptr(ptr).bits()
}

fn test_class_bits(_py: &PyToken<'_>, name: &[u8], attrs: &[(&[u8], u64)]) -> u64 {
    let builtins = builtin_classes(_py);
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
        builtins.type_obj,
        name_bits,
        MoltObject::none().bits(),
        namespace_bits,
        MoltObject::none().bits(),
    );
    assert!(!obj_from_bits(class_bits).is_none());
    assert!(!exception_pending(_py));
    dec_ref_bits(_py, namespace_bits);
    dec_ref_bits(_py, name_bits);
    class_bits
}

#[test]
fn descriptor_function_receiver_uses_canonical_slot_wrapper_identity() {
    let _transaction = crate::test_support::RuntimeTestTransaction::new();
    crate::with_gil_entry_nopanic!(_py, {
        let owner_bits = builtin_classes(_py).int;
        let owner_ptr = obj_from_bits(owner_bits).as_ptr().unwrap();
        let scalar_bits = MoltObject::from_int(7).bits();
        for (name, target, arity) in [
            (
                "molt_object_getattribute",
                crate::molt_object_getattribute as *const (),
                2,
            ),
            (
                "molt_object_setattr",
                crate::molt_object_setattr as *const (),
                3,
            ),
            (
                "molt_object_delattr",
                crate::molt_object_delattr as *const (),
                2,
            ),
        ] {
            let function_bits = runtime_function_bits(_py, name, target, arity);
            let function_ptr = obj_from_bits(function_bits).as_ptr().unwrap();
            unsafe {
                assert_eq!(
                    function_descriptor_receiver(function_ptr, Some(owner_bits)),
                    None
                );
                assert_eq!(
                    function_descriptor_receiver(function_ptr, Some(scalar_bits)),
                    Some(scalar_bits)
                );
                let class_value = descriptor_bind(
                    _py,
                    function_bits,
                    Some(MoltObject::from_ptr(owner_ptr).bits()),
                    Some(owner_bits),
                )
                .unwrap();
                assert_eq!(class_value, function_bits);
                dec_ref_bits(_py, class_value);
            }
            dec_ref_bits(_py, function_bits);
        }
    });
}

#[test]
fn descriptor_bind_preserves_exact_scalar_receiver_bits_for_functions_and_properties() {
    let _transaction = crate::test_support::RuntimeTestTransaction::new();
    crate::with_gil_entry_nopanic!(_py, {
        let builtins = builtin_classes(_py);
        let owner_ptr = obj_from_bits(builtins.int)
            .as_ptr()
            .expect("int class pointer");
        let function_bits = runtime_function_bits(
            _py,
            "scalar_attr_identity_exact_bits",
            scalar_attr_identity as *const (),
            1,
        );
        let property_ptr = crate::alloc_property_obj(
            _py,
            function_bits,
            MoltObject::none().bits(),
            MoltObject::none().bits(),
        );
        assert!(!property_ptr.is_null());
        let property_bits = MoltObject::from_ptr(property_ptr).bits();

        let negative_zero = MoltObject::from_float(-0.0).bits();
        assert_ne!(negative_zero, MoltObject::from_float(0.0).bits());
        let heap_bigint =
            crate::builtins::numbers::int_bits_from_bigint(_py, BigInt::from(1u64) << 100usize);
        let heap_nan = crate::object::ops::float_result_bits(_py, f64::NAN);
        let receivers = [
            ("inline int", MoltObject::from_int(42).bits()),
            ("inline bool", MoltObject::from_bool(true).bits()),
            ("inline float", MoltObject::from_float(3.25).bits()),
            ("inline negative zero", negative_zero),
            ("heap bigint", heap_bigint),
            ("heap NaN", heap_nan),
        ];

        for (label, receiver_bits) in receivers {
            let bound_bits = unsafe {
                descriptor_bind(
                    _py,
                    function_bits,
                    Some(MoltObject::from_ptr(owner_ptr).bits()),
                    Some(receiver_bits),
                )
            }
            .unwrap_or_else(|| panic!("function descriptor must bind {label}"));
            let function_result = unsafe { call_callable0(_py, bound_bits) };
            assert!(
                !exception_pending(_py),
                "function descriptor failed for {label}"
            );
            assert_eq!(
                function_result, receiver_bits,
                "function receiver changed for {label}"
            );
            dec_ref_bits(_py, function_result);
            dec_ref_bits(_py, bound_bits);

            let property_result = unsafe {
                descriptor_bind(
                    _py,
                    property_bits,
                    Some(MoltObject::from_ptr(owner_ptr).bits()),
                    Some(receiver_bits),
                )
            }
            .unwrap_or_else(|| panic!("property descriptor must bind {label}"));
            assert!(
                !exception_pending(_py),
                "property descriptor failed for {label}"
            );
            assert_eq!(
                property_result, receiver_bits,
                "property receiver changed for {label}"
            );
            dec_ref_bits(_py, property_result);
        }

        dec_ref_bits(_py, heap_nan);
        dec_ref_bits(_py, heap_bigint);
        dec_ref_bits(_py, property_bits);
        dec_ref_bits(_py, function_bits);
    });
}

#[test]
fn descriptor_bind_distinguishes_missing_instance_from_python_none() {
    let _transaction = crate::test_support::RuntimeTestTransaction::new();
    crate::with_gil_entry_nopanic!(_py, {
        let builtins = builtin_classes(_py);
        let owner_ptr = obj_from_bits(builtins.int)
            .as_ptr()
            .expect("int class pointer");
        let none_bits = MoltObject::none().bits();
        let function_bits = runtime_function_bits(
            _py,
            "scalar_attr_identity_optional_instance",
            scalar_attr_identity as *const (),
            1,
        );
        let property_ptr = crate::alloc_property_obj(_py, function_bits, none_bits, none_bits);
        assert!(!property_ptr.is_null());
        let property_bits = MoltObject::from_ptr(property_ptr).bits();

        let unbound_function = unsafe {
            descriptor_bind(
                _py,
                function_bits,
                Some(MoltObject::from_ptr(owner_ptr).bits()),
                None,
            )
        }
        .unwrap();
        assert_eq!(unbound_function, function_bits);
        dec_ref_bits(_py, unbound_function);

        let bound_to_none = unsafe {
            descriptor_bind(
                _py,
                function_bits,
                Some(MoltObject::from_ptr(owner_ptr).bits()),
                Some(none_bits),
            )
        }
        .unwrap();
        assert_ne!(bound_to_none, function_bits);
        let function_result = unsafe { call_callable0(_py, bound_to_none) };
        assert!(!exception_pending(_py));
        assert_eq!(function_result, none_bits);
        dec_ref_bits(_py, function_result);
        dec_ref_bits(_py, bound_to_none);

        let unbound_property = unsafe {
            descriptor_bind(
                _py,
                property_bits,
                Some(MoltObject::from_ptr(owner_ptr).bits()),
                None,
            )
        }
        .unwrap();
        assert_eq!(unbound_property, property_bits);
        dec_ref_bits(_py, unbound_property);

        let property_result = unsafe {
            descriptor_bind(
                _py,
                property_bits,
                Some(MoltObject::from_ptr(owner_ptr).bits()),
                Some(none_bits),
            )
        }
        .unwrap();
        assert!(!exception_pending(_py));
        // Unlike ordinary function/custom descriptor binding, property itself
        // treats an explicit None receiver as class access (CPython tp_descr_get).
        assert_eq!(property_result, property_bits);
        dec_ref_bits(_py, property_result);

        dec_ref_bits(_py, property_bits);
        dec_ref_bits(_py, function_bits);
    });
}

#[test]
fn descriptor_bind_classmethod_uses_owner_and_staticmethod_remains_unbound() {
    let _transaction = crate::test_support::RuntimeTestTransaction::new();
    crate::with_gil_entry_nopanic!(_py, {
        let builtins = builtin_classes(_py);
        let owner_ptr = obj_from_bits(builtins.float)
            .as_ptr()
            .expect("float class pointer");
        let function_bits = runtime_function_bits(
            _py,
            "scalar_attr_identity_method_wrappers",
            scalar_attr_identity as *const (),
            1,
        );
        let classmethod_ptr = crate::alloc_classmethod_obj(_py, function_bits);
        let staticmethod_ptr = crate::alloc_staticmethod_obj(_py, function_bits);
        assert!(!classmethod_ptr.is_null());
        assert!(!staticmethod_ptr.is_null());
        let classmethod_bits = MoltObject::from_ptr(classmethod_ptr).bits();
        let staticmethod_bits = MoltObject::from_ptr(staticmethod_ptr).bits();
        let receiver_bits = MoltObject::from_float(-0.0).bits();

        let bound_classmethod = unsafe {
            descriptor_bind(
                _py,
                classmethod_bits,
                Some(MoltObject::from_ptr(owner_ptr).bits()),
                Some(receiver_bits),
            )
        }
        .unwrap();
        let classmethod_result = unsafe { call_callable0(_py, bound_classmethod) };
        assert!(!exception_pending(_py));
        assert_eq!(classmethod_result, builtins.float);
        dec_ref_bits(_py, classmethod_result);
        dec_ref_bits(_py, bound_classmethod);

        let unbound_staticmethod = unsafe {
            descriptor_bind(
                _py,
                staticmethod_bits,
                Some(MoltObject::from_ptr(owner_ptr).bits()),
                Some(receiver_bits),
            )
        }
        .unwrap();
        assert_eq!(unbound_staticmethod, function_bits);
        let staticmethod_result =
            unsafe { call_callable1(_py, unbound_staticmethod, receiver_bits) };
        assert!(!exception_pending(_py));
        assert_eq!(staticmethod_result, receiver_bits);
        dec_ref_bits(_py, staticmethod_result);
        dec_ref_bits(_py, unbound_staticmethod);

        dec_ref_bits(_py, staticmethod_bits);
        dec_ref_bits(_py, classmethod_bits);
        dec_ref_bits(_py, function_bits);
    });
}

#[test]
fn descriptor_bind_custom_get_preserves_arguments_across_owner_mutation() {
    let _transaction = crate::test_support::RuntimeTestTransaction::new();
    crate::with_gil_entry_nopanic!(_py, {
        CUSTOM_GET_SELF_BITS.store(0, Ordering::SeqCst);
        CUSTOM_GET_INSTANCE_BITS.store(0, Ordering::SeqCst);
        CUSTOM_GET_OWNER_BITS.store(0, Ordering::SeqCst);

        let get_bits = runtime_function_bits(
            _py,
            "scalar_attr_custom_get_reentrant",
            scalar_attr_custom_get_reentrant as *const (),
            3,
        );
        let descriptor_class_bits =
            test_class_bits(_py, b"ScalarAttrDescriptor", &[(b"__get__", get_bits)]);
        let descriptor_class_ptr = obj_from_bits(descriptor_class_bits)
            .as_ptr()
            .expect("descriptor class pointer");
        let descriptor_bits = unsafe { crate::alloc_instance_for_class(_py, descriptor_class_ptr) };
        assert!(!obj_from_bits(descriptor_bits).is_none());

        let owner_bits = test_class_bits(_py, b"ScalarAttrOwner", &[(b"probe", descriptor_bits)]);
        let owner_ptr = obj_from_bits(owner_bits)
            .as_ptr()
            .expect("owner class pointer");
        let instance_bits = MoltObject::from_float(-0.0).bits();

        // Leave the owner namespace as the descriptor's sole pre-bind owner.
        dec_ref_bits(_py, descriptor_bits);
        let result_bits = unsafe {
            descriptor_bind(
                _py,
                descriptor_bits,
                Some(MoltObject::from_ptr(owner_ptr).bits()),
                Some(instance_bits),
            )
        }
        .expect("custom descriptor must bind");
        assert!(!exception_pending(_py));
        assert_eq!(result_bits, descriptor_bits);
        assert_eq!(CUSTOM_GET_SELF_BITS.load(Ordering::SeqCst), descriptor_bits);
        assert_eq!(
            CUSTOM_GET_INSTANCE_BITS.load(Ordering::SeqCst),
            instance_bits
        );
        assert_eq!(CUSTOM_GET_OWNER_BITS.load(Ordering::SeqCst), owner_bits);

        dec_ref_bits(_py, result_bits);
        dec_ref_bits(_py, owner_bits);
        dec_ref_bits(_py, descriptor_class_bits);
        dec_ref_bits(_py, get_bits);
    });
}

#[test]
fn descriptor_bind_returns_plain_class_values_unchanged() {
    let _transaction = crate::test_support::RuntimeTestTransaction::new();
    crate::with_gil_entry_nopanic!(_py, {
        let builtins = builtin_classes(_py);
        let owner_ptr = obj_from_bits(builtins.int)
            .as_ptr()
            .expect("int class pointer");
        let receiver_bits = MoltObject::from_int(7).bits();
        let heap_string = string_bits(_py, b"plain class value");
        let values = [
            ("inline value", MoltObject::from_int(99).bits()),
            ("heap value", heap_string),
            ("class value", builtins.float),
        ];

        for (label, value_bits) in values {
            let result = unsafe {
                descriptor_bind(
                    _py,
                    value_bits,
                    Some(MoltObject::from_ptr(owner_ptr).bits()),
                    Some(receiver_bits),
                )
            }
            .unwrap_or_else(|| panic!("plain {label} must be returned"));
            assert!(!exception_pending(_py));
            assert_eq!(result, value_bits, "plain {label} was spuriously bound");
            dec_ref_bits(_py, result);
        }

        dec_ref_bits(_py, heap_string);
    });
}

#[test]
fn dir_raw_mro_lookup_preserves_class_held_function_descriptor() {
    let _transaction = crate::test_support::RuntimeTestTransaction::new();
    crate::with_gil_entry_nopanic!(_py, {
        let dir_function_bits = runtime_function_bits(
            _py,
            "descriptor_test_dir",
            descriptor_test_dir as *const (),
            1,
        );
        let class_bits = test_class_bits(
            _py,
            b"DescriptorDirOwner",
            &[(b"__dir__", dir_function_bits)],
        );
        let class_ptr = obj_from_bits(class_bits).as_ptr().expect("class pointer");
        let instance_bits = unsafe { crate::alloc_instance_for_class(_py, class_ptr) };
        assert!(!obj_from_bits(instance_bits).is_none());

        let function_refcount = heap_refcount(dir_function_bits);
        for invocation in 0..3 {
            let result_bits = crate::molt_dir_builtin(instance_bits);
            assert!(
                !exception_pending(_py),
                "dir invocation {invocation} raised unexpectedly"
            );
            let result_ptr = obj_from_bits(result_bits)
                .as_ptr()
                .expect("dir list pointer");
            assert_eq!(unsafe { object_type_id(result_ptr) }, TYPE_ID_LIST);
            dec_ref_bits(_py, result_bits);
            assert_eq!(
                heap_refcount(dir_function_bits),
                function_refcount,
                "dir invocation {invocation} consumed the class-held function descriptor"
            );
        }

        dec_ref_bits(_py, instance_bits);
        dec_ref_bits(_py, class_bits);
        dec_ref_bits(_py, dir_function_bits);
    });
}

#[test]
fn descriptor_bind_property_preserves_attribute_and_runtime_errors() {
    let _transaction = crate::test_support::RuntimeTestTransaction::new();
    crate::with_gil_entry_nopanic!(_py, {
        let builtins = builtin_classes(_py);
        let owner_ptr = obj_from_bits(builtins.int)
            .as_ptr()
            .expect("int class pointer");
        let receiver_bits = MoltObject::from_int(17).bits();
        let none_bits = MoltObject::none().bits();
        let attribute_error_getter = runtime_function_bits(
            _py,
            "descriptor_test_property_attribute_error",
            descriptor_test_property_attribute_error as *const (),
            1,
        );
        let runtime_error_getter = runtime_function_bits(
            _py,
            "descriptor_test_property_runtime_error",
            descriptor_test_property_runtime_error as *const (),
            1,
        );
        let attribute_error_property_ptr =
            crate::alloc_property_obj(_py, attribute_error_getter, none_bits, none_bits);
        let runtime_error_property_ptr =
            crate::alloc_property_obj(_py, runtime_error_getter, none_bits, none_bits);
        assert!(!attribute_error_property_ptr.is_null());
        assert!(!runtime_error_property_ptr.is_null());
        let attribute_error_property = MoltObject::from_ptr(attribute_error_property_ptr).bits();
        let runtime_error_property = MoltObject::from_ptr(runtime_error_property_ptr).bits();

        let failed_attribute_lookup = unsafe {
            descriptor_bind(
                _py,
                attribute_error_property,
                Some(MoltObject::from_ptr(owner_ptr).bits()),
                Some(receiver_bits),
            )
        };
        assert_eq!(failed_attribute_lookup, Some(none_bits));
        assert!(exception_pending(_py));
        let attribute_error = crate::builtins::exceptions::molt_exception_last_pending();
        assert!(crate::builtins::exceptions::exception_matches_builtin_name(
            _py,
            attribute_error,
            "AttributeError"
        ));
        crate::molt_exception_clear();
        dec_ref_bits(_py, attribute_error);

        // Binding preserves the same error for ordinary attributes and special
        // calls. Only the enclosing normal lookup may choose __getattr__.
        let failed_call = unsafe {
            descriptor_call1(
                _py,
                attribute_error_property,
                owner_ptr,
                Some(receiver_bits),
                none_bits,
            )
        };
        assert_eq!(failed_call, Some(none_bits));
        assert!(exception_pending(_py));
        let invocation_error = crate::builtins::exceptions::molt_exception_last_pending();
        assert!(crate::builtins::exceptions::exception_matches_builtin_name(
            _py,
            invocation_error,
            "AttributeError"
        ));
        crate::molt_exception_clear();
        dec_ref_bits(_py, invocation_error);

        let failed = unsafe {
            descriptor_bind(
                _py,
                runtime_error_property,
                Some(MoltObject::from_ptr(owner_ptr).bits()),
                Some(receiver_bits),
            )
        };
        assert_eq!(failed, Some(none_bits));
        assert!(exception_pending(_py));
        let error_bits = crate::builtins::exceptions::molt_exception_last_pending();
        assert!(crate::builtins::exceptions::exception_matches_builtin_name(
            _py,
            error_bits,
            "RuntimeError"
        ));
        crate::molt_exception_clear();
        dec_ref_bits(_py, error_bits);

        let call_failed = unsafe {
            descriptor_call1(
                _py,
                runtime_error_property,
                owner_ptr,
                Some(receiver_bits),
                MoltObject::from_int(23).bits(),
            )
        };
        assert_eq!(call_failed, Some(none_bits));
        assert!(exception_pending(_py));
        let call_error_bits = crate::builtins::exceptions::molt_exception_last_pending();
        assert!(crate::builtins::exceptions::exception_matches_builtin_name(
            _py,
            call_error_bits,
            "RuntimeError"
        ));
        crate::molt_exception_clear();
        dec_ref_bits(_py, call_error_bits);

        dec_ref_bits(_py, runtime_error_property);
        dec_ref_bits(_py, attribute_error_property);
        dec_ref_bits(_py, runtime_error_getter);
        dec_ref_bits(_py, attribute_error_getter);
    });
}

#[test]
fn descriptor_call1_exact_function_matches_bind_then_call_for_immediate_and_heap_receivers() {
    let _transaction = crate::test_support::RuntimeTestTransaction::new();
    crate::with_gil_entry_nopanic!(_py, {
        let builtins = builtin_classes(_py);
        let owner_ptr = obj_from_bits(builtins.int)
            .as_ptr()
            .expect("int class pointer");
        let function_bits = runtime_function_bits(
            _py,
            "descriptor_test_call1",
            descriptor_test_call1 as *const (),
            2,
        );
        let argument_bits = string_bits(_py, b"call1 argument");
        let heap_receiver =
            crate::builtins::numbers::int_bits_from_bigint(_py, BigInt::from(1u64) << 100usize);
        let receivers = [
            ("immediate", MoltObject::from_float(-0.0).bits()),
            ("heap", heap_receiver),
        ];
        let function_refcount = heap_refcount(function_bits);

        for (label, receiver_bits) in receivers {
            CALL1_RECEIVER_BITS.store(0, Ordering::SeqCst);
            CALL1_ARGUMENT_BITS.store(0, Ordering::SeqCst);
            let bound_bits = unsafe {
                descriptor_bind(
                    _py,
                    function_bits,
                    Some(MoltObject::from_ptr(owner_ptr).bits()),
                    Some(receiver_bits),
                )
            }
            .unwrap_or_else(|| panic!("function must bind for {label} receiver"));
            let bound_result = unsafe { call_callable1(_py, bound_bits, argument_bits) };
            dec_ref_bits(_py, bound_bits);
            assert!(!exception_pending(_py));
            assert_eq!(CALL1_RECEIVER_BITS.load(Ordering::SeqCst), receiver_bits);
            assert_eq!(CALL1_ARGUMENT_BITS.load(Ordering::SeqCst), argument_bits);

            CALL1_RECEIVER_BITS.store(0, Ordering::SeqCst);
            CALL1_ARGUMENT_BITS.store(0, Ordering::SeqCst);
            let direct_result = unsafe {
                descriptor_call1(
                    _py,
                    function_bits,
                    owner_ptr,
                    Some(receiver_bits),
                    argument_bits,
                )
            }
            .unwrap_or_else(|| panic!("direct descriptor call must succeed for {label}"));
            assert!(!exception_pending(_py));
            assert_eq!(CALL1_RECEIVER_BITS.load(Ordering::SeqCst), receiver_bits);
            assert_eq!(CALL1_ARGUMENT_BITS.load(Ordering::SeqCst), argument_bits);
            assert_eq!(direct_result, bound_result);
            assert_eq!(direct_result, argument_bits);
            dec_ref_bits(_py, direct_result);
            dec_ref_bits(_py, bound_result);
            assert_eq!(
                heap_refcount(function_bits),
                function_refcount,
                "{label} direct call did not balance its function pin"
            );
        }

        dec_ref_bits(_py, heap_receiver);
        dec_ref_bits(_py, argument_bits);
        dec_ref_bits(_py, function_bits);
    });
}

#[test]
fn descriptor_call1_pins_generic_get_method_across_self_replacement() {
    let _transaction = crate::test_support::RuntimeTestTransaction::new();
    crate::with_gil_entry_nopanic!(_py, {
        MUTATING_GET_SELF_BITS.store(0, Ordering::SeqCst);
        MUTATING_GET_OWNER_BITS.store(0, Ordering::SeqCst);
        MUTATING_GET_RETURN_CALLABLE_BITS.store(0, Ordering::SeqCst);

        let return_callable_bits = runtime_function_bits(
            _py,
            "descriptor_test_get_return_callable",
            scalar_attr_identity as *const (),
            1,
        );
        MUTATING_GET_RETURN_CALLABLE_BITS.store(return_callable_bits, Ordering::SeqCst);
        let get_bits = runtime_function_bits(
            _py,
            "descriptor_test_get_replaces_own_method",
            descriptor_test_get_replaces_own_method as *const (),
            3,
        );
        let descriptor_class_bits = test_class_bits(
            _py,
            b"SelfReplacingGetDescriptor",
            &[(b"__get__", get_bits)],
        );
        let descriptor_class_ptr = obj_from_bits(descriptor_class_bits)
            .as_ptr()
            .expect("descriptor class pointer");
        let descriptor_bits = unsafe { crate::alloc_instance_for_class(_py, descriptor_class_ptr) };
        assert!(!obj_from_bits(descriptor_bits).is_none());
        let owner_bits = test_class_bits(
            _py,
            b"SelfReplacingGetOwner",
            &[(b"probe", descriptor_bits)],
        );
        let owner_ptr = obj_from_bits(owner_bits)
            .as_ptr()
            .expect("owner class pointer");
        let receiver_bits = MoltObject::from_float(-0.0).bits();
        let argument_bits = string_bits(_py, b"generic descriptor argument");

        // The descriptor class and owner class are the sole owners of the raw
        // __get__ function and descriptor value consumed below.
        dec_ref_bits(_py, get_bits);
        dec_ref_bits(_py, descriptor_bits);
        let result_bits = unsafe {
            descriptor_call1(
                _py,
                descriptor_bits,
                owner_ptr,
                Some(receiver_bits),
                argument_bits,
            )
        }
        .expect("generic descriptor call must produce a result");
        assert!(!exception_pending(_py));
        assert_eq!(result_bits, argument_bits);
        assert_eq!(
            MUTATING_GET_SELF_BITS.load(Ordering::SeqCst),
            descriptor_bits
        );
        assert_eq!(MUTATING_GET_OWNER_BITS.load(Ordering::SeqCst), owner_bits);

        let get_name_bits = string_bits(_py, b"__get__");
        let replacement =
            unsafe { class_attr_lookup_raw_mro(_py, descriptor_class_ptr, get_name_bits) };
        assert_eq!(replacement, Some(MoltObject::none().bits()));
        dec_ref_bits(_py, get_name_bits);

        dec_ref_bits(_py, result_bits);
        dec_ref_bits(_py, argument_bits);
        dec_ref_bits(_py, owner_bits);
        dec_ref_bits(_py, descriptor_class_bits);
        dec_ref_bits(_py, return_callable_bits);
        MUTATING_GET_RETURN_CALLABLE_BITS.store(0, Ordering::SeqCst);
    });
}
