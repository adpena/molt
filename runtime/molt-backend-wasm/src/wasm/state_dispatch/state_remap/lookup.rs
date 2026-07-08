use std::borrow::Cow;
use std::collections::BTreeMap;
use wasm_encoder::{BlockType, Function, Instruction};

use super::{STATE_REMAP_TABLE_MAX_ENTRIES, STATE_REMAP_TABLE_MAX_SPARSITY};

const BR_TABLE_MIN_ENTRIES: usize = 5;

/// Check whether `sorted_entries` form a dense-enough range suitable for
/// `br_table` dispatch. Returns `Some((min_state, table_size))` when the
/// sparsity ratio (table_size / entry_count) is within
/// `STATE_REMAP_TABLE_MAX_SPARSITY` and there are at least
/// `BR_TABLE_MIN_ENTRIES` entries.
fn br_table_state_remap_params(sorted_entries: &[(i64, i64)]) -> Option<(i64, usize)> {
    if sorted_entries.len() < BR_TABLE_MIN_ENTRIES {
        return None;
    }
    let min_state = sorted_entries.first()?.0;
    let max_state = sorted_entries.last()?.0;
    let table_size = (max_state - min_state + 1) as usize;
    if table_size
        > sorted_entries
            .len()
            .saturating_mul(STATE_REMAP_TABLE_MAX_SPARSITY)
    {
        return None;
    }
    if table_size > STATE_REMAP_TABLE_MAX_ENTRIES {
        return None;
    }
    Some((min_state, table_size))
}

fn emit_br_table_state_remap_lookup(
    func: &mut Function,
    state_local: u32,
    sorted_entries: &[(i64, i64)],
    min_state: i64,
    table_size: usize,
) {
    let mut slot_to_target: Vec<Option<i64>> = vec![None; table_size];
    for &(state_id, target_idx) in sorted_entries {
        let slot = (state_id - min_state) as usize;
        slot_to_target[slot] = Some(target_idx);
    }

    let mut unique_targets: Vec<i64> = sorted_entries.iter().map(|&(_, target)| target).collect();
    unique_targets.sort_unstable();
    unique_targets.dedup();
    let target_block_count = unique_targets.len();

    let target_to_case: BTreeMap<i64, usize> = unique_targets
        .iter()
        .enumerate()
        .map(|(case_idx, &target_idx)| (target_idx, case_idx))
        .collect();

    let default_depth: u32 = target_block_count as u32;
    let br_targets: Vec<u32> = slot_to_target
        .iter()
        .map(|slot| match slot {
            Some(target_idx) => {
                let case_idx = target_to_case[target_idx];
                (target_block_count - 1 - case_idx) as u32
            }
            None => default_depth,
        })
        .collect();

    func.instruction(&Instruction::Block(BlockType::Empty));
    for _ in 0..target_block_count {
        func.instruction(&Instruction::Block(BlockType::Empty));
    }

    func.instruction(&Instruction::LocalGet(state_local));
    if min_state != 0 {
        func.instruction(&Instruction::I64Const(min_state));
        func.instruction(&Instruction::I64Sub);
    }
    func.instruction(&Instruction::I32WrapI64);

    let targets_cow: Cow<[u32]> = br_targets.into();
    func.instruction(&Instruction::BrTable(targets_cow, default_depth));

    for rev_i in 0..target_block_count {
        let case_idx = target_block_count - 1 - rev_i;
        func.instruction(&Instruction::End);
        let target_idx = unique_targets[case_idx];
        func.instruction(&Instruction::I64Const(target_idx));
        func.instruction(&Instruction::LocalSet(state_local));
        if rev_i < target_block_count - 1 {
            func.instruction(&Instruction::Br(case_idx as u32));
        }
    }

    func.instruction(&Instruction::End);
}

pub(in crate::wasm::state_dispatch) fn emit_sparse_state_remap_lookup(
    func: &mut Function,
    state_local: u32,
    sorted_entries: &[(i64, i64)],
) {
    if let Some((min_state, table_size)) = br_table_state_remap_params(sorted_entries) {
        emit_br_table_state_remap_lookup(func, state_local, sorted_entries, min_state, table_size);
        return;
    }

    fn emit_node(func: &mut Function, state_local: u32, entries: &[(i64, i64)]) {
        if entries.is_empty() {
            return;
        }

        let mid = entries.len() / 2;
        let (state_id, target_idx) = entries[mid];
        let left = &entries[..mid];
        let right = &entries[mid + 1..];

        func.instruction(&Instruction::LocalGet(state_local));
        func.instruction(&Instruction::I64Const(state_id));
        func.instruction(&Instruction::I64Eq);
        func.instruction(&Instruction::If(BlockType::Empty));
        func.instruction(&Instruction::I64Const(target_idx));
        func.instruction(&Instruction::LocalSet(state_local));
        if !left.is_empty() || !right.is_empty() {
            func.instruction(&Instruction::Else);
            match (!left.is_empty(), !right.is_empty()) {
                (true, true) => {
                    func.instruction(&Instruction::LocalGet(state_local));
                    func.instruction(&Instruction::I64Const(state_id));
                    func.instruction(&Instruction::I64LtS);
                    func.instruction(&Instruction::If(BlockType::Empty));
                    emit_node(func, state_local, left);
                    func.instruction(&Instruction::Else);
                    emit_node(func, state_local, right);
                    func.instruction(&Instruction::End);
                }
                (true, false) => {
                    func.instruction(&Instruction::LocalGet(state_local));
                    func.instruction(&Instruction::I64Const(state_id));
                    func.instruction(&Instruction::I64LtS);
                    func.instruction(&Instruction::If(BlockType::Empty));
                    emit_node(func, state_local, left);
                    func.instruction(&Instruction::End);
                }
                (false, true) => {
                    func.instruction(&Instruction::LocalGet(state_local));
                    func.instruction(&Instruction::I64Const(state_id));
                    func.instruction(&Instruction::I64GtS);
                    func.instruction(&Instruction::If(BlockType::Empty));
                    emit_node(func, state_local, right);
                    func.instruction(&Instruction::End);
                }
                (false, false) => {}
            }
        }
        func.instruction(&Instruction::End);
    }

    emit_node(func, state_local, sorted_entries);
}

#[cfg(test)]
mod tests {
    use super::br_table_state_remap_params;

    #[test]
    fn br_table_viable_for_dense_entries() {
        let entries: Vec<(i64, i64)> = (0..6).map(|i| (i as i64, i as i64)).collect();
        let result = br_table_state_remap_params(&entries);
        assert!(result.is_some(), "dense 6-entry range should be viable");
        let (min_state, table_size) = result.unwrap();
        assert_eq!(min_state, 0);
        assert_eq!(table_size, 6);
    }

    #[test]
    fn br_table_viable_with_offset_range() {
        let entries: Vec<(i64, i64)> = (10..15).map(|i| (i as i64, (i - 10) as i64)).collect();
        let result = br_table_state_remap_params(&entries);
        assert!(result.is_some(), "dense 5-entry range should be viable");
        let (min_state, table_size) = result.unwrap();
        assert_eq!(min_state, 10);
        assert_eq!(table_size, 5);
    }

    #[test]
    fn br_table_rejected_for_few_entries() {
        let entries: Vec<(i64, i64)> = (0..4).map(|i| (i as i64, i as i64)).collect();
        let result = br_table_state_remap_params(&entries);
        assert!(result.is_none(), "4 entries should be below the threshold");
    }

    #[test]
    fn br_table_rejected_for_sparse_entries() {
        let entries: Vec<(i64, i64)> = vec![(0, 0), (25, 1), (50, 2), (75, 3), (100, 4)];
        let result = br_table_state_remap_params(&entries);
        assert!(result.is_none(), "sparsity 20 exceeds max allowed 8");
    }

    #[test]
    fn br_table_boundary_at_exactly_threshold() {
        let entries: Vec<(i64, i64)> = (0..5).map(|i| (i as i64, i as i64)).collect();
        let result = br_table_state_remap_params(&entries);
        assert!(result.is_some(), "exactly 5 entries should pass");
        let (min_state, table_size) = result.unwrap();
        assert_eq!(min_state, 0);
        assert_eq!(table_size, 5);
    }

    #[test]
    fn br_table_sparsity_at_max_boundary() {
        let entries: Vec<(i64, i64)> = vec![(0, 0), (10, 1), (20, 2), (30, 3), (39, 4)];
        let result = br_table_state_remap_params(&entries);
        assert!(result.is_some(), "sparsity exactly 8 should be accepted");
        let (min_state, table_size) = result.unwrap();
        assert_eq!(min_state, 0);
        assert_eq!(table_size, 40);
    }

    #[test]
    fn br_table_sparsity_just_over_max() {
        let entries: Vec<(i64, i64)> = vec![(0, 0), (10, 1), (20, 2), (30, 3), (40, 4)];
        let result = br_table_state_remap_params(&entries);
        assert!(result.is_none(), "sparsity 8.2 should be rejected");
    }
}
