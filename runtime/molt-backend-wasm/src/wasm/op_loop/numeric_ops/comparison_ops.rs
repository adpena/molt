use super::common::{
    binary_operands, emit_boxed_binary_result, emit_guarded_int_binary_result_or_boxed,
    emit_plain_f64_binary_result_or_boxed, emit_trusted_int_binary_operand_tees, int_binary_temps,
    store_numeric_result,
};
use crate::OpIR;
use crate::representation_plan::ScalarRepresentationPlan;
use crate::wasm::WasmFrameLocals;
use crate::wasm_abi_generated::{WasmNumericOpLoopKind, WasmNumericRuntimeSelection};
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

fn emit_boxed_identity_compare(
    func: &mut Function,
    op_loop_kind: WasmNumericOpLoopKind,
    lhs: u32,
    rhs: u32,
) {
    func.instruction(&Instruction::LocalGet(lhs));
    func.instruction(&Instruction::LocalGet(rhs));
    match op_loop_kind {
        WasmNumericOpLoopKind::Eq => func.instruction(&Instruction::I64Eq),
        WasmNumericOpLoopKind::Ne => func.instruction(&Instruction::I64Ne),
        _ => unreachable!("non-equality numeric selector routed to equality compare emitter"),
    };
    emit_box_bool_from_i32(func);
}

#[allow(unused_variables)]
pub(super) fn emit_comparison_numeric_op(
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
    match selection.op_loop_kind {
        WasmNumericOpLoopKind::Lt
        | WasmNumericOpLoopKind::Le
        | WasmNumericOpLoopKind::Gt
        | WasmNumericOpLoopKind::Ge => emit_ordered_compare_op(
            func,
            op,
            import_ids,
            locals,
            const_cache,
            scalar_plan,
            reloc_enabled,
            known_raw_ints,
            selection.op_loop_kind,
            selection.import_name,
        ),
        WasmNumericOpLoopKind::Eq | WasmNumericOpLoopKind::Ne => emit_equality_compare_op(
            func,
            op,
            import_ids,
            locals,
            scalar_plan,
            reloc_enabled,
            known_raw_ints,
            selection.op_loop_kind,
            selection.import_name,
        ),
        WasmNumericOpLoopKind::StringEq => emit_boxed_binary_result(
            func,
            op,
            import_ids,
            locals,
            selection.import_name,
            reloc_enabled,
        ),
        _ => unreachable!("non-comparison numeric selector routed to comparison emitter"),
    }
}

fn emit_ordered_compare_op(
    func: &mut Function,
    op: &OpIR,
    import_ids: &TrackedImportIds,
    locals: &WasmFrameLocals,
    const_cache: &ConstantCache,
    scalar_plan: &ScalarRepresentationPlan,
    reloc_enabled: bool,
    known_raw_ints: &BTreeMap<u32, i64>,
    compare_op: WasmNumericOpLoopKind,
    import_name: &str,
) {
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

fn emit_equality_compare_op(
    func: &mut Function,
    op: &OpIR,
    import_ids: &TrackedImportIds,
    locals: &WasmFrameLocals,
    scalar_plan: &ScalarRepresentationPlan,
    reloc_enabled: bool,
    known_raw_ints: &BTreeMap<u32, i64>,
    compare_op: WasmNumericOpLoopKind,
    import_name: &str,
) {
    let operands = binary_operands(op, locals);
    if wasm_scalar_integer_fast_path_for_op(&scalar_plan, op) {
        emit_guarded_int_binary_result_or_boxed(
            func,
            operands,
            import_ids,
            import_name,
            reloc_enabled,
            known_raw_ints,
            IntFastLane::IntOnly,
            |func| emit_boxed_identity_compare(func, compare_op, operands.lhs, operands.rhs),
        );
    } else {
        emit_boxed_binary_result(func, op, import_ids, locals, import_name, reloc_enabled);
        return;
    }
    store_numeric_result(func, op, locals);
}
