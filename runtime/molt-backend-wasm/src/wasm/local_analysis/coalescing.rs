use crate::FunctionIR;
use crate::wasm::control_flow::has_non_linear_control_flow;
use molt_tir::tir::simple_def_use::{visit_simple_ir_defined_names, visit_simple_ir_reads};
use std::collections::{BTreeMap, BTreeSet};

pub(super) fn coalesced_locals(
    func_ir: &FunctionIR,
    read_vars: &BTreeSet<String>,
    param_set: &BTreeSet<String>,
) -> BTreeMap<String, String> {
    if has_non_linear_control_flow(&func_ir.ops) {
        return BTreeMap::new();
    }

    let mut first_write: BTreeMap<String, usize> = BTreeMap::new();
    let mut last_access: BTreeMap<String, usize> = BTreeMap::new();
    for (op_idx, op) in func_ir.ops.iter().enumerate() {
        visit_simple_ir_defined_names(op, |name| {
            first_write.entry(name.to_string()).or_insert(op_idx);
            // A later write still touches the physical slot even when its
            // value is dead; it cannot overwrite a new occupant after reuse.
            last_access.insert(name.to_string(), op_idx);
        });
        visit_simple_ir_reads(op, |read| {
            last_access.insert(read.name.to_string(), op_idx);
        });
    }

    let mut ranges: Vec<(usize, usize, String)> = Vec::new();
    for (name, start) in &first_write {
        if !is_coalescable_local(name, read_vars, param_set) {
            continue;
        }
        let end = last_access.get(name).copied().unwrap_or(*start);
        ranges.push((*start, end, name.clone()));
    }
    ranges.sort_by_key(|range| range.0);

    let mut slot_end: Vec<usize> = Vec::new();
    let mut slot_repr: Vec<String> = Vec::new();
    let mut map: BTreeMap<String, String> = BTreeMap::new();
    for (start, end, name) in &ranges {
        let mut assigned = false;
        for (idx, slot_end_idx) in slot_end.iter_mut().enumerate() {
            if *slot_end_idx < *start {
                *slot_end_idx = *end;
                map.insert(name.clone(), slot_repr[idx].clone());
                assigned = true;
                break;
            }
        }
        if !assigned {
            slot_end.push(*end);
            slot_repr.push(name.clone());
            map.insert(name.clone(), name.clone());
        }
    }
    map
}

fn is_coalescable_local(
    name: &str,
    read_vars: &BTreeSet<String>,
    param_set: &BTreeSet<String>,
) -> bool {
    is_optimizer_temp_value_name(name) && !param_set.contains(name) && read_vars.contains(name)
}

fn is_optimizer_temp_value_name(name: &str) -> bool {
    name.starts_with("__tmp") || name.starts_with("__v")
}
