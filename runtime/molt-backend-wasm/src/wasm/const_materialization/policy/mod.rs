use molt_tir::tir::op_kinds_generated::OwnedLiteralPayloadKind;
mod inline_seed;
mod literal_bytes;
mod materialization;
mod raw_int;

#[cfg(test)]
mod tests;

use crate::OpIR;
use crate::wasm_abi_generated::{
    WasmConstInlineSeed, WasmConstOpPolicySpec, WasmConstRawIntEffect, WasmRuntimeImport,
    wasm_const_op_policy,
};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct WasmConstOpPolicy(&'static WasmConstOpPolicySpec);

impl WasmConstOpPolicy {
    pub(in crate::wasm) fn for_op(op: &OpIR) -> Option<Self> {
        Self::for_kind(op.kind)
    }

    pub(in crate::wasm) fn for_kind(kind: molt_ir::OpKind) -> Option<Self> {
        wasm_const_op_policy(kind).map(Self)
    }

    pub(in crate::wasm) fn inline_seed(self) -> WasmConstInlineSeed {
        self.0.inline_seed
    }

    pub(in crate::wasm) fn literal_payload(self) -> Option<OwnedLiteralPayloadKind> {
        self.0.literal_payload
    }

    pub(in crate::wasm) fn materializer_import(self) -> Option<WasmRuntimeImport> {
        self.0.materializer_import
    }

    pub(in crate::wasm) fn raw_int_effect(self) -> WasmConstRawIntEffect {
        self.0.raw_int_effect
    }

    pub(in crate::wasm) fn needs_runtime_anchor(self) -> bool {
        self.0.dispatch_runtime_seed
    }

    pub(in crate::wasm) fn materialization_can_fail(self) -> bool {
        self.literal_payload().is_some() || matches!(self.inline_seed(), WasmConstInlineSeed::Int)
    }
}
