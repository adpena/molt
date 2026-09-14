use super::*;

static MODULE_FALLBACK_CALLS: AtomicU64 = AtomicU64::new(0);

extern "C" fn echo_module_fallback(name: u64) -> u64 {
    crate::with_gil_entry_nopanic!(py, {
        MODULE_FALLBACK_CALLS.fetch_add(1, Ordering::Relaxed);
        inc_ref_bits(py, name);
        name
    })
}

#[test]
fn descriptor_module_default_lookup_bypasses_module_getattr() {
    let _transaction = crate::test_support::RuntimeTestTransaction::new();
    crate::with_gil_entry_nopanic!(py, {
        MODULE_FALLBACK_CALLS.store(0, Ordering::Relaxed);
        let module_name = string_bits(py, b"descriptor_module_default");
        let ptr = crate::alloc_module_obj(py, module_name);
        assert!(!ptr.is_null());
        let module = MoltObject::from_ptr(ptr).bits();
        let getter_name = string_bits(py, b"__getattr__");
        let getter = runtime_function_bits(
            py,
            "echo_module_fallback",
            echo_module_fallback as *const (),
            1,
        );
        let dict = unsafe { module_dict_bits(ptr) };
        unsafe {
            dict_set_in_place(
                py,
                obj_from_bits(dict).as_ptr().unwrap(),
                getter_name,
                getter,
            )
        };
        let name = string_bits(py, b"absent");
        let normal = crate::molt_get_attr_name(module, name);
        assert!(!exception_pending(py));
        assert_eq!(normal, name);
        dec_ref_bits(py, normal);
        assert_eq!(MODULE_FALLBACK_CALLS.load(Ordering::Relaxed), 1);
        let raw = crate::molt_object_getattribute(module, name);
        dec_ref_bits(py, raw);
        assert!(exception_pending(py));
        assert_eq!(MODULE_FALLBACK_CALLS.load(Ordering::Relaxed), 1);
        clear_exception(py);
        let class_name = string_bits(py, b"__class__");
        let class = crate::molt_get_attr_name(module, class_name);
        assert!(!exception_pending(py));
        assert_eq!(class, builtin_classes(py).module);
        dec_ref_bits(py, class);
        dec_ref_bits(py, class_name);
        dec_ref_bits(py, name);
        dec_ref_bits(py, getter);
        dec_ref_bits(py, getter_name);
        dec_ref_bits(py, module);
        dec_ref_bits(py, module_name);
    });
}

extern "C" fn echo_descriptor_owner(_descriptor: u64, _instance: u64, owner: u64) -> u64 {
    crate::with_gil_entry_nopanic!(py, {
        inc_ref_bits(py, owner);
        owner
    })
}

#[test]
fn descriptor_invocation_keeps_tagged_owner_without_pointer_roundtrip() {
    let _transaction = crate::test_support::RuntimeTestTransaction::new();
    crate::with_gil_entry_nopanic!(py, {
        let getter = runtime_function_bits(
            py,
            "echo_descriptor_owner",
            echo_descriptor_owner as *const (),
            3,
        );
        let class = test_class_bits(py, b"OwnerEchoDescriptor", &[(b"__get__", getter)]);
        let descriptor =
            unsafe { crate::alloc_instance_for_class(py, obj_from_bits(class).as_ptr().unwrap()) };
        for owner in [
            MoltObject::from_int(73).bits(),
            MoltObject::from_float(-0.0).bits(),
            MoltObject::from_bool(false).bits(),
            MoltObject::none().bits(),
        ] {
            let result = unsafe {
                descriptor_bind(
                    py,
                    descriptor,
                    Some(owner),
                    Some(MoltObject::from_int(9).bits()),
                )
            }
            .expect("custom descriptor binding");
            assert!(!exception_pending(py));
            assert_eq!(result, owner);
            dec_ref_bits(py, result);
        }
        dec_ref_bits(py, descriptor);
        dec_ref_bits(py, class);
        dec_ref_bits(py, getter);
    });
}

#[test]
fn wrapper_descriptors_have_real_flavor_identity_and_shared_metadata() {
    let _transaction = crate::test_support::RuntimeTestTransaction::new();
    crate::with_gil_entry_nopanic!(py, {
        let classes = builtin_classes(py);
        let member_class = crate::builtins::types::member_descriptor_class(py);
        let getset_class = crate::builtins::types::getset_descriptor_class(py);
        assert_ne!(member_class, classes.property);
        assert_ne!(getset_class, classes.property);
        assert_ne!(member_class, getset_class);
        for (owner, name, expected_class) in [
            (classes.staticmethod, b"__func__".as_slice(), member_class),
            (classes.classmethod, b"__wrapped__".as_slice(), member_class),
            (classes.property, b"fget".as_slice(), member_class),
            (classes.property, b"__doc__".as_slice(), member_class),
            (classes.staticmethod, b"__dict__".as_slice(), getset_class),
            (
                classes.reference_type,
                b"__callback__".as_slice(),
                getset_class,
            ),
        ] {
            let name_bits = string_bits(py, name);
            let class_ptr = obj_from_bits(owner).as_ptr().unwrap();
            let descriptor = unsafe { class_attr_lookup_raw_mro(py, class_ptr, name_bits) }
                .expect("real descriptor in class namespace");
            inc_ref_bits(py, descriptor);
            assert_eq!(type_of_bits(py, descriptor), expected_class);
            for (attribute, expected) in [
                (b"__name__".as_slice(), name_bits),
                (b"__objclass__".as_slice(), owner),
            ] {
                let key = string_bits(py, attribute);
                let value = crate::molt_get_attr_name(descriptor, key);
                assert!(!exception_pending(py));
                assert!(obj_eq(py, obj_from_bits(value), obj_from_bits(expected)));
                dec_ref_bits(py, value);
                dec_ref_bits(py, key);
            }
            let class_access = unsafe {
                descriptor_bind(
                    py,
                    descriptor,
                    Some(MoltObject::from_ptr(class_ptr).bits()),
                    None,
                )
            }
            .unwrap();
            assert_eq!(class_access, descriptor);
            dec_ref_bits(py, class_access);
            dec_ref_bits(py, descriptor);
            dec_ref_bits(py, name_bits);
        }
    });
}

#[test]
fn property_member_binding_reads_storage_and_rejects_mutation() {
    let _transaction = crate::test_support::RuntimeTestTransaction::new();
    crate::with_gil_entry_nopanic!(py, {
        let getter = runtime_function_bits(
            py,
            "property_member_getter",
            scalar_attr_identity as *const (),
            1,
        );
        let none = MoltObject::none().bits();
        let property = crate::molt_property_new(getter, none, none);
        assert!(!exception_pending(py));
        let name = string_bits(py, b"fget");
        let owner = obj_from_bits(builtin_classes(py).property)
            .as_ptr()
            .unwrap();
        let descriptor = unsafe { class_attr_lookup_raw_mro(py, owner, name) }.unwrap();
        let result = crate::molt_get_attr_name(property, name);
        assert!(!exception_pending(py));
        assert_eq!(result, getter);
        dec_ref_bits(py, result);
        let result =
            unsafe { descriptor_mutate(py, descriptor, property, DescriptorMutation::Set(none)) };
        assert!(matches!(result, DescriptorMutationOutcome::Error));
        let error = crate::builtins::exceptions::molt_exception_last_pending();
        assert!(exception_matches_builtin_name(py, error, "AttributeError"));
        clear_exception(py);
        dec_ref_bits(py, error);
        let preserved = crate::molt_get_attr_name(property, name);
        assert_eq!(preserved, getter);
        dec_ref_bits(py, preserved);
        dec_ref_bits(py, name);
        dec_ref_bits(py, property);
        dec_ref_bits(py, getter);
    });
}
