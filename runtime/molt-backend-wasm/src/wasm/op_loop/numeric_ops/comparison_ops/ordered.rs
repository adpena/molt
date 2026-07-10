use super::super::common::{
    binary_operands, emit_guarded_int_binary_result_or_boxed,
    emit_plain_f64_binary_result_or_boxed, emit_trusted_int_binary_operand_tees, int_binary_temps,
    store_numeric_result,
};
use crate::OpIR;
use crate::representation_plan::ScalarRepresentationPlan;
use crate::wasm::WasmFrameLocals;
use crate::wasm_abi_generated::WasmNumericOpLoopKind;
use crate::wasm_import_tracking::TrackedImportIds;
use crate::wasm_plan::wasm_scalar_integer_fast_path_for_op;
use crate::wasm_values::{ConstantCache, IntFastLane, emit_box_bool_from_i32};
use std::collections::BTreeMap;
use wasm_encoder::{Function, Instruction};

fn emit_i64_ordered_compare(func: &mut Function, op_loop_kind: WasmNumericOpLoopKind) {
    match op_loop_kind {
        WasmNumericOpLoopKind::Lt => func.instruction(&Instruction::I64LtS),
        WasmNumericOpLoopKind::Le => func.instruction(&Instruction::I64LeS),
        WasmNumericOpLoopKind::Gt => func.instruction(&Instruction::I64GtS),
        WasmNumericOpLoopKind::Ge => func.instruction(&Instruction::I64GeS),
        _ => unreachable!("non-ordered numeric selector routed to ordered compare emitter"),
    };
}

fn emit_f64_ordered_compare(func: &mut Function, op_loop_kind: WasmNumericOpLoopKind) {
    match op_loop_kind {
        WasmNumericOpLoopKind::Lt => func.instruction(&Instruction::F64Lt),
        WasmNumericOpLoopKind::Le => func.instruction(&Instruction::F64Le),
        WasmNumericOpLoopKind::Gt => func.instruction(&Instruction::F64Gt),
        WasmNumericOpLoopKind::Ge => func.instruction(&Instruction::F64Ge),
        _ => unreachable!("non-ordered numeric selector routed to ordered compare emitter"),
    };
}

pub(super) fn emit_ordered_compare_op(
    func: &mut Function,
    op: &OpIR,
    import_ids: &TrackedImportIds,
    locals: &WasmFrameLocals,
    const_cache: &ConstantCache,
    scalar_plan: &ScalarRepresentationPlan,
    reloc_enabled: bool,
    known_raw_ints: &BTreeMap<u32, i64>,
    compare_op: WasmNumericOpLoopKind,
    import_name: crate::wasm_abi_generated::WasmRuntimeImport,
) {
    let operands = binary_operands(op, locals);
    if wasm_scalar_integer_fast_path_for_op(scalar_plan, op) {
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
                emit_i64_ordered_compare(func, compare_op);
                emit_box_bool_from_i32(func);
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
            |func, _scratch_local| {
                emit_f64_ordered_compare(func, compare_op);
                emit_box_bool_from_i32(func);
            },
        );
    }
    store_numeric_result(func, op, locals);
}
