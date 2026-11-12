use super::boxed::emit_boxed_binary_call;
use super::operands::{BinaryOperands, IntBinaryTemps};
use crate::wasm_abi_generated::WasmRuntimeImport;
use crate::wasm_binary::Function;
use crate::wasm_import_tracking::TrackedImportIds;
use crate::wasm_values::{
    ConstantCache, IntFastLane, emit_box_int_from_local_opt, emit_inline_int_range_check,
    emit_trusted_int_fast_path_guard_close, emit_trusted_int_fast_path_guard_open,
    emit_unbox_int_local_trusted_tee_opt,
};
use std::collections::BTreeMap;
use wasm_encoder::{BlockType, Instruction, ValType};

pub(in crate::wasm::op_loop::numeric_ops) fn emit_trusted_int_binary_operand_tees(
    func: &mut Function,
    operands: BinaryOperands,
    temps: IntBinaryTemps,
    const_cache: &ConstantCache,
    known_raw_ints: &BTreeMap<u32, i64>,
) {
    emit_unbox_int_local_trusted_tee_opt(
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
}

pub(in crate::wasm::op_loop::numeric_ops) fn emit_guarded_int_binary_result_or_boxed(
    func: &mut Function,
    operands: BinaryOperands,
    import_ids: &TrackedImportIds,
    import: WasmRuntimeImport,
    reloc_enabled: bool,
    known_raw_ints: &BTreeMap<u32, i64>,
    lane: IntFastLane,
    emit_fast_result: impl FnOnce(&mut Function),
) {
    let operand_locals = operands.locals();
    let guarded =
        emit_trusted_int_fast_path_guard_open(func, &operand_locals, known_raw_ints, lane);
    emit_fast_result(func);
    if guarded {
        emit_trusted_int_fast_path_guard_close(
            func,
            reloc_enabled,
            &operand_locals,
            import_ids[import],
        );
    }
}

pub(in crate::wasm::op_loop::numeric_ops) fn emit_inline_int_result_or_boxed(
    func: &mut Function,
    raw_result_local: u32,
    operands: BinaryOperands,
    import_ids: &TrackedImportIds,
    import: WasmRuntimeImport,
    const_cache: &ConstantCache,
    reloc_enabled: bool,
    known_raw_ints: &BTreeMap<u32, i64>,
) {
    emit_inline_int_range_check(func, raw_result_local, const_cache);
    emit_inline_int_if_else_boxed(
        func,
        raw_result_local,
        operands,
        import_ids,
        import,
        reloc_enabled,
        known_raw_ints,
    );
}

/// `i64.mul` wraps, so the product of two inline integers can leave i64 and
/// still land in the inline range (2**40 * 2**40 wraps to 0). Keep the product
/// only when it is exact (`lhs == 0` or `product / lhs == rhs`) and inline, as
/// the left shift keeps `(result >> rhs) == lhs`. Both operands are inline ints
/// (the tag guard admits only those, and a known raw int is always inline), so
/// neither is `i64::MIN`, and a zero `lhs` takes the first arm: the division
/// cannot trap.
pub(in crate::wasm::op_loop::numeric_ops) fn emit_inline_int_product_or_boxed(
    func: &mut Function,
    temps: IntBinaryTemps,
    operands: BinaryOperands,
    import_ids: &TrackedImportIds,
    import: WasmRuntimeImport,
    const_cache: &ConstantCache,
    reloc_enabled: bool,
    known_raw_ints: &BTreeMap<u32, i64>,
) {
    func.instruction(&Instruction::LocalGet(temps.lhs));
    func.instruction(&Instruction::I64Eqz);
    func.instruction(&Instruction::If(BlockType::Result(ValType::I32)));
    func.instruction(&Instruction::I32Const(1));
    func.instruction(&Instruction::Else);
    func.instruction(&Instruction::LocalGet(temps.result));
    func.instruction(&Instruction::LocalGet(temps.lhs));
    func.instruction(&Instruction::I64DivS);
    func.instruction(&Instruction::LocalGet(temps.rhs));
    func.instruction(&Instruction::I64Eq);
    func.instruction(&Instruction::End);
    emit_inline_int_range_check(func, temps.result, const_cache);
    func.instruction(&Instruction::I32And);
    emit_inline_int_if_else_boxed(
        func,
        temps.result,
        operands,
        import_ids,
        import,
        reloc_enabled,
        known_raw_ints,
    );
}

/// Consume the i32 condition on the stack: box the raw result inline when it
/// holds, else call the runtime on the boxed operands.
fn emit_inline_int_if_else_boxed(
    func: &mut Function,
    raw_result_local: u32,
    operands: BinaryOperands,
    import_ids: &TrackedImportIds,
    import: WasmRuntimeImport,
    reloc_enabled: bool,
    known_raw_ints: &BTreeMap<u32, i64>,
) {
    func.instruction(&Instruction::If(BlockType::Result(ValType::I64)));
    emit_box_int_from_local_opt(func, raw_result_local, known_raw_ints);
    func.instruction(&Instruction::Else);
    emit_boxed_binary_call(func, operands, import_ids, import, reloc_enabled);
    func.instruction(&Instruction::End);
}

pub(in crate::wasm::op_loop::numeric_ops) fn emit_inline_int_result(
    func: &mut Function,
    raw_result_local: u32,
    known_raw_ints: &BTreeMap<u32, i64>,
) {
    emit_box_int_from_local_opt(func, raw_result_local, known_raw_ints);
}
