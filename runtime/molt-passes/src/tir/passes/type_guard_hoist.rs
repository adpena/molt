//! Type Guard Hoisting pass for TIR.
//!
//! Hoists TypeGuard ops out of loops when the guarded value is loop-invariant.
//!
//! Before: loop { %ok = TypeGuard(%x, INT); use(%ok) }
//! After:  %ok = TypeGuard(%x, INT); loop { use(%ok) }
//!
//! A value is loop-invariant if it is defined OUTSIDE the loop (i.e. in a
//! block that dominates the loop header).
//!
//! ## Loop-shape authority
//!
//! Loop headers and natural-loop bodies come from the shared
//! [`LoopForest`](crate::tir::analysis::LoopForest) analysis. The pass derives
//! a loop preheader from the header predecessors outside that canonical body,
//! and uses immediate dominators to prove a guarded value is available before
//! the loop.
//!
//! 1. Build a map: ValueId → BlockId (the block that defines the value).
//!    Block arguments are treated as defined in their own block.
//! 2. Read LoopForest headers and body sets.
//! 3. The "preheader" of a loop is the unique predecessor of the header that is
//!    outside the LoopForest body.
//! 4. A TypeGuard inside a loop block B is hoistable if:
//!    - Its first operand is defined outside that LoopForest body.
//!    - Its defining block dominates the loop header.
//!    - A unique preheader block exists.
//!
//! This is conservative: it may miss some hoisting opportunities in irregular
//! CFGs, but it will never hoist unsafely.

use std::collections::{HashMap, HashSet};

use crate::tir::analysis::{AnalysisManager, ImmediateDoms, LoopForest, PredMap};
use crate::tir::blocks::BlockId;
use crate::tir::dominators::dominates;
use crate::tir::function::TirFunction;
use crate::tir::ops::{OpCode, TirOp};
use crate::tir::values::ValueId;

use super::PassStats;

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

/// Build a map: ValueId → BlockId that defines it (block args + op results).
///
/// This intentionally EXCLUDES function params (unlike the canonical
/// [`DefMap`](crate::tir::analysis::DefMap) analysis): a TypeGuard whose
/// operand is a param has no in-function defining block here, so it is left
/// un-hoisted. Routing through the param-including `DefMap` would change which
/// guards are considered loop-invariant, so this stays a local computation.
fn build_def_map(func: &TirFunction) -> HashMap<ValueId, BlockId> {
    let mut def_map: HashMap<ValueId, BlockId> = HashMap::new();
    for (&bid, block) in &func.blocks {
        // Block arguments are "defined" at block entry.
        for arg in &block.args {
            def_map.insert(arg.id, bid);
        }
        // Op results are defined in this block.
        for op in &block.ops {
            for &result in &op.results {
                def_map.insert(result, bid);
            }
        }
    }
    def_map
}

// ---------------------------------------------------------------------------
// Main pass
// ---------------------------------------------------------------------------

/// Hoist TypeGuard ops out of loops when the guarded value is loop-invariant.
pub fn run(func: &mut TirFunction, am: &mut AnalysisManager) -> PassStats {
    let mut stats = PassStats {
        name: "type_guard_hoist",
        ..Default::default()
    };

    if func.blocks.is_empty() {
        return stats;
    }

    // When exception handling is present, bail out entirely — hoisting a
    // TypeGuard out of a loop that is inside a try region could move the
    // guard across an exception boundary, changing semantics if the
    // exception handler resets the type.
    if func.has_exception_handling {
        return stats;
    }

    // Build def map (param-excluding; see `build_def_map`) and take the cached
    // predecessor map. type_guard_hoist only reaches this point when
    // `has_exception_handling == false` (it bailed above otherwise), so the
    // function has no exception-edge ops and the full-CFG `PredMap` coincides
    // with the terminator-only predecessor relation this pass needs.
    let def_map = build_def_map(func);
    let pred_map = am.get::<PredMap>(func).clone();
    let idoms = am.get::<ImmediateDoms>(func).clone();
    let loop_forest = am.get::<LoopForest>(func).clone();

    // For each LoopForest header, keep the canonical body and derive the unique
    // preheader from incoming header edges outside that body.
    struct LoopInfo {
        preheader: Option<BlockId>,
        body: HashSet<BlockId>,
    }

    let mut loop_headers: HashMap<BlockId, LoopInfo> = HashMap::new();

    for &header in &loop_forest.headers {
        let Some(body) = loop_forest.bodies.get(&header) else {
            continue;
        };
        let mut preheaders: Vec<BlockId> = pred_map
            .get(&header)
            .map(|preds| {
                preds
                    .iter()
                    .copied()
                    .filter(|pred| !body.contains(pred))
                    .collect()
            })
            .unwrap_or_default();
        preheaders.sort_unstable_by_key(|b| b.0);
        preheaders.dedup();
        let preheader = if preheaders.len() == 1 {
            Some(preheaders[0])
        } else {
            None // ambiguous or no preheader — skip hoisting
        };
        loop_headers.insert(
            header,
            LoopInfo {
                preheader,
                body: body.clone(),
            },
        );
    }

    if loop_headers.is_empty() {
        return stats;
    }

    // Identify all blocks that are inside any loop. When loop bodies overlap,
    // choose the smallest body first so hoisting targets the innermost loop
    // deterministically.
    let mut block_to_header: HashMap<BlockId, BlockId> = HashMap::new();
    let mut ordered_loops: Vec<(BlockId, usize)> = loop_headers
        .iter()
        .map(|(&header, info)| (header, info.body.len()))
        .collect();
    ordered_loops.sort_unstable_by_key(|(header, body_len)| (*body_len, header.0));

    for (header, _) in ordered_loops {
        let Some(info) = loop_headers.get(&header) else {
            continue;
        };
        for &bid in &info.body {
            block_to_header.entry(bid).or_insert(header);
        }
    }

    // For each block inside a loop, check for hoistable TypeGuard ops.
    // Collect (preheader_id, op) pairs to insert, and mark source ops for removal.
    struct HoistWork {
        preheader: BlockId,
        op: TirOp,
        source_block: BlockId,
        source_idx: usize,
    }

    let mut hoist_list: Vec<HoistWork> = Vec::new();

    // Collect block ids to iterate (avoids borrow conflict).
    let block_ids: Vec<BlockId> = func.blocks.keys().copied().collect();

    for bid in &block_ids {
        // Is this block inside a loop?
        let header = match block_to_header.get(bid) {
            Some(&h) => h,
            None => continue,
        };

        // Is the header block the same as bid? If so it's the header itself;
        // TypeGuards in the header are not "inside" the loop body from the
        // pre-header perspective, but they do execute every iteration.
        // We hoist from the header too if the operand is defined before the header.

        // Get the preheader.
        let preheader = match loop_headers.get(&header).and_then(|li| li.preheader) {
            Some(p) => p,
            None => continue, // no unique preheader — skip
        };

        let block = match func.blocks.get(bid) {
            Some(b) => b,
            None => continue,
        };

        for (idx, op) in block.ops.iter().enumerate() {
            if op.opcode != OpCode::TypeGuard {
                continue;
            }
            // The guarded operand is operands[0].
            let guarded = match op.operands.first().copied() {
                Some(v) => v,
                None => continue,
            };
            // Is the guarded value defined outside the loop and available at
            // header entry?
            let def_block = match def_map.get(&guarded) {
                Some(&b) => b,
                None => continue, // defined externally / unknown — skip
            };
            let Some(loop_info) = loop_headers.get(&header) else {
                continue;
            };
            if loop_info.body.contains(&def_block) {
                // Defined inside the loop — not hoistable.
                continue;
            }
            if !dominates(def_block, header, &idoms) {
                continue;
            }
            // Hoistable! Record the work.
            hoist_list.push(HoistWork {
                preheader,
                op: op.clone(),
                source_block: *bid,
                source_idx: idx,
            });
        }
    }

    if hoist_list.is_empty() {
        return stats;
    }

    // Apply: remove ops from source blocks (in reverse index order per block),
    // then insert into preheader blocks.

    // Group by source block for efficient removal.
    let mut removals: HashMap<BlockId, Vec<usize>> = HashMap::new();
    let mut inserts: HashMap<BlockId, Vec<TirOp>> = HashMap::new();

    for work in hoist_list {
        removals
            .entry(work.source_block)
            .or_default()
            .push(work.source_idx);
        inserts.entry(work.preheader).or_default().push(work.op);
    }

    // Remove from source blocks (reverse order to keep indices valid).
    for (bid, mut indices) in removals {
        indices.sort_unstable_by(|a, b| b.cmp(a)); // reverse
        indices.dedup();
        if let Some(block) = func.blocks.get_mut(&bid) {
            for idx in &indices {
                block.ops.remove(*idx);
                stats.ops_removed += 1;
            }
        }
    }

    // Insert into preheader blocks (at the end, before the terminator).
    for (bid, ops) in inserts {
        if let Some(block) = func.blocks.get_mut(&bid) {
            for op in ops {
                block.ops.push(op);
                stats.ops_added += 1;
            }
        }
    }

    stats
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use crate::tir::blocks::{Terminator, TirBlock};
    use crate::tir::function::TirFunction;
    use crate::tir::ops::{AttrDict, AttrValue, Dialect, OpCode, TirOp};
    use crate::tir::types::TirType;
    use crate::tir::values::ValueId;

    fn make_op(opcode: OpCode, operands: Vec<ValueId>, results: Vec<ValueId>) -> TirOp {
        TirOp {
            dialect: Dialect::Molt,
            opcode,
            operands,
            results,
            attrs: AttrDict::new(),
            source_span: None,
        }
    }

    fn make_type_guard(operand: ValueId, result: ValueId) -> TirOp {
        let mut attrs = AttrDict::new();
        attrs.insert("ty".to_string(), AttrValue::Str("INT".to_string()));
        TirOp {
            dialect: Dialect::Molt,
            opcode: OpCode::TypeGuard,
            operands: vec![operand],
            results: vec![result],
            attrs,
            source_span: None,
        }
    }

    // -----------------------------------------------------------------------
    // Test 1: TypeGuard with loop-invariant operand → hoisted to preheader
    // -----------------------------------------------------------------------
    #[test]
    fn typeguard_loop_invariant_hoisted() {
        // Structure:
        //   bb0 (entry, preheader): defines %x
        //   bb1 (loop header, back-edge from bb2): TypeGuard(%x) → %ok
        //   bb2 (loop body): uses %ok; loops back to bb1
        //
        // After hoisting: TypeGuard(%x) moves to bb0.
        let mut func = TirFunction::new("f".into(), vec![], TirType::None);

        let x = func.fresh_value();
        let ok = func.fresh_value();

        let loop_header_id = func.fresh_block(); // BlockId(1)
        let loop_body_id = func.fresh_block(); // BlockId(2)

        // Entry (bb0): define x, branch to loop header
        {
            let entry = func.blocks.get_mut(&func.entry_block).unwrap();
            entry.ops.push(make_op(OpCode::ConstInt, vec![], vec![x]));
            entry.terminator = Terminator::Branch {
                target: loop_header_id,
                args: vec![],
            };
        }

        // Loop header (bb1): TypeGuard(%x) → %ok; branch to body
        let header_block = TirBlock {
            id: loop_header_id,
            args: vec![],
            ops: vec![make_type_guard(x, ok)],
            terminator: Terminator::Branch {
                target: loop_body_id,
                args: vec![],
            },
        };
        func.blocks.insert(loop_header_id, header_block);

        // Loop body (bb2): loops back to header (back-edge: 2 → 1)
        let body_block = TirBlock {
            id: loop_body_id,
            args: vec![],
            ops: vec![],
            terminator: Terminator::Branch {
                target: loop_header_id,
                args: vec![],
            },
        };
        func.blocks.insert(loop_body_id, body_block);

        let stats = run(&mut func, &mut crate::tir::analysis::AnalysisManager::new());

        // The TypeGuard should have been moved.
        assert!(
            stats.ops_removed >= 1,
            "expected at least one op removed from loop block"
        );
        assert!(
            stats.ops_added >= 1,
            "expected at least one op added to preheader"
        );

        // TypeGuard should now be in the preheader (bb0), not in bb1.
        let entry_ops = &func.blocks[&func.entry_block].ops;
        assert!(
            entry_ops.iter().any(|op| op.opcode == OpCode::TypeGuard),
            "TypeGuard should be in preheader (bb0)"
        );
        let header_ops = &func.blocks[&loop_header_id].ops;
        assert!(
            !header_ops.iter().any(|op| op.opcode == OpCode::TypeGuard),
            "TypeGuard should NOT remain in loop header (bb1)"
        );
    }

    #[test]
    fn typeguard_hoists_when_latch_id_precedes_header() {
        // Non-monotonic block ids prove the pass is using LoopForest/dominance
        // rather than the legacy ordering heuristic.
        let mut func = TirFunction::new("f".into(), vec![], TirType::None);

        let x = func.fresh_value();
        let ok = func.fresh_value();
        let header = BlockId(20);
        let body = BlockId(5);
        func.next_block = 21;

        {
            let entry = func.blocks.get_mut(&func.entry_block).unwrap();
            entry.ops.push(make_op(OpCode::ConstInt, vec![], vec![x]));
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
                ops: vec![make_type_guard(x, ok)],
                terminator: Terminator::Branch {
                    target: body,
                    args: vec![],
                },
            },
        );
        func.blocks.insert(
            body,
            TirBlock {
                id: body,
                args: vec![],
                ops: vec![],
                terminator: Terminator::Branch {
                    target: header,
                    args: vec![],
                },
            },
        );

        let stats = run(&mut func, &mut crate::tir::analysis::AnalysisManager::new());

        assert_eq!(stats.ops_removed, 1);
        assert_eq!(stats.ops_added, 1);
        assert!(
            func.blocks[&func.entry_block]
                .ops
                .iter()
                .any(|op| op.opcode == OpCode::TypeGuard)
        );
        assert!(
            !func.blocks[&header]
                .ops
                .iter()
                .any(|op| op.opcode == OpCode::TypeGuard)
        );
    }

    // -----------------------------------------------------------------------
    // Test 2: TypeGuard with operand defined inside loop → NOT hoisted
    // -----------------------------------------------------------------------
    #[test]
    fn typeguard_loop_local_not_hoisted() {
        // bb0 (preheader) → bb1 (header) → bb2 (body, defines %y, TypeGuard(%y)) → bb1 (back)
        let mut func = TirFunction::new("f".into(), vec![], TirType::None);

        let y = func.fresh_value();
        let ok = func.fresh_value();

        let loop_header_id = func.fresh_block(); // BlockId(1)
        let loop_body_id = func.fresh_block(); // BlockId(2)

        {
            let entry = func.blocks.get_mut(&func.entry_block).unwrap();
            entry.terminator = Terminator::Branch {
                target: loop_header_id,
                args: vec![],
            };
        }

        // Header: just branches to body
        let header_block = TirBlock {
            id: loop_header_id,
            args: vec![],
            ops: vec![],
            terminator: Terminator::Branch {
                target: loop_body_id,
                args: vec![],
            },
        };
        func.blocks.insert(loop_header_id, header_block);

        // Body: defines %y, TypeGuard(%y), back-edge to header
        let body_block = TirBlock {
            id: loop_body_id,
            args: vec![],
            ops: vec![
                make_op(OpCode::ConstInt, vec![], vec![y]),
                make_type_guard(y, ok),
            ],
            terminator: Terminator::Branch {
                target: loop_header_id,
                args: vec![],
            },
        };
        func.blocks.insert(loop_body_id, body_block);

        let stats = run(&mut func, &mut crate::tir::analysis::AnalysisManager::new());

        // TypeGuard on %y (defined in loop body) must NOT be hoisted.
        assert_eq!(
            stats.ops_removed, 0,
            "should not hoist TypeGuard on loop-local value"
        );
        let body_ops = &func.blocks[&loop_body_id].ops;
        assert!(
            body_ops.iter().any(|op| op.opcode == OpCode::TypeGuard),
            "TypeGuard should remain in loop body"
        );
    }

    // -----------------------------------------------------------------------
    // Test 3: No TypeGuard ops → no changes
    // -----------------------------------------------------------------------
    #[test]
    fn no_typeguard_no_changes() {
        let mut func = TirFunction::new("f".into(), vec![], TirType::None);
        let v = func.fresh_value();

        let entry = func.blocks.get_mut(&func.entry_block).unwrap();
        entry.ops.push(make_op(OpCode::ConstInt, vec![], vec![v]));
        entry.terminator = Terminator::Return { values: vec![v] };

        let stats = run(&mut func, &mut crate::tir::analysis::AnalysisManager::new());
        assert_eq!(stats.ops_removed, 0);
        assert_eq!(stats.ops_added, 0);
    }

    // -----------------------------------------------------------------------
    // Test 4: TypeGuard outside any loop → unchanged
    // -----------------------------------------------------------------------
    #[test]
    fn typeguard_outside_loop_unchanged() {
        let mut func = TirFunction::new("f".into(), vec![], TirType::None);
        let x = func.fresh_value();
        let ok = func.fresh_value();

        let entry = func.blocks.get_mut(&func.entry_block).unwrap();
        entry.ops.push(make_op(OpCode::ConstInt, vec![], vec![x]));
        entry.ops.push(make_type_guard(x, ok));
        entry.terminator = Terminator::Return { values: vec![ok] };

        let stats = run(&mut func, &mut crate::tir::analysis::AnalysisManager::new());
        assert_eq!(stats.ops_removed, 0);
        assert_eq!(stats.ops_added, 0);
        // TypeGuard is still in the entry block
        assert!(
            func.blocks[&func.entry_block]
                .ops
                .iter()
                .any(|op| op.opcode == OpCode::TypeGuard)
        );
    }
}
