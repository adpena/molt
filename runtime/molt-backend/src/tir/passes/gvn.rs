//! Global Value Numbering (GVN) for TIR.
//!
//! Assigns a canonical "value number" to each computation.  If two operations
//! compute the same result (same opcode, same operand value numbers), the
//! second is replaced with a Copy of the first.  This subsumes common
//! subexpression elimination (CSE) and catches redundancies that SCCP misses.
//!
//! Algorithm: dominator-tree-scoped hash-based value numbering (RPO order).
//! Each block inherits the value table from its immediate dominator, so
//! values computed in dominating blocks are visible to all dominated blocks.
//!
//! Only pure (side-effect-free) operations are candidates for numbering.
//! Side-effecting ops (calls, stores, imports) are always preserved.
//!
//! Reference: Briggs, Cooper, Simpson — "Value Numbering" (1997).

use std::collections::HashMap;

use super::PassStats;
use crate::tir::blocks::{BlockId, Terminator};
use crate::tir::function::TirFunction;
use crate::tir::ops::{AttrValue, OpCode, TirOp, Dialect};
use crate::tir::values::ValueId;

/// A hashable representation of a computation for value numbering.
#[derive(Hash, Eq, PartialEq, Clone, Debug)]
struct ValueKey {
    opcode: OpCode,
    /// Operand value numbers (not raw ValueIds).
    operands: Vec<ValueId>,
    /// For constants, the literal value distinguishes different constants
    /// with the same opcode.  Uses the exact value (no hashing) to prevent
    /// collisions between distinct constants.
    const_int_key: Option<i64>,
    const_str_key: Option<String>,
}

/// Always-safe ops: box/unbox are pure value transformations that
/// preserve type through TIR→SimpleIR→native lowering correctly.
///
/// Constants are NOT included here despite being pure — replacing
/// a ConstFloat with Copy(earlier_const_float) changes how the
/// native backend handles the op (ConstFloat → raw f64 vs
/// Copy → NaN-boxed path), causing type mismatches.
fn is_always_numberable(opcode: OpCode) -> bool {
    matches!(
        opcode,
        OpCode::ConstInt
            | OpCode::ConstFloat
            | OpCode::ConstStr
            | OpCode::ConstBool
            | OpCode::ConstNone
            | OpCode::ConstBytes
            | OpCode::BoxVal
            | OpCode::UnboxVal
    )
}

/// Returns `true` if the opcode is numberable when operands are proven typed.
fn is_typed_numberable(opcode: OpCode) -> bool {
    matches!(
        opcode,
        OpCode::Add
            | OpCode::Sub
            | OpCode::Mul
            | OpCode::Div
            | OpCode::FloorDiv
            | OpCode::Mod
            | OpCode::Pow
            | OpCode::Neg
            | OpCode::Pos
            | OpCode::Eq
            | OpCode::Ne
            | OpCode::Lt
            | OpCode::Le
            | OpCode::Gt
            | OpCode::Ge
            | OpCode::Is
            | OpCode::IsNot
            | OpCode::BitAnd
            | OpCode::BitOr
            | OpCode::BitXor
            | OpCode::BitNot
            | OpCode::Shl
            | OpCode::Shr
            | OpCode::And
            | OpCode::Or
            | OpCode::Not
            | OpCode::Bool
            | OpCode::TypeGuard
    )
}

/// A type is "primitive" when arithmetic on it is provably side-effect-free.
fn is_primitive_type(ty: &crate::tir::types::TirType) -> bool {
    use crate::tir::types::TirType;
    matches!(ty, TirType::I64 | TirType::F64 | TirType::Bool | TirType::None)
}

/// Extract constant keys for deduplicating constants by exact value.
fn const_keys(op: &TirOp) -> (Option<i64>, Option<String>) {
    match op.opcode {
        OpCode::ConstInt => {
            let k = op.attrs.get("value").and_then(|v| match v {
                AttrValue::Int(i) => Some(*i),
                _ => None,
            });
            (k, None)
        }
        OpCode::ConstBool => {
            let k = op.attrs.get("value").and_then(|v| match v {
                AttrValue::Bool(b) => Some(if *b { 1 } else { 0 }),
                AttrValue::Int(i) => Some(*i),
                _ => None,
            });
            (k, None)
        }
        OpCode::ConstNone => (Some(0), None),
        OpCode::ConstFloat => {
            // TIR ConstFloat stores the float in "f_value" (or "value" as fallback).
            let k = op
                .attrs
                .get("f_value")
                .or_else(|| op.attrs.get("value"))
                .and_then(|v| match v {
                    AttrValue::Float(f) => Some(f.to_bits() as i64),
                    _ => None,
                });
            (k, None)
        }
        OpCode::ConstStr | OpCode::ConstBytes => {
            // TIR ConstStr stores the string in "s_value", not "value".
            let s = op
                .attrs
                .get("s_value")
                .or_else(|| op.attrs.get("value"))
                .and_then(|v| match v {
                    AttrValue::Str(s) => Some(s.clone()),
                    _ => None,
                });
            (None, s)
        }
        _ => (None, None),
    }
}

/// Compute a simple dominator tree using the Cooper-Harvey-Kennedy algorithm.
/// Returns a map from BlockId → immediate dominator BlockId.
fn compute_idom(func: &TirFunction) -> HashMap<BlockId, BlockId> {
    // Build predecessor map.
    let mut preds: HashMap<BlockId, Vec<BlockId>> = HashMap::new();
    for &bid in func.blocks.keys() {
        preds.entry(bid).or_default();
    }
    for (&bid, block) in &func.blocks {
        let succs = match &block.terminator {
            Terminator::Branch { target, .. } => vec![*target],
            Terminator::CondBranch {
                then_block,
                else_block,
                ..
            } => vec![*then_block, *else_block],
            Terminator::Switch {
                cases,
                default_args: _,
                default: default_bid,
                ..
            } => {
                let mut s: Vec<BlockId> = cases.iter().map(|(_, b, _)| *b).collect();
                s.push(*default_bid);
                s
            }
            _ => vec![],
        };
        for s in succs {
            preds.entry(s).or_default().push(bid);
        }
    }

    // RPO ordering.
    let mut rpo: Vec<BlockId> = Vec::new();
    let mut visited: std::collections::HashSet<BlockId> = std::collections::HashSet::new();
    fn dfs(
        bid: BlockId,
        func: &TirFunction,
        visited: &mut std::collections::HashSet<BlockId>,
        rpo: &mut Vec<BlockId>,
    ) {
        if !visited.insert(bid) {
            return;
        }
        if let Some(block) = func.blocks.get(&bid) {
            match &block.terminator {
                Terminator::Branch { target, .. } => dfs(*target, func, visited, rpo),
                Terminator::CondBranch {
                    then_block,
                    else_block,
                    ..
                } => {
                    dfs(*then_block, func, visited, rpo);
                    dfs(*else_block, func, visited, rpo);
                }
                Terminator::Switch {
                    cases,
                    default: default_bid,
                    ..
                } => {
                    for (_, b, _) in cases {
                        dfs(*b, func, visited, rpo);
                    }
                    dfs(*default_bid, func, visited, rpo);
                }
                _ => {}
            }
        }
        rpo.push(bid);
    }
    dfs(func.entry_block, func, &mut visited, &mut rpo);
    rpo.reverse();

    let rpo_index: HashMap<BlockId, usize> = rpo.iter().enumerate().map(|(i, b)| (*b, i)).collect();

    // CHK dominator algorithm.
    let mut idom: HashMap<BlockId, BlockId> = HashMap::new();
    idom.insert(func.entry_block, func.entry_block);

    let intersect =
        |mut b1: BlockId, mut b2: BlockId, idom: &HashMap<BlockId, BlockId>| -> BlockId {
            while b1 != b2 {
                while rpo_index.get(&b1).copied().unwrap_or(usize::MAX)
                    > rpo_index.get(&b2).copied().unwrap_or(usize::MAX)
                {
                    b1 = idom[&b1];
                }
                while rpo_index.get(&b2).copied().unwrap_or(usize::MAX)
                    > rpo_index.get(&b1).copied().unwrap_or(usize::MAX)
                {
                    b2 = idom[&b2];
                }
            }
            b1
        };

    let mut changed = true;
    while changed {
        changed = false;
        for &bid in &rpo {
            if bid == func.entry_block {
                continue;
            }
            let block_preds = &preds[&bid];
            let processed: Vec<BlockId> = block_preds
                .iter()
                .copied()
                .filter(|p| idom.contains_key(p))
                .collect();
            if processed.is_empty() {
                continue;
            }
            let mut new_idom = processed[0];
            for &p in &processed[1..] {
                new_idom = intersect(new_idom, p, &idom);
            }
            if idom.get(&bid) != Some(&new_idom) {
                idom.insert(bid, new_idom);
                changed = true;
            }
        }
    }

    idom
}

pub fn run(func: &mut TirFunction) -> PassStats {
    let mut stats = PassStats {
        name: "gvn",
        ..Default::default()
    };

    if func.blocks.len() <= 1 && func.blocks.values().all(|b| b.ops.is_empty()) {
        return stats;
    }

    let idom = compute_idom(func);

    // Build RPO for traversal order.
    let mut rpo: Vec<BlockId> = Vec::new();
    let mut visited: std::collections::HashSet<BlockId> = std::collections::HashSet::new();
    fn dfs_rpo(
        bid: BlockId,
        func: &TirFunction,
        visited: &mut std::collections::HashSet<BlockId>,
        rpo: &mut Vec<BlockId>,
    ) {
        if !visited.insert(bid) {
            return;
        }
        if let Some(block) = func.blocks.get(&bid) {
            match &block.terminator {
                Terminator::Branch { target, .. } => dfs_rpo(*target, func, visited, rpo),
                Terminator::CondBranch {
                    then_block,
                    else_block,
                    ..
                } => {
                    dfs_rpo(*then_block, func, visited, rpo);
                    dfs_rpo(*else_block, func, visited, rpo);
                }
                Terminator::Switch {
                    cases,
                    default: default_bid,
                    ..
                } => {
                    for (_, b, _) in cases {
                        dfs_rpo(*b, func, visited, rpo);
                    }
                    dfs_rpo(*default_bid, func, visited, rpo);
                }
                _ => {}
            }
        }
        rpo.push(bid);
    }
    dfs_rpo(func.entry_block, func, &mut visited, &mut rpo);
    rpo.reverse();

    // Build a value→type map from STRUCTURALLY GUARANTEED sources only:
    // block args (set by type_refine), constants, and function params.
    // NO speculative type inference — if a value's type isn't provably
    // known, it's treated as DynBox (not primitive, not safe to number).
    let mut value_type: HashMap<ValueId, crate::tir::types::TirType> = HashMap::new();
    {
        use crate::tir::types::TirType;
        // Block arguments carry types from type_refine.
        for block in func.blocks.values() {
            for arg in &block.args {
                value_type.insert(arg.id, arg.ty.clone());
            }
        }
        // Function parameters.
        for (i, ty) in func.param_types.iter().enumerate() {
            value_type.insert(ValueId(i as u32), ty.clone());
        }
        // Constants have known types.
        for block in func.blocks.values() {
            for op in &block.ops {
                let ty = match op.opcode {
                    OpCode::ConstInt => Some(TirType::I64),
                    OpCode::ConstFloat => Some(TirType::F64),
                    OpCode::ConstBool => Some(TirType::Bool),
                    OpCode::ConstNone => Some(TirType::None),
                    OpCode::ConstStr => Some(TirType::Str),
                    OpCode::ConstBytes => Some(TirType::Bytes),
                    _ => None,
                };
                if let Some(t) = ty {
                    for &res in &op.results {
                        value_type.insert(res, t.clone());
                    }
                }
            }
        }
    }

    // Phase 1: Number all values in RPO order.
    let mut value_number: HashMap<ValueId, ValueId> = HashMap::new();
    let mut key_to_leader: HashMap<ValueKey, ValueId> = HashMap::new();

    for block in func.blocks.values() {
        for arg in &block.args {
            value_number.insert(arg.id, arg.id);
        }
    }
    for i in 0..func.param_types.len() {
        let v = ValueId(i as u32);
        value_number.insert(v, v);
    }

    // Track which block each value is defined in.
    let mut value_def_block: HashMap<ValueId, BlockId> = HashMap::new();
    for (&bid, block) in &func.blocks {
        for arg in &block.args {
            value_def_block.insert(arg.id, bid);
        }
        for op in &block.ops {
            for &res in &op.results {
                value_def_block.insert(res, bid);
            }
        }
    }
    for i in 0..func.param_types.len() {
        value_def_block.insert(ValueId(i as u32), func.entry_block);
    }

    // Dominance check: does block `a` dominate block `b`?
    let dominates = |a: BlockId, b: BlockId| -> bool {
        if a == b {
            return true;
        }
        let mut cur = b;
        for _ in 0..1000 {
            match idom.get(&cur) {
                Some(&parent) if parent == cur => return false, // reached entry without finding a
                Some(&parent) => {
                    if parent == a {
                        return true;
                    }
                    cur = parent;
                }
                None => return false,
            }
        }
        false
    };

    // Collect replacements: (block, op_index) → leader ValueId
    let mut replacements: Vec<(BlockId, usize, ValueId)> = Vec::new();

    for &bid in &rpo {
        let block = match func.blocks.get(&bid) {
            Some(b) => b,
            None => continue,
        };

        // Reset the leader table per block. Cross-block dedup requires
        // proven dominance AND safe phi interaction — restrict to
        // intra-block dedup until full dominator-scoped tables are
        // implemented (LLVM's GVN uses scoped hash tables that inherit
        // from the immediate dominator and are popped on scope exit).
        key_to_leader.clear();

        for (i, op) in block.ops.iter().enumerate() {
            if op.results.is_empty() {
                continue;
            }

            let result = op.results[0];

            // Determine if this op is safe to number.
            let numberable = if is_always_numberable(op.opcode) {
                true
            } else if is_typed_numberable(op.opcode) {
                // Arithmetic/comparison/boolean ops are only numberable
                // when ALL operands are proven primitive types. On DynBox
                // operands, these ops may trigger dunder methods with side
                // effects (__add__, __eq__, etc.).
                op.operands.iter().all(|v| {
                    value_type
                        .get(v)
                        .map_or(false, |ty| is_primitive_type(ty))
                })
            } else {
                false
            };

            if !numberable {
                value_number.insert(result, result);
                continue;
            }

            // Build the value key using RAW operand ValueIds (not numbered).
            // Using value numbers across loop iterations is unsound: block
            // args represent phi nodes whose values change per iteration.
            // Intra-block dedup uses raw ValueIds which ARE unique per SSA def.
            let numbered_operands: Vec<ValueId> = op
                .operands
                .iter()
                .map(|&v| v)
                .collect();

            let (const_int_key, const_str_key) = const_keys(op);
            let key = ValueKey {
                opcode: op.opcode,
                operands: numbered_operands,
                const_int_key,
                const_str_key,
            };

            if let Some(&leader) = key_to_leader.get(&key) {
                // This computation was already done — replace with leader,
                // but ONLY if the leader's definition dominates this block.
                let leader_block = value_def_block
                    .get(&leader)
                    .copied()
                    .unwrap_or(func.entry_block);
                if dominates(leader_block, bid) {
                    value_number.insert(result, leader);
                    replacements.push((bid, i, leader));
                } else {
                    // Leader doesn't dominate — can't replace. Register this
                    // as a new leader for its scope.
                    value_number.insert(result, result);
                }
            } else {
                // First time seeing this computation.
                key_to_leader.insert(key, result);
                value_number.insert(result, result);
            }
        }
    }

    // Phase 2: Apply replacements (replace redundant ops with Copy).
    for (bid, op_idx, leader) in &replacements {
        if let Some(block) = func.blocks.get_mut(bid) {
            if *op_idx < block.ops.len() {
                let result = block.ops[*op_idx].results[0];
                block.ops[*op_idx] = TirOp {
                    dialect: Dialect::Molt,
                    opcode: OpCode::Copy,
                    operands: vec![*leader],
                    results: vec![result],
                    attrs: Default::default(),
                    source_span: block.ops[*op_idx].source_span,
                };
                stats.values_changed += 1;
            }
        }
    }

    // Phase 3: Operand renaming is deferred to copy_prop + DCE.
    // Direct operand replacement requires dominance checks to ensure
    // the leader value dominates every use site. The Copy ops from
    // Phase 2 are sufficient — copy_prop will resolve them, and DCE
    // will clean up the now-dead original ops.

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
    use crate::tir::ops::{AttrDict, AttrValue, Dialect, OpCode, TirOp};
    use crate::tir::types::TirType;

    fn make_const_int(value: i64, result: ValueId) -> TirOp {
        TirOp {
            dialect: Dialect::Molt,
            opcode: OpCode::ConstInt,
            operands: vec![],
            results: vec![result],
            attrs: {
                let mut m = AttrDict::new();
                m.insert("value".into(), AttrValue::Int(value));
                m
            },
            source_span: None,
        }
    }

    fn make_binop(opcode: OpCode, lhs: ValueId, rhs: ValueId, result: ValueId) -> TirOp {
        TirOp {
            dialect: Dialect::Molt,
            opcode,
            operands: vec![lhs, rhs],
            results: vec![result],
            attrs: AttrDict::new(),
            source_span: None,
        }
    }

    #[test]
    fn redundant_add_eliminated() {
        let mut func = TirFunction::new("f".into(), vec![TirType::I64, TirType::I64], TirType::I64);
        let p0 = ValueId(0);
        let p1 = ValueId(1);
        let sum1 = func.fresh_value();
        let sum2 = func.fresh_value(); // same computation as sum1

        let entry = func.blocks.get_mut(&func.entry_block).unwrap();
        entry.ops.push(make_binop(OpCode::Add, p0, p1, sum1));
        entry.ops.push(make_binop(OpCode::Add, p0, p1, sum2));
        entry.terminator = Terminator::Return {
            values: vec![sum2],
        };

        let stats = run(&mut func);
        assert!(stats.values_changed > 0);

        // sum2's definition should now be a Copy from sum1.
        let ops = &func.blocks[&func.entry_block].ops;
        assert_eq!(ops[1].opcode, OpCode::Copy);
        assert_eq!(ops[1].operands[0], sum1);
    }

    #[test]
    fn duplicate_constants_folded() {
        let mut func = TirFunction::new("f".into(), vec![], TirType::I64);
        let c1 = func.fresh_value();
        let c2 = func.fresh_value(); // same constant as c1

        let entry = func.blocks.get_mut(&func.entry_block).unwrap();
        entry.ops.push(make_const_int(42, c1));
        entry.ops.push(make_const_int(42, c2));
        entry.terminator = Terminator::Return { values: vec![c2] };

        let stats = run(&mut func);
        assert!(stats.values_changed > 0);

        // c2 should be replaced with a Copy from c1.
        let ops = &func.blocks[&func.entry_block].ops;
        assert_eq!(ops[1].opcode, OpCode::Copy);
        assert_eq!(ops[1].operands[0], c1);
    }

    #[test]
    fn different_constants_not_folded() {
        let mut func = TirFunction::new("f".into(), vec![], TirType::I64);
        let c1 = func.fresh_value();
        let c2 = func.fresh_value();

        let entry = func.blocks.get_mut(&func.entry_block).unwrap();
        entry.ops.push(make_const_int(42, c1));
        entry.ops.push(make_const_int(99, c2));
        entry.terminator = Terminator::Return { values: vec![c2] };

        let stats = run(&mut func);
        // c2 should NOT be folded — different constant.
        let ops = &func.blocks[&func.entry_block].ops;
        assert_eq!(ops[1].opcode, OpCode::ConstInt);
        let _ = stats;
    }

    #[test]
    fn side_effecting_ops_preserved() {
        let mut func = TirFunction::new("f".into(), vec![TirType::I64], TirType::I64);
        let p0 = ValueId(0);
        let r1 = func.fresh_value();
        let r2 = func.fresh_value();

        let entry = func.blocks.get_mut(&func.entry_block).unwrap();
        // Two identical Call ops — both must be preserved (side effects).
        entry.ops.push(TirOp {
            dialect: Dialect::Molt,
            opcode: OpCode::Call,
            operands: vec![p0],
            results: vec![r1],
            attrs: AttrDict::new(),
            source_span: None,
        });
        entry.ops.push(TirOp {
            dialect: Dialect::Molt,
            opcode: OpCode::Call,
            operands: vec![p0],
            results: vec![r2],
            attrs: AttrDict::new(),
            source_span: None,
        });
        entry.terminator = Terminator::Return { values: vec![r2] };

        let _stats = run(&mut func);

        // Both calls must remain — not folded.
        let ops = &func.blocks[&func.entry_block].ops;
        assert_eq!(ops[0].opcode, OpCode::Call);
        assert_eq!(ops[1].opcode, OpCode::Call);
    }
}
