use std::collections::{HashMap, HashSet, VecDeque};

use crate::tir::blocks::{BlockId, TirBlock};
use crate::tir::dominators::{self, CfgEdgePolicy};
use crate::tir::function::TirFunction;
use crate::tir::ops::AttrValue;
use crate::tir::passes::alias_analysis::{AliasUnionFind, build_alias_union_find};
use crate::tir::values::ValueId;

use super::api::TirLivenessResult;
use super::flow::{live_out_of, terminator_direct_uses};
use super::raw::compute_raw_scalars;

/// One representation/alias domain for block and instruction-point liveness.
/// Exceptional successors enter the backward transfer at the actual observation,
/// never at the block terminator: definitions after a check cannot kill a value
/// needed by its handler.
struct LivenessProblem<'a> {
    func: &'a TirFunction,
    aliases: &'a AliasUnionFind,
    raw: &'a HashSet<ValueId>,
    block_args: HashMap<BlockId, HashSet<ValueId>>,
    exception_targets: HashMap<(BlockId, usize), BlockId>,
    reachable: HashSet<BlockId>,
}

impl<'a> LivenessProblem<'a> {
    fn new(func: &'a TirFunction, aliases: &'a AliasUnionFind, raw: &'a HashSet<ValueId>) -> Self {
        let labels = dominators::exception_label_to_block(func);
        let mut exception_targets = HashMap::new();
        for (&bid, block) in &func.blocks {
            for (index, op) in block.ops.iter().enumerate() {
                if dominators::is_exception_transfer_edge(op.opcode)
                    && let Some(AttrValue::Int(label)) = op.attrs.get("value")
                    && let Some(&target) = labels.get(label)
                {
                    exception_targets.insert((bid, index), target);
                }
            }
        }
        Self {
            func,
            aliases,
            raw,
            block_args: func
                .blocks
                .iter()
                .map(|(&bid, block)| (bid, block.args.iter().map(|arg| arg.id).collect()))
                .collect(),
            exception_targets,
            reachable: dominators::reachable_blocks_with(func, CfgEdgePolicy::Full),
        }
    }

    fn add_use(&self, values: &mut HashSet<ValueId>, value: ValueId) {
        let root = self.aliases.root(value);
        if !self.raw.contains(&root) {
            values.insert(root);
        }
    }

    fn normal_live_out(
        &self,
        block: &TirBlock,
        live_in: &HashMap<BlockId, HashSet<ValueId>>,
    ) -> HashSet<ValueId> {
        live_out_of(
            block,
            live_in,
            &self.block_args,
            &|v| !self.raw.contains(&self.aliases.root(v)),
            &|v| self.aliases.root(v),
        )
    }

    fn transfer(
        &self,
        block: &TirBlock,
        mut live: HashSet<ValueId>,
        live_in: &HashMap<BlockId, HashSet<ValueId>>,
        mut observe: impl FnMut(usize, BlockId, &HashSet<ValueId>, &HashSet<ValueId>),
    ) -> HashSet<ValueId> {
        for value in terminator_direct_uses(&block.terminator) {
            self.add_use(&mut live, value);
        }
        for (index, op) in block.ops.iter().enumerate().rev() {
            if let Some(&target) = self.exception_targets.get(&(block.id, index)) {
                let mut exceptional = live_in[&target].clone();
                exceptional.retain(|value| !self.block_args[&target].contains(value));
                for &value in &op.operands {
                    self.add_use(&mut exceptional, value);
                }
                // Separate normal and exceptional demands without a quadratic
                // table of per-instruction live sets.
                observe(index, target, &live, &exceptional);
                live.extend(exceptional);
            }
            for &result in &op.results {
                // A transparent copy names the same owner; it does not define
                // (and must not kill) that owner's lifetime.
                if self.aliases.root(result) == result {
                    live.remove(&result);
                }
            }
            for &operand in &op.operands {
                self.add_use(&mut live, operand);
            }
        }
        for arg in &block.args {
            live.remove(&self.aliases.root(arg.id));
        }
        live
    }

    fn solve(&self) -> TirLivenessResult {
        let mut live_in: HashMap<_, _> = self
            .func
            .blocks
            .keys()
            .map(|&bid| (bid, HashSet::new()))
            .collect();
        let mut live_out = live_in.clone();
        let predecessors = dominators::build_pred_map_with(self.func, CfgEdgePolicy::Full);
        let mut order: Vec<_> = self.reachable.iter().copied().collect();
        order.sort_unstable_by_key(|bid| std::cmp::Reverse(bid.0));
        let mut pending: VecDeque<_> = order.into();
        let mut queued = self.reachable.clone();
        while let Some(bid) = pending.pop_front() {
            queued.remove(&bid);
            let block = &self.func.blocks[&bid];
            let new_out = self.normal_live_out(block, &live_in);
            let new_in = self.transfer(block, new_out.clone(), &live_in, |_, _, _, _| {});
            live_out.insert(bid, new_out);
            if live_in[&bid] == new_in {
                continue;
            }
            live_in.insert(bid, new_in);
            if let Some(preds) = predecessors.get(&bid) {
                for &pred in preds {
                    if self.reachable.contains(&pred) && queued.insert(pred) {
                        pending.push_back(pred);
                    }
                }
            }
        }
        TirLivenessResult {
            live_in,
            live_out,
            raw_scalars: self.raw.clone(),
        }
    }
}

/// Backward dataflow over normal and point-specific exceptional successors.
/// `live_out` means the normal terminator boundary; exceptional demand enters
/// at its operation and propagates into `live_in`.
pub fn compute_liveness(func: &TirFunction) -> TirLivenessResult {
    let aliases = build_alias_union_find(func);
    let raw = compute_raw_scalars(func);
    compute_liveness_in_domain(func, &aliases, &raw)
}

/// Solve on the current operations with an already established value domain.
/// Alias and representation facts may be reused only if definitions and
/// transparent aliases are unchanged; operation uses are always read afresh.
pub(crate) fn compute_liveness_in_domain(
    func: &TirFunction,
    aliases: &AliasUnionFind,
    raw: &HashSet<ValueId>,
) -> TirLivenessResult {
    LivenessProblem::new(func, aliases, raw).solve()
}

/// Visit final exceptional ownership boundaries using the same transfer function
/// as the cached analysis. The caller may reuse the representation/alias domain
/// only across edits that preserve value definitions and transparent aliases (such as
/// inserting RC operations and argument-free edge blocks). No per-operation
/// live-set table is materialized.
pub(crate) fn visit_exception_liveness(
    func: &TirFunction,
    aliases: &AliasUnionFind,
    raw: &HashSet<ValueId>,
    mut observe: impl FnMut(BlockId, usize, BlockId, &HashSet<ValueId>, &HashSet<ValueId>),
) {
    let problem = LivenessProblem::new(func, aliases, raw);
    if problem.exception_targets.is_empty() {
        return;
    }
    let live = problem.solve();
    let mut blocks: Vec<_> = problem.reachable.iter().copied().collect();
    blocks.sort_unstable_by_key(|bid| bid.0);
    for bid in blocks {
        problem.transfer(
            &func.blocks[&bid],
            live.live_out[&bid].clone(),
            &live.live_in,
            |index, target, normal, exceptional| observe(bid, index, target, normal, exceptional),
        );
    }
}
