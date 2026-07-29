use wasm_encoder::{Function, Instruction};

mod task;

use task::{emit_task_trampoline, task_trampoline_local_types};

use crate::wasm::WasmBackend;
use crate::wasm_binary::emit_call;
use crate::wasm_table::WasmCallableTableTarget;
use crate::{TrampolineBehavior, TrampolineSpec};

impl WasmBackend {
    pub(super) fn compile_trampoline(
        &mut self,
        reloc_enabled: bool,
        target_func_index: u32,
        table_target: WasmCallableTableTarget,
        spec: TrampolineSpec,
    ) {
        let TrampolineSpec {
            arity,
            has_closure,
            kind,
            closure_size,
            target_has_ret: _,
        } = spec;
        let behavior = kind.behavior();
        let func_index = self.func_count;
        self.funcs.function(5);
        self.func_count += 1;
        let mut local_types = Vec::new();
        if let TrampolineBehavior::Task(task_kind) = behavior {
            let task_local_types = task_trampoline_local_types(task_kind);
            local_types.extend(task_local_types);
        }
        let mut func = Function::new_with_locals_types(local_types);
        match behavior {
            TrampolineBehavior::ForwardCallFrame => {
                func.instruction(&Instruction::LocalGet(0));
                func.instruction(&Instruction::LocalGet(1));
                func.instruction(&Instruction::LocalGet(2));
                emit_call(&mut func, reloc_enabled, target_func_index);
            }
            TrampolineBehavior::Task(task_kind) => emit_task_trampoline(
                self,
                &mut func,
                reloc_enabled,
                func_index,
                &table_target,
                task_kind,
                arity,
                has_closure,
                closure_size,
            ),
            TrampolineBehavior::UnpackArgs => {
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
            }
        }
        func.instruction(&Instruction::End);
        self.codes.function(&func);
    }
}
