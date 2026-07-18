use super::payload::emit_register_cancel_token;
use wasm_encoder::{Function, Instruction, ValType};

use crate::TrampolineTaskKind;
use crate::wasm_abi::{
    GEN_CONTROL_SIZE, TASK_KIND_COROUTINE, TASK_KIND_FUTURE, TASK_KIND_GENERATOR, WasmRuntimeImport,
};
use crate::wasm_binary::emit_call;
use crate::wasm_import_tracking::TrackedImportIds;
use crate::wasm_table::{WasmCallableTableTarget, WasmTableRelocations};

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum WasmTaskCompletion {
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

    pub(in crate::wasm) fn for_trampoline_task_kind(kind: TrampolineTaskKind) -> Self {
        match kind {
            TrampolineTaskKind::Generator => Self::generator(),
            TrampolineTaskKind::Coroutine => Self::coroutine(),
            TrampolineTaskKind::AsyncGen => Self {
                diagnostic_name: "async generator",
                runtime_task_kind: TASK_KIND_GENERATOR,
                payload_base_offset: GEN_CONTROL_SIZE,
                completion: WasmTaskCompletion::WrapAsyncGen,
            },
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
