//! FFI bridge shims for `molt-runtime-tk`.
//!
//! Each function here is a thin `#[no_mangle] extern "C"` wrapper around an
//! internal `pub(crate)` function.  The tk crate declares matching
//! `unsafe extern "C"` imports and they are resolved at link time.

use crate::*;

// ---------------------------------------------------------------------------
// Object layout access
// ---------------------------------------------------------------------------

/// Return the type-id tag for the object at `ptr`.
#[unsafe(no_mangle)]
pub extern "C" fn molt_rt_object_type_id(ptr: *mut u8) -> u32 {
    if ptr.is_null() {
        return 0;
    }
    unsafe { object_type_id(ptr) }
}

/// Write the (pointer, length) of a list/tuple's element buffer into the
/// out-params.  Caller must NOT free the pointer — it points into the
/// live Vec<u64> owned by the runtime.
#[unsafe(no_mangle)]
pub extern "C" fn molt_rt_seq_vec_ref(
    ptr: *mut u8,
    out_ptr: *mut *const u64,
    out_len: *mut usize,
) {
    if ptr.is_null() {
        unsafe {
            *out_ptr = std::ptr::null();
            *out_len = 0;
        }
        return;
    }
    let vec = unsafe { crate::object::layout::seq_vec_ref(ptr) };
    unsafe {
        *out_ptr = vec.as_ptr();
        *out_len = vec.len();
    }
}

/// Write the (pointer, length) of a dict's order-vector into the out-params.
/// The order vector is the raw key-value interleaved storage: [k0,v0,k1,v1,...].
/// Caller must NOT free the pointer.
#[unsafe(no_mangle)]
pub extern "C" fn molt_rt_dict_order(
    ptr: *mut u8,
    out_ptr: *mut *const u64,
    out_len: *mut usize,
) {
    if ptr.is_null() {
        unsafe {
            *out_ptr = std::ptr::null();
            *out_len = 0;
        }
        return;
    }
    let vec = unsafe { crate::builtins::containers::dict_order(ptr) };
    unsafe {
        *out_ptr = vec.as_ptr();
        *out_len = vec.len();
    }
}
