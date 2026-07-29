use std::collections::BTreeMap;

use wasm_encoder::EntityType;

use super::type_layout::WasmModuleTypeLayout;
use crate::SimpleIR;
use crate::wasm::WasmBackend;
use crate::wasm_abi::USER_FUNCTION_IMPORT_MODULE;

#[derive(Default)]
pub(super) struct WasmUserFunctionImports {
    function_indices: BTreeMap<String, u32>,
    import_ordinals_by_function_index: BTreeMap<u32, u32>,
}

impl WasmUserFunctionImports {
    pub(super) fn function_index(&self, name: &str) -> Option<u32> {
        self.function_indices.get(name).copied()
    }

    pub(super) fn import_ordinal(&self, function_index: u32) -> Option<u32> {
        self.import_ordinals_by_function_index
            .get(&function_index)
            .copied()
    }
}

impl WasmBackend {
    pub(super) fn emit_user_function_import_surface(
        &mut self,
        ir: &SimpleIR,
        type_layout: &WasmModuleTypeLayout,
    ) -> WasmUserFunctionImports {
        crate::ir::validate_extern_call_abis(ir).unwrap_or_else(|error| panic!("WASM {error}"));
        let mut imports = WasmUserFunctionImports::default();
        let mut import_ordinal = 0u32;
        for function in ir.functions.iter().filter(|function| function.is_extern) {
            let signature = function.extern_signature().unwrap_or_else(|error| {
                panic!("invalid WASM extern function declaration: {error}")
            });
            debug_assert_eq!(signature.arity, function.params.len());
            debug_assert_eq!(signature.execution_context, function.execution_context);

            let function_index = self.func_count;
            self.imports.import(
                USER_FUNCTION_IMPORT_MODULE,
                &function.name,
                EntityType::Function(type_layout.type_idx_for_function(function)),
            );
            if imports
                .function_indices
                .insert(function.name.clone(), function_index)
                .is_some()
            {
                panic!(
                    "duplicate WASM extern function import for {}",
                    function.name
                );
            }
            imports
                .import_ordinals_by_function_index
                .insert(function_index, import_ordinal);
            self.func_count = self
                .func_count
                .checked_add(1)
                .expect("WASM user function import count overflow");
            import_ordinal = import_ordinal
                .checked_add(1)
                .expect("WASM user function import ordinal overflow");
        }
        imports
    }
}
