mod binary;
mod checked;
mod unary;

pub(in crate::wasm::lir_fast) use binary::emit_lir_binary_arith;
pub(in crate::wasm::lir_fast) use checked::{emit_lir_checked_add, emit_lir_checked_mul};
pub(in crate::wasm::lir_fast) use unary::{emit_lir_unary_arith, emit_lir_unary_pos};
