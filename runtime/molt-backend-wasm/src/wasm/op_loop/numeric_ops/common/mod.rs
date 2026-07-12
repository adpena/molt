mod boxed;
mod float_fast;
mod int_fast;
mod operands;

pub(super) use boxed::{
    emit_boxed_binary_call, emit_boxed_binary_result, emit_boxed_ternary_result,
    emit_boxed_unary_result, store_numeric_result,
};
pub(super) use float_fast::{
    emit_plain_f64_arithmetic_result, emit_plain_f64_binary_result,
    emit_plain_f64_binary_result_or_boxed,
};
pub(super) use int_fast::{
    emit_guarded_int_binary_result_or_boxed, emit_inline_int_result,
    emit_inline_int_result_or_boxed, emit_trusted_int_binary_operand_tees,
};
pub(super) use operands::{BinaryOperands, IntBinaryTemps, binary_operands, int_binary_temps};
