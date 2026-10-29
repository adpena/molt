use crate::OpIR;
use std::collections::BTreeMap;

use super::{STATE_REMAP_TABLE_MAX_ENTRIES, STATE_REMAP_TABLE_MAX_SPARSITY};

pub(in crate::wasm::state_dispatch) fn build_state_resume_maps(
    ops: &[OpIR],
    initial_dispatch_index: usize,
) -> BTreeMap<i64, usize> {
    molt_ir::ir_schema::validate_state_dispatch(ops)
        .unwrap_or_else(|error| panic!("invalid WASM state dispatch: {error}"));
    let targets = ops
        .iter()
        .find_map(|op| op.state_targets.as_ref())
        .expect("WASM state dispatch requires the shared terminal StateDispatch map");
    let labels: BTreeMap<_, _> = ops
        .iter()
        .enumerate()
        .filter_map(|(index, op)| {
            matches!(
                op.kind,
                molt_ir::OpKind::Label | molt_ir::OpKind::StateLabel
            )
            .then(|| op.value.map(|label| (label, index)))
            .flatten()
        })
        .collect();
    let mut state_map = BTreeMap::from([(0, initial_dispatch_index)]);
    for &(state, label) in targets {
        state_map.insert(state, labels[&label]);
    }
    state_map
}

pub(in crate::wasm::state_dispatch) fn stateful_entry_prologue_end(ops: &[OpIR]) -> usize {
    let mut switches = ops
        .iter()
        .enumerate()
        .filter(|(_, op)| op.kind == molt_ir::OpKind::StateSwitch)
        .map(|(index, _)| index);
    let index = switches
        .next()
        .expect("stateful wasm function missing canonical state_switch delimiter");
    assert!(
        switches.next().is_none(),
        "stateful wasm function has multiple state_switch delimiters"
    );
    index
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

#[cfg(test)]
mod tests {
    use super::{build_state_resume_maps, stateful_entry_prologue_end};
    use crate::OpIR;

    fn op(kind: molt_ir::OpKind, value: Option<i64>) -> OpIR {
        OpIR {
            value,
            ..OpIR::new(kind)
        }
    }

    #[test]
    fn stateful_entry_prologue_runs_before_initial_and_resumed_dispatch() {
        let ops = vec![
            op(molt_ir::OpKind::ExceptionStackEnter, None),
            op(molt_ir::OpKind::StoreVar, None),
            OpIR {
                state_targets: Some(vec![(5, 15)]),
                ..op(molt_ir::OpKind::StateSwitch, None)
            },
            op(molt_ir::OpKind::Jump, Some(9)),
            op(molt_ir::OpKind::StateSet, Some(5)),
            op(molt_ir::OpKind::StateLabel, Some(15)),
            op(molt_ir::OpKind::Ret, None),
        ];
        let prologue_end = stateful_entry_prologue_end(&ops);
        let state_map = build_state_resume_maps(&ops, prologue_end + 1);

        assert_eq!(prologue_end, 2);
        assert_eq!(state_map.get(&0), Some(&3));
        assert_eq!(state_map.get(&5), Some(&5));
    }
}
