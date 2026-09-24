use std::collections::HashSet;

use crate::tir::blocks::BlockId;

use super::descriptor::CountedLoop;

/// The set of blocks that make up the loop region between `header` and the
/// back-edge, including all interposed normal-path blocks.
pub fn region_blocks(loop_info: &CountedLoop) -> HashSet<BlockId> {
    loop_info
        .guard_path
        .iter()
        .chain(&loop_info.body_path)
        .copied()
        .collect()
}
