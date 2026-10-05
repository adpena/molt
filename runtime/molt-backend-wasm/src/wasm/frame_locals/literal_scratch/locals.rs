use crate::wasm::const_materialization::WasmConstOpPolicy;
use crate::wasm::frame_locals::{WasmFrameLocalKind, WasmFrameLocals};
use molt_tir::tir::op_kinds_generated::OwnedLiteralPayloadKind;
use wasm_encoder::ValType;

#[derive(Clone, Copy)]
pub(in crate::wasm) struct WasmLiteralScratchLocals {
    ptr_local: u32,
    len_local: u32,
}

impl WasmLiteralScratchLocals {
    pub(in crate::wasm) fn ptr_local(self) -> u32 {
        self.ptr_local
    }

    pub(in crate::wasm) fn len_local(self) -> u32 {
        self.len_local
    }
}

impl WasmFrameLocals {
    pub(in crate::wasm) fn ensure_literal_scratch(
        &mut self,
        out_name: &str,
        payload: OwnedLiteralPayloadKind,
        local_types: &mut Vec<ValType>,
        local_count: &mut u32,
    ) -> WasmLiteralScratchLocals {
        self.record_literal_scratch_payload(out_name, payload);
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
        }
    }

    pub(in crate::wasm) fn ensure_literal_scratch_for_policy(
        &mut self,
        out_name: &str,
        policy: WasmConstOpPolicy,
        local_types: &mut Vec<ValType>,
        local_count: &mut u32,
    ) -> Option<WasmLiteralScratchLocals> {
        policy
            .literal_payload()
            .map(|payload| self.ensure_literal_scratch(out_name, payload, local_types, local_count))
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
        self.literal_scratch_payloads.get(out_name)?;
        Some(WasmLiteralScratchLocals {
            ptr_local: self.get(ptr_name.as_str()).copied()?,
            len_local: self.get(len_name.as_str()).copied()?,
        })
    }

    fn record_literal_scratch_payload(&mut self, out_name: &str, payload: OwnedLiteralPayloadKind) {
        if let Some(existing) = self.literal_scratch_payloads.get(out_name) {
            assert_eq!(
                *existing, payload,
                "wasm literal scratch payload for {out_name} changed"
            );
            return;
        }
        self.literal_scratch_payloads
            .insert(out_name.to_string(), payload);
    }

    fn literal_ptr_name(out_name: &str) -> String {
        format!("{out_name}_ptr")
    }

    fn literal_len_name(out_name: &str) -> String {
        format!("{out_name}_len")
    }
}
