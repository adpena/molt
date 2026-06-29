use wasm_encoder::{Function, Instruction, ValType};

use crate::TrampolineKind;
use crate::wasm::WasmBackend;
use crate::wasm_abi::{GEN_CONTROL_SIZE, TASK_KIND_COROUTINE, TASK_KIND_GENERATOR};
use crate::wasm_binary::{emit_call, emit_table_index_i64};

const TASK_LOCAL: u32 = 3;
const BASE_LOCAL: u32 = 4;
const VAL_LOCAL: u32 = 5;
const ARGS_BASE_LOCAL: u32 = 6;

pub(super) fn task_trampoline_local_types(kind: TrampolineKind) -> Option<[ValType; 4]> {
    TaskTrampolineLayout::for_kind(kind)
        .map(|_| [ValType::I64, ValType::I32, ValType::I64, ValType::I32])
}

pub(super) fn emit_task_trampoline(
    backend: &mut WasmBackend,
    func: &mut Function,
    reloc_enabled: bool,
    table_idx: u32,
    kind: TrampolineKind,
    arity: usize,
    has_closure: bool,
    closure_size: i64,
) -> bool {
    let Some(layout) = TaskTrampolineLayout::for_kind(kind) else {
        return false;
    };
    layout.validate_closure_size(closure_size, arity, has_closure);

    emit_table_index_i64(func, reloc_enabled, table_idx);
    func.instruction(&Instruction::I64Const(closure_size));
    func.instruction(&Instruction::I64Const(layout.runtime_task_kind));
    emit_call(
        func,
        reloc_enabled,
        backend.import_ids[crate::wasm_abi_generated::WasmRuntimeImport::TaskNew],
    );
    func.instruction(&Instruction::LocalSet(TASK_LOCAL));

    emit_payload_slots(
        backend,
        func,
        reloc_enabled,
        layout.payload_base_offset,
        arity,
        has_closure,
    );
    layout.emit_completion(backend, func, reloc_enabled);
    true
}

struct TaskTrampolineLayout {
    diagnostic_name: &'static str,
    runtime_task_kind: i64,
    payload_base_offset: i32,
    completion: TaskTrampolineCompletion,
}

impl TaskTrampolineLayout {
    fn for_kind(kind: TrampolineKind) -> Option<Self> {
        match kind {
            TrampolineKind::Generator => Some(Self {
                diagnostic_name: "generator",
                runtime_task_kind: TASK_KIND_GENERATOR,
                payload_base_offset: GEN_CONTROL_SIZE,
                completion: TaskTrampolineCompletion::ReturnTask,
            }),
            TrampolineKind::Coroutine => Some(Self {
                diagnostic_name: "coroutine",
                runtime_task_kind: TASK_KIND_COROUTINE,
                payload_base_offset: 0,
                completion: TaskTrampolineCompletion::RegisterCancelToken,
            }),
            TrampolineKind::AsyncGen => Some(Self {
                diagnostic_name: "async generator",
                runtime_task_kind: TASK_KIND_GENERATOR,
                payload_base_offset: GEN_CONTROL_SIZE,
                completion: TaskTrampolineCompletion::WrapAsyncGen,
            }),
            TrampolineKind::Plain => None,
        }
    }

    fn validate_closure_size(&self, closure_size: i64, arity: usize, has_closure: bool) {
        if closure_size < 0 {
            panic!("{} closure size must be non-negative", self.diagnostic_name);
        }
        let payload_slots = arity + usize::from(has_closure);
        let needed = i64::from(self.payload_base_offset) + (payload_slots as i64) * 8;
        if closure_size < needed {
            panic!(
                "{} closure size too small for trampoline",
                self.diagnostic_name
            );
        }
    }

    fn emit_completion(&self, backend: &mut WasmBackend, func: &mut Function, reloc_enabled: bool) {
        match self.completion {
            TaskTrampolineCompletion::ReturnTask => {
                func.instruction(&Instruction::LocalGet(TASK_LOCAL));
            }
            TaskTrampolineCompletion::RegisterCancelToken => {
                func.instruction(&Instruction::LocalGet(TASK_LOCAL));
                emit_call(
                    func,
                    reloc_enabled,
                    backend.import_ids
                        [crate::wasm_abi_generated::WasmRuntimeImport::CancelTokenGetCurrent],
                );
                emit_call(
                    func,
                    reloc_enabled,
                    backend.import_ids
                        [crate::wasm_abi_generated::WasmRuntimeImport::TaskRegisterTokenOwned],
                );
                func.instruction(&Instruction::Drop);
                func.instruction(&Instruction::LocalGet(TASK_LOCAL));
            }
            TaskTrampolineCompletion::WrapAsyncGen => {
                func.instruction(&Instruction::LocalGet(TASK_LOCAL));
                emit_call(
                    func,
                    reloc_enabled,
                    backend.import_ids[crate::wasm_abi_generated::WasmRuntimeImport::AsyncgenNew],
                );
            }
        }
        func.instruction(&Instruction::End);
    }
}

#[derive(Clone, Copy)]
enum TaskTrampolineCompletion {
    ReturnTask,
    RegisterCancelToken,
    WrapAsyncGen,
}

fn emit_payload_slots(
    backend: &mut WasmBackend,
    func: &mut Function,
    reloc_enabled: bool,
    payload_base_offset: i32,
    arity: usize,
    has_closure: bool,
) {
    let payload_slots = arity + usize::from(has_closure);
    if payload_slots == 0 {
        return;
    }

    func.instruction(&Instruction::LocalGet(TASK_LOCAL));
    emit_call(
        func,
        reloc_enabled,
        backend.import_ids[crate::wasm_abi_generated::WasmRuntimeImport::HandleResolve],
    );
    func.instruction(&Instruction::LocalSet(BASE_LOCAL));
    if arity > 0 {
        func.instruction(&Instruction::LocalGet(1));
        func.instruction(&Instruction::I32WrapI64);
        func.instruction(&Instruction::LocalSet(ARGS_BASE_LOCAL));
    }

    let mut offset = payload_base_offset;
    if has_closure {
        store_payload_value(backend, func, reloc_enabled, offset, 0);
        offset += 8;
    }
    for idx in 0..arity {
        let arg_offset = offset + (idx as i32) * 8;
        func.instruction(&Instruction::LocalGet(ARGS_BASE_LOCAL));
        func.instruction(&Instruction::I64Load(wasm_encoder::MemArg {
            align: 3,
            offset: (idx * std::mem::size_of::<u64>()) as u64,
            memory_index: 0,
        }));
        func.instruction(&Instruction::LocalSet(VAL_LOCAL));
        store_payload_value(backend, func, reloc_enabled, arg_offset, VAL_LOCAL);
    }
}

fn store_payload_value(
    backend: &mut WasmBackend,
    func: &mut Function,
    reloc_enabled: bool,
    offset: i32,
    value_local: u32,
) {
    func.instruction(&Instruction::LocalGet(BASE_LOCAL));
    func.instruction(&Instruction::I32Const(offset));
    func.instruction(&Instruction::I32Add);
    func.instruction(&Instruction::LocalGet(value_local));
    func.instruction(&Instruction::I64Store(wasm_encoder::MemArg {
        align: 3,
        offset: 0,
        memory_index: 0,
    }));
    func.instruction(&Instruction::LocalGet(value_local));
    emit_call(
        func,
        reloc_enabled,
        backend.import_ids[crate::wasm_abi_generated::WasmRuntimeImport::IncRefObj],
    );
}
