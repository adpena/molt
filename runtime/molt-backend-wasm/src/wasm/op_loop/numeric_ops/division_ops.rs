use super::common::{
    binary_operands, emit_boxed_binary_call, emit_boxed_binary_result, emit_boxed_ternary_result,
    emit_boxed_unary_result, emit_guarded_int_binary_result_or_boxed,
    emit_inline_int_result_or_boxed, emit_plain_f64_arithmetic_result,
    emit_plain_f64_binary_result_or_boxed, int_binary_temps, store_numeric_result,
};
use crate::OpIR;
use crate::representation_plan::ScalarRepresentationPlan;
use crate::wasm::{WasmFrameLocals, WasmFrameSyntheticLocal};
use crate::wasm_import_tracking::TrackedImportIds;
use crate::wasm_plan::wasm_scalar_integer_fast_path_for_op;
use crate::wasm_values::{
    ConstantCache, IntFastLane, emit_unbox_int_local_trusted_opt,
    emit_unbox_int_local_trusted_tee_opt,
};
use std::collections::BTreeMap;
use wasm_encoder::{BlockType, Function, Instruction, ValType};

#[derive(Clone, Copy)]
enum DivisionNumericOp {
    TrueDiv,
    FloorDiv,
    Mod,
}

impl DivisionNumericOp {
    fn from_kind(kind: &str) -> Option<(Self, &'static str)> {
        match kind {
            "div" => Some((Self::TrueDiv, "div")),
            "inplace_div" => Some((Self::TrueDiv, "inplace_div")),
            "floordiv" => Some((Self::FloorDiv, "floordiv")),
            "inplace_floordiv" => Some((Self::FloorDiv, "inplace_floordiv")),
            "mod" => Some((Self::Mod, "mod")),
            "inplace_mod" => Some((Self::Mod, "inplace_mod")),
            _ => None,
        }
    }
}

#[allow(unused_variables)]
pub(super) fn emit_division_numeric_op(
    func: &mut Function,
    op: &OpIR,
    import_ids: &TrackedImportIds,
    locals: &WasmFrameLocals,
    const_cache: &ConstantCache,
    scalar_plan: &ScalarRepresentationPlan,
    reloc_enabled: bool,
    known_raw_ints: &BTreeMap<u32, i64>,
) -> bool {
    if let Some((division_op, import_name)) = DivisionNumericOp::from_kind(op.kind.as_str()) {
        emit_division_binary_op(
            func,
            op,
            import_ids,
            locals,
            const_cache,
            scalar_plan,
            reloc_enabled,
            known_raw_ints,
            division_op,
            import_name,
        );
        return true;
    }

    match op.kind.as_str() {
        "matmul" | "inplace_matmul" => {
            let import_name = if op.kind == "inplace_matmul" {
                "inplace_matmul"
            } else {
                "matmul"
            };
            emit_boxed_binary_result(func, op, import_ids, locals, import_name, reloc_enabled);
        }
        "pow" | "inplace_pow" => {
            let import_name = if op.kind == "inplace_pow" {
                "inplace_pow"
            } else {
                "pow"
            };
            emit_boxed_binary_result(func, op, import_ids, locals, import_name, reloc_enabled);
        }
        "pow_mod" => {
            emit_boxed_ternary_result(func, op, import_ids, locals, "pow_mod", reloc_enabled);
        }
        "round" => {
            emit_boxed_ternary_result(func, op, import_ids, locals, "round", reloc_enabled);
        }
        "trunc" => {
            emit_boxed_unary_result(func, op, import_ids, locals, "trunc", reloc_enabled);
        }
        _ => return false,
    }
    true
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
    division_op: DivisionNumericOp,
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
    } else if matches!(division_op, DivisionNumericOp::TrueDiv) {
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
    division_op: DivisionNumericOp,
    temps: super::common::IntBinaryTemps,
) {
    func.instruction(&Instruction::I64Const(0));
    func.instruction(&Instruction::I64Ne);
    func.instruction(&Instruction::If(BlockType::Result(ValType::I64)));
    match division_op {
        DivisionNumericOp::TrueDiv => emit_raw_true_div(func, temps, f64_scratch_local),
        DivisionNumericOp::FloorDiv => emit_raw_floor_div(
            func,
            operands,
            import_ids,
            const_cache,
            reloc_enabled,
            known_raw_ints,
            import_name,
            temps,
        ),
        DivisionNumericOp::Mod => emit_raw_mod(
            func,
            operands,
            import_ids,
            const_cache,
            reloc_enabled,
            known_raw_ints,
            import_name,
            temps,
        ),
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
