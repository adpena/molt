//! Shared runtime bridge helpers for Tk and tkinter parsing intrinsics.
//!
//! This module is the only `molt-runtime-tk` owner of runtime object-layout
//! bridge declarations and small MoltObject conversion helpers. `tk.rs` and
//! `tkinter_core.rs` consume this boundary instead of carrying local shim
//! copies.

use molt_runtime_core::BuiltinExc;
use molt_runtime_core::prelude::{
    ExcKind, MoltObject, OwnedBridgeHandleSnapshot, OwnedRuntimeValue, PyToken, TYPE_ID_LIST,
    TYPE_ID_TUPLE, bridge_owned_handle_snapshot, obj_from_bits, rt_dec_ref, rt_exception_clear,
    rt_exception_pending, rt_inc_ref, rt_is_truthy, rt_list, rt_raise_str, rt_str,
    rt_string_as_bytes, rt_string_from, rt_tuple,
};

unsafe extern "C" {
    #[link_name = molt_runtime_core::attached_symbol!("__molt_tk_has_capability")]
    fn __molt_tk_has_capability(__molt_context: u64, name_ptr: *const u8, name_len: usize) -> i32;
    #[link_name = molt_runtime_core::attached_symbol!("molt_call_bind") ]
    fn molt_call_bind(__molt_context: u64, call_bits: u64, builder_bits: u64) -> u64;
    #[link_name = molt_runtime_core::attached_symbol!("molt_callargs_new") ]
    fn molt_callargs_new(__molt_context: u64, pos_capacity: u64, kw_capacity: u64) -> u64;
    #[link_name = molt_runtime_core::attached_symbol!("molt_callargs_push_pos") ]
    fn molt_callargs_push_pos(__molt_context: u64, builder_bits: u64, val: u64) -> u64;
    #[link_name = molt_runtime_core::attached_symbol!("molt_int_from_obj") ]
    fn molt_int_from_obj(
        __molt_context: u64,
        val_bits: u64,
        base_bits: u64,
        has_base_bits: u64,
    ) -> u64;
    fn molt_is_callable_bool(obj_bits: u64) -> i32;
    #[link_name = molt_runtime_core::attached_symbol!("molt_dict_snapshot") ]
    fn molt_dict_snapshot(
        __molt_context: u64,
        ptr: *mut u8,
        out_ptr: *mut *const u64,
        out_len: *mut usize,
    ) -> i32;
    #[link_name = molt_runtime_core::attached_symbol!("molt_rt_object_type_id") ]
    fn molt_rt_object_type_id(__molt_context: u64, ptr: *mut u8) -> u32;
    #[link_name = molt_runtime_core::attached_symbol!("molt_seq_snapshot") ]
    fn molt_seq_snapshot(
        __molt_context: u64,
        ptr: *mut u8,
        out_ptr: *mut *const u64,
        out_len: *mut usize,
    ) -> i32;
}

/// Copy a borrowed string before the runtime can reenter or release custody.
///
/// # Safety
/// The caller must keep `obj` live under current-thread execution/GIL custody
/// for this complete copy. No borrowed storage escapes this function.
pub(crate) unsafe fn string_obj_to_owned(_py: &PyToken, obj: MoltObject) -> Option<String> {
    // SAFETY: the caller owns this borrowed argument through the immediate copy.
    unsafe { rt_string_as_bytes(_py, obj.bits()) }.map(|b| String::from_utf8_lossy(b).into_owned())
}

pub(crate) fn to_i64(obj: MoltObject) -> Option<i64> {
    obj.as_int()
}

pub(crate) fn to_f64(obj: MoltObject) -> Option<f64> {
    obj.as_float()
}

pub(crate) fn dec_ref_bits(_py: &PyToken, bits: u64) {
    rt_dec_ref(_py, bits);
}

pub(crate) fn inc_ref_bits(_py: &PyToken, bits: u64) {
    rt_inc_ref(_py, bits);
}

pub(crate) fn exception_pending(_py: &PyToken) -> bool {
    rt_exception_pending(_py)
}

pub(crate) fn clear_exception(_py: &PyToken) {
    rt_exception_clear(_py);
}

pub(crate) fn raise_exception_u64(_py: &PyToken, kind: impl Into<ExcKind>, msg: &str) -> u64 {
    rt_raise_str(_py, kind, msg)
}

pub(crate) fn is_truthy(_py: &PyToken, obj: MoltObject) -> bool {
    rt_is_truthy(_py, obj.bits())
}

pub(crate) fn has_capability(_py: &PyToken, name: &str) -> bool {
    unsafe { __molt_tk_has_capability(_py.attached_context(), name.as_ptr(), name.len()) != 0 }
}

pub(crate) unsafe fn call_callable0(_py: &PyToken, call_bits: u64) -> u64 {
    let builder_bits = unsafe { molt_callargs_new(_py.attached_context(), 0, 0) };
    unsafe { molt_call_bind(_py.attached_context(), call_bits, builder_bits) }
}

pub(crate) unsafe fn call_callable_args(_py: &PyToken, callback_bits: u64, args: &[u64]) -> u64 {
    let builder_bits = unsafe { molt_callargs_new(_py.attached_context(), args.len() as u64, 0) };
    if builder_bits == 0 {
        return MoltObject::none().bits();
    }
    for &arg in args {
        let _ = unsafe { molt_callargs_push_pos(_py.attached_context(), builder_bits, arg) };
    }
    unsafe { molt_call_bind(_py.attached_context(), callback_bits, builder_bits) }
}

pub(crate) fn is_callable_bits(bits: u64) -> bool {
    unsafe { molt_is_callable_bool(bits) != 0 }
}

pub(crate) fn int_from_obj(
    _py: &PyToken,
    val_bits: u64,
    base_bits: u64,
    has_base_bits: u64,
) -> u64 {
    unsafe { molt_int_from_obj(_py.attached_context(), val_bits, base_bits, has_base_bits) }
}

/// Pin every element until the consumer finishes, including Python callbacks
/// that mutate or clear the source container during recursive Tcl conversion.
pub(crate) fn decode_value_list(
    _py: &PyToken,
    obj: MoltObject,
) -> Option<OwnedBridgeHandleSnapshot<'_>> {
    let ptr = obj.as_ptr()?;
    let type_id = object_type_id(_py, ptr);
    if type_id != TYPE_ID_LIST && type_id != TYPE_ID_TUPLE {
        return None;
    }
    unsafe { seq_snapshot(_py, ptr) }
}

pub(crate) fn decode_value_list_bits(
    _py: &PyToken,
    bits: u64,
) -> Option<OwnedBridgeHandleSnapshot<'_>> {
    decode_value_list(_py, obj_from_bits(bits))
}

pub(crate) fn object_type_id(_py: &PyToken, ptr: *mut u8) -> u32 {
    unsafe { molt_rt_object_type_id(_py.attached_context(), ptr) }
}

pub(crate) unsafe fn seq_snapshot(
    _py: &PyToken,
    ptr: *mut u8,
) -> Option<OwnedBridgeHandleSnapshot<'_>> {
    let mut out_ptr: *const u64 = std::ptr::null();
    let mut out_len: usize = 0;
    if unsafe { molt_seq_snapshot(_py.attached_context(), ptr, &mut out_ptr, &mut out_len) } == 0 {
        return None;
    }
    Some(unsafe { bridge_owned_handle_snapshot(_py, out_ptr, out_len) })
}

pub(crate) fn dict_snapshot(_py: &PyToken, ptr: *mut u8) -> Option<OwnedBridgeHandleSnapshot<'_>> {
    let mut out_ptr: *const u64 = std::ptr::null();
    let mut out_len: usize = 0;
    if unsafe { molt_dict_snapshot(_py.attached_context(), ptr, &mut out_ptr, &mut out_len) } == 0 {
        return None;
    }
    Some(unsafe { bridge_owned_handle_snapshot(_py, out_ptr, out_len) })
}

pub(crate) fn format_obj_str(_py: &PyToken, obj: MoltObject) -> String {
    // SAFETY: rt_str returns the one reference adopted by this existing owner.
    let text = unsafe { OwnedRuntimeValue::from_owned_bits(_py, rt_str(_py, obj.bits())) };
    text.string_bytes(_py)
        .map(|bytes| String::from_utf8_lossy(bytes).into_owned())
        .unwrap_or_default()
}

pub(crate) fn alloc_string_result(_py: &PyToken, value: &str, error_msg: &str) -> Result<u64, u64> {
    let bits = rt_string_from(_py, value);
    if bits == 0 || rt_exception_pending(_py) {
        return Err(rt_raise_str(_py, BuiltinExc::MemoryError, error_msg));
    }
    Ok(bits)
}

pub(crate) fn alloc_list_result(_py: &PyToken, elems: &[u64], error_msg: &str) -> Result<u64, u64> {
    let bits = rt_list(_py, elems);
    if bits == 0 || rt_exception_pending(_py) {
        return Err(rt_raise_str(_py, BuiltinExc::MemoryError, error_msg));
    }
    Ok(bits)
}

pub(crate) fn alloc_tuple_result(
    _py: &PyToken,
    elems: &[u64],
    error_msg: &str,
) -> Result<u64, u64> {
    let bits = rt_tuple(_py, elems);
    if bits == 0 || rt_exception_pending(_py) {
        return Err(rt_raise_str(_py, BuiltinExc::MemoryError, error_msg));
    }
    Ok(bits)
}
