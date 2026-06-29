use wasm_encoder::{Function, Instruction, ValType};

use crate::TrampolineKind;
use crate::wasm::WasmBackend;
use crate::wasm::task_runtime::{
    WasmTaskRuntimeLayout, emit_store_task_payload_local, emit_task_payload_base,
};

const TASK_LOCAL: u32 = 3;
const BASE_LOCAL: u32 = 4;
const VAL_LOCAL: u32 = 5;
const ARGS_BASE_LOCAL: u32 = 6;

pub(super) fn task_trampoline_local_types(kind: TrampolineKind) -> Option<[ValType; 4]> {
    WasmTaskRuntimeLayout::for_trampoline_kind(kind).map(|layout| layout.trampoline_local_types())
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
    let Some(layout) = WasmTaskRuntimeLayout::for_trampoline_kind(kind) else {
        return false;
    };
    layout.validate_closure_size(closure_size, arity, has_closure);

    layout.emit_task_new(
        func,
        &backend.import_ids,
        reloc_enabled,
        table_idx,
        closure_size,
    );
    func.instruction(&Instruction::LocalSet(TASK_LOCAL));

    emit_payload_slots(
        backend,
        func,
        reloc_enabled,
        layout.payload_base_offset(),
        arity,
        has_closure,
    );
    layout.emit_completion_result(func, &backend.import_ids, reloc_enabled, TASK_LOCAL);
    func.instruction(&Instruction::End);
    true
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

    emit_task_payload_base(
        func,
        &backend.import_ids,
        reloc_enabled,
        TASK_LOCAL,
        BASE_LOCAL,
    );
    if arity > 0 {
        func.instruction(&Instruction::LocalGet(1));
        func.instruction(&Instruction::I32WrapI64);
        func.instruction(&Instruction::LocalSet(ARGS_BASE_LOCAL));
    }

    let mut offset = payload_base_offset;
    if has_closure {
        emit_store_task_payload_local(
            func,
            &backend.import_ids,
            reloc_enabled,
            BASE_LOCAL,
            offset,
            0,
        );
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
        emit_store_task_payload_local(
            func,
            &backend.import_ids,
            reloc_enabled,
            BASE_LOCAL,
            arg_offset,
            VAL_LOCAL,
        );
    }
}
