use super::super::lir_context::{LirLowerCtx, lir_terminator_successors};
use super::super::lir_control::{
    LirReturnAbi, emit_lir_terminator, emit_lir_terminator_multiblock,
};
use super::super::lir_ops::emit_lir_block_ops;
use molt_tir::tir::blocks::BlockId;
use std::collections::HashMap;
use wasm_encoder::{BlockType, Instruction};

pub(super) fn emit_lir_function_body(ctx: &mut LirLowerCtx, return_abi: LirReturnAbi) {
    let rpo = ctx.rpo.clone();
    let num_blocks = rpo.len();
    if num_blocks <= 1 {
        if let Some(block) = ctx.func.blocks.get(&ctx.func.entry_block) {
            emit_lir_block_ops(ctx, block);
            emit_lir_terminator(ctx, &block.terminator, return_abi);
        }
        return;
    }

    let back_edge_targets = compute_back_edge_targets(ctx, &rpo);
    for (i, &bid) in rpo.iter().enumerate() {
        if i < num_blocks - 1 {
            if back_edge_targets.contains_key(&bid) {
                ctx.instructions.push(Instruction::Loop(BlockType::Empty));
            } else {
                ctx.instructions.push(Instruction::Block(BlockType::Empty));
            }
        }
    }

    for (i, &bid) in rpo.iter().enumerate() {
        if let Some(block) = ctx.func.blocks.get(&bid) {
            emit_lir_block_ops(ctx, block);
            emit_lir_terminator_multiblock(ctx, &block.terminator, num_blocks, return_abi);
        }
        if i < num_blocks - 1 {
            ctx.instructions.push(Instruction::End);
        }
    }
}

fn compute_back_edge_targets(ctx: &LirLowerCtx, rpo: &[BlockId]) -> HashMap<BlockId, bool> {
    let mut targets = HashMap::new();
    for (src_idx, &bid) in rpo.iter().enumerate() {
        if let Some(block) = ctx.func.blocks.get(&bid) {
            for succ in lir_terminator_successors(&block.terminator) {
                if let Some(&tgt_idx) = ctx.block_index.get(&succ)
                    && tgt_idx <= src_idx
                {
                    targets.insert(succ, true);
                }
            }
        }
    }
    targets
}
