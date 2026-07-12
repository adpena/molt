use super::super::common::{
    BinaryOperands, IntBinaryTemps, emit_boxed_binary_call, emit_inline_int_result_or_boxed,
    emit_plain_f64_arithmetic_result,
};
use crate::wasm_abi_generated::WasmNumericOpLoopKind;
use crate::wasm_import_tracking::TrackedImportIds;
use crate::wasm_values::ConstantCache;
use std::collections::BTreeMap;
use wasm_encoder::{BlockType, Function, Instruction, ValType};

pub(super) fn emit_nonzero_rhs_raw_division_or_boxed(
    func: &mut Function,
    operands: BinaryOperands,
    import_ids: &TrackedImportIds,
    const_cache: &ConstantCache,
    f64_scratch_local: u32,
    reloc_enabled: bool,
    known_raw_ints: &BTreeMap<u32, i64>,
    import_name: crate::wasm_abi_generated::WasmRuntimeImport,
    division_op: WasmNumericOpLoopKind,
    temps: IntBinaryTemps,
) {
    func.instruction(&Instruction::I64Const(0));
    func.instruction(&Instruction::I64Ne);
    func.instruction(&Instruction::If(BlockType::Result(ValType::I64)));
    match division_op {
        WasmNumericOpLoopKind::TrueDiv => emit_raw_true_div(func, temps, f64_scratch_local),
        WasmNumericOpLoopKind::FloorDiv => emit_raw_floor_div(
            func,
            operands,
            import_ids,
            const_cache,
            reloc_enabled,
            known_raw_ints,
            import_name,
            temps,
        ),
        WasmNumericOpLoopKind::Mod => emit_raw_mod(
            func,
            operands,
            import_ids,
            const_cache,
            reloc_enabled,
            known_raw_ints,
            import_name,
            temps,
        ),
        _ => unreachable!("non-division numeric selector routed to raw division"),
    }
    func.instruction(&Instruction::Else);
    emit_boxed_binary_call(func, operands, import_ids, import_name, reloc_enabled);
    func.instruction(&Instruction::End);
}

fn emit_raw_true_div(func: &mut Function, temps: IntBinaryTemps, scratch: u32) {
    func.instruction(&Instruction::LocalGet(temps.lhs));
    func.instruction(&Instruction::F64ConvertI64S);
    func.instruction(&Instruction::LocalGet(temps.rhs));
    func.instruction(&Instruction::F64ConvertI64S);
    func.instruction(&Instruction::F64Div);
    emit_plain_f64_arithmetic_result(func, scratch);
}

fn emit_raw_floor_div(
    func: &mut Function,
    operands: BinaryOperands,
    import_ids: &TrackedImportIds,
    const_cache: &ConstantCache,
    reloc_enabled: bool,
    known_raw_ints: &BTreeMap<u32, i64>,
    import_name: crate::wasm_abi_generated::WasmRuntimeImport,
    temps: IntBinaryTemps,
) {
    func.instruction(&Instruction::LocalGet(temps.lhs));
    func.instruction(&Instruction::LocalGet(temps.rhs));
    func.instruction(&Instruction::I64DivS);
    func.instruction(&Instruction::LocalSet(temps.result));

    func.instruction(&Instruction::LocalGet(temps.lhs));
    func.instruction(&Instruction::LocalGet(temps.rhs));
    func.instruction(&Instruction::I64RemS);
    func.instruction(&Instruction::I64Const(0));
    func.instruction(&Instruction::I64Ne);
    emit_quotient_signs_differ(func, temps);
    func.instruction(&Instruction::I32And);
    func.instruction(&Instruction::If(BlockType::Empty));
    func.instruction(&Instruction::LocalGet(temps.result));
    func.instruction(&Instruction::I64Const(1));
    func.instruction(&Instruction::I64Sub);
    func.instruction(&Instruction::LocalSet(temps.result));
    func.instruction(&Instruction::End);

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
}

fn emit_raw_mod(
    func: &mut Function,
    operands: BinaryOperands,
    import_ids: &TrackedImportIds,
    const_cache: &ConstantCache,
    reloc_enabled: bool,
    known_raw_ints: &BTreeMap<u32, i64>,
    import_name: crate::wasm_abi_generated::WasmRuntimeImport,
    temps: IntBinaryTemps,
) {
    func.instruction(&Instruction::LocalGet(temps.lhs));
    func.instruction(&Instruction::LocalGet(temps.rhs));
    func.instruction(&Instruction::I64RemS);
    func.instruction(&Instruction::LocalSet(temps.result));
    func.instruction(&Instruction::LocalGet(temps.result));
    func.instruction(&Instruction::I64Const(0));
    func.instruction(&Instruction::I64Ne);
    emit_quotient_signs_differ(func, temps);
    func.instruction(&Instruction::I32And);
    func.instruction(&Instruction::If(BlockType::Empty));
    func.instruction(&Instruction::LocalGet(temps.result));
    func.instruction(&Instruction::LocalGet(temps.rhs));
    func.instruction(&Instruction::I64Add);
    func.instruction(&Instruction::LocalSet(temps.result));
    func.instruction(&Instruction::End);
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
}

fn emit_quotient_signs_differ(func: &mut Function, temps: IntBinaryTemps) {
    func.instruction(&Instruction::LocalGet(temps.lhs));
    func.instruction(&Instruction::I64Const(0));
    func.instruction(&Instruction::I64LtS);
    func.instruction(&Instruction::LocalGet(temps.rhs));
    func.instruction(&Instruction::I64Const(0));
    func.instruction(&Instruction::I64LtS);
    func.instruction(&Instruction::I32Xor);
}
