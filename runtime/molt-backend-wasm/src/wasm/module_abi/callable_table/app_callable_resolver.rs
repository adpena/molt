use wasm_encoder::{BlockType, Function, Instruction, MemArg, ValType};

use super::{WasmAppCallableResolverEntry, WasmCallableTablePlan};
use crate::wasm::WasmBackend;
use crate::wasm_abi::static_func_type_idx;

impl WasmBackend {
    pub(in crate::wasm::module_abi) fn emit_app_callable_resolver(
        &mut self,
        plan: &WasmCallableTablePlan,
        reloc_enabled: bool,
    ) {
        let Some(resolver) = plan.app_callable_resolver.as_ref() else {
            return;
        };
        if self.func_count != resolver.resolver_func_index {
            panic!(
                "wasm app callable resolver index mismatch: expected {}, got {}",
                resolver.resolver_func_index, self.func_count
            );
        }
        let type_idx = static_func_type_idx(&[ValType::I32, ValType::I32], &[ValType::I64])
            .unwrap_or_else(|| {
                panic!("WASM ABI static types missing app callable resolver signature")
            });
        self.funcs.function(type_idx);
        self.func_count += 1;
        let candidate_ptr_local = 2;
        let mut func = Function::new_with_locals_types(vec![ValType::I32]);
        for entry in &resolver.entries {
            emit_resolver_entry(
                self,
                &mut func,
                reloc_enabled,
                resolver.resolver_func_index,
                candidate_ptr_local,
                entry,
            );
        }
        func.instruction(&Instruction::I64Const(0));
        func.instruction(&Instruction::End);
        self.codes.function(&func);
    }
}

fn emit_resolver_entry(
    backend: &mut WasmBackend,
    func: &mut Function,
    reloc_enabled: bool,
    resolver_func_index: u32,
    candidate_ptr_local: u32,
    entry: &WasmAppCallableResolverEntry,
) {
    let name_bytes = entry.name.as_bytes();
    let name_segment = backend.add_data_segment(reloc_enabled, name_bytes);
    backend.emit_data_ptr_i32(reloc_enabled, resolver_func_index, func, name_segment);
    func.instruction(&Instruction::LocalSet(candidate_ptr_local));
    func.instruction(&Instruction::LocalGet(1));
    func.instruction(&Instruction::I32Const(name_bytes.len() as i32));
    func.instruction(&Instruction::I32Eq);
    func.instruction(&Instruction::If(BlockType::Empty));
    func.instruction(&Instruction::I32Const(1));
    for offset in 0..name_bytes.len() {
        func.instruction(&Instruction::LocalGet(0));
        func.instruction(&Instruction::I32Load8U(MemArg {
            align: 0,
            offset: offset as u64,
            memory_index: 0,
        }));
        func.instruction(&Instruction::LocalGet(candidate_ptr_local));
        func.instruction(&Instruction::I32Load8U(MemArg {
            align: 0,
            offset: offset as u64,
            memory_index: 0,
        }));
        func.instruction(&Instruction::I32Eq);
        func.instruction(&Instruction::I32And);
    }
    func.instruction(&Instruction::If(BlockType::Empty));
    backend.table_relocations.emit_i64(
        reloc_enabled,
        backend.func_import_count,
        resolver_func_index,
        func,
        &entry.target,
    );
    func.instruction(&Instruction::Return);
    func.instruction(&Instruction::End);
    func.instruction(&Instruction::End);
}
