//! Real consumers of cold managed class projections and their native children.
use super::*;

#[test]
fn cold_type_doc_slot_borrows_creation_doc_without_an_additional_owner() {
    use crate::object::class_storage::ClassReferenceSlot;
    let _transaction = crate::test_support::RuntimeTestTransaction::new();
    crate::with_gil_entry_nopanic!(py, {
        unsafe {
            for has_birth_doc in [true, false] {
                let name = crate::attr_name_bits_from_bytes(py, b"ColdCreationDoc").unwrap();
                let key = crate::attr_name_bits_from_bytes(py, b"__doc__").unwrap();
                let birth_doc = MoltObject::from_ptr(crate::alloc_string(
                    py,
                    b"ColdCreationDoc(value)\n--\n\nBirth documentation.\0discarded",
                ))
                .bits();
                let class = crate::molt_class_new(name);
                crate::molt_class_set_base(class, crate::builtin_classes(py).object);
                if has_birth_doc {
                    member(py, class, b"__doc__", birth_doc);
                }
                let class_pointer = obj_from_bits(class).as_ptr().unwrap();
                crate::object::class_finish_definition(py, class_pointer).unwrap();
                let captured = ClassReferenceSlot::CreationDoc.load(class_pointer);
                let captured_pointer = obj_from_bits(captured).as_ptr();
                let before = captured_pointer
                    .map(|doc| (*crate::header_from_obj_ptr(doc)).ref_count_snapshot());
                let late_doc = MoltObject::from_ptr(crate::alloc_string(
                    py,
                    b"Changed before cold projection.",
                ))
                .bits();
                crate::molt_set_attr_name(class, key, late_doc);
                dec_ref_bits(py, birth_doc);
                assert!(!GLOBAL_BRIDGE.type_has_projection(class));
                let view =
                    OwnedPyObject::from_owned(GLOBAL_BRIDGE.borrowed_handle_to_new_pyobj(class));
                assert!(!view.as_ptr().is_null());
                let tp = view.as_ptr().cast::<PyTypeObject>();
                let doc = typeobj::PyType_GetSlot(tp, slots::Py_tp_doc).cast::<std::ffi::c_char>();
                if let Some(captured_pointer) = captured_pointer {
                    assert_eq!(
                        doc.cast_const().cast::<u8>(),
                        crate::string_bytes(captured_pointer)
                    );
                    assert_eq!(
                        std::ffi::CStr::from_ptr(doc).to_bytes(),
                        b"ColdCreationDoc(value)\n--\n\nBirth documentation."
                    );
                    assert_eq!(
                        Some((*crate::header_from_obj_ptr(captured_pointer)).ref_count_snapshot()),
                        before
                    );
                } else {
                    assert!(
                        doc.is_null(),
                        "captured None must not adopt a later __doc__"
                    );
                }
                crate::molt_set_attr_name(class, key, MoltObject::from_int(29).bits());
                assert_eq!(typeobj::PyType_Ready(tp), 0);
                assert_eq!(typeobj::PyType_GetSlot(tp, slots::Py_tp_doc), doc.cast());

                // A direct native alias has its own C declaration. Projection
                // of the same runtime namespace does not replace its storage.
                let mut alias = NativeType::<PyTypeObject>::subtype(
                    &raw mut PyBaseObject_Type,
                    c"readiness.NativeDocAlias",
                );
                alias.tp_doc = c"Native declaration.".as_ptr();
                let alias_pointer = &raw mut *alias;
                GLOBAL_BRIDGE
                    .bind_static_pyobj_to_runtime_handle(alias_pointer.cast(), class, false)
                    .unwrap();
                assert_eq!(typeobj::PyType_Ready(alias_pointer), 0);
                assert_eq!(
                    std::ffi::CStr::from_ptr(
                        typeobj::PyType_GetSlot(alias_pointer, slots::Py_tp_doc).cast()
                    )
                    .to_bytes(),
                    b"Native declaration."
                );
                assert!(
                    GLOBAL_BRIDGE
                        .unbind_static_pyobj_from_runtime_handle(alias_pointer.cast(), class)
                );
                drop(alias);
                dec_ref_bits(py, class);
                if let Some(captured_pointer) = captured_pointer {
                    // Only the existing view/class lifetime anchors these bytes.
                    assert_eq!(
                        std::ffi::CStr::from_ptr(doc).to_bytes(),
                        b"ColdCreationDoc(value)\n--\n\nBirth documentation."
                    );
                    assert_eq!(
                        Some((*crate::header_from_obj_ptr(captured_pointer)).ref_count_snapshot()),
                        before
                    );
                }
                for bits in [late_doc, key, name] {
                    dec_ref_bits(py, bits);
                }
                assert!(errors::PyErr_Occurred().is_null());
            }
        }
    });
}

unsafe fn sequence_length_slot(tp: *mut PyTypeObject, receiver: *mut PyObject) -> isize {
    unsafe {
        let slot = typeobj::PyType_GetSlot(tp, slots::Py_sq_length);
        assert!(
            !slot.is_null(),
            "cold projection must expose its existing __len__"
        );
        let length: unsafe extern "C" fn(*mut PyObject) -> isize = std::mem::transmute(slot);
        let result = length(receiver);
        if result < 0 {
            eprintln!("native length failed: {}", native_error_description());
        }
        result
    }
}

#[test]
fn cold_managed_readiness_exposes_existing_slots_and_tracks_mutation() {
    let _transaction = crate::test_support::RuntimeTestTransaction::new();
    crate::with_gil_entry_nopanic!(py, {
        unsafe {
            let class = snapshot_class(py, b"ColdPhysicalSlots");
            let key = crate::attr_name_bits_from_bytes(py, b"__len__").unwrap();
            let seventeen = function(py, length_seventeen as *const (), 1);
            let twenty_three = function(py, length_twenty_three as *const (), 1);
            crate::molt_set_attr_name(class, key, seventeen);
            assert!(!GLOBAL_BRIDGE.type_has_projection(class));
            let namespace = crate::class_dict_bits(obj_from_bits(class).as_ptr().unwrap());
            let namespace_size = crate::dict_len(obj_from_bits(namespace).as_ptr().unwrap());
            let view = OwnedPyObject::from_owned(GLOBAL_BRIDGE.borrowed_handle_to_new_pyobj(class));
            assert!(!view.as_ptr().is_null());
            let tp = view.as_ptr().cast::<PyTypeObject>();
            assert_ne!((*tp).tp_flags & Py_TPFLAGS_READY, 0);
            assert_eq!((*tp).tp_flags & Py_TPFLAGS_READYING, 0);
            assert_eq!(
                crate::dict_len(obj_from_bits(namespace).as_ptr().unwrap()),
                namespace_size
            );
            assert_eq!(
                GLOBAL_BRIDGE
                    .molt_handle_for_pyobj((*tp).tp_dict)
                    .unwrap()
                    .bits(),
                namespace
            );
            assert_eq!(typeobj::PyType_Ready(tp), 0);

            let runtime_instance = snapshot_receiver(py, class, false);
            let managed = OwnedPyObject::from_owned(
                GLOBAL_BRIDGE.borrowed_handle_to_new_pyobj(runtime_instance),
            );
            let child = native_class(c"readiness.ColdPhysicalChild", view.as_ptr(), vec![]);
            let native = OwnedPyObject::from_owned(object::PyObject_CallNoArgs(child.as_ptr()));
            assert!(!native.as_ptr().is_null());
            for receiver in [managed.as_ptr(), native.as_ptr()] {
                assert_eq!(sequence_length_slot(tp, receiver), 17);
                assert_eq!(
                    molt_cpython_abi::api::abstract_sequence::PySequence_Size(receiver),
                    17
                );
                assert_eq!(
                    molt_cpython_abi::api::abstract_mapping::PyMapping_Size(receiver),
                    17,
                    "{}",
                    native_error_description()
                );
            }

            crate::molt_set_attr_name(class, key, twenty_three);
            for receiver in [managed.as_ptr(), native.as_ptr()] {
                assert_eq!(sequence_length_slot(tp, receiver), 23);
                assert_eq!(
                    molt_cpython_abi::api::abstract_sequence::PySequence_Size(receiver),
                    23
                );
                assert_eq!(
                    molt_cpython_abi::api::abstract_mapping::PyMapping_Size(receiver),
                    23,
                    "{}",
                    native_error_description()
                );
            }
            crate::molt_del_attr_name(class, key);
            assert!(typeobj::PyType_GetSlot(tp, slots::Py_sq_length).is_null());
            assert!(typeobj::PyType_GetSlot(child.as_ptr().cast(), slots::Py_sq_length).is_null());
            assert_eq!(
                molt_cpython_abi::api::abstract_sequence::PySequence_Size(native.as_ptr()),
                -1
            );
            assert_ne!(
                errors::PyErr_ExceptionMatches((&raw mut PyExc_TypeError).cast()),
                0
            );
            errors::PyErr_Clear();
            crate::molt_set_attr_name(class, key, seventeen);
            assert_eq!(sequence_length_slot(tp, managed.as_ptr()), 17);
            assert_eq!(
                molt_cpython_abi::api::abstract_sequence::PySequence_Size(native.as_ptr()),
                17
            );
            for bits in [runtime_instance, twenty_three, seventeen, key, class] {
                dec_ref_bits(py, bits);
            }
            assert!(errors::PyErr_Occurred().is_null());
        }
    });
}

unsafe extern "C" fn count_class_edge(object: *mut PyObject, state: *mut std::ffi::c_void) -> i32 {
    let state = unsafe { &mut *state.cast::<(*mut PyObject, usize)>() };
    if object == state.0 {
        state.1 += 1;
    }
    0
}

#[test]
fn native_instance_through_managed_heap_bases_visits_and_releases_one_class_edge() {
    let _transaction = crate::test_support::RuntimeTestTransaction::new();
    crate::with_gil_entry_nopanic!(py, {
        unsafe {
            let class = snapshot_class(py, b"PhysicalLifecycleBase");
            let base = OwnedPyObject::from_owned(GLOBAL_BRIDGE.borrowed_handle_to_new_pyobj(class));
            let child = native_class(c"readiness.LifecycleChild", base.as_ptr(), vec![]);
            let grandchild = native_class(c"readiness.LifecycleGrandchild", child.as_ptr(), vec![]);
            for class_view in [&child, &grandchild] {
                let tp = class_view.as_ptr().cast::<PyTypeObject>();
                assert_ne!((*tp).tp_flags & Py_TPFLAGS_HAVE_GC, 0);
                let before = (*class_view.as_ptr()).ob_refcnt;
                let instance = OwnedPyObject::from_owned(typeobj::PyType_GenericAlloc(tp, 0));
                assert!(!instance.as_ptr().is_null());
                assert_eq!((*class_view.as_ptr()).ob_refcnt, before + 1);
                let mut visit = (class_view.as_ptr(), 0usize);
                assert_eq!(
                    (*tp).tp_traverse.unwrap()(
                        instance.as_ptr(),
                        count_class_edge as *const () as *mut std::ffi::c_void,
                        (&raw mut visit).cast(),
                    ),
                    0
                );
                assert_eq!(visit.1, 1);
                assert_eq!((*tp).tp_clear.unwrap()(instance.as_ptr()), 0);
                drop(instance);
                assert_eq!((*class_view.as_ptr()).ob_refcnt, before);
            }
            dec_ref_bits(py, class);
            assert!(errors::PyErr_Occurred().is_null());
        }
    });
}

#[test]
fn mro_first_publication_returns_a_physically_ready_managed_class() {
    let _transaction = crate::test_support::RuntimeTestTransaction::new();
    crate::with_gil_entry_nopanic!(py, {
        unsafe {
            let class = snapshot_class(py, b"MroFirstPhysicalClass");
            let key = crate::attr_name_bits_from_bytes(py, b"__len__").unwrap();
            let seventeen = function(py, length_seventeen as *const (), 1);
            crate::molt_set_attr_name(class, key, seventeen);
            assert!(!GLOBAL_BRIDGE.type_has_projection(class));
            let result = (molt_cpython_abi::hooks::hooks_or_stubs().type_metadata)(
                class,
                molt_cpython_abi::hooks::TypeMetadataField::Mro,
            );
            let mro = OwnedPyObject::from_owned(GLOBAL_BRIDGE.owned_result_to_pyobj(result));
            assert!(!mro.as_ptr().is_null());
            let tp = sequences::PyTuple_GetItem(mro.as_ptr(), 0).cast::<PyTypeObject>();
            assert!(!tp.is_null());
            assert_eq!((*tp).tp_mro, mro.as_ptr());
            assert_ne!((*tp).tp_flags & Py_TPFLAGS_READY, 0);
            assert_eq!((*tp).tp_flags & Py_TPFLAGS_READYING, 0);
            assert!((*tp).tp_dealloc.is_some());
            let child = native_class(c"readiness.MroFirstChild", tp.cast(), vec![]);
            let instance = OwnedPyObject::from_owned(object::PyObject_CallNoArgs(child.as_ptr()));
            assert!(!instance.as_ptr().is_null());
            assert_eq!(
                molt_cpython_abi::api::abstract_sequence::PySequence_Size(instance.as_ptr()),
                17
            );
            for bits in [seventeen, key, class] {
                dec_ref_bits(py, bits);
            }
            assert!(errors::PyErr_Occurred().is_null());
        }
    });
}

#[test]
fn static_managed_protocol_tables_dispatch_without_overwriting_object() {
    let _transaction = crate::test_support::RuntimeTestTransaction::new();
    crate::with_gil_entry_nopanic!(py, {
        unsafe {
            let class = crate::builtins::types::mappingproxy_class(py);
            let object_type = &raw mut PyBaseObject_Type;
            let object_mapping = (*object_type).tp_as_mapping;
            let view = OwnedPyObject::from_owned(GLOBAL_BRIDGE.borrowed_handle_to_new_pyobj(class));
            assert!(!view.as_ptr().is_null());
            let tp = view.as_ptr().cast::<PyTypeObject>();
            assert_eq!(tp, &raw mut PyDictProxy_Type);
            assert_eq!((*tp).tp_flags & Py_TPFLAGS_HEAPTYPE, 0);
            assert_ne!((*tp).tp_flags & Py_TPFLAGS_READY, 0);
            assert!(!(*tp).tp_as_mapping.is_null());
            assert!(!(*tp).tp_as_sequence.is_null());
            assert!(!(*tp).tp_as_number.is_null());
            let namespace = crate::class_dict_bits(obj_from_bits(class).as_ptr().unwrap());
            assert_eq!(
                GLOBAL_BRIDGE
                    .molt_handle_for_pyobj((*tp).tp_dict)
                    .unwrap()
                    .bits(),
                namespace
            );
            let before = crate::dict_len(obj_from_bits(namespace).as_ptr().unwrap());
            assert_eq!(
                typeobj::PyType_Ready(tp),
                0,
                "{}",
                native_error_description()
            );
            assert_eq!(
                crate::dict_len(obj_from_bits(namespace).as_ptr().unwrap()),
                before
            );
            assert_ne!((*tp).tp_as_mapping, object_mapping);
            assert_eq!((*object_type).tp_as_mapping, object_mapping);
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
            let proxy = crate::builtins::types::mappingproxy_from_mapping(py, dictionary_bits);
            let receiver = OwnedPyObject::from_owned(GLOBAL_BRIDGE.owned_handle_to_pyobj(proxy));
            let slot = typeobj::PyType_GetSlot(tp, slots::Py_mp_length);
            assert!(!slot.is_null());
            let length: unsafe extern "C" fn(*mut PyObject) -> isize = std::mem::transmute(slot);
            assert_eq!(
                length(receiver.as_ptr()),
                1,
                "{}",
                native_error_description()
            );
            assert_eq!(
                molt_cpython_abi::api::abstract_mapping::PyMapping_Size(receiver.as_ptr()),
                1,
                "{}",
                native_error_description()
            );
            let key = OwnedPyObject::from_owned(strings::PyUnicode_FromString(c"one".as_ptr()));
            let slot = typeobj::PyType_GetSlot(tp, slots::Py_mp_subscript);
            assert!(!slot.is_null());
            let subscript: unsafe extern "C" fn(*mut PyObject, *mut PyObject) -> *mut PyObject =
                std::mem::transmute(slot);
            let value = OwnedPyObject::from_owned(subscript(receiver.as_ptr(), key.as_ptr()));
            assert_eq!(
                value.as_ptr(),
                &raw mut Py_None,
                "{}",
                native_error_description()
            );
            let slot = typeobj::PyType_GetSlot(tp, slots::Py_sq_contains);
            assert!(!slot.is_null());
            let contains: unsafe extern "C" fn(*mut PyObject, *mut PyObject) -> i32 =
                std::mem::transmute(slot);
            assert_eq!(
                contains(receiver.as_ptr(), key.as_ptr()),
                1,
                "{}",
                native_error_description()
            );
            assert_eq!((*object_type).tp_as_mapping, object_mapping);
            assert!(errors::PyErr_Occurred().is_null());
        }
    });
}

#[test]
fn managed_container_base_preserves_native_gc_storage_rejection() {
    let _transaction = crate::test_support::RuntimeTestTransaction::new();
    crate::with_gil_entry_nopanic!(py, {
        unsafe {
            let name = crate::attr_name_bits_from_bytes(py, b"ManagedListStorage").unwrap();
            let class = crate::molt_class_new(name);
            dec_ref_bits(py, name);
            crate::molt_class_set_base(class, crate::builtin_classes(py).list);
            crate::object::class_finish_definition(py, obj_from_bits(class).as_ptr().unwrap())
                .unwrap();
            let view = OwnedPyObject::from_owned(GLOBAL_BRIDGE.borrowed_handle_to_new_pyobj(class));
            assert!(!view.as_ptr().is_null());
            let tp = view.as_ptr().cast::<PyTypeObject>();
            assert_ne!((*tp).tp_flags & Py_TPFLAGS_HAVE_GC, 0);
            assert!((*tp).tp_traverse.is_some());
            assert!((*tp).tp_clear.is_some());
            assert!(typeobj::PyType_GenericAlloc(tp, 0).is_null());
            let error = OwnedPyObject::from_owned(errors::PyErr_GetRaisedException());
            assert!(!error.as_ptr().is_null());
            let message = OwnedPyObject::from_owned(typeobj::PyObject_Str(error.as_ptr()));
            let text = std::ffi::CStr::from_ptr(strings::PyUnicode_AsUTF8(message.as_ptr()))
                .to_string_lossy();
            assert!(text.contains("managed builtin GC storage slots"));
            dec_ref_bits(py, class);
            assert!(errors::PyErr_Occurred().is_null());
        }
    });
}

#[path = "protocol_capabilities.rs"]
mod protocol_capabilities;
