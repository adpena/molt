//! FFI bridge shims for `molt-runtime-zoneinfo`.

use crate::object::ops::string_obj_to_owned as _string_obj_to_owned;
use crate::*;

// ---------------------------------------------------------------------------
// Exception / error handling
// ---------------------------------------------------------------------------

#[unsafe(export_name = molt_codegen_abi::attached_symbol!("__molt_zoneinfo_raise_exception"))]
pub extern "C" fn __molt_zoneinfo_raise_exception(
    __molt_context: u64,
    kind_code: u16,
    msg_ptr: *const u8,
    msg_len: usize,
) -> u64 {
    {
        let _py = unsafe { crate::PyToken::from_attached_context(__molt_context) };
        let msg = unsafe { std::slice::from_raw_parts(msg_ptr, msg_len) };
        crate::builtins::exceptions::raise_exception_code_bytes(_py, kind_code, msg)
    }
}

// ---------------------------------------------------------------------------
// Object allocation
// ---------------------------------------------------------------------------

#[unsafe(export_name = molt_codegen_abi::attached_symbol!("__molt_zoneinfo_alloc_string"))]
pub extern "C" fn __molt_zoneinfo_alloc_string(
    __molt_context: u64,
    data_ptr: *const u8,
    data_len: usize,
) -> *mut u8 {
    {
        let _py = unsafe { crate::PyToken::from_attached_context(__molt_context) };
        let data = unsafe { std::slice::from_raw_parts(data_ptr, data_len) };
        alloc_string(_py, data)
    }
}

#[unsafe(export_name = molt_codegen_abi::attached_symbol!("__molt_zoneinfo_alloc_set_with_entries"))]
pub extern "C" fn __molt_zoneinfo_alloc_set_with_entries(
    __molt_context: u64,
    elems_ptr: *const u64,
    elems_len: usize,
) -> *mut u8 {
    {
        let _py = unsafe { crate::PyToken::from_attached_context(__molt_context) };
        let elems = unsafe { std::slice::from_raw_parts(elems_ptr, elems_len) };
        alloc_set_with_entries(_py, elems)
    }
}

// ---------------------------------------------------------------------------
// Object inspection
// ---------------------------------------------------------------------------

#[unsafe(no_mangle)]
pub extern "C" fn __molt_zoneinfo_string_obj_to_owned(
    bits: u64,
    out_ptr: *mut *const u8,
    out_len: *mut usize,
) -> i32 {
    let obj = obj_from_bits(bits);
    match _string_obj_to_owned(obj) {
        Some(s) => {
            let bytes = s.into_bytes().into_boxed_slice();
            unsafe { crate::resource::bridge_buffer::export_u8_box(bytes, out_ptr, out_len) }
        }
        None => 0,
    }
}

#[unsafe(no_mangle)]
pub extern "C" fn __molt_zoneinfo_object_type_id(ptr: *mut u8) -> u32 {
    unsafe { object_type_id(ptr) }
}

#[unsafe(export_name = molt_codegen_abi::attached_symbol!("__molt_zoneinfo_seq_snapshot"))]
pub extern "C" fn __molt_zoneinfo_seq_snapshot(
    __molt_context: u64,
    ptr: *mut u8,
    out_ptr: *mut *const u64,
    out_len: *mut usize,
) -> i32 {
    {
        let _py = unsafe { crate::PyToken::from_attached_context(__molt_context) };
        unsafe { crate::seq_snapshot_bridge::export(_py, ptr, out_ptr, out_len) }
    }
}

// ---------------------------------------------------------------------------
// Numeric helpers
// ---------------------------------------------------------------------------

#[unsafe(no_mangle)]
pub extern "C" fn __molt_zoneinfo_to_i64(bits: u64, out: *mut i64) -> i32 {
    let obj = obj_from_bits(bits);
    match to_i64(obj) {
        Some(v) => {
            unsafe {
                *out = v;
            }
            1
        }
        None => 0,
    }
}

#[unsafe(export_name = molt_codegen_abi::attached_symbol!("__molt_zoneinfo_int_bits_from_i64"))]
pub extern "C" fn __molt_zoneinfo_int_bits_from_i64(__molt_context: u64, val: i64) -> u64 {
    {
        let _py = unsafe { crate::PyToken::from_attached_context(__molt_context) };
        int_bits_from_i64(_py, val)
    }
}
