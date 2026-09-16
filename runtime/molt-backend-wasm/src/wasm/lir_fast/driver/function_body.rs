mod control_plan;

use super::super::lir_context::LirLowerCtx;
use super::super::lir_control::{LirControlLabel, LirReturnAbi, emit_lir_terminator};
use super::super::lir_ops::emit_lir_block_ops;
use control_plan::{ControlPlan, Scope};
use wasm_encoder::{BlockType, Instruction};

pub(super) fn emit_lir_function_body(ctx: &mut LirLowerCtx, return_abi: LirReturnAbi) {
    let plan = ControlPlan::new(&ctx.cfg, &ctx.rpo);
    let mut scopes: Vec<Scope> = Vec::new();
    let mut labels = Vec::new();
    let mut next_scope = 0;
    for position in 0..=plan.order.len() {
        while scopes.last().is_some_and(|scope| scope.end == position) {
            scopes.pop();
            labels.pop();
            ctx.instructions.push(Instruction::End);
        }
        while plan
            .scopes
            .get(next_scope)
            .is_some_and(|scope| scope.start == position)
        {
            let scope = plan.scopes[next_scope];
            if let Some(parent) = scopes.last() {
                assert!(scope.end <= parent.end, "crossing WASM control scopes");
            }
            ctx.instructions.push(match scope.label {
                LirControlLabel::LoopContinue(_) => Instruction::Loop(BlockType::Empty),
                LirControlLabel::ForwardExit(_) => Instruction::Block(BlockType::Empty),
                LirControlLabel::Selection => unreachable!(),
            });
            scopes.push(scope);
            labels.push(scope.label);
            next_scope += 1;
        }
        if let Some(id) = plan.order.get(position) {
            let block = &ctx.func.blocks[id];
            emit_lir_block_ops(ctx, block);
            emit_lir_terminator(ctx, &block.terminator, return_abi, &mut labels);
        }
    }
    assert!(scopes.is_empty() && labels.is_empty());
    // Every CFG terminator transfers control. Even a nonreturning single-block
    // loop must type-check for a value-returning function without fake results.
    ctx.instructions.push(Instruction::Unreachable);
}
