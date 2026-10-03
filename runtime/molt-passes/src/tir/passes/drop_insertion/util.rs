use std::collections::HashSet;

use crate::tir::function::TirFunction;
use crate::tir::op_kinds_generated::opcode_is_drop_insertion_return_deferral_barrier_table;
use crate::tir::ops::{AttrDict, AttrValue, Dialect, OpCode, TirOp};
use crate::tir::values::ValueId;

#[inline]
pub(crate) fn attr_is_true(func: &TirFunction, name: &str) -> bool {
    matches!(func.attrs.get(name), Some(AttrValue::Bool(true)))
}

pub(super) fn make_op(opcode: OpCode, operands: Vec<ValueId>) -> TirOp {
    TirOp {
        dialect: Dialect::Molt,
        opcode,
        operands,
        results: vec![],
        attrs: AttrDict::new(),
        source_span: None,
    }
}

pub(super) fn sorted_values(values: &[ValueId]) -> Vec<ValueId> {
    let mut ordered = values.to_vec();
    ordered.sort_unstable_by_key(|value| value.0);
    ordered
}

pub(super) fn sorted_unique_values(values: &[ValueId]) -> Vec<ValueId> {
    let mut ordered = sorted_values(values);
    ordered.dedup();
    ordered
}

pub(super) fn ordered_unique_after_op_values<F>(
    values: &[ValueId],
    op: &TirOp,
    canon: &F,
) -> Vec<ValueId>
where
    F: Fn(ValueId) -> ValueId,
{
    let mut remaining: HashSet<ValueId> = values.iter().copied().collect();
    let mut ordered = Vec::with_capacity(remaining.len());
    for &operand in &op.operands {
        let root = canon(operand);
        if remaining.remove(&root) {
            ordered.push(root);
        }
    }
    for &result in &op.results {
        let root = canon(result);
        if remaining.remove(&root) {
            ordered.push(root);
        }
    }
    let mut rest: Vec<ValueId> = remaining.into_iter().collect();
    rest.sort_unstable_by_key(|value| value.0);
    ordered.extend(rest);
    ordered
}

/// True if an opcode's operands are explicit ownership rails that block
/// return-boundary deferral for touched finalizer-sensitive roots.
pub(super) fn is_return_deferral_barrier(opcode: OpCode) -> bool {
    opcode_is_drop_insertion_return_deferral_barrier_table(opcode)
}
