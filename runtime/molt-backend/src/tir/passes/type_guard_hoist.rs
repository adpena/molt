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
//! ## Simplified loop detection
//!
//! TirBlock does not carry a `loop_depth` field — that information lives in
//! the `CFG` built from `SimpleIR`, which is not available here.  We therefore
//! use a structural approximation:
//!
//! 1. Build a map: ValueId → BlockId (the block that defines the value).
//!    Block arguments are treated as defined in their own block.
//! 2. Detect back-edges by examining the TIR block graph: an edge A → B is a
//!    back-edge if B.0 ≤ A.0 (lower or equal BlockId number).
//! 3. A block B is "inside a loop" if it has at least one back-edge predecessor
//!    (a predecessor with higher BlockId whose terminator targets B).
//! 4. The "preheader" of the loop is the unique predecessor of B that is NOT
//!    a back-edge (i.e., has a lower BlockId and is not the back-edge block).
//! 5. A TypeGuard inside a loop block B is hoistable if:
//!    - Its first operand is defined in a block with strictly lower BlockId
//!      than B (i.e., defined before the loop).
//!    - A unique preheader block exists for B.
//!
//! This approximation is conservative: it may miss some hoisting opportunities
//! in irregular CFGs, but it will never hoist unsafely.

use std::collections::{HashMap, HashSet};

use crate::tir::blocks::{BlockId, Terminator};
use crate::tir::function::TirFunction;
use crate::tir::ops::{OpCode, TirOp};
use crate::tir::values::ValueId;

use super::PassStats;

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

/// Collect successor BlockIds from a terminator.
fn terminator_successors(term: &Terminator) -> Vec<BlockId> {
    match term {
        Terminator::Branch { target, .. } => vec![*target],
        Terminator::CondBranch {
            then_block,
            else_block,
            ..
        } => vec![*then_block, *else_block],
        Terminator::Switch { cases, default, .. } => {
            let mut targets: Vec<BlockId> = cases.iter().map(|(_, t, _)| *t).collect();
            targets.push(*default);
            targets.dedup();
            targets
        }
        Terminator::Return { .. } | Terminator::Unreachable => vec![],
    }
}

/// Build a map: ValueId → BlockId that defines it.
/// Covers both block arguments and op results.
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

/// Build predecessor map: BlockId → Vec<BlockId>.
fn build_pred_map(func: &TirFunction) -> HashMap<BlockId, Vec<BlockId>> {
    let mut pred_map: HashMap<BlockId, Vec<BlockId>> = HashMap::new();
    // Initialise all blocks with empty predecessor lists.
    for &bid in func.blocks.keys() {
        pred_map.entry(bid).or_default();
    }
    for (&bid, block) in &func.blocks {
        for succ in terminator_successors(&block.terminator) {
            pred_map.entry(succ).or_default().push(bid);
        }
    }
    pred_map
}

// ---------------------------------------------------------------------------
// Main pass
// ---------------------------------------------------------------------------

/// Hoist TypeGuard ops out of loops when the guarded value is loop-invariant.
pub fn run(func: &mut TirFunction) -> PassStats {
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

    // Build def map and predecessor map.
    let def_map = build_def_map(func);
    let pred_map = build_pred_map(func);

    // Identify loop headers: a block B is a loop header if it has a predecessor
    // P where P.0 >= B.0 (back-edge by BlockId ordering).
    // For each loop header B, collect:
    //   - back-edge predecessors (P.0 >= B.0)
    //   - non-back-edge predecessors (the preheader candidates)
    struct LoopInfo {
        preheader: Option<BlockId>,
    }

    let mut loop_headers: HashMap<BlockId, LoopInfo> = HashMap::new();

    for (&bid, preds) in &pred_map {
        let back_preds: Vec<BlockId> = preds
            .iter()
            .copied()
            .filter(|p| p.0 >= bid.0)
            .collect();
        if back_preds.is_empty() {
            continue; // not a loop header
        }
        let non_back_preds: Vec<BlockId> = preds
            .iter()
            .copied()
            .filter(|p| p.0 < bid.0)
            .collect();
        // Unique preheader = exactly one non-back predecessor.
        let preheader = if non_back_preds.len() == 1 {
            Some(non_back_preds[0])
        } else {
            None // ambiguous or no preheader — skip hoisting
        };
        loop_headers.insert(bid, LoopInfo { preheader });
    }

    if loop_headers.is_empty() {
        return stats;
    }

    // Identify all blocks that are "inside" any loop.
    // A block B is inside a loop rooted at header H if B's BlockId >= H.0
    // and B can reach H via the back-edge. For the simplified approximation
    // we just consider blocks that are NOT loop headers but are reachable
    // from a back-edge predecessor. We collect the natural loop bodies by
    // BFS backwards from the back-edge predecessor to the header.
    let mut block_to_header: HashMap<BlockId, BlockId> = HashMap::new();

    for (&header, _) in &loop_headers {
        // All back-edge predecessors and blocks reachable from them down to
        // header (exclusive) are in the loop. For our purposes we just mark
        // all blocks with BlockId in [header.0, back_pred.0] as belonging
        // to this loop. This is a safe over-approximation.
        let back_preds: Vec<BlockId> = pred_map[&header]
            .iter()
            .copied()
            .filter(|p| p.0 >= header.0)
            .collect();
        for back_pred in back_preds {
            // BFS backwards from back_pred to header through the pred_map.
            let mut visited: HashSet<BlockId> = HashSet::new();
            let mut worklist = vec![back_pred];
            visited.insert(header); // don't cross the header boundary
            while let Some(node) = worklist.pop() {
                if !visited.insert(node) {
                    continue;
                }
                block_to_header.entry(node).or_insert(header);
                // Walk predecessors that are within the loop (>= header.0).
                for &pred in pred_map.get(&node).map(|v| v.as_slice()).unwrap_or(&[]) {
                    if pred.0 >= header.0 && !visited.contains(&pred) {
                        worklist.push(pred);
                    }
                }
            }
            // The header itself is also in the loop.
            block_to_header.entry(header).or_insert(header);
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
            // Is the guarded value defined outside the loop?
            // "Outside" means defined in a block with BlockId strictly < header.0.
            let def_block = match def_map.get(&guarded) {
                Some(&b) => b,
                None => continue, // defined externally / unknown — skip
            };
            if def_block.0 >= header.0 {
                // Defined inside the loop — not hoistable.
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
        removals.entry(work.source_block).or_default().push(work.source_idx);
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
    use crate::tir::blocks::{TirBlock, Terminator};
    use crate::tir::function::TirFunction;
    use crate::tir::ops::{AttrValue, AttrDict, Dialect, OpCode, TirOp};
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
        let loop_body_id = func.fresh_block();   // BlockId(2)

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

        let stats = run(&mut func);

        // The TypeGuard should have been moved.
        assert!(stats.ops_removed >= 1, "expected at least one op removed from loop block");
        assert!(stats.ops_added >= 1, "expected at least one op added to preheader");

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
        let loop_body_id = func.fresh_block();   // BlockId(2)

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

        let stats = run(&mut func);

        // TypeGuard on %y (defined in loop body) must NOT be hoisted.
        assert_eq!(stats.ops_removed, 0, "should not hoist TypeGuard on loop-local value");
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

        let stats = run(&mut func);
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

        let stats = run(&mut func);
        assert_eq!(stats.ops_removed, 0);
        assert_eq!(stats.ops_added, 0);
        // TypeGuard is still in the entry block
        assert!(func.blocks[&func.entry_block]
            .ops
            .iter()
            .any(|op| op.opcode == OpCode::TypeGuard));
    }
}
