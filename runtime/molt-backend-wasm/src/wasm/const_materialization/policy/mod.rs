mod inline_seed;
mod literal_bytes;
mod materialization;
mod raw_int;

#[cfg(test)]
mod tests;

use crate::OpIR;
use crate::wasm_abi_generated::{
    WasmConstInlineSeed, WasmConstLirFastPolicy, WasmConstLiteralPayload, WasmConstOpPolicySpec,
    WasmConstRawIntEffect, WasmConstScalarValue, WasmRuntimeImport, wasm_const_op_policy,
    wasm_const_op_policy_for_opcode,
};
use molt_tir::tir::ops::{OpCode, TirOp};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct WasmConstOpPolicy(&'static WasmConstOpPolicySpec);

impl WasmConstOpPolicy {
    pub(in crate::wasm) fn for_op(op: &OpIR) -> Option<Self> {
        Self::for_kind(op.kind.as_str())
    }

    pub(in crate::wasm) fn for_kind(kind: &str) -> Option<Self> {
        wasm_const_op_policy(kind).map(Self)
    }

    pub(in crate::wasm) fn for_tir_opcode(opcode: OpCode) -> Option<Self> {
        wasm_const_op_policy_for_opcode(opcode).map(Self)
    }

    pub(in crate::wasm) fn inline_seed(self) -> WasmConstInlineSeed {
        self.0.inline_seed
    }

    pub(in crate::wasm) fn literal_payload(self) -> WasmConstLiteralPayload {
        self.0.literal_payload
    }

    pub(in crate::wasm) fn parse_scalar_literal(self) -> bool {
        self.0.parse_scalar_literal
    }

    pub(in crate::wasm) fn materializer_import(self) -> Option<WasmRuntimeImport> {
        self.0.materializer_import
    }

    pub(in crate::wasm) fn raw_int_effect(self) -> WasmConstRawIntEffect {
        self.0.raw_int_effect
    }

    pub(in crate::wasm) fn lir_fast_policy(self) -> WasmConstLirFastPolicy {
        self.0.lir_fast
    }

    pub(in crate::wasm) fn needs_literal_scratch(self) -> bool {
        !matches!(self.literal_payload(), WasmConstLiteralPayload::None)
    }

    pub(in crate::wasm) fn needs_dispatch_runtime_seed(self) -> bool {
        self.0.dispatch_runtime_seed
    }

    pub(in crate::wasm) fn required_tir_scalar_value(self, op: &TirOp) -> WasmConstScalarValue {
        self.0.required_tir_scalar_value(op)
    }
}
