use super::common::{
    binary_operands, emit_boxed_binary_call, emit_boxed_binary_result, emit_boxed_ternary_result,
    emit_boxed_unary_result, emit_guarded_int_binary_result_or_boxed,
    emit_inline_int_result_or_boxed, emit_plain_f64_arithmetic_result,
    emit_plain_f64_binary_result_or_boxed, int_binary_temps, store_numeric_result,
};
use crate::OpIR;
use crate::representation_plan::ScalarRepresentationPlan;
use crate::wasm::{WasmFrameLocals, WasmFrameSyntheticLocal};
use crate::wasm_abi_generated::{WasmNumericOpLoopKind, WasmNumericRuntimeSelection};
use crate::wasm_import_tracking::TrackedImportIds;
use crate::wasm_plan::wasm_scalar_integer_fast_path_for_op;
use crate::wasm_values::{
    ConstantCache, IntFastLane, emit_unbox_int_local_trusted_opt,
    emit_unbox_int_local_trusted_tee_opt,
};
use std::collections::BTreeMap;
use wasm_encoder::{BlockType, Function, Instruction, ValType};

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
) {
    match selection.op_loop_kind {
        WasmNumericOpLoopKind::TrueDiv
        | WasmNumericOpLoopKind::FloorDiv
        | WasmNumericOpLoopKind::Mod => emit_division_binary_op(
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
        WasmNumericOpLoopKind::Matmul | WasmNumericOpLoopKind::Pow => emit_boxed_binary_result(
            func,
            op,
            import_ids,
            locals,
            selection.import_name,
            reloc_enabled,
        ),
        WasmNumericOpLoopKind::PowMod | WasmNumericOpLoopKind::Round => {
            emit_boxed_ternary_result(
                func,
                op,
                import_ids,
                locals,
                selection.import_name,
                reloc_enabled,
            );
        }
        WasmNumericOpLoopKind::Trunc => emit_boxed_unary_result(
            func,
            op,
            import_ids,
            locals,
            selection.import_name,
            reloc_enabled,
        ),
        _ => unreachable!("non-division numeric selector routed to division emitter"),
    }
}

fn emit_division_binary_op(
    func: &mut Function,
    op: &OpIR,
    import_ids: &TrackedImportIds,
    locals: &WasmFrameLocals,
    const_cache: &ConstantCache,
    scalar_plan: &ScalarRepresentationPlan,
    reloc_enabled: bool,
    known_raw_ints: &BTreeMap<u32, i64>,
    division_op: WasmNumericOpLoopKind,
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
        emit_boxed_binary_call(func, operands, import_ids, import_name, reloc_enabled);
    }
    store_numeric_result(func, op, locals);
}

fn emit_nonzero_rhs_raw_division_or_boxed(
    func: &mut Function,
    operands: super::common::BinaryOperands,
    import_ids: &TrackedImportIds,
    const_cache: &ConstantCache,
    f64_scratch_local: u32,
    reloc_enabled: bool,
    known_raw_ints: &BTreeMap<u32, i64>,
    import_name: &str,
    division_op: WasmNumericOpLoopKind,
    temps: super::common::IntBinaryTemps,
) {
    func.instruction(&Instruction::I64Const(0));
    func.instruction(&Instruction::I64Ne);
    func.instruction(&Instruction::If(BlockType::Result(ValType::I64)));
    match division_op {
        WasmNumericOpLoopKind::TrueDiv => emit_raw_true_div(func, temps, f64_scratch_local),
        WasmNumericOpLoopKind::FloorDiv => emit_raw_floor_div(
            func,
            operands,
            import_ids,
            const_cache,
            reloc_enabled,
            known_raw_ints,
            import_name,
            temps,
        ),
        WasmNumericOpLoopKind::Mod => emit_raw_mod(
            func,
            operands,
            import_ids,
            const_cache,
            reloc_enabled,
            known_raw_ints,
            import_name,
            temps,
        ),
        _ => unreachable!("non-division numeric selector routed to raw division"),
    }
    func.instruction(&Instruction::Else);
    emit_boxed_binary_call(func, operands, import_ids, import_name, reloc_enabled);
    func.instruction(&Instruction::End);
}

fn emit_raw_true_div(func: &mut Function, temps: super::common::IntBinaryTemps, scratch: u32) {
    func.instruction(&Instruction::LocalGet(temps.lhs));
    func.instruction(&Instruction::F64ConvertI64S);
    func.instruction(&Instruction::LocalGet(temps.rhs));
    func.instruction(&Instruction::F64ConvertI64S);
    func.instruction(&Instruction::F64Div);
    emit_plain_f64_arithmetic_result(func, scratch);
}

fn emit_raw_floor_div(
    func: &mut Function,
    operands: super::common::BinaryOperands,
    import_ids: &TrackedImportIds,
    const_cache: &ConstantCache,
    reloc_enabled: bool,
    known_raw_ints: &BTreeMap<u32, i64>,
    import_name: &str,
    temps: super::common::IntBinaryTemps,
) {
    func.instruction(&Instruction::LocalGet(temps.lhs));
    func.instruction(&Instruction::LocalGet(temps.rhs));
    func.instruction(&Instruction::I64DivS);
    func.instruction(&Instruction::LocalSet(temps.result));

    func.instruction(&Instruction::LocalGet(temps.lhs));
    func.instruction(&Instruction::LocalGet(temps.rhs));
    func.instruction(&Instruction::I64RemS);
    func.instruction(&Instruction::I64Const(0));
    func.instruction(&Instruction::I64Ne);
    emit_quotient_signs_differ(func, temps);
    func.instruction(&Instruction::I32And);
    func.instruction(&Instruction::If(BlockType::Empty));
    func.instruction(&Instruction::LocalGet(temps.result));
    func.instruction(&Instruction::I64Const(1));
    func.instruction(&Instruction::I64Sub);
    func.instruction(&Instruction::LocalSet(temps.result));
    func.instruction(&Instruction::End);

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
}

fn emit_raw_mod(
    func: &mut Function,
    operands: super::common::BinaryOperands,
    import_ids: &TrackedImportIds,
    const_cache: &ConstantCache,
    reloc_enabled: bool,
    known_raw_ints: &BTreeMap<u32, i64>,
    import_name: &str,
    temps: super::common::IntBinaryTemps,
) {
    func.instruction(&Instruction::LocalGet(temps.lhs));
    func.instruction(&Instruction::LocalGet(temps.rhs));
    func.instruction(&Instruction::I64RemS);
    func.instruction(&Instruction::LocalSet(temps.result));
    func.instruction(&Instruction::LocalGet(temps.result));
    func.instruction(&Instruction::I64Const(0));
    func.instruction(&Instruction::I64Ne);
    emit_quotient_signs_differ(func, temps);
    func.instruction(&Instruction::I32And);
    func.instruction(&Instruction::If(BlockType::Empty));
    func.instruction(&Instruction::LocalGet(temps.result));
    func.instruction(&Instruction::LocalGet(temps.rhs));
    func.instruction(&Instruction::I64Add);
    func.instruction(&Instruction::LocalSet(temps.result));
    func.instruction(&Instruction::End);
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
}

fn emit_quotient_signs_differ(func: &mut Function, temps: super::common::IntBinaryTemps) {
    func.instruction(&Instruction::LocalGet(temps.lhs));
    func.instruction(&Instruction::I64Const(0));
    func.instruction(&Instruction::I64LtS);
    func.instruction(&Instruction::LocalGet(temps.rhs));
    func.instruction(&Instruction::I64Const(0));
    func.instruction(&Instruction::I64LtS);
    func.instruction(&Instruction::I32Xor);
}
