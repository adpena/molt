//! MemorySSA — Tier-0 substrate **S5, Phase 2a** (standalone analysis).
//!
//! MemorySSA assigns a **memory version** to every program point where memory is
//! defined or consumed. It is the single-source-of-truth answer to the question
//! that unblocks MemGVN (load dedup / store-to-load forwarding), cross-block dead
//! store elimination, SROA (object-field promotion) and LICM-of-loads:
//!
//! > *Which store produced the value this load reads?*
//!
//! ## Three node kinds
//!
//! ```text
//! MemoryDef(n)  — an op that writes/clobbers memory; produces a new version n,
//!                 consuming the version it flows through (`def_ver`).
//! MemoryUse(n)  — a load that reads memory version n (its reaching def).
//! MemoryPhi     — at a join point, selects a memory version per predecessor edge.
//! ```
//!
//! Version `0` is [`LIVE_ON_ENTRY`]: all externally-visible memory that exists
//! before the function's first op.
//!
//! ## Built ON the alias oracle (S5 phase 1), never duplicating it
//!
//! This module classifies each op into Def / Use / neither using **only** the
//! public queries of [`AliasAnalysisResult`]:
//!
//! * [`AliasAnalysisResult::region_of`] — the op's [`MemRegion`].
//! * [`AliasAnalysisResult::load_purity`] — whether a load is a proven-pure
//!   typed-slot read or `MayDispatch` (opaque, may run a user dunder).
//! * [`MemRegion::may_alias`] — the TBAA-style disambiguation that lets a store
//!   to offset 8 *not* kill a load from offset 0.
//!
//! The classification rule (Phase A) is, by construction, a **conservative
//! superset** of "writes memory": every op that touches a non-scalar region and
//! is *not* a proven-pure read is treated as a clobbering [`MemAccess::Def`]. A
//! `MayDispatch` load (`get_attr`, `Index`, …) is a Def against `GenericHeap`
//! because it may dispatch `__getattr__` / `__getitem__` that writes any field.
//! No heap-barrier op-list is re-maintained here — the alias oracle's
//! `region_of` already widens every barrier (`Call`, `Raise`, `Yield`, …) to
//! `GenericHeap`, which `may_alias`-es every heap region.
//!
//! ## Soundness model: FAIL-CLOSED
//!
//! Region-based reaching-def is conservative because [`MemRegion::may_alias`] is
//! conservative (it returns `true` when in doubt) and every `GenericHeap` def
//! may-aliases everything. A *missed* clobber would let a consumer forward a
//! stale value (a miscompile); the analysis never misses one because:
//!
//! 1. Every op that is not a proven-pure read becomes a Def against the (already
//!    conservatively-widened) region the alias oracle assigns it.
//! 2. A use's reaching def is the most-recent dominating Def/Phi whose region
//!    *may-alias* the use's region — so a `GenericHeap` def between a store and a
//!    load always intercepts the load (a call clobbers a typed field).
//! 3. Phi placement uses the standard iterated-dominance-frontier algorithm
//!    (Cooper/Harvey/Kennedy), which over-places, never under-places, phis.
//! 4. The renaming walk is a standard dominator-tree walk that never binds a use
//!    to a def that does not dominate it. The fail-closed case is "no reaching
//!    def found" ([`LIVE_ON_ENTRY`] is always the floor), which only ever
//!    *prevents* an optimization.
//!
//! Any imprecision errs toward **more** dependencies (more clobbers, coarser
//! versions), never fewer — RC/UAF-critical per the integrated program's Risk 2.
//!
//! ## CFG view
//!
//! Phi placement and renaming traverse the **full** CFG (terminator + implicit
//! exception edges), exactly the view the S1 [`AnalysisManager`] dominator/pred
//! analyses use ([`CfgEdgePolicy::Full`]). A handler block reached only via an
//! exception edge therefore receives a sound memory phi: a store in a protected
//! region must be assumed visible (or clobbered) along the exception edge.
//!
//! ## What this arc (S5-2a) delivers
//!
//! The value types ([`MemVersion`], [`MemAccess`], [`MemorySsaResult`]),
//! [`compute_standalone`], and the [`MemorySSA`] marker registering the analysis
//! with the S1 [`AnalysisManager`] (`am.get::<MemorySSA>(func)`) — a STANDALONE
//! analysis with no pipeline consumers and **zero behavior change**. The first
//! consumer is MemGVN (S5-2b).
//!
//! [`AnalysisManager`]: crate::tir::analysis::AnalysisManager
//! [`CfgEdgePolicy::Full`]: crate::tir::dominators::CfgEdgePolicy

mod access;
mod builder;
mod cfg;
mod classify;

#[cfg(test)]
mod tests;

pub use access::{LIVE_ON_ENTRY, MemAccess, MemVersion, MemorySsaResult};
pub use builder::compute_standalone;
pub use classify::typed_slot_store_value;

use crate::tir::analysis::{Analysis, AnalysisId};
use crate::tir::function::TirFunction;

use super::alias_analysis::AliasAnalysis;

/// Zero-sized marker registering MemorySSA with the S1
/// [`AnalysisManager`](crate::tir::analysis::AnalysisManager)
/// (`am.get::<MemorySSA>(func)`).
pub struct MemorySSA;

impl Analysis for MemorySSA {
    type Result = MemorySsaResult;
    const ID: AnalysisId = AnalysisId::MemorySSA;
    // CFG-sensitive (phi placement/renaming walk the full CFG) AND
    // ops-sensitive (Def/Use classification reads every op): invalidated by the
    // same mutation classes as its [`AliasAnalysis`] substrate.
    const CFG_SENSITIVE: bool = true;
    const OPS_SENSITIVE: bool = true;
    fn compute(func: &TirFunction) -> Self::Result {
        // Derive the alias substrate through its own `Analysis` interface —
        // the same inline-dependency pattern `ValueRange::compute` uses for
        // SCEV (`Analysis::compute` only receives the function, so a dependent
        // analysis recomputes its input; the manager memoizes *this* result).
        let alias = AliasAnalysis::compute(func);
        compute_standalone(func, &alias)
    }
}
