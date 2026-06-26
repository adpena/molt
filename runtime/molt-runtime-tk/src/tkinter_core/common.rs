use molt_runtime_core::prelude::*;

use crate::bridge::{
    alloc_list_result as bridge_alloc_list_result,
    alloc_string_result as bridge_alloc_string_result,
    alloc_tuple_result as bridge_alloc_tuple_result, string_obj_to_owned, to_f64, to_i64,
};

// ─── Internal helpers ────────────────────────────────────────────────────────

/// Allocate a Molt string from a Rust `&str` and return its NaN-boxed bits.
/// Returns `Err(bits)` on allocation failure (MemoryError raised).
pub(super) fn alloc_str_bits(value: &str) -> Result<u64, u64> {
    bridge_alloc_string_result(value, "failed to allocate tkinter_core string")
}

/// Allocate a Molt list from a slice of owned bits and return its NaN-boxed bits.
/// The elements are inc-ref'd by `rt_list`.
/// Returns `Err(bits)` on allocation failure.
pub(super) fn alloc_list_bits(elems: &[u64]) -> Result<u64, u64> {
    bridge_alloc_list_result(elems, "failed to allocate tkinter_core list")
}

/// Allocate a Molt tuple from a slice of owned bits and return its NaN-boxed bits.
/// The elements are inc-ref'd by `rt_tuple`.
/// Returns `Err(bits)` on allocation failure.
pub(super) fn alloc_tuple_result(elems: &[u64]) -> Result<u64, u64> {
    bridge_alloc_tuple_result(elems, "failed to allocate tkinter_core tuple")
}

/// Extract a Rust `String` from NaN-boxed bits that should be a Molt string.
/// Returns `None` if the value is not a string (could be int, None, etc.).
pub(super) fn bits_to_string(bits: u64) -> Option<String> {
    string_obj_to_owned(obj_from_bits(bits))
}

/// Try to interpret bits as an integer.  Handles int, bool, and int-subclass.
pub(super) fn bits_as_i64(bits: u64) -> Option<i64> {
    to_i64(obj_from_bits(bits))
}

/// Try to interpret bits as a float.
pub(super) fn bits_as_f64(bits: u64) -> Option<f64> {
    to_f64(obj_from_bits(bits))
}

/// Check if the value is None.
pub(super) fn bits_is_none(bits: u64) -> bool {
    obj_from_bits(bits).is_none()
}

/// Check if the value is an empty string.
fn bits_is_empty_string(bits: u64) -> bool {
    bits_to_string(bits).is_some_and(|s| s.is_empty())
}

/// Check if the value is None or an empty string.
pub(super) fn bits_is_empty_or_none(bits: u64) -> bool {
    bits_is_none(bits) || bits_is_empty_string(bits)
}

/// Dec-ref all bits in a vec (cleanup helper for error paths).
pub(super) fn cleanup_list(items: &[u64]) {
    for &bits in items {
        rt_dec_ref(bits);
    }
}
