//! FFI bridge shims for `molt-runtime-xml`.
//!
//! Each function here is a thin `#[no_mangle] extern "C"` wrapper around an
//! internal `pub(crate)` function.  The xml crate declares matching
//! `extern "C"` imports and they are resolved at link time.

use crate::object::ops::string_obj_to_owned as _string_obj_to_owned;
use crate::*;

// ---------------------------------------------------------------------------
// Exception / error handling
// ---------------------------------------------------------------------------

#[unsafe(export_name = molt_codegen_abi::attached_symbol!("__molt_xml_raise_exception"))]
pub extern "C" fn __molt_xml_raise_exception(
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

#[unsafe(export_name = molt_codegen_abi::attached_symbol!("__molt_xml_alloc_tuple"))]
pub extern "C" fn __molt_xml_alloc_tuple(
    __molt_context: u64,
    elems_ptr: *const u64,
    elems_len: usize,
) -> *mut u8 {
    {
        let _py = unsafe { crate::PyToken::from_attached_context(__molt_context) };
        let elems = unsafe { std::slice::from_raw_parts(elems_ptr, elems_len) };
        alloc_tuple(_py, elems)
    }
}

#[unsafe(export_name = molt_codegen_abi::attached_symbol!("__molt_xml_alloc_list"))]
pub extern "C" fn __molt_xml_alloc_list(
    __molt_context: u64,
    elems_ptr: *const u64,
    elems_len: usize,
) -> *mut u8 {
    {
        let _py = unsafe { crate::PyToken::from_attached_context(__molt_context) };
        let elems = unsafe { std::slice::from_raw_parts(elems_ptr, elems_len) };
        alloc_list(_py, elems)
    }
}

#[unsafe(export_name = molt_codegen_abi::attached_symbol!("__molt_xml_alloc_string"))]
pub extern "C" fn __molt_xml_alloc_string(
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

// ---------------------------------------------------------------------------
// Object inspection
// ---------------------------------------------------------------------------

#[unsafe(no_mangle)]
pub extern "C" fn __molt_xml_string_obj_to_owned(
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

#[unsafe(export_name = molt_codegen_abi::attached_symbol!("__molt_xml_is_truthy"))]
pub extern "C" fn __molt_xml_is_truthy(__molt_context: u64, bits: u64) -> i32 {
    {
        let _py = unsafe { crate::PyToken::from_attached_context(__molt_context) };
        let obj = obj_from_bits(bits);
        if is_truthy(_py, obj) { 1 } else { 0 }
    }
}

// ---------------------------------------------------------------------------
// Reference counting
// ---------------------------------------------------------------------------

#[unsafe(export_name = molt_codegen_abi::attached_symbol!("__molt_xml_inc_ref_bits"))]
pub extern "C" fn __molt_xml_inc_ref_bits(__molt_context: u64, bits: u64) {
    {
        let _py = unsafe { crate::PyToken::from_attached_context(__molt_context) };
        inc_ref_bits(_py, bits);
    }
}

// ---------------------------------------------------------------------------
// Numeric helpers
// ---------------------------------------------------------------------------

#[unsafe(no_mangle)]
pub extern "C" fn __molt_xml_to_i64(bits: u64, out: *mut i64) -> i32 {
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

#[unsafe(export_name = molt_codegen_abi::attached_symbol!("__molt_xml_int_bits_from_i64"))]
pub extern "C" fn __molt_xml_int_bits_from_i64(__molt_context: u64, val: i64) -> u64 {
    {
        let _py = unsafe { crate::PyToken::from_attached_context(__molt_context) };
        int_bits_from_i64(_py, val)
    }
}
