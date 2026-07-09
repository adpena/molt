use super::{WasmFrameLocalKind, WasmFrameLocals};
use crate::wasm::const_materialization::WasmConstOpPolicy;
use crate::wasm_abi_generated::WasmConstLiteralPayload;
use wasm_encoder::ValType;

mod policy;
#[cfg(test)]
mod tests;

pub(in crate::wasm) use policy::WasmLiteralScratchPolicy;

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
    pub(in crate::wasm) fn payload(self) -> WasmConstLiteralPayload {
        self.policy.payload()
    }

    pub(in crate::wasm) fn parse_scalar_eligible(self) -> bool {
        self.policy.parse_scalar_eligible()
    }
}

impl WasmFrameLocals {
    pub(in crate::wasm) fn ensure_literal_scratch(
        &mut self,
        out_name: &str,
        payload: WasmConstLiteralPayload,
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
