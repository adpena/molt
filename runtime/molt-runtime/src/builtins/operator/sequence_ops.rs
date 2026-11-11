use crate::BuiltinExc;
use molt_obj_model::MoltObject;
use num_traits::ToPrimitive;

use crate::builtins::exceptions::{
    exception_matches_kind, molt_exception_last_pending_attached_body,
};
use crate::{
    call_callable0, dec_ref_bits, exception_pending, exception_stack_pop, exception_stack_push,
    molt_concat_attached_body, molt_contains_attached_body, molt_delitem_method_attached_body,
    molt_getitem_method_attached_body, molt_inplace_add_attached_body,
    molt_inplace_bit_and_attached_body, molt_inplace_bit_or_attached_body,
    molt_inplace_bit_xor_attached_body, molt_inplace_concat_attached_body,
    molt_inplace_div_attached_body, molt_inplace_floordiv_attached_body,
    molt_inplace_lshift_attached_body, molt_inplace_matmul_attached_body,
    molt_inplace_mod_attached_body, molt_inplace_mul_attached_body, molt_inplace_pow_attached_body,
    molt_inplace_rshift_attached_body, molt_inplace_sub_attached_body, molt_len_attached_body,
    molt_setitem_method_attached_body, obj_from_bits, raise_exception,
};

pub(crate) fn molt_operator_contains_attached_body(
    _py: &crate::PyToken<'_>,
    container_bits: u64,
    item_bits: u64,
) -> u64 {
    molt_contains_attached_body(_py, container_bits, item_bits)
}

pub(crate) fn molt_operator_getitem_attached_body(
    _py: &crate::PyToken<'_>,
    obj_bits: u64,
    key_bits: u64,
) -> u64 {
    molt_getitem_method_attached_body(_py, obj_bits, key_bits)
}

pub(crate) fn molt_operator_setitem_attached_body(
    _py: &crate::PyToken<'_>,
    obj_bits: u64,
    key_bits: u64,
    val_bits: u64,
) -> u64 {
    molt_setitem_method_attached_body(_py, obj_bits, key_bits, val_bits)
}

pub(crate) fn molt_operator_delitem_attached_body(
    _py: &crate::PyToken<'_>,
    obj_bits: u64,
    key_bits: u64,
) -> u64 {
    molt_delitem_method_attached_body(_py, obj_bits, key_bits)
}

pub(crate) fn molt_operator_countof_attached_body(
    _py: &crate::PyToken<'_>,
    container_bits: u64,
    value_bits: u64,
) -> u64 {
    crate::with_attached_entry_nopanic!(_py, _py, {
        let Some(mut iter) = crate::object::iterable::OwnedIterator::new(_py, container_bits)
        else {
            return MoltObject::none().bits();
        };
        let mut count = 0i64;
        loop {
            let item = match iter.next() {
                Ok(Some(item)) => item,
                Ok(None) => return crate::int_bits_from_i64(_py, count),
                Err(molt_runtime_core::ErrorIndicatorSet) => return MoltObject::none().bits(),
            };
            let result = crate::object::ops_compare::compare_object_eq_bool(
                _py,
                obj_from_bits(item),
                obj_from_bits(value_bits),
            );
            dec_ref_bits(_py, item);
            match result {
                crate::object::ops_compare::CompareBoolOutcome::True => count += 1,
                crate::object::ops_compare::CompareBoolOutcome::False => {}
                _ => return MoltObject::none().bits(),
            }
        }
    })
}

// PyLong_AsSsize_t semantics after actual integer admission. The caller owns
// the value across this extraction and selects its later negative-value policy.
fn length_hint_integer_value(py: &crate::PyToken<'_>, bits: u64) -> Result<Option<isize>, ()> {
    let value = if let Some(value) = crate::builtins::numbers::index_i64_integral_bits(bits) {
        isize::try_from(value).ok()
    } else {
        match crate::builtins::numbers::integer_storage_bigint(py, bits)? {
            Some(value) => value.to_isize(),
            None => return Ok(None),
        }
    };
    value.map(Some).ok_or_else(|| {
        raise_exception::<()>(
            py,
            BuiltinExc::OverflowError,
            "Python int too large to convert to C ssize_t",
        );
    })
}

pub(crate) fn molt_operator_length_hint_attached_body(
    _py: &crate::PyToken<'_>,
    obj_bits: u64,
    default_bits: u64,
) -> u64 {
    crate::with_attached_entry_nopanic!(_py, _py, {
        let default = crate::builtins::numbers::index_from_object(_py, default_bits);
        let default_owner = unsafe {
            molt_runtime_core::OwnedRuntimeValue::from_owned_bits(_py.core_token(), default)
        };
        if exception_pending(_py) {
            return MoltObject::none().bits();
        }
        let default = match length_hint_integer_value(_py, default_owner.bits()) {
            Ok(Some(value)) => value,
            Err(()) => return MoltObject::none().bits(),
            Ok(None) => {
                return raise_exception::<_>(
                    _py,
                    BuiltinExc::SystemError,
                    "validated index has no integer storage",
                );
            }
        };
        // The default's converted owner retires before any target lookup, and
        // remains live if ssize extraction above selects an overflow error.
        drop(default_owner);
        exception_stack_push();
        let length = molt_len_attached_body(_py, obj_bits);
        if !exception_pending(_py) {
            exception_stack_pop(_py);
            return length;
        }
        dec_ref_bits(_py, length);
        let error = molt_exception_last_pending_attached_body(_py);
        let type_error = exception_matches_kind(_py, error, crate::BuiltinExc::TypeError);
        if type_error {
            crate::molt_exception_clear_attached_body(_py);
        }
        dec_ref_bits(_py, error);
        exception_stack_pop(_py);
        if !type_error {
            return MoltObject::none().bits();
        }

        let method = unsafe {
            crate::builtins::attr::lookup_special_method(
                _py,
                obj_bits,
                crate::NameId::DunderLengthHint,
            )
            .into_result()
        };
        let method = match method {
            Ok(method) => method,
            Err(()) => return MoltObject::none().bits(),
        };
        let Some(method) = method else {
            return crate::int_bits_from_i64(_py, default as i64);
        };
        let method = unsafe {
            molt_runtime_core::OwnedRuntimeValue::from_owned_bits(_py.core_token(), method)
        };
        exception_stack_push();
        let result = unsafe { call_callable0(_py, method.bits()) };
        let result = unsafe {
            molt_runtime_core::OwnedRuntimeValue::from_owned_bits(_py.core_token(), result)
        };
        drop(method);
        if exception_pending(_py) {
            drop(result);
            let error = molt_exception_last_pending_attached_body(_py);
            let type_error = exception_matches_kind(_py, error, crate::BuiltinExc::TypeError);
            if type_error {
                crate::molt_exception_clear_attached_body(_py);
            }
            dec_ref_bits(_py, error);
            exception_stack_pop(_py);
            return if type_error {
                crate::int_bits_from_i64(_py, default as i64)
            } else {
                MoltObject::none().bits()
            };
        }
        exception_stack_pop(_py);
        if crate::is_not_implemented_bits(_py, result.bits()) {
            drop(result);
            return crate::int_bits_from_i64(_py, default as i64);
        }
        // Hints require an actual integer, including native integer storage;
        // unlike __len__, an index-only object must never run __index__ here.
        let value = match length_hint_integer_value(_py, result.bits()) {
            Ok(Some(value)) => value,
            Err(()) => return MoltObject::none().bits(),
            Ok(None) => {
                let name = crate::object::ops::protocol_type_name(_py, result.bits(), 100);
                return raise_exception::<_>(
                    _py,
                    BuiltinExc::TypeError,
                    &format!("__length_hint__ must be an integer, not {name}"),
                );
            }
        };
        drop(result);
        if value < 0 {
            return raise_exception::<_>(
                _py,
                BuiltinExc::ValueError,
                "__length_hint__() should return >= 0",
            );
        }
        crate::int_bits_from_i64(_py, value as i64)
    })
}

pub(crate) fn molt_operator_concat_attached_body(_py: &crate::PyToken<'_>, a: u64, b: u64) -> u64 {
    molt_concat_attached_body(_py, a, b)
}

pub(crate) fn molt_operator_iconcat_attached_body(_py: &crate::PyToken<'_>, a: u64, b: u64) -> u64 {
    molt_inplace_concat_attached_body(_py, a, b)
}

pub(crate) fn molt_operator_iadd_attached_body(_py: &crate::PyToken<'_>, a: u64, b: u64) -> u64 {
    molt_inplace_add_attached_body(_py, a, b)
}

pub(crate) fn molt_operator_isub_attached_body(_py: &crate::PyToken<'_>, a: u64, b: u64) -> u64 {
    molt_inplace_sub_attached_body(_py, a, b)
}

pub(crate) fn molt_operator_imul_attached_body(_py: &crate::PyToken<'_>, a: u64, b: u64) -> u64 {
    molt_inplace_mul_attached_body(_py, a, b)
}

pub(crate) fn molt_operator_imatmul_attached_body(_py: &crate::PyToken<'_>, a: u64, b: u64) -> u64 {
    molt_inplace_matmul_attached_body(_py, a, b)
}

pub(crate) fn molt_operator_itruediv_attached_body(
    _py: &crate::PyToken<'_>,
    a: u64,
    b: u64,
) -> u64 {
    molt_inplace_div_attached_body(_py, a, b)
}

pub(crate) fn molt_operator_ifloordiv_attached_body(
    _py: &crate::PyToken<'_>,
    a: u64,
    b: u64,
) -> u64 {
    molt_inplace_floordiv_attached_body(_py, a, b)
}

pub(crate) fn molt_operator_imod_attached_body(_py: &crate::PyToken<'_>, a: u64, b: u64) -> u64 {
    molt_inplace_mod_attached_body(_py, a, b)
}

pub(crate) fn molt_operator_ipow_attached_body(_py: &crate::PyToken<'_>, a: u64, b: u64) -> u64 {
    molt_inplace_pow_attached_body(_py, a, b)
}

pub(crate) fn molt_operator_ilshift_attached_body(_py: &crate::PyToken<'_>, a: u64, b: u64) -> u64 {
    molt_inplace_lshift_attached_body(_py, a, b)
}

pub(crate) fn molt_operator_irshift_attached_body(_py: &crate::PyToken<'_>, a: u64, b: u64) -> u64 {
    molt_inplace_rshift_attached_body(_py, a, b)
}

pub(crate) fn molt_operator_iand_attached_body(_py: &crate::PyToken<'_>, a: u64, b: u64) -> u64 {
    molt_inplace_bit_and_attached_body(_py, a, b)
}

pub(crate) fn molt_operator_ior_attached_body(_py: &crate::PyToken<'_>, a: u64, b: u64) -> u64 {
    molt_inplace_bit_or_attached_body(_py, a, b)
}

pub(crate) fn molt_operator_ixor_attached_body(_py: &crate::PyToken<'_>, a: u64, b: u64) -> u64 {
    molt_inplace_bit_xor_attached_body(_py, a, b)
}

include!("../../attached_exports_generated/builtins/operator/sequence_ops.rs");
