//! Loop Invariant Code Motion (LICM) for TIR.
//!
//! Hoists operations out of loop bodies when their operands are all
//! defined outside the loop (loop-invariant).  This eliminates redundant
//! recomputation of values that don't change across iterations.
//!
//! ```python
//! for i in range(n):
//!     x = a + b          # invariant: a, b don't change in the loop
//!     result += x * i
//! ```
//!
//! After LICM:
//! ```python
//! x = a + b              # hoisted to preheader
//! for i in range(n):
//!     result += x * i
//! ```
//!
//! Multi-level hoisting: nested loops are processed innermost-first. An
//! op invariant w.r.t. the inner loop is hoisted to the inner preheader
//! (which still lives inside the outer loop). When the outer loop is
//! processed, that op now sits in a block belonging to the outer loop's
//! body and may itself be invariant w.r.t. the outer loop, in which case
//! it is hoisted again - to the outer preheader. This is a fixpoint
//! traversal of the loop nesting tree.
//!
//! Safety conditions:
//! 1. The op must be pure (no side effects).
//! 2. All operands must be defined outside the loop (or be other invariants).
//! 3. The op must dominate all loop exits (guaranteed by hoisting to preheader).
//! 4. Exception-handling regions are conservatively excluded.
//! 5. The op's result must not appear as a branch argument (phi value).
//!    Such uses cross block boundaries via terminators; excluding them is
//!    sufficient to ensure the only escapes from a loop go through phi
//!    nodes, so direct uses of any hoist candidate are dominated by the
//!    chosen preheader.
//!
//! Reference: Muchnick, "Advanced Compiler Design and Implementation" ch. 13.

use super::PassStats;
use crate::tir::analysis::AnalysisManager;
use crate::tir::function::TirFunction;

mod hoist;
mod safety;
#[cfg(test)]
mod tests;

pub fn run(func: &mut TirFunction, am: &mut AnalysisManager) -> PassStats {
    hoist::run(func, am)
}
