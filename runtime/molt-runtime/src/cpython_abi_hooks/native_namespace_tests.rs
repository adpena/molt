//! Canonical native declarations across Python and C namespace consumers.
use super::*;
use molt_cpython_abi::api::{errors, mapping, refcount, strings, typeobj};

#[test]
fn eval_builtins_borrows_the_exact_captured_object_without_mapping_validation() {
    let _transaction = crate::test_support::RuntimeTestTransaction::new();
    with_gil(|py| unsafe {
        let dictionary = MoltObject::from_ptr(crate::alloc_dict_with_pairs(&py, &[])).bits();
        let mapping_candidate = MoltObject::from_ptr(crate::alloc_list(&py, &[])).bits();
        for bits in [
            dictionary,
            mapping_candidate,
            MoltObject::none().bits(),
            MoltObject::from_int(42).bits(),
        ] {
            crate::inc_ref_bits(&py, bits);
            crate::builtins::frames::frame_stack_push_owned(&py, 0, 0, bits, 0);
            let view = molt_cpython_abi::api::eval::PyEval_GetBuiltins();
            assert!(!view.is_null());
            assert_eq!(
                molt_cpython_abi::bridge::GLOBAL_BRIDGE
                    .observed_handle_for_pyobj(view)
                    .unwrap()
                    .bits(),
                bits
            );
            assert!(errors::PyErr_Occurred().is_null());
            assert!(!crate::exception_pending(&py));
            crate::builtins::frames::frame_stack_pop(&py);
        }
        crate::dec_ref_bits(&py, mapping_candidate);
        crate::dec_ref_bits(&py, dictionary);
    });
}

#[test]
fn native_type_namespace_projects_the_owner_dictionary_and_preserves_edits() {
    let _transaction = crate::test_support::RuntimeTestTransaction::new();
    with_gil(|py| unsafe {
        let classes = crate::builtin_classes(&py);
        let class = crate::obj_from_bits(classes.list).as_ptr().unwrap();
        let dictionary = crate::class_dict_bits(class);
        let dict = crate::obj_from_bits(dictionary).as_ptr().unwrap();
        let append_name = crate::attr_name_bits_from_bytes(&py, b"append").unwrap();
        let reverse_name = crate::attr_name_bits_from_bytes(&py, b"reverse").unwrap();
        let previous_reverse = crate::dict_get_in_place(&py, dict, reverse_name);
        let append =
            crate::builtins::attr::class_namespace_lookup_raw(&py, class, append_name).unwrap();
        assert_eq!(
            crate::dict_get_in_place(&py, dict, reverse_name),
            previous_reverse,
            "one ordinary lookup must not enumerate the owner's other declarations"
        );
        let c_name = strings::PyUnicode_FromString(c"append".as_ptr());
        let type_view = molt_cpython_abi::bridge::GLOBAL_BRIDGE
            .handle_to_borrowed_pyobj(classes.list)
            .cast::<PyTypeObject>();
        let raw = typeobj::_PyType_Lookup(type_view, c_name);
        assert_eq!(
            molt_cpython_abi::bridge::GLOBAL_BRIDGE
                .observed_handle_for_pyobj(raw)
                .unwrap()
                .bits(),
            append
        );
        assert_eq!(typeobj::PyType_Ready(type_view), 0);
        let namespace = (*type_view).tp_dict;
        assert_eq!(
            molt_cpython_abi::bridge::GLOBAL_BRIDGE
                .observed_handle_for_pyobj(namespace)
                .unwrap()
                .bits(),
            dictionary
        );
        assert_eq!(mapping::PyDict_GetItem(namespace, c_name), raw);
        // Keep the original descriptor while the admitted C dictionary drops
        // its owner; repeat readiness and enumeration must not recreate it.
        crate::inc_ref_bits(&py, append);
        assert_eq!(mapping::PyDict_DelItem(namespace, c_name), 0);
        assert_eq!(typeobj::PyType_Ready(type_view), 0);
        assert!(typeobj::_PyType_Lookup(type_view, c_name).is_null());
        assert!(errors::PyErr_Occurred().is_null());
        assert!(crate::builtins::methods::publish_builtin_class_methods(
            &py,
            classes.list
        ));
        assert!(
            crate::builtins::attr::class_namespace_lookup_raw(&py, class, append_name).is_none()
        );
        crate::dict_set_in_place(&py, dict, append_name, append);
        crate::dec_ref_bits(&py, append);
        refcount::Py_DECREF(c_name);
        crate::dec_ref_bits(&py, append_name);
        crate::dec_ref_bits(&py, reverse_name);
        assert!(!crate::exception_pending(&py));
    });
}

#[test]
fn managed_type_projection_owns_one_mirrored_dictionary_edge() {
    let _transaction = crate::test_support::RuntimeTestTransaction::with_gc_isolation();
    with_gil(|py| unsafe {
        let name = crate::attr_name_bits_from_bytes(&py, b"NativeNamespaceOwner").unwrap();
        let class = crate::molt_class_new(name);
        crate::dec_ref_bits(&py, name);
        crate::molt_class_set_base(class, crate::builtin_classes(&py).object);
        crate::object::class_finish_definition(&py, crate::obj_from_bits(class).as_ptr().unwrap())
            .expect("seal the namespace class through its canonical layout owner");
        assert!(!crate::exception_pending(&py));
        let class_ptr = crate::obj_from_bits(class).as_ptr().unwrap();
        let namespace = crate::class_dict_bits(class_ptr);
        let dictionary_view =
            molt_cpython_abi::bridge::GLOBAL_BRIDGE.handle_to_borrowed_pyobj(namespace);
        let before = (*dictionary_view).ob_refcnt;
        let view = molt_cpython_abi::bridge::GLOBAL_BRIDGE
            .handle_to_borrowed_pyobj(class)
            .cast::<PyTypeObject>();
        assert!(!view.is_null());
        assert_eq!((*view).tp_dict, dictionary_view);
        assert_eq!((*dictionary_view).ob_refcnt, before + 1);
        assert!(!molt_cpython_abi::bridge::GLOBAL_BRIDGE.has_direct_c_refs(namespace));
        assert_eq!(typeobj::PyType_Ready(view), 0);
        assert_eq!((*dictionary_view).ob_refcnt, before + 1);
        assert!(molt_cpython_abi::bridge::GLOBAL_BRIDGE.retire_runtime_type_views(&[class]));
        assert_eq!((*dictionary_view).ob_refcnt, before);
        assert!(
            molt_cpython_abi::bridge::GLOBAL_BRIDGE
                .managed_handle_for_pyobj(view.cast())
                .is_none()
        );
        crate::dec_ref_bits(&py, class);
        assert!(!crate::exception_pending(&py));
    });
}

#[test]
fn gc_retires_type_mro_projection_cycle_before_releasing_candidate_pins() {
    let _transaction = crate::test_support::RuntimeTestTransaction::with_gc_isolation();
    with_gil(|py| unsafe {
        let name = crate::attr_name_bits_from_bytes(&py, b"CollectedProjectionOwner").unwrap();
        let class = crate::molt_class_new(name);
        crate::dec_ref_bits(&py, name);
        let result = crate::molt_class_set_base(class, crate::builtin_classes(&py).object);
        crate::dec_ref_bits(&py, result);
        crate::object::class_finish_definition(&py, crate::obj_from_bits(class).as_ptr().unwrap())
            .expect("seal class before publishing its recursive C projection");
        let bridge = &molt_cpython_abi::bridge::GLOBAL_BRIDGE;
        let view = bridge
            .handle_to_borrowed_pyobj(class)
            .cast::<PyTypeObject>();
        assert!(!view.is_null());
        let mro = (*view).tp_mro;
        assert_eq!(
            molt_cpython_abi::api::sequences::PyTuple_GetItem(mro, 0),
            view.cast()
        );
        assert_eq!(bridge.mirrored_c_refcount(view.addr()), 1);
        assert!(!bridge.has_direct_c_refs(class));
        // A live C alias must preserve the entire semantic/projection cycle.
        let alias = molt_cpython_abi::api::sequences::PyTuple_New(1);
        assert!(!alias.is_null());
        molt_cpython_abi::api::refcount::Py_INCREF(mro);
        assert_eq!(
            molt_cpython_abi::api::sequences::PyTuple_SetItem(alias, 0, mro),
            0
        );
        crate::dec_ref_bits(&py, class);
        let reachable = crate::object::gc::collect_cycles(&py);
        assert_eq!(
            reachable.status,
            crate::object::gc::GcCollectStatus::Completed
        );
        assert_eq!(bridge.managed_handle_for_pyobj(view.cast()), Some(class));
        assert!(bridge.managed_handle_for_pyobj(mro).is_some());
        molt_cpython_abi::api::refcount::Py_DECREF(alias);
        let result = crate::object::gc::collect_cycles(&py);
        assert_eq!(result.status, crate::object::gc::GcCollectStatus::Completed);
        for pointer in [view.cast(), mro] {
            assert!(bridge.managed_handle_for_pyobj(pointer).is_none());
            assert_eq!(bridge.mirrored_c_refcount(pointer.addr()), 0);
        }
        assert!(!crate::exception_pending(&py));
    });
}

mod readiness;
