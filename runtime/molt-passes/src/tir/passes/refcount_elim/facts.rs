use molt_ir::deterministic_hash::{HashMap, HashSet};

use crate::tir::ValueRangeResult;
use crate::tir::analysis::{AnalysisManager, DomChildren, PredMap};
use crate::tir::blocks::{BlockId, Terminator};
use crate::tir::dominators::{exception_label_to_block, exception_successors};
use crate::tir::op_kinds_generated::opcode_proves_immediate_on_true;

use super::balance::refcount_balance_role;
use crate::tir::function::TirFunction;
use crate::tir::passes::alias_analysis::AliasAnalysisResult;
use crate::tir::values::ValueId;

/// Only shared physical non-heap carrier facts grant unconditional RC elision.
/// Allocation placement and capture state do not. Exact aliases inherit representation; mixed
/// CFG arguments require the shared carrier analysis's all-incoming proof.
pub(super) fn collect_rc_inert_values(
    func: &TirFunction,
    alias: &AliasAnalysisResult,
    ranges: &ValueRangeResult,
) -> HashSet<ValueId> {
    let facts =
        super::super::value_identity::ScalarFacts::with_types(func, alias.exact_scalar_types());
    crate::representation_facts::non_heap_values_from(&facts, ranges)
        .into_iter()
        .map(|value| alias.root(value))
        .collect()
}

/// A successful tag test proves the tested word carries no heap owner. Keep
/// the proof on its true edge and along the dominated subtree. At a join
/// that can also be reached without that edge the release is still needed.
pub(super) fn collect_guarded_rc_sites(
    func: &TirFunction,
    am: &AnalysisManager,
    alias: &AliasAnalysisResult,
) -> HashSet<(BlockId, usize)> {
    let mut sites = HashSet::default();
    let mut tests = HashMap::default();
    for block in func.blocks.values() {
        for op in &block.ops {
            if opcode_proves_immediate_on_true(op.opcode)
                && let ([operand], [result]) = (op.operands.as_slice(), op.results.as_slice())
            {
                tests.insert(*result, alias.root(*operand));
            }
        }
    }
    if tests.is_empty() {
        return sites;
    }

    let predecessors = am.cached::<PredMap>().expect("prepared predecessors");
    let labels = exception_label_to_block(func);
    let mut guards = HashMap::default();
    for (&source, block) in &func.blocks {
        let Terminator::CondBranch {
            cond,
            then_block,
            else_block,
            ..
        } = block.terminator
        else {
            continue;
        };
        let Some(&word) = tests.get(&alias.root(cond)) else {
            continue;
        };
        // A predecessor block may also transfer through an exception or
        // through the untaken arm. Neither entry carries the tag proof.
        if then_block != else_block
            && then_block != func.entry_block
            && predecessors
                .get(&then_block)
                .is_some_and(|preds| preds.as_slice() == [source])
            && !exception_successors(block, &labels).contains(&then_block)
        {
            guards.insert(then_block, word);
        }
    }
    if guards.is_empty() {
        return sites;
    }

    let children = am
        .cached::<DomChildren>()
        .expect("prepared dominator children");
    let mut active = HashSet::default();
    let mut pending = vec![(func.entry_block, None)];
    while let Some((block_id, exit)) = pending.pop() {
        if let Some(word) = exit {
            active.remove(&word);
            continue;
        }
        if let Some(&word) = guards.get(&block_id)
            && active.insert(word)
        {
            pending.push((block_id, Some(word)));
        }
        if !active.is_empty() {
            for (index, op) in func.blocks[&block_id].ops.iter().enumerate() {
                if refcount_balance_role(op.opcode).is_refcount_balance()
                    && let [word] = op.operands.as_slice()
                    && active.contains(&alias.root(*word))
                {
                    sites.insert((block_id, index));
                }
            }
        }
        pending.extend(children[&block_id].iter().map(|&child| (child, None)));
    }
    sites
}
