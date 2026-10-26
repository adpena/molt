use crate::OpIR;
use crate::wasm::frame_locals::WasmDispatchFrameLocals;
use molt_tir::tir::op_kinds_generated::{
    simpleir_kind_is_wasm_dispatch_block_leader, simpleir_kind_is_wasm_dispatch_block_terminator,
};
use wasm_encoder::{BlockType, Function, Instruction};

pub(super) fn build_dispatch_blocks(ops: &[OpIR]) -> (Vec<usize>, Vec<usize>) {
    let op_count = ops.len();
    if op_count == 0 {
        return (Vec::new(), Vec::new());
    }

    let mut is_start = vec![false; op_count];
    is_start[0] = true;
    for (idx, op) in ops.iter().enumerate() {
        if simpleir_kind_is_wasm_dispatch_block_leader(op.kind.as_str()) {
            is_start[idx] = true;
        }
        if simpleir_kind_is_wasm_dispatch_block_terminator(op.kind.as_str()) && idx + 1 < op_count {
            is_start[idx + 1] = true;
        }
    }

    let mut starts = Vec::new();
    for (idx, start) in is_start.iter().enumerate() {
        if *start {
            starts.push(idx);
        }
    }

    let mut block_for_op = vec![0; op_count];
    let mut block_idx = 0usize;
    let mut next_start = starts.get(1).copied().unwrap_or(op_count);
    for (idx, block_slot) in block_for_op.iter_mut().enumerate().take(op_count) {
        if idx == next_start {
            block_idx += 1;
            next_start = starts.get(block_idx + 1).copied().unwrap_or(op_count);
        }
        *block_slot = block_idx;
    }

    (starts, block_for_op)
}

pub(super) fn build_dispatch_block_map(block_for_op: &[usize], targets: &[usize]) -> Vec<u8> {
    let mut ordinals = vec![u32::MAX; block_for_op.iter().max().map_or(0, |block| block + 1)];
    for (ordinal, &block) in targets.iter().enumerate() {
        ordinals[block] = ordinal as u32;
    }
    let mut bytes = Vec::with_capacity(block_for_op.len() * 4);
    for &block_idx in block_for_op {
        bytes.extend_from_slice(&ordinals[block_idx].to_le_bytes());
    }
    bytes
}

pub(super) fn emit_dispatch_block_lookup(
    func: &mut Function,
    op_count: usize,
    targets: &[usize],
    locals: WasmDispatchFrameLocals,
) {
    // An invalid operation or block index must trap, never become a loop
    // backedge. The innermost block is only the br_table failure destination;
    // it is closed before block bodies, so their label depths do not change.
    func.instruction(&Instruction::LocalGet(locals.state_local));
    func.instruction(&Instruction::I64Const(op_count as i64));
    func.instruction(&Instruction::I64GeU);
    func.instruction(&Instruction::If(BlockType::Empty));
    func.instruction(&Instruction::Unreachable);
    func.instruction(&Instruction::End);
    func.instruction(&Instruction::Block(BlockType::Empty));
    func.instruction(&Instruction::LocalGet(locals.block_map_base_local));
    func.instruction(&Instruction::I32WrapI64);
    func.instruction(&Instruction::LocalGet(locals.state_local));
    func.instruction(&Instruction::I32WrapI64);
    func.instruction(&Instruction::I32Const(4));
    func.instruction(&Instruction::I32Mul);
    func.instruction(&Instruction::I32Add);
    func.instruction(&Instruction::I32Load(wasm_encoder::MemArg {
        align: 2,
        offset: 0,
        memory_index: 0,
    }));
    let targets: Vec<u32> = targets.iter().map(|&block| block as u32 + 1).collect();
    func.instruction(&Instruction::BrTable(targets.into(), 0));
    func.instruction(&Instruction::End);
    func.instruction(&Instruction::Unreachable);
    func.instruction(&Instruction::End);
}
