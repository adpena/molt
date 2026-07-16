use crate::OpIR;
use crate::wasm::control_flow::{ControlKind, dispatch_control_panic};
use crate::wasm::op_loop::WasmFunctionEmitContext;
use crate::wasm::state_dispatch::DispatchMode;
use crate::wasm::state_dispatch::common::{
    emit_arena_free, emit_conditional_state_branch, emit_dispatch_check_exception,
    emit_dispatch_if, emit_dispatch_loop_break_cond, emit_set_state_and_br, label_target,
    loop_break_target, require_stateful,
};
use crate::wasm::state_dispatch::plan::{NonLinearDispatchLocals, NonLinearDispatchPlan};
use crate::wasm::state_dispatch::stateful_ops::{
    emit_chan_recv_yield, emit_chan_send_yield, emit_state_transition, emit_state_yield,
};
use crate::wasm_binary::emit_call;
use crate::wasm_values::emit_branch_truthiness_i32;
use std::collections::{BTreeMap, BTreeSet};
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
    op_emitter: &mut WasmFunctionEmitContext<'_, '_>,
    plan: &NonLinearDispatchPlan,
    locals: NonLinearDispatchLocals,
    mode: DispatchMode,
    op: &OpIR,
    idx: usize,
    depth: u32,
    exception_regions: &BTreeSet<usize>,
    scratch: &mut DispatchOpScratch,
) -> bool {
    let func_ir = op_emitter.func_ir;
    match op.kind.as_str() {
        "state_switch" => {
            require_stateful(mode, func_ir, idx, op);
            emit_set_state_and_br(func, locals.state_local, idx + 1, depth);
            true
        }
        "aiter" if mode == DispatchMode::Stateful => {
            let args = op.args.as_ref().unwrap();
            let iter = op_emitter.locals()[&args[0]];
            func.instruction(&Instruction::LocalGet(iter));
            emit_call(
                func,
                op_emitter.reloc_enabled,
                op_emitter.import_ids[crate::wasm_abi_generated::WasmRuntimeImport::Aiter],
            );
            func.instruction(&Instruction::LocalSet(
                op_emitter.locals()[op.out.as_ref().unwrap()],
            ));
            false
        }
        "state_transition" => {
            require_stateful(mode, func_ir, idx, op);
            emit_state_transition(func, op_emitter, plan, locals, op, idx, depth);
            true
        }
        "state_yield" => {
            require_stateful(mode, func_ir, idx, op);
            emit_state_yield(func, op_emitter, plan, locals, op, idx);
            true
        }
        "chan_send_yield" => {
            require_stateful(mode, func_ir, idx, op);
            emit_chan_send_yield(func, op_emitter, plan, locals, op, idx, depth);
            true
        }
        "chan_recv_yield" => {
            require_stateful(mode, func_ir, idx, op);
            emit_chan_recv_yield(func, op_emitter, plan, locals, op, idx, depth);
            true
        }
        "if" => {
            emit_dispatch_if(func, op_emitter, plan, locals, op, idx, depth);
            true
        }
        "else" => {
            let end_idx = plan
                .control_maps
                .end_for_else
                .get(&idx)
                .copied()
                .unwrap_or_else(|| {
                    dispatch_control_panic(&func_ir.name, idx, "else without end_if")
                });
            emit_set_state_and_br(func, locals.state_local, end_idx + 1, depth);
            true
        }
        "end_if" | "loop_start" | "loop_end" | "try_start" | "try_end" | "label"
        | "state_label" => {
            emit_set_state_and_br(func, locals.state_local, idx + 1, depth);
            true
        }
        "loop_index_start" => {
            let args = op.args.as_ref().unwrap();
            let start = op_emitter.locals()[&args[0]];
            let out = op_emitter.locals()[op.out.as_ref().unwrap()];
            func.instruction(&Instruction::LocalGet(start));
            func.instruction(&Instruction::LocalSet(out));
            emit_set_state_and_br(func, locals.state_local, idx + 1, depth);
            true
        }
        "loop_break_if_true" => {
            emit_dispatch_loop_break_cond(func, op_emitter, plan, locals, op, idx, depth, false);
            true
        }
        "loop_break_if_false" => {
            emit_dispatch_loop_break_cond(func, op_emitter, plan, locals, op, idx, depth, true);
            true
        }
        "loop_break_if_exception" => {
            let end_idx = loop_break_target(plan, func_ir, idx, "loop_break_if_exception");
            let end_block = end_idx + 1;
            let next_block = idx + 1;
            emit_call(
                func,
                op_emitter.reloc_enabled,
                op_emitter.import_ids
                    [crate::wasm_abi_generated::WasmRuntimeImport::ExceptionPending],
            );
            func.instruction(&Instruction::I64Const(0));
            func.instruction(&Instruction::I64Ne);
            emit_conditional_state_branch(
                func,
                locals.state_local,
                end_block,
                next_block,
                depth + 1,
            );
            true
        }
        "loop_break" => {
            let end_idx = loop_break_target(plan, func_ir, idx, "loop_break");
            emit_set_state_and_br(func, locals.state_local, end_idx + 1, depth);
            true
        }
        "loop_continue" => {
            let start_idx = plan
                .control_maps
                .loop_continue_target
                .get(&idx)
                .copied()
                .unwrap_or_else(|| {
                    dispatch_control_panic(&func_ir.name, idx, "loop_continue without loop")
                });
            emit_set_state_and_br(func, locals.state_local, start_idx + 1, depth);
            true
        }
        "jump" => {
            let target_label = op.value.unwrap_or_else(|| {
                dispatch_control_panic(&func_ir.name, idx, "jump missing label")
            });
            let target_idx = label_target(plan, func_ir, idx, target_label, "jump");
            emit_set_state_and_br(func, locals.state_local, target_idx, depth);
            true
        }
        "br_if" => {
            let args = op.args.as_ref().unwrap();
            let cond = op_emitter.locals()[&args[0]];
            let target_label = op.value.unwrap_or_else(|| {
                dispatch_control_panic(&func_ir.name, idx, "br_if missing label")
            });
            let target_idx = label_target(plan, func_ir, idx, target_label, "br_if");
            emit_branch_truthiness_i32(
                func,
                cond,
                op_emitter.import_ids[crate::wasm_abi_generated::WasmRuntimeImport::IsTruthy],
                op_emitter.reloc_enabled,
            );
            func.instruction(&Instruction::If(BlockType::Empty));
            emit_set_state_and_br(func, locals.state_local, target_idx, depth + 1);
            func.instruction(&Instruction::End);
            false
        }
        "check_exception" | "async_work_poll" => {
            emit_dispatch_check_exception(
                func,
                op_emitter,
                plan,
                locals,
                op,
                idx,
                depth,
                exception_regions,
            );
            true
        }
        "ret" => {
            let ret_local = op
                .var
                .as_ref()
                .and_then(|name| op_emitter.locals().get(name).copied());
            if let Some(local_idx) = ret_local {
                func.instruction(&Instruction::LocalGet(local_idx));
            } else {
                dispatch_control_panic(
                    &func_ir.name,
                    idx,
                    format_args!("ret target local {:?} is not present", op.var),
                );
            }
            emit_arena_free(func, op_emitter);
            func.instruction(&Instruction::Return);
            true
        }
        "ret_void" => {
            emit_arena_free(func, op_emitter);
            func.instruction(&Instruction::I64Const(0));
            func.instruction(&Instruction::Return);
            true
        }
        _ => {
            op_emitter.emit_ops(
                func,
                std::slice::from_ref(op),
                &mut scratch.control,
                &mut scratch.try_stack,
                &mut scratch.label_stack,
                &mut scratch.label_depths,
                idx,
            );
            false
        }
    }
}
