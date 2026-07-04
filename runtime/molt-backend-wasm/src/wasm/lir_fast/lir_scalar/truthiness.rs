mod bool_ops;
mod predicate;
mod select;

pub(in crate::wasm::lir_fast) use bool_ops::{
    emit_lir_bool, emit_lir_not, emit_lir_truthy_cond_builtin,
};
pub(in crate::wasm::lir_fast) use predicate::emit_lir_truthiness_i32;
pub(in crate::wasm::lir_fast) use select::emit_lir_bool_select;
