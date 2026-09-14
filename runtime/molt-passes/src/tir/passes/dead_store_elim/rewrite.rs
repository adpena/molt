use crate::tir::analysis::AnalysisManager;
use crate::tir::function::TirFunction;
use crate::tir::passes::PassStats;
use crate::tir::passes::typed_slot_access;

/// DSE consumes the same pristine-field facts as backend store lowering.
pub(super) fn run(func: &mut TirFunction, am: &mut AnalysisManager) -> PassStats {
    let plan = typed_slot_access::for_function(func, am);
    // Ordered site cursors preserve linear stable compaction. HashMap iteration
    // in the analysis cannot affect the emitted instruction order.
    for block in func.blocks.values_mut() {
        let mut dead = plan
            .dead_stores
            .range((block.id, 0)..=(block.id, usize::MAX))
            .map(|(_, index)| *index)
            .peekable();
        let mut index = 0;
        block.ops.retain(|_| {
            let remove = dead.peek() == Some(&index);
            if remove {
                dead.next();
            }
            index += 1;
            !remove
        });
    }
    PassStats {
        name: "dead_store_elim",
        ops_removed: plan.dead_stores.len(),
        ..Default::default()
    }
}
