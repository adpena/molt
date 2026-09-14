use super::*;
use crate::builtins::functions::{alloc_runtime_function_obj, runtime_fn_addr};
use std::sync::atomic::{AtomicU64, Ordering};

static CALLBACK_DICT: AtomicU64 = AtomicU64::new(0);
static CALLBACK_KEY: AtomicU64 = AtomicU64::new(0);
static CALLBACK_MODE: AtomicU64 = AtomicU64::new(0);
static CALLBACK_CALLS: AtomicU64 = AtomicU64::new(0);
static CALLBACK_INPUT_OWNERS: [AtomicU64; 4] = [const { AtomicU64::new(0) }; 4];

extern "C" fn mutate_dictionary_during_add(self_bits: u64, _other: u64) -> u64 {
    crate::with_gil_entry_nopanic!(py, {
        CALLBACK_CALLS.fetch_add(1, Ordering::SeqCst);
        if CALLBACK_MODE.load(Ordering::SeqCst) == 3 {
            // Release the original owners on the first token. Subsequent
            // callbacks must be covered by the outer scan, not its caller or
            // the now-completed first arithmetic invocation.
            for owner in &CALLBACK_INPUT_OWNERS {
                dec_ref_bits(py, owner.swap(0, Ordering::SeqCst));
            }
            return MoltObject::from_int(42).bits();
        }
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

unsafe fn increment(py: &PyToken<'_>, lane: u8, dictionary: u64, key: u64, delta: u64) -> bool {
    unsafe {
        let ptr = obj_from_bits(dictionary).as_ptr().unwrap();
        match lane {
            0 => dict_inc_in_place(py, ptr, key, delta),
            1 => dict_inc_prehashed_string_key_in_place(py, ptr, key, delta)
                .unwrap_or_else(|| dict_inc_in_place(py, ptr, key, delta)),
            2 => {
                let mut last = SplitDictIncrementLast::new(py);
                let result = dict_inc_with_string_token(py, ptr, b"item", delta, &mut last);
                if result {
                    assert_eq!(
                        string_obj_to_owned(obj_from_bits(last.bits.unwrap())).as_deref(),
                        Some("item")
                    );
                }
                result
            }
            3 | 4 => {
                let line = MoltObject::from_ptr(alloc_string(py, b"item")).bits();
                let result = if lane == 3 {
                    molt_string_split_ws_dict_inc(line, dictionary, delta)
                } else {
                    let separator = MoltObject::from_ptr(alloc_string(py, b",")).bits();
                    let result = molt_string_split_sep_dict_inc(line, separator, dictionary, delta);
                    dec_ref_bits(py, separator);
                    result
                };
                dec_ref_bits(py, result);
                dec_ref_bits(py, line);
                !exception_pending(py)
            }
            _ => unreachable!(),
        }
    }
}

#[test]
fn increment_arithmetic_reentry_reacquires_mapping_across_all_lanes() {
    let _transaction = crate::test_support::RuntimeTestTransaction::new();
    crate::with_gil_entry_nopanic!(py, {
        for lane in 0..5 {
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
                let success =
                    unsafe { increment(py, lane, dictionary, key, MoltObject::from_int(1).bits()) };
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
fn increment_missing_key_reverse_addition_rechecks_after_callback_insertion() {
    let _transaction = crate::test_support::RuntimeTestTransaction::new();
    crate::with_gil_entry_nopanic!(py, {
        for lane in 0..5 {
            let key = MoltObject::from_ptr(alloc_string(py, b"item")).bits();
            let (delta, class, function) = arithmetic_value(py, b"__radd__");
            let dictionary = MoltObject::from_ptr(alloc_dict_with_pairs(py, &[])).bits();
            CALLBACK_DICT.store(dictionary, Ordering::SeqCst);
            CALLBACK_KEY.store(key, Ordering::SeqCst);
            CALLBACK_MODE.store(0, Ordering::SeqCst);
            CALLBACK_CALLS.store(0, Ordering::SeqCst);
            assert!(unsafe { increment(py, lane, dictionary, key, delta) });
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
                assert!(unsafe {
                    increment(
                        py,
                        lane,
                        dictionary,
                        key,
                        MoltObject::from_int(delta).bits(),
                    )
                });
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
fn increment_fast_paths_cannot_mutate_frozen_layout_maps() {
    let _transaction = crate::test_support::RuntimeTestTransaction::new();
    crate::with_gil_entry_nopanic!(py, {
        for lane in 0..5 {
            let key = MoltObject::from_ptr(alloc_string(py, b"item")).bits();
            let value = MoltObject::from_int(4).bits();
            let dictionary = MoltObject::from_ptr(alloc_dict_with_pairs(py, &[key, value])).bits();
            let ptr = obj_from_bits(dictionary).as_ptr().unwrap();
            unsafe {
                (*header_from_obj_ptr(ptr))
                    .fetch_or_flags(crate::object::HEADER_FLAG_FROZEN_LAYOUT_MAP);
                assert!(!increment(
                    py,
                    lane,
                    dictionary,
                    key,
                    MoltObject::from_int(1).bits()
                ));
            }
            assert!(exception_pending(py));
            crate::molt_exception_clear();
            assert_eq!(unsafe { dict_get_in_place(py, ptr, key) }, Some(value));
            for bits in [dictionary, key] {
                dec_ref_bits(py, bits);
            }
        }
    });
}

#[test]
fn increment_split_failure_releases_previous_token_owner() {
    let _transaction = crate::test_support::RuntimeTestTransaction::new();
    crate::with_gil_entry_nopanic!(py, {
        for separator in [None, Some(b",".as_slice())] {
            // The exact-string fast probe retains this existing dictionary key.
            // Keep it mortal so a leaked last-token owner is observable.
            let first =
                MoltObject::from_ptr(crate::object::builders::alloc_string_nointern(py, b"first"))
                    .bits();
            let key = MoltObject::from_ptr(alloc_string(py, b"item")).bits();
            let (value, class, function) = arithmetic_value(py, b"__add__");
            let dictionary = MoltObject::from_ptr(alloc_dict_with_pairs(
                py,
                &[first, MoltObject::from_int(2).bits(), key, value],
            ))
            .bits();
            dec_ref_bits(py, value);
            CALLBACK_DICT.store(dictionary, Ordering::SeqCst);
            CALLBACK_KEY.store(key, Ordering::SeqCst);
            CALLBACK_MODE.store(2, Ordering::SeqCst);
            let line = MoltObject::from_ptr(alloc_string(
                py,
                if separator.is_some() {
                    b"first,item"
                } else {
                    b"first item"
                },
            ))
            .bits();
            let result = if let Some(separator) = separator {
                let separator = MoltObject::from_ptr(alloc_string(py, separator)).bits();
                let result = molt_string_split_sep_dict_inc(
                    line,
                    separator,
                    dictionary,
                    MoltObject::from_int(1).bits(),
                );
                dec_ref_bits(py, separator);
                result
            } else {
                molt_string_split_ws_dict_inc(line, dictionary, MoltObject::from_int(1).bits())
            };
            assert!(exception_pending(py));
            crate::molt_exception_clear();
            assert_eq!(
                unsafe {
                    (*header_from_obj_ptr(obj_from_bits(first).as_ptr().unwrap()))
                        .ref_count_snapshot()
                },
                1,
                "no leaked last-token owner after callback failure"
            );
            CALLBACK_DICT.store(0, Ordering::SeqCst);
            CALLBACK_KEY.store(0, Ordering::SeqCst);
            for bits in [result, line, dictionary, first, key, class, function] {
                dec_ref_bits(py, bits);
            }
            assert!(!exception_pending(py));
        }
    });
}

#[test]
fn increment_split_retains_all_inputs_across_successive_callbacks() {
    let _transaction = crate::test_support::RuntimeTestTransaction::new();
    crate::with_gil_entry_nopanic!(py, {
        for (line_text, separator_text) in [
            ("first next", None),
            ("first\u{2003}next", None),
            ("first,next", Some(",")),
            ("first::next", Some("::")),
        ] {
            let (delta, class, function) = arithmetic_value(py, b"__radd__");
            let line = MoltObject::from_ptr(alloc_string(py, line_text.as_bytes())).bits();
            let separator = separator_text.map_or(MoltObject::none().bits(), |text| {
                MoltObject::from_ptr(alloc_string(py, text.as_bytes())).bits()
            });
            let dictionary = MoltObject::from_ptr(alloc_dict_with_pairs(py, &[])).bits();
            for (slot, bits) in CALLBACK_INPUT_OWNERS
                .iter()
                .zip([line, separator, dictionary, delta])
            {
                assert_eq!(slot.swap(bits, Ordering::SeqCst), 0);
            }
            CALLBACK_MODE.store(3, Ordering::SeqCst);
            CALLBACK_CALLS.store(0, Ordering::SeqCst);
            let result = if separator_text.is_some() {
                molt_string_split_sep_dict_inc(line, separator, dictionary, delta)
            } else {
                molt_string_split_ws_dict_inc(line, dictionary, delta)
            };
            assert!(!exception_pending(py));
            assert_eq!(CALLBACK_CALLS.load(Ordering::SeqCst), 2);
            assert!(
                CALLBACK_INPUT_OWNERS
                    .iter()
                    .all(|slot| slot.load(Ordering::SeqCst) == 0)
            );
            let (token, had_any) = unsafe {
                crate::object::seq_access::with_immutable_tuple_slice(
                    obj_from_bits(result).as_ptr().unwrap(),
                    |parts| (parts[0], parts[1]),
                )
                .unwrap()
            };
            assert_eq!(
                string_obj_to_owned(obj_from_bits(token)).as_deref(),
                Some("next")
            );
            assert_eq!(had_any, MoltObject::from_bool(true).bits());
            // The callback consumed every initial input owner. Only the
            // returned token and the test's class/function owners remain here.
            for bits in [result, class, function] {
                dec_ref_bits(py, bits);
            }
            CALLBACK_MODE.store(0, Ordering::SeqCst);
            assert!(!exception_pending(py));
        }
    });
}
