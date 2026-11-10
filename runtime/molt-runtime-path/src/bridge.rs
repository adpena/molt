//! FFI bridge to molt-runtime internal functions.
//!
//! These `extern "C"` declarations are resolved at link time when
//! molt-runtime-path is linked into the same binary as molt-runtime.
//! Each function has a corresponding `#[no_mangle]` shim in
//! `molt-runtime/src/path_bridge.rs`.

use molt_runtime_core::host_capabilities_generated::OperationId;
use molt_runtime_core::prelude::*;

// ---------------------------------------------------------------------------
// Exception / error handling
// ---------------------------------------------------------------------------

unsafe extern "C" {
    #[link_name = molt_runtime_core::attached_symbol!("__molt_path_raise_exception")]
    fn __molt_path_raise_exception(
        __molt_context: u64,
        kind_code: u16,
        msg_ptr: *const u8,
        msg_len: usize,
    ) -> u64;

    #[link_name = molt_runtime_core::attached_symbol!("__molt_path_exception_pending")]
    fn __molt_path_exception_pending(__molt_context: u64) -> i32;

    #[link_name = molt_runtime_core::attached_symbol!("__molt_path_raise_os_error")]
    fn __molt_path_raise_os_error(
        __molt_context: u64,
        raw_error: i64,
        err_msg_ptr: *const u8,
        err_msg_len: usize,
        ctx_ptr: *const u8,
        ctx_len: usize,
    ) -> u64;

    #[link_name = molt_runtime_core::attached_symbol!("__molt_path_raise_os_error_errno")]
    fn __molt_path_raise_os_error_errno(
        __molt_context: u64,
        errno: i64,
        ctx_ptr: *const u8,
        ctx_len: usize,
    ) -> u64;
}

pub fn raise_exception<T: ExceptionSentinel>(
    _py: &CoreGilToken,
    type_name: impl Into<ExcKind>,
    msg: &str,
) -> T {
    let bits = unsafe {
        __molt_path_raise_exception(
            _py.attached_context(),
            type_name.into().code(),
            msg.as_ptr(),
            msg.len(),
        )
    };
    T::from_bits(bits)
}

pub fn exception_pending(_py: &CoreGilToken) -> bool {
    unsafe { __molt_path_exception_pending(_py.attached_context()) != 0 }
}

pub fn raise_os_error<T: ExceptionSentinel>(
    _py: &CoreGilToken,
    err: std::io::Error,
    ctx: &str,
) -> T {
    let raw_error = err.raw_os_error().map(i64::from).unwrap_or(i64::MIN);
    let msg = err.to_string();
    let bits = unsafe {
        __molt_path_raise_os_error(
            _py.attached_context(),
            raw_error,
            msg.as_ptr(),
            msg.len(),
            ctx.as_ptr(),
            ctx.len(),
        )
    };
    T::from_bits(bits)
}

pub fn raise_os_error_errno<T: ExceptionSentinel>(_py: &CoreGilToken, errno: i64, ctx: &str) -> T {
    let bits = unsafe {
        __molt_path_raise_os_error_errno(_py.attached_context(), errno, ctx.as_ptr(), ctx.len())
    };
    T::from_bits(bits)
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
    #[link_name = molt_runtime_core::attached_symbol!("__molt_path_alloc_tuple")]
    fn __molt_path_alloc_tuple(
        __molt_context: u64,
        elems_ptr: *const u64,
        elems_len: usize,
    ) -> *mut u8;
    #[link_name = molt_runtime_core::attached_symbol!("__molt_path_alloc_list")]
    fn __molt_path_alloc_list(
        __molt_context: u64,
        elems_ptr: *const u64,
        elems_len: usize,
    ) -> *mut u8;
    #[link_name = molt_runtime_core::attached_symbol!("__molt_path_alloc_string")]
    fn __molt_path_alloc_string(
        __molt_context: u64,
        data_ptr: *const u8,
        data_len: usize,
    ) -> *mut u8;
    #[link_name = molt_runtime_core::attached_symbol!("__molt_path_alloc_bytes")]
    fn __molt_path_alloc_bytes(
        __molt_context: u64,
        data_ptr: *const u8,
        data_len: usize,
    ) -> *mut u8;
    #[link_name = molt_runtime_core::attached_symbol!("__molt_path_alloc_dict_with_pairs")]
    fn __molt_path_alloc_dict_with_pairs(
        __molt_context: u64,
        pairs_ptr: *const u64,
        pairs_len: usize,
    ) -> *mut u8;
}

pub fn alloc_tuple(_py: &CoreGilToken, elems: &[u64]) -> *mut u8 {
    unsafe { __molt_path_alloc_tuple(_py.attached_context(), elems.as_ptr(), elems.len()) }
}

pub fn alloc_list(_py: &CoreGilToken, elems: &[u64]) -> *mut u8 {
    unsafe { __molt_path_alloc_list(_py.attached_context(), elems.as_ptr(), elems.len()) }
}

pub fn alloc_string(_py: &CoreGilToken, data: &[u8]) -> *mut u8 {
    unsafe { __molt_path_alloc_string(_py.attached_context(), data.as_ptr(), data.len()) }
}

pub fn alloc_bytes(_py: &CoreGilToken, data: &[u8]) -> *mut u8 {
    unsafe { __molt_path_alloc_bytes(_py.attached_context(), data.as_ptr(), data.len()) }
}

pub fn alloc_dict_with_pairs(_py: &CoreGilToken, pairs: &[u64]) -> *mut u8 {
    unsafe {
        __molt_path_alloc_dict_with_pairs(_py.attached_context(), pairs.as_ptr(), pairs.len())
    }
}

// ---------------------------------------------------------------------------
// Object inspection
// ---------------------------------------------------------------------------

unsafe extern "C" {
    fn __molt_path_object_type_id(ptr: *mut u8) -> u32;
    fn __molt_path_string_obj_to_owned(
        bits: u64,
        out_ptr: *mut *const u8,
        out_len: *mut usize,
    ) -> i32;
    #[link_name = molt_runtime_core::attached_symbol!("__molt_path_is_truthy")]
    fn __molt_path_is_truthy(__molt_context: u64, bits: u64) -> i32;
    fn __molt_path_bytes_like_slice(
        ptr: *mut u8,
        out_ptr: *mut *const u8,
        out_len: *mut usize,
    ) -> i32;
}

/// # Safety
///
/// `ptr` must be a valid Molt runtime object pointer for the lifetime of this
/// call.
pub unsafe fn object_type_id(ptr: *mut u8) -> u32 {
    unsafe { __molt_path_object_type_id(ptr) }
}

pub fn string_obj_bytes(obj: MoltObject) -> Option<Vec<u8>> {
    let mut out_ptr: *const u8 = std::ptr::null();
    let mut out_len: usize = 0;
    let ok = unsafe { __molt_path_string_obj_to_owned(obj.bits(), &mut out_ptr, &mut out_len) };
    if ok != 0 {
        Some(unsafe { bridge_owned_u8_buffer(out_ptr, out_len) }.into_vec())
    } else {
        None
    }
}

pub fn string_obj_to_owned(obj: MoltObject) -> Option<String> {
    string_obj_bytes(obj).map(|raw| {
        String::from_utf8(raw)
            .unwrap_or_else(|error| String::from_utf8_lossy(error.as_bytes()).into_owned())
    })
}

pub fn is_truthy(_py: &CoreGilToken, obj: MoltObject) -> bool {
    unsafe { __molt_path_is_truthy(_py.attached_context(), obj.bits()) != 0 }
}

/// # Safety
///
/// `ptr` must refer to a live Molt object that the runtime recognizes as a
/// bytes-like object when this function returns `Some`.
pub unsafe fn bytes_like_slice(ptr: *mut u8) -> Option<&'static [u8]> {
    let mut out_ptr: *const u8 = std::ptr::null();
    let mut out_len: usize = 0;
    let ok = unsafe { __molt_path_bytes_like_slice(ptr, &mut out_ptr, &mut out_len) };
    if ok != 0 {
        Some(unsafe { std::slice::from_raw_parts(out_ptr, out_len) })
    } else {
        None
    }
}

// ---------------------------------------------------------------------------
// Reference counting
// ---------------------------------------------------------------------------

unsafe extern "C" {
    #[link_name = molt_runtime_core::attached_symbol!("__molt_path_dec_ref_bits")]
    fn __molt_path_dec_ref_bits(__molt_context: u64, bits: u64);
    #[link_name = molt_runtime_core::attached_symbol!("__molt_path_inc_ref_bits")]
    fn __molt_path_inc_ref_bits(__molt_context: u64, bits: u64);
}

pub fn dec_ref_bits(_py: &CoreGilToken, bits: u64) {
    unsafe { __molt_path_dec_ref_bits(_py.attached_context(), bits) }
}

pub fn inc_ref_bits(_py: &CoreGilToken, bits: u64) {
    unsafe { __molt_path_inc_ref_bits(_py.attached_context(), bits) }
}

// ---------------------------------------------------------------------------
// Numeric helpers
// ---------------------------------------------------------------------------

unsafe extern "C" {
    fn __molt_path_to_i64(bits: u64, out: *mut i64) -> i32;
    fn __molt_path_to_f64(bits: u64, out: *mut f64) -> i32;
}

unsafe extern "C" {
    #[link_name = molt_runtime_core::attached_symbol!("__molt_path_index_c_int")]
    fn __molt_path_index_c_int(__molt_context: u64, bits: u64, out: *mut i32) -> i32;
    #[link_name = molt_runtime_core::attached_symbol!("__molt_path_index_i64")]
    fn __molt_path_index_i64(__molt_context: u64, bits: u64, out: *mut i64) -> i32;
}

pub fn index_c_int_from_obj(_py: &CoreGilToken, bits: u64) -> Option<i32> {
    let mut value = 0;
    if unsafe { __molt_path_index_c_int(_py.attached_context(), bits, &mut value) } != 0 {
        Some(value)
    } else {
        None
    }
}

/// CPython's `Py_off_t` argument converter (lseek pos, truncate length).
pub fn index_off_t_from_obj(_py: &CoreGilToken, bits: u64) -> Option<i64> {
    let mut value = 0;
    if unsafe { __molt_path_index_i64(_py.attached_context(), bits, &mut value) } != 0 {
        Some(value)
    } else {
        None
    }
}

pub fn to_i64(obj: MoltObject) -> Option<i64> {
    let mut out: i64 = 0;
    let ok = unsafe { __molt_path_to_i64(obj.bits(), &mut out) };
    if ok != 0 { Some(out) } else { None }
}

pub fn to_f64(obj: MoltObject) -> Option<f64> {
    let mut out: f64 = 0.0;
    let ok = unsafe { __molt_path_to_f64(obj.bits(), &mut out) };
    if ok != 0 { Some(out) } else { None }
}

// ---------------------------------------------------------------------------
// Container helpers
// ---------------------------------------------------------------------------

#[allow(improper_ctypes)]
unsafe extern "C" {
    #[link_name = molt_runtime_core::attached_symbol!("__molt_path_seq_snapshot")]
    fn __molt_path_seq_snapshot(
        __molt_context: u64,
        ptr: *mut u8,
        out_ptr: *mut *const u64,
        out_len: *mut usize,
    ) -> i32;
}

/// # Safety
///
/// `ptr` must refer to a live Molt list or tuple for the duration of this call.
pub unsafe fn seq_snapshot(_py: &PyToken, ptr: *mut u8) -> OwnedBridgeHandleSnapshot<'_> {
    let mut out_ptr: *const u64 = std::ptr::null();
    let mut out_len = 0usize;
    let ok = unsafe {
        __molt_path_seq_snapshot(_py.attached_context(), ptr, &mut out_ptr, &mut out_len)
    };
    if ok == 0 {
        out_ptr = std::ptr::null();
        out_len = 0;
    }
    unsafe { bridge_owned_handle_snapshot(_py, out_ptr, out_len) }
}

// ---------------------------------------------------------------------------
// Capability helpers
// ---------------------------------------------------------------------------

unsafe extern "C" {
    #[link_name = molt_runtime_core::attached_symbol!("__molt_path_require_operation")]
    fn __molt_path_require_operation(
        __molt_context: u64,
        operation_ptr: *const u8,
        operation_len: usize,
        arg_kind: u32,
        arg_ptr: *const u8,
        arg_len: usize,
        out_error_bits: *mut u64,
    ) -> i32;
}

pub enum AuditArg {
    None,
    Path(String),
}

/// Require the generated operation through the runtime host's canonical policy.
/// The host returns the exact raised sentinel separately from its success flag.
pub fn require_operation(_py: &CoreGilToken, operation: &str, arg: AuditArg) -> Result<(), u64> {
    let (arg_kind, arg_ptr, arg_len) = match &arg {
        AuditArg::None => (0, std::ptr::null(), 0),
        AuditArg::Path(path) => (1, path.as_ptr(), path.len()),
    };
    let mut error_bits = MoltObject::none().bits();
    let allowed = unsafe {
        __molt_path_require_operation(
            _py.attached_context(),
            operation.as_ptr(),
            operation.len(),
            arg_kind,
            arg_ptr,
            arg_len,
            &mut error_bits,
        )
    };
    if allowed != 0 {
        Ok(())
    } else {
        Err(error_bits)
    }
}

// ---------------------------------------------------------------------------
// Hash helpers
// ---------------------------------------------------------------------------

unsafe extern "C" {
    #[link_name = molt_runtime_core::attached_symbol!("__molt_path_molt_object_hash")]
    fn __molt_path_molt_object_hash(__molt_context: u64, bits: u64) -> u64;
}

pub fn molt_object_hash(_py: &PyToken, bits: u64) -> u64 {
    unsafe { __molt_path_molt_object_hash(_py.attached_context(), bits) }
}

// ---------------------------------------------------------------------------
// Path resolution helpers
// ---------------------------------------------------------------------------

unsafe extern "C" {
    #[link_name = molt_runtime_core::attached_symbol!("__molt_path_path_from_bits")]
    fn __molt_path_path_from_bits(
        __molt_context: u64,
        bits: u64,
        out_ptr: *mut *const u8,
        out_len: *mut usize,
        out_bytes: *mut bool,
    ) -> i32;
    #[link_name = molt_runtime_core::attached_symbol!("__molt_path_path_result")]
    fn __molt_path_path_result(
        __molt_context: u64,
        data: *const u8,
        len: usize,
        bytes: bool,
    ) -> u64;
    #[link_name = molt_runtime_core::attached_symbol!("__molt_path_fspath_bits")]
    fn __molt_path_fspath_bits(__molt_context: u64, bits: u64) -> u64;

    #[link_name = molt_runtime_core::attached_symbol!("__molt_path_type_name")]
    fn __molt_path_type_name(
        __molt_context: u64,
        bits: u64,
        out_ptr: *mut *const u8,
        out_len: *mut usize,
    ) -> i32;
}

pub fn path_from_bits(py: &CoreGilToken, bits: u64) -> Result<std::path::PathBuf, u64> {
    path_from_bits_with_flavor(py, bits).map(|(path, _)| path)
}

pub fn path_from_bits_with_flavor(
    _py: &CoreGilToken,
    bits: u64,
) -> Result<(std::path::PathBuf, bool), u64> {
    let mut out_ptr: *const u8 = std::ptr::null();
    let mut out_len: usize = 0;
    let mut bytes_flavor = false;
    let ok = unsafe {
        __molt_path_path_from_bits(
            _py.attached_context(),
            bits,
            &mut out_ptr,
            &mut out_len,
            &mut bytes_flavor,
        )
    };
    if ok == 0 {
        return Err(MoltObject::none().bits());
    }
    let bytes = unsafe { bridge_owned_u8_buffer(out_ptr, out_len) };
    // SAFETY: unchanged OsString::into_encoded_bytes output from this process's
    // runtime, built with the same Rust version and target.
    let path = unsafe { std::ffi::OsStr::from_encoded_bytes_unchecked(&bytes) };
    Ok((std::path::PathBuf::from(path), bytes_flavor))
}

pub fn path_result(_py: &CoreGilToken, value: &std::path::Path, bytes: bool) -> u64 {
    let raw = value.as_os_str().as_encoded_bytes();
    unsafe { __molt_path_path_result(_py.attached_context(), raw.as_ptr(), raw.len(), bytes) }
}

pub fn fspath_bits(_py: &PyToken, bits: u64) -> u64 {
    unsafe { __molt_path_fspath_bits(_py.attached_context(), bits) }
}

pub fn type_name(_py: &CoreGilToken, obj: MoltObject) -> String {
    let mut out_ptr: *const u8 = std::ptr::null();
    let mut out_len: usize = 0;
    let ok = unsafe {
        __molt_path_type_name(
            _py.attached_context(),
            obj.bits(),
            &mut out_ptr,
            &mut out_len,
        )
    };
    if ok != 0 && !out_ptr.is_null() && out_len > 0 {
        let boxed = unsafe { bridge_owned_u8_buffer(out_ptr, out_len) };
        String::from_utf8_lossy(&boxed).into_owned()
    } else {
        "<unknown>".to_string()
    }
}

unsafe extern "C" {
    #[link_name = molt_runtime_core::attached_symbol!("__molt_path_metadata")]
    fn __molt_path_metadata(
        __molt_context: u64,
        path: *const u8,
        len: usize,
        follow: bool,
        operation: *const std::ffi::c_void,
        suppress: bool,
        out: *mut molt_runtime_platform::stat_support::PathMetadata,
    ) -> i32;
    #[link_name = molt_runtime_core::attached_symbol!("__molt_path_readlink")]
    fn __molt_path_readlink(__molt_context: u64, path: *const u8, len: usize, bytes: bool) -> u64;
    #[link_name = molt_runtime_core::attached_symbol!("__molt_path_samefile")]
    fn __molt_path_samefile(
        __molt_context: u64,
        left: *const u8,
        left_len: usize,
        right: *const u8,
        right_len: usize,
        operation: *const std::ffi::c_void,
    ) -> u64;
}

pub fn path_metadata(
    _py: &CoreGilToken,
    path: &std::path::Path,
    follow: bool,
    operation: OperationId,
    suppress: bool,
) -> Result<Option<molt_runtime_platform::stat_support::PathMetadata>, u64> {
    let raw = path.as_os_str().as_encoded_bytes();
    let mut out = std::mem::MaybeUninit::uninit();
    match unsafe {
        __molt_path_metadata(
            _py.attached_context(),
            raw.as_ptr(),
            raw.len(),
            follow,
            std::ptr::from_ref(&operation).cast(),
            suppress,
            out.as_mut_ptr(),
        )
    } {
        1 => Ok(Some(unsafe { out.assume_init() })),
        0 => Ok(None),
        _ => Err(MoltObject::none().bits()),
    }
}

pub fn paths_same_file(
    _py: &CoreGilToken,
    left: &std::path::Path,
    right: &std::path::Path,
    operation: OperationId,
) -> u64 {
    let left = left.as_os_str().as_encoded_bytes();
    let right = right.as_os_str().as_encoded_bytes();
    unsafe {
        __molt_path_samefile(
            _py.attached_context(),
            left.as_ptr(),
            left.len(),
            right.as_ptr(),
            right.len(),
            std::ptr::from_ref(&operation).cast(),
        )
    }
}

/// Return the canonical metadata-route readlink result in the caller's flavor.
pub fn path_read_link(_py: &CoreGilToken, path: &std::path::Path, bytes: bool) -> u64 {
    let raw = path.as_os_str().as_encoded_bytes();
    unsafe { __molt_path_readlink(_py.attached_context(), raw.as_ptr(), raw.len(), bytes) }
}
