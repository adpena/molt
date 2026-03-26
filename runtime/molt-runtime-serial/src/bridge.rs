//! FFI bridge to molt-runtime internal functions.
//!
//! All dispatch goes through a single `RuntimeVtable` fetched once at init.
//! The only extern "C" symbol is `__molt_serial_get_vtable`.

use molt_runtime_core::prelude::*;
use molt_runtime_core::RuntimeVtable;
use std::borrow::Cow;
use std::sync::OnceLock;

/// Global vtable reference, populated once at init time.
static VTABLE: OnceLock<&'static RuntimeVtable> = OnceLock::new();

/// Initialize the vtable. Called once by the runtime at startup.
/// After this, all bridge functions dispatch through the vtable.
pub fn init_vtable() {
    unsafe extern "C" {
        fn __molt_serial_get_vtable() -> *const RuntimeVtable;
    }
    let ptr = unsafe { __molt_serial_get_vtable() };
    if !ptr.is_null() {
        let vtable = unsafe { &*ptr };
        let _ = VTABLE.set(vtable);
    }
}

/// Get the vtable reference. Panics if not initialized.
#[inline(always)]
fn vt() -> &'static RuntimeVtable {
    VTABLE
        .get()
        .copied()
        .expect("molt-runtime-serial: vtable not initialized — call bridge::init_vtable() first")
}

// ---------------------------------------------------------------------------
// Exception / error handling
// ---------------------------------------------------------------------------

pub fn raise_exception<T: ExceptionSentinel>(_py: &PyToken, type_name: &str, msg: &str) -> T {
    let bits = unsafe {
        (vt().raise_exception)(
            type_name.as_ptr(),
            type_name.len(),
            msg.as_ptr(),
            msg.len(),
        )
    };
    T::from_bits(bits)
}

pub fn exception_pending(_py: &PyToken) -> bool {
    unsafe { (vt().exception_pending)() != 0 }
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

pub fn alloc_tuple(_py: &PyToken, elems: &[u64]) -> *mut u8 {
    unsafe { (vt().alloc_tuple)(elems.as_ptr(), elems.len()) }
}

pub fn alloc_list(_py: &PyToken, elems: &[u64]) -> *mut u8 {
    unsafe { (vt().alloc_list)(elems.as_ptr(), elems.len()) }
}

pub fn alloc_string(_py: &PyToken, data: &[u8]) -> *mut u8 {
    unsafe { (vt().alloc_string)(data.as_ptr(), data.len()) }
}

pub fn alloc_bytes(_py: &PyToken, data: &[u8]) -> *mut u8 {
    unsafe { (vt().alloc_bytes)(data.as_ptr(), data.len()) }
}

// ---------------------------------------------------------------------------
// Object inspection
// ---------------------------------------------------------------------------

pub unsafe fn object_type_id(ptr: *mut u8) -> u32 {
    unsafe { (vt().object_type_id)(ptr) }
}

pub fn string_obj_to_owned(obj: MoltObject) -> Option<String> {
    let mut out_ptr: *const u8 = std::ptr::null();
    let mut out_len: usize = 0;
    let ok = unsafe { (vt().string_obj_to_owned)(obj.bits(), &mut out_ptr, &mut out_len) };
    if ok != 0 {
        let boxed =
            unsafe { Box::from_raw(std::slice::from_raw_parts_mut(out_ptr as *mut u8, out_len)) };
        Some(String::from_utf8_lossy(&boxed).into_owned())
    } else {
        None
    }
}

pub fn type_name(_py: &PyToken, obj: MoltObject) -> Cow<'static, str> {
    let mut out_ptr: *const u8 = std::ptr::null();
    let mut out_len: usize = 0;
    let ok = unsafe { (vt().type_name)(obj.bits(), &mut out_ptr, &mut out_len) };
    if ok != 0 && !out_ptr.is_null() {
        let boxed =
            unsafe { Box::from_raw(std::slice::from_raw_parts_mut(out_ptr as *mut u8, out_len)) };
        Cow::Owned(String::from_utf8_lossy(&boxed).into_owned())
    } else {
        Cow::Borrowed("<unknown>")
    }
}

pub fn is_truthy(_py: &PyToken, obj: MoltObject) -> bool {
    unsafe { (vt().is_truthy)(obj.bits()) != 0 }
}

pub unsafe fn bytes_like_slice(ptr: *mut u8) -> Option<&'static [u8]> {
    let mut out_ptr: *const u8 = std::ptr::null();
    let mut out_len: usize = 0;
    unsafe {
        if (vt().bytes_like_slice)(ptr, &mut out_ptr, &mut out_len) != 0 {
            Some(std::slice::from_raw_parts(out_ptr, out_len))
        } else {
            None
        }
    }
}

pub unsafe fn string_bytes(ptr: *mut u8) -> &'static [u8] {
    let mut out_ptr: *const u8 = std::ptr::null();
    let mut out_len: usize = 0;
    unsafe {
        (vt().string_bytes)(ptr, &mut out_ptr, &mut out_len);
        std::slice::from_raw_parts(out_ptr, out_len)
    }
}

pub fn string_len(ptr: *mut u8) -> usize {
    unsafe { (vt().string_len)(ptr) }
}

// ---------------------------------------------------------------------------
// Memoryview / bytes-like helpers
// ---------------------------------------------------------------------------

pub unsafe fn bytes_like_slice_raw(ptr: *mut u8) -> Option<&'static [u8]> {
    let mut out_ptr: *const u8 = std::ptr::null();
    let mut out_len: usize = 0;
    unsafe {
        if (vt().bytes_like_slice_raw)(ptr, &mut out_ptr, &mut out_len) != 0 {
            Some(std::slice::from_raw_parts(out_ptr, out_len))
        } else {
            None
        }
    }
}

pub unsafe fn memoryview_is_c_contiguous_view(ptr: *mut u8) -> bool {
    unsafe { (vt().memoryview_is_c_contiguous_view)(ptr) != 0 }
}

pub unsafe fn memoryview_readonly(ptr: *mut u8) -> bool {
    unsafe { (vt().memoryview_readonly)(ptr) != 0 }
}

pub unsafe fn memoryview_nbytes(ptr: *mut u8) -> usize {
    unsafe { (vt().memoryview_nbytes)(ptr) }
}

pub unsafe fn memoryview_offset(ptr: *mut u8) -> isize {
    unsafe { (vt().memoryview_offset)(ptr) }
}

pub unsafe fn memoryview_owner_bits(ptr: *mut u8) -> u64 {
    unsafe { (vt().memoryview_owner_bits)(ptr) }
}

// ---------------------------------------------------------------------------
// Reference counting / pointer management
// ---------------------------------------------------------------------------

pub fn release_ptr(ptr: *mut u8) {
    unsafe { (vt().release_ptr)(ptr) }
}

pub fn dec_ref_bits(_py: &PyToken, bits: u64) {
    unsafe { (vt().dec_ref_bits)(bits) }
}

pub fn inc_ref_bits(_py: &PyToken, bits: u64) {
    unsafe { (vt().inc_ref_bits)(bits) }
}

// ---------------------------------------------------------------------------
// Numeric helpers
// ---------------------------------------------------------------------------

pub fn to_i64(obj: MoltObject) -> Option<i64> {
    let mut out: i64 = 0;
    let ok = unsafe { (vt().to_i64)(obj.bits(), &mut out) };
    if ok != 0 { Some(out) } else { None }
}

pub fn to_f64(obj: MoltObject) -> Option<f64> {
    let mut out: f64 = 0.0;
    let ok = unsafe { (vt().to_f64)(obj.bits(), &mut out) };
    if ok != 0 { Some(out) } else { None }
}

pub fn to_bigint(obj: MoltObject) -> Option<num_bigint::BigInt> {
    use num_bigint::{BigInt, Sign};
    let mut out_sign: i32 = 0;
    let mut out_ptr: *const u8 = std::ptr::null();
    let mut out_len: usize = 0;
    let ok =
        unsafe { (vt().to_bigint)(obj.bits(), &mut out_sign, &mut out_ptr, &mut out_len) };
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
    let bytes =
        unsafe { Box::from_raw(std::slice::from_raw_parts_mut(out_ptr as *mut u8, out_len)) };
    Some(BigInt::from_bytes_be(sign, &bytes))
}

pub fn int_bits_from_i64(_py: &PyToken, val: i64) -> u64 {
    unsafe { (vt().int_bits_from_i64)(val) }
}

pub fn int_bits_from_i128(_py: &PyToken, val: i128) -> u64 {
    let lo = val as u64;
    let hi = (val >> 64) as u64;
    unsafe { (vt().int_bits_from_i128)(lo, hi) }
}

pub fn int_bits_from_bigint(_py: &PyToken, value: num_bigint::BigInt) -> u64 {
    use num_bigint::Sign;
    let (sign, bytes) = value.to_bytes_be();
    let sign_i32 = match sign {
        Sign::Minus => -1i32,
        Sign::NoSign => 0i32,
        Sign::Plus => 1i32,
    };
    unsafe { (vt().int_bits_from_bigint)(sign_i32, bytes.as_ptr(), bytes.len()) }
}

pub fn bigint_ptr_from_bits(bits: u64) -> Option<*mut u8> {
    let ptr = unsafe { (vt().bigint_ptr_from_bits)(bits) };
    if ptr.is_null() { None } else { Some(ptr) }
}

/// Read the BigInt stored at a raw pointer. The bridge serializes it.
pub fn bigint_ref(ptr: *mut u8) -> num_bigint::BigInt {
    use num_bigint::{BigInt, Sign};
    let mut out_sign: i32 = 0;
    let mut out_ptr: *const u8 = std::ptr::null();
    let mut out_len: usize = 0;
    let ok =
        unsafe { (vt().bigint_ref)(ptr, &mut out_sign, &mut out_ptr, &mut out_len) };
    if ok == 0 || out_len == 0 {
        return BigInt::from(0);
    }
    let sign = match out_sign {
        -1 => Sign::Minus,
        0 => Sign::NoSign,
        _ => Sign::Plus,
    };
    let bytes =
        unsafe { Box::from_raw(std::slice::from_raw_parts_mut(out_ptr as *mut u8, out_len)) };
    BigInt::from_bytes_be(sign, &bytes)
}

pub fn bigint_from_f64_trunc(val: f64) -> num_bigint::BigInt {
    use num_bigint::{BigInt, Sign};
    let mut out_sign: i32 = 0;
    let mut out_ptr: *const u8 = std::ptr::null();
    let mut out_len: usize = 0;
    let ok = unsafe {
        (vt().bigint_from_f64_trunc)(val, &mut out_sign, &mut out_ptr, &mut out_len)
    };
    if ok == 0 || out_len == 0 {
        return BigInt::from(0);
    }
    let sign = match out_sign {
        -1 => Sign::Minus,
        0 => Sign::NoSign,
        _ => Sign::Plus,
    };
    let bytes =
        unsafe { Box::from_raw(std::slice::from_raw_parts_mut(out_ptr as *mut u8, out_len)) };
    BigInt::from_bytes_be(sign, &bytes)
}

pub fn bigint_bits(_py: &PyToken, value: &num_bigint::BigInt) -> u64 {
    use num_bigint::Sign;
    let (sign, bytes) = value.to_bytes_be();
    let sign_i32 = match sign {
        Sign::Minus => -1i32,
        Sign::NoSign => 0i32,
        Sign::Plus => 1i32,
    };
    unsafe { (vt().bigint_bits)(sign_i32, bytes.as_ptr(), bytes.len()) }
}

pub fn bigint_to_inline(_py: &PyToken, value: &num_bigint::BigInt) -> u64 {
    use num_bigint::Sign;
    let (sign, bytes) = value.to_bytes_be();
    let sign_i32 = match sign {
        Sign::Minus => -1i32,
        Sign::NoSign => 0i32,
        Sign::Plus => 1i32,
    };
    unsafe { (vt().bigint_to_inline)(sign_i32, bytes.as_ptr(), bytes.len()) }
}

pub fn index_i64_from_obj(_py: &PyToken, obj_bits: u64, err: &str) -> i64 {
    unsafe { (vt().index_i64_from_obj)(obj_bits, err.as_ptr(), err.len()) }
}

pub fn index_bigint_from_obj(
    _py: &PyToken,
    obj_bits: u64,
    err: &str,
) -> Option<num_bigint::BigInt> {
    use num_bigint::{BigInt, Sign};
    let mut out_sign: i32 = 0;
    let mut out_ptr: *const u8 = std::ptr::null();
    let mut out_len: usize = 0;
    let ok = unsafe {
        (vt().index_bigint_from_obj)(
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
    let bytes =
        unsafe { Box::from_raw(std::slice::from_raw_parts_mut(out_ptr as *mut u8, out_len)) };
    Some(BigInt::from_bytes_be(sign, &bytes))
}

// ---------------------------------------------------------------------------
// Callable / protocol helpers
// ---------------------------------------------------------------------------

pub fn call_callable0(_py: &PyToken, call_bits: u64) -> u64 {
    unsafe { (vt().call_callable0)(call_bits) }
}

pub fn call_callable2(_py: &PyToken, call_bits: u64, arg0: u64, arg1: u64) -> u64 {
    unsafe { (vt().call_callable2)(call_bits, arg0, arg1) }
}

pub fn attr_lookup_ptr_allow_missing(_py: &PyToken, ptr: *mut u8, name_bits: u64) -> Option<u64> {
    let result = unsafe { (vt().attr_lookup_ptr_allow_missing)(ptr, name_bits) };
    if result == 0 { None } else { Some(result) }
}

pub fn intern_static_name(_py: &PyToken, key: &[u8]) -> u64 {
    unsafe { (vt().intern_static_name)(key.as_ptr(), key.len()) }
}

pub fn class_name_for_error(type_bits: u64) -> String {
    let mut out_ptr: *const u8 = std::ptr::null();
    let mut out_len: usize = 0;
    let ok =
        unsafe { (vt().class_name_for_error)(type_bits, &mut out_ptr, &mut out_len) };
    if ok != 0 && !out_ptr.is_null() {
        let boxed =
            unsafe { Box::from_raw(std::slice::from_raw_parts_mut(out_ptr as *mut u8, out_len)) };
        String::from_utf8_lossy(&boxed).into_owned()
    } else {
        "<unknown>".to_string()
    }
}

pub fn type_of_bits(_py: &PyToken, val_bits: u64) -> u64 {
    unsafe { (vt().type_of_bits)(val_bits) }
}

pub fn maybe_ptr_from_bits(bits: u64) -> Option<*mut u8> {
    let ptr = unsafe { (vt().maybe_ptr_from_bits)(bits) };
    if ptr.is_null() { None } else { Some(ptr) }
}

pub fn molt_is_callable(_py: &PyToken, bits: u64) -> bool {
    unsafe { (vt().molt_is_callable)(bits) != 0 }
}

pub fn format_obj(_py: &PyToken, obj: MoltObject) -> String {
    let mut out_ptr: *const u8 = std::ptr::null();
    let mut out_len: usize = 0;
    let ok = unsafe { (vt().format_obj)(obj.bits(), &mut out_ptr, &mut out_len) };
    if ok != 0 && !out_ptr.is_null() {
        let boxed =
            unsafe { Box::from_raw(std::slice::from_raw_parts_mut(out_ptr as *mut u8, out_len)) };
        String::from_utf8_lossy(&boxed).into_owned()
    } else {
        "<?>".to_string()
    }
}

pub fn format_obj_str(_py: &PyToken, obj: MoltObject) -> String {
    let mut out_ptr: *const u8 = std::ptr::null();
    let mut out_len: usize = 0;
    let ok = unsafe { (vt().format_obj_str)(obj.bits(), &mut out_ptr, &mut out_len) };
    if ok != 0 && !out_ptr.is_null() {
        let boxed =
            unsafe { Box::from_raw(std::slice::from_raw_parts_mut(out_ptr as *mut u8, out_len)) };
        String::from_utf8_lossy(&boxed).into_owned()
    } else {
        "<?>".to_string()
    }
}

// ---------------------------------------------------------------------------
// Bytearray helpers
// ---------------------------------------------------------------------------

pub unsafe fn bytearray_vec(ptr: *mut u8) -> &'static mut Vec<u8> {
    unsafe { &mut *(vt().bytearray_vec)(ptr) }
}

// ---------------------------------------------------------------------------
// Container helpers
// ---------------------------------------------------------------------------

pub unsafe fn dict_get_in_place(
    _py: &PyToken,
    dict_ptr: *mut u8,
    key_bits: u64,
) -> Option<u64> {
    let mut out: u64 = 0;
    let ok = unsafe { (vt().dict_get_in_place)(dict_ptr, key_bits, &mut out) };
    if ok != 0 { Some(out) } else { None }
}

pub unsafe fn dict_set_in_place(
    _py: &PyToken,
    dict_ptr: *mut u8,
    key_bits: u64,
    val_bits: u64,
) -> bool {
    unsafe { (vt().dict_set_in_place)(dict_ptr, key_bits, val_bits) != 0 }
}

pub unsafe fn list_len(ptr: *mut u8) -> usize {
    unsafe { (vt().list_len)(ptr) }
}

pub unsafe fn seq_vec_ref(ptr: *mut u8) -> &'static Vec<u64> {
    unsafe { &*(vt().seq_vec_ptr)(ptr) }
}

// ---------------------------------------------------------------------------
// Iteration helpers
// ---------------------------------------------------------------------------

pub fn molt_iter(_py: &PyToken, bits: u64) -> u64 {
    unsafe { (vt().molt_iter)(bits) }
}

pub fn molt_iter_next(_py: &PyToken, iter_bits: u64) -> Option<u64> {
    let mut out: u64 = 0;
    let ok = unsafe { (vt().molt_iter_next)(iter_bits, &mut out) };
    if ok != 0 { Some(out) } else { None }
}

pub fn raise_not_iterable(_py: &PyToken, bits: u64) -> u64 {
    unsafe { (vt().raise_not_iterable)(bits) }
}

pub fn molt_sorted_builtin(_py: &PyToken, bits: u64) -> u64 {
    unsafe { (vt().molt_sorted_builtin)(bits) }
}

pub fn molt_mul(_py: &PyToken, a: u64, b: u64) -> u64 {
    unsafe { (vt().molt_mul)(a, b) }
}

// ---------------------------------------------------------------------------
// OS randomness
// ---------------------------------------------------------------------------

pub fn fill_os_random(buf: &mut [u8]) -> Result<(), ()> {
    let ok = unsafe { (vt().fill_os_random)(buf.as_mut_ptr(), buf.len()) };
    if ok != 0 { Ok(()) } else { Err(()) }
}

// ---------------------------------------------------------------------------
// Dict helpers (configparser-specific)
// ---------------------------------------------------------------------------

pub fn alloc_dict_with_pairs(_py: &PyToken, pairs: &[u64]) -> *mut u8 {
    unsafe { (vt().alloc_dict_with_pairs)(pairs.as_ptr(), pairs.len()) }
}

/// Returns a cloned copy of the dict's insertion order as a Vec of [k0, v0, k1, v1, ...].
pub fn dict_order_clone(_py: &PyToken, ptr: *mut u8) -> Vec<u64> {
    let mut out_ptr: *const u64 = std::ptr::null();
    let mut out_len: usize = 0;
    let ok = unsafe { (vt().dict_order_clone)(ptr, &mut out_ptr, &mut out_len) };
    if ok == 0 || out_len == 0 {
        return Vec::new();
    }
    let boxed =
        unsafe { Box::from_raw(std::slice::from_raw_parts_mut(out_ptr as *mut u64, out_len)) };
    boxed.into_vec()
}

// ---------------------------------------------------------------------------
// Extended helpers (email / zipfile / decimal)
// ---------------------------------------------------------------------------

pub fn alloc_list_with_capacity(_py: &PyToken, elems: &[u64], capacity: usize) -> *mut u8 {
    unsafe { (vt().alloc_list_with_capacity)(elems.as_ptr(), elems.len(), capacity) }
}

pub fn attr_name_bits_from_bytes(_py: &PyToken, name: &[u8]) -> Option<u64> {
    let mut out: u64 = 0;
    let ok = unsafe { (vt().attr_name_bits_from_bytes)(name.as_ptr(), name.len(), &mut out) };
    if ok != 0 { Some(out) } else { None }
}

pub unsafe fn call_class_init_with_args(
    _py: &PyToken,
    class_ptr: *mut u8,
    args: &[u64],
) -> u64 {
    unsafe { (vt().call_class_init_with_args)(class_ptr, args.as_ptr(), args.len()) }
}

pub fn missing_bits(_py: &PyToken) -> u64 {
    unsafe { (vt().missing_bits)() }
}

pub fn molt_getattr_builtin(obj_bits: u64, name_bits: u64, default_bits: u64) -> u64 {
    unsafe { (vt().molt_getattr_builtin)(obj_bits, name_bits, default_bits) }
}

pub fn molt_module_import(name_bits: u64) -> u64 {
    unsafe { (vt().molt_module_import)(name_bits) }
}

// ---------------------------------------------------------------------------
// Local helper functions (reimplemented for serial crate)
// ---------------------------------------------------------------------------

/// Iterate over a Python iterable and collect all items as String.
pub fn iterable_to_string_vec(_py: &PyToken, values_bits: u64) -> Result<Vec<String>, u64> {
    let iter_bits = molt_iter(_py, values_bits);
    if exception_pending(_py) {
        return Err(MoltObject::none().bits());
    }
    let mut out: Vec<String> = Vec::new();
    loop {
        match molt_iter_next(_py, iter_bits) {
            Some(item_bits) => {
                let Some(item) = string_obj_to_owned(obj_from_bits(item_bits)) else {
                    return Err(raise_exception::<u64>(_py, "TypeError", "expected str item"));
                };
                out.push(item);
            }
            None => {
                if exception_pending(_py) {
                    return Err(MoltObject::none().bits());
                }
                break;
            }
        }
    }
    Ok(out)
}

/// Allocate a Molt string object and return its bits.
pub fn alloc_string_bits(_py: &PyToken, value: &str) -> Option<u64> {
    let ptr = alloc_string(_py, value.as_bytes());
    if ptr.is_null() {
        None
    } else {
        Some(MoltObject::from_ptr(ptr).bits())
    }
}

/// Allocate a Molt list of strings.
pub fn alloc_string_list(_py: &PyToken, values: &[String]) -> u64 {
    let mut item_bits: Vec<u64> = Vec::with_capacity(values.len());
    for value in values {
        let ptr = alloc_string(_py, value.as_bytes());
        if ptr.is_null() {
            for bits in &item_bits {
                dec_ref_bits(_py, *bits);
            }
            return MoltObject::none().bits();
        }
        item_bits.push(MoltObject::from_ptr(ptr).bits());
    }
    let list_ptr = alloc_list_with_capacity(_py, item_bits.as_slice(), item_bits.len());
    for bits in &item_bits {
        dec_ref_bits(_py, *bits);
    }
    if list_ptr.is_null() {
        MoltObject::none().bits()
    } else {
        MoltObject::from_ptr(list_ptr).bits()
    }
}
