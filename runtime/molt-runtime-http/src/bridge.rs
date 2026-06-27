//! FFI bridge to molt-runtime internal functions.
//!
//! These `extern "C"` declarations are resolved at link time when
//! molt-runtime-http is linked into the same binary as molt-runtime.
//! Each function has a corresponding `#[no_mangle]` shim in
//! `molt-runtime/src/http_bridge.rs`.

use molt_runtime_core::prelude::*;

// ---------------------------------------------------------------------------
// Exception / error handling
// ---------------------------------------------------------------------------

unsafe extern "C" {
    fn __molt_http_raise_exception(
        type_ptr: *const u8,
        type_len: usize,
        msg_ptr: *const u8,
        msg_len: usize,
    ) -> u64;

    fn __molt_http_exception_pending() -> i32;
    fn __molt_http_clear_exception();
    fn __molt_http_clear_attribute_error_if_pending() -> i32;
    fn __molt_http_molt_exception_last() -> u64;
    fn __molt_http_exception_kind_bits(ptr: *mut u8) -> u64;
    fn __molt_http_molt_exception_init(self_bits: u64, args_bits: u64) -> u64;
    fn __molt_http_molt_raise(exc_bits: u64) -> u64;
}

pub fn raise_exception<T: ExceptionSentinel>(_py: &CoreGilToken, type_name: &str, msg: &str) -> T {
    let bits = unsafe {
        __molt_http_raise_exception(type_name.as_ptr(), type_name.len(), msg.as_ptr(), msg.len())
    };
    T::from_bits(bits)
}

pub fn exception_pending(_py: &CoreGilToken) -> bool {
    unsafe { __molt_http_exception_pending() != 0 }
}

pub fn clear_exception(_py: &CoreGilToken) {
    unsafe { __molt_http_clear_exception() }
}

pub fn clear_attribute_error_if_pending(_py: &CoreGilToken) -> bool {
    unsafe { __molt_http_clear_attribute_error_if_pending() != 0 }
}

pub fn molt_exception_last() -> u64 {
    unsafe { __molt_http_molt_exception_last() }
}

/// # Safety
///
/// `ptr` must refer to a live Molt exception object for the duration of this
/// call.
pub unsafe fn exception_kind_bits(ptr: *mut u8) -> u64 {
    unsafe { __molt_http_exception_kind_bits(ptr) }
}

pub fn molt_exception_init(self_bits: u64, args_bits: u64) -> u64 {
    unsafe { __molt_http_molt_exception_init(self_bits, args_bits) }
}

pub fn molt_raise(exc_bits: u64) -> u64 {
    unsafe { __molt_http_molt_raise(exc_bits) }
}

/// Trait for exception return sentinels.
pub trait ExceptionSentinel {
    fn from_bits(bits: u64) -> Self;
}

impl ExceptionSentinel for u64 {
    #[inline]
    fn from_bits(bits: u64) -> Self {
        bits
    }
}

impl<T> ExceptionSentinel for Option<T> {
    #[inline]
    fn from_bits(_bits: u64) -> Self {
        None
    }
}

impl ExceptionSentinel for () {
    #[inline]
    fn from_bits(_bits: u64) -> Self {}
}

// ---------------------------------------------------------------------------
// Object allocation
// ---------------------------------------------------------------------------

unsafe extern "C" {
    fn __molt_http_alloc_tuple(elems_ptr: *const u64, elems_len: usize) -> *mut u8;
    fn __molt_http_alloc_list_with_capacity(
        elems_ptr: *const u64,
        elems_len: usize,
        cap: usize,
    ) -> *mut u8;
    fn __molt_http_alloc_string(data_ptr: *const u8, data_len: usize) -> *mut u8;
    fn __molt_http_alloc_bytes(data_ptr: *const u8, data_len: usize) -> *mut u8;
    fn __molt_http_alloc_dict_with_pairs(pairs_ptr: *const u64, pairs_len: usize) -> *mut u8;
}

pub fn alloc_tuple(_py: &CoreGilToken, elems: &[u64]) -> *mut u8 {
    unsafe { __molt_http_alloc_tuple(elems.as_ptr(), elems.len()) }
}

pub fn alloc_list_with_capacity(_py: &CoreGilToken, elems: &[u64], cap: usize) -> *mut u8 {
    unsafe { __molt_http_alloc_list_with_capacity(elems.as_ptr(), elems.len(), cap) }
}

pub fn alloc_string(_py: &CoreGilToken, data: &[u8]) -> *mut u8 {
    unsafe { __molt_http_alloc_string(data.as_ptr(), data.len()) }
}

pub fn alloc_string_bits(_py: &CoreGilToken, value: &str) -> Option<u64> {
    let ptr = alloc_string(_py, value.as_bytes());
    if ptr.is_null() {
        None
    } else {
        Some(MoltObject::from_ptr(ptr).bits())
    }
}

pub fn alloc_bytes(_py: &CoreGilToken, data: &[u8]) -> *mut u8 {
    unsafe { __molt_http_alloc_bytes(data.as_ptr(), data.len()) }
}

pub fn alloc_dict_with_pairs(_py: &CoreGilToken, pairs: &[u64]) -> *mut u8 {
    unsafe { __molt_http_alloc_dict_with_pairs(pairs.as_ptr(), pairs.len()) }
}

// ---------------------------------------------------------------------------
// Object inspection
// ---------------------------------------------------------------------------

unsafe extern "C" {
    fn __molt_http_object_type_id(ptr: *mut u8) -> u32;
    fn __molt_http_string_obj_to_owned(
        bits: u64,
        out_ptr: *mut *const u8,
        out_len: *mut usize,
    ) -> i32;
    fn __molt_http_is_truthy(bits: u64) -> i32;
    fn __molt_http_maybe_ptr_from_bits(bits: u64) -> *mut u8;
}

/// # Safety
///
/// `ptr` must be a valid Molt runtime object pointer for the duration of this
/// call.
pub unsafe fn object_type_id(ptr: *mut u8) -> u32 {
    unsafe { __molt_http_object_type_id(ptr) }
}

pub fn string_obj_to_owned(obj: MoltObject) -> Option<String> {
    let mut out_ptr: *const u8 = std::ptr::null();
    let mut out_len: usize = 0;
    let ok = unsafe { __molt_http_string_obj_to_owned(obj.bits(), &mut out_ptr, &mut out_len) };
    if ok != 0 {
        let boxed = unsafe { bridge_owned_u8_buffer(out_ptr, out_len) };
        Some(String::from_utf8_lossy(&boxed).into_owned())
    } else {
        None
    }
}

pub fn is_truthy(_py: &CoreGilToken, obj: MoltObject) -> bool {
    unsafe { __molt_http_is_truthy(obj.bits()) != 0 }
}

pub fn maybe_ptr_from_bits(bits: u64) -> Option<*mut u8> {
    let ptr = unsafe { __molt_http_maybe_ptr_from_bits(bits) };
    if ptr.is_null() { None } else { Some(ptr) }
}

// ---------------------------------------------------------------------------
// Reference counting
// ---------------------------------------------------------------------------

unsafe extern "C" {
    fn __molt_http_dec_ref_bits(bits: u64);
    fn __molt_http_inc_ref_bits(bits: u64);
}

pub fn dec_ref_bits(_py: &CoreGilToken, bits: u64) {
    unsafe { __molt_http_dec_ref_bits(bits) }
}

pub fn inc_ref_bits(_py: &CoreGilToken, bits: u64) {
    unsafe { __molt_http_inc_ref_bits(bits) }
}

// ---------------------------------------------------------------------------
// Numeric helpers
// ---------------------------------------------------------------------------

unsafe extern "C" {
    fn __molt_http_to_i64(bits: u64, out: *mut i64) -> i32;
    fn __molt_http_to_f64(bits: u64, out: *mut f64) -> i32;
    fn __molt_http_index_bigint_from_obj(
        obj_bits: u64,
        err_ptr: *const u8,
        err_len: usize,
        out_sign: *mut i32,
        out_ptr: *mut *const u8,
        out_len: *mut usize,
    ) -> i32;
    fn __molt_http_int_bits_from_bigint(sign: i32, data_ptr: *const u8, data_len: usize) -> u64;
    fn __molt_http_molt_float_from_obj(val_bits: u64) -> u64;
}

pub fn to_i64(obj: MoltObject) -> Option<i64> {
    let mut out: i64 = 0;
    let ok = unsafe { __molt_http_to_i64(obj.bits(), &mut out) };
    if ok != 0 { Some(out) } else { None }
}

pub fn to_f64(obj: MoltObject) -> Option<f64> {
    let mut out: f64 = 0.0;
    let ok = unsafe { __molt_http_to_f64(obj.bits(), &mut out) };
    if ok != 0 { Some(out) } else { None }
}

pub fn index_bigint_from_obj(
    _py: &CoreGilToken,
    obj_bits: u64,
    err: &str,
) -> Option<num_bigint::BigInt> {
    use num_bigint::{BigInt, Sign};
    let mut out_sign: i32 = 0;
    let mut out_ptr: *const u8 = std::ptr::null();
    let mut out_len: usize = 0;
    let ok = unsafe {
        __molt_http_index_bigint_from_obj(
            obj_bits,
            err.as_ptr(),
            err.len(),
            &mut out_sign,
            &mut out_ptr,
            &mut out_len,
        )
    };
    if ok == 0 {
        return None;
    }
    let sign = match out_sign {
        -1 => Sign::Minus,
        0 => Sign::NoSign,
        _ => Sign::Plus,
    };
    if out_len == 0 {
        return Some(BigInt::from(0));
    }
    let bytes = unsafe { bridge_owned_u8_buffer(out_ptr, out_len) };
    Some(BigInt::from_bytes_be(sign, &bytes))
}

pub fn int_bits_from_bigint(_py: &CoreGilToken, value: num_bigint::BigInt) -> u64 {
    use num_bigint::Sign;
    let (sign, bytes) = value.to_bytes_be();
    let sign_i32 = match sign {
        Sign::Minus => -1i32,
        Sign::NoSign => 0i32,
        Sign::Plus => 1i32,
    };
    unsafe { __molt_http_int_bits_from_bigint(sign_i32, bytes.as_ptr(), bytes.len()) }
}

pub fn molt_float_from_obj(val_bits: u64) -> u64 {
    unsafe { __molt_http_molt_float_from_obj(val_bits) }
}

// ---------------------------------------------------------------------------
// Container helpers
// ---------------------------------------------------------------------------

#[allow(improper_ctypes)]
unsafe extern "C" {
    fn __molt_http_seq_vec_ptr(ptr: *mut u8) -> *mut Vec<u64>;
    fn __molt_http_dict_get_in_place(dict_ptr: *mut u8, key_bits: u64, out: *mut u64) -> i32;
    fn __molt_http_molt_list_insert(list_bits: u64, index_bits: u64, value_bits: u64) -> u64;
    fn __molt_http_molt_dict_new(initial_capacity: usize) -> u64;
}

/// # Safety
///
/// `ptr` must refer to a live Molt sequence object backed by `Vec<u64>`.
pub unsafe fn seq_vec_ref(ptr: *mut u8) -> &'static Vec<u64> {
    unsafe { &*__molt_http_seq_vec_ptr(ptr) }
}

/// # Safety
///
/// `dict_ptr` must refer to a live Molt dictionary object for the duration of
/// this call.
pub unsafe fn dict_get_in_place(
    _py: &CoreGilToken,
    dict_ptr: *mut u8,
    key_bits: u64,
) -> Option<u64> {
    let mut out: u64 = 0;
    let ok = unsafe { __molt_http_dict_get_in_place(dict_ptr, key_bits, &mut out) };
    if ok != 0 { Some(out) } else { None }
}

pub fn molt_list_insert(list_bits: u64, index_bits: u64, value_bits: u64) -> u64 {
    unsafe { __molt_http_molt_list_insert(list_bits, index_bits, value_bits) }
}

pub fn molt_dict_new(initial_capacity: usize) -> u64 {
    unsafe { __molt_http_molt_dict_new(initial_capacity) }
}

// ---------------------------------------------------------------------------
// Iteration helpers
// ---------------------------------------------------------------------------

unsafe extern "C" {
    fn __molt_http_molt_iter(bits: u64) -> u64;
    fn __molt_http_molt_iter_next(iter_bits: u64) -> u64;
}

pub fn molt_iter(bits: u64) -> u64 {
    unsafe { __molt_http_molt_iter(bits) }
}

pub fn molt_iter_next(iter_bits: u64) -> u64 {
    unsafe { __molt_http_molt_iter_next(iter_bits) }
}

// ---------------------------------------------------------------------------
// Attribute / callable helpers
// ---------------------------------------------------------------------------

unsafe extern "C" {
    fn __molt_http_attr_name_bits_from_bytes(key_ptr: *const u8, key_len: usize) -> u64;
    fn __molt_http_call_callable0(call_bits: u64) -> u64;
    fn __molt_http_call_callable1(call_bits: u64, arg0: u64) -> u64;
    fn __molt_http_call_callable2(call_bits: u64, arg0: u64, arg1: u64) -> u64;
    fn __molt_http_call_class_init_with_args(
        class_bits: u64,
        args_ptr: *const u64,
        args_len: usize,
    ) -> u64;
    fn __molt_http_missing_bits() -> u64;
    fn __molt_http_molt_getattr_builtin(obj_bits: u64, name_bits: u64, default_bits: u64) -> u64;
    fn __molt_http_molt_is_callable(bits: u64) -> i32;
}

pub fn attr_name_bits_from_bytes(_py: &CoreGilToken, name: &[u8]) -> Option<u64> {
    let result = unsafe { __molt_http_attr_name_bits_from_bytes(name.as_ptr(), name.len()) };
    if result == 0 { None } else { Some(result) }
}

pub fn call_callable0(_py: &CoreGilToken, call_bits: u64) -> u64 {
    unsafe { __molt_http_call_callable0(call_bits) }
}

pub fn call_callable1(_py: &CoreGilToken, call_bits: u64, arg0: u64) -> u64 {
    unsafe { __molt_http_call_callable1(call_bits, arg0) }
}

pub fn call_callable2(_py: &CoreGilToken, call_bits: u64, arg0: u64, arg1: u64) -> u64 {
    unsafe { __molt_http_call_callable2(call_bits, arg0, arg1) }
}

pub fn call_class_init_with_args(_py: &CoreGilToken, class_bits: u64, args: &[u64]) -> u64 {
    unsafe { __molt_http_call_class_init_with_args(class_bits, args.as_ptr(), args.len()) }
}

pub fn missing_bits(_py: &CoreGilToken) -> u64 {
    unsafe { __molt_http_missing_bits() }
}

pub fn molt_getattr_builtin(obj_bits: u64, name_bits: u64, default_bits: u64) -> u64 {
    unsafe { __molt_http_molt_getattr_builtin(obj_bits, name_bits, default_bits) }
}

pub fn attr_optional(_py: &CoreGilToken, obj_bits: u64, name: &[u8]) -> Result<Option<u64>, u64> {
    let Some(name_bits) = attr_name_bits_from_bytes(_py, name) else {
        return Err(MoltObject::none().bits());
    };
    let missing = missing_bits(_py);
    let value_bits = molt_getattr_builtin(obj_bits, name_bits, missing);
    dec_ref_bits(_py, name_bits);
    if exception_pending(_py) {
        if clear_attribute_error_if_pending(_py) {
            return Ok(None);
        }
        return Err(MoltObject::none().bits());
    }
    if value_bits == missing {
        return Ok(None);
    }
    Ok(Some(value_bits))
}

pub fn molt_is_callable(bits: u64) -> bool {
    unsafe { __molt_http_molt_is_callable(bits) != 0 }
}

// ---------------------------------------------------------------------------
// String formatting / representation helpers
// ---------------------------------------------------------------------------

unsafe extern "C" {
    fn __molt_http_format_obj_str(bits: u64, out_ptr: *mut *const u8, out_len: *mut usize) -> i32;
    fn __molt_http_molt_repr_from_obj(bits: u64) -> u64;
    fn __molt_http_molt_str_from_obj(bits: u64) -> u64;
}

pub fn format_obj_str(_py: &CoreGilToken, obj: MoltObject) -> String {
    let mut out_ptr: *const u8 = std::ptr::null();
    let mut out_len: usize = 0;
    let ok = unsafe { __molt_http_format_obj_str(obj.bits(), &mut out_ptr, &mut out_len) };
    if ok != 0 {
        let boxed = unsafe { bridge_owned_u8_buffer(out_ptr, out_len) };
        String::from_utf8_lossy(&boxed).into_owned()
    } else {
        String::new()
    }
}

pub fn molt_repr_from_obj(bits: u64) -> u64 {
    unsafe { __molt_http_molt_repr_from_obj(bits) }
}

pub fn molt_str_from_obj(bits: u64) -> u64 {
    unsafe { __molt_http_molt_str_from_obj(bits) }
}

// ---------------------------------------------------------------------------
// Module / object attribute helpers
// ---------------------------------------------------------------------------

unsafe extern "C" {
    fn __molt_http_molt_module_import(name_bits: u64) -> u64;
    fn __molt_http_molt_object_setattr(obj_bits: u64, name_bits: u64, value_bits: u64);
}

pub fn molt_module_import(name_bits: u64) -> u64 {
    unsafe { __molt_http_molt_module_import(name_bits) }
}

pub fn molt_object_setattr(obj_bits: u64, name_bits: u64, value_bits: u64) {
    unsafe { __molt_http_molt_object_setattr(obj_bits, name_bits, value_bits) }
}

// ---------------------------------------------------------------------------
// Buffer export
// ---------------------------------------------------------------------------

/// Mirrors crate::BufferExport from molt-runtime.
/// IMPORTANT: This layout must match the runtime's BufferExport exactly.
#[repr(C)]
pub struct BufferExport {
    pub ptr: u64,
    pub len: u64,
    pub readonly: u64,
    pub stride: i64,
    pub itemsize: u64,
}

unsafe extern "C" {
    fn __molt_http_molt_buffer_export(buffer_bits: u64, export: *mut BufferExport) -> i32;
    fn __molt_http_bytes_like_slice(bits: u64, out_ptr: *mut *const u8, out_len: *mut usize)
    -> i32;
}

pub fn molt_buffer_export(buffer_bits: u64, export: &mut BufferExport) -> bool {
    unsafe { __molt_http_molt_buffer_export(buffer_bits, export as *mut BufferExport) != 0 }
}

pub fn bytes_like_slice(bits: u64) -> Option<&'static [u8]> {
    let mut out_ptr: *const u8 = std::ptr::null();
    let mut out_len: usize = 0;
    let ok = unsafe { __molt_http_bytes_like_slice(bits, &mut out_ptr, &mut out_len) };
    if ok != 0 {
        Some(unsafe { std::slice::from_raw_parts(out_ptr, out_len) })
    } else {
        None
    }
}

// ---------------------------------------------------------------------------
// Capability / environment helpers
// ---------------------------------------------------------------------------

unsafe extern "C" {
    fn __molt_http_has_capability(name_ptr: *const u8, name_len: usize) -> i32;
    fn __molt_http_env_state_get(
        key_ptr: *const u8,
        key_len: usize,
        out_ptr: *mut *const u8,
        out_len: *mut usize,
    ) -> i32;
}

pub fn has_capability(_py: &CoreGilToken, name: &str) -> bool {
    unsafe { __molt_http_has_capability(name.as_ptr(), name.len()) != 0 }
}

pub fn env_state_get(key: &str) -> Option<String> {
    let mut out_ptr: *const u8 = std::ptr::null();
    let mut out_len: usize = 0;
    let ok =
        unsafe { __molt_http_env_state_get(key.as_ptr(), key.len(), &mut out_ptr, &mut out_len) };
    if ok != 0 {
        let boxed = unsafe { bridge_owned_u8_buffer(out_ptr, out_len) };
        Some(String::from_utf8_lossy(&boxed).into_owned())
    } else {
        None
    }
}

// ---------------------------------------------------------------------------
// Class / type resolution
// ---------------------------------------------------------------------------

unsafe extern "C" {
    fn __molt_http_builtin_classes(name_ptr: *const u8, name_len: usize) -> u64;
    fn __molt_http_resolve_global_bits(
        module_ptr: *const u8,
        module_len: usize,
        name_ptr: *const u8,
        name_len: usize,
        out: *mut u64,
    ) -> i32;
}

pub fn builtin_classes(_py: &CoreGilToken, name: &str) -> u64 {
    unsafe { __molt_http_builtin_classes(name.as_ptr(), name.len()) }
}

pub fn resolve_global_bits(_py: &CoreGilToken, module: &str, name: &str) -> Result<u64, u64> {
    let mut out: u64 = MoltObject::none().bits();
    let status = unsafe {
        __molt_http_resolve_global_bits(
            module.as_ptr(),
            module.len(),
            name.as_ptr(),
            name.len(),
            &mut out,
        )
    };
    if status == 1 { Ok(out) } else { Err(out) }
}

// ---------------------------------------------------------------------------
// GIL release guard
// ---------------------------------------------------------------------------

unsafe extern "C" {
    fn __molt_http_gil_release_new() -> u64;
    fn __molt_http_gil_release_drop(handle: u64);
}

/// RAII guard that releases the GIL while alive.
pub struct GilReleaseGuard {
    handle: u64,
}

impl GilReleaseGuard {
    pub fn new() -> Self {
        Self {
            handle: unsafe { __molt_http_gil_release_new() },
        }
    }
}

impl Default for GilReleaseGuard {
    fn default() -> Self {
        Self::new()
    }
}

impl Drop for GilReleaseGuard {
    fn drop(&mut self) {
        unsafe { __molt_http_gil_release_drop(self.handle) }
    }
}

// ---------------------------------------------------------------------------
// Type ID constants
// ---------------------------------------------------------------------------

unsafe extern "C" {
    fn __molt_http_type_id_module() -> u32;
    fn __molt_http_type_id_list() -> u32;
    fn __molt_http_type_id_tuple() -> u32;
    fn __molt_http_type_id_dict() -> u32;
}

pub fn type_id_module() -> u32 {
    unsafe { __molt_http_type_id_module() }
}
pub fn type_id_list() -> u32 {
    unsafe { __molt_http_type_id_list() }
}
pub fn type_id_tuple() -> u32 {
    unsafe { __molt_http_type_id_tuple() }
}
pub fn type_id_dict() -> u32 {
    unsafe { __molt_http_type_id_dict() }
}
