//! FFI bridge for object-owned sequence access.
//!
//! Storage synchronization, pinning, and snapshots live in
//! `object::seq_access`; this module only translates those contracts to the
//! satellite C ABI and resource-accounted exported buffers.

use crate::*;

#[unsafe(no_mangle)]
pub extern "C" fn molt_seq_read_len(ptr: *mut u8) -> usize {
    crate::gil_assert();
    unsafe { crate::object::seq_access::len(ptr) }
}

/// Return one GIL-borrowed handle. The caller must keep the sequence alive and
/// consume the result before releasing the runtime GIL.
#[unsafe(no_mangle)]
pub extern "C" fn molt_seq_read_item_gil_borrowed(
    ptr: *mut u8,
    index: usize,
    out: *mut u64,
) -> i32 {
    unsafe { crate::object::seq_access::read_item_gil_borrowed(ptr, index, out) }
}

/// Return one owned handle. A successful result must be released with
/// `molt_dec_ref_obj` by the caller.
#[unsafe(no_mangle)]
pub extern "C" fn molt_seq_read_item_owned(ptr: *mut u8, index: usize, out: *mut u64) -> i32 {
    crate::object::seq_access::read_item_owned(ptr, index, out)
}

/// Export the canonical pinned sequence snapshot for every satellite runtime
/// crate. This symbol is intentionally feature-independent: Tk, itertools,
/// and future consumers must not acquire link custody from one another's
/// optional feature bridge.
#[unsafe(no_mangle)]
pub extern "C" fn molt_seq_snapshot(
    ptr: *mut u8,
    out_ptr: *mut *const u64,
    out_len: *mut usize,
) -> i32 {
    crate::with_gil_entry_nopanic!(py, { unsafe { export(py, ptr, out_ptr, out_len) } })
}

/// Export a stable, resource-accounted snapshot. The caller owns one reference
/// to every returned handle and releases the buffer through the bridge
/// allocator after releasing those references.
pub(crate) unsafe fn export(
    py: &PyToken<'_>,
    ptr: *mut u8,
    out_ptr: *mut *const u64,
    out_len: *mut usize,
) -> i32 {
    if ptr.is_null() || out_ptr.is_null() || out_len.is_null() {
        return 0;
    }
    unsafe {
        crate::object::seq_access::with_borrowed(ptr, |values| {
            export_handles(py, values, out_ptr, out_len)
        })
    }
}

/// Export pinned dictionary key/value pairs in insertion order. An empty
/// dictionary succeeds; allocation failure leaves a pending exception.
#[unsafe(no_mangle)]
pub extern "C" fn molt_dict_snapshot(
    ptr: *mut u8,
    out_ptr: *mut *const u64,
    out_len: *mut usize,
) -> i32 {
    crate::with_gil_entry_nopanic!(py, { unsafe { export_dict(py, ptr, out_ptr, out_len) } })
}

pub(crate) unsafe fn export_dict(
    py: &PyToken<'_>,
    ptr: *mut u8,
    out_ptr: *mut *const u64,
    out_len: *mut usize,
) -> i32 {
    if ptr.is_null() || out_ptr.is_null() || out_len.is_null() {
        return 0;
    }
    unsafe {
        *out_ptr = std::ptr::null();
        *out_len = 0;
    }
    let Some(snapshot) = (unsafe {
        crate::object::ops_dict::dict_snapshot(
            py,
            ptr,
            crate::object::ops_dict::DictSnapshotKind::Entries,
        )
    }) else {
        return 0;
    };
    unsafe { export_handles(py, &snapshot, out_ptr, out_len) }
}

unsafe fn export_handles(
    py: &PyToken<'_>,
    values: &[u64],
    out_ptr: *mut *const u64,
    out_len: *mut usize,
) -> i32 {
    let exported =
        unsafe { crate::resource::bridge_buffer::export_u64_slice(values, out_ptr, out_len) };
    if exported == 0 {
        unsafe {
            *out_ptr = std::ptr::null();
            *out_len = 0;
        }
        return crate::abi_return::fail_memory::<crate::abi_return::FailureStatus>(py);
    }
    for &bits in values {
        inc_ref_bits(py, bits);
    }
    exported
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::resource::{LimitedTracker, ResourceLimits, UnlimitedTracker, set_tracker};

    struct TrackerReset;
    impl Drop for TrackerReset {
        fn drop(&mut self) {
            set_tracker(Box::new(UnlimitedTracker));
        }
    }

    fn owners(bits: u64) -> u32 {
        unsafe {
            (*header_from_obj_ptr(obj_from_bits(bits).as_ptr().unwrap())).ref_count_snapshot()
        }
    }

    #[test]
    fn sequence_bridge_empty_failure_and_owned_success_have_distinct_statuses() {
        let _transaction = crate::test_support::RuntimeTestTransaction::new();
        assert!(crate::cpython_abi_hooks::register_cpython_hooks());
        crate::with_gil_entry_nopanic!(py, {
            let value_ptr = alloc_list(py, &[]);
            assert!(!value_ptr.is_null());
            let value = MoltObject::from_ptr(value_ptr).bits();
            // Exercise both the mutable-list and immutable-tuple access paths.
            for tuple in [false, true] {
                let empty = if tuple {
                    alloc_tuple(py, &[])
                } else {
                    alloc_list(py, &[])
                };
                assert!(!empty.is_null());
                let mut ptr = std::ptr::dangling();
                let mut len = 99;
                let reset = TrackerReset;
                set_tracker(Box::new(LimitedTracker::new(&ResourceLimits {
                    max_allocations: Some(0),
                    ..ResourceLimits::default()
                })));
                let result = molt_seq_snapshot(empty, &mut ptr, &mut len);
                drop(reset);
                assert_eq!(result, 1, "empty export does not allocate");
                assert!(ptr.is_null());
                assert_eq!(len, 0);
                assert!(!exception_pending(py));
                dec_ref_bits(py, MoltObject::from_ptr(empty).bits());

                let expected = [value, value];
                let sequence = if tuple {
                    alloc_tuple(py, &expected)
                } else {
                    alloc_list(py, &expected)
                };
                assert!(!sequence.is_null());
                assert_eq!(owners(value), 3);
                let reset = TrackerReset;
                set_tracker(Box::new(LimitedTracker::new(&ResourceLimits {
                    max_allocations: Some(0),
                    ..ResourceLimits::default()
                })));
                ptr = std::ptr::dangling();
                len = 99;
                let result = molt_seq_snapshot(sequence, &mut ptr, &mut len);
                assert_eq!(result, 0);
                assert!(ptr.is_null());
                assert_eq!(len, 0);
                assert_eq!(owners(value), 3, "failed export must not retain handles");
                crate::test_support::assert_and_clear_emergency_memory_error(py);
                drop(reset);

                assert_eq!(molt_seq_snapshot(sequence, &mut ptr, &mut len), 1);
                let snapshot = unsafe { molt_runtime_core::bridge_owned_handle_snapshot(ptr, len) };
                assert_eq!(&*snapshot, &expected);
                assert_eq!(owners(value), 5);
                dec_ref_bits(py, MoltObject::from_ptr(sequence).bits());
                assert_eq!(owners(value), 3);
                assert_eq!(&*snapshot, &expected);
                drop(snapshot);
                assert_eq!(owners(value), 1);
                assert!(!exception_pending(py));
            }
            dec_ref_bits(py, value);
        });
    }

    #[test]
    fn dictionary_bridge_snapshot_pins_sparse_order_until_consumer_release() {
        let _transaction = crate::test_support::RuntimeTestTransaction::new();
        crate::with_gil_entry_nopanic!(py, {
            let value_ptr = alloc_list(py, &[]);
            assert!(!value_ptr.is_null());
            let value = MoltObject::from_ptr(value_ptr).bits();
            let keys = [1, 2, 3, 4].map(|key| MoltObject::from_int(key).bits());
            let dict = alloc_dict_with_pairs(py, &[keys[0], value, keys[1], value, keys[2], value]);
            assert!(!dict.is_null());
            unsafe {
                assert!(dict_del_in_place(py, dict, keys[1]));
                dict_set_in_place(py, dict, keys[3], value);
            }
            assert!(!exception_pending(py));
            assert_eq!(owners(value), 4);
            let mut ptr = std::ptr::null();
            let mut len = 0;
            assert_eq!(molt_dict_snapshot(dict, &mut ptr, &mut len), 1);
            let snapshot = unsafe { molt_runtime_core::bridge_owned_handle_snapshot(ptr, len) };
            // This literal order distinguishes occupied rows from dense indexing
            // or table-bucket order. The same owner used by satellites releases it.
            assert_eq!(
                &*snapshot,
                &[keys[0], value, keys[2], value, keys[3], value]
            );
            assert_eq!(owners(value), 7);
            dec_ref_bits(py, MoltObject::from_ptr(dict).bits());
            assert_eq!(owners(value), 4);
            assert_eq!(snapshot[3], value);
            drop(snapshot);
            assert_eq!(owners(value), 1);
            dec_ref_bits(py, value);
            assert!(!exception_pending(py));
        });
    }

    #[test]
    fn dictionary_bridge_empty_success_and_both_allocation_failures_are_distinct() {
        let _transaction = crate::test_support::RuntimeTestTransaction::new();
        assert!(crate::cpython_abi_hooks::register_cpython_hooks());
        crate::with_gil_entry_nopanic!(py, {
            let dict = alloc_dict_with_pairs(py, &[]);
            assert!(!dict.is_null());
            let mut ptr = std::ptr::dangling();
            let mut len = 99;
            assert_eq!(molt_dict_snapshot(dict, &mut ptr, &mut len), 1);
            assert!(ptr.is_null());
            assert_eq!(len, 0);
            assert!(!exception_pending(py));
            let value_ptr = alloc_list(py, &[]);
            assert!(!value_ptr.is_null());
            let value = MoltObject::from_ptr(value_ptr).bits();
            unsafe {
                dict_set_in_place(py, dict, MoltObject::from_int(7).bits(), value);
            }
            assert_eq!(owners(value), 2);
            // Zero denies the canonical snapshot reservation; one admits it
            // but denies the independent exported-buffer reservation.
            for admitted_allocations in [0, 1] {
                let reset = TrackerReset;
                set_tracker(Box::new(LimitedTracker::new(&ResourceLimits {
                    max_allocations: Some(admitted_allocations),
                    ..ResourceLimits::default()
                })));
                ptr = std::ptr::dangling();
                len = 99;
                let result = molt_dict_snapshot(dict, &mut ptr, &mut len);
                assert_eq!(result, 0);
                assert!(ptr.is_null());
                assert_eq!(len, 0);
                assert_eq!(owners(value), 2);
                assert_eq!(unsafe { dict_len(dict) }, 1);
                crate::test_support::assert_and_clear_emergency_memory_error(py);
                drop(reset);
            }
            dec_ref_bits(py, MoltObject::from_ptr(dict).bits());
            dec_ref_bits(py, value);
        });
    }
}
