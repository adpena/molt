use self::ops::{DispatchOpScratch, emit_dispatch_op};
use super::super::op_loop::WasmFunctionEmitContext;
use super::DispatchMode;
use super::block_layout::emit_dispatch_block_lookup;
use super::common::{
    emit_dispatch_trailing_return, emit_set_state_and_br, emit_stateful_resume_prelude,
    exception_handler_region_indices_from_label_map,
};
use super::plan::{NonLinearDispatchLocals, NonLinearDispatchPlan};
use wasm_encoder::{BlockType, Function, Instruction};

mod ops;

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

    let mut scratch = DispatchOpScratch::default();

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
            block_terminated = emit_dispatch_op(
                func,
                op_emitter,
                plan,
                locals,
                mode,
                op,
                idx,
                depth,
                &exception_regions,
                &mut scratch,
            );
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
