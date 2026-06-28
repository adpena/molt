use super::{WasmFrameLocalKind, WasmFrameLocals};
use wasm_encoder::ValType;

impl WasmFrameLocals {
    pub(in crate::wasm) fn ensure_multi_return_callee_value(
        &mut self,
        index: usize,
        local_types: &mut Vec<ValType>,
        local_count: &mut u32,
    ) -> u32 {
        self.ensure_named_i64(
            Self::multi_return_callee_name(index),
            WasmFrameLocalKind::MultiReturnCalleeValue,
            local_types,
            local_count,
        )
    }

    pub(in crate::wasm) fn ensure_multi_return_call_value(
        &mut self,
        result_var: &str,
        index: usize,
        local_types: &mut Vec<ValType>,
        local_count: &mut u32,
    ) -> u32 {
        self.ensure_named_i64(
            Self::multi_return_call_name(result_var, index),
            WasmFrameLocalKind::MultiReturnCallValue,
            local_types,
            local_count,
        )
    }

    fn multi_return_callee_name(index: usize) -> String {
        format!("__multi_ret_{index}")
    }

    fn multi_return_call_name(result_var: &str, index: usize) -> String {
        format!("__multi_call_{result_var}_{index}")
    }
}
