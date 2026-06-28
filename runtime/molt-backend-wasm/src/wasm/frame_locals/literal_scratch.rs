use super::{WasmFrameLocalKind, WasmFrameLocals};
use crate::wasm::const_materialization::WasmConstOpPolicy;
use crate::wasm_abi_generated::WasmConstLiteralPayload;
use wasm_encoder::ValType;

#[derive(Clone, Copy)]
pub(in crate::wasm) struct WasmLiteralScratchLocals {
    ptr_local: u32,
    len_local: u32,
    policy: WasmLiteralScratchPolicy,
}

impl WasmLiteralScratchLocals {
    pub(in crate::wasm) fn ptr_local(self) -> u32 {
        self.ptr_local
    }

    pub(in crate::wasm) fn len_local(self) -> u32 {
        self.len_local
    }

    #[cfg(test)]
    pub(in crate::wasm) fn payload(self) -> WasmLiteralPayload {
        self.policy.payload()
    }

    pub(in crate::wasm) fn parse_scalar_eligible(self) -> bool {
        self.policy.parse_scalar_eligible()
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(in crate::wasm) enum WasmLiteralPayload {
    None,
    String,
    Bytes,
    BigintDecimal,
}

impl WasmLiteralPayload {
    fn needs_literal_scratch(self) -> bool {
        !matches!(self, Self::None)
    }

    fn from_const_payload(payload: WasmConstLiteralPayload) -> Self {
        match payload {
            WasmConstLiteralPayload::None => Self::None,
            WasmConstLiteralPayload::String => Self::String,
            WasmConstLiteralPayload::Bytes => Self::Bytes,
            WasmConstLiteralPayload::BigintDecimal => Self::BigintDecimal,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(in crate::wasm) struct WasmLiteralScratchPolicy {
    payload: WasmLiteralPayload,
    parse_scalar_eligible: bool,
}

impl WasmLiteralScratchPolicy {
    pub(in crate::wasm) fn new(payload: WasmLiteralPayload, parse_scalar_eligible: bool) -> Self {
        assert!(
            payload.needs_literal_scratch(),
            "literal scratch policy requires a typed literal payload"
        );
        assert!(
            !matches!(payload, WasmLiteralPayload::BigintDecimal) || !parse_scalar_eligible,
            "const_bigint decimal literal scratch must not be scalar-parse eligible"
        );
        Self {
            payload,
            parse_scalar_eligible,
        }
    }

    pub(in crate::wasm) fn payload(self) -> WasmLiteralPayload {
        self.payload
    }

    pub(in crate::wasm) fn parse_scalar_eligible(self) -> bool {
        self.parse_scalar_eligible
    }

    fn from_const_policy(policy: WasmConstOpPolicy) -> Option<Self> {
        if !policy.needs_literal_scratch() {
            return None;
        }
        let payload = WasmLiteralPayload::from_const_payload(policy.literal_payload());
        Some(Self::new(payload, policy.parse_scalar_literal()))
    }
}

impl WasmFrameLocals {
    pub(in crate::wasm) fn ensure_literal_scratch(
        &mut self,
        out_name: &str,
        payload: WasmLiteralPayload,
        parse_scalar_eligible: bool,
        local_types: &mut Vec<ValType>,
        local_count: &mut u32,
    ) -> WasmLiteralScratchLocals {
        let policy = WasmLiteralScratchPolicy::new(payload, parse_scalar_eligible);
        self.record_literal_scratch_policy(out_name, policy);
        let ptr_local = self.ensure_named_i64(
            Self::literal_ptr_name(out_name),
            WasmFrameLocalKind::LiteralScratchPtr,
            local_types,
            local_count,
        );
        let len_local = self.ensure_named_i64(
            Self::literal_len_name(out_name),
            WasmFrameLocalKind::LiteralScratchLen,
            local_types,
            local_count,
        );
        WasmLiteralScratchLocals {
            ptr_local,
            len_local,
            policy,
        }
    }

    pub(in crate::wasm) fn ensure_literal_scratch_for_policy(
        &mut self,
        out_name: &str,
        policy: WasmConstOpPolicy,
        local_types: &mut Vec<ValType>,
        local_count: &mut u32,
    ) -> Option<WasmLiteralScratchLocals> {
        WasmLiteralScratchPolicy::from_const_policy(policy).map(|literal_policy| {
            self.ensure_literal_scratch(
                out_name,
                literal_policy.payload(),
                literal_policy.parse_scalar_eligible(),
                local_types,
                local_count,
            )
        })
    }

    pub(in crate::wasm) fn literal_scratch(&self, out_name: &str) -> WasmLiteralScratchLocals {
        self.try_literal_scratch(out_name).unwrap_or_else(|| {
            panic!("wasm literal scratch locals for {out_name} are not allocated")
        })
    }

    pub(in crate::wasm) fn try_literal_scratch(
        &self,
        out_name: &str,
    ) -> Option<WasmLiteralScratchLocals> {
        let ptr_name = Self::literal_ptr_name(out_name);
        let len_name = Self::literal_len_name(out_name);
        let policy = self.literal_scratch_policies.get(out_name).copied()?;
        Some(WasmLiteralScratchLocals {
            ptr_local: self.get(ptr_name.as_str()).copied()?,
            len_local: self.get(len_name.as_str()).copied()?,
            policy,
        })
    }

    pub(in crate::wasm) fn try_parse_scalar_literal_scratch(
        &self,
        out_name: &str,
    ) -> Option<WasmLiteralScratchLocals> {
        self.try_literal_scratch(out_name)
            .filter(|scratch| scratch.parse_scalar_eligible())
    }

    fn record_literal_scratch_policy(&mut self, out_name: &str, policy: WasmLiteralScratchPolicy) {
        if let Some(existing) = self.literal_scratch_policies.get(out_name) {
            assert_eq!(
                *existing, policy,
                "wasm literal scratch policy for {out_name} changed"
            );
            return;
        }
        self.literal_scratch_policies
            .insert(out_name.to_string(), policy);
    }

    fn literal_ptr_name(out_name: &str) -> String {
        format!("{out_name}_ptr")
    }

    fn literal_len_name(out_name: &str) -> String {
        format!("{out_name}_len")
    }
}

#[cfg(test)]
mod tests {
    use super::{WasmFrameLocalKind, WasmFrameLocals, WasmLiteralPayload};
    use crate::wasm::const_materialization::WasmConstOpPolicy;
    use wasm_encoder::ValType;

    #[test]
    fn literal_scratch_locals_are_owned_and_reused_by_frame_locals() {
        let mut locals = WasmFrameLocals::new();
        let mut local_types = Vec::new();
        let mut local_count = 0;

        let first = locals.ensure_literal_scratch(
            "payload",
            WasmLiteralPayload::String,
            true,
            &mut local_types,
            &mut local_count,
        );
        let second = locals.ensure_literal_scratch(
            "payload",
            WasmLiteralPayload::String,
            true,
            &mut local_types,
            &mut local_count,
        );
        let looked_up = locals.literal_scratch("payload");
        let maybe_lookup = locals.try_literal_scratch("payload");
        let parse_lookup = locals.try_parse_scalar_literal_scratch("payload");

        assert_eq!(first.ptr_local(), 0);
        assert_eq!(first.len_local(), 1);
        assert_eq!(first.payload(), WasmLiteralPayload::String);
        assert!(first.parse_scalar_eligible());
        assert_eq!(second.ptr_local(), first.ptr_local());
        assert_eq!(second.len_local(), first.len_local());
        assert_eq!(looked_up.ptr_local(), first.ptr_local());
        assert_eq!(looked_up.len_local(), first.len_local());
        assert_eq!(maybe_lookup.map(|scratch| scratch.ptr_local()), Some(0));
        assert_eq!(parse_lookup.map(|scratch| scratch.len_local()), Some(1));
        assert!(locals.try_literal_scratch("missing").is_none());
        assert!(locals.try_parse_scalar_literal_scratch("missing").is_none());
        assert_eq!(
            locals.local_kind("payload_ptr"),
            Some(WasmFrameLocalKind::LiteralScratchPtr)
        );
        assert_eq!(
            locals.local_kind("payload_len"),
            Some(WasmFrameLocalKind::LiteralScratchLen)
        );
        assert!(
            locals
                .named_locals()
                .find(|local| local.name() == "payload_ptr")
                .is_some_and(|local| local.kind().is_call_retention_exempt())
        );
        assert!(
            locals
                .named_locals()
                .find(|local| local.name() == "payload_len")
                .is_some_and(|local| local.kind().is_call_retention_exempt())
        );
        assert_eq!(local_types, vec![ValType::I64, ValType::I64]);
        assert_eq!(local_count, 2);
    }

    #[test]
    fn literal_scratch_policy_controls_scalar_parse_eligibility() {
        let mut locals = WasmFrameLocals::new();
        let mut local_types = Vec::new();
        let mut local_count = 0;

        let string_scratch = locals
            .ensure_literal_scratch_for_policy(
                "text",
                WasmConstOpPolicy::for_kind("const_str").expect("const_str policy"),
                &mut local_types,
                &mut local_count,
            )
            .expect("const_str should allocate literal scratch");
        let bigint_scratch = locals
            .ensure_literal_scratch_for_policy(
                "digits",
                WasmConstOpPolicy::for_kind("const_bigint").expect("const_bigint policy"),
                &mut local_types,
                &mut local_count,
            )
            .expect("const_bigint should allocate literal scratch");
        let bytes_scratch = locals
            .ensure_literal_scratch_for_policy(
                "blob",
                WasmConstOpPolicy::for_kind("const_bytes").expect("const_bytes policy"),
                &mut local_types,
                &mut local_count,
            )
            .expect("const_bytes should allocate literal scratch");
        let none_scratch = locals.ensure_literal_scratch_for_policy(
            "none",
            WasmConstOpPolicy::for_kind("const_none").expect("const_none policy"),
            &mut local_types,
            &mut local_count,
        );

        assert_eq!(string_scratch.payload(), WasmLiteralPayload::String);
        assert!(string_scratch.parse_scalar_eligible());
        assert_eq!(bigint_scratch.payload(), WasmLiteralPayload::BigintDecimal);
        assert!(!bigint_scratch.parse_scalar_eligible());
        assert_eq!(bytes_scratch.payload(), WasmLiteralPayload::Bytes);
        assert!(bytes_scratch.parse_scalar_eligible());
        assert!(none_scratch.is_none());
        assert!(locals.try_parse_scalar_literal_scratch("text").is_some());
        assert!(locals.try_parse_scalar_literal_scratch("blob").is_some());
        assert!(locals.try_literal_scratch("digits").is_some());
        assert!(locals.try_parse_scalar_literal_scratch("digits").is_none());
        assert_eq!(
            locals
                .try_literal_scratch("digits")
                .map(|scratch| scratch.payload()),
            Some(WasmLiteralPayload::BigintDecimal)
        );
        assert_eq!(
            local_types,
            vec![
                ValType::I64,
                ValType::I64,
                ValType::I64,
                ValType::I64,
                ValType::I64,
                ValType::I64,
            ]
        );
        assert_eq!(local_count, 6);
    }
}
