// -----------------------------------------------------------------------
// Phase 1 C-API tests
// -----------------------------------------------------------------------

#[test]
fn c_api_list_new_size_getitem_setitem() {
    let _guard = CApiTestGuard::new();
    crate::with_gil_entry_nopanic!(_py, {
        let list = PyList_New(3);
        assert_ne!(list, 0);
        assert_eq!(PyList_Size(list), 3);

        // All slots default to None.
        let item0 = PyList_GetItem(list, 0);
        assert!(obj_from_bits(item0).is_none());

        // SetItem steals the ref, so we inc_ref first for the value we're inserting.
        let val = MoltObject::from_int(42).bits();
        inc_ref_bits(_py, val);
        assert_eq!(PyList_SetItem(list, 1, val), 0);
        let got = PyList_GetItem(list, 1);
        assert_eq!(to_i64(obj_from_bits(got)), Some(42));

        // Append
        let extra = MoltObject::from_int(99).bits();
        assert_eq!(PyList_Append(list, extra), 0);
        assert_eq!(PyList_Size(list), 4);
        let got_last = PyList_GetItem(list, 3);
        assert_eq!(to_i64(obj_from_bits(got_last)), Some(99));

        // Negative index
        let got_neg = PyList_GetItem(list, -1);
        assert_eq!(to_i64(obj_from_bits(got_neg)), Some(99));

        // Out-of-bounds
        let bad = PyList_GetItem(list, 100);
        assert_eq!(bad, 0);
        assert!(exception_pending(_py));
        let _ = molt_exception_clear();

        dec_ref_bits(_py, list);
    });
}

#[test]
fn c_api_list_new_negative_size_fails() {
    let _guard = CApiTestGuard::new();
    crate::with_gil_entry_nopanic!(_py, {
        let list = PyList_New(-1);
        assert_eq!(list, 0);
        assert!(exception_pending(_py));
        let _ = molt_exception_clear();
    });
}

#[test]
fn c_api_dict_new_setitem_getitem_contains_size() {
    let _guard = CApiTestGuard::new();
    crate::with_gil_entry_nopanic!(_py, {
        let dict = PyDict_New();
        assert_ne!(dict, 0);
        assert_eq!(PyDict_Size(dict), 0);

        let key = MoltObject::from_int(10).bits();
        let val = MoltObject::from_int(20).bits();
        assert_eq!(PyDict_SetItem(dict, key, val), 0);
        assert_eq!(PyDict_Size(dict), 1);
        assert_eq!(PyDict_Contains(dict, key), 1);

        let got = PyDict_GetItem(dict, key);
        assert_ne!(got, 0);
        assert_eq!(to_i64(obj_from_bits(got)), Some(20));

        // Missing key returns 0 (no exception).
        let missing_key = MoltObject::from_int(999).bits();
        let missing = PyDict_GetItem(dict, missing_key);
        assert_eq!(missing, 0);
        assert!(!exception_pending(_py));

        assert_eq!(PyDict_Contains(dict, missing_key), 0);

        dec_ref_bits(_py, dict);
    });
}

#[test]
fn c_api_dict_set_item_string() {
    let _guard = CApiTestGuard::new();
    crate::with_gil_entry_nopanic!(_py, {
        let dict = PyDict_New();
        assert_ne!(dict, 0);

        let val = MoltObject::from_int(42).bits();
        let rc = unsafe { PyDict_SetItemString(dict, c"hello".as_ptr(), val) };
        assert_eq!(rc, 0);
        assert_eq!(PyDict_Size(dict), 1);

        // Verify we can retrieve by constructing a matching key.
        let key_ptr = alloc_string(_py, b"hello");
        assert!(!key_ptr.is_null());
        let key_bits = MoltObject::from_ptr(key_ptr).bits();
        let got = PyDict_GetItem(dict, key_bits);
        assert_eq!(to_i64(obj_from_bits(got)), Some(42));

        dec_ref_bits(_py, key_bits);
        dec_ref_bits(_py, dict);
    });
}

#[test]
fn c_api_tuple_new_size_getitem_setitem() {
    let _guard = CApiTestGuard::new();
    crate::with_gil_entry_nopanic!(_py, {
        let tuple = PyTuple_New(3);
        assert_ne!(tuple, 0);
        assert_eq!(PyTuple_Size(tuple), 3);

        // CPython construction slots are NULL until PyTuple_SetItem fills them.
        let item0 = PyTuple_GetItem(tuple, 0);
        assert_eq!(item0, 0);

        // SetItem steals the ref, so inc_ref the value first.
        let val = MoltObject::from_int(77).bits();
        inc_ref_bits(_py, val);
        assert_eq!(PyTuple_SetItem(tuple, 2, val), 0);
        let got = PyTuple_GetItem(tuple, 2);
        assert_eq!(to_i64(obj_from_bits(got)), Some(77));

        // Out-of-bounds
        let bad = PyTuple_GetItem(tuple, 5);
        assert_eq!(bad, 0);
        assert!(exception_pending(_py));
        let _ = molt_exception_clear();

        // Negative index in SetItem should fail (CPython tuple uses non-negative only).
        let steal_val = MoltObject::from_int(1).bits();
        inc_ref_bits(_py, steal_val);
        let rc = PyTuple_SetItem(tuple, -1, steal_val);
        assert_eq!(rc, -1);
        assert!(exception_pending(_py));
        let _ = molt_exception_clear();

        dec_ref_bits(_py, tuple);
    });
}

#[test]
fn c_api_type_checks() {
    let _guard = CApiTestGuard::new();
    crate::with_gil_entry_nopanic!(_py, {
        // Int
        let int_val = MoltObject::from_int(42).bits();
        assert_eq!(PyLong_Check(int_val), 1);
        assert_eq!(PyFloat_Check(int_val), 0);
        assert_eq!(PyBool_Check(int_val), 0);
        assert_eq!(PyNone_Check(int_val), 0);
        assert_eq!(PyUnicode_Check(int_val), 0);
        assert_eq!(PyList_Check(int_val), 0);
        assert_eq!(PyDict_Check(int_val), 0);
        assert_eq!(PyTuple_Check(int_val), 0);

        // Float
        let float_val = MoltObject::from_float(3.125).bits();
        assert_eq!(PyFloat_Check(float_val), 1);
        assert_eq!(PyLong_Check(float_val), 0);

        // Bool
        let bool_val = MoltObject::from_bool(true).bits();
        assert_eq!(PyBool_Check(bool_val), 1);

        // None
        let none_val = MoltObject::none().bits();
        assert_eq!(PyNone_Check(none_val), 1);
        assert_eq!(PyBool_Check(none_val), 0);

        // String
        let str_ptr = alloc_string(_py, b"hello");
        assert!(!str_ptr.is_null());
        let str_bits = MoltObject::from_ptr(str_ptr).bits();
        assert_eq!(PyUnicode_Check(str_bits), 1);
        assert_eq!(PyLong_Check(str_bits), 0);
        dec_ref_bits(_py, str_bits);

        // List
        let list = PyList_New(0);
        assert_ne!(list, 0);
        assert_eq!(PyList_Check(list), 1);
        assert_eq!(PyTuple_Check(list), 0);
        assert_eq!(PyDict_Check(list), 0);
        dec_ref_bits(_py, list);

        // Dict
        let dict = PyDict_New();
        assert_ne!(dict, 0);
        assert_eq!(PyDict_Check(dict), 1);
        assert_eq!(PyList_Check(dict), 0);
        dec_ref_bits(_py, dict);

        // Tuple
        let tuple = PyTuple_New(0);
        assert_ne!(tuple, 0);
        assert_eq!(PyTuple_Check(tuple), 1);
        assert_eq!(PyList_Check(tuple), 0);
        dec_ref_bits(_py, tuple);
    });
}

#[test]
fn c_api_iter_protocol_on_list() {
    let _guard = CApiTestGuard::new();
    crate::with_gil_entry_nopanic!(_py, {
        // Build a list [10, 20, 30]
        let list_ptr = alloc_list(
            _py,
            &[
                MoltObject::from_int(10).bits(),
                MoltObject::from_int(20).bits(),
                MoltObject::from_int(30).bits(),
            ],
        );
        assert!(!list_ptr.is_null());
        let list_bits = MoltObject::from_ptr(list_ptr).bits();

        // Check PyIter_Check on the list (not an iterator itself).
        assert_eq!(PyIter_Check(list_bits), 0);

        // Get an iterator.
        let iter = PyObject_GetIter(list_bits);
        assert_ne!(iter, 0);
        assert!(!exception_pending(_py));

        // The iterator should pass PyIter_Check.
        assert_eq!(PyIter_Check(iter), 1);

        // Iterate: 10, 20, 30, then NULL.
        let v1 = PyIter_Next(iter);
        assert_ne!(v1, 0);
        assert_eq!(to_i64(obj_from_bits(v1)), Some(10));
        dec_ref_bits(_py, v1);

        let v2 = PyIter_Next(iter);
        assert_ne!(v2, 0);
        assert_eq!(to_i64(obj_from_bits(v2)), Some(20));
        dec_ref_bits(_py, v2);

        let v3 = PyIter_Next(iter);
        assert_ne!(v3, 0);
        assert_eq!(to_i64(obj_from_bits(v3)), Some(30));
        dec_ref_bits(_py, v3);

        // Exhausted — returns 0 with no exception.
        let v4 = PyIter_Next(iter);
        assert_eq!(v4, 0);
        assert!(!exception_pending(_py));

        dec_ref_bits(_py, iter);
        dec_ref_bits(_py, list_bits);
    });
}

#[test]
fn c_api_get_iter_on_non_iterable_fails() {
    let _guard = CApiTestGuard::new();
    crate::with_gil_entry_nopanic!(_py, {
        let int_val = MoltObject::from_int(42).bits();
        let iter = PyObject_GetIter(int_val);
        assert_eq!(iter, 0);
        assert!(exception_pending(_py));
        let _ = molt_exception_clear();
    });
}

#[test]
fn c_api_list_setitem_steals_ref_on_error() {
    let _guard = CApiTestGuard::new();
    // Verify that PyList_SetItem steals the reference even when the call fails.
    crate::with_gil_entry_nopanic!(_py, {
        let dict = PyDict_New();
        assert_ne!(dict, 0);
        // Try to SetItem on a dict (not a list) — should fail and steal the ref.
        let val = MoltObject::from_int(1).bits();
        inc_ref_bits(_py, val);
        let rc = PyList_SetItem(dict, 0, val);
        assert_eq!(rc, -1);
        assert!(exception_pending(_py));
        let _ = molt_exception_clear();
        dec_ref_bits(_py, dict);
    });
}

// -----------------------------------------------------------------------
// Number Protocol tests
// -----------------------------------------------------------------------

#[test]
fn c_api_number_add_int() {
    let _guard = CApiTestGuard::new();
    crate::with_gil_entry_nopanic!(_py, {
        let a = MoltObject::from_int(10).bits();
        let b = MoltObject::from_int(20).bits();
        let res = PyNumber_Add(a, b);
        assert_ne!(res, 0);
        assert_eq!(to_i64(obj_from_bits(res)), Some(30));
        dec_ref_bits(_py, res);
    });
}

#[test]
fn c_api_number_add_float() {
    let _guard = CApiTestGuard::new();
    crate::with_gil_entry_nopanic!(_py, {
        let a = MoltObject::from_float(1.5).bits();
        let b = MoltObject::from_float(2.5).bits();
        let res = PyNumber_Add(a, b);
        assert_ne!(res, 0);
        assert_eq!(obj_from_bits(res).as_float(), Some(4.0));
        dec_ref_bits(_py, res);
    });
}

#[test]
fn c_api_number_subtract() {
    let _guard = CApiTestGuard::new();
    crate::with_gil_entry_nopanic!(_py, {
        let a = MoltObject::from_int(50).bits();
        let b = MoltObject::from_int(30).bits();
        let res = PyNumber_Subtract(a, b);
        assert_ne!(res, 0);
        assert_eq!(to_i64(obj_from_bits(res)), Some(20));
        dec_ref_bits(_py, res);
    });
}

#[test]
fn c_api_number_multiply() {
    let _guard = CApiTestGuard::new();
    crate::with_gil_entry_nopanic!(_py, {
        let a = MoltObject::from_int(6).bits();
        let b = MoltObject::from_int(7).bits();
        let res = PyNumber_Multiply(a, b);
        assert_ne!(res, 0);
        assert_eq!(to_i64(obj_from_bits(res)), Some(42));
        dec_ref_bits(_py, res);
    });
}

#[test]
fn c_api_number_truedivide() {
    let _guard = CApiTestGuard::new();
    crate::with_gil_entry_nopanic!(_py, {
        let a = MoltObject::from_int(10).bits();
        let b = MoltObject::from_int(4).bits();
        let res = PyNumber_TrueDivide(a, b);
        assert_ne!(res, 0);
        assert_eq!(obj_from_bits(res).as_float(), Some(2.5));
        dec_ref_bits(_py, res);
    });
}

#[test]
fn c_api_number_truedivide_by_zero() {
    let _guard = CApiTestGuard::new();
    crate::with_gil_entry_nopanic!(_py, {
        let a = MoltObject::from_int(10).bits();
        let b = MoltObject::from_int(0).bits();
        let res = PyNumber_TrueDivide(a, b);
        assert_eq!(res, 0);
        assert!(exception_pending(_py));
        let _ = molt_exception_clear();
    });
}

#[test]
fn c_api_number_floordivide() {
    let _guard = CApiTestGuard::new();
    crate::with_gil_entry_nopanic!(_py, {
        let a = MoltObject::from_int(17).bits();
        let b = MoltObject::from_int(5).bits();
        let res = PyNumber_FloorDivide(a, b);
        assert_ne!(res, 0);
        assert_eq!(to_i64(obj_from_bits(res)), Some(3));
        dec_ref_bits(_py, res);
    });
}

#[test]
fn c_api_number_remainder() {
    let _guard = CApiTestGuard::new();
    crate::with_gil_entry_nopanic!(_py, {
        let a = MoltObject::from_int(17).bits();
        let b = MoltObject::from_int(5).bits();
        let res = PyNumber_Remainder(a, b);
        assert_ne!(res, 0);
        assert_eq!(to_i64(obj_from_bits(res)), Some(2));
        dec_ref_bits(_py, res);
    });
}

#[test]
fn c_api_number_power() {
    let _guard = CApiTestGuard::new();
    crate::with_gil_entry_nopanic!(_py, {
        let a = MoltObject::from_int(2).bits();
        let b = MoltObject::from_int(10).bits();
        let res = PyNumber_Power(a, b, 0);
        assert_ne!(res, 0);
        assert_eq!(to_i64(obj_from_bits(res)), Some(1024));
        dec_ref_bits(_py, res);
    });
}

#[test]
fn c_api_number_power_with_mod() {
    let _guard = CApiTestGuard::new();
    crate::with_gil_entry_nopanic!(_py, {
        // pow(2, 10, 100) = 1024 % 100 = 24
        let a = MoltObject::from_int(2).bits();
        let b = MoltObject::from_int(10).bits();
        let m = MoltObject::from_int(100).bits();
        let res = PyNumber_Power(a, b, m);
        assert_ne!(res, 0);
        assert_eq!(to_i64(obj_from_bits(res)), Some(24));
        dec_ref_bits(_py, res);
    });
}

#[test]
fn c_api_number_negative() {
    let _guard = CApiTestGuard::new();
    crate::with_gil_entry_nopanic!(_py, {
        let a = MoltObject::from_int(42).bits();
        let res = PyNumber_Negative(a);
        assert_ne!(res, 0);
        assert_eq!(to_i64(obj_from_bits(res)), Some(-42));
        dec_ref_bits(_py, res);
    });
}

#[test]
fn c_api_number_positive() {
    let _guard = CApiTestGuard::new();
    crate::with_gil_entry_nopanic!(_py, {
        let a = MoltObject::from_int(-7).bits();
        let res = PyNumber_Positive(a);
        assert_ne!(res, 0);
        assert_eq!(to_i64(obj_from_bits(res)), Some(-7));
        dec_ref_bits(_py, res);
    });
}

#[test]
fn c_api_number_absolute() {
    let _guard = CApiTestGuard::new();
    crate::with_gil_entry_nopanic!(_py, {
        let a = MoltObject::from_int(-42).bits();
        let res = PyNumber_Absolute(a);
        assert_ne!(res, 0);
        assert_eq!(to_i64(obj_from_bits(res)), Some(42));
        dec_ref_bits(_py, res);

        let b = MoltObject::from_float(-3.125).bits();
        let res2 = PyNumber_Absolute(b);
        assert_ne!(res2, 0);
        assert_eq!(obj_from_bits(res2).as_float(), Some(3.125));
        dec_ref_bits(_py, res2);
    });
}

#[test]
fn c_api_number_invert() {
    let _guard = CApiTestGuard::new();
    crate::with_gil_entry_nopanic!(_py, {
        let a = MoltObject::from_int(0).bits();
        let res = PyNumber_Invert(a);
        assert_ne!(res, 0);
        assert_eq!(to_i64(obj_from_bits(res)), Some(-1));
        dec_ref_bits(_py, res);

        let b = MoltObject::from_int(7).bits();
        let res2 = PyNumber_Invert(b);
        assert_ne!(res2, 0);
        assert_eq!(to_i64(obj_from_bits(res2)), Some(-8));
        dec_ref_bits(_py, res2);
    });
}

#[test]
fn c_api_number_lshift_rshift() {
    let _guard = CApiTestGuard::new();
    crate::with_gil_entry_nopanic!(_py, {
        let a = MoltObject::from_int(1).bits();
        let b = MoltObject::from_int(4).bits();
        let res = PyNumber_Lshift(a, b);
        assert_ne!(res, 0);
        assert_eq!(to_i64(obj_from_bits(res)), Some(16));
        dec_ref_bits(_py, res);

        let c = MoltObject::from_int(32).bits();
        let d = MoltObject::from_int(3).bits();
        let res2 = PyNumber_Rshift(c, d);
        assert_ne!(res2, 0);
        assert_eq!(to_i64(obj_from_bits(res2)), Some(4));
        dec_ref_bits(_py, res2);
    });
}

#[test]
fn c_api_number_and_or_xor() {
    let _guard = CApiTestGuard::new();
    crate::with_gil_entry_nopanic!(_py, {
        let a = MoltObject::from_int(0b1100).bits();
        let b = MoltObject::from_int(0b1010).bits();

        let and_res = PyNumber_And(a, b);
        assert_ne!(and_res, 0);
        assert_eq!(to_i64(obj_from_bits(and_res)), Some(0b1000));
        dec_ref_bits(_py, and_res);

        let or_res = PyNumber_Or(a, b);
        assert_ne!(or_res, 0);
        assert_eq!(to_i64(obj_from_bits(or_res)), Some(0b1110));
        dec_ref_bits(_py, or_res);

        let xor_res = PyNumber_Xor(a, b);
        assert_ne!(xor_res, 0);
        assert_eq!(to_i64(obj_from_bits(xor_res)), Some(0b0110));
        dec_ref_bits(_py, xor_res);
    });
}

#[test]
fn c_api_number_check() {
    let _guard = CApiTestGuard::new();
    crate::with_gil_entry_nopanic!(_py, {
        assert_eq!(PyNumber_Check(MoltObject::from_int(42).bits()), 1);
        assert_eq!(PyNumber_Check(MoltObject::from_float(3.125).bits()), 1);
        assert_eq!(PyNumber_Check(MoltObject::from_bool(true).bits()), 1);
        assert_eq!(PyNumber_Check(MoltObject::none().bits()), 0);

        let str_ptr = alloc_string(_py, b"hello");
        assert!(!str_ptr.is_null());
        let str_bits = MoltObject::from_ptr(str_ptr).bits();
        assert_eq!(PyNumber_Check(str_bits), 0);
        dec_ref_bits(_py, str_bits);

        let list = PyList_New(0);
        assert_ne!(list, 0);
        assert_eq!(PyNumber_Check(list), 0);
        dec_ref_bits(_py, list);
    });
}

#[test]
fn c_api_number_long_and_float() {
    let _guard = CApiTestGuard::new();
    crate::with_gil_entry_nopanic!(_py, {
        // int(3.7) == 3
        let f = MoltObject::from_float(3.7).bits();
        let long_res = PyNumber_Long(f);
        assert_ne!(long_res, 0);
        assert_eq!(to_i64(obj_from_bits(long_res)), Some(3));
        dec_ref_bits(_py, long_res);

        // float(42) == 42.0
        let i = MoltObject::from_int(42).bits();
        let float_res = PyNumber_Float(i);
        assert_ne!(float_res, 0);
        assert_eq!(obj_from_bits(float_res).as_float(), Some(42.0));
        dec_ref_bits(_py, float_res);
    });
}

// -----------------------------------------------------------------------
// Mapping Protocol tests
// -----------------------------------------------------------------------

#[test]
fn c_api_mapping_length() {
    let _guard = CApiTestGuard::new();
    crate::with_gil_entry_nopanic!(_py, {
        let dict = PyDict_New();
        assert_ne!(dict, 0);
        assert_eq!(PyMapping_Length(dict), 0);

        let key = MoltObject::from_int(1).bits();
        let val = MoltObject::from_int(100).bits();
        assert_eq!(PyDict_SetItem(dict, key, val), 0);
        assert_eq!(PyMapping_Length(dict), 1);

        dec_ref_bits(_py, dict);
    });
}

#[test]
fn c_api_mapping_keys_values_items() {
    let _guard = CApiTestGuard::new();
    crate::with_gil_entry_nopanic!(_py, {
        let dict = PyDict_New();
        assert_ne!(dict, 0);

        let k1 = MoltObject::from_int(1).bits();
        let v1 = MoltObject::from_int(10).bits();
        let k2 = MoltObject::from_int(2).bits();
        let v2 = MoltObject::from_int(20).bits();
        assert_eq!(PyDict_SetItem(dict, k1, v1), 0);
        assert_eq!(PyDict_SetItem(dict, k2, v2), 0);

        let keys = PyMapping_Keys(dict);
        assert_ne!(keys, 0);
        assert_eq!(PySequence_Length(keys), 2);
        dec_ref_bits(_py, keys);

        let values = PyMapping_Values(dict);
        assert_ne!(values, 0);
        assert_eq!(PySequence_Length(values), 2);
        dec_ref_bits(_py, values);

        let items = PyMapping_Items(dict);
        assert_ne!(items, 0);
        assert_eq!(PySequence_Length(items), 2);
        dec_ref_bits(_py, items);

        dec_ref_bits(_py, dict);
    });
}

#[test]
fn c_api_public_libmolt_iterator_dict_list_surface() {
    let _guard = CApiTestGuard::new();
    crate::with_gil_entry_nopanic!(_py, {
        let list = PyList_New(0);
        assert_ne!(list, 0);
        let first = MoltObject::from_int(10).bits();
        let second = MoltObject::from_int(20).bits();
        let append_first = crate::molt_list_append(list, first);
        assert!(obj_from_bits(append_first).is_none());
        let append_second = crate::molt_list_append(list, second);
        assert!(obj_from_bits(append_second).is_none());
        assert_eq!(PySequence_Length(list), 2);

        let iter = PyObject_GetIter(list);
        assert_ne!(iter, 0);

        let pair1 = crate::molt_iter_next(iter);
        let Some(pair1_ptr) = obj_from_bits(pair1).as_ptr() else {
            panic!("molt_iter_next should return a tuple pair");
        };
        let pair1_items = unsafe {
            crate::object::seq_access::snapshot(_py, pair1_ptr, "test snapshot allocation failed")
                .unwrap()
        };
        assert_eq!(pair1_items.len(), 2);
        assert_eq!(to_i64(obj_from_bits(pair1_items[0])), Some(10));
        assert!(!is_truthy(_py, obj_from_bits(pair1_items[1])));
        dec_ref_bits(_py, pair1);

        let pair2 = crate::molt_iter_next(iter);
        let Some(pair2_ptr) = obj_from_bits(pair2).as_ptr() else {
            panic!("molt_iter_next should return a tuple pair");
        };
        let pair2_items = unsafe {
            crate::object::seq_access::snapshot(_py, pair2_ptr, "test snapshot allocation failed")
                .unwrap()
        };
        assert_eq!(to_i64(obj_from_bits(pair2_items[0])), Some(20));
        assert!(!is_truthy(_py, obj_from_bits(pair2_items[1])));
        dec_ref_bits(_py, pair2);

        let pair3 = crate::molt_iter_next(iter);
        let Some(pair3_ptr) = obj_from_bits(pair3).as_ptr() else {
            panic!("molt_iter_next should return exhausted tuple pair");
        };
        let pair3_items = unsafe {
            crate::object::seq_access::snapshot(_py, pair3_ptr, "test snapshot allocation failed")
                .unwrap()
        };
        assert!(is_truthy(_py, obj_from_bits(pair3_items[1])));
        dec_ref_bits(_py, pair3);
        dec_ref_bits(_py, iter);

        let dict = PyDict_New();
        assert_ne!(dict, 0);
        let key = MoltObject::from_int(1).bits();
        let val = MoltObject::from_int(99).bits();
        assert_eq!(PyDict_SetItem(dict, key, val), 0);

        let borrowed = crate::molt_dict_getitem_borrowed(dict, key);
        assert_ne!(borrowed, 0);
        assert_eq!(to_i64(obj_from_bits(borrowed)), Some(99));
        let missing = crate::molt_dict_getitem_borrowed(dict, MoltObject::from_int(2).bits());
        assert_eq!(missing, 0);
        assert!(!exception_pending(_py));

        let keys = crate::molt_dict_keys(dict);
        assert_ne!(keys, 0);
        assert_eq!(PySequence_Length(keys), 1);
        dec_ref_bits(_py, keys);

        let values = crate::molt_dict_values(dict);
        assert_ne!(values, 0);
        assert_eq!(PySequence_Length(values), 1);
        dec_ref_bits(_py, values);

        let items = crate::molt_dict_items(dict);
        assert_ne!(items, 0);
        assert_eq!(PySequence_Length(items), 1);
        dec_ref_bits(_py, items);

        dec_ref_bits(_py, dict);
        dec_ref_bits(_py, list);
    });
}

#[test]
fn c_api_mapping_getitemstring() {
    let _guard = CApiTestGuard::new();
    // PyMapping_GetItemString → molt_getitem_method → molt_index
    // each re-enter with_gil_entry!, producing 3 nested GIL frames.
    // In debug mode this overflows the 2MB default thread stack when
    // prior tests have consumed stack budget.  Run on a dedicated
    // thread with 8MB stack.
    let r = std::thread::Builder::new()
        .stack_size(8 * 1024 * 1024)
        .name("c_api_mapping_test".into())
        .spawn(|| {
            c_api_mapping_getitemstring_body();
        })
        .expect("spawn mapping test thread")
        .join();
    if let Err(e) = r {
        std::panic::resume_unwind(e);
    }
}

fn c_api_mapping_getitemstring_body() {
    let dict = PyDict_New();
    assert_ne!(dict, 0);

    // Set up the key via GIL entry (no deep nesting here).
    let key_bits_cell = std::cell::Cell::new(0u64);
    crate::with_gil_entry_nopanic!(_py, {
        let key_ptr = alloc_string(_py, b"hello");
        assert!(!key_ptr.is_null());
        let val = MoltObject::from_int(99).bits();
        let kb = MoltObject::from_ptr(key_ptr).bits();
        assert_eq!(PyDict_SetItem(dict, kb, val), 0);
        key_bits_cell.set(kb);
    });
    let key_bits = key_bits_cell.get();

    // Call PyMapping_GetItemString outside with_gil_entry! to avoid
    // triple-nested GIL entry stack overflow.
    let got = unsafe { PyMapping_GetItemString(dict, c"hello".as_ptr()) };
    assert_ne!(got, 0);
    crate::with_gil_entry_nopanic!(_py, {
        assert_eq!(to_i64(obj_from_bits(got)), Some(99));
        dec_ref_bits(_py, got);
    });

    // Missing key should fail.
    let missing = unsafe { PyMapping_GetItemString(dict, c"nope".as_ptr()) };
    assert_eq!(missing, 0);
    crate::with_gil_entry_nopanic!(_py, {
        assert!(exception_pending(_py));
        let _ = molt_exception_clear();
    });

    // NULL key should fail.
    let null_key = unsafe { PyMapping_GetItemString(dict, std::ptr::null()) };
    assert_eq!(null_key, 0);
    crate::with_gil_entry_nopanic!(_py, {
        assert!(exception_pending(_py));
        let _ = molt_exception_clear();
        dec_ref_bits(_py, key_bits);
        dec_ref_bits(_py, dict);
    });
}

#[test]
fn c_api_mapping_haskey() {
    let _guard = CApiTestGuard::new();
    crate::with_gil_entry_nopanic!(_py, {
        let dict = PyDict_New();
        assert_ne!(dict, 0);

        let key = MoltObject::from_int(42).bits();
        let val = MoltObject::from_int(1).bits();
        assert_eq!(PyDict_SetItem(dict, key, val), 0);

        assert_eq!(PyMapping_HasKey(dict, key), 1);
        assert_eq!(PyMapping_HasKey(dict, MoltObject::from_int(999).bits()), 0);

        dec_ref_bits(_py, dict);
    });
}

// -----------------------------------------------------------------------
// Sequence Protocol addition tests
// -----------------------------------------------------------------------

#[test]
fn c_api_sequence_getitem() {
    let _guard = CApiTestGuard::new();
    crate::with_gil_entry_nopanic!(_py, {
        let list_ptr = alloc_list(
            _py,
            &[
                MoltObject::from_int(10).bits(),
                MoltObject::from_int(20).bits(),
                MoltObject::from_int(30).bits(),
            ],
        );
        assert!(!list_ptr.is_null());
        let list_bits = MoltObject::from_ptr(list_ptr).bits();

        let item = PySequence_GetItem(list_bits, 1);
        assert_ne!(item, 0);
        assert_eq!(to_i64(obj_from_bits(item)), Some(20));
        dec_ref_bits(_py, item);

        // Negative index: -1 should get last element.
        let last = PySequence_GetItem(list_bits, -1);
        assert_ne!(last, 0);
        assert_eq!(to_i64(obj_from_bits(last)), Some(30));
        dec_ref_bits(_py, last);

        // Out-of-bounds.
        let bad = PySequence_GetItem(list_bits, 100);
        assert_eq!(bad, 0);
        assert!(exception_pending(_py));
        let _ = molt_exception_clear();

        dec_ref_bits(_py, list_bits);
    });
}

#[test]
fn c_api_sequence_length() {
    let _guard = CApiTestGuard::new();
    crate::with_gil_entry_nopanic!(_py, {
        let list_ptr = alloc_list(
            _py,
            &[
                MoltObject::from_int(1).bits(),
                MoltObject::from_int(2).bits(),
            ],
        );
        assert!(!list_ptr.is_null());
        let list_bits = MoltObject::from_ptr(list_ptr).bits();
        assert_eq!(PySequence_Length(list_bits), 2);

        let tuple = PyTuple_New(5);
        assert_ne!(tuple, 0);
        assert_eq!(PySequence_Length(tuple), 5);

        dec_ref_bits(_py, list_bits);
        dec_ref_bits(_py, tuple);
    });
}

#[test]
fn c_api_sequence_contains() {
    let _guard = CApiTestGuard::new();
    crate::with_gil_entry_nopanic!(_py, {
        let list_ptr = alloc_list(
            _py,
            &[
                MoltObject::from_int(1).bits(),
                MoltObject::from_int(2).bits(),
                MoltObject::from_int(3).bits(),
            ],
        );
        assert!(!list_ptr.is_null());
        let list_bits = MoltObject::from_ptr(list_ptr).bits();

        assert_eq!(
            PySequence_Contains(list_bits, MoltObject::from_int(2).bits()),
            1
        );
        assert_eq!(
            PySequence_Contains(list_bits, MoltObject::from_int(9).bits()),
            0
        );

        dec_ref_bits(_py, list_bits);
    });
}

// -----------------------------------------------------------------------
// Bytes/String Protocol tests
// -----------------------------------------------------------------------

#[test]
fn c_api_bytes_from_string_and_size() {
    let _guard = CApiTestGuard::new();
    crate::with_gil_entry_nopanic!(_py, {
        let data = b"hello bytes";
        let bytes = unsafe { PyBytes_FromStringAndSize(data.as_ptr(), data.len() as isize) };
        assert_ne!(bytes, 0);

        let size = PyBytes_Size(bytes);
        assert_eq!(size, data.len() as isize);

        let ptr = PyBytes_AsString(bytes);
        assert!(!ptr.is_null());
        let observed = unsafe { std::slice::from_raw_parts(ptr, size as usize) };
        assert_eq!(observed, data);

        dec_ref_bits(_py, bytes);
    });
}

#[test]
fn c_api_bytes_empty() {
    let _guard = CApiTestGuard::new();
    crate::with_gil_entry_nopanic!(_py, {
        let bytes = unsafe { PyBytes_FromStringAndSize(std::ptr::null(), 0) };
        assert_ne!(bytes, 0);
        assert_eq!(PyBytes_Size(bytes), 0);
        dec_ref_bits(_py, bytes);
    });
}

#[test]
fn c_api_bytes_null_with_nonzero_len_fails() {
    let _guard = CApiTestGuard::new();
    crate::with_gil_entry_nopanic!(_py, {
        let bytes = unsafe { PyBytes_FromStringAndSize(std::ptr::null(), 5) };
        assert_eq!(bytes, 0);
        assert!(exception_pending(_py));
        let _ = molt_exception_clear();
    });
}

#[test]
fn c_api_bytes_negative_len_fails() {
    let _guard = CApiTestGuard::new();
    crate::with_gil_entry_nopanic!(_py, {
        let bytes = unsafe { PyBytes_FromStringAndSize(b"abc".as_ptr(), -1) };
        assert_eq!(bytes, 0);
        assert!(exception_pending(_py));
        let _ = molt_exception_clear();
    });
}

#[test]
fn c_api_bytes_asstring_type_error() {
    let _guard = CApiTestGuard::new();
    crate::with_gil_entry_nopanic!(_py, {
        let int_val = MoltObject::from_int(42).bits();
        let ptr = PyBytes_AsString(int_val);
        assert!(ptr.is_null());
        assert!(exception_pending(_py));
        let _ = molt_exception_clear();
    });
}

#[test]
fn c_api_bytes_size_type_error() {
    let _guard = CApiTestGuard::new();
    crate::with_gil_entry_nopanic!(_py, {
        let int_val = MoltObject::from_int(42).bits();
        let size = PyBytes_Size(int_val);
        assert_eq!(size, -1);
        assert!(exception_pending(_py));
        let _ = molt_exception_clear();
    });
}

#[test]
fn c_api_unicode_from_string() {
    let _guard = CApiTestGuard::new();
    crate::with_gil_entry_nopanic!(_py, {
        let str_bits = unsafe { PyUnicode_FromString(c"hello world".as_ptr()) };
        assert_ne!(str_bits, 0);
        assert_eq!(PyUnicode_Check(str_bits), 1);

        let utf8_ptr = PyUnicode_AsUTF8(str_bits);
        assert!(!utf8_ptr.is_null());
        let observed = unsafe { std::ffi::CStr::from_ptr(utf8_ptr).to_bytes() };
        assert_eq!(observed, b"hello world");
        // The string content might not be NUL-terminated in molt's internal
        // storage, so compare the known length.
        let mut out_size: isize = 0;
        let utf8_ptr2 = unsafe { PyUnicode_AsUTF8AndSize(str_bits, &mut out_size as *mut isize) };
        assert!(!utf8_ptr2.is_null());
        assert_eq!(out_size, 11); // "hello world" is 11 bytes
        let observed2 =
            unsafe { std::slice::from_raw_parts(utf8_ptr2 as *const u8, out_size as usize) };
        assert_eq!(observed2, b"hello world");

        dec_ref_bits(_py, str_bits);
    });
}

#[test]
fn c_api_unicode_from_string_null_fails() {
    let _guard = CApiTestGuard::new();
    crate::with_gil_entry_nopanic!(_py, {
        let str_bits = unsafe { PyUnicode_FromString(std::ptr::null()) };
        assert_eq!(str_bits, 0);
        assert!(exception_pending(_py));
        let _ = molt_exception_clear();
    });
}

#[test]
fn c_api_unicode_asutf8_type_error() {
    let _guard = CApiTestGuard::new();
    crate::with_gil_entry_nopanic!(_py, {
        let int_val = MoltObject::from_int(42).bits();
        let ptr = PyUnicode_AsUTF8(int_val);
        assert!(ptr.is_null());
        assert!(exception_pending(_py));
        let _ = molt_exception_clear();
    });
}

#[test]
fn c_api_unicode_asutf8andsize_null_size_ok() {
    let _guard = CApiTestGuard::new();
    crate::with_gil_entry_nopanic!(_py, {
        let str_bits = unsafe { PyUnicode_FromString(c"abc".as_ptr()) };
        assert_ne!(str_bits, 0);
        // Pass NULL for size — should not crash.
        let ptr = unsafe { PyUnicode_AsUTF8AndSize(str_bits, std::ptr::null_mut()) };
        assert!(!ptr.is_null());
        dec_ref_bits(_py, str_bits);
    });
}

// -----------------------------------------------------------------------
// Memory Protocol tests
// -----------------------------------------------------------------------

#[test]
fn c_api_pymem_malloc_realloc_free() {
    let _guard = CApiTestGuard::new();
    let ptr = unsafe { PyMem_Malloc(64) };
    assert!(!ptr.is_null());
    // Write to the allocated memory to verify it is usable.
    unsafe {
        std::ptr::write_bytes(ptr, 0xAB, 64);
        assert_eq!(*ptr, 0xAB);
    }
    let ptr2 = unsafe { PyMem_Realloc(ptr, 128) };
    assert!(!ptr2.is_null());
    // Original content should be preserved.
    unsafe {
        assert_eq!(*ptr2, 0xAB);
    }
    unsafe {
        PyMem_Free(ptr2);
    }
}

#[test]
fn c_api_pymem_malloc_zero_size() {
    let _guard = CApiTestGuard::new();
    // CPython returns a non-NULL pointer for size 0.
    let ptr = unsafe { PyMem_Malloc(0) };
    assert!(!ptr.is_null());
    unsafe {
        PyMem_Free(ptr);
    }
}

#[test]
fn c_api_pymem_free_null_is_safe() {
    let _guard = CApiTestGuard::new();
    // Freeing NULL should be a no-op.
    unsafe {
        PyMem_Free(std::ptr::null_mut());
    }
}

#[test]
fn c_api_pyobject_malloc_realloc_free() {
    let _guard = CApiTestGuard::new();
    let ptr = unsafe { PyObject_Malloc(32) };
    assert!(!ptr.is_null());
    unsafe {
        std::ptr::write_bytes(ptr, 0xCD, 32);
    }
    let ptr2 = unsafe { PyObject_Realloc(ptr, 64) };
    assert!(!ptr2.is_null());
    unsafe {
        assert_eq!(*ptr2, 0xCD);
    }
    unsafe {
        PyObject_Free(ptr2);
    }
}

#[test]
fn c_api_pyobject_free_null_is_safe() {
    let _guard = CApiTestGuard::new();
    // PyObject_Free delegates to PyMem_Free; NULL should be safe.
    unsafe {
        PyObject_Free(std::ptr::null_mut());
    }
}

// -----------------------------------------------------------------------
// Cross-protocol integration tests
// -----------------------------------------------------------------------

#[test]
fn c_api_number_mixed_int_float_arithmetic() {
    let _guard = CApiTestGuard::new();
    crate::with_gil_entry_nopanic!(_py, {
        // int + float -> float
        let a = MoltObject::from_int(3).bits();
        let b = MoltObject::from_float(0.125).bits();
        let res = PyNumber_Add(a, b);
        assert_ne!(res, 0);
        let val = obj_from_bits(res).as_float().unwrap();
        assert!((val - 3.125).abs() < 1e-10);
        dec_ref_bits(_py, res);

        // float * int -> float
        let c = MoltObject::from_float(2.5).bits();
        let d = MoltObject::from_int(4).bits();
        let res2 = PyNumber_Multiply(c, d);
        assert_ne!(res2, 0);
        assert_eq!(obj_from_bits(res2).as_float(), Some(10.0));
        dec_ref_bits(_py, res2);
    });
}

#[test]
fn c_api_sequence_and_mapping_on_dict() {
    let _guard = CApiTestGuard::new();
    crate::with_gil_entry_nopanic!(_py, {
        let dict = PyDict_New();
        assert_ne!(dict, 0);

        let k1 = MoltObject::from_int(1).bits();
        let v1 = MoltObject::from_int(100).bits();
        let k2 = MoltObject::from_int(2).bits();
        let v2 = MoltObject::from_int(200).bits();
        assert_eq!(PyDict_SetItem(dict, k1, v1), 0);
        assert_eq!(PyDict_SetItem(dict, k2, v2), 0);

        // PyMapping_Length works on dict.
        assert_eq!(PyMapping_Length(dict), 2);

        // PyMapping_HasKey works.
        assert_eq!(PyMapping_HasKey(dict, k1), 1);
        assert_eq!(PyMapping_HasKey(dict, MoltObject::from_int(999).bits()), 0);

        // PySequence_Contains also works on dict (checks keys).
        assert_eq!(PySequence_Contains(dict, k2), 1);
        assert_eq!(
            PySequence_Contains(dict, MoltObject::from_int(999).bits()),
            0
        );

        dec_ref_bits(_py, dict);
    });
}

#[test]
fn c_api_bytes_roundtrip_via_protocol() {
    let _guard = CApiTestGuard::new();
    crate::with_gil_entry_nopanic!(_py, {
        let data = b"\x00\x01\x02\xff";
        let bytes = unsafe { PyBytes_FromStringAndSize(data.as_ptr(), data.len() as isize) };
        assert_ne!(bytes, 0);
        assert_eq!(PyBytes_Size(bytes), 4);
        let ptr = PyBytes_AsString(bytes);
        assert!(!ptr.is_null());
        let observed = unsafe { std::slice::from_raw_parts(ptr, 4) };
        assert_eq!(observed, data);
        dec_ref_bits(_py, bytes);
    });
}

#[test]
fn c_api_object_protocol_repr_str_hash_truthy() {
    let _guard = CApiTestGuard::new();
    crate::with_gil_entry_nopanic!(_py, {
        let int_val = MoltObject::from_int(42).bits();

        // PyObject_Repr — re-entrant GIL acquisition
        let repr = PyObject_Repr(int_val);
        assert_ne!(repr, 0);
        dec_ref_bits(_py, repr);

        // PyObject_Str
        let str_val = PyObject_Str(int_val);
        assert_ne!(str_val, 0);
        dec_ref_bits(_py, str_val);

        // PyObject_Hash
        let hash = PyObject_Hash(int_val);
        assert_ne!(hash, -1);

        // PyObject_IsTrue / PyObject_Not
        assert_eq!(PyObject_IsTrue(int_val), 1);
        assert_eq!(PyObject_Not(int_val), 0);
        assert_eq!(PyObject_IsTrue(MoltObject::from_int(0).bits()), 0);
        assert_eq!(PyObject_Not(MoltObject::from_int(0).bits()), 1);
        assert_eq!(PyObject_IsTrue(MoltObject::from_bool(true).bits()), 1);
        assert_eq!(PyObject_IsTrue(MoltObject::from_bool(false).bits()), 0);
    });
}

#[test]
fn c_api_object_type_and_length() {
    let _guard = CApiTestGuard::new();
    // C-API functions acquire GIL internally — don't nest
    let list = PyList_New(3);
    assert_ne!(list, 0);

    let ty = PyObject_Type(list);
    assert_ne!(ty, 0);
    crate::with_gil_entry_nopanic!(_py, { dec_ref_bits(_py, ty) });

    assert_eq!(PyObject_Length(list), 3);
    assert_eq!(PyObject_Size(list), 3);

    crate::with_gil_entry_nopanic!(_py, { dec_ref_bits(_py, list) });
}

#[test]
fn c_api_rich_compare() {
    let _guard = CApiTestGuard::new();
    let a = MoltObject::from_int(10).bits();
    let b = MoltObject::from_int(20).bits();

    assert_eq!(PyObject_RichCompareBool(a, b, 0), 1); // 10 < 20
    assert_eq!(PyObject_RichCompareBool(a, b, 1), 1); // 10 <= 20
    assert_eq!(PyObject_RichCompareBool(a, b, 2), 0); // 10 == 20
    assert_eq!(PyObject_RichCompareBool(a, b, 3), 1); // 10 != 20
    assert_eq!(PyObject_RichCompareBool(a, b, 4), 0); // 10 > 20
    assert_eq!(PyObject_RichCompareBool(a, b, 5), 0); // 10 >= 20

    // Invalid op
    assert_eq!(PyObject_RichCompareBool(a, b, 99), -1);
    crate::with_gil_entry_nopanic!(_py, {
        assert!(exception_pending(_py));
        let _ = molt_exception_clear();
    });

    let cmp = PyObject_RichCompare(a, b, 2);
    assert_ne!(cmp, 0);
    crate::with_gil_entry_nopanic!(_py, { dec_ref_bits(_py, cmp) });
}

#[test]
fn c_api_callable_check_and_isinstance() {
    let _guard = CApiTestGuard::new();
    let int_val = MoltObject::from_int(5).bits();
    assert_eq!(PyCallable_Check(int_val), 0);

    crate::with_gil_entry_nopanic!(_py, {
        let builtins = builtin_classes(_py);
        let int_type = builtins.int;
        let result = PyObject_IsInstance(int_val, int_type);
        assert_eq!(result, 1);

        let none_val = none_bits();
        let result2 = PyObject_IsInstance(none_val, int_type);
        assert_eq!(result2, 0);
    });
}

#[test]
fn c_api_set_protocol() {
    let _guard = CApiTestGuard::new();
    crate::with_gil_entry_nopanic!(_py, {
        // Create empty set — capacity is raw u64, NOT NaN-boxed
        let set = molt_set_new(0u64);
        assert!(!obj_from_bits(set).is_none());

        // PySet_Check / PyFrozenSet_Check
        assert_eq!(PySet_Check(set), 1);
        assert_eq!(PyFrozenSet_Check(set), 0);

        // Add elements via runtime directly
        let k1 = MoltObject::from_int(10).bits();
        let k2 = MoltObject::from_int(20).bits();
        let add_res1 = molt_set_add(set, k1);
        assert!(!exception_pending(_py));
        if !obj_from_bits(add_res1).is_none() {
            dec_ref_bits(_py, add_res1);
        }
        let add_res2 = molt_set_add(set, k2);
        if !obj_from_bits(add_res2).is_none() {
            dec_ref_bits(_py, add_res2);
        }

        // PySet_Size
        assert_eq!(PySet_Size(set), 2);

        // PySet_Contains
        assert_eq!(PySet_Contains(set, k1), 1);
        assert_eq!(PySet_Contains(set, MoltObject::from_int(99).bits()), 0);

        // Discard
        let disc_res = molt_set_discard(set, k1);
        if !obj_from_bits(disc_res).is_none() {
            dec_ref_bits(_py, disc_res);
        }
        assert_eq!(PySet_Contains(set, k1), 0);

        // Pop
        let popped = PySet_Pop(set);
        assert_ne!(popped, 0);
        assert_eq!(PySet_Size(set), 0);
        dec_ref_bits(_py, popped);

        // Clear
        let add_res3 = molt_set_add(set, k1);
        if !obj_from_bits(add_res3).is_none() {
            dec_ref_bits(_py, add_res3);
        }
        assert_eq!(PySet_Clear(set), 0);
        assert_eq!(PySet_Size(set), 0);

        dec_ref_bits(_py, set);
    });
}

#[test]
fn c_api_dict_extended_operations() {
    let _guard = CApiTestGuard::new();
    let dict = PyDict_New();
    assert_ne!(dict, 0);

    crate::with_gil_entry_nopanic!(_py, {
        let k1_ptr = alloc_string(_py, b"hello");
        assert!(!k1_ptr.is_null());
        let k1 = MoltObject::from_ptr(k1_ptr).bits();
        let v1 = MoltObject::from_int(100).bits();
        assert_eq!(PyDict_SetItem(dict, k1, v1), 0);

        let got = PyDict_GetItemString(dict, c"hello".as_ptr());
        assert_ne!(got, 0);

        assert_eq!(PyDict_DelItem(dict, k1), 0);
        assert_eq!(PyDict_Size(dict), 0);

        let rc = PyDict_DelItem(dict, k1);
        assert_eq!(rc, -1);
        assert!(exception_pending(_py));
        let _ = molt_exception_clear();

        assert_eq!(PyDict_SetItem(dict, k1, v1), 0);
        let keys = PyDict_Keys(dict);
        assert_ne!(keys, 0);
        dec_ref_bits(_py, keys);
        let vals = PyDict_Values(dict);
        assert_ne!(vals, 0);
        dec_ref_bits(_py, vals);
        let items = PyDict_Items(dict);
        assert_ne!(items, 0);
        dec_ref_bits(_py, items);

        let copy = PyDict_Copy(dict);
        assert_ne!(copy, 0);
        assert_eq!(PyDict_Size(copy), 1);
        dec_ref_bits(_py, copy);

        dec_ref_bits(_py, k1);
        dec_ref_bits(_py, dict);
    });
}

#[test]
fn c_api_list_extended_operations() {
    let _guard = CApiTestGuard::new();
    let list = PyList_New(0);
    assert_ne!(list, 0);

    assert_eq!(PyList_Append(list, MoltObject::from_int(3).bits()), 0);
    assert_eq!(PyList_Append(list, MoltObject::from_int(1).bits()), 0);
    assert_eq!(PyList_Append(list, MoltObject::from_int(2).bits()), 0);
    assert_eq!(PyList_Size(list), 3);

    assert_eq!(PyList_Insert(list, 0, MoltObject::from_int(0).bits()), 0);
    assert_eq!(PyList_Size(list), 4);

    assert_eq!(PyList_Reverse(list), 0);
    assert_eq!(PyList_Sort(list), 0);

    let tup = PyList_AsTuple(list);
    assert_ne!(tup, 0);
    assert_eq!(PyTuple_Size(tup), 4);
    crate::with_gil_entry_nopanic!(_py, {
        dec_ref_bits(_py, tup);
        dec_ref_bits(_py, list);
    });
}

#[test]
fn c_api_exception_protocol() {
    let _guard = CApiTestGuard::new();
    assert_eq!(PyErr_Occurred(), 0);

    PyErr_SetString(0, c"test error".as_ptr());
    assert_ne!(PyErr_Occurred(), 0);

    PyErr_Clear();
    assert_eq!(PyErr_Occurred(), 0);

    let _ = PyErr_NoMemory();
    assert_ne!(PyErr_Occurred(), 0);
    PyErr_Clear();
}

#[test]
fn c_api_refcount_and_conversions() {
    let _guard = CApiTestGuard::new();
    // PyLong_FromLong / PyLong_AsLong — inline NaN-boxed, no GIL needed
    let long = PyLong_FromLong(42);
    assert_ne!(long, 0);
    assert_eq!(PyLong_AsLong(long), 42);

    let float = PyFloat_FromDouble(3.125);
    let val = PyFloat_AsDouble(float);
    assert!((val - 3.125).abs() < 0.001);

    let t = PyBool_FromLong(1);
    assert_eq!(PyObject_IsTrue(t), 1);
    let f = PyBool_FromLong(0);
    assert_eq!(PyObject_IsTrue(f), 0);

    let n = Py_BuildNone();
    assert!(obj_from_bits(n).is_none());

    crate::with_gil_entry_nopanic!(_py, {
        let s_ptr = alloc_string(_py, b"refcount_test");
        assert!(!s_ptr.is_null());
        let s = MoltObject::from_ptr(s_ptr).bits();
        Py_IncRef(s);
        Py_DecRef(s);
        Py_XINCREF(s);
        Py_XDECREF(s);
        dec_ref_bits(_py, s);
    });
}

#[test]
fn c_api_unicode_extended() {
    let _guard = CApiTestGuard::new();
    crate::with_gil_entry_nopanic!(_py, {
        let s_ptr = alloc_string(_py, b"hello");
        assert!(!s_ptr.is_null());
        let s = MoltObject::from_ptr(s_ptr).bits();

        assert_eq!(PyUnicode_GetLength(s), 5);

        let sub_ptr = alloc_string(_py, b"ell");
        assert!(!sub_ptr.is_null());
        let sub = MoltObject::from_ptr(sub_ptr).bits();
        assert_eq!(PyUnicode_Contains(s, sub), 1);

        let s2_ptr = alloc_string(_py, b" world");
        assert!(!s2_ptr.is_null());
        let s2 = MoltObject::from_ptr(s2_ptr).bits();
        let concat = PyUnicode_Concat(s, s2);
        assert_ne!(concat, 0);
        assert_eq!(PyUnicode_GetLength(concat), 11);
        dec_ref_bits(_py, concat);

        let cmp = PyUnicode_CompareWithASCIIString(s, c"hello".as_ptr());
        assert_eq!(cmp, 0);

        dec_ref_bits(_py, s2);
        dec_ref_bits(_py, sub);
        dec_ref_bits(_py, s);
    });
}

// --- C-heap ndarray buffer lease custody -------------------------------------
//
// These tests exercise the additive buffer-lease surface layered over the
// membership-only `molt_c_heap_{register,unregister,contains,type_canonicalize}`
// registry: a source-recompiled extension registers a per-kind exporter/releaser
// keyed on its typed `_MoltCHeapObject` header, then `molt_c_heap_export_buffer`
// hands out a descriptor only after the runtime's typed strided storage
// authority validates it and rejects malformed spans (draining the lease).

#[repr(C)]
struct TestCHeapHeader {
    magic: u64,
    refcnt: u32,
    kind: u32,
    type_ptr: usize,
    dealloc: usize,
}

const TEST_C_HEAP_MAGIC: u64 = 0x4d4f4c54434f424a;
static TEST_C_HEAP_BUFFER: [u8; 4] = [1, 2, 3, 4];
static TEST_C_HEAP_RELEASES: AtomicUsize = AtomicUsize::new(0);

unsafe extern "C" fn test_c_heap_buffer_exporter(
    _ptr: usize,
    out_view: *mut MoltBufferView,
) -> i32 {
    if out_view.is_null() {
        return -1;
    }
    unsafe {
        (*out_view).data = TEST_C_HEAP_BUFFER.as_ptr().cast_mut();
        (*out_view).len = TEST_C_HEAP_BUFFER.len() as u64;
        (*out_view).backing_capacity = TEST_C_HEAP_BUFFER.len() as u64;
        (*out_view).readonly = 1;
        (*out_view).ndim = 1;
        (*out_view).itemsize = 1;
        (*out_view).shape[0] = TEST_C_HEAP_BUFFER.len() as isize;
        (*out_view).strides[0] = 1;
        (*out_view).format[0] = b'B';
    }
    0
}

unsafe extern "C" fn test_c_heap_conflicting_exporter(
    _ptr: usize,
    _out_view: *mut MoltBufferView,
) -> i32 {
    -1
}

unsafe extern "C" fn test_c_heap_buffer_releaser(_ptr: usize, view: *mut MoltBufferView) -> i32 {
    if view.is_null() {
        return -1;
    }
    TEST_C_HEAP_RELEASES.fetch_add(1, Ordering::SeqCst);
    unsafe {
        (*view).data = std::ptr::null_mut();
        (*view).len = 0;
    }
    0
}

unsafe extern "C" fn test_c_heap_conflicting_releaser(
    _ptr: usize,
    _view: *mut MoltBufferView,
) -> i32 {
    -1
}

unsafe extern "C" fn test_c_heap_invalid_buffer_exporter(
    _ptr: usize,
    out_view: *mut MoltBufferView,
) -> i32 {
    if out_view.is_null() {
        return -1;
    }
    unsafe {
        (*out_view).data = std::ptr::null_mut();
        (*out_view).len = 1;
        (*out_view).backing_capacity = 1;
        (*out_view).readonly = 1;
        (*out_view).ndim = 1;
        (*out_view).itemsize = 1;
        (*out_view).shape[0] = 1;
        (*out_view).strides[0] = 1;
        (*out_view).format[0] = b'B';
    }
    0
}

unsafe extern "C" fn test_c_heap_leased_invalid_buffer_exporter(
    _ptr: usize,
    out_view: *mut MoltBufferView,
) -> i32 {
    TEST_C_HEAP_RELEASES.fetch_add(1, Ordering::SeqCst);
    unsafe { test_c_heap_invalid_buffer_exporter(_ptr, out_view) }
}

unsafe extern "C" fn test_c_heap_overflow_buffer_exporter(
    _ptr: usize,
    out_view: *mut MoltBufferView,
) -> i32 {
    if out_view.is_null() {
        return -1;
    }
    unsafe {
        (*out_view).data = TEST_C_HEAP_BUFFER.as_ptr().cast_mut();
        (*out_view).len = 1;
        (*out_view).backing_capacity = u64::MAX;
        (*out_view).readonly = 1;
        (*out_view).ndim = (*out_view).shape.len() as u32;
        (*out_view).itemsize = u64::MAX;
        for i in 0..(*out_view).shape.len() {
            (*out_view).shape[i] = isize::MAX;
            (*out_view).strides[i] = isize::MAX;
        }
        (*out_view).format[0] = b'B';
    }
    0
}

unsafe extern "C" fn test_c_heap_len_mismatch_buffer_exporter(
    _ptr: usize,
    out_view: *mut MoltBufferView,
) -> i32 {
    if out_view.is_null() {
        return -1;
    }
    unsafe {
        (*out_view).data = TEST_C_HEAP_BUFFER.as_ptr().cast_mut();
        (*out_view).len = TEST_C_HEAP_BUFFER.len() as u64;
        (*out_view).backing_capacity = TEST_C_HEAP_BUFFER.len() as u64;
        (*out_view).readonly = 1;
        (*out_view).ndim = 1;
        (*out_view).itemsize = 1;
        (*out_view).shape[0] = 0;
        (*out_view).strides[0] = 1;
        (*out_view).format[0] = b'B';
    }
    0
}

unsafe extern "C" fn test_c_heap_undersized_scalar_buffer_exporter(
    _ptr: usize,
    out_view: *mut MoltBufferView,
) -> i32 {
    if out_view.is_null() {
        return -1;
    }
    unsafe {
        (*out_view).data = TEST_C_HEAP_BUFFER.as_ptr().cast_mut();
        (*out_view).len = 1;
        (*out_view).backing_capacity = 1;
        (*out_view).readonly = 1;
        (*out_view).ndim = 0;
        (*out_view).itemsize = 4;
        (*out_view).format[0] = b'I';
    }
    0
}

#[test]
fn c_heap_buffer_lease_export_roundtrip() {
    let _guard = CApiTestGuard::new();
    const TYPE_KIND: u32 = 0x4e505401;
    const OBJECT_KIND: u32 = 0x4e504101;
    const WRONG_OBJECT_KIND: u32 = 0x4e504102;

    let type_header = Box::leak(Box::new(TestCHeapHeader {
        magic: TEST_C_HEAP_MAGIC,
        refcnt: molt_codegen_abi::IMMORTAL_REFCOUNT,
        kind: TYPE_KIND,
        type_ptr: 0,
        dealloc: 0,
    }));
    let type_ptr = (type_header as *mut TestCHeapHeader) as usize;
    type_header.type_ptr = type_ptr;
    assert_eq!(molt_c_heap_type_canonicalize(TYPE_KIND, type_ptr), type_ptr);
    assert_eq!(molt_c_heap_contains(type_ptr), 1);

    let mut object_header = TestCHeapHeader {
        magic: TEST_C_HEAP_MAGIC,
        refcnt: 1,
        kind: OBJECT_KIND,
        type_ptr,
        dealloc: 0,
    };
    let object_ptr = (&mut object_header as *mut TestCHeapHeader) as usize;
    assert_eq!(molt_c_heap_register(object_ptr), 0);
    assert_eq!(molt_c_heap_contains(object_ptr), 1);

    // Exporter registration is idempotent for the same (kind, type) and rejects
    // a conflicting type binding.
    assert_eq!(
        molt_c_heap_register_buffer_exporter(
            OBJECT_KIND,
            type_ptr,
            Some(test_c_heap_buffer_exporter)
        ),
        0
    );
    assert_eq!(
        molt_c_heap_register_buffer_exporter(
            OBJECT_KIND,
            type_ptr,
            Some(test_c_heap_buffer_exporter)
        ),
        0
    );
    assert_eq!(
        molt_c_heap_register_buffer_exporter(
            OBJECT_KIND,
            type_ptr,
            Some(test_c_heap_conflicting_exporter)
        ),
        0
    );

    // A releaser must name a kind whose exporter was registered.
    assert_eq!(
        molt_c_heap_register_buffer_releaser(
            WRONG_OBJECT_KIND,
            type_ptr,
            Some(test_c_heap_buffer_releaser)
        ),
        -1
    );
    assert_eq!(
        molt_c_heap_register_buffer_releaser(
            OBJECT_KIND,
            type_ptr,
            Some(test_c_heap_buffer_releaser)
        ),
        0
    );
    // Releaser registration is idempotent and pinned to the first binding.
    assert_eq!(
        molt_c_heap_register_buffer_releaser(
            OBJECT_KIND,
            type_ptr,
            Some(test_c_heap_conflicting_releaser)
        ),
        0
    );

    let mut view = MoltBufferView::default();
    assert_eq!(
        unsafe { molt_c_heap_export_buffer(object_ptr, &mut view) },
        0
    );
    assert_eq!(view.data, TEST_C_HEAP_BUFFER.as_ptr().cast_mut());
    assert_eq!(view.len, TEST_C_HEAP_BUFFER.len() as u64);
    assert_eq!(view.backing_capacity, TEST_C_HEAP_BUFFER.len() as u64);
    assert_eq!(view.owner, 0);
    assert_eq!(view.base, 0);
    assert_eq!(view.shape[0], TEST_C_HEAP_BUFFER.len() as isize);
    assert_eq!(view.strides[0], 1);

    TEST_C_HEAP_RELEASES.store(0, Ordering::SeqCst);
    assert_eq!(
        unsafe { molt_c_heap_release_buffer(object_ptr, &mut view) },
        0
    );
    assert_eq!(TEST_C_HEAP_RELEASES.load(Ordering::SeqCst), 1);
    assert!(view.data.is_null());
    assert_eq!(view.len, 0);
    assert_eq!(molt_c_heap_unregister(object_ptr), 0);
}

#[test]
fn c_heap_exporter_rejects_invalid_buffer_descriptor() {
    let _guard = CApiTestGuard::new();
    const TYPE_KIND: u32 = 0x4d545401;
    const OBJECT_KIND: u32 = 0x4d544101;

    let type_header = Box::leak(Box::new(TestCHeapHeader {
        magic: TEST_C_HEAP_MAGIC,
        refcnt: molt_codegen_abi::IMMORTAL_REFCOUNT,
        kind: TYPE_KIND,
        type_ptr: 0,
        dealloc: 0,
    }));
    let type_ptr = (type_header as *mut TestCHeapHeader) as usize;
    type_header.type_ptr = type_ptr;
    assert_eq!(molt_c_heap_type_canonicalize(TYPE_KIND, type_ptr), type_ptr);

    let mut object_header = TestCHeapHeader {
        magic: TEST_C_HEAP_MAGIC,
        refcnt: 1,
        kind: OBJECT_KIND,
        type_ptr,
        dealloc: 0,
    };
    let object_ptr = (&mut object_header as *mut TestCHeapHeader) as usize;
    assert_eq!(molt_c_heap_register(object_ptr), 0);
    assert_eq!(
        molt_c_heap_register_buffer_exporter(
            OBJECT_KIND,
            type_ptr,
            Some(test_c_heap_leased_invalid_buffer_exporter)
        ),
        0
    );
    assert_eq!(
        molt_c_heap_register_buffer_releaser(
            OBJECT_KIND,
            type_ptr,
            Some(test_c_heap_buffer_releaser)
        ),
        0
    );

    TEST_C_HEAP_RELEASES.store(0, Ordering::SeqCst);
    let mut view = MoltBufferView {
        data: TEST_C_HEAP_BUFFER.as_ptr().cast_mut(),
        len: 99,
        backing_capacity: 99,
        ..MoltBufferView::default()
    };
    assert_eq!(
        unsafe { molt_c_heap_export_buffer(object_ptr, &mut view) },
        -1
    );
    assert_eq!(view.len, 0);
    assert!(view.data.is_null());
    assert_eq!(TEST_C_HEAP_RELEASES.load(Ordering::SeqCst), 2);
    assert_eq!(molt_c_heap_unregister(object_ptr), 0);
}

#[test]
fn c_heap_exporter_rejects_overflowing_buffer_span() {
    let _guard = CApiTestGuard::new();
    const TYPE_KIND: u32 = 0x4d555401;
    const OBJECT_KIND: u32 = 0x4d554101;

    let type_header = Box::leak(Box::new(TestCHeapHeader {
        magic: TEST_C_HEAP_MAGIC,
        refcnt: molt_codegen_abi::IMMORTAL_REFCOUNT,
        kind: TYPE_KIND,
        type_ptr: 0,
        dealloc: 0,
    }));
    let type_ptr = (type_header as *mut TestCHeapHeader) as usize;
    type_header.type_ptr = type_ptr;
    assert_eq!(molt_c_heap_type_canonicalize(TYPE_KIND, type_ptr), type_ptr);

    let mut object_header = TestCHeapHeader {
        magic: TEST_C_HEAP_MAGIC,
        refcnt: 1,
        kind: OBJECT_KIND,
        type_ptr,
        dealloc: 0,
    };
    let object_ptr = (&mut object_header as *mut TestCHeapHeader) as usize;
    assert_eq!(molt_c_heap_register(object_ptr), 0);
    assert_eq!(
        molt_c_heap_register_buffer_exporter(
            OBJECT_KIND,
            type_ptr,
            Some(test_c_heap_overflow_buffer_exporter)
        ),
        0
    );

    let mut view = MoltBufferView {
        data: TEST_C_HEAP_BUFFER.as_ptr().cast_mut(),
        len: 99,
        backing_capacity: 99,
        ..MoltBufferView::default()
    };
    assert_eq!(
        unsafe { molt_c_heap_export_buffer(object_ptr, &mut view) },
        -1
    );
    assert_eq!(view.len, 0);
    assert!(view.data.is_null());
    assert_eq!(molt_c_heap_unregister(object_ptr), 0);
}

#[test]
fn c_heap_exporter_rejects_len_shape_mismatch() {
    let _guard = CApiTestGuard::new();
    const TYPE_KIND: u32 = 0x4d565401;
    const OBJECT_KIND: u32 = 0x4d564101;

    let type_header = Box::leak(Box::new(TestCHeapHeader {
        magic: TEST_C_HEAP_MAGIC,
        refcnt: molt_codegen_abi::IMMORTAL_REFCOUNT,
        kind: TYPE_KIND,
        type_ptr: 0,
        dealloc: 0,
    }));
    let type_ptr = (type_header as *mut TestCHeapHeader) as usize;
    type_header.type_ptr = type_ptr;
    assert_eq!(molt_c_heap_type_canonicalize(TYPE_KIND, type_ptr), type_ptr);

    let mut object_header = TestCHeapHeader {
        magic: TEST_C_HEAP_MAGIC,
        refcnt: 1,
        kind: OBJECT_KIND,
        type_ptr,
        dealloc: 0,
    };
    let object_ptr = (&mut object_header as *mut TestCHeapHeader) as usize;
    assert_eq!(molt_c_heap_register(object_ptr), 0);
    assert_eq!(
        molt_c_heap_register_buffer_exporter(
            OBJECT_KIND,
            type_ptr,
            Some(test_c_heap_len_mismatch_buffer_exporter)
        ),
        0
    );

    let mut view = MoltBufferView {
        data: TEST_C_HEAP_BUFFER.as_ptr().cast_mut(),
        len: 99,
        backing_capacity: 99,
        ..MoltBufferView::default()
    };
    assert_eq!(
        unsafe { molt_c_heap_export_buffer(object_ptr, &mut view) },
        -1
    );
    assert_eq!(view.len, 0);
    assert!(view.data.is_null());
    assert_eq!(molt_c_heap_unregister(object_ptr), 0);
}

#[test]
fn c_heap_exporter_rejects_undersized_scalar_buffer() {
    let _guard = CApiTestGuard::new();
    const TYPE_KIND: u32 = 0x4d575401;
    const OBJECT_KIND: u32 = 0x4d574101;

    let type_header = Box::leak(Box::new(TestCHeapHeader {
        magic: TEST_C_HEAP_MAGIC,
        refcnt: molt_codegen_abi::IMMORTAL_REFCOUNT,
        kind: TYPE_KIND,
        type_ptr: 0,
        dealloc: 0,
    }));
    let type_ptr = (type_header as *mut TestCHeapHeader) as usize;
    type_header.type_ptr = type_ptr;
    assert_eq!(molt_c_heap_type_canonicalize(TYPE_KIND, type_ptr), type_ptr);

    let mut object_header = TestCHeapHeader {
        magic: TEST_C_HEAP_MAGIC,
        refcnt: 1,
        kind: OBJECT_KIND,
        type_ptr,
        dealloc: 0,
    };
    let object_ptr = (&mut object_header as *mut TestCHeapHeader) as usize;
    assert_eq!(molt_c_heap_register(object_ptr), 0);
    assert_eq!(
        molt_c_heap_register_buffer_exporter(
            OBJECT_KIND,
            type_ptr,
            Some(test_c_heap_undersized_scalar_buffer_exporter)
        ),
        0
    );

    let mut view = MoltBufferView {
        data: TEST_C_HEAP_BUFFER.as_ptr().cast_mut(),
        len: 99,
        backing_capacity: 99,
        ..MoltBufferView::default()
    };
    assert_eq!(
        unsafe { molt_c_heap_export_buffer(object_ptr, &mut view) },
        -1
    );
    assert_eq!(view.len, 0);
    assert!(view.data.is_null());
    assert_eq!(molt_c_heap_unregister(object_ptr), 0);
}

// Derived from PR #31 "Revoke C-heap buffer hooks on type unregister", adapted
// onto main's surgical buffer-lease API. Unregistering a type pointer must drop
// its canonical `C_HEAP_TYPES` mapping and every `C_HEAP_BUFFER_EXPORTERS` entry
// it owns, so a stale type authority cannot keep leasing buffers after its type
// object is gone, and the freed kind is re-canonicalizable to a new type.
#[test]
fn c_heap_unregister_revokes_type_owned_buffer_hooks_and_canonical_type() {
    let _guard = CApiTestGuard::new();
    const TYPE_KIND: u32 = 0x5251_5401;
    const OBJECT_KIND: u32 = 0x5251_4101;

    let type_header = Box::leak(Box::new(TestCHeapHeader {
        magic: TEST_C_HEAP_MAGIC,
        refcnt: molt_codegen_abi::IMMORTAL_REFCOUNT,
        kind: TYPE_KIND,
        type_ptr: 0,
        dealloc: 0,
    }));
    let type_ptr = (type_header as *mut TestCHeapHeader) as usize;
    type_header.type_ptr = type_ptr;

    let next_type_header = Box::leak(Box::new(TestCHeapHeader {
        magic: TEST_C_HEAP_MAGIC,
        refcnt: molt_codegen_abi::IMMORTAL_REFCOUNT,
        kind: TYPE_KIND,
        type_ptr: 0,
        dealloc: 0,
    }));
    let next_type_ptr = (next_type_header as *mut TestCHeapHeader) as usize;
    next_type_header.type_ptr = next_type_ptr;

    let mut object_header = TestCHeapHeader {
        magic: TEST_C_HEAP_MAGIC,
        refcnt: 1,
        kind: OBJECT_KIND,
        type_ptr,
        dealloc: 0,
    };
    let object_ptr = (&mut object_header as *mut TestCHeapHeader) as usize;

    assert_eq!(molt_c_heap_type_canonicalize(TYPE_KIND, type_ptr), type_ptr);
    assert_eq!(molt_c_heap_register(object_ptr), 0);
    assert_eq!(
        molt_c_heap_register_buffer_exporter(
            OBJECT_KIND,
            type_ptr,
            Some(test_c_heap_buffer_exporter),
        ),
        0
    );
    assert_eq!(
        molt_c_heap_register_buffer_releaser(
            OBJECT_KIND,
            type_ptr,
            Some(test_c_heap_buffer_releaser),
        ),
        0
    );

    // The lease works before unregister.
    let mut view = MoltBufferView::default();
    assert_eq!(
        unsafe { molt_c_heap_export_buffer(object_ptr, &mut view) },
        0
    );
    assert_eq!(
        unsafe { molt_c_heap_release_buffer(object_ptr, &mut view) },
        0
    );

    // Unregistering the type pointer revokes its canonical mapping and hooks.
    assert_eq!(molt_c_heap_unregister(type_ptr), 0);
    assert_eq!(molt_c_heap_contains(type_ptr), 0);

    // A stale type's exporter must no longer fire for its objects.
    let mut stale_view = MoltBufferView::default();
    assert_eq!(
        unsafe { molt_c_heap_export_buffer(object_ptr, &mut stale_view) },
        -1
    );

    // The freed kind can canonicalize to a fresh type pointer.
    assert_eq!(
        molt_c_heap_type_canonicalize(TYPE_KIND, next_type_ptr),
        next_type_ptr
    );

    let _ = molt_c_heap_unregister(object_ptr);
    let _ = molt_c_heap_unregister(next_type_ptr);
}

// Derived from PR #44 "Validate C-heap buffer formats" + "Reject noncanonical
// C buffer readonly flags", adapted onto main's surgical buffer-lease API. Each
// exporter yields an otherwise-valid descriptor that fails exactly one of the
// new admission predicates; export must return -1 and drain the lease through
// the registered releaser.
unsafe extern "C" fn test_c_heap_unsupported_format_buffer_exporter(
    _ptr: usize,
    out_view: *mut MoltBufferView,
) -> i32 {
    if out_view.is_null() {
        return -1;
    }
    unsafe {
        (*out_view).data = TEST_C_HEAP_BUFFER.as_ptr().cast_mut();
        (*out_view).len = TEST_C_HEAP_BUFFER.len() as u64;
        (*out_view).backing_capacity = TEST_C_HEAP_BUFFER.len() as u64;
        (*out_view).readonly = 1;
        (*out_view).ndim = 1;
        (*out_view).itemsize = 1;
        (*out_view).shape[0] = TEST_C_HEAP_BUFFER.len() as isize;
        (*out_view).strides[0] = 1;
        (*out_view).format[0] = b'Z';
        (*out_view).format[1] = 0;
    }
    0
}

unsafe extern "C" fn test_c_heap_format_itemsize_mismatch_buffer_exporter(
    _ptr: usize,
    out_view: *mut MoltBufferView,
) -> i32 {
    if out_view.is_null() {
        return -1;
    }
    unsafe {
        (*out_view).data = TEST_C_HEAP_BUFFER.as_ptr().cast_mut();
        (*out_view).len = TEST_C_HEAP_BUFFER.len() as u64;
        (*out_view).backing_capacity = TEST_C_HEAP_BUFFER.len() as u64;
        (*out_view).readonly = 1;
        (*out_view).ndim = 1;
        (*out_view).itemsize = 1;
        (*out_view).shape[0] = TEST_C_HEAP_BUFFER.len() as isize;
        (*out_view).strides[0] = 1;
        // 'I' is a 4-byte format but itemsize claims 1: mismatch must reject.
        (*out_view).format[0] = b'I';
        (*out_view).format[1] = 0;
    }
    0
}

unsafe extern "C" fn test_c_heap_invalid_readonly_buffer_exporter(
    _ptr: usize,
    out_view: *mut MoltBufferView,
) -> i32 {
    if out_view.is_null() {
        return -1;
    }
    unsafe {
        (*out_view).data = TEST_C_HEAP_BUFFER.as_ptr().cast_mut();
        (*out_view).len = TEST_C_HEAP_BUFFER.len() as u64;
        (*out_view).backing_capacity = TEST_C_HEAP_BUFFER.len() as u64;
        (*out_view).readonly = 2;
        (*out_view).ndim = 1;
        (*out_view).itemsize = 1;
        (*out_view).shape[0] = TEST_C_HEAP_BUFFER.len() as isize;
        (*out_view).strides[0] = 1;
        (*out_view).format[0] = b'B';
        (*out_view).format[1] = 0;
    }
    0
}

fn assert_c_heap_exporter_rejects_and_releases(
    type_kind: u32,
    object_kind: u32,
    exporter: unsafe extern "C" fn(usize, *mut MoltBufferView) -> i32,
) {
    let _guard = CApiTestGuard::new();
    let type_header = Box::leak(Box::new(TestCHeapHeader {
        magic: TEST_C_HEAP_MAGIC,
        refcnt: molt_codegen_abi::IMMORTAL_REFCOUNT,
        kind: type_kind,
        type_ptr: 0,
        dealloc: 0,
    }));
    let type_ptr = (type_header as *mut TestCHeapHeader) as usize;
    type_header.type_ptr = type_ptr;
    assert_eq!(molt_c_heap_type_canonicalize(type_kind, type_ptr), type_ptr);

    let mut object_header = TestCHeapHeader {
        magic: TEST_C_HEAP_MAGIC,
        refcnt: 1,
        kind: object_kind,
        type_ptr,
        dealloc: 0,
    };
    let object_ptr = (&mut object_header as *mut TestCHeapHeader) as usize;
    assert_eq!(molt_c_heap_register(object_ptr), 0);
    assert_eq!(
        molt_c_heap_register_buffer_exporter(object_kind, type_ptr, Some(exporter)),
        0
    );
    assert_eq!(
        molt_c_heap_register_buffer_releaser(
            object_kind,
            type_ptr,
            Some(test_c_heap_buffer_releaser)
        ),
        0
    );

    TEST_C_HEAP_RELEASES.store(0, Ordering::SeqCst);
    let mut view = MoltBufferView::default();
    assert_eq!(
        unsafe { molt_c_heap_export_buffer(object_ptr, &mut view) },
        -1
    );
    assert_eq!(view.len, 0);
    assert!(view.data.is_null());
    assert_eq!(TEST_C_HEAP_RELEASES.load(Ordering::SeqCst), 1);
    assert_eq!(molt_c_heap_unregister(object_ptr), 0);
}

#[test]
fn c_heap_exporter_rejects_unsupported_buffer_format() {
    assert_c_heap_exporter_rejects_and_releases(
        0x4d585401,
        0x4d584101,
        test_c_heap_unsupported_format_buffer_exporter,
    );
}

#[test]
fn c_heap_exporter_rejects_format_itemsize_mismatch() {
    assert_c_heap_exporter_rejects_and_releases(
        0x4d595401,
        0x4d594101,
        test_c_heap_format_itemsize_mismatch_buffer_exporter,
    );
}

#[test]
fn c_heap_exporter_rejects_invalid_readonly_flag() {
    assert_c_heap_exporter_rejects_and_releases(
        0x4d5a5401,
        0x4d5a4101,
        test_c_heap_invalid_readonly_buffer_exporter,
    );
}

#[test]
fn memoryview_from_c_buffer_rejects_invalid_readonly_flag() {
    let _guard = CApiTestGuard::new();
    crate::with_gil_entry_nopanic!(_py, {
        let mut data = [1u8];
        let mut source = MoltBufferView {
            data: data.as_mut_ptr(),
            len: data.len() as u64,
            backing_capacity: data.len() as u64,
            readonly: 2,
            ..MoltBufferView::default()
        };
        source.shape[0] = 1;
        source.strides[0] = 1;
        source.format[0] = b'B';

        let view_bits = unsafe { molt_memoryview_from_buffer(&source as *const MoltBufferView) };
        assert_none_with_exception_class(_py, view_bits, "BufferError");
    });
}
#[test]
fn nested_public_gil_release_preserves_outer_attachment() {
    let _guard = crate::test_support::RuntimeTestTransaction::new();
    assert_eq!(crate::c_api::molt_init(), 0);
    assert_eq!(crate::c_api::molt_gil_acquire(), 0);
    assert_eq!(crate::c_api::molt_gil_acquire(), 0);
    assert_eq!(
        molt_cpython_abi::api::object::runtime_execution_attachment_count(),
        1
    );
    assert_eq!(crate::c_api::molt_gil_release(), 0);
    assert_eq!(crate::c_api::molt_gil_is_held(), 1);
    assert_eq!(
        molt_cpython_abi::api::object::runtime_execution_attachment_count(),
        1
    );
    assert_eq!(crate::c_api::molt_gil_release(), 0);
    assert_eq!(
        molt_cpython_abi::api::object::runtime_execution_attachment_count(),
        0
    );
}

#[test]
fn public_gil_boundary_reuses_macro_entry_attachment_when_gil_is_preheld() {
    let _guard = crate::test_support::RuntimeTestTransaction::new();
    assert_eq!(crate::c_api::molt_init(), 0);
    crate::with_gil_entry_nopanic!(_py, {
        assert!(molt_cpython_abi::api::object::runtime_execution_thread_is_attached());
        let attachments = molt_cpython_abi::api::object::runtime_execution_attachment_count();
        assert_eq!(crate::c_api::molt_gil_acquire(), 0);
        assert!(molt_cpython_abi::api::object::runtime_execution_thread_is_attached());
        assert_eq!(
            molt_cpython_abi::api::object::runtime_execution_attachment_count(),
            attachments,
            "persistent public boundary must reuse macro entry attachment"
        );
        assert_eq!(crate::c_api::molt_gil_release(), 0);
        assert!(crate::concurrency::gil::gil_held());
        assert_eq!(
            molt_cpython_abi::api::object::runtime_execution_attachment_count(),
            attachments,
            "persistent public release must not decrement the macro attachment"
        );
        assert!(
            molt_cpython_abi::api::object::runtime_execution_thread_is_attached(),
            "public release must preserve the owning macro attachment"
        );
    });
    assert!(!molt_cpython_abi::api::object::runtime_execution_thread_is_attached());
}

#[test]
fn public_gil_boundary_does_not_steal_nested_molt_custody() {
    let _guard = crate::test_support::RuntimeTestTransaction::new();
    assert_eq!(crate::c_api::molt_init(), 0);
    crate::with_gil_entry_nopanic!(_py, {
        molt_cpython_abi::api::object::attach_runtime_execution_thread();
        assert_eq!(crate::c_api::molt_gil_acquire(), 0);
        assert_eq!(crate::c_api::molt_gil_release(), 0);
        assert!(crate::concurrency::gil::gil_held());
        assert!(molt_cpython_abi::api::object::runtime_execution_thread_is_attached());
        molt_cpython_abi::api::object::detach_runtime_execution_thread();
    });
}
