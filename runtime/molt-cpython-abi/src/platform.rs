//! Target C-runtime authority used by the CPython ABI surface.

use std::ffi::{c_int, c_long, c_ulong, c_void};

// The C integer width conversions are owned by `molt_codegen_abi::c_width`;
// this crate re-exports them so every ABI site spells one authority.
pub(crate) use molt_codegen_abi::c_width::{
    c_char_to_u8, c_long_to_i64, c_longlong_to_i64, c_ulong_to_c_uint, c_ulong_to_u64,
    c_ulonglong_to_u64,
};

/// `LONG_MIN` of the target C ABI, widened to `i64`.
pub(crate) const C_LONG_MIN: i64 = c_long_to_i64(c_long::MIN);
/// `LONG_MAX` of the target C ABI, widened to `i64`.
pub(crate) const C_LONG_MAX: i64 = c_long_to_i64(c_long::MAX);
/// `ULONG_MAX` of the target C ABI, widened to `u64`.
pub(crate) const C_ULONG_MAX: u64 = c_ulong_to_u64(c_ulong::MAX);

#[cfg(not(all(target_arch = "wasm32", target_os = "unknown")))]
pub(crate) const C_EDOM: c_int = libc::EDOM;
#[cfg(not(all(target_arch = "wasm32", target_os = "unknown")))]
pub(crate) const C_ERANGE: c_int = libc::ERANGE;

#[cfg(all(target_arch = "wasm32", target_os = "unknown"))]
mod freestanding_errno {
    use super::c_int;

    include!(concat!(env!("OUT_DIR"), "/freestanding_errno.rs"));
}
#[cfg(all(target_arch = "wasm32", target_os = "unknown"))]
pub(crate) use freestanding_errno::{C_EDOM, C_ERANGE};

#[repr(C)]
pub struct CFile {
    _opaque: [u8; 0],
}

unsafe extern "C" {
    fn molt_capi_write_bytes(text: *const u8, length: usize, stream: *mut CFile) -> c_int;
    fn molt_capi_malloc(size: usize) -> *mut c_void;
    fn molt_capi_calloc(size: usize) -> *mut c_void;
    fn molt_capi_realloc(ptr: *mut c_void, size: usize) -> *mut c_void;
    fn molt_capi_free(ptr: *mut c_void);
}

/// Write through the exact C runtime that owns the caller's `FILE` object.
///
/// Rust must not project or interpret `FILE`: it is target-libc-owned and is
/// opaque on freestanding wasm.  The C shim is compiled for the final target,
/// so native, WASI, and freestanding providers all consume one FILE/fwrite ABI
/// authority instead of a Rust-side target fallback.
#[inline]
pub(crate) unsafe fn write_bytes(text: *const u8, length: usize, stream: *mut CFile) -> c_int {
    unsafe { molt_capi_write_bytes(text, length, stream) }
}

/// Allocate through the target C runtime that also serves extension objects.
///
/// The provider owns CPython ABI allocation on every target. In particular,
/// freestanding wasm static links these symbols against the same WASI libc as
/// extension objects, so allocation and release can never cross heaps.
#[inline]
pub(crate) unsafe fn c_malloc(size: usize) -> *mut c_void {
    unsafe { molt_capi_malloc(size) }
}

#[inline]
pub(crate) unsafe fn c_calloc(size: usize) -> *mut c_void {
    unsafe { molt_capi_calloc(size) }
}

#[inline]
pub(crate) unsafe fn c_realloc(ptr: *mut c_void, size: usize) -> *mut c_void {
    unsafe { molt_capi_realloc(ptr, size) }
}

#[inline]
pub(crate) unsafe fn c_free(ptr: *mut c_void) {
    unsafe { molt_capi_free(ptr) };
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn allocator_authority_preserves_zero_and_reallocation_contracts() {
        unsafe {
            let ptr = c_calloc(8).cast::<u8>();
            assert!(!ptr.is_null());
            assert!((0..8).all(|index| *ptr.add(index) == 0));
            *ptr = 0xa5;
            let ptr = c_realloc(ptr.cast(), 32).cast::<u8>();
            assert!(!ptr.is_null());
            assert_eq!(*ptr, 0xa5);
            c_free(ptr.cast());

            let zero = c_malloc(0);
            assert!(!zero.is_null());
            c_free(zero);
        }
    }

    #[cfg(not(all(target_arch = "wasm32", target_os = "unknown")))]
    #[test]
    fn libc_targets_use_pointer_compatible_zero_header_allocations() {
        unsafe {
            let ptr = c_malloc(16);
            assert!(!ptr.is_null());
            let ptr = libc::realloc(ptr, 32);
            assert!(!ptr.is_null());
            libc::free(ptr);
        }
    }
}
