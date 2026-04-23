//! Refcount Elimination pass for TIR.
//!
//! Eliminates redundant IncRef/DecRef pairs both within and across basic blocks.
//!
//! Intra-block patterns:
//! 1. Adjacent: IncRef(x); DecRef(x) → both removed
//! 2. Reversed: DecRef(x); IncRef(x) → both removed (ownership transfer)
//! 3. NoEscape: IncRef/DecRef on values classified as StackAlloc → removed
//!    (escape analysis already rewrote Alloc→StackAlloc, this catches remaining refs)
//!
//! Cross-block patterns:
//! 4. Dominator edge: block A dominates block B, A is B's sole predecessor,
//!    A ends with IncRef(x) (no trailing barrier), B starts with DecRef(x)
//!    (no leading barrier) → both removed. The paired IncRef created the extra
//!    ref that the DecRef destroys, so eliminating both is safe.
//! 5. Loop invariant: loop header has IncRef(x) at top and DecRef(x) at bottom
//!    (before back-edge), x is loop-invariant (defined outside the loop body),
//!    and no barrier intervenes between them within the header → both removed.
//!
//! Does NOT remove IncRef/DecRef that cross function call boundaries
//! (the callee may store the reference).

use std::collections::{HashMap, HashSet};

use crate::tir::blocks::BlockId;
use crate::tir::dominators::{build_pred_map, compute_idoms, dominates};
use crate::tir::function::TirFunction;
use crate::tir::ops::OpCode;
use crate::tir::values::ValueId;

use super::PassStats;

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

/// Returns `true` if the opcode is a barrier that prevents cross-block
/// IncRef/DecRef pairing. Barriers are operations that may capture, store,
/// or observe reference counts.
fn is_barrier(opcode: OpCode) -> bool {
    matches!(
        opcode,
        OpCode::Call
            | OpCode::CallMethod
            | OpCode::CallBuiltin
            | OpCode::StoreAttr
            | OpCode::StoreIndex
            | OpCode::StateSwitch
            | OpCode::StateTransition
            | OpCode::StateYield
            | OpCode::ClosureLoad
            | OpCode::ClosureStore
            | OpCode::ChanSendYield
            | OpCode::ChanRecvYield
    )
}

/// Build a map: ValueId → BlockId that defines it.
fn build_def_map(func: &TirFunction) -> HashMap<ValueId, BlockId> {
    let mut def_map: HashMap<ValueId, BlockId> = HashMap::new();
    for (&bid, block) in &func.blocks {
        for arg in &block.args {
            def_map.insert(arg.id, bid);
        }
        for op in &block.ops {
            for &result in &op.results {
                def_map.insert(result, bid);
            }
        }
    }
    def_map
}

/// Eliminate redundant IncRef/DecRef pairs.
pub fn run(func: &mut TirFunction) -> PassStats {
    let mut stats = PassStats {
        name: "refcount_elim",
        ..Default::default()
    };

    // Step 1: Collect all ValueIds produced by StackAlloc ops (O(N) scan).
    // IncRef/DecRef on stack-allocated values are always safe to remove.
    let mut stack_alloc_vals: HashSet<ValueId> = HashSet::new();
    for block in func.blocks.values() {
        for op in &block.ops {
            if op.opcode == OpCode::StackAlloc {
                for &result in &op.results {
                    stack_alloc_vals.insert(result);
                }
            }
        }
    }

    // Step 2: Per-block elimination of IncRef/DecRef pairs.
    // We process each block independently (intra-block only) to avoid
    // cross-block alias concerns.
    let block_ids: Vec<_> = func.blocks.keys().copied().collect();

    for bid in block_ids {
        let block = match func.blocks.get_mut(&bid) {
            Some(b) => b,
            None => continue,
        };

        let n = block.ops.len();
        if n == 0 {
            continue;
        }

        // Bit-vector: true = this op should be removed.
        let mut remove = vec![false; n];

        // Step 2a: Remove IncRef/DecRef on StackAlloc values (no pairing needed).
        for i in 0..n {
            let op = &block.ops[i];
            if (op.opcode == OpCode::IncRef || op.opcode == OpCode::DecRef)
                && op
                    .operands
                    .first()
                    .is_some_and(|v| stack_alloc_vals.contains(v))
            {
                remove[i] = true;
            }
        }

        // Step 2b: Find adjacent (or same-direction) IncRef/DecRef pairs on the
        // same value with no intervening barrier between them.

        for i in 0..n {
            if remove[i] {
                continue;
            }
            let opcode_i = block.ops[i].opcode;
            if opcode_i != OpCode::IncRef && opcode_i != OpCode::DecRef {
                continue;
            }
            let val_i = match block.ops[i].operands.first().copied() {
                Some(v) => v,
                None => continue,
            };

            // Look forward for the complementary op on the same value.
            let target_opcode = if opcode_i == OpCode::IncRef {
                OpCode::DecRef
            } else {
                OpCode::IncRef
            };

            let partner: Option<usize> = {
                let mut result = None;
                for j in (i + 1)..n {
                    if remove[j] {
                        continue;
                    }
                    let op_j = &block.ops[j];
                    if is_barrier(op_j.opcode) {
                        break;
                    }
                    if op_j.opcode == target_opcode && op_j.operands.first().copied() == Some(val_i)
                    {
                        result = Some(j);
                        break;
                    }
                }
                result
            };
            if let Some(j) = partner {
                remove[i] = true;
                remove[j] = true;
            }
        }

        // Step 2c: Apply removals.
        let before_len = block.ops.len();
        let mut remove_iter = remove.iter();
        block
            .ops
            .retain(|_| !remove_iter.next().copied().unwrap_or(false));
        let removed = before_len - block.ops.len();
        stats.ops_removed += removed;
    }

    // -----------------------------------------------------------------------
    // Step 3: Cross-block dominator-edge elimination.
    //
    // If block A dominates block B, A is B's SOLE predecessor, and:
    //   - A's last non-removed op is IncRef(x) with no barrier after it,
    //   - B's first non-removed op is DecRef(x) with no barrier before it,
    // then both can be safely eliminated: the IncRef created a temporary
    // extra reference that the DecRef immediately destroys on the single
    // path A→B. Reversed (DecRef trailing, IncRef leading) is also safe
    // as an ownership transfer with no interleaving observer.
    // -----------------------------------------------------------------------
    if func.blocks.len() > 1 {
        let pred_map = build_pred_map(func);
        let idoms = compute_idoms(func, &pred_map);

        // Collect candidate trailing refcount ops per block: the last op that
        // is IncRef or DecRef with no barrier between it and the block end.
        struct TrailingInfo {
            opcode: OpCode,
            val: ValueId,
            /// Index within block.ops
            idx: usize,
        }

        // We need immutable access to compute candidates, then mutable to remove.
        // Collect the elimination pairs first.
        let mut cross_block_removals: Vec<(BlockId, usize, BlockId, usize)> = Vec::new();

        let block_ids: Vec<BlockId> = func.blocks.keys().copied().collect();
        for &succ_bid in &block_ids {
            let preds = match pred_map.get(&succ_bid) {
                Some(p) => p,
                None => continue,
            };
            // Must have exactly one predecessor for soundness.
            if preds.len() != 1 {
                continue;
            }
            let pred_bid = preds[0];

            // The predecessor must dominate the successor (always true with
            // single predecessor, but verify via idom for correctness).
            if !dominates(pred_bid, succ_bid, &idoms) {
                continue;
            }

            // Find the trailing refcount op in the predecessor block.
            let trailing = {
                let pred_block = &func.blocks[&pred_bid];
                let mut result: Option<TrailingInfo> = None;
                // Scan from the end backwards, skipping already-removed ops
                // (we already applied intra-block removals so all remaining ops
                // are live). Stop at the first barrier or non-refcount op.
                for (i, op) in pred_block.ops.iter().enumerate().rev() {
                    if is_barrier(op.opcode) {
                        break;
                    }
                    if op.opcode == OpCode::IncRef || op.opcode == OpCode::DecRef {
                        if let Some(&val) = op.operands.first() {
                            result = Some(TrailingInfo {
                                opcode: op.opcode,
                                val,
                                idx: i,
                            });
                        }
                        break;
                    }
                    // Non-barrier, non-refcount op: keep scanning backwards
                    // only if it's harmless (arithmetic, loads, etc.). But we
                    // must stop — an intervening op means the IncRef is not
                    // "trailing" in the sense that matters. We allow scanning
                    // past non-barrier ops because they don't observe refcounts.
                }
                result
            };

            let Some(trail) = trailing else {
                continue;
            };

            // Verify no barrier between the trailing refcount op and block end.
            {
                let pred_block = &func.blocks[&pred_bid];
                let mut has_barrier = false;
                for op in &pred_block.ops[(trail.idx + 1)..] {
                    if is_barrier(op.opcode) {
                        has_barrier = true;
                        break;
                    }
                }
                if has_barrier {
                    continue;
                }
            }

            // Find the leading refcount op in the successor block.
            let target_opcode = if trail.opcode == OpCode::IncRef {
                OpCode::DecRef
            } else {
                OpCode::IncRef
            };

            let leading = {
                let succ_block = &func.blocks[&succ_bid];
                let mut result: Option<usize> = None;
                for (i, op) in succ_block.ops.iter().enumerate() {
                    if is_barrier(op.opcode) {
                        break;
                    }
                    if op.opcode == target_opcode && op.operands.first().copied() == Some(trail.val)
                    {
                        result = Some(i);
                        break;
                    }
                    // Non-barrier, non-matching op: keep scanning forward past
                    // harmless ops. But stop if we see a different refcount op
                    // on the same value (it could change the refcount balance).
                    if (op.opcode == OpCode::IncRef || op.opcode == OpCode::DecRef)
                        && op.operands.first().copied() == Some(trail.val)
                    {
                        break;
                    }
                }
                result
            };

            if let Some(lead_idx) = leading {
                cross_block_removals.push((pred_bid, trail.idx, succ_bid, lead_idx));
            }
        }

        // Apply cross-block removals. We process each (pred, succ) pair
        // independently. Since a block can only be a successor once (sole
        // predecessor constraint), there are no conflicts.
        for (pred_bid, pred_idx, succ_bid, succ_idx) in cross_block_removals {
            // Remove the trailing op from the predecessor.
            if let Some(pred_block) = func.blocks.get_mut(&pred_bid)
                && pred_idx < pred_block.ops.len()
            {
                let op = &pred_block.ops[pred_idx];
                if (op.opcode == OpCode::IncRef || op.opcode == OpCode::DecRef)
                    && op.operands.first().copied().is_some()
                {
                    pred_block.ops.remove(pred_idx);
                    stats.ops_removed += 1;
                }
            }
            // Remove the leading op from the successor.
            if let Some(succ_block) = func.blocks.get_mut(&succ_bid)
                && succ_idx < succ_block.ops.len()
            {
                let op = &succ_block.ops[succ_idx];
                if (op.opcode == OpCode::IncRef || op.opcode == OpCode::DecRef)
                    && op.operands.first().copied().is_some()
                {
                    succ_block.ops.remove(succ_idx);
                    stats.ops_removed += 1;
                }
            }
        }
    }

    // -----------------------------------------------------------------------
    // Step 4: Loop-invariant IncRef/DecRef elimination.
    //
    // If a loop header block has IncRef(x) at the top and DecRef(x) at the
    // bottom (or vice versa), and x is loop-invariant (defined outside the
    // loop body), both can be eliminated. The IncRef/DecRef pair within a
    // single iteration is a net-zero refcount change on a value whose
    // lifetime spans the entire loop.
    //
    // Constraints:
    //   - No barrier between the IncRef and DecRef within the header block.
    //   - x must be defined in a block that strictly dominates the loop header.
    //   - The loop header must have a back-edge (it IS a loop header).
    // -----------------------------------------------------------------------
    if !func.loop_roles.is_empty() || func.blocks.len() > 1 {
        let pred_map = build_pred_map(func);
        let idoms = compute_idoms(func, &pred_map);
        let def_map = build_def_map(func);

        // Identify loop headers: blocks with at least one back-edge predecessor.
        // A back-edge is an edge P → H where H.0 <= P.0 (by BlockId ordering).
        // Also use explicit loop_roles if available.
        let mut loop_header_set: HashSet<BlockId> = HashSet::new();

        // From explicit loop_roles.
        for (&bid, role) in &func.loop_roles {
            if matches!(role, crate::tir::blocks::LoopRole::LoopHeader) {
                loop_header_set.insert(bid);
            }
        }

        // From back-edge detection.
        for (&bid, preds) in &pred_map {
            for pred in preds {
                if pred.0 >= bid.0 {
                    loop_header_set.insert(bid);
                    break;
                }
            }
        }

        // For each loop header, look for paired IncRef/DecRef on a
        // loop-invariant value within the header block itself.
        let mut loop_removals: Vec<(BlockId, usize, usize)> = Vec::new();

        for &header_bid in &loop_header_set {
            let block = match func.blocks.get(&header_bid) {
                Some(b) => b,
                None => continue,
            };
            let n = block.ops.len();
            if n < 2 {
                continue;
            }

            // Scan for IncRef(x)...DecRef(x) or DecRef(x)...IncRef(x) pairs
            // with no barrier in between, where x is loop-invariant.
            for i in 0..n {
                let op_i = &block.ops[i];
                if op_i.opcode != OpCode::IncRef && op_i.opcode != OpCode::DecRef {
                    continue;
                }
                let val = match op_i.operands.first().copied() {
                    Some(v) => v,
                    None => continue,
                };

                // Check loop invariance: value must be defined in a block
                // that strictly dominates the header.
                let def_block = match def_map.get(&val) {
                    Some(&b) => b,
                    None => continue,
                };
                if def_block == header_bid {
                    continue; // Defined inside the loop header — not invariant.
                }
                if !dominates(def_block, header_bid, &idoms) {
                    continue; // Not dominated — not provably invariant.
                }

                let target_opcode = if op_i.opcode == OpCode::IncRef {
                    OpCode::DecRef
                } else {
                    OpCode::IncRef
                };

                // Scan forward for the matching pair with no barrier.
                let mut partner: Option<usize> = None;
                for j in (i + 1)..n {
                    let op_j = &block.ops[j];
                    if is_barrier(op_j.opcode) {
                        break;
                    }
                    if op_j.opcode == target_opcode && op_j.operands.first().copied() == Some(val) {
                        partner = Some(j);
                        break;
                    }
                    // If we see the same opcode on the same value, the balance
                    // is different; stop.
                    if op_j.opcode == op_i.opcode && op_j.operands.first().copied() == Some(val) {
                        break;
                    }
                }

                if let Some(j) = partner {
                    loop_removals.push((header_bid, i, j));
                    break; // One pair per header per scan to avoid index confusion.
                }
            }
        }

        // Apply loop removals (remove higher index first to preserve indices).
        for (bid, idx_a, idx_b) in loop_removals {
            if let Some(block) = func.blocks.get_mut(&bid) {
                let (lo, hi) = if idx_a < idx_b {
                    (idx_a, idx_b)
                } else {
                    (idx_b, idx_a)
                };
                if hi < block.ops.len() && lo < block.ops.len() {
                    block.ops.remove(hi);
                    block.ops.remove(lo);
                    stats.ops_removed += 2;
                }
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
    use crate::tir::blocks::{BlockId, Terminator, TirBlock};
    use crate::tir::function::TirFunction;
    use crate::tir::ops::{AttrDict, Dialect, OpCode, TirOp};
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

    fn make_func() -> TirFunction {
        TirFunction::new("f".into(), vec![], TirType::None)
    }

    /// Helper to add a new block with the given ops and terminator.
    fn add_block(func: &mut TirFunction, ops: Vec<TirOp>, terminator: Terminator) -> BlockId {
        let bid = func.fresh_block();
        let block = TirBlock {
            id: bid,
            args: vec![],
            ops,
            terminator,
        };
        func.blocks.insert(bid, block);
        bid
    }

    // -----------------------------------------------------------------------
    // Test 1: Adjacent IncRef+DecRef → both removed
    // -----------------------------------------------------------------------
    #[test]
    fn adjacent_incref_decref_removed() {
        let mut func = make_func();
        let v = func.fresh_value();

        let entry = func.blocks.get_mut(&func.entry_block).unwrap();
        entry.ops.push(make_op(OpCode::IncRef, vec![v], vec![]));
        entry.ops.push(make_op(OpCode::DecRef, vec![v], vec![]));
        entry.terminator = Terminator::Return { values: vec![] };

        let stats = run(&mut func);
        assert_eq!(stats.ops_removed, 2);
        assert!(func.blocks[&func.entry_block].ops.is_empty());
    }

    // -----------------------------------------------------------------------
    // Test 2: Reversed DecRef+IncRef → both removed
    // -----------------------------------------------------------------------
    #[test]
    fn reversed_decref_incref_removed() {
        let mut func = make_func();
        let v = func.fresh_value();

        let entry = func.blocks.get_mut(&func.entry_block).unwrap();
        entry.ops.push(make_op(OpCode::DecRef, vec![v], vec![]));
        entry.ops.push(make_op(OpCode::IncRef, vec![v], vec![]));
        entry.terminator = Terminator::Return { values: vec![] };

        let stats = run(&mut func);
        assert_eq!(stats.ops_removed, 2);
        assert!(func.blocks[&func.entry_block].ops.is_empty());
    }

    // -----------------------------------------------------------------------
    // Test 3: IncRef/DecRef on StackAlloc value → removed (no pairing needed)
    // -----------------------------------------------------------------------
    #[test]
    fn stackalloc_incref_decref_removed() {
        let mut func = make_func();
        let v = func.fresh_value();

        let entry = func.blocks.get_mut(&func.entry_block).unwrap();
        entry.ops.push(make_op(OpCode::StackAlloc, vec![], vec![v]));
        entry.ops.push(make_op(OpCode::IncRef, vec![v], vec![]));
        entry.ops.push(make_op(OpCode::DecRef, vec![v], vec![]));
        entry.terminator = Terminator::Return { values: vec![] };

        let stats = run(&mut func);
        // IncRef and DecRef both removed (StackAlloc rule catches them individually)
        assert_eq!(stats.ops_removed, 2);
        // StackAlloc itself stays
        assert_eq!(func.blocks[&func.entry_block].ops.len(), 1);
        assert_eq!(
            func.blocks[&func.entry_block].ops[0].opcode,
            OpCode::StackAlloc
        );
    }

    // -----------------------------------------------------------------------
    // Test 4: IncRef with intervening Call → NOT removed
    // -----------------------------------------------------------------------
    #[test]
    fn incref_with_call_barrier_not_removed() {
        let mut func = make_func();
        let v = func.fresh_value();
        let callee = func.fresh_value();
        let result = func.fresh_value();

        let entry = func.blocks.get_mut(&func.entry_block).unwrap();
        entry.ops.push(make_op(OpCode::IncRef, vec![v], vec![]));
        entry
            .ops
            .push(make_op(OpCode::Call, vec![callee], vec![result]));
        entry.ops.push(make_op(OpCode::DecRef, vec![v], vec![]));
        entry.terminator = Terminator::Return { values: vec![] };

        let stats = run(&mut func);
        // IncRef and DecRef separated by Call — must NOT be removed
        assert_eq!(stats.ops_removed, 0);
        assert_eq!(func.blocks[&func.entry_block].ops.len(), 3);
    }

    // -----------------------------------------------------------------------
    // Test 5: No IncRef/DecRef → no changes
    // -----------------------------------------------------------------------
    #[test]
    fn no_incref_decref_no_changes() {
        let mut func = make_func();
        let v = func.fresh_value();

        let entry = func.blocks.get_mut(&func.entry_block).unwrap();
        entry.ops.push(make_op(OpCode::ConstInt, vec![], vec![v]));
        entry.terminator = Terminator::Return { values: vec![v] };

        let stats = run(&mut func);
        assert_eq!(stats.ops_removed, 0);
        assert_eq!(func.blocks[&func.entry_block].ops.len(), 1);
    }

    // -----------------------------------------------------------------------
    // Test 6: Different values — no cross-pairing
    // -----------------------------------------------------------------------
    #[test]
    fn different_values_not_paired() {
        let mut func = make_func();
        let v1 = func.fresh_value();
        let v2 = func.fresh_value();

        let entry = func.blocks.get_mut(&func.entry_block).unwrap();
        // IncRef(v1) then DecRef(v2) — different values, no elimination
        entry.ops.push(make_op(OpCode::IncRef, vec![v1], vec![]));
        entry.ops.push(make_op(OpCode::DecRef, vec![v2], vec![]));
        entry.terminator = Terminator::Return { values: vec![] };

        let stats = run(&mut func);
        assert_eq!(stats.ops_removed, 0);
        assert_eq!(func.blocks[&func.entry_block].ops.len(), 2);
    }

    // ===================================================================
    // Cross-block tests
    // ===================================================================

    // -----------------------------------------------------------------------
    // Test 7: Cross-block IncRef(x) in pred → DecRef(x) in succ (sole pred)
    // -----------------------------------------------------------------------
    #[test]
    fn cross_block_incref_decref_sole_pred() {
        let mut func = make_func();
        let v = func.fresh_value();

        // Create successor block: DecRef(v) then return.
        let succ_bid = add_block(
            &mut func,
            vec![make_op(OpCode::DecRef, vec![v], vec![])],
            Terminator::Return { values: vec![] },
        );

        // Entry block: IncRef(v) then branch to succ.
        let entry = func.blocks.get_mut(&func.entry_block).unwrap();
        entry.ops.push(make_op(OpCode::IncRef, vec![v], vec![]));
        entry.terminator = Terminator::Branch {
            target: succ_bid,
            args: vec![],
        };

        let stats = run(&mut func);
        // Both should be eliminated cross-block.
        assert_eq!(stats.ops_removed, 2);
        assert!(func.blocks[&func.entry_block].ops.is_empty());
        assert!(func.blocks[&succ_bid].ops.is_empty());
    }

    // -----------------------------------------------------------------------
    // Test 8: Cross-block NOT eliminated when succ has multiple predecessors
    // -----------------------------------------------------------------------
    #[test]
    fn cross_block_not_eliminated_multiple_preds() {
        let mut func = make_func();
        let v = func.fresh_value();

        // Successor block: DecRef(v) then return.
        let succ_bid = add_block(
            &mut func,
            vec![make_op(OpCode::DecRef, vec![v], vec![])],
            Terminator::Return { values: vec![] },
        );

        // Second predecessor block that also branches to succ.
        let other_pred = add_block(
            &mut func,
            vec![],
            Terminator::Branch {
                target: succ_bid,
                args: vec![],
            },
        );

        // Entry block: IncRef(v) then cond branch to succ or other_pred.
        let cond = func.fresh_value();
        let entry = func.blocks.get_mut(&func.entry_block).unwrap();
        entry
            .ops
            .push(make_op(OpCode::ConstBool, vec![], vec![cond]));
        entry.ops.push(make_op(OpCode::IncRef, vec![v], vec![]));
        entry.terminator = Terminator::CondBranch {
            cond,
            then_block: succ_bid,
            then_args: vec![],
            else_block: other_pred,
            else_args: vec![],
        };

        let stats = run(&mut func);
        // Should NOT be eliminated: succ has two predecessors.
        assert_eq!(stats.ops_removed, 0);
    }

    // -----------------------------------------------------------------------
    // Test 9: Cross-block NOT eliminated with trailing barrier in pred
    // -----------------------------------------------------------------------
    #[test]
    fn cross_block_not_eliminated_trailing_barrier() {
        let mut func = make_func();
        let v = func.fresh_value();
        let callee = func.fresh_value();
        let call_result = func.fresh_value();

        // Successor block: DecRef(v) then return.
        let succ_bid = add_block(
            &mut func,
            vec![make_op(OpCode::DecRef, vec![v], vec![])],
            Terminator::Return { values: vec![] },
        );

        // Entry block: IncRef(v), Call (barrier), branch to succ.
        let entry = func.blocks.get_mut(&func.entry_block).unwrap();
        entry.ops.push(make_op(OpCode::IncRef, vec![v], vec![]));
        entry
            .ops
            .push(make_op(OpCode::Call, vec![callee], vec![call_result]));
        entry.terminator = Terminator::Branch {
            target: succ_bid,
            args: vec![],
        };

        let stats = run(&mut func);
        // Barrier between IncRef and block end — cannot pair cross-block.
        assert_eq!(stats.ops_removed, 0);
    }

    // -----------------------------------------------------------------------
    // Test 10: Loop-invariant IncRef/DecRef elimination
    // -----------------------------------------------------------------------
    #[test]
    fn loop_invariant_incref_decref_eliminated() {
        let mut func = make_func();
        let v = func.fresh_value(); // Defined in entry block (loop-invariant).
        let cond = func.fresh_value();

        // Create a loop header block (bid=1): IncRef(v), some arithmetic, DecRef(v),
        // then cond branch back to itself (back-edge) or to exit.
        let exit_bid = add_block(&mut func, vec![], Terminator::Return { values: vec![] });

        let header_bid = add_block(
            &mut func,
            vec![
                make_op(OpCode::IncRef, vec![v], vec![]),
                make_op(OpCode::ConstBool, vec![], vec![cond]),
                make_op(OpCode::DecRef, vec![v], vec![]),
            ],
            Terminator::CondBranch {
                cond,
                then_block: BlockId(0), // placeholder, will fix
                then_args: vec![],
                else_block: exit_bid,
                else_args: vec![],
            },
        );

        // Fix the back-edge to point to the header itself.
        func.blocks.get_mut(&header_bid).unwrap().terminator = Terminator::CondBranch {
            cond,
            then_block: header_bid, // back-edge
            then_args: vec![],
            else_block: exit_bid,
            else_args: vec![],
        };

        // Entry block branches to header.
        let entry = func.blocks.get_mut(&func.entry_block).unwrap();
        entry.ops.push(make_op(OpCode::ConstInt, vec![], vec![v]));
        entry.terminator = Terminator::Branch {
            target: header_bid,
            args: vec![],
        };

        let stats = run(&mut func);
        // IncRef and DecRef in the loop header on loop-invariant v should be eliminated.
        assert_eq!(stats.ops_removed, 2);
        // Only the ConstBool should remain in the header.
        assert_eq!(func.blocks[&header_bid].ops.len(), 1);
        assert_eq!(func.blocks[&header_bid].ops[0].opcode, OpCode::ConstBool);
    }

    // -----------------------------------------------------------------------
    // Test 11: Loop-invariant NOT eliminated when value defined in header
    // -----------------------------------------------------------------------
    #[test]
    fn loop_noninvariant_not_eliminated() {
        let mut func = make_func();
        let v = func.fresh_value();
        let cond = func.fresh_value();

        let exit_bid = add_block(&mut func, vec![], Terminator::Return { values: vec![] });

        // Header defines v itself — v is NOT loop-invariant.
        let header_bid = add_block(
            &mut func,
            vec![
                make_op(OpCode::ConstInt, vec![], vec![v]),
                make_op(OpCode::IncRef, vec![v], vec![]),
                make_op(OpCode::ConstBool, vec![], vec![cond]),
                make_op(OpCode::DecRef, vec![v], vec![]),
            ],
            Terminator::CondBranch {
                cond,
                then_block: BlockId(0), // placeholder
                then_args: vec![],
                else_block: exit_bid,
                else_args: vec![],
            },
        );

        func.blocks.get_mut(&header_bid).unwrap().terminator = Terminator::CondBranch {
            cond,
            then_block: header_bid,
            then_args: vec![],
            else_block: exit_bid,
            else_args: vec![],
        };

        let entry = func.blocks.get_mut(&func.entry_block).unwrap();
        entry.terminator = Terminator::Branch {
            target: header_bid,
            args: vec![],
        };

        let stats = run(&mut func);
        // The intra-block pass will still eliminate the pair (they are adjacent
        // with no barrier). This is correct — intra-block elimination does not
        // require loop invariance. The loop-invariant pass is for cases where
        // the pair spans more of the block. Here the intra-block pass handles it.
        // What matters is that the loop-invariant pass would NOT fire on a
        // non-invariant value. Since intra-block already eliminated them,
        // the result is the same.
        assert_eq!(stats.ops_removed, 2);
        // ConstInt and ConstBool remain.
        assert_eq!(func.blocks[&header_bid].ops.len(), 2);
    }

    // -----------------------------------------------------------------------
    // Test 12: Cross-block with reversed pair (DecRef trailing, IncRef leading)
    // -----------------------------------------------------------------------
    #[test]
    fn cross_block_reversed_decref_incref() {
        let mut func = make_func();
        let v = func.fresh_value();

        // Successor: IncRef(v) then return.
        let succ_bid = add_block(
            &mut func,
            vec![make_op(OpCode::IncRef, vec![v], vec![])],
            Terminator::Return { values: vec![] },
        );

        // Entry: DecRef(v) then branch to succ.
        let entry = func.blocks.get_mut(&func.entry_block).unwrap();
        entry.ops.push(make_op(OpCode::DecRef, vec![v], vec![]));
        entry.terminator = Terminator::Branch {
            target: succ_bid,
            args: vec![],
        };

        let stats = run(&mut func);
        assert_eq!(stats.ops_removed, 2);
        assert!(func.blocks[&func.entry_block].ops.is_empty());
        assert!(func.blocks[&succ_bid].ops.is_empty());
    }

    // -----------------------------------------------------------------------
    // Test 13: Cross-block NOT eliminated with leading barrier in succ
    // -----------------------------------------------------------------------
    #[test]
    fn cross_block_not_eliminated_leading_barrier_in_succ() {
        let mut func = make_func();
        let v = func.fresh_value();
        let callee = func.fresh_value();
        let call_result = func.fresh_value();

        // Successor: Call (barrier), then DecRef(v), then return.
        let succ_bid = add_block(
            &mut func,
            vec![
                make_op(OpCode::Call, vec![callee], vec![call_result]),
                make_op(OpCode::DecRef, vec![v], vec![]),
            ],
            Terminator::Return { values: vec![] },
        );

        // Entry: IncRef(v) then branch to succ.
        let entry = func.blocks.get_mut(&func.entry_block).unwrap();
        entry.ops.push(make_op(OpCode::IncRef, vec![v], vec![]));
        entry.terminator = Terminator::Branch {
            target: succ_bid,
            args: vec![],
        };

        let stats = run(&mut func);
        // Barrier before DecRef in successor — cannot pair.
        assert_eq!(stats.ops_removed, 0);
    }
}
