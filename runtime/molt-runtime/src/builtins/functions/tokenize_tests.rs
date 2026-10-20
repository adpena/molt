use super::*;
use crate::builtins::exceptions::{ExceptionValue, molt_exception_last_pending};
use crate::object::seq_access;
use crate::resource::{LimitedTracker, ResourceLimits, UnlimitedTracker, set_tracker};
use crate::test_support::RuntimeTestTransaction;

fn text(py: &PyToken<'_>, value: &str) -> u64 {
    let ptr = alloc_string(py, value.as_bytes());
    assert!(!ptr.is_null());
    MoltObject::from_ptr(ptr).bits()
}

fn refs(bits: u64) -> u32 {
    unsafe {
        (*crate::object::header_from_obj_ptr(obj_from_bits(bits).as_ptr().unwrap()))
            .ref_count_snapshot()
    }
}

fn lookup(py: &PyToken<'_>, dictionary: u64, name: &str) -> Option<u64> {
    let key = ExceptionValue::adopt(py, text(py, name));
    unsafe {
        dict_get_in_place(py, obj_from_bits(dictionary).as_ptr().unwrap(), key.bits())
            .expect("real dictionary lookup")
    }
}

fn number(py: &PyToken<'_>, dictionary: u64, name: &str) -> i64 {
    to_i64(obj_from_bits(lookup(py, dictionary, name).expect(name))).expect("integer constant")
}

fn strings(py: &PyToken<'_>, list: u64) -> Vec<String> {
    let values = unsafe {
        seq_access::snapshot(
            py,
            obj_from_bits(list).as_ptr().unwrap(),
            "token test snapshot",
        )
    }
    .unwrap();
    values
        .iter()
        .map(|&bits| string_obj_to_owned(obj_from_bits(bits)).expect("export name"))
        .collect()
}

#[test]
fn token_payload_objects_match_independent_cpython_minor_controls() {
    let transaction = RuntimeTestTransaction::new();
    crate::with_gil_entry_nopanic!(py, {
        // Literal public ABI controls from the pinned CPython sources. Do not
        // derive the oracle from TokenConstant or the generated runtime table.
        for (minor, comment, nl, encoding, count) in [
            (12, 64, 65, 67, 68),
            (13, 62, 63, 65, 66),
            (14, 65, 66, 68, 69),
        ] {
            transaction.with_target_python_minor(py, minor, || {
                let payload = ExceptionValue::adopt(py, crate::molt_token_payload());
                assert!(!exception_pending(py));
                let payload = payload.bits();
                for (name, expected) in [
                    ("ENDMARKER", 0),
                    ("NAME", 1),
                    ("NUMBER", 2),
                    ("NEWLINE", 4),
                    ("EXCLAMATION", 54),
                    ("OP", 55),
                    ("COMMENT", comment),
                    ("NL", nl),
                    ("ENCODING", encoding),
                    ("N_TOKENS", count),
                    ("NT_OFFSET", 256),
                ] {
                    assert_eq!(number(py, payload, name), expected, "{minor}: {name}");
                }
                for (name, expected) in [("AWAIT", 56), ("ASYNC", 57)] {
                    assert_eq!(
                        lookup(py, payload, name).and_then(|bits| to_i64(obj_from_bits(bits))),
                        (minor == 12).then_some(expected)
                    );
                }
                for (name, expected) in [
                    ("TSTRING_START", 62),
                    ("TSTRING_MIDDLE", 63),
                    ("TSTRING_END", 64),
                ] {
                    assert_eq!(
                        lookup(py, payload, name).and_then(|bits| to_i64(obj_from_bits(bits))),
                        (minor == 14).then_some(expected)
                    );
                }
                let names = lookup(py, payload, "tok_name").unwrap();
                let exact = lookup(py, payload, "EXACT_TOKEN_TYPES").unwrap();
                for (symbol, name, expected) in [
                    ("!", "EXCLAMATION", 54),
                    ("=", "EQUAL", 22),
                    ("!=", "NOTEQUAL", 28),
                    ("**=", "DOUBLESTAREQUAL", 46),
                    (":=", "COLONEQUAL", 53),
                    ("...", "ELLIPSIS", 52),
                ] {
                    assert_eq!(number(py, exact, symbol), expected);
                    assert_eq!(number(py, payload, name), expected);
                    let observed = unsafe {
                        dict_get_in_place(
                            py,
                            obj_from_bits(names).as_ptr().unwrap(),
                            MoltObject::from_int(expected).bits(),
                        )
                        .unwrap()
                        .unwrap()
                    };
                    assert_eq!(
                        string_obj_to_owned(obj_from_bits(observed)).as_deref(),
                        Some(name)
                    );
                }
                let exports = strings(py, lookup(py, payload, "__all__").unwrap());
                assert_eq!(
                    &exports[..5],
                    [
                        "tok_name",
                        "ISTERMINAL",
                        "ISNONTERMINAL",
                        "ISEOF",
                        "EXACT_TOKEN_TYPES"
                    ]
                );
                assert_eq!(exports.len(), count as usize + 7);
                assert_eq!(
                    exports
                        .iter()
                        .collect::<std::collections::HashSet<_>>()
                        .len(),
                    exports.len()
                );
                assert_eq!(&exports[exports.len() - 2..], ["N_TOKENS", "NT_OFFSET"]);
                for name in &exports[5..] {
                    let value = number(py, payload, name);
                    let observed = unsafe {
                        dict_get_in_place(
                            py,
                            obj_from_bits(names).as_ptr().unwrap(),
                            MoltObject::from_int(value).bits(),
                        )
                        .unwrap()
                        .unwrap()
                    };
                    assert_eq!(
                        string_obj_to_owned(obj_from_bits(observed)).as_deref(),
                        Some(name.as_str())
                    );
                }
            });
        }
    });
}

#[test]
fn scanner_object_ids_follow_selected_target_for_supported_simple_tokens() {
    let transaction = RuntimeTestTransaction::new();
    crate::with_gil_entry_nopanic!(py, {
        let source = ExceptionValue::adopt(py, text(py, "x = 1\n# note\n"));
        for (minor, comment, nl) in [(12, 64, 65), (13, 62, 63), (14, 65, 66)] {
            transaction.with_target_python_minor(py, minor, || {
                let result = ExceptionValue::adopt(py, molt_tokenize_scan(source.bits()));
                assert!(!exception_pending(py));
                let rows = unsafe {
                    seq_access::snapshot(
                        py,
                        obj_from_bits(result.bits()).as_ptr().unwrap(),
                        "token rows",
                    )
                }
                .unwrap();
                let observed: Vec<(i64, String)> = rows
                    .iter()
                    .map(|&row| unsafe {
                        seq_access::with_immutable_tuple_slice(
                            obj_from_bits(row).as_ptr().unwrap(),
                            |fields| {
                                assert_eq!(fields.len(), 5);
                                (
                                    to_i64(obj_from_bits(fields[0])).unwrap(),
                                    string_obj_to_owned(obj_from_bits(fields[1])).unwrap(),
                                )
                            },
                        )
                        .unwrap()
                    })
                    .collect();
                assert_eq!(
                    observed,
                    [
                        (1, "x".into()),
                        (55, "=".into()),
                        (2, "1".into()),
                        (4, "\n".into()),
                        (comment, "# note".into()),
                        (nl, "\n".into()),
                        (0, "".into()),
                    ]
                );
            });
        }
    });
}

#[test]
fn token_and_encoding_tuples_transfer_exactly_one_owned_reference_per_child() {
    let _transaction = RuntimeTestTransaction::new();
    crate::with_gil_entry_nopanic!(py, {
        let line = ExceptionValue::adopt(py, text(py, "a separately retained source line\n"));
        let baseline = refs(line.bits());
        let result = make_token_tuple(
            py,
            1,
            "a noninterned token string",
            (1, 0),
            (1, 25),
            line.bits(),
        )
        .expect("complete owned token tuple");
        assert!(!exception_pending(py));
        assert_eq!(refs(line.bits()), baseline + 1);
        let children = unsafe {
            seq_access::with_immutable_tuple_slice(
                obj_from_bits(result).as_ptr().unwrap(),
                |fields| {
                    let children = [fields[1], fields[2], fields[3]];
                    for &child in &children {
                        assert_eq!(refs(child), 1, "outer tuple is the sole child owner");
                        inc_ref_bits(py, child);
                    }
                    children
                },
            )
            .unwrap()
        };
        dec_ref_bits(py, result);
        assert_eq!(refs(line.bits()), baseline);
        for child in children {
            assert_eq!(refs(child), 1, "retained witness survives tuple retirement");
            dec_ref_bits(py, child);
        }

        let encoding = encoding_detection_tuple(py, "utf-8", false);
        assert!(!exception_pending(py));
        let name = unsafe {
            seq_access::with_immutable_tuple_slice(
                obj_from_bits(encoding).as_ptr().unwrap(),
                |fields| {
                    assert_eq!(fields.len(), 2);
                    assert_eq!(refs(fields[0]), 1);
                    inc_ref_bits(py, fields[0]);
                    fields[0]
                },
            )
            .unwrap()
        };
        dec_ref_bits(py, encoding);
        assert_eq!(refs(name), 1);
        assert_eq!(
            string_obj_to_owned(obj_from_bits(name)).as_deref(),
            Some("utf-8")
        );
        dec_ref_bits(py, name);
    });
}

struct RestoreBudget;

impl Drop for RestoreBudget {
    fn drop(&mut self) {
        set_tracker(Box::new(UnlimitedTracker));
    }
}

#[test]
fn token_constructors_fail_atomically_and_preserve_the_original_pending_error() {
    let _transaction = RuntimeTestTransaction::new();
    crate::with_gil_entry_nopanic!(py, {
        let line = ExceptionValue::adopt(py, text(py, "retained failure witness line\n"));
        let baseline = refs(line.bits());
        let token =
            || match make_token_tuple(py, 1, "owned failed token", (1, 0), (1, 18), line.bits()) {
                Some(bits) => bits,
                None => {
                    assert!(exception_pending(py));
                    MoltObject::none().bits()
                }
            };
        let encoding = || encoding_detection_tuple(py, "utf-8", false);
        for constructor in [&token as &dyn Fn() -> u64, &encoding] {
            let warm = constructor();
            assert!(!exception_pending(py));
            dec_ref_bits(py, warm);
            let mut succeeded = false;
            let mut failed = false;
            // Same allocation-denial authority and bounded first-success sweep
            // as the owning sys metadata constructor tests.
            for limit in 0..=64 {
                set_tracker(Box::new(LimitedTracker::new(&ResourceLimits {
                    max_allocations: Some(limit + 1),
                    ..Default::default()
                })));
                let budget = RestoreBudget;
                let sentinel = crate::object::backing::tracked_vec_box_with_capacity::<u64>(0)
                    .expect("one separately owned allocation witness");
                let result = constructor();
                if obj_from_bits(result).is_none() {
                    assert!(
                        exception_pending(py),
                        "silent allocation failure at {limit}"
                    );
                    failed = true;
                } else {
                    assert!(
                        !exception_pending(py),
                        "partial result published at {limit}"
                    );
                    dec_ref_bits(py, result);
                    succeeded = true;
                }
                assert_eq!(refs(line.bits()), baseline);
                crate::resource::with_tracker(|tracker| {
                    for _ in 0..limit {
                        assert!(
                            tracker.on_allocate(1).is_ok(),
                            "constructor rollback leaked a completed child"
                        );
                    }
                    assert!(
                        tracker.on_allocate(1).is_err(),
                        "cleanup released another owner's allocation"
                    );
                    for _ in 0..limit {
                        tracker.on_free(1);
                    }
                });
                unsafe { drop(crate::object::backing::tracked_vec_box_from_raw(sentinel)) };
                drop(budget);
                crate::clear_exception(py);
                if succeeded {
                    break;
                }
            }
            assert!(failed && succeeded);
        }

        let source = ExceptionValue::adopt(py, text(py, "x = 1\n"));
        let scan = || molt_tokenize_scan(source.bits());
        let payload = || crate::molt_token_payload();
        for constructor in [&token as &dyn Fn() -> u64, &encoding, &scan, &payload] {
            raise_exception::<u64>(py, "ValueError", "original token construction error");
            let original = ExceptionValue::adopt(py, molt_exception_last_pending());
            let result = constructor();
            assert!(
                obj_from_bits(result).is_none(),
                "pending error must stop publication"
            );
            let observed = ExceptionValue::adopt(py, molt_exception_last_pending());
            assert_eq!(
                observed.bits(),
                original.bits(),
                "first error identity must survive"
            );
            assert_eq!(refs(line.bits()), baseline);
            crate::clear_exception(py);
        }
    });
}
