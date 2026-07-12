use std::collections::HashSet;

use crate::tir::blocks::BlockId;

use super::descriptor::CountedLoop;

/// The set of blocks that make up the loop region between `header` and the
/// back-edge: `{header, cond_block, body}`. Used by a transform to decide which
/// blocks to retire when fully unrolling. Header -> cond is a single edge by
/// construction; there are no interposed guard blocks in a recognized loop.
pub fn region_blocks(loop_info: &CountedLoop) -> HashSet<BlockId> {
    let mut set = HashSet::new();
    set.insert(loop_info.header);
    set.insert(loop_info.cond_block);
    set.insert(loop_info.body);
    set
}
