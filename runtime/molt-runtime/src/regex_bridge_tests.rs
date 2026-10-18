use crate::resource::{LimitedTracker, ResourceLimits, ResourceTracker, UnlimitedTracker};
use crate::*;
use molt_runtime_core::OwnedRuntimeValue;
use molt_runtime_regex::regex::{
    molt_re_compile, molt_re_group_values, molt_re_pattern_info, molt_re_split, molt_re_sub,
};

struct TrackerReset;

impl Drop for TrackerReset {
    fn drop(&mut self) {
        crate::resource::set_tracker(Box::new(UnlimitedTracker));
    }
}

struct LiveAllocationCounter(std::rc::Rc<std::cell::Cell<isize>>);

impl ResourceTracker for LiveAllocationCounter {
    fn on_allocate(&mut self, _size: usize) -> Result<(), crate::resource::ResourceError> {
        self.0.set(self.0.get() + 1);
        Ok(())
    }
    fn on_free(&mut self, _size: usize) {
        self.0.set(self.0.get() - 1);
    }
    fn on_grow(&mut self, _additional_bytes: usize) -> Result<(), crate::resource::ResourceError> {
        Ok(())
    }
    fn on_shrink(&mut self, _released_bytes: usize) {}
    fn check_time(&mut self) -> Result<(), crate::resource::ResourceError> {
        Ok(())
    }
    fn check_recursion_depth(
        &mut self,
        _depth: usize,
    ) -> Result<(), crate::resource::ResourceError> {
        Ok(())
    }
    fn check_operation_size(
        &mut self,
        _op: &crate::resource::OperationEstimate,
    ) -> Result<(), crate::resource::ResourceError> {
        Ok(())
    }
}

#[test]
fn regex_aggregates_retain_each_created_child_once() {
    let _transaction = crate::test_support::RuntimeTestTransaction::new();
    crate::with_gil_entry_nopanic!(py, {
        let zero = MoltObject::from_int(0).bits();
        for byte_mode in [false, true] {
            let make = |data: &[u8]| {
                let ptr = if byte_mode {
                    alloc_bytes(py, data)
                } else {
                    alloc_string(py, data)
                };
                assert!(!ptr.is_null());
                unsafe {
                    OwnedRuntimeValue::from_owned_bits(
                        py.core_token(),
                        MoltObject::from_ptr(ptr).bits(),
                    )
                }
            };
            let source = make(b"left_owner::right_owner");
            let expression = make(b"::");
            let replacement = make(b"replacement_owner");
            let pattern = unsafe {
                OwnedRuntimeValue::from_owned_bits(
                    py.core_token(),
                    molt_re_compile(expression.bits(), zero),
                )
            };
            assert!(!exception_pending(py));

            let split = unsafe {
                OwnedRuntimeValue::from_owned_bits(
                    py.core_token(),
                    molt_re_split(pattern.bits(), source.bits(), zero),
                )
            };
            assert!(!exception_pending(py));
            let children = unsafe { seq_snapshot(ptr_from_bits(split.bits())) };
            assert_eq!(children.len(), 2);
            for &child in children.iter() {
                assert_eq!(
                    unsafe { (*header_from_obj_ptr(ptr_from_bits(child))).ref_count_snapshot() },
                    1
                );
            }

            let replaced = unsafe {
                OwnedRuntimeValue::from_owned_bits(
                    py.core_token(),
                    molt_re_sub(pattern.bits(), replacement.bits(), source.bits(), zero),
                )
            };
            assert!(!exception_pending(py));
            let children = unsafe { seq_snapshot(ptr_from_bits(replaced.bits())) };
            assert_eq!(children.len(), 2);
            assert_eq!(
                unsafe { (*header_from_obj_ptr(ptr_from_bits(children[0]))).ref_count_snapshot() },
                1
            );

            // Independent CPython oracle: opening order z, a, m differs from
            // lexical order and nested-group closing order. The unnamed group
            // leaves an ordinal hole, so names cannot be assigned dense ranks.
            let named_expression = make(b"(?P<z>(a)(?P<a>b))(?P<m>c)?");
            let named_pattern = unsafe {
                OwnedRuntimeValue::from_owned_bits(
                    py.core_token(),
                    molt_re_compile(named_expression.bits(), zero),
                )
            };
            assert!(!exception_pending(py));
            let info = unsafe {
                OwnedRuntimeValue::from_owned_bits(
                    py.core_token(),
                    molt_re_pattern_info(named_pattern.bits()),
                )
            };
            assert!(!exception_pending(py));
            let children = unsafe { seq_snapshot(ptr_from_bits(info.bits())) };
            assert_eq!(children.len(), 4);
            assert_eq!(obj_from_bits(children[0]).as_int(), Some(4));
            assert_eq!(
                unsafe { (*header_from_obj_ptr(ptr_from_bits(children[1]))).ref_count_snapshot() },
                1
            );
            let names = unsafe {
                crate::object::ops_dict::dict_snapshot(
                    py,
                    ptr_from_bits(children[1]),
                    crate::object::ops_dict::DictSnapshotKind::Entries,
                )
            }
            .expect("published named-group dictionary");
            assert_eq!(names.len(), 6);
            for (pair, (name, ordinal)) in names.chunks_exact(2).zip([("z", 1), ("a", 3), ("m", 4)])
            {
                assert_eq!(
                    string_obj_to_owned(obj_from_bits(pair[0])).as_deref(),
                    Some(name)
                );
                assert_eq!(obj_from_bits(pair[1]).as_int(), Some(ordinal));
            }
        }
    });
}

#[test]
fn regex_capture_aggregate_owns_children_and_rolls_back_every_failure() {
    use crate::resource::set_tracker;
    use num_bigint::BigInt;

    let _transaction = crate::test_support::RuntimeTestTransaction::new();
    crate::with_gil_entry_nopanic!(py, {
        for byte_mode in [false, true] {
            let make = |data: &[u8]| {
                let ptr = if byte_mode {
                    alloc_bytes(py, data)
                } else {
                    alloc_string(py, data)
                };
                assert!(!ptr.is_null());
                unsafe {
                    OwnedRuntimeValue::from_owned_bits(
                        py.core_token(),
                        MoltObject::from_ptr(ptr).bits(),
                    )
                }
            };
            let source = make(b"ab");
            let first = unsafe {
                OwnedRuntimeValue::from_owned_bits(
                    py.core_token(),
                    MoltObject::from_ptr(alloc_tuple(
                        py,
                        &[
                            MoltObject::from_int(0).bits(),
                            MoltObject::from_int(1).bits(),
                        ],
                    ))
                    .bits(),
                )
            };
            let second = unsafe {
                OwnedRuntimeValue::from_owned_bits(
                    py.core_token(),
                    MoltObject::from_ptr(alloc_tuple(
                        py,
                        &[
                            MoltObject::from_int(1).bits(),
                            MoltObject::from_int(2).bits(),
                        ],
                    ))
                    .bits(),
                )
            };
            let spans = unsafe {
                OwnedRuntimeValue::from_owned_bits(
                    py.core_token(),
                    MoltObject::from_ptr(alloc_tuple(py, &[first.bits(), second.bits()])).bits(),
                )
            };
            assert!(!exception_pending(py));

            let captured = unsafe {
                OwnedRuntimeValue::from_owned_bits(
                    py.core_token(),
                    molt_re_group_values(source.bits(), spans.bits()),
                )
            };
            assert!(!exception_pending(py));
            let children = unsafe { seq_snapshot(ptr_from_bits(captured.bits())) };
            assert_eq!(children.len(), 2);
            for &child in children.iter() {
                assert_eq!(
                    unsafe { (*header_from_obj_ptr(ptr_from_bits(child))).ref_count_snapshot() },
                    1,
                    "each boxed capture has exactly one tuple owner"
                );
            }
            drop(captured);
            assert!(!exception_pending(py));

            // Warm the exact error-class/message projection before measuring
            // allocation custody, then prove a prior successful capture and
            // the raised error are both retired after a later reversed span.
            let reversed = unsafe {
                OwnedRuntimeValue::from_owned_bits(
                    py.core_token(),
                    MoltObject::from_ptr(alloc_tuple(
                        py,
                        &[
                            MoltObject::from_int(2).bits(),
                            MoltObject::from_int(1).bits(),
                        ],
                    ))
                    .bits(),
                )
            };
            let malformed = unsafe {
                OwnedRuntimeValue::from_owned_bits(
                    py.core_token(),
                    MoltObject::from_ptr(alloc_tuple(py, &[first.bits(), reversed.bits()])).bits(),
                )
            };
            let failed = molt_re_group_values(source.bits(), malformed.bits());
            assert!(obj_from_bits(failed).is_none());
            assert!(exception_pending(py));
            let _ = molt_exception_clear();
            assert!(!exception_pending(py));

            let live = std::rc::Rc::new(std::cell::Cell::new(0isize));
            // The same warmed reversed-span path now runs under a live-owner
            // counter. A nonzero count after clearing the exception means the
            // first boxed capture escaped the aggregate's rollback.
            let reset = TrackerReset;
            set_tracker(Box::new(LiveAllocationCounter(live.clone())));
            let failed = molt_re_group_values(source.bits(), malformed.bits());
            assert!(obj_from_bits(failed).is_none());
            assert!(exception_pending(py));
            let _ = molt_exception_clear();
            assert_eq!(
                live.get(),
                0,
                "partial capture result and error must retire"
            );
            drop(reset);
            assert!(!exception_pending(py));

            // A tuple refusal after both captures were boxed must propagate
            // MemoryError and release both boxes. The remaining two-allocation
            // budget then serves as an independent capacity probe.
            let reset = TrackerReset;
            set_tracker(Box::new(LimitedTracker::new(&ResourceLimits {
                max_allocations: Some(2),
                ..ResourceLimits::default()
            })));
            let failed = molt_re_group_values(source.bits(), spans.bits());
            assert!(obj_from_bits(failed).is_none());
            crate::test_support::assert_and_clear_emergency_memory_error(py);
            let mut probes = Vec::new();
            for index in 0..2 {
                let bits = int_bits_from_bigint(py, (BigInt::from(1u8) << 100) + index);
                assert!(
                    !obj_from_bits(bits).is_none(),
                    "released capture capacity {index}"
                );
                probes.push(bits);
            }
            let refused = int_bits_from_bigint(py, BigInt::from(1u8) << 100);
            assert!(obj_from_bits(refused).is_none());
            crate::test_support::assert_and_clear_emergency_memory_error(py);
            for bits in probes.into_iter().rev() {
                dec_ref_bits(py, bits);
            }
            drop(reset);
            assert!(!exception_pending(py));
        }
    });
}
