//! Per-op surface and representation-rule verification (box/unbox, checked i64,
//! truthiness materialization).
//!
//! Moved move-only from the monolithic `verify_lir.rs`; no logic changes.

use std::collections::HashMap;

use super::super::blocks::BlockId;
use super::super::lir::{LirFunction, LirOp, LirRepr};
use super::super::op_kinds_generated::{LirVerifyRule, opcode_lir_verify_rule_table};
use super::super::ops::AttrValue;
use super::super::types::TirType;
use super::super::values::ValueId;
use super::terminators::verify_use_dominates;
use super::{DominatorInfo, LirVerifyError, ValueDef};

pub(super) fn verify_ops(
    func: &LirFunction,
    values: &HashMap<ValueId, ValueDef>,
    dominators: &DominatorInfo,
    errors: &mut Vec<LirVerifyError>,
) {
    for (bid, block) in &func.blocks {
        for (op_index, op) in block.ops.iter().enumerate() {
            verify_op_surface(*bid, op_index, op, errors);
            for operand in &op.tir_op.operands {
                verify_use_dominates(
                    *bid,
                    op_index,
                    *operand,
                    values,
                    dominators,
                    errors,
                    "op operand",
                );
            }
            match opcode_lir_verify_rule_table(op.tir_op.opcode) {
                LirVerifyRule::None => {}
                LirVerifyRule::BoxValue => verify_box_op(*bid, op_index, op, values, errors),
                LirVerifyRule::UnboxValue => verify_unbox_op(*bid, op_index, op, values, errors),
                LirVerifyRule::CheckedI64Arithmetic => {
                    verify_checked_i64_arithmetic(*bid, op_index, op, errors)
                }
                LirVerifyRule::TruthyMaterialization => {
                    verify_truthy_materialization(*bid, op_index, op, errors)
                }
            }
        }
    }
}

fn verify_op_surface(bid: BlockId, op_index: usize, op: &LirOp, errors: &mut Vec<LirVerifyError>) {
    if op.tir_op.results.len() != op.result_values.len() {
        errors.push(LirVerifyError {
            block: Some(bid),
            op_index: Some(op_index),
            message: format!(
                "op result arity drift: tir has {} results but lir has {}",
                op.tir_op.results.len(),
                op.result_values.len()
            ),
        });
        return;
    }

    for (slot, (tir_id, lir_value)) in op
        .tir_op
        .results
        .iter()
        .zip(op.result_values.iter())
        .enumerate()
    {
        if *tir_id != lir_value.id {
            errors.push(LirVerifyError {
                block: Some(bid),
                op_index: Some(op_index),
                message: format!(
                    "result id drift at slot {}: tir uses {} but lir uses {}",
                    slot, tir_id, lir_value.id
                ),
            });
        }
    }
}

fn verify_checked_i64_arithmetic(
    bid: BlockId,
    op_index: usize,
    op: &LirOp,
    errors: &mut Vec<LirVerifyError>,
) {
    let checked = matches!(
        op.tir_op.attrs.get("lir.checked_overflow"),
        Some(AttrValue::Bool(true))
    );
    if !checked {
        return;
    }
    if op.result_values.len() != 3 {
        errors.push(LirVerifyError {
            block: Some(bid),
            op_index: Some(op_index),
            message: format!(
                "checked i64 arithmetic requires 3 results, found {}",
                op.result_values.len()
            ),
        });
        return;
    }
    let main = &op.result_values[0];
    let overflow_box = &op.result_values[1];
    let overflow_flag = &op.result_values[2];
    if main.ty != TirType::I64 || main.repr != LirRepr::I64 {
        errors.push(LirVerifyError {
            block: Some(bid),
            op_index: Some(op_index),
            message: format!(
                "checked i64 arithmetic main result must be I64/I64, found {:?}/{:?}",
                main.ty, main.repr
            ),
        });
    }
    if overflow_box.ty != TirType::DynBox || overflow_box.repr != LirRepr::DynBox {
        errors.push(LirVerifyError {
            block: Some(bid),
            op_index: Some(op_index),
            message: format!(
                "checked i64 arithmetic overflow box must be DynBox/DynBox, found {:?}/{:?}",
                overflow_box.ty, overflow_box.repr
            ),
        });
    }
    if overflow_flag.ty != TirType::Bool || overflow_flag.repr != LirRepr::Bool1 {
        errors.push(LirVerifyError {
            block: Some(bid),
            op_index: Some(op_index),
            message: format!(
                "checked i64 arithmetic overflow flag must be Bool/Bool1, found {:?}/{:?}",
                overflow_flag.ty, overflow_flag.repr
            ),
        });
    }
}

fn verify_truthy_materialization(
    bid: BlockId,
    op_index: usize,
    op: &LirOp,
    errors: &mut Vec<LirVerifyError>,
) {
    let truthy = matches!(
        op.tir_op.attrs.get("lir.truthy_cond"),
        Some(AttrValue::Bool(true))
    );
    if !truthy {
        return;
    }
    if op.tir_op.operands.len() != 1 || op.result_values.len() != 1 {
        errors.push(LirVerifyError {
            block: Some(bid),
            op_index: Some(op_index),
            message: "truthiness materialization requires one operand and one result".to_string(),
        });
        return;
    }
    let result = &op.result_values[0];
    if result.ty != TirType::Bool || result.repr != LirRepr::Bool1 {
        errors.push(LirVerifyError {
            block: Some(bid),
            op_index: Some(op_index),
            message: format!(
                "truthiness materialization must produce Bool/Bool1, found {:?}/{:?}",
                result.ty, result.repr
            ),
        });
    }
}

fn verify_box_op(
    bid: BlockId,
    op_index: usize,
    op: &LirOp,
    _values: &HashMap<ValueId, ValueDef>,
    errors: &mut Vec<LirVerifyError>,
) {
    if op.tir_op.operands.len() != 1 || op.result_values.len() != 1 {
        errors.push(LirVerifyError {
            block: Some(bid),
            op_index: Some(op_index),
            message: "box op requires exactly one operand and one result".to_string(),
        });
        return;
    }
    let result = &op.result_values[0];
    if result.repr != LirRepr::DynBox {
        errors.push(LirVerifyError {
            block: Some(bid),
            op_index: Some(op_index),
            message: format!(
                "box op must produce a DynBox-lane result, found {:?}/{:?}",
                result.ty, result.repr,
            ),
        });
    }
}

fn verify_unbox_op(
    bid: BlockId,
    op_index: usize,
    op: &LirOp,
    values: &HashMap<ValueId, ValueDef>,
    errors: &mut Vec<LirVerifyError>,
) {
    if op.tir_op.operands.len() != 1 || op.result_values.len() != 1 {
        errors.push(LirVerifyError {
            block: Some(bid),
            op_index: Some(op_index),
            message: "unbox op requires exactly one operand and one result".to_string(),
        });
        return;
    }
    let result = &op.result_values[0];
    if result.repr == LirRepr::DynBox {
        errors.push(LirVerifyError {
            block: Some(bid),
            op_index: Some(op_index),
            message: "unbox op must produce a non-DynBox result".to_string(),
        });
    }
    match values.get(&op.tir_op.operands[0]) {
        Some(def)
            if def.value.repr == LirRepr::DynBox
                && matches!(def.value.ty, TirType::DynBox | TirType::Box(_)) => {}
        Some(def) => errors.push(LirVerifyError {
            block: Some(bid),
            op_index: Some(op_index),
            message: format!(
                "unbox op requires Box(_) or DynBox operand with DynBox repr, found {:?}/{:?}",
                def.value.ty, def.value.repr
            ),
        }),
        None => {}
    }
}
