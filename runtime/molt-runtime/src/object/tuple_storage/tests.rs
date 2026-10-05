use super::*;
use molt_cpython_abi::abi_types::*;
use molt_cpython_abi::api::{errors, numbers, typeobj};
use std::ptr;
use std::sync::atomic::{AtomicUsize, Ordering};

static OVERRIDES: AtomicUsize = AtomicUsize::new(0);

unsafe extern "C" fn overridden_length(_: *mut PyObject) -> isize {
    OVERRIDES.fetch_add(1, Ordering::SeqCst);
    99
}

unsafe extern "C" fn overridden_getitem(_: *mut PyObject, _: *mut PyObject) -> *mut PyObject {
    OVERRIDES.fetch_add(1, Ordering::SeqCst);
    unsafe { numbers::PyLong_FromLong(999) }
}

unsafe extern "C" fn overridden_contains(_: *mut PyObject, _: *mut PyObject) -> i32 {
    OVERRIDES.fetch_add(1, Ordering::SeqCst);
    0
}

unsafe extern "C" fn overridden_iter(_: *mut PyObject) -> *mut PyObject {
    OVERRIDES.fetch_add(1, Ordering::SeqCst);
    unsafe {
        errors::PyErr_SetString(
            (&raw mut PyExc_ValueError).cast(),
            c"tuple override".as_ptr(),
        )
    };
    ptr::null_mut()
}

fn descriptor(py: &PyToken<'_>, name: &str, args: &[u64]) -> u64 {
    let callable = crate::builtins::containers::tuple_method_bits(py, name).unwrap();
    unsafe { crate::call::bind::call_bind_borrowed(py, callable, None, args, &[], &[]) }
}

unsafe fn native_tuple_of(class: *mut PyTypeObject, values: &[i64]) -> OwnedPyObject {
    let tuple = unsafe {
        OwnedPyObject::from_owned(typeobj::PyType_GenericAlloc(class, values.len() as isize))
    };
    assert!(!tuple.as_ptr().is_null());
    for (index, value) in values.iter().enumerate() {
        assert_eq!(
            unsafe {
                sequences::PyTuple_SetItem(
                    tuple.as_ptr(),
                    index as isize,
                    numbers::PyLong_FromLongLong(*value),
                )
            },
            0
        );
    }
    tuple
}

fn assert_values(py: &PyToken<'_>, bits: u64, expected: &[i64]) {
    let tuple = TupleStorage::from_bits(py, bits).expect("physical tuple result");
    assert_eq!(tuple.len(), Some(expected.len()));
    for (index, expected) in expected.iter().enumerate() {
        let value = tuple.item(index).unwrap();
        assert_eq!(obj_from_bits(value).as_int(), Some(*expected));
        dec_ref_bits(py, value);
    }
}

fn exercise_descriptors(py: &PyToken<'_>, bits: u64) {
    let one = MoltObject::from_int(1).bits();
    assert_eq!(
        obj_from_bits(descriptor(py, "__len__", &[bits])).as_int(),
        Some(3)
    );
    assert_eq!(
        obj_from_bits(descriptor(
            py,
            "__getitem__",
            &[bits, MoltObject::from_int(-1).bits()]
        ))
        .as_int(),
        Some(1)
    );
    assert_eq!(
        obj_from_bits(descriptor(py, "__contains__", &[bits, one])).as_bool(),
        Some(true)
    );
    assert_eq!(
        obj_from_bits(descriptor(py, "count", &[bits, one])).as_int(),
        Some(2)
    );
    assert_eq!(
        obj_from_bits(descriptor(py, "index", &[bits, one])).as_int(),
        Some(0)
    );
    assert_eq!(
        obj_from_bits(descriptor(py, "index", &[bits, one, one])).as_int(),
        Some(2)
    );

    let iterator = descriptor(py, "__iter__", &[bits]);
    for expected in [1, 2, 1] {
        let mut value = MoltObject::none().bits();
        let done = unsafe {
            crate::object::ops_iter::molt_iter_next_unboxed(
                iterator,
                (&raw mut value) as usize as u64,
            )
        };
        assert_eq!(obj_from_bits(done).as_bool(), Some(false));
        assert_eq!(obj_from_bits(value).as_int(), Some(expected));
        dec_ref_bits(py, value);
    }
    let mut value = MoltObject::none().bits();
    assert_eq!(
        obj_from_bits(unsafe {
            crate::object::ops_iter::molt_iter_next_unboxed(
                iterator,
                (&raw mut value) as usize as u64,
            )
        })
        .as_bool(),
        Some(true)
    );
    dec_ref_bits(py, iterator);

    let managed =
        MoltObject::from_ptr(alloc_tuple(py, &[one, MoltObject::from_int(2).bits(), one])).bits();
    for (name, expected) in [
        ("__eq__", true),
        ("__ne__", false),
        ("__lt__", false),
        ("__le__", true),
        ("__gt__", false),
        ("__ge__", true),
    ] {
        assert_eq!(
            obj_from_bits(descriptor(py, name, &[bits, managed])).as_bool(),
            Some(expected),
            "{name}"
        );
        assert_eq!(
            obj_from_bits(descriptor(py, name, &[managed, bits])).as_bool(),
            Some(expected),
            "reflected {name}"
        );
    }
    for args in [[bits, managed], [managed, bits]] {
        let result = descriptor(py, "__add__", &args);
        assert_values(py, result, &[1, 2, 1, 1, 2, 1]);
        dec_ref_bits(py, result);
    }
    // The public C sequence APIs must share the same root tuple operations as
    // explicit runtime descriptors, including mixed physical representations.
    for args in [[bits, managed], [managed, bits]] {
        let left = unsafe {
            OwnedPyObject::from_owned(GLOBAL_BRIDGE.borrowed_handle_to_new_pyobj(args[0]))
        };
        let right = unsafe {
            OwnedPyObject::from_owned(GLOBAL_BRIDGE.borrowed_handle_to_new_pyobj(args[1]))
        };
        let result = unsafe {
            OwnedPyObject::from_owned(molt_cpython_abi::api::abstract_sequence::PySequence_Concat(
                left.as_ptr(),
                right.as_ptr(),
            ))
        };
        assert!(!result.as_ptr().is_null());
        assert_eq!(unsafe { sequences::PyTuple_CheckExact(result.as_ptr()) }, 1);
        assert_eq!(unsafe { sequences::PyTuple_Size(result.as_ptr()) }, 6);
        for (index, expected) in [1, 2, 1, 1, 2, 1].into_iter().enumerate() {
            assert_eq!(
                unsafe {
                    numbers::PyLong_AsLongLong(sequences::PyTuple_GetItem(
                        result.as_ptr(),
                        index as isize,
                    ))
                },
                expected
            );
        }
    }
    for receiver in [bits, managed] {
        let view = unsafe {
            OwnedPyObject::from_owned(GLOBAL_BRIDGE.borrowed_handle_to_new_pyobj(receiver))
        };
        for (count, expected) in [
            (0, &[][..]),
            (1, &[1, 2, 1][..]),
            (2, &[1, 2, 1, 1, 2, 1][..]),
        ] {
            let result = unsafe {
                OwnedPyObject::from_owned(
                    molt_cpython_abi::api::abstract_sequence::PySequence_Repeat(
                        view.as_ptr(),
                        count,
                    ),
                )
            };
            assert!(!result.as_ptr().is_null());
            assert_eq!(unsafe { sequences::PyTuple_CheckExact(result.as_ptr()) }, 1);
            assert_eq!(
                unsafe { sequences::PyTuple_Size(result.as_ptr()) },
                expected.len() as isize
            );
            for (index, &value) in expected.iter().enumerate() {
                assert_eq!(
                    unsafe {
                        numbers::PyLong_AsLongLong(sequences::PyTuple_GetItem(
                            result.as_ptr(),
                            index as isize,
                        ))
                    },
                    value
                );
            }
            if count == 1 && unsafe { sequences::PyTuple_CheckExact(view.as_ptr()) } != 0 {
                assert_eq!(result.as_ptr(), view.as_ptr());
            }
        }
    }
    for name in ["__mul__", "__rmul__"] {
        let result = descriptor(py, name, &[bits, MoltObject::from_int(2).bits()]);
        assert_values(py, result, &[1, 2, 1, 1, 2, 1]);
        dec_ref_bits(py, result);
    }
    let slice_bits = crate::object::ops_slice::molt_slice_new(
        one,
        MoltObject::none().bits(),
        MoltObject::none().bits(),
    );
    let result = descriptor(py, "__getitem__", &[bits, slice_bits]);
    assert_values(py, result, &[2, 1]);
    dec_ref_bits(py, result);
    dec_ref_bits(py, slice_bits);
    dec_ref_bits(py, managed);
    assert!(!exception_pending(py));
    assert!(unsafe { errors::PyErr_Occurred() }.is_null());
}

#[test]
fn native_tuple_descriptor_family_reads_real_storage_and_owned_results() {
    let _transaction = crate::test_support::RuntimeTestTransaction::with_gc_isolation();
    assert!(crate::cpython_abi_hooks::register_cpython_hooks());
    crate::with_gil(|py| unsafe {
        let native = native_tuple_of(&raw mut PyTuple_Type, &[1, 2, 1]);
        let bits = GLOBAL_BRIDGE.molt_value_for_pyobj(native.as_ptr()).unwrap();
        assert_eq!(
            object_type_id(obj_from_bits(bits).as_ptr().unwrap()),
            TYPE_ID_FOREIGN
        );
        exercise_descriptors(&py, bits);
        assert_eq!(obj_from_bits(crate::molt_len(bits)).as_int(), Some(3));
        assert_eq!(
            obj_from_bits(crate::molt_index(bits, MoltObject::from_int(1).bits())).as_int(),
            Some(2)
        );
        assert_eq!(
            obj_from_bits(crate::molt_contains(bits, MoltObject::from_int(2).bits())).as_bool(),
            Some(true)
        );
        let iterator = crate::molt_iter(bits);
        assert!(!exception_pending(&py));
        dec_ref_bits(&py, iterator);
        dec_ref_bits(&py, bits);
    });
}

#[test]
fn native_tuple_base_descriptors_bypass_subclass_overrides() {
    let _transaction = crate::test_support::RuntimeTestTransaction::with_gc_isolation();
    assert!(crate::cpython_abi_hooks::register_cpython_hooks());
    crate::with_gil(|py| unsafe {
        OVERRIDES.store(0, Ordering::SeqCst);
        let mut slots = [
            PyType_Slot {
                slot: molt_cpython_abi::type_slots::Py_sq_length,
                pfunc: overridden_length as *const () as *mut _,
            },
            PyType_Slot {
                slot: molt_cpython_abi::type_slots::Py_mp_subscript,
                pfunc: overridden_getitem as *const () as *mut _,
            },
            PyType_Slot {
                slot: molt_cpython_abi::type_slots::Py_sq_contains,
                pfunc: overridden_contains as *const () as *mut _,
            },
            PyType_Slot {
                slot: molt_cpython_abi::type_slots::Py_tp_iter,
                pfunc: overridden_iter as *const () as *mut _,
            },
            PyType_Slot {
                slot: 0,
                pfunc: ptr::null_mut(),
            },
        ];
        let mut spec = PyType_Spec {
            name: c"tuple_storage.OverriddenTuple".as_ptr(),
            basicsize: 0,
            itemsize: 0,
            flags: Py_TPFLAGS_DEFAULT as u32,
            slots: slots.as_mut_ptr(),
        };
        let class = OwnedPyObject::from_owned(typeobj::PyType_FromSpecWithBases(
            &raw mut spec,
            (&raw mut PyTuple_Type).cast(),
        ));
        assert!(!class.as_ptr().is_null());
        let native = native_tuple_of(class.as_ptr().cast(), &[1, 2, 1]);
        let bits = GLOBAL_BRIDGE.molt_value_for_pyobj(native.as_ptr()).unwrap();
        exercise_descriptors(&py, bits);
        assert_eq!(OVERRIDES.load(Ordering::SeqCst), 0);
        assert_eq!(obj_from_bits(crate::molt_len(bits)).as_int(), Some(99));
        assert_eq!(
            obj_from_bits(crate::molt_index(bits, MoltObject::from_int(1).bits())).as_int(),
            Some(999)
        );
        assert_eq!(
            obj_from_bits(crate::molt_contains(bits, MoltObject::from_int(1).bits())).as_bool(),
            Some(false)
        );
        let result = crate::molt_iter(bits);
        assert!(exception_pending(&py));
        crate::clear_exception(&py);
        errors::PyErr_Clear();
        dec_ref_bits(&py, result);
        assert_eq!(OVERRIDES.load(Ordering::SeqCst), 4);
        dec_ref_bits(&py, bits);
        drop(native);
        assert_eq!(typeobj::molt_type_clear(class.as_ptr()), 0);
    });
}

#[test]
fn native_tuple_rejection_and_pending_error_do_not_become_success() {
    let _transaction = crate::test_support::RuntimeTestTransaction::with_gc_isolation();
    assert!(crate::cpython_abi_hooks::register_cpython_hooks());
    crate::with_gil(|py| unsafe {
        let foreign =
            OwnedPyObject::from_owned(typeobj::PyType_GenericAlloc(&raw mut PyBaseObject_Type, 0));
        let wrong = GLOBAL_BRIDGE
            .molt_value_for_pyobj(foreign.as_ptr())
            .unwrap();
        assert!(native_tuple(wrong).is_none());
        let result = descriptor(&py, "__len__", &[wrong]);
        assert!(exception_pending(&py));
        crate::clear_exception(&py);
        errors::PyErr_Clear();
        dec_ref_bits(&py, result);
        dec_ref_bits(&py, wrong);

        let native = native_tuple_of(&raw mut PyTuple_Type, &[1]);
        let bits = GLOBAL_BRIDGE.molt_value_for_pyobj(native.as_ptr()).unwrap();
        errors::PyErr_SetString(
            (&raw mut PyExc_ValueError).cast(),
            c"tuple pending sentinel".as_ptr(),
        );
        let tuple = TupleStorage::from_bits(&py, bits).unwrap();
        assert_eq!(tuple.len(), None);
        assert!(exception_pending(&py));
        crate::clear_exception(&py);
        errors::PyErr_Clear();
        drop(tuple);
        dec_ref_bits(&py, bits);

        let uninitialized =
            OwnedPyObject::from_owned(typeobj::PyType_GenericAlloc(&raw mut PyTuple_Type, 1));
        let bits = GLOBAL_BRIDGE
            .molt_value_for_pyobj(uninitialized.as_ptr())
            .unwrap();
        let tuple = TupleStorage::from_bits(&py, bits).unwrap();
        assert_eq!(tuple.item(0), None);
        assert!(exception_pending(&py));
        crate::clear_exception(&py);
        errors::PyErr_Clear();
        drop(tuple);
        dec_ref_bits(&py, bits);
    });
}

#[test]
fn native_tuple_item_keeps_its_identity_after_source_retirement() {
    let _transaction = crate::test_support::RuntimeTestTransaction::with_gc_isolation();
    assert!(crate::cpython_abi_hooks::register_cpython_hooks());
    crate::with_gil(|py| unsafe {
        let element =
            OwnedPyObject::from_owned(typeobj::PyType_GenericAlloc(&raw mut PyBaseObject_Type, 0));
        assert!(!element.as_ptr().is_null());
        let baseline = (*element.as_ptr()).ob_refcnt;
        let tuple =
            OwnedPyObject::from_owned(typeobj::PyType_GenericAlloc(&raw mut PyTuple_Type, 1));
        assert_eq!(
            sequences::PyTuple_SetItem(
                tuple.as_ptr(),
                0,
                molt_cpython_abi::api::object::Py_NewRef(element.as_ptr())
            ),
            0
        );
        let bits = GLOBAL_BRIDGE.molt_value_for_pyobj(tuple.as_ptr()).unwrap();
        let value = descriptor(&py, "__getitem__", &[bits, MoltObject::from_int(0).bits()]);
        assert!(!exception_pending(&py));
        drop(tuple);
        dec_ref_bits(&py, bits);
        assert_eq!(
            GLOBAL_BRIDGE.handle_to_borrowed_pyobj(value),
            element.as_ptr()
        );
        assert!((*element.as_ptr()).ob_refcnt > baseline);
        dec_ref_bits(&py, value);
        assert_eq!((*element.as_ptr()).ob_refcnt, baseline);
        assert!(!exception_pending(&py));
        assert!(errors::PyErr_Occurred().is_null());
    });
}
