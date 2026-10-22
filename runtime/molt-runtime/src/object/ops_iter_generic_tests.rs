use super::*;

extern "C" fn generic_iteration_identity_next(receiver: u64) -> u64 {
    crate::with_gil_entry_nopanic!(py, {
        crate::inc_ref_bits(py, receiver);
        receiver
    })
}

#[test]
fn generic_iteration_owns_identity_without_dispatch_temporaries() {
    let _guard = crate::test_support::RuntimeTestTransaction::with_gc_isolation();
    crate::state::metrics::enable_profile_for_test();
    crate::with_gil_entry_nopanic!(py, {
        unsafe {
            let name = MoltObject::from_ptr(alloc_string(py, b"GenericIterationIdentity")).bits();
            let class = crate::molt_class_new(name);
            let function_ptr = crate::builtins::functions::alloc_runtime_function_obj(
                py,
                crate::provenance::abi::expose_function_address(
                    generic_iteration_identity_next as *const (),
                ),
                1,
            );
            assert!(!function_ptr.is_null());
            let function = MoltObject::from_ptr(function_ptr).bits();
            let next_name = MoltObject::from_ptr(alloc_string(py, b"__next__")).bits();
            crate::molt_set_attr_name(class, next_name, function);
            let iterator = crate::alloc_instance_for_class(
                py,
                MoltObject::from_bits(class)
                    .as_ptr()
                    .expect("fixture class"),
            );
            assert!(!crate::exception_pending(py));
            let pointer = MoltObject::from_bits(iterator)
                .as_ptr()
                .expect("fixture instance");
            assert!((*crate::header_from_obj_ptr(pointer)).gc_is_published());
            let refcount = || (*crate::header_from_obj_ptr(pointer)).owned_ref_count_snapshot();
            let baseline = refcount();
            for mode in ["direct", "unboxed", "boxed", "next", "next-default"] {
                // Warm shared names/call metadata, then count the public
                // ABI itself. The user callback merely retains its receiver.
                for measuring in [false, true] {
                    let before = crate::ALLOC_COUNT.load(std::sync::atomic::Ordering::Relaxed);
                    for _ in 0..if measuring { 400 } else { 1 } {
                        if mode == "boxed" {
                            let pair = molt_iter_next(iterator);
                            assert!(!crate::exception_pending(py));
                            let pair_ptr = MoltObject::from_bits(pair).as_ptr().unwrap();
                            let (value, done) = crate::object::seq_access::tuple_pair(pair_ptr)
                                .expect("boxed iterator result");
                            assert_eq!(value, iterator);
                            assert_eq!(MoltObject::from_bits(done).as_bool(), Some(false));
                            dec_ref_bits(py, pair);
                        } else if mode == "unboxed" {
                            let mut value = MoltObject::none().bits();
                            let done =
                                molt_iter_next_unboxed(iterator, (&raw mut value) as usize as u64);
                            assert!(!crate::exception_pending(py));
                            assert_eq!(MoltObject::from_bits(done).as_bool(), Some(false));
                            assert_eq!(value, iterator);
                            dec_ref_bits(py, value);
                        }
                        if matches!(mode, "direct" | "next" | "next-default") {
                            let value = if mode == "direct" {
                                call_callable1(py, function, iterator)
                            } else {
                                let default = if mode == "next" {
                                    missing_bits(py)
                                } else {
                                    iterator
                                };
                                molt_next_builtin(iterator, default)
                            };
                            assert!(!crate::exception_pending(py));
                            assert_eq!(value, iterator);
                            dec_ref_bits(py, value);
                        }
                        assert_eq!(refcount(), baseline, "{mode} leaked an owner");
                    }
                    if measuring {
                        let allocations =
                            crate::ALLOC_COUNT.load(std::sync::atomic::Ordering::Relaxed) - before;
                        assert_eq!(allocations, if mode == "boxed" { 400 } else { 0 });
                        eprintln!(
                            "generic iteration: mode={mode} steps=400 allocations={allocations}"
                        );
                    }
                }
            }
            for value in [iterator, next_name, function, class, name] {
                dec_ref_bits(py, value);
            }
            assert!(!crate::exception_pending(py));
        }
    });
}

extern "C" fn generic_iteration_raise_next(receiver: u64) -> u64 {
    crate::with_gil_entry_nopanic!(py, {
        let name = MoltObject::from_ptr(alloc_string(py, b"error")).bits();
        let error = molt_getattr_builtin(receiver, name, missing_bits(py));
        dec_ref_bits(py, name);
        let result = molt_raise(error);
        dec_ref_bits(py, error);
        result
    })
}

#[test]
fn generic_iteration_preserves_exception_identity_and_releases_completion_payloads() {
    let _guard = crate::test_support::RuntimeTestTransaction::with_gc_isolation();
    crate::with_gil_entry_nopanic!(py, {
        unsafe {
            let name = MoltObject::from_ptr(alloc_string(py, b"GenericIterationStops")).bits();
            let class = crate::molt_class_new(name);
            let function =
                MoltObject::from_ptr(crate::builtins::functions::alloc_runtime_function_obj(
                    py,
                    crate::provenance::abi::expose_function_address(
                        generic_iteration_raise_next as *const (),
                    ),
                    1,
                ))
                .bits();
            let next_name = MoltObject::from_ptr(alloc_string(py, b"__next__")).bits();
            crate::molt_set_attr_name(class, next_name, function);
            let iterator =
                crate::alloc_instance_for_class(py, obj_from_bits(class).as_ptr().unwrap());
            let error_name = MoltObject::from_ptr(alloc_string(py, b"error")).bits();
            let payload = MoltObject::from_ptr(alloc_list(py, &[])).bits();
            let args = MoltObject::from_ptr(alloc_tuple(py, &[payload, iterator])).bits();
            for exception_name in ["StopIteration", "RuntimeError"] {
                let error = crate::builtins::exceptions::molt_exception_new_from_class(
                    exception_type_bits_from_name(py, exception_name),
                    args,
                );
                crate::molt_set_attr_name(iterator, error_name, error);
                assert!(!exception_pending(py));
                let payload_pointer = obj_from_bits(payload).as_ptr().unwrap();
                let baseline = (*header_from_obj_ptr(payload_pointer)).owned_ref_count_snapshot();
                for mode in ["next", "next-default", "boxed", "unboxed"] {
                    let result = match mode {
                        "next" => molt_next_builtin(iterator, missing_bits(py)),
                        "next-default" => molt_next_builtin(iterator, payload),
                        "boxed" => molt_iter_next(iterator),
                        "unboxed" => {
                            let mut output = payload;
                            let done =
                                molt_iter_next_unboxed(iterator, (&raw mut output) as usize as u64);
                            assert!(obj_from_bits(output).is_none());
                            done
                        }
                        _ => unreachable!(),
                    };
                    if exception_name == "RuntimeError" || mode == "next" {
                        assert!(exception_pending(py));
                        let caught = molt_exception_last_pending();
                        assert_eq!(caught, error, "{mode} replaced the original exception");
                        dec_ref_bits(py, caught);
                        clear_exception(py);
                    } else {
                        assert!(!exception_pending(py));
                        if mode == "next-default" {
                            assert_eq!(result, payload);
                        }
                        if mode == "boxed" {
                            let (value, done) = crate::object::seq_access::tuple_pair(
                                obj_from_bits(result).as_ptr().unwrap(),
                            )
                            .unwrap();
                            assert_eq!(value, payload);
                            assert_eq!(obj_from_bits(done).as_bool(), Some(true));
                        }
                        if mode == "unboxed" {
                            assert_eq!(obj_from_bits(result).as_bool(), Some(true));
                        }
                    }
                    dec_ref_bits(py, result);
                    assert_eq!(
                        (*header_from_obj_ptr(payload_pointer)).owned_ref_count_snapshot(),
                        baseline
                    );
                }
                crate::molt_set_attr_name(iterator, error_name, MoltObject::none().bits());
                dec_ref_bits(py, error);
            }
            for value in [
                args, payload, error_name, iterator, next_name, function, class, name,
            ] {
                dec_ref_bits(py, value);
            }
            assert!(!exception_pending(py));
        }
    });
}

#[test]
fn zip_partial_result_lives_in_cache_and_aliases_stay_immutable() {
    let _guard = crate::test_support::RuntimeTestTransaction::with_gc_isolation();
    crate::with_gil_entry_nopanic!(py, {
        unsafe {
            let token = MoltObject::from_ptr(alloc_string(py, b"zip partial token")).bits();
            let source = MoltObject::from_ptr(alloc_tuple(py, &[token, token])).bits();
            let empty = MoltObject::from_ptr(alloc_tuple(py, &[])).bits();
            let token_ptr = obj_from_bits(token).as_ptr().unwrap();
            let refs = || (*header_from_obj_ptr(token_ptr)).owned_ref_count_snapshot();
            let baseline = refs();
            let zip = zip_new_impl(py, &[source, empty], false);
            let result = molt_next_builtin(zip, missing_bits(py));
            assert!(exception_pending(py));
            crate::molt_exception_clear();
            dec_ref_bits(py, result);
            assert_eq!(
                refs(),
                baseline + 1,
                "partial row belongs to the live zip cache"
            );
            dec_ref_bits(py, zip);
            assert_eq!(refs(), baseline, "zip retirement releases its partial row");

            let zip = zip_new_impl(py, &[source], false);
            let first = molt_next_builtin(zip, missing_bits(py));
            assert!(!exception_pending(py));
            let second = molt_next_builtin(zip, missing_bits(py));
            assert!(!exception_pending(py));
            assert_ne!(first, second, "an aliased result must force a fresh row");
            assert_eq!(
                crate::object::seq_access::item(obj_from_bits(first).as_ptr().unwrap(), 0),
                Some(token)
            );
            dec_ref_bits(py, second);
            dec_ref_bits(py, first);
            dec_ref_bits(py, zip);
            assert_eq!(refs(), baseline);
            for bits in [empty, source, token] {
                dec_ref_bits(py, bits);
            }
        }
    });
}

#[test]
fn tuple_cache_publication_retracks_a_demoted_result_before_retirement() {
    let _guard = crate::test_support::RuntimeTestTransaction::with_gc_isolation();
    crate::with_gil_entry_nopanic!(py, {
        unsafe {
            let cache = alloc_tuple(py, &[MoltObject::none().bits()]);
            crate::object::gc::gc_untrack(
                py,
                cache,
                TYPE_ID_TUPLE,
                crate::object::gc::GcUntrackReason::DynamicProjection,
            );
            assert!(!crate::object::gc::gc_is_tracked(cache));
            let list = MoltObject::from_ptr(alloc_list(py, &[])).bits();
            let mut lease =
                crate::object::seq_access::TupleCacheMutation::claim(py, cache, 1).unwrap();
            assert!(
                crate::object::seq_access::TupleCacheMutation::claim(py, cache, 1).is_none(),
                "live mutation claim prevents reentry"
            );
            let old = lease.replace_owned(0, list);
            assert_eq!(old, MoltObject::none().bits());
            assert!(crate::object::gc::gc_is_tracked(cache));
            assert!(
                (*header_from_obj_ptr(cache)).has_flag(crate::object::HEADER_FLAG_CONTAINS_REFS)
            );
            dec_ref_bits(py, old);
            drop(lease);
            dec_ref_ptr(py, cache);
        }
    });
}
