use crate::OpIR;
use crate::wasm::const_materialization::WasmConstOpPolicy;
use crate::wasm::frame_locals::WasmFrameLocalKind;
use crate::wasm::function_frame::{WasmConstAnchorRegion, WasmConstAnchorSlot};
use crate::wasm::{WasmBackend, WasmFrameLocals, WasmFrameSyntheticLocal};
use crate::wasm_binary::Function;
use crate::wasm_data::DataSegmentRef;
use crate::wasm_import_tracking::TrackedImportIds;

impl WasmConstOpPolicy {
    fn emit_anchor_materialization(
        self,
        backend: &mut WasmBackend,
        func: &mut Function,
        op: &OpIR,
        func_index: u32,
        reloc_enabled: bool,
        import_ids: &TrackedImportIds,
        const_str_scratch_segment: DataSegmentRef,
        region: WasmConstAnchorRegion,
        slot: WasmConstAnchorSlot,
    ) {
        if self.inline_seed_bits(op).is_some() {
            panic!("inline const op {} does not need a runtime anchor", op.kind);
        }
        self.simple_ir_materialization_into(op, region.temporary())
            .emit_with_imports(
                backend,
                func,
                func_index,
                reloc_enabled,
                import_ids,
                const_str_scratch_segment,
            );
        // Publish the constructor owner before the pending-exception check.
        region.emit_store_temporary(func, slot);
    }
}

pub(in crate::wasm) fn emit_const_anchor_materialization(
    backend: &mut WasmBackend,
    func: &mut Function,
    op: &OpIR,
    func_index: u32,
    reloc_enabled: bool,
    import_ids: &TrackedImportIds,
    const_str_scratch_segment: DataSegmentRef,
    region: WasmConstAnchorRegion,
    slot: WasmConstAnchorSlot,
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
        func_index,
        reloc_enabled,
        import_ids,
        const_str_scratch_segment,
        region,
        slot,
    );
}

pub(in crate::wasm) fn emit_const_anchor_result(
    func: &mut Function,
    op: &OpIR,
    locals: &WasmFrameLocals,
    region: WasmConstAnchorRegion,
    slot: WasmConstAnchorSlot,
    import_ids: &TrackedImportIds,
    reloc_enabled: bool,
) {
    let out_name = op
        .out
        .as_ref()
        .unwrap_or_else(|| panic!("anchored const op {} requires an output", op.kind));
    let out_local = locals.result_slot(out_name);
    region.emit_load(func, slot);
    crate::wasm::refcount::emit_refcount(
        func,
        reloc_enabled,
        import_ids,
        locals.refcount(),
        molt_codegen_abi::inline_refcount::RefcountOperation::Retain,
    );
    region.emit_load(func, slot);
    func.instruction(&wasm_encoder::Instruction::LocalSet(out_local));

    if locals.bound_result_slot(Some(out_name)).is_none()
        || matches!(
            locals.local_kind(out_name),
            Some(WasmFrameLocalKind::FixedSynthetic(
                WasmFrameSyntheticLocal::DeadSink
            ))
        )
    {
        func.instruction(&wasm_encoder::Instruction::LocalGet(out_local));
        crate::wasm::refcount::emit_refcount(
            func,
            reloc_enabled,
            import_ids,
            locals.refcount(),
            molt_codegen_abi::inline_refcount::RefcountOperation::Release,
        );
    }
}
