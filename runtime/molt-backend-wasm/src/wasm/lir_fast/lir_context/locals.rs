use super::LirLowerCtx;
use super::repr::lir_repr_to_val;
use molt_tir::tir::lir::LirValue;
use molt_tir::tir::values::ValueId;
use wasm_encoder::{Instruction, ValType};

impl LirLowerCtx<'_> {
    pub(in crate::wasm::lir_fast) fn local_for(&mut self, value: &LirValue) -> u32 {
        if let Some(&idx) = self.value_locals.get(&value.id) {
            return idx;
        }
        let idx = self.next_local;
        self.next_local += 1;
        self.value_locals.insert(value.id, idx);
        self.value_reprs.insert(value.id, value.repr);
        self.value_types.insert(value.id, value.ty.clone());
        self.local_types.insert(idx, lir_repr_to_val(value.repr));
        idx
    }

    pub(in crate::wasm::lir_fast) fn allocate_function_locals(&mut self) {
        if let Some(entry) = self.func.blocks.get(&self.func.entry_block) {
            for arg in &entry.args {
                self.local_for(arg);
            }
        }
        for &bid in &self.rpo.clone() {
            if let Some(block) = self.func.blocks.get(&bid) {
                for arg in &block.args {
                    self.local_for(arg);
                }
                for op in &block.ops {
                    for value in &op.result_values {
                        self.local_for(value);
                    }
                }
            }
        }
    }

    pub(in crate::wasm::lir_fast) fn local_declarations_after(
        &self,
        first_local: u32,
    ) -> Vec<ValType> {
        let mut locals = Vec::with_capacity(self.next_local.saturating_sub(first_local) as usize);
        for idx in first_local..self.next_local {
            locals.push(self.local_types.get(&idx).copied().unwrap_or(ValType::I64));
        }
        locals
    }

    pub(in crate::wasm::lir_fast) fn get_local(&self, vid: ValueId) -> u32 {
        self.value_locals[&vid]
    }

    pub(in crate::wasm::lir_fast) fn emit_get(&mut self, vid: ValueId) {
        self.instructions
            .push(Instruction::LocalGet(self.get_local(vid)));
    }

    pub(in crate::wasm::lir_fast) fn emit_set(&mut self, vid: ValueId) {
        self.instructions
            .push(Instruction::LocalSet(self.get_local(vid)));
    }

    pub(in crate::wasm::lir_fast) fn alloc_scratch_local(&mut self, val_type: ValType) -> u32 {
        let idx = self.next_local;
        self.next_local += 1;
        self.local_types.insert(idx, val_type);
        idx
    }
}
