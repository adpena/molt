//! Tests for the RuntimeHooks vtable and stub hooks.

#![allow(non_snake_case)]

use molt_cpython_abi::hooks::{DecodedHandleResult, hooks_or_stubs};
use std::ptr;

fn init() {
    molt_cpython_abi::bridge::molt_cpython_abi_init();
}

// ---------------------------------------------------------------------------
// hooks_or_stubs returns stubs when no runtime registered
// ---------------------------------------------------------------------------

#[test]
fn test_hooks_or_stubs_returns_stubs() {
    init();
    let h = hooks_or_stubs();
    // In test context, no runtime is registered, so we get stubs
    // Verify stub functions return expected fallback values
    let str_bits = unsafe { (h.alloc_str)(b"hello".as_ptr(), 5) };
    assert_eq!(str_bits, 0);

    let bytes_bits = unsafe { (h.alloc_bytes)(b"data".as_ptr(), 4) };
    assert_eq!(bytes_bits, 0);

    let int_bits = unsafe { (h.int_from_i64)(i64::MAX) };
    assert_eq!(int_bits, 0);

    let uint_bits = unsafe { (h.int_from_u64)(u64::MAX) };
    assert_eq!(uint_bits, 0);

    let int_value = unsafe { (h.int_as_i64)(0) };
    assert_eq!(int_value, -1);

    let list_bits = unsafe { (h.alloc_list)() };
    assert_eq!(list_bits, 0);

    let tuple_bits = unsafe { (h.alloc_tuple)(3) };
    assert_eq!(tuple_bits, 0);

    let dict_bits = unsafe { (h.alloc_dict)() };
    assert_eq!(dict_bits, 0);
}

#[test]
fn test_stub_list_operations() {
    init();
    let h = hooks_or_stubs();

    // list_len / list_item on nonexistent list
    let len = unsafe { (h.list_len)(0) };
    assert_eq!(len, 0);

    let item = unsafe { (h.list_item)(0, 0) };
    assert!(matches!(item.decode(), DecodedHandleResult::Error));

    assert_eq!(unsafe { (h.list_append)(0, 0, std::ptr::null_mut()) }, -1);
}

#[test]
fn test_stub_tuple_operations() {
    init();
    let h = hooks_or_stubs();

    let len = unsafe { (h.tuple_len)(0) };
    assert_eq!(len, 0);

    let item = unsafe { (h.tuple_item)(0, 0) };
    assert!(matches!(item.decode(), DecodedHandleResult::Error));

    assert!(matches!(
        unsafe { (h.tuple_set)(0, 0, 0, std::ptr::null_mut()) }.decode(),
        DecodedHandleResult::Error
    ));
}

#[test]
fn test_stub_dict_operations() {
    init();
    let h = hooks_or_stubs();

    let len = unsafe { (h.dict_len)(0) };
    assert_eq!(len, 0);

    let val = unsafe { (h.dict_get)(0, 0) };
    assert!(matches!(val.decode(), DecodedHandleResult::Error));

    assert_eq!(unsafe { (h.dict_set)(0, 0, 0) }, -1);
}

#[test]
fn test_stub_str_data() {
    init();
    let h = hooks_or_stubs();
    let mut len: usize = 999;
    let ptr = unsafe { (h.str_data)(0, &mut len) };
    assert!(!ptr.is_null());
    assert_eq!(len, 0);
}

#[test]
fn test_stub_bytes_data() {
    init();
    let h = hooks_or_stubs();
    let mut len: usize = 999;
    let ptr = unsafe { (h.bytes_data)(0, &mut len) };
    assert!(ptr.is_null());
    assert_eq!(len, 0);
}

#[test]
fn test_stub_str_data_null_out_len() {
    init();
    let h = hooks_or_stubs();
    // Should not crash when out_len is null
    let ptr = unsafe { (h.str_data)(0, ptr::null_mut()) };
    assert!(!ptr.is_null());
}

#[test]
fn test_stub_bytes_data_null_out_len() {
    init();
    let h = hooks_or_stubs();
    let ptr = unsafe { (h.bytes_data)(0, ptr::null_mut()) };
    assert!(ptr.is_null());
}

#[test]
fn test_stub_buffer_hooks_fail_closed_and_clear_view() {
    init();
    let h = hooks_or_stubs();
    let mut view = molt_cpython_abi::hooks::MoltBufferView {
        data: std::ptr::dangling_mut::<u8>(),
        len: 8,
        readonly: 0,
        ..molt_cpython_abi::hooks::MoltBufferView::default()
    };
    assert_eq!(unsafe { (h.buffer_acquire)(0, &mut view) }, -1);
    assert!(view.data.is_null());
    assert_eq!(unsafe { (h.buffer_release)(&mut view) }, 0);
    assert!(view.data.is_null());
}

#[test]
fn test_stub_classify_heap() {
    init();
    let h = hooks_or_stubs();
    let tag = unsafe { (h.classify_heap)(0) };
    assert_eq!(tag, molt_cpython_abi::abi_types::MoltTypeTag::Other as u8);
}

#[test]
fn test_stub_inc_dec_ref_no_crash() {
    init();
    let h = hooks_or_stubs();
    // Should be noops
    unsafe { (h.inc_ref)(0) };
    unsafe { (h.dec_ref)(0) };
    unsafe { (h.inc_ref)(12345) };
    unsafe { (h.dec_ref)(12345) };
}

// ---------------------------------------------------------------------------
// F2/F3/F6 teeth: the numeric and dict hooks return explicit errors under stubs
// so the ABI never fabricates a wrong answer when the runtime authority is
// absent. (The real bignum-correct / exception-setting behavior lives in the
// runtime authority and is proved there; here we prove the stub fails closed.)
// ---------------------------------------------------------------------------

#[test]
fn test_stub_number_hooks_fail_closed() {
    init();
    let h = hooks_or_stubs();
    // Every discriminant must return the typed error status under the stub table.
    for op in 0..12u32 {
        assert!(matches!(
            unsafe { (h.number_binary_op)(op, 1, 2) }.decode(),
            DecodedHandleResult::Error
        ));
    }
    for op in 0..4u32 {
        assert!(matches!(
            unsafe { (h.number_unary_op)(op, 1) }.decode(),
            DecodedHandleResult::Error
        ));
    }
    assert!(matches!(
        unsafe { (h.number_power)(2, 3, 0) }.decode(),
        DecodedHandleResult::Error
    ));
    assert!(matches!(
        unsafe { (h.number_power)(2, 3, 5) }.decode(),
        DecodedHandleResult::Error
    ));
}

#[test]
fn test_stub_dict_op_hook_fails_closed() {
    init();
    let h = hooks_or_stubs();
    for op in 0..3u32 {
        assert_eq!(unsafe { (h.dict_op)(op, 0) }, 0);
    }
}

// ---------------------------------------------------------------------------
// F1 teeth: PyArg_ParseTuple must NOT fake success. A format that requires a
// positional argument, against an empty argument tuple, must return 0 (failure)
// with an exception set — never 1 (success). Previously an unknown/unsatisfied
// format unit could still return 1, poisoning the caller with uninitialized
// output slots.
// ---------------------------------------------------------------------------

#[test]
fn test_pyarg_parse_missing_required_arg_fails_closed() {
    init();
    unsafe { molt_cpython_abi::api::errors::PyErr_Clear() };
    // args tuple is 0 (empty / None under stubs); format "i" wants one int.
    let mut out_slot: std::os::raw::c_int = 4242;
    let mut outs: [*mut std::ffi::c_void; 1] = [(&mut out_slot as *mut std::os::raw::c_int).cast()];
    let rc = unsafe {
        molt_cpython_abi::api::errors::molt_pyarg_parse_tuple_inner(
            ptr::null_mut(),
            c"i".as_ptr(),
            outs.as_mut_ptr(),
            1,
        )
    };
    assert_eq!(
        rc, 0,
        "PyArg_ParseTuple must fail (0), not fake success (1)"
    );
    assert!(
        !unsafe { molt_cpython_abi::api::errors::PyErr_Occurred() }.is_null(),
        "a failed PyArg_ParseTuple must leave an exception set"
    );
    unsafe { molt_cpython_abi::api::errors::PyErr_Clear() };
}
