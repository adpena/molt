use crate::OpIR;
use molt_tir::tir::op_kinds_generated::simpleir_kind_is_wasm_state_resume_at;
use std::collections::{BTreeMap, BTreeSet};

pub(in crate::wasm) fn exception_handler_region_indices(ops: &[OpIR]) -> BTreeSet<usize> {
    let mut label_to_op_index: BTreeMap<i64, usize> = BTreeMap::new();
    for (idx, op) in ops.iter().enumerate() {
        if simpleir_kind_is_wasm_state_resume_at(op.kind.as_str())
            && let Some(label_id) = op.value
        {
            label_to_op_index.insert(label_id, idx);
        }
    }
    exception_handler_region_indices_from_label_map(ops, &label_to_op_index)
}

pub(in crate::wasm::state_dispatch) fn exception_handler_region_indices_from_label_map(
    ops: &[OpIR],
    label_to_index: &BTreeMap<i64, usize>,
) -> BTreeSet<usize> {
    let mut regions = BTreeSet::new();
    let handler_labels: Vec<i64> = ops
        .iter()
        .filter_map(|op| (op.kind == "check_exception").then_some(op.value).flatten())
        .collect();
    for label in handler_labels {
        let Some(&start_idx) = label_to_index.get(&label) else {
            continue;
        };
        let mut nested_pushes = 0usize;
        for handler_idx in start_idx..ops.len() {
            let handler_op = &ops[handler_idx];
            regions.insert(handler_idx);
            match handler_op.kind.as_str() {
                "exception_push" => nested_pushes += 1,
                "exception_pop" => {
                    if nested_pushes == 0 {
                        break;
                    }
                    nested_pushes -= 1;
                }
                "ret" | "ret_void" => break,
                _ => {}
            }
        }
    }
    regions
}
