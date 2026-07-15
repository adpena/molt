//! Natural-loop discovery for module-slot promotion: innermost,
//! single-preheader, linear-body loops derived from dominance back-edges over
//! the terminator-only CFG (module chunks carry no `loop_roles`).

use std::collections::{HashMap, HashSet};

use crate::tir::blocks::BlockId;
use crate::tir::dominators::{
    CfgEdgePolicy, collect_loop_blocks, dominates, reachable_blocks_with, terminator_successors,
};
use crate::tir::function::TirFunction;

use super::DebugLog;

pub(super) struct LoopInfo {
    pub(super) header: BlockId,
    pub(super) blocks: HashSet<BlockId>,
    /// The unique preheader (the single predecessor of the header outside the
    /// loop). Loops with multiple entries are refused.
    pub(super) preheader: BlockId,
    /// Loop blocks in the unique linear in-loop order starting at the header
    /// (every block has at most one in-loop successor).
    pub(super) linear_order: Vec<BlockId>,
}

/// Discover innermost, single-preheader, linear-body natural loops over the
/// terminator-only CFG (module chunks carry no `loop_roles` — their loops are
/// jump-shaped — so headers are derived from dominance back-edges).
pub(super) fn discover_loops(
    func: &TirFunction,
    pred_map: &HashMap<BlockId, Vec<BlockId>>,
    idoms: &HashMap<BlockId, Option<BlockId>>,
    dbg: &mut DebugLog,
) -> Vec<LoopInfo> {
    // Dead blocks (e.g. a vestigial loop-else that still carries a branch to
    // the header) must not affect discovery: an UNREACHABLE outside
    // predecessor would otherwise break the unique-preheader requirement for a
    // perfectly promotable loop. Reachability over the terminator-only CFG.
    let reachable = reachable_blocks_with(func, CfgEdgePolicy::TerminatorOnly);

    // Headers: reachable blocks with a reachable predecessor they dominate.
    let mut headers: Vec<BlockId> = func
        .blocks
        .keys()
        .copied()
        .filter(|h| reachable.contains(h))
        .filter(|&h| {
            pred_map.get(&h).is_some_and(|ps| {
                ps.iter()
                    .any(|&p| reachable.contains(&p) && dominates(h, p, idoms))
            })
        })
        .collect();
    headers.sort_by_key(|b| b.0);

    let mut loops = Vec::new();
    'next_header: for &header in &headers {
        let blocks = collect_loop_blocks(func, pred_map, idoms, header);
        // Innermost only: no other header inside this loop's body.
        if headers
            .iter()
            .any(|&h2| h2 != header && blocks.contains(&h2))
        {
            dbg.note(format!("{} loop@{:?}: not innermost", func.name, header));
            continue;
        }
        // Unique REACHABLE preheader (dead outside-preds are ignored).
        let outside_preds: Vec<BlockId> = pred_map
            .get(&header)
            .map(|ps| {
                ps.iter()
                    .copied()
                    .filter(|p| !blocks.contains(p) && reachable.contains(p))
                    .collect()
            })
            .unwrap_or_default();
        let [preheader] = outside_preds[..] else {
            dbg.note(format!(
                "{} loop@{:?}: refused ({} reachable outside preds, need exactly 1)",
                func.name,
                header,
                outside_preds.len()
            ));
            continue;
        };
        // Linear in-loop order: from the header, follow the unique in-loop
        // terminator successor; every loop block must be visited exactly once
        // and have ≤1 in-loop successor (no internal joins/splits inside).
        let mut order = vec![header];
        let mut seen: HashSet<BlockId> = [header].into();
        let mut cur = header;
        while order.len() < blocks.len() {
            let succs = terminator_successors(&func.blocks[&cur].terminator);
            let inside: Vec<BlockId> = succs
                .iter()
                .copied()
                .filter(|s| blocks.contains(s) && !seen.contains(s))
                .collect();
            let [next] = inside[..] else {
                dbg.note(format!(
                    "{} loop@{:?}: refused (non-linear body at {:?}: {} unseen in-loop succs)",
                    func.name,
                    header,
                    cur,
                    inside.len()
                ));
                continue 'next_header;
            };
            order.push(next);
            seen.insert(next);
            cur = next;
        }
        loops.push(LoopInfo {
            header,
            blocks,
            preheader,
            linear_order: order,
        });
    }
    loops
}
