//! FFI bridge shims for `molt-stdlib-text`.
//!
//! Each function here is a thin `#[no_mangle] extern "C"` wrapper around an
//! internal `pub(crate)` function.  The text crate declares matching
//! `extern "C"` imports and they are resolved at link time.

use crate::BuiltinExc;
use crate::object::ops::string_obj_to_owned as _string_obj_to_owned;
use crate::object::ops::type_name as _type_name;
use crate::*;

#[unsafe(export_name = molt_codegen_abi::attached_symbol!("__molt_text_east_asian_width"))]
pub extern "C" fn __molt_text_east_asian_width(__molt_context: u64, bits: u64) -> u64 {
    {
        let _py = unsafe { crate::PyToken::from_attached_context(__molt_context) };
        let Some(ptr) = obj_from_bits(bits)
            .as_ptr()
            .filter(|ptr| unsafe { object_type_id(*ptr) == TYPE_ID_STRING })
        else {
            return raise_exception::<u64>(_py, BuiltinExc::TypeError, "argument must be str");
        };
        let bytes = unsafe { std::slice::from_raw_parts(string_bytes(ptr), string_len(ptr)) };
        let mut codes = molt_obj_model::text_storage::code_points(bytes);
        let Some(code) = codes.next().filter(|_| codes.next().is_none()) else {
            return raise_exception::<u64>(
                _py,
                BuiltinExc::TypeError,
                "need a single Unicode character as parameter",
            );
        };
        let width = crate::object::ops::unicode_data(_py).east_asian_width(code);
        let ptr = alloc_string(_py, width.as_bytes());
        if ptr.is_null() {
            MoltObject::none().bits()
        } else {
            MoltObject::from_ptr(ptr).bits()
        }
    }
}

#[unsafe(export_name = molt_codegen_abi::attached_symbol!("__molt_text_unidata_version"))]
pub extern "C" fn __molt_text_unidata_version(__molt_context: u64) -> u64 {
    {
        let _py = unsafe { crate::PyToken::from_attached_context(__molt_context) };
        let version = crate::object::ops::unicode_data(_py).version;
        let ptr = alloc_string(_py, version.as_bytes());
        if ptr.is_null() {
            MoltObject::none().bits()
        } else {
            MoltObject::from_ptr(ptr).bits()
        }
    }
}

// ---------------------------------------------------------------------------
// Exception / error handling
// ---------------------------------------------------------------------------

#[unsafe(export_name = molt_codegen_abi::attached_symbol!("__molt_text_raise_exception"))]
pub extern "C" fn __molt_text_raise_exception(
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

#[unsafe(export_name = molt_codegen_abi::attached_symbol!("__molt_text_exception_pending"))]
pub extern "C" fn __molt_text_exception_pending(__molt_context: u64) -> i32 {
    {
        let _py = unsafe { crate::PyToken::from_attached_context(__molt_context) };
        if exception_pending(_py) { 1 } else { 0 }
    }
}

// ---------------------------------------------------------------------------
// Object allocation
// ---------------------------------------------------------------------------

#[unsafe(export_name = molt_codegen_abi::attached_symbol!("__molt_text_alloc_tuple"))]
pub extern "C" fn __molt_text_alloc_tuple(
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

#[unsafe(export_name = molt_codegen_abi::attached_symbol!("__molt_text_alloc_list"))]
pub extern "C" fn __molt_text_alloc_list(
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

#[unsafe(export_name = molt_codegen_abi::attached_symbol!("__molt_text_alloc_string"))]
pub extern "C" fn __molt_text_alloc_string(
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

#[unsafe(export_name = molt_codegen_abi::attached_symbol!("__molt_text_alloc_dict_with_pairs"))]
pub extern "C" fn __molt_text_alloc_dict_with_pairs(
    __molt_context: u64,
    pairs_ptr: *const u64,
    pairs_len: usize,
) -> *mut u8 {
    {
        let _py = unsafe { crate::PyToken::from_attached_context(__molt_context) };
        let pairs = unsafe { std::slice::from_raw_parts(pairs_ptr, pairs_len) };
        alloc_dict_with_pairs(_py, pairs)
    }
}

// ---------------------------------------------------------------------------
// Object inspection
// ---------------------------------------------------------------------------

#[unsafe(no_mangle)]
pub extern "C" fn __molt_text_string_obj_to_owned(
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

#[unsafe(export_name = molt_codegen_abi::attached_symbol!("__molt_text_is_truthy"))]
pub extern "C" fn __molt_text_is_truthy(__molt_context: u64, bits: u64) -> i32 {
    {
        let _py = unsafe { crate::PyToken::from_attached_context(__molt_context) };
        let obj = obj_from_bits(bits);
        if is_truthy(_py, obj) { 1 } else { 0 }
    }
}

#[unsafe(export_name = molt_codegen_abi::attached_symbol!("__molt_text_type_name"))]
pub extern "C" fn __molt_text_type_name(
    __molt_context: u64,
    bits: u64,
    out_ptr: *mut *const u8,
    out_len: *mut usize,
) -> i32 {
    {
        let _py = unsafe { crate::PyToken::from_attached_context(__molt_context) };
        let obj = obj_from_bits(bits);
        let name = _type_name(_py, obj);
        let bytes = name.into_owned().into_bytes().into_boxed_slice();
        unsafe { crate::resource::bridge_buffer::export_u8_box(bytes, out_ptr, out_len) }
    }
}

// ---------------------------------------------------------------------------
// Reference counting
// ---------------------------------------------------------------------------

#[unsafe(export_name = molt_codegen_abi::attached_symbol!("__molt_text_dec_ref_bits"))]
pub extern "C" fn __molt_text_dec_ref_bits(__molt_context: u64, bits: u64) {
    {
        let _py = unsafe { crate::PyToken::from_attached_context(__molt_context) };
        dec_ref_bits(_py, bits);
    }
}

#[unsafe(export_name = molt_codegen_abi::attached_symbol!("__molt_text_inc_ref_bits"))]
pub extern "C" fn __molt_text_inc_ref_bits(__molt_context: u64, bits: u64) {
    {
        let _py = unsafe { crate::PyToken::from_attached_context(__molt_context) };
        inc_ref_bits(_py, bits);
    }
}

// ---------------------------------------------------------------------------
// Numeric helpers
// ---------------------------------------------------------------------------

#[unsafe(no_mangle)]
pub extern "C" fn __molt_text_to_i64(bits: u64, out: *mut i64) -> i32 {
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

#[unsafe(export_name = molt_codegen_abi::attached_symbol!("__molt_text_int_bits_from_i64"))]
pub extern "C" fn __molt_text_int_bits_from_i64(__molt_context: u64, val: i64) -> u64 {
    {
        let _py = unsafe { crate::PyToken::from_attached_context(__molt_context) };
        int_bits_from_i64(_py, val)
    }
}
