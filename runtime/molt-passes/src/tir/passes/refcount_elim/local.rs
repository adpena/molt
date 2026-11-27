use molt_ir::deterministic_hash::{HashMap, HashSet};

use crate::tir::blocks::BlockId;
use crate::tir::function::TirFunction;
use crate::tir::op_kinds_generated::{RefcountBalanceRole, copy_kind_mints_owned_alias_ref_table};
use crate::tir::ops::OpCode;
use crate::tir::passes::alias_analysis::AliasAnalysisResult;
use crate::tir::values::ValueId;

use super::super::PassStats;
use super::balance::refcount_balance_role;

pub(super) fn eliminate_local_pairs(
    func: &mut TirFunction,
    alias: &AliasAnalysisResult,
    inert_values: &HashSet<ValueId>,
    guarded_sites: &HashSet<(BlockId, usize)>,
    stats: &mut PassStats,
) {
    let dead_aliases = adjacent_alias_releases(func);
    for (&block_id, block) in &mut func.blocks {
        let mut pending: HashMap<ValueId, Vec<usize>> = HashMap::default();
        let mut remove = vec![false; block.ops.len()];
        for (index, op) in block.ops.iter().enumerate() {
            if let [result] = op.results.as_slice()
                && dead_aliases.contains(result)
            {
                remove[index] = true;
                remove[index + 1] = true;
                continue;
            }
            if remove[index] {
                continue;
            }
            let role = refcount_balance_role(op.opcode);
            if role.is_refcount_balance() && op.has_valid_shape() && op.operands.len() == 1 {
                let root = alias.root(op.operands[0]);
                if inert_values.contains(&root) || guarded_sites.contains(&(block_id, index)) {
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

/// The owned alias retains its input's existing carrier. An immediately
/// returned credit with no other readers follows the same local balance rule
/// as an explicit retain followed by a release.
fn adjacent_alias_releases(func: &TirFunction) -> HashSet<ValueId> {
    let mut candidates: HashMap<ValueId, (BlockId, usize)> = HashMap::default();
    for (&id, block) in &func.blocks {
        for (index, pair) in block.ops.windows(2).enumerate() {
            let alias = &pair[0];
            let release = &pair[1];
            if alias.opcode == OpCode::Copy
                && alias
                    .origin
                    .is_some_and(copy_kind_mints_owned_alias_ref_table)
                && alias.operands.len() == 1
                && let [owned] = alias.results.as_slice()
                && release.opcode == OpCode::DecRef
                && release.operands == [*owned]
                && release.results.is_empty()
            {
                candidates.insert(*owned, (id, index + 1));
            }
        }
    }
    if candidates.is_empty() {
        return HashSet::default();
    }
    for (&id, block) in &func.blocks {
        for (index, op) in block.ops.iter().enumerate() {
            for operand in &op.operands {
                if candidates
                    .get(operand)
                    .is_some_and(|&site| site != (id, index))
                {
                    candidates.remove(operand);
                }
            }
        }
        block.terminator.for_each_value(|value| {
            candidates.remove(&value);
        });
    }
    candidates.into_keys().collect()
}

#[cfg(test)]
mod tests {
    use crate::tir::analysis::AnalysisManager;
    use crate::tir::blocks::Terminator;
    use crate::tir::ops::TirOp;
    use crate::tir::types::TirType;

    use super::*;

    #[test]
    fn adjacent_owned_alias_requires_a_single_release_use() {
        for escapes in [false, true] {
            let mut func = TirFunction::new(
                "owned_alias".into(),
                vec![TirType::DynBox],
                TirType::DynBox,
                molt_ir::FunctionReturnAbi::Value,
            );
            let source = func.blocks[&func.entry_block].args[0].id;
            let owned = func.fresh_value();
            func.value_types.insert(owned, TirType::DynBox);
            let mut alias = TirOp::new(OpCode::Copy, vec![source], vec![owned]);
            alias.origin = Some(molt_ir::OpKind::BindingAlias);
            let release = TirOp::new(OpCode::DecRef, vec![owned], vec![]);
            let entry = func.blocks.get_mut(&func.entry_block).unwrap();
            entry.ops = vec![alias, release];
            entry.terminator = Terminator::Return {
                values: vec![if escapes { owned } else { source }],
            };

            super::super::run_post_drop(&mut func, &mut AnalysisManager::new());
            assert_eq!(
                func.blocks[&func.entry_block].ops.len(),
                if escapes { 2 } else { 0 }
            );
        }
    }
}
