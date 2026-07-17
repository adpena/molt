use super::*;
use crate::resource::{LimitedTracker, ResourceLimits, UnlimitedTracker, set_tracker};
use std::sync::atomic::{AtomicU64, AtomicUsize, Ordering};

static FAILING_ITER_VALUE: AtomicU64 = AtomicU64::new(0);
static FAILING_ITER_CALLS: AtomicUsize = AtomicUsize::new(0);

extern "C" fn yield_heap_value_then_raise() -> u64 {
    crate::with_gil_entry_nopanic!(_py, {
        if FAILING_ITER_CALLS.fetch_add(1, Ordering::Relaxed) == 0 {
            let bits = FAILING_ITER_VALUE.load(Ordering::Relaxed);
            inc_ref_bits(_py, bits);
            bits
        } else {
            raise_exception::<u64>(_py, "LookupError", "injected iterator failure")
        }
    })
}

extern "C" fn yield_heap_value_then_sentinel() -> u64 {
    crate::with_gil_entry_nopanic!(_py, {
        if FAILING_ITER_CALLS.fetch_add(1, Ordering::Relaxed) == 0 {
            let bits = FAILING_ITER_VALUE.load(Ordering::Relaxed);
            inc_ref_bits(_py, bits);
            bits
        } else {
            MoltObject::from_int(777).bits()
        }
    })
}

extern "C" fn yield_raise_yield_then_sentinel() -> u64 {
    crate::with_gil_entry_nopanic!(_py, {
        match FAILING_ITER_CALLS.fetch_add(1, Ordering::Relaxed) {
            0 | 2 => {
                let bits = FAILING_ITER_VALUE.load(Ordering::Relaxed);
                inc_ref_bits(_py, bits);
                bits
            }
            1 => raise_exception::<u64>(_py, "LookupError", "injected iterator failure"),
            _ => MoltObject::from_int(777).bits(),
        }
    })
}

fn alloc_test_call_iterator(_py: &PyToken<'_>, target: extern "C" fn() -> u64) -> u64 {
    let callable_ptr = crate::object::builders::alloc_function_obj(
        _py,
        crate::provenance::abi::expose_function_address(target as *const ()),
        0,
    );
    assert!(!callable_ptr.is_null());
    unsafe {
        crate::object::layout::function_set_call_target_ptr(callable_ptr, target as *const ());
    }
    let callable_bits = MoltObject::from_ptr(callable_ptr).bits();
    let iter_bits = crate::molt_iter_sentinel(callable_bits, MoltObject::from_int(777).bits());
    dec_ref_bits(_py, callable_bits);
    assert!(!obj_from_bits(iter_bits).is_none());
    iter_bits
}

struct TrackerReset;

impl Drop for TrackerReset {
    fn drop(&mut self) {
        set_tracker(Box::new(UnlimitedTracker));
    }
}

#[test]
fn failed_unpack_initializes_every_result_slot_to_none() {
    let _lock = crate::test_mutex_guard();
    let mut outputs = [u64::MAX, u64::MAX, u64::MAX];
    let result = unsafe {
        molt_unpack_sequence(
            MoltObject::from_int(7).bits(),
            outputs.len() as u64,
            crate::provenance::abi::expose_address(outputs.as_mut_ptr()),
        )
    };
    assert_eq!(result, MoltObject::none().bits());
    assert_eq!(outputs, [MoltObject::none().bits(); 3]);
    assert_eq!(crate::molt_exception_pending(), 1);
    let _ = crate::molt_exception_clear();
}

#[test]
fn preexisting_exception_still_initializes_every_result_slot() {
    let _lock = crate::test_mutex_guard();
    crate::with_gil_entry_nopanic!(_py, {
        let _: u64 = raise_exception(_py, "RuntimeError", "prior failure");
        let mut outputs = [u64::MAX, u64::MAX];
        let result = unsafe {
            molt_unpack_sequence(
                MoltObject::none().bits(),
                outputs.len() as u64,
                crate::provenance::abi::expose_address(outputs.as_mut_ptr()),
            )
        };
        assert_eq!(result, MoltObject::none().bits());
        assert_eq!(outputs, [MoltObject::none().bits(); 2]);
        assert_eq!(crate::molt_exception_pending(), 1);
        let _ = crate::molt_exception_clear();
    });
}

#[test]
fn zero_target_unpack_validates_arity_without_output_memory() {
    let _lock = crate::test_mutex_guard();
    crate::with_gil_entry_nopanic!(_py, {
        let empty_ptr = crate::alloc_tuple(_py, &[]);
        let empty_bits = MoltObject::from_ptr(empty_ptr).bits();
        let result = unsafe { molt_unpack_sequence(empty_bits, 0, 0) };
        assert_eq!(result, 0);
        assert_eq!(crate::molt_exception_pending(), 0);
        dec_ref_bits(_py, empty_bits);

        let item = MoltObject::from_int(1).bits();
        let one_ptr = crate::alloc_tuple(_py, &[item]);
        let one_bits = MoltObject::from_ptr(one_ptr).bits();
        let result = unsafe { molt_unpack_sequence(one_bits, 0, 0) };
        assert_eq!(result, MoltObject::none().bits());
        assert_eq!(crate::molt_exception_pending(), 1);
        let _ = crate::molt_exception_clear();
        dec_ref_bits(_py, one_bits);
    });
}

#[test]
fn nonzero_unpack_with_null_output_raises_memory_error_before_iteration() {
    let _lock = crate::test_mutex_guard();
    crate::with_gil_entry_nopanic!(_py, {
        let source_ptr = crate::alloc_tuple(_py, &[MoltObject::from_int(1).bits()]);
        let source_bits = MoltObject::from_ptr(source_ptr).bits();
        let result = unsafe { molt_unpack_sequence(source_bits, 1, 0) };
        assert_eq!(result, MoltObject::none().bits());
        assert_eq!(crate::molt_exception_pending(), 1);
        let exc_bits = crate::builtins::exceptions::molt_exception_last_pending();
        let exc_ptr = obj_from_bits(exc_bits)
            .as_ptr()
            .expect("MemoryError must be an exception object");
        let message = crate::format_exception_message(_py, exc_ptr);
        assert!(message.contains("sequence unpack result allocation failed"));
        let _ = crate::molt_exception_clear();
        dec_ref_bits(_py, exc_bits);
        dec_ref_bits(_py, source_bits);
    });
}

#[test]
fn exact_builtin_unpack_is_allocation_free_after_source_construction() {
    let _lock = crate::test_mutex_guard();
    crate::with_gil_entry_nopanic!(_py, {
        let values = [
            MoltObject::from_int(11).bits(),
            MoltObject::from_int(13).bits(),
        ];
        let tuple_bits = MoltObject::from_ptr(crate::alloc_tuple(_py, &values)).bits();
        let list_bits = MoltObject::from_ptr(crate::alloc_list(_py, &values)).bits();

        set_tracker(Box::new(LimitedTracker::new(&ResourceLimits {
            max_memory: Some(0),
            ..Default::default()
        })));
        let reset = TrackerReset;

        for source_bits in [tuple_bits, list_bits] {
            let mut outputs = [u64::MAX; 2];
            let result = unsafe {
                molt_unpack_sequence(
                    source_bits,
                    outputs.len() as u64,
                    crate::provenance::abi::expose_address(outputs.as_mut_ptr()),
                )
            };
            assert_eq!(result, 0);
            assert_eq!(outputs, values);
            assert_eq!(crate::molt_exception_pending(), 0);
        }

        drop(reset);
        dec_ref_bits(_py, tuple_bits);
        dec_ref_bits(_py, list_bits);
    });
}

#[test]
fn exact_builtin_unpack_mints_and_releases_one_owner_per_heap_output() {
    let _lock = crate::test_mutex_guard();
    crate::with_gil_entry_nopanic!(_py, {
        let first_bits = MoltObject::from_ptr(crate::alloc_list(_py, &[])).bits();
        let second_bits = MoltObject::from_ptr(crate::alloc_list(_py, &[])).bits();
        let source_bits =
            MoltObject::from_ptr(crate::alloc_tuple(_py, &[first_bits, second_bits])).bits();
        let first_ptr = obj_from_bits(first_bits).as_ptr().unwrap();
        let second_ptr = obj_from_bits(second_bits).as_ptr().unwrap();
        let refcount =
            |ptr: *mut u8| unsafe { (*crate::header_from_obj_ptr(ptr)).ref_count_snapshot() };
        let before = [refcount(first_ptr), refcount(second_ptr)];
        let mut outputs = [0; 2];
        assert_eq!(
            unsafe {
                molt_unpack_sequence(
                    source_bits,
                    2,
                    crate::provenance::abi::expose_address(outputs.as_mut_ptr()),
                )
            },
            0
        );
        assert_eq!(outputs, [first_bits, second_bits]);
        assert_eq!(refcount(first_ptr), before[0] + 1);
        assert_eq!(refcount(second_ptr), before[1] + 1);
        dec_ref_bits(_py, outputs[0]);
        dec_ref_bits(_py, outputs[1]);
        assert_eq!(refcount(first_ptr), before[0]);
        assert_eq!(refcount(second_ptr), before[1]);
        dec_ref_bits(_py, source_bits);
        dec_ref_bits(_py, first_bits);
        dec_ref_bits(_py, second_bits);
    });
}

#[test]
fn generic_unpack_rolls_back_heap_outputs_when_iteration_raises() {
    let _lock = crate::test_mutex_guard();
    crate::with_gil_entry_nopanic!(_py, {
        let value_bits = MoltObject::from_ptr(crate::alloc_list(_py, &[])).bits();
        let value_ptr = obj_from_bits(value_bits).as_ptr().unwrap();
        let refcount = || unsafe { (*crate::header_from_obj_ptr(value_ptr)).ref_count_snapshot() };
        let baseline = refcount();

        FAILING_ITER_VALUE.store(value_bits, Ordering::Relaxed);
        FAILING_ITER_CALLS.store(0, Ordering::Relaxed);
        let iter_bits = alloc_test_call_iterator(_py, yield_heap_value_then_raise);

        let mut outputs = [u64::MAX; 2];
        assert_eq!(
            unsafe {
                molt_unpack_sequence(
                    iter_bits,
                    outputs.len() as u64,
                    crate::provenance::abi::expose_address(outputs.as_mut_ptr()),
                )
            },
            MoltObject::none().bits()
        );
        assert_eq!(outputs, [MoltObject::none().bits(); 2]);
        assert_eq!(
            refcount(),
            baseline,
            "published heap owner must be rolled back"
        );
        assert_eq!(crate::molt_exception_pending(), 1);
        let exc_bits = crate::builtins::exceptions::molt_exception_last_pending();
        let exc_ptr = obj_from_bits(exc_bits)
            .as_ptr()
            .expect("LookupError must remain pending");
        assert!(
            crate::format_exception_message(_py, exc_ptr).contains("injected iterator failure")
        );
        let _ = crate::molt_exception_clear();
        dec_ref_bits(_py, exc_bits);

        dec_ref_bits(_py, iter_bits);
        FAILING_ITER_VALUE.store(0, Ordering::Relaxed);
        dec_ref_bits(_py, value_bits);
    });
}

#[test]
fn callable_iterator_exhaustion_clears_its_cached_heap_edge() {
    let _lock = crate::test_mutex_guard();
    crate::with_gil_entry_nopanic!(_py, {
        let value_bits = MoltObject::from_ptr(crate::alloc_list(_py, &[])).bits();
        let value_ptr = obj_from_bits(value_bits).as_ptr().unwrap();
        let refcount = || unsafe { (*crate::header_from_obj_ptr(value_ptr)).ref_count_snapshot() };
        let baseline = refcount();
        FAILING_ITER_VALUE.store(value_bits, Ordering::Relaxed);
        FAILING_ITER_CALLS.store(0, Ordering::Relaxed);
        let iter_bits = alloc_test_call_iterator(_py, yield_heap_value_then_sentinel);

        let mut output = MoltObject::none().bits();
        let done = unsafe {
            crate::molt_iter_next_unboxed(
                iter_bits,
                crate::provenance::abi::expose_address(&raw mut output),
            )
        };
        assert!(!is_truthy(_py, obj_from_bits(done)));
        assert_eq!(output, value_bits);
        dec_ref_bits(_py, output);
        assert_eq!(refcount(), baseline + 1, "live cache owns one edge");

        let done = unsafe {
            crate::molt_iter_next_unboxed(
                iter_bits,
                crate::provenance::abi::expose_address(&raw mut output),
            )
        };
        assert!(is_truthy(_py, obj_from_bits(done)));
        assert_eq!(output, MoltObject::none().bits());
        assert_eq!(refcount(), baseline, "exhaustion must clear the cache edge");

        dec_ref_bits(_py, iter_bits);
        FAILING_ITER_VALUE.store(0, Ordering::Relaxed);
        dec_ref_bits(_py, value_bits);
    });
}

#[test]
fn callable_iterator_exception_clears_cache_before_reentry() {
    let _lock = crate::test_mutex_guard();
    crate::with_gil_entry_nopanic!(_py, {
        let value_bits = MoltObject::from_ptr(crate::alloc_list(_py, &[])).bits();
        let value_ptr = obj_from_bits(value_bits).as_ptr().unwrap();
        let refcount = || unsafe { (*crate::header_from_obj_ptr(value_ptr)).ref_count_snapshot() };
        let baseline = refcount();
        FAILING_ITER_VALUE.store(value_bits, Ordering::Relaxed);
        FAILING_ITER_CALLS.store(0, Ordering::Relaxed);
        let iter_bits = alloc_test_call_iterator(_py, yield_raise_yield_then_sentinel);
        let mut output = MoltObject::none().bits();

        let first = unsafe {
            crate::molt_iter_next_unboxed(
                iter_bits,
                crate::provenance::abi::expose_address(&raw mut output),
            )
        };
        assert!(!is_truthy(_py, obj_from_bits(first)));
        dec_ref_bits(_py, output);
        assert_eq!(refcount(), baseline + 1);

        let failed = unsafe {
            crate::molt_iter_next_unboxed(
                iter_bits,
                crate::provenance::abi::expose_address(&raw mut output),
            )
        };
        assert_eq!(failed, MoltObject::none().bits());
        assert_eq!(output, MoltObject::none().bits());
        assert_eq!(
            refcount(),
            baseline,
            "error propagation must clear the cache"
        );
        assert_eq!(crate::molt_exception_pending(), 1);
        let _ = crate::molt_exception_clear();

        let resumed = unsafe {
            crate::molt_iter_next_unboxed(
                iter_bits,
                crate::provenance::abi::expose_address(&raw mut output),
            )
        };
        assert!(!is_truthy(_py, obj_from_bits(resumed)));
        assert_eq!(output, value_bits);
        dec_ref_bits(_py, output);
        assert_eq!(
            refcount(),
            baseline + 1,
            "re-entry may establish one new cache edge"
        );

        let exhausted = unsafe {
            crate::molt_iter_next_unboxed(
                iter_bits,
                crate::provenance::abi::expose_address(&raw mut output),
            )
        };
        assert!(is_truthy(_py, obj_from_bits(exhausted)));
        assert_eq!(refcount(), baseline);
        dec_ref_bits(_py, iter_bits);
        FAILING_ITER_VALUE.store(0, Ordering::Relaxed);
        dec_ref_bits(_py, value_bits);
    });
}

#[test]
fn callable_iterator_teardown_clears_cache_after_early_exit() {
    let _lock = crate::test_mutex_guard();
    crate::with_gil_entry_nopanic!(_py, {
        let value_bits = MoltObject::from_ptr(crate::alloc_list(_py, &[])).bits();
        let value_ptr = obj_from_bits(value_bits).as_ptr().unwrap();
        let refcount = || unsafe { (*crate::header_from_obj_ptr(value_ptr)).ref_count_snapshot() };
        let baseline = refcount();
        FAILING_ITER_VALUE.store(value_bits, Ordering::Relaxed);
        FAILING_ITER_CALLS.store(0, Ordering::Relaxed);
        let iter_bits = alloc_test_call_iterator(_py, yield_heap_value_then_sentinel);
        let mut output = MoltObject::none().bits();

        let done = unsafe {
            crate::molt_iter_next_unboxed(
                iter_bits,
                crate::provenance::abi::expose_address(&raw mut output),
            )
        };
        assert!(!is_truthy(_py, obj_from_bits(done)));
        dec_ref_bits(_py, output);
        assert_eq!(refcount(), baseline + 1);
        dec_ref_bits(_py, iter_bits);
        assert_eq!(
            refcount(),
            baseline,
            "iterator teardown must drain its cache owner"
        );

        FAILING_ITER_VALUE.store(0, Ordering::Relaxed);
        dec_ref_bits(_py, value_bits);
    });
}

#[cfg(feature = "l7-attestation-probe")]
#[test]
fn exact_builtin_unpack_hot_path_performs_zero_allocator_calls() {
    let _lock = crate::test_mutex_guard();
    crate::with_gil_entry_nopanic!(_py, {
        let values = [
            MoltObject::from_int(1).bits(),
            MoltObject::from_int(2).bits(),
        ];
        let tuple_bits = MoltObject::from_ptr(crate::alloc_tuple(_py, &values)).bits();
        let list_bits = MoltObject::from_ptr(crate::alloc_list(_py, &values)).bits();
        crate::attestation_probe::reset();
        crate::attestation_probe::set_tracking(true);
        for source in [tuple_bits, list_bits].into_iter().cycle().take(2_000) {
            let mut outputs = [0; 2];
            assert_eq!(
                unsafe {
                    molt_unpack_sequence(
                        source,
                        2,
                        crate::provenance::abi::expose_address(outputs.as_mut_ptr()),
                    )
                },
                0
            );
            for output in outputs {
                dec_ref_bits(_py, output);
            }
        }
        crate::attestation_probe::set_tracking(false);
        let observed = crate::attestation_probe::snapshot();
        assert_eq!(
            observed.allocations, 0,
            "exact unpack hot path must not allocate"
        );
        dec_ref_bits(_py, tuple_bits);
        dec_ref_bits(_py, list_bits);
    });
}
