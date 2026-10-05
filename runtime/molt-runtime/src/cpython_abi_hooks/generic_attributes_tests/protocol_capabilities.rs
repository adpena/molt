//! Public C consumers distinguished by the CPython 3.12.13 protocol oracle.
use super::*;
use molt_cpython_abi::api::{abstract_mapping, abstract_sequence};

unsafe fn assert_protocol_slots(class: *mut PyTypeObject, present: &[i32]) {
    unsafe {
        for slot in [
            slots::Py_mp_ass_subscript,
            slots::Py_mp_length,
            slots::Py_mp_subscript,
            slots::Py_sq_ass_item,
            slots::Py_sq_concat,
            slots::Py_sq_contains,
            slots::Py_sq_inplace_concat,
            slots::Py_sq_inplace_repeat,
            slots::Py_sq_item,
            slots::Py_sq_length,
            slots::Py_sq_repeat,
        ] {
            assert_eq!(
                !typeobj::PyType_GetSlot(class, slot).is_null(),
                present.contains(&slot),
                "protocol slot {slot}"
            );
        }
    }
}

unsafe fn assert_type_error(status: isize, message: &str) {
    unsafe {
        assert_eq!(status, -1);
        assert_eq!(
            errors::PyErr_ExceptionMatches((&raw mut PyExc_TypeError).cast()),
            1,
            "{}",
            native_error_description()
        );
        assert_eq!(native_error_description(), message);
        // Only the independently checked expected failure is consumed.
        errors::PyErr_Clear();
    }
}

#[test]
fn native_mapping_proxies_publish_only_their_declared_protocols() {
    let _transaction = crate::test_support::RuntimeTestTransaction::new();
    crate::with_gil_entry_nopanic!(py, {
        unsafe {
            let dictionary = OwnedPyObject::from_owned(mapping::PyDict_New());
            assert_eq!(
                mapping::PyDict_SetItemString(
                    dictionary.as_ptr(),
                    c"one".as_ptr(),
                    &raw mut Py_None
                ),
                0
            );
            let dictionary_bits = GLOBAL_BRIDGE
                .molt_handle_for_pyobj(dictionary.as_ptr())
                .unwrap()
                .bits();
            let proxy = OwnedPyObject::from_owned(GLOBAL_BRIDGE.owned_handle_to_pyobj(
                crate::builtins::types::mappingproxy_from_mapping(py, dictionary_bits),
            ));
            assert!(!proxy.as_ptr().is_null(), "{}", native_error_description());
            let class = &raw mut PyDictProxy_Type;
            assert_protocol_slots(
                class,
                &[
                    slots::Py_mp_length,
                    slots::Py_mp_subscript,
                    slots::Py_sq_contains,
                ],
            );
            assert_eq!(abstract_mapping::PyMapping_Size(proxy.as_ptr()), 1);
            assert_eq!(abstract_sequence::PySequence_Check(proxy.as_ptr()), 0);
            assert_type_error(
                abstract_sequence::PySequence_Size(proxy.as_ptr()),
                "mappingproxy is not a sequence",
            );
            assert_eq!(
                typeobj::PyType_Ready(class),
                0,
                "{}",
                native_error_description()
            );
            assert_protocol_slots(
                class,
                &[
                    slots::Py_mp_length,
                    slots::Py_mp_subscript,
                    slots::Py_sq_contains,
                ],
            );

            // The adjacent mutable mapping factory owns assignment as well,
            // but its shared Python names must not manufacture sequence slots.
            let frame_proxy =
                OwnedPyObject::from_owned(GLOBAL_BRIDGE.borrowed_handle_to_new_pyobj(
                    crate::builtins::types::frame_locals_proxy_class(py),
                ));
            assert!(
                !frame_proxy.as_ptr().is_null(),
                "{}",
                native_error_description()
            );
            assert_protocol_slots(
                frame_proxy.as_ptr().cast(),
                &[
                    slots::Py_mp_length,
                    slots::Py_mp_subscript,
                    slots::Py_mp_ass_subscript,
                    slots::Py_sq_contains,
                ],
            );
            assert!(errors::PyErr_Occurred().is_null());
        }
    });
}

#[test]
fn native_dictionary_views_have_live_sequence_length_without_mapping_length() {
    let _transaction = crate::test_support::RuntimeTestTransaction::new();
    crate::with_gil_entry_nopanic!(py, {
        unsafe {
            let dictionary = OwnedPyObject::from_owned(mapping::PyDict_New());
            assert_eq!(
                mapping::PyDict_SetItemString(
                    dictionary.as_ptr(),
                    c"one".as_ptr(),
                    &raw mut Py_None
                ),
                0
            );
            let bits = GLOBAL_BRIDGE
                .molt_handle_for_pyobj(dictionary.as_ptr())
                .unwrap()
                .bits();
            for (name, view, class, contains) in [
                (
                    "dict_keys",
                    crate::molt_dict_keys(bits),
                    crate::builtin_classes(py).dict_keys,
                    true,
                ),
                (
                    "dict_items",
                    crate::molt_dict_items(bits),
                    crate::builtin_classes(py).dict_items,
                    true,
                ),
                (
                    "dict_values",
                    crate::molt_dict_values(bits),
                    crate::builtin_classes(py).dict_values,
                    false,
                ),
            ] {
                let receiver = OwnedPyObject::from_owned(GLOBAL_BRIDGE.owned_handle_to_pyobj(view));
                let class =
                    OwnedPyObject::from_owned(GLOBAL_BRIDGE.borrowed_handle_to_new_pyobj(class));
                assert!(
                    !receiver.as_ptr().is_null() && !class.as_ptr().is_null(),
                    "{}",
                    native_error_description()
                );
                let expected: &[i32] = if contains {
                    &[slots::Py_sq_length, slots::Py_sq_contains]
                } else {
                    &[slots::Py_sq_length]
                };
                assert_protocol_slots(class.as_ptr().cast(), expected);
                assert_eq!(abstract_sequence::PySequence_Check(receiver.as_ptr()), 0);
                assert_eq!(abstract_mapping::PyMapping_Check(receiver.as_ptr()), 0);
                assert_eq!(
                    abstract_sequence::PySequence_Size(receiver.as_ptr()),
                    1,
                    "{}",
                    native_error_description()
                );
                assert_type_error(
                    abstract_mapping::PyMapping_Size(receiver.as_ptr()),
                    &format!("{name} is not a mapping"),
                );
                assert_eq!(
                    mapping::PyDict_SetItemString(
                        dictionary.as_ptr(),
                        c"two".as_ptr(),
                        &raw mut Py_None
                    ),
                    0
                );
                assert_eq!(
                    abstract_sequence::PySequence_Size(receiver.as_ptr()),
                    2,
                    "{}",
                    native_error_description()
                );
                assert_eq!(
                    mapping::PyDict_DelItemString(dictionary.as_ptr(), c"two".as_ptr()),
                    0
                );
            }
            assert!(errors::PyErr_Occurred().is_null());
        }
    });
}

#[test]
fn python_subclasses_keep_both_length_slots_through_override_and_deletion() {
    let _transaction = crate::test_support::RuntimeTestTransaction::new();
    crate::with_gil_entry_nopanic!(py, {
        unsafe {
            let name = crate::attr_name_bits_from_bytes(py, b"mappingproxy").unwrap();
            let key = crate::attr_name_bits_from_bytes(py, b"__len__").unwrap();
            let seventeen = function(py, length_seventeen as *const (), 1);
            for base in [
                crate::builtin_classes(py).dict,
                crate::builtin_classes(py).set,
            ] {
                let class = crate::molt_class_new(name);
                crate::molt_class_set_base(class, base);
                crate::object::class_finish_definition(py, obj_from_bits(class).as_ptr().unwrap())
                    .unwrap();
                let class_view =
                    OwnedPyObject::from_owned(GLOBAL_BRIDGE.borrowed_handle_to_new_pyobj(class));
                let receiver =
                    OwnedPyObject::from_owned(object::PyObject_CallNoArgs(class_view.as_ptr()));
                assert!(
                    !receiver.as_ptr().is_null(),
                    "{}",
                    native_error_description()
                );
                for expected in [0, 17, 0] {
                    if expected == 17 {
                        crate::molt_set_attr_name(class, key, seventeen);
                        assert!(!crate::exception_pending(py));
                    }
                    assert_eq!(
                        abstract_sequence::PySequence_Size(receiver.as_ptr()),
                        expected,
                        "{}",
                        native_error_description()
                    );
                    assert_eq!(
                        abstract_mapping::PyMapping_Size(receiver.as_ptr()),
                        expected,
                        "{}",
                        native_error_description()
                    );
                    if expected == 17 {
                        crate::molt_del_attr_name(class, key);
                        assert!(
                            !crate::exception_pending(py),
                            "{}",
                            native_error_description()
                        );
                    }
                }
                dec_ref_bits(py, class);
            }
            for bits in [seventeen, key, name] {
                dec_ref_bits(py, bits);
            }
            assert!(errors::PyErr_Occurred().is_null());
        }
    });
}

thread_local! {
    static CAPTURED_PROTOCOL_ARGS: std::cell::Cell<u64> = const { std::cell::Cell::new(0) };
}

extern "C" fn capture_protocol_args(args: u64) -> u64 {
    crate::with_gil_entry_nopanic!(py, {
        inc_ref_bits(py, args);
        let old = CAPTURED_PROTOCOL_ARGS.with(|captured| captured.replace(args));
        if old != 0 {
            dec_ref_bits(py, old);
        }
        MoltObject::none().bits()
    })
}

const TUPLE_PROTOCOL_SLOTS: &[i32] = &[
    slots::Py_mp_length,
    slots::Py_mp_subscript,
    slots::Py_sq_concat,
    slots::Py_sq_contains,
    slots::Py_sq_item,
    slots::Py_sq_length,
    slots::Py_sq_repeat,
];

#[test]
fn native_unraisable_args_inherit_tuple_protocol_slots_and_consumers() {
    let _transaction = crate::test_support::RuntimeTestTransaction::new();
    crate::with_gil_entry_nopanic!(py, {
        unsafe {
            // Obtain the real hidden native subtype through its production
            // reporting path, retaining the exact argument passed to the hook.
            let sys_name = crate::attr_name_bits_from_bytes(py, b"sys").unwrap();
            let sys = crate::builtins::modules::molt_module_new(sys_name);
            crate::builtins::module_table::publish_interpreter_sys_for_test(py, sys);
            let key = crate::attr_name_bits_from_bytes(py, b"unraisablehook").unwrap();
            let hook = function(py, capture_protocol_args as *const (), 1);
            crate::builtins::modules::molt_module_set_attr(sys, key, hook);
            assert!(!crate::exception_pending(py));
            assert_eq!(CAPTURED_PROTOCOL_ARGS.with(|captured| captured.get()), 0);
            errors::PyErr_SetString(
                (&raw mut PyExc_ValueError).cast(),
                c"protocol capture".as_ptr(),
            );
            errors::PyErr_WriteUnraisable(&raw mut Py_None);
            assert!(
                errors::PyErr_Occurred().is_null(),
                "{}",
                native_error_description()
            );
            let bits = CAPTURED_PROTOCOL_ARGS.with(|captured| captured.replace(0));
            assert_ne!(bits, 0);
            assert_eq!(
                obj_from_bits(
                    crate::builtins::exceptions::molt_unraisable_hook_args_is_exact(bits)
                )
                .as_bool(),
                Some(true)
            );
            let args = OwnedPyObject::from_owned(GLOBAL_BRIDGE.owned_handle_to_pyobj(bits));
            assert!(!args.as_ptr().is_null(), "{}", native_error_description());
            // The physical carrier is a tuple; the public type inquiry must
            // project the actual reporting-produced native subtype itself.
            let class_view = OwnedPyObject::from_owned(typeobj::PyObject_Type(args.as_ptr()));
            assert!(
                !class_view.as_ptr().is_null(),
                "{}",
                native_error_description()
            );
            let class = class_view.as_ptr().cast::<PyTypeObject>();
            assert_ne!(class, &raw mut PyTuple_Type);
            assert_eq!(
                GLOBAL_BRIDGE
                    .molt_handle_for_pyobj(class_view.as_ptr())
                    .unwrap()
                    .bits(),
                crate::object_class_bits(obj_from_bits(bits).as_ptr().unwrap())
            );
            assert_eq!(typeobj::PyType_IsSubtype(class, &raw mut PyTuple_Type), 1);
            assert_eq!(sequences::PyTuple_Check(args.as_ptr()), 1);
            assert_eq!(sequences::PyTuple_CheckExact(args.as_ptr()), 0);
            assert_eq!(
                sequences::PyTuple_Size(args.as_ptr()),
                5,
                "{}",
                native_error_description()
            );
            assert_protocol_slots(class, TUPLE_PROTOCOL_SLOTS);
            for &slot in TUPLE_PROTOCOL_SLOTS {
                assert_eq!(
                    typeobj::PyType_GetSlot(class, slot),
                    typeobj::PyType_GetSlot(&raw mut PyTuple_Type, slot),
                    "native inherited tuple slot {slot}"
                );
            }
            assert_eq!(abstract_sequence::PySequence_Check(args.as_ptr()), 1);
            assert_eq!(abstract_mapping::PyMapping_Check(args.as_ptr()), 1);
            assert_eq!(
                abstract_sequence::PySequence_Size(args.as_ptr()),
                5,
                "{}",
                native_error_description()
            );
            assert_eq!(
                abstract_mapping::PyMapping_Size(args.as_ptr()),
                5,
                "{}",
                native_error_description()
            );
            let index = OwnedPyObject::from_owned(numbers::PyLong_FromLong(4));
            let item =
                OwnedPyObject::from_owned(abstract_sequence::PySequence_GetItem(args.as_ptr(), 4));
            let mapped =
                OwnedPyObject::from_owned(object::PyObject_GetItem(args.as_ptr(), index.as_ptr()));
            assert_eq!(item.as_ptr(), &raw mut Py_None);
            assert_eq!(mapped.as_ptr(), &raw mut Py_None);
            assert_eq!(
                abstract_sequence::PySequence_Contains(args.as_ptr(), &raw mut Py_None),
                1
            );
            let empty = OwnedPyObject::from_owned(sequences::PyTuple_New(0));
            let concat = OwnedPyObject::from_owned(abstract_sequence::PySequence_Concat(
                args.as_ptr(),
                empty.as_ptr(),
            ));
            let repeat =
                OwnedPyObject::from_owned(abstract_sequence::PySequence_Repeat(args.as_ptr(), 2));
            assert_eq!(
                sequences::PyTuple_Size(concat.as_ptr()),
                5,
                "{}",
                native_error_description()
            );
            assert_eq!(
                sequences::PyTuple_Size(repeat.as_ptr()),
                10,
                "{}",
                native_error_description()
            );
            assert_eq!(typeobj::PyType_Ready(class), 0);
            assert_protocol_slots(class, TUPLE_PROTOCOL_SLOTS);
            for bits in [sys_name, sys, key, hook] {
                dec_ref_bits(py, bits);
            }
            assert!(
                errors::PyErr_Occurred().is_null(),
                "{}",
                native_error_description()
            );
        }
    });
}

unsafe fn native_protocol_subclass(py: &crate::PyToken<'_>, name: &[u8], base: u64) -> u64 {
    unsafe {
        let name = crate::attr_name_bits_from_bytes(py, name).unwrap();
        let class = crate::molt_class_new(name);
        dec_ref_bits(py, name);
        let pointer = obj_from_bits(class).as_ptr().unwrap();
        crate::object::class_storage::class_declare_native_slots(
            pointer,
            crate::object::class_storage::ClassSlotPolicy::default(),
        );
        crate::molt_class_set_base(class, base);
        assert!(!crate::exception_pending(py));
        class
    }
}

extern "C" fn protocol_multiply_seventeen(_: u64, _: u64) -> u64 {
    MoltObject::from_int(17).bits()
}

#[test]
fn native_inherited_slot_admission_obeys_nearest_declaration_and_sibling_names() {
    let _transaction = crate::test_support::RuntimeTestTransaction::new();
    crate::with_gil_entry_nopanic!(py, {
        unsafe {
            let parent = native_protocol_subclass(
                py,
                b"NativeProtocolParent",
                crate::builtin_classes(py).tuple,
            );
            let pointer = obj_from_bits(parent).as_ptr().unwrap();
            crate::object::class_storage::class_declare_native_protocols(
                pointer,
                &[molt_cpython_abi::hooks::NativeProtocolSlot::MappingLength],
            );
            let len = crate::attr_name_bits_from_bytes(py, b"__len__").unwrap();
            let mul = crate::attr_name_bits_from_bytes(py, b"__mul__").unwrap();
            let length = function(py, length_seventeen as *const (), 1);
            let multiply = function(py, protocol_multiply_seventeen as *const (), 2);
            crate::molt_set_attr_name(parent, len, length);
            crate::molt_set_attr_name(parent, mul, multiply);
            crate::object::class_finish_definition(py, pointer).unwrap();
            let child = native_protocol_subclass(py, b"NativeProtocolChild", parent);
            crate::object::class_finish_definition(py, obj_from_bits(child).as_ptr().unwrap())
                .unwrap();
            let parent_view =
                OwnedPyObject::from_owned(GLOBAL_BRIDGE.borrowed_handle_to_new_pyobj(parent));
            let child_view =
                OwnedPyObject::from_owned(GLOBAL_BRIDGE.borrowed_handle_to_new_pyobj(child));
            let parent_value =
                OwnedPyObject::from_owned(object::PyObject_CallNoArgs(parent_view.as_ptr()));
            let child_value =
                OwnedPyObject::from_owned(object::PyObject_CallNoArgs(child_view.as_ptr()));
            assert!(
                !parent_value.as_ptr().is_null() && !child_value.as_ptr().is_null(),
                "{}",
                native_error_description()
            );
            let restricted = &[
                slots::Py_mp_length,
                slots::Py_mp_subscript,
                slots::Py_sq_concat,
                slots::Py_sq_contains,
                slots::Py_sq_item,
            ];
            for (class, value) in [(&parent_view, &parent_value), (&child_view, &child_value)] {
                assert_protocol_slots(class.as_ptr().cast(), restricted);
                assert_eq!(
                    abstract_mapping::PyMapping_Size(value.as_ptr()),
                    17,
                    "{}",
                    native_error_description()
                );
            }
            // Copying a tuple descriptor into the excluded own namespace must
            // not bypass that class's mask merely because its callable owner is tuple.
            let copied = crate::molt_get_attr_name(crate::builtin_classes(py).tuple, len);
            crate::molt_set_attr_name(parent, len, copied);
            dec_ref_bits(py, copied);
            for (class, value) in [(&parent_view, &parent_value), (&child_view, &child_value)] {
                assert_protocol_slots(class.as_ptr().cast(), restricted);
                assert_eq!(
                    abstract_mapping::PyMapping_Size(value.as_ptr()),
                    0,
                    "{}",
                    native_error_description()
                );
            }
            crate::molt_del_attr_name(parent, len);
            crate::molt_del_attr_name(parent, mul);
            assert!(
                !crate::exception_pending(py),
                "{}",
                native_error_description()
            );
            for (class, value) in [(&parent_view, &parent_value), (&child_view, &child_value)] {
                assert_protocol_slots(class.as_ptr().cast(), TUPLE_PROTOCOL_SLOTS);
                assert_eq!(abstract_sequence::PySequence_Size(value.as_ptr()), 0);
                assert_eq!(abstract_mapping::PyMapping_Size(value.as_ptr()), 0);
            }
            for bits in [parent, child, len, mul, length, multiply] {
                dec_ref_bits(py, bits);
            }
            assert!(
                errors::PyErr_Occurred().is_null(),
                "{}",
                native_error_description()
            );
        }
    });
}

#[test]
fn native_mapping_check_matrix_uses_real_runtime_protocols() {
    let _transaction = crate::test_support::RuntimeTestTransaction::new();
    crate::with_gil_entry_nopanic!(_py, {
        unsafe {
            let dictionary = OwnedPyObject::from_owned(mapping::PyDict_New());
            let bits = GLOBAL_BRIDGE
                .molt_handle_for_pyobj(dictionary.as_ptr())
                .unwrap()
                .bits();
            for (name, receiver, expected) in [
                ("list", sequences::PyList_New(0), 1),
                ("tuple", sequences::PyTuple_New(0), 1),
                ("str", strings::PyUnicode_FromString(c"".as_ptr()), 1),
                ("dict", GLOBAL_BRIDGE.borrowed_handle_to_new_pyobj(bits), 1),
                ("int", numbers::PyLong_FromLong(1), 0),
                ("set", sequences::PySet_New(ptr::null_mut()), 0),
                ("frozenset", sequences::PyFrozenSet_New(ptr::null_mut()), 0),
                (
                    "dict_keys",
                    GLOBAL_BRIDGE.owned_handle_to_pyobj(crate::molt_dict_keys(bits)),
                    0,
                ),
                (
                    "dict_items",
                    GLOBAL_BRIDGE.owned_handle_to_pyobj(crate::molt_dict_items(bits)),
                    0,
                ),
                (
                    "dict_values",
                    GLOBAL_BRIDGE.owned_handle_to_pyobj(crate::molt_dict_values(bits)),
                    0,
                ),
            ] {
                let receiver = OwnedPyObject::from_owned(receiver);
                assert!(
                    !receiver.as_ptr().is_null(),
                    "{name}: {}",
                    native_error_description()
                );
                assert_eq!(
                    abstract_mapping::PyMapping_Check(receiver.as_ptr()),
                    expected,
                    "{name}"
                );
                assert!(
                    errors::PyErr_Occurred().is_null(),
                    "{name}: {}",
                    native_error_description()
                );
            }
        }
    });
}

#[test]
fn native_dictionary_view_length_descriptors_report_received_type() {
    let _transaction = crate::test_support::RuntimeTestTransaction::new();
    crate::with_gil_entry_nopanic!(py, {
        unsafe {
            let integer = OwnedPyObject::from_owned(numbers::PyLong_FromLong(1));
            let tuple = OwnedPyObject::from_owned(sequences::PyTuple_New(0));
            for (name, class) in [
                ("dict_keys", crate::builtin_classes(py).dict_keys),
                ("dict_items", crate::builtin_classes(py).dict_items),
                ("dict_values", crate::builtin_classes(py).dict_values),
            ] {
                let class =
                    OwnedPyObject::from_owned(GLOBAL_BRIDGE.borrowed_handle_to_new_pyobj(class));
                let descriptor = OwnedPyObject::from_owned(object::PyObject_GetAttrString(
                    class.as_ptr(),
                    c"__len__".as_ptr(),
                ));
                assert!(
                    !descriptor.as_ptr().is_null(),
                    "{}",
                    native_error_description()
                );
                for (received, value) in [("int", &integer), ("tuple", &tuple)] {
                    let result = OwnedPyObject::from_owned(object::PyObject_CallOneArg(
                        descriptor.as_ptr(),
                        value.as_ptr(),
                    ));
                    assert!(result.as_ptr().is_null());
                    assert_type_error(
                        -1,
                        &format!(
                            "descriptor '__len__' requires a '{name}' object but received a '{received}'"
                        ),
                    );
                }
            }
            assert!(errors::PyErr_Occurred().is_null());
        }
    });
}

unsafe fn published_native_descriptor(owner: u64, name: &std::ffi::CStr) -> OwnedPyObject {
    unsafe {
        let class = OwnedPyObject::from_owned(GLOBAL_BRIDGE.borrowed_handle_to_new_pyobj(owner));
        assert!(!class.as_ptr().is_null(), "{}", native_error_description());
        let class = class.as_ptr().cast::<PyTypeObject>();
        assert_eq!(typeobj::PyType_Ready(class), 0);
        // Read the declaration itself: attribute lookup would bind a classmethod.
        let descriptor = mapping::PyDict_GetItemString((*class).tp_dict, name.as_ptr());
        assert!(!descriptor.is_null(), "{}", native_error_description());
        refcount::Py_INCREF(descriptor);
        OwnedPyObject::from_owned(descriptor)
    }
}

unsafe fn bind_native_descriptor(
    descriptor: *mut PyObject,
    receiver: *mut PyObject,
    owner: *mut PyObject,
) -> OwnedPyObject {
    unsafe {
        let class = OwnedPyObject::from_owned(typeobj::PyObject_Type(descriptor));
        assert!(!class.as_ptr().is_null(), "{}", native_error_description());
        let class = class.as_ptr().cast::<PyTypeObject>();
        assert_eq!(typeobj::PyType_Ready(class), 0);
        let bind = (*class)
            .tp_descr_get
            .expect("native descriptor binding slot");
        OwnedPyObject::from_owned(bind(descriptor, receiver, owner))
    }
}

#[test]
fn native_descriptor_public_names_and_call_binding_diagnostics_match_cpython() {
    let _transaction = crate::test_support::RuntimeTestTransaction::new();
    crate::with_gil_entry_nopanic!(py, {
        unsafe {
            let integer = OwnedPyObject::from_owned(numbers::PyLong_FromLong(1));
            for (owner, name, representation, direct, absent, binding) in [
                (
                    crate::builtin_classes(py).dict,
                    c"__len__",
                    "<slot wrapper '__len__' of 'dict' objects>",
                    "descriptor '__len__' requires a 'dict' object but received a 'int'",
                    "descriptor '__len__' of 'dict' object needs an argument",
                    "descriptor '__len__' for 'dict' objects doesn't apply to a 'int' object",
                ),
                (
                    crate::builtin_classes(py).list,
                    c"append",
                    "<method 'append' of 'list' objects>",
                    "descriptor 'append' for 'list' objects doesn't apply to a 'int' object",
                    "unbound method list.append() needs an argument",
                    "descriptor 'append' for 'list' objects doesn't apply to a 'int' object",
                ),
                (
                    crate::builtin_classes(py).dict,
                    c"fromkeys",
                    "<method 'fromkeys' of 'dict' objects>",
                    "descriptor 'fromkeys' for type 'dict' needs a type, not a 'int' as arg 2",
                    "descriptor 'fromkeys' of 'dict' object needs an argument",
                    "descriptor 'fromkeys' requires a subtype of 'dict' but received 'int'",
                ),
            ] {
                let descriptor = published_native_descriptor(owner, name);
                let rendered =
                    OwnedPyObject::from_owned(typeobj::PyObject_Repr(descriptor.as_ptr()));
                assert!(
                    !rendered.as_ptr().is_null(),
                    "{}",
                    native_error_description()
                );
                let text = strings::PyUnicode_AsUTF8(rendered.as_ptr());
                assert!(!text.is_null(), "{}", native_error_description());
                assert_eq!(
                    std::ffi::CStr::from_ptr(text).to_bytes(),
                    representation.as_bytes()
                );
                for (receiver, expected) in [(Some(integer.as_ptr()), direct), (None, absent)] {
                    let result = OwnedPyObject::from_owned(match receiver {
                        Some(receiver) => {
                            object::PyObject_CallOneArg(descriptor.as_ptr(), receiver)
                        }
                        None => object::PyObject_CallNoArgs(descriptor.as_ptr()),
                    });
                    assert!(result.as_ptr().is_null());
                    assert_type_error(-1, expected);
                }
                let result =
                    bind_native_descriptor(descriptor.as_ptr(), integer.as_ptr(), ptr::null_mut());
                assert!(result.as_ptr().is_null());
                assert_type_error(-1, binding);
            }

            let classmethod =
                published_native_descriptor(crate::builtin_classes(py).dict, c"fromkeys");
            let unrelated = OwnedPyObject::from_owned(
                GLOBAL_BRIDGE.borrowed_handle_to_new_pyobj(crate::builtin_classes(py).list),
            );
            let result = OwnedPyObject::from_owned(object::PyObject_CallOneArg(
                classmethod.as_ptr(),
                unrelated.as_ptr(),
            ));
            assert!(result.as_ptr().is_null());
            assert_type_error(
                -1,
                "descriptor 'fromkeys' requires a subtype of 'dict' but received 'list'",
            );
            for (owner, expected) in [
                (
                    integer.as_ptr(),
                    "descriptor 'fromkeys' for type 'dict' needs a type, not a 'int' as arg 2",
                ),
                (
                    unrelated.as_ptr(),
                    "descriptor 'fromkeys' requires a subtype of 'dict' but received 'list'",
                ),
                (
                    ptr::null_mut(),
                    "descriptor 'fromkeys' for type 'dict' needs either an object or a type",
                ),
            ] {
                let result = bind_native_descriptor(classmethod.as_ptr(), ptr::null_mut(), owner);
                assert!(result.as_ptr().is_null());
                assert_type_error(-1, expected);
            }
            let subclass =
                native_protocol_subclass(py, b"DiagnosticDict", crate::builtin_classes(py).dict);
            crate::object::class_finish_definition(py, obj_from_bits(subclass).as_ptr().unwrap())
                .unwrap();
            let class = OwnedPyObject::from_owned(GLOBAL_BRIDGE.owned_handle_to_pyobj(subclass));
            let instance = OwnedPyObject::from_owned(object::PyObject_CallNoArgs(class.as_ptr()));
            assert!(
                !instance.as_ptr().is_null(),
                "{}",
                native_error_description()
            );
            for (receiver, owner) in [
                (ptr::null_mut(), class.as_ptr()),
                (instance.as_ptr(), ptr::null_mut()),
            ] {
                let bound = bind_native_descriptor(classmethod.as_ptr(), receiver, owner);
                assert!(!bound.as_ptr().is_null(), "{}", native_error_description());
                let actual = OwnedPyObject::from_owned(object::PyObject_GetAttrString(
                    bound.as_ptr(),
                    c"__self__".as_ptr(),
                ));
                assert_eq!(actual.as_ptr(), class.as_ptr());
            }

            let anonymous = function(py, length_seventeen as *const (), 1);
            assert!(crate::builtins::functions::native_callable::configure_native_callable(
                py, obj_from_bits(anonymous).as_ptr().unwrap(),
                crate::builtins::functions::native_callable::NativeCallableSpec::uncached_function(),
            ));
            let anonymous =
                OwnedPyObject::from_owned(GLOBAL_BRIDGE.owned_handle_to_pyobj(anonymous));
            let rendered = OwnedPyObject::from_owned(typeobj::PyObject_Repr(anonymous.as_ptr()));
            assert!(
                !rendered.as_ptr().is_null(),
                "{}",
                native_error_description()
            );
            let text = strings::PyUnicode_AsUTF8(rendered.as_ptr());
            assert!(!text.is_null());
            assert_eq!(
                std::ffi::CStr::from_ptr(text).to_bytes(),
                b"<built-in function>"
            );
            assert!(errors::PyErr_Occurred().is_null());
        }
    });
}

thread_local! {
    static DIAGNOSTIC_IDENTITY_CALLS: Cell<usize> = const { Cell::new(0) };
}

unsafe extern "C" fn diagnostic_identity_trap(_: *mut PyObject, _: *mut PyObject) -> *mut PyObject {
    DIAGNOSTIC_IDENTITY_CALLS.with(|calls| calls.set(calls.get() + 1));
    unsafe {
        errors::PyErr_SetString(
            (&raw mut PyExc_RuntimeError).cast(),
            c"identity hook ran".as_ptr(),
        )
    };
    ptr::null_mut()
}

#[test]
fn native_descriptor_diagnostics_use_semantic_receiver_types_without_hooks() {
    use crate::cpython_abi_hooks::native_test_fixture::NativeType;
    let _transaction = crate::test_support::RuntimeTestTransaction::new();
    crate::with_gil_entry_nopanic!(py, {
        unsafe {
            let descriptor =
                published_native_descriptor(crate::builtin_classes(py).dict, c"__len__");
            let subclass =
                native_protocol_subclass(py, b"DiagnosticTuple", crate::builtin_classes(py).tuple);
            crate::object::class_finish_definition(py, obj_from_bits(subclass).as_ptr().unwrap())
                .unwrap();
            let class = OwnedPyObject::from_owned(GLOBAL_BRIDGE.owned_handle_to_pyobj(subclass));
            let tuple = OwnedPyObject::from_owned(object::PyObject_CallNoArgs(class.as_ptr()));
            assert!(!tuple.as_ptr().is_null(), "{}", native_error_description());
            assert_eq!(sequences::PyTuple_Check(tuple.as_ptr()), 1);

            let mut native = NativeType::<PyTypeObject>::subtype(
                &raw mut PyBaseObject_Type,
                c"diagnostic.NativeReceiver",
            );
            native.tp_basicsize = std::mem::size_of::<PyObject>() as isize;
            native.tp_getattro = Some(diagnostic_identity_trap);
            assert_eq!(native.ready(), 0);
            let type_references = native.ob_base.ob_base.ob_refcnt;
            let mut receiver = PyObject {
                ob_refcnt: 1,
                ob_type: &raw mut *native,
            };
            DIAGNOSTIC_IDENTITY_CALLS.with(|calls| calls.set(0));
            for (value, name) in [
                (tuple.as_ptr(), "DiagnosticTuple"),
                (&raw mut receiver, "diagnostic.NativeReceiver"),
            ] {
                let result = OwnedPyObject::from_owned(object::PyObject_CallOneArg(
                    descriptor.as_ptr(),
                    value,
                ));
                assert!(result.as_ptr().is_null());
                assert_type_error(
                    -1,
                    &format!(
                        "descriptor '__len__' requires a 'dict' object but received a '{name}'"
                    ),
                );
                let result = bind_native_descriptor(descriptor.as_ptr(), value, ptr::null_mut());
                assert!(result.as_ptr().is_null());
                assert_type_error(
                    -1,
                    &format!(
                        "descriptor '__len__' for 'dict' objects doesn't apply to a '{name}' object"
                    ),
                );
            }
            assert_eq!(DIAGNOSTIC_IDENTITY_CALLS.with(Cell::get), 0);
            assert_eq!(
                receiver.ob_refcnt, 1,
                "diagnostics retire temporary foreign receiver owners"
            );
            assert_eq!(
                native.ob_base.ob_base.ob_refcnt, type_references,
                "diagnostics retire temporary foreign type owners"
            );
            assert!(errors::PyErr_Occurred().is_null());
        }
    });
}

#[test]
fn native_factory_classes_finish_before_projection_and_first_instance() {
    use crate::object::class_storage::ClassReferenceSlot;
    let _transaction = crate::test_support::RuntimeTestTransaction::new();
    crate::with_gil_entry_nopanic!(py, {
        unsafe {
            // Bootstrap binds native exception shells. Every schema factory
            // must already be sealed, before allocating an exception instance.
            for spec in molt_obj_model::builtin_exception_specs() {
                let class =
                    crate::builtins::exceptions::exception_type_bits_from_name(py, spec.name());
                let pointer = obj_from_bits(class).as_ptr().expect("exception class");
                assert!(
                    crate::object::class_definition_is_finished(pointer),
                    "{}",
                    spec.name()
                );
                assert_ne!(
                    ClassReferenceSlot::CreationDoc.load(pointer),
                    0,
                    "{}",
                    spec.name()
                );
            }
            // The same cache also owns synthetic exception classes, whose
            // first C view cannot be satisfied by a prebound process shell.
            let exception = crate::builtins::exceptions::exception_type_bits_from_name(
                py,
                "ColdFactoryProjectionError",
            );
            let repeat = crate::molt_itertools_repeat_type();
            assert!(
                !crate::exception_pending(py),
                "{}",
                native_error_description()
            );
            let integer = OwnedPyObject::from_owned(numbers::PyLong_FromLong(17));
            for (class, iterator) in [(exception, false), (repeat, true)] {
                let pointer = obj_from_bits(class).as_ptr().expect("factory class");
                assert!(crate::object::class_definition_is_finished(pointer));
                assert_ne!(ClassReferenceSlot::CreationDoc.load(pointer), 0);
                assert!(!GLOBAL_BRIDGE.type_has_projection(class));
                let view =
                    OwnedPyObject::from_owned(GLOBAL_BRIDGE.borrowed_handle_to_new_pyobj(class));
                assert!(!view.as_ptr().is_null(), "{}", native_error_description());
                let value = OwnedPyObject::from_owned(if iterator {
                    object::PyObject_CallOneArg(view.as_ptr(), integer.as_ptr())
                } else {
                    object::PyObject_CallNoArgs(view.as_ptr())
                });
                assert!(!value.as_ptr().is_null(), "{}", native_error_description());
                let actual = OwnedPyObject::from_owned(typeobj::PyObject_Type(value.as_ptr()));
                assert_eq!(actual.as_ptr(), view.as_ptr());
                if iterator {
                    let iter = OwnedPyObject::from_owned(object::PyObject_GetIter(value.as_ptr()));
                    assert_eq!(iter.as_ptr(), value.as_ptr());
                    let item = OwnedPyObject::from_owned(object::PyIter_Next(iter.as_ptr()));
                    assert!(!item.as_ptr().is_null(), "{}", native_error_description());
                    assert_eq!(numbers::PyLong_AsLong(item.as_ptr()), 17);
                } else {
                    assert_eq!(
                        errors::PyErr_GivenExceptionMatches(value.as_ptr(), view.as_ptr()),
                        1
                    );
                }
                assert!(
                    errors::PyErr_Occurred().is_null(),
                    "{}",
                    native_error_description()
                );
            }
            // The exception cache returns a borrow; the public repeat export
            // returns an owner. Release exactly the latter caller reference.
            dec_ref_bits(py, repeat);
        }
    });
}
