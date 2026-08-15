#[test]
fn c_api_version_is_nonzero() {
    let _guard = CApiTestGuard::new();
    assert!(molt_c_api_version() >= 1);
}

#[test]
fn err_set_matches_fetch_roundtrip() {
    let _guard = CApiTestGuard::new();
    let runtime_error = crate::with_gil_entry_nopanic!(_py, { runtime_error_type_bits(_py) });
    let msg = b"boom";
    let rc = unsafe { molt_err_set(runtime_error, msg.as_ptr(), msg.len() as u64) };
    assert_eq!(rc, 0);
    assert_eq!(molt_exception_pending(), 1);
    assert_eq!(molt_err_matches(runtime_error), 1);
    let exc_bits = molt_err_fetch();
    assert!(!obj_from_bits(exc_bits).is_none());
    assert_eq!(molt_exception_pending(), 0);
    let kind_bits = molt_exception_kind(exc_bits);
    let class_bits = molt_exception_class(kind_bits);
    assert_eq!(molt_err_matches(runtime_error), 0);
    assert!(issubclass_bits(class_bits, runtime_error));
    crate::with_gil_entry_nopanic!(_py, {
        dec_ref_bits(_py, kind_bits);
        dec_ref_bits(_py, class_bits);
        dec_ref_bits(_py, exc_bits);
    });
}

#[test]
#[cfg_attr(
    miri,
    ignore = "Miri strict provenance cannot execute exposed-address dynamic method dispatch until all method cache callables use provenance-registered function keys"
)]
fn object_call_numeric_and_sequence_wrappers() {
    let _guard = CApiTestGuard::new();
    crate::with_gil_entry_nopanic!(_py, {
        let list_ptr = alloc_list(
            _py,
            &[
                MoltObject::from_int(3).bits(),
                MoltObject::from_int(4).bits(),
            ],
        );
        assert!(!list_ptr.is_null());
        let list_bits = MoltObject::from_ptr(list_ptr).bits();

        let append_name_ptr = alloc_string(_py, b"append");
        assert!(!append_name_ptr.is_null());
        let append_name_bits = MoltObject::from_ptr(append_name_ptr).bits();
        let append_bits = molt_object_getattr(list_bits, append_name_bits);
        assert!(!obj_from_bits(append_bits).is_none());
        let append_args_ptr = alloc_tuple(_py, &[MoltObject::from_int(5).bits()]);
        assert!(!append_args_ptr.is_null());
        let append_args_bits = MoltObject::from_ptr(append_args_ptr).bits();
        let append_out = molt_object_call(append_bits, append_args_bits, none_bits());
        assert!(!exception_pending(_py));
        assert!(obj_from_bits(append_out).is_none());
        dec_ref_bits(_py, append_args_bits);
        dec_ref_bits(_py, append_bits);
        dec_ref_bits(_py, append_name_bits);

        assert_eq!(molt_sequence_length(list_bits), 3);
        let idx_bits = MoltObject::from_int(1).bits();
        let got_bits = molt_sequence_getitem(list_bits, idx_bits);
        assert_eq!(to_i64(obj_from_bits(got_bits)), Some(4));
        let rc = molt_sequence_setitem(
            list_bits,
            MoltObject::from_int(0).bits(),
            MoltObject::from_int(9).bits(),
        );
        assert_eq!(rc, 0);
        let got0 = molt_sequence_getitem(list_bits, MoltObject::from_int(0).bits());
        assert_eq!(to_i64(obj_from_bits(got0)), Some(9));
        let got2 = molt_sequence_getitem(list_bits, MoltObject::from_int(2).bits());
        assert_eq!(to_i64(obj_from_bits(got2)), Some(5));
        dec_ref_bits(_py, got_bits);
        dec_ref_bits(_py, got0);
        dec_ref_bits(_py, got2);
        dec_ref_bits(_py, list_bits);
    });
}

#[test]
fn buffer_acquire_and_release_pins_owner() {
    let _guard = CApiTestGuard::new();
    let bytes_bits = unsafe { molt_bytes_from(b"abc".as_ptr(), 3) };
    assert!(!obj_from_bits(bytes_bits).is_none());
    let mut view = MoltBufferView::default();
    let rc = unsafe { molt_buffer_acquire(bytes_bits, &mut view as *mut MoltBufferView) };
    assert_eq!(rc, 0);
    assert_eq!(view.len, 3);
    assert_eq!(view.backing_capacity, 3);
    assert_eq!(view.readonly, 1);
    assert_eq!(view.ndim, 1);
    assert_eq!(view.itemsize, 1);
    assert_eq!(view.offset, 0);
    assert_eq!(view.base, bytes_bits);
    assert_eq!(view.shape[0], 3);
    assert_eq!(view.strides[0], 1);
    assert_eq!(view.format[0], b'B');
    assert!(!view.data.is_null());
    assert_eq!(view.owner, bytes_bits);
    let observed = unsafe { std::slice::from_raw_parts(view.data as *const u8, view.len as usize) };
    assert_eq!(observed, b"abc");
    let rc_release = unsafe { molt_buffer_release(&mut view as *mut MoltBufferView) };
    assert_eq!(rc_release, 0);
    assert!(view.data.is_null());
    assert_eq!(view.owner, 0);
    crate::with_gil_entry_nopanic!(_py, {
        dec_ref_bits(_py, bytes_bits);
    });
}

/// End-to-end interlock guard: exporting an `array.array` buffer must pin the
/// array against resize for the buffer's whole lifetime, and release must lift
/// the pin. This proves the export-lease is fully wired — `ArrayCell::exports`
/// is actually incremented on export and decremented on release, so
/// `resize_blocked` (and the `ensure_array_resizable` mutator guards) are
/// load-bearing rather than inert. It fails on the pre-wire code (arrays are not
/// buffer-exportable at all, so `molt_buffer_acquire` returns -1) and again if
/// the lease is ever un-wired (the mid-export append would silently succeed).
#[test]
fn array_buffer_export_lease_blocks_resize_until_release() {
    let _guard = CApiTestGuard::new();
    crate::with_gil_entry_nopanic!(_py, {
        use crate::builtins::array_mod::{molt_array_append, molt_array_new};

        let tc_ptr = alloc_string(_py, b"i");
        assert!(!tc_ptr.is_null());
        let tc_bits = MoltObject::from_ptr(tc_ptr).bits();
        let arr_bits = molt_array_new(tc_bits);
        assert!(!obj_from_bits(arr_bits).is_none());
        dec_ref_bits(_py, tc_bits);

        // Seed two elements so the exported buffer has real backing bytes.
        for value in [7_i64, 8] {
            let appended = molt_array_append(arr_bits, MoltObject::from_int(value).bits());
            assert!(!exception_pending(_py), "seed append must succeed");
            assert!(obj_from_bits(appended).is_none());
        }

        // Export the array through the live C buffer-protocol acquire path.
        let mut view = MoltBufferView::default();
        let rc = unsafe { molt_buffer_acquire(arr_bits, &mut view as *mut MoltBufferView) };
        assert_eq!(rc, 0, "array.array must be buffer-exportable");
        assert_eq!(view.itemsize, 4);
        assert_eq!(view.len, 8, "2 elements * 4-byte itemcode 'i'");
        assert_eq!(view.base, arr_bits);
        assert!(!view.data.is_null());

        // While the buffer is exported, resizing must raise the interlock error.
        let blocked = molt_array_append(arr_bits, MoltObject::from_int(9).bits());
        assert!(obj_from_bits(blocked).is_none());
        assert_pending_exception_class(_py, "BufferError");
        let _ = molt_err_clear();

        // Releasing the buffer drops the export lease, decrementing `exports`.
        let rc_release = unsafe { molt_buffer_release(&mut view as *mut MoltBufferView) };
        assert_eq!(rc_release, 0);
        assert!(view.data.is_null());
        assert_eq!(view.owner, 0);

        // After release, resizing succeeds again.
        let after = molt_array_append(arr_bits, MoltObject::from_int(10).bits());
        assert!(
            !exception_pending(_py),
            "append after buffer release must succeed"
        );
        assert!(obj_from_bits(after).is_none());

        dec_ref_bits(_py, arr_bits);
    });
}

#[test]
fn buffer_acquire_exports_shaped_memoryview_descriptor() {
    let _guard = CApiTestGuard::new();
    crate::with_gil_entry_nopanic!(_py, {
        let owner_ptr = alloc_bytes(_py, b"abcdefghijkl");
        assert!(!owner_ptr.is_null());
        let owner_bits = MoltObject::from_ptr(owner_ptr).bits();
        let format_ptr = alloc_string(_py, b"B");
        assert!(!format_ptr.is_null());
        let format_bits = MoltObject::from_ptr(format_ptr).bits();
        let view_ptr = alloc_shaped_memoryview_for_test(
            _py,
            owner_bits,
            0,
            1,
            false,
            format_bits,
            vec![3, 4],
            vec![4, 1],
        );
        assert!(!view_ptr.is_null());
        let view_bits = MoltObject::from_ptr(view_ptr).bits();
        let mut view = MoltBufferView::default();
        let rc = unsafe { molt_buffer_acquire(view_bits, &mut view as *mut MoltBufferView) };
        assert_eq!(rc, 0);
        assert_eq!(view.len, 12);
        assert_eq!(view.backing_capacity, 12);
        assert_eq!(view.readonly, 0);
        assert_eq!(view.ndim, 2);
        assert_eq!(view.itemsize, 1);
        assert_eq!(view.owner, view_bits);
        assert_eq!(view.base, owner_bits);
        assert_eq!(&view.shape[..2], &[3, 4]);
        assert_eq!(&view.strides[..2], &[4, 1]);
        assert_eq!(view.format[0], b'B');
        let observed =
            unsafe { std::slice::from_raw_parts(view.data as *const u8, view.len as usize) };
        assert_eq!(observed, b"abcdefghijkl");
        let rc_release = unsafe { molt_buffer_release(&mut view as *mut MoltBufferView) };
        assert_eq!(rc_release, 0);
        assert!(view.data.is_null());
        assert_eq!(view.owner, 0);
        dec_ref_bits(_py, view_bits);
        dec_ref_bits(_py, owner_bits);
        dec_ref_bits(_py, format_bits);
    });
}

#[test]
fn memoryview_clone_and_c_export_share_typed_strided_descriptor() {
    let _guard = CApiTestGuard::new();
    crate::with_gil_entry_nopanic!(_py, {
        let owner_ptr = alloc_bytes(_py, b"abcdefghijkl");
        assert!(!owner_ptr.is_null());
        let owner_bits = MoltObject::from_ptr(owner_ptr).bits();
        let format_ptr = alloc_string(_py, b"B");
        assert!(!format_ptr.is_null());
        let format_bits = MoltObject::from_ptr(format_ptr).bits();
        let view_ptr = alloc_shaped_memoryview_for_test(
            _py,
            owner_bits,
            0,
            1,
            false,
            format_bits,
            vec![3, 4],
            vec![4, 1],
        );
        assert!(!view_ptr.is_null());
        let view_bits = MoltObject::from_ptr(view_ptr).bits();
        let clone_bits = crate::molt_memoryview_new(view_bits);
        assert!(!obj_from_bits(clone_bits).is_none());
        let mut view = MoltBufferView::default();
        let rc = unsafe { molt_buffer_acquire(clone_bits, &mut view as *mut MoltBufferView) };
        assert_eq!(rc, 0);
        assert_eq!(view.len, 12);
        assert_eq!(view.backing_capacity, 12);
        assert_eq!(view.ndim, 2);
        assert_eq!(view.itemsize, 1);
        assert_eq!(view.owner, clone_bits);
        assert_eq!(view.base, owner_bits);
        assert_eq!(&view.shape[..2], &[3, 4]);
        assert_eq!(&view.strides[..2], &[4, 1]);
        assert_eq!(view.format[0], b'B');
        let rc_release = unsafe { molt_buffer_release(&mut view as *mut MoltBufferView) };
        assert_eq!(rc_release, 0);
        dec_ref_bits(_py, clone_bits);
        dec_ref_bits(_py, view_bits);
        dec_ref_bits(_py, owner_bits);
        dec_ref_bits(_py, format_bits);
    });
}

#[test]
fn memoryview_from_c_buffer_roundtrips_typed_descriptor() {
    let _guard = CApiTestGuard::new();
    crate::with_gil_entry_nopanic!(_py, {
        let mut data = [1u8, 2, 3, 4];
        let mut source = MoltBufferView {
            data: data.as_mut_ptr(),
            len: data.len() as u64,
            backing_capacity: data.len() as u64,
            readonly: 0,
            ..MoltBufferView::default()
        };
        source.shape[0] = data.len() as isize;
        source.strides[0] = 1;
        source.format[0] = b'B';

        let view_bits = unsafe { molt_memoryview_from_buffer(&source as *const MoltBufferView) };
        assert!(!obj_from_bits(view_bits).is_none());
        assert_eq!(molt_memoryview_check(view_bits), 1);

        let mut exported = MoltBufferView::default();
        let rc = unsafe { molt_buffer_acquire(view_bits, &mut exported as *mut MoltBufferView) };
        assert_eq!(rc, 0);
        assert_eq!(exported.data, data.as_mut_ptr());
        assert_eq!(exported.len, 4);
        assert_eq!(exported.backing_capacity, 4);
        assert_eq!(exported.readonly, 0);
        assert_eq!(exported.ndim, 1);
        assert_eq!(exported.itemsize, 1);
        assert_eq!(exported.owner, view_bits);
        assert_eq!(exported.base, 0);
        assert_eq!(exported.shape[0], 4);
        assert_eq!(exported.strides[0], 1);
        assert_eq!(exported.format[0], b'B');
        let rc_release = unsafe { molt_buffer_release(&mut exported as *mut MoltBufferView) };
        assert_eq!(rc_release, 0);

        let store_result = crate::molt_store_index(
            view_bits,
            MoltObject::from_int(1).bits(),
            MoltObject::from_int(9).bits(),
        );
        assert_eq!(store_result, view_bits);
        assert_eq!(data[1], 9);

        let slice_bits = crate::molt_slice_new(
            MoltObject::from_int(1).bits(),
            MoltObject::from_int(3).bits(),
            none_bits(),
        );
        assert!(!obj_from_bits(slice_bits).is_none());
        let sliced_bits = crate::molt_index(view_bits, slice_bits);
        assert!(!obj_from_bits(sliced_bits).is_none());
        let mut sliced = MoltBufferView::default();
        let slice_rc =
            unsafe { molt_buffer_acquire(sliced_bits, &mut sliced as *mut MoltBufferView) };
        assert_eq!(slice_rc, 0);
        assert_eq!(sliced.data, unsafe { data.as_mut_ptr().add(1) });
        assert_eq!(sliced.len, 2);
        assert_eq!(sliced.backing_capacity, 2);
        assert_eq!(sliced.base, 0);
        assert_eq!(sliced.shape[0], 2);
        let slice_release = unsafe { molt_buffer_release(&mut sliced as *mut MoltBufferView) };
        assert_eq!(slice_release, 0);
        dec_ref_bits(_py, sliced_bits);
        dec_ref_bits(_py, slice_bits);
        dec_ref_bits(_py, view_bits);
    });
}

#[test]
fn memoryview_from_c_buffer_roundtrips_strided_runtime_export_capacity() {
    let _guard = CApiTestGuard::new();
    crate::with_gil_entry_nopanic!(_py, {
        let bytearray_ptr = alloc_bytearray(_py, b"abcde");
        assert!(!bytearray_ptr.is_null());
        let bytearray_bits = MoltObject::from_ptr(bytearray_ptr).bits();
        let view_bits = crate::molt_memoryview_new(bytearray_bits);
        assert!(!obj_from_bits(view_bits).is_none());
        let slice_bits =
            crate::molt_slice_new(none_bits(), none_bits(), MoltObject::from_int(2).bits());
        assert!(!obj_from_bits(slice_bits).is_none());
        let sliced_bits = crate::molt_index(view_bits, slice_bits);
        assert!(!obj_from_bits(sliced_bits).is_none());

        let mut exported = MoltBufferView::default();
        let rc = unsafe { molt_buffer_acquire(sliced_bits, &mut exported as *mut MoltBufferView) };
        assert_eq!(rc, 0);
        assert_eq!(exported.len, 3);
        assert_eq!(exported.backing_capacity, 5);
        assert_eq!(exported.base, bytearray_bits);
        assert_eq!(exported.shape[0], 3);
        assert_eq!(exported.strides[0], 2);

        let cloned_bits =
            unsafe { molt_memoryview_from_buffer(&exported as *const MoltBufferView) };
        assert!(!obj_from_bits(cloned_bits).is_none());
        assert_eq!(molt_memoryview_check(cloned_bits), 1);
        let clone_bytes_bits = crate::molt_memoryview_tobytes(cloned_bits);
        let Some(clone_bytes_ptr) = obj_from_bits(clone_bytes_bits).as_ptr() else {
            panic!("strided clone tobytes did not return bytes");
        };
        let clone_bytes = unsafe {
            std::slice::from_raw_parts(bytes_data(clone_bytes_ptr), bytes_len(clone_bytes_ptr))
        };
        assert_eq!(clone_bytes, b"ace");

        let release_rc = unsafe { molt_buffer_release(&mut exported as *mut MoltBufferView) };
        assert_eq!(release_rc, 0);
        dec_ref_bits(_py, clone_bytes_bits);
        dec_ref_bits(_py, cloned_bits);
        dec_ref_bits(_py, sliced_bits);
        dec_ref_bits(_py, slice_bits);
        dec_ref_bits(_py, view_bits);
        dec_ref_bits(_py, bytearray_bits);
    });
}

#[test]
fn memoryview_from_c_buffer_rejects_base_pointer_spoof() {
    let _guard = CApiTestGuard::new();
    crate::with_gil_entry_nopanic!(_py, {
        let bytearray_ptr = alloc_bytearray(_py, b"abcde");
        assert!(!bytearray_ptr.is_null());
        let bytearray_bits = MoltObject::from_ptr(bytearray_ptr).bits();
        let mut bogus = [b'x'];
        let mut source = MoltBufferView {
            data: bogus.as_mut_ptr(),
            len: 1,
            backing_capacity: 1,
            base: bytearray_bits,
            readonly: 0,
            ..MoltBufferView::default()
        };
        source.shape[0] = 1;
        source.strides[0] = 1;
        source.format[0] = b'B';

        let view_bits = unsafe { molt_memoryview_from_buffer(&source as *const MoltBufferView) };
        assert_none_with_exception_class(_py, view_bits, "BufferError");
        dec_ref_bits(_py, bytearray_bits);
    });
}

#[test]
fn memoryview_from_c_buffer_rejects_strided_span_past_backing() {
    let _guard = CApiTestGuard::new();
    crate::with_gil_entry_nopanic!(_py, {
        let mut data = [1u8];
        let mut source = MoltBufferView {
            data: data.as_mut_ptr(),
            len: data.len() as u64,
            backing_capacity: data.len() as u64,
            readonly: 0,
            ..MoltBufferView::default()
        };
        source.shape[0] = 16;
        source.strides[0] = 1;
        source.format[0] = b'B';

        let view_bits = unsafe { molt_memoryview_from_buffer(&source as *const MoltBufferView) };
        assert_none_with_exception_class(_py, view_bits, "BufferError");
    });
}

#[test]
fn memoryview_from_c_buffer_rejects_logical_len_mismatch() {
    let _guard = CApiTestGuard::new();
    crate::with_gil_entry_nopanic!(_py, {
        let mut data = [1u8, 2, 3, 4];
        let mut source = MoltBufferView {
            data: data.as_mut_ptr(),
            len: 1,
            backing_capacity: data.len() as u64,
            readonly: 0,
            ..MoltBufferView::default()
        };
        source.shape[0] = data.len() as isize;
        source.strides[0] = 1;
        source.format[0] = b'B';

        let view_bits = unsafe { molt_memoryview_from_buffer(&source as *const MoltBufferView) };
        assert_none_with_exception_class(_py, view_bits, "BufferError");
    });
}

#[test]
fn memoryview_from_c_buffer_accepts_zero_length_null_data() {
    let _guard = CApiTestGuard::new();
    crate::with_gil_entry_nopanic!(_py, {
        let mut source = MoltBufferView::default();
        source.shape[0] = 0;
        source.strides[0] = 1;
        source.format[0] = b'B';

        let view_bits = unsafe { molt_memoryview_from_buffer(&source as *const MoltBufferView) };
        assert!(!obj_from_bits(view_bits).is_none());
        assert_eq!(molt_memoryview_check(view_bits), 1);

        let mut exported = MoltBufferView::default();
        let rc = unsafe { molt_buffer_acquire(view_bits, &mut exported as *mut MoltBufferView) };
        assert_eq!(rc, 0);
        assert!(!exported.data.is_null());
        assert_eq!(exported.len, 0);
        assert_eq!(exported.backing_capacity, 0);
        assert_eq!(exported.shape[0], 0);
        let release_rc = unsafe { molt_buffer_release(&mut exported as *mut MoltBufferView) };
        assert_eq!(release_rc, 0);
        dec_ref_bits(_py, view_bits);
    });
}

#[test]
fn memoryview_release_closes_typed_storage_export_and_runtime_access() {
    let _guard = CApiTestGuard::new();
    crate::with_gil_entry_nopanic!(_py, {
        let owner_ptr = alloc_bytes(_py, b"abcdefghijkl");
        assert!(!owner_ptr.is_null());
        let owner_bits = MoltObject::from_ptr(owner_ptr).bits();
        let format_ptr = alloc_string(_py, b"B");
        assert!(!format_ptr.is_null());
        let format_bits = MoltObject::from_ptr(format_ptr).bits();
        let view_ptr = alloc_shaped_memoryview_for_test(
            _py,
            owner_bits,
            0,
            1,
            false,
            format_bits,
            vec![3, 4],
            vec![4, 1],
        );
        assert!(!view_ptr.is_null());
        let view_bits = MoltObject::from_ptr(view_ptr).bits();

        let release_bits = crate::molt_memoryview_release(view_bits);
        assert!(obj_from_bits(release_bits).is_none());
        assert!(!exception_pending(_py));
        let second_release_bits = crate::molt_memoryview_release(view_bits);
        assert!(obj_from_bits(second_release_bits).is_none());
        assert!(!exception_pending(_py));

        let clone_bits = crate::molt_memoryview_new(view_bits);
        assert!(obj_from_bits(clone_bits).is_none());
        assert!(exception_pending(_py));
        clear_exception(_py);

        let mut view = MoltBufferView::default();
        let rc = unsafe { molt_buffer_acquire(view_bits, &mut view as *mut MoltBufferView) };
        assert_eq!(rc, -1);
        assert!(exception_pending(_py));
        assert!(view.data.is_null());
        assert_eq!(view.owner, 0);
        clear_exception(_py);

        let bytes_bits = crate::molt_memoryview_tobytes(view_bits);
        assert!(obj_from_bits(bytes_bits).is_none());
        assert!(exception_pending(_py));
        clear_exception(_py);

        let len_bits = crate::molt_len(view_bits);
        assert!(obj_from_bits(len_bits).is_none());
        assert!(exception_pending(_py));
        clear_exception(_py);

        let item_bits = crate::molt_index(view_bits, MoltObject::from_int(0).bits());
        assert!(obj_from_bits(item_bits).is_none());
        assert!(exception_pending(_py));
        clear_exception(_py);

        dec_ref_bits(_py, view_bits);
        dec_ref_bits(_py, owner_bits);
        dec_ref_bits(_py, format_bits);
    });
}

#[test]
fn memoryview_release_closes_byteslike_method_arguments() {
    let _guard = CApiTestGuard::new();
    crate::with_gil_entry_nopanic!(_py, {
        let owner_ptr = alloc_bytes(_py, b"b");
        assert!(!owner_ptr.is_null());
        let owner_bits = MoltObject::from_ptr(owner_ptr).bits();
        let format_ptr = alloc_string(_py, b"B");
        assert!(!format_ptr.is_null());
        let format_bits = MoltObject::from_ptr(format_ptr).bits();
        let view_ptr = alloc_shaped_memoryview_for_test(
            _py,
            owner_bits,
            0,
            1,
            false,
            format_bits,
            vec![1],
            vec![1],
        );
        assert!(!view_ptr.is_null());
        let view_bits = MoltObject::from_ptr(view_ptr).bits();

        let release_bits = crate::molt_memoryview_release(view_bits);
        assert!(obj_from_bits(release_bits).is_none());
        assert!(!exception_pending(_py));

        let hay_ptr = alloc_bytes(_py, b"abc");
        assert!(!hay_ptr.is_null());
        let hay_bits = MoltObject::from_ptr(hay_ptr).bits();
        let bytearray_ptr = alloc_bytearray(_py, b"abc");
        assert!(!bytearray_ptr.is_null());
        let bytearray_bits = MoltObject::from_ptr(bytearray_ptr).bits();
        let repl_ptr = alloc_bytes(_py, b"x");
        assert!(!repl_ptr.is_null());
        let repl_bits = MoltObject::from_ptr(repl_ptr).bits();
        let count_bits = MoltObject::from_int(-1).bits();

        assert_none_with_exception_class(
            _py,
            crate::molt_bytes_find(hay_bits, view_bits),
            "ValueError",
        );
        assert_none_with_exception_class(
            _py,
            crate::molt_bytes_startswith(hay_bits, view_bits),
            "ValueError",
        );
        assert_none_with_exception_class(
            _py,
            crate::molt_bytes_split(hay_bits, view_bits),
            "ValueError",
        );
        assert_none_with_exception_class(
            _py,
            crate::molt_bytes_replace(hay_bits, view_bits, repl_bits, count_bits),
            "ValueError",
        );
        assert_none_with_exception_class(
            _py,
            crate::molt_bytearray_find(bytearray_bits, view_bits),
            "ValueError",
        );
        assert_none_with_exception_class(
            _py,
            crate::molt_bytearray_replace(bytearray_bits, view_bits, repl_bits, count_bits),
            "ValueError",
        );

        let sep_ptr = alloc_bytes(_py, b",");
        assert!(!sep_ptr.is_null());
        let sep_bits = MoltObject::from_ptr(sep_ptr).bits();
        let list_ptr = alloc_list(_py, &[view_bits]);
        assert!(!list_ptr.is_null());
        let list_bits = MoltObject::from_ptr(list_ptr).bits();
        assert_none_with_exception_class(
            _py,
            crate::molt_bytes_join(sep_bits, list_bits),
            "TypeError",
        );

        dec_ref_bits(_py, list_bits);
        dec_ref_bits(_py, sep_bits);
        dec_ref_bits(_py, repl_bits);
        dec_ref_bits(_py, bytearray_bits);
        dec_ref_bits(_py, hay_bits);
        dec_ref_bits(_py, view_bits);
        dec_ref_bits(_py, owner_bits);
        dec_ref_bits(_py, format_bits);
    });
}

#[test]
fn err_pending_peek_restore_roundtrip() {
    let _guard = CApiTestGuard::new();
    crate::with_gil_entry_nopanic!(_py, {
        let runtime_error = runtime_error_type_bits(_py);
        let msg = b"boom";
        let rc = unsafe { molt_err_set(runtime_error, msg.as_ptr(), msg.len() as u64) };
        assert_eq!(rc, 0);
        assert_eq!(molt_err_pending(), 1);
        let peek_bits = molt_err_peek();
        assert!(!obj_from_bits(peek_bits).is_none());
        assert_eq!(molt_err_pending(), 1);
        let fetched_bits = molt_err_fetch();
        assert!(!obj_from_bits(fetched_bits).is_none());
        assert_eq!(molt_err_pending(), 0);
        assert_eq!(molt_err_restore(fetched_bits), 0);
        assert_eq!(molt_err_pending(), 1);
        let restored_bits = molt_err_fetch();
        assert!(!obj_from_bits(restored_bits).is_none());
        assert_eq!(molt_err_pending(), 0);
        dec_ref_bits(_py, peek_bits);
        dec_ref_bits(_py, fetched_bits);
        dec_ref_bits(_py, restored_bits);
    });
}

#[test]
fn err_clear_resets_last_exception_slot() {
    let _guard = CApiTestGuard::new();
    let runtime_error = crate::with_gil_entry_nopanic!(_py, { runtime_error_type_bits(_py) });
    let msg = b"boom";
    let rc = unsafe { molt_err_set(runtime_error, msg.as_ptr(), msg.len() as u64) };
    assert_eq!(rc, 0);
    assert_eq!(molt_exception_pending(), 1);

    let peek_bits = molt_exception_last();
    assert!(!obj_from_bits(peek_bits).is_none());

    let _ = molt_exception_clear();
    assert_eq!(molt_exception_pending(), 0);

    let after_clear_bits = molt_exception_last();
    assert!(obj_from_bits(after_clear_bits).is_none());
    assert_eq!(molt_exception_pending(), 0);

    crate::with_gil_entry_nopanic!(_py, {
        dec_ref_bits(_py, peek_bits);
    });
}

#[test]
fn mapping_length_success_and_failure_paths() {
    let _guard = CApiTestGuard::new();
    crate::with_gil_entry_nopanic!(_py, {
        let dict_ptr = alloc_dict_with_pairs(_py, &[]);
        assert!(!dict_ptr.is_null());
        let dict_bits = MoltObject::from_ptr(dict_ptr).bits();
        let key_ptr = alloc_string(_py, b"k");
        assert!(!key_ptr.is_null());
        let key_bits = MoltObject::from_ptr(key_ptr).bits();
        let value_bits = MoltObject::from_int(7).bits();
        assert_eq!(molt_mapping_setitem(dict_bits, key_bits, value_bits), 0);
        assert_eq!(molt_mapping_length(dict_bits), 1);
        let invalid_bits = MoltObject::from_int(42).bits();
        assert_eq!(molt_mapping_length(invalid_bits), -1);
        assert!(exception_pending(_py));
        let _ = molt_exception_clear();
        dec_ref_bits(_py, key_bits);
        dec_ref_bits(_py, dict_bits);
    });
}

#[test]
#[cfg_attr(
    miri,
    ignore = "Miri strict provenance cannot execute exposed-address dynamic method dispatch until all C-API test callbacks use provenance-registered function keys"
)]
fn mapping_keys_success_and_failure_paths() {
    let _guard = CApiTestGuard::new();
    crate::with_gil_entry_nopanic!(_py, {
        let dict_ptr = alloc_dict_with_pairs(_py, &[]);
        assert!(!dict_ptr.is_null());
        let dict_bits = MoltObject::from_ptr(dict_ptr).bits();
        let key_ptr = alloc_string(_py, b"k");
        assert!(!key_ptr.is_null());
        let key_bits = MoltObject::from_ptr(key_ptr).bits();
        let value_bits = MoltObject::from_int(7).bits();
        assert_eq!(molt_mapping_setitem(dict_bits, key_bits, value_bits), 0);

        let keys_bits = molt_mapping_keys(dict_bits);
        assert!(!obj_from_bits(keys_bits).is_none());
        assert_eq!(molt_sequence_length(keys_bits), 1);
        assert_eq!(molt_object_contains(keys_bits, key_bits), 1);
        dec_ref_bits(_py, keys_bits);

        let invalid_bits = MoltObject::from_int(42).bits();
        assert!(obj_from_bits(molt_mapping_keys(invalid_bits)).is_none());
        assert!(exception_pending(_py));
        let _ = molt_exception_clear();

        dec_ref_bits(_py, key_bits);
        dec_ref_bits(_py, dict_bits);
    });
}

#[test]
fn string_from_as_ptr_roundtrip_and_type_errors() {
    let _guard = CApiTestGuard::new();
    let text = b"hello";
    let string_bits = unsafe { molt_string_from(text.as_ptr(), text.len() as u64) };
    assert!(!obj_from_bits(string_bits).is_none());
    let mut out_len = 0u64;
    let ptr = unsafe { molt_string_as_ptr(string_bits, &mut out_len as *mut u64) };
    assert!(!ptr.is_null());
    assert_eq!(out_len, text.len() as u64);
    let observed = unsafe { std::slice::from_raw_parts(ptr, out_len as usize) };
    assert_eq!(observed, text);

    let invalid_bits = MoltObject::from_int(9).bits();
    crate::with_gil_entry_nopanic!(_py, {
        let bad_ptr = unsafe { molt_string_as_ptr(invalid_bits, std::ptr::null_mut()) };
        assert!(bad_ptr.is_null());
        assert_eq!(molt_err_pending(), 1);
        assert_eq!(molt_err_clear(), 0);
    });

    let null_bits = crate::with_gil_entry_nopanic!(_py, {
        let null_bits = unsafe { molt_string_from(std::ptr::null(), 1) };
        assert_eq!(molt_err_pending(), 1);
        assert_eq!(molt_err_clear(), 0);
        null_bits
    });

    crate::with_gil_entry_nopanic!(_py, {
        dec_ref_bits(_py, string_bits);
        if !obj_from_bits(null_bits).is_none() {
            dec_ref_bits(_py, null_bits);
        }
    });
}

#[test]
fn object_setattr_symbol_roundtrip() {
    let _guard = CApiTestGuard::new();
    crate::with_gil_entry_nopanic!(_py, {
        let runtime_error = runtime_error_type_bits(_py);
        let msg_ptr = alloc_string(_py, b"msg");
        assert!(!msg_ptr.is_null());
        let msg_bits = MoltObject::from_ptr(msg_ptr).bits();
        let exc_bits = molt_exception_new_from_class(runtime_error, msg_bits);
        assert!(!obj_from_bits(exc_bits).is_none());
        let attr_ptr = alloc_string(_py, b"custom");
        assert!(!attr_ptr.is_null());
        let attr_bits = MoltObject::from_ptr(attr_ptr).bits();
        let value_bits = MoltObject::from_int(99).bits();
        let set_result = molt_object_setattr(exc_bits, attr_bits, value_bits);
        assert!(!exception_pending(_py));
        let got_bits = molt_object_getattr(exc_bits, attr_bits);
        assert_eq!(to_i64(obj_from_bits(got_bits)), Some(99));
        dec_ref_bits(_py, got_bits);
        if !obj_from_bits(set_result).is_none() {
            dec_ref_bits(_py, set_result);
        }
        dec_ref_bits(_py, attr_bits);
        dec_ref_bits(_py, exc_bits);
        dec_ref_bits(_py, msg_bits);
    });
}

#[test]
fn attr_object_ic_keeps_type_objects_distinct_per_site() {
    let _guard = CApiTestGuard::new();
    crate::with_gil_entry_nopanic!(_py, {
        crate::object::bump_type_version();
        let class_a_bits = create_test_heap_class(_py, b"A", &[]);
        let class_b_bits = create_test_heap_class(_py, b"B", &[]);
        let site_bits = MoltObject::from_int(37).bits();

        let a_name_bits = unsafe {
            crate::builtins::attributes::molt_get_attr_object_ic(
                class_a_bits,
                b"__name__".as_ptr(),
                b"__name__".len() as u64,
                site_bits,
            ) as u64
        };
        let b_name_bits = unsafe {
            crate::builtins::attributes::molt_get_attr_object_ic(
                class_b_bits,
                b"__name__".as_ptr(),
                b"__name__".len() as u64,
                site_bits,
            ) as u64
        };

        let mut a_len = 0u64;
        let a_ptr = unsafe { molt_string_as_ptr(a_name_bits, &mut a_len as *mut u64) };
        assert!(!a_ptr.is_null());
        assert_eq!(
            unsafe { std::slice::from_raw_parts(a_ptr, a_len as usize) },
            b"A"
        );

        let mut b_len = 0u64;
        let b_ptr = unsafe { molt_string_as_ptr(b_name_bits, &mut b_len as *mut u64) };
        assert!(!b_ptr.is_null());
        assert_eq!(
            unsafe { std::slice::from_raw_parts(b_ptr, b_len as usize) },
            b"B"
        );

        dec_ref_bits(_py, b_name_bits);
        dec_ref_bits(_py, a_name_bits);
        dec_ref_bits(_py, class_b_bits);
        dec_ref_bits(_py, class_a_bits);
    });
}

#[test]
fn attr_object_ic_keeps_class_attrs_distinct_per_site() {
    let _guard = CApiTestGuard::new();
    crate::with_gil_entry_nopanic!(_py, {
        // Invalidate stale IC entries from prior tests that may hold
        // dangling pointers to freed heap objects.
        crate::object::bump_type_version();
        let class_a_bits =
            create_test_heap_class(_py, b"A", &[(b"x", MoltObject::from_int(1).bits())]);
        let class_b_bits =
            create_test_heap_class(_py, b"B", &[(b"x", MoltObject::from_int(2).bits())]);
        let site_bits = MoltObject::from_int(41).bits();

        let a_x_bits = unsafe {
            crate::builtins::attributes::molt_get_attr_object_ic(
                class_a_bits,
                b"x".as_ptr(),
                1,
                site_bits,
            ) as u64
        };
        let b_x_bits = unsafe {
            crate::builtins::attributes::molt_get_attr_object_ic(
                class_b_bits,
                b"x".as_ptr(),
                1,
                site_bits,
            ) as u64
        };

        assert_eq!(to_i64(obj_from_bits(a_x_bits)), Some(1));
        assert_eq!(to_i64(obj_from_bits(b_x_bits)), Some(2));

        dec_ref_bits(_py, b_x_bits);
        dec_ref_bits(_py, a_x_bits);
        dec_ref_bits(_py, class_b_bits);
        dec_ref_bits(_py, class_a_bits);
    });
}

#[test]
fn plain_function_object_has_no_set_name_attr() {
    let _guard = CApiTestGuard::new();
    crate::with_gil_entry_nopanic!(_py, {
        let func_ptr = crate::builtins::functions::alloc_runtime_function_obj(
            _py,
            crate::builtins::functions::runtime_fn_addr(
                "crate::molt_id",
                crate::molt_id as *const (),
            ),
            1,
        );
        assert!(!func_ptr.is_null());
        let func_bits = MoltObject::from_ptr(func_ptr).bits();

        let name_ptr = alloc_string(_py, b"__set_name__");
        assert!(!name_ptr.is_null());
        let name_bits = MoltObject::from_ptr(name_ptr).bits();

        let none_bits = MoltObject::none().bits();
        let got_bits = crate::builtins::attributes::molt_get_attr_name_default(
            func_bits, name_bits, none_bits,
        );

        assert!(obj_from_bits(got_bits).is_none());
        assert!(!exception_pending(_py));

        dec_ref_bits(_py, got_bits);
        dec_ref_bits(_py, name_bits);
        dec_ref_bits(_py, func_bits);
    });
}

#[test]
fn class_apply_set_name_tolerates_plain_function_attrs() {
    let _guard = CApiTestGuard::new();
    crate::with_gil_entry_nopanic!(_py, {
        let func_ptr = crate::builtins::functions::alloc_runtime_function_obj(
            _py,
            crate::builtins::functions::runtime_fn_addr(
                "crate::molt_id",
                crate::molt_id as *const (),
            ),
            1,
        );
        assert!(!func_ptr.is_null());
        let func_bits = MoltObject::from_ptr(func_ptr).bits();
        let class_bits = create_test_heap_class(_py, b"A", &[(b"f", func_bits)]);

        let res_bits = crate::molt_class_apply_set_name(class_bits);
        assert!(obj_from_bits(res_bits).is_none());
        assert!(!exception_pending(_py));

        dec_ref_bits(_py, class_bits);
        dec_ref_bits(_py, func_bits);
    });
}

#[test]
fn runtime_intrinsic_function_obj_zero_arg_vec_call_returns_value() {
    let _guard = CApiTestGuard::new();
    crate::with_gil_entry_nopanic!(_py, {
        let func_ptr = crate::builtins::functions::alloc_runtime_function_obj(
            _py,
            crate::builtins::functions::runtime_fn_addr(
                "crate::molt_sys_version_info",
                crate::molt_sys_version_info as *const (),
            ),
            0,
        );
        assert!(!func_ptr.is_null());
        let func_bits = MoltObject::from_ptr(func_ptr).bits();

        let out_bits = unsafe { crate::call::function::call_function_obj_vec(_py, func_bits, &[]) };
        let out_ptr = obj_from_bits(out_bits).as_ptr();
        assert!(out_ptr.is_some(), "expected tuple result, got none");
        assert_eq!(unsafe { object_type_id(out_ptr.unwrap()) }, TYPE_ID_TUPLE);
        assert!(!exception_pending(_py));

        dec_ref_bits(_py, out_bits);
        dec_ref_bits(_py, func_bits);
    });
}

#[test]
fn runtime_intrinsic_function_obj_zero_arg_indirect_call_returns_value() {
    let _guard = CApiTestGuard::new();
    crate::with_gil_entry_nopanic!(_py, {
        let func_ptr = crate::builtins::functions::alloc_runtime_function_obj(
            _py,
            crate::builtins::functions::runtime_fn_addr(
                "crate::molt_sys_version_info",
                crate::molt_sys_version_info as *const (),
            ),
            0,
        );
        assert!(!func_ptr.is_null());
        let func_bits = MoltObject::from_ptr(func_ptr).bits();
        let site_bits = MoltObject::from_int(17).bits();
        let builder_bits = crate::call::bind::molt_callargs_new(0, 0);

        let out_bits = crate::call::bind::molt_call_indirect_ic(site_bits, func_bits, builder_bits);
        let out_ptr = obj_from_bits(out_bits).as_ptr();
        assert!(out_ptr.is_some(), "expected tuple result, got none");
        assert_eq!(unsafe { object_type_id(out_ptr.unwrap()) }, TYPE_ID_TUPLE);
        assert!(!exception_pending(_py));

        dec_ref_bits(_py, out_bits);
        dec_ref_bits(_py, func_bits);
    });
}

#[test]
fn call_bind_ic_hits_bound_direct_function_method() {
    let _guard = CApiTestGuard::new();
    crate::with_gil_entry_nopanic!(_py, {
        let func_ptr = crate::builtins::functions::alloc_runtime_function_obj(
            _py,
            crate::builtins::functions::runtime_fn_addr(
                "c_api_test_bound_identity",
                c_api_test_bound_identity as *const (),
            ),
            2,
        );
        assert!(!func_ptr.is_null());
        let func_bits = MoltObject::from_ptr(func_ptr).bits();
        let class_bits = create_test_heap_class(_py, b"A", &[(b"f", func_bits)]);
        let _ = crate::molt_class_apply_set_name(class_bits);
        let class_ptr = obj_from_bits(class_bits).as_ptr().expect("class ptr");
        let inst_bits = unsafe { crate::alloc_instance_for_class(_py, class_ptr) };
        let name_ptr = alloc_string(_py, b"f");
        assert!(!name_ptr.is_null());
        let name_bits = MoltObject::from_ptr(name_ptr).bits();
        let method_bits = molt_object_getattr(inst_bits, name_bits);
        assert!(!obj_from_bits(method_bits).is_none());
        let site_bits = MoltObject::from_int(31).bits();

        let builder_a = crate::call::bind::molt_callargs_new(1, 0);
        let _ = unsafe { crate::molt_callargs_push_pos(builder_a, MoltObject::from_int(9).bits()) };
        let out_a = crate::call::bind::molt_call_bind_ic(site_bits, method_bits, builder_a);
        assert_eq!(to_i64(obj_from_bits(out_a)), Some(9));
        assert!(!exception_pending(_py));

        let builder_b = crate::call::bind::molt_callargs_new(1, 0);
        let _ =
            unsafe { crate::molt_callargs_push_pos(builder_b, MoltObject::from_int(11).bits()) };
        let out_b = crate::call::bind::molt_call_bind_ic(site_bits, method_bits, builder_b);
        assert_eq!(to_i64(obj_from_bits(out_b)), Some(11));
        assert!(!exception_pending(_py));

        dec_ref_bits(_py, out_b);
        dec_ref_bits(_py, out_a);
        dec_ref_bits(_py, method_bits);
        dec_ref_bits(_py, name_bits);
        dec_ref_bits(_py, inst_bits);
        dec_ref_bits(_py, class_bits);
        dec_ref_bits(_py, func_bits);
    });
}

#[test]
fn call_bind_ic_hits_simple_object_call_bound_function() {
    let _guard = CApiTestGuard::new();
    crate::with_gil_entry_nopanic!(_py, {
        let func_ptr = crate::builtins::functions::alloc_runtime_function_obj(
            _py,
            crate::builtins::functions::runtime_fn_addr(
                "c_api_test_bound_identity",
                c_api_test_bound_identity as *const (),
            ),
            2,
        );
        assert!(!func_ptr.is_null());
        let func_bits = MoltObject::from_ptr(func_ptr).bits();
        let class_bits = create_test_heap_class(_py, b"CallableA", &[(b"__call__", func_bits)]);
        let _ = crate::molt_class_apply_set_name(class_bits);
        let class_ptr = obj_from_bits(class_bits).as_ptr().expect("class ptr");
        let inst_bits = unsafe { crate::alloc_instance_for_class(_py, class_ptr) };
        let site_bits = MoltObject::from_int(37).bits();

        let builder_a = crate::call::bind::molt_callargs_new(1, 0);
        let _ = unsafe { crate::molt_callargs_push_pos(builder_a, MoltObject::from_int(5).bits()) };
        let out_a = crate::call::bind::molt_call_bind_ic(site_bits, inst_bits, builder_a);
        assert_eq!(to_i64(obj_from_bits(out_a)), Some(5));
        assert!(!exception_pending(_py));

        let builder_b = crate::call::bind::molt_callargs_new(1, 0);
        let _ = unsafe { crate::molt_callargs_push_pos(builder_b, MoltObject::from_int(7).bits()) };
        let out_b = crate::call::bind::molt_call_bind_ic(site_bits, inst_bits, builder_b);
        assert_eq!(to_i64(obj_from_bits(out_b)), Some(7));
        assert!(!exception_pending(_py));

        dec_ref_bits(_py, out_b);
        dec_ref_bits(_py, out_a);
        dec_ref_bits(_py, inst_bits);
        dec_ref_bits(_py, class_bits);
        dec_ref_bits(_py, func_bits);
    });
}

#[test]
fn call_bind_ic_classification_does_not_rebind_stateful_call_descriptor() {
    let _guard = CApiTestGuard::new();
    crate::with_gil_entry_nopanic!(_py, {
        crate::call::bind::clear_call_bind_ic_cache(_py);
        CALL_DESCRIPTOR_GET_COUNT.store(0, Ordering::SeqCst);

        let target_ptr = crate::builtins::functions::alloc_runtime_function_obj(
            _py,
            crate::builtins::functions::runtime_fn_addr(
                "c_api_test_identity",
                c_api_test_identity as *const (),
            ),
            1,
        );
        assert!(!target_ptr.is_null());
        let target_bits = MoltObject::from_ptr(target_ptr).bits();
        CALL_DESCRIPTOR_TARGET_BITS.store(target_bits, Ordering::SeqCst);

        let get_ptr = crate::builtins::functions::alloc_runtime_function_obj(
            _py,
            crate::builtins::functions::runtime_fn_addr(
                "c_api_test_call_descriptor_get_once",
                c_api_test_call_descriptor_get_once as *const (),
            ),
            3,
        );
        assert!(!get_ptr.is_null());
        let get_bits = MoltObject::from_ptr(get_ptr).bits();
        let descriptor_class_bits =
            create_test_heap_class(_py, b"OneShotCallDescriptor", &[(b"__get__", get_bits)]);
        let descriptor_class_ptr = obj_from_bits(descriptor_class_bits)
            .as_ptr()
            .expect("descriptor class ptr");
        let descriptor_bits = unsafe { crate::alloc_instance_for_class(_py, descriptor_class_ptr) };
        assert!(!obj_from_bits(descriptor_bits).is_none());

        let callable_class_bits =
            create_test_heap_class(_py, b"StatefulCallable", &[(b"__call__", descriptor_bits)]);
        let callable_class_ptr = obj_from_bits(callable_class_bits)
            .as_ptr()
            .expect("callable class ptr");
        let callable_bits = unsafe { crate::alloc_instance_for_class(_py, callable_class_ptr) };
        assert!(!obj_from_bits(callable_bits).is_none());

        let site_id = 113u64;
        let builder_bits = crate::call::bind::molt_callargs_new(1, 0);
        let _ =
            unsafe { crate::molt_callargs_push_pos(builder_bits, MoltObject::from_int(29).bits()) };
        let out_bits = crate::call::bind::molt_call_bind_ic(
            MoltObject::from_int(site_id as i64).bits(),
            callable_bits,
            builder_bits,
        );

        assert_eq!(to_i64(obj_from_bits(out_bits)), Some(29));
        assert_eq!(
            CALL_DESCRIPTOR_GET_COUNT.load(Ordering::SeqCst),
            1,
            "IC classification must not execute __get__ after the real bind"
        );
        assert!(!exception_pending(_py));
        assert!(
            !crate::call::bind::call_bind_ic_site_cached_for_test(site_id),
            "a dynamic __call__ descriptor must never install a direct-call IC"
        );

        CALL_DESCRIPTOR_TARGET_BITS.store(0, Ordering::SeqCst);
        dec_ref_bits(_py, out_bits);
        dec_ref_bits(_py, callable_bits);
        dec_ref_bits(_py, callable_class_bits);
        dec_ref_bits(_py, descriptor_bits);
        dec_ref_bits(_py, descriptor_class_bits);
        dec_ref_bits(_py, get_bits);
        dec_ref_bits(_py, target_bits);
        crate::call::bind::clear_call_bind_ic_cache(_py);
    });
}

#[test]
fn call_bind_ic_never_bypasses_custom_metaclass_call() {
    let _guard = CApiTestGuard::new();
    crate::with_gil_entry_nopanic!(_py, {
        crate::call::bind::clear_call_bind_ic_cache(_py);
        CUSTOM_METACLASS_CALL_COUNT.store(0, Ordering::SeqCst);

        let call_ptr = crate::builtins::functions::alloc_runtime_function_obj(
            _py,
            crate::builtins::functions::runtime_fn_addr(
                "c_api_test_custom_metaclass_call",
                c_api_test_custom_metaclass_call as *const (),
            ),
            2,
        );
        assert!(!call_ptr.is_null());
        let call_bits = MoltObject::from_ptr(call_ptr).bits();
        let builtins = crate::builtins::classes::builtin_classes(_py);
        let metaclass_bits = create_test_type(
            _py,
            builtins.type_obj,
            b"CountingMeta",
            builtins.type_obj,
            &[(b"__call__", call_bits)],
        );
        let class_bits =
            create_test_type(_py, metaclass_bits, b"MetaCallable", builtins.object, &[]);

        let site_id = 127u64;
        for (index, expected) in [41i64, 43i64].into_iter().enumerate() {
            let builder_bits = crate::call::bind::molt_callargs_new(1, 0);
            let _ = unsafe {
                crate::molt_callargs_push_pos(builder_bits, MoltObject::from_int(expected).bits())
            };
            let out_bits = crate::call::bind::molt_call_bind_ic(
                MoltObject::from_int(site_id as i64).bits(),
                class_bits,
                builder_bits,
            );
            assert_eq!(to_i64(obj_from_bits(out_bits)), Some(expected));
            assert_eq!(
                CUSTOM_METACLASS_CALL_COUNT.load(Ordering::SeqCst),
                index + 1,
                "every call at the same site must dispatch through metaclass __call__"
            );
            assert!(!exception_pending(_py));
            assert!(
                !crate::call::bind::call_bind_ic_site_cached_for_test(site_id),
                "custom metaclass __call__ must never install TYPE_CALL"
            );
            dec_ref_bits(_py, out_bits);
        }

        dec_ref_bits(_py, class_bits);
        dec_ref_bits(_py, metaclass_bits);
        dec_ref_bits(_py, call_bits);
        crate::call::bind::clear_call_bind_ic_cache(_py);
    });
}

#[test]
fn type_call_ic_invalidates_when_metaclass_call_policy_changes() {
    let _guard = CApiTestGuard::new();
    crate::with_gil_entry_nopanic!(_py, {
        crate::call::bind::clear_call_bind_ic_cache(_py);
        MUTATED_METACLASS_CALL_COUNT.store(0, Ordering::SeqCst);

        let builtins = crate::builtins::classes::builtin_classes(_py);
        let metaclass_bits = create_test_type(
            _py,
            builtins.type_obj,
            b"MutableMeta",
            builtins.type_obj,
            &[],
        );
        let class_bits = create_test_type(
            _py,
            metaclass_bits,
            b"InitiallyDefaultCall",
            builtins.object,
            &[],
        );
        let site_id = 131u64;

        let first_builder = crate::call::bind::molt_callargs_new(0, 0);
        let first = crate::call::bind::molt_call_bind_ic(
            MoltObject::from_int(site_id as i64).bits(),
            class_bits,
            first_builder,
        );
        assert!(obj_from_bits(first).as_ptr().is_some());
        assert!(crate::call::bind::call_bind_ic_site_cached_for_test(
            site_id
        ));
        dec_ref_bits(_py, first);

        let call_ptr = crate::builtins::functions::alloc_runtime_function_obj(
            _py,
            crate::builtins::functions::runtime_fn_addr(
                "c_api_test_mutated_metaclass_call",
                c_api_test_mutated_metaclass_call as *const (),
            ),
            1,
        );
        assert!(!call_ptr.is_null());
        let call_bits = MoltObject::from_ptr(call_ptr).bits();
        let call_name_bits = unsafe { molt_string_from(b"__call__".as_ptr(), 8) };
        let _ = crate::molt_set_attr_name(metaclass_bits, call_name_bits, call_bits);
        assert!(!exception_pending(_py));

        let second_builder = crate::call::bind::molt_callargs_new(0, 0);
        let second = crate::call::bind::molt_call_bind_ic(
            MoltObject::from_int(site_id as i64).bits(),
            class_bits,
            second_builder,
        );
        assert_eq!(to_i64(obj_from_bits(second)), Some(97));
        assert_eq!(MUTATED_METACLASS_CALL_COUNT.load(Ordering::SeqCst), 1);
        assert!(!exception_pending(_py));

        dec_ref_bits(_py, second);
        dec_ref_bits(_py, call_name_bits);
        dec_ref_bits(_py, call_bits);
        dec_ref_bits(_py, class_bits);
        dec_ref_bits(_py, metaclass_bits);
        crate::call::bind::clear_call_bind_ic_cache(_py);
    });
}

#[test]
fn empty_class_layout_memo_does_not_republish_type_epoch() {
    let _guard = CApiTestGuard::new();
    crate::with_gil_entry_nopanic!(_py, {
        let builtins = crate::builtins::classes::builtin_classes(_py);
        let class_bits = create_test_type(
            _py,
            builtins.type_obj,
            b"MemoizedEmptyLayout",
            builtins.object,
            &[],
        );
        let class_ptr = obj_from_bits(class_bits).as_ptr().expect("class ptr");
        let first = unsafe { crate::call::class_init::class_layout_size_cached(_py, class_ptr) };
        let epoch_after_publish = crate::global_type_version();
        let second = unsafe { crate::call::class_init::class_layout_size_cached(_py, class_ptr) };
        assert_eq!(second, first);
        assert_eq!(
            crate::global_type_version(),
            epoch_after_publish,
            "reading an already-published empty-class layout memo must not look like a type mutation"
        );
        dec_ref_bits(_py, class_bits);
    });
}

#[test]
fn heap_call_ic_invalidates_when_inherited_call_changes() {
    let _guard = CApiTestGuard::new();
    crate::with_gil_entry_nopanic!(_py, {
        crate::call::bind::clear_call_bind_ic_cache(_py);
        INHERITED_CALL_REPLACEMENT_COUNT.store(0, Ordering::SeqCst);

        let original_ptr = crate::builtins::functions::alloc_runtime_function_obj(
            _py,
            crate::builtins::functions::runtime_fn_addr(
                "c_api_test_bound_identity",
                c_api_test_bound_identity as *const (),
            ),
            2,
        );
        assert!(!original_ptr.is_null());
        let original_bits = MoltObject::from_ptr(original_ptr).bits();
        let base_bits = create_test_heap_class(
            _py,
            b"InheritedCallableBase",
            &[(b"__call__", original_bits)],
        );
        let builtins = crate::builtins::classes::builtin_classes(_py);
        let derived_bits = create_test_type(
            _py,
            builtins.type_obj,
            b"InheritedCallableDerived",
            base_bits,
            &[],
        );
        let derived_ptr = obj_from_bits(derived_bits)
            .as_ptr()
            .expect("derived class ptr");
        let instance_bits = unsafe { crate::alloc_instance_for_class(_py, derived_ptr) };
        let site_id = 137u64;

        let first_builder = crate::call::bind::molt_callargs_new(1, 0);
        let _ = unsafe {
            crate::molt_callargs_push_pos(first_builder, MoltObject::from_int(17).bits())
        };
        let first = crate::call::bind::molt_call_bind_ic(
            MoltObject::from_int(site_id as i64).bits(),
            instance_bits,
            first_builder,
        );
        assert_eq!(to_i64(obj_from_bits(first)), Some(17));
        assert!(crate::call::bind::call_bind_ic_site_cached_for_test(
            site_id
        ));
        dec_ref_bits(_py, first);

        let replacement_ptr = crate::builtins::functions::alloc_runtime_function_obj(
            _py,
            crate::builtins::functions::runtime_fn_addr(
                "c_api_test_replaced_inherited_call",
                c_api_test_replaced_inherited_call as *const (),
            ),
            2,
        );
        assert!(!replacement_ptr.is_null());
        let replacement_bits = MoltObject::from_ptr(replacement_ptr).bits();
        let call_name_bits = unsafe { molt_string_from(b"__call__".as_ptr(), 8) };
        let version_before = crate::global_type_version();
        let _ = crate::molt_set_attr_name(base_bits, call_name_bits, replacement_bits);
        assert!(crate::global_type_version() > version_before);
        assert!(!exception_pending(_py));

        let second_builder = crate::call::bind::molt_callargs_new(1, 0);
        let _ = unsafe {
            crate::molt_callargs_push_pos(second_builder, MoltObject::from_int(19).bits())
        };
        let second = crate::call::bind::molt_call_bind_ic(
            MoltObject::from_int(site_id as i64).bits(),
            instance_bits,
            second_builder,
        );
        assert_eq!(to_i64(obj_from_bits(second)), Some(19));
        assert_eq!(INHERITED_CALL_REPLACEMENT_COUNT.load(Ordering::SeqCst), 1);
        assert!(!exception_pending(_py));

        dec_ref_bits(_py, second);
        dec_ref_bits(_py, call_name_bits);
        dec_ref_bits(_py, replacement_bits);
        dec_ref_bits(_py, instance_bits);
        dec_ref_bits(_py, derived_bits);
        dec_ref_bits(_py, base_bits);
        dec_ref_bits(_py, original_bits);
        crate::call::bind::clear_call_bind_ic_cache(_py);
    });
}

#[test]
fn runtime_intrinsic_function_obj_zero_arg_fastcall_returns_value() {
    let _guard = CApiTestGuard::new();
    crate::with_gil_entry_nopanic!(_py, {
        let func_ptr = crate::builtins::functions::alloc_runtime_function_obj(
            _py,
            crate::builtins::functions::runtime_fn_addr(
                "crate::molt_sys_version_info",
                crate::molt_sys_version_info as *const (),
            ),
            0,
        );
        assert!(!func_ptr.is_null());
        let func_bits = MoltObject::from_ptr(func_ptr).bits();

        let out_bits = crate::molt_call_func_fast0(func_bits);
        let out_ptr = obj_from_bits(out_bits).as_ptr();
        assert!(out_ptr.is_some(), "expected tuple result, got none");
        assert_eq!(unsafe { object_type_id(out_ptr.unwrap()) }, TYPE_ID_TUPLE);
        assert!(!exception_pending(_py));

        dec_ref_bits(_py, out_bits);
        dec_ref_bits(_py, func_bits);
    });
}

#[test]
fn resolved_intrinsic_function_obj_zero_arg_indirect_call_returns_value() {
    let _guard = CApiTestGuard::new();
    crate::with_gil_entry_nopanic!(_py, {
        crate::intrinsics::registry::register_intrinsics_module(_py);

        let name_ptr = alloc_string(_py, b"molt_sys_version_info");
        assert!(!name_ptr.is_null());
        let name_bits = MoltObject::from_ptr(name_ptr).bits();
        let func_bits = crate::intrinsics::registry::molt_intrinsic_resolve(name_bits);
        assert!(
            !obj_from_bits(func_bits).is_none(),
            "resolver returned none"
        );

        let site_bits = MoltObject::from_int(19).bits();
        let builder_bits = crate::call::bind::molt_callargs_new(0, 0);
        let out_bits = crate::call::bind::molt_call_indirect_ic(site_bits, func_bits, builder_bits);
        let out_ptr = obj_from_bits(out_bits).as_ptr();
        assert!(out_ptr.is_some(), "expected tuple result, got none");
        assert_eq!(unsafe { object_type_id(out_ptr.unwrap()) }, TYPE_ID_TUPLE);
        assert!(!exception_pending(_py));

        dec_ref_bits(_py, out_bits);
        dec_ref_bits(_py, func_bits);
        dec_ref_bits(_py, name_bits);
    });
}

#[test]
fn intrinsic_resolver_function_obj_indirect_call_returns_callable() {
    let _guard = CApiTestGuard::new();
    crate::with_gil_entry_nopanic!(_py, {
        let resolver_ptr = crate::builtins::functions::alloc_runtime_function_obj(
            _py,
            crate::builtins::functions::runtime_fn_addr(
                "crate::intrinsics::registry::molt_intrinsic_resolve",
                crate::intrinsics::registry::molt_intrinsic_resolve as *const (),
            ),
            1,
        );
        assert!(!resolver_ptr.is_null());
        let resolver_bits = MoltObject::from_ptr(resolver_ptr).bits();

        let name_ptr = alloc_string(_py, b"molt_sys_version_info");
        assert!(!name_ptr.is_null());
        let name_bits = MoltObject::from_ptr(name_ptr).bits();
        let site_bits = MoltObject::from_int(23).bits();
        let builder_bits = crate::call::bind::molt_callargs_new(1, 0);
        let _ = unsafe { crate::molt_callargs_push_pos(builder_bits, name_bits) };

        let func_bits =
            crate::call::bind::molt_call_indirect_ic(site_bits, resolver_bits, builder_bits);
        let func_ptr = obj_from_bits(func_bits).as_ptr();
        assert!(func_ptr.is_some(), "resolver returned none");
        assert_eq!(
            unsafe { object_type_id(func_ptr.unwrap()) },
            TYPE_ID_FUNCTION
        );
        assert!(!exception_pending(_py));

        dec_ref_bits(_py, func_bits);
        dec_ref_bits(_py, name_bits);
        dec_ref_bits(_py, resolver_bits);
    });
}

#[test]
fn runtime_intrinsic_module_import_fast_call_returns_module() {
    let _guard = CApiTestGuard::new();
    crate::with_gil_entry_nopanic!(_py, {
        let func_ptr = crate::builtins::functions::alloc_runtime_function_obj(
            _py,
            crate::builtins::functions::runtime_fn_addr(
                "crate::molt_module_import",
                crate::molt_module_import as *const (),
            ),
            1,
        );
        assert!(!func_ptr.is_null());
        let func_bits = MoltObject::from_ptr(func_ptr).bits();

        let name_ptr = alloc_string(_py, b"sys");
        assert!(!name_ptr.is_null());
        let name_bits = MoltObject::from_ptr(name_ptr).bits();
        let cache_restore = CApiModuleCacheRestore::new(name_bits);

        let sys_bits = crate::builtins::modules::molt_module_new(cache_restore.name_bits());
        let sys_ptr = obj_from_bits(sys_bits)
            .as_ptr()
            .expect("test sys module allocation should return module");
        assert_eq!(unsafe { object_type_id(sys_ptr) }, TYPE_ID_MODULE);
        let cache_set_bits =
            crate::builtins::modules::molt_module_cache_set(cache_restore.name_bits(), sys_bits);
        assert!(obj_from_bits(cache_set_bits).is_none());
        assert!(!exception_pending(_py));

        let out_bits = crate::molt_call_func_fast1(func_bits, cache_restore.name_bits());
        let out_ptr = obj_from_bits(out_bits)
            .as_ptr()
            .expect("expected module import result");
        let ty = unsafe { object_type_id(out_ptr) };
        assert!(
            ty == TYPE_ID_MODULE || ty == TYPE_ID_DICT,
            "expected module-like result, got type_id={ty}"
        );
        assert!(!exception_pending(_py));

        dec_ref_bits(_py, out_bits);
        dec_ref_bits(_py, sys_bits);
        dec_ref_bits(_py, func_bits);
    });
}

#[test]
fn scalar_handle_helpers_roundtrip() {
    let _guard = CApiTestGuard::new();
    assert!(obj_from_bits(molt_none()).is_none());

    let true_bits = molt_bool_from_i32(1);
    let false_bits = molt_bool_from_i32(0);
    assert_eq!(molt_object_truthy(true_bits), 1);
    assert_eq!(molt_object_truthy(false_bits), 0);

    let int_bits = molt_int_from_i64(-42);
    assert_eq!(molt_int_as_i64(int_bits), -42);

    let float_bits = molt_float_from_f64(3.5);
    assert_eq!(molt_float_as_f64(float_bits), 3.5);
    assert_eq!(molt_float_as_f64(int_bits), -42.0);

    let heap_nan_bits = crate::with_gil_entry_nopanic!(_py, { float_result_bits(_py, f64::NAN) });
    assert!(molt_float_as_f64(heap_nan_bits).is_nan());
    assert_eq!(
        obj_from_bits(molt_eq(heap_nan_bits, heap_nan_bits)).as_bool(),
        Some(false)
    );
    assert_eq!(
        obj_from_bits(molt_ne(heap_nan_bits, heap_nan_bits)).as_bool(),
        Some(true)
    );

    crate::with_gil_entry_nopanic!(_py, {
        assert_eq!(molt_int_as_i64(float_bits), -1);
        assert_eq!(molt_err_pending(), 1);
        assert_eq!(molt_err_clear(), 0);
    });

    crate::with_gil_entry_nopanic!(_py, {
        dec_ref_bits(_py, true_bits);
        dec_ref_bits(_py, false_bits);
        dec_ref_bits(_py, int_bits);
        dec_ref_bits(_py, float_bits);
        dec_ref_bits(_py, heap_nan_bits);
    });
}

#[test]
fn scalar_extractors_preserve_pending_exception() {
    // A scalar extractor (`molt_float_as_f64` / `molt_int_as_i64`) is chained by
    // native codegen onto the result of a fallible boxing call (e.g.
    // `molt_float_from_obj` for `float(x)` feeding a raw-f64 lane). When that
    // prior call raised — e.g. `float("nope")` raised `ValueError` and returned
    // the `None` sentinel — the extractor receives the sentinel and fails its
    // type check. It must NOT clobber the already-pending exception with its
    // generic "X-compatible object expected" TypeError. Regression for
    // `float("nope")` surfacing `TypeError` instead of the real `ValueError`.
    let _guard = CApiTestGuard::new();
    crate::with_gil_entry_nopanic!(_py, {
        // Arrange a known pending exception, standing in for the prior raise.
        let runtime_error = runtime_error_type_bits(_py);
        let msg = b"prior-boom";
        assert_eq!(
            unsafe { molt_err_set(runtime_error, msg.as_ptr(), msg.len() as u64) },
            0
        );
        assert_eq!(molt_err_pending(), 1);
        let before_bits = molt_err_peek();

        // The float extractor returns its sentinel WITHOUT raising over the
        // pending exception — the current exception object stays identical.
        assert_eq!(molt_float_as_f64(molt_none()), -1.0);
        assert_eq!(molt_err_pending(), 1);
        let after_float = molt_err_peek();
        assert_eq!(after_float, before_bits);
        dec_ref_bits(_py, after_float);

        // Same invariant for the integer extractor.
        assert_eq!(molt_int_as_i64(molt_none()), -1);
        assert_eq!(molt_err_pending(), 1);
        let after_int = molt_err_peek();
        assert_eq!(after_int, before_bits);
        dec_ref_bits(_py, after_int);

        let _ = molt_err_clear();
        dec_ref_bits(_py, before_bits);
    });
}

#[test]
fn object_bytes_compare_and_contains_helpers() {
    let _guard = CApiTestGuard::new();
    crate::with_gil_entry_nopanic!(_py, {
        let runtime_error = runtime_error_type_bits(_py);
        let msg_ptr = alloc_string(_py, b"msg");
        assert!(!msg_ptr.is_null());
        let msg_bits = MoltObject::from_ptr(msg_ptr).bits();
        let exc_bits = molt_exception_new_from_class(runtime_error, msg_bits);
        assert!(!obj_from_bits(exc_bits).is_none());

        let value_bits = MoltObject::from_int(77).bits();
        let set_rc = unsafe {
            molt_object_setattr_bytes(
                exc_bits,
                b"custom".as_ptr(),
                b"custom".len() as u64,
                value_bits,
            )
        };
        assert_eq!(set_rc, 0);
        let got_bits = unsafe {
            molt_object_getattr_bytes(exc_bits, b"custom".as_ptr(), b"custom".len() as u64)
        };
        assert_eq!(to_i64(obj_from_bits(got_bits)), Some(77));
        dec_ref_bits(_py, got_bits);

        assert_eq!(
            molt_object_equal(
                MoltObject::from_int(5).bits(),
                MoltObject::from_int(5).bits()
            ),
            1
        );
        assert_eq!(
            molt_object_not_equal(
                MoltObject::from_int(5).bits(),
                MoltObject::from_int(6).bits()
            ),
            1
        );

        let list_ptr = alloc_list(
            _py,
            &[
                MoltObject::from_int(1).bits(),
                MoltObject::from_int(2).bits(),
                MoltObject::from_int(3).bits(),
            ],
        );
        assert!(!list_ptr.is_null());
        let list_bits = MoltObject::from_ptr(list_ptr).bits();
        assert_eq!(
            molt_object_contains(list_bits, MoltObject::from_int(2).bits()),
            1
        );
        assert_eq!(
            molt_object_contains(list_bits, MoltObject::from_int(9).bits()),
            0
        );

        dec_ref_bits(_py, list_bits);
        dec_ref_bits(_py, exc_bits);
        dec_ref_bits(_py, msg_bits);
    });
}

#[test]
fn array_constructors_roundtrip() {
    let _guard = CApiTestGuard::new();
    let elems = [
        MoltObject::from_int(10).bits(),
        MoltObject::from_int(20).bits(),
        MoltObject::from_int(30).bits(),
    ];
    let tuple_bits = unsafe { molt_tuple_from_array(elems.as_ptr(), elems.len() as u64) };
    let list_bits = unsafe { molt_list_from_array(elems.as_ptr(), elems.len() as u64) };
    assert!(!obj_from_bits(tuple_bits).is_none());
    assert!(!obj_from_bits(list_bits).is_none());
    assert_eq!(molt_sequence_length(tuple_bits), 3);
    assert_eq!(molt_sequence_length(list_bits), 3);

    let keys = [
        MoltObject::from_int(1).bits(),
        MoltObject::from_int(2).bits(),
    ];
    let values = [
        MoltObject::from_int(100).bits(),
        MoltObject::from_int(200).bits(),
    ];
    let dict_bits = unsafe { molt_dict_from_pairs(keys.as_ptr(), values.as_ptr(), 2) };
    assert!(!obj_from_bits(dict_bits).is_none());
    assert_eq!(molt_mapping_length(dict_bits), 2);
    let got_bits = molt_mapping_getitem(dict_bits, keys[1]);
    assert_eq!(to_i64(obj_from_bits(got_bits)), Some(200));
    crate::with_gil_entry_nopanic!(_py, {
        dec_ref_bits(_py, got_bits);
        dec_ref_bits(_py, tuple_bits);
        dec_ref_bits(_py, list_bits);
        dec_ref_bits(_py, dict_bits);
    });

    crate::with_gil_entry_nopanic!(_py, {
        let null_tuple_bits = unsafe { molt_tuple_from_array(std::ptr::null::<MoltHandle>(), 1) };
        assert!(obj_from_bits(null_tuple_bits).is_none());
        assert_eq!(molt_err_pending(), 1);
        assert_eq!(molt_err_clear(), 0);
    });
}

#[test]
fn type_ready_and_module_parity_wrappers_roundtrip() {
    let _guard = CApiTestGuard::new();
    crate::with_gil_entry_nopanic!(_py, {
        let builtins = crate::builtins::classes::builtin_classes(_py);
        assert_eq!(molt_type_ready(builtins.type_obj), 0);
        assert_eq!(molt_type_ready(MoltObject::from_int(1).bits()), -1);
        assert_eq!(molt_err_pending(), 1);
        assert_eq!(molt_err_clear(), 0);

        let module_name_bits = unsafe { molt_string_from(b"demo_ext".as_ptr(), 8) };
        assert!(!obj_from_bits(module_name_bits).is_none());
        let module_bits = molt_module_create(module_name_bits);
        assert!(!obj_from_bits(module_bits).is_none());

        let answer_name_ptr = alloc_string(_py, b"answer");
        assert!(!answer_name_ptr.is_null());
        let answer_name_bits = MoltObject::from_ptr(answer_name_ptr).bits();
        assert_eq!(
            molt_module_add_int_constant(module_bits, answer_name_bits, 42),
            0
        );
        let answer_bits = molt_module_get_object(module_bits, answer_name_bits);
        assert_eq!(to_i64(obj_from_bits(answer_bits)), Some(42));

        assert_eq!(
            unsafe {
                molt_module_add_object_bytes(
                    module_bits,
                    b"status".as_ptr(),
                    b"status".len() as u64,
                    MoltObject::from_int(7).bits(),
                )
            },
            0
        );
        let status_bits = unsafe {
            molt_module_get_object_bytes(module_bits, b"status".as_ptr(), b"status".len() as u64)
        };
        assert_eq!(to_i64(obj_from_bits(status_bits)), Some(7));

        let label_name_ptr = alloc_string(_py, b"label");
        assert!(!label_name_ptr.is_null());
        let label_name_bits = MoltObject::from_ptr(label_name_ptr).bits();
        assert_eq!(
            unsafe {
                molt_module_add_string_constant(module_bits, label_name_bits, b"ok".as_ptr(), 2)
            },
            0
        );
        let label_bits = molt_module_get_object(module_bits, label_name_bits);
        let mut label_len = 0u64;
        let label_ptr = unsafe { molt_string_as_ptr(label_bits, &mut label_len as *mut u64) };
        assert!(!label_ptr.is_null());
        assert_eq!(label_len, 2);
        let label_text = unsafe { std::slice::from_raw_parts(label_ptr, label_len as usize) };
        assert_eq!(label_text, b"ok");

        assert_eq!(molt_module_add_type(module_bits, builtins.type_obj), 0);
        let type_name_ptr = alloc_string(_py, b"type");
        assert!(!type_name_ptr.is_null());
        let type_name_bits = MoltObject::from_ptr(type_name_ptr).bits();
        let added_type_bits = molt_module_get_object(module_bits, type_name_bits);
        assert_eq!(molt_object_equal(added_type_bits, builtins.type_obj), 1);
        assert_eq!(
            molt_module_add_type(module_bits, MoltObject::from_int(1).bits()),
            -1
        );
        assert_eq!(molt_err_pending(), 1);
        assert_eq!(molt_err_clear(), 0);

        let dict_bits = molt_module_get_dict(module_bits);
        assert!(!obj_from_bits(dict_bits).is_none());
        assert!(molt_mapping_length(dict_bits) >= 4);

        dec_ref_bits(_py, added_type_bits);
        dec_ref_bits(_py, type_name_bits);
        dec_ref_bits(_py, dict_bits);
        dec_ref_bits(_py, label_bits);
        dec_ref_bits(_py, label_name_bits);
        dec_ref_bits(_py, status_bits);
        dec_ref_bits(_py, answer_bits);
        dec_ref_bits(_py, answer_name_bits);
        dec_ref_bits(_py, module_bits);
        dec_ref_bits(_py, module_name_bits);
    });
}

#[test]
fn module_capi_metadata_and_state_registry_roundtrip() {
    let _guard = CApiTestGuard::new();
    crate::with_gil_entry_nopanic!(_py, {
        let module_name_bits = unsafe { molt_string_from(b"demo_meta".as_ptr(), 9) };
        assert!(!obj_from_bits(module_name_bits).is_none());
        let module_bits = molt_module_create(module_name_bits);
        assert!(!obj_from_bits(module_bits).is_none());
        let module_ptr = obj_from_bits(module_bits)
            .as_ptr()
            .expect("module pointer should be valid");
        let module_def_ptr = 0xD15EA5Eusize;

        assert_eq!(
            molt_module_capi_register(module_bits, module_def_ptr, 32),
            0
        );
        assert_eq!(molt_module_capi_get_def(module_bits), module_def_ptr);
        let state_ptr = molt_module_capi_get_state(module_bits);
        assert!(!state_ptr.is_null());
        let state_slice = unsafe { std::slice::from_raw_parts_mut(state_ptr, 32) };
        for byte in state_slice.iter() {
            assert_eq!(*byte, 0);
        }
        state_slice[0] = 7;
        state_slice[31] = 9;

        assert_eq!(molt_module_state_add(module_bits, module_def_ptr), 0);
        assert_eq!(molt_module_state_find(module_def_ptr), module_bits);
        assert_eq!(molt_module_state_remove(module_def_ptr), 0);
        assert_eq!(molt_module_state_find(module_def_ptr), 0);

        assert_eq!(molt_module_state_remove(module_def_ptr), -1);
        assert_eq!(molt_err_pending(), 1);
        assert_eq!(molt_err_clear(), 0);

        assert_eq!(molt_module_state_add(module_bits, module_def_ptr), 0);
        assert!(c_api_module_detach_on_teardown(_py, module_ptr).is_none());
        assert_eq!(molt_module_capi_get_def(module_bits), 0);
        assert!(molt_module_capi_get_state(module_bits).is_null());
        assert_eq!(molt_module_state_find(module_def_ptr), 0);

        dec_ref_bits(_py, module_bits);
        dec_ref_bits(_py, module_name_bits);
    });
}

#[test]
fn module_capi_teardown_detaches_registry_edge_before_release() {
    let _guard = CApiTestGuard::new();
    crate::with_gil_entry_nopanic!(_py, {
        let owner = crate::alloc_list(_py, &[]);
        let referent = crate::alloc_list(_py, &[]);
        let owner_key = owner as usize;
        let referent_bits = MoltObject::from_ptr(referent).bits();
        let def_key = 0xDEC0DEusize;
        inc_ref_bits(_py, referent_bits);
        {
            let mut state = c_api_module_state(_py);
            state.state_registry.by_module.insert(owner_key, def_key);
            state.state_registry.by_def.insert(def_key, referent_bits);
        }

        let mut visited = Vec::new();
        c_api_module_visit_owned_edge(_py, owner, |bits| visited.push(bits));
        assert_eq!(visited, vec![referent_bits]);
        let detached = c_api_module_detach_on_teardown(_py, owner);
        assert_eq!(detached, Some(referent_bits));
        {
            let state = c_api_module_state(_py);
            assert!(!state.state_registry.by_module.contains_key(&owner_key));
            assert!(!state.state_registry.by_def.contains_key(&def_key));
        }

        // Releasing the detached edge happens only after both maps are empty.
        dec_ref_bits(_py, detached.expect("detached registry edge"));
        dec_ref_bits(_py, MoltObject::from_ptr(owner).bits());
        dec_ref_bits(_py, referent_bits);
    });
}

#[test]
fn module_capi_state_is_runtime_scoped_and_clearable() {
    let _guard = CApiTestGuard::new();
    crate::with_gil_entry_nopanic!(_py, {
        let state = runtime_state(_py);
        c_api_module_clear_state(_py, state);

        let module_name_bits = unsafe { molt_string_from(b"demo_capi_state".as_ptr(), 15) };
        assert!(!obj_from_bits(module_name_bits).is_none());
        let module_bits = molt_module_create(module_name_bits);
        assert!(!obj_from_bits(module_bits).is_none());
        let module_def_ptr = 0xC0FFEEusize;

        assert_eq!(
            molt_module_capi_register(module_bits, module_def_ptr, 16),
            0
        );
        assert_eq!(molt_module_state_add(module_bits, module_def_ptr), 0);
        {
            let guard = c_api_module_state(_py);
            assert_eq!(guard.metadata.len(), 1);
            assert_eq!(guard.state_registry.by_def.len(), 1);
            assert_eq!(guard.state_registry.by_module.len(), 1);
        }

        c_api_module_clear_state(_py, state);
        {
            let guard = c_api_module_state(_py);
            assert!(guard.metadata.is_empty());
            assert!(guard.state_registry.by_def.is_empty());
            assert!(guard.state_registry.by_module.is_empty());
        }
        assert_eq!(molt_module_capi_get_def(module_bits), 0);
        assert!(molt_module_capi_get_state(module_bits).is_null());
        assert_eq!(molt_module_state_find(module_def_ptr), 0);

        dec_ref_bits(_py, module_bits);
        dec_ref_bits(_py, module_name_bits);
    });
}

#[test]
fn module_capi_method_bridge_handles_supported_flags() {
    let _guard = CApiTestGuard::new();
    crate::with_gil_entry_nopanic!(_py, {
        let module_name_bits = unsafe { molt_string_from(b"demo_capi".as_ptr(), 9) };
        assert!(!obj_from_bits(module_name_bits).is_none());
        let module_bits = molt_module_create(module_name_bits);
        assert!(!obj_from_bits(module_bits).is_none());

        assert_eq!(
            unsafe {
                molt_module_add_cfunction_bytes(
                    module_bits,
                    b"meth_varargs".as_ptr(),
                    b"meth_varargs".len() as u64,
                    Some(c_api_test_meth_varargs),
                    C_API_METH_VARARGS,
                    b"varargs".as_ptr(),
                    b"varargs".len() as u64,
                )
            },
            0
        );
        assert_eq!(
            unsafe {
                molt_module_add_cfunction_keywords_bytes(
                    module_bits,
                    b"meth_kwargs".as_ptr(),
                    b"meth_kwargs".len() as u64,
                    Some(c_api_test_meth_varargs_keywords),
                    C_API_METH_VARARGS | C_API_METH_KEYWORDS,
                    std::ptr::null(),
                    0,
                )
            },
            0
        );
        assert_eq!(
            unsafe {
                molt_module_add_cfunction_bytes(
                    module_bits,
                    b"meth_noargs".as_ptr(),
                    b"meth_noargs".len() as u64,
                    Some(c_api_test_meth_noargs),
                    C_API_METH_NOARGS,
                    std::ptr::null(),
                    0,
                )
            },
            0
        );
        assert_eq!(
            unsafe {
                molt_module_add_cfunction_bytes(
                    module_bits,
                    b"meth_o".as_ptr(),
                    b"meth_o".len() as u64,
                    Some(c_api_test_meth_o),
                    C_API_METH_O,
                    std::ptr::null(),
                    0,
                )
            },
            0
        );

        let meth_varargs_bits =
            unsafe { molt_module_get_object_bytes(module_bits, b"meth_varargs".as_ptr(), 12) };
        let meth_kwargs_bits =
            unsafe { molt_module_get_object_bytes(module_bits, b"meth_kwargs".as_ptr(), 11) };
        let meth_noargs_bits =
            unsafe { molt_module_get_object_bytes(module_bits, b"meth_noargs".as_ptr(), 11) };
        let meth_o_bits =
            unsafe { molt_module_get_object_bytes(module_bits, b"meth_o".as_ptr(), 6) };

        let args3_ptr = alloc_tuple(
            _py,
            &[
                MoltObject::from_int(1).bits(),
                MoltObject::from_int(2).bits(),
                MoltObject::from_int(3).bits(),
            ],
        );
        assert!(!args3_ptr.is_null());
        let args3_bits = MoltObject::from_ptr(args3_ptr).bits();
        let out_varargs = molt_object_call(meth_varargs_bits, args3_bits, none_bits());
        assert_eq!(to_i64(obj_from_bits(out_varargs)), Some(3));
        dec_ref_bits(_py, out_varargs);

        let key_ptr = alloc_string(_py, b"k");
        assert!(!key_ptr.is_null());
        let key_bits = MoltObject::from_ptr(key_ptr).bits();
        let kwargs_ptr = alloc_dict_with_pairs(_py, &[key_bits, MoltObject::from_int(9).bits()]);
        assert!(!kwargs_ptr.is_null());
        let kwargs_bits = MoltObject::from_ptr(kwargs_ptr).bits();

        let out_kwargs = molt_object_call(meth_kwargs_bits, args3_bits, kwargs_bits);
        assert_eq!(to_i64(obj_from_bits(out_kwargs)), Some(31));
        dec_ref_bits(_py, out_kwargs);

        let reject_kwargs = molt_object_call(meth_varargs_bits, args3_bits, kwargs_bits);
        assert!(obj_from_bits(reject_kwargs).is_none());
        assert!(exception_pending(_py));
        let _ = molt_exception_clear();

        let args0_ptr = alloc_tuple(_py, &[]);
        assert!(!args0_ptr.is_null());
        let args0_bits = MoltObject::from_ptr(args0_ptr).bits();
        let out_noargs = molt_object_call(meth_noargs_bits, args0_bits, none_bits());
        assert_eq!(to_i64(obj_from_bits(out_noargs)), Some(101));
        dec_ref_bits(_py, out_noargs);

        let reject_noargs = molt_object_call(meth_noargs_bits, args3_bits, none_bits());
        assert!(obj_from_bits(reject_noargs).is_none());
        assert!(exception_pending(_py));
        let _ = molt_exception_clear();

        let args1_ptr = alloc_tuple(_py, &[MoltObject::from_int(55).bits()]);
        assert!(!args1_ptr.is_null());
        let args1_bits = MoltObject::from_ptr(args1_ptr).bits();
        let out_o = molt_object_call(meth_o_bits, args1_bits, none_bits());
        assert_eq!(to_i64(obj_from_bits(out_o)), Some(55));
        dec_ref_bits(_py, out_o);

        let reject_o = molt_object_call(meth_o_bits, args0_bits, none_bits());
        assert!(obj_from_bits(reject_o).is_none());
        assert!(exception_pending(_py));
        let _ = molt_exception_clear();

        dec_ref_bits(_py, args1_bits);
        dec_ref_bits(_py, args0_bits);
        dec_ref_bits(_py, kwargs_bits);
        dec_ref_bits(_py, key_bits);
        dec_ref_bits(_py, args3_bits);
        dec_ref_bits(_py, meth_o_bits);
        dec_ref_bits(_py, meth_noargs_bits);
        dec_ref_bits(_py, meth_kwargs_bits);
        dec_ref_bits(_py, meth_varargs_bits);
        dec_ref_bits(_py, module_bits);
        dec_ref_bits(_py, module_name_bits);
    });
}

#[test]
fn module_capi_method_bridge_rejects_unsupported_flags() {
    let _guard = CApiTestGuard::new();
    crate::with_gil_entry_nopanic!(_py, {
        let module_name_bits = unsafe { molt_string_from(b"demo_bad".as_ptr(), 8) };
        assert!(!obj_from_bits(module_name_bits).is_none());
        let module_bits = molt_module_create(module_name_bits);
        assert!(!obj_from_bits(module_bits).is_none());

        let rc = unsafe {
            molt_module_add_cfunction_bytes(
                module_bits,
                b"bad".as_ptr(),
                3,
                Some(c_api_test_meth_varargs),
                C_API_METH_VARARGS | C_API_METH_O,
                std::ptr::null(),
                0,
            )
        };
        assert_eq!(rc, -1);
        assert!(exception_pending(_py));
        let _ = molt_exception_clear();

        dec_ref_bits(_py, module_bits);
        dec_ref_bits(_py, module_name_bits);
    });
}

#[test]
fn c_api_method_dispatch_supports_dynamic_self_callbacks() {
    let _guard = CApiTestGuard::new();
    crate::with_gil_entry_nopanic!(_py, {
        let dyn_varargs_bits = unsafe {
            molt_cfunction_create_bytes(
                none_bits(),
                b"dyn_varargs".as_ptr(),
                b"dyn_varargs".len() as u64,
                Some(c_api_test_dynamic_varargs),
                C_API_METH_VARARGS,
                std::ptr::null(),
                0,
            )
        };
        let dyn_noargs_bits = unsafe {
            molt_cfunction_create_bytes(
                none_bits(),
                b"dyn_noargs".as_ptr(),
                b"dyn_noargs".len() as u64,
                Some(c_api_test_dynamic_noargs),
                C_API_METH_NOARGS,
                std::ptr::null(),
                0,
            )
        };
        let dyn_o_bits = unsafe {
            molt_cfunction_create_bytes(
                none_bits(),
                b"dyn_o".as_ptr(),
                b"dyn_o".len() as u64,
                Some(c_api_test_dynamic_o),
                C_API_METH_O,
                std::ptr::null(),
                0,
            )
        };
        assert!(!obj_from_bits(dyn_varargs_bits).is_none());
        assert!(!obj_from_bits(dyn_noargs_bits).is_none());
        assert!(!obj_from_bits(dyn_o_bits).is_none());

        let args_var_ptr = alloc_tuple(
            _py,
            &[
                MoltObject::from_int(40).bits(),
                MoltObject::from_int(1).bits(),
                MoltObject::from_int(2).bits(),
            ],
        );
        assert!(!args_var_ptr.is_null());
        let args_var_bits = MoltObject::from_ptr(args_var_ptr).bits();
        let out_var = molt_object_call(dyn_varargs_bits, args_var_bits, none_bits());
        assert_eq!(to_i64(obj_from_bits(out_var)), Some(402));
        dec_ref_bits(_py, out_var);

        let args_none_ptr = alloc_tuple(_py, &[MoltObject::from_int(7).bits()]);
        assert!(!args_none_ptr.is_null());
        let args_none_bits = MoltObject::from_ptr(args_none_ptr).bits();
        let out_noargs = molt_object_call(dyn_noargs_bits, args_none_bits, none_bits());
        assert_eq!(to_i64(obj_from_bits(out_noargs)), Some(1007));
        dec_ref_bits(_py, out_noargs);

        let args_o_ptr = alloc_tuple(
            _py,
            &[
                MoltObject::from_int(5).bits(),
                MoltObject::from_int(9).bits(),
            ],
        );
        assert!(!args_o_ptr.is_null());
        let args_o_bits = MoltObject::from_ptr(args_o_ptr).bits();
        let out_o = molt_object_call(dyn_o_bits, args_o_bits, none_bits());
        assert_eq!(to_i64(obj_from_bits(out_o)), Some(509));
        dec_ref_bits(_py, out_o);

        let args_missing_self_ptr = alloc_tuple(_py, &[]);
        assert!(!args_missing_self_ptr.is_null());
        let args_missing_self_bits = MoltObject::from_ptr(args_missing_self_ptr).bits();
        let reject_missing_self =
            molt_object_call(dyn_varargs_bits, args_missing_self_bits, none_bits());
        assert!(obj_from_bits(reject_missing_self).is_none());
        assert!(exception_pending(_py));
        let _ = molt_exception_clear();

        let args_bad_noargs_ptr = alloc_tuple(
            _py,
            &[
                MoltObject::from_int(7).bits(),
                MoltObject::from_int(1).bits(),
            ],
        );
        assert!(!args_bad_noargs_ptr.is_null());
        let args_bad_noargs_bits = MoltObject::from_ptr(args_bad_noargs_ptr).bits();
        let reject_noargs = molt_object_call(dyn_noargs_bits, args_bad_noargs_bits, none_bits());
        assert!(obj_from_bits(reject_noargs).is_none());
        assert!(exception_pending(_py));
        let _ = molt_exception_clear();

        let args_bad_o_ptr = alloc_tuple(_py, &[MoltObject::from_int(7).bits()]);
        assert!(!args_bad_o_ptr.is_null());
        let args_bad_o_bits = MoltObject::from_ptr(args_bad_o_ptr).bits();
        let reject_o = molt_object_call(dyn_o_bits, args_bad_o_bits, none_bits());
        assert!(obj_from_bits(reject_o).is_none());
        assert!(exception_pending(_py));
        let _ = molt_exception_clear();

        dec_ref_bits(_py, args_bad_o_bits);
        dec_ref_bits(_py, args_bad_noargs_bits);
        dec_ref_bits(_py, args_missing_self_bits);
        dec_ref_bits(_py, args_o_bits);
        dec_ref_bits(_py, args_none_bits);
        dec_ref_bits(_py, args_var_bits);
        dec_ref_bits(_py, dyn_o_bits);
        dec_ref_bits(_py, dyn_noargs_bits);
        dec_ref_bits(_py, dyn_varargs_bits);
    });
}

#[test]
fn c_api_method_dispatch_supports_null_self_for_static_callbacks() {
    let _guard = CApiTestGuard::new();
    crate::with_gil_entry_nopanic!(_py, {
        let static_noargs_bits = unsafe {
            molt_cfunction_create_bytes(
                0,
                b"static_noargs".as_ptr(),
                b"static_noargs".len() as u64,
                Some(c_api_test_static_noargs),
                C_API_METH_NOARGS,
                std::ptr::null(),
                0,
            )
        };
        assert!(!obj_from_bits(static_noargs_bits).is_none());

        let args_empty_ptr = alloc_tuple(_py, &[]);
        assert!(!args_empty_ptr.is_null());
        let args_empty_bits = MoltObject::from_ptr(args_empty_ptr).bits();
        let out = molt_object_call(static_noargs_bits, args_empty_bits, none_bits());
        assert_eq!(to_i64(obj_from_bits(out)), Some(204));
        dec_ref_bits(_py, out);

        let args_bad_ptr = alloc_tuple(_py, &[MoltObject::from_int(1).bits()]);
        assert!(!args_bad_ptr.is_null());
        let args_bad_bits = MoltObject::from_ptr(args_bad_ptr).bits();
        let reject = molt_object_call(static_noargs_bits, args_bad_bits, none_bits());
        assert!(obj_from_bits(reject).is_none());
        assert!(exception_pending(_py));
        let _ = molt_exception_clear();

        dec_ref_bits(_py, args_bad_bits);
        dec_ref_bits(_py, args_empty_bits);
        dec_ref_bits(_py, static_noargs_bits);
    });
}
