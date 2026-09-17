use super::payload::emit_register_cancel_token;
use molt_tir::trampolines::{
    TaskCompletion, TaskConstructorLayout, TaskRuntimeKind, TrampolineTaskKind,
};
use wasm_encoder::{Function, Instruction, ValType};

use crate::wasm_abi::{
    GEN_CONTROL_SIZE, TASK_KIND_COROUTINE, TASK_KIND_FUTURE, TASK_KIND_GENERATOR, WasmRuntimeImport,
};
use crate::wasm_binary::emit_call;
use crate::wasm_import_tracking::TrackedImportIds;
use crate::wasm_table::{WasmCallableTableTarget, WasmTableRelocations};

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(in crate::wasm) struct WasmTaskRuntimeLayout {
    runtime_task_kind: i64,
    payload_base_offset: i32,
    completion: TaskCompletion,
}

impl WasmTaskRuntimeLayout {
    pub(in crate::wasm) fn for_alloc_task_kind(task_kind: Option<&str>) -> Self {
        Self::from_constructor_layout(TaskConstructorLayout::for_alloc_kind(task_kind))
    }

    pub(in crate::wasm) fn for_call_async() -> Self {
        Self::from_constructor_layout(TaskConstructorLayout::for_call_async())
    }

    pub(in crate::wasm) fn for_trampoline_task_kind(kind: TrampolineTaskKind) -> Self {
        Self::from_constructor_layout(kind.constructor_layout())
    }

    fn from_constructor_layout(layout: TaskConstructorLayout) -> Self {
        let runtime_task_kind = match layout.runtime_kind() {
            TaskRuntimeKind::Future => TASK_KIND_FUTURE,
            TaskRuntimeKind::Generator => TASK_KIND_GENERATOR,
            TaskRuntimeKind::Coroutine => TASK_KIND_COROUTINE,
        };
        Self {
            runtime_task_kind,
            payload_base_offset: layout.payload_base_offset(GEN_CONTROL_SIZE),
            completion: layout.completion(),
        }
    }

    pub(in crate::wasm) fn runtime_task_kind(self) -> i64 {
        self.runtime_task_kind
    }

    pub(in crate::wasm) fn payload_base_offset(self) -> i32 {
        self.payload_base_offset
    }

    pub(in crate::wasm) fn registers_cancel_token(self) -> bool {
        matches!(self.completion, TaskCompletion::RegisterCancelToken)
    }

    pub(in crate::wasm) fn needs_alloc_resolve(self, has_payload_args: bool) -> bool {
        has_payload_args
    }

    pub(in crate::wasm) fn trampoline_local_types(self) -> [ValType; 4] {
        let _ = self;
        [ValType::I64, ValType::I32, ValType::I64, ValType::I32]
    }

    pub(in crate::wasm) fn emit_task_new(
        self,
        func: &mut Function,
        import_ids: &TrackedImportIds,
        table_relocations: &mut WasmTableRelocations,
        reloc_enabled: bool,
        func_import_count: u32,
        owner_func_index: u32,
        table_target: &WasmCallableTableTarget,
        payload_size_bytes: i64,
    ) {
        table_relocations.emit_i64(
            reloc_enabled,
            func_import_count,
            owner_func_index,
            func,
            table_target,
        );
        func.instruction(&Instruction::I64Const(payload_size_bytes));
        func.instruction(&Instruction::I64Const(self.runtime_task_kind()));
        emit_call(func, reloc_enabled, import_ids[WasmRuntimeImport::TaskNew]);
    }

    pub(in crate::wasm) fn emit_completion_result(
        self,
        func: &mut Function,
        import_ids: &TrackedImportIds,
        reloc_enabled: bool,
        task_local: u32,
        result_local: u32,
    ) {
        match self.completion {
            TaskCompletion::ReturnTask => {
                func.instruction(&Instruction::LocalGet(task_local));
            }
            TaskCompletion::RegisterCancelToken => {
                emit_register_cancel_token(func, import_ids, reloc_enabled, task_local);
                func.instruction(&Instruction::LocalGet(task_local));
            }
            TaskCompletion::WrapAsyncGen => {
                func.instruction(&Instruction::LocalGet(task_local));
                emit_call(
                    func,
                    reloc_enabled,
                    import_ids[WasmRuntimeImport::AsyncgenNew],
                );
                func.instruction(&Instruction::LocalSet(result_local));
                func.instruction(&Instruction::LocalGet(task_local));
                emit_call(
                    func,
                    reloc_enabled,
                    import_ids[WasmRuntimeImport::DecRefObj],
                );
                func.instruction(&Instruction::LocalGet(result_local));
            }
        }
    }
}
