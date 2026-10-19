use std::collections::BTreeMap;

use wasm_encoder::{ExportKind, Function, Instruction};

use crate::wasm::WasmBackend;
use crate::wasm_abi::CALL_INDIRECT_IMPORTS;
use crate::wasm_binary::emit_call_indirect;
use crate::{FunctionIR, SimpleIR};

pub(super) struct WasmModuleTypeLayout {
    function_type_map: BTreeMap<String, u32>,
}

#[cfg(test)]
mod tests {
    use super::*;
    use wasmparser::{CompositeInnerType, Parser, Payload};

    #[test]
    fn function_layout_does_not_materialize_unobserved_arities() {
        let ir = SimpleIR {
            functions: [0, 512]
                .into_iter()
                .map(|arity| FunctionIR {
                    name: format!("arity_{arity}"),
                    params: (0..arity).map(|i| format!("arg{i}")).collect(),
                    ..Default::default()
                })
                .collect(),
            profile: None,
        };
        let mut backend = WasmBackend::new();
        let layout = WasmModuleTypeLayout::build(&mut backend, &ir);
        let mut module = wasm_encoder::Module::new();
        module.section(&backend.types);
        let bytes = module.finish();
        let mut arities = Vec::new();
        for payload in Parser::new(0).parse_all(&bytes) {
            if let Payload::TypeSection(reader) = payload.unwrap() {
                for group in reader {
                    for ty in group.unwrap().into_types() {
                        let CompositeInnerType::Func(function) = ty.composite_type.inner else {
                            panic!("expected function signature");
                        };
                        arities.push(function.params().len());
                    }
                }
            }
        }
        assert_eq!(arities, [0, 512]);
        for function in &ir.functions {
            assert_eq!(
                arities[layout.type_idx_for_function(function) as usize],
                function.params.len()
            );
        }
    }
}

impl WasmModuleTypeLayout {
    pub(super) fn build(backend: &mut WasmBackend, ir: &SimpleIR) -> Self {
        let mut function_type_map = BTreeMap::new();
        for func_ir in &ir.functions {
            let (arity, returns_value) = if func_ir.is_extern {
                let signature = func_ir.extern_signature().unwrap_or_else(|error| {
                    panic!("invalid WASM extern function declaration: {error}")
                });
                (signature.arity, signature.returns_value)
            } else {
                (func_ir.params.len(), true)
            };
            let type_idx = backend.types.boxed(arity, returns_value);
            if function_type_map
                .insert(func_ir.name.clone(), type_idx)
                .is_some()
            {
                panic!(
                    "duplicate wasm function type assignment for {}",
                    func_ir.name
                );
            }
        }
        Self { function_type_map }
    }

    pub(super) fn type_idx_for_function(&self, func_ir: &FunctionIR) -> u32 {
        *self
            .function_type_map
            .get(&func_ir.name)
            .unwrap_or_else(|| panic!("missing wasm function type for {}", func_ir.name))
    }

    pub(super) fn emit_call_indirect_exports_and_sentinel(
        &self,
        backend: &mut WasmBackend,
        reloc_enabled: bool,
    ) -> u32 {
        for spec in CALL_INDIRECT_IMPORTS {
            let arity = spec.arity;
            let sig_idx = backend.types.boxed(arity + 1, true);
            let callee_idx = backend.types.boxed(arity, true);
            backend.funcs.function(sig_idx);
            backend
                .exports
                .export(spec.import_name, ExportKind::Func, backend.func_count);
            let mut call_indirect = Function::new_with_locals_types(Vec::new());
            for idx in 0..arity {
                call_indirect.instruction(&Instruction::LocalGet((idx + 1) as u32));
            }
            call_indirect.instruction(&Instruction::LocalGet(0));
            call_indirect.instruction(&Instruction::I32WrapI64);
            emit_call_indirect(&mut call_indirect, reloc_enabled, callee_idx, 0);
            call_indirect.instruction(&Instruction::End);
            backend.codes.function(&call_indirect);
            backend.func_count += 1;
        }

        let sentinel_func_idx = backend.func_count;
        let sentinel_type = backend.types.boxed(1, true);
        backend.funcs.function(sentinel_type);
        let mut sentinel = Function::new_with_locals_types(Vec::new());
        sentinel.instruction(&Instruction::Unreachable);
        sentinel.instruction(&Instruction::End);
        backend.codes.function(&sentinel);
        backend.func_count += 1;
        sentinel_func_idx
    }
}
