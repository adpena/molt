use super::common::{
    binary_operands, emit_boxed_binary_call, emit_boxed_unary_result,
    emit_guarded_int_binary_result_or_boxed, emit_inline_int_result_or_boxed,
    emit_trusted_int_binary_operand_tees, int_binary_temps, store_numeric_result,
};
use crate::OpIR;
use crate::representation_plan::ScalarRepresentationPlan;
use crate::wasm::WasmFrameLocals;
use crate::wasm_abi_generated::{WasmNumericOpLoopKind, WasmNumericRuntimeSelection};
use crate::wasm_import_tracking::TrackedImportIds;
use crate::wasm_plan::wasm_scalar_integer_fast_path_for_op;
use crate::wasm_values::{
    ConstantCache, IntFastLane, emit_box_int_from_local_opt, emit_inline_int_range_check,
    emit_unbox_int_local_trusted_opt, emit_unbox_int_local_trusted_tee_opt,
};
use std::collections::BTreeMap;
use wasm_encoder::{BlockType, Function, Instruction, ValType};

fn emit_i64_bitwise(func: &mut Function, op_loop_kind: WasmNumericOpLoopKind) {
    match op_loop_kind {
        WasmNumericOpLoopKind::BitOr => func.instruction(&Instruction::I64Or),
        WasmNumericOpLoopKind::BitAnd => func.instruction(&Instruction::I64And),
        WasmNumericOpLoopKind::BitXor => func.instruction(&Instruction::I64Xor),
        _ => unreachable!("non-bitwise numeric selector routed to bitwise emitter"),
    };
}

#[allow(unused_variables)]
pub(super) fn emit_bitwise_numeric_op(
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
        WasmNumericOpLoopKind::BitAnd
        | WasmNumericOpLoopKind::BitOr
        | WasmNumericOpLoopKind::BitXor => emit_simple_bitwise_op(
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
        WasmNumericOpLoopKind::Invert | WasmNumericOpLoopKind::Neg | WasmNumericOpLoopKind::Pos => {
            emit_boxed_unary_result(
                func,
                op,
                import_ids,
                locals,
                selection.import_name,
                reloc_enabled,
            )
        }
        WasmNumericOpLoopKind::LShift => emit_shift_op(
            func,
            op,
            import_ids,
            locals,
            const_cache,
            scalar_plan,
            reloc_enabled,
            known_raw_ints,
            selection.import_name,
            ShiftDirection::Left,
        ),
        WasmNumericOpLoopKind::RShift => emit_shift_op(
            func,
            op,
            import_ids,
            locals,
            const_cache,
            scalar_plan,
            reloc_enabled,
            known_raw_ints,
            selection.import_name,
            ShiftDirection::Right,
        ),
        _ => unreachable!("non-bitwise numeric selector routed to bitwise emitter"),
    }
}

fn emit_simple_bitwise_op(
    func: &mut Function,
    op: &OpIR,
    import_ids: &TrackedImportIds,
    locals: &WasmFrameLocals,
    const_cache: &ConstantCache,
    scalar_plan: &ScalarRepresentationPlan,
    reloc_enabled: bool,
    known_raw_ints: &BTreeMap<u32, i64>,
    bitwise_op: WasmNumericOpLoopKind,
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
        emit_boxed_binary_call(func, operands, import_ids, import_name, reloc_enabled);
    }
    store_numeric_result(func, op, locals);
}

#[derive(Clone, Copy)]
enum ShiftDirection {
    Left,
    Right,
}

fn emit_shift_op(
    func: &mut Function,
    op: &OpIR,
    import_ids: &TrackedImportIds,
    locals: &WasmFrameLocals,
    const_cache: &ConstantCache,
    scalar_plan: &ScalarRepresentationPlan,
    reloc_enabled: bool,
    known_raw_ints: &BTreeMap<u32, i64>,
    import_name: &str,
    direction: ShiftDirection,
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
                emit_shift_fast_path(
                    func,
                    operands,
                    import_ids,
                    locals,
                    const_cache,
                    reloc_enabled,
                    known_raw_ints,
                    import_name,
                    direction,
                )
            },
        );
    } else {
        emit_boxed_binary_call(func, operands, import_ids, import_name, reloc_enabled);
    }
    store_numeric_result(func, op, locals);
}

fn emit_shift_fast_path(
    func: &mut Function,
    operands: super::common::BinaryOperands,
    import_ids: &TrackedImportIds,
    locals: &WasmFrameLocals,
    const_cache: &ConstantCache,
    reloc_enabled: bool,
    known_raw_ints: &BTreeMap<u32, i64>,
    import_name: &str,
    direction: ShiftDirection,
) {
    let temps = int_binary_temps(locals);
    emit_unbox_int_local_trusted_opt(func, operands.lhs, temps.lhs, const_cache, known_raw_ints);
    emit_unbox_int_local_trusted_tee_opt(
        func,
        operands.rhs,
        temps.rhs,
        const_cache,
        known_raw_ints,
    );
    func.instruction(&Instruction::I64Const(0));
    func.instruction(&Instruction::I64GeS);
    func.instruction(&Instruction::LocalGet(temps.rhs));
    func.instruction(&Instruction::I64Const(64));
    func.instruction(&Instruction::I64LtS);
    func.instruction(&Instruction::I32And);
    func.instruction(&Instruction::If(BlockType::Result(ValType::I64)));
    func.instruction(&Instruction::LocalGet(temps.lhs));
    func.instruction(&Instruction::LocalGet(temps.rhs));
    match direction {
        ShiftDirection::Left => emit_left_shift_fast_path(
            func,
            operands,
            import_ids,
            const_cache,
            reloc_enabled,
            known_raw_ints,
            import_name,
            temps,
        ),
        ShiftDirection::Right => {
            func.instruction(&Instruction::I64ShrS);
            func.instruction(&Instruction::LocalSet(temps.result));
            emit_box_int_from_local_opt(func, temps.result, known_raw_ints);
        }
    }
    func.instruction(&Instruction::Else);
    emit_boxed_binary_call(func, operands, import_ids, import_name, reloc_enabled);
    func.instruction(&Instruction::End);
}

fn emit_left_shift_fast_path(
    func: &mut Function,
    operands: super::common::BinaryOperands,
    import_ids: &TrackedImportIds,
    const_cache: &ConstantCache,
    reloc_enabled: bool,
    known_raw_ints: &BTreeMap<u32, i64>,
    import_name: &str,
    temps: super::common::IntBinaryTemps,
) {
    func.instruction(&Instruction::I64Shl);
    func.instruction(&Instruction::LocalSet(temps.result));
    func.instruction(&Instruction::LocalGet(temps.result));
    func.instruction(&Instruction::LocalGet(temps.rhs));
    func.instruction(&Instruction::I64ShrS);
    func.instruction(&Instruction::LocalGet(temps.lhs));
    func.instruction(&Instruction::I64Eq);
    emit_inline_int_range_check(func, temps.result, const_cache);
    func.instruction(&Instruction::I32And);
    func.instruction(&Instruction::If(BlockType::Result(ValType::I64)));
    emit_box_int_from_local_opt(func, temps.result, known_raw_ints);
    func.instruction(&Instruction::Else);
    emit_boxed_binary_call(func, operands, import_ids, import_name, reloc_enabled);
    func.instruction(&Instruction::End);
}
