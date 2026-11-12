use super::WasmConstOpPolicy;
use crate::OpIR;
use molt_ir::literal_payload::required_simple_literal_bytes;
use std::sync::Arc;

impl WasmConstOpPolicy {
    pub(in crate::wasm) fn required_simple_ir_literal_bytes(self, op: &OpIR) -> Arc<[u8]> {
        assert_eq!(
            self.0.kind, op.kind,
            "literal policy must match its operation"
        );
        Arc::from(required_simple_literal_bytes(op))
    }
}
