//! FFI bridge for object-owned sequence access.
//!
//! Storage synchronization, pinning, and snapshots live in
//! `object::seq_access`; this module only translates those contracts to the
//! satellite C ABI and resource-accounted exported buffers.

use crate::*;

#[unsafe(no_mangle)]
pub extern "C" fn molt_seq_read_len(ptr: *mut u8) -> usize {
    crate::gil_assert();
    unsafe { crate::object::seq_access::len(ptr) }
}

/// Return one GIL-borrowed handle. The caller must keep the sequence alive and
/// consume the result before releasing the runtime GIL.
#[unsafe(no_mangle)]
pub extern "C" fn molt_seq_read_item_gil_borrowed(
    ptr: *mut u8,
    index: usize,
    out: *mut u64,
) -> i32 {
    unsafe { crate::object::seq_access::read_item_gil_borrowed(ptr, index, out) }
}

/// Return one owned handle. A successful result must be released with
/// `molt_dec_ref_obj` by the caller.
#[unsafe(no_mangle)]
pub extern "C" fn molt_seq_read_item_owned(ptr: *mut u8, index: usize, out: *mut u64) -> i32 {
    crate::object::seq_access::read_item_owned(ptr, index, out)
}

/// Export the canonical pinned sequence snapshot for every satellite runtime
/// crate. This symbol is intentionally feature-independent: Tk, itertools,
/// and future consumers must not acquire link custody from one another's
/// optional feature bridge.
#[unsafe(no_mangle)]
pub extern "C" fn molt_seq_snapshot(
    ptr: *mut u8,
    out_ptr: *mut *const u64,
    out_len: *mut usize,
) -> i32 {
    crate::with_gil_entry_nopanic!(py, { unsafe { export(py, ptr, out_ptr, out_len) } })
}

/// Export a stable, resource-accounted snapshot. The caller owns one reference
/// to every returned handle and releases the buffer through the bridge
/// allocator after releasing those references.
pub(crate) unsafe fn export(
    py: &PyToken<'_>,
    ptr: *mut u8,
    out_ptr: *mut *const u64,
    out_len: *mut usize,
) -> i32 {
    if ptr.is_null() || out_ptr.is_null() || out_len.is_null() {
        return 0;
    }
    let exported = unsafe {
        crate::object::seq_access::with_borrowed(ptr, |values| {
            if values.is_empty() {
                *out_ptr = std::ptr::null();
                *out_len = 0;
                return 1;
            }
            let exported =
                crate::resource::bridge_buffer::export_u64_slice(values, out_ptr, out_len);
            if exported != 0 {
                for &bits in std::slice::from_raw_parts(*out_ptr, *out_len) {
                    inc_ref_bits(py, bits);
                }
            }
            exported
        })
    };
    if exported == 0 {
        unsafe {
            *out_ptr = std::ptr::null();
            *out_len = 0;
        }
        return crate::abi_return::fail_memory::<crate::abi_return::FailureStatus>(py);
    }
    exported
}
