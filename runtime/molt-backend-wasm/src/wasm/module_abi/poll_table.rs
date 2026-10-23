use std::collections::BTreeMap;

use wasm_encoder::{Function, Instruction};

use crate::wasm::WasmBackend;
use crate::wasm_abi::{POLL_TABLE_IMPORTS, WasmRuntimeImport, poll_table_import_slot};
use crate::wasm_binary::emit_call;
use crate::wasm_import_tracking::TrackedImportIds;
use crate::wasm_options::WasmRuntimePollSlots;

/// The fixed poll-table prefix of the app's callable table.
///
/// The prefix length comes from the generated slots, so the table layout is
/// the same for every runtime. Only the slots the bound runtime defines hold
/// its poll functions; the app does not import an absent one, and its slot
/// holds the sentinel. No future can name an absent slot: the runtime that
/// would create it lacks the feature that defines the poll function.
pub(super) struct WasmPollTableLayout {
    prefix_len: u32,
    runtime_slots: WasmRuntimePollSlots,
}

impl WasmPollTableLayout {
    pub(super) fn build(runtime_slots: WasmRuntimePollSlots) -> Self {
        let prefix_len = POLL_TABLE_IMPORTS
            .iter()
            .map(|spec| spec.table_slot)
            .max()
            .unwrap_or(0)
            + 1;
        Self {
            prefix_len,
            runtime_slots,
        }
    }

    pub(super) fn prefix_len(&self) -> u32 {
        self.prefix_len
    }

    /// `(slot, import)` for each poll function the bound runtime defines.
    fn defined_imports(&self) -> impl Iterator<Item = (u32, WasmRuntimeImport)> + '_ {
        POLL_TABLE_IMPORTS
            .iter()
            .filter(|spec| self.runtime_slots.defines_slot(spec.table_slot))
            .map(|spec| (spec.table_slot, spec.import))
    }

    pub(super) fn emit_import_wrappers(
        &self,
        backend: &mut WasmBackend,
        reloc_enabled: bool,
    ) -> BTreeMap<String, u32> {
        let mut table_import_wrappers = BTreeMap::new();
        if !reloc_enabled {
            return table_import_wrappers;
        }

        for (_, import) in self.defined_imports() {
            let import_name = import.name();
            let arity = 1usize;
            let type_idx = backend.types.boxed(arity, true);
            let import_idx = backend.import_ids[import];
            backend.funcs.function(type_idx);
            let func_index = backend.func_count;
            backend.func_count += 1;
            let mut func = Function::new_with_locals_types(Vec::new());
            for idx in 0..arity {
                func.instruction(&Instruction::LocalGet(idx as u32));
            }
            emit_call(&mut func, reloc_enabled, import_idx);
            func.instruction(&Instruction::End);
            backend.codes.function(&func);
            table_import_wrappers.insert(import_name.to_string(), func_index);
        }

        table_import_wrappers
    }

    pub(super) fn initial_table_indices(
        &self,
        table_import_wrappers: &BTreeMap<String, u32>,
        import_ids: &TrackedImportIds,
        sentinel_func_idx: u32,
    ) -> Vec<u32> {
        let mut table_indices = vec![sentinel_func_idx; self.prefix_len as usize];
        for (table_slot, import) in self.defined_imports() {
            let name = import.name();
            let idx = table_import_wrappers.get(name).copied().unwrap_or_else(|| {
                *import_ids
                    .get(import)
                    .unwrap_or_else(|| panic!("missing poll import for {name}"))
            });
            let slot = table_slot as usize;
            *table_indices
                .get_mut(slot)
                .unwrap_or_else(|| panic!("poll table slot {slot} outside poll table prefix")) =
                safe_table_index(idx, sentinel_func_idx);
        }
        debug_assert_eq!(table_indices.len(), self.prefix_len as usize);
        table_indices
    }

    pub(super) fn seed_function_table_slots(&self, func_to_table_idx: &mut BTreeMap<String, u32>) {
        for (_, import) in self.defined_imports() {
            let table_slot = poll_table_import_slot(import).unwrap_or_else(|| {
                panic!("missing generated poll table slot for {}", import.name())
            });
            func_to_table_idx.insert(import.runtime_export_name().to_string(), table_slot);
        }
    }
}

fn safe_table_index(idx: u32, sentinel_func_idx: u32) -> u32 {
    if idx == u32::MAX {
        sentinel_func_idx
    } else {
        idx
    }
}
