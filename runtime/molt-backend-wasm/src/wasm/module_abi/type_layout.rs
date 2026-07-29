use std::collections::BTreeMap;

use wasm_encoder::{ExportKind, Function, Instruction, ValType};

use crate::wasm::WasmBackend;
use crate::wasm_abi::{CALL_INDIRECT_IMPORTS, CALL_INDIRECT_MAX_ARITY, TypeSectionExt};
use crate::wasm_binary::emit_call_indirect;
use crate::{FunctionIR, SimpleIR};

pub(super) struct WasmModuleTypeLayout {
    user_type_map: BTreeMap<usize, u32>,
    function_type_map: BTreeMap<String, u32>,
}

impl WasmModuleTypeLayout {
    pub(super) fn build(
        backend: &mut WasmBackend,
        ir: &SimpleIR,
        mut next_type_idx: u32,
        max_func_arity: usize,
        max_call_arity: usize,
    ) -> Self {
        let mut user_type_map = BTreeMap::new();
        for func_ir in &ir.functions {
            if func_ir.name.ends_with("_poll") {
                continue;
            }
            let arity = func_ir.params.len();
            if let std::collections::btree_map::Entry::Vacant(entry) = user_type_map.entry(arity) {
                backend.types.function(
                    std::iter::repeat_n(ValType::I64, arity),
                    std::iter::once(ValType::I64),
                );
                entry.insert(next_type_idx);
                next_type_idx += 1;
            }
        }

        let max_needed_arity = max_func_arity
            .max(max_call_arity.saturating_add(3))
            .max(CALL_INDIRECT_MAX_ARITY + 1);
        for arity in 0..=max_needed_arity {
            if let std::collections::btree_map::Entry::Vacant(entry) = user_type_map.entry(arity) {
                backend.types.function(
                    std::iter::repeat_n(ValType::I64, arity),
                    std::iter::once(ValType::I64),
                );
                entry.insert(next_type_idx);
                next_type_idx += 1;
            }
        }

        let function_type_map = Self::assign_function_type_indices(ir, &user_type_map);

        Self {
            user_type_map,
            function_type_map,
        }
    }

    pub(super) fn user_type_map(&self) -> &BTreeMap<usize, u32> {
        &self.user_type_map
    }

    pub(super) fn type_idx_for_function(&self, func_ir: &FunctionIR) -> u32 {
        *self
            .function_type_map
            .get(&func_ir.name)
            .unwrap_or_else(|| panic!("missing wasm function type for {}", func_ir.name))
    }

    fn assign_function_type_indices(
        ir: &SimpleIR,
        user_type_map: &BTreeMap<usize, u32>,
    ) -> BTreeMap<String, u32> {
        let mut function_type_map = BTreeMap::new();
        for func_ir in &ir.functions {
            let type_idx = Self::function_type_idx(func_ir, user_type_map);
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
        function_type_map
    }

    fn function_type_idx(func_ir: &FunctionIR, user_type_map: &BTreeMap<usize, u32>) -> u32 {
        if func_ir.name.ends_with("_poll") {
            return 2;
        }
        *user_type_map.get(&func_ir.params.len()).unwrap_or_else(|| {
            panic!(
                "missing user wasm signature for arity {}",
                func_ir.params.len()
            )
        })
    }

    pub(super) fn emit_call_indirect_exports_and_sentinel(
        &self,
        backend: &mut WasmBackend,
        reloc_enabled: bool,
    ) -> u32 {
        for spec in CALL_INDIRECT_IMPORTS {
            let arity = spec.arity;
            let sig_idx = *self.user_type_for_arity(arity + 1);
            let callee_idx = *self.user_type_for_arity(arity);
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
        backend.funcs.function(2);
        let mut sentinel = Function::new_with_locals_types(Vec::new());
        sentinel.instruction(&Instruction::Unreachable);
        sentinel.instruction(&Instruction::End);
        backend.codes.function(&sentinel);
        backend.func_count += 1;
        sentinel_func_idx
    }

    fn user_type_for_arity(&self, arity: usize) -> &u32 {
        self.user_type_map
            .get(&arity)
            .unwrap_or_else(|| panic!("missing user wasm signature for arity {arity}"))
    }
}
