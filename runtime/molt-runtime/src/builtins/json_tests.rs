use super::*;
use crate::BuiltinExc;

#[test]
fn json_decoding_preserves_python_codepoints_and_character_positions() {
    let _transaction = crate::test_support::RuntimeTestTransaction::new();
    crate::concurrency::gil::with_gil(|py| {
        let py = &py;
        let options = JsonLoadsOptions {
            parse_float: None,
            parse_int: None,
            parse_constant: None,
            object_hook: None,
            object_pairs_hook: None,
            strict: true,
        };
        // CPython v3.12.13 Lib/json/__init__.py and Modules/_json.c:
        // byte inputs use surrogatepass; only escaped pairs combine.
        let cases: &[(&[u8], &[u8])] = &[
            (b"\"\\ud800\"", b"\xed\xa0\x80"),
            (b"\"\\udfff\"", b"\xed\xbf\xbf"),
            (b"\"\\ud800\\udfff\"", b"\xf0\x90\x8f\xbf"),
            (b"\"\xed\xa0\x80\xed\xbf\xbf\"", b"\xed\xa0\x80\xed\xbf\xbf"),
            (b"\"\\udbff\\udfff\"", b"\xf4\x8f\xbf\xbf"),
            (b"\"\xf0\x90\x80\x80\"", b"\xf0\x90\x80\x80"),
            (b"\xef\xbb\xbf\"\xed\xa0\x80\"", b"\xed\xa0\x80"),
            (b"\xff\xfe\"\x00\x00\xd8\"\x00", b"\xed\xa0\x80"),
            (b"\xfe\xff\x00\"\xdc\x00\x00\"", b"\xed\xb0\x80"),
            (
                b"\xff\xfe\x00\x00\"\x00\x00\x00\x00\xd8\x00\x00\"\x00\x00\x00",
                b"\xed\xa0\x80",
            ),
            (
                b"\x00\x00\xfe\xff\x00\x00\x00\"\x00\x00\xdc\x00\x00\x00\x00\"",
                b"\xed\xb0\x80",
            ),
        ];
        for &(wire, expected) in cases {
            let text = decode_json_text(py, wire).unwrap();
            let parsed = run_json_parse(py, &text, options)
                .unwrap_or_else(|_| panic!("JSON parse failed: {wire:?}"));
            assert_eq!(
                crate::object::ops_format::string_obj_bytes(obj_from_bits(parsed)).as_deref(),
                Some(expected),
                "{wire:?}"
            );
            dec_ref_bits(py, parsed);
            let (scanned, end) = json_scanstring_decode(&text, 1, true).unwrap();
            assert_eq!(scanned, expected, "{wire:?}");
            assert_eq!(end, byte_index_to_char_index(&text, text.len()));
        }
        let text = b"x\"\xf0\x90\x80\x80\xed\xa0\x80\" tail";
        let (parsed, end) = run_json_raw_decode(py, text, 1, options)
            .unwrap_or_else(|_| panic!("raw decode failed"));
        assert_eq!(end, 5);
        assert_eq!(
            crate::object::ops_format::string_obj_bytes(obj_from_bits(parsed)).as_deref(),
            Some(b"\xf0\x90\x80\x80\xed\xa0\x80".as_slice())
        );
        dec_ref_bits(py, parsed);
        let invalid = b"[\n \"\xc3\xa9\xed\xa0\x80\",?]";
        let Err(JsonParseError::Message { msg, pos }) = run_json_parse(py, invalid, options) else {
            panic!("invalid JSON unexpectedly parsed")
        };
        assert_eq!(
            json_parse_error_message(invalid, &msg, pos),
            "Expecting value: line 2 column 7 (char 8)"
        );
        assert_eq!(json_string_line_col(invalid, 8), (2, 7));
        let input = alloc_string(py, b"\"decoded heap string \\ud800\"");
        assert!(!input.is_null());
        let input_bits = MoltObject::from_ptr(input).bits();
        let baseline = unsafe { (*header_from_obj_ptr(input)).ref_count_snapshot() };
        assert_eq!(molt_json_coerce_text(input_bits), input_bits);
        assert_eq!(
            unsafe { (*header_from_obj_ptr(input)).ref_count_snapshot() },
            baseline + 1
        );
        dec_ref_bits(py, input_bits);
        let scanned = molt_json_scanstring_obj(
            input_bits,
            MoltObject::from_int(1).bits(),
            MoltObject::from_bool(true).bits(),
        );
        let tuple = obj_from_bits(scanned)
            .as_ptr()
            .expect("scanner result tuple");
        let child = unsafe { crate::object::seq_access::item(tuple, 0) }.unwrap();
        let child = obj_from_bits(child).as_ptr().unwrap();
        assert_eq!(
            unsafe { (*header_from_obj_ptr(child)).ref_count_snapshot() },
            1,
            "only the result tuple owns the decoded heap string"
        );
        dec_ref_bits(py, scanned);
        dec_ref_bits(py, input_bits);
        assert!(!exception_pending(py));
    });
}

#[test]
fn json_encoding_detection_retains_cpython_malformed_prefix_policy() {
    // The encoding decision precedes validation, so even malformed
    // prefixes select the decoder (and therefore the error label).
    for (input, expected) in [
        (b"".as_slice(), "utf-8"),
        (b"\x00", "utf-8"),
        (b"\x00\x01", "utf-16-be"),
        (b"\x01\x00", "utf-16-le"),
        (b"\x00\x01\x00", "utf-8"),
        (b"\x01\x00\x00", "utf-8"),
        (b"\x00\x00\x01\x22", "utf-32-be"),
        (b"\x00\x01\x22\x00", "utf-16-be"),
        (b"\x22\x00\x00\x01", "utf-16-le"),
        (b"\x22\x00\x00\x00", "utf-32-le"),
        (b"\xff\xfe\x00\x00", "utf-32"),
        (b"\x00\x00\xfe\xff", "utf-32"),
    ] {
        assert_eq!(detect_json_encoding(input), expected, "{input:?}");
    }
}

#[test]
fn json_finite_float_tokens_use_canonical_cpython_repr() {
    let cases = [
        (1e16, "1e+16"),
        (1e-5, "1e-05"),
        (1e100, "1e+100"),
        (5e-324, "5e-324"),
        (137839762462415.62, "137839762462415.62"),
        (-0.0, "-0.0"),
    ];
    for (value, expected) in cases {
        match json_float_token(value, true) {
            Ok(actual) => assert_eq!(actual, expected),
            Err(_) => panic!("finite float token unexpectedly failed for {value:?}"),
        }
    }
}

#[test]
fn json_nonfinite_policy_remains_json_specific() {
    assert!(matches!(
        json_float_token(f64::NAN, true),
        Ok(token) if token == "NaN"
    ));
    assert!(matches!(
        json_float_token(f64::INFINITY, true),
        Ok(token) if token == "Infinity"
    ));
    assert!(matches!(
        json_float_token(f64::NEG_INFINITY, false),
        Err(JsonEncodeError::Value(_))
    ));
}

#[cfg(test)]
mod dict_order_tests {
    use super::*;
    use crate::builtins::functions::{alloc_runtime_function_obj, runtime_fn_addr};
    use std::sync::atomic::{AtomicBool, AtomicU64, AtomicUsize, Ordering};

    static EQ: AtomicBool = AtomicBool::new(false);
    static LT_CALLS: AtomicUsize = AtomicUsize::new(0);
    static BOOL_CALLS: AtomicUsize = AtomicUsize::new(0);
    static DEFAULT_CALLS: AtomicUsize = AtomicUsize::new(0);
    static RESULT: AtomicU64 = AtomicU64::new(0);
    static FAILURE: AtomicU64 = AtomicU64::new(0);
    static CLEAR_DICT: AtomicU64 = AtomicU64::new(0);
    static PINNED_VALUE: AtomicU64 = AtomicU64::new(0);
    static PINNED_REFS: AtomicUsize = AtomicUsize::new(0);

    fn reset() {
        EQ.store(false, Ordering::SeqCst);
        LT_CALLS.store(0, Ordering::SeqCst);
        BOOL_CALLS.store(0, Ordering::SeqCst);
        DEFAULT_CALLS.store(0, Ordering::SeqCst);
        RESULT.store(0, Ordering::SeqCst);
        FAILURE.store(0, Ordering::SeqCst);
        CLEAR_DICT.store(0, Ordering::SeqCst);
        PINNED_VALUE.store(0, Ordering::SeqCst);
        PINNED_REFS.store(0, Ordering::SeqCst);
    }

    fn refs(bits: u64) -> u32 {
        let pointer = obj_from_bits(bits).as_ptr().expect("heap fixture value");
        unsafe { (*header_from_obj_ptr(pointer)).ref_count_snapshot() }
    }

    fn install(py: &PyToken<'_>, class: u64, name: &[u8], target: *const (), arity: u64) {
        let name = attr_name_bits_from_bytes(py, name).unwrap();
        let function = MoltObject::from_ptr(alloc_runtime_function_obj(
            py,
            runtime_fn_addr("json_order_callback", target),
            arity,
        ))
        .bits();
        let result = molt_set_attr_name(class, name, function);
        for bits in [result, function, name] {
            dec_ref_bits(py, bits);
        }
        assert!(!exception_pending(py));
    }

    fn subclass(py: &PyToken<'_>, base: u64) -> u64 {
        let name = attr_name_bits_from_bytes(py, b"JsonOrderProbe").unwrap();
        let class = molt_class_new(name);
        dec_ref_bits(py, name);
        let result = molt_class_set_base(class, base);
        dec_ref_bits(py, result);
        unsafe {
            crate::object::class_finish_definition(py, obj_from_bits(class).as_ptr().unwrap())
        }
        .expect("fixture subclass");
        class
    }

    extern "C" fn equality(_left: u64, _right: u64) -> u64 {
        MoltObject::from_bool(EQ.load(Ordering::SeqCst)).bits()
    }
    extern "C" fn identity_hash(value: u64) -> u64 {
        crate::molt_id(value)
    }
    extern "C" fn comparison(_left: u64, _right: u64) -> u64 {
        crate::with_gil_entry_nopanic!(py, {
            LT_CALLS.fetch_add(1, Ordering::SeqCst);
            let result = RESULT.load(Ordering::SeqCst);
            inc_ref_bits(py, result);
            result
        })
    }
    extern "C" fn truth(_self: u64) -> u64 {
        crate::with_gil_entry_nopanic!(py, {
            BOOL_CALLS.fetch_add(1, Ordering::SeqCst);
            let dict = CLEAR_DICT.swap(0, Ordering::SeqCst);
            if dict != 0 {
                dec_ref_bits(py, crate::molt_dict_clear(dict));
                PINNED_REFS.store(
                    refs(PINNED_VALUE.load(Ordering::SeqCst)) as usize,
                    Ordering::SeqCst,
                );
            }
            let error = FAILURE.load(Ordering::SeqCst);
            if error != 0 {
                return crate::molt_raise(error);
            }
            MoltObject::from_bool(false).bits()
        })
    }

    extern "C" fn default_failure(_object: u64) -> u64 {
        DEFAULT_CALLS.fetch_add(1, Ordering::SeqCst);
        crate::molt_raise(FAILURE.load(Ordering::SeqCst))
    }

    fn extended(py: &PyToken<'_>, dict: u64, skip: bool, sort: bool) -> u64 {
        let item = MoltObject::from_ptr(alloc_string(py, b", ")).bits();
        let key = MoltObject::from_ptr(alloc_string(py, b": ")).bits();
        let result = molt_json_dumps_ex(
            dict,
            MoltObject::from_bool(skip).bits(),
            MoltObject::from_bool(true).bits(),
            MoltObject::from_bool(true).bits(),
            MoltObject::from_bool(true).bits(),
            MoltObject::from_bool(sort).bits(),
            MoltObject::none().bits(),
            item,
            key,
            MoltObject::none().bits(),
        );
        dec_ref_bits(py, key);
        dec_ref_bits(py, item);
        result
    }

    fn assert_error(py: &PyToken<'_>, result: u64, name: crate::BuiltinExc) {
        assert!(obj_from_bits(result).is_none());
        assert!(exception_pending(py));
        let error = molt_exception_last();
        assert!(crate::builtins::exceptions::exception_matches_kind(
            py, error, name
        ));
        crate::clear_exception(py);
        dec_ref_bits(py, error);
        dec_ref_bits(py, result);
    }

    #[test]
    fn original_numeric_items_sort_before_json_key_conversion_in_both_encoders() {
        let _transaction = crate::test_support::RuntimeTestTransaction::new();
        crate::with_gil_entry_nopanic!(py, {
            let dict = MoltObject::from_ptr(alloc_dict_with_pairs(
                py,
                &[
                    MoltObject::from_int(10).bits(),
                    MoltObject::from_int(100).bits(),
                    MoltObject::from_int(2).bits(),
                    MoltObject::from_int(200).bits(),
                ],
            ))
            .bits();
            for result in [
                molt_json_dumps(
                    dict,
                    MoltObject::none().bits(),
                    MoltObject::from_bool(true).bits(),
                    MoltObject::from_bool(true).bits(),
                ),
                extended(py, dict, false, true),
            ] {
                assert!(!exception_pending(py));
                assert_eq!(
                    string_obj_to_owned(obj_from_bits(result)).as_deref(),
                    Some("{\"2\": 200, \"10\": 100}")
                );
                dec_ref_bits(py, result);
            }
            let result = extended(py, dict, false, false);
            assert_eq!(
                string_obj_to_owned(obj_from_bits(result)).as_deref(),
                Some("{\"10\": 100, \"2\": 200}")
            );
            dec_ref_bits(py, result);
            dec_ref_bits(py, dict);
        });
    }

    #[test]
    fn mixed_original_keys_raise_before_skipkeys_and_release_snapshot_edges() {
        let _transaction = crate::test_support::RuntimeTestTransaction::new();
        crate::with_gil_entry_nopanic!(py, {
            let tuple =
                MoltObject::from_ptr(alloc_tuple(py, &[MoltObject::from_int(0).bits()])).bits();
            let value = MoltObject::from_ptr(alloc_list(py, &[])).bits();
            let dict = MoltObject::from_ptr(alloc_dict_with_pairs(
                py,
                &[MoltObject::from_int(1).bits(), value, tuple, value],
            ))
            .bits();
            let before = (refs(tuple), refs(value));
            for skip in [false, true] {
                assert_error(
                    py,
                    extended(py, dict, skip, true),
                    crate::BuiltinExc::TypeError,
                );
                assert_eq!((refs(tuple), refs(value)), before);
            }
            assert_error(
                py,
                molt_json_dumps(
                    dict,
                    MoltObject::none().bits(),
                    MoltObject::from_bool(true).bits(),
                    MoltObject::from_bool(true).bits(),
                ),
                crate::BuiltinExc::TypeError,
            );
            let result = extended(py, dict, true, false);
            assert_eq!(
                string_obj_to_owned(obj_from_bits(result)).as_deref(),
                Some("{\"1\": []}")
            );
            for bits in [result, dict, value, tuple] {
                dec_ref_bits(py, bits);
            }
            assert!(!exception_pending(py));
        });
    }

    #[test]
    fn tuple_key_equality_reaches_value_comparison_before_key_filtering() {
        let _transaction = crate::test_support::RuntimeTestTransaction::new();
        crate::with_gil_entry_nopanic!(py, {
            reset();
            let class = subclass(py, builtin_classes(py).object);
            install(py, class, b"__eq__", equality as *const (), 2);
            install(py, class, b"__hash__", identity_hash as *const (), 1);
            install(py, class, b"__lt__", comparison as *const (), 2);
            let left = unsafe { call_callable0(py, class) };
            let right = unsafe { call_callable0(py, class) };
            let text = MoltObject::from_ptr(alloc_string(py, b"incomparable value")).bits();
            let dict = MoltObject::from_ptr(alloc_dict_with_pairs(
                py,
                &[left, MoltObject::from_int(1).bits(), right, text],
            ))
            .bits();
            assert_eq!(
                unsafe { dict_len(obj_from_bits(dict).as_ptr().unwrap()) },
                2
            );
            EQ.store(true, Ordering::SeqCst);
            assert_error(
                py,
                extended(py, dict, true, true),
                crate::BuiltinExc::TypeError,
            );
            assert_eq!(
                LT_CALLS.load(Ordering::SeqCst),
                0,
                "equal keys compare the original values instead"
            );
            reset();
            for bits in [dict, text, right, left, class] {
                dec_ref_bits(py, bits);
            }
        });
    }

    #[test]
    fn comparison_truth_callbacks_keep_snapshot_values_alive_after_dict_clear() {
        let _transaction = crate::test_support::RuntimeTestTransaction::new();
        crate::with_gil_entry_nopanic!(py, {
            reset();
            let class = subclass(py, builtin_classes(py).object);
            install(py, class, b"__eq__", equality as *const (), 2);
            install(py, class, b"__hash__", identity_hash as *const (), 1);
            install(py, class, b"__lt__", comparison as *const (), 2);
            let truth_class = subclass(py, builtin_classes(py).object);
            install(py, truth_class, b"__bool__", truth as *const (), 1);
            let result_object = unsafe { call_callable0(py, truth_class) };
            let left = unsafe { call_callable0(py, class) };
            let right = unsafe { call_callable0(py, class) };
            let value = MoltObject::from_ptr(alloc_list(py, &[])).bits();
            let dict = MoltObject::from_ptr(alloc_dict_with_pairs(
                py,
                &[left, value, right, MoltObject::none().bits()],
            ))
            .bits();
            assert!(!exception_pending(py));
            RESULT.store(result_object, Ordering::SeqCst);
            CLEAR_DICT.store(dict, Ordering::SeqCst);
            PINNED_VALUE.store(value, Ordering::SeqCst);
            let result = extended(py, dict, true, true);
            assert!(!exception_pending(py));
            assert_eq!(
                string_obj_to_owned(obj_from_bits(result)).as_deref(),
                Some("{}")
            );
            assert!(LT_CALLS.load(Ordering::SeqCst) > 0);
            assert!(BOOL_CALLS.load(Ordering::SeqCst) > 0);
            assert!(PINNED_REFS.load(Ordering::SeqCst) >= 2);
            assert_eq!(
                refs(value),
                1,
                "all snapshot/tuple edges retired after encoding"
            );
            assert_eq!(
                refs(result_object),
                1,
                "comparison result ownership retired"
            );
            reset();
            for bits in [
                result,
                dict,
                value,
                right,
                left,
                result_object,
                truth_class,
                class,
            ] {
                dec_ref_bits(py, bits);
            }
        });
    }

    #[test]
    fn truth_callback_exception_identity_and_owners_survive_failed_sort() {
        let _transaction = crate::test_support::RuntimeTestTransaction::new();
        crate::with_gil_entry_nopanic!(py, {
            reset();
            let class = subclass(py, builtin_classes(py).object);
            install(py, class, b"__eq__", equality as *const (), 2);
            install(py, class, b"__hash__", identity_hash as *const (), 1);
            install(py, class, b"__lt__", comparison as *const (), 2);
            let truth_class = subclass(py, builtin_classes(py).object);
            install(py, truth_class, b"__bool__", truth as *const (), 1);
            let result_object = unsafe { call_callable0(py, truth_class) };
            let left = unsafe { call_callable0(py, class) };
            let right = unsafe { call_callable0(py, class) };
            let value = MoltObject::from_ptr(alloc_list(py, &[])).bits();
            let dict =
                MoltObject::from_ptr(alloc_dict_with_pairs(py, &[left, value, right, value]))
                    .bits();
            let before = (refs(left), refs(right), refs(value));
            raise_exception::<()>(
                py,
                BuiltinExc::LookupError,
                "independent JSON truth failure",
            );
            let failure = molt_exception_last();
            crate::clear_exception(py);
            FAILURE.store(failure, Ordering::SeqCst);
            RESULT.store(result_object, Ordering::SeqCst);
            let result = extended(py, dict, true, true);
            assert!(obj_from_bits(result).is_none() && exception_pending(py));
            let actual = molt_exception_last();
            assert_eq!(actual, failure);
            crate::clear_exception(py);
            dec_ref_bits(py, actual);
            assert_eq!((refs(left), refs(right), refs(value)), before);
            assert_eq!(refs(result_object), 1);
            reset();
            for bits in [
                result,
                failure,
                dict,
                value,
                right,
                left,
                result_object,
                truth_class,
                class,
            ] {
                dec_ref_bits(py, bits);
            }
        });
    }

    #[test]
    fn stable_tuple_order_preserves_nan_unordered_runs() {
        let _transaction = crate::test_support::RuntimeTestTransaction::new();
        crate::with_gil_entry_nopanic!(py, {
            let dict = MoltObject::from_ptr(alloc_dict_with_pairs(
                py,
                &[
                    MoltObject::from_float(f64::NAN).bits(),
                    MoltObject::from_int(0).bits(),
                    MoltObject::from_int(2).bits(),
                    MoltObject::from_int(2).bits(),
                    MoltObject::from_int(10).bits(),
                    MoltObject::from_int(10).bits(),
                ],
            ))
            .bits();
            let result = extended(py, dict, false, true);
            assert!(!exception_pending(py));
            assert_eq!(
                string_obj_to_owned(obj_from_bits(result)).as_deref(),
                Some("{\"NaN\": 0, \"2\": 2, \"10\": 10}")
            );
            dec_ref_bits(py, result);
            dec_ref_bits(py, dict);
        });
    }

    #[test]
    fn first_value_default_precedes_later_nonfinite_key_in_both_orders() {
        let _transaction = crate::test_support::RuntimeTestTransaction::new();
        crate::with_gil_entry_nopanic!(py, {
            reset();
            let class = subclass(py, builtin_classes(py).object);
            let object = unsafe { call_callable0(py, class) };
            let dict = MoltObject::from_ptr(alloc_dict_with_pairs(
                py,
                &[
                    MoltObject::from_float(1.0).bits(),
                    object,
                    MoltObject::from_float(f64::INFINITY).bits(),
                    MoltObject::from_int(0).bits(),
                ],
            ))
            .bits();
            let callback = MoltObject::from_ptr(alloc_runtime_function_obj(
                py,
                runtime_fn_addr("json_default_failure", default_failure as *const ()),
                1,
            ))
            .bits();
            let item = MoltObject::from_ptr(alloc_string(py, b", ")).bits();
            let key = MoltObject::from_ptr(alloc_string(py, b": ")).bits();
            let before = refs(object);
            raise_exception::<()>(py, BuiltinExc::LookupError, "first JSON value marker");
            let failure = molt_exception_last();
            crate::clear_exception(py);
            FAILURE.store(failure, Ordering::SeqCst);
            for sort in [false, true] {
                DEFAULT_CALLS.store(0, Ordering::SeqCst);
                let result = molt_json_dumps_ex(
                    dict,
                    MoltObject::from_bool(false).bits(),
                    MoltObject::from_bool(true).bits(),
                    MoltObject::from_bool(true).bits(),
                    MoltObject::from_bool(false).bits(),
                    MoltObject::from_bool(sort).bits(),
                    MoltObject::none().bits(),
                    item,
                    key,
                    callback,
                );
                assert!(obj_from_bits(result).is_none() && exception_pending(py));
                let actual = molt_exception_last();
                assert_eq!(
                    actual, failure,
                    "the first value's callback precedes the later Infinity key"
                );
                assert_eq!(DEFAULT_CALLS.load(Ordering::SeqCst), 1);
                crate::clear_exception(py);
                dec_ref_bits(py, actual);
                dec_ref_bits(py, result);
                assert_eq!(refs(object), before);
                // The basic encoder has no default callback: its first value's
                // TypeError must likewise precede the later key's ValueError.
                assert_error(
                    py,
                    molt_json_dumps(
                        dict,
                        MoltObject::none().bits(),
                        MoltObject::from_bool(sort).bits(),
                        MoltObject::from_bool(true).bits(),
                    ),
                    crate::BuiltinExc::TypeError,
                );
                assert_eq!(refs(object), before);
            }
            reset();
            for bits in [failure, key, item, callback, dict, object, class] {
                dec_ref_bits(py, bits);
            }
        });
    }

    #[test]
    fn skipped_items_preserve_original_nonempty_indentation_in_both_orders() {
        let _transaction = crate::test_support::RuntimeTestTransaction::new();
        crate::with_gil_entry_nopanic!(py, {
            let zero = MoltObject::from_int(0).bits();
            let one = MoltObject::from_int(1).bits();
            let two = MoltObject::from_int(2).bits();
            let left = MoltObject::from_ptr(alloc_tuple(py, &[zero])).bits();
            let right = MoltObject::from_ptr(alloc_tuple(py, &[two])).bits();
            let skipped =
                MoltObject::from_ptr(alloc_dict_with_pairs(py, &[left, zero, right, two])).bits();
            let empty = MoltObject::from_ptr(alloc_dict_with_pairs(py, &[])).bits();
            let mixed = MoltObject::from_ptr(alloc_dict_with_pairs(
                py,
                &[left, zero, one, one, right, two],
            ))
            .bits();
            let item = MoltObject::from_ptr(alloc_string(py, b", ")).bits();
            let key = MoltObject::from_ptr(alloc_string(py, b": ")).bits();
            let dashes = MoltObject::from_ptr(alloc_string(py, b"--")).bits();
            // CPython3.12.15's actual dict-item encoder distinguishes empty
            // input from nonempty input whose keys were all skipped.
            for sort in [false, true] {
                for (indent, expected_skipped) in [
                    (MoltObject::none().bits(), "{}"),
                    (zero, "{\n\n}"),
                    (two, "{\n  \n}"),
                    (dashes, "{\n--\n}"),
                ] {
                    for (dict, expected) in [(skipped, expected_skipped), (empty, "{}")] {
                        let result = molt_json_dumps_ex(
                            dict,
                            MoltObject::from_bool(true).bits(),
                            MoltObject::from_bool(true).bits(),
                            MoltObject::from_bool(true).bits(),
                            MoltObject::from_bool(true).bits(),
                            MoltObject::from_bool(sort).bits(),
                            indent,
                            item,
                            key,
                            MoltObject::none().bits(),
                        );
                        assert!(!exception_pending(py));
                        assert_eq!(
                            string_obj_to_owned(obj_from_bits(result)).as_deref(),
                            Some(expected)
                        );
                        dec_ref_bits(py, result);
                    }
                }
            }
            let result = molt_json_dumps_ex(
                mixed,
                MoltObject::from_bool(true).bits(),
                MoltObject::from_bool(true).bits(),
                MoltObject::from_bool(true).bits(),
                MoltObject::from_bool(true).bits(),
                MoltObject::from_bool(false).bits(),
                two,
                item,
                key,
                MoltObject::none().bits(),
            );
            assert!(!exception_pending(py));
            assert_eq!(
                string_obj_to_owned(obj_from_bits(result)).as_deref(),
                Some("{\n  \"1\": 1\n}")
            );
            dec_ref_bits(py, result);
            for bits in [dashes, key, item, mixed, empty, skipped, right, left] {
                dec_ref_bits(py, bits);
            }
        });
    }
}
