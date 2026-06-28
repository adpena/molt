use super::lir_context::LirLowerCtx;
use super::lir_scalar::{emit_box_none, emit_lir_truthiness_i32, emit_return_boxed_i64};
use molt_tir::tir::blocks::BlockId;
use molt_tir::tir::lir::LirTerminator;
use molt_tir::tir::values::ValueId;
use wasm_encoder::Instruction;

#[derive(Clone, Copy)]
pub(super) enum LirReturnAbi {
    #[cfg(any(test, feature = "test-util"))]
    Native,
    BoxedI64,
}

pub(super) fn emit_lir_terminator(
    ctx: &mut LirLowerCtx,
    term: &LirTerminator,
    return_abi: LirReturnAbi,
) {
    match term {
        LirTerminator::Return { values } => {
            emit_lir_return(ctx, values, return_abi);
        }
        LirTerminator::Unreachable => ctx.instructions.push(Instruction::Unreachable),
        _ => ctx.instructions.push(Instruction::Unreachable),
    }
}

fn emit_lir_return(ctx: &mut LirLowerCtx, values: &[ValueId], return_abi: LirReturnAbi) {
    match return_abi {
        #[cfg(any(test, feature = "test-util"))]
        LirReturnAbi::Native => {
            if let Some(&val) = values.first() {
                ctx.emit_get(val);
            }
        }
        LirReturnAbi::BoxedI64 => {
            if let Some(&val) = values.first() {
                emit_return_boxed_i64(ctx, val);
            } else {
                emit_box_none(ctx);
            }
        }
    }
    ctx.instructions.push(Instruction::Return);
}

pub(super) fn emit_lir_terminator_multiblock(
    ctx: &mut LirLowerCtx,
    term: &LirTerminator,
    num_blocks: usize,
    return_abi: LirReturnAbi,
) {
    match term {
        LirTerminator::Return { values } => {
            emit_lir_return(ctx, values, return_abi);
        }
        LirTerminator::Unreachable => ctx.instructions.push(Instruction::Unreachable),
        LirTerminator::Branch { target, args } => {
            store_lir_block_args(ctx, *target, args);
            if let Some(&tgt_idx) = ctx.block_index.get(target) {
                let depth = (num_blocks - 1).saturating_sub(tgt_idx + 1);
                ctx.instructions.push(Instruction::Br(depth as u32));
            }
        }
        LirTerminator::CondBranch {
            cond,
            then_block,
            then_args,
            else_block,
            else_args,
        } => {
            emit_lir_truthiness_i32(ctx, *cond);
            store_lir_block_args(ctx, *then_block, then_args);
            if let Some(&tgt_idx) = ctx.block_index.get(then_block) {
                let depth = (num_blocks - 1).saturating_sub(tgt_idx + 1);
                ctx.instructions.push(Instruction::BrIf(depth as u32));
            }
            store_lir_block_args(ctx, *else_block, else_args);
            if let Some(&tgt_idx) = ctx.block_index.get(else_block) {
                let depth = (num_blocks - 1).saturating_sub(tgt_idx + 1);
                ctx.instructions.push(Instruction::Br(depth as u32));
            }
        }
        LirTerminator::Switch {
            value,
            cases,
            default,
            default_args,
        } => {
            for (case_val, target, args) in cases {
                ctx.emit_get(*value);
                ctx.instructions.push(Instruction::I64Const(*case_val));
                ctx.instructions.push(Instruction::I64Eq);
                store_lir_block_args(ctx, *target, args);
                if let Some(&tgt_idx) = ctx.block_index.get(target) {
                    let depth = (num_blocks - 1).saturating_sub(tgt_idx + 1);
                    ctx.instructions.push(Instruction::BrIf(depth as u32));
                }
            }
            store_lir_block_args(ctx, *default, default_args);
            if let Some(&tgt_idx) = ctx.block_index.get(default) {
                let depth = (num_blocks - 1).saturating_sub(tgt_idx + 1);
                ctx.instructions.push(Instruction::Br(depth as u32));
            }
        }
        LirTerminator::StateDispatch { .. } => {
            // `StateDispatch` only appears in generator/coroutine `_poll`
            // bodies, which on WASM are lowered by the SimpleIR relooper path
            // (`wasm.rs`), NOT this LIR fast path: `prepare_lir_wasm_fast_plan`
            // is gated to `____molt_globals_builtin__` functions only
            // (`is_production_lir_wasm_fast_path_name`).  Reaching here means a
            // state-machine body was incorrectly routed through the LIR fast
            // lane — fail loud rather than emit a dispatch that silently ignores
            // the saved frame state.
            panic!(
                "StateDispatch terminator reached the LIR→WASM fast lane in '{}'; \
                 generator/coroutine _poll bodies must lower via the SimpleIR relooper",
                ctx.func.name
            );
        }
    }
}

pub(super) fn store_lir_block_args(ctx: &mut LirLowerCtx, target: BlockId, args: &[ValueId]) {
    if let Some(block) = ctx.func.blocks.get(&target) {
        for (arg_val, &src_val) in block.args.iter().zip(args.iter()) {
            ctx.emit_get(src_val);
            let dst_local = ctx.get_local(arg_val.id);
            ctx.instructions.push(Instruction::LocalSet(dst_local));
        }
    }
}
