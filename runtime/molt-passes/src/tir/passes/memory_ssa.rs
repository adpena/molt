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

use std::collections::{HashMap, HashSet};

use crate::tir::analysis::{Analysis, AnalysisId};
use crate::tir::blocks::BlockId;
use crate::tir::dominators::{self, CfgEdgePolicy};
use crate::tir::function::TirFunction;
use crate::tir::ops::{AttrValue, OpCode, TirOp};
use crate::tir::values::ValueId;

use super::alias_analysis::{AliasAnalysis, AliasAnalysisResult, LoadPurity, MemRegion};

// ===========================================================================
// MemVersion
// ===========================================================================

/// A memory access ordinal — unique per function, allocated sequentially.
/// Version `0` is the [`LIVE_ON_ENTRY`] def (all externally-visible memory
/// before the function's first op).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct MemVersion(pub u32);

/// The synthetic def representing all memory live before the function's first
/// op. Every reaching-def query bottoms out here when no in-function Def or Phi
/// dominates the use — the fail-closed floor.
pub const LIVE_ON_ENTRY: MemVersion = MemVersion(0);

// ===========================================================================
// MemAccess
// ===========================================================================

/// A node in the MemorySSA graph.
#[derive(Debug, Clone, PartialEq)]
pub enum MemAccess {
    /// A defining write/clobber. The op at `(block, op_idx)` defines memory
    /// version `ver`, consuming the preceding definition `def_ver` (the memory
    /// it "flows through" — the immediate dominating Def/Phi whose region it
    /// may-clobbers, needed for cross-block kill queries).
    Def {
        ver: MemVersion,
        /// The version this def flows through (its immediate memory dominator).
        def_ver: MemVersion,
        block: BlockId,
        op_idx: usize,
        /// The region this def clobbers, from [`AliasAnalysisResult::region_of`].
        region: MemRegion,
    },
    /// A memory use (a proven-pure load). The op at `(block, op_idx)` reads
    /// version `def_ver` — the most-recent dominating Def/Phi whose region may
    /// alias the load's region.
    Use {
        def_ver: MemVersion,
        block: BlockId,
        op_idx: usize,
        region: MemRegion,
    },
    /// A phi placed at a join point where multiple memory versions meet.
    Phi {
        ver: MemVersion,
        block: BlockId,
        /// `(predecessor BlockId, incoming MemVersion)` pairs.
        incoming: Vec<(BlockId, MemVersion)>,
    },
}

impl MemAccess {
    /// The version this access defines, if it is a Def or Phi (Uses define no
    /// new version).
    #[inline]
    pub fn defined_version(&self) -> Option<MemVersion> {
        match self {
            MemAccess::Def { ver, .. } | MemAccess::Phi { ver, .. } => Some(*ver),
            MemAccess::Use { .. } => None,
        }
    }

    /// The block this access lives in.
    #[inline]
    pub fn block(&self) -> BlockId {
        match self {
            MemAccess::Def { block, .. }
            | MemAccess::Use { block, .. }
            | MemAccess::Phi { block, .. } => *block,
        }
    }
}

// ===========================================================================
// MemorySsaResult
// ===========================================================================

/// The complete MemorySSA result for one function.
// `PartialEq` is required by the `MOLT_VERIFY_ANALYSIS` staleness self-check
// (pass_manager's cached-vs-fresh recompute comparison).
#[derive(Debug, Clone, Default, PartialEq)]
pub struct MemorySsaResult {
    /// Every Def and Phi, keyed by the version it defines. (Uses define no
    /// version and are recorded only in `block_op_to_use_def`.)
    pub defs: HashMap<MemVersion, MemAccess>,
    /// `(block, op_idx)` → the [`MemVersion`] this op defines (Defs only).
    pub block_op_to_def: HashMap<(BlockId, usize), MemVersion>,
    /// `(block, op_idx)` → the [`MemVersion`] this op *reads* (Uses only).
    pub block_op_to_use_def: HashMap<(BlockId, usize), MemVersion>,
    /// Every memory use ([`MemAccess::Use`]), keyed by its `(block, op_idx)`
    /// position, carrying its region and reaching def. Consumers (MemGVN /
    /// LICM-of-loads) iterate this to find forwardable / hoistable loads.
    pub uses: HashMap<(BlockId, usize), MemAccess>,
    /// Per-block memory phi (the version the phi defines), when one was placed.
    pub block_phis: HashMap<BlockId, MemVersion>,
    /// The reaching def at the END of each block (the version that exits it).
    pub exit_def: HashMap<BlockId, MemVersion>,
    /// Next fresh version counter (the count of versions allocated, including
    /// [`LIVE_ON_ENTRY`]). Consumers that splice in new Defs allocate from here.
    pub next_version: u32,
}

impl MemorySsaResult {
    /// The memory version reaching a USE at `(block, op_idx)`: the most-recent
    /// def dominating that use whose region may-alias the load's region.
    /// `None` if the op at that position is not a tracked memory use.
    #[inline]
    pub fn reaching_def_for_use(&self, block: BlockId, op_idx: usize) -> Option<MemVersion> {
        self.block_op_to_use_def.get(&(block, op_idx)).copied()
    }

    /// The version a DEF at `(block, op_idx)` produces, if any.
    #[inline]
    pub fn def_at(&self, block: BlockId, op_idx: usize) -> Option<MemVersion> {
        self.block_op_to_def.get(&(block, op_idx)).copied()
    }

    /// For a [`MemAccess::Def`], the single Def/Phi it flows through (the memory
    /// it observes in the clobber graph). A Phi flows from itself (it *is* the
    /// merged version); a Use defines nothing and returns `None`.
    pub fn def_version_of(&self, ver: MemVersion) -> Option<MemVersion> {
        match self.defs.get(&ver)? {
            MemAccess::Def { def_ver, .. } => Some(*def_ver),
            MemAccess::Phi { ver, .. } => Some(*ver),
            MemAccess::Use { .. } => None,
        }
    }

    /// The [`MemAccess`] that defines `ver` (a Def or a Phi), if recorded.
    #[inline]
    pub fn access(&self, ver: MemVersion) -> Option<&MemAccess> {
        self.defs.get(&ver)
    }

    /// True if `store_ver` is exactly the reaching def of the load at
    /// `(load_block, load_op_idx)` — the single direct memory dependency used
    /// for store-to-load forwarding. False when the load's reaching def is a
    /// phi, a different store, or an intervening clobber.
    #[inline]
    pub fn is_direct_def_of_use(
        &self,
        store_ver: MemVersion,
        load_block: BlockId,
        load_op_idx: usize,
    ) -> bool {
        self.block_op_to_use_def
            .get(&(load_block, load_op_idx))
            .copied()
            == Some(store_ver)
    }
}

// ===========================================================================
// Op classification — derived ENTIRELY from the alias oracle (no duplication)
// ===========================================================================

/// How an op participates in MemorySSA, decided purely from the public alias
/// oracle queries.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum MemRole {
    /// A clobbering write (`StoreAttr`, `StoreIndex`, a call/raise/yield
    /// barrier, a module mutation, or a `MayDispatch` load that may run a user
    /// dunder writing arbitrary memory). Produces a new memory version.
    Def,
    /// A proven-pure typed-slot load. Reads a memory version; defines none.
    Use,
    /// Touches no heap memory (a pure register computation, constant, control
    /// marker). Not part of the memory graph.
    None,
}

/// Classify an op's memory role using ONLY [`AliasAnalysisResult`]'s public
/// surface — the single source of truth for memory-aliasing facts.
///
/// * `ScalarRegister` region ⇒ [`MemRole::None`] (no heap footprint).
/// * A proven-pure load (`load_purity == ProvenPure`, i.e. a typed-slot
///   `LoadAttr`) ⇒ [`MemRole::Use`].
/// * Everything else touching non-scalar memory ⇒ [`MemRole::Def`] (a
///   conservative clobber). This subsumes every heap-barrier opcode — the alias
///   oracle already widens calls/raises/yields/module-mutations to `GenericHeap`
///   — and every `MayDispatch` load, which may dispatch a writing dunder.
fn classify(op: &TirOp, alias: &AliasAnalysisResult) -> MemRole {
    let region = alias.region_of(op);
    if region == MemRegion::ScalarRegister {
        return MemRole::None;
    }
    // A load (`LoadAttr` / `Index`) that is proven-pure reads but never writes.
    // Every other non-scalar op is a clobbering def. `load_purity` only returns
    // `ProvenPure` for typed-slot `LoadAttr` ops, so this gate is exactly the
    // "pure read" set.
    if matches!(op.opcode, OpCode::LoadAttr | OpCode::Index)
        && alias.load_purity(op) == LoadPurity::ProvenPure
    {
        MemRole::Use
    } else {
        MemRole::Def
    }
}

/// `Some((target, stored_value, offset))` for the narrow typed-slot store
/// (`store` / `store_init`) — used to expose the forwarded value to consumers.
/// Operands are `[target, stored_value]`; the offset is the `value` attr.
pub fn typed_slot_store_value(op: &TirOp) -> Option<(ValueId, ValueId, i64)> {
    if op.opcode != OpCode::StoreAttr || op.operands.len() != 2 {
        return None;
    }
    let kind = match op.attrs.get("_original_kind") {
        Some(AttrValue::Str(s)) => s.as_str(),
        _ => return None,
    };
    if !matches!(kind, "store" | "store_init") {
        return None;
    }
    match op.attrs.get("value") {
        Some(AttrValue::Int(offset)) => Some((op.operands[0], op.operands[1], *offset)),
        _ => None,
    }
}

// ===========================================================================
// compute_standalone
// ===========================================================================

/// Build the complete [`MemorySsaResult`] for `func`, given a precomputed
/// [`AliasAnalysisResult`].
///
/// "Standalone" because it takes the alias result as a parameter rather than
/// pulling it from an [`AnalysisManager`](crate::tir::analysis::AnalysisManager):
/// the deferred S1 `Analysis` impl will compute the alias result inline (since
/// `Analysis::compute` takes only `&TirFunction`) and delegate here.
///
/// The three classical phases:
///
/// * **A** — classify each op into Def / Use / neither (via [`classify`]).
/// * **B** — place memory phis at the iterated dominance frontier of every block
///   containing a Def (and the entry, which holds [`LIVE_ON_ENTRY`]).
/// * **C** — a dominator-tree renaming walk binds each Use to its
///   region-aware reaching def and each Def to the version it flows through.
pub fn compute_standalone(func: &TirFunction, alias: &AliasAnalysisResult) -> MemorySsaResult {
    // --- Shared CFG facts (full-CFG view, matching the S1 dominator analyses).
    let pred_map = dominators::build_pred_map(func);
    let idoms = dominators::compute_idoms(func, &pred_map);
    let dom_children = dominators::build_dom_children(&idoms);
    let reachable = dominators::reachable_blocks_with(func, CfgEdgePolicy::Full);

    // Deterministic reverse-postorder over reachable blocks (entry first).
    let rpo = reverse_postorder(func, &reachable);

    // version 0 = LIVE_ON_ENTRY. Allocate fresh versions from 1 upward.
    let mut next_version: u32 = 1;
    let mut result = MemorySsaResult {
        next_version,
        ..Default::default()
    };

    // --- Phase A: collect the per-block op roles (in op order). -------------
    // blocks_with_def: blocks that contain at least one clobbering Def.
    let mut def_blocks: HashSet<BlockId> = HashSet::new();
    for &bid in &rpo {
        let block = &func.blocks[&bid];
        for op in &block.ops {
            if classify(op, alias) == MemRole::Def {
                def_blocks.insert(bid);
                break;
            }
        }
    }

    // --- Phase B: iterated dominance frontier phi placement. ----------------
    let df = compute_dominance_frontiers(&idoms, &pred_map, &reachable);
    let phi_blocks = iterated_dominance_frontier(&def_blocks, &df);
    for &bid in &rpo {
        if phi_blocks.contains(&bid) {
            let ver = MemVersion(next_version);
            next_version += 1;
            result.block_phis.insert(bid, ver);
            // Incoming edges filled in during/after renaming (Phase C).
            result.defs.insert(
                ver,
                MemAccess::Phi {
                    ver,
                    block: bid,
                    incoming: Vec::new(),
                },
            );
        }
    }

    // --- Phase C: dominator-tree renaming walk. -----------------------------
    // `entry_version[b]` = the version live on entry to block `b` (its phi, if
    // any, else the version flowing in from its idom's exit). `exit_def[b]` =
    // the version live at the end of `b`.
    let mut entry_version: HashMap<BlockId, MemVersion> = HashMap::new();
    // The renaming proceeds in dominator-tree preorder so a block's idom is
    // always processed first (its exit version is the block's inherited entry).
    let preorder = dom_tree_preorder(func.entry_block, &dom_children);

    for &bid in &preorder {
        // Inherited version on entry to this block.
        let inherited = match idoms.get(&bid).and_then(|d| *d) {
            Some(idom) if idom != bid => {
                result.exit_def.get(&idom).copied().unwrap_or(LIVE_ON_ENTRY)
            }
            // Entry block (or self-idom): the function's live-on-entry memory.
            _ => LIVE_ON_ENTRY,
        };
        // A phi at this block shadows the inherited version on entry.
        let mut current = match result.block_phis.get(&bid) {
            Some(&phi_ver) => phi_ver,
            None => inherited,
        };
        entry_version.insert(bid, current);

        // Walk the block's ops, threading the current version.
        let block = &func.blocks[&bid];
        for (op_idx, op) in block.ops.iter().enumerate() {
            match classify(op, alias) {
                MemRole::Use => {
                    let region = alias.region_of(op);
                    // Region-aware reaching def: skip back through defs whose
                    // region does NOT may-alias this load's region. `current`
                    // and every version it flows through are the candidate
                    // chain; the first may-aliasing one is the reaching def.
                    let reaching = self_reaching_def(&result, current, &region);
                    // A Use defines no new memory version. The reaching version
                    // it reads goes in `block_op_to_use_def`; the full Use node
                    // (region + position) goes in `uses`. Neither goes in `defs`,
                    // which keys on versions that Defs and Phis produce.
                    result.block_op_to_use_def.insert((bid, op_idx), reaching);
                    result.uses.insert(
                        (bid, op_idx),
                        MemAccess::Use {
                            def_ver: reaching,
                            block: bid,
                            op_idx,
                            region,
                        },
                    );
                }
                MemRole::Def => {
                    let region = alias.region_of(op);
                    let ver = MemVersion(next_version);
                    next_version += 1;
                    result.defs.insert(
                        ver,
                        MemAccess::Def {
                            ver,
                            def_ver: current,
                            block: bid,
                            op_idx,
                            region,
                        },
                    );
                    result.block_op_to_def.insert((bid, op_idx), ver);
                    current = ver;
                }
                MemRole::None => {}
            }
        }

        result.exit_def.insert(bid, current);
    }

    // --- Phase C tail: fill phi incoming edges from predecessors' exits. -----
    // A phi's incoming version on edge `pred → bid` is `pred`'s exit version.
    let phi_versions: Vec<(BlockId, MemVersion)> =
        result.block_phis.iter().map(|(&b, &v)| (b, v)).collect();
    for (bid, phi_ver) in phi_versions {
        let mut incoming: Vec<(BlockId, MemVersion)> = pred_map
            .get(&bid)
            .map(|preds| {
                preds
                    .iter()
                    .filter(|p| reachable.contains(p))
                    .map(|&p| {
                        let v = result.exit_def.get(&p).copied().unwrap_or(LIVE_ON_ENTRY);
                        (p, v)
                    })
                    .collect()
            })
            .unwrap_or_default();
        incoming.sort_unstable_by_key(|(b, _)| b.0);
        if let Some(MemAccess::Phi { incoming: slot, .. }) = result.defs.get_mut(&phi_ver) {
            *slot = incoming;
        }
    }

    result.next_version = next_version;
    result
}

/// Walk the def-flow chain from `current` and return the first version whose
/// region may-alias `use_region` (the region-aware reaching def). A `Phi` and
/// the `LIVE_ON_ENTRY` floor always match (a phi merges possibly-aliasing
/// versions; live-on-entry is opaque external memory) — so the walk is total.
fn self_reaching_def(
    result: &MemorySsaResult,
    mut current: MemVersion,
    use_region: &MemRegion,
) -> MemVersion {
    loop {
        match result.defs.get(&current) {
            Some(MemAccess::Def {
                def_ver, region, ..
            }) => {
                if region.may_alias(use_region) {
                    return current;
                }
                // This def cannot have produced the loaded value; look further
                // back through the version it flows through.
                current = *def_ver;
            }
            // A phi merges versions from multiple paths — conservatively it may
            // carry an aliasing store, so it is a valid (conservative) reaching
            // def. The consumer inspects the phi's incomings to refine.
            Some(MemAccess::Phi { .. }) => return current,
            // LIVE_ON_ENTRY (version 0, not in `defs`) or any unrecorded
            // version: the opaque external-memory floor, always a match.
            _ => return current,
        }
    }
}

// ===========================================================================
// CFG helpers (dominance frontier, IDF, traversals)
// ===========================================================================

/// Reverse-postorder over the reachable blocks, entry first. Deterministic
/// (successor order is terminator-then-exception, both id-stable).
fn reverse_postorder(func: &TirFunction, reachable: &HashSet<BlockId>) -> Vec<BlockId> {
    let mut visited: HashSet<BlockId> = HashSet::new();
    let mut post: Vec<BlockId> = Vec::new();
    let label_to_block: HashMap<i64, BlockId> = func
        .label_id_map
        .iter()
        .map(|(&bid, &label)| (label, BlockId(bid)))
        .collect();
    dfs_post(
        func,
        func.entry_block,
        reachable,
        &label_to_block,
        &mut visited,
        &mut post,
    );
    post.reverse();
    post
}

fn dfs_post(
    func: &TirFunction,
    bid: BlockId,
    reachable: &HashSet<BlockId>,
    label_to_block: &HashMap<i64, BlockId>,
    visited: &mut HashSet<BlockId>,
    post: &mut Vec<BlockId>,
) {
    if !reachable.contains(&bid) || !visited.insert(bid) {
        return;
    }
    if let Some(block) = func.blocks.get(&bid) {
        for s in full_cfg_successors(block, label_to_block) {
            dfs_post(func, s, reachable, label_to_block, visited, post);
        }
    }
    post.push(bid);
}

/// Full-CFG successors (terminator + implicit exception edges) — matches the
/// edge policy of the S1 dominator analyses.
fn full_cfg_successors(
    block: &crate::tir::blocks::TirBlock,
    label_to_block: &HashMap<i64, BlockId>,
) -> Vec<BlockId> {
    let mut succs = dominators::terminator_successors(&block.terminator);
    succs.extend(dominators::exception_successors(block, label_to_block));
    succs
}

/// Dominance frontiers via the Cooper/Harvey/Kennedy algorithm, computed from
/// the immediate-dominator tree and the predecessor map.
fn compute_dominance_frontiers(
    idoms: &HashMap<BlockId, Option<BlockId>>,
    pred_map: &HashMap<BlockId, Vec<BlockId>>,
    reachable: &HashSet<BlockId>,
) -> HashMap<BlockId, HashSet<BlockId>> {
    let mut df: HashMap<BlockId, HashSet<BlockId>> = HashMap::new();
    for (&b, preds) in pred_map {
        if !reachable.contains(&b) {
            continue;
        }
        // Only join points (≥2 reachable preds) contribute to frontiers.
        let live_preds: Vec<BlockId> = preds
            .iter()
            .copied()
            .filter(|p| reachable.contains(p))
            .collect();
        if live_preds.len() < 2 {
            continue;
        }
        let idom_b = idoms.get(&b).and_then(|d| *d);
        for p in live_preds {
            let mut runner = p;
            // Walk up from `p` until we reach `b`'s idom, adding `b` to each
            // visited node's frontier.
            while Some(runner) != idom_b {
                df.entry(runner).or_default().insert(b);
                match idoms.get(&runner).and_then(|d| *d) {
                    Some(idom) if idom != runner => runner = idom,
                    // Reached the dominator-tree root; stop.
                    _ => break,
                }
            }
        }
    }
    df
}

/// The iterated dominance frontier of the Def-containing block set — the blocks
/// where memory phis must be placed. Standard worklist fixpoint.
fn iterated_dominance_frontier(
    def_blocks: &HashSet<BlockId>,
    df: &HashMap<BlockId, HashSet<BlockId>>,
) -> HashSet<BlockId> {
    let mut phi_blocks: HashSet<BlockId> = HashSet::new();
    let mut worklist: Vec<BlockId> = def_blocks.iter().copied().collect();
    while let Some(b) = worklist.pop() {
        if let Some(frontier) = df.get(&b) {
            for &f in frontier {
                if phi_blocks.insert(f) {
                    // A new phi block is itself a "def" of memory; iterate.
                    worklist.push(f);
                }
            }
        }
    }
    phi_blocks
}

/// Dominator-tree preorder from the root, in deterministic (ascending child id)
/// order. Iterative to avoid deep recursion on long dominator chains.
fn dom_tree_preorder(root: BlockId, dom_children: &HashMap<BlockId, Vec<BlockId>>) -> Vec<BlockId> {
    let mut order: Vec<BlockId> = Vec::new();
    let mut stack: Vec<BlockId> = vec![root];
    let mut seen: HashSet<BlockId> = HashSet::new();
    while let Some(b) = stack.pop() {
        if !seen.insert(b) {
            continue;
        }
        order.push(b);
        if let Some(children) = dom_children.get(&b) {
            // Push in reverse so children pop in ascending order.
            for &c in children.iter().rev() {
                stack.push(c);
            }
        }
    }
    order
}

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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::tir::blocks::{LoopRole, Terminator, TirBlock};
    use crate::tir::ops::{AttrDict, Dialect};
    use crate::tir::types::TirType;

    // -- builders -----------------------------------------------------------

    fn op(opcode: OpCode, operands: Vec<ValueId>, results: Vec<ValueId>) -> TirOp {
        TirOp {
            dialect: Dialect::Molt,
            opcode,
            operands,
            results,
            attrs: AttrDict::new(),
            source_span: None,
        }
    }

    /// A typed-slot store `obj.<offset> = val` of class `Point`
    /// (`_original_kind = "store"`, `_class = "Point"`). Carries the class
    /// identity so the alias oracle assigns a `TypedField { "Point", offset }`.
    fn store(obj: ValueId, val: ValueId, offset: i64) -> TirOp {
        store_of(obj, val, offset, "Point")
    }

    /// A typed-slot store with an explicit class name.
    fn store_of(obj: ValueId, val: ValueId, offset: i64, class: &str) -> TirOp {
        let mut o = op(OpCode::StoreAttr, vec![obj, val], vec![]);
        o.attrs.insert("value".into(), AttrValue::Int(offset));
        o.attrs
            .insert("_original_kind".into(), AttrValue::Str("store".into()));
        o.attrs
            .insert("_class".into(), AttrValue::Str(class.into()));
        o
    }

    /// A typed-slot store with NO class identity (a pre-S5-1.5 cached-artifact
    /// shape): fail-closed to `GenericHeap`.
    fn store_no_class(obj: ValueId, val: ValueId, offset: i64) -> TirOp {
        let mut o = op(OpCode::StoreAttr, vec![obj, val], vec![]);
        o.attrs.insert("value".into(), AttrValue::Int(offset));
        o.attrs
            .insert("_original_kind".into(), AttrValue::Str("store".into()));
        o
    }

    /// A proven-pure typed-slot load `r = obj.<offset>` of class `Point`
    /// (`_original_kind = "load"`, `_class = "Point"`).
    fn load(obj: ValueId, offset: i64, r: ValueId) -> TirOp {
        load_of(obj, offset, r, "Point")
    }

    /// A typed-slot load with an explicit class name.
    fn load_of(obj: ValueId, offset: i64, r: ValueId, class: &str) -> TirOp {
        let mut o = op(OpCode::LoadAttr, vec![obj], vec![r]);
        o.attrs.insert("value".into(), AttrValue::Int(offset));
        o.attrs
            .insert("_original_kind".into(), AttrValue::Str("load".into()));
        o.attrs
            .insert("_class".into(), AttrValue::Str(class.into()));
        o
    }

    /// A typed-slot load with NO class identity: fail-closed to `GenericHeap`.
    fn load_no_class(obj: ValueId, offset: i64, r: ValueId) -> TirOp {
        let mut o = op(OpCode::LoadAttr, vec![obj], vec![r]);
        o.attrs.insert("value".into(), AttrValue::Int(offset));
        o.attrs
            .insert("_original_kind".into(), AttrValue::Str("load".into()));
        o
    }

    /// An opaque call that clobbers `GenericHeap`.
    fn call(args: Vec<ValueId>, r: ValueId) -> TirOp {
        op(OpCode::Call, args, vec![r])
    }

    fn alias_of(func: &TirFunction) -> AliasAnalysisResult {
        // The alias analysis's `compute` is private; route through the public
        // S1 manager to obtain the same cached result a consumer would.
        use crate::tir::analysis::AnalysisManager;
        use crate::tir::passes::alias_analysis::AliasAnalysis;
        let mut am = AnalysisManager::new();
        am.get::<AliasAnalysis>(func).clone()
    }

    fn run(func: &TirFunction) -> MemorySsaResult {
        let alias = alias_of(func);
        compute_standalone(func, &alias)
    }

    // ── Test 1: straight-line def-use forwarding ───────────────────────────

    #[test]
    fn single_block_store_then_load_has_direct_reaching_def() {
        // entry: store(obj, val, 0); r = load(obj, 0); return r
        let mut func = TirFunction::new(
            "f".into(),
            vec![TirType::DynBox, TirType::DynBox],
            TirType::DynBox,
        );
        let obj = ValueId(0);
        let val = ValueId(1);
        let r = func.fresh_value();
        {
            let entry = func.blocks.get_mut(&func.entry_block).unwrap();
            entry.ops.push(store(obj, val, 0));
            entry.ops.push(load(obj, 0, r));
            entry.terminator = Terminator::Return { values: vec![r] };
        }
        let mem = run(&func);
        let store_ver = mem
            .def_at(func.entry_block, 0)
            .expect("store defines a version");
        let load_reaching = mem
            .reaching_def_for_use(func.entry_block, 1)
            .expect("load is a tracked use");
        assert_eq!(
            load_reaching, store_ver,
            "the load must read exactly the dominating store's version"
        );
        assert!(mem.is_direct_def_of_use(store_ver, func.entry_block, 1));
    }

    // ── CheckException is not a clobber ─────────────────────────────────────

    #[test]
    fn check_exception_between_store_and_load_does_not_clobber() {
        // store(obj, val, 0); check_exception; r = load(obj, 0)
        //
        // `CheckException` reads the pending-exception flag — it never writes
        // heap memory (its handler-edge control flow is modeled by the CFG, and
        // `may_observe_slot` is false for it). It must NOT bump the memory
        // version between the store and the load: it is emitted after nearly
        // every op in exception-bearing bodies, so classifying it as a
        // GenericHeap def starves store-to-load forwarding function-wide.
        let mut func = TirFunction::new(
            "f".into(),
            vec![TirType::DynBox, TirType::DynBox],
            TirType::DynBox,
        );
        let obj = ValueId(0);
        let val = ValueId(1);
        let r = func.fresh_value();
        {
            let entry = func.blocks.get_mut(&func.entry_block).unwrap();
            entry.ops.push(store(obj, val, 0));
            entry.ops.push(op(OpCode::CheckException, vec![], vec![]));
            entry.ops.push(load(obj, 0, r));
            entry.terminator = Terminator::Return { values: vec![r] };
        }
        let mem = run(&func);
        let store_ver = mem
            .def_at(func.entry_block, 0)
            .expect("store defines a version");
        assert!(
            mem.def_at(func.entry_block, 1).is_none(),
            "CheckException must not be a MemoryDef"
        );
        let reaching = mem
            .reaching_def_for_use(func.entry_block, 2)
            .expect("load is a tracked use");
        assert_eq!(
            reaching, store_ver,
            "the load must still read the store's version across CheckException"
        );
        assert!(mem.is_direct_def_of_use(store_ver, func.entry_block, 2));
    }

    // ── AnalysisManager registration ────────────────────────────────────────

    #[test]
    fn analysis_manager_registration_matches_compute_standalone() {
        // The S1 manager path (`am.get::<MemorySSA>`) must yield exactly the
        // result `compute_standalone` produces over the alias substrate.
        let mut func = TirFunction::new(
            "f".into(),
            vec![TirType::DynBox, TirType::DynBox],
            TirType::DynBox,
        );
        let obj = ValueId(0);
        let val = ValueId(1);
        let r = func.fresh_value();
        {
            let entry = func.blocks.get_mut(&func.entry_block).unwrap();
            entry.ops.push(store(obj, val, 0));
            entry.ops.push(load(obj, 0, r));
            entry.terminator = Terminator::Return { values: vec![r] };
        }
        let direct = run(&func);
        use crate::tir::analysis::AnalysisManager;
        let mut am = AnalysisManager::new();
        let via_manager = am.get::<MemorySSA>(&func);
        assert_eq!(via_manager.next_version, direct.next_version);
        assert_eq!(
            via_manager.def_at(func.entry_block, 0),
            direct.def_at(func.entry_block, 0),
        );
        assert_eq!(
            via_manager.reaching_def_for_use(func.entry_block, 1),
            direct.reaching_def_for_use(func.entry_block, 1),
        );
    }

    // ── Test 2: store-store kill (last store dominates the read) ───────────

    #[test]
    fn store_store_kills_earlier_version_for_load() {
        // store(obj, v1, 0); store(obj, v2, 0); r = load(obj, 0)
        let mut func = TirFunction::new(
            "f".into(),
            vec![TirType::DynBox, TirType::DynBox, TirType::DynBox],
            TirType::DynBox,
        );
        let obj = ValueId(0);
        let v1 = ValueId(1);
        let v2 = ValueId(2);
        let r = func.fresh_value();
        {
            let entry = func.blocks.get_mut(&func.entry_block).unwrap();
            entry.ops.push(store(obj, v1, 0));
            entry.ops.push(store(obj, v2, 0));
            entry.ops.push(load(obj, 0, r));
            entry.terminator = Terminator::Return { values: vec![r] };
        }
        let mem = run(&func);
        let first = mem.def_at(func.entry_block, 0).unwrap();
        let second = mem.def_at(func.entry_block, 1).unwrap();
        let reaching = mem.reaching_def_for_use(func.entry_block, 2).unwrap();
        assert_eq!(
            reaching, second,
            "load reads the SECOND store (it kills the first)"
        );
        assert_ne!(
            reaching, first,
            "the first store is killed by the overwrite"
        );
        // The second def flows through the first (the clobber chain is intact).
        assert_eq!(mem.def_version_of(second), Some(first));
    }

    // ── Test 3: may_alias-blocked forwarding (distinct offsets) ────────────

    #[test]
    fn distinct_offsets_have_independent_reaching_defs() {
        // store(obj, v1, 0); store(obj, v2, 8); r0 = load(obj, 0); r8 = load(obj, 8)
        // With class-aware `TypedField` regions (S5-1.5), the same-class fields at
        // offsets 0 and 8 are DISJOINT, so each load refines to the store of its
        // OWN offset — store@8 does NOT clobber load@0.
        let mut func = TirFunction::new(
            "f".into(),
            vec![TirType::DynBox, TirType::DynBox, TirType::DynBox],
            TirType::DynBox,
        );
        let obj = ValueId(0);
        let v1 = ValueId(1);
        let v2 = ValueId(2);
        let r0 = func.fresh_value();
        let r8 = func.fresh_value();
        {
            let entry = func.blocks.get_mut(&func.entry_block).unwrap();
            entry.ops.push(store(obj, v1, 0)); // op 0 — TypedField{Point, 0}
            entry.ops.push(store(obj, v2, 8)); // op 1 — TypedField{Point, 8}
            entry.ops.push(load(obj, 0, r0)); // op 2 — TypedField{Point, 0}
            entry.ops.push(load(obj, 8, r8)); // op 3 — TypedField{Point, 8}
            entry.terminator = Terminator::Return {
                values: vec![r0, r8],
            };
        }
        let mem = run(&func);
        let store0 = mem.def_at(func.entry_block, 0).unwrap();
        let store8 = mem.def_at(func.entry_block, 1).unwrap();
        let load0_reaching = mem.reaching_def_for_use(func.entry_block, 2).unwrap();
        let load8_reaching = mem.reaching_def_for_use(func.entry_block, 3).unwrap();
        // Offset disambiguation: each load reaches the store of ITS offset.
        assert_eq!(
            load0_reaching, store0,
            "load@0 reaches store@0 (store@8 is a disjoint field)"
        );
        assert_eq!(load8_reaching, store8, "load@8 reaches store@8");
        // The clobber chain is still store0 ← store8 (store@8 flows through
        // store@0 — they are ordered defs, just disjoint regions).
        assert_eq!(mem.def_version_of(store8), Some(store0));
        // Forwarding is now unblocked for BOTH loads.
        assert!(mem.is_direct_def_of_use(store0, func.entry_block, 2));
        assert!(mem.is_direct_def_of_use(store8, func.entry_block, 3));
    }

    #[test]
    fn distinct_classes_at_same_offset_do_not_clobber() {
        // A `Point.x@0` store followed by a `Line.a@0` store must NOT clobber a
        // `Point.x@0` load: distinct concrete classes never share an object, so
        // `TypedField{Point,0}` and `TypedField{Line,0}` are disjoint.
        let mut func = TirFunction::new(
            "f".into(),
            vec![
                TirType::DynBox,
                TirType::DynBox,
                TirType::DynBox,
                TirType::DynBox,
            ],
            TirType::DynBox,
        );
        let p = ValueId(0);
        let l = ValueId(1);
        let v1 = ValueId(2);
        let v2 = ValueId(3);
        let r = func.fresh_value();
        {
            let entry = func.blocks.get_mut(&func.entry_block).unwrap();
            entry.ops.push(store_of(p, v1, 0, "Point")); // op 0
            entry.ops.push(store_of(l, v2, 0, "Line")); // op 1 — disjoint class
            entry.ops.push(load_of(p, 0, r, "Point")); // op 2
            entry.terminator = Terminator::Return { values: vec![r] };
        }
        let mem = run(&func);
        let store_point = mem.def_at(func.entry_block, 0).unwrap();
        let reaching = mem.reaching_def_for_use(func.entry_block, 2).unwrap();
        assert_eq!(
            reaching, store_point,
            "the Point.x load reaches the Point store, not the disjoint Line store"
        );
        assert!(mem.is_direct_def_of_use(store_point, func.entry_block, 2));
    }

    #[test]
    fn same_class_offset_store_still_clobbers() {
        // A same-class same-offset store BETWEEN a store and a load IS a clobber:
        // object identity is untracked, so two `Point.x@0` accesses may-alias.
        let mut func = TirFunction::new(
            "f".into(),
            vec![
                TirType::DynBox,
                TirType::DynBox,
                TirType::DynBox,
                TirType::DynBox,
            ],
            TirType::DynBox,
        );
        let a = ValueId(0);
        let b = ValueId(1); // possibly the same Point as `a` at runtime
        let v1 = ValueId(2);
        let v2 = ValueId(3);
        let r = func.fresh_value();
        {
            let entry = func.blocks.get_mut(&func.entry_block).unwrap();
            entry.ops.push(store_of(a, v1, 0, "Point")); // op 0
            entry.ops.push(store_of(b, v2, 0, "Point")); // op 1 — same class+offset
            entry.ops.push(load_of(a, 0, r, "Point")); // op 2
            entry.terminator = Terminator::Return { values: vec![r] };
        }
        let mem = run(&func);
        let store_b = mem.def_at(func.entry_block, 1).unwrap();
        let reaching = mem.reaching_def_for_use(func.entry_block, 2).unwrap();
        assert_eq!(
            reaching, store_b,
            "a same-class+offset store on a possibly-different object still clobbers"
        );
    }

    #[test]
    fn no_class_typed_slot_falls_back_to_generic_heap() {
        // A typed-slot op with NO `_class` proof (a pre-S5-1.5 cached artifact)
        // must fail-closed to GenericHeap: the offset-8 store then clobbers the
        // offset-0 load (GenericHeap may-aliases everything).
        let mut func = TirFunction::new(
            "f".into(),
            vec![TirType::DynBox, TirType::DynBox, TirType::DynBox],
            TirType::DynBox,
        );
        let obj = ValueId(0);
        let v1 = ValueId(1);
        let v2 = ValueId(2);
        let r0 = func.fresh_value();
        {
            let entry = func.blocks.get_mut(&func.entry_block).unwrap();
            entry.ops.push(store_no_class(obj, v1, 0)); // op 0 — GenericHeap
            entry.ops.push(store_no_class(obj, v2, 8)); // op 1 — GenericHeap
            entry.ops.push(load_no_class(obj, 0, r0)); // op 2 — GenericHeap
            entry.terminator = Terminator::Return { values: vec![r0] };
        }
        let mem = run(&func);
        let store8 = mem.def_at(func.entry_block, 1).unwrap();
        let load0_reaching = mem.reaching_def_for_use(func.entry_block, 2).unwrap();
        assert_eq!(
            load0_reaching, store8,
            "fail-closed: a class-less typed-slot load reaches the most-recent GenericHeap store"
        );
    }

    // ── Test 4: cross-block phi placement at a diamond join ────────────────

    #[test]
    fn phi_placed_at_join_of_two_stores() {
        // bb0 -> {bb1: store(obj,v1,0), bb2: store(obj,v2,0)} -> bb3: r = load(obj,0)
        let mut func = TirFunction::new(
            "f".into(),
            vec![
                TirType::DynBox,
                TirType::DynBox,
                TirType::DynBox,
                TirType::Bool,
            ],
            TirType::DynBox,
        );
        let obj = ValueId(0);
        let v1 = ValueId(1);
        let v2 = ValueId(2);
        let cond = ValueId(3);
        let bb1 = func.fresh_block();
        let bb2 = func.fresh_block();
        let bb3 = func.fresh_block();
        let r = func.fresh_value();
        {
            let entry = func.blocks.get_mut(&func.entry_block).unwrap();
            entry.terminator = Terminator::CondBranch {
                cond,
                then_block: bb1,
                then_args: vec![],
                else_block: bb2,
                else_args: vec![],
            };
        }
        func.blocks.insert(
            bb1,
            TirBlock {
                id: bb1,
                args: vec![],
                ops: vec![store(obj, v1, 0)],
                terminator: Terminator::Branch {
                    target: bb3,
                    args: vec![],
                },
            },
        );
        func.blocks.insert(
            bb2,
            TirBlock {
                id: bb2,
                args: vec![],
                ops: vec![store(obj, v2, 0)],
                terminator: Terminator::Branch {
                    target: bb3,
                    args: vec![],
                },
            },
        );
        func.blocks.insert(
            bb3,
            TirBlock {
                id: bb3,
                args: vec![],
                ops: vec![load(obj, 0, r)],
                terminator: Terminator::Return { values: vec![r] },
            },
        );
        let mem = run(&func);
        let phi = mem
            .block_phis
            .get(&bb3)
            .copied()
            .expect("a memory phi at the join");
        let reaching = mem.reaching_def_for_use(bb3, 0).unwrap();
        assert_eq!(
            reaching, phi,
            "the load reads the join phi, not either branch store"
        );
        // The phi has two incomings, one per branch, each that branch's store.
        match mem.access(phi) {
            Some(MemAccess::Phi { incoming, .. }) => {
                assert_eq!(incoming.len(), 2, "phi merges both predecessor edges");
                let store1 = mem.def_at(bb1, 0).unwrap();
                let store2 = mem.def_at(bb2, 0).unwrap();
                let versions: HashSet<MemVersion> = incoming.iter().map(|(_, v)| *v).collect();
                assert!(versions.contains(&store1), "incoming includes bb1's store");
                assert!(versions.contains(&store2), "incoming includes bb2's store");
            }
            other => panic!("expected a Phi access, got {other:?}"),
        }
        // The forwarding query must NOT claim a single direct def (it is a phi).
        let store1 = mem.def_at(bb1, 0).unwrap();
        assert!(
            !mem.is_direct_def_of_use(store1, bb3, 0),
            "a phi-merged load has no single direct store def — forwarding must be blocked"
        );
    }

    // ── Test 5: call-barrier via the alias-oracle region classification ────

    #[test]
    fn generic_heap_call_kills_typed_field_load_reaching_def() {
        // bb0: store(obj, v1, 0); call(obj); r = load(obj, 0)
        // The call is a GenericHeap def (the alias oracle widens Call), which
        // may_alias-es the typed-slot load — so the load reaches the CALL's
        // version, not the store's. Forwarding is correctly blocked.
        let mut func = TirFunction::new(
            "f".into(),
            vec![TirType::DynBox, TirType::DynBox],
            TirType::DynBox,
        );
        let obj = ValueId(0);
        let v1 = ValueId(1);
        let call_r = func.fresh_value();
        let r = func.fresh_value();
        {
            let entry = func.blocks.get_mut(&func.entry_block).unwrap();
            entry.ops.push(store(obj, v1, 0)); // op 0
            entry.ops.push(call(vec![obj], call_r)); // op 1 — GenericHeap def
            entry.ops.push(load(obj, 0, r)); // op 2
            entry.terminator = Terminator::Return { values: vec![r] };
        }
        let mem = run(&func);
        let store_ver = mem.def_at(func.entry_block, 0).unwrap();
        let call_ver = mem
            .def_at(func.entry_block, 1)
            .expect("the call is a memory def");
        let reaching = mem.reaching_def_for_use(func.entry_block, 2).unwrap();
        assert_eq!(
            reaching, call_ver,
            "load reaches the clobbering call, not the store"
        );
        assert_ne!(
            reaching, store_ver,
            "the call kills the store's reaching-def relationship"
        );
        assert!(
            !mem.is_direct_def_of_use(store_ver, func.entry_block, 2),
            "store-to-load forwarding across a call barrier must be blocked"
        );
    }

    // ── Test 6: ModuleDict def is independent of a heap (stack) field load ─

    #[test]
    fn module_dict_def_does_not_kill_stack_object_field_load() {
        // obj = ObjectNewBound (non-escaping ⇒ the alias oracle proves NoEscape
        // and classifies obj's slots as a StackObject region); store(obj, v, 0);
        // ModuleSetAttr(...); r = load(obj, 0).
        //
        // The module mutation's region is ModuleDict; the stack object's field is
        // a StackObject region. `MemRegion::may_alias(StackObject, ModuleDict)` is
        // false, so the module def does NOT become the load's reaching def — the
        // store does. This is the region-disjointness precision the alias oracle
        // provides and MemorySSA must preserve. (We build the *pre-rewrite*
        // `ObjectNewBound`: escape analysis tracks it and proves NoEscape, which
        // is exactly the condition under which the oracle assigns a StackObject
        // region — a bare `ObjectNewBoundStack` op is the post-rewrite form the
        // escape pass produces and does not re-add to its tracked-root set.)
        let mut func = TirFunction::new(
            "f".into(),
            vec![TirType::DynBox, TirType::DynBox],
            TirType::DynBox,
        );
        let cls = ValueId(0);
        let v = ValueId(1);
        let obj = func.fresh_value();
        let modset_r = func.fresh_value();
        let r = func.fresh_value();
        {
            let entry = func.blocks.get_mut(&func.entry_block).unwrap();
            // obj = object_new_bound(cls)  (non-escaping ⇒ StackObject region)
            let mut alloc = op(OpCode::ObjectNewBound, vec![cls], vec![obj]);
            alloc.attrs.insert("value".into(), AttrValue::Int(16));
            entry.ops.push(alloc); // op 0
            entry.ops.push(store(obj, v, 0)); // op 1 — StackObject def
            // A module-dict mutation (distinct region).
            let mut modset = op(OpCode::ModuleSetAttr, vec![ValueId(99)], vec![modset_r]);
            modset.attrs.insert("value".into(), AttrValue::Int(0));
            entry.ops.push(modset); // op 2 — ModuleDict def
            entry.ops.push(load(obj, 0, r)); // op 3 — StackObject use
            entry.terminator = Terminator::Return { values: vec![r] };
        }
        // Precondition: the alias oracle must actually assign disjoint regions,
        // else the test would pass vacuously. Pin it.
        let alias = alias_of(&func);
        assert!(
            matches!(
                alias.region_of(&func.blocks[&func.entry_block].ops[1]),
                MemRegion::StackObject { .. }
            ),
            "the field store must classify as a StackObject region for this test to be meaningful"
        );
        assert_eq!(
            alias.region_of(&func.blocks[&func.entry_block].ops[2]),
            MemRegion::ModuleDict
        );

        let mem = compute_standalone(&func, &alias);
        let store_ver = mem
            .def_at(func.entry_block, 1)
            .expect("the field store is a def");
        let reaching = mem.reaching_def_for_use(func.entry_block, 3).unwrap();
        assert_eq!(
            reaching, store_ver,
            "the StackObject field load reaches its store, NOT the disjoint ModuleDict mutation"
        );
        assert!(
            mem.is_direct_def_of_use(store_ver, func.entry_block, 3),
            "region disjointness lets forwarding succeed across the module mutation"
        );
    }

    // ── Test 7: loop back-edge phi placement ───────────────────────────────

    #[test]
    fn loop_back_edge_places_memory_phi_at_header() {
        // preheader(entry) -> header; header: store(obj, v, 0) then cond back to
        // header or exit. The store on the back edge forces a memory phi at the
        // header (its own def reaches it on the back edge).
        let mut func = TirFunction::new(
            "f".into(),
            vec![TirType::DynBox, TirType::DynBox, TirType::Bool],
            TirType::None,
        );
        let obj = ValueId(0);
        let v = ValueId(1);
        let cond = ValueId(2);
        let header = func.fresh_block();
        let exit = func.fresh_block();
        {
            let entry = func.blocks.get_mut(&func.entry_block).unwrap();
            entry.terminator = Terminator::Branch {
                target: header,
                args: vec![],
            };
        }
        func.blocks.insert(
            header,
            TirBlock {
                id: header,
                args: vec![],
                ops: vec![store(obj, v, 0)],
                terminator: Terminator::CondBranch {
                    cond,
                    then_block: header,
                    then_args: vec![],
                    else_block: exit,
                    else_args: vec![],
                },
            },
        );
        func.blocks.insert(
            exit,
            TirBlock {
                id: exit,
                args: vec![],
                ops: vec![],
                terminator: Terminator::Return { values: vec![] },
            },
        );
        func.loop_roles.insert(header, LoopRole::LoopHeader);

        let mem = run(&func);
        // The header is in the dominance frontier of itself (back-edge join),
        // so a memory phi must be placed there.
        let phi = mem.block_phis.get(&header).copied();
        assert!(
            phi.is_some(),
            "a back-edge loop header must receive a memory phi"
        );
        let phi = phi.unwrap();
        match mem.access(phi) {
            Some(MemAccess::Phi { incoming, .. }) => {
                // Two incoming edges: the preheader (entry) and the back edge
                // (header's own exit version).
                assert_eq!(incoming.len(), 2, "header phi merges preheader + back edge");
                let header_store = mem.def_at(header, 0).unwrap();
                let from_back: Vec<MemVersion> = incoming
                    .iter()
                    .filter(|(b, _)| *b == header)
                    .map(|(_, v)| *v)
                    .collect();
                assert_eq!(
                    from_back,
                    vec![header_store],
                    "the back edge carries the header store's version into the phi"
                );
                let from_pre: Vec<MemVersion> = incoming
                    .iter()
                    .filter(|(b, _)| *b == func.entry_block)
                    .map(|(_, v)| *v)
                    .collect();
                assert_eq!(
                    from_pre,
                    vec![LIVE_ON_ENTRY],
                    "the preheader carries live-on-entry into the phi"
                );
            }
            other => panic!("expected a header Phi, got {other:?}"),
        }
    }

    // ── Structural invariants ──────────────────────────────────────────────

    #[test]
    fn empty_function_has_no_memory_accesses() {
        let mut func = TirFunction::new("f".into(), vec![], TirType::None);
        func.blocks.get_mut(&func.entry_block).unwrap().terminator =
            Terminator::Return { values: vec![] };
        let mem = run(&func);
        assert!(mem.defs.is_empty(), "no memory ops ⇒ no Def/Phi nodes");
        assert!(mem.block_op_to_def.is_empty());
        assert!(mem.block_op_to_use_def.is_empty());
        assert!(mem.block_phis.is_empty());
        // exit_def for entry is LIVE_ON_ENTRY.
        assert_eq!(mem.exit_def.get(&func.entry_block), Some(&LIVE_ON_ENTRY));
        assert_eq!(mem.next_version, 1, "only LIVE_ON_ENTRY consumed");
    }

    #[test]
    fn typed_slot_store_value_extracts_target_value_offset() {
        let s = store(ValueId(3), ValueId(7), 8);
        assert_eq!(
            typed_slot_store_value(&s),
            Some((ValueId(3), ValueId(7), 8))
        );
        // A non-store op yields None.
        let l = load(ValueId(3), 8, ValueId(9));
        assert_eq!(typed_slot_store_value(&l), None);
    }

    #[test]
    fn use_node_carries_region_and_reaching_def() {
        // store(obj, val, 0); r = load(obj, 0) — the `uses` map records the load
        // as a full `Use` node carrying its region (TypedField/GenericHeap) and
        // the reaching def. This pins the `MemAccess::Use` fields as load-bearing
        // for the S5-2b MemGVN consumer.
        let mut func = TirFunction::new(
            "f".into(),
            vec![TirType::DynBox, TirType::DynBox],
            TirType::DynBox,
        );
        let obj = ValueId(0);
        let val = ValueId(1);
        let r = func.fresh_value();
        {
            let entry = func.blocks.get_mut(&func.entry_block).unwrap();
            entry.ops.push(store(obj, val, 0));
            entry.ops.push(load(obj, 0, r));
            entry.terminator = Terminator::Return { values: vec![r] };
        }
        let mem = run(&func);
        let store_ver = mem.def_at(func.entry_block, 0).unwrap();
        let use_node = mem
            .uses
            .get(&(func.entry_block, 1))
            .expect("the load is recorded as a Use node");
        match use_node {
            MemAccess::Use {
                def_ver,
                block,
                op_idx,
                region,
            } => {
                assert_eq!(*def_ver, store_ver, "Use reads the store's version");
                assert_eq!(*block, func.entry_block);
                assert_eq!(*op_idx, 1);
                // The typed-slot load carries its proven class identity, so it
                // names a `TypedField { "Point", 0 }` region (S5-1.5).
                assert_eq!(
                    *region,
                    MemRegion::TypedField {
                        class: "Point".into(),
                        offset: 0
                    }
                );
            }
            other => panic!("expected a Use node, got {other:?}"),
        }
        // The Use node's accessor helpers agree.
        assert_eq!(use_node.defined_version(), None, "a Use defines no version");
        assert_eq!(use_node.block(), func.entry_block);
    }
}
