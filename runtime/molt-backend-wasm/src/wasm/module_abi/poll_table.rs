use std::collections::BTreeMap;

use wasm_encoder::{Function, Instruction};

use crate::wasm::WasmBackend;
use crate::wasm_abi::{POLL_TABLE_IMPORTS, poll_table_import_slot};
use crate::wasm_binary::emit_call;
use crate::wasm_import_tracking::TrackedImportIds;

pub(super) struct WasmPollTableLayout {
    prefix_len: u32,
}

impl WasmPollTableLayout {
    pub(super) fn build() -> Self {
        let prefix_len = POLL_TABLE_IMPORTS
            .iter()
            .map(|spec| spec.table_slot)
            .max()
            .unwrap_or(0)
            + 1;
        Self { prefix_len }
    }

    pub(super) fn prefix_len(&self) -> u32 {
        self.prefix_len
    }

    pub(super) fn emit_import_wrappers(
        &self,
        backend: &mut WasmBackend,
        reloc_enabled: bool,
        user_type_map: &BTreeMap<usize, u32>,
    ) -> BTreeMap<String, u32> {
        let mut table_import_wrappers = BTreeMap::new();
        if !reloc_enabled {
            return table_import_wrappers;
        }

        for spec in POLL_TABLE_IMPORTS {
            let import_name = spec.import_name;
            let arity = 1usize;
            let type_idx = *user_type_map
                .get(&arity)
                .unwrap_or_else(|| panic!("missing wrapper signature for arity {arity}"));
            let import_idx = *backend
                .import_ids
                .get(import_name)
                .unwrap_or_else(|| panic!("missing import for {import_name}"));
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
        for spec in POLL_TABLE_IMPORTS {
            let name = spec.import_name;
            let idx = *table_import_wrappers.get(name).unwrap_or(&import_ids[name]);
            let slot = spec.table_slot as usize;
            *table_indices
                .get_mut(slot)
                .unwrap_or_else(|| panic!("poll table slot {slot} outside poll table prefix")) =
                safe_table_index(idx, sentinel_func_idx);
        }
        debug_assert_eq!(table_indices.len(), self.prefix_len as usize);
        table_indices
    }

    pub(super) fn seed_function_table_slots(&self, func_to_table_idx: &mut BTreeMap<String, u32>) {
        for spec in POLL_TABLE_IMPORTS {
            let table_slot = poll_table_import_slot(spec.import_name).unwrap_or_else(|| {
                panic!("missing generated poll table slot for {}", spec.import_name)
            });
            func_to_table_idx.insert(format!("molt_{}", spec.import_name), table_slot);
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
