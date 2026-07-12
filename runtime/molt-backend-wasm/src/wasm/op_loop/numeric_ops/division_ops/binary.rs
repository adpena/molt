use super::super::common::{
    binary_operands, emit_boxed_binary_call, emit_guarded_int_binary_result_or_boxed,
    emit_plain_f64_arithmetic_result, emit_plain_f64_binary_result_or_boxed, int_binary_temps,
    store_numeric_result,
};
use super::raw::emit_nonzero_rhs_raw_division_or_boxed;
use crate::OpIR;
use crate::representation_plan::ScalarRepresentationPlan;
use crate::wasm::{WasmFrameLocals, WasmFrameSyntheticLocal, WasmNumericLaneStats};
use crate::wasm_abi_generated::WasmNumericOpLoopKind;
use crate::wasm_import_tracking::TrackedImportIds;
use crate::wasm_plan::wasm_scalar_integer_fast_path_for_op;
use crate::wasm_values::{
    ConstantCache, IntFastLane, emit_unbox_int_local_trusted_opt,
    emit_unbox_int_local_trusted_tee_opt,
};
use std::collections::BTreeMap;
use wasm_encoder::{Function, Instruction};

pub(super) fn emit_division_binary_op(
    func: &mut Function,
    op: &OpIR,
    import_ids: &TrackedImportIds,
    locals: &WasmFrameLocals,
    const_cache: &ConstantCache,
    scalar_plan: &ScalarRepresentationPlan,
    reloc_enabled: bool,
    known_raw_ints: &BTreeMap<u32, i64>,
    numeric_lane_stats: &mut WasmNumericLaneStats,
    division_op: WasmNumericOpLoopKind,
    import_name: crate::wasm_abi_generated::WasmRuntimeImport,
) {
    let operands = binary_operands(op, locals);
    if wasm_scalar_integer_fast_path_for_op(scalar_plan, op) {
        numeric_lane_stats.record_op_loop_division_guarded_int_site();
        emit_guarded_int_binary_result_or_boxed(
            func,
            operands,
            import_ids,
            import_name,
            reloc_enabled,
            known_raw_ints,
            IntFastLane::IntOrBool,
            |func| {
                let temps = int_binary_temps(locals);
                emit_unbox_int_local_trusted_opt(
                    func,
                    operands.lhs,
                    temps.lhs,
                    const_cache,
                    known_raw_ints,
                );
                emit_unbox_int_local_trusted_tee_opt(
                    func,
                    operands.rhs,
                    temps.rhs,
                    const_cache,
                    known_raw_ints,
                );
                emit_nonzero_rhs_raw_division_or_boxed(
                    func,
                    operands,
                    import_ids,
                    const_cache,
                    locals.synthetic(WasmFrameSyntheticLocal::MoltTmp3),
                    reloc_enabled,
                    known_raw_ints,
                    import_name,
                    division_op,
                    temps,
                );
            },
        );
    } else if matches!(division_op, WasmNumericOpLoopKind::TrueDiv) {
        numeric_lane_stats.record_op_loop_division_boxed_runtime_site();
        emit_plain_f64_binary_result_or_boxed(
            func,
            operands,
            import_ids,
            import_name,
            locals,
            reloc_enabled,
            |func, scratch_local| {
                func.instruction(&Instruction::F64Div);
                emit_plain_f64_arithmetic_result(func, scratch_local);
            },
        );
    } else {
        numeric_lane_stats.record_op_loop_division_boxed_runtime_site();
        emit_boxed_binary_call(func, operands, import_ids, import_name, reloc_enabled);
    }
    store_numeric_result(func, op, locals);
}
