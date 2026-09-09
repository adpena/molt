use molt_obj_model::MoltObject;
use num_traits::{Signed, ToPrimitive};

use crate::builtins::exceptions::{exception_matches_builtin_name, molt_exception_last_pending};
use crate::builtins::numbers::index_bigint_from_obj;
use crate::{
    call_callable0, dec_ref_bits, exception_pending, exception_stack_pop, exception_stack_push,
    molt_concat, molt_contains, molt_delitem_method, molt_getitem_method, molt_inplace_add,
    molt_inplace_bit_and, molt_inplace_bit_or, molt_inplace_bit_xor, molt_inplace_concat,
    molt_inplace_div, molt_inplace_floordiv, molt_inplace_lshift, molt_inplace_matmul,
    molt_inplace_mod, molt_inplace_mul, molt_inplace_pow, molt_inplace_rshift, molt_inplace_sub,
    molt_len, molt_setitem_method, obj_from_bits, raise_exception, type_name,
};

#[unsafe(no_mangle)]
pub extern "C" fn molt_operator_contains(container_bits: u64, item_bits: u64) -> u64 {
    molt_contains(container_bits, item_bits)
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_operator_getitem(obj_bits: u64, key_bits: u64) -> u64 {
    molt_getitem_method(obj_bits, key_bits)
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_operator_setitem(obj_bits: u64, key_bits: u64, val_bits: u64) -> u64 {
    molt_setitem_method(obj_bits, key_bits, val_bits)
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_operator_delitem(obj_bits: u64, key_bits: u64) -> u64 {
    molt_delitem_method(obj_bits, key_bits)
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_operator_countof(container_bits: u64, value_bits: u64) -> u64 {
    crate::with_gil_entry_nopanic!(_py, {
        let Some(mut iter) = crate::object::iterable::OwnedIterator::new(_py, container_bits)
        else {
            return MoltObject::none().bits();
        };
        let mut count = 0i64;
        loop {
            let item = match iter.next() {
                Ok(Some(item)) => item,
                Ok(None) => return crate::int_bits_from_i64(_py, count),
                Err(()) => return MoltObject::none().bits(),
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

#[unsafe(no_mangle)]
pub extern "C" fn molt_operator_length_hint(obj_bits: u64, default_bits: u64) -> u64 {
    crate::with_gil_entry_nopanic!(_py, {
        let error = format!(
            "'{}' object cannot be interpreted as an integer",
            type_name(_py, obj_from_bits(default_bits))
        );
        let Some(default) = index_bigint_from_obj(_py, default_bits, &error) else {
            return MoltObject::none().bits();
        };
        let Some(default) = default.to_isize() else {
            return raise_exception::<_>(
                _py,
                "OverflowError",
                "Python int too large to convert to C ssize_t",
            );
        };
        exception_stack_push();
        let length = molt_len(obj_bits);
        if !exception_pending(_py) {
            exception_stack_pop(_py);
            return length;
        }
        dec_ref_bits(_py, length);
        let error = molt_exception_last_pending();
        let type_error = exception_matches_builtin_name(_py, error, "TypeError");
        if type_error {
            crate::molt_exception_clear();
        }
        dec_ref_bits(_py, error);
        exception_stack_pop(_py);
        if !type_error {
            return MoltObject::none().bits();
        }

        let method = unsafe {
            crate::builtins::attr::lookup_special_method(_py, obj_bits, b"__length_hint__")
        };
        if exception_pending(_py) {
            return MoltObject::none().bits();
        }
        let Some(method) = method else {
            return crate::int_bits_from_i64(_py, default as i64);
        };
        exception_stack_push();
        let result = unsafe { call_callable0(_py, method) };
        dec_ref_bits(_py, method);
        if exception_pending(_py) {
            dec_ref_bits(_py, result);
            let error = molt_exception_last_pending();
            let type_error = exception_matches_builtin_name(_py, error, "TypeError");
            if type_error {
                crate::molt_exception_clear();
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
        if crate::is_not_implemented_bits(_py, result) {
            dec_ref_bits(_py, result);
            return crate::int_bits_from_i64(_py, default as i64);
        }
        let value = crate::builtins::numbers::index_bigint_integral_bits(result);
        let Some(value) = value else {
            let name = type_name(_py, obj_from_bits(result)).into_owned();
            dec_ref_bits(_py, result);
            return raise_exception::<_>(
                _py,
                "TypeError",
                &format!("__length_hint__ must be an integer, not {name}"),
            );
        };
        dec_ref_bits(_py, result);
        if value.is_negative() {
            return raise_exception::<_>(_py, "ValueError", "__length_hint__() should return >= 0");
        }
        let Some(value) = value.to_isize() else {
            return raise_exception::<_>(
                _py,
                "OverflowError",
                "cannot fit 'int' into an index-sized integer",
            );
        };
        crate::int_bits_from_i64(_py, value as i64)
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_operator_concat(a: u64, b: u64) -> u64 {
    molt_concat(a, b)
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_operator_iconcat(a: u64, b: u64) -> u64 {
    molt_inplace_concat(a, b)
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_operator_iadd(a: u64, b: u64) -> u64 {
    molt_inplace_add(a, b)
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_operator_isub(a: u64, b: u64) -> u64 {
    molt_inplace_sub(a, b)
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_operator_imul(a: u64, b: u64) -> u64 {
    molt_inplace_mul(a, b)
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_operator_imatmul(a: u64, b: u64) -> u64 {
    molt_inplace_matmul(a, b)
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_operator_itruediv(a: u64, b: u64) -> u64 {
    molt_inplace_div(a, b)
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_operator_ifloordiv(a: u64, b: u64) -> u64 {
    molt_inplace_floordiv(a, b)
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_operator_imod(a: u64, b: u64) -> u64 {
    molt_inplace_mod(a, b)
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_operator_ipow(a: u64, b: u64) -> u64 {
    molt_inplace_pow(a, b)
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_operator_ilshift(a: u64, b: u64) -> u64 {
    molt_inplace_lshift(a, b)
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_operator_irshift(a: u64, b: u64) -> u64 {
    molt_inplace_rshift(a, b)
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_operator_iand(a: u64, b: u64) -> u64 {
    molt_inplace_bit_and(a, b)
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_operator_ior(a: u64, b: u64) -> u64 {
    molt_inplace_bit_or(a, b)
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_operator_ixor(a: u64, b: u64) -> u64 {
    molt_inplace_bit_xor(a, b)
}
