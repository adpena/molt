mod layout;
mod payload;

pub(in crate::wasm) use self::layout::WasmTaskRuntimeLayout;
pub(in crate::wasm) use self::payload::{
    emit_register_cancel_token, emit_store_task_payload_local, emit_task_payload_base,
};

#[cfg(test)]
mod tests {
    use super::*;
    use crate::wasm_abi::{GEN_CONTROL_SIZE, TASK_KIND_COROUTINE, TASK_KIND_FUTURE};

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
