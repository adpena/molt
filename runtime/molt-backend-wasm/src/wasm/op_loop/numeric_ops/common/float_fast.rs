use super::boxed::emit_boxed_binary_call;
use super::operands::BinaryOperands;
use crate::wasm_abi_generated::{WasmNumericOpLoopKind, WasmRuntimeImport};
use crate::wasm_binary::Function;
use crate::wasm_import_tracking::TrackedImportIds;
use wasm_encoder::{BlockType, Instruction, ValType};

pub(in crate::wasm::op_loop::numeric_ops) fn emit_plain_f64_binary_result(
    func: &mut Function,
    operands: BinaryOperands,
    emit_f64_result: impl FnOnce(&mut Function),
) {
    func.instruction(&Instruction::LocalGet(operands.lhs));
    func.instruction(&Instruction::F64ReinterpretI64);
    func.instruction(&Instruction::LocalGet(operands.rhs));
    func.instruction(&Instruction::F64ReinterpretI64);
    emit_f64_result(func);
}

pub(in crate::wasm::op_loop::numeric_ops) fn emit_plain_f64_binary_result_or_boxed(
    func: &mut Function,
    operands: BinaryOperands,
    import_ids: &TrackedImportIds,
    import: WasmRuntimeImport,
    op_loop_kind: WasmNumericOpLoopKind,
    reloc_enabled: bool,
    emit_f64_result: impl FnOnce(&mut Function),
) {
    emit_plain_f64_binary_guard(func, operands);
    // The generated operation selector owns binary/in-place family identity.
    // IEEE division by either signed zero must reach Python exception dispatch.
    if matches!(op_loop_kind, WasmNumericOpLoopKind::TrueDiv) {
        func.instruction(&Instruction::LocalGet(operands.rhs));
        func.instruction(&Instruction::F64ReinterpretI64);
        func.instruction(&Instruction::F64Const(0.0.into()));
        func.instruction(&Instruction::F64Ne);
        func.instruction(&Instruction::I32And);
    }
    func.instruction(&Instruction::If(BlockType::Result(ValType::I64)));
    emit_plain_f64_binary_result(func, operands, emit_f64_result);
    func.instruction(&Instruction::Else);
    emit_boxed_binary_call(func, operands, import_ids, import, reloc_enabled);
    func.instruction(&Instruction::End);
}

/// Box an f64 arithmetic result: the bit cast (runtime spec 2.1.1). Arithmetic
/// on floats yields a float word (quieting sets bit 51 only, and the engine's
/// default NaN is 0x7ff8_0000_0000_0000).
pub(in crate::wasm::op_loop::numeric_ops) fn emit_plain_f64_arithmetic_result(func: &mut Function) {
    func.instruction(&Instruction::I64ReinterpretF64);
}

fn emit_plain_f64_binary_guard(func: &mut Function, operands: BinaryOperands) {
    func.instruction(&Instruction::LocalGet(operands.lhs));
    emit_plain_f64_predicate(func);
    func.instruction(&Instruction::LocalGet(operands.rhs));
    emit_plain_f64_predicate(func);
    func.instruction(&Instruction::I32And);
}

/// The float test `molt_codegen_abi::is_float_bits` on the word on the stack:
/// `bits << 1 < SHIFTED_FIRST_TAG_BOUNDARY`.
fn emit_plain_f64_predicate(func: &mut Function) {
    func.instruction(&Instruction::I64Const(1));
    func.instruction(&Instruction::I64Shl);
    func.instruction(&Instruction::I64Const(
        molt_codegen_abi::SHIFTED_FIRST_TAG_BOUNDARY as i64,
    ));
    func.instruction(&Instruction::I64LtU);
}
