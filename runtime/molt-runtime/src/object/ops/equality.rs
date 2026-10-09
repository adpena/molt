use super::*;

#[cfg(test)]
mod tests;

pub(in crate::object) unsafe fn eq_bool_from_bits(
    _py: &PyToken<'_>,
    lhs_bits: u64,
    rhs_bits: u64,
) -> Option<bool> {
    match crate::object::ops_compare::compare_object_eq_bool(
        _py,
        obj_from_bits(lhs_bits),
        obj_from_bits(rhs_bits),
    ) {
        crate::object::ops_compare::CompareBoolOutcome::True => Some(true),
        crate::object::ops_compare::CompareBoolOutcome::False => Some(false),
        _ => None,
    }
}

pub(in crate::object) enum BinaryDunderOutcome {
    Value(u64),
    NotImplemented,
    Missing,
    Error,
}

unsafe fn call_dunder_raw(
    _py: &PyToken<'_>,
    raw_bits: u64,
    owner_ptr: *mut u8,
    instance_bits: u64,
    arg_bits: u64,
    modulus: Option<u64>,
) -> BinaryDunderOutcome {
    unsafe {
        let result = match modulus {
            Some(modulus) => crate::builtins::attr::descriptor_call2(
                _py,
                raw_bits,
                owner_ptr,
                Some(instance_bits),
                arg_bits,
                modulus,
            ),
            None => crate::builtins::attr::descriptor_special_call1(
                _py,
                raw_bits,
                owner_ptr,
                Some(instance_bits),
                arg_bits,
                crate::builtins::attr::DescriptorCallPolicy::Optional,
            ),
        };
        let Some(res_bits) = result else {
            if exception_pending(_py) {
                return BinaryDunderOutcome::Error;
            }
            return BinaryDunderOutcome::Missing;
        };
        if exception_pending(_py) {
            molt_cpython_abi::api::errors::with_preserved_error(|| dec_ref_bits(_py, res_bits));
            return BinaryDunderOutcome::Error;
        }
        if is_not_implemented_bits(_py, res_bits) {
            dec_ref_bits(_py, res_bits);
            return BinaryDunderOutcome::NotImplemented;
        }
        BinaryDunderOutcome::Value(res_bits)
    }
}

/// Resolve and invoke one binary special method against the receiver's current
/// type. The owner pin and raw descriptor lookup live only for this attempt;
/// neither can become stale across an earlier user callback.
#[derive(Clone, Copy)]
enum DunderPhase {
    Numeric,
    Sequence,
}

unsafe fn call_current_dunder(
    py: &PyToken<'_>,
    receiver: u64,
    argument: u64,
    name: u64,
    modulus: Option<u64>,
    phase: DunderPhase,
) -> BinaryDunderOutcome {
    unsafe {
        let owner = type_of_bits(py, receiver);
        let Some(owner_ptr) = obj_from_bits(owner).as_ptr() else {
            return if exception_pending(py) {
                BinaryDunderOutcome::Error
            } else {
                BinaryDunderOutcome::Missing
            };
        };
        inc_ref_bits(py, owner);
        let outcome = match class_attr_lookup_raw_mro(py, owner_ptr, name) {
            Some(raw) => {
                let sequence = crate::object::ops_arith::native_slots::is_sequence_slot(Some(raw));
                if matches!(phase, DunderPhase::Numeric) && sequence
                    || matches!(phase, DunderPhase::Sequence) && !sequence
                {
                    BinaryDunderOutcome::Missing
                } else {
                    call_dunder_raw(py, raw, owner_ptr, receiver, argument, modulus)
                }
            }
            None if exception_pending(py) => BinaryDunderOutcome::Error,
            None => BinaryDunderOutcome::Missing,
        };
        molt_cpython_abi::api::errors::with_preserved_error(|| dec_ref_bits(py, owner));
        outcome
    }
}

pub(in crate::object) unsafe fn call_sequence_dunder(
    py: &PyToken<'_>,
    receiver: u64,
    argument: u64,
    name: u64,
) -> Option<u64> {
    unsafe {
        dunder_result(call_current_dunder(
            py,
            receiver,
            argument,
            name,
            None,
            DunderPhase::Sequence,
        ))
    }
}

fn dunder_result(outcome: BinaryDunderOutcome) -> Option<u64> {
    match outcome {
        BinaryDunderOutcome::Value(bits) => Some(bits),
        BinaryDunderOutcome::Error => Some(MoltObject::none().bits()),
        BinaryDunderOutcome::Missing | BinaryDunderOutcome::NotImplemented => None,
    }
}

/// Numeric-only dispatch shared by normal, in-place fallback and ternary power.
pub(in crate::object) unsafe fn call_numeric_dunder(
    _py: &PyToken<'_>,
    lhs_bits: u64,
    rhs_bits: u64,
    op_name_bits: u64,
    rop_name_bits: u64,
    modulus: Option<u64>,
) -> Option<u64> {
    unsafe {
        call_numeric_dunder_sides(
            _py,
            lhs_bits,
            rhs_bits,
            op_name_bits,
            rop_name_bits,
            modulus,
            [true, true],
        )
    }
}

pub(in crate::object) unsafe fn call_numeric_dunder_sides(
    _py: &PyToken<'_>,
    lhs_bits: u64,
    rhs_bits: u64,
    op_name_bits: u64,
    rop_name_bits: u64,
    modulus: Option<u64>,
    [left_dispatches, right_dispatches]: [bool; 2],
) -> Option<u64> {
    unsafe {
        // Snapshot identities only to choose CPython's reflected-subclass
        // ordering. Every invocation below performs a fresh lookup.
        let (different_types, prefer_rhs) = {
            let lhs_type_bits = type_of_bits(_py, lhs_bits);
            let rhs_type_bits = type_of_bits(_py, rhs_bits);
            let different_types = rhs_type_bits != lhs_type_bits;
            let rhs_is_subclass = different_types && issubclass_bits(rhs_type_bits, lhs_type_bits);
            let prefer_rhs = if !rhs_is_subclass {
                false
            } else {
                // Only an overridden reflected method gets subtype priority;
                // compare __rop__ on both types, not lhs.__op__ to rhs.__rop__.
                let lhs_rop_raw = obj_from_bits(lhs_type_bits)
                    .as_ptr()
                    .and_then(|ptr| class_attr_lookup_raw_mro(_py, ptr, rop_name_bits));
                if exception_pending(_py) {
                    return Some(MoltObject::none().bits());
                }
                let rhs_rop_raw = obj_from_bits(rhs_type_bits)
                    .as_ptr()
                    .and_then(|ptr| class_attr_lookup_raw_mro(_py, ptr, rop_name_bits));
                if exception_pending(_py) {
                    return Some(MoltObject::none().bits());
                }
                rhs_rop_raw.is_some()
                    && lhs_rop_raw.is_none_or(|lhs_raw| lhs_raw != rhs_rop_raw.unwrap())
            };
            (different_types, prefer_rhs)
        };

        let mut tried_rhs = false;
        if left_dispatches && right_dispatches && prefer_rhs {
            tried_rhs = true;
            match call_current_dunder(
                _py,
                rhs_bits,
                lhs_bits,
                rop_name_bits,
                modulus,
                DunderPhase::Numeric,
            ) {
                BinaryDunderOutcome::Value(bits) => return Some(bits),
                BinaryDunderOutcome::Error => return Some(MoltObject::none().bits()),
                BinaryDunderOutcome::NotImplemented | BinaryDunderOutcome::Missing => {}
            }
        }

        if left_dispatches {
            match call_current_dunder(
                _py,
                lhs_bits,
                rhs_bits,
                op_name_bits,
                modulus,
                DunderPhase::Numeric,
            ) {
                BinaryDunderOutcome::Value(bits) => return Some(bits),
                BinaryDunderOutcome::Error => return Some(MoltObject::none().bits()),
                BinaryDunderOutcome::NotImplemented | BinaryDunderOutcome::Missing => {}
            }
        }

        if right_dispatches && different_types && !tried_rhs {
            match call_current_dunder(
                _py,
                rhs_bits,
                lhs_bits,
                rop_name_bits,
                modulus,
                DunderPhase::Numeric,
            ) {
                BinaryDunderOutcome::Value(bits) => return Some(bits),
                BinaryDunderOutcome::Error => return Some(MoltObject::none().bits()),
                BinaryDunderOutcome::NotImplemented | BinaryDunderOutcome::Missing => {}
            }
        }
        None
    }
}

pub(in crate::object) unsafe fn call_inplace_dunder(
    _py: &PyToken<'_>,
    lhs_bits: u64,
    rhs_bits: u64,
    op_name_bits: u64,
) -> Option<u64> {
    unsafe {
        match call_current_dunder(
            _py,
            lhs_bits,
            rhs_bits,
            op_name_bits,
            None,
            DunderPhase::Numeric,
        ) {
            BinaryDunderOutcome::Value(bits) => Some(bits),
            BinaryDunderOutcome::Error => Some(MoltObject::none().bits()),
            BinaryDunderOutcome::NotImplemented | BinaryDunderOutcome::Missing => None,
        }
    }
}

/// Source operators complete numeric dispatch before inherited sequence slots.
pub(in crate::object) unsafe fn call_binary_dunder(
    py: &PyToken<'_>,
    left: u64,
    right: u64,
    op: u64,
    reflected: u64,
) -> Option<u64> {
    unsafe {
        call_numeric_dunder(py, left, right, op, reflected, None)
            .or_else(|| call_sequence_dunder(py, left, right, op))
            .or_else(|| call_sequence_dunder(py, right, left, reflected))
    }
}
