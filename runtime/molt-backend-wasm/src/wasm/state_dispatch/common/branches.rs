use super::super::super::control_flow::dispatch_control_panic;
use super::super::super::frame_locals::WasmDispatchFrameLocals;
use super::super::super::op_loop::WasmFunctionEmitContext;
use super::super::DispatchMode;
use super::super::plan::{ControlScopeDepth, NonLinearControlPlan, StaticDispatchEdge};
use crate::wasm_binary::emit_call;
use crate::wasm_plan::wasm_scalar_truthiness_fast_path_for_name;
use crate::wasm_values::emit_branch_truthiness_i32;
use crate::{FunctionIR, OpIR};
use molt_ir::simple_verify::EdgeRole;
use wasm_encoder::{BlockType, Function, Instruction};

pub(in crate::wasm::state_dispatch) fn emit_dispatch_if(
    func: &mut Function,
    context: &WasmFunctionEmitContext<'_, '_>,
    plan: &NonLinearControlPlan,
    locals: Option<WasmDispatchFrameLocals>,
    op: &OpIR,
    source: usize,
    depth: ControlScopeDepth,
) {
    let args = op.args.as_ref().unwrap();
    let cond = context.locals()[&args[0]];
    let truthy = if wasm_scalar_truthiness_fast_path_for_name(context.scalar_plan(), &args[0]) {
        crate::wasm_abi_generated::WasmRuntimeImport::IsTruthyInt
    } else {
        crate::wasm_abi_generated::WasmRuntimeImport::IsTruthy
    };
    emit_branch_truthiness_i32(
        func,
        cond,
        context.import_ids[truthy],
        context.reloc_enabled,
    );
    emit_conditional_state_branch(
        func,
        context,
        plan,
        locals,
        source,
        plan.successor(source, EdgeRole::BranchTrue),
        plan.successor(source, EdgeRole::BranchFalse),
        depth,
    );
}

pub(in crate::wasm::state_dispatch) fn emit_dispatch_loop_break_cond(
    func: &mut Function,
    context: &WasmFunctionEmitContext<'_, '_>,
    plan: &NonLinearControlPlan,
    locals: Option<WasmDispatchFrameLocals>,
    op: &OpIR,
    source: usize,
    depth: ControlScopeDepth,
) {
    let args = op.args.as_ref().unwrap();
    emit_branch_truthiness_i32(
        func,
        context.locals()[&args[0]],
        context.import_ids[crate::wasm_abi_generated::WasmRuntimeImport::IsTruthy],
        context.reloc_enabled,
    );
    emit_conditional_state_branch(
        func,
        context,
        plan,
        locals,
        source,
        plan.successor(source, EdgeRole::BranchTrue),
        plan.successor(source, EdgeRole::BranchFalse),
        depth,
    );
}

pub(in crate::wasm::state_dispatch) fn emit_dispatch_check_exception(
    func: &mut Function,
    context: &WasmFunctionEmitContext<'_, '_>,
    plan: &NonLinearControlPlan,
    locals: Option<WasmDispatchFrameLocals>,
    op: &OpIR,
    source: usize,
    depth: ControlScopeDepth,
) {
    // Observe exactly the authored statement boundary, including async polls.
    emit_call(
        func,
        context.reloc_enabled,
        context.import_ids[if op.is_async_work_poll() {
            crate::wasm_abi_generated::WasmRuntimeImport::AsyncWorkPollAndExceptionPending
        } else {
            crate::wasm_abi_generated::WasmRuntimeImport::ExceptionPending
        }],
    );
    func.instruction(&Instruction::I64Const(0));
    func.instruction(&Instruction::I64Ne);
    emit_conditional_state_branch(
        func,
        context,
        plan,
        locals,
        source,
        plan.successor(source, EdgeRole::Exception),
        plan.successor(source, EdgeRole::Normal),
        depth,
    );
}

pub(in crate::wasm::state_dispatch) fn emit_conditional_state_branch(
    func: &mut Function,
    context: &WasmFunctionEmitContext<'_, '_>,
    plan: &NonLinearControlPlan,
    locals: Option<WasmDispatchFrameLocals>,
    source: usize,
    true_target: usize,
    false_target: usize,
    depth: ControlScopeDepth,
) {
    func.instruction(&Instruction::If(BlockType::Empty));
    emit_static_dispatch_edge(func, context, plan, locals, source, true_target, depth, 1);
    func.instruction(&Instruction::Else);
    emit_static_dispatch_edge(func, context, plan, locals, source, false_target, depth, 1);
    func.instruction(&Instruction::End);
}

pub(in crate::wasm::state_dispatch) fn emit_static_dispatch_edge(
    func: &mut Function,
    context: &WasmFunctionEmitContext<'_, '_>,
    plan: &NonLinearControlPlan,
    locals: Option<WasmDispatchFrameLocals>,
    source: usize,
    target: usize,
    depth: ControlScopeDepth,
    selection_depth: u32,
) {
    match plan.static_edge(source, target) {
        StaticDispatchEdge::Forward { label_depth } => {
            func.instruction(&Instruction::Br(label_depth + selection_depth));
        }
        StaticDispatchEdge::Redispatch { operation } => {
            let locals = locals.expect("canonical backward transfer requires dispatch locals");
            func.instruction(&Instruction::I64Const(operation as i64));
            func.instruction(&Instruction::LocalSet(locals.state_local));
            func.instruction(&Instruction::Br(depth.redispatch_depth(selection_depth)));
        }
        StaticDispatchEdge::ReturnNone => {
            plan.require_terminal_return(&context.func_ir.name, source);
            context.const_cache().emit_none(func);
            context
                .frame
                .emit_return(func, depth.return_depth(selection_depth));
        }
    }
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
