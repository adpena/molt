use std::collections::{BTreeMap, BTreeSet};

use wasm_encoder::{Function, Instruction};

use crate::wasm::WasmBackend;
use crate::wasm_abi::{
    RESERVED_RUNTIME_CALLABLE_SPECS, RUNTIME_CALLABLE_IMPORTS, RuntimeCallableResult,
};
use crate::wasm_abi_generated::WasmRuntimeImport;
use crate::wasm_binary::emit_call;
use crate::wasm_values::box_none;

#[derive(Clone)]
pub(super) struct WasmCompactBuiltinRuntimeCallable {
    pub(super) runtime_name: String,
    pub(super) import: WasmRuntimeImport,
    pub(super) arity: usize,
}

struct WasmRuntimeCallableWrapperSpec {
    runtime_name: String,
    import: WasmRuntimeImport,
    arity: usize,
    result: RuntimeCallableResult,
}

pub(super) struct WasmRuntimeCallableTablePlan {
    compact_builtin_runtime_callables: Vec<WasmCompactBuiltinRuntimeCallable>,
    wrapper_specs: Vec<WasmRuntimeCallableWrapperSpec>,
}

impl WasmRuntimeCallableTablePlan {
    pub(super) fn build(builtin_trampoline_specs: &BTreeMap<String, usize>) -> Self {
        let reserved_runtime_callable_names: BTreeSet<&str> = RESERVED_RUNTIME_CALLABLE_SPECS
            .iter()
            .map(|spec| spec.runtime_name)
            .collect();
        let generated_runtime_names: BTreeSet<&str> = RUNTIME_CALLABLE_IMPORTS
            .iter()
            .map(|spec| spec.runtime_name)
            .chain(
                RESERVED_RUNTIME_CALLABLE_SPECS
                    .iter()
                    .map(|spec| spec.runtime_name),
            )
            .collect();

        let compact_builtin_runtime_callables: Vec<WasmCompactBuiltinRuntimeCallable> =
            RUNTIME_CALLABLE_IMPORTS
                .iter()
                .filter(|spec| !reserved_runtime_callable_names.contains(spec.runtime_name))
                .filter(|spec| builtin_trampoline_specs.contains_key(spec.runtime_name))
                .map(|spec| WasmCompactBuiltinRuntimeCallable {
                    runtime_name: spec.runtime_name.to_string(),
                    import: spec.import,
                    arity: spec.arity,
                })
                .collect();

        if builtin_trampoline_specs.len() != compact_builtin_runtime_callables.len() {
            for name in builtin_trampoline_specs.keys() {
                if !generated_runtime_names.contains(name.as_str()) {
                    panic!("builtin {name} missing from generated WASM callable table");
                }
            }
        }

        let wrapper_specs: Vec<WasmRuntimeCallableWrapperSpec> = RUNTIME_CALLABLE_IMPORTS
            .iter()
            .filter(|spec| !reserved_runtime_callable_names.contains(spec.runtime_name))
            .filter(|spec| builtin_trampoline_specs.contains_key(spec.runtime_name))
            .map(|spec| WasmRuntimeCallableWrapperSpec {
                runtime_name: spec.runtime_name.to_string(),
                import: spec.import,
                arity: spec.arity,
                result: spec.result,
            })
            .collect();

        Self {
            compact_builtin_runtime_callables,
            wrapper_specs,
        }
    }

    pub(super) fn compact_builtin_table_len(&self) -> usize {
        self.compact_builtin_runtime_callables.len()
    }

    pub(super) fn compact_builtin_trampoline_count(&self) -> u32 {
        self.compact_builtin_runtime_callables.len() as u32
    }

    pub(super) fn compact_builtin_runtime_callables(&self) -> &[WasmCompactBuiltinRuntimeCallable] {
        &self.compact_builtin_runtime_callables
    }
}

impl WasmBackend {
    pub(super) fn emit_runtime_callable_wrappers(
        &mut self,
        plan: &WasmRuntimeCallableTablePlan,
        user_type_map: &BTreeMap<usize, u32>,
        reloc_enabled: bool,
    ) -> BTreeMap<String, u32> {
        let mut wrapper_indices = BTreeMap::new();
        for spec in &plan.wrapper_specs {
            let type_idx = *user_type_map.get(&spec.arity).unwrap_or_else(|| {
                panic!("missing builtin wrapper signature for arity {}", spec.arity)
            });
            let import_idx = self.import_ids[spec.import];
            self.funcs.function(type_idx);
            let func_index = self.func_count;
            self.func_count += 1;
            let mut func = Function::new_with_locals_types(Vec::new());
            for idx in 0..spec.arity {
                func.instruction(&Instruction::LocalGet(idx as u32));
            }
            emit_call(&mut func, reloc_enabled, import_idx);
            if spec.result == RuntimeCallableResult::Void {
                func.instruction(&Instruction::I64Const(box_none()));
            }
            func.instruction(&Instruction::End);
            self.codes.function(&func);
            wrapper_indices.insert(spec.runtime_name.clone(), func_index);
        }
        wrapper_indices
    }
}
