use crate::wasm_binary::emit_call;
use molt_codegen_abi::{QNAN, QNAN_TAG_MASK_I64, TAG_BOOL};
use wasm_encoder::{BlockType, Function, Instruction, ValType};

/// Push an `i32` boolean (`1` = truthy, `0` = falsy) for `cond_local` to be
/// consumed by a control-flow branch (`br_if` / `if` / `loop_break_if_*`).
///
/// For a NaN-boxed **bool** this reads bit 0 directly; for everything else it
/// falls back to the runtime `molt_is_truthy`.  This mirrors the native
/// backend's `br_if` truthiness dispatch (which checks the bool tag and reads
/// bit 0 inline) and is the load-bearing correctness fix for the exception
/// break:
///
/// `molt_is_truthy` returns **false** whenever an exception is pending
/// (CPython truthiness can never be evaluated with an exception in flight).
/// The iterator-consumer exception break is gated on
/// `box_bool(molt_exception_pending())`; routing that boxed bool through
/// `is_truthy` while the very exception it checks is pending would make the
/// break unconditionally not-taken â€” the loop would spin forever (OOM).
/// Reading bit 0 of a boxed bool is exception-independent and value-exact
/// (`True`â†’1, `False`â†’0), so the break fires correctly.  For non-bool
/// conditions the behaviour is unchanged (the runtime helper is still called).
pub(crate) fn emit_branch_truthiness_i32(
    func: &mut Function,
    cond_local: u32,
    is_truthy_import: u32,
    reloc_enabled: bool,
) {
    // is_boxed_bool = (cond & QNAN_TAG_MASK) == (QNAN | TAG_BOOL)
    func.instruction(&Instruction::LocalGet(cond_local));
    func.instruction(&Instruction::I64Const(QNAN_TAG_MASK_I64));
    func.instruction(&Instruction::I64And);
    func.instruction(&Instruction::I64Const((QNAN | TAG_BOOL) as i64));
    func.instruction(&Instruction::I64Eq);
    func.instruction(&Instruction::If(BlockType::Result(ValType::I32)));
    // Boxed bool: truthiness is bit 0 (no GIL/exception dependence).
    func.instruction(&Instruction::LocalGet(cond_local));
    func.instruction(&Instruction::I32WrapI64);
    func.instruction(&Instruction::I32Const(1));
    func.instruction(&Instruction::I32And);
    func.instruction(&Instruction::Else);
    // Non-bool: defer to the runtime truthiness helper (`!= 0`).
    func.instruction(&Instruction::LocalGet(cond_local));
    emit_call(func, reloc_enabled, is_truthy_import);
    func.instruction(&Instruction::I64Const(0));
    func.instruction(&Instruction::I64Ne);
    func.instruction(&Instruction::End);
}
