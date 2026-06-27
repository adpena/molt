use crate::object::ops::string_obj_to_owned as runtime_string_obj_to_owned;
use molt_runtime_core::prelude::*;

// ---------------------------------------------------------------------------
// Exception / error handling
// ---------------------------------------------------------------------------

pub fn raise_exception<T: ExceptionSentinel>(_py: &CoreGilToken, type_name: &str, msg: &str) -> T {
    crate::with_gil_entry_nopanic!(py, {
        T::from_bits(crate::raise_exception::<u64>(py, type_name, msg))
    })
}

pub fn exception_pending(_py: &CoreGilToken) -> bool {
    crate::with_gil_entry_nopanic!(py, { crate::exception_pending(py) })
}

pub fn clear_exception(_py: &CoreGilToken) {
    crate::with_gil_entry_nopanic!(py, {
        crate::clear_exception(py);
    })
}

pub fn raise_os_error<T: ExceptionSentinel>(
    _py: &CoreGilToken,
    err: std::io::Error,
    ctx: &str,
) -> T {
    crate::with_gil_entry_nopanic!(py, {
        T::from_bits(crate::raise_os_error::<u64>(py, err, ctx))
    })
}

pub fn raise_os_error_errno<T: ExceptionSentinel>(_py: &CoreGilToken, errno: i64, ctx: &str) -> T {
    crate::with_gil_entry_nopanic!(py, {
        T::from_bits(crate::raise_os_error_errno::<u64>(py, errno, ctx))
    })
}

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

pub fn alloc_tuple(_py: &CoreGilToken, elems: &[u64]) -> *mut u8 {
    crate::with_gil_entry_nopanic!(py, { crate::alloc_tuple(py, elems) })
}

pub fn alloc_list(_py: &CoreGilToken, elems: &[u64]) -> *mut u8 {
    crate::with_gil_entry_nopanic!(py, { crate::alloc_list(py, elems) })
}

pub fn alloc_string(_py: &CoreGilToken, data: &[u8]) -> *mut u8 {
    crate::with_gil_entry_nopanic!(py, { crate::alloc_string(py, data) })
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
    crate::with_gil_entry_nopanic!(py, { crate::alloc_bytes(py, data) })
}

pub fn alloc_dict_with_pairs(_py: &CoreGilToken, pairs: &[u64]) -> *mut u8 {
    crate::with_gil_entry_nopanic!(py, { crate::alloc_dict_with_pairs(py, pairs) })
}

// ---------------------------------------------------------------------------
// Object inspection
// ---------------------------------------------------------------------------

/// # Safety
///
/// `ptr` must be a valid Molt runtime object pointer for the lifetime of this
/// call.
pub unsafe fn object_type_id(ptr: *mut u8) -> u32 {
    unsafe { crate::object_type_id(ptr) }
}

pub fn string_obj_to_owned(obj: MoltObject) -> Option<String> {
    runtime_string_obj_to_owned(obj)
}

pub fn is_truthy(_py: &CoreGilToken, obj: MoltObject) -> bool {
    crate::with_gil_entry_nopanic!(py, { crate::is_truthy(py, obj) })
}

/// # Safety
///
/// `ptr` must refer to a live Molt object that the runtime recognizes as a
/// bytes-like object when this function returns `Some`.
pub unsafe fn bytes_like_slice(ptr: *mut u8) -> Option<&'static [u8]> {
    unsafe { crate::object::memoryview::bytes_like_slice(ptr) }
}

// ---------------------------------------------------------------------------
// Reference counting
// ---------------------------------------------------------------------------

pub fn dec_ref_bits(_py: &CoreGilToken, bits: u64) {
    crate::with_gil_entry_nopanic!(py, {
        crate::dec_ref_bits(py, bits);
    })
}

pub fn inc_ref_bits(_py: &CoreGilToken, bits: u64) {
    crate::with_gil_entry_nopanic!(py, {
        crate::inc_ref_bits(py, bits);
    })
}

// ---------------------------------------------------------------------------
// Numeric helpers
// ---------------------------------------------------------------------------

pub fn to_i64(obj: MoltObject) -> Option<i64> {
    crate::to_i64(obj)
}

pub fn to_f64(obj: MoltObject) -> Option<f64> {
    crate::to_f64(obj)
}

// ---------------------------------------------------------------------------
// Container helpers
// ---------------------------------------------------------------------------

/// # Safety
///
/// `ptr` must refer to a live Molt sequence object backed by `Vec<u64>`.
pub unsafe fn seq_vec_ref(ptr: *mut u8) -> &'static Vec<u64> {
    unsafe { crate::seq_vec_ref(ptr) }
}

pub unsafe fn dict_get_in_place(_py: &CoreGilToken, ptr: *mut u8, key_bits: u64) -> Option<u64> {
    crate::with_gil_entry_nopanic!(py, {
        unsafe { crate::dict_get_in_place(py, ptr, key_bits) }
    })
}

pub fn type_id_list() -> u32 {
    crate::TYPE_ID_LIST
}

pub fn type_id_tuple() -> u32 {
    crate::TYPE_ID_TUPLE
}

pub fn type_id_dict() -> u32 {
    crate::TYPE_ID_DICT
}

pub fn molt_object_hash(bits: u64) -> u64 {
    crate::object::ops_sys::molt_object_hash(bits)
}
