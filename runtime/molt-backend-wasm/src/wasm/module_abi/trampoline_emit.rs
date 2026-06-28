use wasm_encoder::{Function, Instruction, ValType};

mod task;

use task::{emit_task_trampoline, task_trampoline_local_types};

use crate::wasm::WasmBackend;
use crate::wasm_binary::emit_call;
use crate::wasm_values::box_int;
use crate::{TrampolineKind, TrampolineSpec};

impl WasmBackend {
    pub(super) fn compile_trampoline(
        &mut self,
        reloc_enabled: bool,
        target_func_index: u32,
        table_idx: u32,
        spec: TrampolineSpec,
        multi_return_count: Option<usize>,
    ) {
        let TrampolineSpec {
            arity,
            has_closure,
            kind,
            closure_size,
            target_has_ret: _,
        } = spec;
        self.funcs.function(5);
        self.func_count += 1;
        let mut local_types = Vec::new();
        if let Some(task_local_types) = task_trampoline_local_types(kind) {
            local_types.extend(task_local_types);
        }
        // For multi-value return trampolines (Plain kind only): allocate
        // N temp locals for the return values + 1 local for the tuple builder.
        // Params occupy locals 0..=2, so extra locals start at index 3.
        let mr_locals_start: u32 = 3 + local_types.len() as u32;
        if let (Some(ret_count), TrampolineKind::Plain) = (multi_return_count, &kind) {
            // N temp locals for storing each return value
            for _ in 0..ret_count {
                local_types.push(ValType::I64);
            }
            // 1 local for the tuple builder handle
            local_types.push(ValType::I64);
            let _ = ret_count; // suppress unused warning
        }
        let mut func = Function::new_with_locals_types(local_types);
        if emit_task_trampoline(
            self,
            &mut func,
            reloc_enabled,
            table_idx,
            kind,
            arity,
            has_closure,
            closure_size,
        ) {
            self.codes.function(&func);
            return;
        }
        if has_closure {
            func.instruction(&Instruction::LocalGet(0));
        }
        for idx in 0..arity {
            func.instruction(&Instruction::LocalGet(1));
            func.instruction(&Instruction::I32WrapI64);
            func.instruction(&Instruction::I64Load(wasm_encoder::MemArg {
                align: 3,
                offset: (idx * std::mem::size_of::<u64>()) as u64,
                memory_index: 0,
            }));
        }
        emit_call(&mut func, reloc_enabled, target_func_index);
        if let Some(ret_count) = multi_return_count {
            // The target function pushed `ret_count` i64 values onto the
            // stack.  Pop them into temp locals (last return value is on
            // top, so store in reverse order) then reconstruct a tuple.
            let builder_local = mr_locals_start + ret_count as u32;
            for i in (0..ret_count).rev() {
                func.instruction(&Instruction::LocalSet(mr_locals_start + i as u32));
            }
            // list_builder_new(count) -> builder handle
            func.instruction(&Instruction::I64Const(box_int(ret_count as i64)));
            emit_call(
                &mut func,
                reloc_enabled,
                self.import_ids["list_builder_new"],
            );
            func.instruction(&Instruction::LocalSet(builder_local));
            // list_builder_append(builder, value) for each value in order
            for i in 0..ret_count {
                func.instruction(&Instruction::LocalGet(builder_local));
                func.instruction(&Instruction::LocalGet(mr_locals_start + i as u32));
                emit_call(
                    &mut func,
                    reloc_enabled,
                    self.import_ids["list_builder_append"],
                );
            }
            // tuple_builder_finish(builder) -> tuple handle (single i64)
            func.instruction(&Instruction::LocalGet(builder_local));
            emit_call(
                &mut func,
                reloc_enabled,
                self.import_ids["tuple_builder_finish"],
            );
        }
        func.instruction(&Instruction::End);
        self.codes.function(&func);
    }
}
