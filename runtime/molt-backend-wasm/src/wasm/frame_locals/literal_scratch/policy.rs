use crate::wasm::const_materialization::WasmConstOpPolicy;
use crate::wasm_abi_generated::WasmConstLiteralPayload;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(in crate::wasm) struct WasmLiteralScratchPolicy {
    payload: WasmConstLiteralPayload,
    parse_scalar_eligible: bool,
}

impl WasmLiteralScratchPolicy {
    pub(in crate::wasm) fn new(
        payload: WasmConstLiteralPayload,
        parse_scalar_eligible: bool,
    ) -> Self {
        assert!(
            !matches!(payload, WasmConstLiteralPayload::None),
            "literal scratch policy requires a typed literal payload"
        );
        assert!(
            !matches!(payload, WasmConstLiteralPayload::BigintDecimal) || !parse_scalar_eligible,
            "const_bigint decimal literal scratch must not be scalar-parse eligible"
        );
        Self {
            payload,
            parse_scalar_eligible,
        }
    }

    pub(in crate::wasm) fn payload(self) -> WasmConstLiteralPayload {
        self.payload
    }

    pub(in crate::wasm) fn parse_scalar_eligible(self) -> bool {
        self.parse_scalar_eligible
    }

    pub(super) fn from_const_policy(policy: WasmConstOpPolicy) -> Option<Self> {
        if !policy.needs_literal_scratch() {
            return None;
        }
        Some(Self::new(
            policy.literal_payload(),
            policy.parse_scalar_literal(),
        ))
    }
}
