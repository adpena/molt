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
) -> BinaryDunderOutcome {
    unsafe {
        let Some(res_bits) = crate::builtins::attr::descriptor_special_call1(
            _py,
            raw_bits,
            owner_ptr,
            Some(instance_bits),
            arg_bits,
            crate::builtins::attr::DescriptorCallPolicy::Optional,
        ) else {
            if exception_pending(_py) {
                return BinaryDunderOutcome::Error;
            }
            return BinaryDunderOutcome::Missing;
        };
        if exception_pending(_py) {
            dec_ref_bits(_py, res_bits);
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
pub(in crate::object) unsafe fn call_current_binary_dunder(
    _py: &PyToken<'_>,
    receiver_bits: u64,
    arg_bits: u64,
    name_bits: u64,
) -> BinaryDunderOutcome {
    unsafe {
        let owner_bits = type_of_bits(_py, receiver_bits);
        let Some(owner_ptr) = obj_from_bits(owner_bits).as_ptr() else {
            return if exception_pending(_py) {
                BinaryDunderOutcome::Error
            } else {
                BinaryDunderOutcome::Missing
            };
        };

        // A descriptor hook may replace the receiver's __class__. Keep the
        // owner passed to descriptor_call1 alive through that immediate call.
        inc_ref_bits(_py, owner_bits);
        let outcome = match class_attr_lookup_raw_mro(_py, owner_ptr, name_bits) {
            Some(raw_bits) => call_dunder_raw(_py, raw_bits, owner_ptr, receiver_bits, arg_bits),
            None if exception_pending(_py) => BinaryDunderOutcome::Error,
            None => BinaryDunderOutcome::Missing,
        };
        dec_ref_bits(_py, owner_bits);
        outcome
    }
}

pub(in crate::object) unsafe fn call_binary_dunder(
    _py: &PyToken<'_>,
    lhs_bits: u64,
    rhs_bits: u64,
    op_name_bits: u64,
    rop_name_bits: u64,
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

        // Builtin sequence concat/repeat remains a sequence fallback when
        // inherited by a heap subtype. Numeric reflected methods run first.
        let lhs_sequence_slot = obj_from_bits(type_of_bits(_py, lhs_bits))
            .as_ptr()
            .map(|ptr| class_attr_lookup_raw_mro(_py, ptr, op_name_bits))
            .is_some_and(|raw| crate::object::ops_arith::native_slots::is_sequence_slot(raw));
        let rhs_sequence_slot = obj_from_bits(type_of_bits(_py, rhs_bits))
            .as_ptr()
            .map(|ptr| class_attr_lookup_raw_mro(_py, ptr, rop_name_bits))
            .is_some_and(|raw| crate::object::ops_arith::native_slots::is_sequence_slot(raw));
        if exception_pending(_py) {
            return Some(MoltObject::none().bits());
        }
        let mut tried_rhs = false;
        if prefer_rhs || (different_types && lhs_sequence_slot && !rhs_sequence_slot) {
            tried_rhs = true;
            match call_current_binary_dunder(_py, rhs_bits, lhs_bits, rop_name_bits) {
                BinaryDunderOutcome::Value(bits) => return Some(bits),
                BinaryDunderOutcome::Error => return Some(MoltObject::none().bits()),
                BinaryDunderOutcome::NotImplemented | BinaryDunderOutcome::Missing => {}
            }
        }

        match call_current_binary_dunder(_py, lhs_bits, rhs_bits, op_name_bits) {
            BinaryDunderOutcome::Value(bits) => return Some(bits),
            BinaryDunderOutcome::Error => return Some(MoltObject::none().bits()),
            BinaryDunderOutcome::NotImplemented | BinaryDunderOutcome::Missing => {}
        }

        if different_types && !tried_rhs {
            match call_current_binary_dunder(_py, rhs_bits, lhs_bits, rop_name_bits) {
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
        match call_current_binary_dunder(_py, lhs_bits, rhs_bits, op_name_bits) {
            BinaryDunderOutcome::Value(bits) => Some(bits),
            BinaryDunderOutcome::Error => Some(MoltObject::none().bits()),
            BinaryDunderOutcome::NotImplemented | BinaryDunderOutcome::Missing => None,
        }
    }
}
