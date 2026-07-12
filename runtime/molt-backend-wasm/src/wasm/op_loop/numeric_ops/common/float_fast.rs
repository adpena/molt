use super::boxed::emit_boxed_binary_call;
use super::operands::BinaryOperands;
use crate::wasm::{WasmFrameLocals, WasmFrameSyntheticLocal};
use crate::wasm_abi_generated::WasmRuntimeImport;
use crate::wasm_import_tracking::TrackedImportIds;
use crate::wasm_values::emit_f64_to_i64_canonical;
use wasm_encoder::{BlockType, Function, Instruction, ValType};

pub(in crate::wasm::op_loop::numeric_ops) fn emit_plain_f64_binary_result(
    func: &mut Function,
    operands: BinaryOperands,
    locals: &WasmFrameLocals,
    emit_f64_result: impl FnOnce(&mut Function, u32),
) {
    func.instruction(&Instruction::LocalGet(operands.lhs));
    func.instruction(&Instruction::F64ReinterpretI64);
    func.instruction(&Instruction::LocalGet(operands.rhs));
    func.instruction(&Instruction::F64ReinterpretI64);
    emit_f64_result(func, locals.synthetic(WasmFrameSyntheticLocal::MoltTmp3));
}

pub(in crate::wasm::op_loop::numeric_ops) fn emit_plain_f64_binary_result_or_boxed(
    func: &mut Function,
    operands: BinaryOperands,
    import_ids: &TrackedImportIds,
    import: WasmRuntimeImport,
    locals: &WasmFrameLocals,
    reloc_enabled: bool,
    emit_f64_result: impl FnOnce(&mut Function, u32),
) {
    emit_plain_f64_binary_guard(func, operands);
    func.instruction(&Instruction::If(BlockType::Result(ValType::I64)));
    emit_plain_f64_binary_result(func, operands, locals, emit_f64_result);
    func.instruction(&Instruction::Else);
    emit_boxed_binary_call(func, operands, import_ids, import, reloc_enabled);
    func.instruction(&Instruction::End);
}

pub(in crate::wasm::op_loop::numeric_ops) fn emit_plain_f64_arithmetic_result(
    func: &mut Function,
    scratch_local: u32,
) {
    emit_f64_to_i64_canonical(func, scratch_local);
}

fn emit_plain_f64_binary_guard(func: &mut Function, operands: BinaryOperands) {
    func.instruction(&Instruction::LocalGet(operands.lhs));
    emit_plain_f64_predicate(func);
    func.instruction(&Instruction::LocalGet(operands.rhs));
    emit_plain_f64_predicate(func);
    func.instruction(&Instruction::I32And);
}

fn emit_plain_f64_predicate(func: &mut Function) {
    func.instruction(&Instruction::I64Const(48));
    func.instruction(&Instruction::I64ShrU);
    func.instruction(&Instruction::I64Const(0x7FF9));
    func.instruction(&Instruction::I64Sub);
    func.instruction(&Instruction::I64Const(5));
    func.instruction(&Instruction::I64LtU);
    func.instruction(&Instruction::I32Eqz);
}
