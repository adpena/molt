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
    #[link_name = molt_runtime_core::attached_symbol!("__molt_http_raise_exception")]
    fn __molt_http_raise_exception(
        __molt_context: u64,
        kind_code: u16,
        msg_ptr: *const u8,
        msg_len: usize,
    ) -> u64;

    #[link_name = molt_runtime_core::attached_symbol!("__molt_http_exception_pending")]
    fn __molt_http_exception_pending(__molt_context: u64) -> i32;
    #[link_name = molt_runtime_core::attached_symbol!("__molt_http_clear_exception")]
    fn __molt_http_clear_exception(__molt_context: u64);
    #[link_name = molt_runtime_core::attached_symbol!("__molt_http_pending_exception_matches_builtin")]
    fn __molt_http_pending_exception_matches_builtin(__molt_context: u64, kind_code: u16) -> i32;
    #[link_name = molt_runtime_core::attached_symbol!("__molt_http_with_exception_scope")]
    fn __molt_http_with_exception_scope(
        __molt_context: u64,
        restore_on_success: i32,
        callback: unsafe extern "C" fn(*mut std::ffi::c_void) -> i32,
        context: *mut std::ffi::c_void,
    );
    #[link_name = molt_runtime_core::attached_symbol!("__molt_http_molt_exception_init")]
    fn __molt_http_molt_exception_init(__molt_context: u64, self_bits: u64, args_bits: u64) -> u64;
    #[link_name = molt_runtime_core::attached_symbol!("__molt_http_molt_raise")]
    fn __molt_http_molt_raise(__molt_context: u64, exc_bits: u64) -> u64;
}

pub fn raise_exception<T: ExceptionSentinel>(
    _py: &CoreGilToken,
    type_name: impl Into<ExcKind>,
    msg: &str,
) -> T {
    let bits = unsafe {
        __molt_http_raise_exception(
            _py.attached_context(),
            type_name.into().code(),
            msg.as_ptr(),
            msg.len(),
        )
    };
    T::from_bits(bits)
}

pub fn exception_pending(_py: &CoreGilToken) -> bool {
    unsafe { __molt_http_exception_pending(_py.attached_context()) != 0 }
}

pub fn clear_exception(_py: &CoreGilToken) {
    unsafe { __molt_http_clear_exception(_py.attached_context()) }
}

/// Resolve the builtin target and match actual exception inheritance without
/// consulting names/hooks or substituting an older active handled exception.
pub fn pending_exception_matches_builtin(_py: &CoreGilToken, kind: impl Into<ExcKind>) -> bool {
    unsafe {
        __molt_http_pending_exception_matches_builtin(_py.attached_context(), kind.into().code())
            != 0
    }
}

fn with_exception_scope<T, F: FnOnce() -> Result<T, u64>>(
    _py: &PyToken,
    restore_on_success: bool,
    run: F,
) -> Result<T, u64> {
    struct Callback<F, T> {
        run: Option<F>,
        result: Option<std::thread::Result<Result<T, u64>>>,
    }

    unsafe extern "C" fn invoke<T, F: FnOnce() -> Result<T, u64>>(
        context: *mut std::ffi::c_void,
    ) -> i32 {
        let callback = unsafe { &mut *context.cast::<Callback<F, T>>() };
        let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            callback
                .run
                .take()
                .expect("HTTP exception callback must run once")()
        }));
        let status = match &result {
            Ok(Ok(_)) => 1,
            Ok(Err(_)) => 0,
            Err(_) => -1,
        };
        callback.result = Some(result);
        status
    }

    let mut callback = Callback::<F, T> {
        run: Some(run),
        result: None,
    };
    unsafe {
        __molt_http_with_exception_scope(
            _py.attached_context(),
            i32::from(restore_on_success),
            invoke::<T, F>,
            (&raw mut callback).cast(),
        );
    }
    match callback
        .result
        .expect("HTTP exception callback must run synchronously")
    {
        Ok(result) => result,
        Err(payload) => std::panic::resume_unwind(payload),
    }
}

/// Run a finally/probe callback with the incoming raised error temporarily
/// handled. Success restores its exact owner; a callback failure replaces it.
/// The outer handled exception is restored on every exit, including unwinding.
pub fn with_saved_exception<T>(
    _py: &CoreGilToken,
    run: impl FnOnce() -> Result<T, u64>,
) -> Result<T, u64> {
    with_exception_scope(_py, true, run)
}

/// Handle the pending error for the complete callback suite. Success consumes
/// it; failure propagates the new error with the handled error as its context.
pub fn with_handled_exception<T>(
    _py: &CoreGilToken,
    run: impl FnOnce() -> Result<T, u64>,
) -> Result<T, u64> {
    with_exception_scope(_py, false, run)
}

pub fn molt_exception_init(_py: &PyToken, self_bits: u64, args_bits: u64) -> u64 {
    unsafe { __molt_http_molt_exception_init(_py.attached_context(), self_bits, args_bits) }
}

pub fn molt_raise(_py: &PyToken, exc_bits: u64) -> u64 {
    unsafe { __molt_http_molt_raise(_py.attached_context(), exc_bits) }
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
    #[link_name = molt_runtime_core::attached_symbol!("__molt_http_alloc_tuple")]
    fn __molt_http_alloc_tuple(
        __molt_context: u64,
        elems_ptr: *const u64,
        elems_len: usize,
    ) -> *mut u8;
    #[link_name = molt_runtime_core::attached_symbol!("__molt_http_alloc_list_with_capacity")]
    fn __molt_http_alloc_list_with_capacity(
        __molt_context: u64,
        elems_ptr: *const u64,
        elems_len: usize,
        cap: usize,
    ) -> *mut u8;
    #[link_name = molt_runtime_core::attached_symbol!("__molt_http_alloc_string")]
    fn __molt_http_alloc_string(
        __molt_context: u64,
        data_ptr: *const u8,
        data_len: usize,
    ) -> *mut u8;
    #[link_name = molt_runtime_core::attached_symbol!("__molt_http_alloc_bytes")]
    fn __molt_http_alloc_bytes(
        __molt_context: u64,
        data_ptr: *const u8,
        data_len: usize,
    ) -> *mut u8;
    #[link_name = molt_runtime_core::attached_symbol!("__molt_http_alloc_dict_with_pairs")]
    fn __molt_http_alloc_dict_with_pairs(
        __molt_context: u64,
        pairs_ptr: *const u64,
        pairs_len: usize,
    ) -> *mut u8;
}

pub fn alloc_tuple(_py: &CoreGilToken, elems: &[u64]) -> *mut u8 {
    unsafe { __molt_http_alloc_tuple(_py.attached_context(), elems.as_ptr(), elems.len()) }
}

pub fn alloc_list_with_capacity(_py: &CoreGilToken, elems: &[u64], cap: usize) -> *mut u8 {
    unsafe {
        __molt_http_alloc_list_with_capacity(
            _py.attached_context(),
            elems.as_ptr(),
            elems.len(),
            cap,
        )
    }
}

pub fn alloc_string(_py: &CoreGilToken, data: &[u8]) -> *mut u8 {
    unsafe { __molt_http_alloc_string(_py.attached_context(), data.as_ptr(), data.len()) }
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
    unsafe { __molt_http_alloc_bytes(_py.attached_context(), data.as_ptr(), data.len()) }
}

pub fn alloc_dict_with_pairs(_py: &CoreGilToken, pairs: &[u64]) -> *mut u8 {
    unsafe {
        __molt_http_alloc_dict_with_pairs(_py.attached_context(), pairs.as_ptr(), pairs.len())
    }
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
    #[link_name = molt_runtime_core::attached_symbol!("__molt_http_is_truthy")]
    fn __molt_http_is_truthy(__molt_context: u64, bits: u64) -> i32;
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
    unsafe { __molt_http_is_truthy(_py.attached_context(), obj.bits()) != 0 }
}

pub fn maybe_ptr_from_bits(bits: u64) -> Option<*mut u8> {
    let ptr = unsafe { __molt_http_maybe_ptr_from_bits(bits) };
    if ptr.is_null() { None } else { Some(ptr) }
}

// ---------------------------------------------------------------------------
// Reference counting
// ---------------------------------------------------------------------------

unsafe extern "C" {
    #[link_name = molt_runtime_core::attached_symbol!("__molt_http_dec_ref_bits")]
    fn __molt_http_dec_ref_bits(__molt_context: u64, bits: u64);
    #[link_name = molt_runtime_core::attached_symbol!("__molt_http_inc_ref_bits")]
    fn __molt_http_inc_ref_bits(__molt_context: u64, bits: u64);
}

pub fn dec_ref_bits(_py: &CoreGilToken, bits: u64) {
    unsafe { __molt_http_dec_ref_bits(_py.attached_context(), bits) }
}

pub fn inc_ref_bits(_py: &CoreGilToken, bits: u64) {
    unsafe { __molt_http_inc_ref_bits(_py.attached_context(), bits) }
}

// ---------------------------------------------------------------------------
// Numeric helpers
// ---------------------------------------------------------------------------

unsafe extern "C" {
    fn __molt_http_to_i64(bits: u64, out: *mut i64) -> i32;
    fn __molt_http_to_f64(bits: u64, out: *mut f64) -> i32;
    #[link_name = molt_runtime_core::attached_symbol!("__molt_http_index_bigint_from_obj")]
    fn __molt_http_index_bigint_from_obj(
        __molt_context: u64,
        obj_bits: u64,
        err_ptr: *const u8,
        err_len: usize,
        out_sign: *mut i32,
        out_ptr: *mut *const u8,
        out_len: *mut usize,
    ) -> i32;
    #[link_name = molt_runtime_core::attached_symbol!("__molt_http_int_bits_from_bigint")]
    fn __molt_http_int_bits_from_bigint(
        __molt_context: u64,
        sign: i32,
        data_ptr: *const u8,
        data_len: usize,
    ) -> u64;
    #[link_name = molt_runtime_core::attached_symbol!("__molt_http_float_from_object_value")]
    fn __molt_http_float_from_object_value(
        __molt_context: u64,
        val_bits: u64,
        out: *mut f64,
    ) -> i32;
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
            _py.attached_context(),
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
    unsafe {
        __molt_http_int_bits_from_bigint(
            _py.attached_context(),
            sign_i32,
            bytes.as_ptr(),
            bytes.len(),
        )
    }
}

pub fn float_from_object_value(_py: &PyToken, val_bits: u64) -> Option<f64> {
    let mut value = 0.0;
    let ok = unsafe {
        __molt_http_float_from_object_value(_py.attached_context(), val_bits, &mut value)
    };
    (ok != 0).then_some(value)
}

// ---------------------------------------------------------------------------
// Container helpers
// ---------------------------------------------------------------------------

#[allow(improper_ctypes)]
unsafe extern "C" {
    #[link_name = molt_runtime_core::attached_symbol!("__molt_http_seq_snapshot")]
    fn __molt_http_seq_snapshot(
        __molt_context: u64,
        ptr: *mut u8,
        out_ptr: *mut *const u64,
        out_len: *mut usize,
    ) -> i32;
    #[link_name = molt_runtime_core::attached_symbol!("__molt_http_dict_get_in_place")]
    fn __molt_http_dict_get_in_place(
        __molt_context: u64,
        dict_ptr: *mut u8,
        key_bits: u64,
        out: *mut u64,
    ) -> i32;
    #[link_name = molt_runtime_core::attached_symbol!("__molt_http_molt_list_insert")]
    fn __molt_http_molt_list_insert(
        __molt_context: u64,
        list_bits: u64,
        index_bits: u64,
        value_bits: u64,
    ) -> u64;
    #[link_name = molt_runtime_core::attached_symbol!("__molt_http_molt_dict_new")]
    fn __molt_http_molt_dict_new(__molt_context: u64, initial_capacity: usize) -> u64;
}

/// # Safety
///
/// `ptr` must refer to a live Molt list or tuple for the duration of this call.
pub unsafe fn seq_snapshot(_py: &PyToken, ptr: *mut u8) -> OwnedBridgeHandleSnapshot<'_> {
    let mut out_ptr: *const u64 = std::ptr::null();
    let mut out_len = 0usize;
    let ok = unsafe {
        __molt_http_seq_snapshot(_py.attached_context(), ptr, &mut out_ptr, &mut out_len)
    };
    if ok == 0 {
        out_ptr = std::ptr::null();
        out_len = 0;
    }
    unsafe { bridge_owned_handle_snapshot(_py, out_ptr, out_len) }
}

/// # Safety
///
/// `dict_ptr` must refer to a live Molt dictionary object for the duration of
/// this call.
pub unsafe fn dict_get_in_place(
    _py: &CoreGilToken,
    dict_ptr: *mut u8,
    key_bits: u64,
) -> Result<Option<u64>, molt_runtime_core::ErrorIndicatorSet> {
    let mut out: u64 = 0;
    let ok = unsafe {
        __molt_http_dict_get_in_place(_py.attached_context(), dict_ptr, key_bits, &mut out)
    };
    match ok {
        1 => Ok(Some(out)),
        0 => Ok(None),
        -1 => Err(molt_runtime_core::ErrorIndicatorSet),
        _ => unreachable!("invalid dictionary lookup status"),
    }
}

pub fn molt_list_insert(_py: &PyToken, list_bits: u64, index_bits: u64, value_bits: u64) -> u64 {
    unsafe {
        __molt_http_molt_list_insert(_py.attached_context(), list_bits, index_bits, value_bits)
    }
}

pub fn molt_dict_new(_py: &PyToken, initial_capacity: usize) -> u64 {
    unsafe { __molt_http_molt_dict_new(_py.attached_context(), initial_capacity) }
}

// ---------------------------------------------------------------------------
// Iteration helpers
// ---------------------------------------------------------------------------

unsafe extern "C" {
    #[link_name = molt_runtime_core::attached_symbol!("__molt_http_molt_iter")]
    fn __molt_http_molt_iter(__molt_context: u64, bits: u64) -> u64;
}

pub fn molt_iter(_py: &PyToken, bits: u64) -> u64 {
    unsafe { __molt_http_molt_iter(_py.attached_context(), bits) }
}

// ---------------------------------------------------------------------------
// Attribute / callable helpers
// ---------------------------------------------------------------------------

unsafe extern "C" {
    #[link_name = molt_runtime_core::attached_symbol!("__molt_http_attr_name_bits_from_bytes")]
    fn __molt_http_attr_name_bits_from_bytes(
        __molt_context: u64,
        key_ptr: *const u8,
        key_len: usize,
    ) -> u64;
    #[link_name = molt_runtime_core::attached_symbol!("__molt_http_call_callable0")]
    fn __molt_http_call_callable0(__molt_context: u64, call_bits: u64) -> u64;
    #[link_name = molt_runtime_core::attached_symbol!("__molt_http_call_callable1")]
    fn __molt_http_call_callable1(__molt_context: u64, call_bits: u64, arg0: u64) -> u64;
    #[link_name = molt_runtime_core::attached_symbol!("__molt_http_call_callable2")]
    fn __molt_http_call_callable2(__molt_context: u64, call_bits: u64, arg0: u64, arg1: u64)
    -> u64;
    #[link_name = molt_runtime_core::attached_symbol!("molt_callargs_new") ]
    fn molt_callargs_new(__molt_context: u64, pos_capacity: u64, kw_capacity: u64) -> u64;
    #[link_name = molt_runtime_core::attached_symbol!("molt_callargs_push_pos") ]
    fn molt_callargs_push_pos(__molt_context: u64, builder_bits: u64, value_bits: u64) -> u64;
    #[link_name = molt_runtime_core::attached_symbol!("molt_callargs_push_kw") ]
    fn molt_callargs_push_kw(
        __molt_context: u64,
        builder_bits: u64,
        name_bits: u64,
        value_bits: u64,
    ) -> u64;
    #[link_name = molt_runtime_core::attached_symbol!("molt_call_bind") ]
    fn molt_call_bind(__molt_context: u64, call_bits: u64, builder_bits: u64) -> u64;
    #[link_name = molt_runtime_core::attached_symbol!("__molt_http_call_class_init_with_args")]
    fn __molt_http_call_class_init_with_args(
        __molt_context: u64,
        class_bits: u64,
        args_ptr: *const u64,
        args_len: usize,
    ) -> u64;
    #[link_name = molt_runtime_core::attached_symbol!("__molt_http_missing_bits")]
    fn __molt_http_missing_bits(__molt_context: u64) -> u64;
    #[link_name = molt_runtime_core::attached_symbol!("__molt_http_molt_getattr_builtin")]
    fn __molt_http_molt_getattr_builtin(
        __molt_context: u64,
        obj_bits: u64,
        name_bits: u64,
        default_bits: u64,
    ) -> u64;
    #[link_name = molt_runtime_core::attached_symbol!("__molt_http_lookup_attribute")]
    fn __molt_http_lookup_attribute(
        __molt_context: u64,
        obj_bits: u64,
        name_bits: u64,
    ) -> molt_obj_model::AttributeLookup;
    #[link_name = molt_runtime_core::attached_symbol!("__molt_http_molt_is_callable")]
    fn __molt_http_molt_is_callable(__molt_context: u64, bits: u64) -> i32;
}

pub fn attr_name_bits_from_bytes(_py: &CoreGilToken, name: &[u8]) -> Option<u64> {
    let result = unsafe {
        __molt_http_attr_name_bits_from_bytes(_py.attached_context(), name.as_ptr(), name.len())
    };
    if result == 0 { None } else { Some(result) }
}

pub fn call_callable0(_py: &CoreGilToken, call_bits: u64) -> u64 {
    unsafe { __molt_http_call_callable0(_py.attached_context(), call_bits) }
}

pub fn call_callable1(_py: &CoreGilToken, call_bits: u64, arg0: u64) -> u64 {
    unsafe { __molt_http_call_callable1(_py.attached_context(), call_bits, arg0) }
}

pub fn call_callable2(_py: &CoreGilToken, call_bits: u64, arg0: u64, arg1: u64) -> u64 {
    unsafe { __molt_http_call_callable2(_py.attached_context(), call_bits, arg0, arg1) }
}

/// Preserve keyword binding for callbacks whose public signature is extensible.
/// The canonical binder consumes the builder even when argument preparation fails.
pub(crate) fn call_callable1_keyword(
    py: &CoreGilToken,
    call_bits: u64,
    arg_bits: u64,
    name: &[u8],
    value_bits: u64,
) -> u64 {
    let Some(name_bits) = attr_name_bits_from_bytes(py, name) else {
        return MoltObject::none().bits();
    };
    let name = unsafe { molt_runtime_core::OwnedRuntimeValue::from_owned_bits(py, name_bits) };
    let builder = unsafe { molt_callargs_new(py.attached_context(), 1, 1) };
    if builder == 0 {
        return MoltObject::none().bits();
    }
    unsafe {
        let _ = molt_callargs_push_pos(py.attached_context(), builder, arg_bits);
        let _ = molt_callargs_push_kw(py.attached_context(), builder, name.bits(), value_bits);
        molt_call_bind(py.attached_context(), call_bits, builder)
    }
}

pub fn call_class_init_with_args(_py: &CoreGilToken, class_bits: u64, args: &[u64]) -> u64 {
    unsafe {
        __molt_http_call_class_init_with_args(
            _py.attached_context(),
            class_bits,
            args.as_ptr(),
            args.len(),
        )
    }
}

pub fn missing_bits(_py: &CoreGilToken) -> u64 {
    unsafe { __molt_http_missing_bits(_py.attached_context()) }
}

pub fn molt_getattr_builtin(
    _py: &PyToken,
    obj_bits: u64,
    name_bits: u64,
    default_bits: u64,
) -> u64 {
    unsafe {
        __molt_http_molt_getattr_builtin(_py.attached_context(), obj_bits, name_bits, default_bits)
    }
}

pub fn lookup_attribute(
    _py: &PyToken,
    obj_bits: u64,
    name_bits: u64,
) -> molt_obj_model::AttributeLookup {
    unsafe { __molt_http_lookup_attribute(_py.attached_context(), obj_bits, name_bits) }
}

pub fn attr_optional(_py: &CoreGilToken, obj_bits: u64, name: &[u8]) -> Result<Option<u64>, u64> {
    let Some(name_bits) = attr_name_bits_from_bytes(_py, name) else {
        return Err(MoltObject::none().bits());
    };
    let result = lookup_attribute(_py, obj_bits, name_bits);
    dec_ref_bits(_py, name_bits);
    result.into_result().map_err(|()| MoltObject::none().bits())
}

pub fn molt_is_callable(_py: &PyToken, bits: u64) -> bool {
    unsafe { __molt_http_molt_is_callable(_py.attached_context(), bits) != 0 }
}

// ---------------------------------------------------------------------------
// String formatting / representation helpers
// ---------------------------------------------------------------------------

unsafe extern "C" {
    #[link_name = molt_runtime_core::attached_symbol!("__molt_http_format_obj_str")]
    fn __molt_http_format_obj_str(
        __molt_context: u64,
        bits: u64,
        out_ptr: *mut *const u8,
        out_len: *mut usize,
    ) -> i32;
}

pub fn format_obj_str(_py: &CoreGilToken, obj: MoltObject) -> Option<String> {
    let mut out_ptr: *const u8 = std::ptr::null();
    let mut out_len: usize = 0;
    let ok = unsafe {
        __molt_http_format_obj_str(
            _py.attached_context(),
            obj.bits(),
            &mut out_ptr,
            &mut out_len,
        )
    };
    if ok != 0 {
        let boxed = unsafe { bridge_owned_u8_buffer(out_ptr, out_len) };
        Some(String::from_utf8_lossy(&boxed).into_owned())
    } else {
        None
    }
}

// ---------------------------------------------------------------------------
// Module / object attribute helpers
// ---------------------------------------------------------------------------

unsafe extern "C" {
    #[link_name = molt_runtime_core::attached_symbol!("__molt_http_molt_module_import")]
    fn __molt_http_molt_module_import(__molt_context: u64, name_bits: u64) -> u64;
    #[link_name = molt_runtime_core::attached_symbol!("__molt_http_molt_set_attr_name")]
    fn __molt_http_molt_set_attr_name(
        __molt_context: u64,
        obj_bits: u64,
        name_bits: u64,
        value_bits: u64,
    );
}

pub fn molt_module_import(_py: &PyToken, name_bits: u64) -> u64 {
    unsafe { __molt_http_molt_module_import(_py.attached_context(), name_bits) }
}

pub fn molt_set_attr_name(_py: &PyToken, obj_bits: u64, name_bits: u64, value_bits: u64) {
    unsafe {
        __molt_http_molt_set_attr_name(_py.attached_context(), obj_bits, name_bits, value_bits)
    }
}

// ---------------------------------------------------------------------------
// Buffer export
// ---------------------------------------------------------------------------

unsafe extern "C" {
    fn __molt_http_bytes_like_slice(bits: u64, out_ptr: *mut *const u8, out_len: *mut usize)
    -> i32;
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
    #[link_name = molt_runtime_core::attached_symbol!("__molt_http_require_operation")]
    fn __molt_http_require_operation(
        __molt_context: u64,
        name_ptr: *const u8,
        name_len: usize,
        environment: i32,
    ) -> i32;
    #[link_name = molt_runtime_core::attached_symbol!("__molt_http_has_capability")]
    fn __molt_http_has_capability(__molt_context: u64, name_ptr: *const u8, name_len: usize)
    -> i32;

}

pub fn require_operation(
    _py: &CoreGilToken,
    operation: molt_runtime_core::host_capabilities_generated::OperationId,
) -> Result<(), u64> {
    require_route_operation(_py, operation, false)
}

pub fn require_route_operation(
    _py: &CoreGilToken,
    operation: molt_runtime_core::host_capabilities_generated::OperationId,
    environment: bool,
) -> Result<(), u64> {
    let name = operation.as_str();
    if unsafe {
        __molt_http_require_operation(
            _py.attached_context(),
            name.as_ptr(),
            name.len(),
            i32::from(environment),
        )
    } != 0
    {
        Ok(())
    } else {
        Err(MoltObject::none().bits())
    }
}

pub fn has_capability(_py: &CoreGilToken, name: &str) -> bool {
    unsafe { __molt_http_has_capability(_py.attached_context(), name.as_ptr(), name.len()) != 0 }
}

// ---------------------------------------------------------------------------
// Class / type resolution
// ---------------------------------------------------------------------------

unsafe extern "C" {
    #[link_name = molt_runtime_core::attached_symbol!("__molt_http_builtin_classes")]
    fn __molt_http_builtin_classes(
        __molt_context: u64,
        name_ptr: *const u8,
        name_len: usize,
    ) -> u64;
    #[link_name = molt_runtime_core::attached_symbol!("__molt_http_resolve_global_bits")]
    fn __molt_http_resolve_global_bits(
        __molt_context: u64,
        module_ptr: *const u8,
        module_len: usize,
        name_ptr: *const u8,
        name_len: usize,
        out: *mut u64,
    ) -> i32;
}

pub fn builtin_classes(_py: &CoreGilToken, name: &str) -> u64 {
    unsafe { __molt_http_builtin_classes(_py.attached_context(), name.as_ptr(), name.len()) }
}

pub fn resolve_global_bits(_py: &CoreGilToken, module: &str, name: &str) -> Result<u64, u64> {
    let mut out: u64 = MoltObject::none().bits();
    let status = unsafe {
        __molt_http_resolve_global_bits(
            _py.attached_context(),
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
    /// Suspend current-thread GIL custody until this guard is dropped.
    /// Panics when called without current-thread GIL custody.
    pub fn suspend() -> Self {
        Self {
            handle: unsafe { __molt_http_gil_release_new() },
        }
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
