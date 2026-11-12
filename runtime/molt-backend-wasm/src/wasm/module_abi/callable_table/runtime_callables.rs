use std::collections::{BTreeMap, BTreeSet};

use molt_ir::runtime_callable_abi_generated::RuntimeCallableTrampolineAbi;
use crate::wasm_binary::Function;
use wasm_encoder::Instruction;

use crate::wasm::WasmBackend;
use crate::wasm_abi::{RESERVED_RUNTIME_CALLABLE_SPECS, RUNTIME_CALLABLE_IMPORTS};
use crate::wasm_abi_generated::WasmRuntimeImport;
use crate::wasm_binary::emit_call;

#[derive(Clone)]
pub(super) struct WasmCompactBuiltinRuntimeCallable {
    pub(super) runtime_name: String,
    pub(super) import: WasmRuntimeImport,
    pub(super) arity: usize,
}

/// A reserved runtime callable that a `builtin_func` names. Its identity is
/// its key (HF-699 K1). Its trampoline follows native's typed trampoline for
/// the same call shape: an `UnpackArgs` callable gets an app trampoline that
/// unpacks the argument vector and calls the runtime import; a `CallFrame`
/// callable consumes `(closure, argv, argc)` itself, so its key is also its
/// trampoline.
#[derive(Clone)]
pub(super) struct WasmKeyedRuntimeCallable {
    pub(super) runtime_name: String,
    pub(super) key: u64,
    pub(super) import: WasmRuntimeImport,
    pub(super) arity: usize,
    pub(super) trampoline_abi: RuntimeCallableTrampolineAbi,
}

impl WasmKeyedRuntimeCallable {
    pub(super) fn needs_app_trampoline(&self) -> bool {
        self.trampoline_abi == RuntimeCallableTrampolineAbi::UnpackArgs
    }
}

pub(super) struct WasmRuntimeCallableTablePlan {
    compact_builtin_runtime_callables: Vec<WasmCompactBuiltinRuntimeCallable>,
    keyed_runtime_callables: Vec<WasmKeyedRuntimeCallable>,
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
            .chain(reserved_runtime_callable_names.iter().copied())
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
        let keyed_runtime_callables: Vec<WasmKeyedRuntimeCallable> =
            RESERVED_RUNTIME_CALLABLE_SPECS
                .iter()
                .filter(|spec| builtin_trampoline_specs.contains_key(spec.runtime_name))
                .map(|spec| WasmKeyedRuntimeCallable {
                    runtime_name: spec.runtime_name.to_string(),
                    key: spec.key(),
                    import: spec.import,
                    arity: spec.arity,
                    trampoline_abi: spec.trampoline_abi,
                })
                .collect();

        if builtin_trampoline_specs.len()
            != compact_builtin_runtime_callables.len() + keyed_runtime_callables.len()
        {
            for name in builtin_trampoline_specs.keys() {
                if !generated_runtime_names.contains(name.as_str()) {
                    panic!("builtin {name} missing from generated WASM callable table");
                }
            }
        }

        Self {
            compact_builtin_runtime_callables,
            keyed_runtime_callables,
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

    pub(super) fn keyed_runtime_callables(&self) -> &[WasmKeyedRuntimeCallable] {
        &self.keyed_runtime_callables
    }

    /// App trampolines of keyed `UnpackArgs` callables, one table slot and one
    /// defined function each.
    pub(super) fn keyed_trampoline_count(&self) -> usize {
        self.keyed_runtime_callables
            .iter()
            .filter(|callable| callable.needs_app_trampoline())
            .count()
    }
}

impl WasmBackend {
    pub(super) fn emit_runtime_callable_wrappers(
        &mut self,
        plan: &WasmRuntimeCallableTablePlan,
        reloc_enabled: bool,
    ) -> BTreeMap<String, u32> {
        let mut wrapper_indices = BTreeMap::new();
        for spec in &plan.compact_builtin_runtime_callables {
            let type_idx = self.types.boxed(spec.arity + 1, true);
            let import_idx = self.import_ids[spec.import];
            self.funcs.function(type_idx);
            let func_index = self.func_count;
            std::rc::Rc::make_mut(&mut self.execution_call_abis)
                .register_attached(func_index, &vec![wasm_encoder::ValType::I64; spec.arity]);
            self.func_count += 1;
            let mut func = Function::new_with_locals_types(Vec::new());
            func.attach_context(spec.arity + 1, self.execution_call_abis.clone());
            for idx in 0..spec.arity {
                func.instruction(&Instruction::LocalGet(idx as u32 + 1));
            }
            emit_call(&mut func, reloc_enabled, import_idx);
            func.instruction(&Instruction::End);
            self.codes.function(&func);
            wrapper_indices.insert(spec.runtime_name.clone(), func_index);
        }
        wrapper_indices
    }
}
