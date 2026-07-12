mod binary;
mod raw;

use super::common::{emit_boxed_binary_result, emit_boxed_ternary_result, emit_boxed_unary_result};
use crate::OpIR;
use crate::representation_plan::ScalarRepresentationPlan;
use crate::wasm::{WasmFrameLocals, WasmNumericLaneStats};
use crate::wasm_abi_generated::{WasmNumericOpLoopKind, WasmNumericRuntimeSelection};
use crate::wasm_import_tracking::TrackedImportIds;
use crate::wasm_values::ConstantCache;
use std::collections::BTreeMap;
use wasm_encoder::Function;

#[allow(unused_variables)]
pub(super) fn emit_division_numeric_op(
    func: &mut Function,
    op: &OpIR,
    selection: WasmNumericRuntimeSelection,
    import_ids: &TrackedImportIds,
    locals: &WasmFrameLocals,
    const_cache: &ConstantCache,
    scalar_plan: &ScalarRepresentationPlan,
    reloc_enabled: bool,
    known_raw_ints: &BTreeMap<u32, i64>,
    numeric_lane_stats: &mut WasmNumericLaneStats,
) {
    match selection.op_loop_kind {
        WasmNumericOpLoopKind::TrueDiv
        | WasmNumericOpLoopKind::FloorDiv
        | WasmNumericOpLoopKind::Mod => binary::emit_division_binary_op(
            func,
            op,
            import_ids,
            locals,
            const_cache,
            scalar_plan,
            reloc_enabled,
            known_raw_ints,
            numeric_lane_stats,
            selection.op_loop_kind,
            selection.import,
        ),
        WasmNumericOpLoopKind::Matmul | WasmNumericOpLoopKind::Pow => {
            numeric_lane_stats.record_op_loop_division_boxed_runtime_site();
            emit_boxed_binary_result(
                func,
                op,
                import_ids,
                locals,
                selection.import,
                reloc_enabled,
            );
        }
        WasmNumericOpLoopKind::PowMod | WasmNumericOpLoopKind::Round => {
            numeric_lane_stats.record_op_loop_division_boxed_runtime_site();
            emit_boxed_ternary_result(
                func,
                op,
                import_ids,
                locals,
                selection.import,
                reloc_enabled,
            );
        }
        WasmNumericOpLoopKind::Trunc => {
            numeric_lane_stats.record_op_loop_division_boxed_runtime_site();
            emit_boxed_unary_result(
                func,
                op,
                import_ids,
                locals,
                selection.import,
                reloc_enabled,
            );
        }
        _ => unreachable!("non-division numeric selector routed to division emitter"),
    }
}
