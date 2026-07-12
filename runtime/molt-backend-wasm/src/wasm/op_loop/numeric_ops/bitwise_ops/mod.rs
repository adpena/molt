mod shift;
mod simple;

use super::common::emit_boxed_unary_result;
use crate::OpIR;
use crate::representation_plan::ScalarRepresentationPlan;
use crate::wasm::{WasmFrameLocals, WasmNumericLaneStats};
use crate::wasm_abi_generated::{WasmNumericOpLoopKind, WasmNumericRuntimeSelection};
use crate::wasm_import_tracking::TrackedImportIds;
use crate::wasm_values::ConstantCache;
use std::collections::BTreeMap;
use wasm_encoder::Function;

#[allow(unused_variables)]
pub(super) fn emit_bitwise_numeric_op(
    func: &mut Function,
    op: &OpIR,
    op_idx: usize,
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
        WasmNumericOpLoopKind::BitAnd
        | WasmNumericOpLoopKind::BitOr
        | WasmNumericOpLoopKind::BitXor => simple::emit_simple_bitwise_op(
            func,
            op,
            import_ids,
            locals,
            const_cache,
            scalar_plan,
            reloc_enabled,
            known_raw_ints,
            numeric_lane_stats,
            op_idx,
            selection.op_loop_kind,
            selection.import,
        ),
        WasmNumericOpLoopKind::Invert | WasmNumericOpLoopKind::Neg | WasmNumericOpLoopKind::Pos => {
            numeric_lane_stats.record_op_loop_bitwise_boxed_runtime_site();
            emit_boxed_unary_result(
                func,
                op,
                import_ids,
                locals,
                selection.import,
                reloc_enabled,
            )
        }
        WasmNumericOpLoopKind::LShift => shift::emit_shift_op(
            func,
            op,
            import_ids,
            locals,
            const_cache,
            scalar_plan,
            reloc_enabled,
            known_raw_ints,
            numeric_lane_stats,
            selection.import,
            shift::ShiftDirection::Left,
        ),
        WasmNumericOpLoopKind::RShift => shift::emit_shift_op(
            func,
            op,
            import_ids,
            locals,
            const_cache,
            scalar_plan,
            reloc_enabled,
            known_raw_ints,
            numeric_lane_stats,
            selection.import,
            shift::ShiftDirection::Right,
        ),
        _ => unreachable!("non-bitwise numeric selector routed to bitwise emitter"),
    }
}
