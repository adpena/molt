use std::collections::BTreeMap;

use wasm_encoder::{DataSymbolDefinition, SymbolTable};

use crate::wasm_abi::{
    CALL_INDIRECT_IMPORTS, NATIVE_CALLABLE_IMPORT_MODULE, RUNTIME_IMPORT_MODULE,
    wasm_runtime_export_name,
};
use crate::wasm_data::DataSegmentInfo;

#[derive(Clone, Debug)]
pub(super) struct FunctionImport {
    pub(super) module: String,
    pub(super) name: String,
}

pub(super) struct RelocSymbolTable {
    pub(super) table: SymbolTable,
    pub(super) function_symbols: Vec<u32>,
    pub(super) data_symbols: Vec<u32>,
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
        module => panic!(
            "unsupported WASM function import module `{module}` for import `{}`; relocatable output only supports `{RUNTIME_IMPORT_MODULE}` runtime ABI imports and `{NATIVE_CALLABLE_IMPORT_MODULE}` native callable object imports",
            import.name
        ),
    }
}

pub(super) fn build_reloc_symbol_table(
    func_imports: &[FunctionImport],
    func_exports: &BTreeMap<u32, String>,
    func_import_count: u32,
    defined_func_count: u32,
    table_import_count: u32,
    table_defined_count: u32,
    data_segments: &[DataSegmentInfo],
) -> RelocSymbolTable {
    let total_funcs = func_import_count + defined_func_count;
    let mut function_symbols = vec![0u32; total_funcs as usize];
    let mut data_symbols = vec![0u32; data_segments.len()];
    let mut symbol_index = 0u32;

    let mut table = SymbolTable::new();
    let mut import_names: Vec<String> = Vec::new();
    for (idx, import) in func_imports.iter().enumerate() {
        let flags = SymbolTable::WASM_SYM_UNDEFINED | SymbolTable::WASM_SYM_EXPLICIT_NAME;
        let symbol_name = linker_symbol_name_for_function_import(import);
        import_names.push(symbol_name);
        let name_ref = import_names.last().unwrap();
        table.function(flags, idx as u32, Some(name_ref));
        function_symbols[idx] = symbol_index;
        symbol_index += 1;
    }

    let mut func_names: Vec<String> = Vec::new();
    for def_idx in 0..defined_func_count {
        let func_index = func_import_count + def_idx;
        let export_name = func_exports.get(&func_index).cloned();
        // Keep linker symbol names module-scoped so linked output/runtime objects
        // cannot accidentally alias local function symbols with identical names.
        // Preserve explicit call_indirect export symbols because wasm_link.py
        // resolves/aliases those by name for runtime ABI wiring.
        let name = match export_name.as_deref() {
            Some("molt_host_init") | Some("molt_main") | Some("molt_table_init") => {
                export_name.clone().unwrap_or_default()
            }
            Some(exported) if is_manifest_call_indirect_import_name(exported) => {
                exported.to_string()
            }
            Some(_) => format!("__molt_output_export_{func_index}"),
            None => format!("__molt_output_fn_{func_index}"),
        };
        func_names.push(name);
        let name_ref = func_names.last().unwrap();
        let flags = if export_name.is_some() {
            SymbolTable::WASM_SYM_EXPORTED | SymbolTable::WASM_SYM_NO_STRIP
        } else {
            0
        };
        table.function(flags, func_index, Some(name_ref));
        function_symbols[func_index as usize] = symbol_index;
        symbol_index += 1;
    }

    for table_idx in 0..table_import_count {
        let flags = SymbolTable::WASM_SYM_UNDEFINED | SymbolTable::WASM_SYM_NO_STRIP;
        table.table(flags, table_idx, None);
        symbol_index += 1;
    }

    let mut table_names: Vec<String> = Vec::new();
    for table_idx in 0..table_defined_count {
        let index = table_import_count + table_idx;
        let name = format!("__molt_output_table_{index}");
        table_names.push(name);
        let name_ref = table_names.last().unwrap();
        table.table(0, index, Some(name_ref));
        symbol_index += 1;
    }

    let mut data_names: Vec<String> = Vec::new();
    for (idx, info) in data_segments.iter().enumerate() {
        let name = format!("__molt_output_data_{idx}");
        data_names.push(name);
        let name_ref = data_names.last().unwrap();
        table.data(
            0,
            name_ref,
            Some(DataSymbolDefinition {
                index: idx as u32,
                offset: 0,
                size: info.size,
            }),
        );
        data_symbols[idx] = symbol_index;
        symbol_index += 1;
    }

    RelocSymbolTable {
        table,
        function_symbols,
        data_symbols,
    }
}
