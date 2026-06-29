use super::common::{
    binary_operands, emit_guarded_int_binary_result_or_boxed, emit_inline_int_result_or_boxed,
    emit_plain_f64_arithmetic_result, emit_plain_f64_binary_result_or_boxed,
    emit_trusted_int_binary_operand_tees, int_binary_temps, store_numeric_result,
};
use crate::OpIR;
use crate::representation_plan::ScalarRepresentationPlan;
use crate::wasm::WasmFrameLocals;
use crate::wasm_abi_generated::{WasmNumericOpLoopKind, WasmNumericRuntimeSelection};
use crate::wasm_import_tracking::TrackedImportIds;
use crate::wasm_plan::wasm_scalar_integer_fast_path_for_op;
use crate::wasm_values::{ConstantCache, IntFastLane};
use std::collections::BTreeMap;
use wasm_encoder::{Function, Instruction};

fn emit_i64_additive(func: &mut Function, op_loop_kind: WasmNumericOpLoopKind) {
    match op_loop_kind {
        WasmNumericOpLoopKind::Add => func.instruction(&Instruction::I64Add),
        WasmNumericOpLoopKind::Sub => func.instruction(&Instruction::I64Sub),
        WasmNumericOpLoopKind::Mul => func.instruction(&Instruction::I64Mul),
        _ => unreachable!("non-additive numeric selector routed to additive emitter"),
    };
}

fn emit_f64_additive(func: &mut Function, op_loop_kind: WasmNumericOpLoopKind) {
    match op_loop_kind {
        WasmNumericOpLoopKind::Add => func.instruction(&Instruction::F64Add),
        WasmNumericOpLoopKind::Sub => func.instruction(&Instruction::F64Sub),
        WasmNumericOpLoopKind::Mul => func.instruction(&Instruction::F64Mul),
        _ => unreachable!("non-additive numeric selector routed to additive emitter"),
    };
}

#[allow(unused_variables)]
pub(super) fn emit_additive_numeric_op(
    func: &mut Function,
    op: &OpIR,
    selection: WasmNumericRuntimeSelection,
    import_ids: &TrackedImportIds,
    locals: &WasmFrameLocals,
    const_cache: &ConstantCache,
    scalar_plan: &ScalarRepresentationPlan,
    reloc_enabled: bool,
    known_raw_ints: &BTreeMap<u32, i64>,
) {
    let import_name = selection.import;
    let operands = binary_operands(op, locals);
    if wasm_scalar_integer_fast_path_for_op(&scalar_plan, op) {
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
                emit_i64_additive(func, selection.op_loop_kind);
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
        emit_plain_f64_binary_result_or_boxed(
            func,
            operands,
            import_ids,
            import_name,
            locals,
            reloc_enabled,
            |func, scratch_local| {
                emit_f64_additive(func, selection.op_loop_kind);
                emit_plain_f64_arithmetic_result(func, scratch_local);
            },
        );
    }
    store_numeric_result(func, op, locals);
}
