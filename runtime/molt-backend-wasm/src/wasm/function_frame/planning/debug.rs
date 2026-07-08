use crate::FunctionIR;
use crate::wasm::WasmFrameLocals;
use std::collections::BTreeMap;

pub(super) fn emit_seed_debug(
    func_ir: &FunctionIR,
    locals: &WasmFrameLocals,
    const_seed_locals: &[(u32, i64)],
    runtime_const_op_count: usize,
) {
    if std::env::var("MOLT_DEBUG_WASM_SEEDS_FUNC").ok().as_deref() != Some(func_ir.name.as_str()) {
        return;
    }
    eprintln!(
        "WASM_SEEDS_FUNC name={} seeds={:?} runtime_const_ops={}",
        func_ir.name, const_seed_locals, runtime_const_op_count
    );
    for name in &func_ir.params {
        if let Some(idx) = locals.get(name) {
            eprintln!("WASM_SEEDS_PARAM name={} slot={}", name, idx);
        }
    }
    let mut slot_to_names: BTreeMap<u32, Vec<String>> = BTreeMap::new();
    for local in locals.named_locals() {
        slot_to_names
            .entry(local.slot())
            .or_default()
            .push(local.name().to_string());
    }
    for (slot, _) in const_seed_locals {
        if let Some(names) = slot_to_names.get(slot) {
            eprintln!("WASM_SEEDS_SLOT slot={} names={:?}", slot, names);
        }
    }
}
