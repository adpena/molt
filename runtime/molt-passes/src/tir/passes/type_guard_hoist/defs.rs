use std::collections::HashMap;

use crate::tir::blocks::BlockId;
use crate::tir::function::TirFunction;
use crate::tir::values::ValueId;

/// Build a map: ValueId -> BlockId that defines it (block args + op results).
///
/// This intentionally excludes function params, unlike the canonical
/// [`DefMap`](crate::tir::analysis::DefMap) analysis. A TypeGuard whose operand
/// is a param has no in-function defining block here, so it is left un-hoisted.
pub(super) fn build_def_map(func: &TirFunction) -> HashMap<ValueId, BlockId> {
    let mut def_map = HashMap::new();
    for (&bid, block) in &func.blocks {
        for arg in &block.args {
            def_map.insert(arg.id, bid);
        }
        for op in &block.ops {
            for &result in &op.results {
                def_map.insert(result, bid);
            }
        }
    }
    def_map
}
