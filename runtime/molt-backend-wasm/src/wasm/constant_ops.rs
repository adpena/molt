use super::const_materialization::WasmConstOpPolicy;
use super::context::CompileFuncContext;
use super::{WasmBackend, WasmFrameLocals};
use crate::OpIR;
use crate::wasm_abi_generated::WasmConstInlineSeed;
use crate::wasm_data::DataSegmentRef;
use crate::wasm_import_tracking::TrackedImportIds;
use crate::wasm_values::ConstantCache;
use std::collections::BTreeMap;
use wasm_encoder::Function;

pub(super) struct ConstantOpContext<'a, 'ctx> {
    pub(super) backend: &'a mut WasmBackend,
    pub(super) ctx: &'a CompileFuncContext<'ctx>,
    pub(super) import_ids: &'a TrackedImportIds,
    pub(super) locals: &'a WasmFrameLocals,
    pub(super) const_cache: &'a ConstantCache,
    pub(super) func_index: u32,
    pub(super) reloc_enabled: bool,
}

impl WasmConstOpPolicy {
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
        self.simple_ir_materialization(op, locals)
            .emit_with_imports(
                backend,
                func,
                func_index,
                reloc_enabled,
                import_ids,
                const_str_scratch_segment,
            );
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
