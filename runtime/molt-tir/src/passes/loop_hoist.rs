use crate::{FunctionIR, OpIR};
use std::collections::{BTreeMap, BTreeSet, HashSet};

#[cfg_attr(
    not(any(feature = "native-backend", feature = "wasm-backend")),
    allow(dead_code)
)]
const HOISTABLE_OPS: &[&str] = &[
    "const",
    "const_int",
    "const_float",
    "const_str",
    "const_bool",
    "const_none",
    "const_bytes",
    "list_new",
    "tuple_new",
];

fn is_hoistable(op: &OpIR) -> bool {
    let kind = op.kind.as_str();
    // list_new/tuple_new/dict_new allocate fresh heap objects.
    // Hoisting them out of loops causes ONE allocation to be shared across
    // all iterations, leading to aliasing corruption if the object is mutated.
    if matches!(kind, "list_new" | "tuple_new" | "dict_new" | "set_new") {
        return false;
    }
    HOISTABLE_OPS.contains(&kind)
}

pub fn hoist_loop_invariants(func_ir: &mut FunctionIR) {
    if std::env::var("MOLT_DISABLE_LICM").is_ok() {
        return;
    }
    let ops = &func_ir.ops;
    let len = ops.len();
    if len == 0 {
        return;
    }
    let mut loop_regions: Vec<(usize, usize)> = Vec::new();
    let mut loop_start_stack: Vec<usize> = Vec::new();
    for (idx, op) in ops.iter().enumerate() {
        match op.kind.as_str() {
            "loop_start" => {
                let next_is_index = ops
                    .get(idx + 1)
                    .is_some_and(|next| next.kind == "loop_index_start");
                if !next_is_index {
                    loop_start_stack.push(idx);
                }
            }
            "loop_index_start" => {
                loop_start_stack.push(idx);
            }
            "loop_end" => {
                if let Some(start) = loop_start_stack.pop() {
                    loop_regions.push((start, idx));
                }
            }
            _ => {}
        }
    }
    if loop_regions.is_empty() {
        return;
    }
    let mut hoist_before: BTreeMap<usize, Vec<usize>> = BTreeMap::new();
    let mut hoisted_set: BTreeSet<usize> = BTreeSet::new();
    for &(start, end) in &loop_regions {
        let mut defined_in_loop: HashSet<String> = HashSet::new();
        for idx in (start + 1)..end {
            if let Some(out) = ops[idx].out.as_deref() {
                defined_in_loop.insert(out.to_string());
            }
        }
        let mut to_hoist: Vec<usize> = Vec::new();
        for idx in (start + 1)..end {
            let op = &ops[idx];
            if !is_hoistable(op) {
                continue;
            }
            let inputs_outside = op
                .args
                .as_ref()
                .is_none_or(|args| args.iter().all(|arg| !defined_in_loop.contains(arg)));
            if !inputs_outside {
                continue;
            }
            if let Some(out) = op.out.as_deref() {
                let mut write_count = 0;
                for j in (start + 1)..end {
                    if ops[j].out.as_deref() == Some(out) {
                        write_count += 1;
                    }
                }
                if write_count > 1 {
                    continue;
                }
            }
            to_hoist.push(idx);
        }
        if !to_hoist.is_empty() {
            for &idx in &to_hoist {
                hoisted_set.insert(idx);
            }
            hoist_before.entry(start).or_default().extend(to_hoist);
        }
    }
    if hoisted_set.is_empty() {
        return;
    }
    let mut new_ops: Vec<OpIR> = Vec::with_capacity(len);
    for (idx, op) in ops.iter().enumerate() {
        if let Some(hoisted_indices) = hoist_before.get(&idx) {
            for &hi in hoisted_indices {
                new_ops.push(ops[hi].clone());
            }
        }
        if hoisted_set.contains(&idx) {
            continue;
        }
        new_ops.push(op.clone());
    }
    func_ir.ops = new_ops;
}
