use crate::FunctionIR;
use crate::wasm::WasmFrameLocals;
use std::collections::{BTreeMap, BTreeSet};
use wasm_encoder::ValType;

pub(super) fn build_callee_layout(
    func_ir: &FunctionIR,
    multi_return_candidates: &BTreeMap<String, usize>,
    locals: &mut WasmFrameLocals,
    local_types: &mut Vec<ValType>,
    local_count: &mut u32,
) -> (Option<usize>, Vec<u32>, BTreeSet<String>) {
    let callee_return_count = multi_return_candidates.get(&func_ir.name).copied();
    let mut callee_value_locals = Vec::new();
    let mut callee_tuple_vars = BTreeSet::new();

    if let Some(ret_count) = callee_return_count {
        for i in 0..ret_count {
            let local_idx = locals.ensure_multi_return_callee_value(i, local_types, local_count);
            callee_value_locals.push(local_idx);
        }
        for op in &func_ir.ops {
            if op.kind == "tuple_new"
                && let Some(args) = &op.args
                && args.len() == ret_count
                && let Some(out) = &op.out
            {
                callee_tuple_vars.insert(out.clone());
            }
        }
    }

    (callee_return_count, callee_value_locals, callee_tuple_vars)
}
