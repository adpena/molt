use crate::wasm_binary::Function;
use wasm_encoder::Instruction;

mod task;

use task::{emit_task_trampoline, task_trampoline_local_types};

use crate::wasm::WasmBackend;
use crate::wasm_binary::emit_call;
use crate::wasm_table::WasmCallableTableSlot;
use crate::wasm_values::emit_boxed_none;
use crate::{TrampolineBehavior, TrampolineSpec};

impl WasmBackend {
    pub(super) fn compile_trampoline(
        &mut self,
        reloc_enabled: bool,
        target_func_index: u32,
        table_target: WasmCallableTableSlot,
        spec: TrampolineSpec,
    ) {
        let TrampolineSpec {
            arity,
            has_closure,
            kind,
            closure_size,
            target_has_ret,
        } = spec;
        let behavior = kind.behavior();
        let func_index = self.func_count;
        let type_idx = self.types.boxed(4, true);
        self.funcs.function(type_idx);
        self.func_count += 1;
        let mut local_types = Vec::new();
        if let TrampolineBehavior::Task(task_kind) = behavior {
            let task_local_types = task_trampoline_local_types(task_kind);
            local_types.extend(task_local_types);
        }
        let refcount = matches!(behavior, TrampolineBehavior::Task(_)).then(|| {
            crate::wasm::refcount::WasmRefcountScratch::allocate(|ty| {
                let slot = 4 + local_types.len() as u32;
                local_types.push(ty);
                slot
            })
        });
        let mut func = Function::new_with_locals_types(local_types);
        func.attach_context(4, self.execution_call_abis.clone());
        if let Some(scratch) = refcount {
            scratch.initialize(&mut func, &self.import_ids, reloc_enabled);
        }
        match behavior {
            TrampolineBehavior::ForwardCallFrame => {
                func.instruction(&Instruction::LocalGet(1));
                func.instruction(&Instruction::LocalGet(2));
                func.instruction(&Instruction::LocalGet(3));
                emit_call(&mut func, reloc_enabled, target_func_index);
                if !target_has_ret {
                    emit_boxed_none(&mut func);
                }
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
                refcount.expect("task refcount admission"),
            ),
            TrampolineBehavior::UnpackArgs => {
                if has_closure {
                    func.instruction(&Instruction::LocalGet(1));
                }
                for idx in 0..arity {
                    func.instruction(&Instruction::LocalGet(2));
                    func.instruction(&Instruction::I32WrapI64);
                    func.instruction(&Instruction::I64Load(wasm_encoder::MemArg {
                        align: 3,
                        offset: (idx * std::mem::size_of::<u64>()) as u64,
                        memory_index: 0,
                    }));
                }
                emit_call(&mut func, reloc_enabled, target_func_index);
                if !target_has_ret {
                    emit_boxed_none(&mut func);
                }
            }
        }
        func.instruction(&Instruction::End);
        self.codes.function(&func);
    }
}
