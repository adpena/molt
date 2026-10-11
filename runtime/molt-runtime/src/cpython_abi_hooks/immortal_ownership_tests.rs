//! Public C ownership queries observe both runtime and direct C owners.

use super::*;
use molt_cpython_abi::abi_types::IMMORTAL_REFCNT;
use molt_cpython_abi::api::{
    errors, mapping, memory, numbers, object, refcount, sequences, strings,
};
use molt_cpython_abi::bridge::GLOBAL_BRIDGE;

unsafe fn assert_unique(view: *mut PyObject, expected: c_int) {
    assert_eq!(
        unsafe { object::PyUnstable_Object_IsUniquelyReferenced(view) },
        expected
    );
    assert_eq!(
        unsafe { object::PyUnstable_Object_IsUniqueReferencedTemporary(view) },
        0,
        "unique ownership does not prove temporary argument provenance"
    );
}

#[test]
fn managed_unique_reference_queries_count_runtime_and_c_owners() {
    let _transaction = crate::test_support::RuntimeTestTransaction::new();
    with_gil(|py| unsafe {
        let bits = MoltObject::from_ptr(crate::alloc_list(&py, &[])).bits();
        let view = GLOBAL_BRIDGE.handle_to_borrowed_pyobj(bits);
        assert!(!view.is_null());
        assert_eq!(hook_ref_count(bits), 2);
        assert_eq!((*view).ob_refcnt, 1);
        assert_unique(view, 1);

        inc_ref_bits(&py, bits);
        assert_eq!(hook_ref_count(bits), 3);
        assert_eq!((*view).ob_refcnt, 1, "the C header is still only a bias");
        assert_unique(view, 0);
        assert_eq!(object::PyUnstable_SetImmortal(view), 0);
        assert_eq!((*view).ob_refcnt, 1);
        dec_ref_bits(&py, bits);
        assert_unique(view, 1);

        let direct = GLOBAL_BRIDGE.borrowed_handle_to_new_pyobj(bits);
        assert_eq!(direct, view);
        assert_eq!(hook_ref_count(bits), 2);
        assert_unique(view, 0);
        refcount::Py_DECREF(direct);
        assert_unique(view, 1);

        // Transfer the caller's runtime owner to one direct C reference.
        let owned = GLOBAL_BRIDGE.owned_handle_to_pyobj(bits);
        assert_eq!(owned, view);
        assert_eq!(hook_ref_count(bits), 1);
        assert_eq!((*view).ob_refcnt, 1);
        assert_unique(view, 1);
        refcount::Py_INCREF(view);
        assert_unique(view, 0);
        assert_eq!(object::PyUnstable_SetImmortal(view), 0);
        refcount::Py_DECREF(view);
        assert_unique(view, 1);
        refcount::Py_DECREF(owned);
        assert!(errors::PyErr_Occurred().is_null());
        assert!(!crate::exception_pending(&py));
    });
}

#[test]
fn managed_unique_reference_queries_discount_only_mirrored_c_owners() {
    let _transaction = crate::test_support::RuntimeTestTransaction::new();
    with_gil(|py| unsafe {
        let child = MoltObject::from_ptr(crate::alloc_list(&py, &[])).bits();
        let parent = MoltObject::from_ptr(crate::alloc_list(&py, &[child])).bits();
        let child_view = GLOBAL_BRIDGE.handle_to_borrowed_pyobj(child);
        let parent_view = GLOBAL_BRIDGE.owned_handle_to_pyobj(parent);
        assert!(!child_view.is_null() && !parent_view.is_null());
        dec_ref_bits(&py, child);
        assert_eq!(hook_ref_count(child), 2);
        assert_eq!(GLOBAL_BRIDGE.mirrored_c_refcount(child_view.addr()), 1);
        assert_eq!((*child_view).ob_refcnt, 2);
        assert_unique(child_view, 1);

        let direct = GLOBAL_BRIDGE.borrowed_handle_to_new_pyobj(child);
        assert_unique(child_view, 0);
        refcount::Py_DECREF(direct);
        assert_unique(child_view, 1);
        refcount::Py_DECREF(parent_view);
        assert!(errors::PyErr_Occurred().is_null());
        assert!(!crate::exception_pending(&py));
    });
}

#[test]
fn immortal_promotion_rejects_a_unique_managed_unicode_without_error() {
    let _transaction = crate::test_support::RuntimeTestTransaction::new();
    with_gil(|py| unsafe {
        let value =
            crate::object::builders::alloc_string_nointern(&py, b"unique non-interned text!");
        assert!(!value.is_null());
        let bits = MoltObject::from_ptr(value).bits();
        let view = GLOBAL_BRIDGE.owned_handle_to_pyobj(bits);
        assert!(!view.is_null());
        assert_eq!(strings::PyUnicode_Check(view), 1);
        assert_eq!(hook_ref_count(bits), 1);
        assert_unique(view, 1);
        assert_eq!(object::PyUnstable_SetImmortal(view), 0);
        assert_eq!((*view).ob_refcnt, 1);
        refcount::Py_DECREF(view);
        assert!(errors::PyErr_Occurred().is_null());
        assert!(!crate::exception_pending(&py));
    });
}

#[test]
#[ignore = "retains process-lifetime C roots; run alone in a fresh test process"]
fn immortal_c_views_preserve_runtime_roots_and_gc() {
    let _transaction = crate::test_support::RuntimeTestTransaction::new();
    with_gil(|py| unsafe {
        for route in 0..3 {
            let view = sequences::PyList_New(0);
            assert!(!view.is_null());
            let bits = GLOBAL_BRIDGE
                .managed_handle_for_pyobj(view)
                .expect("list has a canonical managed view");
            let runtime = MoltObject::from_bits(bits).as_ptr().unwrap();
            assert_eq!(
                memory::PyObject_GC_IsTracked(view) != 0,
                crate::object::gc::gc_is_tracked(runtime)
            );
            memory::PyObject_GC_UnTrack(view.cast());
            assert!(!crate::object::gc::gc_is_tracked(runtime));
            memory::PyObject_GC_Track(view.cast());
            assert!(crate::object::gc::gc_is_tracked(runtime));
            assert_eq!(memory::PyObject_GC_IsTracked(view), 1);

            // Raw header promotion also covers a pre-existing mirror ledger.
            let mirror_owner = if route == 2 {
                let owner = MoltObject::from_ptr(crate::alloc_list(&py, &[bits])).bits();
                let owner_view = GLOBAL_BRIDGE.owned_handle_to_pyobj(owner);
                assert!(!owner_view.is_null());
                assert_eq!(GLOBAL_BRIDGE.mirrored_c_refcount(view.addr()), 1);
                Some(owner_view)
            } else {
                None
            };
            match route {
                0 => {
                    assert_eq!(object::PyUnstable_SetImmortal(view), 1);
                    assert!(!crate::object::gc::gc_is_tracked(runtime));
                    assert_eq!(memory::PyObject_GC_IsTracked(view), 0);
                }
                1 => molt_cpython_abi::bridge::molt_capi_set_refcnt(view, IMMORTAL_REFCNT),
                _ => (*view).ob_refcnt = IMMORTAL_REFCNT,
            }
            assert_eq!((*view).ob_refcnt, IMMORTAL_REFCNT);
            assert_eq!(
                (*header_from_obj_ptr(runtime)).load_synchronized_flags()
                    & crate::object::HEADER_FLAG_IMMORTAL,
                0,
                "C immortality retains a bridge root; it does not claim a canonical runtime owner"
            );
            assert_unique(view, 0);
            assert_eq!(object::PyUnstable_SetImmortal(view), 0);
            assert!(GLOBAL_BRIDGE.has_direct_c_refs(bits));
            assert_eq!(GLOBAL_BRIDGE.gc_ref_adjustment(bits), 0);

            if let Some(owner) = mirror_owner {
                refcount::Py_DECREF(owner);
                assert_eq!(GLOBAL_BRIDGE.mirrored_c_refcount(view.addr()), 0);
            }
            assert_eq!(hook_ref_count(bits), 1);
            refcount::Py_INCREF(view);
            refcount::Py_DECREF(view);
            molt_cpython_abi::bridge::molt_capi_set_refcnt(view, 1);
            assert_eq!((*view).ob_refcnt, IMMORTAL_REFCNT);

            assert_eq!(sequences::PyList_Append(view, view), 0);
            let collection = crate::object::gc::collect_cycles(&py);
            assert_eq!(
                collection.status,
                crate::object::gc::GcCollectStatus::Completed
            );
            assert_eq!(GLOBAL_BRIDGE.handle_to_borrowed_pyobj(bits), view);
            assert_eq!(sequences::PyList_Size(view), 1);
            crate::molt_list_clear(bits);
            assert_eq!(hook_ref_count(bits), 1);
            let retained = GLOBAL_BRIDGE.borrowed_handle_to_new_pyobj(bits);
            assert_eq!(retained, view);
            refcount::Py_DECREF(retained);
            assert_eq!(hook_ref_count(bits), 1);
            assert_eq!((*view).ob_refcnt, IMMORTAL_REFCNT);
            assert!(errors::PyErr_Occurred().is_null());
            assert!(!crate::exception_pending(&py));
            // The interpreter cannot revoke explicit C immortality. Its stable
            // bridge hold intentionally survives until this test process exits.
        }

        // Dict mutation normally promotes membership when a tracked child is
        // inserted. Explicit immortality must survive that automatic path.
        let dictionary = mapping::PyDict_New();
        assert!(!dictionary.is_null());
        let dict_bits = GLOBAL_BRIDGE
            .managed_handle_for_pyobj(dictionary)
            .expect("dictionary has a canonical managed view");
        let dict_runtime = MoltObject::from_bits(dict_bits).as_ptr().unwrap();
        assert_unique(dictionary, 1);
        assert_eq!(object::PyUnstable_SetImmortal(dictionary), 1);
        assert!(!crate::object::gc::gc_is_tracked(dict_runtime));
        assert_eq!(memory::PyObject_GC_IsTracked(dictionary), 0);

        let child = sequences::PyList_New(0);
        let key = numbers::PyLong_FromLong(7);
        assert!(!child.is_null() && !key.is_null());
        assert_eq!(memory::PyObject_GC_IsTracked(child), 1);
        assert_eq!(mapping::PyDict_SetItem(dictionary, key, child), 0);
        assert!(!crate::object::gc::gc_is_tracked(dict_runtime));
        assert_eq!(memory::PyObject_GC_IsTracked(dictionary), 0);
        memory::PyObject_GC_Track(dictionary.cast());
        assert!(!crate::object::gc::gc_is_tracked(dict_runtime));
        assert_eq!(memory::PyObject_GC_IsTracked(dictionary), 0);
        assert!(GLOBAL_BRIDGE.has_direct_c_refs(dict_bits));
        assert_eq!(GLOBAL_BRIDGE.gc_ref_adjustment(dict_bits), 0);

        mapping::PyDict_Clear(dictionary);
        refcount::Py_DECREF(child);
        refcount::Py_DECREF(key);
        assert_eq!(hook_ref_count(dict_bits), 1);
        assert_eq!((*dictionary).ob_refcnt, IMMORTAL_REFCNT);
        assert!(errors::PyErr_Occurred().is_null());
        assert!(!crate::exception_pending(&py));
    });
}

#[test]
fn canonical_special_singletons_share_native_value_and_type_identity() {
    use crate::builtins::exceptions::ExceptionValue;
    use molt_cpython_abi::abi_types::{
        Py_EllipsisObject, Py_NotImplementedSentinel, PyEllipsis_Type, PyExc_ValueError,
        PyNotImplemented_Type,
    };
    use molt_cpython_abi::api::{refcount::OwnedPyObject, typeobj};
    use molt_cpython_abi::hooks::DecodedHandleResult;

    let _transaction = crate::test_support::RuntimeTestTransaction::new();
    with_gil(|py| unsafe {
        let py = &py;
        errors::PyErr_SetString(
            (&raw mut PyExc_ValueError).cast(),
            c"unchanged across canonical singleton crossings".as_ptr(),
        );
        let prior = OwnedPyObject::from_owned(errors::PyErr_GetRaisedException());
        assert!(!prior.as_ptr().is_null());
        let builtins = crate::builtin_classes(py);
        for (bits, pointer, class, kind, type_id) in [
            (
                crate::not_implemented_bits(py),
                &raw mut Py_NotImplementedSentinel,
                builtins.not_implemented_type,
                &raw mut PyNotImplemented_Type,
                crate::TYPE_ID_NOT_IMPLEMENTED,
            ),
            (
                crate::ellipsis_bits(py),
                &raw mut Py_EllipsisObject,
                builtins.ellipsis_type,
                &raw mut PyEllipsis_Type,
                crate::TYPE_ID_ELLIPSIS,
            ),
        ] {
            assert_eq!((*pointer).ob_type, kind, "physical singleton type");
            let runtime = crate::obj_from_bits(bits).as_ptr().unwrap();
            assert_eq!(object_type_id(runtime), type_id);
            let native_refs = (*pointer).ob_refcnt;
            let runtime_refs = hook_ref_count(bits);
            for channel in 0..3 {
                if channel != 0 {
                    errors::PyErr_SetRaisedException(object::Py_NewRef(prior.as_ptr()));
                    if channel == 2 {
                        super::propagate_native_failure(py, "singleton test prior");
                        assert!(crate::exception_pending(py));
                    }
                }
                // Native callback-result ingress owns one reference. It must
                // select the canonical value, never allocate a FOREIGN wrapper.
                let result = molt_cpython_abi::bridge::owned_native_result_to_runtime(
                    object::Py_NewRef(pointer),
                );
                let DecodedHandleResult::Ok(received) = result.decode() else {
                    panic!("singleton ingress must succeed with an admitted prior error");
                };
                let received = ExceptionValue::adopt(py, received);
                assert_eq!(received.bits(), bits);
                assert_eq!(
                    object_type_id(crate::obj_from_bits(received.bits()).as_ptr().unwrap()),
                    type_id
                );
                let exported = OwnedPyObject::from_owned(
                    GLOBAL_BRIDGE.owned_handle_to_pyobj(received.into_bits()),
                );
                assert_eq!(exported.as_ptr(), pointer);
                let borrowed =
                    OwnedPyObject::from_owned(GLOBAL_BRIDGE.borrowed_handle_to_new_pyobj(bits));
                assert_eq!(borrowed.as_ptr(), pointer);
                assert_eq!(GLOBAL_BRIDGE.handle_to_borrowed_pyobj(bits), pointer);

                let native_class = OwnedPyObject::from_owned(typeobj::PyObject_Type(pointer));
                assert_eq!(native_class.as_ptr(), kind.cast());
                assert_eq!(typeobj::_Py_TYPE(pointer), kind);
                let received_class = ExceptionValue::adopt(
                    py,
                    GLOBAL_BRIDGE
                        .molt_value_for_pyobj(kind.cast())
                        .expect("native singleton type has its runtime class"),
                );
                assert_eq!(received_class.bits(), class);
                assert_eq!(GLOBAL_BRIDGE.handle_to_borrowed_pyobj(class), kind.cast());
                drop(received_class);
                drop(native_class);
                drop(borrowed);
                drop(exported);

                if channel == 0 {
                    assert!(errors::PyErr_Occurred().is_null());
                    assert!(!crate::exception_pending(py));
                } else {
                    let selected = OwnedPyObject::from_owned(errors::PyErr_GetRaisedException());
                    assert_eq!(selected.as_ptr(), prior.as_ptr());
                }
                assert_eq!((*pointer).ob_refcnt, native_refs);
                assert_eq!(hook_ref_count(bits), runtime_refs);
            }
            // The existing class constructor must return this same value through
            // ordinary public C call completion; no singleton-specific call path.
            let constructed = OwnedPyObject::from_owned(object::PyObject_CallNoArgs(kind.cast()));
            assert_eq!(constructed.as_ptr(), pointer);
            assert!(errors::PyErr_Occurred().is_null());
            assert!(!crate::exception_pending(py));
        }
    });
}
