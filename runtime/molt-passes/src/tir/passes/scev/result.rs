use std::collections::HashMap;

use crate::tir::analysis::{Analysis, AnalysisId};
use crate::tir::blocks::BlockId;
use crate::tir::function::TirFunction;
use crate::tir::numeric_facts::{ScevExpr, TripCount};
use crate::tir::values::ValueId;

use super::compute::compute_scev;

/// Per-function scalar-evolution facts.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ScevResult {
    /// SCEV expression for each value the analysis could classify. A value
    /// absent from the map is treated as `Unknown` by `scev_of`.
    pub(super) exprs: HashMap<ValueId, ScevExpr>,
    /// Trip count per loop header.
    pub(super) trip_counts: HashMap<BlockId, TripCount>,
    /// Loop headers, ascending — mirrors the loop forest used to build this.
    pub(super) headers: Vec<BlockId>,
}

impl ScevResult {
    /// The SCEV expression for `v` (`Unknown` if unclassified).
    pub fn scev_of(&self, v: ValueId) -> ScevExpr {
        self.exprs.get(&v).cloned().unwrap_or(ScevExpr::Unknown)
    }

    /// The trip count of the loop whose header is `header` (`Unknown` if none).
    pub fn trip_count(&self, header: BlockId) -> TripCount {
        self.trip_counts
            .get(&header)
            .cloned()
            .unwrap_or(TripCount::Unknown)
    }

    /// All loop headers (ascending), for iteration by consumers.
    pub fn headers(&self) -> &[BlockId] {
        &self.headers
    }

    /// True if `v` is the canonical induction variable of some loop (its SCEV
    /// is an `AddRec`).
    pub fn is_induction_var(&self, v: ValueId) -> bool {
        self.exprs.get(&v).map(|e| e.is_add_rec()).unwrap_or(false)
    }
}

// ---------------------------------------------------------------------------
// Analysis registration (S1 AnalysisManager)
// ---------------------------------------------------------------------------

/// Scalar-evolution analysis marker. Cached by the [`AnalysisManager`].
///
/// CFG-sensitive (loop structure and back-edges define the recurrences) and
/// ops-sensitive (the increment ops and constants feed the recurrence shape).
pub struct ScalarEvolution;

impl Analysis for ScalarEvolution {
    type Result = ScevResult;
    const ID: AnalysisId = AnalysisId::ScalarEvolution;
    const CFG_SENSITIVE: bool = true;
    const OPS_SENSITIVE: bool = true;
    fn compute(func: &TirFunction) -> Self::Result {
        compute_scev(func)
    }
}

// ---------------------------------------------------------------------------
// Computation
// ---------------------------------------------------------------------------
