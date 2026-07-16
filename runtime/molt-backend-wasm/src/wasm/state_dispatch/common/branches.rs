use super::super::super::control_flow::dispatch_control_panic;
use super::super::super::op_loop::WasmFunctionEmitContext;
use super::super::DispatchMode;
use super::super::plan::{NonLinearDispatchLocals, NonLinearDispatchPlan};
use crate::wasm_binary::emit_call;
use crate::wasm_plan::wasm_scalar_truthiness_fast_path_for_name;
use crate::wasm_values::emit_branch_truthiness_i32;
use crate::{FunctionIR, OpIR};
use std::collections::BTreeSet;
use wasm_encoder::{BlockType, Function, Instruction};

pub(in crate::wasm::state_dispatch) fn emit_dispatch_if(
    func: &mut Function,
    op_emitter: &WasmFunctionEmitContext<'_, '_>,
    plan: &NonLinearDispatchPlan,
    locals: NonLinearDispatchLocals,
    op: &OpIR,
    idx: usize,
    depth: u32,
) {
    let args = op.args.as_ref().unwrap();
    let cond = op_emitter.locals()[&args[0]];
    let else_idx = plan.control_maps.else_for_if.get(&idx).copied();
    let end_idx = plan
        .control_maps
        .end_for_if
        .get(&idx)
        .copied()
        .unwrap_or_else(|| {
            dispatch_control_panic(&op_emitter.func_ir.name, idx, "if without end_if")
        });
    let false_target = if let Some(else_pos) = else_idx {
        else_pos + 1
    } else {
        end_idx + 1
    };
    let truthy_import =
        if wasm_scalar_truthiness_fast_path_for_name(op_emitter.scalar_plan(), &args[0]) {
            crate::wasm_abi_generated::WasmRuntimeImport::IsTruthyInt
        } else {
            crate::wasm_abi_generated::WasmRuntimeImport::IsTruthy
        };
    emit_branch_truthiness_i32(
        func,
        cond,
        op_emitter.import_ids[truthy_import],
        op_emitter.reloc_enabled,
    );
    emit_conditional_state_branch(func, locals.state_local, idx + 1, false_target, depth + 1);
}

pub(in crate::wasm::state_dispatch) fn emit_dispatch_loop_break_cond(
    func: &mut Function,
    op_emitter: &WasmFunctionEmitContext<'_, '_>,
    plan: &NonLinearDispatchPlan,
    locals: NonLinearDispatchLocals,
    op: &OpIR,
    idx: usize,
    depth: u32,
    invert: bool,
) {
    let args = op.args.as_ref().unwrap();
    let cond = op_emitter.locals()[&args[0]];
    let end_idx = loop_break_target(plan, op_emitter.func_ir, idx, op.kind.as_str());
    let end_block = end_idx + 1;
    let next_block = idx + 1;
    emit_branch_truthiness_i32(
        func,
        cond,
        op_emitter.import_ids[crate::wasm_abi_generated::WasmRuntimeImport::IsTruthy],
        op_emitter.reloc_enabled,
    );
    if invert {
        func.instruction(&Instruction::I32Eqz);
    }
    emit_conditional_state_branch(func, locals.state_local, end_block, next_block, depth + 1);
}

pub(in crate::wasm::state_dispatch) fn emit_dispatch_check_exception(
    func: &mut Function,
    op_emitter: &WasmFunctionEmitContext<'_, '_>,
    plan: &NonLinearDispatchPlan,
    locals: NonLinearDispatchLocals,
    op: &OpIR,
    idx: usize,
    depth: u32,
    exception_regions: &BTreeSet<usize>,
) {
    let async_work_poll = op.kind == "async_work_poll";
    if !async_work_poll && (op_emitter.native_eh_enabled || exception_regions.contains(&idx)) {
        emit_set_state_and_br(func, locals.state_local, idx + 1, depth);
        return;
    }
    let target_label = op.value.unwrap_or_else(|| {
        dispatch_control_panic(
            &op_emitter.func_ir.name,
            idx,
            "check_exception missing label",
        )
    });
    let target_idx = label_target(
        plan,
        op_emitter.func_ir,
        idx,
        target_label,
        "check_exception",
    );
    emit_call(
        func,
        op_emitter.reloc_enabled,
        op_emitter.import_ids[if async_work_poll {
            crate::wasm_abi_generated::WasmRuntimeImport::AsyncWorkPollAndExceptionPending
        } else {
            crate::wasm_abi_generated::WasmRuntimeImport::ExceptionPending
        }],
    );
    func.instruction(&Instruction::I64Const(0));
    func.instruction(&Instruction::I64Ne);
    emit_conditional_state_branch(func, locals.state_local, target_idx, idx + 1, depth + 1);
}

pub(in crate::wasm::state_dispatch) fn emit_conditional_state_branch(
    func: &mut Function,
    state_local: u32,
    true_state: usize,
    false_state: usize,
    branch_depth: u32,
) {
    func.instruction(&Instruction::If(BlockType::Empty));
    emit_set_state_and_br(func, state_local, true_state, branch_depth);
    func.instruction(&Instruction::Else);
    emit_set_state_and_br(func, state_local, false_state, branch_depth);
    func.instruction(&Instruction::End);
}

pub(in crate::wasm::state_dispatch) fn emit_set_state_and_br(
    func: &mut Function,
    state_local: u32,
    state: usize,
    depth: u32,
) {
    func.instruction(&Instruction::I64Const(state as i64));
    func.instruction(&Instruction::LocalSet(state_local));
    func.instruction(&Instruction::Br(depth));
}

pub(in crate::wasm::state_dispatch) fn loop_break_target(
    plan: &NonLinearDispatchPlan,
    func_ir: &FunctionIR,
    idx: usize,
    kind: &str,
) -> usize {
    plan.control_maps
        .loop_break_target
        .get(&idx)
        .copied()
        .unwrap_or_else(|| {
            dispatch_control_panic(&func_ir.name, idx, format_args!("{kind} without loop"))
        })
}

pub(in crate::wasm::state_dispatch) fn label_target(
    plan: &NonLinearDispatchPlan,
    func_ir: &FunctionIR,
    idx: usize,
    label: i64,
    kind: &str,
) -> usize {
    plan.control_maps
        .label_to_index
        .get(&label)
        .copied()
        .unwrap_or_else(|| {
            dispatch_control_panic(
                &func_ir.name,
                idx,
                format_args!("unknown {kind} label {label}"),
            )
        })
}

pub(in crate::wasm::state_dispatch) fn require_stateful(
    mode: DispatchMode,
    func_ir: &FunctionIR,
    idx: usize,
    op: &OpIR,
) {
    if mode == DispatchMode::Stateful {
        return;
    }
    dispatch_control_panic(
        &func_ir.name,
        idx,
        format_args!("jumpful path hit stateful op {}", op.kind),
    );
}
