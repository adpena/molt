use std::collections::{HashMap, HashSet};

use crate::tir::blocks::{BlockId, Terminator};
use crate::tir::dominators;
use crate::tir::function::TirFunction;
use crate::tir::values::ValueId;

pub(crate) struct MaterialLoopGuard {
    pub block: BlockId,
    pub condition: ValueId,
    pub continue_on_true: bool,
    /// Blocks dominated by the successful normal guard edge, not merely by
    /// the guard block (an exception edge can leave that block before testing).
    pub success_blocks: Vec<BlockId>,
}

/// Executable dominance is built once for the analysis, not once per loop.
pub(crate) struct LoopGuardContext {
    reachable: HashSet<BlockId>,
    preds: HashMap<BlockId, Vec<BlockId>>,
    idoms: HashMap<BlockId, Option<BlockId>>,
    labels: HashMap<i64, BlockId>,
}

impl LoopGuardContext {
    pub(crate) fn new(func: &TirFunction) -> Self {
        let preds = dominators::build_pred_map(func);
        Self {
            reachable: dominators::executable_reachable_blocks(func),
            idoms: dominators::compute_idoms(func, &preds),
            preds,
            labels: dominators::exception_label_to_block(func),
        }
    }

    pub(crate) fn recurrence_is_guarded(
        &self,
        func: &TirFunction,
        header: BlockId,
        body: &HashSet<BlockId>,
        guard: &MaterialLoopGuard,
    ) -> bool {
        self.preds.get(&header).is_some_and(|preds| {
            preds.iter().all(|pred| {
                !self.reachable.contains(pred)
                    || ((!body.contains(pred) || guard.success_blocks.contains(pred))
                        && !dominators::exception_successors(&func.blocks[pred], &self.labels)
                            .contains(&header))
            })
        })
    }

    pub(crate) fn material_guard(
        &self,
        func: &TirFunction,
        header: BlockId,
        body: &HashSet<BlockId>,
    ) -> Option<MaterialLoopGuard> {
        let path = loop_guard_path(func, header, body, None)?;
        let guard = *path.last()?;
        let Terminator::CondBranch {
            cond,
            then_block,
            else_block,
            ..
        } = &func.blocks.get(&guard)?.terminator
        else {
            return None;
        };
        let continue_on_true = body.contains(then_block);
        let entry = if continue_on_true {
            *then_block
        } else {
            *else_block
        };
        // A unique executable predecessor plus absence of a parallel exceptional
        // edge makes entry dominance an edge-dominance proof. Refuse alternate
        // entry rather than inheriting a predicate from a block-level dominator.
        let edge_is_unique = self
            .preds
            .get(&entry)?
            .iter()
            .filter(|pred| self.reachable.contains(pred))
            .all(|pred| *pred == guard)
            && !dominators::exception_successors(&func.blocks[&guard], &self.labels)
                .contains(&entry);
        let mut success_blocks = Vec::new();
        if edge_is_unique {
            success_blocks.extend(body.iter().copied().filter(|bid| {
                !path.contains(bid) && dominators::dominates(entry, *bid, &self.idoms)
            }));
            success_blocks.sort_unstable_by_key(|bid| bid.0);
        }
        Some(MaterialLoopGuard {
            block: guard,
            condition: *cond,
            continue_on_true,
            success_blocks,
        })
    }
}

/// Ordered normal path from the header to its exit test. Exception-transfer
/// boundaries may interpose arbitrarily many blocks; cycles, not a depth cap,
/// terminate discovery. A terminal structured loop can name its non-material
/// guard explicitly. SCEV requests only material guards.
pub(crate) fn loop_guard_path(
    func: &TirFunction,
    header: BlockId,
    body: &HashSet<BlockId>,
    terminal_guard: Option<BlockId>,
) -> Option<Vec<BlockId>> {
    let mut path = Vec::new();
    let mut seen = HashSet::new();
    let mut current = header;
    loop {
        if !body.contains(&current) || !seen.insert(current) {
            return None;
        }
        path.push(current);
        let block = func.blocks.get(&current)?;
        match &block.terminator {
            Terminator::CondBranch {
                then_block,
                else_block,
                ..
            } if body.contains(then_block) != body.contains(else_block) => {
                return Some(path);
            }
            Terminator::Branch { .. } if terminal_guard == Some(current) => {
                return Some(path);
            }
            Terminator::Branch { target, .. } => current = *target,
            _ => return None,
        }
    }
}

/// Ordered straight-line body, including its latch but excluding the header.
pub(super) fn loop_body_path(
    func: &TirFunction,
    header: BlockId,
    entry: BlockId,
    body: &HashSet<BlockId>,
) -> Option<Vec<BlockId>> {
    let mut path = Vec::new();
    let mut seen = HashSet::new();
    let mut current = entry;
    while current != header {
        if !body.contains(&current) || !seen.insert(current) {
            return None;
        }
        path.push(current);
        match &func.blocks.get(&current)?.terminator {
            Terminator::Branch { target, .. } => current = *target,
            _ => return None,
        }
    }
    (!path.is_empty()).then_some(path)
}
