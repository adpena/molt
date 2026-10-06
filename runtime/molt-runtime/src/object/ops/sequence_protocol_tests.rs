use super::*;
use molt_cpython_abi::abi_types::{self, PyObject, PySequenceMethods, PyTypeObject};
use molt_cpython_abi::api::{errors, object, refcount};
use molt_cpython_abi::bridge::GLOBAL_BRIDGE;
use std::ffi::{c_int, c_void};

#[test]
fn sequence_slice_failed_publication_retains_error_and_original_storage() {
    let _transaction = crate::test_support::RuntimeTestTransaction::new();
    crate::with_gil(|py| unsafe {
        let item = MoltObject::from_int(7).bits();
        let list = crate::alloc_list(&py, &[item]);
        assert!(!list.is_null());
        let bits = MoltObject::from_ptr(list).bits();
        // A malformed physical projection must fail before publishing either
        // storage view and must report the hook's canonical -1 sentinel.
        let projection: [*mut PyObject; 0] = [];
        let status = (molt_cpython_abi::hooks::hooks_or_stubs().list_set_slice)(
            bits,
            0,
            1,
            &item,
            1,
            projection.as_ptr(),
            0,
        );
        assert_eq!(status, -1);
        let raised = crate::builtins::exceptions::molt_exception_last_pending();
        assert!(!obj_from_bits(raised).is_none());
        let raised_view = GLOBAL_BRIDGE.handle_to_borrowed_pyobj(raised);
        assert!(!raised_view.is_null());
        assert_eq!(crate::exception_last_bits_noinc(&py), Some(raised));
        assert_eq!(GLOBAL_BRIDGE.handle_to_borrowed_pyobj(raised), raised_view);
        assert_eq!(crate::exception_last_bits_noinc(&py), Some(raised));
        let observed = errors::PyErr_GetRaisedException();
        assert_eq!(observed, raised_view);
        refcount::Py_DECREF(observed);
        assert_eq!(crate::list_len(list), 1);
        assert_eq!((&*crate::seq_vec_ptr(list))[0], item);
        dec_ref_bits(&py, raised);
        dec_ref_bits(&py, bits);
        assert!(!crate::exception_pending(&py));
    });
}

#[test]
fn cold_projection_preserves_independent_raised_channels() {
    let _transaction = crate::test_support::RuntimeTestTransaction::new();
    crate::with_gil(|py| unsafe {
        errors::PyErr_SetString(
            (&raw mut abi_types::PyExc_ValueError).cast(),
            c"C error".as_ptr(),
        );
        let c_error = errors::take_current_error().expect("C error");
        let c_type = c_error.exc_type;
        let c_value = c_error.value;
        errors::restore_current_error_exact(c_error);
        let runtime_ptr =
            crate::builtins::exceptions::alloc_exception(&py, "KeyError", "runtime error");
        let runtime = MoltObject::from_ptr(runtime_ptr).bits();
        crate::record_exception(&py, runtime_ptr);
        let projected = GLOBAL_BRIDGE.handle_to_borrowed_pyobj(runtime);
        assert!(
            !projected.is_null(),
            "cold exception view must materialize its fields"
        );
        assert_eq!(crate::exception_last_bits_noinc(&py), Some(runtime));
        let restored = errors::take_current_error().expect("C channel is independent");
        assert_eq!(restored.exc_type, c_type);
        assert_eq!(restored.value, c_value);
        assert!(restored.traceback.is_null());
        drop(restored);
        crate::clear_exception(&py);
        dec_ref_bits(&py, runtime);
        assert!(errors::PyErr_Occurred().is_null());
    });
}

extern "C" fn forbidden_storage_iteration(_: u64) -> u64 {
    crate::with_gil(|py| {
        crate::raise_exception::<u64>(&py, "AssertionError", "iterated list storage")
    })
}

extern "C" fn replacement_iteration(_: u64) -> u64 {
    crate::with_gil(|py| {
        let values =
            MoltObject::from_ptr(crate::alloc_list(&py, &[MoltObject::from_int(9).bits()])).bits();
        let iterator = crate::molt_iter(values);
        dec_ref_bits(&py, values);
        iterator
    })
}

unsafe fn list_subtype(py: &PyToken<'_>, iter: *const ()) -> u64 {
    unsafe {
        let name = crate::attr_name_bits_from_bytes(py, b"SequenceProtocolList").unwrap();
        let class = crate::molt_class_new(name);
        dec_ref_bits(py, name);
        crate::molt_class_set_base(class, builtin_classes(py).list);
        let method = MoltObject::from_ptr(crate::builtins::functions::alloc_runtime_function_obj(
            py,
            crate::provenance::abi::expose_function_address(iter),
            1,
        ))
        .bits();
        let key = crate::attr_name_bits_from_bytes(py, b"__iter__").unwrap();
        crate::molt_set_attr_name(class, key, method);
        dec_ref_bits(py, key);
        dec_ref_bits(py, method);
        crate::object::class_finish_definition(py, obj_from_bits(class).as_ptr().unwrap()).unwrap();
        let instance = crate::call_callable0(py, class);
        dec_ref_bits(py, class);
        assert!(!crate::exception_pending(py));
        instance
    }
}

#[test]
fn sequence_materialization_separates_subtype_iteration_from_list_storage_mutation() {
    use molt_cpython_abi::api::{abstract_sequence, numbers, sequences};
    let _transaction = crate::test_support::RuntimeTestTransaction::new();
    crate::with_gil(|py| unsafe {
        let destination = list_subtype(&py, forbidden_storage_iteration as *const ());
        crate::molt_list_append(destination, MoltObject::from_int(1).bits());
        crate::molt_list_append(destination, MoltObject::from_int(2).bits());
        let source = list_subtype(&py, replacement_iteration as *const ());
        crate::molt_list_append(source, MoltObject::from_int(5).bits());
        let dest_view = GLOBAL_BRIDGE.handle_to_borrowed_pyobj(destination);
        let source_view = GLOBAL_BRIDGE.handle_to_borrowed_pyobj(source);
        assert!(!dest_view.is_null() && !source_view.is_null());
        assert_eq!(sequences::PyList_SetSlice(dest_view, 0, 1, source_view), 0);
        let items = |expected: &[i64]| {
            assert_eq!(sequences::PyList_Size(dest_view), expected.len() as isize);
            for (i, value) in expected.iter().enumerate() {
                assert_eq!(
                    numbers::PyLong_AsLongLong(sequences::PyList_GetItem(dest_view, i as isize)),
                    *value
                );
            }
        };
        items(&[9, 2]);
        // Explicit sequence iteration bypasses destination __iter__ in both C transports.
        molt_cpython_abi_test_support::link();
        for probe in [
            molt_linked_type_identity_probe_sequence_iterator_first,
            molt_public_type_identity_probe_sequence_iterator_first,
        ] {
            let first = probe(dest_view);
            assert_eq!(numbers::PyLong_AsLongLong(first), 9);
            refcount::Py_DECREF(first);
        }
        assert_eq!(sequences::PyList_SetSlice(dest_view, 2, 2, dest_view), 0);
        items(&[9, 2, 9, 2]);
        let repeated = abstract_sequence::PySequence_InPlaceRepeat(dest_view, 2);
        assert_eq!(repeated, dest_view);
        refcount::Py_DECREF(repeated);
        items(&[9, 2, 9, 2, 9, 2, 9, 2]);
        let converted = abstract_sequence::PySequence_List(dest_view);
        assert!(
            converted.is_null(),
            "iterable conversion must honor the override"
        );
        assert_eq!(
            errors::PyErr_ExceptionMatches((&raw mut abi_types::PyExc_AssertionError).cast()),
            1
        );
        errors::PyErr_Clear();
        dec_ref_bits(&py, source);
        dec_ref_bits(&py, destination);
        assert!(!crate::exception_pending(&py));
    });
}

unsafe extern "C" {
    fn molt_linked_type_identity_probe_iteration(
        value: *mut PyObject,
        completion: *mut PyObject,
    ) -> c_int;
    fn molt_public_type_identity_probe_iteration(
        value: *mut PyObject,
        completion: *mut PyObject,
    ) -> c_int;
    fn molt_linked_type_identity_probe_sequence_iterator_first(
        value: *mut PyObject,
    ) -> *mut PyObject;
    fn molt_public_type_identity_probe_sequence_iterator_first(
        value: *mut PyObject,
    ) -> *mut PyObject;
    fn molt_linked_type_identity_probe_sequence_check(value: *mut PyObject) -> c_int;
    fn molt_public_type_identity_probe_sequence_check(value: *mut PyObject) -> c_int;
    fn molt_linked_type_identity_probe_sequence_materialization(
        value: *mut PyObject,
        first: *mut PyObject,
    ) -> c_int;
    fn molt_public_type_identity_probe_sequence_materialization(
        value: *mut PyObject,
        first: *mut PyObject,
    ) -> c_int;
    fn molt_linked_type_identity_probe_length_hint(value: *mut PyObject, default: isize) -> isize;
    fn molt_public_type_identity_probe_length_hint(value: *mut PyObject, default: isize) -> isize;
}

#[test]
fn sequence_length_hint_uses_type_lookup_and_distinguishes_lookup_from_call_failure() {
    use molt_cpython_abi::api::{mapping, numbers, strings};
    use std::ptr;
    unsafe extern "C" fn hint(value: *mut PyObject, _: *mut PyObject) -> *mut PyObject {
        unsafe { object::Py_NewRef(value) }
    }
    unsafe extern "C" fn call_failure(_: *mut PyObject, _: *mut PyObject) -> *mut PyObject {
        unsafe {
            errors::PyErr_SetString(
                (&raw mut abi_types::PyExc_TypeError).cast(),
                c"hint call".as_ptr(),
            )
        };
        ptr::null_mut()
    }
    unsafe extern "C" fn lookup_failure(
        _: *mut PyObject,
        _: *mut PyObject,
        _: *mut PyObject,
    ) -> *mut PyObject {
        unsafe {
            errors::PyErr_SetString(
                (&raw mut abi_types::PyExc_TypeError).cast(),
                c"hint descriptor".as_ptr(),
            )
        };
        ptr::null_mut()
    }
    unsafe extern "C" fn ordinary_lookup(_: *mut PyObject, _: *mut PyObject) -> *mut PyObject {
        unsafe {
            errors::PyErr_SetString(
                (&raw mut abi_types::PyExc_AssertionError).cast(),
                c"ordinary lookup invoked".as_ptr(),
            )
        };
        ptr::null_mut()
    }
    let _transaction = crate::test_support::RuntimeTestTransaction::new();
    molt_cpython_abi_test_support::link();
    crate::with_gil(|py| unsafe {
        let mut ty: PyTypeObject = std::mem::zeroed();
        ty.ob_base.ob_base.ob_refcnt = 1;
        ty.tp_name = c"NativeHint".as_ptr();
        ty.tp_flags = abi_types::Py_TPFLAGS_READY;
        ty.tp_getattro = Some(ordinary_lookup);
        ty.tp_dict = mapping::PyDict_New();
        assert!(!ty.tp_dict.is_null());
        let name = strings::PyUnicode_FromString(c"__length_hint__".as_ptr());
        let mut receiver = PyObject {
            ob_refcnt: 1,
            ob_type: &raw mut ty,
        };
        let mut method = abi_types::PyMethodDef {
            ml_name: c"__length_hint__".as_ptr(),
            ml_meth: Some(hint),
            ml_flags: abi_types::METH_NOARGS,
            ml_doc: ptr::null(),
        };
        let mut failing_method = abi_types::PyMethodDef {
            ml_name: c"__length_hint__".as_ptr(),
            ml_meth: Some(call_failure),
            ml_flags: abi_types::METH_NOARGS,
            ml_doc: ptr::null(),
        };
        let mut descriptor_type: PyTypeObject = std::mem::zeroed();
        descriptor_type.tp_name = c"HintDescriptor".as_ptr();
        descriptor_type.tp_descr_get = Some(lookup_failure);
        let mut descriptor = PyObject {
            ob_refcnt: 1,
            ob_type: &raw mut descriptor_type,
        };
        for probe in [
            molt_linked_type_identity_probe_length_hint,
            molt_public_type_identity_probe_length_hint,
        ] {
            assert_eq!(probe(&raw mut receiver, 17), 17);
            assert_eq!(probe(&raw mut receiver, -7), -7);
            assert!(errors::PyErr_Occurred().is_null());
            for (value, expected, error_class) in [
                (numbers::PyLong_FromLong(5), 5, ptr::null_mut::<PyObject>()),
                (
                    object::Py_NewRef(&raw mut abi_types::Py_NotImplementedSentinel),
                    17,
                    ptr::null_mut(),
                ),
                (
                    numbers::PyLong_FromLong(-2),
                    -1,
                    (&raw mut abi_types::PyExc_ValueError).cast(),
                ),
                (
                    numbers::PyFloat_FromDouble(1.5),
                    -1,
                    (&raw mut abi_types::PyExc_TypeError).cast(),
                ),
            ] {
                let callable = object::PyCFunction_New(&raw mut method, value);
                assert!(!callable.is_null());
                assert_eq!(mapping::PyDict_SetItem(ty.tp_dict, name, callable), 0);
                refcount::Py_DECREF(callable);
                refcount::Py_DECREF(value);
                assert_eq!(probe(&raw mut receiver, 17), expected);
                if error_class.is_null() {
                    assert!(errors::PyErr_Occurred().is_null());
                } else {
                    assert_eq!(errors::PyErr_ExceptionMatches(error_class), 1);
                    errors::PyErr_Clear();
                }
            }
            let callable = object::PyCFunction_New(&raw mut failing_method, ptr::null_mut());
            assert!(!callable.is_null());
            assert_eq!(mapping::PyDict_SetItem(ty.tp_dict, name, callable), 0);
            refcount::Py_DECREF(callable);
            assert_eq!(
                probe(&raw mut receiver, 17),
                17,
                "call TypeError selects default"
            );
            assert!(errors::PyErr_Occurred().is_null());
            assert_eq!(
                mapping::PyDict_SetItem(ty.tp_dict, name, &raw mut descriptor),
                0
            );
            assert_eq!(
                probe(&raw mut receiver, 17),
                -1,
                "descriptor TypeError propagates"
            );
            assert_eq!(
                errors::PyErr_ExceptionMatches((&raw mut abi_types::PyExc_TypeError).cast()),
                1
            );
            errors::PyErr_Clear();
            assert_eq!(mapping::PyDict_DelItem(ty.tp_dict, name), 0);
        }
        refcount::Py_DECREF(ty.tp_dict);
        refcount::Py_DECREF(name);
        assert_eq!(descriptor.ob_refcnt, 1);
        assert_eq!(receiver.ob_refcnt, 1);
        assert!(!crate::exception_pending(&py));
    });
}

#[test]
fn sequence_headers_share_admission_materialization_and_pending_error_custody() {
    let _transaction = crate::test_support::RuntimeTestTransaction::new();
    molt_cpython_abi_test_support::link();
    crate::with_gil(|py| unsafe {
        let marker = crate::alloc_string(&py, b"first");
        assert!(!marker.is_null());
        let marker_bits = MoltObject::from_ptr(marker).bits();
        let list = crate::alloc_list(&py, &[marker_bits, MoltObject::from_float(0.0).bits()]);
        dec_ref_bits(&py, marker_bits);
        let dict = crate::alloc_dict_with_pairs(&py, &[]);
        let list_bits = MoltObject::from_ptr(list).bits();
        let dict_bits = MoltObject::from_ptr(dict).bits();
        let list_view = GLOBAL_BRIDGE.handle_to_borrowed_pyobj(list_bits);
        let dict_view = GLOBAL_BRIDGE.handle_to_borrowed_pyobj(dict_bits);
        let first = GLOBAL_BRIDGE.handle_to_borrowed_pyobj(marker_bits);
        assert!(!list_view.is_null() && !dict_view.is_null() && !first.is_null());
        for probe in [
            molt_linked_type_identity_probe_sequence_materialization,
            molt_public_type_identity_probe_sequence_materialization,
        ] {
            assert_eq!(probe(list_view, first), 0);
            assert!(!crate::exception_pending(&py));
            assert!(errors::PyErr_Occurred().is_null());
        }
        for probe in [
            molt_linked_type_identity_probe_sequence_check,
            molt_public_type_identity_probe_sequence_check,
        ] {
            assert_eq!(probe(list_view), 1);
            assert_eq!(probe(dict_view), 0);
            errors::PyErr_SetString(
                (&raw mut abi_types::PyExc_ValueError).cast(),
                c"incoming C error".as_ptr(),
            );
            let incoming = errors::PyErr_GetRaisedException();
            assert!(!incoming.is_null());
            errors::PyErr_SetRaisedException(object::Py_NewRef(incoming));
            assert_eq!(probe(list_view), 1);
            assert_eq!(probe(dict_view), 0);
            let observed = errors::PyErr_GetRaisedException();
            assert_eq!(observed, incoming);
            refcount::Py_DECREF(observed);
            refcount::Py_DECREF(incoming);
            crate::raise_exception::<()>(&py, "LookupError", "incoming runtime error");
            let incoming = crate::builtins::exceptions::molt_exception_last_pending();
            assert_eq!(probe(list_view), 1);
            assert_eq!(probe(dict_view), 0);
            let observed = crate::builtins::exceptions::molt_exception_last_pending();
            assert_eq!(observed, incoming);
            crate::clear_exception(&py);
            dec_ref_bits(&py, observed);
            dec_ref_bits(&py, incoming);
        }
        dec_ref_bits(&py, list_bits);
        dec_ref_bits(&py, dict_bits);
        assert!(!crate::exception_pending(&py));
    });
}

#[test]
fn physical_sequence_admission_preserves_an_unrelated_raised_error() {
    let _transaction = crate::test_support::RuntimeTestTransaction::new();
    crate::with_gil(|_py| unsafe {
        unsafe extern "C" fn item(_: *mut PyObject, _: isize) -> *mut PyObject {
            std::ptr::null_mut()
        }
        let mut methods: PySequenceMethods = std::mem::zeroed();
        methods.sq_item = item as *const () as *mut c_void;
        let mut ty: PyTypeObject = std::mem::zeroed();
        ty.tp_as_sequence = (&raw mut methods).cast();
        ty.tp_name = c"NativeSequence".as_ptr();
        let mut object = PyObject {
            ob_refcnt: 1,
            ob_type: &raw mut ty,
        };
        errors::PyErr_SetString(
            (&raw mut abi_types::PyExc_ValueError).cast(),
            c"native pending".as_ptr(),
        );
        let incoming = errors::PyErr_Occurred();
        assert!(!incoming.is_null());
        for probe in [
            molt_linked_type_identity_probe_sequence_check,
            molt_public_type_identity_probe_sequence_check,
        ] {
            assert_eq!(probe(&raw mut object), 1);
            assert_eq!(errors::PyErr_Occurred(), incoming);
        }
        errors::PyErr_Clear();
    });
}

thread_local! {
    static PROJECTION_NEXT: std::cell::Cell<(usize, u64)> = const {
        std::cell::Cell::new((0, 0))
    };
}

extern "C" fn projection_self_iter(value: u64) -> u64 {
    crate::with_gil(|py| {
        inc_ref_bits(&py, value);
        value
    })
}

extern "C" fn projection_next(_: u64) -> u64 {
    PROJECTION_NEXT.with(|state| {
        let (step, exception) = state.get();
        state.set((step + 1, exception));
        if step == 0 {
            // A zero handle payload is a successful Python float, not NULL.
            MoltObject::from_float(0.0).bits()
        } else {
            crate::molt_raise(exception)
        }
    })
}

extern "C" fn projection_replaced_next(_: u64) -> u64 {
    MoltObject::from_int(41).bits()
}

fn projection_install(py: &PyToken<'_>, class: u64, name: &[u8], callback: *const ()) {
    let method = MoltObject::from_ptr(crate::builtins::functions::alloc_runtime_function_obj(
        py,
        crate::provenance::abi::expose_function_address(callback),
        1,
    ))
    .bits();
    let key = crate::attr_name_bits_from_bytes(py, name).unwrap();
    crate::molt_set_attr_name(class, key, method);
    dec_ref_bits(py, key);
    dec_ref_bits(py, method);
    assert!(!crate::exception_pending(py));
}

#[test]
fn managed_iteration_preserves_owned_values_completion_and_live_class_lookup() {
    use molt_cpython_abi::api::numbers;
    let _transaction = crate::test_support::RuntimeTestTransaction::new();
    crate::with_gil(|py| unsafe {
        let name = crate::attr_name_bits_from_bytes(&py, b"ProjectedIterator").unwrap();
        let class = crate::molt_class_new(name);
        dec_ref_bits(&py, name);
        crate::molt_class_set_base(class, builtin_classes(&py).object);
        projection_install(&py, class, b"__iter__", projection_self_iter as *const ());
        projection_install(&py, class, b"__next__", projection_next as *const ());
        crate::object::class_finish_definition(&py, obj_from_bits(class).as_ptr().unwrap())
            .unwrap();
        let instance = crate::call_callable0(&py, class);
        let view = GLOBAL_BRIDGE.handle_to_borrowed_pyobj(instance);
        assert!(!view.is_null());
        let payload = MoltObject::from_ptr(crate::alloc_list(&py, &[])).bits();
        let payload_view = GLOBAL_BRIDGE.handle_to_borrowed_pyobj(payload);
        let stop = MoltObject::from_ptr(crate::builtins::exceptions::alloc_exception(
            &py,
            "StopIteration",
            "completion",
        ))
        .bits();
        let value_name = crate::attr_name_bits_from_bytes(&py, b"value").unwrap();
        crate::molt_set_attr_name(stop, value_name, payload);
        dec_ref_bits(&py, value_name);
        assert!(!crate::exception_pending(&py));
        PROJECTION_NEXT.with(|state| state.set((0, stop)));
        assert_eq!(object::PyIter_Check(view), 1);
        assert_eq!(PROJECTION_NEXT.with(|state| state.get().0), 0);
        let iter = object::PyObject_GetIter(view);
        assert_eq!(iter, view);
        let first = object::PyIter_Next(iter);
        assert!(!first.is_null());
        assert_eq!(numbers::PyFloat_AsDouble(first), 0.0);
        refcount::Py_DECREF(first);
        let mut result = std::ptr::null_mut();
        assert_eq!(
            object::PyIter_Send(iter, &raw mut abi_types::Py_None, &raw mut result),
            0
        );
        assert_eq!(
            result, payload_view,
            "completion retains arbitrary value identity"
        );
        refcount::Py_DECREF(result);
        assert!(object::PyIter_Next(iter).is_null());
        assert!(errors::PyErr_Occurred().is_null());

        molt_cpython_abi_test_support::link();
        for probe in [
            molt_linked_type_identity_probe_iteration,
            molt_public_type_identity_probe_iteration,
        ] {
            PROJECTION_NEXT.with(|state| state.set((0, stop)));
            assert_eq!(probe(view, payload_view), 0);
            assert!(errors::PyErr_Occurred().is_null());
        }

        let failure = MoltObject::from_ptr(crate::builtins::exceptions::alloc_exception(
            &py,
            "ValueError",
            "iterator callback",
        ))
        .bits();
        let failure_view = GLOBAL_BRIDGE.handle_to_borrowed_pyobj(failure);
        assert!(!failure_view.is_null());
        PROJECTION_NEXT.with(|state| state.set((1, failure)));
        assert_eq!(
            object::PyIter_Send(iter, &raw mut abi_types::Py_None, &raw mut result),
            -1
        );
        assert!(result.is_null());
        let observed = errors::PyErr_GetRaisedException();
        assert_eq!(observed, failure_view);
        refcount::Py_DECREF(observed);
        assert!(object::PyIter_Next(iter).is_null());
        let observed = errors::PyErr_GetRaisedException();
        assert_eq!(observed, failure_view);
        refcount::Py_DECREF(observed);

        // A previously projected class does not freeze its Python slot owner.
        projection_install(
            &py,
            class,
            b"__next__",
            projection_replaced_next as *const (),
        );
        assert_eq!(object::PyIter_Check(view), 1);
        let next = object::PyIter_Next(iter);
        assert_eq!(numbers::PyLong_AsLongLong(next), 41);
        refcount::Py_DECREF(next);
        refcount::Py_DECREF(iter);
        PROJECTION_NEXT.with(|state| state.set((0, 0)));
        for bits in [failure, stop, payload, instance, class] {
            dec_ref_bits(&py, bits);
        }
        assert!(!crate::exception_pending(&py));
    });
}

#[test]
fn managed_iteration_rejects_noniterator_results_and_noniterable_values() {
    let _transaction = crate::test_support::RuntimeTestTransaction::new();
    crate::with_gil(|py| unsafe {
        let instance = list_subtype(&py, projection_replaced_next as *const ());
        let view = GLOBAL_BRIDGE.handle_to_borrowed_pyobj(instance);
        assert!(!view.is_null());
        assert_eq!(object::PyIter_Check(view), 0);
        assert!(object::PyObject_GetIter(view).is_null());
        assert_eq!(
            errors::PyErr_ExceptionMatches((&raw mut abi_types::PyExc_TypeError).cast()),
            1
        );
        errors::PyErr_Clear();
        let number = GLOBAL_BRIDGE.handle_to_borrowed_pyobj(MoltObject::from_int(5).bits());
        assert!(object::PyObject_GetIter(number).is_null());
        assert_eq!(
            errors::PyErr_ExceptionMatches((&raw mut abi_types::PyExc_TypeError).cast()),
            1
        );
        errors::PyErr_Clear();
        dec_ref_bits(&py, instance);
        assert!(!crate::exception_pending(&py));
    });
}

extern "C" fn indexed_read_short_length(_: u64) -> u64 {
    MoltObject::from_int(1).bits()
}

#[test]
fn indexed_reads_share_inherited_slots_and_normalize_negative_indices_once() {
    use crate::object::sequence_index::sequence_item_at_index;
    use molt_cpython_abi::api::{abstract_sequence, numbers};
    let _transaction = crate::test_support::RuntimeTestTransaction::new();
    crate::with_gil(|py| unsafe {
        let list = list_subtype(&py, forbidden_storage_iteration as *const ());
        for item in [11, 22, 33] {
            crate::molt_list_append(list, MoltObject::from_int(item).bits());
        }
        let view = GLOBAL_BRIDGE.handle_to_borrowed_pyobj(list);
        assert_eq!(sequence_check_bits(&py, list), 1);
        assert_eq!(
            to_i64(obj_from_bits(sequence_item_at_index(&py, list, -1))),
            Some(33)
        );
        let _ = sequence_item_at_index(&py, list, -4);
        assert_eq!(
            errors::PyErr_ExceptionMatches((&raw mut abi_types::PyExc_IndexError).cast()),
            1
        );
        errors::PyErr_Clear();
        let class = type_of_bits(&py, list);
        projection_install(
            &py,
            class,
            b"__len__",
            indexed_read_short_length as *const (),
        );
        // Inherited native sq_item uses the new length, then reads raw storage.
        assert_eq!(
            to_i64(obj_from_bits(sequence_item_at_index(&py, list, -1))),
            Some(11)
        );
        let result = abstract_sequence::PySequence_GetItem(view, -1);
        assert!(!result.is_null());
        assert_eq!(numbers::PyLong_AsLongLong(result), 11);
        refcount::Py_DECREF(result);
        assert!(abstract_sequence::PySequence_GetItem(view, -2).is_null());
        assert_eq!(
            errors::PyErr_ExceptionMatches((&raw mut abi_types::PyExc_IndexError).cast()),
            1
        );
        errors::PyErr_Clear();
        dec_ref_bits(&py, list);
        assert!(!crate::exception_pending(&py));
    });
}

#[test]
fn indexed_foreign_reads_use_native_slots_and_preserve_result_ownership() {
    use crate::object::sequence_index::sequence_item_at_index;
    let _transaction = crate::test_support::RuntimeTestTransaction::new();
    crate::with_gil(|py| unsafe {
        unsafe extern "C" fn length(_: *mut PyObject) -> isize {
            3
        }
        unsafe extern "C" fn item(_: *mut PyObject, index: isize) -> *mut PyObject {
            if !(0..3).contains(&index) {
                unsafe {
                    errors::PyErr_SetString(
                        (&raw mut abi_types::PyExc_IndexError).cast(),
                        c"native read exhausted".as_ptr(),
                    )
                };
                return std::ptr::null_mut();
            }
            unsafe { molt_cpython_abi::api::numbers::PyLong_FromLongLong(index as i64) }
        }
        let mut methods: PySequenceMethods = std::mem::zeroed();
        methods.sq_length = length as *const () as *mut c_void;
        methods.sq_item = item as *const () as *mut c_void;
        let mut ty: PyTypeObject = std::mem::zeroed();
        ty.tp_as_sequence = (&raw mut methods).cast();
        ty.tp_name = c"NativeIndexedRead".as_ptr();
        let mut native = PyObject {
            ob_refcnt: 1,
            ob_type: &raw mut ty,
        };
        let receiver = GLOBAL_BRIDGE
            .molt_value_for_pyobj(&raw mut native)
            .expect("foreign receiver");
        assert_eq!(
            to_i64(obj_from_bits(sequence_item_at_index(&py, receiver, -1))),
            Some(2)
        );
        let _ = sequence_item_at_index(&py, receiver, -4);
        assert_eq!(
            errors::PyErr_ExceptionMatches((&raw mut abi_types::PyExc_IndexError).cast()),
            1
        );
        errors::PyErr_Clear();
        dec_ref_bits(&py, receiver);
        assert_eq!(native.ob_refcnt, 1);
        assert!(!crate::exception_pending(&py));
    });
}

extern "C" fn replacement_list_inplace_repeat(_: u64, _: u64) -> u64 {
    MoltObject::from_int(71).bits()
}

#[test]
fn list_root_sequence_slots_preserve_storage_and_follow_override_mutation() {
    use molt_cpython_abi::api::refcount::OwnedPyObject;
    use molt_cpython_abi::api::{abstract_number, abstract_sequence, numbers, sequences, typeobj};
    use molt_cpython_abi::type_slots as slots;
    let _transaction = crate::test_support::RuntimeTestTransaction::new();
    crate::with_gil(|py| unsafe {
        let value = list_subtype(&py, forbidden_storage_iteration as *const ());
        crate::molt_list_append(value, MoltObject::from_int(1).bits());
        crate::molt_list_append(value, MoltObject::from_int(2).bits());
        let class = crate::type_of_bits(&py, value);
        let view = OwnedPyObject::from_owned(GLOBAL_BRIDGE.borrowed_handle_to_new_pyobj(value));
        let class_view =
            OwnedPyObject::from_owned(GLOBAL_BRIDGE.borrowed_handle_to_new_pyobj(class));
        assert!(!view.as_ptr().is_null() && !class_view.as_ptr().is_null());
        let root = &raw mut abi_types::PyList_Type;
        for slot in [
            slots::Py_sq_length,
            slots::Py_sq_concat,
            slots::Py_sq_repeat,
            slots::Py_sq_item,
            slots::Py_sq_ass_item,
            slots::Py_sq_contains,
            slots::Py_sq_inplace_concat,
            slots::Py_sq_inplace_repeat,
        ] {
            assert!(!typeobj::PyType_GetSlot(root, slot).is_null());
        }
        let root_repeat = typeobj::PyType_GetSlot(root, slots::Py_sq_inplace_repeat);
        assert_eq!(
            typeobj::PyType_GetSlot(class_view.as_ptr().cast(), slots::Py_sq_inplace_repeat),
            root_repeat
        );

        let concat = OwnedPyObject::from_owned(abstract_sequence::PySequence_Concat(
            view.as_ptr(),
            view.as_ptr(),
        ));
        let repeated =
            OwnedPyObject::from_owned(abstract_sequence::PySequence_Repeat(view.as_ptr(), 2));
        for result in [concat.as_ptr(), repeated.as_ptr()] {
            assert!(!result.is_null());
            assert_eq!(sequences::PyList_CheckExact(result), 1);
            assert_eq!(sequences::PyList_Size(result), 4);
            for (index, expected) in [1, 2, 1, 2].into_iter().enumerate() {
                assert_eq!(
                    numbers::PyLong_AsLongLong(sequences::PyList_GetItem(result, index as isize)),
                    expected
                );
            }
        }
        let extended = OwnedPyObject::from_owned(abstract_sequence::PySequence_InPlaceConcat(
            view.as_ptr(),
            view.as_ptr(),
        ));
        assert_eq!(extended.as_ptr(), view.as_ptr());
        assert_eq!(sequences::PyList_Size(view.as_ptr()), 4);
        let repeated = OwnedPyObject::from_owned(abstract_sequence::PySequence_InPlaceRepeat(
            view.as_ptr(),
            2,
        ));
        assert_eq!(repeated.as_ptr(), view.as_ptr());
        assert_eq!(sequences::PyList_Size(view.as_ptr()), 8);

        // The direct declaring slot returns a new item reference and mutates
        // list storage without consulting the subtype's forbidden iterator.
        type Item = unsafe extern "C" fn(*mut PyObject, isize) -> *mut PyObject;
        type Assign = unsafe extern "C" fn(*mut PyObject, isize, *mut PyObject) -> c_int;
        type Contains = unsafe extern "C" fn(*mut PyObject, *mut PyObject) -> c_int;
        let item: Item = std::mem::transmute(typeobj::PyType_GetSlot(root, slots::Py_sq_item));
        let assign: Assign =
            std::mem::transmute(typeobj::PyType_GetSlot(root, slots::Py_sq_ass_item));
        let contains: Contains =
            std::mem::transmute(typeobj::PyType_GetSlot(root, slots::Py_sq_contains));
        let eight = OwnedPyObject::from_owned(numbers::PyLong_FromLong(8));
        assert_eq!(assign(view.as_ptr(), 0, eight.as_ptr()), 0);
        let first = OwnedPyObject::from_owned(item(view.as_ptr(), 0));
        assert_eq!(first.as_ptr(), eight.as_ptr());
        assert_eq!(contains(view.as_ptr(), eight.as_ptr()), 1);
        assert_eq!(assign(view.as_ptr(), 0, std::ptr::null_mut()), 0);
        assert_eq!(sequences::PyList_Size(view.as_ptr()), 7);
        assert_eq!(numbers::PyLong_AsLongLong(first.as_ptr()), 8);

        let overflow = abstract_sequence::PySequence_InPlaceRepeat(view.as_ptr(), isize::MAX);
        assert!(overflow.is_null());
        assert_eq!(
            errors::PyErr_ExceptionMatches((&raw mut abi_types::PyExc_MemoryError).cast()),
            1
        );
        errors::PyErr_Clear();
        assert_eq!(sequences::PyList_Size(view.as_ptr()), 7);

        let method = MoltObject::from_ptr(crate::builtins::functions::alloc_runtime_function_obj(
            &py,
            crate::provenance::abi::expose_function_address(
                replacement_list_inplace_repeat as *const (),
            ),
            2,
        ))
        .bits();
        let key = crate::attr_name_bits_from_bytes(&py, b"__imul__").unwrap();
        crate::molt_set_attr_name(class, key, method);
        assert!(!crate::exception_pending(&py));
        assert!(
            typeobj::PyType_GetSlot(class_view.as_ptr().cast(), slots::Py_sq_inplace_repeat)
                .is_null()
        );
        // PySequence_InPlaceRepeat keeps the inherited sq_repeat fallback;
        // Python *= uses nb_inplace_multiply and therefore sees __imul__.
        let sequence_fallback = OwnedPyObject::from_owned(
            abstract_sequence::PySequence_InPlaceRepeat(view.as_ptr(), 2),
        );
        assert!(!sequence_fallback.as_ptr().is_null());
        assert_ne!(sequence_fallback.as_ptr(), view.as_ptr());
        assert_eq!(sequences::PyList_CheckExact(sequence_fallback.as_ptr()), 1);
        assert_eq!(sequences::PyList_Size(sequence_fallback.as_ptr()), 14);
        assert_eq!(sequences::PyList_Size(view.as_ptr()), 7);
        let count = OwnedPyObject::from_owned(numbers::PyLong_FromLong(2));
        let overridden = OwnedPyObject::from_owned(abstract_number::PyNumber_InPlaceMultiply(
            view.as_ptr(),
            count.as_ptr(),
        ));
        assert!(!overridden.as_ptr().is_null());
        assert_eq!(numbers::PyLong_AsLongLong(overridden.as_ptr()), 71);
        assert_eq!(sequences::PyList_Size(view.as_ptr()), 7);
        crate::molt_del_attr_name(class, key);
        assert!(!crate::exception_pending(&py));
        assert_eq!(
            typeobj::PyType_GetSlot(class_view.as_ptr().cast(), slots::Py_sq_inplace_repeat),
            root_repeat
        );
        let cleared = OwnedPyObject::from_owned(abstract_sequence::PySequence_InPlaceRepeat(
            view.as_ptr(),
            0,
        ));
        assert_eq!(cleared.as_ptr(), view.as_ptr());
        assert_eq!(sequences::PyList_Size(view.as_ptr()), 0);
        for bits in [key, method, value] {
            dec_ref_bits(&py, bits);
        }
        assert!(errors::PyErr_Occurred().is_null());
        assert!(!crate::exception_pending(&py));
    });
}
