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
    match op.kind.as_str() {
        "state_switch" => {
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
        "aiter" if mode == DispatchMode::Stateful => {
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
        "state_set" | "task_wait" | "is_pending" => {
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
        "state_transition" | "state_yield" => panic!(
            "{}: suspension must be normalized by shared terminal ownership before WASM emission",
            context.func_ir.name
        ),
        "if" => {
            emit_dispatch_if(func, context, plan, locals, op, idx, depth);
            true
        }
        "else" | "end_if" | "loop_start" | "loop_end" | "try_start" | "try_end" | "label"
        | "state_label" | "loop_break" | "loop_continue" | "jump" => {
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
        "loop_break_if_true" | "loop_break_if_false" => {
            emit_dispatch_loop_break_cond(func, context, plan, locals, op, idx, depth);
            true
        }
        "loop_break_if_exception" => {
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
        "br_if" => {
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
        kind if molt_tir::tir::op_kinds_generated::simpleir_kind_is_exception_check(kind) => {
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
            molt_tir::tir::op_kinds_generated::simpleir_kind_is_return_terminator(&op.kind)
        }
    }
}
