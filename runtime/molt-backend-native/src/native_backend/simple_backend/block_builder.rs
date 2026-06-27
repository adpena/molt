use super::*;

#[cfg(feature = "native-backend")]
pub(crate) fn find_zero_pred_blocks(func: &Function) -> Vec<Block> {
    let mut preds: BTreeMap<Block, usize> = BTreeMap::new();
    for block in func.layout.blocks() {
        preds.entry(block).or_insert(0);
    }
    for block in func.layout.blocks() {
        for inst in func.layout.block_insts(block) {
            for dest in func.dfg.insts[inst]
                .branch_destination(&func.dfg.jump_tables, &func.dfg.exception_tables)
            {
                let dest_block = dest.block(&func.dfg.value_lists);
                *preds.entry(dest_block).or_insert(0) += 1;
            }
        }
    }
    let entry = func.layout.entry_block();
    preds
        .into_iter()
        .filter(|(block, count)| Some(*block) != entry && *count == 0)
        .map(|(block, _)| block)
        .collect()
}

#[cfg(feature = "native-backend")]
pub(crate) fn ensure_block_in_layout(builder: &mut FunctionBuilder, block: Block) {
    if builder.func.layout.is_block_inserted(block) {
        return;
    }
    if let Some(current) = builder.current_block()
        && builder.func.layout.is_block_inserted(current)
    {
        builder.insert_block_after(block, current);
        return;
    }
    builder.func.layout.append_block(block);
}

#[cfg(feature = "native-backend")]
pub(crate) fn block_has_terminator(builder: &FunctionBuilder, block: Block) -> bool {
    builder
        .func
        .layout
        .last_inst(block)
        .map(|inst| builder.func.dfg.insts[inst].opcode().is_terminator())
        .unwrap_or(false)
}

#[cfg(feature = "native-backend")]
#[allow(dead_code)]
pub(crate) fn sync_block_filled(builder: &FunctionBuilder, is_block_filled: &mut bool) {
    if let Some(block) = builder.current_block() {
        if block_has_terminator(builder, block) {
            *is_block_filled = true;
        } else {
            // The current block is open (no terminator) - clear the flag so
            // subsequent ops are not incorrectly skipped.  This fixes cases
            // where a control-flow op (e.g. check_exception) switched to a
            // fresh fallthrough block and cleared the flag via
            // switch_to_block_tracking, but a stale `true` value from a
            // previous iteration leaked through.
            *is_block_filled = false;
        }
    }
}

#[cfg(feature = "native-backend")]
pub(crate) fn switch_to_block_tracking(
    builder: &mut FunctionBuilder,
    block: Block,
    is_block_filled: &mut bool,
) {
    // Guard: if the block already has a terminator instruction, Cranelift's
    // `switch_to_block` will panic with "you cannot switch to a block which
    // is already filled".  This happens in complex control flow (e.g. stdlib
    // modules with nested try/except + if/else) where multiple paths converge
    // on the same block and a previous path already sealed it with a branch.
    // In that case we must NOT switch to it - just mark as filled so
    // subsequent ops create a fresh block or skip dead code.
    if block_has_terminator(builder, block) {
        *is_block_filled = true;
        return;
    }
    ensure_block_in_layout(builder, block);
    builder.switch_to_block(block);
    *is_block_filled = false;
}

#[cfg(feature = "native-backend")]
pub(crate) fn resolve_cleanup_value(
    builder: &mut FunctionBuilder,
    vars: &BTreeMap<String, Variable>,
    entry_vars: &BTreeMap<String, Value>,
    name: &str,
) -> Option<Value> {
    entry_vars
        .get(name)
        .copied()
        .or_else(|| var_get(builder, vars, name).map(|v| *v))
}
