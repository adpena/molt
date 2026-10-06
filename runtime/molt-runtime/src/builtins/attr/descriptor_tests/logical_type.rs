use super::*;

extern "C" fn logical_type_poll(_task: u64) -> i64 {
    MoltObject::none().bits() as i64
}

#[test]
fn logical_type_lookup_covers_task_generator_and_awaitable_consumers() {
    let _transaction = crate::test_support::RuntimeTestTransaction::new();
    crate::with_gil_entry_nopanic!(py, {
        let classes = builtin_classes(py);
        let poll = crate::provenance::abi::expose_function_address(logical_type_poll as *const ());
        let coroutine = crate::molt_task_new(poll, 0, crate::TASK_KIND_COROUTINE);
        let generator = crate::molt_task_new(
            poll,
            crate::GEN_CONTROL_SIZE as u64,
            crate::TASK_KIND_GENERATOR,
        );
        let asyncgen = crate::molt_asyncgen_new(generator);
        let wrapper = crate::molt_awaitable_await(coroutine);
        let future = crate::molt_future_new(poll, 0);
        assert!(!exception_pending(py));
        let name = string_bits(py, b"__class__");
        let dictionary = crate::molt_dict_new(0);
        let missing = MoltObject::from_int(-1).bits();
        for (receiver, expected) in [
            (coroutine, classes.coroutine),
            (generator, classes.generator),
            (asyncgen, classes.async_generator),
            (wrapper, classes.coroutine_wrapper),
            (future, classes.object),
        ] {
            assert_eq!(type_of_bits(py, receiver), expected);
            let reads = [
                crate::molt_get_attr_name(receiver, name),
                crate::molt_get_attr_name_default(receiver, name, missing),
                crate::molt_getattr_builtin(receiver, name, missing),
                crate::molt_object_getattribute(receiver, name),
                unsafe { crate::molt_get_attr_object(receiver, b"__class__".as_ptr(), 9) },
                unsafe { object_attr_lookup_with_dict(py, receiver, name, dictionary, false) }
                    .expect("generic dictionary lookup preserves the logical type"),
            ];
            for actual in reads {
                assert!(!exception_pending(py));
                assert_eq!(actual, expected);
                dec_ref_bits(py, actual);
            }
            assert_eq!(
                crate::molt_has_attr_name(receiver, name),
                MoltObject::from_bool(true).bits()
            );
            // The actual C generic entrypoint must observe the same descriptor.
            unsafe {
                use molt_cpython_abi::api::{object, refcount};
                use molt_cpython_abi::bridge::GLOBAL_BRIDGE;
                let view = GLOBAL_BRIDGE.handle_to_borrowed_pyobj(receiver);
                let key = GLOBAL_BRIDGE.handle_to_borrowed_pyobj(name);
                let result = object::PyObject_GenericGetAttr(view, key);
                assert!(!result.is_null());
                let actual = GLOBAL_BRIDGE.molt_value_for_pyobj(result).unwrap();
                assert_eq!(actual, expected);
                dec_ref_bits(py, actual);
                refcount::Py_DECREF(result);
            }
            assert!(!exception_pending(py));
        }
        let await_name = string_bits(py, b"__await__");
        for reader in [crate::molt_get_attr_name, crate::molt_object_getattribute] {
            let result = reader(generator, await_name);
            dec_ref_bits(py, result);
            assert!(crate::builtins::attr::clear_attribute_error_if_pending(py));
        }
        assert!(!crate::async_rt::generators::is_native_poll_future_bits(
            generator
        ));
        assert!(crate::async_rt::generators::is_native_poll_future_bits(
            future
        ));
        for reader in [crate::molt_get_attr_name, crate::molt_object_getattribute] {
            let method = reader(future, await_name);
            assert!(!exception_pending(py));
            let result = unsafe { call_callable0(py, method) };
            assert_eq!(
                result, future,
                "class resolution must retain native future polling"
            );
            assert!(!exception_pending(py));
            dec_ref_bits(py, result);
            dec_ref_bits(py, method);
        }
        unsafe {
            dict_set_in_place(
                py,
                obj_from_bits(dictionary).as_ptr().unwrap(),
                await_name,
                missing,
            );
            let result = object_attr_lookup_with_dict(py, future, await_name, dictionary, false);
            assert_eq!(
                result,
                Some(missing),
                "the explicit dictionary precedes the poll adapter"
            );
        }
        let closed = crate::async_rt::awaitable::molt_coroutine_close_method(coroutine);
        dec_ref_bits(py, closed);
        assert!(!exception_pending(py));
        for bits in [
            await_name, name, dictionary, wrapper, coroutine, asyncgen, generator, future,
        ] {
            dec_ref_bits(py, bits);
        }
    });
}

static CLASS_DESCRIPTOR_CALLS: AtomicU64 = AtomicU64::new(0);

extern "C" fn apparent_class(_descriptor: u64, _receiver: u64, _owner: u64) -> u64 {
    crate::with_gil_entry_nopanic!(py, {
        CLASS_DESCRIPTOR_CALLS.fetch_add(1, Ordering::Relaxed);
        let class = builtin_classes(py).str;
        inc_ref_bits(py, class);
        class
    })
}

#[test]
fn logical_type_lookup_preserves_nondata_class_shadow_and_real_descriptor_identity() {
    let _transaction = crate::test_support::RuntimeTestTransaction::new();
    crate::with_gil_entry_nopanic!(py, {
        CLASS_DESCRIPTOR_CALLS.store(0, Ordering::Relaxed);
        let get = runtime_function_bits(py, "apparent_class", apparent_class as *const (), 3);
        let descriptor_class =
            test_class_bits(py, b"ApparentClassDescriptor", &[], &[(b"__get__", get)]);
        let apparent_descriptor = unsafe { call_callable0(py, descriptor_class) };
        let owner = test_class_bits(
            py,
            b"ActualClassOwner",
            &[],
            &[(b"__class__", apparent_descriptor)],
        );
        let receiver = unsafe { call_callable0(py, owner) };
        let name = string_bits(py, b"__class__");
        let dict_name = string_bits(py, b"__dict__");
        let dictionary = crate::molt_get_attr_name(receiver, dict_name);
        let expected = builtin_classes(py).int;
        unsafe {
            dict_set_in_place(
                py,
                obj_from_bits(dictionary).as_ptr().unwrap(),
                name,
                expected,
            )
        };
        for reader in [crate::molt_get_attr_name, crate::molt_object_getattribute] {
            let result = reader(receiver, name);
            assert!(!exception_pending(py));
            assert_eq!(result, expected);
            dec_ref_bits(py, result);
        }
        assert_eq!(CLASS_DESCRIPTOR_CALLS.load(Ordering::Relaxed), 0);
        assert_eq!(type_of_bits(py, receiver), owner);
        let root = obj_from_bits(builtin_classes(py).object).as_ptr().unwrap();
        let descriptor = unsafe { class_attr_lookup_raw_mro(py, root, name) }.unwrap();
        let result =
            unsafe { descriptor_bind(py, descriptor, Some(owner), Some(receiver)) }.unwrap();
        assert_eq!(
            result, owner,
            "the inherited data descriptor reads actual type identity"
        );
        dec_ref_bits(py, result);
        for bits in [
            dictionary,
            dict_name,
            name,
            receiver,
            owner,
            apparent_descriptor,
            descriptor_class,
            get,
        ] {
            dec_ref_bits(py, bits);
        }
    });
}

extern "C" fn failing_apparent_class(_receiver: u64) -> u64 {
    crate::with_gil_entry_nopanic!(py, {
        CLASS_DESCRIPTOR_CALLS.fetch_add(1, Ordering::Relaxed);
        raise_exception::<u64>(py, "AttributeError", "class descriptor denied")
    })
}

#[test]
fn logical_type_lookup_keeps_descriptor_failure_and_optional_default() {
    let _transaction = crate::test_support::RuntimeTestTransaction::new();
    crate::with_gil_entry_nopanic!(py, {
        let get = runtime_function_bits(
            py,
            "failing_apparent_class",
            failing_apparent_class as *const (),
            1,
        );
        let descriptor =
            crate::molt_property_new(get, MoltObject::none().bits(), MoltObject::none().bits());
        let owner = test_class_bits(py, b"FailingClassOwner", &[], &[(b"__class__", descriptor)]);
        let receiver = unsafe { call_callable0(py, owner) };
        let name = string_bits(py, b"__class__");
        let missing = MoltObject::from_int(73).bits();
        for reader in [crate::molt_get_attr_name, crate::molt_object_getattribute] {
            CLASS_DESCRIPTOR_CALLS.store(0, Ordering::Relaxed);
            let result = reader(receiver, name);
            dec_ref_bits(py, result);
            assert_eq!(CLASS_DESCRIPTOR_CALLS.load(Ordering::Relaxed), 1);
            let exception = crate::builtins::exceptions::molt_exception_last_pending();
            assert!(crate::builtins::exceptions::exception_matches_builtin_name(
                py,
                exception,
                "AttributeError"
            ));
            clear_exception(py);
            dec_ref_bits(py, exception);
            assert_eq!(type_of_bits(py, receiver), owner);
        }
        CLASS_DESCRIPTOR_CALLS.store(0, Ordering::Relaxed);
        assert_eq!(
            crate::molt_get_attr_name_default(receiver, name, missing),
            missing
        );
        assert_eq!(CLASS_DESCRIPTOR_CALLS.load(Ordering::Relaxed), 1);
        assert!(!exception_pending(py));
        for bits in [name, receiver, owner, descriptor, get] {
            dec_ref_bits(py, bits);
        }
    });
}

extern "C" fn super_precedence_probe(_receiver: u64) -> u64 {
    MoltObject::from_int(17).bits()
}

#[test]
fn logical_type_super_delegation_and_explicit_generic_are_distinct() {
    let _transaction = crate::test_support::RuntimeTestTransaction::new();
    crate::with_gil_entry_nopanic!(py, {
        unsafe {
            let probe = runtime_function_bits(
                py,
                "super_precedence_probe",
                super_precedence_probe as *const (),
                1,
            );
            let base = test_class_bits(
                py,
                b"SuperBase",
                &[],
                &[
                    (b"__repr__", probe),
                    (b"__self__", MoltObject::from_int(41).bits()),
                ],
            );
            let child = test_class_bits(py, b"SuperChild", &[base], &[]);
            let receiver = call_callable0(py, child);
            assert!(!exception_pending(py), "ordinary child construction");
            let proxy = crate::molt_super_new(child, receiver);
            assert!(!exception_pending(py), "instance-mode super construction");
            let proxy_ptr = obj_from_bits(proxy).as_ptr().unwrap();
            let method = crate::molt_get_attr_generic(proxy_ptr, b"__repr__".as_ptr(), 8);
            let result = call_callable0(py, method);
            assert_eq!(result, MoltObject::from_int(17).bits());
            dec_ref_bits(py, method);
            dec_ref_bits(py, result);
            let name = string_bits(py, b"__self__");
            assert_eq!(
                crate::molt_get_attr_name(proxy, name),
                MoltObject::from_int(41).bits()
            );
            let own = crate::molt_object_getattribute(proxy, name);
            assert_eq!(own, receiver);
            dec_ref_bits(py, own);
            let dictionary = crate::molt_dict_new(0);
            dict_set_in_place(
                py,
                obj_from_bits(dictionary).as_ptr().unwrap(),
                name,
                MoltObject::from_int(99).bits(),
            );
            let own = object_attr_lookup_with_dict(py, proxy, name, dictionary, false).unwrap();
            assert_eq!(
                own, receiver,
                "readonly native metadata is a data descriptor"
            );
            dec_ref_bits(py, own);
            use molt_cpython_abi::api::{object, refcount};
            use molt_cpython_abi::bridge::GLOBAL_BRIDGE;
            let view = GLOBAL_BRIDGE.handle_to_borrowed_pyobj(proxy);
            let key = GLOBAL_BRIDGE.handle_to_borrowed_pyobj(name);
            let own = object::PyObject_GenericGetAttr(view, key);
            assert!(!own.is_null());
            let own_bits = GLOBAL_BRIDGE.molt_value_for_pyobj(own).unwrap();
            assert_eq!(own_bits, receiver);
            dec_ref_bits(py, own_bits);
            refcount::Py_DECREF(own);
            let class_name = string_bits(py, b"__class__");
            let class = crate::molt_get_attr_name(proxy, class_name);
            assert_eq!(class, builtin_classes(py).super_type);
            dec_ref_bits(py, class);
            let new_name = string_bits(py, b"__new__");
            let body_new_owner = test_class_bits(py, b"BodyNewOwner", &[], &[(b"__new__", probe)]);
            let body_new = class_namespace_lookup_raw(
                py,
                obj_from_bits(body_new_owner).as_ptr().unwrap(),
                new_name,
            )
            .unwrap();
            assert_eq!(
                object_type_id(obj_from_bits(body_new).as_ptr().unwrap()),
                TYPE_ID_STATICMETHOD
            );
            let unbound = crate::molt_get_attr_name(body_new_owner, new_name);
            assert!(
                !exception_pending(py),
                "class-body constructor descriptor lookup"
            );
            assert_eq!(unbound, probe);
            dec_ref_bits(py, unbound);
            dec_ref_bits(py, body_new_owner);
            // Later assignment remains a normal function descriptor. Super
            // must use descriptor identity, not a spelling-based __new__ rule.
            let assigned = crate::molt_set_attr_name(base, new_name, probe);
            dec_ref_bits(py, assigned);
            assert!(!exception_pending(py), "late constructor assignment");
            let selected = super_resolve_method_unbound(py, child, receiver, new_name).unwrap();
            assert_eq!(selected.func_bits, probe);
            dec_ref_bits(py, selected.func_bits);
            let bound_new = crate::molt_get_attr_name(proxy, new_name);
            assert_eq!(
                crate::bound_method_func_bits(obj_from_bits(bound_new).as_ptr().unwrap()),
                probe
            );
            assert_eq!(
                crate::bound_method_self_bits(obj_from_bits(bound_new).as_ptr().unwrap()),
                receiver
            );
            dec_ref_bits(py, bound_new);
            dec_ref_bits(py, new_name);
            // A class receiver must select type.__new__, never object.__new__.
            let meta = test_class_bits(py, b"SuperMeta", &[builtin_classes(py).type_obj], &[]);
            let meta_proxy = crate::molt_super_new(meta, meta);
            assert!(!exception_pending(py), "class-mode super construction");
            let new = crate::molt_get_attr_generic(
                obj_from_bits(meta_proxy).as_ptr().unwrap(),
                b"__new__".as_ptr(),
                7,
            );
            assert_eq!(
                new,
                crate::builtins::methods::type_method_bits(py, "__new__").unwrap()
            );
            assert!(!exception_pending(py));
            for bits in [
                new, meta_proxy, meta, class_name, dictionary, name, proxy, receiver, child, base,
                probe,
            ] {
                dec_ref_bits(py, bits);
            }
        }
    });
}

#[test]
fn logical_type_class_generic_read_uses_only_heap_own_dictionary() {
    let _transaction = crate::test_support::RuntimeTestTransaction::new();
    crate::with_gil_entry_nopanic!(py, {
        let classes = builtin_classes(py);
        let parent = test_class_bits(
            py,
            b"GenericParent",
            &[],
            &[(b"inherited", MoltObject::from_int(17).bits())],
        );
        let child = test_class_bits(
            py,
            b"GenericChild",
            &[parent],
            &[(b"own", MoltObject::from_int(23).bits())],
        );
        let own = string_bits(py, b"own");
        let inherited = string_bits(py, b"inherited");
        let bit_length = string_bits(py, b"bit_length");
        let dictionary = string_bits(py, b"__dict__");
        let name = string_bits(py, b"__name__");
        for _phase in 0..2 {
            for (owner, key, expected) in [(parent, inherited, 17), (child, own, 23)] {
                let result = crate::molt_object_getattribute(owner, key);
                assert_eq!(result, MoltObject::from_int(expected).bits());
                assert!(!exception_pending(py));
                dec_ref_bits(py, result);
            }
            for (owner, key) in [
                (child, inherited),
                (classes.int, bit_length),
                (classes.bool, bit_length),
            ] {
                let result = crate::molt_object_getattribute(owner, key);
                dec_ref_bits(py, result);
                assert!(crate::builtins::attr::clear_attribute_error_if_pending(py));
            }
            for owner in [parent, child, classes.int, classes.bool] {
                for key in [dictionary, name] {
                    let result = crate::molt_object_getattribute(owner, key);
                    assert!(!exception_pending(py));
                    assert!(obj_from_bits(result).as_ptr().is_some());
                    dec_ref_bits(py, result);
                }
            }
            // Ordinary type lookup may publish native members or visit bases;
            // neither operation changes generic object-dictionary admission.
            for (owner, key) in [
                (child, inherited),
                (classes.int, bit_length),
                (classes.bool, bit_length),
            ] {
                let result = crate::molt_get_attr_name(owner, key);
                assert!(!exception_pending(py));
                dec_ref_bits(py, result);
            }
        }
        for bits in [name, dictionary, bit_length, inherited, own, child, parent] {
            dec_ref_bits(py, bits);
        }
    });
}

#[test]
fn logical_type_coroutine_members_share_normal_explicit_and_c_generic_lookup() {
    let _transaction = crate::test_support::RuntimeTestTransaction::new();
    crate::with_gil_entry_nopanic!(py, {
        unsafe {
            let coroutine = crate::molt_task_new(0, 0, crate::TASK_KIND_COROUTINE);
            let dictionary = crate::molt_dict_new(0);
            for (spelling, expected) in [
                (
                    b"cr_running".as_slice(),
                    MoltObject::from_bool(false).bits(),
                ),
                (b"cr_frame".as_slice(), MoltObject::none().bits()),
                (b"cr_code".as_slice(), MoltObject::none().bits()),
                (b"cr_await".as_slice(), MoltObject::none().bits()),
            ] {
                let name = string_bits(py, spelling);
                dict_set_in_place(
                    py,
                    obj_from_bits(dictionary).as_ptr().unwrap(),
                    name,
                    MoltObject::from_int(99).bits(),
                );
                for reader in [crate::molt_get_attr_name, crate::molt_object_getattribute] {
                    let result = reader(coroutine, name);
                    assert_eq!(result, expected);
                    assert!(!exception_pending(py));
                    dec_ref_bits(py, result);
                }
                let result =
                    object_attr_lookup_with_dict(py, coroutine, name, dictionary, false).unwrap();
                assert_eq!(result, expected);
                dec_ref_bits(py, result);
                use molt_cpython_abi::api::{object, refcount};
                use molt_cpython_abi::bridge::GLOBAL_BRIDGE;
                let result = object::PyObject_GenericGetAttr(
                    GLOBAL_BRIDGE.handle_to_borrowed_pyobj(coroutine),
                    GLOBAL_BRIDGE.handle_to_borrowed_pyobj(name),
                );
                assert!(!result.is_null());
                let bits = GLOBAL_BRIDGE.molt_value_for_pyobj(result).unwrap();
                assert_eq!(bits, expected);
                dec_ref_bits(py, bits);
                refcount::Py_DECREF(result);
                dec_ref_bits(py, name);
            }
            dec_ref_bits(py, dictionary);
            dec_ref_bits(py, coroutine);
        }
    });
}

#[test]
fn logical_type_super_cache_admits_only_plain_python_methods() {
    let _transaction = crate::test_support::RuntimeTestTransaction::new();
    crate::with_gil_entry_nopanic!(py, {
        unsafe {
            use crate::builtins::functions::native_callable::NativeCallableKind;
            let init_name = string_bits(py, b"__init__");
            // OSError owns a native layout-root wrapper but is not a builtin
            // anchor. Eligibility must agree with dict's native wrapper.
            for (base, label) in [
                (builtin_classes(py).dict, b"SuperDictChild".as_slice()),
                (
                    crate::builtins::exceptions::exception_type_bits_from_name(py, "OSError"),
                    b"SuperOSErrorChild".as_slice(),
                ),
            ] {
                let child = test_class_bits(py, label, &[base], &[]);
                let receiver = call_callable0(py, child);
                assert!(!exception_pending(py), "native subtype construction");
                let (_, selected) = super_attribute_owned(py, child, child, init_name).unwrap();
                assert_eq!(
                    NativeCallableKind::from_class(
                        py,
                        object_class_bits(obj_from_bits(selected).as_ptr().unwrap())
                    ),
                    Some(NativeCallableKind::WrapperDescriptor)
                );
                dec_ref_bits(py, selected);
                assert!(super_resolve_method_unbound(py, child, receiver, init_name).is_none());
                assert!(!exception_pending(py));
                let proxy = crate::molt_super_new(child, receiver);
                let bound = crate::molt_get_attr_name(proxy, init_name);
                assert!(!exception_pending(py), "ordinary native descriptor binding");
                let result = call_callable0(py, bound);
                assert_eq!(result, MoltObject::none().bits());
                assert!(!exception_pending(py), "ordinary native initializer call");
                for bits in [result, bound, proxy, receiver, child] {
                    dec_ref_bits(py, bits);
                }
            }
            let probe = runtime_function_bits(
                py,
                "super_precedence_probe",
                super_precedence_probe as *const (),
                1,
            );
            let property = crate::molt_property_new(
                probe,
                MoltObject::none().bits(),
                MoltObject::none().bits(),
            );
            let base = test_class_bits(
                py,
                b"PythonSuperOwner",
                &[],
                &[(b"method", probe), (b"field", property)],
            );
            let child = test_class_bits(py, b"PythonSuperChild", &[base], &[]);
            let receiver = call_callable0(py, child);
            let method_name = string_bits(py, b"method");
            let field_name = string_bits(py, b"field");
            let selected = super_resolve_method_unbound(py, child, receiver, method_name).unwrap();
            assert_eq!(selected.func_bits, probe);
            dec_ref_bits(py, selected.func_bits);
            assert!(super_resolve_method_unbound(py, child, receiver, field_name).is_none());
            let proxy = crate::molt_super_new(child, receiver);
            let bound = crate::molt_get_attr_name(proxy, method_name);
            let result = call_callable0(py, bound);
            assert_eq!(result, MoltObject::from_int(17).bits());
            assert_eq!(
                crate::molt_get_attr_name(proxy, field_name),
                MoltObject::from_int(17).bits()
            );
            assert!(!exception_pending(py));
            for bits in [
                result,
                bound,
                proxy,
                field_name,
                method_name,
                receiver,
                child,
                base,
                property,
                probe,
                init_name,
            ] {
                dec_ref_bits(py, bits);
            }
        }
    });
}

#[test]
fn logical_coroutine_wrapper_keeps_capture_owner_out_of_instance_dictionary() {
    let _transaction = crate::test_support::RuntimeTestTransaction::new();
    crate::with_gil_entry_nopanic!(py, {
        unsafe {
            let poll =
                crate::provenance::abi::expose_function_address(logical_type_poll as *const ());
            let coroutine = crate::molt_task_new(poll, 0, crate::TASK_KIND_COROUTINE);
            let coroutine_ptr = obj_from_bits(coroutine).as_ptr().unwrap();
            let baseline = (*header_from_obj_ptr(coroutine_ptr)).ref_count_snapshot();
            let wrapper = crate::molt_awaitable_await(coroutine);
            let ptr = obj_from_bits(wrapper).as_ptr().unwrap();
            assert_eq!(crate::object::object_payload_size(ptr), 8);
            assert_eq!(*ptr.cast::<u64>(), coroutine);
            assert_eq!(
                type_of_bits(py, wrapper),
                builtin_classes(py).coroutine_wrapper
            );
            assert!(crate::object::instance_dict_bits_ptr(ptr).is_null());
            assert!(matches!(
                crate::object::field_storage::current_dictionary(py, ptr),
                Ok(None)
            ));

            // The original guest failed on this non-data descriptor lookup:
            // resolving __class__ alone can return before reading a dictionary.
            let name = string_bits(py, b"__repr__");
            for reader in [crate::molt_get_attr_name, crate::molt_object_getattribute] {
                let repr = reader(wrapper, name);
                assert!(!exception_pending(py));
                assert!(crate::builtins::callable::is_callable_impl(py, repr));
                dec_ref_bits(py, repr);
            }
            use molt_cpython_abi::api::{object, refcount};
            use molt_cpython_abi::bridge::GLOBAL_BRIDGE;
            let view = GLOBAL_BRIDGE.handle_to_borrowed_pyobj(wrapper);
            let key = GLOBAL_BRIDGE.handle_to_borrowed_pyobj(name);
            let repr = object::PyObject_GenericGetAttr(view, key);
            assert!(!repr.is_null());
            refcount::Py_DECREF(repr);
            dec_ref_bits(py, name);

            let dictionary = crate::molt_dict_new(0);
            crate::object::field_storage::replace_dictionary(py, ptr, Some(dictionary));
            assert!(crate::builtins::attr::clear_attribute_error_if_pending(py));
            dec_ref_bits(py, dictionary);
            crate::object::field_storage::reset(py, ptr);
            assert_eq!(*ptr.cast::<u64>(), coroutine);
            let mut owners = 0;
            crate::object::heap_lifecycle::visit_owned_edges(py, ptr, &mut |child| {
                owners += usize::from(child == coroutine_ptr);
            });
            assert_eq!(owners, 1);
            assert_eq!(
                (*header_from_obj_ptr(coroutine_ptr)).ref_count_snapshot(),
                baseline + 1
            );
            assert_eq!(
                crate::object::heap_lifecycle::try_clear_cycle_edges(py, ptr),
                0
            );
            assert_eq!(*ptr.cast::<u64>(), MoltObject::none().bits());
            assert_eq!(
                (*header_from_obj_ptr(coroutine_ptr)).ref_count_snapshot(),
                baseline
            );
            dec_ref_bits(py, wrapper);
            assert_eq!(
                (*header_from_obj_ptr(coroutine_ptr)).ref_count_snapshot(),
                baseline
            );
            dec_ref_bits(py, coroutine);
            assert!(!exception_pending(py));
        }
    });
}
