pub(super) use std::collections::{HashMap, HashSet};

pub(super) use super::cleanup::{
    eliminate_dead_labels, validate_labels, validate_structured_if_markers,
};
pub(super) use super::runner::lower_to_simple_ir;
pub(super) use super::structured::emit_guard_raise_path;
pub(super) use crate::ir::{FunctionIR, OpIR};
pub(super) use crate::tir::blocks::{BlockId, LoopBreakKind, LoopRole, Terminator, TirBlock};
pub(super) use crate::tir::function::TirFunction;
pub(super) use crate::tir::lower_from_simple::lower_to_tir;
pub(super) use crate::tir::ops::{AttrDict, AttrValue, Dialect, OpCode, TirOp};
pub(super) use crate::tir::simple_value_names::{SimpleValueNames, value_var};
pub(super) use crate::tir::types::TirType;
pub(super) use crate::tir::values::{TirValue, ValueId};

pub(super) fn add_function() -> TirFunction {
    let mut func = TirFunction::new("add".into(), vec![TirType::I64, TirType::I64], TirType::I64);

    let result = ValueId(func.next_value);
    func.next_value += 1;

    let entry = func.blocks.get_mut(&func.entry_block).unwrap();
    entry.ops.push(TirOp {
        dialect: Dialect::Molt,
        opcode: OpCode::Add,
        operands: vec![ValueId(0), ValueId(1)],
        results: vec![result],
        attrs: AttrDict::new(),
        source_span: None,
    });
    entry.terminator = Terminator::Return {
        values: vec![result],
    };

    func
}

mod basic_roundtrip;
mod bool_ops;
mod loop_guards;
mod naming_metadata;
mod object_calls;
mod structured_control;
