use crate::OpIR;
use molt_tir::tir::op_kinds_generated::{
    simpleir_kind_is_wasm_state_resume_after, simpleir_kind_is_wasm_state_resume_at,
};
use std::collections::BTreeMap;

use super::{STATE_REMAP_TABLE_MAX_ENTRIES, STATE_REMAP_TABLE_MAX_SPARSITY};

pub(in crate::wasm::state_dispatch) fn build_state_resume_maps(
    ops: &[OpIR],
) -> (BTreeMap<i64, usize>, BTreeMap<String, i64>) {
    let mut state_map: BTreeMap<i64, usize> = BTreeMap::new();
    state_map.insert(0, 0);
    let mut const_ints: BTreeMap<String, i64> = BTreeMap::new();

    for (idx, op) in ops.iter().enumerate() {
        if simpleir_kind_is_wasm_state_resume_after(op.kind.as_str()) {
            if let Some(state_id) = op.value {
                state_map.insert(state_id, idx + 1);
            }
        } else if simpleir_kind_is_wasm_state_resume_at(op.kind.as_str()) {
            if let Some(state_id) = op.value {
                state_map.insert(state_id, idx);
            }
        } else if op.kind.as_str() == "const"
            && let (Some(out), Some(value)) = (op.out.as_ref(), op.value)
        {
            const_ints.insert(out.clone(), value);
        }
    }

    (state_map, const_ints)
}

pub(in crate::wasm::state_dispatch) fn build_dense_state_remap_table(
    state_map: &BTreeMap<i64, usize>,
) -> Option<Vec<u8>> {
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

pub(in crate::wasm::state_dispatch) fn build_sparse_state_remap_entries(
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
