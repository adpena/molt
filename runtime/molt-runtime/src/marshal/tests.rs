use super::*;

fn decoded<'a>(py: &'a PyToken<'_>, bytes: &[u8]) -> Value<'a> {
    read::decode(py, read::Source::memory(bytes)).unwrap()
}

pub(super) fn assert_pending(py: &PyToken<'_>, kind: BuiltinExc, message: &str) {
    let exception = molt_exception_last_pending_attached_body(py);
    assert!(crate::builtins::exceptions::exception_matches_kind(
        py, exception, kind
    ));
    let pointer = obj_from_bits(exception).as_ptr().unwrap();
    assert_eq!(crate::format_exception_message(py, pointer), message);
    dec_ref_bits(py, exception);
    clear_exception(py);
}

#[test]
fn historical_float_parser_matches_cpython_grammar_and_diagnostics() {
    let _transaction = crate::test_support::RuntimeTestTransaction::new();
    crate::concurrency::gil::with_gil(|py| {
        let py = &py;
        for (text, expected) in [
            (b"1.2\0junk".as_slice(), 1.2),
            (b"-Infinity", f64::NEG_INFINITY),
        ] {
            let mut wire = vec![b'f', text.len() as u8];
            wire.extend_from_slice(text);
            let value = decoded(py, &wire);
            assert_eq!(
                as_float_extended(obj_from_bits(value.bits())),
                Some(expected)
            );
        }
        for (text, rendered) in [
            (b" 1.2".as_slice(), "' 1.2'"),
            (b"1_2", "'1_2'"),
            (b"\xff", "'�'"),
            (b"", "''"),
        ] {
            let mut wire = vec![b'f', text.len() as u8];
            wire.extend_from_slice(text);
            assert!(read::decode(py, read::Source::memory(&wire)).is_err());
            assert_pending(
                py,
                BuiltinExc::ValueError,
                &format!("could not convert string to float: {rendered}"),
            );
        }
        for value in [-(1i64 << 32), 1i64 << 32] {
            assert!(version_argument(py, MoltObject::from_int(value).bits()).is_err());
            assert_pending(
                py,
                BuiltinExc::OverflowError,
                "Python int too large to convert to C int",
            );
        }
    });
}

#[test]
fn equal_wire_set_keys_follow_target_python_partial_order() {
    let transaction = crate::test_support::RuntimeTestTransaction::new();
    crate::concurrency::gil::with_gil(|py| {
        let py = &py;
        let mut wire = b"<\x02\0\0\0".to_vec();
        for _ in 0..2 {
            wire.push(b'y');
            wire.extend_from_slice(&f64::NAN.to_le_bytes());
            wire.extend_from_slice(&0f64.to_le_bytes());
        }
        let sample = decoded(py, &wire);
        for minor in 12..=14 {
            transaction.with_target_python_minor(py, minor, || {
                for version in 0..=4 {
                    let result = write::encode(py, sample.bits(), version);
                    if minor >= 14 {
                        assert!(result.is_ok());
                        let roundtrip = decoded(py, &result.unwrap());
                        assert_eq!(
                            unsafe { set_len(obj_from_bits(roundtrip.bits()).as_ptr().unwrap()) },
                            2
                        );
                    } else {
                        assert!(result.is_err());
                        assert_pending(py, BuiltinExc::MemoryError, "");
                    }
                }
            });
        }
    });
}

#[test]
fn decoded_strings_keep_cpython_interned_state_when_dumped_again() {
    // CPython 3.12.16: marshal.dumps(marshal.loads(blob), 4) for each blob.
    // Non-interned tags make a fresh string; the empty and one-character
    // strings are statically interned singletons (adversarial review,
    // 2026-11-07: the reader auto-interned identifier-like text).
    let _transaction = crate::test_support::RuntimeTestTransaction::new();
    crate::concurrency::gil::with_gil(|py| {
        let py = &py;
        for (blob, expected) in [
            (&b"z\x03abc"[..], &b"z\x03abc"[..]),
            (&b"a\x03\0\0\0abc"[..], &b"z\x03abc"[..]),
            (&b"u\x03\0\0\0abc"[..], &b"z\x03abc"[..]),
            (&b"\xda\x03abc"[..], &b"\xda\x03abc"[..]),
            (&b"z\x01a"[..], &b"\xda\x01a"[..]),
            (&b"z\x00"[..], &b"\xda\x00"[..]),
        ] {
            let value = decoded(py, blob);
            assert_eq!(
                write::encode(py, value.bits(), 4).unwrap(),
                expected,
                "{blob:?}"
            );
        }
    });
}

#[test]
fn scalar_wire_matches_cpython_and_declared_identity_partition() {
    let _transaction = crate::test_support::RuntimeTestTransaction::new();
    crate::concurrency::gil::with_gil(|py| {
        let py = &py;
        // CPython 3.12-3.14 marshal.c: cached ints are immortal, fresh other
        // scalar records are unique. These fixtures are independent wire data.
        assert_eq!(
            write::encode(py, MoltObject::from_int(123).bits(), 4).unwrap(),
            b"\xe9{\0\0\0"
        );
        assert_eq!(
            write::encode(py, MoltObject::from_int(1234567).bits(), 4).unwrap(),
            b"i\x87\xd6\x12\0"
        );
        assert_eq!(
            write::encode(py, MoltObject::from_float(1.25).bits(), 4).unwrap(),
            b"g\0\0\0\0\0\0\xf4?"
        );
        let list = allocated(
            py,
            alloc_list(
                py,
                &[
                    MoltObject::from_int(1).bits(),
                    MoltObject::from_int(1).bits(),
                ],
            ),
        )
        .unwrap();
        assert_eq!(
            write::encode(py, list.bits(), 4).unwrap(),
            b"[\x02\0\0\0\xe9\x01\0\0\0r\0\0\0\0"
        );
        let value = decoded(py, b"\xdb\x02\0\0\0\xe7\0\0\0\0\0\0\xf4?r\x01\0\0\0");
        let ptr = obj_from_bits(value.bits()).as_ptr().unwrap();
        assert_eq!(
            unsafe { crate::object::seq_access::item(ptr, 0) },
            Some(MoltObject::from_float(1.25).bits())
        );
        assert_eq!(
            unsafe { crate::object::seq_access::item(ptr, 1) },
            Some(MoltObject::from_float(1.25).bits())
        );
    });
}

#[test]
fn heap_references_and_private_tuple_cycles_keep_original_identity() {
    let _transaction = crate::test_support::RuntimeTestTransaction::new();
    crate::concurrency::gil::with_gil(|py| {
        let py = &py;
        let value = decoded(py, b"\xa8\x01\0\0\0[\x01\0\0\0r\0\0\0\0");
        let tuple = obj_from_bits(value.bits()).as_ptr().unwrap();
        let list_bits = unsafe { crate::object::seq_access::item(tuple, 0) }.unwrap();
        let list = obj_from_bits(list_bits).as_ptr().unwrap();
        assert_eq!(
            unsafe { crate::object::seq_access::item(list, 0) },
            Some(value.bits())
        );
        let wire = write::encode(py, value.bits(), 4).unwrap();
        let again = decoded(py, &wire);
        let tuple = obj_from_bits(again.bits()).as_ptr().unwrap();
        let child = unsafe { crate::object::seq_access::item(tuple, 0) }.unwrap();
        assert_eq!(
            unsafe { crate::object::seq_access::item(obj_from_bits(child).as_ptr().unwrap(), 0) },
            Some(again.bits())
        );
        // Break both test-owned cycles, then let normal guards retire them.
        molt_list_clear_attached_body(py, list_bits);
        molt_list_clear_attached_body(py, child);
    });
}

#[test]
fn malformed_references_and_constructing_tuple_keys_fail_before_hashing() {
    let _transaction = crate::test_support::RuntimeTestTransaction::new();
    crate::concurrency::gil::with_gil(|py| {
        let py = &py;
        for wire in [
            b"r\0\0\0\0".as_slice(),
            b"\xbe\x01\0\0\0r\0\0\0\0",
            b"\xa8\x01\0\0\0{r\0\0\0\0N0",
            b"l\x02\0\0\0\0\0\0\0",
        ] {
            assert!(
                read::decode(py, read::Source::memory(wire)).is_err(),
                "{wire:?}"
            );
            let error = owned(py, MoltObject::none().bits());
            assert!(error.is_err());
            let exception = molt_exception_last_pending_attached_body(py);
            assert!(crate::builtins::exceptions::exception_matches_kind(
                py,
                exception,
                BuiltinExc::ValueError
            ));
            dec_ref_bits(py, exception);
            clear_exception(py);
        }
        // Repeated immutable DAG edges are valid; they are not active cycles.
        let value = decoded(py, b"{\xa9\x02\xa9\x01i\x01\0\0\0r\x01\0\0\0N0");
        assert_eq!(
            unsafe { dict_len(obj_from_bits(value.bits()).as_ptr().unwrap()) },
            1
        );
    });
}

#[test]
fn admitted_target_facts_select_slice_version_and_aot_errors() {
    let transaction = crate::test_support::RuntimeTestTransaction::new();
    crate::concurrency::gil::with_gil(|py| {
        let py = &py;
        for minor in 12..=14 {
            transaction.with_target_python_minor(py, minor, || {
                assert_eq!(
                    obj_from_bits(molt_marshal_version_attached_body(py)).as_int(),
                    Some(if minor >= 14 { 5 } else { 4 })
                );
                if minor >= 14 {
                    let slice = decoded(py, b":NNN");
                    assert_eq!(write::encode(py, slice.bits(), 5).unwrap(), b":NNN");
                    assert!(write::encode(py, slice.bits(), 4).is_err());
                } else {
                    assert!(read::decode(py, read::Source::memory(b":NNN")).is_err());
                }
                clear_exception(py);
                assert!(read::decode(py, read::Source::memory(b"c")).is_err());
                let exception = molt_exception_last_pending_attached_body(py);
                assert!(crate::builtins::exceptions::exception_matches_kind(
                    py,
                    exception,
                    BuiltinExc::ValueError
                ));
                dec_ref_bits(py, exception);
                clear_exception(py);
            });
        }
    });
}

#[test]
fn public_marshal_c_api_uses_wire_codec_and_physical_numeric_ingress() {
    use molt_cpython_abi::api::{marshal as api, numbers, refcount, strings};
    let _transaction = crate::test_support::RuntimeTestTransaction::new();
    crate::concurrency::gil::with_gil(|py| {
        let py = &py;
        unsafe {
            let value = numbers::PyLong_FromLongLong(1234567);
            assert!(!value.is_null());
            let bytes = api::PyMarshal_WriteObjectToString(value, 4);
            assert!(!bytes.is_null());
            let mut data = std::ptr::null_mut();
            let mut length = 0;
            assert_eq!(
                strings::PyBytes_AsStringAndSize(bytes, &mut data, &mut length),
                0
            );
            assert_eq!(
                std::slice::from_raw_parts(data.cast::<u8>(), length as usize),
                b"i\x87\xd6\x12\0"
            );
            let result = api::PyMarshal_ReadObjectFromString(data, length);
            assert!(!result.is_null());
            assert_eq!(numbers::PyLong_AsLongLong(result), 1234567);
            refcount::Py_DECREF(result);
            refcount::Py_DECREF(bytes);
            refcount::Py_DECREF(value);

            let value = numbers::PyFloat_FromDouble(1.25);
            let bytes = api::PyMarshal_WriteObjectToString(value, 1);
            assert!(!bytes.is_null());
            assert_eq!(
                strings::PyBytes_AsStringAndSize(bytes, &mut data, &mut length),
                0
            );
            assert_eq!(
                std::slice::from_raw_parts(data.cast::<u8>(), length as usize),
                b"f\x041.25"
            );
            refcount::Py_DECREF(bytes);
            refcount::Py_DECREF(value);

            assert!(api::PyMarshal_ReadObjectFromString(std::ptr::null(), 0).is_null());
            assert!(!molt_cpython_abi::api::errors::PyErr_Occurred().is_null());
            molt_cpython_abi::api::errors::PyErr_Clear();
            clear_exception(py);
        }
    });
}

#[test]
fn public_marshal_c_api_preserves_container_aliases() {
    use molt_cpython_abi::api::{marshal as api, refcount, sequences, strings};
    let _transaction = crate::test_support::RuntimeTestTransaction::new();
    crate::concurrency::gil::with_gil(|py| {
        let py = &py;
        unsafe {
            let child = sequences::PyList_New(0);
            let root = sequences::PyList_New(2);
            assert!(!child.is_null() && !root.is_null());
            refcount::Py_INCREF(child);
            assert_eq!(sequences::PyList_SetItem(root, 0, child), 0);
            assert_eq!(sequences::PyList_SetItem(root, 1, child), 0);
            let bytes = api::PyMarshal_WriteObjectToString(root, 4);
            assert!(!bytes.is_null());
            let mut data = std::ptr::null_mut();
            let mut length = 0;
            assert_eq!(
                strings::PyBytes_AsStringAndSize(bytes, &mut data, &mut length),
                0
            );
            let decoded = api::PyMarshal_ReadObjectFromString(data, length);
            assert!(!decoded.is_null());
            assert_eq!(sequences::PyList_Size(decoded), 2);
            assert_eq!(
                sequences::PyList_GetItem(decoded, 0),
                sequences::PyList_GetItem(decoded, 1)
            );
            refcount::Py_DECREF(decoded);
            refcount::Py_DECREF(bytes);
            refcount::Py_DECREF(root);
            assert!(!exception_pending(py));
        }
    });
}
