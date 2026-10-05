//! Physical arithmetic slots shared by exact fast paths and inherited native
//! descriptors. Source operators own special-method and reflected dispatch.

use super::*;

/// Concatenation rejection belongs to the sequence slot, not generic numeric
/// dispatch. A Python class name is diagnostic data, never slot identity.
#[derive(Clone, Copy)]
pub(in crate::object) enum SequenceConcatKind {
    String,
    List,
    Tuple,
    BytesLike,
}

impl SequenceConcatKind {
    pub(in crate::object) fn raise<T: crate::builtins::exceptions::ExceptionSentinel>(
        self,
        py: &PyToken<'_>,
        left: u64,
        right: u64,
    ) -> T {
        let right_name = type_name(py, obj_from_bits(right));
        let message = match self {
            Self::String | Self::List | Self::Tuple => {
                let sequence = match self {
                    Self::String => "str",
                    Self::List => "list",
                    Self::Tuple => "tuple",
                    Self::BytesLike => unreachable!(),
                };
                // CPython's %.Ns truncates bytes before replacement decoding.
                let right_name =
                    String::from_utf8_lossy(&right_name.as_bytes()[..right_name.len().min(200)]);
                format!("can only concatenate {sequence} (not \"{right_name}\") to {sequence}")
            }
            Self::BytesLike => {
                let left_name = type_name(py, obj_from_bits(left));
                let left_name =
                    String::from_utf8_lossy(&left_name.as_bytes()[..left_name.len().min(100)]);
                let right_name =
                    String::from_utf8_lossy(&right_name.as_bytes()[..right_name.len().min(100)]);
                format!("can't concat {right_name} to {left_name}")
            }
        };
        raise_exception(py, "TypeError", &message)
    }
}

/// Sequence concat/repeat descriptors do not become numeric slots merely by
/// inheritance. Reuse runtime-symbol identity from the callable authority.
pub(crate) unsafe fn is_sequence_slot(raw: Option<u64>) -> bool {
    [
        fn_key!(sequence_add_slot),
        fn_key!(sequence_repeat_slot),
        fn_key!(molt_str_add_method),
        fn_key!(molt_list_add_method),
        fn_key!(molt_list_mul_method),
    ]
    .into_iter()
    .any(|symbol| unsafe { crate::call::type_policy::callable_matches_runtime_symbol(raw, symbol) })
}

pub(crate) fn sequence_add(py: &PyToken<'_>, left: u64, right: u64) -> u64 {
    unsafe {
        let Some(lhs) = obj_from_bits(left).as_ptr() else {
            return raise_exception(
                py,
                "TypeError",
                "sequence concatenation requires a native sequence",
            );
        };
        let kind = object_type_id(lhs);
        if crate::object::tuple_storage::native_tuple(left).is_some()
            || (kind == TYPE_ID_TUPLE
                && crate::object::tuple_storage::native_tuple(right).is_some())
        {
            let tuple = crate::object::tuple_storage::TupleStorage::from_bits(py, left).unwrap();
            return tuple.concat(right);
        }
        if kind == TYPE_ID_TUPLE {
            let Some(rhs) = obj_from_bits(right)
                .as_ptr()
                .filter(|&ptr| object_type_id(ptr) == TYPE_ID_TUPLE)
            else {
                return SequenceConcatKind::Tuple.raise(py, left, right);
            };
            let Some(items) = crate::object::seq_access::snapshot_concat(
                py,
                lhs,
                rhs,
                "tuple concatenation allocation failed",
            ) else {
                return MoltObject::none().bits();
            };
            let result = alloc_tuple(py, &items);
            return if result.is_null() {
                MoltObject::none().bits()
            } else {
                MoltObject::from_ptr(result).bits()
            };
        }
        if kind == TYPE_ID_STRING {
            let Some(rhs) = obj_from_bits(right)
                .as_ptr()
                .filter(|&ptr| object_type_id(ptr) == TYPE_ID_STRING)
            else {
                return SequenceConcatKind::String.raise(py, left, right);
            };
            let lhs = std::slice::from_raw_parts(string_bytes(lhs), string_len(lhs));
            let rhs = std::slice::from_raw_parts(string_bytes(rhs), string_len(rhs));
            return concat_bytes_like(py, lhs, rhs, kind)
                .unwrap_or_else(|| MoltObject::none().bits());
        }
        if matches!(kind, TYPE_ID_BYTES | TYPE_ID_BYTEARRAY) {
            if !crate::object::buffer_exports::supports_buffer(py, right) {
                return SequenceConcatKind::BytesLike.raise(py, left, right);
            }
            // Hold the export while reading its detached copy. No mutable byte
            // view survives an exporter callback.
            let Some((rhs, _export)) = crate::object::ops_bytes::byte_buffer(py, right) else {
                return MoltObject::none().bits();
            };
            let lhs = bytes_like_slice_raw(lhs).unwrap();
            return concat_bytes_like(py, lhs, &rhs, kind)
                .unwrap_or_else(|| MoltObject::none().bits());
        }
        raise_exception(
            py,
            "TypeError",
            "sequence concatenation requires a native sequence",
        )
    }
}

pub(crate) extern "C" fn sequence_add_slot(left: u64, right: u64) -> u64 {
    crate::with_gil_entry_nopanic!(py, { sequence_add(py, left, right) })
}

pub(crate) extern "C" fn sequence_repeat_slot(value: u64, count: u64) -> u64 {
    crate::with_gil_entry_nopanic!(py, {
        if crate::object::tuple_storage::native_tuple(value).is_some() {
            let tuple = crate::object::tuple_storage::TupleStorage::from_bits(py, value).unwrap();
            let Some(count) = sequence_repeat_count(py, count) else {
                return MoltObject::none().bits();
            };
            return tuple.repeat(count as isize);
        }
        let Some(ptr) = obj_from_bits(value).as_ptr().filter(|&ptr| unsafe {
            matches!(
                object_type_id(ptr),
                TYPE_ID_TUPLE | TYPE_ID_STRING | TYPE_ID_BYTES | TYPE_ID_BYTEARRAY
            )
        }) else {
            return raise_exception(py, "TypeError", "repetition requires a native sequence");
        };
        let Some(count) = sequence_repeat_count(py, count) else {
            return MoltObject::none().bits();
        };
        repeat_sequence(py, ptr, count).unwrap_or_else(|| MoltObject::none().bits())
    })
}

pub(crate) extern "C" fn string_mod_slot(value: u64, args: u64) -> u64 {
    crate::with_gil_entry_nopanic!(py, {
        let Some(ptr) = crate::object::ops_string::validate_string_receiver(py, value, "__mod__")
        else {
            return MoltObject::none().bits();
        };
        // Pin the immutable receiver across conversion callbacks; no full-input
        // copy or second parser buffer is needed.
        inc_ref_bits(py, value);
        let text = unsafe { std::slice::from_raw_parts(string_bytes(ptr), string_len(ptr)) };
        let rendered = string_percent_format_impl(py, text, value, args);
        molt_cpython_abi::api::errors::with_preserved_error(|| dec_ref_bits(py, value));
        let Some(rendered) = rendered else {
            return MoltObject::none().bits();
        };
        rendered.into_bits(py)
    })
}

pub(crate) extern "C" fn bytearray_iadd_slot(value: u64, other: u64) -> u64 {
    crate::with_gil_entry_nopanic!(py, {
        let Some(ptr) = obj_from_bits(value)
            .as_ptr()
            .filter(|&ptr| unsafe { object_type_id(ptr) == TYPE_ID_BYTEARRAY })
        else {
            return raise_exception(
                py,
                "TypeError",
                "bytearray in-place concatenation requires a bytearray",
            );
        };
        if !unsafe { bytearray_concat_in_place(py, ptr, other) } {
            return MoltObject::none().bits();
        }
        inc_ref_bits(py, value);
        value
    })
}

pub(crate) extern "C" fn bytearray_imul_slot(value: u64, count: u64) -> u64 {
    crate::with_gil_entry_nopanic!(py, {
        let Some(ptr) = obj_from_bits(value)
            .as_ptr()
            .filter(|&ptr| unsafe { object_type_id(ptr) == TYPE_ID_BYTEARRAY })
        else {
            return raise_exception(
                py,
                "TypeError",
                "bytearray in-place repetition requires a bytearray",
            );
        };
        let Some(count) = sequence_repeat_count(py, count) else {
            return MoltObject::none().bits();
        };
        repeat_sequence_in_place(py, ptr, count).unwrap_or_else(|| MoltObject::none().bits())
    })
}

#[derive(Clone, Copy)]
enum SetOp {
    Union,
    Intersection,
    Difference,
    Symdiff,
}

fn set_binary(py: &PyToken<'_>, left: u64, right: u64, op: SetOp) -> u64 {
    unsafe {
        let (Some(lhs), Some(rhs)) = (
            obj_from_bits(left)
                .as_ptr()
                .filter(|&ptr| is_set_like_type(object_type_id(ptr))),
            obj_from_bits(right)
                .as_ptr()
                .filter(|&ptr| is_set_like_type(object_type_id(ptr))),
        ) else {
            return not_implemented_bits(py);
        };
        let kind = set_like_result_type_id(object_type_id(lhs));
        match op {
            SetOp::Union => set_like_union(py, lhs, rhs, kind),
            SetOp::Intersection => set_like_intersection(py, lhs, rhs, kind),
            SetOp::Difference => set_like_difference(py, lhs, rhs, kind),
            SetOp::Symdiff => set_like_symdiff(py, lhs, rhs, kind),
        }
    }
}

fn set_inplace(py: &PyToken<'_>, left: u64, right: u64, op: SetOp) -> u64 {
    if !obj_from_bits(left)
        .as_ptr()
        .is_some_and(|ptr| unsafe { object_type_id(ptr) == TYPE_ID_SET })
    {
        return raise_exception(py, "TypeError", "in-place set arithmetic requires a set");
    }
    if !obj_from_bits(right)
        .as_ptr()
        .is_some_and(|ptr| unsafe { is_set_like_type(object_type_id(ptr)) })
    {
        return not_implemented_bits(py);
    }
    match op {
        SetOp::Union => {
            molt_set_update(left, right);
        }
        SetOp::Intersection => {
            molt_set_intersection_update(left, right);
        }
        SetOp::Difference => {
            molt_set_difference_update(left, right);
        }
        SetOp::Symdiff => {
            molt_set_symdiff_update(left, right);
        }
    }
    if exception_pending(py) {
        return MoltObject::none().bits();
    }
    inc_ref_bits(py, left);
    left
}

pub(crate) extern "C" fn set_or_slot(a: u64, b: u64) -> u64 {
    crate::with_gil_entry_nopanic!(py, { set_binary(py, a, b, SetOp::Union) })
}
pub(crate) extern "C" fn set_ror_slot(a: u64, b: u64) -> u64 {
    crate::with_gil_entry_nopanic!(py, { set_binary(py, b, a, SetOp::Union) })
}
pub(crate) extern "C" fn set_and_slot(a: u64, b: u64) -> u64 {
    crate::with_gil_entry_nopanic!(py, { set_binary(py, a, b, SetOp::Intersection) })
}
pub(crate) extern "C" fn set_rand_slot(a: u64, b: u64) -> u64 {
    crate::with_gil_entry_nopanic!(py, { set_binary(py, b, a, SetOp::Intersection) })
}
pub(crate) extern "C" fn set_sub_slot(a: u64, b: u64) -> u64 {
    crate::with_gil_entry_nopanic!(py, { set_binary(py, a, b, SetOp::Difference) })
}
pub(crate) extern "C" fn set_rsub_slot(a: u64, b: u64) -> u64 {
    crate::with_gil_entry_nopanic!(py, { set_binary(py, b, a, SetOp::Difference) })
}
pub(crate) extern "C" fn set_xor_slot(a: u64, b: u64) -> u64 {
    crate::with_gil_entry_nopanic!(py, { set_binary(py, a, b, SetOp::Symdiff) })
}
pub(crate) extern "C" fn set_rxor_slot(a: u64, b: u64) -> u64 {
    crate::with_gil_entry_nopanic!(py, { set_binary(py, b, a, SetOp::Symdiff) })
}
pub(crate) extern "C" fn set_ior_slot(a: u64, b: u64) -> u64 {
    crate::with_gil_entry_nopanic!(py, { set_inplace(py, a, b, SetOp::Union) })
}
pub(crate) extern "C" fn set_iand_slot(a: u64, b: u64) -> u64 {
    crate::with_gil_entry_nopanic!(py, { set_inplace(py, a, b, SetOp::Intersection) })
}
pub(crate) extern "C" fn set_isub_slot(a: u64, b: u64) -> u64 {
    crate::with_gil_entry_nopanic!(py, { set_inplace(py, a, b, SetOp::Difference) })
}
pub(crate) extern "C" fn set_ixor_slot(a: u64, b: u64) -> u64 {
    crate::with_gil_entry_nopanic!(py, { set_inplace(py, a, b, SetOp::Symdiff) })
}

fn complex_slot(py: &PyToken<'_>, a: u64, b: u64, op: ComplexArith) -> u64 {
    complex_binary_payload(py, op, obj_from_bits(a), obj_from_bits(b))
        .unwrap_or_else(|| not_implemented_bits(py))
}
pub(crate) extern "C" fn complex_add_slot(a: u64, b: u64) -> u64 {
    crate::with_gil_entry_nopanic!(py, { complex_slot(py, a, b, ComplexArith::Add) })
}
pub(crate) extern "C" fn complex_radd_slot(a: u64, b: u64) -> u64 {
    crate::with_gil_entry_nopanic!(py, { complex_slot(py, b, a, ComplexArith::Add) })
}
pub(crate) extern "C" fn complex_sub_slot(a: u64, b: u64) -> u64 {
    crate::with_gil_entry_nopanic!(py, { complex_slot(py, a, b, ComplexArith::Sub) })
}
pub(crate) extern "C" fn complex_rsub_slot(a: u64, b: u64) -> u64 {
    crate::with_gil_entry_nopanic!(py, { complex_slot(py, b, a, ComplexArith::Sub) })
}
pub(crate) extern "C" fn complex_mul_slot(a: u64, b: u64) -> u64 {
    crate::with_gil_entry_nopanic!(py, { complex_slot(py, a, b, ComplexArith::Mul) })
}
pub(crate) extern "C" fn complex_rmul_slot(a: u64, b: u64) -> u64 {
    crate::with_gil_entry_nopanic!(py, { complex_slot(py, b, a, ComplexArith::Mul) })
}
pub(crate) extern "C" fn complex_div_slot(a: u64, b: u64) -> u64 {
    crate::with_gil_entry_nopanic!(py, { complex_slot(py, a, b, ComplexArith::TrueDiv) })
}
pub(crate) extern "C" fn complex_rdiv_slot(a: u64, b: u64) -> u64 {
    crate::with_gil_entry_nopanic!(py, { complex_slot(py, b, a, ComplexArith::TrueDiv) })
}
pub(crate) extern "C" fn complex_pow_slot(a: u64, b: u64, modulus: u64) -> u64 {
    crate::with_gil_entry_nopanic!(py, {
        if !obj_from_bits(modulus).is_none() {
            return raise_exception(py, "ValueError", "complex modulo");
        }
        complex_power_payload(py, obj_from_bits(a), obj_from_bits(b))
            .unwrap_or_else(|| not_implemented_bits(py))
    })
}
pub(crate) extern "C" fn complex_rpow_slot(a: u64, b: u64, modulus: u64) -> u64 {
    crate::with_gil_entry_nopanic!(py, {
        if !obj_from_bits(modulus).is_none() {
            return raise_exception(py, "ValueError", "complex modulo");
        }
        complex_power_payload(py, obj_from_bits(b), obj_from_bits(a))
            .unwrap_or_else(|| not_implemented_bits(py))
    })
}

pub(crate) extern "C" fn complex_neg_slot(value: u64) -> u64 {
    crate::with_gil_entry_nopanic!(py, {
        let Some(ptr) = complex_ptr_from_bits(value) else {
            return not_implemented_bits(py);
        };
        let value = unsafe { *complex_ref(ptr) };
        complex_bits(py, -value.re, -value.im)
    })
}
pub(crate) extern "C" fn complex_pos_slot(value: u64) -> u64 {
    crate::object::ops_convert::complex_complex(value)
}
pub(crate) extern "C" fn complex_abs_slot(value: u64) -> u64 {
    crate::with_gil_entry_nopanic!(py, {
        let Some(ptr) = complex_ptr_from_bits(value) else {
            return not_implemented_bits(py);
        };
        let value = unsafe { *complex_ref(ptr) };
        float_result_bits(py, value.re.hypot(value.im))
    })
}
pub(crate) extern "C" fn complex_bool_slot(value: u64) -> u64 {
    crate::with_gil_entry_nopanic!(py, {
        let Some(ptr) = complex_ptr_from_bits(value) else {
            return not_implemented_bits(py);
        };
        let value = unsafe { *complex_ref(ptr) };
        MoltObject::from_bool(value.re != 0.0 || value.im != 0.0).bits()
    })
}

pub(crate) extern "C" fn float_bool_slot(value: u64) -> u64 {
    crate::with_gil_entry_nopanic!(py, {
        let value = as_float_extended(obj_from_bits(value));
        let Some(value) = value else {
            return raise_exception(py, "TypeError", "float.__bool__ requires a float");
        };
        MoltObject::from_bool(value != 0.0).bits()
    })
}

/// An explicit float arithmetic descriptor consumes numeric storage without
/// redispatching receiver/operand overrides. Normalize only owned scalar
/// carriers, then reuse the source operator's exact-builtin implementation.
/// This keeps one arithmetic algorithm and makes reflected dispatch finite.
fn float_binary_slot(
    py: &PyToken<'_>,
    receiver: u64,
    other: u64,
    reflected: bool,
    operation: extern "C" fn(u64, u64) -> u64,
) -> u64 {
    let Some(value) = as_float_extended(obj_from_bits(receiver)) else {
        return raise_exception(
            py,
            "TypeError",
            "float arithmetic requires a float receiver",
        );
    };
    let other_obj = obj_from_bits(other);
    let other = if let Some(value) = as_float_extended(other_obj) {
        float_result_bits(py, value)
    } else if let Some(value) = crate::builtins::numbers::index_bigint_integral_bits(other) {
        int_bits_from_bigint(py, value)
    } else {
        return not_implemented_bits(py);
    };
    if exception_pending(py) {
        dec_ref_bits(py, other);
        return MoltObject::none().bits();
    }
    let receiver = float_result_bits(py, value);
    if exception_pending(py) {
        dec_ref_bits(py, receiver);
        dec_ref_bits(py, other);
        return MoltObject::none().bits();
    }
    let result = if reflected {
        operation(other, receiver)
    } else {
        operation(receiver, other)
    };
    dec_ref_bits(py, receiver);
    dec_ref_bits(py, other);
    result
}

pub(crate) extern "C" fn float_add_slot(a: u64, b: u64) -> u64 {
    crate::with_gil_entry_nopanic!(py, { float_binary_slot(py, a, b, false, crate::molt_add) })
}

pub(crate) extern "C" fn float_radd_slot(a: u64, b: u64) -> u64 {
    crate::with_gil_entry_nopanic!(py, { float_binary_slot(py, a, b, true, crate::molt_add) })
}

pub(crate) extern "C" fn float_sub_slot(a: u64, b: u64) -> u64 {
    crate::with_gil_entry_nopanic!(py, { float_binary_slot(py, a, b, false, crate::molt_sub) })
}

pub(crate) extern "C" fn float_rsub_slot(a: u64, b: u64) -> u64 {
    crate::with_gil_entry_nopanic!(py, { float_binary_slot(py, a, b, true, crate::molt_sub) })
}

pub(crate) extern "C" fn float_mul_slot(a: u64, b: u64) -> u64 {
    crate::with_gil_entry_nopanic!(py, { float_binary_slot(py, a, b, false, crate::molt_mul) })
}

pub(crate) extern "C" fn float_rmul_slot(a: u64, b: u64) -> u64 {
    crate::with_gil_entry_nopanic!(py, { float_binary_slot(py, a, b, true, crate::molt_mul) })
}

pub(crate) extern "C" fn float_truediv_slot(a: u64, b: u64) -> u64 {
    crate::with_gil_entry_nopanic!(py, { float_binary_slot(py, a, b, false, crate::molt_div) })
}

pub(crate) extern "C" fn float_rtruediv_slot(a: u64, b: u64) -> u64 {
    crate::with_gil_entry_nopanic!(py, { float_binary_slot(py, a, b, true, crate::molt_div) })
}

pub(crate) extern "C" fn float_floordiv_slot(a: u64, b: u64) -> u64 {
    crate::with_gil_entry_nopanic!(py, {
        float_binary_slot(py, a, b, false, crate::molt_floordiv)
    })
}

pub(crate) extern "C" fn float_rfloordiv_slot(a: u64, b: u64) -> u64 {
    crate::with_gil_entry_nopanic!(py, {
        float_binary_slot(py, a, b, true, crate::molt_floordiv)
    })
}

pub(crate) extern "C" fn float_mod_slot(a: u64, b: u64) -> u64 {
    crate::with_gil_entry_nopanic!(py, { float_binary_slot(py, a, b, false, crate::molt_mod) })
}

pub(crate) extern "C" fn float_rmod_slot(a: u64, b: u64) -> u64 {
    crate::with_gil_entry_nopanic!(py, { float_binary_slot(py, a, b, true, crate::molt_mod) })
}

pub(crate) extern "C" fn float_divmod_slot(a: u64, b: u64) -> u64 {
    crate::with_gil_entry_nopanic!(py, {
        float_binary_slot(py, a, b, false, crate::molt_divmod_builtin)
    })
}

pub(crate) extern "C" fn float_rdivmod_slot(a: u64, b: u64) -> u64 {
    crate::with_gil_entry_nopanic!(py, {
        float_binary_slot(py, a, b, true, crate::molt_divmod_builtin)
    })
}

pub(crate) extern "C" fn float_pow_slot(a: u64, b: u64, modulus: u64) -> u64 {
    crate::with_gil_entry_nopanic!(py, {
        if !obj_from_bits(modulus).is_none() {
            return raise_exception(
                py,
                "TypeError",
                "pow() 3rd argument not allowed unless all arguments are integers",
            );
        }
        float_binary_slot(py, a, b, false, crate::molt_pow)
    })
}

pub(crate) extern "C" fn float_rpow_slot(a: u64, b: u64, modulus: u64) -> u64 {
    crate::with_gil_entry_nopanic!(py, {
        if !obj_from_bits(modulus).is_none() {
            return raise_exception(
                py,
                "TypeError",
                "pow() 3rd argument not allowed unless all arguments are integers",
            );
        }
        float_binary_slot(py, a, b, true, crate::molt_pow)
    })
}
