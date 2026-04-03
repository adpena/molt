//! Representation-aware LIR verifier.
//!
//! This checker is intentionally narrow in the first Task 1 slice. It proves
//! the core invariants required before any backend starts consuming LIR:
//! - entry block exists;
//! - every branch passes the right number of arguments;
//! - branch arguments match the target block parameters in semantic type and
//!   low-level representation;
//! - conditional branches consume `Bool1`;
//! - return values match the declared function return arity and representation.

use std::collections::{HashMap, HashSet};

use super::blocks::BlockId;
use super::lir::{LirFunction, LirOp, LirRepr, LirTerminator, LirValue};
use super::ops::OpCode;
use super::types::TirType;
use super::values::ValueId;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LirVerifyError {
    pub block: Option<BlockId>,
    pub op_index: Option<usize>,
    pub message: String,
}

impl LirVerifyError {
    fn func(message: impl Into<String>) -> Self {
        Self {
            block: None,
            op_index: None,
            message: message.into(),
        }
    }

    fn block(block: BlockId, message: impl Into<String>) -> Self {
        Self {
            block: Some(block),
            op_index: None,
            message: message.into(),
        }
    }
}

pub fn verify_lir_function(func: &LirFunction) -> Result<(), Vec<LirVerifyError>> {
    let mut errors = Vec::new();
    if !func.blocks.contains_key(&func.entry_block) {
        errors.push(LirVerifyError::func(format!(
            "entry block ^{} does not exist in blocks map",
            func.entry_block
        )));
        return Err(errors);
    }

    verify_entry_block_signature(func, &mut errors);
    let values = build_value_table(func, &mut errors);
    let dominators = compute_dominators(func);
    verify_ops(func, &values, &dominators, &mut errors);
    verify_terminators(func, &values, &dominators, &mut errors);

    if errors.is_empty() {
        Ok(())
    } else {
        Err(errors)
    }
}

#[derive(Debug, Clone)]
struct ValueDef {
    value: LirValue,
    block: BlockId,
    op_index: Option<usize>,
}

fn verify_entry_block_signature(func: &LirFunction, errors: &mut Vec<LirVerifyError>) {
    if func.param_names.len() != func.param_types.len() {
        errors.push(LirVerifyError::func(format!(
            "function {} declares {} param names but {} param types",
            func.name,
            func.param_names.len(),
            func.param_types.len()
        )));
    }

    let Some(entry_block) = func.blocks.get(&func.entry_block) else {
        return;
    };

    if entry_block.args.len() != func.param_types.len() {
        errors.push(LirVerifyError::block(
            func.entry_block,
            format!(
                "entry block ^{} expects {} params but function signature declares {}",
                func.entry_block,
                entry_block.args.len(),
                func.param_types.len()
            ),
        ));
        return;
    }

    for (idx, (actual, expected_ty)) in entry_block
        .args
        .iter()
        .zip(func.param_types.iter())
        .enumerate()
    {
        let expected_repr = LirRepr::for_type(expected_ty);
        if actual.ty != *expected_ty {
            errors.push(LirVerifyError::block(
                func.entry_block,
                format!(
                    "entry block ^{} type mismatch for param {}: expected {:?}, found {:?}",
                    func.entry_block, idx, expected_ty, actual.ty
                ),
            ));
        }
        if actual.repr != expected_repr {
            errors.push(LirVerifyError::block(
                func.entry_block,
                format!(
                    "entry block ^{} representation mismatch for param {}: expected {:?}, found {:?}",
                    func.entry_block, idx, expected_repr, actual.repr
                ),
            ));
        }
    }
}

fn build_value_table(
    func: &LirFunction,
    errors: &mut Vec<LirVerifyError>,
) -> HashMap<ValueId, ValueDef> {
    let mut table = HashMap::new();
    for (bid, block) in &func.blocks {
        if block.id != *bid {
            errors.push(LirVerifyError::block(
                *bid,
                format!(
                    "block map key ^{} does not match embedded id ^{}",
                    bid, block.id
                ),
            ));
        }
        for arg in &block.args {
            if table
                .insert(
                    arg.id,
                    ValueDef {
                        value: arg.clone(),
                        block: *bid,
                        op_index: None,
                    },
                )
                .is_some()
            {
                errors.push(LirVerifyError::block(
                    *bid,
                    format!("duplicate definition of {}", arg.id),
                ));
            }
        }
        for (op_index, op) in block.ops.iter().enumerate() {
            insert_op_results(*bid, op_index, op, &mut table, errors);
        }
    }
    table
}

fn insert_op_results(
    bid: BlockId,
    op_index: usize,
    op: &LirOp,
    table: &mut HashMap<ValueId, ValueDef>,
    errors: &mut Vec<LirVerifyError>,
) {
    for value in &op.result_values {
        if table
            .insert(
                value.id,
                ValueDef {
                    value: value.clone(),
                    block: bid,
                    op_index: Some(op_index),
                },
            )
            .is_some()
        {
            errors.push(LirVerifyError::block(
                bid,
                format!("duplicate definition of {}", value.id),
            ));
        }
    }
}

fn compute_dominators(func: &LirFunction) -> HashMap<BlockId, HashSet<BlockId>> {
    let all_blocks: HashSet<BlockId> = func.blocks.keys().copied().collect();
    let predecessors = compute_predecessors(func);
    let mut dominators = HashMap::new();
    for bid in func.blocks.keys().copied() {
        if bid == func.entry_block {
            dominators.insert(bid, HashSet::from([bid]));
        } else {
            dominators.insert(bid, all_blocks.clone());
        }
    }

    let mut changed = true;
    while changed {
        changed = false;
        for bid in func
            .blocks
            .keys()
            .copied()
            .filter(|bid| *bid != func.entry_block)
        {
            let preds = predecessors.get(&bid).cloned().unwrap_or_default();
            let mut new_set = if preds.is_empty() {
                HashSet::from([bid])
            } else {
                let mut iter = preds.iter();
                let first = iter
                    .next()
                    .and_then(|pred| dominators.get(pred).cloned())
                    .unwrap_or_default();
                let mut acc = first;
                for pred in iter {
                    if let Some(pred_set) = dominators.get(pred) {
                        acc = acc.intersection(pred_set).copied().collect();
                    }
                }
                acc.insert(bid);
                acc
            };
            new_set.insert(bid);
            let slot = dominators.get_mut(&bid).expect("dominator set initialized");
            if *slot != new_set {
                *slot = new_set;
                changed = true;
            }
        }
    }

    dominators
}

fn compute_predecessors(func: &LirFunction) -> HashMap<BlockId, Vec<BlockId>> {
    let mut preds: HashMap<BlockId, Vec<BlockId>> = func
        .blocks
        .keys()
        .copied()
        .map(|bid| (bid, Vec::new()))
        .collect();

    for (bid, block) in &func.blocks {
        for succ in terminator_successors(&block.terminator) {
            preds.entry(succ).or_default().push(*bid);
        }
    }

    preds
}

fn terminator_successors(terminator: &LirTerminator) -> Vec<BlockId> {
    match terminator {
        LirTerminator::Branch { target, .. } => vec![*target],
        LirTerminator::CondBranch {
            then_block,
            else_block,
            ..
        } => vec![*then_block, *else_block],
        LirTerminator::Switch { cases, default, .. } => {
            let mut targets = cases.iter().map(|(_, block, _)| *block).collect::<Vec<_>>();
            targets.push(*default);
            targets
        }
        LirTerminator::Return { .. } | LirTerminator::Unreachable => Vec::new(),
    }
}

fn verify_ops(
    func: &LirFunction,
    values: &HashMap<ValueId, ValueDef>,
    dominators: &HashMap<BlockId, HashSet<BlockId>>,
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
            match op.tir_op.opcode {
                OpCode::BoxVal => verify_box_op(*bid, op_index, op, values, errors),
                OpCode::UnboxVal => verify_unbox_op(*bid, op_index, op, values, errors),
                _ => {}
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

fn verify_box_op(
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
            message: "box op requires exactly one operand and one result".to_string(),
        });
        return;
    }
    let result = &op.result_values[0];
    if result.repr != LirRepr::DynBox || result.ty != TirType::DynBox {
        errors.push(LirVerifyError {
            block: Some(bid),
            op_index: Some(op_index),
            message: format!(
                "box op must produce DynBox/DynBox, found {:?}/{:?}",
                result.ty, result.repr
            ),
        });
    }
    if let Some(def) = values.get(&op.tir_op.operands[0]) {
        if def.value.repr == LirRepr::DynBox {
            errors.push(LirVerifyError {
                block: Some(bid),
                op_index: Some(op_index),
                message: "box op operand is already DynBox".to_string(),
            });
        }
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
        Some(def) if def.value.repr == LirRepr::DynBox && def.value.ty == TirType::DynBox => {}
        Some(def) => errors.push(LirVerifyError {
            block: Some(bid),
            op_index: Some(op_index),
            message: format!(
                "unbox op requires DynBox operand, found {:?}/{:?}",
                def.value.ty, def.value.repr
            ),
        }),
        None => {}
    }
}

fn verify_terminators(
    func: &LirFunction,
    values: &HashMap<ValueId, ValueDef>,
    dominators: &HashMap<BlockId, HashSet<BlockId>>,
    errors: &mut Vec<LirVerifyError>,
) {
    for (bid, block) in &func.blocks {
        let use_index = block.ops.len();
        match &block.terminator {
            LirTerminator::Branch { target, args } => {
                verify_branch_args(
                    *bid, use_index, *target, args, func, values, dominators, errors,
                );
            }
            LirTerminator::CondBranch {
                cond,
                then_block,
                then_args,
                else_block,
                else_args,
            } => {
                verify_use_dominates(
                    *bid,
                    use_index,
                    *cond,
                    values,
                    dominators,
                    errors,
                    "conditional branch condition",
                );
                match values.get(cond) {
                    Some(def) if def.value.repr == LirRepr::Bool1 && def.value.ty == TirType::Bool => {}
                    Some(def) if def.value.repr != LirRepr::Bool1 => errors.push(LirVerifyError::block(
                        *bid,
                        format!(
                            "conditional branch requires Bool1 condition, found {:?} for {}",
                            def.value.repr, def.value.id
                        ),
                    )),
                    Some(def) => errors.push(LirVerifyError::block(
                        *bid,
                        format!(
                            "conditional branch requires semantic Bool condition, found {:?} for {}",
                            def.value.ty, def.value.id
                        ),
                    )),
                    None => {}
                }
                verify_branch_args(
                    *bid,
                    use_index,
                    *then_block,
                    then_args,
                    func,
                    values,
                    dominators,
                    errors,
                );
                verify_branch_args(
                    *bid,
                    use_index,
                    *else_block,
                    else_args,
                    func,
                    values,
                    dominators,
                    errors,
                );
            }
            LirTerminator::Return {
                values: return_values,
            } => {
                if return_values.len() != func.return_types.len() {
                    errors.push(LirVerifyError::block(
                        *bid,
                        format!(
                            "return arity mismatch: expected {}, found {}",
                            func.return_types.len(),
                            return_values.len()
                        ),
                    ));
                    continue;
                }
                for (idx, (value_id, expected_ty)) in return_values
                    .iter()
                    .zip(func.return_types.iter())
                    .enumerate()
                {
                    verify_use_dominates(
                        *bid,
                        use_index,
                        *value_id,
                        values,
                        dominators,
                        errors,
                        "return value",
                    );
                    let expected_repr = LirRepr::for_type(expected_ty);
                    match values.get(value_id) {
                        Some(def) => {
                            if def.value.ty != *expected_ty {
                                errors.push(LirVerifyError::block(
                                    *bid,
                                    format!(
                                        "return value {} type mismatch at slot {}: expected {:?}, found {:?}",
                                        def.value.id, idx, expected_ty, def.value.ty
                                    ),
                                ));
                            }
                            if def.value.repr != expected_repr {
                                errors.push(LirVerifyError::block(
                                    *bid,
                                    format!(
                                        "return value {} representation mismatch at slot {}: expected {:?}, found {:?}",
                                        def.value.id, idx, expected_repr, def.value.repr
                                    ),
                                ));
                            }
                        }
                        None => {}
                    }
                }
            }
            LirTerminator::Switch {
                value,
                cases,
                default,
                default_args,
            } => {
                verify_use_dominates(
                    *bid,
                    use_index,
                    *value,
                    values,
                    dominators,
                    errors,
                    "switch value",
                );
                for (_, target, args) in cases {
                    verify_branch_args(
                        *bid, use_index, *target, args, func, values, dominators, errors,
                    );
                }
                verify_branch_args(
                    *bid,
                    use_index,
                    *default,
                    default_args,
                    func,
                    values,
                    dominators,
                    errors,
                );
            }
            LirTerminator::Unreachable => {}
        }
    }
}

fn verify_branch_args(
    source: BlockId,
    use_index: usize,
    target: BlockId,
    args: &[ValueId],
    func: &LirFunction,
    values: &HashMap<ValueId, ValueDef>,
    dominators: &HashMap<BlockId, HashSet<BlockId>>,
    errors: &mut Vec<LirVerifyError>,
) {
    let Some(target_block) = func.blocks.get(&target) else {
        errors.push(LirVerifyError::block(
            source,
            format!("branch targets missing block ^{}", target),
        ));
        return;
    };

    if args.len() != target_block.args.len() {
        errors.push(LirVerifyError::block(
            source,
            format!(
                "branch to ^{} passes {} args but target expects {}",
                target,
                args.len(),
                target_block.args.len()
            ),
        ));
        return;
    }

    for (idx, (arg_id, expected)) in args.iter().zip(target_block.args.iter()).enumerate() {
        verify_use_dominates(
            source,
            use_index,
            *arg_id,
            values,
            dominators,
            errors,
            "branch argument",
        );
        match values.get(arg_id) {
            Some(actual) => {
                if actual.value.ty != expected.ty {
                    errors.push(LirVerifyError::block(
                        source,
                        format!(
                            "branch type mismatch for target ^{} arg {}: expected {:?}, found {:?}",
                            target, idx, expected.ty, actual.value.ty
                        ),
                    ));
                }
                if actual.value.repr != expected.repr {
                    errors.push(LirVerifyError::block(
                        source,
                        format!(
                            "branch representation mismatch for target ^{} arg {}: expected {:?}, found {:?}",
                            target, idx, expected.repr, actual.value.repr
                        ),
                    ));
                }
            }
            None => {}
        }
    }
}

fn verify_use_dominates(
    use_block: BlockId,
    use_index: usize,
    value_id: ValueId,
    values: &HashMap<ValueId, ValueDef>,
    dominators: &HashMap<BlockId, HashSet<BlockId>>,
    errors: &mut Vec<LirVerifyError>,
    context: &str,
) {
    match values.get(&value_id) {
        Some(def) if definition_dominates(def, use_block, use_index, dominators) => {}
        Some(def) => errors.push(LirVerifyError::block(
            use_block,
            format!(
                "{context} {} defined in ^{} does not dominate use in ^{}",
                value_id, def.block, use_block
            ),
        )),
        None => errors.push(LirVerifyError::block(
            use_block,
            format!("{context} uses undefined value {}", value_id),
        )),
    }
}

fn definition_dominates(
    def: &ValueDef,
    use_block: BlockId,
    use_index: usize,
    dominators: &HashMap<BlockId, HashSet<BlockId>>,
) -> bool {
    if def.block == use_block {
        match def.op_index {
            None => true,
            Some(def_index) => def_index < use_index,
        }
    } else {
        dominators
            .get(&use_block)
            .map(|doms| doms.contains(&def.block))
            .unwrap_or(false)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::tir::blocks::BlockId;
    use crate::tir::lir::{LirBlock, LirFunction, LirTerminator};

    fn value(id: u32, ty: TirType, repr: LirRepr) -> LirValue {
        LirValue {
            id: ValueId(id),
            ty,
            repr,
        }
    }

    #[test]
    fn repr_for_bool_return_must_match_bool1() {
        let entry = LirBlock {
            id: BlockId(0),
            args: vec![value(0, TirType::Bool, LirRepr::Bool1)],
            ops: vec![],
            terminator: LirTerminator::Return {
                values: vec![ValueId(0)],
            },
        };
        let mut blocks = HashMap::new();
        blocks.insert(BlockId(0), entry);
        let func = LirFunction {
            name: "bool_return".to_string(),
            param_names: vec!["flag".to_string()],
            param_types: vec![TirType::Bool],
            return_types: vec![TirType::Bool],
            blocks,
            entry_block: BlockId(0),
        };
        assert!(verify_lir_function(&func).is_ok());
    }
}
