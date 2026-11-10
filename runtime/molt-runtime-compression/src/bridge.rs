//! Bridge declarations for molt-runtime helper functions.
//!
//! These functions are `pub(crate)` in molt-runtime and cannot be called directly
//! from this crate. We declare them as `unsafe extern "C"` here; matching `#[no_mangle]`
//! wrappers in `molt-runtime/src/builtins/compression_bridge.rs` provide the
//! implementations at link time.
//!
//! Convention: all bridge functions use C-ABI-safe types only (u64, i64, bool,
//! raw pointers, usize). No `&PyToken`, `&str`, `MoltObject`, or `Cow` in signatures.

unsafe extern "C" {
    // -- Numbers / ints -------------------------------------------------------
    /// `int_bits_from_i64(_py, val) -> u64`
    #[link_name = molt_runtime_core::attached_symbol!("molt_bridge_int_bits_from_i64") ]
    pub fn molt_bridge_int_bits_from_i64(__molt_context: u64, val: i64) -> u64;

    /// `to_i64(obj) -> Option<i64>`. Returns i64; sets *ok = true/false.
    #[link_name = molt_runtime_core::attached_symbol!("molt_bridge_to_i64") ]
    pub fn molt_bridge_to_i64(__molt_context: u64, obj_bits: u64, ok: *mut bool) -> i64;

    /// `index_i64_from_obj(_py, bits, err) -> i64`
    /// `err_ptr`/`err_len` are a UTF-8 string slice for the error message.
    #[link_name = molt_runtime_core::attached_symbol!("molt_bridge_index_i64_from_obj") ]
    pub fn molt_bridge_index_i64_from_obj(
        __molt_context: u64,
        obj_bits: u64,
        err_ptr: *const u8,
        err_len: usize,
    ) -> i64;

    // -- Exceptions -----------------------------------------------------------
    /// `exception_pending(_py) -> bool`
    #[link_name = molt_runtime_core::attached_symbol!("molt_bridge_exception_pending") ]
    pub fn molt_bridge_exception_pending(__molt_context: u64) -> bool;

    // -- Bytes / memory -------------------------------------------------------
    /// `alloc_bytes(_py, data) -> *mut u8` (null on OOM)
    #[link_name = molt_runtime_core::attached_symbol!("molt_bridge_alloc_bytes") ]
    pub fn molt_bridge_alloc_bytes(
        __molt_context: u64,
        data_ptr: *const u8,
        data_len: usize,
    ) -> *mut u8;

    /// `bytes_data(ptr) -> *const u8` (raw data pointer of a bytes object)
    #[link_name = molt_runtime_core::attached_symbol!("molt_bridge_bytes_data") ]
    pub fn molt_bridge_bytes_data(__molt_context: u64, ptr: *mut u8) -> *const u8;

    /// `bytes_len(ptr) -> usize`
    #[link_name = molt_runtime_core::attached_symbol!("molt_bridge_bytes_len") ]
    pub fn molt_bridge_bytes_len(__molt_context: u64, ptr: *mut u8) -> usize;

    // -- Strings --------------------------------------------------------------
    /// `string_obj_to_owned(obj) -> Option<String>`
    /// Returns ptr to heap-allocated UTF-8 string (caller must free with molt_bridge_free_string),
    /// or null if obj is not a string. Writes length to *out_len.
    #[link_name = molt_runtime_core::attached_symbol!("molt_bridge_string_obj_to_owned") ]
    pub fn molt_bridge_string_obj_to_owned(
        __molt_context: u64,
        obj_bits: u64,
        out_len: *mut usize,
    ) -> *mut u8;

    /// Free a string allocated by `molt_bridge_string_obj_to_owned`.
    pub fn molt_bridge_free_string(ptr: *mut u8, len: usize);

    /// `type_name(_py, obj) -> Cow<str>`
    /// Returns ptr to UTF-8 string; writes length to *out_len.
    /// The returned pointer is valid until the next call to this function (static buffer).
    #[link_name = molt_runtime_core::attached_symbol!("molt_bridge_type_name") ]
    pub fn molt_bridge_type_name(
        __molt_context: u64,
        obj_bits: u64,
        out_len: *mut usize,
    ) -> *const u8;

    /// `alloc_string(_py, s) -> *mut u8`
    #[link_name = molt_runtime_core::attached_symbol!("molt_bridge_alloc_string") ]
    pub fn molt_bridge_alloc_string(
        __molt_context: u64,
        data_ptr: *const u8,
        data_len: usize,
    ) -> *mut u8;

    // -- Object allocation ----------------------------------------------------
    /// `alloc_list(_py, items, len) -> *mut u8`
    #[link_name = molt_runtime_core::attached_symbol!("molt_bridge_alloc_list") ]
    pub fn molt_bridge_alloc_list(
        __molt_context: u64,
        items_ptr: *const u64,
        items_len: usize,
    ) -> *mut u8;

    /// `alloc_tuple(_py, items, len) -> *mut u8`
    #[link_name = molt_runtime_core::attached_symbol!("molt_bridge_alloc_tuple") ]
    pub fn molt_bridge_alloc_tuple(
        __molt_context: u64,
        items_ptr: *const u64,
        items_len: usize,
    ) -> *mut u8;

    // -- Object inspection ----------------------------------------------------
    /// `object_type_id(ptr) -> u32`
    #[link_name = molt_runtime_core::attached_symbol!("molt_bridge_object_type_id") ]
    pub fn molt_bridge_object_type_id(__molt_context: u64, ptr: *mut u8) -> u32;

    // -- Pointer registry -----------------------------------------------------
    /// `release_ptr(ptr)`
    #[link_name = molt_runtime_core::attached_symbol!("molt_bridge_release_ptr") ]
    pub fn molt_bridge_release_ptr(__molt_context: u64, ptr: *mut u8);
}

// ── Ergonomic Rust wrappers ─────────────────────────────────────────────────

use molt_runtime_core::prelude::*;

// ---------------------------------------------------------------------------
// Runtime-scoped extension state
// ---------------------------------------------------------------------------

pub type RuntimeExtensionStateInit = unsafe extern "C" fn() -> *mut u8;
pub type RuntimeExtensionStateClear = unsafe extern "C" fn(u64, *mut u8);
pub type RuntimeExtensionStateDrop = unsafe extern "C" fn(*mut u8);

unsafe extern "C" {
    #[link_name = molt_runtime_core::attached_symbol!("molt_bridge_runtime_state_get_or_init") ]
    fn molt_bridge_runtime_state_get_or_init(
        __molt_context: u64,
        key_ptr: *const u8,
        key_len: usize,
        init: RuntimeExtensionStateInit,
        clear: RuntimeExtensionStateClear,
        drop: RuntimeExtensionStateDrop,
    ) -> *mut u8;
}

pub fn runtime_state_get_or_init(
    _py: &PyToken,
    key: &[u8],
    init: RuntimeExtensionStateInit,
    clear: RuntimeExtensionStateClear,
    drop: RuntimeExtensionStateDrop,
) -> *mut u8 {
    unsafe {
        molt_bridge_runtime_state_get_or_init(
            _py.attached_context(),
            key.as_ptr(),
            key.len(),
            init,
            clear,
            drop,
        )
    }
}

/// Raise an exception and return a sentinel u64.
#[inline]
pub fn raise_exception(_py: &PyToken, kind: impl Into<ExcKind>, msg: &str) -> u64 {
    molt_runtime_core::rt_raise_str(_py, kind, msg)
}

/// Check whether an exception is currently pending.
#[inline]
pub fn exception_pending(_py: &PyToken) -> bool {
    unsafe { molt_bridge_exception_pending(_py.attached_context()) }
}

/// Convert an i64 to NaN-boxed int bits.
#[inline]
pub fn int_bits_from_i64(_py: &PyToken, val: i64) -> u64 {
    unsafe { molt_bridge_int_bits_from_i64(_py.attached_context(), val) }
}

/// Extract i64 from a MoltObject, raising TypeError on failure.
#[inline]
pub fn index_i64_from_obj(_py: &PyToken, bits: u64, err: &str) -> i64 {
    unsafe { molt_bridge_index_i64_from_obj(_py.attached_context(), bits, err.as_ptr(), err.len()) }
}

/// Try to extract i64 from a MoltObject.
#[inline]
pub fn to_i64(_py: &PyToken, obj: MoltObject) -> Option<i64> {
    let mut ok = false;
    let val = unsafe { molt_bridge_to_i64(_py.attached_context(), obj.bits(), &mut ok) };
    if ok { Some(val) } else { None }
}

/// Allocate a new bytes object. Returns null on OOM.
#[inline]
pub fn alloc_bytes(_py: &PyToken, data: &[u8]) -> *mut u8 {
    unsafe { molt_bridge_alloc_bytes(_py.attached_context(), data.as_ptr(), data.len()) }
}

/// Extract an owned String from a string MoltObject.
#[inline]
pub fn string_obj_to_owned(_py: &PyToken, obj: MoltObject) -> Option<String> {
    let mut len: usize = 0;
    let ptr =
        unsafe { molt_bridge_string_obj_to_owned(_py.attached_context(), obj.bits(), &mut len) };
    if ptr.is_null() {
        None
    } else {
        let slice = unsafe { std::slice::from_raw_parts(ptr, len) };
        let s = String::from_utf8_lossy(slice).into_owned();
        unsafe { molt_bridge_free_string(ptr, len) };
        Some(s)
    }
}

/// Get the type name of an object.
#[inline]
pub fn type_name(_py: &PyToken, obj: MoltObject) -> String {
    let mut len: usize = 0;
    let ptr = unsafe { molt_bridge_type_name(_py.attached_context(), obj.bits(), &mut len) };
    if ptr.is_null() {
        "object".to_string()
    } else {
        let slice = unsafe { std::slice::from_raw_parts(ptr, len) };
        String::from_utf8_lossy(slice).into_owned()
    }
}

/// Release a pointer from the provenance registry.
#[inline]
/// # Safety
///
/// `ptr` must have been allocated by the paired runtime bridge and must not be
/// used again after release.
pub unsafe fn release_ptr(_py: &PyToken, ptr: *mut u8) {
    unsafe { molt_bridge_release_ptr(_py.attached_context(), ptr) }
}

/// Allocate a string object.
#[inline]
pub fn alloc_string(_py: &PyToken, s: &[u8]) -> *mut u8 {
    unsafe { molt_bridge_alloc_string(_py.attached_context(), s.as_ptr(), s.len()) }
}

/// Allocate a list object from a slice of item bits.
#[inline]
pub fn alloc_list(_py: &PyToken, items: &[u64]) -> *mut u8 {
    unsafe { molt_bridge_alloc_list(_py.attached_context(), items.as_ptr(), items.len()) }
}

/// Allocate a tuple object from a slice of item bits.
#[inline]
pub fn alloc_tuple(_py: &PyToken, items: &[u64]) -> *mut u8 {
    unsafe { molt_bridge_alloc_tuple(_py.attached_context(), items.as_ptr(), items.len()) }
}

/// Get the type ID of an object from its pointer.
#[inline]
/// # Safety
///
/// `ptr` must be a valid Molt runtime object pointer for the duration of this
/// call.
pub unsafe fn object_type_id(_py: &PyToken, ptr: *mut u8) -> u32 {
    unsafe { molt_bridge_object_type_id(_py.attached_context(), ptr) }
}

/// Get raw bytes data pointer.
#[inline]
/// # Safety
///
/// `ptr` must refer to a live Molt bytes-like object.
pub unsafe fn bytes_data(_py: &PyToken, ptr: *mut u8) -> *const u8 {
    unsafe { molt_bridge_bytes_data(_py.attached_context(), ptr) }
}

/// Get bytes length.
#[inline]
/// # Safety
///
/// `ptr` must refer to a live Molt bytes-like object.
pub unsafe fn bytes_len(_py: &PyToken, ptr: *mut u8) -> usize {
    unsafe { molt_bridge_bytes_len(_py.attached_context(), ptr) }
}

unsafe extern "C" {
    #[link_name = molt_runtime_core::attached_symbol!("__molt_compression_zlib_compressor_owner_new")]
    pub fn __molt_compression_zlib_compressor_owner_new(
        __molt_context: u64,
        pointer: *const std::ffi::c_void,
    ) -> u64;
    #[link_name = molt_runtime_core::attached_symbol!("__molt_compression_zlib_compressor_owner_clone")]
    pub fn __molt_compression_zlib_compressor_owner_clone(
        __molt_context: u64,
        bits: u64,
    ) -> *const std::ffi::c_void;
    #[link_name = molt_runtime_core::attached_symbol!("__molt_compression_zlib_decompressor_owner_new")]
    pub fn __molt_compression_zlib_decompressor_owner_new(
        __molt_context: u64,
        pointer: *const std::ffi::c_void,
    ) -> u64;
    #[link_name = molt_runtime_core::attached_symbol!("__molt_compression_zlib_decompressor_owner_clone")]
    pub fn __molt_compression_zlib_decompressor_owner_clone(
        __molt_context: u64,
        bits: u64,
    ) -> *const std::ffi::c_void;
    #[link_name = molt_runtime_core::attached_symbol!("__molt_compression_bz2_compressor_owner_new")]
    pub fn __molt_compression_bz2_compressor_owner_new(
        __molt_context: u64,
        pointer: *const std::ffi::c_void,
    ) -> u64;
    #[link_name = molt_runtime_core::attached_symbol!("__molt_compression_bz2_compressor_owner_clone")]
    pub fn __molt_compression_bz2_compressor_owner_clone(
        __molt_context: u64,
        bits: u64,
    ) -> *const std::ffi::c_void;
    #[link_name = molt_runtime_core::attached_symbol!("__molt_compression_bz2_decompressor_owner_new")]
    pub fn __molt_compression_bz2_decompressor_owner_new(
        __molt_context: u64,
        pointer: *const std::ffi::c_void,
    ) -> u64;
    #[link_name = molt_runtime_core::attached_symbol!("__molt_compression_bz2_decompressor_owner_clone")]
    pub fn __molt_compression_bz2_decompressor_owner_clone(
        __molt_context: u64,
        bits: u64,
    ) -> *const std::ffi::c_void;
    #[link_name = molt_runtime_core::attached_symbol!("__molt_compression_bz2_file_owner_new")]
    pub fn __molt_compression_bz2_file_owner_new(
        __molt_context: u64,
        pointer: *const std::ffi::c_void,
    ) -> u64;
    #[link_name = molt_runtime_core::attached_symbol!("__molt_compression_bz2_file_owner_clone")]
    pub fn __molt_compression_bz2_file_owner_clone(
        __molt_context: u64,
        bits: u64,
    ) -> *const std::ffi::c_void;
    #[cfg(not(target_arch = "wasm32"))]
    #[link_name = molt_runtime_core::attached_symbol!("__molt_compression_lzma_compressor_owner_new")]
    pub fn __molt_compression_lzma_compressor_owner_new(
        __molt_context: u64,
        pointer: *const std::ffi::c_void,
    ) -> u64;
    #[cfg(not(target_arch = "wasm32"))]
    #[link_name = molt_runtime_core::attached_symbol!("__molt_compression_lzma_compressor_owner_clone")]
    pub fn __molt_compression_lzma_compressor_owner_clone(
        __molt_context: u64,
        bits: u64,
    ) -> *const std::ffi::c_void;
    #[cfg(not(target_arch = "wasm32"))]
    #[link_name = molt_runtime_core::attached_symbol!("__molt_compression_lzma_decompressor_owner_new")]
    pub fn __molt_compression_lzma_decompressor_owner_new(
        __molt_context: u64,
        pointer: *const std::ffi::c_void,
    ) -> u64;
    #[cfg(not(target_arch = "wasm32"))]
    #[link_name = molt_runtime_core::attached_symbol!("__molt_compression_lzma_decompressor_owner_clone")]
    pub fn __molt_compression_lzma_decompressor_owner_clone(
        __molt_context: u64,
        bits: u64,
    ) -> *const std::ffi::c_void;
    #[cfg(not(target_arch = "wasm32"))]
    #[link_name = molt_runtime_core::attached_symbol!("__molt_compression_lzma_file_owner_new")]
    pub fn __molt_compression_lzma_file_owner_new(
        __molt_context: u64,
        pointer: *const std::ffi::c_void,
    ) -> u64;
    #[cfg(not(target_arch = "wasm32"))]
    #[link_name = molt_runtime_core::attached_symbol!("__molt_compression_lzma_file_owner_clone")]
    pub fn __molt_compression_lzma_file_owner_clone(
        __molt_context: u64,
        bits: u64,
    ) -> *const std::ffi::c_void;
    #[link_name = molt_runtime_core::attached_symbol!("__molt_compression_gzip_file_owner_new")]
    pub fn __molt_compression_gzip_file_owner_new(
        __molt_context: u64,
        pointer: *const std::ffi::c_void,
    ) -> u64;
    #[link_name = molt_runtime_core::attached_symbol!("__molt_compression_gzip_file_owner_clone")]
    pub fn __molt_compression_gzip_file_owner_clone(
        __molt_context: u64,
        bits: u64,
    ) -> *const std::ffi::c_void;
}
