use super::*;
use crate::TYPE_ID_OBJECT;
use crate::call::bind::test_support::*;

#[test]
fn cached_method_name_must_match_even_at_the_same_site() {
    let _test = crate::test_support::RuntimeTestTransaction::new();
    crate::with_gil_entry_nopanic!(_py, {
        let alpha_ptr = crate::call::bind::alloc_string(_py, b"alpha");
        assert!(!alpha_ptr.is_null());
        let alpha_bits = MoltObject::from_ptr(alpha_ptr).bits();
        assert!(crate::object::ops_format::string_obj_matches_bytes(
            obj_from_bits(alpha_bits),
            b"alpha"
        ));
        assert!(
            !crate::object::ops_format::string_obj_matches_bytes(
                obj_from_bits(alpha_bits),
                b"beta"
            ),
            "a same-site lookup for another name must miss instead of reusing the target"
        );
        dec_ref_bits(_py, alpha_bits);
    });
}

#[test]
fn type_call_ic_returns_single_owned_constructor_result_after_borrowed_init() {
    let _test = crate::test_support::RuntimeTestTransaction::new();
    crate::with_gil_entry_nopanic!(_py, {
        clear_call_bind_ic_cache(_py);
        let init_ptr = crate::builtins::functions::alloc_runtime_function_obj(
            _py,
            crate::provenance::abi::expose_function_address(
                compiled_init_borrows_self_for_type_call_ic as *const (),
            ),
            1,
        );
        assert!(!init_ptr.is_null());
        let init_bits = MoltObject::from_ptr(init_ptr).bits();
        let builtins = crate::builtins::classes::builtin_classes(_py);
        let name_ptr = crate::call::bind::alloc_string(_py, b"IcCtor");
        let init_name_ptr = crate::call::bind::alloc_string(_py, b"__init__");
        assert!(!name_ptr.is_null());
        assert!(!init_name_ptr.is_null());
        let name_bits = MoltObject::from_ptr(name_ptr).bits();
        let init_name_bits = MoltObject::from_ptr(init_name_ptr).bits();
        let attrs = [init_name_bits, init_bits];
        let bases = [builtins.object];
        let class_bits = unsafe {
            crate::object::ops::molt_guarded_class_def(
                name_bits,
                crate::provenance::abi::expose_address(bases.as_ptr()),
                bases.len() as u64,
                crate::provenance::abi::expose_address(attrs.as_ptr()),
                1,
                std::mem::size_of::<u64>() as i64,
                0,
                1, // Install the supplied bases before inherited-hook dispatch.
            )
        };
        assert!(!obj_from_bits(class_bits).is_none());
        let class_ptr = obj_from_bits(class_bits).as_ptr().expect("class ptr");
        let layout_size = unsafe {
            crate::call::class_init::class_layout_size_cached(_py, class_ptr)
                .expect("class layout must be representable")
        };
        let churn_name = crate::call::bind::alloc_string(_py, b"unrelated_attr");
        let churn_bits = MoltObject::from_ptr(churn_name).bits();
        assert_eq!(
            crate::molt_set_attr_name(class_bits, churn_bits, MoltObject::from_int(1).bits()),
            MoltObject::none().bits()
        );
        assert_eq!(
            unsafe { crate::call::class_init::class_layout_size_cached(_py, class_ptr) },
            Some(layout_size),
            "ordinary class attribute churn must not invalidate immutable payload size"
        );
        dec_ref_bits(_py, churn_bits);

        let layout_name = crate::call::bind::alloc_string(_py, b"__molt_layout_size__");
        let layout_name_bits = MoltObject::from_ptr(layout_name).bits();
        let _ =
            crate::molt_set_attr_name(class_bits, layout_name_bits, MoltObject::from_int(1).bits());
        assert_eq!(crate::molt_exception_pending(), 1);
        let _ = crate::molt_exception_clear();
        assert_eq!(
            unsafe { crate::call::class_init::class_layout_size_cached(_py, class_ptr) },
            Some(layout_size)
        );
        dec_ref_bits(_py, layout_name_bits);
        let entry = CallBindIcEntry {
            fn_ptr: crate::provenance::abi::expose_function_address(
                compiled_init_borrows_self_for_type_call_ic as *const (),
            ),
            target_bits: init_bits,
            class_bits,
            class_version: unsafe { crate::class_layout_version_bits(class_ptr) },
            type_version: crate::global_type_version(),
            function_version: 0,
            cached_alloc_size: layout_size
                .checked_add(std::mem::size_of::<crate::object::MoltHeader>())
                .expect("class allocation size must be representable"),
            arity: 0,
            kind: CALL_BIND_IC_KIND_TYPE_CALL,
        };
        let mut arguments =
            crate::call::bind::arguments::CallArguments::retained(_py, None, &[], &[], &[])
                .expect("an empty argument vector");
        let result_bits = unsafe {
            try_call_bind_ic_fast(_py, entry, class_bits, &mut arguments)
                .expect("type-call IC entry should apply")
        };
        let result_ptr = obj_from_bits(result_bits).as_ptr().expect("live instance");
        assert_eq!(unsafe { object_type_id(result_ptr) }, TYPE_ID_OBJECT);
        let ref_count =
            unsafe { (*crate::object::header_from_obj_ptr(result_ptr)).ref_count_snapshot() };
        assert_eq!(
            ref_count, 1,
            "type-call IC must return exactly the constructor result owner; borrowed __init__ self must not leave a hidden retain"
        );
        dec_ref_bits(_py, result_bits);
        // The same physical allocation shortcut must miss after abstractness
        // changes; the slow/default and explicit object allocators then agree.
        let abstract_name = crate::attr_name_bits_from_bytes(_py, b"__abstractmethods__").unwrap();
        let methods =
            MoltObject::from_ptr(crate::alloc_tuple(_py, &[init_name_bits, name_bits])).bits();
        crate::molt_set_attr_name(class_bits, abstract_name, methods);
        assert!(!crate::exception_pending(_py));
        assert!(unsafe { try_call_bind_ic_fast(_py, entry, class_bits, &mut arguments) }.is_none());
        for direct in [false, true] {
            let rejected = if direct {
                crate::molt_object_new_bound(class_bits)
            } else {
                unsafe { crate::call::class_init::call_class_init_with_args(_py, class_ptr, &[]) }
            };
            assert!(obj_from_bits(rejected).is_none());
            let error = crate::exception_last_bits_noinc(_py).unwrap();
            assert_eq!(
                crate::format_exception_message(_py, obj_from_bits(error).as_ptr().unwrap()),
                "Can't instantiate abstract class IcCtor without an implementation for abstract methods 'IcCtor', '__init__'"
            );
            crate::molt_exception_clear();
        }
        crate::molt_del_attr_name(class_bits, abstract_name);
        let recovered = crate::molt_object_new_bound(class_bits);
        assert!(!obj_from_bits(recovered).is_none());
        assert!(!crate::exception_pending(_py));
        for bits in [recovered, methods, abstract_name] {
            dec_ref_bits(_py, bits);
        }
        drop(arguments);
        dec_ref_bits(_py, init_name_bits);
        dec_ref_bits(_py, name_bits);
        dec_ref_bits(_py, class_bits);
        dec_ref_bits(_py, init_bits);
    });
}

#[test]
fn clear_call_bind_ic_cache_clears_thread_local_cache() {
    let _test = crate::test_support::RuntimeTestTransaction::new();
    crate::with_gil_entry_nopanic!(_py, {
        let entry = CallBindIcEntry {
            fn_ptr: 11,
            target_bits: 22,
            class_bits: 0,
            class_version: 33,
            type_version: 0,
            function_version: 0,
            cached_alloc_size: 44,
            arity: 1,
            kind: CALL_BIND_IC_KIND_DIRECT_FUNC,
        };
        ic_tls_insert(_py, 99, entry);
        assert!(ic_tls_lookup(99).is_some());
        clear_call_bind_ic_cache(_py);
        assert!(ic_tls_lookup(99).is_none());
    });
}

#[test]
fn mro_resolved_call_cache_owns_and_releases_target() {
    let _test = crate::test_support::RuntimeTestTransaction::new();
    crate::with_gil_entry_nopanic!(_py, {
        clear_call_bind_ic_cache(_py);
        let func_ptr = crate::builtins::functions::alloc_runtime_function_obj(
            _py,
            crate::provenance::abi::expose_function_address(
                compiled_init_borrows_self_for_type_call_ic as *const (),
            ),
            1,
        );
        assert!(!func_ptr.is_null());
        let target_bits = MoltObject::from_ptr(func_ptr).bits();
        let before = unsafe { (*crate::header_from_obj_ptr(func_ptr)).ref_count_snapshot() };
        let entry = CallBindIcEntry {
            fn_ptr: crate::provenance::abi::expose_function_address(
                compiled_init_borrows_self_for_type_call_ic as *const (),
            ),
            target_bits,
            class_bits: 0,
            class_version: 0,
            type_version: crate::global_type_version(),
            function_version: 0,
            cached_alloc_size: 0,
            arity: 0,
            kind: CALL_BIND_IC_KIND_HEAP_CALL_SIMPLE_BOUND_FUNC,
        };
        ic_tls_insert(_py, 101, entry);
        let retained = unsafe { (*crate::header_from_obj_ptr(func_ptr)).ref_count_snapshot() };
        assert_eq!(retained, before + 1);
        clear_call_bind_ic_cache(_py);
        let released = unsafe { (*crate::header_from_obj_ptr(func_ptr)).ref_count_snapshot() };
        assert_eq!(released, before);
        dec_ref_bits(_py, target_bits);
    });
}

#[test]
fn public_gil_release_drains_foreign_thread_call_cache_owners() {
    let _guard = crate::test_support::RuntimeTestTransaction::new();
    assert_eq!(crate::c_api::molt_init(), 0);
    let (target_bits, target_address, before) = crate::with_gil_entry_nopanic!(_py, {
        let func_ptr = crate::builtins::functions::alloc_runtime_function_obj(
            _py,
            crate::provenance::abi::expose_function_address(
                compiled_init_borrows_self_for_type_call_ic as *const (),
            ),
            1,
        );
        assert!(!func_ptr.is_null());
        let target_bits = MoltObject::from_ptr(func_ptr).bits();
        let before = unsafe { (*crate::header_from_obj_ptr(func_ptr)).ref_count_snapshot() };
        (target_bits, func_ptr as usize, before)
    });

    let worker = std::thread::spawn(move || {
        assert_eq!(crate::c_api::molt_gil_acquire(), 0);
        let retained = crate::with_gil_entry_nopanic!(_py, {
            ic_tls_insert(
                _py,
                0x4d4f_4c54,
                CallBindIcEntry {
                    fn_ptr: crate::provenance::abi::expose_function_address(
                        compiled_init_borrows_self_for_type_call_ic as *const (),
                    ),
                    target_bits,
                    class_bits: 0,
                    class_version: 0,
                    type_version: crate::global_type_version(),
                    function_version: 0,
                    cached_alloc_size: 0,
                    arity: 0,
                    kind: CALL_BIND_IC_KIND_HEAP_CALL_SIMPLE_BOUND_FUNC,
                },
            );
            unsafe { (*crate::header_from_obj_ptr(target_address as *mut u8)).ref_count_snapshot() }
        });
        assert_eq!(crate::c_api::molt_gil_release(), 0);
        let released = unsafe {
            (*crate::header_from_obj_ptr(target_address as *mut u8)).ref_count_snapshot()
        };
        (retained, released)
    });
    let (retained, released) = worker.join().expect("foreign thread must exit cleanly");
    assert_eq!(retained, before + 1, "thread-local IC must own its target");
    assert_eq!(
        released, before,
        "outermost public GIL release must drain foreign-thread IC owners before detach"
    );
    crate::with_gil_entry_nopanic!(_py, {
        dec_ref_bits(_py, target_bits);
    });
}

#[cfg(feature = "l7-attestation-probe")]
#[test]
fn direct_call_ic_hot_path_is_allocation_free_with_slow_path_control() {
    let _guard = crate::test_support::RuntimeTestTransaction::new();
    crate::with_gil_entry_nopanic!(_py, {
        let func_ptr = crate::builtins::functions::alloc_runtime_function_obj(
            _py,
            crate::provenance::abi::expose_function_address(
                compiled_identity_returns_owned_arg as *const (),
            ),
            1,
        );
        assert!(!func_ptr.is_null());
        let func_bits = MoltObject::from_ptr(func_ptr).bits();
        let mut args = crate::call::bind::arguments::CallArguments::retained(
            _py,
            None,
            &[MoltObject::from_int(17).bits()],
            &[],
            &[],
        )
        .expect("an argument vector");
        let entry = CallBindIcEntry {
            fn_ptr: crate::provenance::abi::expose_function_address(
                compiled_identity_returns_owned_arg as *const (),
            ),
            target_bits: 0,
            class_bits: 0,
            class_version: 0,
            type_version: crate::global_type_version(),
            function_version: 0,
            cached_alloc_size: 0,
            arity: 1,
            kind: CALL_BIND_IC_KIND_DIRECT_FUNC,
        };
        for _ in 0..64 {
            assert_eq!(
                unsafe { try_call_bind_ic_fast(_py, entry, func_bits, &mut args) },
                Some(MoltObject::from_int(17).bits())
            );
        }

        crate::attestation_probe::reset();
        crate::attestation_probe::set_tracking(true);
        for _ in 0..10_000 {
            assert_eq!(
                unsafe { try_call_bind_ic_fast(_py, entry, func_bits, &mut args) },
                Some(MoltObject::from_int(17).bits())
            );
        }
        crate::attestation_probe::set_tracking(false);
        let fast = crate::attestation_probe::snapshot();
        assert_eq!(
            fast.allocations, 0,
            "direct IC hot path allocated: {fast:?}"
        );

        // Bypass the IC and exercise the production CallArgs/binder entry as
        // the observer control. This must register allocation traffic, or a
        // zero fast-path count would not be meaningful evidence.
        crate::attestation_probe::reset();
        crate::attestation_probe::set_tracking(true);
        for _ in 0..64 {
            let builder_bits = crate::call::bind::arguments::molt_callargs_new(1, 0);
            assert!(!obj_from_bits(builder_bits).is_none());
            assert_eq!(
                unsafe {
                    crate::call::bind::arguments::molt_callargs_push_pos(
                        builder_bits,
                        MoltObject::from_int(17).bits(),
                    )
                },
                MoltObject::none().bits()
            );
            assert_eq!(
                crate::call::bind::molt_call_bind(func_bits, builder_bits),
                MoltObject::from_int(17).bits()
            );
        }
        crate::attestation_probe::set_tracking(false);
        let slow = crate::attestation_probe::snapshot();
        assert!(
            slow.allocations > 0,
            "slow-path control observed no allocations: {slow:?}"
        );
        dec_ref_bits(_py, func_bits);
    });
}

#[test]
fn type_epoch_invalidates_every_mro_resolved_call_cache_family() {
    let recorded = crate::global_type_version();
    assert!(type_epoch_matches(recorded));
    assert!(type_resolution_epoch_is_stable(recorded));
    crate::bump_type_version();
    assert!(!type_epoch_matches(recorded));
    assert!(!type_resolution_epoch_is_stable(recorded));
}

#[test]
fn native_method_load_preserves_descriptor_identity_and_one_receiver_owner() {
    let _test = crate::test_support::RuntimeTestTransaction::new();
    crate::with_gil_entry_nopanic!(py, {
        unsafe {
            let name = crate::attr_name_bits_from_bytes(py, b"reverse").unwrap();
            let label = crate::attr_name_bits_from_bytes(py, b"NoArgsList").unwrap();
            let class = crate::molt_class_new(label);
            crate::molt_class_set_base(class, builtin_classes(py).list);
            assert!(!exception_pending(py));
            let receiver = crate::call::bind::call_bind_borrowed(py, class, None, &[], &[], &[]);
            assert!(!exception_pending(py));
            let pointer = obj_from_bits(receiver)
                .as_ptr()
                .expect("list subtype instance");
            let refs = || (*crate::object::header_from_obj_ptr(pointer)).ref_count_snapshot();
            let original_refs = refs();

            // Cold and repeated LOAD_ATTR both return the class descriptor,
            // never a freshly allocated bound-method payload. The pair owns
            // exactly one receiver reference, released by CALL or the caller.
            for site in [964, 964] {
                let loaded = load_method(
                    py,
                    MoltObject::from_int(site).bits(),
                    receiver,
                    b"reverse".as_ptr(),
                    7,
                );
                assert!(!exception_pending(py));
                assert_eq!(loaded.self_or_null, receiver);
                assert_eq!(refs(), original_refs + 1);
                let descriptor = crate::molt_get_attr_name(builtin_classes(py).list, name);
                assert_eq!(loaded.callable, descriptor);
                assert_eq!(
                    object_type_id(obj_from_bits(loaded.callable).as_ptr().unwrap()),
                    TYPE_ID_FUNCTION
                );
                dec_ref_bits(py, descriptor);
                let result = call_loaded_method(
                    py,
                    loaded.callable,
                    loaded.self_or_null,
                    &[MoltObject::from_int(1).bits()],
                );
                dec_ref_bits(py, result);
                let error = crate::exception_last_bits_noinc(py).unwrap();
                assert_eq!(
                    crate::format_exception_message(py, obj_from_bits(error).as_ptr().unwrap()),
                    "list.reverse() takes no arguments (1 given)"
                );
                crate::molt_exception_clear();
                assert_eq!(refs(), original_refs);
            }

            let bound = crate::molt_get_attr_name(receiver, name);
            assert_eq!(
                object_type_id(obj_from_bits(bound).as_ptr().unwrap()),
                TYPE_ID_BOUND_METHOD
            );
            assert_eq!(refs(), original_refs + 1);
            let result = crate::call::bind::call_bind_borrowed(
                py,
                bound,
                None,
                &[MoltObject::from_int(1).bits()],
                &[],
                &[],
            );
            dec_ref_bits(py, result);
            let error = crate::exception_last_bits_noinc(py).unwrap();
            assert_eq!(
                crate::format_exception_message(py, obj_from_bits(error).as_ptr().unwrap()),
                "NoArgsList.reverse() takes no arguments (1 given)"
            );
            crate::molt_exception_clear();
            dec_ref_bits(py, bound);
            assert_eq!(refs(), original_refs);
            for value in [receiver, class, label, name] {
                dec_ref_bits(py, value);
            }
        }
    });
}
