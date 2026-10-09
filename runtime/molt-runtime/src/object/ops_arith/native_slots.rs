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
        fn_key!(crate::object::ops_list::list_iadd_slot),
        fn_key!(crate::object::ops_list::molt_list_imul_method),
        fn_key!(bytearray_iadd_slot),
        fn_key!(bytearray_imul_slot),
    ]
    .into_iter()
    .any(|symbol| unsafe { crate::call::type_policy::callable_matches_runtime_symbol(raw, symbol) })
}

/// Match the existing heap-type table allocation and native protocol facts.
/// This is table presence, not the broader Python sequence predicate.
pub(crate) unsafe fn has_sequence_table(py: &PyToken<'_>, receiver: u64) -> bool {
    unsafe {
        let Some(class) = obj_from_bits(type_of_bits(py, receiver)).as_ptr() else {
            return false;
        };
        crate::object::class_storage::class_is_heap_type(class)
            || crate::builtins::type_ops::class_mro_view(py, class)
                .iter()
                .any(|&base| {
                    obj_from_bits(base)
                        .as_ptr()
                        .and_then(|base| crate::object::class_storage::class_native_protocols(base))
                        .is_some_and(|slots| {
                            slots & molt_cpython_abi::hooks::NativeProtocolSlot::SEQUENCE_MASK != 0
                        })
                })
    }
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

/// The declaring dict slot admits actual dict storage, never a mapping protocol.
fn dict_binary(py: &PyToken<'_>, left: u64, right: u64) -> u64 {
    if ![left, right].into_iter().all(|value| {
        obj_from_bits(value)
            .as_ptr()
            .is_some_and(|ptr| unsafe { object_type_id(ptr) == TYPE_ID_DICT })
    }) {
        return not_implemented_bits(py);
    }
    let result = molt_dict_copy(left);
    if exception_pending(py) {
        return MoltObject::none().bits();
    }
    molt_dict_update(result, right);
    if exception_pending(py) {
        molt_cpython_abi::api::errors::with_preserved_error(|| dec_ref_bits(py, result));
        MoltObject::none().bits()
    } else {
        result
    }
}
pub(crate) extern "C" fn dict_or_slot(a: u64, b: u64) -> u64 {
    crate::with_gil_entry_nopanic!(py, { dict_binary(py, a, b) })
}
pub(crate) extern "C" fn dict_ror_slot(a: u64, b: u64) -> u64 {
    crate::with_gil_entry_nopanic!(py, { dict_binary(py, b, a) })
}
pub(crate) extern "C" fn dict_ior_slot(a: u64, b: u64) -> u64 {
    crate::with_gil_entry_nopanic!(py, {
        // Unlike normal union, dict_update_apply accepts mappings and pair
        // iterables, including their partial update and original error behavior.
        molt_dict_update(a, b);
        if exception_pending(py) {
            MoltObject::none().bits()
        } else {
            inc_ref_bits(py, a);
            a
        }
    })
}

#[derive(Clone, Copy)]
enum SetOp {
    Union,
    Intersection,
    Difference,
    Symdiff,
}

fn view_type(value: u64) -> Option<u32> {
    obj_from_bits(value).as_ptr().and_then(|ptr| unsafe {
        let kind = object_type_id(ptr);
        is_set_view_type(kind).then_some(kind)
    })
}

/// View intersection tests membership before adding an item to the result.
/// Eager conversion of an items view incorrectly hashes cancelled/absent values.
fn view_intersection(py: &PyToken<'_>, mut view: u64, mut other: u64) -> u64 {
    use crate::builtins::exceptions::ExceptionValue;
    use crate::object::ops_compare::builtin_families::BuiltinComparison;
    unsafe {
        if view_type(view).is_none() {
            std::mem::swap(&mut view, &mut other);
        }
        let view_ptr = obj_from_bits(view).as_ptr().unwrap();
        let length = dict_view_len(view_ptr);
        if let Some(other_ptr) = obj_from_bits(other).as_ptr() {
            if object_type_id(other_ptr) == TYPE_ID_SET
                && builtin_operand(py, obj_from_bits(other))
                && length <= crate::builtins::containers::set_len(other_ptr)
            {
                return crate::object::ops_set::set_intersection_bits(
                    py,
                    other_ptr,
                    view,
                    TYPE_ID_SET,
                );
            }
            if view_type(other).is_some() && dict_view_len(other_ptr) > length {
                std::mem::swap(&mut view, &mut other);
            }
        }
        let result = ExceptionValue::adopt(py, molt_set_new(0));
        let Some(result_ptr) = obj_from_bits(result.bits()).as_ptr() else {
            return MoltObject::none().bits();
        };
        let Some(mut iterator) = crate::object::iterable::OwnedIterator::new(py, other) else {
            return MoltObject::none().bits();
        };
        let family = if view_type(view) == Some(TYPE_ID_DICT_KEYS_VIEW) {
            BuiltinComparison::DictKeys
        } else {
            BuiltinComparison::DictItems
        };
        loop {
            let item = match iterator.next() {
                Ok(Some(item)) => ExceptionValue::adopt(py, item),
                Ok(None) => break,
                Err(_) => return MoltObject::none().bits(),
            };
            let contains = family.invoke_contains(py, view, item.bits());
            if exception_pending(py) {
                return MoltObject::none().bits();
            }
            if obj_from_bits(contains).as_bool() == Some(true) {
                set_add_in_place(py, result_ptr, item.bits(), HashContext::SetElement);
                if exception_pending(py) {
                    return MoltObject::none().bits();
                }
            }
        }
        result.into_bits()
    }
}

/// Items xor cancels equal values before hashing any result tuple. The source
/// entry's cached hash and both edges are captured together before callbacks.
fn items_view_xor(py: &PyToken<'_>, left: u64, right: u64) -> u64 {
    use crate::builtins::exceptions::ExceptionValue;
    use crate::object::ops::{dict_del_with_hash_deferred, dict_get_with_hash_in_place};
    use crate::object::ops_compare::{CompareBoolOutcome, compare_object_eq_bool};
    unsafe {
        let left_dict = dict_view_dict_bits(obj_from_bits(left).as_ptr().unwrap());
        let right_dict = ExceptionValue::pin(
            py,
            dict_view_dict_bits(obj_from_bits(right).as_ptr().unwrap()),
        );
        let temporary = ExceptionValue::adopt(py, molt_dict_copy(left_dict));
        if exception_pending(py) {
            return MoltObject::none().bits();
        }
        let temporary_ptr = obj_from_bits(temporary.bits()).as_ptr().unwrap();
        let result = ExceptionValue::adopt(py, molt_set_new(0));
        let Some(result_ptr) = obj_from_bits(result.bits()).as_ptr() else {
            return MoltObject::none().bits();
        };
        let right_ptr = obj_from_bits(right_dict.bits()).as_ptr().unwrap();
        let mut index = 0;
        while let Some(row) = dict_next_entry(right_ptr, &mut index) {
            let (key, value, hash) = (
                row.key,
                row.value,
                row.hash.expect("live dictionary row").get(),
            );
            let key = ExceptionValue::pin(py, key);
            let value = ExceptionValue::pin(py, value);

            let old = dict_get_with_hash_in_place(py, temporary_ptr, key.bits(), hash)
                .map(|bits| ExceptionValue::pin(py, bits));
            if exception_pending(py) {
                return MoltObject::none().bits();
            }
            let equal = if let Some(old) = &old {
                match compare_object_eq_bool(
                    py,
                    obj_from_bits(old.bits()),
                    obj_from_bits(value.bits()),
                ) {
                    CompareBoolOutcome::True => true,
                    CompareBoolOutcome::False | CompareBoolOutcome::NotComparable => false,
                    CompareBoolOutcome::Error => return MoltObject::none().bits(),
                }
            } else {
                false
            };
            if equal {
                let removed = dict_del_with_hash_deferred(py, temporary_ptr, key.bits(), hash);
                if removed.is_none() || exception_pending(py) {
                    return MoltObject::none().bits();
                }
                drop(removed);
            } else {
                let pair = alloc_tuple(py, &[key.bits(), value.bits()]);
                if pair.is_null() {
                    return MoltObject::none().bits();
                }
                let pair = ExceptionValue::adopt(py, MoltObject::from_ptr(pair).bits());
                set_add_in_place(py, result_ptr, pair.bits(), HashContext::SetElement);
            }
            if exception_pending(py) {
                return MoltObject::none().bits();
            }
        }
        let remaining = ExceptionValue::adopt(py, molt_dict_items(temporary.bits()));
        if exception_pending(py)
            || crate::object::ops_set::set_update_iterable(
                py,
                result_ptr,
                remaining.bits(),
                HashContext::SetElement,
            )
            .is_err()
        {
            return MoltObject::none().bits();
        }
        result.into_bits()
    }
}

fn view_binary(py: &PyToken<'_>, left: u64, right: u64, op: SetOp) -> u64 {
    use crate::builtins::exceptions::ExceptionValue;
    if view_type(left).is_none() && view_type(right).is_none() {
        return not_implemented_bits(py);
    }
    if matches!(op, SetOp::Intersection) {
        return view_intersection(py, left, right);
    }
    if matches!(op, SetOp::Symdiff)
        && view_type(left) == Some(TYPE_ID_DICT_ITEMS_VIEW)
        && view_type(right) == Some(TYPE_ID_DICT_ITEMS_VIEW)
    {
        return items_view_xor(py, left, right);
    }
    let result = ExceptionValue::adopt(py, molt_set_new(0));
    let Some(result_ptr) = obj_from_bits(result.bits()).as_ptr() else {
        return MoltObject::none().bits();
    };
    unsafe {
        // Exact dict keys may consume the dictionary's stored hashes, as
        // CPython dictviews_to_set does. Other operands retain iterator order.
        let source = if view_type(left) == Some(TYPE_ID_DICT_KEYS_VIEW) {
            let dict = dict_view_dict_bits(obj_from_bits(left).as_ptr().unwrap());
            if type_of_bits(py, dict) == builtin_classes(py).dict {
                dict
            } else {
                left
            }
        } else {
            left
        };
        if crate::object::ops_set::set_update_iterable(
            py,
            result_ptr,
            source,
            HashContext::SetElement,
        )
        .is_err()
        {
            return MoltObject::none().bits();
        }
        let updated = match op {
            SetOp::Union => crate::object::ops_set::set_update_iterable(
                py,
                result_ptr,
                right,
                HashContext::SetElement,
            ),
            SetOp::Difference => {
                crate::object::ops_set::set_difference_update_iterable(py, result_ptr, right)
            }
            SetOp::Symdiff => {
                crate::object::ops_set::set_symdiff_update_iterable(py, result_ptr, right)
            }
            SetOp::Intersection => unreachable!(),
        };
        if updated.is_err() {
            MoltObject::none().bits()
        } else {
            result.into_bits()
        }
    }
}

pub(crate) extern "C" fn view_or_slot(a: u64, b: u64) -> u64 {
    crate::with_gil_entry_nopanic!(py, { view_binary(py, a, b, SetOp::Union) })
}
pub(crate) extern "C" fn view_ror_slot(a: u64, b: u64) -> u64 {
    crate::with_gil_entry_nopanic!(py, { view_binary(py, b, a, SetOp::Union) })
}
pub(crate) extern "C" fn view_and_slot(a: u64, b: u64) -> u64 {
    crate::with_gil_entry_nopanic!(py, { view_binary(py, a, b, SetOp::Intersection) })
}
pub(crate) extern "C" fn view_rand_slot(a: u64, b: u64) -> u64 {
    crate::with_gil_entry_nopanic!(py, { view_binary(py, b, a, SetOp::Intersection) })
}
pub(crate) extern "C" fn view_sub_slot(a: u64, b: u64) -> u64 {
    crate::with_gil_entry_nopanic!(py, { view_binary(py, a, b, SetOp::Difference) })
}
pub(crate) extern "C" fn view_rsub_slot(a: u64, b: u64) -> u64 {
    crate::with_gil_entry_nopanic!(py, { view_binary(py, b, a, SetOp::Difference) })
}
pub(crate) extern "C" fn view_xor_slot(a: u64, b: u64) -> u64 {
    crate::with_gil_entry_nopanic!(py, { view_binary(py, a, b, SetOp::Symdiff) })
}
pub(crate) extern "C" fn view_rxor_slot(a: u64, b: u64) -> u64 {
    crate::with_gil_entry_nopanic!(py, { view_binary(py, b, a, SetOp::Symdiff) })
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
        complex_power_payload(py, obj_from_bits(a), obj_from_bits(b), modulus)
            .unwrap_or_else(|| not_implemented_bits(py))
    })
}
pub(crate) extern "C" fn complex_rpow_slot(a: u64, b: u64, modulus: u64) -> u64 {
    crate::with_gil_entry_nopanic!(py, {
        complex_power_payload(py, obj_from_bits(b), obj_from_bits(a), modulus)
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

/// Borrow the existing sealed integer carrier. These slots never invoke a
/// conversion protocol or retain a subclass payload across a callback: both
/// operands passed to the shared arithmetic kernel are exact builtin values.
fn integer_carrier(bits: u64) -> Option<u64> {
    let bits = crate::builtins::numbers::index_integral_payload_bits(bits)?;
    Some(
        obj_from_bits(bits)
            .as_bool()
            .map_or(bits, |value| MoltObject::from_int(i64::from(value)).bits()),
    )
}

fn int_unary_slot(py: &PyToken<'_>, value: u64, operation: extern "C" fn(u64) -> u64) -> u64 {
    let Some(value) = integer_carrier(value) else {
        return raise_exception(py, "TypeError", "int arithmetic requires an int receiver");
    };
    operation(value)
}
pub(crate) extern "C" fn int_neg_slot(value: u64) -> u64 {
    crate::with_gil_entry_nopanic!(py, { int_unary_slot(py, value, crate::molt_neg) })
}
pub(crate) extern "C" fn int_pos_slot(value: u64) -> u64 {
    crate::with_gil_entry_nopanic!(py, { int_unary_slot(py, value, crate::molt_pos) })
}
pub(crate) extern "C" fn int_abs_slot(value: u64) -> u64 {
    crate::with_gil_entry_nopanic!(py, { int_unary_slot(py, value, crate::molt_abs_builtin) })
}
pub(crate) extern "C" fn int_invert_slot(value: u64) -> u64 {
    crate::with_gil_entry_nopanic!(py, { int_unary_slot(py, value, crate::molt_invert) })
}
pub(crate) extern "C" fn int_bool_slot(value: u64) -> u64 {
    crate::with_gil_entry_nopanic!(py, {
        let Some(value) = integer_carrier(value) else {
            return raise_exception(py, "TypeError", "int truth requires an int receiver");
        };
        MoltObject::from_bool(is_truthy(py, obj_from_bits(value))).bits()
    })
}
pub(crate) extern "C" fn bool_and_slot(a: u64, b: u64) -> u64 {
    if obj_from_bits(a).is_bool() && obj_from_bits(b).is_bool() {
        crate::molt_bit_and(a, b)
    } else {
        int_and_slot(a, b)
    }
}
pub(crate) extern "C" fn bool_or_slot(a: u64, b: u64) -> u64 {
    if obj_from_bits(a).is_bool() && obj_from_bits(b).is_bool() {
        crate::molt_bit_or(a, b)
    } else {
        int_or_slot(a, b)
    }
}
pub(crate) extern "C" fn bool_xor_slot(a: u64, b: u64) -> u64 {
    if obj_from_bits(a).is_bool() && obj_from_bits(b).is_bool() {
        crate::molt_bit_xor(a, b)
    } else {
        int_xor_slot(a, b)
    }
}
pub(crate) extern "C" fn bool_invert_slot(value: u64) -> u64 {
    crate::molt_invert(value)
}

fn int_binary_slot(
    py: &PyToken<'_>,
    receiver: u64,
    other: u64,
    reflected: bool,
    operation: extern "C" fn(u64, u64) -> u64,
) -> u64 {
    let Some(receiver) = integer_carrier(receiver) else {
        return raise_exception(py, "TypeError", "int arithmetic requires an int receiver");
    };
    let Some(other) = integer_carrier(other) else {
        return not_implemented_bits(py);
    };
    if reflected {
        operation(other, receiver)
    } else {
        operation(receiver, other)
    }
}

pub(crate) extern "C" fn int_add_slot(a: u64, b: u64) -> u64 {
    crate::with_gil_entry_nopanic!(py, { int_binary_slot(py, a, b, false, crate::molt_add) })
}

pub(crate) extern "C" fn int_radd_slot(a: u64, b: u64) -> u64 {
    crate::with_gil_entry_nopanic!(py, { int_binary_slot(py, a, b, true, crate::molt_add) })
}

pub(crate) extern "C" fn int_sub_slot(a: u64, b: u64) -> u64 {
    crate::with_gil_entry_nopanic!(py, { int_binary_slot(py, a, b, false, crate::molt_sub) })
}

pub(crate) extern "C" fn int_rsub_slot(a: u64, b: u64) -> u64 {
    crate::with_gil_entry_nopanic!(py, { int_binary_slot(py, a, b, true, crate::molt_sub) })
}

pub(crate) extern "C" fn int_mul_slot(a: u64, b: u64) -> u64 {
    crate::with_gil_entry_nopanic!(py, { int_binary_slot(py, a, b, false, crate::molt_mul) })
}

pub(crate) extern "C" fn int_rmul_slot(a: u64, b: u64) -> u64 {
    crate::with_gil_entry_nopanic!(py, { int_binary_slot(py, a, b, true, crate::molt_mul) })
}

pub(crate) extern "C" fn int_truediv_slot(a: u64, b: u64) -> u64 {
    crate::with_gil_entry_nopanic!(py, { int_binary_slot(py, a, b, false, crate::molt_div) })
}

pub(crate) extern "C" fn int_rtruediv_slot(a: u64, b: u64) -> u64 {
    crate::with_gil_entry_nopanic!(py, { int_binary_slot(py, a, b, true, crate::molt_div) })
}

pub(crate) extern "C" fn int_floordiv_slot(a: u64, b: u64) -> u64 {
    crate::with_gil_entry_nopanic!(py, {
        int_binary_slot(py, a, b, false, crate::molt_floordiv)
    })
}

pub(crate) extern "C" fn int_rfloordiv_slot(a: u64, b: u64) -> u64 {
    crate::with_gil_entry_nopanic!(py, {
        int_binary_slot(py, a, b, true, crate::molt_floordiv)
    })
}

pub(crate) extern "C" fn int_mod_slot(a: u64, b: u64) -> u64 {
    crate::with_gil_entry_nopanic!(py, { int_binary_slot(py, a, b, false, crate::molt_mod) })
}

pub(crate) extern "C" fn int_rmod_slot(a: u64, b: u64) -> u64 {
    crate::with_gil_entry_nopanic!(py, { int_binary_slot(py, a, b, true, crate::molt_mod) })
}

pub(crate) extern "C" fn int_divmod_slot(a: u64, b: u64) -> u64 {
    crate::with_gil_entry_nopanic!(py, {
        int_binary_slot(py, a, b, false, crate::molt_divmod_builtin)
    })
}

pub(crate) extern "C" fn int_rdivmod_slot(a: u64, b: u64) -> u64 {
    crate::with_gil_entry_nopanic!(py, {
        int_binary_slot(py, a, b, true, crate::molt_divmod_builtin)
    })
}

pub(crate) extern "C" fn int_lshift_slot(a: u64, b: u64) -> u64 {
    crate::with_gil_entry_nopanic!(py, { int_binary_slot(py, a, b, false, crate::molt_lshift) })
}

pub(crate) extern "C" fn int_rlshift_slot(a: u64, b: u64) -> u64 {
    crate::with_gil_entry_nopanic!(py, { int_binary_slot(py, a, b, true, crate::molt_lshift) })
}

pub(crate) extern "C" fn int_rshift_slot(a: u64, b: u64) -> u64 {
    crate::with_gil_entry_nopanic!(py, { int_binary_slot(py, a, b, false, crate::molt_rshift) })
}

pub(crate) extern "C" fn int_rrshift_slot(a: u64, b: u64) -> u64 {
    crate::with_gil_entry_nopanic!(py, { int_binary_slot(py, a, b, true, crate::molt_rshift) })
}

pub(crate) extern "C" fn int_and_slot(a: u64, b: u64) -> u64 {
    crate::with_gil_entry_nopanic!(py, {
        int_binary_slot(py, a, b, false, crate::molt_bit_and)
    })
}

pub(crate) extern "C" fn int_rand_slot(a: u64, b: u64) -> u64 {
    crate::with_gil_entry_nopanic!(py, { int_binary_slot(py, a, b, true, crate::molt_bit_and) })
}

pub(crate) extern "C" fn int_or_slot(a: u64, b: u64) -> u64 {
    crate::with_gil_entry_nopanic!(py, { int_binary_slot(py, a, b, false, crate::molt_bit_or) })
}

pub(crate) extern "C" fn int_ror_slot(a: u64, b: u64) -> u64 {
    crate::with_gil_entry_nopanic!(py, { int_binary_slot(py, a, b, true, crate::molt_bit_or) })
}

pub(crate) extern "C" fn int_xor_slot(a: u64, b: u64) -> u64 {
    crate::with_gil_entry_nopanic!(py, {
        int_binary_slot(py, a, b, false, crate::molt_bit_xor)
    })
}

pub(crate) extern "C" fn int_rxor_slot(a: u64, b: u64) -> u64 {
    crate::with_gil_entry_nopanic!(py, { int_binary_slot(py, a, b, true, crate::molt_bit_xor) })
}

fn int_power_slot(
    py: &PyToken<'_>,
    receiver: u64,
    other: u64,
    modulus: u64,
    reflected: bool,
) -> u64 {
    let Some(receiver) = integer_carrier(receiver) else {
        return not_implemented_bits(py);
    };
    let Some(other) = integer_carrier(other) else {
        return not_implemented_bits(py);
    };
    let modulus = if obj_from_bits(modulus).is_none() {
        modulus
    } else {
        let Some(modulus) = integer_carrier(modulus) else {
            return not_implemented_bits(py);
        };
        modulus
    };
    if reflected {
        crate::molt_pow_mod(other, receiver, modulus)
    } else {
        crate::molt_pow_mod(receiver, other, modulus)
    }
}

pub(crate) extern "C" fn int_pow_slot(a: u64, b: u64, modulus: u64) -> u64 {
    crate::with_gil_entry_nopanic!(py, { int_power_slot(py, a, b, modulus, false) })
}
pub(crate) extern "C" fn int_rpow_slot(a: u64, b: u64, modulus: u64) -> u64 {
    crate::with_gil_entry_nopanic!(py, { int_power_slot(py, a, b, modulus, true) })
}

fn float_unary_slot(py: &PyToken<'_>, value: u64, operation: extern "C" fn(u64) -> u64) -> u64 {
    let Some(value) = as_float_extended(obj_from_bits(value)) else {
        return raise_exception(
            py,
            "TypeError",
            "float arithmetic requires a float receiver",
        );
    };
    let value = float_result_bits(py, value);
    if exception_pending(py) {
        return MoltObject::none().bits();
    }
    let result = operation(value);
    molt_cpython_abi::api::errors::with_preserved_error(|| dec_ref_bits(py, value));
    result
}
pub(crate) extern "C" fn float_neg_slot(value: u64) -> u64 {
    crate::with_gil_entry_nopanic!(py, { float_unary_slot(py, value, crate::molt_neg) })
}
pub(crate) extern "C" fn float_pos_slot(value: u64) -> u64 {
    crate::with_gil_entry_nopanic!(py, { float_unary_slot(py, value, crate::molt_pos) })
}
pub(crate) extern "C" fn float_abs_slot(value: u64) -> u64 {
    crate::with_gil_entry_nopanic!(py, { float_unary_slot(py, value, crate::molt_abs_builtin) })
}
extern "C" fn exact_float_int(value: u64) -> u64 {
    crate::molt_int_from_obj(
        value,
        MoltObject::none().bits(),
        MoltObject::from_bool(false).bits(),
    )
}
pub(crate) extern "C" fn float_int_slot(value: u64) -> u64 {
    crate::with_gil_entry_nopanic!(py, { float_unary_slot(py, value, exact_float_int) })
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
    let other = match float_operand(py, other) {
        Ok(Some(value)) => float_result_bits(py, value),
        Ok(None) => return not_implemented_bits(py),
        Err(()) => return MoltObject::none().bits(),
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

/// Read only sealed integer/float storage; inherited conversion overrides are
/// not part of a builtin arithmetic slot. The bigint conversion borrows its
/// existing carrier instead of allocating a second integer.
fn float_operand(py: &PyToken<'_>, value: u64) -> Result<Option<f64>, ()> {
    if let Some(value) = as_float_extended(obj_from_bits(value)) {
        return Ok(Some(value));
    }
    let Some(value) = integer_carrier(value) else {
        return Ok(None);
    };
    if let Some(value) = obj_from_bits(value).as_int() {
        return Ok(Some(value as f64));
    }
    let Some(pointer) = bigint_ptr_from_bits(value) else {
        unreachable!("integer carrier");
    };
    crate::builtins::numbers::integer_as_double(py, unsafe { bigint_ref(pointer) })
        .map(Some)
        .ok_or(())
}

fn float_power_slot(py: &PyToken<'_>, a: u64, b: u64, modulus: u64) -> u64 {
    // Unlike complex_pow, CPython float_pow rejects a modulus before operands.
    if !obj_from_bits(modulus).is_none() {
        return raise_exception(
            py,
            "TypeError",
            "pow() 3rd argument not allowed unless all arguments are integers",
        );
    }
    let left = match float_operand(py, a) {
        Ok(Some(value)) => value,
        Ok(None) => return not_implemented_bits(py),
        Err(()) => return MoltObject::none().bits(),
    };
    let right = match float_operand(py, b) {
        Ok(Some(value)) => value,
        Ok(None) => return not_implemented_bits(py),
        Err(()) => return MoltObject::none().bits(),
    };
    let left = float_result_bits(py, left);
    if exception_pending(py) {
        return MoltObject::none().bits();
    }
    let right = float_result_bits(py, right);
    let result = if exception_pending(py) {
        MoltObject::none().bits()
    } else {
        pow_impl(py, left, right, "**")
    };
    molt_cpython_abi::api::errors::with_preserved_error(|| {
        dec_ref_bits(py, left);
        dec_ref_bits(py, right);
    });
    result
}
pub(crate) extern "C" fn float_pow_slot(a: u64, b: u64, modulus: u64) -> u64 {
    crate::with_gil_entry_nopanic!(py, { float_power_slot(py, a, b, modulus) })
}
pub(crate) extern "C" fn float_rpow_slot(a: u64, b: u64, modulus: u64) -> u64 {
    crate::with_gil_entry_nopanic!(py, { float_power_slot(py, b, a, modulus) })
}
