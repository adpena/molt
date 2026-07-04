use super::super::common::{
    binary_operands, emit_boxed_binary_result, emit_guarded_int_binary_result_or_boxed,
    store_numeric_result,
};
use crate::OpIR;
use crate::representation_plan::ScalarRepresentationPlan;
use crate::wasm::WasmFrameLocals;
use crate::wasm_abi_generated::WasmNumericOpLoopKind;
use crate::wasm_import_tracking::TrackedImportIds;
use crate::wasm_plan::wasm_scalar_integer_fast_path_for_op;
use crate::wasm_values::{IntFastLane, emit_box_bool_from_i32};
use std::collections::BTreeMap;
use wasm_encoder::{Function, Instruction};

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

pub(super) fn emit_equality_compare_op(
    func: &mut Function,
    op: &OpIR,
    import_ids: &TrackedImportIds,
    locals: &WasmFrameLocals,
    scalar_plan: &ScalarRepresentationPlan,
    reloc_enabled: bool,
    known_raw_ints: &BTreeMap<u32, i64>,
    compare_op: WasmNumericOpLoopKind,
    import_name: crate::wasm_abi_generated::WasmRuntimeImport,
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
