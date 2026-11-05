//! FFI bridge shims for `molt-runtime-crypto`.
//!
//! Each function here is a thin `#[no_mangle] extern "C"` wrapper around an
//! internal `pub(crate)` function.  The crypto crate declares matching
//! `extern "C"` imports and they are resolved at link time.

use crate::audit::AuditArgs;
use crate::object::ops::string_obj_to_owned as _string_obj_to_owned;
use crate::*;
use num_bigint::{BigInt, Sign};

crate::object::native_handle::native_handle_bridge!(
    __molt_crypto_hash_owner_new => "__molt_crypto_hash_owner_new",
    __molt_crypto_hash_owner_clone => "__molt_crypto_hash_owner_clone",
    std::sync::Mutex<molt_runtime_crypto::hashlib::HashHandle>
);
crate::object::native_handle::native_handle_bridge!(
    __molt_crypto_hmac_owner_new => "__molt_crypto_hmac_owner_new",
    __molt_crypto_hmac_owner_clone => "__molt_crypto_hmac_owner_clone",
    std::sync::Mutex<molt_runtime_crypto::hmac::HmacHandle>
);

// ---------------------------------------------------------------------------
// Exception / error handling
// ---------------------------------------------------------------------------

#[unsafe(export_name = molt_codegen_abi::attached_symbol!("__molt_crypto_raise_exception"))]
pub extern "C" fn __molt_crypto_raise_exception(
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

#[unsafe(export_name = molt_codegen_abi::attached_symbol!("__molt_crypto_exception_pending"))]
pub extern "C" fn __molt_crypto_exception_pending(__molt_context: u64) -> i32 {
    {
        let _py = unsafe { crate::PyToken::from_attached_context(__molt_context) };
        if exception_pending(_py) { 1 } else { 0 }
    }
}

// ---------------------------------------------------------------------------
// Object allocation
// ---------------------------------------------------------------------------

#[unsafe(export_name = molt_codegen_abi::attached_symbol!("__molt_crypto_alloc_bytes"))]
pub extern "C" fn __molt_crypto_alloc_bytes(
    __molt_context: u64,
    data_ptr: *const u8,
    data_len: usize,
) -> *mut u8 {
    {
        let _py = unsafe { crate::PyToken::from_attached_context(__molt_context) };
        let data = unsafe { std::slice::from_raw_parts(data_ptr, data_len) };
        alloc_bytes(_py, data)
    }
}

#[unsafe(export_name = molt_codegen_abi::attached_symbol!("__molt_crypto_alloc_string"))]
pub extern "C" fn __molt_crypto_alloc_string(
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
pub extern "C" fn __molt_crypto_object_type_id(ptr: *mut u8) -> u32 {
    unsafe { object_type_id(ptr) }
}

#[unsafe(no_mangle)]
pub extern "C" fn __molt_crypto_string_obj_to_owned(
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

#[unsafe(export_name = molt_codegen_abi::attached_symbol!("__molt_crypto_type_name"))]
pub extern "C" fn __molt_crypto_type_name(
    __molt_context: u64,
    bits: u64,
    out_ptr: *mut *const u8,
    out_len: *mut usize,
) -> i32 {
    {
        let _py = unsafe { crate::PyToken::from_attached_context(__molt_context) };
        let obj = obj_from_bits(bits);
        let name = type_name(_py, obj);
        let bytes = name.into_owned().into_bytes().into_boxed_slice();
        unsafe { crate::resource::bridge_buffer::export_u8_box(bytes, out_ptr, out_len) }
    }
}

#[unsafe(export_name = molt_codegen_abi::attached_symbol!("__molt_crypto_is_truthy"))]
pub extern "C" fn __molt_crypto_is_truthy(__molt_context: u64, bits: u64) -> i32 {
    {
        let _py = unsafe { crate::PyToken::from_attached_context(__molt_context) };
        let obj = obj_from_bits(bits);
        if is_truthy(_py, obj) { 1 } else { 0 }
    }
}

// ---------------------------------------------------------------------------
// Reference counting / pointer management
// ---------------------------------------------------------------------------

#[unsafe(no_mangle)]
pub extern "C" fn __molt_crypto_release_ptr(ptr: *mut u8) {
    release_ptr(ptr);
}

#[unsafe(export_name = molt_codegen_abi::attached_symbol!("__molt_crypto_dec_ref_bits"))]
pub extern "C" fn __molt_crypto_dec_ref_bits(__molt_context: u64, bits: u64) {
    {
        let _py = unsafe { crate::PyToken::from_attached_context(__molt_context) };
        dec_ref_bits(_py, bits);
    }
}

#[unsafe(export_name = molt_codegen_abi::attached_symbol!("__molt_crypto_inc_ref_bits"))]
pub extern "C" fn __molt_crypto_inc_ref_bits(__molt_context: u64, bits: u64) {
    {
        let _py = unsafe { crate::PyToken::from_attached_context(__molt_context) };
        inc_ref_bits(_py, bits);
    }
}

// ---------------------------------------------------------------------------
// Numeric helpers
// ---------------------------------------------------------------------------

#[unsafe(no_mangle)]
pub extern "C" fn __molt_crypto_to_i64(bits: u64, out: *mut i64) -> i32 {
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

#[unsafe(export_name = molt_codegen_abi::attached_symbol!("__molt_crypto_int_bits_from_i64"))]
pub extern "C" fn __molt_crypto_int_bits_from_i64(__molt_context: u64, val: i64) -> u64 {
    {
        let _py = unsafe { crate::PyToken::from_attached_context(__molt_context) };
        int_bits_from_i64(_py, val)
    }
}

#[unsafe(export_name = molt_codegen_abi::attached_symbol!("__molt_crypto_int_bits_from_bigint"))]
pub extern "C" fn __molt_crypto_int_bits_from_bigint(
    __molt_context: u64,
    sign: i32,
    data_ptr: *const u8,
    data_len: usize,
) -> u64 {
    {
        let _py = unsafe { crate::PyToken::from_attached_context(__molt_context) };
        let bytes = unsafe { std::slice::from_raw_parts(data_ptr, data_len) };
        let sign = match sign {
            -1 => Sign::Minus,
            0 => Sign::NoSign,
            _ => Sign::Plus,
        };
        let value = BigInt::from_bytes_be(sign, bytes);
        int_bits_from_bigint(_py, value)
    }
}

#[unsafe(export_name = molt_codegen_abi::attached_symbol!("__molt_crypto_dict_get_in_place"))]
pub extern "C" fn __molt_crypto_dict_get_in_place(
    __molt_context: u64,
    dict_ptr: *mut u8,
    key_bits: u64,
    out: *mut u64,
) -> i32 {
    {
        let _py = unsafe { crate::PyToken::from_attached_context(__molt_context) };
        match unsafe { dict_get_in_place(_py, dict_ptr, key_bits) } {
            Ok(Some(bits)) => {
                unsafe {
                    *out = bits;
                }
                1
            }
            Ok(None) => 0,
            Err(()) => -1,
        }
    }
}

#[unsafe(no_mangle)]
pub extern "C" fn __molt_crypto_list_len(ptr: *mut u8) -> usize {
    unsafe { list_len(ptr) }
}

#[unsafe(export_name = molt_codegen_abi::attached_symbol!("__molt_crypto_seq_snapshot"))]
pub extern "C" fn __molt_crypto_seq_snapshot(
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
// OS randomness
// ---------------------------------------------------------------------------

#[unsafe(export_name = molt_codegen_abi::attached_symbol!("__molt_crypto_require_random_entropy"))]
pub extern "C" fn __molt_crypto_require_random_entropy(__molt_context: u64) -> i32 {
    {
        let _py = unsafe { crate::PyToken::from_attached_context(__molt_context) };
        crate::require_operation::<u64>(_py, crate::OperationId::RandomEntropy, AuditArgs::None)
            .is_ok() as i32
    }
}

#[unsafe(no_mangle)]
pub extern "C" fn __molt_crypto_fill_os_random(buf_ptr: *mut u8, buf_len: usize) -> i32 {
    let buf = unsafe { std::slice::from_raw_parts_mut(buf_ptr, buf_len) };
    match crate::randomness::fill_os_random(buf) {
        Ok(()) => 1,
        Err(_) => 0,
    }
}
