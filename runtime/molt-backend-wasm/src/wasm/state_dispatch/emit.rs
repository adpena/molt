use self::ops::{DispatchOpScratch, emit_dispatch_op};
use super::super::frame_locals::WasmDispatchFrameLocals;
use super::super::op_loop::WasmFunctionEmitContext;
use super::DispatchMode;
use super::block_layout::emit_dispatch_block_lookup;
use super::common::{
    emit_dispatch_trailing_return, emit_stateful_resume_prelude, emit_static_dispatch_edge,
};
use super::plan::NonLinearControlPlan;
use wasm_encoder::{BlockType, Function, Instruction};

mod ops;

pub(in crate::wasm) fn emit_non_linear_control(
    func: &mut Function,
    op_emitter: &mut WasmFunctionEmitContext<'_, '_>,
    plan: &NonLinearControlPlan,
    locals: Option<WasmDispatchFrameLocals>,
) {
    let op_count = op_emitter.func_ir.ops.len();
    let block_count = plan.block_starts.len();
    let mode = if plan.state_resume.is_some() {
        DispatchMode::Stateful
    } else {
        DispatchMode::Jumpful
    };
    match mode {
        DispatchMode::Stateful => emit_stateful_resume_prelude(
            func,
            op_emitter,
            plan,
            locals.expect("stateful control requires dispatch locals"),
        ),
        DispatchMode::Jumpful => {
            if let Some(locals) = locals {
                func.instruction(&Instruction::I64Const(0));
                func.instruction(&Instruction::LocalSet(locals.state_local));
            }
        }
    }
    if plan.needs_dispatch() {
        func.instruction(&Instruction::Loop(BlockType::Empty));
    }
    for _ in (0..block_count).rev() {
        func.instruction(&Instruction::Block(BlockType::Empty));
    }
    if plan.needs_dispatch() {
        emit_dispatch_block_lookup(
            func,
            op_count,
            &plan.dispatch_targets,
            locals.expect("dynamic control requires dispatch locals"),
        );
    } else {
        // The innermost forward scope ends immediately at operation zero.
        // There is no selector, lookup table, dispatch loop, or state local.
        func.instruction(&Instruction::End);
    }
    let mut scratch = DispatchOpScratch::default();
    for (block_idx, start) in plan.block_starts.iter().enumerate() {
        let end = plan
            .block_starts
            .get(block_idx + 1)
            .copied()
            .unwrap_or(op_count);
        let depth = plan.scope_depth(block_idx);
        let mut block_terminated = false;
        for idx in *start..end {
            if !plan.operation_reachable(idx) {
                // Canonical entry reachability excludes dead ordinary Jumpful
                // edges. Stateful's admitted external state roots are broader.
                func.instruction(&Instruction::Unreachable);
                block_terminated = true;
                break;
            }
            let op = &op_emitter.func_ir.ops[idx];
            block_terminated = emit_dispatch_op(
                func,
                op_emitter,
                plan,
                locals,
                mode,
                op,
                idx,
                depth,
                &mut scratch,
            );
            if block_terminated {
                break;
            }
        }
        if !block_terminated {
            let source = end - 1;
            if let Some(target) = plan.fallthrough_successor(source) {
                emit_static_dispatch_edge(func, op_emitter, plan, locals, source, target, depth, 0);
            } else {
                func.instruction(&Instruction::Unreachable);
            }
        }
        if block_idx + 1 < block_count {
            func.instruction(&Instruction::End);
        }
    }
    emit_dispatch_trailing_return(func, op_emitter, plan.needs_dispatch());
}
