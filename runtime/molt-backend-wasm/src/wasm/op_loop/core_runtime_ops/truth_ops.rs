use crate::OpIR;
use crate::representation_plan::ScalarRepresentationPlan;
use crate::wasm::WasmFrameLocals;
use crate::wasm_binary::emit_call;
use crate::wasm_import_tracking::TrackedImportIds;
use crate::wasm_plan::wasm_scalar_truthiness_fast_path_for_name;
use crate::wasm_values::emit_box_bool_from_i32;
use wasm_encoder::{BlockType, Function, Instruction, ValType};

pub(super) fn emit_truth_runtime_op(
    func: &mut Function,
    op: &OpIR,
    import_ids: &TrackedImportIds,
    locals: &WasmFrameLocals,
    scalar_plan: &ScalarRepresentationPlan,
    reloc_enabled: bool,
) -> bool {
    match op.kind.as_str() {
        "bool" | "cast_bool" | "builtin_bool" => {
            emit_bool_like(func, op, import_ids, locals, scalar_plan, reloc_enabled)
        }
        "and" => emit_and(func, op, import_ids, locals, reloc_enabled),
        "or" => emit_or(func, op, import_ids, locals, reloc_enabled),
        _ => return false,
    }
    true
}

fn emit_bool_like(
    func: &mut Function,
    op: &OpIR,
    import_ids: &TrackedImportIds,
    locals: &WasmFrameLocals,
    scalar_plan: &ScalarRepresentationPlan,
    reloc_enabled: bool,
) {
    let args = op.args.as_ref().unwrap();
    let val = locals[&args[0]];
    let truthy_import = if wasm_scalar_truthiness_fast_path_for_name(scalar_plan, &args[0]) {
        crate::wasm_abi_generated::WasmRuntimeImport::IsTruthyInt
    } else {
        crate::wasm_abi_generated::WasmRuntimeImport::IsTruthy
    };
    func.instruction(&Instruction::LocalGet(val));
    emit_call(func, reloc_enabled, import_ids[truthy_import]);
    func.instruction(&Instruction::I64Const(0));
    func.instruction(&Instruction::I64Ne);
    emit_box_bool_from_i32(func);
    if let Some(out) = op.out.as_ref() {
        let res = locals[out];
        func.instruction(&Instruction::LocalSet(res));
    } else {
        func.instruction(&Instruction::Drop);
    }
}

fn emit_and(
    func: &mut Function,
    op: &OpIR,
    import_ids: &TrackedImportIds,
    locals: &WasmFrameLocals,
    reloc_enabled: bool,
) {
    let args = op.args.as_ref().unwrap();
    let lhs = locals[&args[0]];
    let rhs = locals[&args[1]];
    func.instruction(&Instruction::LocalGet(lhs));
    emit_call(
        func,
        reloc_enabled,
        import_ids[crate::wasm_abi_generated::WasmRuntimeImport::IsTruthy],
    );
    func.instruction(&Instruction::I64Const(0));
    func.instruction(&Instruction::I64Ne);
    func.instruction(&Instruction::If(BlockType::Result(ValType::I64)));
    func.instruction(&Instruction::LocalGet(rhs));
    func.instruction(&Instruction::Else);
    func.instruction(&Instruction::LocalGet(lhs));
    func.instruction(&Instruction::End);
    if let Some(out) = op.out.as_ref() {
        let res = locals[out];
        debug_assert!(
            crate::tir::op_kinds_generated::kind_result_mints_owned_selected_operand_table("and")
        );
        func.instruction(&Instruction::LocalTee(res));
        emit_call(
            func,
            reloc_enabled,
            import_ids[crate::wasm_abi_generated::WasmRuntimeImport::IncRefObj],
        );
    } else {
        func.instruction(&Instruction::Drop);
    }
}

fn emit_or(
    func: &mut Function,
    op: &OpIR,
    import_ids: &TrackedImportIds,
    locals: &WasmFrameLocals,
    reloc_enabled: bool,
) {
    let args = op.args.as_ref().unwrap();
    let lhs = locals[&args[0]];
    let rhs = locals[&args[1]];
    func.instruction(&Instruction::LocalGet(lhs));
    emit_call(
        func,
        reloc_enabled,
        import_ids[crate::wasm_abi_generated::WasmRuntimeImport::IsTruthy],
    );
    func.instruction(&Instruction::I64Const(0));
    func.instruction(&Instruction::I64Ne);
    func.instruction(&Instruction::If(BlockType::Result(ValType::I64)));
    func.instruction(&Instruction::LocalGet(lhs));
    func.instruction(&Instruction::Else);
    func.instruction(&Instruction::LocalGet(rhs));
    func.instruction(&Instruction::End);
    if let Some(out) = op.out.as_ref() {
        let res = locals[out];
        debug_assert!(
            crate::tir::op_kinds_generated::kind_result_mints_owned_selected_operand_table("or")
        );
        func.instruction(&Instruction::LocalTee(res));
        emit_call(
            func,
            reloc_enabled,
            import_ids[crate::wasm_abi_generated::WasmRuntimeImport::IncRefObj],
        );
    } else {
        func.instruction(&Instruction::Drop);
    }
}
