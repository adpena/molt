use crate::OpIR;
use crate::wasm_abi_generated::{
    WasmConstInlineSeed, WasmConstLirFastPolicy, WasmConstLiteralPayload, WasmConstOpPolicySpec,
    WasmConstRawIntEffect, WasmConstScalarValue, wasm_const_op_policy,
    wasm_const_op_policy_for_opcode,
};
use crate::wasm_binary::emit_call;
use crate::wasm_data::DataSegmentRef;
use crate::wasm_import_tracking::TrackedImportIds;
use crate::wasm_values::ConstantCache;
use molt_tir::tir::ops::{AttrValue, OpCode, TirOp};
use std::collections::BTreeMap;
use std::sync::Arc;
use wasm_encoder::{Function, Instruction};

use super::WasmBackend;
use super::frame_locals::{WasmFrameLocals, WasmLiteralScratchLocals};

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

    pub(in crate::wasm) fn materializer_import(self) -> Option<&'static str> {
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
        let payload = match self.literal_payload() {
            WasmConstLiteralPayload::None => WasmConstMaterializationPayload::RuntimeSingleton,
            payload => WasmConstMaterializationPayload::Literal {
                payload,
                bytes: self.required_simple_ir_literal_bytes(op),
                scratch: locals.literal_scratch(out_name).into(),
            },
        };
        WasmConstMaterialization {
            import_name: self.required_materializer_import(),
            out_local,
            payload,
        }
    }

    pub(in crate::wasm) fn tir_materialization(
        self,
        op: &TirOp,
        out_local: u32,
        scratch: Option<WasmConstMaterializationScratch>,
    ) -> WasmConstMaterialization {
        let payload = match self.literal_payload() {
            WasmConstLiteralPayload::None => WasmConstMaterializationPayload::RuntimeSingleton,
            payload => WasmConstMaterializationPayload::Literal {
                payload,
                bytes: self.required_tir_literal_bytes(op),
                scratch: scratch.unwrap_or_else(|| {
                    panic!("const op {} requires literal scratch locals", self.0.kind)
                }),
            },
        };
        WasmConstMaterialization {
            import_name: self.required_materializer_import(),
            out_local,
            payload,
        }
    }

    fn required_materializer_import(self) -> &'static str {
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

#[derive(Debug, Clone)]
pub(crate) struct WasmConstMaterialization {
    import_name: &'static str,
    out_local: u32,
    payload: WasmConstMaterializationPayload,
}

impl WasmConstMaterialization {
    pub(crate) fn runtime_import(&self) -> &'static str {
        self.import_name
    }

    pub(crate) fn emit(
        &self,
        backend: &mut WasmBackend,
        func: &mut Function,
        func_index: u32,
        reloc_enabled: bool,
        import_id: u32,
        const_str_scratch_segment: DataSegmentRef,
    ) {
        match &self.payload {
            WasmConstMaterializationPayload::RuntimeSingleton => {
                emit_call(func, reloc_enabled, import_id);
                func.instruction(&Instruction::LocalSet(self.out_local));
            }
            WasmConstMaterializationPayload::Literal {
                payload,
                bytes,
                scratch,
            } => emit_literal_materialization(
                backend,
                func,
                func_index,
                reloc_enabled,
                import_id,
                const_str_scratch_segment,
                self.out_local,
                *payload,
                bytes,
                *scratch,
            ),
        }
    }

    pub(in crate::wasm) fn emit_with_imports(
        &self,
        backend: &mut WasmBackend,
        func: &mut Function,
        func_index: u32,
        reloc_enabled: bool,
        import_ids: &TrackedImportIds,
        const_str_scratch_segment: DataSegmentRef,
    ) {
        let import_id = import_ids[self.runtime_import()];
        self.emit(
            backend,
            func,
            func_index,
            reloc_enabled,
            import_id,
            const_str_scratch_segment,
        );
    }
}

#[derive(Debug, Clone)]
enum WasmConstMaterializationPayload {
    RuntimeSingleton,
    Literal {
        payload: WasmConstLiteralPayload,
        bytes: Arc<[u8]>,
        scratch: WasmConstMaterializationScratch,
    },
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct WasmConstMaterializationScratch {
    ptr_local: u32,
    len_local: u32,
}

impl WasmConstMaterializationScratch {
    pub(crate) fn new(ptr_local: u32, len_local: u32) -> Self {
        Self {
            ptr_local,
            len_local,
        }
    }
}

impl From<WasmLiteralScratchLocals> for WasmConstMaterializationScratch {
    fn from(scratch: WasmLiteralScratchLocals) -> Self {
        Self::new(scratch.ptr_local(), scratch.len_local())
    }
}

fn emit_literal_materialization(
    backend: &mut WasmBackend,
    func: &mut Function,
    func_index: u32,
    reloc_enabled: bool,
    import_id: u32,
    scratch_segment: DataSegmentRef,
    out_local: u32,
    payload: WasmConstLiteralPayload,
    bytes: &[u8],
    scratch: WasmConstMaterializationScratch,
) {
    emit_literal_ptr_len(backend, func, func_index, reloc_enabled, bytes, scratch);
    match payload {
        WasmConstLiteralPayload::String | WasmConstLiteralPayload::Bytes => {
            func.instruction(&Instruction::LocalGet(scratch.ptr_local));
            func.instruction(&Instruction::I32WrapI64);
            func.instruction(&Instruction::LocalGet(scratch.len_local));
            backend.emit_data_ptr_i32(reloc_enabled, func_index, func, scratch_segment);
            emit_call(func, reloc_enabled, import_id);
            func.instruction(&Instruction::Drop);

            backend.emit_data_ptr_i32(reloc_enabled, func_index, func, scratch_segment);
            func.instruction(&Instruction::I64Load(wasm_encoder::MemArg {
                align: 3,
                offset: 0,
                memory_index: 0,
            }));
            func.instruction(&Instruction::LocalSet(out_local));
        }
        WasmConstLiteralPayload::BigintDecimal => {
            func.instruction(&Instruction::LocalGet(scratch.ptr_local));
            func.instruction(&Instruction::I32WrapI64);
            func.instruction(&Instruction::LocalGet(scratch.len_local));
            emit_call(func, reloc_enabled, import_id);
            func.instruction(&Instruction::LocalSet(out_local));
        }
        WasmConstLiteralPayload::None => unreachable!("literal materialization checked above"),
    }
}

fn emit_literal_ptr_len(
    backend: &mut WasmBackend,
    func: &mut Function,
    func_index: u32,
    reloc_enabled: bool,
    bytes: &[u8],
    scratch: WasmConstMaterializationScratch,
) {
    let data = backend.add_data_segment(reloc_enabled, bytes);
    backend.emit_data_ptr(reloc_enabled, func_index, func, data);
    func.instruction(&Instruction::LocalSet(scratch.ptr_local));
    func.instruction(&Instruction::I64Const(bytes.len() as i64));
    func.instruction(&Instruction::LocalSet(scratch.len_local));
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
