use super::{WasmConstMaterialization, WasmConstMaterializationScratch};
use crate::OpIR;
use crate::wasm::WasmFrameLocals;
use crate::wasm_abi_generated::{
    WasmConstInlineSeed, WasmConstLirFastPolicy, WasmConstLiteralPayload, WasmConstOpPolicySpec,
    WasmConstRawIntEffect, WasmConstScalarValue, WasmRuntimeImport, wasm_const_op_policy,
    wasm_const_op_policy_for_opcode,
};
use crate::wasm_values::ConstantCache;
use molt_tir::tir::ops::{AttrValue, OpCode, TirOp};
use std::collections::BTreeMap;
use std::sync::Arc;
use wasm_encoder::{Function, Instruction};

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

    pub(in crate::wasm) fn inline_seed_bits(self, op: &OpIR) -> Option<i64> {
        (!matches!(self.inline_seed(), WasmConstInlineSeed::None))
            .then(|| self.0.required_simple_ir_inline_seed_bits(op))
    }

    pub(in crate::wasm) fn needs_dispatch_runtime_seed(self) -> bool {
        self.0.dispatch_runtime_seed
    }

    pub(in crate::wasm) fn required_tir_scalar_value(self, op: &TirOp) -> WasmConstScalarValue {
        self.0.required_tir_scalar_value(op)
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

    pub(in crate::wasm) fn apply_raw_int_effect(
        self,
        op: &OpIR,
        locals: &WasmFrameLocals,
        known_raw_ints: &mut BTreeMap<u32, i64>,
    ) {
        match self.raw_int_effect() {
            WasmConstRawIntEffect::SetInt => {
                let out = op.out.as_ref().expect("raw-int const out");
                let local_idx = locals[out];
                let val = op.value.expect("raw-int const value");
                known_raw_ints.insert(local_idx, val);
            }
            WasmConstRawIntEffect::Clear => forget_output_raw_int(op, locals, known_raw_ints),
        }
    }

    pub(in crate::wasm) fn simple_ir_materialization(
        self,
        op: &OpIR,
        locals: &WasmFrameLocals,
    ) -> WasmConstMaterialization {
        let out_name = op
            .out
            .as_ref()
            .unwrap_or_else(|| panic!("const op {} requires an output", self.0.kind));
        let out_local = locals[out_name];
        match self.literal_payload() {
            WasmConstLiteralPayload::None => WasmConstMaterialization::runtime_singleton(
                self.required_materializer_import(),
                out_local,
            ),
            payload => WasmConstMaterialization::literal(
                self.required_materializer_import(),
                out_local,
                payload,
                self.required_simple_ir_literal_bytes(op),
                locals.literal_scratch(out_name).into(),
            ),
        }
    }

    pub(in crate::wasm) fn tir_materialization(
        self,
        op: &TirOp,
        out_local: u32,
        scratch: Option<WasmConstMaterializationScratch>,
    ) -> WasmConstMaterialization {
        match self.literal_payload() {
            WasmConstLiteralPayload::None => WasmConstMaterialization::runtime_singleton(
                self.required_materializer_import(),
                out_local,
            ),
            payload => WasmConstMaterialization::literal(
                self.required_materializer_import(),
                out_local,
                payload,
                self.required_tir_literal_bytes(op),
                scratch.unwrap_or_else(|| {
                    panic!("const op {} requires literal scratch locals", self.0.kind)
                }),
            ),
        }
    }

    fn required_materializer_import(self) -> WasmRuntimeImport {
        self.materializer_import()
            .unwrap_or_else(|| panic!("const op {} has no materializer import", self.0.kind))
    }

    fn required_simple_ir_literal_bytes(self, op: &OpIR) -> Arc<[u8]> {
        match self.literal_payload() {
            WasmConstLiteralPayload::None => {
                panic!("const op {} has no literal payload", self.0.kind)
            }
            WasmConstLiteralPayload::String => {
                if let Some(bytes) = op.bytes.as_deref() {
                    Arc::from(bytes)
                } else {
                    Arc::from(
                        op.s_value
                            .as_ref()
                            .unwrap_or_else(|| panic!("const_str requires s_value or bytes"))
                            .as_bytes(),
                    )
                }
            }
            WasmConstLiteralPayload::BigintDecimal => Arc::from(
                op.s_value
                    .as_ref()
                    .unwrap_or_else(|| panic!("const_bigint requires decimal s_value"))
                    .as_bytes(),
            ),
            WasmConstLiteralPayload::Bytes => Arc::from(
                op.bytes
                    .as_deref()
                    .unwrap_or_else(|| panic!("const_bytes requires bytes payload")),
            ),
        }
    }

    fn required_tir_literal_bytes(self, op: &TirOp) -> Arc<[u8]> {
        match self.literal_payload() {
            WasmConstLiteralPayload::None => {
                panic!("const op {} has no literal payload", self.0.kind)
            }
            WasmConstLiteralPayload::String => match op.attrs.get("bytes") {
                Some(AttrValue::Bytes(bytes)) => Arc::from(bytes.as_slice()),
                _ => Arc::from(required_tir_str_attr(op, "s_value", self.0.kind).as_bytes()),
            },
            WasmConstLiteralPayload::BigintDecimal => {
                Arc::from(required_tir_str_attr(op, "s_value", self.0.kind).as_bytes())
            }
            WasmConstLiteralPayload::Bytes => {
                Arc::from(required_tir_bytes_attr(op, "bytes", self.0.kind))
            }
        }
    }
}

fn forget_output_raw_int(
    op: &OpIR,
    locals: &WasmFrameLocals,
    known_raw_ints: &mut BTreeMap<u32, i64>,
) {
    if let Some(out) = op.out.as_ref()
        && let Some(local_idx) = locals.get(out)
    {
        known_raw_ints.remove(local_idx);
    }
}

fn required_tir_str_attr<'a>(op: &'a TirOp, attr: &str, kind: &str) -> &'a str {
    match op.attrs.get(attr) {
        Some(AttrValue::Str(value)) => value.as_str(),
        _ => panic!("WASM const policy {kind} requires string attr {attr}"),
    }
}

fn required_tir_bytes_attr<'a>(op: &'a TirOp, attr: &str, kind: &str) -> &'a [u8] {
    match op.attrs.get(attr) {
        Some(AttrValue::Bytes(value)) => value.as_slice(),
        _ => panic!("WASM const policy {kind} requires bytes attr {attr}"),
    }
}

#[cfg(test)]
mod tests {
    use super::WasmConstOpPolicy;
    use crate::OpIR;
    use crate::wasm_abi_generated::{
        WasmConstInlineSeed, WasmConstLirFastPolicy, WasmConstLiteralPayload,
        WasmConstRawIntEffect, WasmRuntimeImport,
    };
    use crate::wasm_values::{box_bool, box_int, box_none};
    use molt_codegen_abi::box_float_bits as box_float;

    fn op(kind: &str) -> OpIR {
        OpIR {
            kind: kind.to_string(),
            ..OpIR::default()
        }
    }

    #[test]
    fn const_policy_classifies_inline_seed_bits() {
        let mut int_op = op("const");
        int_op.value = Some(7);
        let mut bool_op = op("const_bool");
        bool_op.value = Some(1);
        let mut float_op = op("const_float");
        float_op.f_value = Some(1.5);
        let none_op = op("const_none");

        assert_eq!(
            WasmConstOpPolicy::for_op(&int_op).map(|policy| policy.inline_seed()),
            Some(WasmConstInlineSeed::Int)
        );
        assert_eq!(
            WasmConstOpPolicy::for_op(&int_op).and_then(|policy| policy.inline_seed_bits(&int_op)),
            Some(box_int(7))
        );
        assert_eq!(
            WasmConstOpPolicy::for_op(&int_op).map(|policy| policy.raw_int_effect()),
            Some(WasmConstRawIntEffect::SetInt)
        );
        assert_eq!(
            WasmConstOpPolicy::for_op(&bool_op).map(|policy| policy.inline_seed()),
            Some(WasmConstInlineSeed::Bool)
        );
        assert_eq!(
            WasmConstOpPolicy::for_op(&bool_op)
                .and_then(|policy| policy.inline_seed_bits(&bool_op)),
            Some(box_bool(1))
        );
        assert_eq!(
            WasmConstOpPolicy::for_op(&float_op).map(|policy| policy.inline_seed()),
            Some(WasmConstInlineSeed::Float)
        );
        assert_eq!(
            WasmConstOpPolicy::for_op(&float_op)
                .and_then(|policy| policy.inline_seed_bits(&float_op)),
            Some(box_float(1.5))
        );
        assert_eq!(
            WasmConstOpPolicy::for_op(&none_op).map(|policy| policy.inline_seed()),
            Some(WasmConstInlineSeed::NoneValue)
        );
        assert_eq!(
            WasmConstOpPolicy::for_op(&none_op)
                .and_then(|policy| policy.inline_seed_bits(&none_op)),
            Some(box_none())
        );
    }

    #[test]
    fn const_policy_classifies_runtime_seed_and_literal_scratch() {
        for (kind, payload, import, parse_scalar, lir_policy) in [
            (
                "const_str",
                WasmConstLiteralPayload::String,
                WasmRuntimeImport::StringFromBytes,
                true,
                WasmConstLirFastPolicy::Materialize,
            ),
            (
                "const_bigint",
                WasmConstLiteralPayload::BigintDecimal,
                WasmRuntimeImport::BigintFromStr,
                false,
                WasmConstLirFastPolicy::Materialize,
            ),
            (
                "const_bytes",
                WasmConstLiteralPayload::Bytes,
                WasmRuntimeImport::BytesFromBytes,
                true,
                WasmConstLirFastPolicy::Materialize,
            ),
        ] {
            let policy = WasmConstOpPolicy::for_kind(kind).expect("literal const policy");
            assert!(
                policy.needs_literal_scratch(),
                "{kind} must allocate literal scratch"
            );
            assert_eq!(policy.literal_payload(), payload);
            assert_eq!(policy.materializer_import(), Some(import));
            assert_eq!(policy.parse_scalar_literal(), parse_scalar);
            assert_eq!(policy.lir_fast_policy(), lir_policy);
            assert!(
                policy.needs_dispatch_runtime_seed(),
                "{kind} must be materialized for dispatch seeds"
            );
        }

        for kind in ["const_not_implemented", "const_ellipsis"] {
            let policy = WasmConstOpPolicy::for_kind(kind).expect("runtime singleton policy");
            assert!(
                !policy.needs_literal_scratch(),
                "{kind} must not allocate literal scratch"
            );
            assert_eq!(policy.literal_payload(), WasmConstLiteralPayload::None);
            assert!(policy.materializer_import().is_some());
            assert_eq!(
                policy.lir_fast_policy(),
                WasmConstLirFastPolicy::Materialize
            );
            assert!(
                policy.needs_dispatch_runtime_seed(),
                "{kind} must be materialized for dispatch seeds"
            );
        }
    }

    #[test]
    fn const_policy_rejects_non_const_ops() {
        assert_eq!(WasmConstOpPolicy::for_kind("add"), None);
        assert_eq!(WasmConstOpPolicy::for_kind("parse_int"), None);
    }

    #[test]
    #[should_panic(expected = "WASM const policy const requires int scalar payload")]
    fn const_policy_fails_closed_on_missing_scalar_payload() {
        let const_op = op("const");
        let policy = WasmConstOpPolicy::for_op(&const_op).expect("const policy");

        let _ = policy.inline_seed_bits(&const_op);
    }
}
