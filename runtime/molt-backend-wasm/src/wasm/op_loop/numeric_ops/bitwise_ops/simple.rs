use super::super::common::{
    binary_operands, emit_boxed_binary_call, emit_guarded_int_binary_result_or_boxed,
    emit_inline_int_result, emit_inline_int_result_or_boxed, emit_trusted_int_binary_operand_tees,
    int_binary_temps, store_numeric_result,
};
use crate::OpIR;
use crate::representation_plan::ScalarRepresentationPlan;
use crate::wasm::{WasmFrameLocals, WasmNumericLaneStats};
use crate::wasm_abi_generated::WasmNumericOpLoopKind;
use crate::wasm_import_tracking::TrackedImportIds;
use crate::wasm_plan::{
    WasmScalarDirectNumericLane, wasm_scalar_direct_numeric_lane_for_op,
    wasm_scalar_integer_fast_path_for_op,
};
use crate::wasm_values::{ConstantCache, IntFastLane};
use std::collections::BTreeMap;
use wasm_encoder::{Function, Instruction};

fn emit_i64_bitwise(func: &mut Function, op_loop_kind: WasmNumericOpLoopKind) {
    match op_loop_kind {
        WasmNumericOpLoopKind::BitOr => func.instruction(&Instruction::I64Or),
        WasmNumericOpLoopKind::BitAnd => func.instruction(&Instruction::I64And),
        WasmNumericOpLoopKind::BitXor => func.instruction(&Instruction::I64Xor),
        _ => unreachable!("non-bitwise numeric selector routed to bitwise emitter"),
    };
}

pub(super) fn emit_simple_bitwise_op(
    func: &mut Function,
    op: &OpIR,
    import_ids: &TrackedImportIds,
    locals: &WasmFrameLocals,
    const_cache: &ConstantCache,
    scalar_plan: &ScalarRepresentationPlan,
    reloc_enabled: bool,
    known_raw_ints: &BTreeMap<u32, i64>,
    numeric_lane_stats: &mut WasmNumericLaneStats,
    op_idx: usize,
    bitwise_op: WasmNumericOpLoopKind,
    import_name: crate::wasm_abi_generated::WasmRuntimeImport,
) {
    let operands = binary_operands(op, locals);
    if wasm_scalar_direct_numeric_lane_for_op(scalar_plan, op_idx, op)
        == Some(WasmScalarDirectNumericLane::InlineInt)
    {
        numeric_lane_stats.record_op_loop_bitwise_inline_int_raw_site();
        let temps = int_binary_temps(locals);
        emit_trusted_int_binary_operand_tees(func, operands, temps, const_cache, known_raw_ints);
        emit_i64_bitwise(func, bitwise_op);
        func.instruction(&Instruction::LocalSet(temps.result));
        emit_inline_int_result(func, temps.result, known_raw_ints);
    } else if wasm_scalar_integer_fast_path_for_op(scalar_plan, op) {
        numeric_lane_stats.record_op_loop_bitwise_guarded_int_site();
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
                emit_trusted_int_binary_operand_tees(
                    func,
                    operands,
                    temps,
                    const_cache,
                    known_raw_ints,
                );
                emit_i64_bitwise(func, bitwise_op);
                func.instruction(&Instruction::LocalSet(temps.result));
                emit_inline_int_result_or_boxed(
                    func,
                    temps.result,
                    operands,
                    import_ids,
                    import_name,
                    const_cache,
                    reloc_enabled,
                    known_raw_ints,
                );
            },
        );
    } else {
        numeric_lane_stats.record_op_loop_bitwise_boxed_runtime_site();
        emit_boxed_binary_call(func, operands, import_ids, import_name, reloc_enabled);
    }
    store_numeric_result(func, op, locals);
}
