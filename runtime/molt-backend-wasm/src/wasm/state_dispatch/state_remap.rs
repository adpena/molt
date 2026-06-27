use crate::OpIR;
use std::borrow::Cow;
use std::collections::BTreeMap;
use wasm_encoder::{BlockType, Function, Instruction};

const STATE_REMAP_TABLE_MAX_ENTRIES: usize = 4096;
const STATE_REMAP_TABLE_MAX_SPARSITY: usize = 8;
const BR_TABLE_MIN_ENTRIES: usize = 5;

pub(super) fn build_state_resume_maps(
    ops: &[OpIR],
) -> (BTreeMap<i64, usize>, BTreeMap<String, i64>) {
    let mut state_map: BTreeMap<i64, usize> = BTreeMap::new();
    state_map.insert(0, 0);
    let mut const_ints: BTreeMap<String, i64> = BTreeMap::new();

    for (idx, op) in ops.iter().enumerate() {
        match op.kind.as_str() {
            "state_transition" | "state_yield" | "chan_send_yield" | "chan_recv_yield" => {
                if let Some(state_id) = op.value {
                    state_map.insert(state_id, idx + 1);
                }
            }
            "label" | "state_label" => {
                if let Some(state_id) = op.value {
                    state_map.insert(state_id, idx);
                }
            }
            "const" => {
                if let (Some(out), Some(value)) = (op.out.as_ref(), op.value) {
                    const_ints.insert(out.clone(), value);
                }
            }
            _ => {}
        }
    }

    (state_map, const_ints)
}

pub(super) fn build_dense_state_remap_table(state_map: &BTreeMap<i64, usize>) -> Option<Vec<u8>> {
    let mut non_negative_entries: Vec<(usize, i64)> = Vec::new();
    for (&state_id, &target_idx) in state_map {
        if state_id < 0 {
            continue;
        }
        let Ok(state_idx) = usize::try_from(state_id) else {
            return None;
        };
        non_negative_entries.push((state_idx, target_idx as i64));
    }
    if non_negative_entries.is_empty() {
        return None;
    }

    let max_state_idx = non_negative_entries
        .iter()
        .map(|(state_idx, _)| *state_idx)
        .max()?;
    let entry_count = max_state_idx.checked_add(1)?;
    if entry_count > STATE_REMAP_TABLE_MAX_ENTRIES {
        return None;
    }
    if entry_count
        > non_negative_entries
            .len()
            .saturating_mul(STATE_REMAP_TABLE_MAX_SPARSITY)
    {
        return None;
    }

    let mut table = vec![-1i64; entry_count];
    for (state_idx, target_idx) in non_negative_entries {
        table[state_idx] = target_idx;
    }
    let mut bytes = Vec::with_capacity(entry_count * std::mem::size_of::<i64>());
    for target_idx in table {
        bytes.extend_from_slice(&target_idx.to_le_bytes());
    }
    Some(bytes)
}

pub(super) fn build_sparse_state_remap_entries(
    state_map: &BTreeMap<i64, usize>,
) -> Vec<(i64, i64)> {
    let mut entries = Vec::with_capacity(state_map.len());
    for (&state_id, &target_idx) in state_map {
        if state_id < 0 {
            continue;
        }
        entries.push((state_id, target_idx as i64));
    }
    entries.sort_unstable_by_key(|(state_id, _)| *state_id);
    entries
}

/// Check whether `sorted_entries` form a dense-enough range suitable for
/// `br_table` dispatch.  Returns `Some((min_state, table_size))` when the
/// sparsity ratio (table_size / entry_count) is within
/// `STATE_REMAP_TABLE_MAX_SPARSITY` and there are at least
/// `BR_TABLE_MIN_ENTRIES` entries.
fn br_table_state_remap_params(sorted_entries: &[(i64, i64)]) -> Option<(i64, usize)> {
    if sorted_entries.len() < BR_TABLE_MIN_ENTRIES {
        return None;
    }
    let min_state = sorted_entries.first()?.0;
    let max_state = sorted_entries.last()?.0;
    // table_size covers [min_state, max_state] inclusive.
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

/// Emit a `br_table`-based O(1) state remap lookup.
///
/// Structure emitted (N = number of remap targets + 1 default):
/// ```wasm
///   block $default          ;; depth 0 â€“ fall-through = no remap
///     block $case_0         ;; depth 1
///       block $case_1       ;; depth 2
///         ...
///       block $case_{N-1}   ;; depth N
///         (local.get state_local)
///         (i64.const min_state)
///         (i64.sub)
///         (i32.wrap_i64)
///         br_table [targets...] $default
///       end  ;; $case_{N-1}
///       ;; set state_local = target for case N-1
///       br $default
///     ...
///   end  ;; $default
/// ```
fn emit_br_table_state_remap_lookup(
    func: &mut Function,
    state_local: u32,
    sorted_entries: &[(i64, i64)],
    min_state: i64,
    table_size: usize,
) {
    // Build a mapping from (state_id - min_state) -> target_idx.
    let mut slot_to_target: Vec<Option<i64>> = vec![None; table_size];
    for &(state_id, target_idx) in sorted_entries {
        let slot = (state_id - min_state) as usize;
        slot_to_target[slot] = Some(target_idx);
    }

    // Deduplicate targets to minimise block count: each unique target_idx
    // gets its own block.  Unmapped slots branch to the default (no-op).
    let mut unique_targets: Vec<i64> = sorted_entries.iter().map(|&(_, t)| t).collect();
    unique_targets.sort_unstable();
    unique_targets.dedup();
    let target_block_count = unique_targets.len(); // number of case blocks

    // Map target_idx -> index into unique_targets (0-based).
    let target_to_case: BTreeMap<i64, usize> = unique_targets
        .iter()
        .enumerate()
        .map(|(i, &t)| (t, i))
        .collect();

    // Block nesting (outermost to innermost):
    //   block $default             depth 0 from br perspective
    //     block $case_0            depth 1
    //       block $case_1          depth 2
    //         ...
    //         block $case_{N-1}    depth N   (= target_block_count)
    //           br_table ...
    //         end $case_{N-1}
    //         <code for case N-1>
    //         br $default          (depth = target_block_count)
    //       end $case_{N-2}
    //       ...
    //     end $case_0
    //   end $default
    //
    // When `br_table` branches to label L, it targets block depth L from
    // the `br_table` instruction.  We want:
    //   - default (unmapped) -> depth 0 ($default, outermost) = skip remap
    //   - case_i             -> depth (target_block_count - i) so that
    //     after `end` of that block we land in code that sets state_local.

    let default_depth: u32 = target_block_count as u32; // reaches $default

    // Build br_table target vector: one entry per table slot.
    let br_targets: Vec<u32> = slot_to_target
        .iter()
        .map(|slot| match slot {
            Some(target_idx) => {
                let case_idx = target_to_case[target_idx];
                // case_idx 0 is outermost case block (depth 1 from br_table).
                // After br_table, we want to land *after* the end of
                // $case_{case_idx}.  The innermost block ($case_0) is at
                // depth target_block_count-1; each subsequent case is one
                // level further out.  So $case_{case_idx} sits at depth
                // (target_block_count - 1 - case_idx).
                (target_block_count - 1 - case_idx) as u32
            }
            None => default_depth,
        })
        .collect();

    // Emit blocks: $default, then $case_0 .. $case_{N-1}.
    func.instruction(&Instruction::Block(BlockType::Empty)); // $default
    for _ in 0..target_block_count {
        func.instruction(&Instruction::Block(BlockType::Empty));
    }

    // Compute table index: (state_local - min_state), then i32.wrap.
    func.instruction(&Instruction::LocalGet(state_local));
    if min_state != 0 {
        func.instruction(&Instruction::I64Const(min_state));
        func.instruction(&Instruction::I64Sub);
    }
    func.instruction(&Instruction::I32WrapI64);

    // br_table dispatch.
    let targets_cow: Cow<[u32]> = br_targets.into();
    func.instruction(&Instruction::BrTable(targets_cow, default_depth));

    // Emit case bodies (innermost block ends first).
    // After `end $case_{N-1-i}`, we're inside $case_{N-2-i}, so we emit
    // the set + branch-to-default for case (N-1-i).
    for rev_i in 0..target_block_count {
        let case_idx = target_block_count - 1 - rev_i;
        func.instruction(&Instruction::End); // end $case_{case_idx}
        let target_idx = unique_targets[case_idx];
        func.instruction(&Instruction::I64Const(target_idx));
        func.instruction(&Instruction::LocalSet(state_local));
        // Branch to $default to skip remaining cases.
        // Depth from here to $default = case_idx + 1 (because we just
        // closed one block).  Actually, after closing $case_{case_idx},
        // the remaining nesting depth above us is (case_idx) case blocks
        // + 1 default block.  We want to branch to $default which is the
        // outermost, so depth = case_idx.
        if rev_i < target_block_count - 1 {
            func.instruction(&Instruction::Br(case_idx as u32));
        }
        // For the last case (case_idx == 0), we fall through to $default's End.
    }

    func.instruction(&Instruction::End); // end $default
}

pub(super) fn emit_sparse_state_remap_lookup(
    func: &mut Function,
    state_local: u32,
    sorted_entries: &[(i64, i64)],
) {
    // When the entries are dense enough, use br_table for O(1) dispatch.
    if let Some((min_state, table_size)) = br_table_state_remap_params(sorted_entries) {
        emit_br_table_state_remap_lookup(func, state_local, sorted_entries, min_state, table_size);
        return;
    }

    // Fallback: binary-search tree of nested if/else.
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
