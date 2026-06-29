use super::super::result_sink::store_result_or_drop;
use crate::OpIR;
use crate::wasm::{WasmFrameLocals, WasmFrameSyntheticLocal};
use crate::wasm_abi_generated::WasmRuntimeImport;
use crate::wasm_binary::emit_call;
use crate::wasm_import_tracking::TrackedImportIds;
use crate::wasm_values::{
    ConstantCache, IntFastLane, emit_box_int_from_local_opt, emit_f64_to_i64_canonical,
    emit_inline_int_range_check, emit_trusted_int_fast_path_guard_close,
    emit_trusted_int_fast_path_guard_open, emit_unbox_int_local_trusted_tee_opt,
};
use std::collections::BTreeMap;
use wasm_encoder::{BlockType, Function, Instruction, ValType};

#[derive(Clone, Copy)]
pub(super) struct BinaryOperands {
    pub(super) lhs: u32,
    pub(super) rhs: u32,
}

impl BinaryOperands {
    fn locals(self) -> [u32; 2] {
        [self.lhs, self.rhs]
    }
}

#[derive(Clone, Copy)]
pub(super) struct IntBinaryTemps {
    pub(super) lhs: u32,
    pub(super) rhs: u32,
    pub(super) result: u32,
}

pub(super) fn binary_operands(op: &OpIR, locals: &WasmFrameLocals) -> BinaryOperands {
    let args = op.args.as_ref().unwrap();
    BinaryOperands {
        lhs: locals[&args[0]],
        rhs: locals[&args[1]],
    }
}

pub(super) fn unary_operand(op: &OpIR, locals: &WasmFrameLocals) -> u32 {
    let args = op.args.as_ref().unwrap();
    locals[&args[0]]
}

pub(super) fn ternary_operands(op: &OpIR, locals: &WasmFrameLocals) -> [u32; 3] {
    let args = op.args.as_ref().unwrap();
    [locals[&args[0]], locals[&args[1]], locals[&args[2]]]
}

pub(super) fn int_binary_temps(locals: &WasmFrameLocals) -> IntBinaryTemps {
    IntBinaryTemps {
        lhs: locals.synthetic(WasmFrameSyntheticLocal::MoltTmp0),
        rhs: locals.synthetic(WasmFrameSyntheticLocal::MoltTmp1),
        result: locals.synthetic(WasmFrameSyntheticLocal::MoltTmp2),
    }
}

pub(super) fn emit_trusted_int_binary_operand_tees(
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

pub(super) fn emit_boxed_unary_call(
    func: &mut Function,
    operand: u32,
    import_ids: &TrackedImportIds,
    import: WasmRuntimeImport,
    reloc_enabled: bool,
) {
    func.instruction(&Instruction::LocalGet(operand));
    emit_call(func, reloc_enabled, import_ids[import]);
}

pub(super) fn emit_boxed_binary_call(
    func: &mut Function,
    operands: BinaryOperands,
    import_ids: &TrackedImportIds,
    import: WasmRuntimeImport,
    reloc_enabled: bool,
) {
    func.instruction(&Instruction::LocalGet(operands.lhs));
    func.instruction(&Instruction::LocalGet(operands.rhs));
    emit_call(func, reloc_enabled, import_ids[import]);
}

pub(super) fn emit_boxed_ternary_call(
    func: &mut Function,
    operands: [u32; 3],
    import_ids: &TrackedImportIds,
    import: WasmRuntimeImport,
    reloc_enabled: bool,
) {
    for operand in operands {
        func.instruction(&Instruction::LocalGet(operand));
    }
    emit_call(func, reloc_enabled, import_ids[import]);
}

pub(super) fn emit_boxed_unary_result(
    func: &mut Function,
    op: &OpIR,
    import_ids: &TrackedImportIds,
    locals: &WasmFrameLocals,
    import: WasmRuntimeImport,
    reloc_enabled: bool,
) {
    emit_boxed_unary_call(
        func,
        unary_operand(op, locals),
        import_ids,
        import,
        reloc_enabled,
    );
    store_numeric_result(func, op, locals);
}

pub(super) fn emit_boxed_binary_result(
    func: &mut Function,
    op: &OpIR,
    import_ids: &TrackedImportIds,
    locals: &WasmFrameLocals,
    import: WasmRuntimeImport,
    reloc_enabled: bool,
) {
    emit_boxed_binary_call(
        func,
        binary_operands(op, locals),
        import_ids,
        import,
        reloc_enabled,
    );
    store_numeric_result(func, op, locals);
}

pub(super) fn emit_boxed_ternary_result(
    func: &mut Function,
    op: &OpIR,
    import_ids: &TrackedImportIds,
    locals: &WasmFrameLocals,
    import: WasmRuntimeImport,
    reloc_enabled: bool,
) {
    emit_boxed_ternary_call(
        func,
        ternary_operands(op, locals),
        import_ids,
        import,
        reloc_enabled,
    );
    store_numeric_result(func, op, locals);
}

pub(super) fn store_numeric_result(func: &mut Function, op: &OpIR, locals: &WasmFrameLocals) {
    store_result_or_drop(func, op, locals);
}

pub(super) fn emit_guarded_int_binary_result_or_boxed(
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

pub(super) fn emit_inline_int_result_or_boxed(
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

pub(super) fn emit_plain_f64_binary_result_or_boxed(
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
    func.instruction(&Instruction::LocalGet(operands.lhs));
    func.instruction(&Instruction::F64ReinterpretI64);
    func.instruction(&Instruction::LocalGet(operands.rhs));
    func.instruction(&Instruction::F64ReinterpretI64);
    emit_f64_result(func, locals.synthetic(WasmFrameSyntheticLocal::MoltTmp3));
    func.instruction(&Instruction::Else);
    emit_boxed_binary_call(func, operands, import_ids, import, reloc_enabled);
    func.instruction(&Instruction::End);
}

pub(super) fn emit_plain_f64_arithmetic_result(func: &mut Function, scratch_local: u32) {
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
