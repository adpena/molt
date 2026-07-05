use std::collections::HashSet;

use crate::tir::function::TirFunction;
use crate::tir::passes::alias_analysis::AliasAnalysisResult;
use crate::tir::values::ValueId;

use super::super::PassStats;
use super::balance::{complementary_refcount_opcode, is_refcount_balance_op};

pub(super) fn eliminate_local_pairs(
    func: &mut TirFunction,
    alias: &AliasAnalysisResult,
    stack_alloc_vals: &HashSet<ValueId>,
    stats: &mut PassStats,
) {
    let block_ids: Vec<_> = func.blocks.keys().copied().collect();

    for bid in block_ids {
        let block = match func.blocks.get_mut(&bid) {
            Some(b) => b,
            None => continue,
        };

        let n = block.ops.len();
        if n == 0 {
            continue;
        }

        let mut remove = vec![false; n];

        for i in 0..n {
            let op = &block.ops[i];
            if is_refcount_balance_op(op.opcode)
                && op
                    .operands
                    .first()
                    .is_some_and(|v| stack_alloc_vals.contains(v))
            {
                remove[i] = true;
            }
        }

        for i in 0..n {
            if remove[i] {
                continue;
            }
            let Some(target_opcode) = complementary_refcount_opcode(block.ops[i].opcode) else {
                continue;
            };
            let Some(val_i) = block.ops[i].operands.first().copied() else {
                continue;
            };

            let partner = {
                let mut result = None;
                for j in (i + 1)..n {
                    if remove[j] {
                        continue;
                    }
                    let op_j = &block.ops[j];
                    if alias.is_rc_barrier(op_j) {
                        break;
                    }
                    if op_j.opcode == target_opcode && op_j.operands.first().copied() == Some(val_i)
                    {
                        result = Some(j);
                        break;
                    }
                }
                result
            };
            if let Some(j) = partner {
                remove[i] = true;
                remove[j] = true;
            }
        }

        let before_len = block.ops.len();
        let mut remove_iter = remove.iter();
        block
            .ops
            .retain(|_| !remove_iter.next().copied().unwrap_or(false));
        stats.ops_removed += before_len - block.ops.len();
    }
}
