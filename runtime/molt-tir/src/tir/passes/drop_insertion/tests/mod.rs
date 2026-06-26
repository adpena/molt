pub(super) use std::collections::HashSet;

pub(super) use super::{DROP_INSERTED_ATTR, EXCEPTION_REGION_DROPS_INSERTED_ATTR, run};
pub(super) use crate::tir::analysis::AnalysisManager;
pub(super) use crate::tir::blocks::{BlockId, LoopRole, Terminator, TirBlock};
pub(super) use crate::tir::function::TirFunction;
pub(super) use crate::tir::ops::{AttrDict, AttrValue, Dialect, OpCode, TirOp};
pub(super) use crate::tir::passes::liveness::TirLiveness;
pub(super) use crate::tir::passes::ownership_lattice_min::original_kind;
pub(super) use crate::tir::types::TirType;
pub(super) use crate::tir::values::{TirValue, ValueId};

pub(super) fn op(opcode: OpCode, operands: Vec<ValueId>, results: Vec<ValueId>) -> TirOp {
    TirOp {
        dialect: Dialect::Molt,
        opcode,
        operands,
        results,
        attrs: AttrDict::new(),
        source_span: None,
    }
}

pub(super) fn const_str(result: ValueId) -> TirOp {
    let mut attrs = AttrDict::new();
    attrs.insert("s_value".into(), AttrValue::Str("x".into()));
    TirOp {
        dialect: Dialect::Molt,
        opcode: OpCode::ConstStr,
        operands: vec![],
        results: vec![result],
        attrs,
        source_span: None,
    }
}

pub(super) fn finalizer_object(result: ValueId) -> TirOp {
    let mut attrs = AttrDict::new();
    attrs.insert("defines_del".into(), AttrValue::Bool(true));
    TirOp {
        dialect: Dialect::Molt,
        opcode: OpCode::ObjectNewBound,
        operands: vec![],
        results: vec![result],
        attrs,
        source_span: None,
    }
}

pub(super) fn finalizer_call_bind(result: ValueId) -> TirOp {
    let mut attrs = AttrDict::new();
    attrs.insert("_original_kind".into(), AttrValue::Str("call_bind".into()));
    attrs.insert("defines_del".into(), AttrValue::Bool(true));
    TirOp {
        dialect: Dialect::Molt,
        opcode: OpCode::Call,
        operands: vec![],
        results: vec![result],
        attrs,
        source_span: None,
    }
}

pub(super) fn count_decrefs(func: &TirFunction) -> usize {
    func.blocks
        .values()
        .flat_map(|b| b.ops.iter())
        .filter(|o| o.opcode == OpCode::DecRef)
        .count()
}
pub(super) fn count_increfs(func: &TirFunction) -> usize {
    func.blocks
        .values()
        .flat_map(|b| b.ops.iter())
        .filter(|o| o.opcode == OpCode::IncRef)
        .count()
}

pub(super) fn original_copy(kind: &str, results: Vec<ValueId>) -> TirOp {
    let mut copy = op(OpCode::Copy, vec![], results);
    copy.attrs
        .insert("_original_kind".into(), AttrValue::Str(kind.into()));
    copy
}

pub(super) fn original_copy_with_operands(
    kind: &str,
    operands: Vec<ValueId>,
    results: Vec<ValueId>,
) -> TirOp {
    let mut copy = op(OpCode::Copy, operands, results);
    copy.attrs
        .insert("_original_kind".into(), AttrValue::Str(kind.into()));
    copy
}

pub(super) fn original_store_var(var: &str, operand: ValueId, result: ValueId) -> TirOp {
    let mut copy = original_copy_with_operands("store_var", vec![operand], vec![result]);
    copy.attrs.insert("_var".into(), AttrValue::Str(var.into()));
    copy
}

pub(super) fn try_start(label: i64) -> TirOp {
    let mut start = op(OpCode::TryStart, vec![], vec![]);
    start.attrs.insert("value".into(), AttrValue::Int(label));
    start
}

mod core_rc;
mod exception_regions;
mod python_lifetimes;
