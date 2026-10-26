mod anchors;

use super::const_materialization::WasmConstOpPolicy;
use super::context::CompileFuncContext;
use super::function_frame::{WasmConstAnchorRegion, WasmConstAnchorSlot};
use super::{WasmBackend, WasmFrameLocals};
use crate::OpIR;
use crate::wasm_data::DataSegmentRef;
use crate::wasm_import_tracking::TrackedImportIds;
use crate::wasm_values::ConstantCache;
use std::collections::BTreeMap;
use wasm_encoder::Function;

pub(in crate::wasm) use anchors::{emit_const_anchor_materialization, emit_const_anchor_result};

pub(super) struct ConstantOpContext<'a, 'ctx> {
    pub(super) backend: &'a mut WasmBackend,
    pub(super) ctx: &'a CompileFuncContext<'ctx>,
    pub(super) import_ids: &'a TrackedImportIds,
    pub(super) locals: &'a WasmFrameLocals,
    pub(super) const_cache: &'a ConstantCache,
    pub(super) func_index: u32,
    pub(super) reloc_enabled: bool,
    pub(super) anchor: Option<(WasmConstAnchorRegion, WasmConstAnchorSlot)>,
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
            anchor,
        } = context;

        if let Some((region, slot)) = anchor {
            emit_const_anchor_result(func, op, locals, region, slot, import_ids, reloc_enabled);
        } else if !self.emit_inline_seed(func, op, locals, const_cache) {
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
        if anchor.is_some() {
            self.clear_raw_int_output(op, locals, known_raw_ints);
        } else {
            self.apply_raw_int_effect(op, locals, known_raw_ints);
        }
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
