use crate::wasm_abi::{
    CALL_INDIRECT_IMPORTS, NATIVE_CALLABLE_IMPORT_MODULE, RUNTIME_IMPORT_MODULE,
    USER_FUNCTION_IMPORT_MODULE, wasm_runtime_export_name,
};
use crate::wasm_abi_generated::{WasmRuntimeImport, wasm_runtime_import};
use crate::wasm_data::DataSegmentInfo;
use crate::wasm_table::WasmFunctionSymbol;

use std::collections::BTreeMap;
use wasm_encoder::{DataSymbolDefinition, SymbolTable};

use super::scan::RelocScan;
use super::types::FunctionImport;

pub(super) struct SymbolMaps {
    pub(super) sym_tab: SymbolTable,
    pub(super) func_symbol_map: Vec<u32>,
    pub(super) data_symbol_map: Vec<u32>,
    pub(super) runtime_import_symbol_map: BTreeMap<WasmRuntimeImport, u32>,
    pub(super) user_import_symbol_map: Vec<u32>,
    pub(super) defined_func_symbol_map: Vec<u32>,
}

impl SymbolMaps {
    pub(super) fn function_symbol(&self, target: &WasmFunctionSymbol) -> Option<u32> {
        match target {
            WasmFunctionSymbol::Defined { defined_func_index } => self
                .defined_func_symbol_map
                .get(*defined_func_index as usize)
                .copied(),
            WasmFunctionSymbol::RuntimeImport(import) => {
                self.runtime_import_symbol_map.get(import).copied()
            }
            WasmFunctionSymbol::UserImport {
                user_import_ordinal,
            } => self
                .user_import_symbol_map
                .get(*user_import_ordinal as usize)
                .copied(),
        }
    }
}

pub(super) fn is_manifest_call_indirect_import_name(name: &str) -> bool {
    CALL_INDIRECT_IMPORTS
        .iter()
        .any(|spec| spec.import_name == name)
}

fn linker_symbol_name_for_function_import(import: &FunctionImport) -> String {
    match import.module.as_str() {
        RUNTIME_IMPORT_MODULE => wasm_runtime_export_name(&import.name)
            .unwrap_or_else(|| {
                panic!(
                    "missing generated runtime export for import {}",
                    import.name
                )
            })
            .to_string(),
        NATIVE_CALLABLE_IMPORT_MODULE => import.name.clone(),
        USER_FUNCTION_IMPORT_MODULE => import.name.clone(),
        module => panic!(
            "unsupported WASM function import module `{module}` for import `{}`; relocatable output only supports `{RUNTIME_IMPORT_MODULE}` runtime ABI imports, `{NATIVE_CALLABLE_IMPORT_MODULE}` native callable object imports, and `{USER_FUNCTION_IMPORT_MODULE}` user-function imports",
            import.name
        ),
    }
}

pub(super) fn build_symbol_maps(scan: &RelocScan, data_segments: &[DataSegmentInfo]) -> SymbolMaps {
    let total_funcs = scan.func_import_count + scan.defined_func_count;
    let mut func_symbol_map = vec![0u32; total_funcs as usize];
    let mut data_symbol_map = vec![0u32; data_segments.len()];
    let mut runtime_import_symbol_map = BTreeMap::new();
    let mut user_import_symbol_map = Vec::new();
    let mut defined_func_symbol_map = vec![0u32; scan.defined_func_count as usize];
    let mut symbol_index = 0u32;

    let mut sym_tab = SymbolTable::new();
    for (idx, import) in scan.func_imports.iter().enumerate() {
        let flags = SymbolTable::WASM_SYM_UNDEFINED | SymbolTable::WASM_SYM_EXPLICIT_NAME;
        let symbol_name = linker_symbol_name_for_function_import(import);
        sym_tab.function(flags, idx as u32, Some(&symbol_name));
        func_symbol_map[idx] = symbol_index;
        if import.module == RUNTIME_IMPORT_MODULE {
            let import_id = wasm_runtime_import(&import.name).unwrap_or_else(|| {
                panic!(
                    "missing generated runtime import identity for {}",
                    import.name
                )
            });
            runtime_import_symbol_map.insert(import_id, symbol_index);
        } else if import.module == USER_FUNCTION_IMPORT_MODULE {
            user_import_symbol_map.push(symbol_index);
        }
        symbol_index += 1;
    }
    for def_idx in 0..scan.defined_func_count {
        let func_index = scan.func_import_count + def_idx;
        let export_name = scan.func_exports.get(&func_index).cloned();
        // Keep linker symbol names module-scoped so linked output/runtime objects
        // cannot accidentally alias local function symbols with identical names.
        // Preserve explicit call_indirect export symbols because wasm_link.py
        // resolves/aliases those by name for runtime ABI wiring.
        let name = match export_name.as_deref() {
            Some("molt_host_init") | Some("molt_main") => export_name.clone().unwrap_or_default(),
            Some(exported) if is_manifest_call_indirect_import_name(exported) => {
                exported.to_string()
            }
            Some(_) => format!("__molt_output_export_{func_index}"),
            None => format!("__molt_output_fn_{func_index}"),
        };
        let flags = if export_name.is_some() {
            SymbolTable::WASM_SYM_EXPORTED | SymbolTable::WASM_SYM_NO_STRIP
        } else {
            0
        };
        sym_tab.function(flags, func_index, Some(&name));
        func_symbol_map[func_index as usize] = symbol_index;
        defined_func_symbol_map[def_idx as usize] = symbol_index;
        symbol_index += 1;
    }

    for table_idx in 0..scan.table_import_count {
        let flags = SymbolTable::WASM_SYM_UNDEFINED | SymbolTable::WASM_SYM_NO_STRIP;
        sym_tab.table(flags, table_idx, None);
        symbol_index += 1;
    }
    for table_idx in 0..scan.table_defined_count {
        let index = scan.table_import_count + table_idx;
        let name = format!("__molt_output_table_{index}");
        sym_tab.table(0, index, Some(&name));
        symbol_index += 1;
    }

    for (idx, info) in data_segments.iter().enumerate() {
        let name = format!("__molt_output_data_{idx}");
        sym_tab.data(
            0,
            &name,
            Some(DataSymbolDefinition {
                index: idx as u32,
                offset: 0,
                size: info.size,
            }),
        );
        data_symbol_map[idx] = symbol_index;
        symbol_index += 1;
    }

    SymbolMaps {
        sym_tab,
        func_symbol_map,
        data_symbol_map,
        runtime_import_symbol_map,
        user_import_symbol_map,
        defined_func_symbol_map,
    }
}
