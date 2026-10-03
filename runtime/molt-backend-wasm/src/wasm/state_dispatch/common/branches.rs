use super::super::super::control_flow::dispatch_control_panic;
use super::super::super::op_loop::WasmFunctionEmitContext;
use super::super::DispatchMode;
use super::super::plan::{NonLinearDispatchLocals, NonLinearDispatchPlan, StaticDispatchEdge};
use crate::wasm_binary::emit_call;
use crate::wasm_plan::wasm_scalar_truthiness_fast_path_for_name;
use crate::wasm_values::emit_branch_truthiness_i32;
use crate::{FunctionIR, OpIR};
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
    emit_conditional_state_branch(
        func,
        op_emitter,
        plan,
        locals,
        idx,
        idx + 1,
        false_target,
        depth,
    );
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
    emit_conditional_state_branch(
        func, op_emitter, plan, locals, idx, end_block, next_block, depth,
    );
}

pub(in crate::wasm::state_dispatch) fn emit_dispatch_check_exception(
    func: &mut Function,
    op_emitter: &WasmFunctionEmitContext<'_, '_>,
    plan: &NonLinearDispatchPlan,
    locals: NonLinearDispatchLocals,
    op: &OpIR,
    idx: usize,
    depth: u32,
) {
    let async_work_poll = op.is_async_work_poll();
    // Each observer carries its actual exceptional successor. Handler/else
    // membership and source order cannot suppress a live check; dispatch uses
    // the polling exception protocol even when native EH was requested.
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
    emit_conditional_state_branch(
        func,
        op_emitter,
        plan,
        locals,
        idx,
        target_idx,
        idx + 1,
        depth,
    );
}

pub(in crate::wasm::state_dispatch) fn emit_conditional_state_branch(
    func: &mut Function,
    op_emitter: &WasmFunctionEmitContext<'_, '_>,
    plan: &NonLinearDispatchPlan,
    locals: NonLinearDispatchLocals,
    source: usize,
    true_state: usize,
    false_state: usize,
    dispatch_depth: u32,
) {
    func.instruction(&Instruction::If(BlockType::Empty));
    emit_static_dispatch_edge(
        func,
        op_emitter,
        plan,
        locals,
        source,
        true_state,
        dispatch_depth,
        1,
    );
    func.instruction(&Instruction::Else);
    emit_static_dispatch_edge(
        func,
        op_emitter,
        plan,
        locals,
        source,
        false_state,
        dispatch_depth,
        1,
    );
    func.instruction(&Instruction::End);
}

pub(in crate::wasm::state_dispatch) fn emit_static_dispatch_edge(
    func: &mut Function,
    op_emitter: &WasmFunctionEmitContext<'_, '_>,
    plan: &NonLinearDispatchPlan,
    locals: NonLinearDispatchLocals,
    source: usize,
    target: usize,
    dispatch_depth: u32,
    selection_depth: u32,
) {
    match plan.static_edge(source, target) {
        StaticDispatchEdge::Forward { label_depth } => {
            func.instruction(&Instruction::Br(label_depth + selection_depth));
        }
        StaticDispatchEdge::Redispatch { operation } => {
            func.instruction(&Instruction::I64Const(operation as i64));
            func.instruction(&Instruction::LocalSet(locals.state_local));
            func.instruction(&Instruction::Br(dispatch_depth + selection_depth));
        }
        StaticDispatchEdge::ReturnNone => {
            if plan.state_resume.is_some() {
                dispatch_control_panic(
                    &op_emitter.func_ir.name,
                    source,
                    "stateful fallthrough requires an explicit terminal return",
                );
            }
            op_emitter.const_cache().emit_none(func);
            op_emitter
                .frame
                .emit_return(func, dispatch_depth + selection_depth + 1);
        }
    }
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
