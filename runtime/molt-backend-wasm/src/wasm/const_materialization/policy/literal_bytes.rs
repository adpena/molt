use super::WasmConstOpPolicy;
use crate::OpIR;
use molt_ir::literal_payload::{required_simple_literal_bytes, required_tir_literal_bytes};
use molt_tir::tir::ops::TirOp;
use std::sync::Arc;

impl WasmConstOpPolicy {
    pub(in crate::wasm) fn required_simple_ir_literal_bytes(self, op: &OpIR) -> Arc<[u8]> {
        assert_eq!(
            self.0.kind, op.kind,
            "literal policy must match its operation"
        );
        Arc::from(required_simple_literal_bytes(op))
    }

    pub(in crate::wasm::const_materialization::policy) fn required_tir_literal_bytes(
        self,
        op: &TirOp,
    ) -> Arc<[u8]> {
        assert_eq!(
            self.0.kind,
            molt_ir::tir::op_kinds_generated::opcode_canonical_kind_table(op.opcode),
            "literal policy must match its operation",
        );
        Arc::from(required_tir_literal_bytes(op))
    }
}
