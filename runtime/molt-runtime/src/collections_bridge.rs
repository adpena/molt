//! FFI bridge shims for `molt-runtime-collections`.
//!
//! Each function here is a thin `#[no_mangle] extern "C"` wrapper around an
//! internal `pub(crate)` function.  The collections crate declares matching
//! `extern "C"` imports and they are resolved at link time.

use crate::object::ops::string_obj_to_owned as _string_obj_to_owned;
use crate::*;

type RuntimeExtensionStateInit = unsafe extern "C" fn() -> *mut u8;
type RuntimeExtensionStateClear = unsafe extern "C" fn(u64, *mut u8);
type RuntimeExtensionStateDrop = unsafe extern "C" fn(*mut u8);

#[unsafe(export_name = molt_codegen_abi::attached_symbol!("__molt_collections_runtime_state_get_or_init"))]
pub extern "C" fn __molt_collections_runtime_state_get_or_init(
    __molt_context: u64,
    key_ptr: *const u8,
    key_len: usize,
    init: RuntimeExtensionStateInit,
    clear: RuntimeExtensionStateClear,
    drop: RuntimeExtensionStateDrop,
) -> *mut u8 {
    {
        let _py = unsafe { crate::PyToken::from_attached_context(__molt_context) };
        let key = unsafe { std::slice::from_raw_parts(key_ptr, key_len) };
        crate::state::runtime_extension_state_get_or_init(
            crate::state::runtime_state::runtime_state(_py),
            key,
            init,
            clear,
            drop,
        )
    }
}

// ---------------------------------------------------------------------------
// Exception / error handling
// ---------------------------------------------------------------------------

#[unsafe(export_name = molt_codegen_abi::attached_symbol!("__molt_collections_raise_exception"))]
pub extern "C" fn __molt_collections_raise_exception(
    __molt_context: u64,
    kind_code: u16,
    msg_ptr: *const u8,
    msg_len: usize,
) -> u64 {
    {
        let _py = unsafe { crate::PyToken::from_attached_context(__molt_context) };
        let msg = unsafe { std::slice::from_raw_parts(msg_ptr, msg_len) };
        crate::builtins::exceptions::raise_exception_code_bytes(_py, kind_code, msg)
    }
}

#[unsafe(export_name = molt_codegen_abi::attached_symbol!("__molt_collections_exception_pending"))]
pub extern "C" fn __molt_collections_exception_pending(__molt_context: u64) -> i32 {
    {
        let _py = unsafe { crate::PyToken::from_attached_context(__molt_context) };
        if exception_pending(_py) { 1 } else { 0 }
    }
}

#[unsafe(export_name = molt_codegen_abi::attached_symbol!("__molt_collections_raise_key_error_with_key"))]
pub extern "C" fn __molt_collections_raise_key_error_with_key(
    __molt_context: u64,
    key_bits: u64,
) -> u64 {
    {
        let _py = unsafe { crate::PyToken::from_attached_context(__molt_context) };
        raise_key_error_with_key::<u64>(_py, key_bits)
    }
}

// ---------------------------------------------------------------------------
// Object allocation
// ---------------------------------------------------------------------------

#[unsafe(export_name = molt_codegen_abi::attached_symbol!("__molt_collections_alloc_tuple"))]
pub extern "C" fn __molt_collections_alloc_tuple(
    __molt_context: u64,
    elems_ptr: *const u64,
    elems_len: usize,
) -> *mut u8 {
    {
        let _py = unsafe { crate::PyToken::from_attached_context(__molt_context) };
        let elems = unsafe { std::slice::from_raw_parts(elems_ptr, elems_len) };
        alloc_tuple(_py, elems)
    }
}

#[unsafe(export_name = molt_codegen_abi::attached_symbol!("__molt_collections_alloc_list"))]
pub extern "C" fn __molt_collections_alloc_list(
    __molt_context: u64,
    elems_ptr: *const u64,
    elems_len: usize,
) -> *mut u8 {
    {
        let _py = unsafe { crate::PyToken::from_attached_context(__molt_context) };
        let elems = unsafe { std::slice::from_raw_parts(elems_ptr, elems_len) };
        alloc_list(_py, elems)
    }
}

#[unsafe(export_name = molt_codegen_abi::attached_symbol!("__molt_collections_alloc_string"))]
pub extern "C" fn __molt_collections_alloc_string(
    __molt_context: u64,
    data_ptr: *const u8,
    data_len: usize,
) -> *mut u8 {
    {
        let _py = unsafe { crate::PyToken::from_attached_context(__molt_context) };
        let data = unsafe { std::slice::from_raw_parts(data_ptr, data_len) };
        alloc_string(_py, data)
    }
}

#[unsafe(export_name = molt_codegen_abi::attached_symbol!("__molt_collections_alloc_dict_with_pairs"))]
pub extern "C" fn __molt_collections_alloc_dict_with_pairs(
    __molt_context: u64,
    pairs_ptr: *const u64,
    pairs_len: usize,
) -> *mut u8 {
    {
        let _py = unsafe { crate::PyToken::from_attached_context(__molt_context) };
        let pairs = unsafe { std::slice::from_raw_parts(pairs_ptr, pairs_len) };
        alloc_dict_with_pairs(_py, pairs)
    }
}

// ---------------------------------------------------------------------------
// Object inspection
// ---------------------------------------------------------------------------

#[unsafe(no_mangle)]
pub extern "C" fn __molt_collections_object_type_id(ptr: *mut u8) -> u32 {
    unsafe { object_type_id(ptr) }
}

#[unsafe(no_mangle)]
pub extern "C" fn __molt_collections_string_obj_to_owned(
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

#[unsafe(export_name = molt_codegen_abi::attached_symbol!("__molt_collections_is_truthy"))]
pub extern "C" fn __molt_collections_is_truthy(__molt_context: u64, bits: u64) -> i32 {
    {
        let _py = unsafe { crate::PyToken::from_attached_context(__molt_context) };
        let obj = obj_from_bits(bits);
        if is_truthy(_py, obj) { 1 } else { 0 }
    }
}

#[unsafe(export_name = molt_codegen_abi::attached_symbol!("__molt_collections_type_name"))]
pub extern "C" fn __molt_collections_type_name(
    __molt_context: u64,
    bits: u64,
    out_ptr: *mut *const u8,
    out_len: *mut usize,
) -> i32 {
    {
        let _py = unsafe { crate::PyToken::from_attached_context(__molt_context) };
        let obj = obj_from_bits(bits);
        let name = type_name(_py, obj);
        let owned: String = name.into_owned();
        let bytes = owned.into_bytes().into_boxed_slice();
        unsafe { crate::resource::bridge_buffer::export_u8_box(bytes, out_ptr, out_len) }
    }
}

// ---------------------------------------------------------------------------
// Reference counting
// ---------------------------------------------------------------------------

#[unsafe(export_name = molt_codegen_abi::attached_symbol!("__molt_collections_dec_ref_bits"))]
pub extern "C" fn __molt_collections_dec_ref_bits(__molt_context: u64, bits: u64) {
    {
        let _py = unsafe { crate::PyToken::from_attached_context(__molt_context) };
        dec_ref_bits(_py, bits);
    }
}

#[unsafe(export_name = molt_codegen_abi::attached_symbol!("__molt_collections_inc_ref_bits"))]
pub extern "C" fn __molt_collections_inc_ref_bits(__molt_context: u64, bits: u64) {
    {
        let _py = unsafe { crate::PyToken::from_attached_context(__molt_context) };
        inc_ref_bits(_py, bits);
    }
}

// ---------------------------------------------------------------------------
// Numeric helpers
// ---------------------------------------------------------------------------

#[unsafe(no_mangle)]
pub extern "C" fn __molt_collections_to_i64(bits: u64, out: *mut i64) -> i32 {
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

#[unsafe(export_name = molt_codegen_abi::attached_symbol!("__molt_collections_index_i64_with_overflow"))]
pub extern "C" fn __molt_collections_index_i64_with_overflow(
    __molt_context: u64,
    bits: u64,
    type_err_ptr: *const u8,
    type_err_len: usize,
    out: *mut i64,
) -> i32 {
    {
        let _py = unsafe { crate::PyToken::from_attached_context(__molt_context) };
        let type_err = unsafe {
            std::str::from_utf8_unchecked(std::slice::from_raw_parts(type_err_ptr, type_err_len))
        };
        match index_i64_with_overflow(_py, bits, type_err, None) {
            Some(v) => {
                unsafe {
                    *out = v;
                }
                1
            }
            None => 0,
        }
    }
}

// ---------------------------------------------------------------------------
// Container helpers
// ---------------------------------------------------------------------------

#[unsafe(export_name = molt_codegen_abi::attached_symbol!("__molt_collections_seq_snapshot"))]
pub extern "C" fn __molt_collections_seq_snapshot(
    __molt_context: u64,
    ptr: *mut u8,
    out_ptr: *mut *const u64,
    out_len: *mut usize,
) -> i32 {
    {
        let _py = unsafe { crate::PyToken::from_attached_context(__molt_context) };
        unsafe { crate::seq_snapshot_bridge::export(_py, ptr, out_ptr, out_len) }
    }
}

#[unsafe(export_name = molt_codegen_abi::attached_symbol!("__molt_collections_dict_snapshot"))]
pub extern "C" fn __molt_collections_dict_snapshot(
    __molt_context: u64,
    ptr: *mut u8,
    out_ptr: *mut *const u64,
    out_len: *mut usize,
) -> i32 {
    {
        let _py = unsafe { crate::PyToken::from_attached_context(__molt_context) };
        unsafe { crate::seq_snapshot_bridge::export_dict(_py, ptr, out_ptr, out_len) }
    }
}

// ---------------------------------------------------------------------------
// Object comparison
// ---------------------------------------------------------------------------

#[unsafe(export_name = molt_codegen_abi::attached_symbol!("__molt_collections_compare_eq"))]
pub extern "C" fn __molt_collections_compare_eq(
    __molt_context: u64,
    lhs_bits: u64,
    rhs_bits: u64,
) -> i32 {
    {
        let _py = unsafe { crate::PyToken::from_attached_context(__molt_context) };
        let lhs = obj_from_bits(lhs_bits);
        let rhs = obj_from_bits(rhs_bits);
        match crate::object::ops_compare::compare_object_eq_bool(_py, lhs, rhs) {
            crate::object::ops_compare::CompareBoolOutcome::True => 1,
            crate::object::ops_compare::CompareBoolOutcome::False
            | crate::object::ops_compare::CompareBoolOutcome::NotComparable => 0,
            crate::object::ops_compare::CompareBoolOutcome::Error => -1,
        }
    }
}

// ---------------------------------------------------------------------------
// Callable helpers
// ---------------------------------------------------------------------------

#[unsafe(export_name = molt_codegen_abi::attached_symbol!("__molt_collections_call_callable0"))]
pub extern "C" fn __molt_collections_call_callable0(__molt_context: u64, call_bits: u64) -> u64 {
    {
        let _py = unsafe { crate::PyToken::from_attached_context(__molt_context) };
        unsafe { crate::call::dispatch::call_callable0(_py, call_bits) }
    }
}

#[cfg(all(test, feature = "stdlib_collections"))]
mod tests {
    use crate::{TYPE_ID_FUNCTION, dec_ref_bits, obj_from_bits, object_type_id};
    use molt_obj_model::MoltObject;
    use std::sync::Once;

    static INIT: Once = Once::new();

    fn init_runtime() {
        INIT.call_once(|| {
            assert_ne!(crate::lifecycle::init(), 0);
        });
        crate::with_gil_entry_nopanic!(py, {
            let _ = crate::molt_exception_clear_attached_body(py);
        });
    }

    extern "C" fn default_factory_marker(__context: u64) -> i64 {
        assert_eq!(
            __context,
            crate::state::molt_attached_context(),
            "Python callback context identity"
        );
        let __py = unsafe { crate::PyToken::from_attached_context(__context) };
        MoltObject::from_int(42).bits() as i64
    }

    #[test]
    fn defaultdict_handle_owns_factory_after_caller_releases_local() {
        let _test = crate::test_support::RuntimeTestTransaction::new();
        init_runtime();

        crate::with_gil_entry_nopanic!(_py, {
            let func_ptr = crate::builtins::functions::alloc_runtime_function_obj(
                _py,
                default_factory_marker as *const () as usize as u64,
                0,
            );
            assert!(!func_ptr.is_null());
            assert_eq!(unsafe { object_type_id(func_ptr) }, TYPE_ID_FUNCTION);
            let func_bits = MoltObject::from_ptr(func_ptr).bits();

            let handle_bits =
                crate::molt_runtime_collections::collections_ext::molt_defaultdict_new_attached_body(_py.core_token(), func_bits);
            assert!(!obj_from_bits(handle_bits).is_none());

            dec_ref_bits(_py, func_bits);
            assert!(
                obj_from_bits(func_bits).as_ptr().is_some(),
                "defaultdict handle must keep the factory function live"
            );
            let rc_after_caller_drop =
                unsafe { (*crate::object::header_from_obj_ptr(func_ptr)).ref_count_snapshot() };
            assert_eq!(
                rc_after_caller_drop, 1,
                "the handle must be the remaining factory owner"
            );

            let key_bits = MoltObject::from_int(7).bits();
            let produced =
                crate::molt_runtime_collections::collections_ext::molt_defaultdict_missing_attached_body(_py.core_token(),
                    handle_bits,
                    key_bits,
                );
            assert_eq!(obj_from_bits(produced).as_int(), Some(42));

            let returned_factory =
                crate::molt_runtime_collections::collections_ext::molt_defaultdict_factory_attached_body(_py.core_token(),
                    handle_bits,
                );
            assert_eq!(returned_factory, func_bits);
            dec_ref_bits(_py, returned_factory);

            let _ = crate::molt_runtime_collections::collections_ext::molt_defaultdict_drop_attached_body(_py.core_token(),
                handle_bits,
            );
        });
    }
}
