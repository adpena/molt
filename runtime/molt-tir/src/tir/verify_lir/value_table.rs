//! Value-definition table construction and `Ref64` provenance checks.
//!
//! Moved move-only from the monolithic `verify_lir.rs`; no logic changes.

use std::collections::HashMap;

use super::super::blocks::BlockId;
use super::super::lir::{LirFunction, LirOp, LirRepr};
use super::super::values::ValueId;
use super::{LirVerifyError, ValueDef};

pub(super) fn build_value_table(
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

pub(super) fn verify_ref64_provenance(func: &LirFunction, errors: &mut Vec<LirVerifyError>) {
    for (bid, block) in &func.blocks {
        if *bid != func.entry_block {
            for arg in &block.args {
                if arg.repr == LirRepr::Ref64 {
                    errors.push(LirVerifyError::block(
                        *bid,
                        format!(
                            "Ref64 block argument {} in non-entry block ^{} has no explicit representation phi provenance",
                            arg.id, bid
                        ),
                    ));
                }
            }
        }
        for (op_index, op) in block.ops.iter().enumerate() {
            for value in &op.result_values {
                if value.repr == LirRepr::Ref64 {
                    errors.push(LirVerifyError {
                        block: Some(*bid),
                        op_index: Some(op_index),
                        message: format!(
                            "Ref64 producer for {} has no supported allocation provenance; owned class allocations must use DynBox",
                            value.id
                        ),
                    });
                }
            }
        }
    }
}
