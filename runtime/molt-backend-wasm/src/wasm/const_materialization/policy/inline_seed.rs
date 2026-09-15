use super::WasmConstOpPolicy;
use crate::OpIR;
use crate::wasm::WasmFrameLocals;
use crate::wasm_abi_generated::WasmConstInlineSeed;
use crate::wasm_values::ConstantCache;
use molt_codegen_abi::fits_inline_int;
use wasm_encoder::{Function, Instruction};

impl WasmConstOpPolicy {
    pub(in crate::wasm) fn inline_seed_bits(self, op: &OpIR) -> Option<i64> {
        match self.inline_seed() {
            WasmConstInlineSeed::None => None,
            WasmConstInlineSeed::Int => {
                let value = op.value.unwrap_or_else(|| {
                    panic!(
                        "WASM const policy {} requires int scalar payload",
                        self.0.kind
                    )
                });
                fits_inline_int(value).then(|| self.0.required_simple_ir_inline_seed_bits(op))
            }
            WasmConstInlineSeed::Bool
            | WasmConstInlineSeed::Float
            | WasmConstInlineSeed::NoneValue => {
                Some(self.0.required_simple_ir_inline_seed_bits(op))
            }
        }
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
        let Some(bits) = self.inline_seed_bits(op) else {
            return false;
        };
        match self.inline_seed() {
            WasmConstInlineSeed::NoneValue => const_cache.emit_none(func),
            WasmConstInlineSeed::Int | WasmConstInlineSeed::Bool | WasmConstInlineSeed::Float => {
                func.instruction(&Instruction::I64Const(bits));
            }
            WasmConstInlineSeed::None => unreachable!("inline seed checked above"),
        }
        let local_idx = locals[out];
        func.instruction(&Instruction::LocalSet(local_idx));
        true
    }
}
