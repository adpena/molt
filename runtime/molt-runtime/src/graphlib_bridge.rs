//! FFI bridge shims for `molt-stdlib-graphlib`.
//!
//! Each function here is a thin `#[no_mangle] extern "C"` wrapper around an
//! internal `pub(crate)` function.  The graphlib crate declares matching
//! `extern "C"` imports and they are resolved at link time.

use crate::object::ops::string_obj_to_owned as _string_obj_to_owned;
use crate::*;

// ---------------------------------------------------------------------------
// Exception / error handling
// ---------------------------------------------------------------------------

#[unsafe(no_mangle)]
pub extern "C" fn __molt_graphlib_raise_exception(
    kind_code: u16,
    msg_ptr: *const u8,
    msg_len: usize,
) -> u64 {
    crate::with_gil_entry_nopanic!(_py, {
        let msg = unsafe { std::slice::from_raw_parts(msg_ptr, msg_len) };
        crate::builtins::exceptions::raise_exception_code_bytes(_py, kind_code, msg)
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn __molt_graphlib_exception_pending() -> i32 {
    crate::with_gil_entry_nopanic!(_py, { if exception_pending(_py) { 1 } else { 0 } })
}

// ---------------------------------------------------------------------------
// Object allocation
// ---------------------------------------------------------------------------

#[unsafe(no_mangle)]
pub extern "C" fn __molt_graphlib_alloc_tuple(elems_ptr: *const u64, elems_len: usize) -> *mut u8 {
    crate::with_gil_entry_nopanic!(_py, {
        let elems = unsafe { std::slice::from_raw_parts(elems_ptr, elems_len) };
        alloc_tuple(_py, elems)
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn __molt_graphlib_alloc_list(elems_ptr: *const u64, elems_len: usize) -> *mut u8 {
    crate::with_gil_entry_nopanic!(_py, {
        let elems = unsafe { std::slice::from_raw_parts(elems_ptr, elems_len) };
        alloc_list(_py, elems)
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn __molt_graphlib_alloc_dict_with_pairs(
    pairs_ptr: *const u64,
    pairs_len: usize,
) -> *mut u8 {
    crate::with_gil_entry_nopanic!(_py, {
        let pairs = unsafe { std::slice::from_raw_parts(pairs_ptr, pairs_len) };
        alloc_dict_with_pairs(_py, pairs)
    })
}

// ---------------------------------------------------------------------------
// Object inspection
// ---------------------------------------------------------------------------

#[unsafe(no_mangle)]
pub extern "C" fn __molt_graphlib_object_type_id(ptr: *mut u8) -> u32 {
    unsafe { object_type_id(ptr) }
}

#[unsafe(no_mangle)]
pub extern "C" fn __molt_graphlib_string_obj_to_owned(
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

// ---------------------------------------------------------------------------
// Reference counting
// ---------------------------------------------------------------------------

#[unsafe(no_mangle)]
pub extern "C" fn __molt_graphlib_dec_ref_bits(bits: u64) {
    crate::with_gil_entry_nopanic!(_py, {
        dec_ref_bits(_py, bits);
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn __molt_graphlib_inc_ref_bits(bits: u64) {
    crate::with_gil_entry_nopanic!(_py, {
        inc_ref_bits(_py, bits);
    })
}

// ---------------------------------------------------------------------------
// Numeric helpers
// ---------------------------------------------------------------------------

#[unsafe(no_mangle)]
pub extern "C" fn __molt_graphlib_to_i64(bits: u64, out: *mut i64) -> i32 {
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

// ---------------------------------------------------------------------------
// Container helpers
// ---------------------------------------------------------------------------

#[unsafe(no_mangle)]
pub extern "C" fn __molt_graphlib_dict_get_in_place(
    dict_ptr: *mut u8,
    key_bits: u64,
    out: *mut u64,
) -> i32 {
    crate::with_gil_entry_nopanic!(_py, {
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
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn __molt_graphlib_dict_set_in_place(
    dict_ptr: *mut u8,
    key_bits: u64,
    val_bits: u64,
) -> i32 {
    crate::with_gil_entry_nopanic!(_py, {
        i32::from(unsafe { dict_set_in_place(_py, dict_ptr, key_bits, val_bits) })
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn __molt_graphlib_seq_snapshot(
    ptr: *mut u8,
    out_ptr: *mut *const u64,
    out_len: *mut usize,
) -> i32 {
    crate::with_gil_entry_nopanic!(_py, {
        unsafe { crate::seq_snapshot_bridge::export(_py, ptr, out_ptr, out_len) }
    })
}

#[cfg(test)]
mod tests {
    use crate::*;

    #[test]
    fn graph_handle_roundtrip_and_retirement_use_the_opaque_registry() {
        let _transaction = crate::test_support::RuntimeTestTransaction::new();
        crate::concurrency::gil::with_gil(|py| {
            let py = &py;
            let handle = molt_graphlib_new();
            assert!(!exception_pending(py));
            let token = obj_from_bits(handle).as_int().unwrap() as u64;
            assert!(molt_obj_model::resolve_opaque_ptr(token).is_some());
            let predecessors = alloc_tuple(py, &[]);
            assert!(!predecessors.is_null());
            let predecessors = MoltObject::from_ptr(predecessors).bits();
            let node = MoltObject::from_int(42).bits();
            molt_graphlib_add(handle, node, predecessors);
            assert!(!exception_pending(py));
            assert!(obj_from_bits(molt_graphlib_prepare(handle)).is_none());
            assert!(!exception_pending(py));
            let ready = molt_graphlib_get_ready(handle);
            assert!(!exception_pending(py));
            assert_eq!(
                molt_tuple_getitem_borrowed(ready, MoltObject::from_int(0).bits()),
                node
            );
            molt_graphlib_done(handle, ready);
            assert!(!exception_pending(py));
            assert_eq!(
                molt_graphlib_is_active(handle),
                MoltObject::from_bool(false).bits()
            );
            dec_ref_bits(py, ready);

            molt_graphlib_drop(handle);
            assert!(!exception_pending(py));
            assert!(molt_obj_model::resolve_opaque_ptr(token).is_none());
            molt_graphlib_drop(handle);
            assert!(!exception_pending(py));
            let replacement = molt_graphlib_new();
            assert!(!exception_pending(py));
            assert_ne!(replacement, handle);
            molt_graphlib_add(handle, node, predecessors);
            assert!(exception_pending(py));
            let error = molt_exception_last_pending();
            assert!(crate::builtins::exceptions::exception_matches_kind(
                py,
                error,
                crate::BuiltinExc::TypeError
            ));
            molt_exception_clear();
            dec_ref_bits(py, error);
            // A retired token cannot alias the new graph, which remains usable.
            molt_graphlib_add(replacement, node, predecessors);
            assert!(!exception_pending(py));
            molt_graphlib_drop(replacement);
            assert!(!exception_pending(py));
            dec_ref_bits(py, predecessors);
        });
    }
}
