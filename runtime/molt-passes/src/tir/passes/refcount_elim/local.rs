use std::collections::{HashMap, HashSet};

use crate::tir::function::TirFunction;
use crate::tir::op_kinds_generated::RefcountBalanceRole;
use crate::tir::passes::alias_analysis::AliasAnalysisResult;
use crate::tir::values::ValueId;

use super::super::PassStats;
use super::balance::refcount_balance_role;

pub(super) fn eliminate_local_pairs(
    func: &mut TirFunction,
    alias: &AliasAnalysisResult,
    inert_values: &HashSet<ValueId>,
    stats: &mut PassStats,
) {
    for block in func.blocks.values_mut() {
        let mut pending: HashMap<ValueId, Vec<usize>> = HashMap::new();
        let mut remove = vec![false; block.ops.len()];
        for (index, op) in block.ops.iter().enumerate() {
            let role = refcount_balance_role(op.opcode);
            if role.is_refcount_balance() && op.has_valid_shape() && op.operands.len() == 1 {
                let root = alias.root(op.operands[0]);
                if inert_values.contains(&root) {
                    remove[index] = true;
                    continue;
                }
                if role == RefcountBalanceRole::Increment {
                    pending.entry(root).or_default().push(index);
                    continue;
                }
                if role == RefcountBalanceRole::Decrement
                    && let Some(retain) = pending.get_mut(&root).and_then(Vec::pop)
                {
                    // This explicit retain proves the matched release cannot
                    // reach zero. Unmatched releases still form a barrier.
                    remove[retain] = true;
                    remove[index] = true;
                    continue;
                }
            }
            if role.is_refcount_balance() || alias.is_rc_barrier(op) {
                pending.clear();
            }
        }
        let before = block.ops.len();
        let mut decisions = remove.into_iter();
        block.ops.retain(|_| {
            !decisions
                .next()
                .expect("one RC decision per source operation")
        });
        stats.ops_removed += before - block.ops.len();
    }
}
