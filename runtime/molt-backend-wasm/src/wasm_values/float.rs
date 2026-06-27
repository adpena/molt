use molt_codegen_abi::{CANONICAL_NAN_BITS, QNAN};
use wasm_encoder::{BlockType, Function, Instruction, ValType};

/// Emit WASM instructions to convert an f64 on the stack to a NaN-canonicalized i64.
/// Uses `scratch_local` (an i64 local) as temporary storage.
/// Expects: stack = [..., f64_val]
/// Produces: stack = [..., i64_boxed] where NaN is replaced with CANONICAL_NAN_BITS.
pub(crate) fn emit_f64_to_i64_canonical(func: &mut Function, scratch_local: u32) {
    // Reinterpret f64 to i64 raw bits, save in scratch
    func.instruction(&Instruction::I64ReinterpretF64);
    func.instruction(&Instruction::LocalTee(scratch_local));
    // Check if raw bits have QNAN prefix: (raw & QNAN) == QNAN
    func.instruction(&Instruction::I64Const(QNAN as i64));
    func.instruction(&Instruction::I64And);
    func.instruction(&Instruction::I64Const(QNAN as i64));
    func.instruction(&Instruction::I64Eq);
    // select(canonical, raw, is_nan) â€” if is_nan is true (nonzero), picks canonical
    func.instruction(&Instruction::If(BlockType::Result(ValType::I64)));
    func.instruction(&Instruction::I64Const(CANONICAL_NAN_BITS as i64));
    func.instruction(&Instruction::Else);
    func.instruction(&Instruction::LocalGet(scratch_local));
    func.instruction(&Instruction::End);
}
