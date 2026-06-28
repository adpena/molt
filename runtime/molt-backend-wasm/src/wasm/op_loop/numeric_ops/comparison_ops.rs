use super::common::{
    binary_operands, emit_boxed_binary_result, emit_guarded_int_binary_result_or_boxed,
    emit_plain_f64_binary_result_or_boxed, emit_trusted_int_binary_operand_tees, int_binary_temps,
    store_numeric_result,
};
use crate::OpIR;
use crate::representation_plan::ScalarRepresentationPlan;
use crate::wasm::WasmFrameLocals;
use crate::wasm_import_tracking::TrackedImportIds;
use crate::wasm_plan::wasm_scalar_integer_fast_path_for_op;
use crate::wasm_values::{ConstantCache, IntFastLane, emit_box_bool_from_i32};
use std::collections::BTreeMap;
use wasm_encoder::{Function, Instruction};

#[derive(Clone, Copy)]
enum OrderedCompareOp {
    Lt,
    Le,
    Gt,
    Ge,
}

impl OrderedCompareOp {
    fn from_kind(kind: &str) -> Option<(Self, &'static str)> {
        match kind {
            "lt" => Some((Self::Lt, "lt")),
            "le" => Some((Self::Le, "le")),
            "gt" => Some((Self::Gt, "gt")),
            "ge" => Some((Self::Ge, "ge")),
            _ => None,
        }
    }

    fn emit_i64(self, func: &mut Function) {
        match self {
            Self::Lt => func.instruction(&Instruction::I64LtS),
            Self::Le => func.instruction(&Instruction::I64LeS),
            Self::Gt => func.instruction(&Instruction::I64GtS),
            Self::Ge => func.instruction(&Instruction::I64GeS),
        };
    }

    fn emit_f64(self, func: &mut Function) {
        match self {
            Self::Lt => func.instruction(&Instruction::F64Lt),
            Self::Le => func.instruction(&Instruction::F64Le),
            Self::Gt => func.instruction(&Instruction::F64Gt),
            Self::Ge => func.instruction(&Instruction::F64Ge),
        };
    }
}

#[derive(Clone, Copy)]
enum EqualityCompareOp {
    Eq,
    Ne,
}

impl EqualityCompareOp {
    fn from_kind(kind: &str) -> Option<(Self, &'static str)> {
        match kind {
            "eq" => Some((Self::Eq, "eq")),
            "ne" => Some((Self::Ne, "ne")),
            _ => None,
        }
    }

    fn emit_boxed_identity_compare(self, func: &mut Function, lhs: u32, rhs: u32) {
        func.instruction(&Instruction::LocalGet(lhs));
        func.instruction(&Instruction::LocalGet(rhs));
        match self {
            Self::Eq => func.instruction(&Instruction::I64Eq),
            Self::Ne => func.instruction(&Instruction::I64Ne),
        };
        emit_box_bool_from_i32(func);
    }
}

#[allow(unused_variables)]
pub(super) fn emit_comparison_numeric_op(
    func: &mut Function,
    op: &OpIR,
    import_ids: &TrackedImportIds,
    locals: &WasmFrameLocals,
    const_cache: &ConstantCache,
    scalar_plan: &ScalarRepresentationPlan,
    reloc_enabled: bool,
    known_raw_ints: &BTreeMap<u32, i64>,
) -> bool {
    if let Some((compare_op, import_name)) = OrderedCompareOp::from_kind(op.kind.as_str()) {
        emit_ordered_compare_op(
            func,
            op,
            import_ids,
            locals,
            const_cache,
            scalar_plan,
            reloc_enabled,
            known_raw_ints,
            compare_op,
            import_name,
        );
        return true;
    }

    if let Some((compare_op, import_name)) = EqualityCompareOp::from_kind(op.kind.as_str()) {
        emit_equality_compare_op(
            func,
            op,
            import_ids,
            locals,
            scalar_plan,
            reloc_enabled,
            known_raw_ints,
            compare_op,
            import_name,
        );
        return true;
    }

    match op.kind.as_str() {
        "string_eq" => {
            emit_boxed_binary_result(func, op, import_ids, locals, "string_eq", reloc_enabled)
        }
        _ => return false,
    }
    true
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
    compare_op: OrderedCompareOp,
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
                compare_op.emit_i64(func);
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
                compare_op.emit_f64(func);
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
    compare_op: EqualityCompareOp,
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
            |func| compare_op.emit_boxed_identity_compare(func, operands.lhs, operands.rhs),
        );
    } else {
        emit_boxed_binary_result(func, op, import_ids, locals, import_name, reloc_enabled);
        return;
    }
    store_numeric_result(func, op, locals);
}
