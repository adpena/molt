use crate::FunctionIR;
use crate::wasm::WasmFrameLocals;
use std::collections::{BTreeMap, BTreeSet};
use wasm_encoder::ValType;

pub(super) fn build_call_layout(
    func_ir: &FunctionIR,
    multi_return_candidates: &BTreeMap<String, usize>,
    locals: &mut WasmFrameLocals,
    local_types: &mut Vec<ValType>,
    local_count: &mut u32,
) -> (BTreeMap<(String, i64), u32>, BTreeSet<String>) {
    let mut call_value_locals = BTreeMap::new();
    let mut call_tuple_vars = BTreeSet::new();
    for (op_idx, op) in func_ir.ops.iter().enumerate() {
        if op.kind != "call_internal" {
            continue;
        }
        let Some(callee) = op.s_value.as_ref() else {
            continue;
        };
        let Some(&ret_count) = multi_return_candidates.get(callee) else {
            continue;
        };
        let Some(result_var) = op.out.as_ref() else {
            continue;
        };
        if !tuple_indexes_immediately_follow(func_ir, op_idx, result_var, ret_count) {
            continue;
        }
        call_tuple_vars.insert(result_var.clone());
        for k in 0..ret_count {
            let local_idx =
                locals.ensure_multi_return_call_value(result_var, k, local_types, local_count);
            call_value_locals.insert((result_var.clone(), k as i64), local_idx);
        }
    }

    (call_value_locals, call_tuple_vars)
}

fn tuple_indexes_immediately_follow(
    func_ir: &FunctionIR,
    op_idx: usize,
    result_var: &str,
    ret_count: usize,
) -> bool {
    for k in 0..ret_count {
        let j = op_idx + 1 + k;
        let Some(next_op) = func_ir.ops.get(j) else {
            return false;
        };
        if next_op.kind != "tuple_index" {
            return false;
        }
        let Some(args) = next_op.args.as_ref() else {
            return false;
        };
        if args.len() < 2 || args[0] != result_var {
            return false;
        }
    }
    true
}
