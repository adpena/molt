use std::collections::HashSet;

use crate::tir::blocks::Terminator;
use crate::tir::function::TirFunction;
use crate::tir::op_kinds_generated::{
    SroaConstImmediateRule, opcode_sroa_const_immediate_rule_table,
};
use crate::tir::ops::{AttrValue, OpCode, TirOp};
use crate::tir::passes::alias_analysis::AliasAnalysisResult;
use crate::tir::passes::value_range::ValueRangeResult;
use crate::tir::types::TirType;
use crate::tir::values::ValueId;

/// `Some((obj_operand, offset))` for the narrow removable typed-slot store
/// (`store` / `store_init`) contract: operands `[obj, value]`, offset on the
/// `value` attr.
pub(super) fn removable_store_obj_offset(op: &TirOp) -> Option<(ValueId, i64)> {
    if op.opcode != OpCode::StoreAttr || op.operands.len() != 2 {
        return None;
    }
    let kind = match op.attrs.get("_original_kind") {
        Some(AttrValue::Str(s)) => s.as_str(),
        _ => return None,
    };
    if !matches!(kind, "store" | "store_init") {
        return None;
    }
    match op.attrs.get("value") {
        Some(AttrValue::Int(offset)) => Some((op.operands[0], *offset)),
        _ => None,
    }
}

/// The allocation-site result `ValueId` of an `ObjectNewBoundStack`, or `None`.
pub(super) fn stack_alloc_result(op: &TirOp) -> Option<ValueId> {
    if op.opcode != OpCode::ObjectNewBoundStack || op.results.len() != 1 {
        return None;
    }
    Some(op.results[0])
}

pub(super) fn store_value_is_refcount_neutral(
    value: ValueId,
    func: &TirFunction,
    const_immediates: &HashSet<ValueId>,
    ranges: &ValueRangeResult,
) -> bool {
    if const_immediates.contains(&value) {
        return true;
    }
    if let Some(TirType::None | TirType::Bool | TirType::F64) = func.value_types.get(&value) {
        return true;
    }
    ranges.fits_inline_int47(value)
}

pub(super) fn collect_const_immediates(
    func: &TirFunction,
    ranges: &ValueRangeResult,
) -> HashSet<ValueId> {
    let mut set = HashSet::new();
    for block in func.blocks.values() {
        for op in &block.ops {
            let Some(&result) = op.results.first() else {
                continue;
            };
            match opcode_sroa_const_immediate_rule_table(op.opcode) {
                SroaConstImmediateRule::AlwaysImmediate => {
                    set.insert(result);
                }
                SroaConstImmediateRule::InlineIntIfRange if ranges.fits_inline_int47(result) => {
                    set.insert(result);
                }
                _ => {}
            }
        }
    }
    set
}

pub(super) fn terminator_references(
    term: &Terminator,
    root: ValueId,
    alias: &AliasAnalysisResult,
) -> bool {
    let hits = |v: &ValueId| alias.root(*v) == root;
    match term {
        Terminator::Branch { args, .. } => args.iter().any(hits),
        Terminator::CondBranch {
            cond,
            then_args,
            else_args,
            ..
        } => hits(cond) || then_args.iter().any(hits) || else_args.iter().any(hits),
        Terminator::Switch {
            value,
            cases,
            default_args,
            ..
        } => {
            hits(value)
                || cases.iter().any(|(_, _, args)| args.iter().any(hits))
                || default_args.iter().any(hits)
        }
        Terminator::StateDispatch {
            cases,
            default_args,
            ..
        } => {
            cases.iter().any(|(_, _, args)| args.iter().any(hits)) || default_args.iter().any(hits)
        }
        Terminator::Return { values } => values.iter().any(hits),
        Terminator::Unreachable => false,
    }
}
