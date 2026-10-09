//! Real managed C inquiries share Python's special-method and error protocols.
use super::*;

#[test]
fn runtime_root_type_allocation_slots_remain_executable() {
    let _transaction = crate::test_support::RuntimeTestTransaction::with_gc_isolation();
    with_gil(|py| unsafe {
        for root in [
            &raw mut molt_cpython_abi::abi_types::PyBaseObject_Type,
            &raw mut molt_cpython_abi::abi_types::PyType_Type,
        ] {
            assert_eq!(molt_cpython_abi::api::typeobj::PyType_Ready(root), 0);
            let allocate = (*root).tp_alloc.expect("root C allocation slot");
            let object = allocate(root, 0);
            assert!(!object.is_null());
            assert_eq!((*object).ob_type, root);
            assert!(!crate::exception_pending(&py));
            molt_cpython_abi::api::refcount::Py_DECREF(object);
        }
    });
}
// CPython v3.12.13 Objects/tupleobject.c::tuplerichcompare is the oracle.
// Managed comparison belongs to the runtime: exercise the exported C entry
// with real storage and hooks instead of a fixture implementing tuple equality.
#[test]
fn ufunc_frontier_tuple_structural_richcompare() {
    let _transaction = crate::test_support::RuntimeTestTransaction::with_gc_isolation();
    with_gil(|_py| unsafe {
        use molt_cpython_abi::api::numbers::PyLong_FromLong;
        use molt_cpython_abi::api::sequences::{PyTuple_New, PyTuple_SetItem};
        use molt_cpython_abi::api::typeobj::PyObject_RichCompareBool;
        const PY_EQ: std::os::raw::c_int = 2;

        let mk = || {
            let t = PyTuple_New(3);
            for i in 0..3 {
                // steals the ref; fresh int per slot
                assert_eq!(PyTuple_SetItem(t, i, PyLong_FromLong(7)), 0);
            }
            t
        };
        const PY_NE: std::os::raw::c_int = 3;
        const PY_LT: std::os::raw::c_int = 0;

        let a = mk();
        let b = mk();
        assert!(!a.is_null() && !b.is_null(), "PyTuple_New returned NULL");
        assert_ne!(a, b, "must be two distinct tuple objects");
        let eq = PyObject_RichCompareBool(a, b, PY_EQ);
        eprintln!(
            "UFUNC-FRONTIER: (7,7,7)==(7,7,7) over distinct ABI tuples -> \
             RichCompareBool={eq}  (CPython 3.12 -> 1)"
        );
        assert_eq!(
            eq, 1,
            "equal tuple contents must match through the real managed C comparison owner"
        );

        // Faithful get_info_no_cast shape: the registered DType tuple and the
        // freshly-built lookup tuple hold the SAME repeated element object (as
        // `PyArray_DTypeFromTypeNum(NPY_BYTE)` does). Distinct tuple objects,
        // equal contents → must match.
        let elem = PyLong_FromLong(11);
        let mk_same = |e: *mut _| {
            let t = PyTuple_New(3);
            for i in 0..3 {
                molt_cpython_abi::api::refcount::Py_INCREF(e);
                assert_eq!(PyTuple_SetItem(t, i, e), 0);
            }
            t
        };
        let reg = mk_same(elem);
        let look = mk_same(elem);
        assert_ne!(reg, look, "distinct tuple objects expected");
        assert_eq!(
            PyObject_RichCompareBool(reg, look, PY_EQ),
            1,
            "get_info_no_cast lookup must match the registered loop tuple"
        );

        // Discriminator: distinct contents must NOT match — otherwise
        // PyUFunc_AddLoop(ignore_duplicate=1) would silently drop a real loop.
        let c = PyTuple_New(3);
        assert_eq!(PyTuple_SetItem(c, 0, PyLong_FromLong(7)), 0);
        assert_eq!(PyTuple_SetItem(c, 1, PyLong_FromLong(7)), 0);
        assert_eq!(PyTuple_SetItem(c, 2, PyLong_FromLong(8)), 0); // differs from (7,7,7)
        assert_eq!(
            PyObject_RichCompareBool(a, c, PY_EQ),
            0,
            "distinct tuples must compare unequal"
        );
        assert_eq!(
            PyObject_RichCompareBool(a, c, PY_NE),
            1,
            "distinct tuples must compare != as True"
        );
        // Ordering path stays correct: (7,7,7) < (7,7,8).
        assert_eq!(
            PyObject_RichCompareBool(a, c, PY_LT),
            1,
            "lexicographic tuple ordering must hold"
        );

        // Length difference decides when a prefix matches: (7,7,7) != (7,7).
        let short = PyTuple_New(2);
        assert_eq!(PyTuple_SetItem(short, 0, PyLong_FromLong(7)), 0);
        assert_eq!(PyTuple_SetItem(short, 1, PyLong_FromLong(7)), 0);
        assert_eq!(
            PyObject_RichCompareBool(a, short, PY_EQ),
            0,
            "tuples of different length must compare unequal"
        );
        for tuple in [a, b, reg, look, c, short] {
            molt_cpython_abi::api::refcount::Py_DECREF(tuple);
        }
        molt_cpython_abi::api::refcount::Py_DECREF(elem);
    });
}

use molt_cpython_abi::api::{errors, object, refcount};
use molt_cpython_abi::bridge::GLOBAL_BRIDGE;

unsafe extern "C" {
    fn molt_linked_type_identity_probe_sequence_search(
        value: *mut PyObject,
        needle: *mut PyObject,
        operation: c_int,
    ) -> isize;
    fn molt_public_type_identity_probe_sequence_search(
        value: *mut PyObject,
        needle: *mut PyObject,
        operation: c_int,
    ) -> isize;
}

fn inquiry_binary_method(py: &crate::PyToken<'_>, class: u64, name: &[u8], target: *const ()) {
    let method = MoltObject::from_ptr(crate::builtins::functions::alloc_runtime_function_obj(
        py,
        crate::provenance::abi::expose_function_address(target),
        2,
    ))
    .bits();
    set_value(py, class, name, method);
    dec_ref_bits(py, method);
}

extern "C" fn inquiry_binary_result(receiver: u64, _needle: u64) -> u64 {
    inquiry_result(receiver)
}

extern "C" fn inquiry_raise_original(receiver: u64) -> u64 {
    with_gil(|py| {
        let failure =
            crate::builtins::exceptions::ExceptionValue::adopt(&py, inquiry_result(receiver));
        crate::molt_raise(failure.bits())
    })
}

extern "C" fn inquiry_binary_raise_original(receiver: u64, _needle: u64) -> u64 {
    inquiry_raise_original(receiver)
}

extern "C" fn inquiry_self(receiver: u64) -> u64 {
    with_gil(|py| inc_ref_bits(&py, receiver));
    receiver
}

extern "C" fn inquiry_mutating_equal(receiver: u64, needle: u64) -> u64 {
    with_gil(|py| {
        let list =
            crate::builtins::exceptions::ExceptionValue::adopt(&py, inquiry_result(receiver));
        let class = crate::type_of_bits(&py, receiver);
        let name = crate::attr_name_bits_from_bytes(&py, b"inquiry_action").unwrap();
        let action = crate::molt_get_attr_name(receiver, name);
        dec_ref_bits(&py, name);
        let action_number = MoltObject::from_bits(action).as_int().unwrap();
        dec_ref_bits(&py, action);
        set_value(
            &py,
            class,
            b"inquiry_action",
            MoltObject::from_int(0).bits(),
        );
        match action_number {
            1 | 2 | 4 => {
                crate::molt_list_clear(list.bits());
                if action_number == 2 {
                    crate::molt_list_append(list.bits(), receiver);
                }
            }
            3 => {
                crate::molt_list_append(list.bits(), needle);
            }
            _ => {}
        }
        if action_number == 4 {
            let name = crate::attr_name_bits_from_bytes(&py, b"inquiry_error").unwrap();
            let failure = crate::molt_get_attr_name(receiver, name);
            dec_ref_bits(&py, name);
            let failure = crate::builtins::exceptions::ExceptionValue::adopt(&py, failure);
            return crate::molt_raise(failure.bits());
        }
        MoltObject::from_bool(false).bits()
    })
}

#[test]
fn c_sequence_contains_shares_runtime_special_lookup_and_builtin_membership() {
    let _transaction = crate::test_support::RuntimeTestTransaction::with_gc_isolation();
    molt_cpython_abi_test_support::link();
    with_gil(|py| unsafe {
        let mut owners = Vec::new();
        let classes = crate::builtin_classes(&py);
        let empty = mapping_owned(
            &py,
            &mut owners,
            MoltObject::from_ptr(crate::alloc_tuple(&py, &[])).bits(),
        );
        let text = mapping_owned(
            &py,
            &mut owners,
            MoltObject::from_ptr(alloc_string(&py, b"abc")).bits(),
        );
        let needle = mapping_view(MoltObject::from_int(99).bits());
        for probe in [
            molt_linked_type_identity_probe_sequence_search,
            molt_public_type_identity_probe_sequence_search,
        ] {
            for base in [
                classes.object,
                classes.list,
                classes.tuple,
                classes.dict,
                classes.str,
                classes.bytes,
                classes.bytearray,
                classes.set,
                classes.frozenset,
            ] {
                let class = mapping_owned(&py, &mut owners, inquiry_class(&py, base, b"__len__"));
                inquiry_binary_method(
                    &py,
                    class,
                    b"__contains__",
                    inquiry_binary_result as *const (),
                );
                let value = if base == classes.object {
                    crate::alloc_instance_for_class(
                        &py,
                        MoltObject::from_bits(class).as_ptr().unwrap(),
                    )
                } else {
                    construct(&py, class, if base == classes.str { text } else { empty })
                };
                let value = mapping_owned(&py, &mut owners, value);
                let view = mapping_view(value);
                for expected in [1, 0] {
                    set_value(
                        &py,
                        class,
                        b"inquiry_result",
                        MoltObject::from_bool(expected != 0).bits(),
                    );
                    assert_eq!(probe(view.as_ptr(), needle.as_ptr(), 0), expected);
                    assert!(errors::PyErr_Occurred().is_null());
                }
            }
            let set = mapping_owned(&py, &mut owners, construct(&py, classes.set, empty));
            let list = mapping_owned(
                &py,
                &mut owners,
                MoltObject::from_ptr(crate::alloc_list(&py, &[])).bits(),
            );
            let set_view = mapping_view(set);
            let list_view = mapping_view(list);
            assert_eq!(probe(set_view.as_ptr(), list_view.as_ptr(), 0), -1);
            assert_eq!(
                errors::PyErr_ExceptionMatches(
                    (&raw mut molt_cpython_abi::abi_types::PyExc_TypeError).cast()
                ),
                1
            );
            errors::PyErr_Clear();
            let bytes = mapping_owned(
                &py,
                &mut owners,
                MoltObject::from_ptr(alloc_bytes(&py, b"ab")).bits(),
            );
            let bytearray = mapping_owned(
                &py,
                &mut owners,
                MoltObject::from_ptr(crate::alloc_bytearray(&py, b"abc")).bits(),
            );
            let bytes_view = mapping_view(bytes);
            let bytearray_view = mapping_view(bytearray);
            assert_eq!(probe(bytearray_view.as_ptr(), bytes_view.as_ptr(), 0), 1);
            // Large arbitrary-precision endpoints with a bounded 64-item
            // fallback cost; this checks arithmetic membership, not timing.
            let start = mapping_owned(
                &py,
                &mut owners,
                int_bits_from_bigint(&py, BigInt::from(1) << 100usize),
            );
            let stop = mapping_owned(
                &py,
                &mut owners,
                int_bits_from_bigint(&py, (BigInt::from(1) << 100usize) + 128),
            );
            let range = mapping_owned(
                &py,
                &mut owners,
                crate::molt_range_new(start, stop, MoltObject::from_int(2).bits()),
            );
            let range_view = mapping_view(range);
            let negative = mapping_view(MoltObject::from_int(-1).bits());
            let start_view = mapping_view(start);
            assert_eq!(probe(range_view.as_ptr(), negative.as_ptr(), 0), 0);
            assert_eq!(probe(range_view.as_ptr(), start_view.as_ptr(), 0), 1);
        }
        assert!(!crate::exception_pending(&py));
    });
}

#[test]
fn c_sequence_search_owns_live_elements_across_clear_shrink_and_growth() {
    let _transaction = crate::test_support::RuntimeTestTransaction::with_gc_isolation();
    molt_cpython_abi_test_support::link();
    with_gil(|py| unsafe {
        let mut owners = Vec::new();
        let needle_bits = MoltObject::from_int(99).bits();
        let needle = mapping_view(needle_bits);
        // Both C headers, then runtime descriptors with default/explicit stop.
        for consumer in 0..4 {
            for operation in 0..3 {
                for action in 1..=3 {
                    let class = mapping_owned(
                        &py,
                        &mut owners,
                        inquiry_class(&py, crate::builtin_classes(&py).object, b"__len__"),
                    );
                    inquiry_binary_method(
                        &py,
                        class,
                        b"__eq__",
                        inquiry_mutating_equal as *const (),
                    );
                    let element = crate::alloc_instance_for_class(
                        &py,
                        MoltObject::from_bits(class).as_ptr().unwrap(),
                    );
                    let contents = if action == 3 {
                        vec![element]
                    } else {
                        vec![element, needle_bits, needle_bits]
                    };
                    let list = mapping_owned(
                        &py,
                        &mut owners,
                        MoltObject::from_ptr(crate::alloc_list(&py, &contents)).bits(),
                    );
                    dec_ref_bits(&py, element); // list owns the only non-callback instance edge
                    set_value(&py, class, b"inquiry_result", list);
                    set_value(
                        &py,
                        class,
                        b"inquiry_action",
                        MoltObject::from_int(action).bits(),
                    );
                    let view = mapping_view(list);
                    let result = if consumer < 2 {
                        let probe = if consumer == 0 {
                            molt_linked_type_identity_probe_sequence_search
                        } else {
                            molt_public_type_identity_probe_sequence_search
                        };
                        probe(view.as_ptr(), needle.as_ptr(), operation)
                    } else {
                        let bits = match operation {
                            0 => crate::molt_contains(list, needle_bits),
                            1 => crate::molt_list_count(list, needle_bits),
                            _ if consumer == 2 => crate::molt_list_index(list, needle_bits),
                            _ => crate::molt_list_index_range(
                                list,
                                needle_bits,
                                MoltObject::from_int(0).bits(),
                                MoltObject::from_int(64).bits(),
                            ),
                        };
                        if crate::exception_pending(&py) {
                            assert!(!errors::PyErr_Occurred().is_null());
                            dec_ref_bits(&py, bits);
                            -1
                        } else {
                            let value = MoltObject::from_bits(bits);
                            let result = value
                                .as_int()
                                .or_else(|| value.as_bool().map(i64::from))
                                .unwrap() as isize;
                            dec_ref_bits(&py, bits);
                            result
                        }
                    };
                    let expected = if action == 3 {
                        1
                    } else if operation == 2 {
                        -1
                    } else {
                        0
                    };
                    assert_eq!(
                        result, expected,
                        "consumer={consumer} operation={operation} action={action}"
                    );
                    if expected < 0 {
                        assert_eq!(
                            errors::PyErr_ExceptionMatches(
                                (&raw mut molt_cpython_abi::abi_types::PyExc_ValueError).cast()
                            ),
                            1
                        );
                        errors::PyErr_Clear();
                    } else {
                        assert!(errors::PyErr_Occurred().is_null());
                    }
                    set_value(&py, class, b"inquiry_result", MoltObject::none().bits());
                }
            }
        }
        // Preserve the old ABI-only fixture's equal-but-distinct heap-string
        // and absent-index coverage using actual runtime objects and owners.
        let left = mapping_owned(
            &py,
            &mut owners,
            MoltObject::from_ptr(crate::object::builders::alloc_string_nointern(
                &py, b"dtype",
            ))
            .bits(),
        );
        let right = mapping_owned(
            &py,
            &mut owners,
            MoltObject::from_ptr(crate::object::builders::alloc_string_nointern(
                &py, b"dtype",
            ))
            .bits(),
        );
        assert_ne!(left, right);
        let list = mapping_owned(
            &py,
            &mut owners,
            MoltObject::from_ptr(crate::alloc_list(&py, &[left])).bits(),
        );
        let view = mapping_view(list);
        let right = mapping_view(right);
        for probe in [
            molt_linked_type_identity_probe_sequence_search,
            molt_public_type_identity_probe_sequence_search,
        ] {
            assert_eq!(probe(view.as_ptr(), right.as_ptr(), 0), 1);
            assert_eq!(probe(view.as_ptr(), right.as_ptr(), 1), 1);
            assert_eq!(probe(view.as_ptr(), right.as_ptr(), 2), 0);
            assert_eq!(probe(view.as_ptr(), needle.as_ptr(), 2), -1);
            assert_eq!(
                errors::PyErr_ExceptionMatches(
                    (&raw mut molt_cpython_abi::abi_types::PyExc_ValueError).cast()
                ),
                1
            );
            errors::PyErr_Clear();
        }
        // A container may also be its own needle; identity must terminate
        // before recursive equality and cleanup must retire the cyclic edge.
        crate::molt_list_append(list, list);
        for probe in [
            molt_linked_type_identity_probe_sequence_search,
            molt_public_type_identity_probe_sequence_search,
        ] {
            assert_eq!(probe(view.as_ptr(), view.as_ptr(), 0), 1);
            assert_eq!(probe(view.as_ptr(), view.as_ptr(), 1), 1);
            assert_eq!(probe(view.as_ptr(), view.as_ptr(), 2), 1);
        }
        crate::molt_list_clear(list);
        assert!(!crate::exception_pending(&py));
    });
}

#[test]
fn c_sequence_search_preserves_original_iteration_equality_and_truth_errors() {
    let _transaction = crate::test_support::RuntimeTestTransaction::with_gc_isolation();
    molt_cpython_abi_test_support::link();
    with_gil(|py| unsafe {
        let mut owners = Vec::new();
        crate::raise_exception::<()>(&py, "LookupError", "original inquiry failure");
        let failure = mapping_owned(&py, &mut owners, crate::molt_exception_last());
        crate::molt_exception_clear();
        let failure_view = mapping_view(failure);
        let needle = mapping_view(MoltObject::from_int(99).bits());
        for probe in [
            molt_linked_type_identity_probe_sequence_search,
            molt_public_type_identity_probe_sequence_search,
        ] {
            for stage in 0..5 {
                let class = mapping_owned(
                    &py,
                    &mut owners,
                    inquiry_class(&py, crate::builtin_classes(&py).object, b"__iter__"),
                );
                let instance = mapping_owned(
                    &py,
                    &mut owners,
                    crate::alloc_instance_for_class(
                        &py,
                        MoltObject::from_bits(class).as_ptr().unwrap(),
                    ),
                );
                set_value(&py, class, b"inquiry_result", failure);
                let container = match stage {
                    0 => {
                        set_method(&py, class, b"__iter__", inquiry_raise_original as *const ());
                        instance
                    }
                    1 => {
                        set_method(&py, class, b"__iter__", inquiry_self as *const ());
                        set_method(&py, class, b"__next__", inquiry_raise_original as *const ());
                        instance
                    }
                    2 => {
                        inquiry_binary_method(
                            &py,
                            class,
                            b"__eq__",
                            inquiry_mutating_equal as *const (),
                        );
                        set_value(&py, class, b"inquiry_error", failure);
                        mapping_owned(
                            &py,
                            &mut owners,
                            MoltObject::from_ptr(crate::alloc_list(&py, &[])).bits(),
                        )
                    }
                    3 => {
                        set_method(&py, class, b"__bool__", inquiry_raise_original as *const ());
                        let holder = mapping_owned(
                            &py,
                            &mut owners,
                            inquiry_class(&py, crate::builtin_classes(&py).object, b"__len__"),
                        );
                        inquiry_binary_method(
                            &py,
                            holder,
                            b"__contains__",
                            inquiry_binary_result as *const (),
                        );
                        set_value(&py, holder, b"inquiry_result", instance);
                        mapping_owned(
                            &py,
                            &mut owners,
                            crate::alloc_instance_for_class(
                                &py,
                                MoltObject::from_bits(holder).as_ptr().unwrap(),
                            ),
                        )
                    }
                    _ => {
                        inquiry_binary_method(
                            &py,
                            class,
                            b"__contains__",
                            inquiry_binary_raise_original as *const (),
                        );
                        instance
                    }
                };
                let view = mapping_view(container);
                for operation in 0..if stage >= 3 { 1 } else { 3 } {
                    if stage == 2 {
                        crate::molt_list_append(container, instance);
                        set_value(&py, class, b"inquiry_result", container);
                        set_value(
                            &py,
                            class,
                            b"inquiry_action",
                            MoltObject::from_int(4).bits(),
                        );
                    }
                    assert_eq!(probe(view.as_ptr(), needle.as_ptr(), operation), -1);
                    let observed =
                        refcount::OwnedPyObject::from_owned(errors::PyErr_GetRaisedException());
                    assert_eq!(
                        observed.as_ptr(),
                        failure_view.as_ptr(),
                        "stage={stage} operation={operation}"
                    );
                    assert!(!crate::exception_pending(&py));
                }
                if stage == 2 {
                    set_value(&py, class, b"inquiry_result", MoltObject::none().bits());
                }
            }
        }
    });
}

extern "C" fn inquiry_memory_equal(receiver: u64, candidate: u64) -> u64 {
    with_gil(|py| {
        let log = crate::builtins::exceptions::ExceptionValue::adopt(&py, inquiry_result(receiver));
        crate::molt_list_append(log.bits(), candidate);
        MoltObject::from_bool(MoltObject::from_bits(candidate).as_int() == Some(98)).bits()
    })
}

extern "C" fn inquiry_memory_release_equal(receiver: u64, candidate: u64) -> u64 {
    with_gil(|py| unsafe {
        let log = crate::builtins::exceptions::ExceptionValue::adopt(&py, inquiry_result(receiver));
        let view =
            crate::object::seq_access::item(MoltObject::from_bits(log.bits()).as_ptr().unwrap(), 0)
                .unwrap();
        crate::molt_list_append(log.bits(), candidate);
        crate::molt_memoryview_release(view);
        MoltObject::from_bool(false).bits()
    })
}

#[test]
fn c_sequence_memoryview_membership_uses_typed_elements_and_original_errors() {
    let _transaction = crate::test_support::RuntimeTestTransaction::with_gc_isolation();
    molt_cpython_abi_test_support::link();
    with_gil(|py| unsafe {
        let mut owners = Vec::new();
        let bytes = mapping_owned(
            &py,
            &mut owners,
            MoltObject::from_ptr(alloc_bytes(&py, b"abcd")).bits(),
        );
        let view = mapping_owned(&py, &mut owners, crate::molt_memoryview_new(bytes));
        let substring = mapping_owned(
            &py,
            &mut owners,
            MoltObject::from_ptr(alloc_bytes(&py, b"ab")).bits(),
        );
        let step = mapping_owned(
            &py,
            &mut owners,
            crate::molt_slice_new(
                MoltObject::none().bits(),
                MoltObject::none().bits(),
                MoltObject::from_int(-2).bits(),
            ),
        );
        let strided = mapping_owned(&py, &mut owners, crate::molt_index(view, step));
        let words = [300u16.to_ne_bytes(), 700u16.to_ne_bytes()].concat();
        let packed = mapping_owned(
            &py,
            &mut owners,
            MoltObject::from_ptr(alloc_bytes(&py, &words)).bits(),
        );
        let packed_view = mapping_owned(&py, &mut owners, crate::molt_memoryview_new(packed));
        let format = mapping_owned(
            &py,
            &mut owners,
            MoltObject::from_ptr(alloc_string(&py, b"H")).bits(),
        );
        let typed = mapping_owned(
            &py,
            &mut owners,
            crate::molt_memoryview_cast(
                packed_view,
                format,
                MoltObject::none().bits(),
                MoltObject::from_bool(false).bits(),
            ),
        );
        assert!(!crate::exception_pending(&py));
        for probe in [
            molt_linked_type_identity_probe_sequence_search,
            molt_public_type_identity_probe_sequence_search,
        ] {
            for (container, needle, expected) in [
                (view, MoltObject::from_int(300).bits(), 0),
                (view, substring, 0),
                (view, MoltObject::from_int(99).bits(), 1),
                (strided, MoltObject::from_int(98).bits(), 1),
                (strided, MoltObject::from_int(97).bits(), 0),
                (typed, MoltObject::from_int(300).bits(), 1),
                (typed, MoltObject::from_int(44).bits(), 0),
            ] {
                let container = mapping_view(container);
                let needle = mapping_view(needle);
                assert_eq!(probe(container.as_ptr(), needle.as_ptr(), 0), expected);
                assert!(errors::PyErr_Occurred().is_null());
            }
            let typed_view = mapping_view(typed);
            let last = mapping_view(MoltObject::from_int(700).bits());
            assert_eq!(probe(typed_view.as_ptr(), last.as_ptr(), 1), 1);
            assert_eq!(probe(typed_view.as_ptr(), last.as_ptr(), 2), 1);
            let class = mapping_owned(
                &py,
                &mut owners,
                inquiry_class(&py, crate::builtin_classes(&py).object, b"__len__"),
            );
            inquiry_binary_method(&py, class, b"__eq__", inquiry_memory_equal as *const ());
            let log = mapping_owned(
                &py,
                &mut owners,
                MoltObject::from_ptr(crate::alloc_list(&py, &[])).bits(),
            );
            set_value(&py, class, b"inquiry_result", log);
            let needle = mapping_owned(
                &py,
                &mut owners,
                crate::alloc_instance_for_class(
                    &py,
                    MoltObject::from_bits(class).as_ptr().unwrap(),
                ),
            );
            let needle_view = mapping_view(needle);
            let container = mapping_view(view);
            assert_eq!(probe(container.as_ptr(), needle_view.as_ptr(), 0), 1);
            let log_ptr = MoltObject::from_bits(log).as_ptr().unwrap();
            assert_eq!(crate::object::seq_access::len(log_ptr), 2);
            assert_eq!(
                crate::object::seq_access::item(log_ptr, 0),
                Some(MoltObject::from_int(97).bits())
            );
            assert_eq!(
                crate::object::seq_access::item(log_ptr, 1),
                Some(MoltObject::from_int(98).bits())
            );
            crate::raise_exception::<()>(&py, "LookupError", "memoryview equality identity");
            let failure = mapping_owned(&py, &mut owners, crate::molt_exception_last());
            crate::molt_exception_clear();
            set_value(&py, class, b"inquiry_result", failure);
            inquiry_binary_method(
                &py,
                class,
                b"__eq__",
                inquiry_binary_raise_original as *const (),
            );
            assert_eq!(probe(container.as_ptr(), needle_view.as_ptr(), 0), -1);
            let error = refcount::OwnedPyObject::from_owned(errors::PyErr_GetRaisedException());
            let original = mapping_view(failure);
            assert_eq!(error.as_ptr(), original.as_ptr());
            let released = mapping_owned(&py, &mut owners, crate::molt_memoryview_new(bytes));
            crate::molt_memoryview_release(released);
            let released_view = mapping_view(released);
            assert_eq!(probe(released_view.as_ptr(), last.as_ptr(), 0), -1);
            assert_eq!(
                errors::PyErr_ExceptionMatches(
                    (&raw mut molt_cpython_abi::abi_types::PyExc_ValueError).cast()
                ),
                1
            );
            errors::PyErr_Clear();
            // Releasing the view during equality fails only if another item
            // remains. At exhaustion no further element read is attempted.
            inquiry_binary_method(
                &py,
                class,
                b"__eq__",
                inquiry_memory_release_equal as *const (),
            );
            for payload in [b"a".as_slice(), b"ab".as_slice()] {
                let data = mapping_owned(
                    &py,
                    &mut owners,
                    MoltObject::from_ptr(alloc_bytes(&py, payload)).bits(),
                );
                let releasing = mapping_owned(&py, &mut owners, crate::molt_memoryview_new(data));
                let log = mapping_owned(
                    &py,
                    &mut owners,
                    MoltObject::from_ptr(crate::alloc_list(&py, &[releasing])).bits(),
                );
                set_value(&py, class, b"inquiry_result", log);
                let releasing_view = mapping_view(releasing);
                assert_eq!(
                    probe(releasing_view.as_ptr(), needle_view.as_ptr(), 0),
                    if payload.len() == 1 { 0 } else { -1 }
                );
                assert_eq!(
                    crate::object::seq_access::len(MoltObject::from_bits(log).as_ptr().unwrap()),
                    2
                );
                if payload.len() != 1 {
                    assert_eq!(
                        errors::PyErr_ExceptionMatches(
                            (&raw mut molt_cpython_abi::abi_types::PyExc_ValueError).cast()
                        ),
                        1
                    );
                    errors::PyErr_Clear();
                } else {
                    assert!(errors::PyErr_Occurred().is_null());
                }
            }
            // CPython validates rank and format syntax when creating an iterator,
            // but an unsupported one-character scalar format fails only on read.
            for (format, length, rank, creation_error, read_error) in [
                (c"Z", 0isize, 1, false, false),
                (c"Z", 1, 1, false, true),
                (c"ZZ", 0, 1, true, false),
                (c"B", 2, 2, true, false),
            ] {
                let mut data = [1u8, 2];
                let mut shape = [if rank == 1 { length } else { 1 }, 2];
                let mut strides = [if rank == 1 { 1 } else { 2 }, 1];
                let mut descriptor: molt_cpython_abi::abi_types::Py_buffer = std::mem::zeroed();
                descriptor.buf = data.as_mut_ptr().cast();
                descriptor.len = length;
                descriptor.itemsize = 1;
                descriptor.readonly = 1;
                descriptor.ndim = rank;
                descriptor.format = format.as_ptr().cast_mut();
                descriptor.shape = shape.as_mut_ptr();
                descriptor.strides = strides.as_mut_ptr();
                let raw = refcount::OwnedPyObject::from_owned(
                    molt_cpython_abi::api::memory::PyMemoryView_FromBuffer(&mut descriptor),
                );
                assert!(!raw.as_ptr().is_null());
                let iter =
                    refcount::OwnedPyObject::from_owned(object::PyObject_GetIter(raw.as_ptr()));
                if creation_error {
                    assert!(iter.as_ptr().is_null());
                } else {
                    assert!(!iter.as_ptr().is_null());
                    let item =
                        refcount::OwnedPyObject::from_owned(object::PyIter_Next(iter.as_ptr()));
                    assert!(item.as_ptr().is_null());
                }
                assert_eq!(
                    errors::PyErr_Occurred().is_null(),
                    !(creation_error || read_error)
                );
                if creation_error || read_error {
                    assert_eq!(
                        errors::PyErr_ExceptionMatches(
                            (&raw mut molt_cpython_abi::abi_types::PyExc_NotImplementedError)
                                .cast()
                        ),
                        1
                    );
                    errors::PyErr_Clear();
                }
                assert_eq!(
                    probe(raw.as_ptr(), last.as_ptr(), 0),
                    if creation_error || read_error { -1 } else { 0 }
                );
                if creation_error || read_error {
                    assert_eq!(
                        errors::PyErr_ExceptionMatches(
                            (&raw mut molt_cpython_abi::abi_types::PyExc_NotImplementedError)
                                .cast()
                        ),
                        1
                    );
                    errors::PyErr_Clear();
                }
            }
        }
        assert!(!crate::exception_pending(&py));
    });
}

#[test]
fn c_sequence_bytes_membership_shares_index_and_simple_buffer_admission() {
    let _transaction = crate::test_support::RuntimeTestTransaction::with_gc_isolation();
    molt_cpython_abi_test_support::link();
    with_gil(|py| unsafe {
        let mut owners = Vec::new();
        let bytes = mapping_owned(
            &py,
            &mut owners,
            MoltObject::from_ptr(alloc_bytes(&py, b"\x01abc")).bits(),
        );
        let bytearray = mapping_owned(
            &py,
            &mut owners,
            MoltObject::from_ptr(crate::alloc_bytearray(&py, b"\x01abc")).bits(),
        );
        let pattern = mapping_owned(
            &py,
            &mut owners,
            MoltObject::from_ptr(alloc_bytes(&py, b"ab")).bits(),
        );
        let buffer = mapping_owned(&py, &mut owners, crate::molt_memoryview_new(pattern));
        let index_class = mapping_owned(
            &py,
            &mut owners,
            inquiry_class(&py, crate::builtin_classes(&py).object, b"__index__"),
        );
        set_value(
            &py,
            index_class,
            b"inquiry_result",
            MoltObject::from_int(98).bits(),
        );
        let index = mapping_owned(
            &py,
            &mut owners,
            crate::alloc_instance_for_class(
                &py,
                MoltObject::from_bits(index_class).as_ptr().unwrap(),
            ),
        );
        let step = mapping_owned(
            &py,
            &mut owners,
            crate::molt_slice_new(
                MoltObject::none().bits(),
                MoltObject::none().bits(),
                MoltObject::from_int(2).bits(),
            ),
        );
        let base_view = mapping_owned(&py, &mut owners, crate::molt_memoryview_new(bytes));
        let noncontiguous = mapping_owned(&py, &mut owners, crate::molt_index(base_view, step));
        for probe in [
            molt_linked_type_identity_probe_sequence_search,
            molt_public_type_identity_probe_sequence_search,
        ] {
            for container in [bytes, bytearray] {
                let container = mapping_view(container);
                for needle in [MoltObject::from_bool(true).bits(), index, buffer] {
                    let needle = mapping_view(needle);
                    assert_eq!(probe(container.as_ptr(), needle.as_ptr(), 0), 1);
                    assert!(errors::PyErr_Occurred().is_null());
                }
                let noncontiguous = mapping_view(noncontiguous);
                assert_eq!(probe(container.as_ptr(), noncontiguous.as_ptr(), 0), -1);
                assert_eq!(
                    errors::PyErr_ExceptionMatches(
                        (&raw mut molt_cpython_abi::abi_types::PyExc_BufferError).cast()
                    ),
                    1
                );
                errors::PyErr_Clear();
                let huge = mapping_owned(
                    &py,
                    &mut owners,
                    int_bits_from_bigint(&py, BigInt::from(1) << 100usize),
                );
                let huge = mapping_view(huge);
                assert_eq!(probe(container.as_ptr(), huge.as_ptr(), 0), -1);
                assert_eq!(
                    errors::PyErr_ExceptionMatches(
                        (&raw mut molt_cpython_abi::abi_types::PyExc_ValueError).cast()
                    ),
                    1
                );
                errors::PyErr_Clear();
                set_method(&py, index_class, b"__index__", inquiry_failure as *const ());
                let index = mapping_view(index);
                assert_eq!(probe(container.as_ptr(), index.as_ptr(), 0), -1);
                assert_eq!(
                    errors::PyErr_ExceptionMatches(
                        (&raw mut molt_cpython_abi::abi_types::PyExc_TypeError).cast()
                    ),
                    1
                );
                errors::PyErr_Clear();
                set_method(&py, index_class, b"__index__", inquiry_result as *const ());
            }
        }
        assert!(!crate::exception_pending(&py));
    });
}

#[test]
fn c_sequence_set_membership_distinguishes_python_and_exact_key_policies() {
    let _transaction = crate::test_support::RuntimeTestTransaction::with_gc_isolation();
    molt_cpython_abi_test_support::link();
    with_gil(|py| unsafe {
        let mut owners = Vec::new();
        let classes = crate::builtin_classes(&py);
        let singleton = mapping_owned(
            &py,
            &mut owners,
            MoltObject::from_ptr(crate::alloc_tuple(&py, &[MoltObject::from_int(1).bits()])).bits(),
        );
        let frozen = mapping_owned(
            &py,
            &mut owners,
            construct(&py, classes.frozenset, singleton),
        );
        let items = mapping_owned(
            &py,
            &mut owners,
            MoltObject::from_ptr(crate::alloc_tuple(&py, &[frozen])).bits(),
        );
        let needle = mapping_owned(&py, &mut owners, construct(&py, classes.set, singleton));
        let needle_view = mapping_view(needle);
        for container_class in [classes.set, classes.frozenset] {
            let container = mapping_owned(&py, &mut owners, construct(&py, container_class, items));
            let container_view = mapping_view(container);
            for probe in [
                molt_linked_type_identity_probe_sequence_search,
                molt_public_type_identity_probe_sequence_search,
            ] {
                assert_eq!(probe(container_view.as_ptr(), needle_view.as_ptr(), 0), 1);
                assert_eq!(
                    MoltObject::from_bits(crate::molt_set_contains(container, needle)).as_bool(),
                    Some(true)
                );
                assert_eq!(probe(container_view.as_ptr(), needle_view.as_ptr(), 3), -1);
                assert_eq!(
                    errors::PyErr_ExceptionMatches(
                        (&raw mut molt_cpython_abi::abi_types::PyExc_TypeError).cast()
                    ),
                    1
                );
                errors::PyErr_Clear();
                let custom = mapping_owned(
                    &py,
                    &mut owners,
                    inquiry_class(&py, classes.set, b"__hash__"),
                );
                if container_class == classes.set {
                    assert_eq!(probe(container_view.as_ptr(), needle_view.as_ptr(), 4), -1);
                    assert_eq!(
                        errors::PyErr_ExceptionMatches(
                            (&raw mut molt_cpython_abi::abi_types::PyExc_TypeError).cast()
                        ),
                        1
                    );
                    errors::PyErr_Clear();
                    assert_eq!(probe(container_view.as_ptr(), needle_view.as_ptr(), 0), 1);
                }
                let hash = mapping_owned(&py, &mut owners, crate::molt_hash_builtin(frozen));
                set_value(&py, custom, b"inquiry_result", hash);
                let hashable = mapping_owned(&py, &mut owners, construct(&py, custom, singleton));
                let hashable_view = mapping_view(hashable);
                assert_eq!(probe(container_view.as_ptr(), hashable_view.as_ptr(), 0), 1);
                assert_eq!(probe(container_view.as_ptr(), hashable_view.as_ptr(), 3), 1);
                crate::raise_exception::<()>(&py, "LookupError", "set hash identity");
                let failure = mapping_owned(&py, &mut owners, crate::molt_exception_last());
                crate::molt_exception_clear();
                set_value(&py, custom, b"inquiry_result", failure);
                set_method(
                    &py,
                    custom,
                    b"__hash__",
                    inquiry_raise_original as *const (),
                );
                let original = mapping_view(failure);
                for operation in [0, 3] {
                    assert_eq!(
                        probe(container_view.as_ptr(), hashable_view.as_ptr(), operation),
                        -1
                    );
                    let error =
                        refcount::OwnedPyObject::from_owned(errors::PyErr_GetRaisedException());
                    assert_eq!(error.as_ptr(), original.as_ptr());
                }
            }
        }
        assert!(!crate::exception_pending(&py));
    });
}

extern "C" fn inquiry_set_collision_hash(_receiver: u64) -> u64 {
    MoltObject::from_int(123).bits()
}

extern "C" fn inquiry_set_mutating_type_error(receiver: u64, needle: u64) -> u64 {
    with_gil(|py| unsafe {
        let state_owner =
            crate::builtins::exceptions::ExceptionValue::adopt(&py, inquiry_result(receiver));
        let state = MoltObject::from_bits(state_owner.bits()).as_ptr().unwrap();
        let container = crate::object::seq_access::item(state, 0).unwrap();
        let replacement = crate::object::seq_access::item(state, 1).unwrap();
        let failure = crate::object::seq_access::item(state, 2).unwrap();
        let log = crate::object::seq_access::item(state, 3).unwrap();
        crate::molt_list_append(log, needle);
        crate::molt_set_clear(container);
        crate::molt_set_add(container, replacement);
        crate::molt_raise(failure)
    })
}

#[test]
fn c_set_membership_reenters_after_mutating_equality_type_error_only_for_python() {
    let _transaction = crate::test_support::RuntimeTestTransaction::with_gc_isolation();
    molt_cpython_abi_test_support::link();
    with_gil(|py| unsafe {
        let mut owners = Vec::new();
        let classes = crate::builtin_classes(&py);
        let singleton = mapping_owned(
            &py,
            &mut owners,
            MoltObject::from_ptr(crate::alloc_tuple(&py, &[MoltObject::from_int(1).bits()])).bits(),
        );
        let empty = mapping_owned(
            &py,
            &mut owners,
            MoltObject::from_ptr(crate::alloc_tuple(&py, &[])).bits(),
        );
        let frozen = mapping_owned(
            &py,
            &mut owners,
            construct(&py, classes.frozenset, singleton),
        );
        let needle_class = mapping_owned(
            &py,
            &mut owners,
            inquiry_class(&py, classes.set, b"__hash__"),
        );
        set_method(
            &py,
            needle_class,
            b"__hash__",
            inquiry_set_collision_hash as *const (),
        );
        let needle = mapping_owned(&py, &mut owners, construct(&py, needle_class, singleton));
        let needle_view = mapping_view(needle);
        crate::raise_exception::<()>(&py, "TypeError", "set equality identity");
        let failure = mapping_owned(&py, &mut owners, crate::molt_exception_last());
        crate::molt_exception_clear();
        let original = mapping_view(failure);
        for probe in [
            molt_linked_type_identity_probe_sequence_search,
            molt_public_type_identity_probe_sequence_search,
        ] {
            for operation in [0, 3, 4] {
                let container = mapping_owned(&py, &mut owners, construct(&py, classes.set, empty));
                let log = mapping_owned(
                    &py,
                    &mut owners,
                    MoltObject::from_ptr(crate::alloc_list(&py, &[])).bits(),
                );
                let state = mapping_owned(
                    &py,
                    &mut owners,
                    MoltObject::from_ptr(crate::alloc_tuple(
                        &py,
                        &[container, frozen, failure, log],
                    ))
                    .bits(),
                );
                let stored_class = mapping_owned(
                    &py,
                    &mut owners,
                    inquiry_class(&py, classes.object, b"__hash__"),
                );
                inquiry_binary_method(
                    &py,
                    stored_class,
                    b"__eq__",
                    inquiry_set_mutating_type_error as *const (),
                );
                set_method(
                    &py,
                    stored_class,
                    b"__hash__",
                    inquiry_set_collision_hash as *const (),
                );
                set_value(&py, stored_class, b"inquiry_result", state);
                let stored = mapping_owned(
                    &py,
                    &mut owners,
                    crate::alloc_instance_for_class(
                        &py,
                        MoltObject::from_bits(stored_class).as_ptr().unwrap(),
                    ),
                );
                crate::molt_set_add(container, stored);
                assert!(!crate::exception_pending(&py));
                let container_view = mapping_view(container);
                assert_eq!(
                    probe(container_view.as_ptr(), needle_view.as_ptr(), operation),
                    if operation == 0 { 1 } else { -1 }
                );
                if operation != 0 {
                    let error =
                        refcount::OwnedPyObject::from_owned(errors::PyErr_GetRaisedException());
                    assert_eq!(error.as_ptr(), original.as_ptr());
                } else {
                    assert!(errors::PyErr_Occurred().is_null());
                }
                assert_eq!(
                    crate::object::seq_access::len(MoltObject::from_bits(log).as_ptr().unwrap()),
                    1
                );
                assert_eq!(
                    crate::builtins::containers::set_len(
                        MoltObject::from_bits(container).as_ptr().unwrap()
                    ),
                    1
                );
                let frozen_view = mapping_view(frozen);
                assert_eq!(probe(container_view.as_ptr(), frozen_view.as_ptr(), 3), 1);
                assert!(errors::PyErr_Occurred().is_null());
            }
        }
    });
}

extern "C" fn inquiry_mutating_bound(receiver: u64) -> u64 {
    with_gil(|py| unsafe {
        let state_owner =
            crate::builtins::exceptions::ExceptionValue::adopt(&py, inquiry_result(receiver));
        let state = MoltObject::from_bits(state_owner.bits()).as_ptr().unwrap();
        let list = crate::object::seq_access::item(state, 0).unwrap();
        let log = crate::object::seq_access::item(state, 1).unwrap();
        let marker = crate::object::seq_access::item(state, 2).unwrap();
        let result = crate::object::seq_access::item(state, 3).unwrap();
        crate::molt_list_append(log, marker);
        crate::molt_list_append(list, MoltObject::from_int(99).bits());
        inc_ref_bits(&py, result);
        result
    })
}

#[test]
fn sequence_search_bounds_clip_at_target_width_and_stop_at_first_callback_error() {
    let _transaction = crate::test_support::RuntimeTestTransaction::with_gc_isolation();
    with_gil(|py| unsafe {
        let mut owners = Vec::new();
        let value = MoltObject::from_int(99).bits();
        let list = mapping_owned(
            &py,
            &mut owners,
            MoltObject::from_ptr(crate::alloc_list(&py, &[value])).bits(),
        );
        let tuple = mapping_owned(
            &py,
            &mut owners,
            MoltObject::from_ptr(crate::alloc_tuple(&py, &[value])).bits(),
        );
        let high = mapping_owned(
            &py,
            &mut owners,
            int_bits_from_bigint(&py, BigInt::from(1) << 100usize),
        );
        let low = mapping_owned(
            &py,
            &mut owners,
            int_bits_from_bigint(&py, -(BigInt::from(1) << 100usize)),
        );
        for (container, search) in [
            (
                list,
                crate::molt_list_index_range as extern "C" fn(u64, u64, u64, u64) -> u64,
            ),
            (
                tuple,
                crate::molt_tuple_index_range as extern "C" fn(u64, u64, u64, u64) -> u64,
            ),
        ] {
            for (start, stop) in [(MoltObject::from_int(0).bits(), high), (low, high)] {
                assert_eq!(
                    MoltObject::from_bits(search(container, value, start, stop)).as_int(),
                    Some(0)
                );
                assert!(!crate::exception_pending(&py));
            }
            let start_class = mapping_owned(
                &py,
                &mut owners,
                inquiry_class(&py, crate::builtin_classes(&py).object, b"__index__"),
            );
            let stop_class = mapping_owned(
                &py,
                &mut owners,
                inquiry_class(&py, crate::builtin_classes(&py).object, b"__index__"),
            );
            crate::raise_exception::<()>(&py, "LookupError", "first bound identity");
            let failure = mapping_owned(&py, &mut owners, crate::molt_exception_last());
            crate::molt_exception_clear();
            set_value(&py, start_class, b"inquiry_result", failure);
            set_method(
                &py,
                start_class,
                b"__index__",
                inquiry_raise_original as *const (),
            );
            set_value(&py, stop_class, b"inquiry_result", high);
            set_method(
                &py,
                stop_class,
                b"__index__",
                mapping_method_result as *const (),
            );
            let start = mapping_owned(
                &py,
                &mut owners,
                crate::alloc_instance_for_class(
                    &py,
                    MoltObject::from_bits(start_class).as_ptr().unwrap(),
                ),
            );
            let stop = mapping_owned(
                &py,
                &mut owners,
                crate::alloc_instance_for_class(
                    &py,
                    MoltObject::from_bits(stop_class).as_ptr().unwrap(),
                ),
            );
            MAPPING_METHOD_CALLS.store(0, std::sync::atomic::Ordering::Relaxed);
            let result = search(container, value, start, stop);
            dec_ref_bits(&py, result);
            let error = refcount::OwnedPyObject::from_owned(errors::PyErr_GetRaisedException());
            let original = mapping_view(failure);
            assert_eq!(error.as_ptr(), original.as_ptr());
            assert_eq!(
                MAPPING_METHOD_CALLS.load(std::sync::atomic::Ordering::Relaxed),
                0
            );
        }
        // Both successful callbacks mutate before either negative bound is
        // normalized, and the callback log proves start-before-stop order.
        for (start_result, stop_result, expected) in [
            (MoltObject::from_int(-1).bits(), high, 2),
            (
                MoltObject::from_int(-2).bits(),
                MoltObject::from_int(-1).bits(),
                1,
            ),
        ] {
            let list = mapping_owned(
                &py,
                &mut owners,
                MoltObject::from_ptr(crate::alloc_list(&py, &[MoltObject::from_int(0).bits()]))
                    .bits(),
            );
            let log = mapping_owned(
                &py,
                &mut owners,
                MoltObject::from_ptr(crate::alloc_list(&py, &[])).bits(),
            );
            let mut bounds = Vec::new();
            for (marker, result) in [(0, start_result), (1, stop_result)] {
                let class = mapping_owned(
                    &py,
                    &mut owners,
                    inquiry_class(&py, crate::builtin_classes(&py).object, b"__index__"),
                );
                let state = mapping_owned(
                    &py,
                    &mut owners,
                    MoltObject::from_ptr(crate::alloc_tuple(
                        &py,
                        &[list, log, MoltObject::from_int(marker).bits(), result],
                    ))
                    .bits(),
                );
                set_value(&py, class, b"inquiry_result", state);
                set_method(
                    &py,
                    class,
                    b"__index__",
                    inquiry_mutating_bound as *const (),
                );
                bounds.push(mapping_owned(
                    &py,
                    &mut owners,
                    crate::alloc_instance_for_class(
                        &py,
                        MoltObject::from_bits(class).as_ptr().unwrap(),
                    ),
                ));
            }
            let result = crate::molt_list_index_range(list, value, bounds[0], bounds[1]);
            assert_eq!(MoltObject::from_bits(result).as_int(), Some(expected));
            assert!(!crate::exception_pending(&py));
            let log = MoltObject::from_bits(log).as_ptr().unwrap();
            assert_eq!(crate::object::seq_access::len(log), 2);
            assert_eq!(
                crate::object::seq_access::item(log, 0),
                Some(MoltObject::from_int(0).bits())
            );
            assert_eq!(
                crate::object::seq_access::item(log, 1),
                Some(MoltObject::from_int(1).bits())
            );
        }
    });
}

static SET_DISCARD_HASH_CALLS: std::sync::atomic::AtomicUsize =
    std::sync::atomic::AtomicUsize::new(0);
static SET_DISCARD_HASH_FAIL_AT: std::sync::atomic::AtomicUsize =
    std::sync::atomic::AtomicUsize::new(usize::MAX);

extern "C" fn inquiry_discard_hash(receiver: u64) -> u64 {
    use std::sync::atomic::Ordering::Relaxed;
    let call = SET_DISCARD_HASH_CALLS.fetch_add(1, Relaxed) + 1;
    if call >= SET_DISCARD_HASH_FAIL_AT.load(Relaxed) {
        inquiry_raise_original(receiver)
    } else {
        MoltObject::from_int(123).bits()
    }
}

#[test]
fn c_set_discard_hashes_once_and_preserves_first_hash_failure() {
    use std::sync::atomic::Ordering::Relaxed;
    let _transaction = crate::test_support::RuntimeTestTransaction::with_gc_isolation();
    molt_cpython_abi_test_support::link();
    with_gil(|py| unsafe {
        let mut owners = Vec::new();
        let classes = crate::builtin_classes(&py);
        let class = mapping_owned(
            &py,
            &mut owners,
            inquiry_class(&py, classes.object, b"__hash__"),
        );
        crate::raise_exception::<()>(&py, "LookupError", "discard hash identity");
        let failure = mapping_owned(&py, &mut owners, crate::molt_exception_last());
        crate::molt_exception_clear();
        set_value(&py, class, b"inquiry_result", failure);
        set_method(&py, class, b"__hash__", inquiry_discard_hash as *const ());
        let key = mapping_owned(
            &py,
            &mut owners,
            crate::alloc_instance_for_class(&py, MoltObject::from_bits(class).as_ptr().unwrap()),
        );
        let items = mapping_owned(
            &py,
            &mut owners,
            MoltObject::from_ptr(crate::alloc_tuple(&py, &[key])).bits(),
        );
        let key_view = mapping_view(key);
        let original = mapping_view(failure);
        for probe in [
            molt_linked_type_identity_probe_sequence_search,
            molt_public_type_identity_probe_sequence_search,
        ] {
            SET_DISCARD_HASH_CALLS.store(0, Relaxed);
            SET_DISCARD_HASH_FAIL_AT.store(usize::MAX, Relaxed);
            let container = mapping_owned(&py, &mut owners, construct(&py, classes.set, items));
            assert!(!crate::exception_pending(&py));
            let ptr = MoltObject::from_bits(container).as_ptr().unwrap();
            let container_view = mapping_view(container);
            assert_eq!(crate::builtins::containers::set_len(ptr), 1);

            // A failure on the first hash leaves the present entry unchanged
            // and returns the exact exception through each installed header.
            SET_DISCARD_HASH_CALLS.store(0, Relaxed);
            SET_DISCARD_HASH_FAIL_AT.store(1, Relaxed);
            assert_eq!(probe(container_view.as_ptr(), key_view.as_ptr(), 4), -1);
            let error = refcount::OwnedPyObject::from_owned(errors::PyErr_GetRaisedException());
            assert_eq!(error.as_ptr(), original.as_ptr());
            assert_eq!(SET_DISCARD_HASH_CALLS.load(Relaxed), 1);
            assert_eq!(crate::builtins::containers::set_len(ptr), 1);

            // A second hash would raise. CPython discards with one hash, so
            // the entry is removed and that second callback never executes.
            SET_DISCARD_HASH_CALLS.store(0, Relaxed);
            SET_DISCARD_HASH_FAIL_AT.store(2, Relaxed);
            assert_eq!(probe(container_view.as_ptr(), key_view.as_ptr(), 4), 1);
            assert!(errors::PyErr_Occurred().is_null());
            assert_eq!(SET_DISCARD_HASH_CALLS.load(Relaxed), 1);
            assert_eq!(crate::builtins::containers::set_len(ptr), 0);

            SET_DISCARD_HASH_CALLS.store(0, Relaxed);
            assert_eq!(probe(container_view.as_ptr(), key_view.as_ptr(), 4), 0);
            assert!(errors::PyErr_Occurred().is_null());
            assert_eq!(SET_DISCARD_HASH_CALLS.load(Relaxed), 1);
        }
        SET_DISCARD_HASH_FAIL_AT.store(usize::MAX, Relaxed);
        assert!(!crate::exception_pending(&py));
    });
}

extern "C" fn inquiry_result(receiver: u64) -> u64 {
    with_gil(|py| {
        let key = crate::attr_name_bits_from_bytes(&py, b"inquiry_result").unwrap();
        let result = crate::molt_get_attr_name(receiver, key);
        dec_ref_bits(&py, key);
        result
    })
}

extern "C" fn inquiry_false(_receiver: u64) -> u64 {
    MoltObject::from_bool(false).bits()
}

extern "C" fn inquiry_first_item(receiver: u64) -> u64 {
    crate::object::ops::molt_getitem_method(receiver, MoltObject::from_int(0).bits())
}

extern "C" fn inquiry_failure(_receiver: u64) -> u64 {
    with_gil(|py| crate::raise_exception(&py, "RuntimeError", "inquiry failed"))
}

static MAPPING_METHOD_CALLS: std::sync::atomic::AtomicUsize =
    std::sync::atomic::AtomicUsize::new(0);
extern "C" fn mapping_method_result(receiver: u64) -> u64 {
    MAPPING_METHOD_CALLS.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
    inquiry_result(receiver)
}

#[test]
fn c_mapping_queries_honor_overrides_and_preserve_exact_list_identity() {
    let _transaction = crate::test_support::RuntimeTestTransaction::with_gc_isolation();
    with_gil(|py| unsafe {
        use molt_cpython_abi::api::{abstract_mapping, mapping, sequences};
        type Query = unsafe extern "C" fn(
            *mut molt_cpython_abi::abi_types::PyObject,
        ) -> *mut molt_cpython_abi::abi_types::PyObject;
        let mut owners = Vec::new();
        let empty = mapping_owned(
            &py,
            &mut owners,
            MoltObject::from_ptr(crate::alloc_tuple(&py, &[])).bits(),
        );
        let outputs = [
            crate::alloc_list(&py, &[MoltObject::from_int(17).bits()]),
            crate::object::builders::alloc_list_int_from_raw_slice(&py, &[17]).unwrap(),
            crate::object::builders::alloc_list_bool_from_raw_slice(&py, &[1]).unwrap(),
        ]
        .map(|ptr| mapping_owned(&py, &mut owners, MoltObject::from_ptr(ptr).bits()));
        for output in outputs {
            let output_view = mapping_view(output);
            for (name, query, dict_query, direct_query) in [
                (
                    b"keys".as_slice(),
                    abstract_mapping::PyMapping_Keys as Query,
                    mapping::PyDict_Keys as Query,
                    crate::c_api::PyMapping_Keys as extern "C" fn(u64) -> u64,
                ),
                (
                    b"values".as_slice(),
                    abstract_mapping::PyMapping_Values as Query,
                    mapping::PyDict_Values as Query,
                    crate::c_api::PyMapping_Values,
                ),
                (
                    b"items".as_slice(),
                    abstract_mapping::PyMapping_Items as Query,
                    mapping::PyDict_Items as Query,
                    crate::c_api::PyMapping_Items,
                ),
            ] {
                let class = mapping_owned(
                    &py,
                    &mut owners,
                    inquiry_class(&py, crate::builtin_classes(&py).dict, name),
                );
                set_method(&py, class, name, mapping_method_result as *const ());
                set_value(&py, class, b"inquiry_result", output);
                let dict = mapping_owned(&py, &mut owners, construct(&py, class, empty));
                let view = mapping_view(dict);
                MAPPING_METHOD_CALLS.store(0, std::sync::atomic::Ordering::Relaxed);
                let snapshot = refcount::OwnedPyObject::from_owned(dict_query(view.as_ptr()));
                assert!(!snapshot.as_ptr().is_null());
                assert_eq!(sequences::PyList_CheckExact(snapshot.as_ptr()), 1);
                assert_eq!(sequences::PyList_Size(snapshot.as_ptr()), 0);
                assert_eq!(
                    MAPPING_METHOD_CALLS.load(std::sync::atomic::Ordering::Relaxed),
                    0
                );
                let result = refcount::OwnedPyObject::from_owned(query(view.as_ptr()));
                assert_eq!(result.as_ptr(), output_view.as_ptr());
                assert_eq!(
                    MAPPING_METHOD_CALLS.load(std::sync::atomic::Ordering::Relaxed),
                    1
                );
                let direct = mapping_owned(&py, &mut owners, direct_query(dict));
                assert_eq!(direct, output);
                set_method(&py, class, name, inquiry_failure as *const ());
                assert!(query(view.as_ptr()).is_null());
                assert_eq!(
                    errors::PyErr_ExceptionMatches(
                        (&raw mut molt_cpython_abi::abi_types::PyExc_RuntimeError).cast()
                    ),
                    1
                );
                errors::PyErr_Clear();
            }
        }
        assert!(!crate::exception_pending(&py));
    });
}

fn set_value(py: &crate::PyToken<'_>, class: u64, name: &[u8], value: u64) {
    let key = crate::attr_name_bits_from_bytes(py, name).unwrap();
    crate::molt_set_attr_name(class, key, value);
    dec_ref_bits(py, key);
    assert!(!crate::exception_pending(py));
}

fn set_method(py: &crate::PyToken<'_>, class: u64, name: &[u8], target: *const ()) {
    let method = MoltObject::from_ptr(crate::builtins::functions::alloc_runtime_function_obj(
        py,
        crate::provenance::abi::expose_function_address(target),
        1,
    ))
    .bits();
    set_value(py, class, name, method);
    dec_ref_bits(py, method);
}

fn inquiry_class(py: &crate::PyToken<'_>, base: u64, special: &[u8]) -> u64 {
    let name = crate::attr_name_bits_from_bytes(py, b"ManagedInquiry").unwrap();
    let class = crate::molt_class_new(name);
    dec_ref_bits(py, name);
    crate::molt_class_set_base(class, base);
    set_method(py, class, special, inquiry_result as *const ());
    set_value(py, class, b"inquiry_result", MoltObject::from_int(0).bits());
    unsafe {
        crate::object::class_finish_definition(py, MoltObject::from_bits(class).as_ptr().unwrap())
            .unwrap();
    }
    class
}

fn construct(py: &crate::PyToken<'_>, class: u64, input: u64) -> u64 {
    let bits =
        unsafe { crate::call::bind::call_bind_borrowed(py, class, None, &[input], &[], &[]) };
    assert!(!crate::exception_pending(py));
    assert_eq!(crate::type_of_bits(py, bits), class);
    bits
}

#[test]
fn managed_c_inquiries_follow_native_subtype_overrides_and_live_mutation() {
    let _transaction = crate::test_support::RuntimeTestTransaction::new();
    with_gil(|py| unsafe {
        let classes = crate::builtin_classes(&py);
        for base in [
            classes.list,
            classes.tuple,
            classes.str,
            classes.bytes,
            classes.bytearray,
            classes.set,
            classes.frozenset,
        ] {
            let class = inquiry_class(&py, base, b"__len__");
            let input = if base == classes.str {
                MoltObject::from_ptr(alloc_string(&py, b"x")).bits()
            } else {
                MoltObject::from_ptr(crate::alloc_tuple(&py, &[MoltObject::from_int(7).bits()]))
                    .bits()
            };
            let bits = construct(&py, class, input);
            let view = GLOBAL_BRIDGE.borrowed_handle_to_new_pyobj(bits);
            assert!(!view.is_null());
            // One stored element, but the actual class says zero.
            assert_eq!(object::PyObject_Size(view), 0);
            assert_eq!(object::PyObject_Length(view), 0);
            assert_eq!(object::PyObject_IsTrue(view), 0);
            assert_eq!(object::PyObject_Not(view), 1);

            set_value(
                &py,
                class,
                b"inquiry_result",
                MoltObject::from_int(3).bits(),
            );
            assert_eq!(object::PyObject_Size(view), 3);
            assert_eq!(object::PyObject_IsTrue(view), 1);
            // A later __bool__ declaration wins over the nonzero __len__.
            set_method(&py, class, b"__bool__", inquiry_false as *const ());
            assert_eq!(object::PyObject_IsTrue(view), 0);
            assert_eq!(object::PyObject_Size(view), 3);
            // Replacing the method on an already-projected class is observed.
            set_method(&py, class, b"__len__", inquiry_failure as *const ());
            assert_eq!(object::PyObject_Size(view), -1);
            assert_eq!(
                errors::PyErr_ExceptionMatches(
                    (&raw mut molt_cpython_abi::abi_types::PyExc_RuntimeError).cast()
                ),
                1
            );
            errors::PyErr_Clear();
            assert!(!crate::exception_pending(&py));
            refcount::Py_DECREF(view);
            for value in [bits, input, class] {
                dec_ref_bits(&py, value);
            }
        }
        // Exact strings count code points, including a surrogate, not WTF-8 bytes.
        let text =
            MoltObject::from_ptr(alloc_string(&py, b"a\xc3\xa9\xf0\x9f\x98\x80\xed\xa0\x80"))
                .bits();
        let view = GLOBAL_BRIDGE.borrowed_handle_to_new_pyobj(text);
        assert_eq!(object::PyObject_Size(view), 4);
        assert_eq!(object::PyObject_IsTrue(view), 1);
        refcount::Py_DECREF(view);
        dec_ref_bits(&py, text);
        assert!(errors::PyErr_Occurred().is_null());
        assert!(!crate::exception_pending(&py));
    });
}

unsafe fn expect_inquiry_error(
    py: &crate::PyToken<'_>,
    view: *mut PyObject,
    expected: *mut PyObject,
) {
    unsafe {
        assert_eq!(object::PyObject_Size(view), -1);
        assert_eq!(errors::PyErr_ExceptionMatches(expected), 1);
        errors::PyErr_Clear();
        assert!(!crate::exception_pending(py));
        assert_eq!(object::PyObject_IsTrue(view), -1);
        assert_eq!(errors::PyErr_ExceptionMatches(expected), 1);
        errors::PyErr_Clear();
        assert!(!crate::exception_pending(py));
    }
}

#[test]
fn managed_c_inquiries_observe_direct_list_writes_and_reject_failed_commits() {
    let _transaction = crate::test_support::RuntimeTestTransaction::new();
    with_gil(|py| unsafe {
        let class = inquiry_class(&py, crate::builtin_classes(&py).list, b"__len__");
        set_method(&py, class, b"__len__", inquiry_first_item as *const ());
        let input =
            MoltObject::from_ptr(crate::alloc_tuple(&py, &[MoltObject::from_int(1).bits()])).bits();
        let bits = construct(&py, class, input);
        let view = GLOBAL_BRIDGE.borrowed_handle_to_new_pyobj(bits);
        assert!(!view.is_null());
        let physical = view.cast::<molt_cpython_abi::abi_types::PyListObject>();
        assert_eq!(object::PyObject_Size(view), 1);

        // As in the direct-list Cython fixture, transfer an owned pointer into
        // ob_item without a setter. Each inquiry must perform the first commit.
        let zero = molt_cpython_abi::api::numbers::PyLong_FromLong(0);
        assert!(!zero.is_null());
        *(*physical).ob_item = zero;
        assert_eq!(object::PyObject_Size(view), 0);
        let three = molt_cpython_abi::api::numbers::PyLong_FromLong(3);
        assert!(!three.is_null());
        *(*physical).ob_item = three;
        assert_eq!(object::PyObject_IsTrue(view), 1);
        assert_eq!(object::PyObject_Size(view), 3);

        // A managed projection with invalid written storage is still managed.
        // Neither inquiry may recover by consulting its foreign C type slots.
        let saved = *(*physical).ob_item;
        *(*physical).ob_item = ptr::null_mut();
        expect_inquiry_error(
            &py,
            view,
            (&raw mut molt_cpython_abi::abi_types::PyExc_SystemError).cast(),
        );
        // Hash observes the same managed projection. A failed commit cannot
        // fall through to the physical list's unhashable TypeError.
        assert_eq!(molt_cpython_abi::api::typeobj::PyObject_Hash(view), -1);
        assert_eq!(
            errors::PyErr_ExceptionMatches(
                (&raw mut molt_cpython_abi::abi_types::PyExc_SystemError).cast()
            ),
            1
        );
        errors::PyErr_Clear();
        *(*physical).ob_item = saved;
        assert_eq!(object::PyObject_Size(view), 3);
        assert!(errors::PyErr_Occurred().is_null());
        refcount::Py_DECREF(view);
        for value in [bits, input, class] {
            dec_ref_bits(&py, value);
        }
        assert!(!crate::exception_pending(&py));
    });
}

#[test]
fn managed_c_inquiries_share_index_conversion_and_pending_error_transfer() {
    let _transaction = crate::test_support::RuntimeTestTransaction::new();
    with_gil(|py| unsafe {
        let classes = crate::builtin_classes(&py);
        let index_class = inquiry_class(&py, classes.object, b"__index__");
        let index = crate::alloc_instance_for_class(
            &py,
            MoltObject::from_bits(index_class).as_ptr().unwrap(),
        );
        let input =
            MoltObject::from_ptr(crate::alloc_tuple(&py, &[MoltObject::from_int(7).bits()])).bits();
        let overflow = int_bits_from_bigint(&py, BigInt::from(isize::MAX) + BigInt::from(1));
        let index_view = GLOBAL_BRIDGE.borrowed_handle_to_new_pyobj(index);
        assert_eq!(
            molt_cpython_abi::api::abstract_number::PyIndex_Check(index_view),
            1
        );
        for value in [0, 3] {
            set_value(
                &py,
                index_class,
                b"inquiry_result",
                MoltObject::from_int(value).bits(),
            );
            let result = molt_cpython_abi::api::abstract_number::PyNumber_Index(index_view);
            assert!(!result.is_null());
            assert_eq!(
                (*result).ob_type,
                &raw mut molt_cpython_abi::abi_types::PyLong_Type
            );
            assert_eq!(
                molt_cpython_abi::api::numbers::PyLong_AsLongLong(result),
                value
            );
            refcount::Py_DECREF(result);
        }
        set_method(&py, index_class, b"__index__", inquiry_failure as *const ());
        assert!(molt_cpython_abi::api::abstract_number::PyNumber_Index(index_view).is_null());
        assert_eq!(
            errors::PyErr_ExceptionMatches(
                (&raw mut molt_cpython_abi::abi_types::PyExc_RuntimeError).cast()
            ),
            1
        );
        errors::PyErr_Clear();
        set_method(&py, index_class, b"__index__", inquiry_result as *const ());
        refcount::Py_DECREF(index_view);
        for base in [classes.list, classes.tuple, classes.bytes] {
            let class = inquiry_class(&py, base, b"__len__");
            set_value(&py, class, b"inquiry_result", index);
            let bits = construct(&py, class, input);
            let view = GLOBAL_BRIDGE.borrowed_handle_to_new_pyobj(bits);
            for length in [0, 3] {
                set_value(
                    &py,
                    index_class,
                    b"inquiry_result",
                    MoltObject::from_int(length).bits(),
                );
                assert_eq!(object::PyObject_Size(view), length as isize);
                assert_eq!(object::PyObject_IsTrue(view), c_int::from(length != 0));
            }
            set_value(
                &py,
                index_class,
                b"inquiry_result",
                MoltObject::from_int(-1).bits(),
            );
            expect_inquiry_error(
                &py,
                view,
                (&raw mut molt_cpython_abi::abi_types::PyExc_ValueError).cast(),
            );
            set_value(&py, index_class, b"inquiry_result", overflow);
            expect_inquiry_error(
                &py,
                view,
                (&raw mut molt_cpython_abi::abi_types::PyExc_OverflowError).cast(),
            );
            set_value(
                &py,
                index_class,
                b"inquiry_result",
                MoltObject::from_float(1.5).bits(),
            );
            expect_inquiry_error(
                &py,
                view,
                (&raw mut molt_cpython_abi::abi_types::PyExc_TypeError).cast(),
            );
            set_method(&py, index_class, b"__index__", inquiry_failure as *const ());
            expect_inquiry_error(
                &py,
                view,
                (&raw mut molt_cpython_abi::abi_types::PyExc_RuntimeError).cast(),
            );
            set_method(&py, index_class, b"__index__", inquiry_result as *const ());
            // A non-bool return and a raising __bool__ retain their own errors.
            set_value(
                &py,
                class,
                b"inquiry_result",
                MoltObject::from_int(1).bits(),
            );
            set_method(&py, class, b"__bool__", inquiry_result as *const ());
            assert_eq!(object::PyObject_IsTrue(view), -1);
            assert_eq!(
                errors::PyErr_ExceptionMatches(
                    (&raw mut molt_cpython_abi::abi_types::PyExc_TypeError).cast()
                ),
                1
            );
            errors::PyErr_Clear();
            set_method(&py, class, b"__bool__", inquiry_failure as *const ());
            assert_eq!(object::PyObject_Not(view), -1);
            assert_eq!(
                errors::PyErr_ExceptionMatches(
                    (&raw mut molt_cpython_abi::abi_types::PyExc_RuntimeError).cast()
                ),
                1
            );
            errors::PyErr_Clear();
            assert!(!crate::exception_pending(&py));
            refcount::Py_DECREF(view);
            for value in [bits, class] {
                dec_ref_bits(&py, value);
            }
        }
        for value in [overflow, input, index, index_class] {
            dec_ref_bits(&py, value);
        }
        assert!(errors::PyErr_Occurred().is_null());
        assert!(!crate::exception_pending(&py));
    });
}

#[test]
fn different_integer_values_compare_unequal_through_the_runtime() {
    let _transaction = crate::test_support::RuntimeTestTransaction::new();
    with_gil(|py| unsafe {
        let left = molt_cpython_abi::api::numbers::PyLong_FromLong(1);
        let right = molt_cpython_abi::api::numbers::PyLong_FromLong(2);
        assert!(!left.is_null() && !right.is_null());
        assert_eq!(
            molt_cpython_abi::api::typeobj::PyObject_RichCompareBool(left, right, 3),
            1
        );
        refcount::Py_DECREF(left);
        refcount::Py_DECREF(right);
        assert!(errors::PyErr_Occurred().is_null());
        assert!(!crate::exception_pending(&py));
    });
}

#[test]
fn integer_ordering_returns_the_exact_boolean_singleton() {
    let _transaction = crate::test_support::RuntimeTestTransaction::new();
    with_gil(|py| unsafe {
        let left = molt_cpython_abi::api::numbers::PyLong_FromLong(1);
        let right = molt_cpython_abi::api::numbers::PyLong_FromLong(2);
        assert!(!left.is_null() && !right.is_null());
        let result = molt_cpython_abi::api::typeobj::PyObject_RichCompare(left, right, 0);
        assert_eq!(
            result,
            (&raw mut molt_cpython_abi::abi_types::Py_True).cast(),
            "1 < 2 must be Py_True, never NotImplemented"
        );
        assert_ne!(
            result,
            &raw mut molt_cpython_abi::abi_types::Py_NotImplementedSentinel
        );
        refcount::Py_DECREF(result);
        refcount::Py_DECREF(left);
        refcount::Py_DECREF(right);
        assert!(errors::PyErr_Occurred().is_null());
        assert!(!crate::exception_pending(&py));
    });
}

// These witnesses execute the actual C consumer compiled against both headers.
unsafe extern "C" {
    fn molt_linked_mapping_result_probe(
        mapping: *mut molt_cpython_abi::abi_types::PyObject,
        method: i32,
        expected: *mut molt_cpython_abi::abi_types::PyObject,
        identity: i32,
    ) -> i32;
    fn molt_public_mapping_result_probe(
        mapping: *mut molt_cpython_abi::abi_types::PyObject,
        method: i32,
        expected: *mut molt_cpython_abi::abi_types::PyObject,
        identity: i32,
    ) -> i32;
    fn molt_linked_dict_subclass_probe(
        dict: *mut molt_cpython_abi::abi_types::PyObject,
        first: *mut molt_cpython_abi::abi_types::PyObject,
        second: *mut molt_cpython_abi::abi_types::PyObject,
    ) -> i32;
    fn molt_public_dict_subclass_probe(
        dict: *mut molt_cpython_abi::abi_types::PyObject,
        first: *mut molt_cpython_abi::abi_types::PyObject,
        second: *mut molt_cpython_abi::abi_types::PyObject,
    ) -> i32;
}

static MAPPING_LENGTH_CALLS: std::sync::atomic::AtomicUsize =
    std::sync::atomic::AtomicUsize::new(0);
extern "C" fn mapping_poisoned_length(receiver: u64) -> u64 {
    MAPPING_LENGTH_CALLS.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
    inquiry_failure(receiver)
}
extern "C" fn mapping_output_iterator(receiver: u64) -> u64 {
    with_gil(|py| {
        let sequence = inquiry_result(receiver);
        let iter = crate::molt_iter(sequence);
        dec_ref_bits(&py, sequence);
        iter
    })
}
extern "C" fn mapping_item_value(_receiver: u64, _key: u64) -> u64 {
    MoltObject::from_int(23).bits()
}
fn mapping_item_method(py: &crate::PyToken<'_>, class: u64) {
    let method = MoltObject::from_ptr(crate::builtins::functions::alloc_runtime_function_obj(
        py,
        crate::provenance::abi::expose_function_address(mapping_item_value as *const ()),
        2,
    ))
    .bits();
    set_value(py, class, b"__getitem__", method);
    dec_ref_bits(py, method);
}
fn mapping_owned<'a, 'py>(
    py: &'a crate::PyToken<'py>,
    owners: &mut Vec<crate::builtins::exceptions::ExceptionValue<'a, 'py>>,
    bits: u64,
) -> u64 {
    owners.push(crate::builtins::exceptions::ExceptionValue::adopt(py, bits));
    bits
}
unsafe fn mapping_view(bits: u64) -> refcount::OwnedPyObject {
    unsafe { refcount::OwnedPyObject::from_owned(GLOBAL_BRIDGE.borrowed_handle_to_new_pyobj(bits)) }
}

#[test]
fn c_mapping_iterator_first_materialization_and_noniterable_diagnostics() {
    let _transaction = crate::test_support::RuntimeTestTransaction::with_gc_isolation();
    molt_cpython_abi_test_support::link();
    with_gil(|py| unsafe {
        use molt_cpython_abi::api::{abstract_mapping, mapping};
        let mut owners = Vec::new();
        let values = mapping_owned(
            &py,
            &mut owners,
            MoltObject::from_ptr(crate::alloc_list(&py, &[MoltObject::from_int(7).bits()])).bits(),
        );
        let output_class = mapping_owned(
            &py,
            &mut owners,
            inquiry_class(&py, crate::builtin_classes(&py).object, b"__iter__"),
        );
        set_method(
            &py,
            output_class,
            b"__iter__",
            mapping_output_iterator as *const (),
        );
        set_method(
            &py,
            output_class,
            b"__len__",
            mapping_poisoned_length as *const (),
        );
        set_value(&py, output_class, b"inquiry_result", values);
        let output = mapping_owned(
            &py,
            &mut owners,
            crate::alloc_instance_for_class(
                &py,
                MoltObject::from_bits(output_class).as_ptr().unwrap(),
            ),
        );
        let class = mapping_owned(
            &py,
            &mut owners,
            inquiry_class(&py, crate::builtin_classes(&py).object, b"keys"),
        );
        for name in [b"keys".as_slice(), b"values", b"items"] {
            set_method(&py, class, name, inquiry_result as *const ());
        }
        mapping_item_method(&py, class);
        set_value(&py, class, b"inquiry_result", output);
        let receiver = mapping_owned(
            &py,
            &mut owners,
            crate::alloc_instance_for_class(&py, MoltObject::from_bits(class).as_ptr().unwrap()),
        );
        let receiver_view = mapping_view(receiver);
        let values_view = mapping_view(values);
        MAPPING_LENGTH_CALLS.store(0, std::sync::atomic::Ordering::Relaxed);
        for method in 0..3 {
            for probe in [
                molt_linked_mapping_result_probe,
                molt_public_mapping_result_probe,
            ] {
                assert_eq!(
                    probe(receiver_view.as_ptr(), method, values_view.as_ptr(), 0),
                    0
                );
            }
            let query = [
                crate::c_api::PyMapping_Keys,
                crate::c_api::PyMapping_Values,
                crate::c_api::PyMapping_Items,
            ][method as usize];
            let result = mapping_owned(&py, &mut owners, query(receiver));
            assert_ne!(result, 0);
            assert_eq!(crate::c_api::PyList_Size(result), 1);
        }
        let target = mapping_owned(
            &py,
            &mut owners,
            MoltObject::from_ptr(alloc_dict_with_pairs(&py, &[])).bits(),
        );
        let target_view = mapping_view(target);
        assert_eq!(
            mapping::PyDict_Update(target_view.as_ptr(), receiver_view.as_ptr()),
            0
        );
        assert_eq!(crate::c_api::PyDict_Update(target, receiver), 0);
        assert_eq!(
            crate::c_api::PyDict_GetItem(target, MoltObject::from_int(7).bits()),
            MoltObject::from_int(23).bits()
        );
        assert_eq!(
            MAPPING_LENGTH_CALLS.load(std::sync::atomic::Ordering::Relaxed),
            0
        );
        // A real int result is non-iterable; TypeError identifies the mapping method.
        set_value(
            &py,
            class,
            b"inquiry_result",
            MoltObject::from_int(4).bits(),
        );
        for (query, name) in [
            (
                abstract_mapping::PyMapping_Keys
                    as unsafe extern "C" fn(
                        *mut molt_cpython_abi::abi_types::PyObject,
                    )
                        -> *mut molt_cpython_abi::abi_types::PyObject,
                "keys",
            ),
            (abstract_mapping::PyMapping_Values, "values"),
            (abstract_mapping::PyMapping_Items, "items"),
        ] {
            assert!(query(receiver_view.as_ptr()).is_null());
            assert_eq!(
                errors::PyErr_ExceptionMatches(
                    (&raw mut molt_cpython_abi::abi_types::PyExc_TypeError).cast()
                ),
                1
            );
            let error = refcount::OwnedPyObject::from_owned(errors::PyErr_GetRaisedException());
            let context =
                refcount::OwnedPyObject::from_owned(errors::PyException_GetContext(error.as_ptr()));
            assert!(
                context.as_ptr().is_null(),
                "discarded iterator TypeError became diagnostic context"
            );
            let text = refcount::OwnedPyObject::from_owned(
                molt_cpython_abi::api::typeobj::PyObject_Str(error.as_ptr()),
            );
            let text = std::ffi::CStr::from_ptr(molt_cpython_abi::api::strings::PyUnicode_AsUTF8(
                text.as_ptr(),
            ));
            assert_eq!(
                text.to_bytes(),
                format!("ManagedInquiry.{name}() returned a non-iterable (type int)").as_bytes()
            );
        }
        // Arbitrary __iter__ failures retain their type and message.
        set_value(&py, class, b"inquiry_result", output);
        set_method(&py, output_class, b"__iter__", inquiry_failure as *const ());
        assert!(abstract_mapping::PyMapping_Keys(receiver_view.as_ptr()).is_null());
        assert_eq!(
            errors::PyErr_ExceptionMatches(
                (&raw mut molt_cpython_abi::abi_types::PyExc_RuntimeError).cast()
            ),
            1
        );
        errors::PyErr_Clear();
        // Every exact physical list representation also survives the C consumer unchanged.
        for list in [
            values,
            mapping_owned(
                &py,
                &mut owners,
                MoltObject::from_ptr(
                    crate::object::builders::alloc_list_int_from_raw_slice(&py, &[7]).unwrap(),
                )
                .bits(),
            ),
            mapping_owned(
                &py,
                &mut owners,
                MoltObject::from_ptr(
                    crate::object::builders::alloc_list_bool_from_raw_slice(&py, &[1]).unwrap(),
                )
                .bits(),
            ),
        ] {
            set_value(&py, class, b"inquiry_result", list);
            let view = mapping_view(list);
            for method in 0..3 {
                for probe in [
                    molt_linked_mapping_result_probe,
                    molt_public_mapping_result_probe,
                ] {
                    assert_eq!(probe(receiver_view.as_ptr(), method, view.as_ptr(), 1), 0);
                }
            }
        }
        assert!(!crate::exception_pending(&py));
    });
}

#[test]
fn c_dict_subclass_storage_and_merge_iterator_override_share_admission() {
    let _transaction = crate::test_support::RuntimeTestTransaction::with_gc_isolation();
    molt_cpython_abi_test_support::link();
    with_gil(|py| unsafe {
        use molt_cpython_abi::api::mapping;
        let mut owners = Vec::new();
        let class = mapping_owned(
            &py,
            &mut owners,
            inquiry_class(&py, crate::builtin_classes(&py).dict, b"keys"),
        );
        set_method(&py, class, b"keys", mapping_method_result as *const ());
        let keys = mapping_owned(
            &py,
            &mut owners,
            MoltObject::from_ptr(crate::alloc_list(&py, &[MoltObject::from_int(7).bits()])).bits(),
        );
        set_value(&py, class, b"inquiry_result", keys);
        let first = mapping_view(MoltObject::from_int(11).bits());
        let second = mapping_view(MoltObject::from_int(22).bits());
        for probe in [
            molt_linked_dict_subclass_probe,
            molt_public_dict_subclass_probe,
        ] {
            let dict = mapping_owned(
                &py,
                &mut owners,
                crate::alloc_instance_for_class(
                    &py,
                    MoltObject::from_bits(class).as_ptr().unwrap(),
                ),
            );
            let view = mapping_view(dict);
            MAPPING_METHOD_CALLS.store(0, std::sync::atomic::Ordering::Relaxed);
            assert_eq!(probe(view.as_ptr(), first.as_ptr(), second.as_ptr()), 0);
            assert_eq!(
                MAPPING_METHOD_CALLS.load(std::sync::atomic::Ordering::Relaxed),
                0
            );
            assert_eq!(crate::c_api::PyDict_Size(dict), 2);
            assert_eq!(
                crate::c_api::PyDict_GetItemString(dict, c"b".as_ptr()),
                MoltObject::from_int(22).bits()
            );
            assert_eq!(
                crate::c_api::PyDict_SetItemString(
                    dict,
                    c"c".as_ptr(),
                    MoltObject::from_int(33).bits()
                ),
                0
            );
            assert_eq!(mapping::PyDict_Size(view.as_ptr()), 3);
        }
        // The same subtype now overrides __iter__: both merge paths must use keys/getitem.
        set_method(
            &py,
            class,
            b"__iter__",
            mapping_output_iterator as *const (),
        );
        mapping_item_method(&py, class);
        let source = mapping_owned(
            &py,
            &mut owners,
            crate::alloc_instance_for_class(&py, MoltObject::from_bits(class).as_ptr().unwrap()),
        );
        let source_view = mapping_view(source);
        let target = mapping_owned(
            &py,
            &mut owners,
            MoltObject::from_ptr(alloc_dict_with_pairs(&py, &[])).bits(),
        );
        let target_view = mapping_view(target);
        MAPPING_METHOD_CALLS.store(0, std::sync::atomic::Ordering::Relaxed);
        assert_eq!(
            mapping::PyDict_Update(target_view.as_ptr(), source_view.as_ptr()),
            0
        );
        assert_eq!(crate::c_api::PyDict_Update(target, source), 0);
        assert_eq!(
            MAPPING_METHOD_CALLS.load(std::sync::atomic::Ordering::Relaxed),
            2
        );
        assert_eq!(
            crate::c_api::PyDict_GetItem(target, MoltObject::from_int(7).bits()),
            MoltObject::from_int(23).bits()
        );
        assert!(!crate::exception_pending(&py));
    });
}

#[test]
fn c_dict_lazy_backing_failures_preserve_original_errors() {
    let _transaction = crate::test_support::RuntimeTestTransaction::with_gc_isolation();
    with_gil(|py| unsafe {
        use crate::resource::{LimitedTracker, ResourceLimits, UnlimitedTracker, set_tracker};
        use molt_cpython_abi::api::mapping;
        struct RestoreTracker;
        impl Drop for RestoreTracker {
            fn drop(&mut self) {
                set_tracker(Box::new(UnlimitedTracker));
            }
        }
        struct RestoreSlot(*mut u64);
        impl Drop for RestoreSlot {
            fn drop(&mut self) {
                unsafe {
                    *self.0 = 0;
                }
            }
        }
        let mut owners = Vec::new();
        let class = mapping_owned(
            &py,
            &mut owners,
            inquiry_class(&py, crate::builtin_classes(&py).dict, b"keys"),
        );
        let receiver = mapping_owned(
            &py,
            &mut owners,
            crate::alloc_instance_for_class(&py, MoltObject::from_bits(class).as_ptr().unwrap()),
        );
        let slot = crate::object::layout::dict_subclass_storage_slot(
            MoltObject::from_bits(receiver).as_ptr().unwrap(),
        )
        .unwrap();
        if *slot != 0 {
            let previous = *slot;
            *slot = 0;
            dec_ref_bits(&py, previous);
        }
        let view = mapping_view(receiver);
        let key_bits = MoltObject::from_int(7).bits();
        let key = mapping_view(key_bits);
        // Every entry point sees the same lazy backing allocation failure. Restore
        // resource availability before materializing the pending MemoryError.
        for operation in 0..11 {
            assert_eq!(*slot, 0);
            {
                let _restore = RestoreTracker;
                set_tracker(Box::new(LimitedTracker::new(&ResourceLimits {
                    max_memory: Some(0),
                    max_allocations: Some(0),
                    ..ResourceLimits::default()
                })));
                match operation {
                    0 => assert!(mapping::PyDict_Keys(view.as_ptr()).is_null()),
                    1 => assert!(mapping::PyDict_Values(view.as_ptr()).is_null()),
                    2 => assert!(mapping::PyDict_Items(view.as_ptr()).is_null()),
                    3 => assert!(mapping::PyDict_Copy(view.as_ptr()).is_null()),
                    4 => assert_eq!(mapping::PyDict_Size(view.as_ptr()), -1),
                    5 => assert!(
                        mapping::PyDict_GetItemWithError(view.as_ptr(), key.as_ptr()).is_null()
                    ),
                    6 => assert_eq!(
                        mapping::PyDict_SetItem(view.as_ptr(), key.as_ptr(), key.as_ptr()),
                        -1
                    ),
                    7 => assert_eq!(mapping::PyDict_Update(view.as_ptr(), view.as_ptr()), -1),
                    8 => mapping::PyDict_Clear(view.as_ptr()),
                    10 => assert!(
                        mapping::_PyDict_GetItem_KnownHash(view.as_ptr(), key.as_ptr(), 7)
                            .is_null()
                    ),
                    _ => {
                        let mut position = 0;
                        assert_eq!(
                            mapping::PyDict_Next(
                                view.as_ptr(),
                                &mut position,
                                std::ptr::null_mut(),
                                std::ptr::null_mut()
                            ),
                            0
                        );
                    }
                }
            }
            assert_eq!(
                errors::PyErr_ExceptionMatches(
                    (&raw mut molt_cpython_abi::abi_types::PyExc_MemoryError).cast()
                ),
                1,
                "operation {operation} replaced backing MemoryError"
            );
            errors::PyErr_Clear();
        }
        // Python dict method projections must not replace the same failure.
        for query in [
            crate::object::ops_dict::molt_dict_copy,
            crate::object::ops_dict::molt_dict_clear,
            crate::object::ops_dict::molt_dict_keys,
            crate::object::ops_dict::molt_dict_values,
            crate::object::ops_dict::molt_dict_items,
            crate::object::ops_dict::molt_dict_popitem,
        ] {
            {
                let _restore = RestoreTracker;
                set_tracker(Box::new(LimitedTracker::new(&ResourceLimits {
                    max_memory: Some(0),
                    max_allocations: Some(0),
                    ..ResourceLimits::default()
                })));
                let result = query(receiver);
                assert!(MoltObject::from_bits(result).is_none());
            }
            assert_eq!(
                errors::PyErr_ExceptionMatches(
                    (&raw mut molt_cpython_abi::abi_types::PyExc_MemoryError).cast()
                ),
                1
            );
            errors::PyErr_Clear();
        }
        // Invalid backing is a distinct SystemError and no-error getters preserve
        // an exact caller exception even when admission fails.
        let _restore_slot = RestoreSlot(slot);
        *slot = MoltObject::from_int(99).bits();
        assert!(mapping::PyDict_Keys(view.as_ptr()).is_null());
        let failure = refcount::OwnedPyObject::from_owned(errors::PyErr_GetRaisedException());
        let message = refcount::OwnedPyObject::from_owned(
            molt_cpython_abi::api::typeobj::PyObject_Str(failure.as_ptr()),
        );
        assert_eq!(
            std::ffi::CStr::from_ptr(molt_cpython_abi::api::strings::PyUnicode_AsUTF8(
                message.as_ptr()
            ))
            .to_bytes(),
            b"invalid dict subclass backing"
        );
        errors::PyErr_SetString(
            (&raw mut molt_cpython_abi::abi_types::PyExc_RuntimeError).cast(),
            c"caller sentinel".as_ptr(),
        );
        let original = refcount::OwnedPyObject::from_owned(errors::PyErr_GetRaisedException());
        refcount::Py_INCREF(original.as_ptr());
        errors::PyErr_SetRaisedException(original.as_ptr());
        assert!(mapping::PyDict_GetItem(view.as_ptr(), key.as_ptr()).is_null());
        assert!(mapping::PyDict_GetItemString(view.as_ptr(), c"a".as_ptr()).is_null());
        assert_eq!(crate::c_api::PyDict_GetItem(receiver, key_bits), 0);
        let observed = refcount::OwnedPyObject::from_owned(errors::PyErr_GetRaisedException());
        assert_eq!(observed.as_ptr(), original.as_ptr());
        assert!(!crate::exception_pending(&py));
    });
}

#[test]
fn c_dict_argument_errors_preserve_identity_and_optional_results() {
    let _transaction = crate::test_support::RuntimeTestTransaction::with_gc_isolation();
    with_gil(|py| unsafe {
        use molt_cpython_abi::api::{mapping, strings};
        use std::ptr::{null, null_mut};
        let dict = refcount::OwnedPyObject::from_owned(mapping::PyDict_New());
        let key =
            refcount::OwnedPyObject::from_owned(strings::PyUnicode_FromString(c"key".as_ptr()));
        let mut owners = Vec::new();
        let value_bits = mapping_owned(
            &py,
            &mut owners,
            MoltObject::from_ptr(crate::alloc_list(&py, &[])).bits(),
        );
        let value = mapping_view(value_bits);
        assert!(!molt_cpython_abi::abi_types::is_immortal_refcnt(
            (*value.as_ptr()).ob_refcnt
        ));
        crate::raise_exception::<()>(&py, "RuntimeError", "dict caller sentinel");
        let original_bits = mapping_owned(&py, &mut owners, crate::molt_exception_last());
        crate::clear_exception(&py);
        let original = mapping_view(original_bits);
        assert!(!original.as_ptr().is_null());
        // Exercise both exception owners: guards must transfer a pending runtime
        // error before deciding whether a new BadInternalCall is necessary.
        for pending_source in 0..3 {
            let set_error = || match pending_source {
                1 => {
                    refcount::Py_INCREF(original.as_ptr());
                    errors::PyErr_SetRaisedException(original.as_ptr());
                }
                2 => crate::builtins::exceptions::record_exception(
                    &py,
                    MoltObject::from_bits(original_bits).as_ptr().unwrap(),
                ),
                _ => {}
            };
            for operation in 0..17 {
                set_error();
                let mut result = value.as_ptr();
                let d = dict.as_ptr();
                let k = key.as_ptr();
                let v = value.as_ptr();
                let rc = match operation {
                    0 => mapping::PyDict_SetItemString(null_mut(), c"key".as_ptr(), v),
                    1 => mapping::PyDict_SetItemString(d, null(), v),
                    2 => mapping::PyDict_SetItemString(d, c"key".as_ptr(), null_mut()),
                    3 => mapping::PyDict_DelItemString(null_mut(), c"key".as_ptr()),
                    4 => mapping::PyDict_DelItemString(d, null()),
                    5 => mapping::PyDict_ContainsString(null_mut(), c"key".as_ptr()),
                    6 => mapping::PyDict_ContainsString(d, null()),
                    7 => mapping::PyDict_GetItemRef(d, k, null_mut()),
                    8 => mapping::PyDict_GetItemStringRef(d, c"key".as_ptr(), null_mut()),
                    9 => mapping::PyDict_GetItemStringRef(d, null(), &mut result),
                    10 => {
                        mapping::PyDict_GetItemStringRef(null_mut(), c"key".as_ptr(), &mut result)
                    }
                    11 | 12 => {
                        let got = if operation == 11 {
                            mapping::_PyDict_GetItemStringWithError(d, null())
                        } else {
                            mapping::_PyDict_GetItemStringWithError(null_mut(), c"key".as_ptr())
                        };
                        assert!(got.is_null());
                        -1
                    }
                    13 => mapping::PyDict_GetItemRef(d, null_mut(), &mut result),
                    14 => mapping::PyDict_GetItemRef(null_mut(), k, &mut result),
                    _ => {
                        let got = if operation == 15 {
                            mapping::_PyDict_GetItem_KnownHash(d, null_mut(), 0)
                        } else {
                            mapping::_PyDict_GetItem_KnownHash(null_mut(), k, 0)
                        };
                        assert!(got.is_null());
                        -1
                    }
                };
                assert_eq!(rc, -1, "operation {operation}, source {pending_source}");
                if matches!(operation, 9 | 10 | 13 | 14) {
                    assert!(result.is_null());
                }
                if pending_source == 0 {
                    assert_eq!(
                        errors::PyErr_ExceptionMatches(
                            (&raw mut molt_cpython_abi::abi_types::PyExc_SystemError).cast()
                        ),
                        1,
                        "operation {operation} returned a silent failure"
                    );
                }
                let observed =
                    refcount::OwnedPyObject::from_owned(errors::PyErr_GetRaisedException());
                if pending_source != 0 {
                    assert_eq!(
                        observed.as_ptr(),
                        original.as_ptr(),
                        "operation {operation}"
                    );
                }
                assert!(!crate::exception_pending(&py));
            }
            // These APIs deliberately suppress lookup/argument errors, including
            // a null key. The caller's exact exception must nevertheless survive.
            set_error();
            assert!(mapping::PyDict_GetItemString(dict.as_ptr(), null()).is_null());
            assert!(mapping::PyDict_GetItemString(null_mut(), c"key".as_ptr()).is_null());
            assert!(mapping::PyDict_GetItem(dict.as_ptr(), null_mut()).is_null());
            let observed = refcount::OwnedPyObject::from_owned(errors::PyErr_GetRaisedException());
            assert_eq!(
                observed.as_ptr(),
                if pending_source == 0 {
                    null_mut()
                } else {
                    original.as_ptr()
                }
            );
            assert!(!crate::exception_pending(&py));
        }
        // Missing keys remain clean misses; optional result sinks do not acquire
        // a reference and are distinct from GetItem[ String ]Ref's required sink.
        let mut result = value.as_ptr();
        assert_eq!(
            mapping::PyDict_GetItemStringRef(dict.as_ptr(), c"key".as_ptr(), &mut result),
            0
        );
        assert!(result.is_null());
        assert!(mapping::_PyDict_GetItemStringWithError(dict.as_ptr(), c"key".as_ptr()).is_null());
        assert_eq!(
            mapping::PyDict_ContainsString(dict.as_ptr(), c"key".as_ptr()),
            0
        );
        let references = (*value.as_ptr()).ob_refcnt;
        assert_eq!(
            mapping::PyDict_SetDefaultRef(dict.as_ptr(), key.as_ptr(), value.as_ptr(), null_mut()),
            0
        );
        assert_eq!(
            mapping::PyDict_SetDefaultRef(dict.as_ptr(), key.as_ptr(), value.as_ptr(), null_mut()),
            1
        );
        assert_eq!((*value.as_ptr()).ob_refcnt, references);
        assert_eq!(
            mapping::PyDict_Pop(dict.as_ptr(), key.as_ptr(), null_mut()),
            1
        );
        assert_eq!(
            mapping::PyDict_Pop(dict.as_ptr(), key.as_ptr(), null_mut()),
            0
        );
        assert_eq!((*value.as_ptr()).ob_refcnt, references);
        assert_eq!(
            mapping::PyDict_Next(dict.as_ptr(), null_mut(), null_mut(), null_mut()),
            0
        );
        assert!(errors::PyErr_Occurred().is_null());
        assert!(!crate::exception_pending(&py));
    });
}

static KNOWN_HASH_CALLS: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);
static KNOWN_HASH_POISON: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);
static KNOWN_EQ_MODE: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);
static KNOWN_EQ_CALLS: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);
static KNOWN_DICT: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
static KNOWN_REPLACEMENT: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
static KNOWN_ERROR: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
const KNOWN_HASH: i64 = -37;
extern "C" fn known_hash_callback(_receiver: u64) -> u64 {
    with_gil(|py| {
        KNOWN_HASH_CALLS.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        if KNOWN_HASH_POISON.load(std::sync::atomic::Ordering::Relaxed) {
            crate::raise_exception(&py, "RuntimeError", "known hash was recomputed")
        } else {
            MoltObject::from_int(KNOWN_HASH).bits()
        }
    })
}
extern "C" fn known_hash_equality(_receiver: u64, query: u64) -> u64 {
    with_gil(|py| unsafe {
        use std::sync::atomic::Ordering::Relaxed;
        let calls = KNOWN_EQ_CALLS.fetch_add(1, Relaxed);
        match KNOWN_EQ_MODE.load(Relaxed) {
            1 => MoltObject::from_bool(true).bits(),
            2 => {
                let error = KNOWN_ERROR.load(Relaxed);
                crate::builtins::exceptions::record_exception(
                    &py,
                    MoltObject::from_bits(error).as_ptr().unwrap(),
                );
                MoltObject::none().bits()
            }
            3 if calls == 0 => {
                let dict = MoltObject::from_bits(KNOWN_DICT.load(Relaxed))
                    .as_ptr()
                    .unwrap();
                crate::object::ops::dict_clear_in_place(&py, dict);
                crate::object::ops::dict_set_with_hash_in_place(
                    &py,
                    dict,
                    query,
                    KNOWN_REPLACEMENT.load(Relaxed),
                    KNOWN_HASH as u64,
                );
                MoltObject::from_bool(true).bits()
            }
            _ => MoltObject::from_bool(false).bits(),
        }
    })
}
unsafe extern "C" {
    fn molt_linked_known_hash_probe(
        dict: *mut molt_cpython_abi::abi_types::PyObject,
        key: *mut molt_cpython_abi::abi_types::PyObject,
        hash: molt_cpython_abi::abi_types::Py_hash_t,
        expected: *mut molt_cpython_abi::abi_types::PyObject,
        expected_error: *mut molt_cpython_abi::abi_types::PyObject,
    ) -> i32;
    fn molt_public_known_hash_probe(
        dict: *mut molt_cpython_abi::abi_types::PyObject,
        key: *mut molt_cpython_abi::abi_types::PyObject,
        hash: molt_cpython_abi::abi_types::Py_hash_t,
        expected: *mut molt_cpython_abi::abi_types::PyObject,
        expected_error: *mut molt_cpython_abi::abi_types::PyObject,
    ) -> i32;
}

#[test]
fn c_dict_known_hash_preserves_hash_value_borrowed_identity_and_reentrant_errors() {
    let _transaction = crate::test_support::RuntimeTestTransaction::with_gc_isolation();
    molt_cpython_abi_test_support::link();
    with_gil(|py| unsafe {
        use molt_cpython_abi::api::mapping;
        use std::sync::atomic::Ordering::Relaxed;
        struct ResetKnownProbe;
        impl Drop for ResetKnownProbe {
            fn drop(&mut self) {
                KNOWN_HASH_POISON.store(false, Relaxed);
                KNOWN_EQ_MODE.store(0, Relaxed);
                KNOWN_DICT.store(0, Relaxed);
                KNOWN_REPLACEMENT.store(0, Relaxed);
                KNOWN_ERROR.store(0, Relaxed);
            }
        }
        let mut owners = Vec::new();
        let _reset = ResetKnownProbe;
        let key_class = mapping_owned(
            &py,
            &mut owners,
            inquiry_class(&py, crate::builtin_classes(&py).object, b"__hash__"),
        );
        set_method(
            &py,
            key_class,
            b"__hash__",
            known_hash_callback as *const (),
        );
        let equal = mapping_owned(
            &py,
            &mut owners,
            MoltObject::from_ptr(crate::builtins::functions::alloc_runtime_function_obj(
                &py,
                crate::provenance::abi::expose_function_address(known_hash_equality as *const ()),
                2,
            ))
            .bits(),
        );
        set_value(&py, key_class, b"__eq__", equal);
        let stored = mapping_owned(
            &py,
            &mut owners,
            crate::alloc_instance_for_class(
                &py,
                MoltObject::from_bits(key_class).as_ptr().unwrap(),
            ),
        );
        let query = mapping_owned(
            &py,
            &mut owners,
            crate::alloc_instance_for_class(
                &py,
                MoltObject::from_bits(key_class).as_ptr().unwrap(),
            ),
        );
        let stored_view = mapping_view(stored);
        let query_view = mapping_view(query);
        let value = mapping_owned(
            &py,
            &mut owners,
            MoltObject::from_ptr(crate::alloc_list(&py, &[])).bits(),
        );
        let replacement = mapping_owned(
            &py,
            &mut owners,
            MoltObject::from_ptr(crate::alloc_list(&py, &[MoltObject::from_int(41).bits()])).bits(),
        );
        let value_view = mapping_view(value);
        let replacement_view = mapping_view(replacement);
        crate::raise_exception::<()>(&py, "RuntimeError", "exact known-hash equality error");
        let error = mapping_owned(&py, &mut owners, crate::molt_exception_last());
        crate::clear_exception(&py);
        let error_view = mapping_view(error);
        KNOWN_ERROR.store(error, Relaxed);
        KNOWN_REPLACEMENT.store(replacement, Relaxed);
        let subclass = mapping_owned(
            &py,
            &mut owners,
            inquiry_class(&py, crate::builtin_classes(&py).dict, b"keys"),
        );
        for probe in [molt_linked_known_hash_probe, molt_public_known_hash_probe] {
            for class in [crate::builtin_classes(&py).dict, subclass] {
                let dict = mapping_owned(
                    &py,
                    &mut owners,
                    if class == crate::builtin_classes(&py).dict {
                        MoltObject::from_ptr(alloc_dict_with_pairs(&py, &[])).bits()
                    } else {
                        crate::alloc_instance_for_class(
                            &py,
                            MoltObject::from_bits(class).as_ptr().unwrap(),
                        )
                    },
                );
                let view = mapping_view(dict);
                KNOWN_EQ_MODE.store(0, Relaxed);
                KNOWN_HASH_POISON.store(false, Relaxed);
                KNOWN_HASH_CALLS.store(0, Relaxed);
                assert_eq!(
                    mapping::PyDict_SetItem(
                        view.as_ptr(),
                        stored_view.as_ptr(),
                        value_view.as_ptr()
                    ),
                    0
                );
                assert_eq!(KNOWN_HASH_CALLS.load(Relaxed), 1);
                KNOWN_HASH_POISON.store(true, Relaxed);
                KNOWN_EQ_CALLS.store(0, Relaxed);
                assert_eq!(
                    probe(
                        view.as_ptr(),
                        stored_view.as_ptr(),
                        KNOWN_HASH as _,
                        value_view.as_ptr(),
                        std::ptr::null_mut()
                    ),
                    0
                );
                assert_eq!(
                    KNOWN_EQ_CALLS.load(Relaxed),
                    0,
                    "identical key must not call equality"
                );
                // Zero is a genuine supplied hash, never the Compute sentinel.
                assert_eq!(
                    probe(
                        view.as_ptr(),
                        stored_view.as_ptr(),
                        0,
                        std::ptr::null_mut(),
                        std::ptr::null_mut()
                    ),
                    0
                );
                assert_eq!(
                    probe(
                        view.as_ptr(),
                        query_view.as_ptr(),
                        KNOWN_HASH as _,
                        std::ptr::null_mut(),
                        std::ptr::null_mut()
                    ),
                    0
                );
                KNOWN_EQ_MODE.store(1, Relaxed);
                assert_eq!(
                    probe(
                        view.as_ptr(),
                        query_view.as_ptr(),
                        KNOWN_HASH as _,
                        value_view.as_ptr(),
                        std::ptr::null_mut()
                    ),
                    0
                );
                KNOWN_EQ_MODE.store(2, Relaxed);
                assert_eq!(
                    probe(
                        view.as_ptr(),
                        query_view.as_ptr(),
                        KNOWN_HASH as _,
                        std::ptr::null_mut(),
                        error_view.as_ptr()
                    ),
                    0
                );
                let raised =
                    refcount::OwnedPyObject::from_owned(errors::PyErr_GetRaisedException());
                assert_eq!(raised.as_ptr(), error_view.as_ptr());
                // Existing epoch-aware lookup must restart after equality replaces all entries.
                KNOWN_DICT.store(
                    crate::object::ops::dict_backing_bits(&py, dict)
                        .unwrap()
                        .unwrap(),
                    Relaxed,
                );
                KNOWN_EQ_MODE.store(3, Relaxed);
                KNOWN_EQ_CALLS.store(0, Relaxed);
                assert_eq!(
                    probe(
                        view.as_ptr(),
                        query_view.as_ptr(),
                        KNOWN_HASH as _,
                        replacement_view.as_ptr(),
                        std::ptr::null_mut()
                    ),
                    0
                );
                assert_eq!(KNOWN_EQ_CALLS.load(Relaxed), 1);
                assert_eq!(
                    KNOWN_HASH_CALLS.load(Relaxed),
                    1,
                    "supplied hash invoked __hash__"
                );
                // WithError keeps ordinary hashing behavior and its new failure.
                assert!(
                    mapping::PyDict_GetItemWithError(view.as_ptr(), query_view.as_ptr()).is_null()
                );
                assert_eq!(
                    errors::PyErr_ExceptionMatches(
                        (&raw mut molt_cpython_abi::abi_types::PyExc_RuntimeError).cast()
                    ),
                    1
                );
                errors::PyErr_Clear();
                assert_eq!(KNOWN_HASH_CALLS.load(Relaxed), 2);
                // A trusted hash also bypasses hashability admission after a class edit.
                set_value(&py, key_class, b"__hash__", MoltObject::none().bits());
                assert_eq!(
                    probe(
                        view.as_ptr(),
                        query_view.as_ptr(),
                        KNOWN_HASH as _,
                        replacement_view.as_ptr(),
                        std::ptr::null_mut()
                    ),
                    0
                );
                set_method(
                    &py,
                    key_class,
                    b"__hash__",
                    known_hash_callback as *const (),
                );
            }
        }
        assert!(!crate::exception_pending(&py));
    });
}
