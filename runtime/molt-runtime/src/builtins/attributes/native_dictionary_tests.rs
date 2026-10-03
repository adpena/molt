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
