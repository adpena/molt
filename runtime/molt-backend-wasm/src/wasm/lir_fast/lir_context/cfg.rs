use molt_tir::tir::blocks::BlockId;
use molt_tir::tir::lir::LirFunction;
use std::collections::HashMap;

pub(in crate::wasm::lir_fast) fn compute_lir_rpo(func: &LirFunction) -> Vec<BlockId> {
    let mut visited = HashMap::new();
    let mut order = Vec::new();
    rpo_visit_lir(func, func.entry_block, &mut visited, &mut order);
    order.reverse();
    order
}

fn rpo_visit_lir(
    func: &LirFunction,
    block_id: BlockId,
    visited: &mut HashMap<BlockId, bool>,
    order: &mut Vec<BlockId>,
) {
    if visited.contains_key(&block_id) {
        return;
    }
    visited.insert(block_id, true);
    if let Some(block) = func.blocks.get(&block_id) {
        for succ in block.terminator.successors() {
            rpo_visit_lir(func, succ, visited, order);
        }
    }
    order.push(block_id);
}
