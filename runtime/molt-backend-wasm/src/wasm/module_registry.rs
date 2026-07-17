use std::borrow::Cow;

use molt_codegen_abi::box_none_bits;
use wasm_encoder::{BlockType, ExportKind, Function, Instruction};

use super::WasmBackend;
use super::module_abi::callable_table::WasmCallableTablePlan;
use crate::wasm_binary::emit_call;

impl WasmBackend {
    pub(super) fn prepare_module_registry_segment(&mut self, reloc_enabled: bool) {
        let Some(registry) = self.module_registry.as_ref() else {
            return;
        };
        let blob = registry.blob.clone();
        self.module_registry_segment = Some(self.add_data_segment(reloc_enabled, &blob));
    }

    /// Emit the app half of the target-neutral ModuleId dispatch contract.
    /// Runtime name resolution and init custody live in ModuleTable; this
    /// function is a dense `br_table` from ModuleId to the compiler body.
    /// No strings, hashing, filesystem probes, or backend-local module list
    /// participate in dispatch.
    pub(super) fn emit_module_registry_dispatch(
        &mut self,
        plan: &WasmCallableTablePlan,
        reloc_enabled: bool,
    ) {
        let Some(registry) = self.module_registry.as_ref() else {
            return;
        };
        let init_rows = registry.init_rows.clone();
        let func_index = self.func_count;
        // Static type 2 is (i64) -> i64, matching the runtime env import.
        self.funcs.function(2);
        self.func_count += 1;
        self.exports
            .export("molt_isolate_import", ExportKind::Func, func_index);

        let mut func = Function::new_with_locals_types(Vec::new());
        // The outer empty block is the default/missing-id destination.  It is
        // distinct from the implicit function block, whose i64 result would
        // otherwise require the br_table edge to carry a value.
        func.instruction(&Instruction::Block(BlockType::Empty));
        for _ in &init_rows {
            func.instruction(&Instruction::Block(BlockType::Empty));
        }
        let default_depth = init_rows.len() as u32;
        let max_id = init_rows.last().map(|(id, _)| *id).unwrap_or(0);
        let mut targets = vec![default_depth; max_id.saturating_add(1) as usize];
        for (ordinal, (id, _)) in init_rows.iter().enumerate() {
            targets[*id as usize] = ordinal as u32;
        }
        func.instruction(&Instruction::LocalGet(0));
        func.instruction(&Instruction::I32WrapI64);
        func.instruction(&Instruction::BrTable(Cow::Owned(targets), default_depth));

        for (_, symbol) in &init_rows {
            func.instruction(&Instruction::End);
            let target = plan
                .function_index(symbol)
                .unwrap_or_else(|| panic!("module catalog initializer {symbol:?} was not emitted"));
            emit_call(&mut func, reloc_enabled, target);
            func.instruction(&Instruction::Drop);
            func.instruction(&Instruction::I64Const(box_none_bits()));
            func.instruction(&Instruction::Return);
        }
        func.instruction(&Instruction::End);
        func.instruction(&Instruction::I64Const(box_none_bits()));
        func.instruction(&Instruction::End);
        self.codes.function(&func);
    }
}
