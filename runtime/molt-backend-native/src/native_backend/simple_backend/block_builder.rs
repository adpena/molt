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
