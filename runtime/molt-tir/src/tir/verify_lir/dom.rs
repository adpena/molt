//! Dominator-tree construction over LIR control flow (including exception edges).
//!
//! Moved move-only from the monolithic `verify_lir.rs`; no logic changes.

use std::collections::{HashMap, HashSet};

use super::super::blocks::BlockId;
use super::super::lir::{LirBlock, LirFunction};
use super::super::ops::AttrValue;
use super::DominatorInfo;

fn compute_dominators(func: &LirFunction) -> HashMap<BlockId, Option<BlockId>> {
    if func.blocks.is_empty() {
        return HashMap::new();
    }

    let rpo = bfs_order(func);
    let rpo_index: HashMap<BlockId, usize> = rpo.iter().enumerate().map(|(i, &b)| (b, i)).collect();

    let mut pred: HashMap<BlockId, Vec<BlockId>> = HashMap::new();
    for bid in func.blocks.keys() {
        pred.entry(*bid).or_default();
    }
    let label_to_block = exception_label_to_block(func);
    for (bid, block) in &func.blocks {
        for succ in block.terminator.successors() {
            pred.entry(succ).or_default().push(*bid);
        }
        for succ in exception_successors(block, &label_to_block) {
            pred.entry(succ).or_default().push(*bid);
        }
    }

    let mut idom: HashMap<BlockId, Option<BlockId>> = HashMap::new();
    let entry = func.entry_block;
    idom.insert(entry, None);

    let mut changed = true;
    while changed {
        changed = false;
        for &b in &rpo {
            if b == entry {
                continue;
            }
            let preds = pred.get(&b).cloned().unwrap_or_default();
            let mut new_idom: Option<BlockId> = None;
            for &p in &preds {
                if idom.contains_key(&p) {
                    new_idom = Some(match new_idom {
                        None => p,
                        Some(cur) => intersect_dom(&idom, &rpo_index, cur, p),
                    });
                }
            }
            let old = idom.get(&b).copied().flatten();
            if !idom.contains_key(&b) || old != new_idom {
                idom.insert(b, new_idom);
                changed = true;
            }
        }
    }

    idom
}

pub(super) fn compute_dominator_tree(func: &LirFunction) -> DominatorInfo {
    let idom = compute_dominators(func);
    if idom.is_empty() {
        return DominatorInfo::default();
    }

    let mut children: HashMap<BlockId, Vec<BlockId>> = HashMap::with_capacity(idom.len());
    for &block in idom.keys() {
        children.entry(block).or_default();
    }
    for (&block, parent) in &idom {
        if let Some(parent) = *parent {
            children.entry(parent).or_default().push(block);
        }
    }

    let mut preorder: HashMap<BlockId, usize> = HashMap::with_capacity(idom.len());
    let mut postorder: HashMap<BlockId, usize> = HashMap::with_capacity(idom.len());
    let mut tick = 0usize;
    let entry = func.entry_block;

    if idom.contains_key(&entry) {
        preorder.insert(entry, tick);
        tick += 1;
        let mut stack: Vec<(BlockId, usize)> = vec![(entry, 0)];
        while let Some((node, child_idx)) = stack.last_mut() {
            let next_child = children
                .get(node)
                .and_then(|child_list| child_list.get(*child_idx))
                .copied();
            if let Some(child) = next_child {
                *child_idx += 1;
                if preorder.contains_key(&child) {
                    continue;
                }
                preorder.insert(child, tick);
                tick += 1;
                stack.push((child, 0));
            } else {
                postorder.insert(*node, tick);
                tick += 1;
                stack.pop();
            }
        }
    }

    DominatorInfo {
        preorder,
        postorder,
    }
}

fn intersect_dom(
    idom: &HashMap<BlockId, Option<BlockId>>,
    rpo: &HashMap<BlockId, usize>,
    mut a: BlockId,
    mut b: BlockId,
) -> BlockId {
    let rpo_of = |x: BlockId| rpo.get(&x).copied().unwrap_or(usize::MAX);
    let max_iters = rpo.len() * 2 + 1;
    let mut iters = 0usize;
    while a != b {
        iters += 1;
        if iters > max_iters {
            break;
        }
        while rpo_of(a) > rpo_of(b) {
            match idom.get(&a).and_then(|x| *x) {
                Some(p) if p != a => a = p,
                _ => break,
            }
        }
        while rpo_of(b) > rpo_of(a) {
            match idom.get(&b).and_then(|x| *x) {
                Some(p) if p != b => b = p,
                _ => break,
            }
        }
        let a_rpo = rpo_of(a);
        let b_rpo = rpo_of(b);
        if a_rpo == b_rpo && a != b {
            break;
        }
    }
    a
}

fn bfs_order(func: &LirFunction) -> Vec<BlockId> {
    let mut visited: HashSet<BlockId> = HashSet::new();
    let mut queue = std::collections::VecDeque::new();
    let mut order = Vec::new();

    queue.push_back(func.entry_block);
    visited.insert(func.entry_block);

    let label_to_block = exception_label_to_block(func);

    while let Some(bid) = queue.pop_front() {
        order.push(bid);
        if let Some(block) = func.blocks.get(&bid) {
            for succ in block.terminator.successors() {
                if visited.insert(succ) {
                    queue.push_back(succ);
                }
            }
            for succ in exception_successors(block, &label_to_block) {
                if visited.insert(succ) {
                    queue.push_back(succ);
                }
            }
        }
    }

    order
}

/// Build the inverse of `LirFunction::label_id_map` for resolving exception
/// edges encoded as op `value` attrs.
fn exception_label_to_block(func: &LirFunction) -> HashMap<i64, BlockId> {
    func.label_id_map
        .iter()
        .map(|(&bid, &label_id)| (label_id, BlockId(bid)))
        .collect()
}

/// Return the implicit successors of `block` that are reached only via
/// exception flow — encoded by `CheckException`/`TryStart` ops with a `value`
/// attr giving the target label_id. `TryEnd` carries pairing metadata, not a
/// handler-transfer edge. The LIR verifier needs to follow real transfer edges
/// so that exception handler blocks are considered reachable from the function
/// entry; otherwise their value uses appear to violate dominance even though at
/// runtime control flow correctly reaches them via the runtime exception path.
fn exception_successors(block: &LirBlock, label_to_block: &HashMap<i64, BlockId>) -> Vec<BlockId> {
    let mut successors = Vec::new();
    for op in &block.ops {
        if crate::tir::dominators::is_exception_transfer_edge(op.tir_op.opcode)
            && let Some(AttrValue::Int(target_label)) = op.tir_op.attrs.get("value")
            && let Some(&target) = label_to_block.get(target_label)
        {
            successors.push(target);
        }
    }
    successors
}
