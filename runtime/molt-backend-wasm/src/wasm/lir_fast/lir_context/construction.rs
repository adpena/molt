use super::LirLowerCtx;
use super::cfg::validated_topology;
use super::facts::compute_lir_flat_list_int_values;
use crate::wasm::body::WasmBodyOps;
use molt_tir::tir::dominators::{CfgEdgePolicy, reverse_postorder_with};
use molt_tir::tir::lir::LirFunction;
use std::collections::HashMap;

impl<'a> LirLowerCtx<'a> {
    pub(in crate::wasm::lir_fast) fn new_with_local_base(
        func: &'a LirFunction,
        local_base: u32,
    ) -> Self {
        let cfg = validated_topology(func);
        let rpo = if cfg.blocks.len() == 1 {
            vec![cfg.entry_block]
        } else {
            reverse_postorder_with(&cfg, CfgEdgePolicy::TerminatorOnly)
        };
        let flat_list_int_values = compute_lir_flat_list_int_values(func);
        Self {
            func,
            value_locals: HashMap::new(),
            value_reprs: HashMap::new(),
            value_types: HashMap::new(),
            flat_list_int_values,
            local_types: HashMap::new(),
            next_local: local_base,
            instructions: WasmBodyOps::default(),
            rpo,
            cfg,
            operation_owners: None,
        }
    }
}
