use super::boxed::emit_boxed_binary_call;
use super::operands::{BinaryOperands, IntBinaryTemps};
use crate::wasm_abi_generated::WasmRuntimeImport;
use crate::wasm_import_tracking::TrackedImportIds;
use crate::wasm_values::{
    ConstantCache, IntFastLane, emit_box_int_from_local_opt, emit_inline_int_range_check,
    emit_trusted_int_fast_path_guard_close, emit_trusted_int_fast_path_guard_open,
    emit_unbox_int_local_trusted_tee_opt,
};
use std::collections::BTreeMap;
use wasm_encoder::{BlockType, Function, Instruction, ValType};

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
