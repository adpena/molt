use super::common::{
    emit_arena_free, emit_conditional_state_branch, emit_dispatch_block_lookup,
    emit_dispatch_check_exception, emit_dispatch_if, emit_dispatch_loop_break_cond,
    emit_dispatch_trailing_return, emit_set_state_and_br, emit_stateful_resume_prelude,
    exception_handler_region_indices_from_label_map, label_target, loop_break_target,
    require_stateful,
};
use super::stateful_ops::{
    emit_chan_recv_yield, emit_chan_send_yield, emit_state_transition, emit_state_yield,
};
use super::*;

pub(in crate::wasm) fn emit_stateful_dispatch(
    func: &mut Function,
    op_emitter: &mut WasmFunctionEmitContext<'_, '_>,
    plan: &NonLinearDispatchPlan,
    locals: NonLinearDispatchLocals,
) {
    emit_non_linear_dispatch(func, op_emitter, plan, locals, DispatchMode::Stateful);
}

pub(in crate::wasm) fn emit_jumpful_dispatch(
    func: &mut Function,
    op_emitter: &mut WasmFunctionEmitContext<'_, '_>,
    plan: &NonLinearDispatchPlan,
    locals: NonLinearDispatchLocals,
) {
    emit_non_linear_dispatch(func, op_emitter, plan, locals, DispatchMode::Jumpful);
}

fn emit_non_linear_dispatch(
    func: &mut Function,
    op_emitter: &mut WasmFunctionEmitContext<'_, '_>,
    plan: &NonLinearDispatchPlan,
    locals: NonLinearDispatchLocals,
    mode: DispatchMode,
) {
    let func_ir = op_emitter.func_ir;
    let op_count = func_ir.ops.len();
    let block_count = plan.block_starts.len();
    let dispatch_depths: Vec<u32> = (0..block_count)
        .map(|idx| (block_count - 1 - idx) as u32)
        .collect();
    let exception_regions = exception_handler_region_indices_from_label_map(
        &func_ir.ops,
        &plan.control_maps.label_to_index,
    );

    match mode {
        DispatchMode::Stateful => emit_stateful_resume_prelude(func, op_emitter, plan, locals),
        DispatchMode::Jumpful => {
            func.instruction(&Instruction::I64Const(0));
            func.instruction(&Instruction::LocalSet(locals.state_local));
        }
    }

    if mode == DispatchMode::Stateful {
        func.instruction(&Instruction::Block(BlockType::Empty));
    }
    func.instruction(&Instruction::Loop(BlockType::Empty));
    for _ in (0..block_count).rev() {
        func.instruction(&Instruction::Block(BlockType::Empty));
    }

    emit_dispatch_block_lookup(func, op_count, block_count, locals);

    let mut scratch_control: Vec<ControlKind> = Vec::new();
    let mut scratch_try: Vec<usize> = Vec::new();
    let mut label_stack: Vec<i64> = Vec::new();
    let mut label_depths: BTreeMap<i64, usize> = BTreeMap::new();

    for (block_idx, start) in plan.block_starts.iter().enumerate() {
        let end = plan
            .block_starts
            .get(block_idx + 1)
            .copied()
            .unwrap_or(op_count);
        let depth = dispatch_depths[block_idx];
        let mut block_terminated = false;

        for idx in *start..end {
            let op = &func_ir.ops[idx];
            match op.kind.as_str() {
                "state_switch" => {
                    require_stateful(mode, func_ir, idx, op);
                    emit_set_state_and_br(func, locals.state_local, idx + 1, depth);
                    block_terminated = true;
                }
                "aiter" if mode == DispatchMode::Stateful => {
                    let args = op.args.as_ref().unwrap();
                    let iter = op_emitter.locals[&args[0]];
                    func.instruction(&Instruction::LocalGet(iter));
                    emit_call(
                        func,
                        op_emitter.reloc_enabled,
                        op_emitter.import_ids["aiter"],
                    );
                    func.instruction(&Instruction::LocalSet(
                        op_emitter.locals[op.out.as_ref().unwrap()],
                    ));
                }
                "state_transition" => {
                    require_stateful(mode, func_ir, idx, op);
                    emit_state_transition(func, op_emitter, plan, locals, op, idx, depth);
                    block_terminated = true;
                }
                "state_yield" => {
                    require_stateful(mode, func_ir, idx, op);
                    emit_state_yield(func, op_emitter, plan, locals, op, idx);
                    block_terminated = true;
                }
                "chan_send_yield" => {
                    require_stateful(mode, func_ir, idx, op);
                    emit_chan_send_yield(func, op_emitter, plan, locals, op, idx, depth);
                    block_terminated = true;
                }
                "chan_recv_yield" => {
                    require_stateful(mode, func_ir, idx, op);
                    emit_chan_recv_yield(func, op_emitter, plan, locals, op, idx, depth);
                    block_terminated = true;
                }
                "if" => {
                    emit_dispatch_if(func, op_emitter, plan, locals, op, idx, depth);
                    block_terminated = true;
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
                    block_terminated = true;
                }
                "end_if" | "loop_start" | "loop_end" | "try_start" | "try_end" | "label"
                | "state_label" => {
                    emit_set_state_and_br(func, locals.state_local, idx + 1, depth);
                    block_terminated = true;
                }
                "loop_index_start" => {
                    let args = op.args.as_ref().unwrap();
                    let start = op_emitter.locals[&args[0]];
                    let out = op_emitter.locals[op.out.as_ref().unwrap()];
                    func.instruction(&Instruction::LocalGet(start));
                    func.instruction(&Instruction::LocalSet(out));
                    emit_set_state_and_br(func, locals.state_local, idx + 1, depth);
                    block_terminated = true;
                }
                "loop_break_if_true" => {
                    emit_dispatch_loop_break_cond(
                        func, op_emitter, plan, locals, op, idx, depth, false,
                    );
                    block_terminated = true;
                }
                "loop_break_if_false" => {
                    emit_dispatch_loop_break_cond(
                        func, op_emitter, plan, locals, op, idx, depth, true,
                    );
                    block_terminated = true;
                }
                "loop_break_if_exception" => {
                    let end_idx = loop_break_target(plan, func_ir, idx, "loop_break_if_exception");
                    let end_block = end_idx + 1;
                    let next_block = idx + 1;
                    emit_call(
                        func,
                        op_emitter.reloc_enabled,
                        op_emitter.import_ids["exception_pending"],
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
                    block_terminated = true;
                }
                "loop_break" => {
                    let end_idx = loop_break_target(plan, func_ir, idx, "loop_break");
                    emit_set_state_and_br(func, locals.state_local, end_idx + 1, depth);
                    block_terminated = true;
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
                    block_terminated = true;
                }
                "jump" => {
                    let target_label = op.value.unwrap_or_else(|| {
                        dispatch_control_panic(&func_ir.name, idx, "jump missing label")
                    });
                    let target_idx = label_target(plan, func_ir, idx, target_label, "jump");
                    emit_set_state_and_br(func, locals.state_local, target_idx, depth);
                    block_terminated = true;
                }
                "br_if" => {
                    let args = op.args.as_ref().unwrap();
                    let cond = op_emitter.locals[&args[0]];
                    let target_label = op.value.unwrap_or_else(|| {
                        dispatch_control_panic(&func_ir.name, idx, "br_if missing label")
                    });
                    let target_idx = label_target(plan, func_ir, idx, target_label, "br_if");
                    emit_branch_truthiness_i32(
                        func,
                        cond,
                        op_emitter.import_ids["is_truthy"],
                        op_emitter.reloc_enabled,
                    );
                    func.instruction(&Instruction::If(BlockType::Empty));
                    emit_set_state_and_br(func, locals.state_local, target_idx, depth + 1);
                    func.instruction(&Instruction::End);
                }
                "check_exception" => {
                    emit_dispatch_check_exception(
                        func,
                        op_emitter,
                        plan,
                        locals,
                        op,
                        idx,
                        depth,
                        &exception_regions,
                    );
                    block_terminated = true;
                }
                "ret" => {
                    let ret_local = op
                        .var
                        .as_ref()
                        .and_then(|name| op_emitter.locals.get(name).copied());
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
                    block_terminated = true;
                }
                "ret_void" => {
                    emit_arena_free(func, op_emitter);
                    func.instruction(&Instruction::I64Const(0));
                    func.instruction(&Instruction::Return);
                    block_terminated = true;
                }
                _ => {
                    op_emitter.emit_ops(
                        func,
                        std::slice::from_ref(op),
                        &mut scratch_control,
                        &mut scratch_try,
                        &mut label_stack,
                        &mut label_depths,
                        idx,
                    );
                }
            }
            if block_terminated {
                break;
            }
        }

        if !block_terminated {
            func.instruction(&Instruction::I64Const(end as i64));
            func.instruction(&Instruction::LocalSet(locals.state_local));
        }
        func.instruction(&Instruction::Br(depth));

        if block_idx + 1 < block_count {
            func.instruction(&Instruction::End);
        }
    }

    emit_dispatch_trailing_return(func, op_emitter, locals, mode);
}
