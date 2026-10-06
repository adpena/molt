use super::*;
use crate::call::bind::test_support::*;
use crate::object::builders::{alloc_list, alloc_tuple};

#[test]
fn bound_call_slots_own_borrowed_values_and_transferred_containers() {
    let _transaction = crate::test_support::RuntimeTestTransaction::new();
    crate::with_gil_entry_nopanic!(_py, {
        unsafe {
            let value = alloc_list(_py, &[]);
            assert!(!value.is_null());
            let value_bits = MoltObject::from_ptr(value).bits();
            let refs = || (*crate::header_from_obj_ptr(value)).ref_count_snapshot();
            let baseline = refs();
            let layout = crate::call::bind::frame_binding::FrameSlotLayout {
                positional: 2,
                has_vararg: false,
                keyword_only: 0,
                has_varkw: false,
            };
            let mut slots =
                crate::call::bind::frame_binding::BoundCallSlots::new(_py, layout).unwrap();
            slots.set_borrowed(0, value_bits);
            assert_eq!(refs(), baseline + 1);
            let tuple = alloc_tuple(_py, &[value_bits]);
            assert!(!tuple.is_null());
            slots.set_owned(1, MoltObject::from_ptr(tuple).bits());
            assert_eq!(refs(), baseline + 2);
            drop(slots);
            assert_eq!(refs(), baseline);
            dec_ref_bits(_py, value_bits);
        }
    });
}

#[test]
fn keyword_default_lookup_returns_an_owner_independent_of_metadata_dictionary() {
    let _transaction = crate::test_support::RuntimeTestTransaction::new();
    crate::with_gil_entry_nopanic!(_py, {
        unsafe {
            let function = crate::builtins::functions::alloc_runtime_function_obj(
                _py,
                crate::provenance::abi::expose_function_address(
                    compiled_identity_returns_owned_arg as *const (),
                ),
                1,
            );
            assert!(!function.is_null());
            let function_bits = MoltObject::from_ptr(function).bits();
            let name = crate::alloc_string(_py, b"value");
            let value = alloc_list(_py, &[]);
            assert!(!name.is_null() && !value.is_null());
            let name_bits = MoltObject::from_ptr(name).bits();
            let value_bits = MoltObject::from_ptr(value).bits();
            let dictionary = crate::alloc_dict_with_pairs(_py, &[name_bits, value_bits]);
            assert!(!dictionary.is_null());
            let dictionary_bits = MoltObject::from_ptr(dictionary).bits();
            let attribute = intern_metadata_name(_py, b"__kwdefaults__");
            assert!(crate::call::class_init::function_set_attr_bits(
                _py,
                function,
                attribute,
                dictionary_bits,
            ));
            dec_ref_bits(_py, dictionary_bits);
            let value_before = (*crate::header_from_obj_ptr(value)).ref_count_snapshot();
            let owned = crate::call::bind::frame_binding::function_kwdefault_owned(
                _py, function, name_bits,
            )
            .unwrap()
            .unwrap();
            assert_eq!(owned, value_bits);
            assert_eq!(
                (*crate::header_from_obj_ptr(value)).ref_count_snapshot(),
                value_before + 1,
            );
            assert!(crate::call::class_init::function_set_attr_bits(
                _py,
                function,
                attribute,
                MoltObject::none().bits(),
            ));
            assert_eq!(
                (*crate::header_from_obj_ptr(value)).ref_count_snapshot(),
                value_before,
                "the owned default survives metadata replacement",
            );
            dec_ref_bits(_py, owned);
            for bits in [function_bits, name_bits, value_bits] {
                dec_ref_bits(_py, bits);
            }
            assert!(!crate::exception_pending(_py));
        }
    });
}

#[test]
fn call_bind_uses_replaced_keyword_value_at_binding_boundary() {
    let _transaction = crate::test_support::RuntimeTestTransaction::new();
    crate::with_gil_entry_nopanic!(_py, {
        unsafe {
            let func_ptr = crate::builtins::functions::alloc_runtime_function_obj(
                _py,
                crate::provenance::abi::expose_function_address(
                    compiled_identity_returns_owned_arg as *const (),
                ),
                1,
            );
            assert!(!func_ptr.is_null());
            let func_bits = MoltObject::from_ptr(func_ptr).bits();
            let key = crate::alloc_string(_py, b"key");
            assert!(!key.is_null());
            let key_bits = MoltObject::from_ptr(key).bits();
            let names = alloc_tuple(_py, &[key_bits]);
            assert!(!names.is_null());
            let names_bits = MoltObject::from_ptr(names).bits();
            assert!(crate::call::class_init::function_set_attr_bits(
                _py,
                func_ptr,
                intern_metadata_name(_py, b"__molt_arg_names__"),
                names_bits,
            ));
            let builder = crate::call::bind::arguments::molt_callargs_new(0, 1);
            assert_ne!(builder, 0);
            crate::call::bind::arguments::molt_callargs_push_kw(
                builder,
                key_bits,
                MoltObject::from_int(1).bits(),
            );
            let args = crate::call::bind::arguments::callargs_ptr(ptr_from_bits(builder));
            let dict = obj_from_bits((*args).keywords).as_ptr().unwrap();
            let expected = MoltObject::from_int(29).bits();
            crate::dict_set_in_place(_py, dict, key_bits, expected);
            assert_eq!(
                crate::call::bind::molt_call_bind(func_bits, builder),
                expected
            );
            assert!(!crate::exception_pending(_py));
            for bits in [names_bits, key_bits, func_bits] {
                dec_ref_bits(_py, bits);
            }
        }
    });
}

#[test]
fn frame_slot_layout_projects_declared_frame_order() {
    // `def f(a, b, *rest, k, **kw)`: the ABI order is `(a, b, rest, k, kw)`
    // while CPython's frame (`co_varnames`) is `(a, b, k, rest, kw)`.
    let layout = crate::call::bind::frame_binding::FrameSlotLayout {
        positional: 2,
        has_vararg: true,
        keyword_only: 1,
        has_varkw: true,
    };
    assert_eq!(layout.len(), 5);
    assert_eq!(layout.vararg_slot(), 2);
    assert_eq!(layout.keyword_only_slot(0), 3);
    assert_eq!(layout.varkw_slot(), 4);
    assert_eq!(layout.declared_order().collect::<Vec<_>>(), [0, 1, 3, 2, 4]);
    // 3.14 clears the same declared slots last to first.
    assert_eq!(
        layout.declared_order().rev().collect::<Vec<_>>(),
        [4, 2, 3, 1, 0]
    );
    let keyword_only = crate::call::bind::frame_binding::FrameSlotLayout {
        positional: 1,
        has_vararg: false,
        keyword_only: 2,
        has_varkw: false,
    };
    assert_eq!(keyword_only.declared_order().collect::<Vec<_>>(), [0, 1, 2]);
}

#[test]
fn bound_frame_is_the_only_argument_owner_during_the_call() {
    use std::sync::Mutex;
    static PROBES: Mutex<Vec<u64>> = Mutex::new(Vec::new());
    static OBSERVED: Mutex<Vec<u32>> = Mutex::new(Vec::new());
    // `def observed(a, *rest, k, **kw)`, called in ABI order `(a, rest, k, kw)`.
    extern "C" fn observe_frame_owners(_a: u64, _rest: u64, _k: u64, _kw: u64) -> i64 {
        let probes = PROBES.lock().unwrap().clone();
        let counts = probes
            .into_iter()
            .map(|bits| unsafe {
                let ptr = obj_from_bits(bits).as_ptr().expect("probed argument");
                (*crate::header_from_obj_ptr(ptr)).ref_count_snapshot()
            })
            .collect();
        *OBSERVED.lock().unwrap() = counts;
        MoltObject::none().bits() as i64
    }
    let _transaction = crate::test_support::RuntimeTestTransaction::new();
    crate::with_gil_entry_nopanic!(_py, {
        unsafe {
            let function = crate::builtins::functions::alloc_runtime_function_obj(
                _py,
                crate::provenance::abi::expose_function_address(observe_frame_owners as *const ()),
                4,
            );
            assert!(!function.is_null());
            let function_bits = MoltObject::from_ptr(function).bits();
            let string = |text: &[u8]| MoltObject::from_ptr(crate::alloc_string(_py, text)).bits();
            let names = [
                string(b"a"),
                string(b"k"),
                string(b"rest"),
                string(b"kw"),
                string(b"x"),
            ];
            let positional_names = MoltObject::from_ptr(alloc_tuple(_py, &names[..1])).bits();
            let keyword_only_names = MoltObject::from_ptr(alloc_tuple(_py, &names[1..2])).bits();
            for (field, value) in [
                (b"__molt_arg_names__".as_slice(), positional_names),
                (b"__molt_kwonly_names__".as_slice(), keyword_only_names),
                (b"__molt_vararg__".as_slice(), names[2]),
                (b"__molt_varkw__".as_slice(), names[3]),
            ] {
                assert!(crate::call::class_init::function_set_attr_bits(
                    _py,
                    function,
                    intern_metadata_name(_py, field),
                    value,
                ));
            }
            let values: Vec<u64> = (0..5)
                .map(|_| MoltObject::from_ptr(alloc_list(_py, &[])).bits())
                .collect();
            *PROBES.lock().unwrap() = values.clone();
            let builder = crate::call::bind::arguments::molt_callargs_new(3, 2);
            for &bits in &values[..3] {
                crate::call::bind::arguments::molt_callargs_push_pos(builder, bits);
            }
            crate::call::bind::arguments::molt_callargs_push_kw(builder, names[1], values[3]);
            crate::call::bind::arguments::molt_callargs_push_kw(builder, names[4], values[4]);
            assert!(!crate::exception_pending(_py));
            let result = crate::call::bind::molt_call_bind(function_bits, builder);
            assert!(!crate::exception_pending(_py));
            assert!(obj_from_bits(result).is_none());
            // `a` and `k` sit in parameter slots, `rest` owns two values and
            // `kw` owns `x`. No builder or keyword pin outlives admission.
            assert_eq!(
                *OBSERVED.lock().unwrap(),
                [2, 2, 2, 2, 2],
                "each argument has this test's owner and exactly one frame owner"
            );
            for &bits in &values {
                let ptr = obj_from_bits(bits).as_ptr().unwrap();
                assert_eq!(
                    (*crate::header_from_obj_ptr(ptr)).ref_count_snapshot(),
                    1,
                    "the frame released its owners when the activation ended"
                );
            }
            for bits in values
                .into_iter()
                .chain([function_bits, positional_names, keyword_only_names])
                .chain(names)
            {
                dec_ref_bits(_py, bits);
            }
        }
    });
}

#[test]
fn failed_binding_releases_every_argument_exactly_once() {
    use std::sync::atomic::{AtomicBool, Ordering};
    static ENTERED: AtomicBool = AtomicBool::new(false);
    // `def two(a, b)`: a failed binding never enters the callee.
    extern "C" fn two(_a: u64, _b: u64) -> i64 {
        ENTERED.store(true, Ordering::SeqCst);
        MoltObject::none().bits() as i64
    }
    let _transaction = crate::test_support::RuntimeTestTransaction::new();
    crate::with_gil_entry_nopanic!(_py, {
        unsafe {
            let function = crate::builtins::functions::alloc_runtime_function_obj(
                _py,
                crate::provenance::abi::expose_function_address(two as *const ()),
                2,
            );
            assert!(!function.is_null());
            let function_bits = MoltObject::from_ptr(function).bits();
            let string = |text: &[u8]| MoltObject::from_ptr(crate::alloc_string(_py, text)).bits();
            let names = [string(b"a"), string(b"b"), string(b"c"), string(b"d")];
            let parameters = MoltObject::from_ptr(alloc_tuple(_py, &names[..2])).bits();
            assert!(crate::call::class_init::function_set_attr_bits(
                _py,
                function,
                intern_metadata_name(_py, b"__molt_arg_names__"),
                parameters,
            ));
            // Surplus positional values; a parameter bound twice; an
            // unexpected keyword after a bound one; a missing parameter.
            for (positional, keywords) in [
                (3, &[][..]),
                (2, &names[..1]),
                (1, &names[1..4]),
                (1, &[][..]),
            ] {
                let values: Vec<u64> = (0..positional + keywords.len())
                    .map(|_| MoltObject::from_ptr(alloc_list(_py, &[])).bits())
                    .collect();
                let builder = crate::call::bind::arguments::molt_callargs_new(
                    positional as u64,
                    keywords.len() as u64,
                );
                for &bits in &values[..positional] {
                    crate::call::bind::arguments::molt_callargs_push_pos(builder, bits);
                }
                for (&name, &bits) in keywords.iter().zip(&values[positional..]) {
                    crate::call::bind::arguments::molt_callargs_push_kw(builder, name, bits);
                }
                assert!(!crate::exception_pending(_py));
                let result = crate::call::bind::molt_call_bind(function_bits, builder);
                assert!(obj_from_bits(result).is_none());
                assert!(crate::exception_pending(_py), "the binding error is raised");
                assert!(!ENTERED.load(Ordering::SeqCst), "the callee never ran");
                crate::molt_exception_clear();
                for bits in values {
                    let ptr = obj_from_bits(bits).as_ptr().unwrap();
                    assert_eq!(
                        (*crate::header_from_obj_ptr(ptr)).ref_count_snapshot(),
                        1,
                        "every argument is released exactly once"
                    );
                    dec_ref_bits(_py, bits);
                }
            }
            for bits in [function_bits, parameters].into_iter().chain(names) {
                dec_ref_bits(_py, bits);
            }
        }
    });
}

#[test]
fn inlined_frames_release_parameters_in_target_version_order() {
    let _transaction = crate::test_support::RuntimeTestTransaction::new();
    crate::with_gil_entry_nopanic!(_py, {
        unsafe {
            let probes = ReleaseProbes::new(_py);
            let string = |text: &[u8]| MoltObject::from_ptr(crate::alloc_string(_py, text)).bits();
            let [a, k, rest, kw, x] = [&b"a"[..], b"k", b"rest", b"kw", b"x"].map(string);
            let positional_names = MoltObject::from_ptr(alloc_tuple(_py, &[a])).bits();
            let keyword_only_names = MoltObject::from_ptr(alloc_tuple(_py, &[k])).bits();
            // `def mixed(a, *rest, k, **kw)`, called in ABI order `(a, rest, k, kw)`.
            let mixed = metadata_function(
                _py,
                none_of_four as *const (),
                4,
                &[
                    (b"__molt_arg_names__", positional_names),
                    (b"__molt_kwonly_names__", keyword_only_names),
                    (b"__molt_vararg__", rest),
                    (b"__molt_varkw__", kw),
                ],
                false,
            );
            // mixed(a, r1, r2, k=k, x=x). CPython 3.12 and 3.13 clear the
            // frame as `a, k, rest, kw`; 3.14 as `kw, rest, k, a`. The
            // `*args` tuple releases last to first in every version.
            for (minor, expected) in [
                (12, [0, 3, 2, 1, 4]),
                (13, [0, 3, 2, 1, 4]),
                (14, [4, 2, 1, 3, 0]),
            ] {
                with_target_minor(_py, minor, || {
                    let values = probes.instances(_py, 5);
                    let builder = last_owner_call(
                        _py,
                        crate::call::bind::arguments::CallForm::Stack,
                        &values[..3],
                        &[(k, values[3]), (x, values[4])],
                    );
                    let result = crate::call::bind::molt_call_bind(mixed, builder);
                    assert!(obj_from_bits(result).is_none());
                    assert!(!crate::exception_pending(_py));
                    assert_eq!(ReleaseProbes::released(&values), expected, "3.{minor}");
                });
            }
            for bits in [
                mixed,
                positional_names,
                keyword_only_names,
                a,
                k,
                rest,
                kw,
                x,
            ] {
                dec_ref_bits(_py, bits);
            }
            probes.release(_py);
        }
    });
}

#[test]
fn failed_frame_binding_releases_in_initialize_locals_order() {
    let _transaction = crate::test_support::RuntimeTestTransaction::new();
    crate::with_gil_entry_nopanic!(_py, {
        unsafe {
            let probes = ReleaseProbes::new(_py);
            let string = |text: &[u8]| MoltObject::from_ptr(crate::alloc_string(_py, text)).bits();
            let [a, b, c, d] = [&b"a"[..], b"b", b"c", b"d"].map(string);
            let two_names = MoltObject::from_ptr(alloc_tuple(_py, &[a, b])).bits();
            let one_names = MoltObject::from_ptr(alloc_tuple(_py, &[a])).bits();
            let two = metadata_function(
                _py,
                none_of_two as *const (),
                2,
                &[(b"__molt_arg_names__", two_names)],
                false,
            );
            let one = metadata_function(
                _py,
                none_of_one as *const (),
                1,
                &[(b"__molt_arg_names__", one_names)],
                false,
            );
            for (minor, remaining_keywords, surplus_duplicate) in
                [(12, [2, 3, 0, 1], [1, 2, 0]), (14, [2, 3, 1, 0], [1, 2, 0])]
            {
                with_target_minor(_py, minor, || {
                    // two(p1, b=k1, c=k2, d=k3): `c` is unexpected. kw_fail
                    // releases k2 and k3, then the partial frame clears.
                    let values = probes.instances(_py, 4);
                    let builder = last_owner_call(
                        _py,
                        crate::call::bind::arguments::CallForm::Stack,
                        &values[..1],
                        &[(b, values[1]), (c, values[2]), (d, values[3])],
                    );
                    let result = crate::call::bind::molt_call_bind(two, builder);
                    assert!(obj_from_bits(result).is_none());
                    assert!(crate::exception_pending(_py), "the binding error is raised");
                    let _ = crate::molt_exception_clear();
                    assert_eq!(
                        ReleaseProbes::released(&values),
                        remaining_keywords,
                        "3.{minor}"
                    );
                    // one(p1, p2, a=k1): p2 is surplus at once, then `a` is
                    // bound twice and the frame clears.
                    let values = probes.instances(_py, 3);
                    let builder = last_owner_call(
                        _py,
                        crate::call::bind::arguments::CallForm::Stack,
                        &values[..2],
                        &[(a, values[2])],
                    );
                    let result = crate::call::bind::molt_call_bind(one, builder);
                    assert!(obj_from_bits(result).is_none());
                    assert!(crate::exception_pending(_py), "the binding error is raised");
                    let _ = crate::molt_exception_clear();
                    assert_eq!(
                        ReleaseProbes::released(&values),
                        surplus_duplicate,
                        "3.{minor}"
                    );
                });
            }
            for bits in [two, one, two_names, one_names, a, b, c, d] {
                dec_ref_bits(_py, bits);
            }
            probes.release(_py);
        }
    });
}

#[test]
fn expanded_calls_keep_their_containers_through_frame_admission() {
    let _transaction = crate::test_support::RuntimeTestTransaction::new();
    crate::with_gil_entry_nopanic!(_py, {
        unsafe {
            let probes = ReleaseProbes::new(_py);
            let string = |text: &[u8]| MoltObject::from_ptr(crate::alloc_string(_py, text)).bits();
            let [a, b] = [&b"a"[..], b"b"].map(string);
            let no_names = MoltObject::from_ptr(alloc_tuple(_py, &[])).bits();
            let two_names = MoltObject::from_ptr(alloc_tuple(_py, &[a, b])).bits();
            let zero = metadata_function(
                _py,
                none_of_none as *const (),
                0,
                &[(b"__molt_arg_names__", no_names)],
                false,
            );
            let two = metadata_function(
                _py,
                none_of_two as *const (),
                2,
                &[(b"__molt_arg_names__", two_names)],
                false,
            );
            for (minor, success) in [(12, [0, 1]), (13, [0, 1]), (14, [1, 0])] {
                with_target_minor(_py, minor, || {
                    // zero(p1, p2, *(), a=k1, b=k2): binding fails. The
                    // frame's references end first; the tuple (last to
                    // first) and then the mapping hold the last ones, as
                    // `_PyEvalFramePushAndInit_Ex` releases them.
                    let values = probes.instances(_py, 4);
                    let builder = last_owner_call(
                        _py,
                        crate::call::bind::arguments::CallForm::Expanded,
                        &values[..2],
                        &[(a, values[2]), (b, values[3])],
                    );
                    let result = crate::call::bind::molt_call_bind(zero, builder);
                    assert!(obj_from_bits(result).is_none());
                    assert!(crate::exception_pending(_py), "the binding error is raised");
                    let _ = crate::molt_exception_clear();
                    assert_eq!(ReleaseProbes::released(&values), [1, 0, 2, 3], "3.{minor}");
                    // two(*(p1, p2)): admission succeeds, the containers end
                    // before the callee runs, and the frame clears its own
                    // parameters in frame order rather than as a tuple.
                    let values = probes.instances(_py, 2);
                    let builder = last_owner_call(
                        _py,
                        crate::call::bind::arguments::CallForm::Expanded,
                        &values,
                        &[],
                    );
                    let result = crate::call::bind::molt_call_bind(two, builder);
                    assert!(obj_from_bits(result).is_none());
                    assert!(!crate::exception_pending(_py));
                    assert_eq!(ReleaseProbes::released(&values), success, "3.{minor}");
                });
            }
            for bits in [zero, two, no_names, two_names, a, b] {
                dec_ref_bits(_py, bits);
            }
            probes.release(_py);
        }
    });
}
