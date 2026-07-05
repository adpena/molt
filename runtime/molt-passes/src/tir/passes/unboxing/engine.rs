use std::collections::{HashMap, HashSet};

use crate::tir::blocks::BlockId;
use crate::tir::function::TirFunction;
use crate::tir::ops::OpCode;
use crate::tir::values::ValueId;

use super::super::PassStats;
use super::terminator::{replace_in_terminator, terminator_values};

/// Run the unboxing elimination pass on `func`.
pub fn run(func: &mut TirFunction) -> PassStats {
    let mut stats = PassStats {
        name: "unboxing",
        ..Default::default()
    };

    let block_ids: Vec<u32> = func.blocks.keys().map(|b| b.0).collect();
    let use_map = build_use_map(func, &block_ids);

    let mut replacements: HashMap<ValueId, ValueId> = HashMap::new();
    let mut ops_to_remove: HashSet<(u32, usize)> = HashSet::new();

    for &bid_u32 in &block_ids {
        let bid = BlockId(bid_u32);
        let block = &func.blocks[&bid];
        for (op_idx, op) in block.ops.iter().enumerate() {
            if op.opcode != OpCode::BoxVal {
                continue;
            }
            if op.operands.len() != 1 || op.results.len() != 1 {
                continue;
            }
            let pre_box_value = op.operands[0];
            let boxed_value = op.results[0];

            let uses = match use_map.get(&boxed_value) {
                Some(u) => u,
                None => {
                    ops_to_remove.insert((bid_u32, op_idx));
                    stats.ops_removed += 1;
                    continue;
                }
            };

            let Some(unbox_ops) = all_uses_are_unboxes(func, uses) else {
                continue;
            };
            if unbox_ops.is_empty() {
                continue;
            }

            ops_to_remove.insert((bid_u32, op_idx));
            stats.ops_removed += 1;

            for (ub_bid, ub_idx) in &unbox_ops {
                let ub_block = &func.blocks[&BlockId(*ub_bid)];
                let ub_op = &ub_block.ops[*ub_idx];
                let unbox_result = ub_op.results[0];
                replacements.insert(unbox_result, pre_box_value);
                ops_to_remove.insert((*ub_bid, *ub_idx));
                stats.ops_removed += 1;
                stats.values_changed += 1;
            }
        }
    }

    if replacements.is_empty() && ops_to_remove.is_empty() {
        return stats;
    }

    let replacements = resolve_transitive(&replacements);
    apply_replacements(func, &replacements);
    remove_marked_ops(func, &ops_to_remove);
    stats
}

fn build_use_map(func: &TirFunction, block_ids: &[u32]) -> HashMap<ValueId, Vec<(u32, usize)>> {
    let mut use_map: HashMap<ValueId, Vec<(u32, usize)>> = HashMap::new();

    for &bid_u32 in block_ids {
        let bid = BlockId(bid_u32);
        let block = &func.blocks[&bid];
        for (op_idx, op) in block.ops.iter().enumerate() {
            for operand in &op.operands {
                use_map.entry(*operand).or_default().push((bid_u32, op_idx));
            }
        }
        for v in terminator_values(&block.terminator) {
            use_map.entry(v).or_default().push((bid_u32, usize::MAX));
        }
    }

    use_map
}

fn all_uses_are_unboxes(func: &TirFunction, uses: &[(u32, usize)]) -> Option<Vec<(u32, usize)>> {
    let mut unbox_ops = Vec::new();
    for &(use_bid, use_op_idx) in uses {
        if use_op_idx == usize::MAX {
            return None;
        }
        let use_block = &func.blocks[&BlockId(use_bid)];
        let use_op = &use_block.ops[use_op_idx];
        if use_op.opcode != OpCode::UnboxVal {
            return None;
        }
        if use_op.operands.len() != 1 || use_op.results.len() != 1 {
            return None;
        }
        unbox_ops.push((use_bid, use_op_idx));
    }
    Some(unbox_ops)
}

/// Resolve transitive replacement chains: if A -> B and B -> C, then A -> C.
fn resolve_transitive(replacements: &HashMap<ValueId, ValueId>) -> HashMap<ValueId, ValueId> {
    let mut resolved = HashMap::with_capacity(replacements.len());
    for (&from, &to) in replacements {
        let mut current = to;
        let mut seen = HashSet::new();
        seen.insert(from);
        while let Some(&next) = replacements.get(&current) {
            if !seen.insert(current) {
                break;
            }
            current = next;
        }
        resolved.insert(from, current);
    }
    resolved
}

fn apply_replacements(func: &mut TirFunction, replacements: &HashMap<ValueId, ValueId>) {
    for block in func.blocks.values_mut() {
        for op in &mut block.ops {
            for operand in &mut op.operands {
                if let Some(&replacement) = replacements.get(operand) {
                    *operand = replacement;
                }
            }
        }
        replace_in_terminator(&mut block.terminator, replacements);
    }
}

fn remove_marked_ops(func: &mut TirFunction, ops_to_remove: &HashSet<(u32, usize)>) {
    for block in func.blocks.values_mut() {
        let bid_u32 = block.id.0;
        let mut indices: Vec<usize> = ops_to_remove
            .iter()
            .filter(|(b, _)| *b == bid_u32)
            .map(|(_, idx)| *idx)
            .collect();
        indices.sort_unstable_by(|a, b| b.cmp(a));
        for idx in indices {
            block.ops.remove(idx);
        }
    }
}
