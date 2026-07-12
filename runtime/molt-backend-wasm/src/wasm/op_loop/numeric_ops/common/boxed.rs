use super::super::super::result_sink::store_result_or_drop;
use super::operands::{BinaryOperands, binary_operands, ternary_operands, unary_operand};
use crate::OpIR;
use crate::wasm::WasmFrameLocals;
use crate::wasm_abi_generated::WasmRuntimeImport;
use crate::wasm_binary::emit_call;
use crate::wasm_import_tracking::TrackedImportIds;
use wasm_encoder::{Function, Instruction};

pub(in crate::wasm::op_loop::numeric_ops) fn emit_boxed_unary_call(
    func: &mut Function,
    operand: u32,
    import_ids: &TrackedImportIds,
    import: WasmRuntimeImport,
    reloc_enabled: bool,
) {
    func.instruction(&Instruction::LocalGet(operand));
    emit_call(func, reloc_enabled, import_ids[import]);
}

pub(in crate::wasm::op_loop::numeric_ops) fn emit_boxed_binary_call(
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

pub(in crate::wasm::op_loop::numeric_ops) fn emit_boxed_ternary_call(
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

pub(in crate::wasm::op_loop::numeric_ops) fn emit_boxed_unary_result(
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

pub(in crate::wasm::op_loop::numeric_ops) fn emit_boxed_binary_result(
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

pub(in crate::wasm::op_loop::numeric_ops) fn emit_boxed_ternary_result(
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

pub(in crate::wasm::op_loop::numeric_ops) fn store_numeric_result(
    func: &mut Function,
    op: &OpIR,
    locals: &WasmFrameLocals,
) {
    store_result_or_drop(func, op, locals);
}
