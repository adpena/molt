//! Terminator, branch-argument, and use-dominance verification.
//!
//! Moved move-only from the monolithic `verify_lir.rs`; no logic changes.

use std::collections::HashMap;

use super::super::blocks::BlockId;
use super::super::lir::{LirFunction, LirRepr, LirTerminator};
use super::super::types::TirType;
use super::super::values::ValueId;
use super::signature::{
    signature_type_accepts_type, signature_value_accepts_repr, signature_value_accepts_type,
};
use super::{DominatorInfo, LirVerifyError, ValueDef};

pub(super) fn verify_terminators(
    func: &LirFunction,
    values: &HashMap<ValueId, ValueDef>,
    dominators: &DominatorInfo,
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
                    if let Some(def) = values.get(value_id) {
                        if !signature_value_accepts_type(expected_ty, &def.value) {
                            errors.push(LirVerifyError::block(
                                *bid,
                                format!(
                                    "return value {} type mismatch at slot {}: expected {:?}, found {:?}",
                                    def.value.id, idx, expected_ty, def.value.ty
                                ),
                            ));
                        }
                        if !signature_value_accepts_repr(expected_ty, &def.value) {
                            errors.push(LirVerifyError::block(
                                *bid,
                                format!(
                                    "return value {} representation mismatch at slot {}: expected {:?}, found {:?}",
                                    def.value.id, idx, expected_repr, def.value.repr
                                ),
                            ));
                        }
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
            LirTerminator::StateDispatch {
                cases,
                default,
                default_args,
            } => {
                // No condition value to dominance-check (the saved state is read
                // from the frame header at codegen time); only the per-edge args.
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
    dominators: &DominatorInfo,
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
        if let Some(actual) = values.get(arg_id) {
            if !signature_type_accepts_type(&expected.ty, &actual.value.ty) {
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
    }
}

pub(super) fn verify_use_dominates(
    use_block: BlockId,
    use_index: usize,
    value_id: ValueId,
    values: &HashMap<ValueId, ValueDef>,
    dominators: &DominatorInfo,
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
    dominators: &DominatorInfo,
) -> bool {
    if def.block == use_block {
        match def.op_index {
            None => true,
            Some(def_index) => def_index < use_index,
        }
    } else {
        dominators.dominates(def.block, use_block)
    }
}
