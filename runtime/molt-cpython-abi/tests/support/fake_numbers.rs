use molt_cpython_abi::abi_types;
use molt_cpython_abi::hooks::{NumberUnaryOp, OwnedHandleResult};
use molt_lang_obj_model::MoltObject;
use std::os::raw::c_int;

pub fn real(bits: u64) -> Option<f64> {
    let value = MoltObject::from_bits(bits);
    value
        .as_float()
        .or_else(|| integer(bits).map(|integer| integer as f64))
}

fn integer(bits: u64) -> Option<i64> {
    let value = MoltObject::from_bits(bits);
    value.as_int().or_else(|| value.as_bool().map(i64::from))
}

pub unsafe extern "C" fn unary(operation: u32, bits: u64) -> OwnedHandleResult {
    if (operation == NumberUnaryOp::Float as u32
        || operation == NumberUnaryOp::FloatAsDouble as u32)
        && let Some(value) = real(bits)
    {
        return OwnedHandleResult::ok(MoltObject::from_float(value).bits());
    }
    unsafe {
        molt_cpython_abi::api::errors::PyErr_SetNone((&raw mut abi_types::PyExc_TypeError).cast())
    };
    OwnedHandleResult::error()
}

pub fn comparison_result(ordering: Option<std::cmp::Ordering>, operation: c_int) -> bool {
    use std::cmp::Ordering;
    match operation {
        0 => ordering == Some(Ordering::Less),
        1 => matches!(ordering, Some(Ordering::Less | Ordering::Equal)),
        2 => ordering == Some(Ordering::Equal),
        3 => ordering != Some(Ordering::Equal),
        4 => ordering == Some(Ordering::Greater),
        5 => matches!(ordering, Some(Ordering::Greater | Ordering::Equal)),
        _ => panic!("invalid fixture comparison operation"),
    }
}

fn complex_value(bits: u64) -> Option<(f64, f64)> {
    let mut real_part = 0.0;
    let mut imaginary_part = 0.0;
    if unsafe { super::fake_complex::parts(bits, &raw mut real_part, &raw mut imaginary_part) } == 0
    {
        Some((real_part, imaginary_part))
    } else {
        real(bits).map(|real_part| (real_part, 0.0))
    }
}

pub unsafe extern "C" fn compare_builtin(
    owner: u64,
    operation: c_int,
    left: u64,
    right: u64,
) -> OwnedHandleResult {
    let class = unsafe { molt_cpython_abi::bridge::GLOBAL_BRIDGE.handle_to_borrowed_pyobj(owner) };
    let ordering = if class == (&raw mut abi_types::PyFloat_Type).cast() {
        match (real(left), real(right)) {
            (Some(left), Some(right)) => Some(left.partial_cmp(&right)),
            _ => None,
        }
    } else if class == (&raw mut abi_types::PyLong_Type).cast()
        || class == (&raw mut abi_types::PyBool_Type).cast()
    {
        match (integer(left), integer(right)) {
            (Some(left), Some(right)) => Some(Some(left.cmp(&right))),
            _ => None,
        }
    } else if class == (&raw mut abi_types::PyComplex_Type).cast() && matches!(operation, 2 | 3) {
        if let (Some(left), Some(right)) = (complex_value(left), complex_value(right)) {
            let equal = left.0 == right.0 && left.1 == right.1;
            return OwnedHandleResult::ok(
                MoltObject::from_bool(if operation == 2 { equal } else { !equal }).bits(),
            );
        }
        None
    } else {
        None
    };
    match ordering {
        Some(ordering) => OwnedHandleResult::ok(
            MoltObject::from_bool(comparison_result(ordering, operation)).bits(),
        ),
        None => OwnedHandleResult::ok(super::fake_runtime::not_implemented()),
    }
}
