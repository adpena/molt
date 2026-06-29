use wasm_encoder::{Function, Instruction, MemArg, ValType};

use crate::TrampolineKind;
use crate::wasm_abi::{
    GEN_CONTROL_SIZE, TASK_KIND_COROUTINE, TASK_KIND_FUTURE, TASK_KIND_GENERATOR, WasmRuntimeImport,
};
use crate::wasm_binary::{emit_call, emit_table_index_i64};
use crate::wasm_import_tracking::TrackedImportIds;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(in crate::wasm) enum WasmTaskCompletion {
    ReturnTask,
    RegisterCancelToken,
    WrapAsyncGen,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(in crate::wasm) struct WasmTaskRuntimeLayout {
    diagnostic_name: &'static str,
    runtime_task_kind: i64,
    payload_base_offset: i32,
    completion: WasmTaskCompletion,
}

impl WasmTaskRuntimeLayout {
    pub(in crate::wasm) fn for_alloc_task_kind(task_kind: Option<&str>) -> Self {
        match task_kind.unwrap_or("future") {
            "generator" => Self::generator(),
            "future" => Self::future(),
            "coroutine" => Self::coroutine(),
            other => panic!("unknown task kind: {other}"),
        }
    }

    pub(in crate::wasm) fn for_call_async() -> Self {
        Self {
            diagnostic_name: "async call",
            runtime_task_kind: TASK_KIND_FUTURE,
            payload_base_offset: 0,
            completion: WasmTaskCompletion::ReturnTask,
        }
    }

    pub(in crate::wasm) fn for_trampoline_kind(kind: TrampolineKind) -> Option<Self> {
        match kind {
            TrampolineKind::Generator => Some(Self::generator()),
            TrampolineKind::Coroutine => Some(Self::coroutine()),
            TrampolineKind::AsyncGen => Some(Self {
                diagnostic_name: "async generator",
                runtime_task_kind: TASK_KIND_GENERATOR,
                payload_base_offset: GEN_CONTROL_SIZE,
                completion: WasmTaskCompletion::WrapAsyncGen,
            }),
            TrampolineKind::Plain => None,
        }
    }

    pub(in crate::wasm) fn runtime_task_kind(self) -> i64 {
        self.runtime_task_kind
    }

    pub(in crate::wasm) fn payload_base_offset(self) -> i32 {
        self.payload_base_offset
    }

    pub(in crate::wasm) fn diagnostic_name(self) -> &'static str {
        self.diagnostic_name
    }

    pub(in crate::wasm) fn registers_cancel_token(self) -> bool {
        matches!(self.completion, WasmTaskCompletion::RegisterCancelToken)
    }

    pub(in crate::wasm) fn needs_alloc_resolve(self, has_payload_args: bool) -> bool {
        has_payload_args
    }

    pub(in crate::wasm) fn validate_closure_size(
        self,
        closure_size: i64,
        arity: usize,
        has_closure: bool,
    ) {
        if closure_size < 0 {
            panic!(
                "{} closure size must be non-negative",
                self.diagnostic_name()
            );
        }
        let payload_slots = arity + usize::from(has_closure);
        let needed = i64::from(self.payload_base_offset()) + (payload_slots as i64) * 8;
        if closure_size < needed {
            panic!(
                "{} closure size too small for trampoline",
                self.diagnostic_name()
            );
        }
    }

    pub(in crate::wasm) fn trampoline_local_types(self) -> [ValType; 4] {
        let _ = self;
        [ValType::I64, ValType::I32, ValType::I64, ValType::I32]
    }

    pub(in crate::wasm) fn emit_task_new(
        self,
        func: &mut Function,
        import_ids: &TrackedImportIds,
        reloc_enabled: bool,
        table_idx: u32,
        payload_size_bytes: i64,
    ) {
        emit_table_index_i64(func, reloc_enabled, table_idx);
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
    ) {
        match self.completion {
            WasmTaskCompletion::ReturnTask => {
                func.instruction(&Instruction::LocalGet(task_local));
            }
            WasmTaskCompletion::RegisterCancelToken => {
                emit_register_cancel_token(func, import_ids, reloc_enabled, task_local);
                func.instruction(&Instruction::LocalGet(task_local));
            }
            WasmTaskCompletion::WrapAsyncGen => {
                func.instruction(&Instruction::LocalGet(task_local));
                emit_call(
                    func,
                    reloc_enabled,
                    import_ids[WasmRuntimeImport::AsyncgenNew],
                );
            }
        }
    }

    fn future() -> Self {
        Self {
            diagnostic_name: "future",
            runtime_task_kind: TASK_KIND_FUTURE,
            payload_base_offset: 0,
            completion: WasmTaskCompletion::RegisterCancelToken,
        }
    }

    fn generator() -> Self {
        Self {
            diagnostic_name: "generator",
            runtime_task_kind: TASK_KIND_GENERATOR,
            payload_base_offset: GEN_CONTROL_SIZE,
            completion: WasmTaskCompletion::ReturnTask,
        }
    }

    fn coroutine() -> Self {
        Self {
            diagnostic_name: "coroutine",
            runtime_task_kind: TASK_KIND_COROUTINE,
            payload_base_offset: 0,
            completion: WasmTaskCompletion::RegisterCancelToken,
        }
    }
}

pub(in crate::wasm) fn emit_register_cancel_token(
    func: &mut Function,
    import_ids: &TrackedImportIds,
    reloc_enabled: bool,
    task_local: u32,
) {
    func.instruction(&Instruction::LocalGet(task_local));
    emit_call(
        func,
        reloc_enabled,
        import_ids[WasmRuntimeImport::CancelTokenGetCurrent],
    );
    emit_call(
        func,
        reloc_enabled,
        import_ids[WasmRuntimeImport::TaskRegisterTokenOwned],
    );
    func.instruction(&Instruction::Drop);
}

pub(in crate::wasm) fn emit_task_payload_base(
    func: &mut Function,
    import_ids: &TrackedImportIds,
    reloc_enabled: bool,
    task_local: u32,
    base_local: u32,
) {
    func.instruction(&Instruction::LocalGet(task_local));
    emit_call(
        func,
        reloc_enabled,
        import_ids[WasmRuntimeImport::HandleResolve],
    );
    func.instruction(&Instruction::LocalSet(base_local));
}

pub(in crate::wasm) fn emit_store_task_payload_local(
    func: &mut Function,
    import_ids: &TrackedImportIds,
    reloc_enabled: bool,
    base_local: u32,
    offset: i32,
    value_local: u32,
) {
    func.instruction(&Instruction::LocalGet(base_local));
    func.instruction(&Instruction::I32Const(offset));
    func.instruction(&Instruction::I32Add);
    func.instruction(&Instruction::LocalGet(value_local));
    func.instruction(&Instruction::I64Store(MemArg {
        align: 3,
        offset: 0,
        memory_index: 0,
    }));
    func.instruction(&Instruction::LocalGet(value_local));
    emit_call(
        func,
        reloc_enabled,
        import_ids[WasmRuntimeImport::IncRefObj],
    );
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn task_layout_decodes_alloc_task_kinds() {
        assert_eq!(
            WasmTaskRuntimeLayout::for_alloc_task_kind(Some("generator")).payload_base_offset(),
            GEN_CONTROL_SIZE
        );
        assert_eq!(
            WasmTaskRuntimeLayout::for_alloc_task_kind(Some("future")).runtime_task_kind(),
            TASK_KIND_FUTURE
        );
        assert_eq!(
            WasmTaskRuntimeLayout::for_alloc_task_kind(Some("coroutine")).runtime_task_kind(),
            TASK_KIND_COROUTINE
        );
        assert_eq!(
            WasmTaskRuntimeLayout::for_alloc_task_kind(None).runtime_task_kind(),
            TASK_KIND_FUTURE
        );
    }
}
