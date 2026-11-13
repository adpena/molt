use super::*;
use crate::BuiltinExc;
use crate::object::{
    accessors, builders, class_finish_definition, field_storage, gc, layout, ops_builtins,
    seq_access,
};

unsafe fn new_class(_py: &PyToken<'_>, name: &[u8]) -> u64 {
    let name_bits = dynamic_attr_name_bits(_py, name).expect("class name");
    let class_bits = molt_class_new_attached_body(_py, name_bits);
    dec_ref_bits(_py, name_bits);
    assert!(obj_from_bits(class_bits).as_ptr().is_some());
    assert_eq!(
        molt_class_set_base_attached_body(_py, class_bits, builtin_classes(_py).object),
        MoltObject::none().bits()
    );
    assert!(!exception_pending(_py));
    class_bits
}

fn heap_refcount(bits: u64) -> u32 {
    let ptr = obj_from_bits(bits).as_ptr().expect("heap object");
    unsafe { (*crate::object::header_from_obj_ptr(ptr)).ref_count_snapshot() }
}

#[test]
fn ordinary_slot_state_omits_shadowed_storage_but_reset_releases_both_owners() {
    let _transaction = crate::test_support::RuntimeTestTransaction::new();
    crate::with_gil_entry_nopanic!(py, {
        let field = dynamic_attr_name_bits(py, b"x").unwrap();
        let slots_name = dynamic_attr_name_bits(py, b"__slots__").unwrap();
        let offsets_name = dynamic_attr_name_bits(py, b"__molt_field_offsets__").unwrap();
        let size_name = dynamic_attr_name_bits(py, b"__molt_layout_size__").unwrap();
        let slots = MoltObject::from_ptr(alloc_tuple(py, &[field])).bits();
        let make_class = |name: &[u8], base: u64, offset: i64| {
            let name = dynamic_attr_name_bits(py, name).unwrap();
            let class = crate::molt_class_new_attached_body(py, name);
            crate::molt_class_set_base_attached_body(py, class, base);
            let offsets = MoltObject::from_ptr(alloc_dict_with_pairs(
                py,
                &[field, MoltObject::from_int(offset).bits()],
            ))
            .bits();
            crate::molt_set_attr_name_attached_body(py, class, slots_name, slots);
            crate::molt_set_attr_name_attached_body(py, class, offsets_name, offsets);
            crate::molt_set_attr_name_attached_body(
                py,
                class,
                size_name,
                MoltObject::from_int(offset + 16).bits(),
            );
            unsafe { class_finish_definition(py, obj_from_bits(class).as_ptr().unwrap()) }
                .expect("seal distinct physical slot");
            dec_ref_bits(py, offsets);
            dec_ref_bits(py, name);
            class
        };
        let base = make_class(b"ShadowBase", builtin_classes(py).object, 0);
        let child = make_class(b"ShadowChild", base, 8);
        let object = builders::alloc_class_instance(py, 24, child);
        let ptr = obj_from_bits(object).as_ptr().unwrap();
        unsafe { gc::gc_publish_initialized(py, ptr) };
        let base_value = MoltObject::from_ptr(alloc_list(py, &[])).bits();
        let child_value = MoltObject::from_ptr(alloc_list(py, &[])).bits();
        unsafe { accessors::molt_object_field_set_attached_body(py, object, 0, base_value) };
        assert_eq!(
            ops_builtins::molt_object_getstate_attached_body(py, object),
            MoltObject::none().bits(),
            "missing visible slot must not reveal a populated base slot"
        );
        unsafe { accessors::molt_object_field_set_attached_body(py, object, 8, child_value) };
        let state = ops_builtins::molt_object_getstate_attached_body(py, object);
        assert!(!exception_pending(py));
        let slot_state = unsafe {
            seq_access::with_immutable_tuple_slice(
                obj_from_bits(state).as_ptr().unwrap(),
                |parts| parts[1],
            )
            .unwrap()
        };
        assert!(unsafe {
            dict_live_entries(obj_from_bits(slot_state).as_ptr().unwrap())
                .flat_map(|row| [row.key, row.value])
                .eq([field, child_value])
        });
        dec_ref_bits(py, state);
        assert_eq!(heap_refcount(base_value), 2);
        assert_eq!(heap_refcount(child_value), 2);
        unsafe { field_storage::reset(py, ptr) };
        assert!(!exception_pending(py));
        assert_eq!(heap_refcount(base_value), 1);
        assert_eq!(heap_refcount(child_value), 1);
        for bits in [
            object,
            child,
            base,
            slots,
            field,
            slots_name,
            offsets_name,
            size_name,
            base_value,
            child_value,
        ] {
            dec_ref_bits(py, bits);
        }
        assert!(!exception_pending(py));
    });
}

static SLOT_STATE_READS: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
static SLOT_STATE_READ_MODE: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);

extern "C" fn slot_state_getattribute(__context: u64, _object: u64, name: u64) -> u64 {
    assert_eq!(
        __context,
        crate::state::molt_attached_context(),
        "Python callback context identity"
    );
    let __py = unsafe { crate::PyToken::from_attached_context(__context) };

    use std::sync::atomic::Ordering;
    {
        let py = __py;
        assert_eq!(
            string_obj_to_owned(obj_from_bits(name)).as_deref(),
            Some("x")
        );
        let count = SLOT_STATE_READS.fetch_add(1, Ordering::SeqCst) + 1;
        match SLOT_STATE_READ_MODE.load(Ordering::SeqCst) {
            1 => raise_exception::<u64>(py, BuiltinExc::AttributeError, "slot absent"),
            2 => raise_exception::<u64>(py, BuiltinExc::RuntimeError, "slot getter failed"),
            _ => MoltObject::from_int(count as i64).bits(),
        }
    }
}

#[test]
fn slot_state_uses_sequential_attribute_protocol_and_propagates_errors() {
    use std::sync::atomic::Ordering;
    let _transaction = crate::test_support::RuntimeTestTransaction::new();
    crate::with_gil_entry_nopanic!(py, {
        let field = dynamic_attr_name_bits(py, b"x").unwrap();
        let slots_name = dynamic_attr_name_bits(py, b"__slots__").unwrap();
        let getter_name = dynamic_attr_name_bits(py, b"__getattribute__").unwrap();
        let slots = MoltObject::from_ptr(alloc_tuple(py, &[field])).bits();
        let base = unsafe { new_class(py, b"StateProtocolBase") };
        crate::molt_set_attr_name_attached_body(py, base, slots_name, slots);
        unsafe {
            class_finish_definition(py, obj_from_bits(base).as_ptr().unwrap()).unwrap();
        }
        let child_name = dynamic_attr_name_bits(py, b"StateProtocolChild").unwrap();
        let child = crate::molt_class_new_attached_body(py, child_name);
        crate::molt_class_set_base_attached_body(py, child, base);
        crate::molt_set_attr_name_attached_body(py, child, slots_name, slots);
        let getter = MoltObject::from_ptr(alloc_runtime_function_obj(
            py,
            runtime_fn_addr(
                "slot_state_getattribute",
                slot_state_getattribute as *const (),
            ),
            2,
        ))
        .bits();
        crate::molt_set_attr_name_attached_body(py, child, getter_name, getter);
        let class = obj_from_bits(child).as_ptr().unwrap();
        unsafe {
            class_finish_definition(py, class).unwrap();
        }
        let size = unsafe { layout::class_cached_layout_size(class).unwrap() };
        let object = builders::alloc_class_instance(py, size, child);
        unsafe {
            gc::gc_publish_initialized(py, obj_from_bits(object).as_ptr().unwrap());
        }
        for mode in 0..3 {
            SLOT_STATE_READS.store(0, Ordering::SeqCst);
            SLOT_STATE_READ_MODE.store(mode, Ordering::SeqCst);
            let state = ops_builtins::molt_object_getstate_attached_body(py, object);
            assert_eq!(
                SLOT_STATE_READS.load(Ordering::SeqCst),
                if mode == 2 { 1 } else { 2 }
            );
            if mode == 0 {
                let slots = unsafe {
                    seq_access::with_immutable_tuple_slice(
                        obj_from_bits(state).as_ptr().unwrap(),
                        |parts| parts[1],
                    )
                    .unwrap()
                };
                assert_eq!(
                    unsafe {
                        dict_get_in_place(py, obj_from_bits(slots).as_ptr().unwrap(), field)
                            .unwrap()
                    },
                    Some(MoltObject::from_int(2).bits())
                );
            } else {
                assert_eq!(state, MoltObject::none().bits());
            }
            assert_eq!(exception_pending(py), mode == 2);
            if mode == 2 {
                crate::molt_exception_clear_attached_body(py);
            }
            dec_ref_bits(py, state);
        }
        for bits in [
            object,
            child,
            child_name,
            base,
            getter,
            getter_name,
            slots,
            slots_name,
            field,
        ] {
            dec_ref_bits(py, bits);
        }
        assert!(!exception_pending(py));
    });
}

#[test]
fn identity_dict_preserves_allocation_failure_and_input_ownership() {
    use crate::resource::{LimitedTracker, ResourceLimits, set_tracker};
    let _transaction = crate::test_support::RuntimeTestTransaction::new();
    assert!(crate::cpython_abi_hooks::register_cpython_hooks());
    crate::with_gil_entry_nopanic!(py, {
        let value = MoltObject::from_ptr(alloc_string(py, b"snapshot owned value")).bits();
        for tuple in [false, true] {
            let source = if tuple {
                alloc_tuple(py, &[value])
            } else {
                alloc_list(py, &[value])
            };
            let source = MoltObject::from_ptr(source).bits();
            let refs = heap_refcount(value);
            {
                let reset = crate::test_support::restore_unlimited_tracker_on_drop();
                set_tracker(Box::new(LimitedTracker::new(&ResourceLimits {
                    max_allocations: Some(0),
                    ..ResourceLimits::default()
                })));
                // Lists fail at snapshot capture; immutable tuple pins succeed,
                // then destination construction encounters the exhausted budget.
                let out = crate::molt_codecs_make_identity_dict_attached_body(py, source);
                assert!(obj_from_bits(out).is_none());
                assert_eq!(heap_refcount(value), refs);
                crate::test_support::assert_and_clear_emergency_memory_error(py);
                drop(reset);
            }
            dec_ref_bits(py, source);
        }
        dec_ref_bits(py, value);
        assert!(!exception_pending(py));
    });
}

// Exercise actual producers under the normal resource authority. The independent
// capacity probe after each denial detects retained partial results without
// reading freed objects or adding an allocator-specific failure hook.
fn assert_owned_tuple_parts(
    py: &PyToken<'_>,
    label: &str,
    allocations: usize,
    owned_slots: &[usize],
    operation: impl Fn() -> u64,
    inspect: impl Fn(&[u64]),
) {
    let result = operation();
    assert!(!exception_pending(py), "{label}: successful preparation");
    let ptr = obj_from_bits(result).as_ptr().expect("tuple result");
    unsafe {
        seq_access::with_immutable_tuple_slice(ptr, |items| {
            inspect(items);
            for &index in owned_slots {
                assert_eq!(heap_refcount(items[index]), 1, "{label}: slot {index}");
            }
        })
        .expect("exact tuple");
    }
    dec_ref_bits(py, result);
    assert_owned_construction_refusal(py, label, allocations, operation);
}

fn assert_owned_construction_refusal(
    py: &PyToken<'_>,
    label: &str,
    allocations: usize,
    operation: impl Fn() -> u64,
) {
    use crate::resource::{LimitedTracker, ResourceLimits, set_tracker};
    for budget in 0..allocations {
        let reset = crate::test_support::restore_unlimited_tracker_on_drop();
        set_tracker(Box::new(LimitedTracker::new(&ResourceLimits {
            max_allocations: Some(budget),
            ..ResourceLimits::default()
        })));
        let result = operation();
        assert!(obj_from_bits(result).is_none(), "{label}: budget {budget}");
        crate::test_support::assert_and_clear_emergency_memory_error(py);

        let mut probes = Vec::new();
        for index in 0..budget {
            let bits = int_bits_from_bigint(py, (BigInt::from(1u8) << 100) + index);
            assert!(
                !exception_pending(py),
                "{label}: leaked capacity at {budget}/{index}"
            );
            assert!(obj_from_bits(bits).as_ptr().is_some());
            probes.push(bits);
        }
        let refused = int_bits_from_bigint(py, BigInt::from(1u8) << 100);
        assert!(obj_from_bits(refused).is_none());
        crate::test_support::assert_and_clear_emergency_memory_error(py);
        for bits in probes.into_iter().rev() {
            dec_ref_bits(py, bits);
        }
        drop(reset);
    }
}

#[test]
fn owned_range_parts_constructor_and_subscription_transfer_and_retire() {
    let _transaction = crate::test_support::RuntimeTestTransaction::new();
    crate::with_gil_entry_nopanic!(py, {
        let start: BigInt = (BigInt::from(1u8) << 80) + 3;
        let stop: BigInt = (BigInt::from(1u8) << 150) + 7;
        let step: BigInt = (BigInt::from(1u8) << 70) + 11;
        let construct = || {
            crate::object::ops::alloc_range_from_bigints(
                py,
                start.clone(),
                stop.clone(),
                step.clone(),
            )
        };
        let range = construct();
        assert!(!exception_pending(py));
        let range_ptr = obj_from_bits(range).as_ptr().expect("range result");
        assert_eq!(
            crate::object::ops::range_components_bigint(range_ptr),
            Some((start.clone(), stop.clone(), step.clone()))
        );
        for field in unsafe {
            [
                range_start_bits(range_ptr),
                range_stop_bits(range_ptr),
                range_step_bits(range_ptr),
            ]
        } {
            assert_eq!(heap_refcount(field), 1, "range owns each boxed part once");
        }
        assert_owned_construction_refusal(py, "range constructor", 4, construct);

        let slice_stop = int_bits_from_bigint(py, BigInt::from(1u8) << 40);
        let slice_step = int_bits_from_bigint(py, (BigInt::from(1u8) << 70) + 5);
        let slice = molt_slice_new_attached_body(
            py,
            MoltObject::from_int(2).bits(),
            slice_stop,
            slice_step,
        );
        assert!(!exception_pending(py));
        let expected = (
            &start + &step * 2,
            &start + &step * (BigInt::from(1u8) << 40),
            &step * ((BigInt::from(1u8) << 70) + 5),
        );
        for builtin in [false, true] {
            let subscribe = || {
                if builtin {
                    crate::object::ops::molt_getitem_builtin_attached_body(py, range, slice)
                } else {
                    crate::object::ops::molt_index_attached_body(py, range, slice)
                }
            };
            let result = subscribe();
            assert!(!exception_pending(py));
            let result_ptr = obj_from_bits(result).as_ptr().expect("sliced range");
            assert_eq!(
                crate::object::ops::range_components_bigint(result_ptr),
                Some(expected.clone())
            );
            for field in unsafe {
                [
                    range_start_bits(result_ptr),
                    range_stop_bits(result_ptr),
                    range_step_bits(result_ptr),
                ]
            } {
                assert_eq!(heap_refcount(field), 1);
            }
            dec_ref_bits(py, result);
            assert_owned_construction_refusal(py, "range subscription", 4, subscribe);
        }
        for bits in [slice, slice_step, slice_stop, range] {
            dec_ref_bits(py, bits);
        }
        assert!(!exception_pending(py));
    });
}

#[test]
fn owned_tuple_parts_slice_reduction_and_indices_transfer_and_retire() {
    let _transaction = crate::test_support::RuntimeTestTransaction::new();
    crate::with_gil_entry_nopanic!(py, {
        let start = int_bits_from_bigint(py, (BigInt::from(1u8) << 80) + 1);
        let stop = int_bits_from_bigint(py, (BigInt::from(1u8) << 81) + 3);
        let step = int_bits_from_bigint(py, (BigInt::from(1u8) << 70) + 5);
        let length = int_bits_from_bigint(py, BigInt::from(1u8) << 90);
        let slice = molt_slice_new_attached_body(py, start, stop, step);
        assert!(!exception_pending(py));
        let counts = [
            heap_refcount(start),
            heap_refcount(stop),
            heap_refcount(step),
        ];
        for extended in [false, true] {
            assert_owned_tuple_parts(
                py,
                "slice reduction",
                2,
                &[1],
                || {
                    if extended {
                        molt_slice_reduce_ex_attached_body(
                            py,
                            slice,
                            MoltObject::from_int(4).bits(),
                        )
                    } else {
                        molt_slice_reduce_attached_body(py, slice)
                    }
                },
                |items| {
                    assert_eq!(items.len(), 2);
                    assert_eq!(items[0], builtin_classes(py).slice);
                    unsafe {
                        seq_access::with_immutable_tuple_slice(
                            obj_from_bits(items[1]).as_ptr().unwrap(),
                            |args| assert_eq!(args, &[start, stop, step]),
                        )
                        .unwrap();
                    }
                },
            );
            assert_eq!(
                [
                    heap_refcount(start),
                    heap_refcount(stop),
                    heap_refcount(step)
                ],
                counts
            );
        }
        assert_owned_tuple_parts(
            py,
            "slice indices",
            4,
            &[0, 1, 2],
            || molt_slice_indices_attached_body(py, slice, length),
            |items| {
                assert_eq!(items.len(), 3);
                let expected = [
                    (BigInt::from(1u8) << 80) + 1,
                    (BigInt::from(1u8) << 81) + 3,
                    (BigInt::from(1u8) << 70) + 5,
                ];
                for (&actual, expected) in items.iter().zip(expected) {
                    assert_eq!(
                        crate::builtins::numbers::index_bigint_integral_bits(actual),
                        Some(expected)
                    );
                }
            },
        );
        assert_eq!(
            [
                heap_refcount(start),
                heap_refcount(stop),
                heap_refcount(step)
            ],
            counts
        );
        for bits in [slice, length, step, stop, start] {
            dec_ref_bits(py, bits);
        }
    });
}

#[test]
fn owned_tuple_parts_ratios_and_divmod_transfer_and_retire() {
    let _transaction = crate::test_support::RuntimeTestTransaction::new();
    crate::with_gil_entry_nopanic!(py, {
        assert_owned_tuple_parts(
            py,
            "float ratio",
            3,
            &[0, 1],
            || {
                crate::molt_float_as_integer_ratio_attached_body(
                    py,
                    MoltObject::from_float(0.5000000000000001).bits(),
                )
            },
            |items| {
                assert_eq!(items.len(), 2);
                for (&actual, expected) in
                    items.iter().zip([4503599627370497_i64, 9007199254740992])
                {
                    assert_eq!(
                        crate::builtins::numbers::index_bigint_integral_bits(actual),
                        Some(BigInt::from(expected))
                    );
                }
            },
        );
        let i64_input = int_bits_from_i128(py, (1i128 << 62) + 1);
        assert_owned_tuple_parts(
            py,
            "i64 divmod",
            2,
            &[0],
            || {
                ops_builtins::molt_divmod_builtin_attached_body(
                    py,
                    i64_input,
                    MoltObject::from_int(3).bits(),
                )
            },
            |items| {
                assert_eq!(items.len(), 2);
                assert_eq!(
                    crate::builtins::numbers::index_bigint_integral_bits(items[0]),
                    Some(BigInt::from(1537228672809129301_i64))
                );
                assert_eq!(obj_from_bits(items[1]).as_int(), Some(2));
            },
        );
        let left = int_bits_from_bigint(py, (BigInt::from(1u8) << 160) + (BigInt::from(1u8) << 79));
        let right = int_bits_from_bigint(py, (BigInt::from(1u8) << 80) + 3);
        assert_owned_tuple_parts(
            py,
            "bigint divmod",
            3,
            &[0, 1],
            || ops_builtins::molt_divmod_builtin_attached_body(py, left, right),
            |items| {
                assert_eq!(items.len(), 2);
                let expected = [(BigInt::from(1u8) << 80) - 3, (BigInt::from(1u8) << 79) + 9];
                for (&actual, expected) in items.iter().zip(expected) {
                    assert_eq!(
                        crate::builtins::numbers::index_bigint_integral_bits(actual),
                        Some(expected)
                    );
                }
            },
        );
        // A float is its own word (HF-833): the NaN parts are inline, so the
        // result tuple is the only allocation and owns no heap part.
        let nan = crate::object::ops::float_result_bits(py, f64::NAN);
        assert_owned_tuple_parts(
            py,
            "NaN divmod",
            1,
            &[],
            || {
                ops_builtins::molt_divmod_builtin_attached_body(
                    py,
                    nan,
                    MoltObject::from_float(3.0).bits(),
                )
            },
            |items| {
                assert_eq!(items.len(), 2);
                for &bits in items {
                    assert!(
                        crate::object::ops::as_float_extended(obj_from_bits(bits))
                            .unwrap()
                            .is_nan()
                    );
                }
            },
        );
        // int.as_integer_ratio intentionally borrows its existing integer.
        let before = heap_refcount(left);
        let ratio = crate::molt_int_as_integer_ratio_attached_body(py, left);
        assert!(!exception_pending(py));
        unsafe {
            seq_access::with_immutable_tuple_slice(
                obj_from_bits(ratio).as_ptr().unwrap(),
                |items| {
                    assert_eq!(items, &[left, MoltObject::from_int(1).bits()]);
                },
            )
            .unwrap();
        }
        assert_eq!(heap_refcount(left), before + 1);
        dec_ref_bits(py, ratio);
        assert_eq!(heap_refcount(left), before);
        for bits in [nan, right, left, i64_input] {
            dec_ref_bits(py, bits);
        }
        assert!(!exception_pending(py));
    });
}
