use std::collections::{HashMap, HashSet};

use crate::tir::analysis::{Analysis, AnalysisId};
use crate::tir::blocks::{BlockId, TirBlock};
use crate::tir::function::TirFunction;
use crate::tir::values::ValueId;

/// Per-function liveness result: the heap-carrying live-in / live-out value sets
/// for every block, plus the raw-scalar exclusion set the drop pass reuses.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct TirLivenessResult {
    /// block → set of heap-carrying values live on entry.
    pub live_in: HashMap<BlockId, HashSet<ValueId>>,
    /// block → set of heap-carrying values live on exit (union of successors'
    /// live-in, minus successor block args supplied by this block's edges).
    pub live_out: HashMap<BlockId, HashSet<ValueId>>,
    /// Values whose physical carrier holds no refcounted heap obligation
    /// (RawI64Safe / Bool / FloatUnboxed / None / Never). Excluded from
    /// `live_in`/`live_out`; exposed so the
    /// drop pass can apply the identical filter to last-use candidates without
    /// recomputing the value-range proof.
    pub raw_scalars: HashSet<ValueId>,
}

impl TirLivenessResult {
    /// True iff `val` (a heap-carrying value) is live on entry to `block`.
    pub fn is_live_in(&self, block: BlockId, val: ValueId) -> bool {
        self.live_in
            .get(&block)
            .is_some_and(|set| set.contains(&val))
    }

    /// True iff `val` (a heap-carrying value) is live on exit from `block`.
    pub fn is_live_out(&self, block: BlockId, val: ValueId) -> bool {
        self.live_out
            .get(&block)
            .is_some_and(|set| set.contains(&val))
    }

    /// True iff `val`'s carrier holds no refcounted heap obligation. Such values
    /// are never dropped.
    pub fn is_raw_scalar(&self, val: ValueId) -> bool {
        self.raw_scalars.contains(&val)
    }

    /// The index of the LAST op in `block` that uses `val`, or `None` if `val`
    /// is not used by any op in the block. Terminator uses are NOT included
    /// (callers that must account for the terminator check `live_out` and the
    /// terminator args separately). A raw-scalar value always returns `None`
    /// from the live-set queries but `last_use_in_block` still reports its true
    /// last op-use position (the position query is repr-agnostic — the caller
    /// applies the repr filter before acting on it).
    pub fn last_use_in_block(&self, block: &TirBlock, val: ValueId) -> Option<usize> {
        let mut last = None;
        for (idx, op) in block.ops.iter().enumerate() {
            if op.operands.contains(&val) {
                last = Some(idx);
            }
        }
        last
    }
}

/// Liveness analysis marker, cached by the [`AnalysisManager`].
///
/// [`AnalysisManager`]: crate::tir::analysis::AnalysisManager
pub struct TirLiveness;

impl Analysis for TirLiveness {
    type Result = TirLivenessResult;
    const ID: AnalysisId = AnalysisId::Liveness;
    // Liveness depends on the CFG edges (successor relation) and on the ops
    // within blocks (use/def positions), so it is invalidated by both.
    const CFG_SENSITIVE: bool = true;
    const OPS_SENSITIVE: bool = true;
    fn compute(func: &TirFunction) -> Self::Result {
        super::solver::compute_liveness(func)
    }
}
