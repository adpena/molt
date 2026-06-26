//! Shared runtime bridge helpers for Tk and tkinter parsing intrinsics.
//!
//! This module is the only `molt-runtime-tk` owner of runtime object-layout
//! bridge declarations and small MoltObject conversion helpers. `tk.rs` and
//! `tkinter_core.rs` consume this boundary instead of carrying local shim
//! copies.

use molt_runtime_core::prelude::*;

unsafe extern "C" {
    fn __molt_tk_has_capability(name_ptr: *const u8, name_len: usize) -> i32;
    fn molt_call_bind(call_bits: u64, builder_bits: u64) -> u64;
    fn molt_callargs_new(pos_capacity: u64, kw_capacity: u64) -> u64;
    fn molt_callargs_push_pos(builder_bits: u64, val: u64) -> u64;
    fn molt_int_from_obj(val_bits: u64, base_bits: u64, has_base_bits: u64) -> u64;
    fn molt_is_callable_bool(obj_bits: u64) -> i32;
    fn molt_rt_dict_order(ptr: *mut u8, out_ptr: *mut *const u64, out_len: *mut usize);
    fn molt_rt_object_type_id(ptr: *mut u8) -> u32;
    fn molt_rt_seq_vec_ref(ptr: *mut u8, out_ptr: *mut *const u64, out_len: *mut usize);
}

pub(crate) fn string_obj_to_owned(obj: MoltObject) -> Option<String> {
    rt_string_as_bytes(obj.bits()).map(|b| String::from_utf8_lossy(b).into_owned())
}

pub(crate) fn to_i64(obj: MoltObject) -> Option<i64> {
    obj.as_int()
}

pub(crate) fn to_f64(obj: MoltObject) -> Option<f64> {
    obj.as_float()
}

pub(crate) fn dec_ref_bits(_py: &PyToken, bits: u64) {
    rt_dec_ref(bits);
}

pub(crate) fn inc_ref_bits(_py: &PyToken, bits: u64) {
    rt_inc_ref(bits);
}

pub(crate) fn exception_pending(_py: &PyToken) -> bool {
    rt_exception_pending()
}

pub(crate) fn clear_exception(_py: &PyToken) {
    rt_exception_clear();
}

pub(crate) fn raise_exception_u64(_py: &PyToken, kind: &str, msg: &str) -> u64 {
    rt_raise_str(kind, msg)
}

pub(crate) fn is_truthy(_py: &PyToken, obj: MoltObject) -> bool {
    rt_is_truthy(obj.bits())
}

pub(crate) fn has_capability(_py: &PyToken, name: &str) -> bool {
    unsafe { __molt_tk_has_capability(name.as_ptr(), name.len()) != 0 }
}

pub(crate) unsafe fn call_callable0(_py: &PyToken, call_bits: u64) -> u64 {
    let builder_bits = unsafe { molt_callargs_new(0, 0) };
    unsafe { molt_call_bind(call_bits, builder_bits) }
}

pub(crate) unsafe fn call_callable_args(_py: &PyToken, callback_bits: u64, args: &[u64]) -> u64 {
    let builder_bits = unsafe { molt_callargs_new(args.len() as u64, 0) };
    if builder_bits == 0 {
        return MoltObject::none().bits();
    }
    for &arg in args {
        let _ = unsafe { molt_callargs_push_pos(builder_bits, arg) };
    }
    unsafe { molt_call_bind(callback_bits, builder_bits) }
}

pub(crate) fn is_callable_bits(bits: u64) -> bool {
    unsafe { molt_is_callable_bool(bits) != 0 }
}

pub(crate) fn int_from_obj(val_bits: u64, base_bits: u64, has_base_bits: u64) -> u64 {
    unsafe { molt_int_from_obj(val_bits, base_bits, has_base_bits) }
}

pub(crate) fn decode_value_list(obj: MoltObject) -> Option<Vec<u64>> {
    let ptr = obj.as_ptr()?;
    let type_id = object_type_id(ptr);
    if type_id != TYPE_ID_LIST && type_id != TYPE_ID_TUPLE {
        return None;
    }
    Some(seq_vec_ref(ptr).to_vec())
}

pub(crate) fn decode_value_list_bits(bits: u64) -> Option<Vec<u64>> {
    decode_value_list(obj_from_bits(bits))
}

pub(crate) fn object_type_id(ptr: *mut u8) -> u32 {
    unsafe { molt_rt_object_type_id(ptr) }
}

pub(crate) fn seq_vec_ref(ptr: *mut u8) -> &'static [u64] {
    let mut out_ptr: *const u64 = std::ptr::null();
    let mut out_len: usize = 0;
    unsafe {
        molt_rt_seq_vec_ref(ptr, &mut out_ptr, &mut out_len);
        std::slice::from_raw_parts(out_ptr, out_len)
    }
}

pub(crate) fn dict_order(ptr: *mut u8) -> Vec<u64> {
    let mut out_ptr: *const u64 = std::ptr::null();
    let mut out_len: usize = 0;
    unsafe {
        molt_rt_dict_order(ptr, &mut out_ptr, &mut out_len);
        std::slice::from_raw_parts(out_ptr, out_len).to_vec()
    }
}

pub(crate) fn format_obj_str(_py: &PyToken, obj: MoltObject) -> String {
    let str_bits = rt_str(obj.bits());
    let result = rt_string_as_bytes(str_bits)
        .map(|b| String::from_utf8_lossy(b).into_owned())
        .unwrap_or_default();
    rt_dec_ref(str_bits);
    result
}

pub(crate) fn alloc_string_result(value: &str, error_msg: &str) -> Result<u64, u64> {
    let bits = rt_string_from(value);
    if bits == 0 || rt_exception_pending() {
        return Err(rt_raise_str("MemoryError", error_msg));
    }
    Ok(bits)
}

pub(crate) fn alloc_list_result(elems: &[u64], error_msg: &str) -> Result<u64, u64> {
    let bits = rt_list(elems);
    if bits == 0 || rt_exception_pending() {
        return Err(rt_raise_str("MemoryError", error_msg));
    }
    Ok(bits)
}

pub(crate) fn alloc_tuple_result(elems: &[u64], error_msg: &str) -> Result<u64, u64> {
    let bits = rt_tuple(elems);
    if bits == 0 || rt_exception_pending() {
        return Err(rt_raise_str("MemoryError", error_msg));
    }
    Ok(bits)
}
