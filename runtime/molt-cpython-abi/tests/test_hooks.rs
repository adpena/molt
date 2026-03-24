//! Tests for the RuntimeHooks vtable and stub hooks.

#![allow(non_snake_case)]

use molt_cpython_abi::hooks::hooks_or_stubs;
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
    assert_eq!(item, 0);

    // list_append should not crash
    unsafe { (h.list_append)(0, 0) };
}

#[test]
fn test_stub_tuple_operations() {
    init();
    let h = hooks_or_stubs();

    let len = unsafe { (h.tuple_len)(0) };
    assert_eq!(len, 0);

    let item = unsafe { (h.tuple_item)(0, 0) };
    assert_eq!(item, 0);

    // tuple_set should not crash
    unsafe { (h.tuple_set)(0, 0, 0) };
}

#[test]
fn test_stub_dict_operations() {
    init();
    let h = hooks_or_stubs();

    let len = unsafe { (h.dict_len)(0) };
    assert_eq!(len, 0);

    let val = unsafe { (h.dict_get)(0, 0) };
    assert_eq!(val, 0);

    // dict_set should not crash
    unsafe { (h.dict_set)(0, 0, 0) };
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
