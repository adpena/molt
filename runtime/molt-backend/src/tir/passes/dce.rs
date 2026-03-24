//! Dead Code Elimination (DCE) pass for TIR.
//!
//! Removes operations whose results are never used by any other op or
//! terminator, provided those operations are free of side effects.
//! Iterates to a fixpoint (at most 10 rounds) to handle cascading removals.
//! Also removes blocks that are unreachable (no predecessors, excluding entry).

use std::collections::{HashMap, HashSet};

use crate::tir::blocks::{BlockId, Terminator};
use crate::tir::function::TirFunction;
use crate::tir::ops::OpCode;
use crate::tir::values::ValueId;

use super::PassStats;

// ---------------------------------------------------------------------------
// Side-effect classification
// ---------------------------------------------------------------------------

/// Returns `true` if an op with this opcode must be preserved even when all
/// of its results are dead.
#[inline]
fn is_side_effecting(opcode: OpCode) -> bool {
    matches!(
        opcode,
        // Calls — may have arbitrary side effects.
        OpCode::Call
        | OpCode::CallMethod
        | OpCode::CallBuiltin
        // Store/delete mutations.
        | OpCode::StoreAttr
        | OpCode::StoreIndex
        | OpCode::DelAttr
        | OpCode::DelIndex
        // Control flow / exception handling.
        | OpCode::Raise
        | OpCode::CheckException
        | OpCode::TryStart
        | OpCode::TryEnd
        | OpCode::StateBlockStart
        | OpCode::StateBlockEnd
        // Generator protocol.
        | OpCode::Yield
        | OpCode::YieldFrom
        // Reference-counting and memory management.
        | OpCode::IncRef
        | OpCode::DecRef
        | OpCode::Free
        // Allocation may trigger a finalizer / GC hook.
        | OpCode::Alloc
        // Import has module-level side effects.
        | OpCode::Import
        | OpCode::ImportFrom
        // Deoptimisation must not be silently dropped.
        | OpCode::Deopt
    )
}

/// Returns `true` if the op may throw an exception.  Used when
/// `has_exception_handling` is set to conservatively keep all
/// potentially-throwing ops alive inside try regions.
#[inline]
fn is_potentially_throwing(opcode: OpCode) -> bool {
    matches!(
        opcode,
        OpCode::Call
        | OpCode::CallMethod
        | OpCode::CallBuiltin
        | OpCode::Raise
        | OpCode::Index
        | OpCode::StoreIndex
        | OpCode::LoadAttr
        | OpCode::StoreAttr
        | OpCode::DelAttr
        | OpCode::DelIndex
        | OpCode::Import
        | OpCode::ImportFrom
        | OpCode::Div
        | OpCode::FloorDiv
        | OpCode::Mod
        | OpCode::GetIter
        | OpCode::IterNext
        | OpCode::ForIter
    )
}

// ---------------------------------------------------------------------------
// Use-count helpers
// ---------------------------------------------------------------------------

/// Increment the use-count of every ValueId mentioned in a terminator.
fn count_terminator_uses(term: &Terminator, uses: &mut HashMap<ValueId, usize>) {
    match term {
        Terminator::Branch { args, .. } => {
            for v in args {
                *uses.entry(*v).or_insert(0) += 1;
            }
        }
        Terminator::CondBranch {
            cond,
            then_args,
            else_args,
            ..
        } => {
            *uses.entry(*cond).or_insert(0) += 1;
            for v in then_args {
                *uses.entry(*v).or_insert(0) += 1;
            }
            for v in else_args {
                *uses.entry(*v).or_insert(0) += 1;
            }
        }
        Terminator::Switch {
            value,
            cases,
            default_args,
            ..
        } => {
            *uses.entry(*value).or_insert(0) += 1;
            for (_, _, args) in cases {
                for v in args {
                    *uses.entry(*v).or_insert(0) += 1;
                }
            }
            for v in default_args {
                *uses.entry(*v).or_insert(0) += 1;
            }
        }
        Terminator::Return { values } => {
            for v in values {
                *uses.entry(*v).or_insert(0) += 1;
            }
        }
        Terminator::Unreachable => {}
    }
}

/// Build a full use-count map from all ops and terminators in the function.
fn build_use_counts(func: &TirFunction) -> HashMap<ValueId, usize> {
    let mut uses: HashMap<ValueId, usize> = HashMap::new();
    for block in func.blocks.values() {
        for op in &block.ops {
            for v in &op.operands {
                *uses.entry(*v).or_insert(0) += 1;
            }
        }
        count_terminator_uses(&block.terminator, &mut uses);
    }
    uses
}

// ---------------------------------------------------------------------------
// Reachability
// ---------------------------------------------------------------------------

/// Collect the set of reachable BlockIds via DFS from the entry block.
fn reachable_blocks(func: &TirFunction) -> HashSet<BlockId> {
    let mut visited: HashSet<BlockId> = HashSet::new();
    let mut stack: Vec<BlockId> = vec![func.entry_block];

    while let Some(id) = stack.pop() {
        if !visited.insert(id) {
            continue;
        }
        if let Some(block) = func.blocks.get(&id) {
            match &block.terminator {
                Terminator::Branch { target, .. } => {
                    stack.push(*target);
                }
                Terminator::CondBranch {
                    then_block,
                    else_block,
                    ..
                } => {
                    stack.push(*then_block);
                    stack.push(*else_block);
                }
                Terminator::Switch {
                    cases, default, ..
                } => {
                    stack.push(*default);
                    for (_, target, _) in cases {
                        stack.push(*target);
                    }
                }
                Terminator::Return { .. } | Terminator::Unreachable => {}
            }
        }
    }
    visited
}

// ---------------------------------------------------------------------------
// Main pass
// ---------------------------------------------------------------------------

/// Remove dead operations (and unreachable blocks) from `func`.
///
/// An operation is dead when:
///   - all of its result values have use-count 0, AND
///   - its opcode is not side-effecting.
///
/// When `func.has_exception_handling` is set, ops inside try regions that
/// may throw are conservatively kept alive (they could transfer control to
/// an exception handler whose side effects must be preserved).
///
/// Returns statistics about the changes made.
pub fn run(func: &mut TirFunction) -> PassStats {
    let mut stats = PassStats {
        name: "dce",
        ..Default::default()
    };

    let has_eh = func.has_exception_handling;

    // --- Phase 1: remove unreachable blocks ---
    let reachable = reachable_blocks(func);
    let unreachable: Vec<BlockId> = func
        .blocks
        .keys()
        .copied()
        .filter(|id| !reachable.contains(id))
        .collect();
    for id in &unreachable {
        func.blocks.remove(id);
        stats.ops_removed += 1; // count the block removal as one unit
    }

    // --- Phase 2: iterative dead-op removal ---
    for _round in 0..10 {
        let mut uses = build_use_counts(func);

        // Collect block ids to iterate (avoids borrow issues).
        let block_ids: Vec<BlockId> = func.blocks.keys().copied().collect();

        // Note: removed_this_round is cumulative across all blocks in this round.
        // The retain is harmless on blocks with no removals.
        let mut removed_this_round = 0usize;

        for bid in &block_ids {
            let block = match func.blocks.get_mut(bid) {
                Some(b) => b,
                None => continue,
            };

            // Track try-region nesting depth within this block.
            // Ops between TryStart..TryEnd are "inside a try region".
            let mut try_depth = Vec::with_capacity(block.ops.len());
            let mut depth: u32 = 0;
            for op in &block.ops {
                match op.opcode {
                    OpCode::TryStart => {
                        try_depth.push(depth);
                        depth += 1;
                    }
                    OpCode::TryEnd => {
                        depth = depth.saturating_sub(1);
                        try_depth.push(depth);
                    }
                    _ => {
                        try_depth.push(depth);
                    }
                }
            }

            // Walk ops in reverse order so that cascading removals within
            // a single pass are applied greedily.
            let mut to_keep: Vec<bool> = vec![true; block.ops.len()];

            for i in (0..block.ops.len()).rev() {
                let op = &block.ops[i];
                if is_side_effecting(op.opcode) {
                    continue;
                }

                // When inside a try region, conservatively keep ops that
                // may throw — they represent implicit edges to the handler.
                if has_eh && try_depth[i] > 0 && is_potentially_throwing(op.opcode) {
                    continue;
                }

                // Check whether every result is dead.
                let all_dead = op
                    .results
                    .iter()
                    .all(|v| uses.get(v).copied().unwrap_or(0) == 0);

                if all_dead {
                    // Mark for removal and release operand uses so that
                    // upstream ops in this same block may become dead too.
                    to_keep[i] = false;
                    for v in &op.operands {
                        let count = uses.entry(*v).or_insert(0);
                        if *count > 0 {
                            *count -= 1;
                        }
                    }
                    removed_this_round += 1;
                }
            }

            if removed_this_round > 0 {
                // Drain ops that were marked dead.
                let mut keep_iter = to_keep.iter();
                block.ops.retain(|_| *keep_iter.next().unwrap());
            }
        }

        stats.ops_removed += removed_this_round;

        if removed_this_round == 0 {
            break; // fixpoint reached
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
    use crate::tir::ops::{AttrDict, Dialect, OpCode, TirOp};
    use crate::tir::types::TirType;
    use crate::tir::values::{TirValue, ValueId};

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

    // -----------------------------------------------------------------------
    // Test 1: unused constant is removed
    // -----------------------------------------------------------------------
    #[test]
    fn unused_constant_removed() {
        let mut func = TirFunction::new("f".into(), vec![], TirType::None);
        let v0 = func.fresh_value();

        let entry = func.blocks.get_mut(&func.entry_block).unwrap();
        entry.ops.push(make_op(OpCode::ConstInt, vec![], vec![v0]));
        entry.terminator = Terminator::Return { values: vec![] };

        let stats = run(&mut func);
        assert_eq!(stats.ops_removed, 1);
        assert!(func.blocks[&func.entry_block].ops.is_empty());
    }

    // -----------------------------------------------------------------------
    // Test 2: unused arithmetic op is removed
    // -----------------------------------------------------------------------
    #[test]
    fn unused_arithmetic_removed() {
        let mut func = TirFunction::new("f".into(), vec![TirType::I64, TirType::I64], TirType::None);
        let p0 = ValueId(0);
        let p1 = ValueId(1);
        let sum = func.fresh_value();

        let entry = func.blocks.get_mut(&func.entry_block).unwrap();
        entry
            .ops
            .push(make_op(OpCode::Add, vec![p0, p1], vec![sum]));
        entry.terminator = Terminator::Return { values: vec![] };

        let stats = run(&mut func);
        assert_eq!(stats.ops_removed, 1);
        assert!(func.blocks[&func.entry_block].ops.is_empty());
    }

    // -----------------------------------------------------------------------
    // Test 3: used value is kept
    // -----------------------------------------------------------------------
    #[test]
    fn used_value_kept() {
        let mut func = TirFunction::new("f".into(), vec![], TirType::I64);
        let v0 = func.fresh_value();

        let entry = func.blocks.get_mut(&func.entry_block).unwrap();
        entry.ops.push(make_op(OpCode::ConstInt, vec![], vec![v0]));
        entry.terminator = Terminator::Return { values: vec![v0] };

        let stats = run(&mut func);
        assert_eq!(stats.ops_removed, 0);
        assert_eq!(func.blocks[&func.entry_block].ops.len(), 1);
    }

    // -----------------------------------------------------------------------
    // Test 4: side-effecting Call with unused result is kept
    // -----------------------------------------------------------------------
    #[test]
    fn side_effecting_call_kept() {
        let mut func = TirFunction::new("f".into(), vec![], TirType::None);
        let callee = func.fresh_value();
        let result = func.fresh_value();

        let entry = func.blocks.get_mut(&func.entry_block).unwrap();
        // Pretend callee is a "known" value — const for the callee pointer.
        entry
            .ops
            .push(make_op(OpCode::ConstInt, vec![], vec![callee]));
        entry
            .ops
            .push(make_op(OpCode::Call, vec![callee], vec![result]));
        entry.terminator = Terminator::Return { values: vec![] };

        let stats = run(&mut func);
        // The Call itself must never be removed.
        let ops = &func.blocks[&func.entry_block].ops;
        assert!(ops.iter().any(|o| o.opcode == OpCode::Call));
        // The ConstInt feeding the Call is used by it, so it stays too.
        let _ = stats;
    }

    // -----------------------------------------------------------------------
    // Test 5: cascade — A→B→C where C is unused
    // -----------------------------------------------------------------------
    #[test]
    fn cascade_removal() {
        let mut func = TirFunction::new("f".into(), vec![], TirType::None);
        let a = func.fresh_value();
        let b = func.fresh_value();
        let c = func.fresh_value();

        let entry = func.blocks.get_mut(&func.entry_block).unwrap();
        entry.ops.push(make_op(OpCode::ConstInt, vec![], vec![a]));
        entry.ops.push(make_op(OpCode::Neg, vec![a], vec![b]));
        entry.ops.push(make_op(OpCode::Neg, vec![b], vec![c]));
        entry.terminator = Terminator::Return { values: vec![] };

        let stats = run(&mut func);
        assert_eq!(stats.ops_removed, 3);
        assert!(func.blocks[&func.entry_block].ops.is_empty());
    }

    // -----------------------------------------------------------------------
    // Test 6: block argument is never removed
    // -----------------------------------------------------------------------
    #[test]
    fn block_arg_not_removed() {
        // Build: entry → loop_body(v_arg) → loop_body   (trivial loop)
        let mut func = TirFunction::new("f".into(), vec![], TirType::None);

        let loop_id = func.fresh_block();
        let v_arg_id = func.fresh_value();

        // Entry branches unconditionally to loop with an arg.
        {
            // Produce the initial arg value (before borrowing blocks mutably).
            let init = func.fresh_value();
            let entry = func.blocks.get_mut(&func.entry_block).unwrap();
            entry
                .ops
                .push(make_op(OpCode::ConstInt, vec![], vec![init]));
            entry.terminator = Terminator::Branch {
                target: loop_id,
                args: vec![init],
            };
        }

        // Loop block has one block argument; it loops back to itself passing
        // the same arg — the arg is therefore "live" via the branch.
        let loop_block = TirBlock {
            id: loop_id,
            args: vec![TirValue {
                id: v_arg_id,
                ty: TirType::I64,
            }],
            ops: vec![],
            terminator: Terminator::Branch {
                target: loop_id,
                args: vec![v_arg_id],
            },
        };
        func.blocks.insert(loop_id, loop_block);

        run(&mut func);

        // Block arguments on loop_id must not have been touched.
        assert_eq!(func.blocks[&loop_id].args.len(), 1);
    }

    // -----------------------------------------------------------------------
    // Test 7: empty function — no changes, no panic
    // -----------------------------------------------------------------------
    #[test]
    fn empty_function_no_change() {
        let mut func = TirFunction::new("empty".into(), vec![], TirType::None);
        let stats = run(&mut func);
        assert_eq!(stats.ops_removed, 0);
    }
}
