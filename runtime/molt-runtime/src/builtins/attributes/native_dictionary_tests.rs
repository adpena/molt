use super::*;

#[test]
fn c_subscription_uses_live_subclass_protocol_for_get_set_delete_and_zero() {
    use molt_cpython_abi::api::{errors, object, refcount};
    use molt_cpython_abi::bridge::GLOBAL_BRIDGE;
    use std::sync::atomic::{AtomicU64, Ordering};

    static WRITTEN: AtomicU64 = AtomicU64::new(u64::MAX);
    static DELETED: AtomicU64 = AtomicU64::new(u64::MAX);
    extern "C" fn get(_self: u64, key: u64) -> u64 {
        crate::with_gil_entry_nopanic!(py, {
            if obj_from_bits(key).as_int() == Some(-1) {
                return raise_exception::<u64>(py, "ValueError", "subscription body failure");
            }
            inc_ref_bits(py, key);
            key
        })
    }
    extern "C" fn set(_self: u64, _key: u64, value: u64) -> u64 {
        WRITTEN.store(value, Ordering::Relaxed);
        MoltObject::none().bits()
    }
    extern "C" fn delete(_self: u64, key: u64) -> u64 {
        DELETED.store(key, Ordering::Relaxed);
        MoltObject::none().bits()
    }

    let _transaction = crate::test_support::RuntimeTestTransaction::new();
    assert!(crate::cpython_abi_hooks::register_cpython_hooks());
    crate::with_gil_entry_nopanic!(py, {
        unsafe {
            WRITTEN.store(u64::MAX, Ordering::Relaxed);
            DELETED.store(u64::MAX, Ordering::Relaxed);
            let name = attr_name_bits_from_bytes(py, b"SubscriptionTuple").unwrap();
            let class = molt_class_new(name);
            molt_class_set_base(class, builtin_classes(py).tuple);
            let class_ptr = obj_from_bits(class).as_ptr().unwrap();
            let namespace = obj_from_bits(class_dict_bits(class_ptr)).as_ptr().unwrap();
            for (name, target, arity) in [
                (b"__getitem__".as_slice(), fn_addr!(get), 2),
                (b"__setitem__".as_slice(), fn_addr!(set), 3),
                (b"__delitem__".as_slice(), fn_addr!(delete), 2),
            ] {
                let name = attr_name_bits_from_bytes(py, name).unwrap();
                let function = alloc_function_obj(py, target, arity);
                let function = MoltObject::from_ptr(function).bits();
                dict_set_in_place(py, namespace, name, function);
                dec_ref_bits(py, function);
                dec_ref_bits(py, name);
            }
            class_bump_layout_version(class_ptr);
            let instance = crate::object::builders::alloc_tuple_subclass(
                py,
                class,
                &[MoltObject::from_int(99).bits()],
            );
            let object = GLOBAL_BRIDGE.borrowed_handle_to_new_pyobj(instance);
            let zero = GLOBAL_BRIDGE.borrowed_handle_to_new_pyobj(0);
            let key = GLOBAL_BRIDGE.borrowed_handle_to_new_pyobj(MoltObject::from_int(7).bits());
            assert!(!object.is_null() && !zero.is_null() && !key.is_null());
            let receiver = obj_from_bits(instance).as_ptr().unwrap();
            let references = (*crate::object::header_from_obj_ptr(receiver)).ref_count_snapshot();
            // An overridden tuple protocol accepts arbitrary keys. Physical
            // indexing would reject float zero or return the stored item 99.
            let observed = object::PyObject_GetItem(object, zero);
            assert_eq!(observed, zero);
            refcount::Py_DECREF(observed);
            assert_eq!(object::PyObject_SetItem(object, key, zero), 0);
            assert_eq!(WRITTEN.load(Ordering::Relaxed), 0);
            assert_eq!(DELETED.load(Ordering::Relaxed), u64::MAX);
            assert_eq!(object::PyObject_DelItem(object, key), 0);
            assert_eq!(
                DELETED.load(Ordering::Relaxed),
                MoltObject::from_int(7).bits()
            );
            assert_eq!(
                (*crate::object::header_from_obj_ptr(receiver)).ref_count_snapshot(),
                references
            );
            for delete in [false, true] {
                let status = if delete {
                    object::PyObject_DelItem(key, zero)
                } else {
                    object::PyObject_SetItem(key, zero, zero)
                };
                assert_eq!(status, -1);
                assert_ne!(
                    errors::PyErr_ExceptionMatches(
                        (&raw mut molt_cpython_abi::abi_types::PyExc_TypeError).cast()
                    ),
                    0
                );
                errors::PyErr_Clear();
            }
            let failing =
                GLOBAL_BRIDGE.borrowed_handle_to_new_pyobj(MoltObject::from_int(-1).bits());
            assert!(object::PyObject_GetItem(object, failing).is_null());
            assert_ne!(
                errors::PyErr_ExceptionMatches(
                    (&raw mut molt_cpython_abi::abi_types::PyExc_ValueError).cast()
                ),
                0
            );
            errors::PyErr_Clear();
            for value in [failing, key, zero, object] {
                refcount::Py_DECREF(value);
            }
            for value in [instance, class, name] {
                dec_ref_bits(py, value);
            }
            assert!(!exception_pending(py));
            assert!(errors::PyErr_Occurred().is_null());
        }
    });
}

fn assert_attribute_error(py: &PyToken<'_>) {
    let exception = crate::builtins::exceptions::molt_exception_last_pending();
    assert!(exception_is_attribute_error(py, exception));
    crate::molt_exception_clear();
    dec_ref_bits(py, exception);
}

#[test]
fn native_function_public_dictionary_policy_preserves_private_metadata() {
    let _transaction = crate::test_support::RuntimeTestTransaction::new();
    crate::with_gil_entry_nopanic!(py, {
        unsafe {
            let builtin =
                crate::builtins::methods::alloc_builtin_function(py, fn_addr!(molt_len), 1);
            let native = obj_from_bits(builtin).as_ptr().unwrap();
            let managed = alloc_function_obj(py, fn_addr!(molt_len), 1);
            assert!(!managed.is_null());
            let managed_bits = MoltObject::from_ptr(managed).bits();
            let name = attr_name_bits_from_bytes(py, b"extra").unwrap();
            let dictionary = attr_name_bits_from_bytes(py, b"__dict__").unwrap();
            let module = attr_name_bits_from_bytes(py, b"__module__").unwrap();
            let value = MoltObject::from_int(37).bits();
            assert!(crate::call::class_init::function_set_attr_bits(
                py, native, name, value
            ));
            let private_dictionary = function_dict_bits(native);
            assert_ne!(private_dictionary, 0);

            assert!(attr_lookup_ptr(py, native, dictionary).is_none());
            assert!(attr_lookup_ptr(py, native, name).is_none());
            assert!(!exception_pending(py));
            molt_set_attr_name(builtin, name, MoltObject::from_int(99).bits());
            assert_attribute_error(py);
            molt_del_attr_name(builtin, name);
            assert_attribute_error(py);
            assert_eq!(function_dict_bits(native), private_dictionary);
            assert_eq!(
                dict_get_in_place(
                    py,
                    obj_from_bits(private_dictionary).as_ptr().unwrap(),
                    name
                ),
                Some(value)
            );
            molt_set_attr_name(builtin, module, value);
            assert!(!exception_pending(py));
            let observed = molt_get_attr_name(builtin, module);
            assert_eq!(observed, value);
            dec_ref_bits(py, observed);
            molt_del_attr_name(builtin, module);
            assert!(!exception_pending(py));
            assert_eq!(
                molt_get_attr_name(builtin, module),
                MoltObject::none().bits()
            );

            molt_set_attr_name(managed_bits, name, value);
            assert!(!exception_pending(py));
            let public_dictionary = molt_get_attr_name(managed_bits, dictionary);
            assert_eq!(
                dict_get_in_place(py, obj_from_bits(public_dictionary).as_ptr().unwrap(), name),
                Some(value)
            );
            molt_del_attr_name(managed_bits, name);
            assert!(!exception_pending(py));
            assert_eq!(
                dict_get_in_place(py, obj_from_bits(public_dictionary).as_ptr().unwrap(), name),
                None
            );

            for bits in [
                public_dictionary,
                managed_bits,
                builtin,
                name,
                dictionary,
                module,
            ] {
                dec_ref_bits(py, bits);
            }
            assert!(!exception_pending(py));
        }
    });
}

#[test]
fn native_tuple_and_io_dictionaries_have_one_traversed_cleared_owner() {
    let _transaction = crate::test_support::RuntimeTestTransaction::with_gc_isolation();
    crate::with_gil_entry_nopanic!(py, {
        unsafe {
            let classes = builtin_classes(py);
            let name = attr_name_bits_from_bytes(py, b"NativeDictionaryTuple").unwrap();
            let class = molt_class_new(name);
            molt_class_set_base(class, classes.tuple);
            assert!(!exception_pending(py));
            let item = alloc_list(py, &[]);
            let item_bits = MoltObject::from_ptr(item).bits();
            let exact = alloc_tuple(py, &[item_bits]);
            assert!(crate::object::instance_dict_bits_ptr(exact).is_null());
            let tuple = crate::object::builders::alloc_tuple_subclass(py, class, &[item_bits]);
            let tuple_ptr = obj_from_bits(tuple).as_ptr().unwrap();
            assert!(!crate::object::instance_dict_bits_ptr(tuple_ptr).is_null());
            let empty_bytes = alloc_bytes(py, &[]);
            let empty_bits = MoltObject::from_ptr(empty_bytes).bits();
            let io = crate::builtins::io::molt_bytesio_new(classes.bytes_io, empty_bits);
            dec_ref_bits(py, empty_bits);
            assert!(obj_from_bits(io).as_ptr().is_some());
            let key = attr_name_bits_from_bytes(py, b"extra").unwrap();
            let dict_key = attr_name_bits_from_bytes(py, b"__dict__").unwrap();
            let closed_key = attr_name_bits_from_bytes(py, b"closed").unwrap();
            for owner in [tuple, io] {
                let pointer = obj_from_bits(owner).as_ptr().unwrap();
                molt_set_attr_name(owner, key, item_bits);
                assert!(!exception_pending(py));
                let dictionary = molt_get_attr_name(owner, dict_key);
                let dictionary_ptr = obj_from_bits(dictionary).as_ptr().unwrap();
                assert_eq!(crate::instance_dict_bits(pointer), dictionary);
                let before =
                    (*crate::object::header_from_obj_ptr(dictionary_ptr)).ref_count_snapshot();
                let mut edges = Vec::new();
                crate::object::gc::molt_traverse(py, pointer, &mut |child| edges.push(child));
                assert_eq!(
                    edges
                        .iter()
                        .filter(|&&child| child == dictionary_ptr)
                        .count(),
                    1
                );
                assert_eq!(
                    (*crate::object::header_from_obj_ptr(dictionary_ptr)).ref_count_snapshot(),
                    before
                );
                if owner == io {
                    dict_set_in_place(
                        py,
                        dictionary_ptr,
                        closed_key,
                        MoltObject::from_int(99).bits(),
                    );
                    assert_eq!(
                        molt_get_attr_name(owner, closed_key),
                        MoltObject::from_bool(false).bits()
                    );
                    molt_set_attr_name(owner, closed_key, MoltObject::from_bool(true).bits());
                    assert_attribute_error(py);
                    molt_set_attr_name(owner, dict_key, dictionary);
                    assert_attribute_error(py);
                    molt_del_attr_name(owner, dict_key);
                    assert_attribute_error(py);
                }
                crate::object::gc::molt_clear(py, pointer);
                assert_eq!(crate::instance_dict_bits(pointer), 0);
                assert_eq!(
                    (*crate::object::header_from_obj_ptr(dictionary_ptr)).ref_count_snapshot(),
                    before - 1
                );
                crate::object::gc::molt_clear(py, pointer);
                assert_eq!(
                    (*crate::object::header_from_obj_ptr(dictionary_ptr)).ref_count_snapshot(),
                    before - 1
                );
                dec_ref_bits(py, dictionary);
            }
            assert_eq!(
                crate::object::seq_access::with_immutable_tuple_slice(tuple_ptr, |items| items
                    .to_vec()),
                Some(vec![item_bits])
            );
            assert_eq!(object_class_bits(tuple_ptr), class);
            assert!(crate::object::instance_dict_bits_ptr(exact).is_null());
            assert_eq!(
                crate::object::seq_access::with_immutable_tuple_slice(exact, |items| items
                    .to_vec()),
                Some(vec![item_bits])
            );
            for bits in [
                tuple,
                io,
                MoltObject::from_ptr(exact).bits(),
                item_bits,
                key,
                dict_key,
                closed_key,
                class,
                name,
            ] {
                dec_ref_bits(py, bits);
            }
            assert!(!exception_pending(py));
        }
    });
}

#[test]
fn native_callable_declarations_own_public_identity_and_binding() {
    let _transaction = crate::test_support::RuntimeTestTransaction::new();
    crate::with_gil_entry_nopanic!(py, {
        unsafe {
            let classes = builtin_classes(py);
            let key = attr_name_bits_from_bytes(py, b"fromkeys").unwrap();
            // Materialize the class dictionary before looking up this member.
            assert!(crate::builtins::methods::publish_builtin_class_methods(
                py,
                classes.dict
            ));
            let dict_class = obj_from_bits(classes.dict).as_ptr().unwrap();
            let namespace = obj_from_bits(class_dict_bits(dict_class)).as_ptr().unwrap();
            let class_descriptor = dict_get_in_place(py, namespace, key).unwrap();
            assert_eq!(
                object_class_bits(obj_from_bits(class_descriptor).as_ptr().unwrap()),
                classes.classmethod_descriptor
            );
            assert_eq!(
                crate::builtins::methods::builtin_class_method_bits(py, classes.dict, "fromkeys"),
                Some(class_descriptor)
            );
            let append =
                crate::builtins::methods::builtin_class_method_bits(py, classes.list, "append")
                    .unwrap();
            let length =
                crate::builtins::methods::builtin_class_method_bits(py, classes.list, "__len__")
                    .unwrap();
            assert_eq!(type_of_bits(py, append), classes.method_descriptor);
            assert_eq!(type_of_bits(py, length), classes.wrapper_descriptor);
            let list_ptr = alloc_list(py, &[]);
            let list = MoltObject::from_ptr(list_ptr).bits();
            let bound_append = descriptor_bind(py, append, Some(classes.list), Some(list)).unwrap();
            let bound_length = descriptor_bind(py, length, Some(classes.list), Some(list)).unwrap();
            assert_eq!(
                type_of_bits(py, bound_append),
                classes.builtin_function_or_method
            );
            assert_eq!(type_of_bits(py, bound_length), classes.method_wrapper);
            for callable in [append, length, class_descriptor, bound_append, bound_length] {
                let pointer = obj_from_bits(callable).as_ptr().unwrap();
                for name in [
                    b"__dict__".as_slice(),
                    b"__defaults__",
                    b"__kwdefaults__",
                    b"__annotations__",
                    b"__code__",
                    b"__func__",
                    b"__molt_arg_names__",
                ] {
                    let name = attr_name_bits_from_bytes(py, name).unwrap();
                    assert!(attr_lookup_ptr(py, pointer, name).is_none());
                    assert!(!exception_pending(py));
                    dec_ref_bits(py, name);
                }
                let name = attr_name_bits_from_bytes(py, b"__qualname__").unwrap();
                let qualified = attr_lookup_ptr(py, pointer, name).unwrap();
                assert!(
                    string_obj_to_owned(obj_from_bits(qualified))
                        .unwrap()
                        .contains('.')
                );
                dec_ref_bits(py, qualified);
                dec_ref_bits(py, name);
            }
            let get = attr_name_bits_from_bytes(py, b"__get__").unwrap();
            assert!(
                attr_lookup_ptr(py, obj_from_bits(bound_append).as_ptr().unwrap(), get).is_none()
            );
            let descriptor_get =
                attr_lookup_ptr(py, obj_from_bits(append).as_ptr().unwrap(), get).unwrap();
            assert_eq!(type_of_bits(py, descriptor_get), classes.method_wrapper);
            dec_ref_bits(py, descriptor_get);
            let dict = MoltObject::from_ptr(alloc_dict_with_pairs(py, &[])).bits();
            let rejected = call_callable1(py, length, dict);
            assert!(exception_pending(py));
            assert!(exception_matches_builtin_name(
                py,
                exception_last_bits_noinc(py).unwrap(),
                "TypeError"
            ));
            molt_exception_clear();
            dec_ref_bits(py, rejected);
            let result = call_callable1(py, length, list);
            assert_eq!(obj_from_bits(result).as_int(), Some(0));
            dec_ref_bits(py, result);

            let native =
                crate::builtins::methods::alloc_builtin_function(py, fn_addr!(molt_len), 1);
            let unchanged = descriptor_bind(py, native, Some(classes.list), Some(list)).unwrap();
            assert_eq!(unchanged, native);
            dec_ref_bits(py, unchanged);
            // Managed binding remains MethodType, including explicit wrappers
            // around a native callable, and delegates reads to its function.
            let managed_ptr = alloc_function_obj(py, fn_addr!(molt_len), 1);
            let managed = MoltObject::from_ptr(managed_ptr).bits();
            let extra = attr_name_bits_from_bytes(py, b"extra").unwrap();
            assert!(crate::call::class_init::function_set_attr_bits(
                py,
                managed_ptr,
                extra,
                list
            ));
            let method = descriptor_bind(py, managed, Some(classes.list), Some(list)).unwrap();
            let explicit = crate::builtins::functions::bound_method_new(py, native, list, false);
            assert_eq!(
                type_of_bits(py, method),
                crate::builtins::types::method_class(py)
            );
            assert_eq!(
                type_of_bits(py, explicit),
                crate::builtins::types::method_class(py)
            );
            let extra_value = molt_get_attr_name(method, extra);
            assert_eq!(extra_value, list);
            dec_ref_bits(py, extra_value);
            molt_set_attr_name(method, extra, dict);
            assert_attribute_error(py);
            assert!(!classes.is_builtin_callable_class(classes.method_descriptor));
            assert!(classes.is_native_callable_class(classes.method_descriptor));
            for bits in [
                key,
                list,
                dict,
                bound_append,
                bound_length,
                get,
                native,
                managed,
                method,
                explicit,
                extra,
            ] {
                dec_ref_bits(py, bits);
            }
            assert!(!exception_pending(py));
        }
    });
}

#[test]
fn native_bound_callable_module_has_one_independent_traced_edge() {
    let _transaction = crate::test_support::RuntimeTestTransaction::with_gc_isolation();
    crate::with_gil_entry_nopanic!(py, {
        unsafe {
            let classes = builtin_classes(py);
            let descriptor =
                crate::builtins::methods::builtin_class_method_bits(py, classes.list, "append")
                    .unwrap();
            let list = MoltObject::from_ptr(alloc_list(py, &[])).bits();
            let first = descriptor_bind(py, descriptor, Some(classes.list), Some(list)).unwrap();
            let second = descriptor_bind(py, descriptor, Some(classes.list), Some(list)).unwrap();
            assert_ne!(first, second);
            let value_ptr = alloc_list(py, &[]);
            let value = MoltObject::from_ptr(value_ptr).bits();
            let before = (*header_from_obj_ptr(value_ptr)).ref_count_snapshot();
            let module = attr_name_bits_from_bytes(py, b"__module__").unwrap();
            molt_set_attr_name(first, module, value);
            assert!(!exception_pending(py));
            assert_eq!(
                (*header_from_obj_ptr(value_ptr)).ref_count_snapshot(),
                before + 1
            );
            assert_eq!(
                molt_get_attr_name(second, module),
                MoltObject::none().bits()
            );
            let first_ptr = obj_from_bits(first).as_ptr().unwrap();
            let mut edges = Vec::new();
            crate::object::gc::molt_traverse(py, first_ptr, &mut |edge| edges.push(edge));
            assert_eq!(edges.iter().filter(|&&edge| edge == value_ptr).count(), 1);
            assert_eq!(
                (*header_from_obj_ptr(value_ptr)).ref_count_snapshot(),
                before + 1
            );
            crate::object::gc::molt_clear(py, first_ptr);
            assert_eq!(
                (*header_from_obj_ptr(value_ptr)).ref_count_snapshot(),
                before
            );
            crate::object::gc::molt_clear(py, first_ptr);
            assert_eq!(
                (*header_from_obj_ptr(value_ptr)).ref_count_snapshot(),
                before
            );
            assert_eq!(bound_method_func_bits(first_ptr), descriptor);
            assert_eq!(bound_method_self_bits(first_ptr), list);
            molt_set_attr_name(second, module, value);
            assert_eq!(
                (*header_from_obj_ptr(value_ptr)).ref_count_snapshot(),
                before + 1
            );
            molt_del_attr_name(second, module);
            assert_eq!(
                (*header_from_obj_ptr(value_ptr)).ref_count_snapshot(),
                before
            );
            molt_set_attr_name(second, module, value);
            dec_ref_bits(py, second);
            assert_eq!(
                (*header_from_obj_ptr(value_ptr)).ref_count_snapshot(),
                before
            );
            for bits in [first, list, value, module] {
                dec_ref_bits(py, bits);
            }
            assert!(!exception_pending(py));
        }
    });
}

#[test]
fn native_type_metadata_and_c_mappingproxy_share_runtime_descriptors() {
    use molt_cpython_abi::abi_types::*;
    use molt_cpython_abi::api::refcount::OwnedPyObject;
    use molt_cpython_abi::api::{errors, mapping, object, strings, typeobj};
    use std::ptr;

    let _transaction = crate::test_support::RuntimeTestTransaction::new();
    crate::with_gil_entry_nopanic!(py, {
        unsafe {
            let mut slots = [
                PyType_Slot {
                    slot: molt_cpython_abi::type_slots::Py_tp_doc,
                    pfunc: c"MetadataProbe($self, /)\n--\n\nVisible documentation."
                        .as_ptr()
                        .cast_mut()
                        .cast(),
                },
                PyType_Slot {
                    slot: 0,
                    pfunc: ptr::null_mut(),
                },
            ];
            let mut spec = PyType_Spec {
                name: c"namespace.MetadataProbe".as_ptr(),
                basicsize: std::mem::size_of::<PyObject>() as i32,
                itemsize: 0,
                flags: Py_TPFLAGS_DEFAULT as u32,
                slots: slots.as_mut_ptr(),
            };
            let class = OwnedPyObject::from_owned(typeobj::PyType_FromSpec(&mut spec));
            assert!(!class.as_ptr().is_null());
            let class_ptr = class.as_ptr().cast::<PyTypeObject>();
            let namespace = OwnedPyObject::from_owned(object::PyObject_GetAttrString(
                class.as_ptr(),
                c"__dict__".as_ptr(),
            ));
            assert!(!namespace.as_ptr().is_null());
            let namespace_bits = molt_cpython_abi::bridge::GLOBAL_BRIDGE
                .molt_value_for_pyobj(namespace.as_ptr())
                .unwrap();
            assert_eq!(
                type_of_bits(py, namespace_bits),
                crate::builtins::types::mappingproxy_class_bits(py)
            );
            let key = OwnedPyObject::from_owned(strings::PyUnicode_FromString(c"__doc__".as_ptr()));
            let get = OwnedPyObject::from_owned(object::PyObject_GetAttrString(
                namespace.as_ptr(),
                c"get".as_ptr(),
            ));
            let doc =
                OwnedPyObject::from_owned(object::PyObject_CallOneArg(get.as_ptr(), key.as_ptr()));
            assert!(!doc.as_ptr().is_null());
            let raw_doc = strings::PyUnicode_AsUTF8(doc.as_ptr());
            assert!(!raw_doc.is_null());
            assert_eq!(
                std::ffi::CStr::from_ptr(raw_doc).to_bytes(),
                b"Visible documentation."
            );

            let signature_key = OwnedPyObject::from_owned(strings::PyUnicode_FromString(
                c"__text_signature__".as_ptr(),
            ));
            let generic_doc = OwnedPyObject::from_owned(object::PyObject_GenericGetAttr(
                class.as_ptr(),
                key.as_ptr(),
            ));
            assert_eq!(
                typeobj::PyObject_RichCompareBool(generic_doc.as_ptr(), doc.as_ptr(), 2),
                1
            );
            let signature = OwnedPyObject::from_owned(object::PyObject_GenericGetAttr(
                class.as_ptr(),
                signature_key.as_ptr(),
            ));
            assert_eq!(
                std::ffi::CStr::from_ptr(strings::PyUnicode_AsUTF8(signature.as_ptr())).to_bytes(),
                b"($self, /)"
            );
            let abstract_key = OwnedPyObject::from_owned(strings::PyUnicode_FromString(
                c"__abstractmethods__".as_ptr(),
            ));
            assert!(
                object::PyObject_GenericGetAttr(class.as_ptr(), abstract_key.as_ptr()).is_null()
            );
            assert_ne!(
                errors::PyErr_ExceptionMatches((&raw mut PyExc_AttributeError).cast()),
                0
            );
            errors::PyErr_Clear();
            let truth =
                OwnedPyObject::from_owned(molt_cpython_abi::api::numbers::PyLong_FromLong(1));
            assert_eq!(
                object::PyObject_SetAttr(class.as_ptr(), abstract_key.as_ptr(), truth.as_ptr()),
                0
            );
            assert_ne!((*class_ptr).tp_flags & Py_TPFLAGS_IS_ABSTRACT, 0);
            let abstract_value = OwnedPyObject::from_owned(object::PyObject_GenericGetAttr(
                class.as_ptr(),
                abstract_key.as_ptr(),
            ));
            assert_eq!(abstract_value.as_ptr(), truth.as_ptr());
            assert_eq!(
                object::PyObject_SetAttr(class.as_ptr(), abstract_key.as_ptr(), ptr::null_mut()),
                0
            );
            assert_eq!((*class_ptr).tp_flags & Py_TPFLAGS_IS_ABSTRACT, 0);
            assert_eq!(
                object::PyObject_SetAttr(class.as_ptr(), abstract_key.as_ptr(), ptr::null_mut()),
                -1
            );
            assert_ne!(
                errors::PyErr_ExceptionMatches((&raw mut PyExc_AttributeError).cast()),
                0
            );
            errors::PyErr_Clear();
            assert_eq!(
                object::PyObject_SetAttr(class.as_ptr(), signature_key.as_ptr(), truth.as_ptr()),
                -1
            );
            assert_ne!(
                errors::PyErr_ExceptionMatches((&raw mut PyExc_AttributeError).cast()),
                0
            );
            errors::PyErr_Clear();
            assert_eq!(
                object::PyObject_SetAttr(class.as_ptr(), key.as_ptr(), truth.as_ptr()),
                0
            );
            let changed_doc = OwnedPyObject::from_owned(object::PyObject_GenericGetAttr(
                class.as_ptr(),
                key.as_ptr(),
            ));
            assert_eq!(changed_doc.as_ptr(), truth.as_ptr());
            assert_eq!(
                object::PyObject_SetAttr(class.as_ptr(), key.as_ptr(), ptr::null_mut()),
                -1
            );
            assert_ne!(
                errors::PyErr_ExceptionMatches((&raw mut PyExc_TypeError).cast()),
                0
            );
            errors::PyErr_Clear();

            assert_eq!(typeobj::PyUnstable_Type_AssignVersionTag(class_ptr), 1);
            let renamed =
                OwnedPyObject::from_owned(strings::PyUnicode_FromString(c"RenamedProbe".as_ptr()));
            assert_eq!(
                object::PyObject_SetAttrString(
                    class.as_ptr(),
                    c"__name__".as_ptr(),
                    renamed.as_ptr()
                ),
                0
            );
            assert_eq!((*class_ptr).tp_flags & Py_TPFLAGS_VALID_VERSION_TAG, 0);
            let observed = OwnedPyObject::from_owned(object::PyObject_GetAttrString(
                class.as_ptr(),
                c"__name__".as_ptr(),
            ));
            assert_eq!(
                typeobj::PyObject_RichCompareBool(observed.as_ptr(), renamed.as_ptr(), 2),
                1
            );
            assert_eq!(
                object::PyObject_SetAttrString(
                    class.as_ptr(),
                    c"__name__".as_ptr(),
                    ptr::null_mut()
                ),
                -1
            );
            assert_ne!(
                errors::PyErr_ExceptionMatches((&raw mut PyExc_TypeError).cast()),
                0
            );
            errors::PyErr_Clear();

            let annotations = OwnedPyObject::from_owned(mapping::PyDict_New());
            assert_eq!(
                object::PyObject_SetAttrString(
                    class.as_ptr(),
                    c"__annotations__".as_ptr(),
                    annotations.as_ptr()
                ),
                0
            );
            let annotated = OwnedPyObject::from_owned(object::PyObject_GetAttrString(
                class.as_ptr(),
                c"__annotations__".as_ptr(),
            ));
            assert_eq!(annotated.as_ptr(), annotations.as_ptr());
            assert_eq!(
                object::PyObject_SetAttrString(
                    class.as_ptr(),
                    c"__annotations__".as_ptr(),
                    ptr::null_mut()
                ),
                0
            );
            let proxy = OwnedPyObject::from_owned(mapping::PyDictProxy_New(annotations.as_ptr()));
            let value = OwnedPyObject::from_owned(strings::PyUnicode_FromString(c"live".as_ptr()));
            assert_eq!(
                mapping::PyDict_SetItem(annotations.as_ptr(), key.as_ptr(), value.as_ptr()),
                0
            );
            let live =
                OwnedPyObject::from_owned(object::PyObject_GetItem(proxy.as_ptr(), key.as_ptr()));
            assert_eq!(
                live.as_ptr(),
                value.as_ptr(),
                "mappingproxy lookup failed: {}",
                exception_last_bits_noinc(py)
                    .and_then(|bits| obj_from_bits(bits).as_ptr())
                    .map(|error| format_exception_message(py, error))
                    .unwrap_or_default()
            );
            assert_eq!(
                object::PyObject_SetItem(proxy.as_ptr(), key.as_ptr(), value.as_ptr()),
                -1
            );
            assert_ne!(
                errors::PyErr_ExceptionMatches((&raw mut PyExc_TypeError).cast()),
                0
            );
            errors::PyErr_Clear();
            dec_ref_bits(py, namespace_bits);
            assert!(!exception_pending(py));
            assert!(errors::PyErr_Occurred().is_null());
        }
    });
}

#[test]
fn function_dictionary_descriptor_vars_generic_reads_and_gc_share_one_owner() {
    use molt_cpython_abi::api::{errors, object, refcount};
    use molt_cpython_abi::bridge::GLOBAL_BRIDGE;

    let _transaction = crate::test_support::RuntimeTestTransaction::with_gc_isolation();
    assert!(crate::cpython_abi_hooks::register_cpython_hooks());
    crate::with_gil_entry_nopanic!(py, {
        unsafe {
            let function = alloc_function_obj(py, fn_addr!(molt_len), 1);
            let receiver = alloc_list(py, &[]);
            assert!(!function.is_null() && !receiver.is_null());
            let function_bits = MoltObject::from_ptr(function).bits();
            let receiver_bits = MoltObject::from_ptr(receiver).bits();
            let key = attr_name_bits_from_bytes(py, b"__dict__").unwrap();
            let extra = attr_name_bits_from_bytes(py, b"extra").unwrap();
            assert_eq!(crate::instance_dict_bits(function), 0);
            assert_eq!(
                crate::object::instance_dict_bits_ptr(function),
                crate::object::layout::function_dict_bits_ptr(function)
            );

            // The actual class data descriptor used to fail before reaching
            // the function-only fallback. Exercise it through public lookup.
            let dictionary = molt_get_attr_name(function_bits, key);
            let dictionary_ptr = obj_from_bits(dictionary).as_ptr().unwrap();
            assert_eq!(object_type_id(dictionary_ptr), TYPE_ID_DICT);
            assert_eq!(crate::instance_dict_bits(function), dictionary);
            molt_set_attr_name(function_bits, extra, MoltObject::from_int(17).bits());
            let bound = crate::molt_bound_method_new(function_bits, receiver_bits);
            assert!(!exception_pending(py));
            for observed in [
                crate::object::ops_builtins::molt_vars_builtin(function_bits),
                molt_get_attr_name(bound, key),
                crate::object::ops_builtins::molt_vars_builtin(bound),
                crate::object::ops_builtins::molt_object_getattribute(function_bits, key),
            ] {
                assert_eq!(observed, dictionary);
                dec_ref_bits(py, observed);
            }
            assert_eq!(
                dict_get_in_place(py, dictionary_ptr, extra),
                Some(MoltObject::from_int(17).bits())
            );

            // The native generic consumer must reach the same logical class
            // descriptor and the same managed dictionary, not a C shadow.
            let view = GLOBAL_BRIDGE.borrowed_handle_to_new_pyobj(function_bits);
            let key_view = GLOBAL_BRIDGE.borrowed_handle_to_new_pyobj(key);
            assert!(!view.is_null() && !key_view.is_null());
            let generic = object::PyObject_GenericGetAttr(view, key_view);
            assert!(!generic.is_null());
            let generic_bits = GLOBAL_BRIDGE.molt_value_for_pyobj(generic).unwrap();
            assert_eq!(generic_bits, dictionary);
            dec_ref_bits(py, generic_bits);
            refcount::Py_DECREF(generic);
            refcount::Py_DECREF(key_view);
            refcount::Py_DECREF(view);

            let replacement_ptr = alloc_dict_with_pairs(py, &[extra, receiver_bits]);
            assert!(!replacement_ptr.is_null());
            let replacement = MoltObject::from_ptr(replacement_ptr).bits();
            molt_set_attr_name(function_bits, key, replacement);
            assert!(!exception_pending(py));
            assert_eq!(crate::instance_dict_bits(function), replacement);
            let forwarded = molt_get_attr_name(bound, key);
            assert_eq!(forwarded, replacement);
            dec_ref_bits(py, forwarded);
            assert_eq!(
                dict_get_in_place(py, dictionary_ptr, extra),
                Some(MoltObject::from_int(17).bits())
            );
            molt_set_attr_name(function_bits, key, receiver_bits);
            assert!(exception_matches_builtin_name(
                py,
                exception_last_bits_noinc(py).unwrap(),
                "TypeError"
            ));
            molt_exception_clear();
            molt_del_attr_name(function_bits, key);
            assert!(exception_matches_builtin_name(
                py,
                exception_last_bits_noinc(py).unwrap(),
                "TypeError"
            ));
            molt_exception_clear();
            assert_eq!(crate::instance_dict_bits(function), replacement);

            let before = (*header_from_obj_ptr(replacement_ptr)).ref_count_snapshot();
            let mut edges = Vec::new();
            crate::object::gc::molt_traverse(py, function, &mut |edge| edges.push(edge));
            assert_eq!(
                edges
                    .iter()
                    .filter(|&&edge| edge == replacement_ptr)
                    .count(),
                1
            );
            crate::object::gc::molt_clear(py, function);
            assert_eq!(crate::instance_dict_bits(function), 0);
            assert_eq!(
                (*header_from_obj_ptr(replacement_ptr)).ref_count_snapshot(),
                before - 1
            );
            crate::object::gc::molt_clear(py, function);
            assert_eq!(
                (*header_from_obj_ptr(replacement_ptr)).ref_count_snapshot(),
                before - 1
            );
            for bits in [
                bound,
                function_bits,
                receiver_bits,
                key,
                extra,
                dictionary,
                replacement,
            ] {
                dec_ref_bits(py, bits);
            }
            assert!(!exception_pending(py));
            assert!(errors::PyErr_Occurred().is_null());
        }
    });
}

#[test]
fn native_callable_module_assignment_failure_preserves_the_physical_owner() {
    use molt_cpython_abi::abi_types::{METH_NOARGS, PyCFunctionObject, PyMethodDef, PyObject};
    use molt_cpython_abi::api::{errors, object, refcount};
    use molt_cpython_abi::bridge::GLOBAL_BRIDGE;

    unsafe extern "C" fn noargs(_self: *mut PyObject, _args: *mut PyObject) -> *mut PyObject {
        unsafe { object::Py_NewRef(&raw mut molt_cpython_abi::abi_types::Py_None) }
    }

    let _transaction = crate::test_support::RuntimeTestTransaction::new();
    assert!(crate::cpython_abi_hooks::register_cpython_hooks());
    crate::with_gil_entry_nopanic!(py, {
        unsafe {
            let mut definition = PyMethodDef {
                ml_name: c"module_failure".as_ptr(),
                ml_meth: Some(noargs),
                ml_flags: METH_NOARGS,
                ml_doc: std::ptr::null(),
            };
            let callable = object::PyCFunction_NewEx(
                &raw mut definition,
                std::ptr::null_mut(),
                std::ptr::null_mut(),
            );
            assert!(!callable.is_null());
            let callable_bits = GLOBAL_BRIDGE.molt_value_for_pyobj(callable).unwrap();
            let module_key = attr_name_bits_from_bytes(py, b"__module__").unwrap();
            let module = attr_name_bits_from_bytes(py, b"unchanged_module").unwrap();
            molt_set_attr_name(callable_bits, module_key, module);
            assert!(
                !exception_pending(py),
                "initial native module assignment failed: {}",
                exception_last_bits_noinc(py)
                    .and_then(|bits| obj_from_bits(bits).as_ptr())
                    .map(|error| format_exception_message(py, error))
                    .unwrap_or_default()
            );
            let original = (*callable.cast::<PyCFunctionObject>()).m_module;
            assert!(!original.is_null());

            // Public class creation rejects NUL. Deliberately corrupt the
            // internal name slot of a valid cold class to exercise real bridge
            // admission failure, without replacing a hook or synthesizing an
            // exception. Restore the invariant before assertions/retirement.
            let name = attr_name_bits_from_bytes(py, b"ModuleValue").unwrap();
            let unrepresentable = molt_class_new(name);
            assert!(!exception_pending(py));
            let bad_name = attr_name_bits_from_bytes(py, b"Module\0Value").unwrap();
            let class_ptr = obj_from_bits(unrepresentable).as_ptr().unwrap();
            crate::object::class_storage::ClassReferenceSlot::Name
                .replace_borrowed(py, class_ptr, bad_name);
            molt_set_attr_name(callable_bits, module_key, unrepresentable);
            molt_cpython_abi::api::errors::with_preserved_error(|| {
                crate::object::class_storage::ClassReferenceSlot::Name
                    .replace_borrowed(py, class_ptr, name);
                dec_ref_bits(py, bad_name);
            });
            assert!(exception_pending(py));
            assert!(exception_matches_builtin_name(
                py,
                exception_last_bits_noinc(py).unwrap(),
                "SystemError"
            ));
            assert_eq!((*callable.cast::<PyCFunctionObject>()).m_module, original);
            molt_exception_clear();
            errors::PyErr_Clear();
            let observed = molt_get_attr_name(callable_bits, module_key);
            assert_eq!(observed, module);
            for bits in [
                observed,
                unrepresentable,
                name,
                module_key,
                module,
                callable_bits,
            ] {
                dec_ref_bits(py, bits);
            }
            refcount::Py_DECREF(callable);
            assert!(!exception_pending(py));
            assert!(errors::PyErr_Occurred().is_null());
        }
    });
}

#[test]
fn function_typed_metadata_survives_public_dictionary_replacement_and_clears_once() {
    use crate::object::function_metadata::FunctionMetadataField as Field;
    let _transaction = crate::test_support::RuntimeTestTransaction::with_gc_isolation();
    crate::with_gil_entry_nopanic!(py, {
        unsafe {
            let function = alloc_function_obj(py, fn_addr!(molt_len), 1);
            let bits = MoltObject::from_ptr(function).bits();
            let value = alloc_list(py, &[]);
            let value_bits = MoltObject::from_ptr(value).bits();
            let defaults = alloc_tuple(py, &[value_bits]);
            let defaults_bits = MoltObject::from_ptr(defaults).bits();
            let key = attr_name_bits_from_bytes(py, b"__defaults__").unwrap();
            let dict_key = attr_name_bits_from_bytes(py, b"__dict__").unwrap();
            assert!(crate::call::class_init::function_set_attr_bits(
                py,
                function,
                key,
                defaults_bits
            ));
            assert_eq!(crate::instance_dict_bits(function), 0);
            assert_eq!(Field::Defaults.load(function), Some(defaults_bits));
            let forged = alloc_dict_with_pairs(py, &[key, MoltObject::from_int(999).bits()]);
            let forged_bits = MoltObject::from_ptr(forged).bits();
            molt_set_attr_name(bits, dict_key, forged_bits);
            assert!(!exception_pending(py));
            let observed = molt_get_attr_name(bits, key);
            assert_eq!(observed, defaults_bits);
            dec_ref_bits(py, observed);
            assert_eq!(
                dict_get_in_place(py, forged, key),
                Some(MoltObject::from_int(999).bits())
            );
            molt_set_attr_name(bits, key, value_bits);
            assert!(exception_matches_builtin_name(
                py,
                exception_last_bits_noinc(py).unwrap(),
                "TypeError"
            ));
            molt_exception_clear();
            assert_eq!(Field::Defaults.load(function), Some(defaults_bits));
            let before = (*header_from_obj_ptr(defaults)).ref_count_snapshot();
            let mut children = Vec::new();
            crate::object::gc::molt_traverse(py, function, &mut |child| children.push(child));
            assert_eq!(
                children.iter().filter(|&&child| child == defaults).count(),
                1
            );
            assert_eq!(children.iter().filter(|&&child| child == forged).count(), 1);
            crate::object::gc::molt_clear(py, function);
            assert!(Field::Defaults.load(function).is_none());
            assert_eq!(crate::instance_dict_bits(function), 0);
            assert_eq!(
                (*header_from_obj_ptr(defaults)).ref_count_snapshot(),
                before - 1
            );
            crate::object::gc::molt_clear(py, function);
            assert_eq!(
                (*header_from_obj_ptr(defaults)).ref_count_snapshot(),
                before - 1
            );
            for bit in [bits, value_bits, defaults_bits, key, dict_key, forged_bits] {
                dec_ref_bits(py, bit);
            }
            assert!(!exception_pending(py));
        }
    });
}

#[test]
fn type_abstract_state_is_local_latched_and_updates_only_existing_c_views() {
    use crate::object::class_storage::class_is_abstract;
    use molt_cpython_abi::abi_types::{Py_TPFLAGS_IS_ABSTRACT, PyTypeObject};
    use molt_cpython_abi::bridge::GLOBAL_BRIDGE;
    let _transaction = crate::test_support::RuntimeTestTransaction::new();
    crate::with_gil_entry_nopanic!(py, {
        unsafe {
            let name = attr_name_bits_from_bytes(py, b"AbstractState").unwrap();
            let class = crate::molt_class_new(name);
            let child = crate::molt_class_new(name);
            dec_ref_bits(py, name);
            crate::molt_class_set_base(class, builtin_classes(py).object);
            crate::molt_class_set_base(child, class);
            let class_ptr = obj_from_bits(class).as_ptr().unwrap();
            let child_ptr = obj_from_bits(child).as_ptr().unwrap();
            crate::object::class_finish_definition(py, class_ptr).unwrap();
            crate::object::class_finish_definition(py, child_ptr).unwrap();
            let key = attr_name_bits_from_bytes(py, b"__abstractmethods__").unwrap();
            let values =
                MoltObject::from_ptr(alloc_list(py, &[MoltObject::from_int(1).bits()])).bits();
            assert!(!GLOBAL_BRIDGE.type_has_projection(class));
            crate::molt_set_attr_name(class, key, values);
            assert!(!exception_pending(py));
            assert!(class_is_abstract(class_ptr));
            assert!(!class_is_abstract(child_ptr));
            assert!(!GLOBAL_BRIDGE.type_has_projection(class));
            let view = GLOBAL_BRIDGE
                .borrowed_handle_to_new_pyobj(class)
                .cast::<PyTypeObject>();
            assert!(!view.is_null());
            assert_ne!((*view).tp_flags & Py_TPFLAGS_IS_ABSTRACT, 0);
            let mut heap_alias: Box<molt_cpython_abi::abi_types::PyHeapTypeObject> =
                Box::new(std::mem::zeroed());
            heap_alias.ht_type.ob_base.ob_base.ob_refcnt =
                molt_cpython_abi::abi_types::IMMORTAL_REFCNT;
            heap_alias.ht_type.tp_flags = molt_cpython_abi::abi_types::Py_TPFLAGS_HEAPTYPE;
            let heap_alias_pointer = (&raw mut heap_alias.ht_type).cast();
            GLOBAL_BRIDGE
                .bind_static_pyobj_to_runtime_handle(heap_alias_pointer, class, false)
                .unwrap();
            crate::molt_set_attr_name(class, key, values);
            assert!(!exception_pending(py));
            assert_ne!(heap_alias.ht_type.tp_flags & Py_TPFLAGS_IS_ABSTRACT, 0);
            crate::molt_set_attr_name(class, key, MoltObject::none().bits());
            assert!(!exception_pending(py));
            assert!(!class_is_abstract(class_ptr));
            assert_eq!((*view).tp_flags & Py_TPFLAGS_IS_ABSTRACT, 0);
            assert_eq!(heap_alias.ht_type.tp_flags & Py_TPFLAGS_IS_ABSTRACT, 0);
            assert!(
                GLOBAL_BRIDGE.unbind_static_pyobj_from_runtime_handle(heap_alias_pointer, class)
            );
            crate::molt_del_attr_name(class, key);
            assert!(!exception_pending(py));
            assert!(!class_is_abstract(class_ptr));
            // Direct abstract getset writes are a CPython exception to normal
            // immutable-type mutation. Every existing static alias observes it.
            let static_class = builtin_classes(py).int;
            let static_ptr = obj_from_bits(static_class).as_ptr().unwrap();
            crate::molt_set_attr_name(static_class, key, values);
            assert!(exception_pending(py));
            molt_exception_clear();
            let metatype = obj_from_bits(builtin_classes(py).type_obj)
                .as_ptr()
                .unwrap();
            let namespace = obj_from_bits(class_dict_bits(metatype)).as_ptr().unwrap();
            let descriptor = dict_get_in_place(py, namespace, key).unwrap();
            let canonical = GLOBAL_BRIDGE
                .borrowed_handle_to_new_pyobj(static_class)
                .cast::<PyTypeObject>();
            let mut alias: Box<PyTypeObject> = Box::new(std::mem::zeroed());
            alias.ob_base.ob_base.ob_refcnt = molt_cpython_abi::abi_types::IMMORTAL_REFCNT;
            alias.tp_flags = molt_cpython_abi::abi_types::Py_TPFLAGS_IMMUTABLETYPE;
            let alias_pointer = (&mut *alias as *mut PyTypeObject).cast();
            GLOBAL_BRIDGE
                .bind_static_pyobj_to_runtime_handle(alias_pointer, static_class, false)
                .unwrap();
            crate::builtins::types::native_descriptor_mutate(
                py,
                descriptor,
                static_class,
                Some(values),
            );
            assert!(!exception_pending(py));
            assert!(class_is_abstract(static_ptr));
            assert_ne!((*canonical).tp_flags & Py_TPFLAGS_IS_ABSTRACT, 0);
            assert_ne!(alias.tp_flags & Py_TPFLAGS_IS_ABSTRACT, 0);
            crate::builtins::types::native_descriptor_mutate(py, descriptor, static_class, None);
            assert!(!exception_pending(py));
            assert!(!class_is_abstract(static_ptr));
            assert_eq!((*canonical).tp_flags & Py_TPFLAGS_IS_ABSTRACT, 0);
            assert_eq!(alias.tp_flags & Py_TPFLAGS_IS_ABSTRACT, 0);
            assert!(
                GLOBAL_BRIDGE.unbind_static_pyobj_from_runtime_handle(alias_pointer, static_class)
            );
            molt_cpython_abi::api::refcount::Py_DECREF(canonical.cast());
            molt_cpython_abi::api::refcount::Py_DECREF(view.cast());
            for bits in [values, key, child, class] {
                dec_ref_bits(py, bits);
            }
        }
    });
}

#[test]
fn managed_type_metadata_publication_orders_watchers_and_displaced_reentry() {
    use molt_cpython_abi::abi_types::{PyObject, PyTypeObject};
    use molt_cpython_abi::api::{refcount::OwnedPyObject, typeobj};
    use molt_cpython_abi::bridge::GLOBAL_BRIDGE;
    use std::sync::atomic::{AtomicU64, AtomicUsize, Ordering};
    static WATCHES: AtomicUsize = AtomicUsize::new(0);
    static ROOT: AtomicU64 = AtomicU64::new(0);
    static VIEW: AtomicUsize = AtomicUsize::new(0);
    static RETIRED: AtomicUsize = AtomicUsize::new(0);
    unsafe extern "C" fn watched(_: *mut PyObject) -> i32 {
        WATCHES.fetch_add(1, Ordering::Relaxed);
        0
    }
    extern "C" fn retired(_: u64) -> u64 {
        crate::with_gil_entry_nopanic!(py, {
            unsafe {
                let class = ROOT.load(Ordering::Relaxed);
                let view = VIEW.load(Ordering::Relaxed) as *mut PyTypeObject;
                let doc = type_metadata::read(py, class, typeobj::TypeAttributeField::Doc);
                if obj_from_bits(doc).as_int() == Some(37)
                    && WATCHES.load(Ordering::Relaxed) == 1
                    && (*view).tp_version_tag == 0
                {
                    RETIRED.store(1, Ordering::Relaxed);
                }
                dec_ref_bits(py, doc);
                // Reentry must see and be allowed to replace the committed doc.
                type_metadata::write(
                    py,
                    class,
                    typeobj::TypeAttributeField::Doc,
                    Some(MoltObject::from_int(73).bits()),
                );
            }
            MoltObject::none().bits()
        })
    }
    let _transaction = crate::test_support::RuntimeTestTransaction::new();
    crate::with_gil_entry_nopanic!(py, {
        unsafe {
            let name = attr_name_bits_from_bytes(py, b"MetadataPublication").unwrap();
            let class = molt_class_new(name);
            molt_class_set_base(class, builtin_classes(py).object);
            let class_ptr = obj_from_bits(class).as_ptr().unwrap();
            crate::object::class_finish_definition(py, class_ptr).unwrap();
            let view = OwnedPyObject::from_owned(GLOBAL_BRIDGE.borrowed_handle_to_new_pyobj(class));
            let tp = view.as_ptr().cast::<PyTypeObject>();
            let watcher = typeobj::PyType_AddWatcher(Some(watched));
            assert!(watcher >= 0);
            assert_eq!(typeobj::PyType_Watch(watcher, view.as_ptr()), 0);
            let dictionary = crate::molt_dict_new(0);
            let methods = MoltObject::from_ptr(alloc_tuple(py, &[name])).bits();
            let root_namespace = obj_from_bits(class_dict_bits(
                obj_from_bits(builtin_classes(py).type_obj)
                    .as_ptr()
                    .unwrap(),
            ))
            .as_ptr()
            .unwrap();
            for direct in [false, true] {
                for (field, value) in [
                    (typeobj::TypeAttributeField::Doc, name),
                    (typeobj::TypeAttributeField::Name, name),
                    (typeobj::TypeAttributeField::QualName, name),
                    (typeobj::TypeAttributeField::Annotations, dictionary),
                    (typeobj::TypeAttributeField::AbstractMethods, methods),
                ] {
                    let key = attr_name_bits_from_bytes(py, field.name().as_bytes()).unwrap();
                    assert_eq!(typeobj::PyUnstable_Type_AssignVersionTag(tp), 1);
                    WATCHES.store(0, Ordering::Relaxed);
                    let epoch = class_layout_version_bits(class_ptr);
                    if direct {
                        let descriptor = dict_get_in_place(py, root_namespace, key).unwrap();
                        crate::builtins::types::native_descriptor_mutate(
                            py,
                            descriptor,
                            class,
                            Some(value),
                        );
                    } else {
                        molt_set_attr_name(class, key, value);
                    }
                    assert!(!exception_pending(py));
                    let invalidates = !direct
                        || !matches!(
                            field,
                            typeobj::TypeAttributeField::Name
                                | typeobj::TypeAttributeField::QualName
                        );
                    assert_eq!(WATCHES.load(Ordering::Relaxed), usize::from(invalidates));
                    assert_eq!((*tp).tp_version_tag == 0, invalidates);
                    assert_ne!(class_layout_version_bits(class_ptr), epoch);
                    dec_ref_bits(py, key);
                }
                assert_eq!(typeobj::PyUnstable_Type_AssignVersionTag(tp), 1);
                WATCHES.store(0, Ordering::Relaxed);
                let epoch = class_layout_version_bits(class_ptr);
                if direct {
                    type_metadata::write(
                        py,
                        class,
                        typeobj::TypeAttributeField::Name,
                        Some(MoltObject::from_int(1).bits()),
                    );
                } else {
                    let key = attr_name_bits_from_bytes(py, b"__name__").unwrap();
                    molt_set_attr_name(class, key, MoltObject::from_int(1).bits());
                    dec_ref_bits(py, key);
                }
                assert!(exception_pending(py));
                molt_exception_clear();
                assert_eq!(WATCHES.load(Ordering::Relaxed), 0);
                assert_ne!((*tp).tp_version_tag, 0);
                assert_eq!(class_layout_version_bits(class_ptr), epoch);
            }
            // An existing class C view must not force a C view for an ordinary
            // exact-string name merely to discover it has no physical slots.
            let key_ptr = alloc_string(py, b"metadata_unprojected_plain_field");
            let key = MoltObject::from_ptr(key_ptr).bits();
            assert!(
                !(*header_from_obj_ptr(key_ptr)).has_flag(crate::object::HEADER_FLAG_HAS_ABI_VIEW)
            );
            molt_set_attr_name(class, key, MoltObject::from_int(11).bits());
            molt_del_attr_name(class, key);
            assert!(!exception_pending(py));
            assert!(
                !(*header_from_obj_ptr(key_ptr)).has_flag(crate::object::HEADER_FLAG_HAS_ABI_VIEW)
            );

            let finalizer_class = molt_class_new(name);
            molt_class_set_base(finalizer_class, builtin_classes(py).object);
            let finalizer_key = attr_name_bits_from_bytes(py, b"__del__").unwrap();
            let function =
                MoltObject::from_ptr(alloc_function_obj(py, fn_addr!(retired), 1)).bits();
            molt_set_attr_name(finalizer_class, finalizer_key, function);
            let old_doc = crate::molt_object_new_bound(finalizer_class);
            type_metadata::write(py, class, typeobj::TypeAttributeField::Doc, Some(old_doc));
            dec_ref_bits(py, old_doc);
            ROOT.store(class, Ordering::Relaxed);
            VIEW.store(tp.addr(), Ordering::Relaxed);
            RETIRED.store(0, Ordering::Relaxed);
            assert_eq!(typeobj::PyUnstable_Type_AssignVersionTag(tp), 1);
            WATCHES.store(0, Ordering::Relaxed);
            type_metadata::write(
                py,
                class,
                typeobj::TypeAttributeField::Doc,
                Some(MoltObject::from_int(37).bits()),
            );
            assert!(!exception_pending(py));
            assert_eq!(RETIRED.load(Ordering::Relaxed), 1);
            assert_eq!(
                type_metadata::read(py, class, typeobj::TypeAttributeField::Doc),
                MoltObject::from_int(73).bits()
            );
            assert_eq!(typeobj::PyType_Unwatch(watcher, view.as_ptr()), 0);
            assert_eq!(typeobj::PyType_ClearWatcher(watcher), 0);
            ROOT.store(0, Ordering::Relaxed);
            VIEW.store(0, Ordering::Relaxed);
            drop(view);
            for bits in [
                finalizer_key,
                function,
                finalizer_class,
                key,
                methods,
                dictionary,
                class,
                name,
            ] {
                dec_ref_bits(py, bits);
            }
        }
    });
}

#[test]
fn class_creation_doc_is_private_traced_and_independent_of_mutable_namespace() {
    use crate::object::class_storage::ClassReferenceSlot;
    use molt_cpython_abi::api::typeobj::TypeAttributeField as Field;
    let _transaction = crate::test_support::RuntimeTestTransaction::new();
    crate::with_gil_entry_nopanic!(py, {
        unsafe {
            for guarded in [false, true] {
                let name = attr_name_bits_from_bytes(py, b"BirthDoc").unwrap();
                let key = attr_name_bits_from_bytes(py, b"__doc__").unwrap();
                let doc = MoltObject::from_ptr(alloc_string(
                    py,
                    b"BirthDoc(a, b)\n--\n\nOriginal.\0ignored",
                ))
                .bits();
                let class = if guarded {
                    let attrs = [key, doc];
                    crate::object::ops::molt_guarded_class_def(
                        name,
                        0,
                        0,
                        crate::provenance::abi::expose_address(attrs.as_ptr()),
                        1,
                        8,
                        1,
                        0,
                    )
                } else {
                    let namespace =
                        MoltObject::from_ptr(alloc_dict_with_pairs(py, &[key, doc])).bits();
                    let class = crate::builtins::types::molt_type_new(
                        builtin_classes(py).type_obj,
                        name,
                        MoltObject::none().bits(),
                        namespace,
                        MoltObject::none().bits(),
                    );
                    dec_ref_bits(py, namespace);
                    class
                };
                assert!(!exception_pending(py));
                let pointer = obj_from_bits(class).as_ptr().unwrap();
                let captured = ClassReferenceSlot::CreationDoc.load(pointer);
                assert_ne!(captured, doc);
                let mut edges = Vec::new();
                crate::object::heap_lifecycle::visit_owned_values(py, pointer, &mut |edge| {
                    edges.push(edge)
                });
                assert!(edges.contains(&captured));
                assert_eq!(
                    string_obj_to_owned(obj_from_bits(captured)).as_deref(),
                    Some("BirthDoc(a, b)\n--\n\nOriginal.")
                );
                molt_set_attr_name(class, key, MoltObject::from_int(19).bits());
                assert_eq!(
                    (*header_from_obj_ptr(obj_from_bits(doc).as_ptr().unwrap()))
                        .ref_count_snapshot(),
                    1
                );
                for (current, expected) in [
                    (b"BirthDoc".as_slice(), Some("(a, b)")),
                    (b"Renamed".as_slice(), None),
                    (b"BirthDoc".as_slice(), Some("(a, b)")),
                ] {
                    let current = attr_name_bits_from_bytes(py, current).unwrap();
                    type_metadata::write(py, class, Field::Name, Some(current));
                    let result = type_metadata::read(py, class, Field::TextSignature);
                    assert_eq!(
                        string_obj_to_owned(obj_from_bits(result)).as_deref(),
                        expected
                    );
                    dec_ref_bits(py, result);
                    dec_ref_bits(py, current);
                }
                assert!(!exception_pending(py));
                for bits in [class, doc, key, name] {
                    dec_ref_bits(py, bits);
                }
            }
        }
    });
}
