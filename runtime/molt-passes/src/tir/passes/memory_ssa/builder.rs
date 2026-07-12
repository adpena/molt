use std::collections::{HashMap, HashSet};

use crate::tir::blocks::BlockId;
use crate::tir::dominators::{self, CfgEdgePolicy};
use crate::tir::function::TirFunction;

use super::super::alias_analysis::{AliasAnalysisResult, MemRegion};
use super::access::{LIVE_ON_ENTRY, MemAccess, MemVersion, MemorySsaResult};
use super::cfg::{
    compute_dominance_frontiers, dom_tree_preorder, iterated_dominance_frontier, reverse_postorder,
};
use super::classify::{MemRole, classify};

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
