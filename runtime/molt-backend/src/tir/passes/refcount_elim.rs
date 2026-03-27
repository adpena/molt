//! Refcount Elimination pass for TIR.
//!
//! Eliminates redundant IncRef/DecRef pairs within basic blocks.
//!
//! Patterns eliminated:
//! 1. Adjacent: IncRef(x); DecRef(x) → both removed
//! 2. Reversed: DecRef(x); IncRef(x) → both removed (ownership transfer)
//! 3. NoEscape: IncRef/DecRef on values classified as StackAlloc → removed
//!    (escape analysis already rewrote Alloc→StackAlloc, this catches remaining refs)
//!
//! Does NOT remove IncRef/DecRef that cross function call boundaries
//! (the callee may store the reference).

use std::collections::HashSet;

use crate::tir::function::TirFunction;
use crate::tir::ops::OpCode;
use crate::tir::values::ValueId;

use super::PassStats;

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
                    .map_or(false, |v| stack_alloc_vals.contains(v))
            {
                remove[i] = true;
            }
        }

        // Step 2b: Find adjacent (or same-direction) IncRef/DecRef pairs on the
        // same value with no intervening call or store between them.
        //
        // We scan forward. When we see an IncRef(x), we look forward for a
        // matching DecRef(x) with no barrier in between, and vice versa.
        //
        // Barriers: Call, CallMethod, CallBuiltin, StoreAttr, StoreIndex
        // (anything that might capture or observe the reference count).

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

            // Scan forward: stop at the first barrier or matching partner.
            // Returns Some(j) if a partner is found before any barrier, None otherwise.
            let partner: Option<usize> = {
                let mut result = None;
                for j in (i + 1)..n {
                    if remove[j] {
                        continue;
                    }
                    let op_j = &block.ops[j];
                    match op_j.opcode {
                        // Barriers: calls and stores that may capture or inspect refcounts.
                        OpCode::Call
                        | OpCode::CallMethod
                        | OpCode::CallBuiltin
                        | OpCode::StoreAttr
                        | OpCode::StoreIndex => {
                            break; // barrier — cannot pair across this
                        }
                        opc if opc == target_opcode => {
                            // Check it's on the same value.
                            if op_j.operands.first().copied() == Some(val_i) {
                                result = Some(j);
                                break;
                            }
                            // Different value — keep scanning.
                        }
                        _ => {}
                    }
                }
                result
            };
            if let Some(j) = partner {
                // Found a pairable IncRef/DecRef — remove both.
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

    stats
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use crate::tir::blocks::Terminator;
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
}
