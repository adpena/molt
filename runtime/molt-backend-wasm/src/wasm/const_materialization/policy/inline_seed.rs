use super::WasmConstOpPolicy;
use crate::OpIR;
use crate::wasm::WasmFrameLocals;
use crate::wasm_abi_generated::WasmConstInlineSeed;
use crate::wasm_values::ConstantCache;
use wasm_encoder::{Function, Instruction};

impl WasmConstOpPolicy {
    pub(in crate::wasm) fn inline_seed_bits(self, op: &OpIR) -> Option<i64> {
        (!matches!(self.inline_seed(), WasmConstInlineSeed::None))
            .then(|| self.0.required_simple_ir_inline_seed_bits(op))
    }

    pub(in crate::wasm) fn emit_inline_seed(
        self,
        func: &mut Function,
        op: &OpIR,
        locals: &WasmFrameLocals,
        const_cache: &ConstantCache,
    ) -> bool {
        let Some(out) = op.out.as_ref() else {
            return false;
        };
        if matches!(self.inline_seed(), WasmConstInlineSeed::None) {
            return false;
        }
        match self.inline_seed() {
            WasmConstInlineSeed::NoneValue => const_cache.emit_none(func),
            WasmConstInlineSeed::Int | WasmConstInlineSeed::Bool | WasmConstInlineSeed::Float => {
                func.instruction(&Instruction::I64Const(
                    self.0.required_simple_ir_inline_seed_bits(op),
                ));
            }
            WasmConstInlineSeed::None => unreachable!("inline seed checked above"),
        }
        let local_idx = locals[out];
        func.instruction(&Instruction::LocalSet(local_idx));
        true
    }
}
