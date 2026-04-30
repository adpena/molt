//! Shared dominator tree utilities for TIR passes.
//!
//! Provides the Cooper-Harvey-Kennedy algorithm for computing immediate
//! dominators and a `dominates` query. Factored out of `refcount_elim.rs`
//! so that multiple passes (refcount elimination, guard-to-type propagation)
//! can reuse the same dominator computation.

use std::collections::{HashMap, HashSet};

use super::blocks::{BlockId, Terminator, TirBlock};
use super::function::TirFunction;
use super::ops::{AttrValue, OpCode};

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

/// Collect successor BlockIds from a terminator.
pub fn terminator_successors(term: &Terminator) -> Vec<BlockId> {
    match term {
        Terminator::Branch { target, .. } => vec![*target],
        Terminator::CondBranch {
            then_block,
            else_block,
            ..
        } => vec![*then_block, *else_block],
        Terminator::Switch { cases, default, .. } => {
            let mut targets: Vec<BlockId> = cases.iter().map(|(_, t, _)| *t).collect();
            targets.push(*default);
            targets.dedup();
            targets
        }
        Terminator::Return { .. } | Terminator::Unreachable => vec![],
    }
}

fn exception_label_to_block(func: &TirFunction) -> HashMap<i64, BlockId> {
    func.label_id_map
        .iter()
        .map(|(&bid, &label_id)| (label_id, BlockId(bid)))
        .collect()
}

fn exception_successors(block: &TirBlock, label_to_block: &HashMap<i64, BlockId>) -> Vec<BlockId> {
    let mut successors = Vec::new();
    for op in &block.ops {
        if matches!(
            op.opcode,
            OpCode::CheckException | OpCode::TryStart | OpCode::TryEnd
        ) && let Some(AttrValue::Int(target_label)) = op.attrs.get("value")
            && let Some(&target) = label_to_block.get(target_label)
        {
            successors.push(target);
        }
    }
    successors
}

/// Build predecessor map: BlockId -> Vec<BlockId>.
pub fn build_pred_map(func: &TirFunction) -> HashMap<BlockId, Vec<BlockId>> {
    let mut pred_map: HashMap<BlockId, Vec<BlockId>> = HashMap::new();
    for &bid in func.blocks.keys() {
        pred_map.entry(bid).or_default();
    }
    let label_to_block = exception_label_to_block(func);
    for (&bid, block) in &func.blocks {
        for succ in terminator_successors(&block.terminator) {
            pred_map.entry(succ).or_default().push(bid);
        }
        for succ in exception_successors(block, &label_to_block) {
            pred_map.entry(succ).or_default().push(bid);
        }
    }
    for preds in pred_map.values_mut() {
        preds.sort_unstable_by_key(|bid| bid.0);
        preds.dedup();
    }
    pred_map
}

/// Blocks executable from the function entry through explicit terminator edges
/// plus implicit exception edges.
///
/// Structural loop metadata can keep block records alive after optimization,
/// but those metadata-only blocks are not executable CFG nodes and must not
/// contribute typed predecessor edges.
pub fn executable_reachable_blocks(func: &TirFunction) -> HashSet<BlockId> {
    let mut visited: HashSet<BlockId> = HashSet::new();
    let mut stack: Vec<BlockId> = vec![func.entry_block];
    let label_to_block = exception_label_to_block(func);

    while let Some(bid) = stack.pop() {
        if !visited.insert(bid) {
            continue;
        }
        let Some(block) = func.blocks.get(&bid) else {
            continue;
        };
        for succ in terminator_successors(&block.terminator) {
            stack.push(succ);
        }
        for succ in exception_successors(block, &label_to_block) {
            stack.push(succ);
        }
    }

    visited
}

// ---------------------------------------------------------------------------
// Dominator tree
// ---------------------------------------------------------------------------

/// Compute immediate dominators using the Cooper-Harvey-Kennedy algorithm.
/// Returns a map from BlockId -> Option<BlockId> (idom). The entry block has
/// no dominator (None).
pub fn compute_idoms(
    func: &TirFunction,
    pred_map: &HashMap<BlockId, Vec<BlockId>>,
) -> HashMap<BlockId, Option<BlockId>> {
    // RPO numbering via DFS from entry.
    let mut rpo_order: Vec<BlockId> = Vec::new();
    let mut visited: HashSet<BlockId> = HashSet::new();

    fn dfs_postorder(
        bid: BlockId,
        func: &TirFunction,
        label_to_block: &HashMap<i64, BlockId>,
        visited: &mut HashSet<BlockId>,
        order: &mut Vec<BlockId>,
    ) {
        if !visited.insert(bid) {
            return;
        }
        if let Some(block) = func.blocks.get(&bid) {
            for succ in terminator_successors(&block.terminator) {
                dfs_postorder(succ, func, label_to_block, visited, order);
            }
            for succ in exception_successors(block, label_to_block) {
                dfs_postorder(succ, func, label_to_block, visited, order);
            }
        }
        order.push(bid);
    }

    let label_to_block = exception_label_to_block(func);
    dfs_postorder(
        func.entry_block,
        func,
        &label_to_block,
        &mut visited,
        &mut rpo_order,
    );
    rpo_order.reverse(); // Now in reverse postorder.

    // Map BlockId -> RPO index for fast lookup.
    let rpo_index: HashMap<BlockId, usize> = rpo_order
        .iter()
        .enumerate()
        .map(|(i, &bid)| (bid, i))
        .collect();

    // Intersect two dominator paths.
    let intersect = |mut a: usize, mut b: usize, doms: &[Option<usize>]| -> usize {
        while a != b {
            while a > b {
                a = doms[a].unwrap();
            }
            while b > a {
                b = doms[b].unwrap();
            }
        }
        a
    };

    let n = rpo_order.len();
    let mut doms: Vec<Option<usize>> = vec![None; n];
    doms[0] = Some(0); // Entry dominates itself.

    let mut changed = true;
    while changed {
        changed = false;
        for i in 1..n {
            let bid = rpo_order[i];
            let preds = &pred_map[&bid];

            // Find first processed predecessor.
            let mut new_idom: Option<usize> = None;
            for pred in preds {
                if let Some(&rpo_i) = rpo_index.get(pred)
                    && doms[rpo_i].is_some()
                {
                    new_idom = Some(rpo_i);
                    break;
                }
            }
            let Some(mut new_idom_val) = new_idom else {
                continue;
            };

            // Intersect with remaining processed predecessors.
            for pred in preds {
                if let Some(&rpo_i) = rpo_index.get(pred)
                    && doms[rpo_i].is_some()
                    && rpo_i != new_idom_val
                {
                    new_idom_val = intersect(rpo_i, new_idom_val, &doms);
                }
            }

            if doms[i] != Some(new_idom_val) {
                doms[i] = Some(new_idom_val);
                changed = true;
            }
        }
    }

    // Convert RPO-index idoms back to BlockIds.
    let mut result: HashMap<BlockId, Option<BlockId>> = HashMap::new();
    for (i, &bid) in rpo_order.iter().enumerate() {
        if i == 0 {
            result.insert(bid, None);
        } else {
            result.insert(bid, doms[i].map(|d| rpo_order[d]));
        }
    }
    result
}

/// Returns `true` if `dominator` dominates `target` according to the idom tree.
pub fn dominates(
    dominator: BlockId,
    target: BlockId,
    idoms: &HashMap<BlockId, Option<BlockId>>,
) -> bool {
    if dominator == target {
        return true;
    }
    let mut current = target;
    loop {
        match idoms.get(&current) {
            Some(Some(idom)) => {
                if *idom == dominator {
                    return true;
                }
                if *idom == current {
                    // Reached the root without finding dominator.
                    return false;
                }
                current = *idom;
            }
            _ => return false,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::tir::blocks::TirBlock;
    use crate::tir::types::TirType;

    #[test]
    fn single_block_dominates_itself() {
        let mut func = TirFunction::new("f".into(), vec![], TirType::None);
        {
            let entry = func.blocks.get_mut(&func.entry_block).unwrap();
            entry.terminator = Terminator::Return { values: vec![] };
        }
        let pred_map = build_pred_map(&func);
        let idoms = compute_idoms(&func, &pred_map);
        assert!(dominates(func.entry_block, func.entry_block, &idoms));
    }

    #[test]
    fn linear_chain_dominance() {
        // bb0 -> bb1 -> bb2
        let mut func = TirFunction::new("f".into(), vec![], TirType::None);
        let bb1 = func.fresh_block();
        let bb2 = func.fresh_block();

        {
            let entry = func.blocks.get_mut(&func.entry_block).unwrap();
            entry.terminator = Terminator::Branch {
                target: bb1,
                args: vec![],
            };
        }

        func.blocks.insert(
            bb1,
            TirBlock {
                id: bb1,
                args: vec![],
                ops: vec![],
                terminator: Terminator::Branch {
                    target: bb2,
                    args: vec![],
                },
            },
        );

        func.blocks.insert(
            bb2,
            TirBlock {
                id: bb2,
                args: vec![],
                ops: vec![],
                terminator: Terminator::Return { values: vec![] },
            },
        );

        let pred_map = build_pred_map(&func);
        let idoms = compute_idoms(&func, &pred_map);

        assert!(dominates(func.entry_block, bb1, &idoms));
        assert!(dominates(func.entry_block, bb2, &idoms));
        assert!(dominates(bb1, bb2, &idoms));
        assert!(!dominates(bb2, bb1, &idoms));
    }

    #[test]
    fn diamond_dominance() {
        // bb0 -> bb1, bb0 -> bb2, bb1 -> bb3, bb2 -> bb3
        let mut func = TirFunction::new("f".into(), vec![], TirType::None);
        let bb1 = func.fresh_block();
        let bb2 = func.fresh_block();
        let bb3 = func.fresh_block();
        let cond = func.fresh_value();

        // Entry has a ConstBool to use as condition.
        {
            let entry = func.blocks.get_mut(&func.entry_block).unwrap();
            entry.ops.push(crate::tir::ops::TirOp {
                dialect: crate::tir::ops::Dialect::Molt,
                opcode: crate::tir::ops::OpCode::ConstBool,
                operands: vec![],
                results: vec![cond],
                attrs: crate::tir::ops::AttrDict::new(),
                source_span: None,
            });
            entry.terminator = Terminator::CondBranch {
                cond,
                then_block: bb1,
                then_args: vec![],
                else_block: bb2,
                else_args: vec![],
            };
        }

        func.blocks.insert(
            bb1,
            TirBlock {
                id: bb1,
                args: vec![],
                ops: vec![],
                terminator: Terminator::Branch {
                    target: bb3,
                    args: vec![],
                },
            },
        );

        func.blocks.insert(
            bb2,
            TirBlock {
                id: bb2,
                args: vec![],
                ops: vec![],
                terminator: Terminator::Branch {
                    target: bb3,
                    args: vec![],
                },
            },
        );

        func.blocks.insert(
            bb3,
            TirBlock {
                id: bb3,
                args: vec![],
                ops: vec![],
                terminator: Terminator::Return { values: vec![] },
            },
        );

        let pred_map = build_pred_map(&func);
        let idoms = compute_idoms(&func, &pred_map);

        // bb0 dominates everything
        assert!(dominates(func.entry_block, bb1, &idoms));
        assert!(dominates(func.entry_block, bb2, &idoms));
        assert!(dominates(func.entry_block, bb3, &idoms));
        // bb1 does NOT dominate bb3 (bb2 also reaches bb3)
        assert!(!dominates(bb1, bb3, &idoms));
        assert!(!dominates(bb2, bb3, &idoms));
    }

    #[test]
    fn exception_edge_target_is_reachable_for_dominance() {
        use crate::tir::ops::{AttrDict, AttrValue, Dialect, OpCode, TirOp};
        use crate::tir::values::TirValue;

        let mut func = TirFunction::new("f".into(), vec![TirType::DynBox], TirType::None);
        let normal = func.fresh_block();
        let handler = func.fresh_block();
        let entry_arg = func.blocks[&func.entry_block].args[0].id;

        {
            let entry = func.blocks.get_mut(&func.entry_block).unwrap();
            let mut attrs = AttrDict::new();
            attrs.insert("value".into(), AttrValue::Int(100));
            entry.ops.push(TirOp {
                dialect: Dialect::Molt,
                opcode: OpCode::CheckException,
                operands: vec![entry_arg],
                results: vec![],
                attrs,
                source_span: None,
            });
            entry.terminator = Terminator::Branch {
                target: normal,
                args: vec![],
            };
        }

        func.blocks.insert(
            normal,
            TirBlock {
                id: normal,
                args: vec![],
                ops: vec![],
                terminator: Terminator::Return { values: vec![] },
            },
        );
        let handler_arg = func.fresh_value();
        func.blocks.insert(
            handler,
            TirBlock {
                id: handler,
                args: vec![TirValue {
                    id: handler_arg,
                    ty: TirType::DynBox,
                }],
                ops: vec![],
                terminator: Terminator::Return { values: vec![] },
            },
        );
        func.label_id_map.insert(handler.0, 100);

        let pred_map = build_pred_map(&func);
        let idoms = compute_idoms(&func, &pred_map);

        assert!(dominates(func.entry_block, handler, &idoms));
    }
}
