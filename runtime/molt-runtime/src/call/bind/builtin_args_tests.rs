use super::*;

fn refs(bits: u64) -> u32 {
    unsafe {
        (*crate::header_from_obj_ptr(obj_from_bits(bits).as_ptr().unwrap())).ref_count_snapshot()
    }
}

#[test]
fn builtin_materialization_scope_releases_packed_owners_on_success_and_error() {
    let _transaction = crate::test_support::RuntimeTestTransaction::new();
    crate::with_gil_entry_nopanic!(py, {
        unsafe {
            let format = MoltObject::from_ptr(alloc_string(py, b"")).bits();
            let key = MoltObject::from_ptr(alloc_string(py, b"key")).bits();
            let value = MoltObject::from_ptr(crate::alloc_list(py, &[])).bits();
            let baseline = refs(value);
            // The same adapters serve ordinary and expanded calls. Both hold
            // their original owners outside the scope of materialized packing.
            for failure in [false, true] {
                let mut storage = BuiltinArgumentStorage::default();
                let positional = [format, value];
                let names = [if failure {
                    MoltObject::from_int(1).bits()
                } else {
                    key
                }];
                let keywords = [value];
                let args = CallArgumentView {
                    pos: &positional,
                    kw_names: &names,
                    kw_values: &keywords,
                };
                let bound = bind_builtin_string_format(py, &args, &mut storage);
                if failure {
                    assert!(bound.is_none());
                    assert!(exception_pending(py));
                    let original = crate::exception_last_bits_noinc(py).unwrap();
                    drop(storage);
                    assert_eq!(crate::exception_last_bits_noinc(py), Some(original));
                    crate::molt_exception_clear();
                } else {
                    let bound = bound.unwrap();
                    assert_eq!(refs(value), baseline + 2);
                    let result = molt_string_format_method(bound[0], bound[1], bound[2]);
                    assert!(!exception_pending(py));
                    dec_ref_bits(py, result);
                    drop(storage);
                }
                assert_eq!(refs(value), baseline, "packing must not outlive the call");
            }
            for bits in [format, key, value] {
                dec_ref_bits(py, bits);
            }
        }
    });
}

#[test]
fn builtin_materialization_print_defaults_and_binding_failure_release_arguments() {
    let _transaction = crate::test_support::RuntimeTestTransaction::new();
    crate::with_gil_entry_nopanic!(py, {
        unsafe {
            let value = MoltObject::from_ptr(crate::alloc_list(py, &[])).bits();
            let key = MoltObject::from_ptr(alloc_string(py, b"invalid")).bits();
            let baseline = refs(value);
            for invalid in [false, true] {
                let mut storage = BuiltinArgumentStorage::default();
                let pos = [value];
                let names = [key];
                let values = [value];
                let args = CallArgumentView {
                    pos: &pos,
                    kw_names: if invalid { &names } else { &[] },
                    kw_values: if invalid { &values } else { &[] },
                };
                let bound = bind_builtin_print(py, &args, &mut storage);
                if invalid {
                    assert!(bound.is_none());
                    assert!(exception_pending(py));
                } else {
                    let bound = bound.unwrap();
                    assert!(obj_from_bits(bound[1]).is_none());
                    assert!(obj_from_bits(bound[2]).is_none());
                }
                assert_eq!(refs(value), baseline + 1);
                drop(storage);
                assert_eq!(refs(value), baseline);
                if invalid {
                    crate::molt_exception_clear();
                }
            }
            dec_ref_bits(py, value);
            dec_ref_bits(py, key);
        }
    });
}

#[test]
fn builtin_materialization_type_keywords_and_set_operands_have_scoped_custody() {
    let _transaction = crate::test_support::RuntimeTestTransaction::new();
    crate::with_gil_entry_nopanic!(py, {
        unsafe {
            let value = MoltObject::from_ptr(crate::alloc_list(py, &[])).bits();
            let key = MoltObject::from_ptr(alloc_string(py, b"custom")).bits();
            let set = crate::molt_set_new(0);
            assert!(!exception_pending(py));
            let baseline = refs(value);
            let mut storage = BuiltinArgumentStorage::default();
            let pos = [set, value];
            let args = CallArgumentView {
                pos: &pos,
                kw_names: &[],
                kw_values: &[],
            };
            assert!(
                bind_builtin_set_multi(py, &args, &mut storage, "union", "set", TYPE_ID_SET)
                    .is_some()
            );
            assert_eq!(refs(value), baseline + 1);
            drop(storage);
            assert_eq!(refs(value), baseline);

            let mut storage = BuiltinArgumentStorage::default();
            let pos = [builtin_classes(py).type_obj, key, set, set];
            let names = [key];
            let values = [value];
            let args = CallArgumentView {
                pos: &pos,
                kw_names: &names,
                kw_values: &values,
            };
            assert!(bind_builtin_type_new_init(py, &args, &mut storage).is_some());
            assert_eq!(refs(value), baseline + 1);
            drop(storage);
            assert_eq!(refs(value), baseline);
            for bits in [value, key, set] {
                dec_ref_bits(py, bits);
            }
        }
    });
}
