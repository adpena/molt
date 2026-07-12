use crate::OpIR;
use crate::wasm::const_materialization::WasmConstOpPolicy;
use crate::wasm::{WasmBackend, WasmFrameLocals};
use crate::wasm_abi_generated::WasmConstInlineSeed;
use crate::wasm_data::DataSegmentRef;
use crate::wasm_import_tracking::TrackedImportIds;
use wasm_encoder::Function;

impl WasmConstOpPolicy {
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

pub(in crate::wasm) fn emit_seeded_runtime_const_op(
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
