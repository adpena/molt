use super::context::CompileFuncContext;
use super::*;
#[cfg(test)]
use crate::wasm_abi_generated::WasmConstLirFastPolicy;
use crate::wasm_abi_generated::{
    WasmConstInlineSeed, WasmConstLiteralPayload, WasmConstOpPolicySpec, WasmConstRawIntEffect,
    wasm_const_op_policy,
};

pub(super) struct ConstantOpContext<'a, 'ctx> {
    pub(super) backend: &'a mut WasmBackend,
    pub(super) ctx: &'a CompileFuncContext<'ctx>,
    pub(super) import_ids: &'a TrackedImportIds,
    pub(super) locals: &'a WasmFrameLocals,
    pub(super) const_cache: &'a ConstantCache,
    pub(super) func_index: u32,
    pub(super) reloc_enabled: bool,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) struct WasmConstOpPolicy(&'static WasmConstOpPolicySpec);

impl WasmConstOpPolicy {
    pub(super) fn for_op(op: &OpIR) -> Option<Self> {
        Self::for_kind(op.kind.as_str())
    }

    pub(super) fn for_kind(kind: &str) -> Option<Self> {
        wasm_const_op_policy(kind).map(Self)
    }

    pub(super) fn inline_seed(self) -> WasmConstInlineSeed {
        self.0.inline_seed
    }

    pub(super) fn literal_payload(self) -> WasmConstLiteralPayload {
        self.0.literal_payload
    }

    pub(super) fn parse_scalar_literal(self) -> bool {
        self.0.parse_scalar_literal
    }

    pub(super) fn materializer_import(self) -> Option<&'static str> {
        self.0.materializer_import
    }

    pub(super) fn raw_int_effect(self) -> WasmConstRawIntEffect {
        self.0.raw_int_effect
    }

    #[cfg(test)]
    pub(super) fn lir_fast_policy(self) -> WasmConstLirFastPolicy {
        self.0.lir_fast
    }

    pub(super) fn needs_literal_scratch(self) -> bool {
        !matches!(self.literal_payload(), WasmConstLiteralPayload::None)
    }

    pub(super) fn inline_seed_bits(self, op: &OpIR) -> Option<i64> {
        (!matches!(self.inline_seed(), WasmConstInlineSeed::None))
            .then(|| self.0.required_simple_ir_inline_seed_bits(op))
    }

    pub(super) fn needs_dispatch_runtime_seed(self) -> bool {
        self.0.dispatch_runtime_seed
    }

    fn required_materializer_import(self) -> &'static str {
        self.materializer_import()
            .unwrap_or_else(|| panic!("const op {} has no materializer import", self.0.kind))
    }

    fn emit_inline_seed(
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

    fn apply_raw_int_effect(
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

    fn emit_materialized(
        self,
        backend: &mut WasmBackend,
        func: &mut Function,
        op: &OpIR,
        locals: &WasmFrameLocals,
        func_index: u32,
        reloc_enabled: bool,
        import_ids: &TrackedImportIds,
        const_str_scratch_segment: DataSegmentRef,
    ) {
        let import_id = import_ids[self.required_materializer_import()];
        match self.literal_payload() {
            WasmConstLiteralPayload::None => {
                emit_runtime_singleton(func, op, locals, reloc_enabled, import_id);
            }
            WasmConstLiteralPayload::String => {
                emit_const_str(
                    backend,
                    func,
                    op,
                    locals,
                    func_index,
                    reloc_enabled,
                    import_id,
                    const_str_scratch_segment,
                );
            }
            WasmConstLiteralPayload::BigintDecimal => {
                emit_const_bigint(
                    backend,
                    func,
                    op,
                    locals,
                    func_index,
                    reloc_enabled,
                    import_id,
                );
            }
            WasmConstLiteralPayload::Bytes => {
                emit_const_bytes(
                    backend,
                    func,
                    op,
                    locals,
                    func_index,
                    reloc_enabled,
                    import_id,
                    const_str_scratch_segment,
                );
            }
        }
    }

    fn emit(
        self,
        context: ConstantOpContext<'_, '_>,
        func: &mut Function,
        op: &OpIR,
        known_raw_ints: &mut BTreeMap<u32, i64>,
    ) {
        let ConstantOpContext {
            backend,
            ctx,
            import_ids,
            locals,
            const_cache,
            func_index,
            reloc_enabled,
        } = context;

        if !self.emit_inline_seed(func, op, locals, const_cache) {
            self.emit_materialized(
                backend,
                func,
                op,
                locals,
                func_index,
                reloc_enabled,
                import_ids,
                ctx.const_str_scratch_segment,
            );
        }
        self.apply_raw_int_effect(op, locals, known_raw_ints);
    }

    fn emit_seeded_runtime(
        self,
        backend: &mut WasmBackend,
        func: &mut Function,
        op: &OpIR,
        locals: &WasmFrameLocals,
        func_index: u32,
        reloc_enabled: bool,
        import_ids: &TrackedImportIds,
        const_str_scratch_segment: DataSegmentRef,
    ) {
        if !matches!(self.inline_seed(), WasmConstInlineSeed::None) {
            panic!("inline const op {} does not need runtime seeding", op.kind);
        }
        self.emit_materialized(
            backend,
            func,
            op,
            locals,
            func_index,
            reloc_enabled,
            import_ids,
            const_str_scratch_segment,
        );
    }
}

pub(super) fn emit_constant_op(
    context: ConstantOpContext<'_, '_>,
    func: &mut Function,
    op: &OpIR,
    known_raw_ints: &mut BTreeMap<u32, i64>,
) -> bool {
    let Some(policy) = WasmConstOpPolicy::for_op(op) else {
        return false;
    };
    policy.emit(context, func, op, known_raw_ints);
    true
}

pub(super) fn emit_seeded_runtime_const_op(
    backend: &mut WasmBackend,
    func: &mut Function,
    op: &OpIR,
    locals: &WasmFrameLocals,
    func_index: u32,
    reloc_enabled: bool,
    import_ids: &TrackedImportIds,
    const_str_scratch_segment: DataSegmentRef,
) {
    let policy = WasmConstOpPolicy::for_op(op)
        .unwrap_or_else(|| panic!("unsupported seeded runtime const op {}", op.kind));
    assert!(
        policy.needs_dispatch_runtime_seed(),
        "const op {} does not need runtime seeding",
        op.kind
    );
    policy.emit_seeded_runtime(
        backend,
        func,
        op,
        locals,
        func_index,
        reloc_enabled,
        import_ids,
        const_str_scratch_segment,
    );
}

fn emit_runtime_singleton(
    func: &mut Function,
    op: &OpIR,
    locals: &WasmFrameLocals,
    reloc_enabled: bool,
    import_id: u32,
) {
    emit_call(func, reloc_enabled, import_id);
    let local_idx = locals[op.out.as_ref().expect("runtime const out")];
    func.instruction(&Instruction::LocalSet(local_idx));
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

fn const_str_bytes(op: &OpIR) -> (&str, &[u8]) {
    let out_name = op.out.as_ref().expect("const_str out");
    let bytes = op
        .bytes
        .as_deref()
        .unwrap_or_else(|| op.s_value.as_ref().expect("const_str bytes").as_bytes());
    (out_name, bytes)
}

fn const_bigint_bytes(op: &OpIR) -> (&str, &[u8]) {
    let out_name = op.out.as_ref().expect("const_bigint out");
    let bytes = op.s_value.as_ref().expect("const_bigint string").as_bytes();
    (out_name, bytes)
}

fn const_bytes_bytes(op: &OpIR) -> (&str, &[u8]) {
    (
        op.out.as_ref().expect("const_bytes out"),
        op.bytes.as_deref().expect("const_bytes bytes"),
    )
}

fn emit_literal_ptr_len(
    backend: &mut WasmBackend,
    func: &mut Function,
    out_name: &str,
    bytes: &[u8],
    locals: &WasmFrameLocals,
    func_index: u32,
    reloc_enabled: bool,
) -> WasmLiteralScratchLocals {
    let data = backend.add_data_segment(reloc_enabled, bytes);
    let scratch = locals.literal_scratch(out_name);
    backend.emit_data_ptr(reloc_enabled, func_index, func, data);
    func.instruction(&Instruction::LocalSet(scratch.ptr_local()));
    func.instruction(&Instruction::I64Const(bytes.len() as i64));
    func.instruction(&Instruction::LocalSet(scratch.len_local()));
    scratch
}

fn emit_scratch_materialized_bytes(
    backend: &mut WasmBackend,
    func: &mut Function,
    out_name: &str,
    bytes: &[u8],
    locals: &WasmFrameLocals,
    func_index: u32,
    reloc_enabled: bool,
    import_id: u32,
    scratch_segment: DataSegmentRef,
) {
    let scratch = emit_literal_ptr_len(
        backend,
        func,
        out_name,
        bytes,
        locals,
        func_index,
        reloc_enabled,
    );
    func.instruction(&Instruction::LocalGet(scratch.ptr_local()));
    func.instruction(&Instruction::I32WrapI64);
    func.instruction(&Instruction::LocalGet(scratch.len_local()));
    backend.emit_data_ptr_i32(reloc_enabled, func_index, func, scratch_segment);
    emit_call(func, reloc_enabled, import_id);
    func.instruction(&Instruction::Drop);

    let out_local = locals[out_name];
    backend.emit_data_ptr_i32(reloc_enabled, func_index, func, scratch_segment);
    func.instruction(&Instruction::I64Load(wasm_encoder::MemArg {
        align: 3,
        offset: 0,
        memory_index: 0,
    }));
    func.instruction(&Instruction::LocalSet(out_local));
}

fn emit_const_str(
    backend: &mut WasmBackend,
    func: &mut Function,
    op: &OpIR,
    locals: &WasmFrameLocals,
    func_index: u32,
    reloc_enabled: bool,
    import_id: u32,
    scratch_segment: DataSegmentRef,
) {
    let (out_name, bytes) = const_str_bytes(op);
    emit_scratch_materialized_bytes(
        backend,
        func,
        out_name,
        bytes,
        locals,
        func_index,
        reloc_enabled,
        import_id,
        scratch_segment,
    );
}

fn emit_const_bigint(
    backend: &mut WasmBackend,
    func: &mut Function,
    op: &OpIR,
    locals: &WasmFrameLocals,
    func_index: u32,
    reloc_enabled: bool,
    import_id: u32,
) {
    let (out_name, bytes) = const_bigint_bytes(op);
    let scratch = emit_literal_ptr_len(
        backend,
        func,
        out_name,
        bytes,
        locals,
        func_index,
        reloc_enabled,
    );
    func.instruction(&Instruction::LocalGet(scratch.ptr_local()));
    func.instruction(&Instruction::I32WrapI64);
    func.instruction(&Instruction::LocalGet(scratch.len_local()));
    emit_call(func, reloc_enabled, import_id);
    let out_local = locals[out_name];
    func.instruction(&Instruction::LocalSet(out_local));
}

fn emit_const_bytes(
    backend: &mut WasmBackend,
    func: &mut Function,
    op: &OpIR,
    locals: &WasmFrameLocals,
    func_index: u32,
    reloc_enabled: bool,
    import_id: u32,
    scratch_segment: DataSegmentRef,
) {
    let (out_name, bytes) = const_bytes_bytes(op);
    emit_scratch_materialized_bytes(
        backend,
        func,
        out_name,
        bytes,
        locals,
        func_index,
        reloc_enabled,
        import_id,
        scratch_segment,
    );
}

#[cfg(test)]
mod tests {
    use super::*;

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
        for (kind, payload, import_name, parse_scalar, lir_policy) in [
            (
                "const_str",
                WasmConstLiteralPayload::String,
                "string_from_bytes",
                true,
                WasmConstLirFastPolicy::BailGeneric,
            ),
            (
                "const_bigint",
                WasmConstLiteralPayload::BigintDecimal,
                "bigint_from_str",
                false,
                WasmConstLirFastPolicy::BailGeneric,
            ),
            (
                "const_bytes",
                WasmConstLiteralPayload::Bytes,
                "bytes_from_bytes",
                true,
                WasmConstLirFastPolicy::BailGeneric,
            ),
        ] {
            let policy = WasmConstOpPolicy::for_kind(kind).expect("literal const policy");
            assert!(
                policy.needs_literal_scratch(),
                "{kind} must allocate literal scratch"
            );
            assert_eq!(policy.literal_payload(), payload);
            assert_eq!(policy.materializer_import(), Some(import_name));
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
