use super::common::{
    binary_operands, emit_guarded_int_binary_result_or_boxed, emit_inline_int_result_or_boxed,
    emit_plain_f64_arithmetic_result, emit_plain_f64_binary_result_or_boxed,
    emit_trusted_int_binary_operand_tees, int_binary_temps, store_numeric_result,
};
use crate::OpIR;
use crate::representation_plan::ScalarRepresentationPlan;
use crate::wasm::WasmFrameLocals;
use crate::wasm_import_tracking::TrackedImportIds;
use crate::wasm_plan::wasm_scalar_integer_fast_path_for_op;
use crate::wasm_values::{ConstantCache, IntFastLane};
use std::collections::BTreeMap;
use wasm_encoder::{Function, Instruction};

#[derive(Clone, Copy)]
enum AdditiveNumericOp {
    Add,
    Sub,
    Mul,
}

impl AdditiveNumericOp {
    fn from_kind(kind: &str) -> Option<(Self, &'static str)> {
        match kind {
            "add" => Some((Self::Add, "add")),
            "inplace_add" => Some((Self::Add, "inplace_add")),
            "sub" => Some((Self::Sub, "sub")),
            "inplace_sub" => Some((Self::Sub, "inplace_sub")),
            "mul" => Some((Self::Mul, "mul")),
            "inplace_mul" => Some((Self::Mul, "inplace_mul")),
            _ => None,
        }
    }

    fn emit_i64(self, func: &mut Function) {
        match self {
            Self::Add => func.instruction(&Instruction::I64Add),
            Self::Sub => func.instruction(&Instruction::I64Sub),
            Self::Mul => func.instruction(&Instruction::I64Mul),
        };
    }

    fn emit_f64(self, func: &mut Function) {
        match self {
            Self::Add => func.instruction(&Instruction::F64Add),
            Self::Sub => func.instruction(&Instruction::F64Sub),
            Self::Mul => func.instruction(&Instruction::F64Mul),
        };
    }
}

#[allow(unused_variables)]
pub(super) fn emit_additive_numeric_op(
    func: &mut Function,
    op: &OpIR,
    import_ids: &TrackedImportIds,
    locals: &WasmFrameLocals,
    const_cache: &ConstantCache,
    scalar_plan: &ScalarRepresentationPlan,
    reloc_enabled: bool,
    known_raw_ints: &BTreeMap<u32, i64>,
) -> bool {
    let Some((numeric_op, import_name)) = AdditiveNumericOp::from_kind(op.kind.as_str()) else {
        return false;
    };
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
                numeric_op.emit_i64(func);
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
                numeric_op.emit_f64(func);
                emit_plain_f64_arithmetic_result(func, scratch_local);
            },
        );
    }
    store_numeric_result(func, op, locals);
    true
}
