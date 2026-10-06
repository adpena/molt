use super::*;
use crate::builtins::functions::{alloc_runtime_function_obj, runtime_fn_addr};
use std::sync::atomic::{AtomicU64, Ordering};

static CALLBACK_DICT: AtomicU64 = AtomicU64::new(0);
static CALLBACK_KEY: AtomicU64 = AtomicU64::new(0);
static CALLBACK_MODE: AtomicU64 = AtomicU64::new(0);
static CALLBACK_CALLS: AtomicU64 = AtomicU64::new(0);

extern "C" fn mutate_dictionary_during_add(self_bits: u64, _other: u64) -> u64 {
    crate::with_gil_entry_nopanic!(py, {
        CALLBACK_CALLS.fetch_add(1, Ordering::SeqCst);
        let dictionary = CALLBACK_DICT.load(Ordering::SeqCst);
        let key = CALLBACK_KEY.load(Ordering::SeqCst);
        let ptr = obj_from_bits(dictionary).as_ptr().unwrap();
        unsafe {
            dict_clear_in_place(py, ptr);
            for index in 0..64 {
                dict_set_in_place(
                    py,
                    ptr,
                    MoltObject::from_int(index).bits(),
                    MoltObject::from_int(index + 100).bits(),
                );
            }
            dict_set_in_place(py, ptr, key, MoltObject::from_int(40).bits());
            // The selected arithmetic receiver must remain alive even when
            // clearing the dictionary releases its original last owner.
            assert_ne!(
                object_class_bits(obj_from_bits(self_bits).as_ptr().unwrap()),
                0
            );
        }
        match CALLBACK_MODE.load(Ordering::SeqCst) {
            1 => MoltObject::none().bits(),
            2 => raise_exception::<u64>(py, "RuntimeError", "increment callback failure"),
            _ => MoltObject::from_int(42).bits(),
        }
    })
}

fn arithmetic_value(py: &PyToken<'_>, method_name: &[u8]) -> (u64, u64, u64) {
    let name = attr_name_bits_from_bytes(py, b"ReentrantIncrementValue").unwrap();
    let class = crate::molt_class_new(name);
    crate::molt_class_set_base(class, builtin_classes(py).object);
    let method = attr_name_bits_from_bytes(py, method_name).unwrap();
    let function = alloc_runtime_function_obj(
        py,
        runtime_fn_addr(
            "mutate_dictionary_during_add",
            mutate_dictionary_during_add as *const (),
        ),
        2,
    );
    assert!(!function.is_null());
    let function = MoltObject::from_ptr(function).bits();
    crate::molt_set_attr_name(class, method, function);
    let class_ptr = obj_from_bits(class).as_ptr().unwrap();
    unsafe {
        crate::object::class_finish_definition(py, class_ptr).unwrap();
    }
    let size = unsafe { crate::object::layout::class_cached_layout_size(class_ptr).unwrap() };
    let value = crate::object::builders::alloc_class_instance(py, size, class);
    unsafe {
        crate::object::gc::gc_publish_initialized(py, obj_from_bits(value).as_ptr().unwrap());
    }
    dec_ref_bits(py, method);
    dec_ref_bits(py, name);
    assert!(!exception_pending(py));
    (value, class, function)
}

/// The in-place increment every fused lane commits through. Called directly it
/// runs the value's `+` exactly as the statement does, callbacks included.
unsafe fn in_place_increment(py: &PyToken<'_>, dictionary: u64, key: u64, delta: u64) -> bool {
    unsafe { dict_inc_in_place(py, obj_from_bits(dictionary).as_ptr().unwrap(), key, delta) }
}

/// The fused statement: `Some(done)`, or `None` with the exception pending.
fn exact_statement(py: &PyToken<'_>, dictionary: u64, key: u64, delta: u64) -> Option<bool> {
    unsafe { dict_increment_exact_statement(py, dictionary, key, delta) }.ok()
}

/// A fused split/count loop over `line`, whitespace words or `sep` words, with
/// an unbound loop target. `(last, ok)`, owned.
fn split_increment(
    py: &PyToken<'_>,
    line: &str,
    sep: Option<&str>,
    dictionary: u64,
    delta: u64,
    target: u64,
) -> (u64, bool) {
    let line = MoltObject::from_ptr(alloc_string(py, line.as_bytes())).bits();
    let result = match sep {
        Some(sep) => {
            let sep = MoltObject::from_ptr(alloc_string(py, sep.as_bytes())).bits();
            let result = molt_string_split_sep_dict_inc(line, sep, dictionary, delta, target);
            dec_ref_bits(py, sep);
            result
        }
        None => molt_string_split_ws_dict_inc(line, dictionary, delta, target),
    };
    dec_ref_bits(py, line);
    assert!(!exception_pending(py), "a fused split never raises");
    let (last, ok) = unsafe {
        crate::object::seq_access::with_immutable_tuple_slice(
            obj_from_bits(result).as_ptr().unwrap(),
            |parts| (parts[0], parts[1]),
        )
        .unwrap()
    };
    inc_ref_bits(py, last);
    dec_ref_bits(py, result);
    (last, ok == MoltObject::from_bool(true).bits())
}

fn missing_target(py: &PyToken<'_>) -> u64 {
    missing_bits(py)
}

fn int_value(py: &PyToken<'_>, dictionary: u64, word: &str) -> Option<i64> {
    let key = MoltObject::from_ptr(alloc_string(py, word.as_bytes())).bits();
    let value = unsafe { dict_get_in_place(py, obj_from_bits(dictionary).as_ptr().unwrap(), key) };
    dec_ref_bits(py, key);
    value.and_then(|bits| to_i64(obj_from_bits(bits)))
}

#[test]
fn in_place_increment_arithmetic_reentry_reacquires_mapping() {
    let _transaction = crate::test_support::RuntimeTestTransaction::new();
    crate::with_gil_entry_nopanic!(py, {
        {
            for mode in 0..3 {
                let key = MoltObject::from_ptr(alloc_string(py, b"item")).bits();
                let (value, class, function) = arithmetic_value(py, b"__add__");
                let dictionary =
                    MoltObject::from_ptr(alloc_dict_with_pairs(py, &[key, value])).bits();
                dec_ref_bits(py, value);
                CALLBACK_DICT.store(dictionary, Ordering::SeqCst);
                CALLBACK_KEY.store(key, Ordering::SeqCst);
                CALLBACK_MODE.store(mode, Ordering::SeqCst);
                CALLBACK_CALLS.store(0, Ordering::SeqCst);
                let success = unsafe {
                    in_place_increment(py, dictionary, key, MoltObject::from_int(1).bits())
                };
                assert_eq!(success, mode != 2);
                assert_eq!(exception_pending(py), mode == 2);
                assert_eq!(CALLBACK_CALLS.load(Ordering::SeqCst), 1);
                if mode == 2 {
                    crate::molt_exception_clear();
                }
                let expected = match mode {
                    1 => MoltObject::none().bits(),
                    2 => MoltObject::from_int(40).bits(),
                    _ => MoltObject::from_int(42).bits(),
                };
                let ptr = obj_from_bits(dictionary).as_ptr().unwrap();
                assert_eq!(unsafe { dict_get_in_place(py, ptr, key) }, Some(expected));
                assert_eq!(
                    unsafe { dict_order(ptr).len() },
                    130,
                    "no duplicate stale-index entry"
                );
                CALLBACK_DICT.store(0, Ordering::SeqCst);
                CALLBACK_KEY.store(0, Ordering::SeqCst);
                for bits in [dictionary, key, class, function] {
                    dec_ref_bits(py, bits);
                }
                assert!(!exception_pending(py));
            }
        }
    });
}

#[test]
fn in_place_increment_missing_key_reverse_addition_rechecks_after_callback_insertion() {
    let _transaction = crate::test_support::RuntimeTestTransaction::new();
    crate::with_gil_entry_nopanic!(py, {
        {
            let key = MoltObject::from_ptr(alloc_string(py, b"item")).bits();
            let (delta, class, function) = arithmetic_value(py, b"__radd__");
            let dictionary = MoltObject::from_ptr(alloc_dict_with_pairs(py, &[])).bits();
            CALLBACK_DICT.store(dictionary, Ordering::SeqCst);
            CALLBACK_KEY.store(key, Ordering::SeqCst);
            CALLBACK_MODE.store(0, Ordering::SeqCst);
            CALLBACK_CALLS.store(0, Ordering::SeqCst);
            assert!(unsafe { in_place_increment(py, dictionary, key, delta) });
            let ptr = obj_from_bits(dictionary).as_ptr().unwrap();
            assert_eq!(CALLBACK_CALLS.load(Ordering::SeqCst), 1);
            assert_eq!(
                unsafe { dict_get_in_place(py, ptr, key) },
                Some(MoltObject::from_int(42).bits())
            );
            assert_eq!(unsafe { dict_order(ptr).len() }, 130);
            CALLBACK_DICT.store(0, Ordering::SeqCst);
            CALLBACK_KEY.store(0, Ordering::SeqCst);
            for bits in [dictionary, key, delta, class, function] {
                dec_ref_bits(py, bits);
            }
            assert!(!exception_pending(py));
        }
    });
}

#[test]
fn exact_statement_declines_before_any_callback_or_mutation() {
    let _transaction = crate::test_support::RuntimeTestTransaction::new();
    crate::with_gil_entry_nopanic!(py, {
        let key = MoltObject::from_ptr(alloc_string(py, b"item")).bits();
        let (value, value_class, value_function) = arithmetic_value(py, b"__add__");
        let dictionary = MoltObject::from_ptr(alloc_dict_with_pairs(py, &[key, value])).bits();
        dec_ref_bits(py, value);
        let one = MoltObject::from_int(1).bits();
        CALLBACK_CALLS.store(0, Ordering::SeqCst);
        // A value whose `+` is Python code: the statement runs itself.
        assert_eq!(exact_statement(py, dictionary, key, one), Some(false));
        // A delta whose `+` is Python code, on a missing key.
        let other = MoltObject::from_ptr(alloc_string(py, b"other")).bits();
        let (delta, delta_class, delta_function) = arithmetic_value(py, b"__radd__");
        assert_eq!(exact_statement(py, dictionary, other, delta), Some(false));
        // A key whose hash or equality could be Python code.
        assert_eq!(exact_statement(py, dictionary, one, one), Some(false));
        // Anything but an exact dict dispatches its own `get` and `__setitem__`.
        let list = MoltObject::from_ptr(alloc_list(py, &[])).bits();
        assert_eq!(exact_statement(py, list, other, one), Some(false));
        assert_eq!(CALLBACK_CALLS.load(Ordering::SeqCst), 0);
        let ptr = obj_from_bits(dictionary).as_ptr().unwrap();
        assert_eq!(unsafe { dict_order(ptr).len() }, 2, "nothing was inserted");
        // Exact ints: the statement's value, the statement's key object.
        assert_eq!(exact_statement(py, dictionary, other, one), Some(true));
        assert_eq!(
            exact_statement(py, dictionary, other, MoltObject::from_bool(true).bits()),
            Some(true)
        );
        assert_eq!(int_value(py, dictionary, "other"), Some(2));
        assert_eq!(unsafe { dict_order(ptr)[2] }, other);
        assert!(!exception_pending(py));
        for bits in [
            dictionary,
            key,
            other,
            delta,
            list,
            value_class,
            value_function,
            delta_class,
            delta_function,
        ] {
            dec_ref_bits(py, bits);
        }
    });
}

#[test]
fn split_increment_declines_before_any_callback_or_mutation() {
    let _transaction = crate::test_support::RuntimeTestTransaction::new();
    crate::with_gil_entry_nopanic!(py, {
        for sep in [None, Some(",")] {
            // A value whose `+` is Python code, found by the second word: the
            // loop must run itself, so the first word stays unincremented too.
            let first = MoltObject::from_ptr(alloc_string(py, b"first")).bits();
            let key = MoltObject::from_ptr(alloc_string(py, b"item")).bits();
            let (value, value_class, value_function) = arithmetic_value(py, b"__add__");
            let dictionary = MoltObject::from_ptr(alloc_dict_with_pairs(
                py,
                &[first, MoltObject::from_int(2).bits(), key, value],
            ))
            .bits();
            dec_ref_bits(py, value);
            CALLBACK_CALLS.store(0, Ordering::SeqCst);
            let line = if sep.is_some() {
                "first,item"
            } else {
                "first item"
            };
            let (last, ok) = split_increment(
                py,
                line,
                sep,
                dictionary,
                MoltObject::from_int(1).bits(),
                missing_target(py),
            );
            assert!(!ok);
            assert_eq!(last, MoltObject::none().bits());
            assert_eq!(CALLBACK_CALLS.load(Ordering::SeqCst), 0);
            assert_eq!(int_value(py, dictionary, "first"), Some(2));
            // A delta whose `+` is Python code declines likewise.
            let (delta, delta_class, delta_function) = arithmetic_value(py, b"__radd__");
            let (_, ok) = split_increment(py, "first", sep, dictionary, delta, missing_target(py));
            assert!(!ok);
            assert_eq!(CALLBACK_CALLS.load(Ordering::SeqCst), 0);
            assert_eq!(int_value(py, dictionary, "first"), Some(2));
            for bits in [
                dictionary,
                first,
                key,
                delta,
                value_class,
                value_function,
                delta_class,
                delta_function,
            ] {
                dec_ref_bits(py, bits);
            }
            assert!(!exception_pending(py));
        }
    });
}

#[test]
fn split_increment_declines_inputs_the_loop_would_dispatch_or_reject() {
    let _transaction = crate::test_support::RuntimeTestTransaction::new();
    crate::with_gil_entry_nopanic!(py, {
        let dictionary = MoltObject::from_ptr(alloc_dict_with_pairs(py, &[])).bits();
        let one = MoltObject::from_int(1).bits();
        // An empty separator raises in str.split; the loop reports it itself.
        assert!(!split_increment(py, "a,b", Some(""), dictionary, one, missing_target(py)).1);
        // An empty whitespace split runs zero iterations: the loop does that.
        assert!(!split_increment(py, " \t ", None, dictionary, one, missing_target(py)).1);
        // A non-str line dispatches `split` on its own type.
        let number = MoltObject::from_int(5).bits();
        let pair = molt_string_split_ws_dict_inc(number, dictionary, one, missing_target(py));
        let ok = unsafe {
            crate::object::seq_access::with_immutable_tuple_slice(
                obj_from_bits(pair).as_ptr().unwrap(),
                |parts| parts[1],
            )
            .unwrap()
        };
        assert_eq!(ok, MoltObject::from_bool(false).bits());
        dec_ref_bits(py, pair);
        // A loop target whose previous value could run a finalizer when rebound.
        let list = MoltObject::from_ptr(alloc_list(py, &[])).bits();
        assert!(!split_increment(py, "a", None, dictionary, one, list).1);
        dec_ref_bits(py, list);
        assert_eq!(
            unsafe { dict_order(obj_from_bits(dictionary).as_ptr().unwrap()).len() },
            0,
            "every decline leaves the dict unchanged"
        );
        dec_ref_bits(py, dictionary);
        assert!(!exception_pending(py));
    });
}

#[test]
fn split_increment_reads_words_through_str_split() {
    let _transaction = crate::test_support::RuntimeTestTransaction::new();
    crate::with_gil_entry_nopanic!(py, {
        let dictionary = MoltObject::from_ptr(alloc_dict_with_pairs(py, &[])).bits();
        let two = MoltObject::from_int(2).bits();
        // U+001C..U+001F and U+2003 are str whitespace; U+200B is not.
        let (last, ok) = split_increment(
            py,
            "a\u{1c}b\u{1f}a\u{2003}c\u{200b}d",
            None,
            dictionary,
            two,
            missing_target(py),
        );
        assert!(ok);
        assert_eq!(
            string_obj_to_owned(obj_from_bits(last)).as_deref(),
            Some("c\u{200b}d")
        );
        assert_eq!(int_value(py, dictionary, "a"), Some(4));
        assert_eq!(int_value(py, dictionary, "b"), Some(2));
        assert_eq!(int_value(py, dictionary, "c\u{200b}d"), Some(2));
        dec_ref_bits(py, last);
        // A separator split keeps empty words and multi-byte separators.
        let (last, ok) = split_increment(
            py,
            "a::::b::",
            Some("::"),
            dictionary,
            two,
            missing_target(py),
        );
        assert!(ok);
        assert_eq!(
            string_obj_to_owned(obj_from_bits(last)).as_deref(),
            Some("")
        );
        assert_eq!(int_value(py, dictionary, ""), Some(4));
        assert_eq!(int_value(py, dictionary, "a"), Some(6));
        dec_ref_bits(py, last);
        dec_ref_bits(py, dictionary);
        assert!(!exception_pending(py));
    });
}

#[test]
fn split_increment_binds_the_inserted_key_only_when_its_own_iteration_inserted_it() {
    let _transaction = crate::test_support::RuntimeTestTransaction::new();
    crate::with_gil_entry_nopanic!(py, {
        let dictionary = MoltObject::from_ptr(alloc_dict_with_pairs(py, &[])).bits();
        let one = MoltObject::from_int(1).bits();
        // Words long enough that no string cache can share their objects.
        let (last, ok) = split_increment(
            py,
            "first-word second-word",
            None,
            dictionary,
            one,
            missing_target(py),
        );
        assert!(ok);
        let ptr = obj_from_bits(dictionary).as_ptr().unwrap();
        let inserted = unsafe { dict_order(ptr)[2] };
        assert_eq!(last, inserted, "the loop target is the new key object");
        dec_ref_bits(py, last);
        let (last, ok) =
            split_increment(py, "second-word", None, dictionary, one, missing_target(py));
        assert!(ok);
        assert_ne!(last, inserted, "an existing key keeps its own object");
        assert_eq!(
            string_obj_to_owned(obj_from_bits(last)).as_deref(),
            Some("second-word")
        );
        assert_eq!(int_value(py, dictionary, "second-word"), Some(2));
        dec_ref_bits(py, last);
        dec_ref_bits(py, dictionary);
        assert!(!exception_pending(py));
    });
}

#[test]
fn increment_scalar_result_respects_inline_carrier_boundary() {
    let _transaction = crate::test_support::RuntimeTestTransaction::new();
    crate::with_gil_entry_nopanic!(py, {
        for lane in 0..5 {
            for (current, delta) in [((1i64 << 46) - 1, 1), (-(1i64 << 46), -1)] {
                let key = MoltObject::from_ptr(alloc_string(py, b"item")).bits();
                let dictionary = MoltObject::from_ptr(alloc_dict_with_pairs(
                    py,
                    &[key, MoltObject::from_int(current).bits()],
                ))
                .bits();
                let delta_bits = MoltObject::from_int(delta).bits();
                match lane {
                    0 => assert!(unsafe { in_place_increment(py, dictionary, key, delta_bits) }),
                    1 => assert_eq!(exact_statement(py, dictionary, key, delta_bits), Some(true)),
                    2 => assert_eq!(
                        unsafe {
                            dict_increment_validated_word(
                                py,
                                obj_from_bits(dictionary).as_ptr().unwrap(),
                                b"item",
                                delta_bits,
                            )
                        },
                        Some(None)
                    ),
                    _ => {
                        let sep = (lane == 4).then_some(",");
                        let (last, ok) = split_increment(
                            py,
                            "item",
                            sep,
                            dictionary,
                            delta_bits,
                            missing_target(py),
                        );
                        assert!(ok);
                        dec_ref_bits(py, last);
                    }
                }
                let result = unsafe {
                    dict_get_in_place(py, obj_from_bits(dictionary).as_ptr().unwrap(), key).unwrap()
                };
                assert_eq!(to_i64(obj_from_bits(result)), Some(current + delta));
                assert!(
                    obj_from_bits(result).as_ptr().is_some(),
                    "wide result owns heap storage"
                );
                for bits in [dictionary, key] {
                    dec_ref_bits(py, bits);
                }
                assert!(!exception_pending(py));
            }
        }
    });
}

#[test]
fn increment_lanes_never_mutate_frozen_layout_maps() {
    let _transaction = crate::test_support::RuntimeTestTransaction::new();
    crate::with_gil_entry_nopanic!(py, {
        for lane in 0..4 {
            let key = MoltObject::from_ptr(alloc_string(py, b"item")).bits();
            let value = MoltObject::from_int(4).bits();
            let dictionary = MoltObject::from_ptr(alloc_dict_with_pairs(py, &[key, value])).bits();
            let ptr = obj_from_bits(dictionary).as_ptr().unwrap();
            unsafe {
                (*header_from_obj_ptr(ptr))
                    .fetch_or_flags(crate::object::HEADER_FLAG_FROZEN_LAYOUT_MAP);
            }
            let one = MoltObject::from_int(1).bits();
            if lane == 0 {
                assert!(!unsafe { in_place_increment(py, dictionary, key, one) });
                assert!(exception_pending(py));
                crate::molt_exception_clear();
            } else if lane == 1 {
                // The fused statement declines; the statement's own store raises.
                assert_eq!(exact_statement(py, dictionary, key, one), Some(false));
            } else {
                // The fused loop declines; the loop's own store then raises.
                let sep = (lane == 3).then_some(",");
                let (_, ok) = split_increment(py, "item", sep, dictionary, one, missing_target(py));
                assert!(!ok);
            }
            assert_eq!(unsafe { dict_get_in_place(py, ptr, key) }, Some(value));
            for bits in [dictionary, key] {
                dec_ref_bits(py, bits);
            }
        }
    });
}
