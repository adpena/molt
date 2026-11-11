//! Slice operations.

use crate::BuiltinExc;
use crate::*;
use molt_obj_model::MoltObject;
use num_bigint::BigInt;
use num_traits::{Signed, Zero};

#[cfg(test)]
#[path = "ops_slice_tests.rs"]
mod tests;

pub(crate) fn molt_slice_new_attached_body(
    _py: &crate::PyToken<'_>,
    start_bits: u64,
    stop_bits: u64,
    step_bits: u64,
) -> u64 {
    crate::with_attached_entry_nopanic!(_py, _py, {
        let ptr = alloc_slice_obj(_py, start_bits, stop_bits, step_bits);
        if ptr.is_null() {
            MoltObject::none().bits()
        } else {
            MoltObject::from_ptr(ptr).bits()
        }
    })
}

fn slice_indices_adjust(mut idx: BigInt, len: &BigInt, lower: &BigInt, upper: &BigInt) -> BigInt {
    if idx.is_negative() {
        idx += len;
    }
    if idx < *lower {
        return lower.clone();
    }
    if idx > *upper {
        return upper.clone();
    }
    idx
}

fn slice_reduce_tuple(_py: &PyToken<'_>, slice_ptr: *mut u8) -> u64 {
    unsafe {
        let start_bits = slice_start_bits(slice_ptr);
        let stop_bits = slice_stop_bits(slice_ptr);
        let step_bits = slice_step_bits(slice_ptr);
        let args_ptr = alloc_tuple(_py, &[start_bits, stop_bits, step_bits]);
        if args_ptr.is_null() {
            return MoltObject::none().bits();
        }
        let args_bits = MoltObject::from_ptr(args_ptr).bits();
        let class_bits = builtin_classes(_py).slice;
        inc_ref_bits(_py, class_bits);
        let res_ptr = crate::object::builders::alloc_tuple_owned(_py, &[class_bits, args_bits]);
        if res_ptr.is_null() {
            dec_ref_bits(_py, class_bits);
            dec_ref_bits(_py, args_bits);
            return MoltObject::none().bits();
        }
        MoltObject::from_ptr(res_ptr).bits()
    }
}

/// Exact arbitrary-precision slice normalization shared by slice.indices and
/// range subscription. Sequence storage's isize-clamped decoder is a distinct
/// contract; range lengths and steps must never truncate to host word size.
///
/// # Safety
/// `slice_ptr` must be a live exact slice; `len` must be nonnegative.
pub(super) unsafe fn slice_indices_bigint(
    _py: &PyToken<'_>,
    slice_ptr: *mut u8,
    len: &BigInt,
) -> Option<(BigInt, BigInt, BigInt)> {
    debug_assert!(!len.is_negative());
    let msg = "slice indices must be integers or None or have an __index__ method";
    let start_obj = obj_from_bits(unsafe { slice_start_bits(slice_ptr) });
    let stop_obj = obj_from_bits(unsafe { slice_stop_bits(slice_ptr) });
    let step_obj = obj_from_bits(unsafe { slice_step_bits(slice_ptr) });
    let step = if step_obj.is_none() {
        BigInt::from(1)
    } else {
        index_bigint_from_obj(_py, step_obj.bits(), msg)?
    };
    if step.is_zero() {
        raise_exception::<()>(_py, BuiltinExc::ValueError, "slice step cannot be zero");
        return None;
    }
    let step_neg = step.is_negative();
    let lower = if step_neg {
        BigInt::from(-1)
    } else {
        BigInt::from(0)
    };
    let upper = if step_neg { len - 1 } else { len.clone() };
    let start = if start_obj.is_none() {
        if step_neg {
            upper.clone()
        } else {
            lower.clone()
        }
    } else {
        let idx = index_bigint_from_obj(_py, start_obj.bits(), msg)?;
        slice_indices_adjust(idx, len, &lower, &upper)
    };
    let stop = if stop_obj.is_none() {
        if step_neg {
            lower.clone()
        } else {
            upper.clone()
        }
    } else {
        let idx = index_bigint_from_obj(_py, stop_obj.bits(), msg)?;
        slice_indices_adjust(idx, len, &lower, &upper)
    };
    Some((start, stop, step))
}

pub(crate) fn molt_slice_indices_attached_body(
    _py: &crate::PyToken<'_>,
    slice_bits: u64,
    length_bits: u64,
) -> u64 {
    crate::with_attached_entry_nopanic!(_py, _py, {
        let Some(slice_ptr) = obj_from_bits(slice_bits).as_ptr() else {
            return MoltObject::none().bits();
        };
        unsafe {
            if object_type_id(slice_ptr) != TYPE_ID_SLICE {
                return MoltObject::none().bits();
            }
            let msg = "slice indices must be integers or None or have an __index__ method";
            let Some(len) = index_bigint_from_obj(_py, length_bits, msg) else {
                return MoltObject::none().bits();
            };
            if len.is_negative() {
                return raise_exception::<_>(
                    _py,
                    BuiltinExc::ValueError,
                    "length should not be negative",
                );
            }
            let Some((start, stop, step)) = slice_indices_bigint(_py, slice_ptr, &len) else {
                return MoltObject::none().bits();
            };
            let start_bits = int_bits_from_bigint(_py, start);
            if exception_pending(_py) {
                return MoltObject::none().bits();
            }
            let stop_bits = int_bits_from_bigint(_py, stop);
            if exception_pending(_py) {
                dec_ref_bits(_py, start_bits);
                return MoltObject::none().bits();
            }
            let step_bits = int_bits_from_bigint(_py, step);
            if exception_pending(_py) {
                dec_ref_bits(_py, stop_bits);
                dec_ref_bits(_py, start_bits);
                return MoltObject::none().bits();
            }
            let tuple_ptr = crate::object::builders::alloc_tuple_owned(
                _py,
                &[start_bits, stop_bits, step_bits],
            );
            if tuple_ptr.is_null() {
                for bits in [step_bits, stop_bits, start_bits] {
                    dec_ref_bits(_py, bits);
                }
                return MoltObject::none().bits();
            }
            MoltObject::from_ptr(tuple_ptr).bits()
        }
    })
}

pub(crate) fn molt_slice_hash_attached_body(_py: &crate::PyToken<'_>, slice_bits: u64) -> u64 {
    crate::with_attached_entry_nopanic!(_py, _py, {
        let Some(slice_ptr) = obj_from_bits(slice_bits).as_ptr() else {
            return MoltObject::none().bits();
        };
        unsafe {
            if object_type_id(slice_ptr) != TYPE_ID_SLICE {
                return MoltObject::none().bits();
            }
            let start_bits = slice_start_bits(slice_ptr);
            let stop_bits = slice_stop_bits(slice_ptr);
            let step_bits = slice_step_bits(slice_ptr);
            let Some(hash) = hash_slice_bits(_py, start_bits, stop_bits, step_bits) else {
                return MoltObject::none().bits();
            };
            int_bits_from_i64(_py, hash)
        }
    })
}

pub(crate) fn molt_slice_eq_attached_body(
    _py: &crate::PyToken<'_>,
    slice_bits: u64,
    other_bits: u64,
) -> u64 {
    crate::with_attached_entry_nopanic!(_py, py, {
        crate::object::ops_compare::builtin_families::BuiltinComparison::Slice.invoke(
            py,
            slice_bits,
            other_bits,
            molt_obj_model::sequence_compare::RichCompareOp::Eq,
        )
    })
}

pub(crate) fn molt_slice_reduce_attached_body(_py: &crate::PyToken<'_>, slice_bits: u64) -> u64 {
    crate::with_attached_entry_nopanic!(_py, _py, {
        let Some(slice_ptr) = obj_from_bits(slice_bits).as_ptr() else {
            return MoltObject::none().bits();
        };
        unsafe {
            if object_type_id(slice_ptr) != TYPE_ID_SLICE {
                return MoltObject::none().bits();
            }
            slice_reduce_tuple(_py, slice_ptr)
        }
    })
}

pub(crate) fn molt_slice_reduce_ex_attached_body(
    _py: &crate::PyToken<'_>,
    slice_bits: u64,
    _protocol_bits: u64,
) -> u64 {
    crate::with_attached_entry_nopanic!(_py, _py, {
        molt_slice_reduce_attached_body(_py, slice_bits)
    })
}

include!("../attached_exports_generated/object/ops_slice.rs");
