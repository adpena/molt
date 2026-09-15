use crate::OpIR;
use crate::wasm::const_materialization::WasmConstOpPolicy;
use crate::wasm::frame_locals::WasmFrameLocalKind;
use crate::wasm::{WasmBackend, WasmFrameLocals, WasmFrameSyntheticLocal};
use crate::wasm_abi_generated::WasmRuntimeImport;
use crate::wasm_binary::emit_call;
use crate::wasm_data::DataSegmentRef;
use crate::wasm_import_tracking::TrackedImportIds;
use wasm_encoder::Function;

impl WasmConstOpPolicy {
    fn emit_anchor_materialization(
        self,
        backend: &mut WasmBackend,
        func: &mut Function,
        op: &OpIR,
        locals: &WasmFrameLocals,
        func_index: u32,
        reloc_enabled: bool,
        import_ids: &TrackedImportIds,
        const_str_scratch_segment: DataSegmentRef,
        anchor_local: u32,
    ) {
        if self.inline_seed_bits(op).is_some() {
            panic!("inline const op {} does not need a runtime anchor", op.kind);
        }
        self.simple_ir_materialization_into(op, locals, anchor_local)
            .emit_with_imports(
                backend,
                func,
                func_index,
                reloc_enabled,
                import_ids,
                const_str_scratch_segment,
            );
    }
}

pub(in crate::wasm) fn emit_const_anchor_materialization(
    backend: &mut WasmBackend,
    func: &mut Function,
    op: &OpIR,
    locals: &WasmFrameLocals,
    func_index: u32,
    reloc_enabled: bool,
    import_ids: &TrackedImportIds,
    const_str_scratch_segment: DataSegmentRef,
    anchor_local: u32,
) {
    let policy = WasmConstOpPolicy::for_op(op)
        .unwrap_or_else(|| panic!("unsupported anchored runtime const op {}", op.kind));
    assert!(
        policy.needs_runtime_anchor(),
        "const op {} does not need a runtime anchor",
        op.kind
    );
    policy.emit_anchor_materialization(
        backend,
        func,
        op,
        locals,
        func_index,
        reloc_enabled,
        import_ids,
        const_str_scratch_segment,
        anchor_local,
    );
}

pub(in crate::wasm) fn emit_const_anchor_result(
    func: &mut Function,
    op: &OpIR,
    locals: &WasmFrameLocals,
    anchor_local: u32,
    import_ids: &TrackedImportIds,
    reloc_enabled: bool,
) {
    let out_name = op
        .out
        .as_ref()
        .unwrap_or_else(|| panic!("anchored const op {} requires an output", op.kind));
    let out_local = locals[out_name];
    func.instruction(&wasm_encoder::Instruction::LocalGet(anchor_local));
    emit_call(
        func,
        reloc_enabled,
        import_ids[WasmRuntimeImport::IncRefObj],
    );
    func.instruction(&wasm_encoder::Instruction::LocalGet(anchor_local));
    func.instruction(&wasm_encoder::Instruction::LocalSet(out_local));

    if matches!(
        locals.local_kind(out_name),
        Some(WasmFrameLocalKind::FixedSynthetic(
            WasmFrameSyntheticLocal::DeadSink
        ))
    ) {
        func.instruction(&wasm_encoder::Instruction::LocalGet(out_local));
        emit_call(
            func,
            reloc_enabled,
            import_ids[WasmRuntimeImport::DecRefObj],
        );
    }
}

pub(in crate::wasm) fn emit_release_const_anchors(
    func: &mut Function,
    anchor_locals: impl DoubleEndedIterator<Item = u32>,
    import_ids: &TrackedImportIds,
    reloc_enabled: bool,
) {
    for anchor_local in anchor_locals.rev() {
        func.instruction(&wasm_encoder::Instruction::LocalGet(anchor_local));
        emit_call(
            func,
            reloc_enabled,
            import_ids[WasmRuntimeImport::DecRefObj],
        );
    }
}
