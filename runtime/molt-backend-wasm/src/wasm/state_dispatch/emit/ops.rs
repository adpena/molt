use crate::OpIR;
use crate::wasm::control_flow::ControlKind;
use crate::wasm::frame_locals::WasmDispatchFrameLocals;
use crate::wasm::op_loop::WasmFunctionEmitContext;
use crate::wasm::state_dispatch::DispatchMode;
use crate::wasm::state_dispatch::common::{
    emit_conditional_state_branch, emit_dispatch_check_exception, emit_dispatch_if,
    emit_dispatch_loop_break_cond, emit_static_dispatch_edge, require_stateful,
};
use crate::wasm::state_dispatch::plan::{ControlScopeDepth, NonLinearControlPlan};
use crate::wasm::state_dispatch::stateful_ops::emit_activation_op;
use crate::wasm_binary::emit_call;
use crate::wasm_values::emit_branch_truthiness_i32;
use molt_ir::simple_verify::EdgeRole;
use std::collections::BTreeMap;
use wasm_encoder::{BlockType, Function, Instruction};

#[derive(Default)]
pub(super) struct DispatchOpScratch {
    control: Vec<ControlKind>,
    try_stack: Vec<usize>,
    label_stack: Vec<i64>,
    label_depths: BTreeMap<i64, usize>,
}

pub(super) fn emit_dispatch_op(
    func: &mut Function,
    context: &mut WasmFunctionEmitContext<'_, '_>,
    plan: &NonLinearControlPlan,
    locals: Option<WasmDispatchFrameLocals>,
    mode: DispatchMode,
    op: &OpIR,
    idx: usize,
    depth: ControlScopeDepth,
    scratch: &mut DispatchOpScratch,
) -> bool {
    match op.kind {
        molt_ir::OpKind::StateSwitch => {
            require_stateful(mode, context.func_ir, idx, op);
            let locals = locals.expect("stateful control requires dispatch locals");
            let resume = locals
                .resume_state_local
                .expect("resume state local missing");
            func.instruction(&Instruction::LocalGet(resume));
            func.instruction(&Instruction::LocalSet(locals.state_local));
            func.instruction(&Instruction::Br(depth.redispatch_depth(0)));
            true
        }
        molt_ir::OpKind::Aiter if mode == DispatchMode::Stateful => {
            let args = op.args.as_ref().unwrap();
            func.instruction(&Instruction::LocalGet(context.locals()[&args[0]]));
            emit_call(
                func,
                context.reloc_enabled,
                context.import_ids[crate::wasm_abi_generated::WasmRuntimeImport::Aiter],
            );
            func.instruction(&Instruction::LocalSet(
                context.locals()[op.out.as_ref().unwrap()],
            ));
            false
        }
        molt_ir::OpKind::StateSet | molt_ir::OpKind::TaskWait | molt_ir::OpKind::IsPending => {
            require_stateful(mode, context.func_ir, idx, op);
            emit_activation_op(
                func,
                context,
                plan,
                locals.expect("stateful control requires dispatch locals"),
                op,
            );
            false
        }
        molt_ir::OpKind::StateTransition | molt_ir::OpKind::StateYield => panic!(
            "{}: suspension must be normalized by shared terminal ownership before WASM emission",
            context.func_ir.name
        ),
        molt_ir::OpKind::If => {
            emit_dispatch_if(func, context, plan, locals, op, idx, depth);
            true
        }
        molt_ir::OpKind::Else
        | molt_ir::OpKind::EndIf
        | molt_ir::OpKind::LoopStart
        | molt_ir::OpKind::LoopEnd
        | molt_ir::OpKind::TryStart
        | molt_ir::OpKind::TryEnd
        | molt_ir::OpKind::Label
        | molt_ir::OpKind::StateLabel
        | molt_ir::OpKind::LoopBreak
        | molt_ir::OpKind::LoopContinue
        | molt_ir::OpKind::Jump => {
            emit_static_dispatch_edge(
                func,
                context,
                plan,
                locals,
                idx,
                plan.sole_successor(idx),
                depth,
                0,
            );
            true
        }
        molt_ir::OpKind::LoopBreakIfTrue | molt_ir::OpKind::LoopBreakIfFalse => {
            emit_dispatch_loop_break_cond(func, context, plan, locals, op, idx, depth);
            true
        }
        molt_ir::OpKind::LoopBreakIfException => {
            emit_call(
                func,
                context.reloc_enabled,
                context.import_ids[crate::wasm_abi_generated::WasmRuntimeImport::ExceptionPending],
            );
            func.instruction(&Instruction::I64Const(0));
            func.instruction(&Instruction::I64Ne);
            emit_conditional_state_branch(
                func,
                context,
                plan,
                locals,
                idx,
                plan.successor(idx, EdgeRole::BranchTrue),
                plan.successor(idx, EdgeRole::BranchFalse),
                depth,
            );
            true
        }
        molt_ir::OpKind::BrIf => {
            let args = op.args.as_ref().unwrap();
            emit_branch_truthiness_i32(
                func,
                context.locals()[&args[0]],
                context.import_ids[crate::wasm_abi_generated::WasmRuntimeImport::IsTruthy],
                context.reloc_enabled,
            );
            func.instruction(&Instruction::If(BlockType::Empty));
            emit_static_dispatch_edge(
                func,
                context,
                plan,
                locals,
                idx,
                plan.successor(idx, EdgeRole::BranchTrue),
                depth,
                1,
            );
            func.instruction(&Instruction::End);
            false
        }
        kind if kind.is_exception_check() => {
            emit_dispatch_check_exception(func, context, plan, locals, op, idx, depth);
            true
        }
        _ => {
            context.emit_ops(
                func,
                std::slice::from_ref(op),
                &mut scratch.control,
                &mut scratch.try_stack,
                &mut scratch.label_stack,
                &mut scratch.label_depths,
                idx,
                depth.return_depth(0),
            );
            op.kind.is_return_terminator()
        }
    }
}
