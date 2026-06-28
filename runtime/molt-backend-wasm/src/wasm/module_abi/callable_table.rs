use std::collections::{BTreeMap, BTreeSet};

use crate::TrampolineSpec;
use crate::wasm::WasmBackend;

pub(super) struct WasmCallableTablePlan {
    pub(super) table_base: u32,
    pub(super) table_indices: Vec<u32>,
    pub(super) split_runtime_owned_slot_start: usize,
    pub(super) split_runtime_shared_abi_slot_end: usize,
    pub(super) func_to_table_idx: BTreeMap<String, u32>,
    pub(super) func_to_index: BTreeMap<String, u32>,
    pub(super) func_to_trampoline_idx: BTreeMap<String, u32>,
    pub(super) closure_functions: BTreeSet<String>,
    pub(super) trampoline_entries: Vec<WasmCallableTrampolineEntry>,
}

pub(super) struct WasmCallableTrampolineEntry {
    pub(super) name: String,
    pub(super) expected_func_index: u32,
    pub(super) target_func_index: u32,
    pub(super) table_index: u32,
    pub(super) spec: TrampolineSpec,
    pub(super) multi_return_count: Option<usize>,
}

impl WasmBackend {
    pub(super) fn emit_table_abi_trampolines(
        &mut self,
        plan: &WasmCallableTablePlan,
        reloc_enabled: bool,
    ) {
        for entry in &plan.trampoline_entries {
            if self.func_count != entry.expected_func_index {
                panic!(
                    "wasm trampoline index mismatch for {}: expected {}, got {}",
                    entry.name, entry.expected_func_index, self.func_count
                );
            }
            self.compile_trampoline(
                reloc_enabled,
                entry.target_func_index,
                entry.table_index,
                entry.spec,
                entry.multi_return_count,
            );
        }
    }
}
