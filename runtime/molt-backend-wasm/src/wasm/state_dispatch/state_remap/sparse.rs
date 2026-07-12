use super::br_table::try_emit_br_table_state_remap_lookup;
use wasm_encoder::{BlockType, Function, Instruction};

pub(in crate::wasm::state_dispatch) fn emit_sparse_state_remap_lookup(
    func: &mut Function,
    state_local: u32,
    sorted_entries: &[(i64, i64)],
) {
    if try_emit_br_table_state_remap_lookup(func, state_local, sorted_entries) {
        return;
    }

    emit_binary_search_node(func, state_local, sorted_entries);
}

fn emit_binary_search_node(func: &mut Function, state_local: u32, entries: &[(i64, i64)]) {
    if entries.is_empty() {
        return;
    }

    let mid = entries.len() / 2;
    let (state_id, target_idx) = entries[mid];
    let left = &entries[..mid];
    let right = &entries[mid + 1..];

    func.instruction(&Instruction::LocalGet(state_local));
    func.instruction(&Instruction::I64Const(state_id));
    func.instruction(&Instruction::I64Eq);
    func.instruction(&Instruction::If(BlockType::Empty));
    func.instruction(&Instruction::I64Const(target_idx));
    func.instruction(&Instruction::LocalSet(state_local));
    if !left.is_empty() || !right.is_empty() {
        func.instruction(&Instruction::Else);
        match (!left.is_empty(), !right.is_empty()) {
            (true, true) => {
                func.instruction(&Instruction::LocalGet(state_local));
                func.instruction(&Instruction::I64Const(state_id));
                func.instruction(&Instruction::I64LtS);
                func.instruction(&Instruction::If(BlockType::Empty));
                emit_binary_search_node(func, state_local, left);
                func.instruction(&Instruction::Else);
                emit_binary_search_node(func, state_local, right);
                func.instruction(&Instruction::End);
            }
            (true, false) => {
                func.instruction(&Instruction::LocalGet(state_local));
                func.instruction(&Instruction::I64Const(state_id));
                func.instruction(&Instruction::I64LtS);
                func.instruction(&Instruction::If(BlockType::Empty));
                emit_binary_search_node(func, state_local, left);
                func.instruction(&Instruction::End);
            }
            (false, true) => {
                func.instruction(&Instruction::LocalGet(state_local));
                func.instruction(&Instruction::I64Const(state_id));
                func.instruction(&Instruction::I64GtS);
                func.instruction(&Instruction::If(BlockType::Empty));
                emit_binary_search_node(func, state_local, right);
                func.instruction(&Instruction::End);
            }
            (false, false) => {}
        }
    }
    func.instruction(&Instruction::End);
}
