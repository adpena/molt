//! Global Value Numbering (GVN) for TIR.
//!
//! Assigns a canonical "value number" to each computation.  If two operations
//! compute the same result (same opcode, same operand value numbers), the
//! second is replaced with a Copy of the first.  This subsumes common
//! subexpression elimination (CSE) and catches redundancies that SCCP misses.
//!
//! Algorithm: dominator-tree-scoped hash-based value numbering.  A scoped
//! hash table is maintained as the dominator tree is walked in pre-order.
//! Each block inherits the leader table of its immediate dominator (entries
//! defined in dominating blocks remain visible) and contributes its own new
//! entries.  On exit from a block, the entries it contributed are removed,
//! restoring the parent scope — so values are only propagated to blocks the
//! defining block actually dominates.  This catches cross-block redundancy
//! (same `a + b` in entry and a dominated body block) without ever exposing
//! a value to a non-dominated sibling.
//!
//! Only pure (side-effect-free) operations are candidates for numbering.
//! Side-effecting ops (calls, stores, imports) are always preserved.
//!
//! Reference: Briggs, Cooper, Simpson — "Value Numbering" (1997).
//! LLVM's GVN uses an analogous scoped-hash-table walk over the dominator
//! tree (see `llvm/lib/Transforms/Scalar/GVN.cpp::ValueTable`).

use std::collections::{HashMap, HashSet, VecDeque};

use super::PassStats;
use crate::tir::blocks::{BlockId, Terminator};
use crate::tir::dominators::{build_pred_map, compute_idoms, dominates};
use crate::tir::function::TirFunction;
use crate::tir::ops::{AttrValue, Dialect, OpCode, TirOp};
use crate::tir::values::ValueId;

/// A hashable representation of a computation for value numbering.
#[derive(Hash, Eq, PartialEq, Clone, Debug)]
struct ValueKey {
    opcode: OpCode,
    /// Operand value numbers (canonicalized through the scoped leader table).
    operands: Vec<ValueId>,
    /// For constants, the literal value distinguishes different constants
    /// with the same opcode.  Uses the exact value (no hashing) to prevent
    /// collisions between distinct constants.
    const_int_key: Option<i64>,
    const_str_key: Option<String>,
    const_bytes_key: Option<Vec<u8>>,
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
            | OpCode::InplaceAdd
            | OpCode::InplaceSub
            | OpCode::InplaceMul
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
fn const_keys(op: &TirOp) -> (Option<i64>, Option<String>, Option<Vec<u8>>) {
    match op.opcode {
        OpCode::ConstInt => {
            let k = op.attrs.get("value").and_then(|v| match v {
                AttrValue::Int(i) => Some(*i),
                _ => None,
            });
            (k, None, None)
        }
        OpCode::ConstBool => {
            let k = op.attrs.get("value").and_then(|v| match v {
                AttrValue::Bool(b) => Some(if *b { 1 } else { 0 }),
                AttrValue::Int(i) => Some(*i),
                _ => None,
            });
            (k, None, None)
        }
        OpCode::ConstNone => (Some(0), None, None),
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
            (k, None, None)
        }
        OpCode::ConstStr => {
            // TIR ConstStr stores the string in "s_value", not "value".
            let s = op
                .attrs
                .get("s_value")
                .or_else(|| op.attrs.get("value"))
                .and_then(|v| match v {
                    AttrValue::Str(s) => Some(s.clone()),
                    _ => None,
                });
            (None, s, None)
        }
        OpCode::ConstBytes => {
            let b = op
                .attrs
                .get("bytes")
                .or_else(|| op.attrs.get("value"))
                .and_then(|v| match v {
                    AttrValue::Bytes(b) => Some(b.clone()),
                    _ => None,
                });
            (None, None, b)
        }
        _ => (None, None, None),
    }
}

/// Build the dominator-tree children map from an idom map.
fn build_dom_children(
    idoms: &HashMap<BlockId, Option<BlockId>>,
) -> HashMap<BlockId, Vec<BlockId>> {
    let mut children: HashMap<BlockId, Vec<BlockId>> = HashMap::new();
    for &bid in idoms.keys() {
        children.entry(bid).or_default();
    }
    for (&child, parent) in idoms {
        if let Some(parent) = parent
            && *parent != child {
                children.entry(*parent).or_default().push(child);
            }
    }
    // Sort children for deterministic traversal order.
    for kids in children.values_mut() {
        kids.sort_unstable_by_key(|b| b.0);
    }
    children
}

/// Set of blocks reachable from entry via terminator-only successors.
///
/// `compute_idoms` is exception-edge-aware: it considers handler blocks
/// reachable through `CheckException`/`TryStart`/`TryEnd` ops and assigns
/// them a dominator.  However, the LIR verifier (`verify_lir`) computes
/// reachability and dominance from terminator successors only — handler
/// blocks reached only via exception edges are unreachable in its view,
/// and therefore are not in its dominator preorder.
///
/// If GVN replaces an op in such a block with `Copy(leader)` where
/// `leader` is defined elsewhere, `verify_lir` will reject the operand:
/// `dominates(leader_block, handler_block)` returns `false` because the
/// handler is not in the strict-CFG dominator tree.
///
/// To stay sound under the strict-CFG verifier, GVN restricts cross-block
/// replacements to use sites that are themselves reachable via terminator
/// successors.  Blocks reachable only through exception edges still get
/// intra-block GVN (their leaders never escape their own scope), matching
/// the behaviour the previous intra-block-only pass had for them.
fn strict_terminator_reachable(func: &TirFunction) -> HashSet<BlockId> {
    let mut visited: HashSet<BlockId> = HashSet::new();
    let mut queue: VecDeque<BlockId> = VecDeque::new();
    queue.push_back(func.entry_block);
    visited.insert(func.entry_block);
    while let Some(bid) = queue.pop_front() {
        let Some(block) = func.blocks.get(&bid) else {
            continue;
        };
        let succs: Vec<BlockId> = match &block.terminator {
            Terminator::Branch { target, .. } => vec![*target],
            Terminator::CondBranch {
                then_block,
                else_block,
                ..
            } => vec![*then_block, *else_block],
            Terminator::Switch { cases, default, .. } => {
                let mut s: Vec<BlockId> = cases.iter().map(|(_, b, _)| *b).collect();
                s.push(*default);
                s
            }
            Terminator::Return { .. } | Terminator::Unreachable => Vec::new(),
        };
        for succ in succs {
            if visited.insert(succ) {
                queue.push_back(succ);
            }
        }
    }
    visited
}

pub fn run(func: &mut TirFunction) -> PassStats {
    let mut stats = PassStats {
        name: "gvn",
        ..Default::default()
    };

    if func.blocks.len() <= 1 && func.blocks.values().all(|b| b.ops.is_empty()) {
        return stats;
    }

    // Build dominator tree (exception-edge-aware).
    let pred_map = build_pred_map(func);
    let idoms = compute_idoms(func, &pred_map);
    let dom_children = build_dom_children(&idoms);

    // Strict-CFG reachability (terminator-only).  Cross-block replacements
    // are only safe when the use site is reachable via terminators — that
    // is the reachability `verify_lir` uses, and emitting `Copy(leader)`
    // into a block that's reachable only through exception edges would
    // cause the verifier to reject the new operand.
    let strict_reachable = strict_terminator_reachable(func);

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

    // Scoped leader table: ValueKey -> leader ValueId.  Entries added by
    // a block are removed when that block's dominator subtree is fully
    // processed, so dedup never crosses non-dominance boundaries.
    let mut key_to_leader: HashMap<ValueKey, ValueId> = HashMap::new();

    // value_number maps each canonicalized value to the current leader for
    // its scope.  Defaults to identity (a value is its own leader).  This
    // is what propagates cross-block value numbers through operand keys.
    let mut value_number: HashMap<ValueId, ValueId> = HashMap::new();
    for block in func.blocks.values() {
        for arg in &block.args {
            value_number.insert(arg.id, arg.id);
        }
    }
    for i in 0..func.param_types.len() {
        let v = ValueId(i as u32);
        value_number.insert(v, v);
    }

    // Replacements collected during traversal: (block, op_idx, leader).
    let mut replacements: Vec<(BlockId, usize, ValueId)> = Vec::new();

    // Iterative dominator-tree pre-order walk.  Each frame is either an
    // `Enter` (push scope, process this block) or an `Exit` (undo this
    // block's scope contributions).  This preserves LLVM-style scoped
    // hash tables without recursion (avoiding stack overflows on deep
    // dominator trees).
    enum Frame {
        Enter(BlockId),
        /// Undo a block's scope contributions on the way out.
        /// `key_undo`: keys this block inserted (with the prior value, if any).
        /// `vn_undo`: value numbers this block inserted (with prior value, if any).
        Exit {
            key_undo: Vec<(ValueKey, Option<ValueId>)>,
            vn_undo: Vec<(ValueId, Option<ValueId>)>,
        },
    }

    let mut stack: Vec<Frame> = vec![Frame::Enter(func.entry_block)];
    // Guard against pathological cyclic idom maps (should never occur from
    // compute_idoms, but the dominator walk must not loop forever).
    let mut visited: HashSet<BlockId> = HashSet::new();

    while let Some(frame) = stack.pop() {
        match frame {
            Frame::Exit { key_undo, vn_undo } => {
                // Restore parent scope.  Iterate in reverse so the latest
                // shadowing entry is undone first (matches LIFO insertion).
                for (key, prior) in key_undo.into_iter().rev() {
                    match prior {
                        Some(v) => {
                            key_to_leader.insert(key, v);
                        }
                        None => {
                            key_to_leader.remove(&key);
                        }
                    }
                }
                for (val, prior) in vn_undo.into_iter().rev() {
                    match prior {
                        Some(v) => {
                            value_number.insert(val, v);
                        }
                        None => {
                            value_number.remove(&val);
                        }
                    }
                }
            }
            Frame::Enter(bid) => {
                if !visited.insert(bid) {
                    continue;
                }
                let block = match func.blocks.get(&bid) {
                    Some(b) => b,
                    None => continue,
                };

                // Per-block undo logs.  Capturing the prior value (if any)
                // lets us restore shadowed entries from outer scopes when
                // popping this block off the dominator stack.
                let mut key_undo: Vec<(ValueKey, Option<ValueId>)> = Vec::new();
                let mut vn_undo: Vec<(ValueId, Option<ValueId>)> = Vec::new();

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
                        op.operands
                            .iter()
                            .all(|v| value_type.get(v).is_some_and(is_primitive_type))
                    } else {
                        false
                    };

                    if !numberable {
                        let prior = value_number.insert(result, result);
                        vn_undo.push((result, prior));
                        continue;
                    }

                    // Canonicalize operands through the leader table so a
                    // computation that uses a value defined in a dominator
                    // matches a later occurrence using that same value.
                    // Constants (no operands) and ops whose operands aren't
                    // yet numbered fall back to identity.
                    let numbered_operands: Vec<ValueId> = op
                        .operands
                        .iter()
                        .map(|v| value_number.get(v).copied().unwrap_or(*v))
                        .collect();

                    let (const_int_key, const_str_key, const_bytes_key) = const_keys(op);
                    let key = ValueKey {
                        opcode: op.opcode,
                        operands: numbered_operands,
                        const_int_key,
                        const_str_key,
                        const_bytes_key,
                    };

                    if let Some(&leader) = key_to_leader.get(&key) {
                        // The leader is in scope iff its definition dominates
                        // this block.  Scoped insertion already enforces this
                        // structurally, but we double-check: dominance of the
                        // leader's defining block over `bid` is the contract
                        // every leader entry must satisfy.
                        let leader_block = value_def_block
                            .get(&leader)
                            .copied()
                            .unwrap_or(func.entry_block);
                        // Cross-block dedup additionally requires that BOTH
                        // the leader's defining block AND the use block are
                        // reachable via strict-CFG terminator successors.
                        // The LIR verifier computes dominance only over that
                        // subgraph; emitting `Copy(leader)` into a block
                        // outside it would cause `verify_lir` to reject the
                        // new operand.  Intra-block replacements (same block
                        // for def and use) bypass the strict-CFG check
                        // because verification handles same-block uses by
                        // op-index ordering rather than dominator lookup.
                        let cross_block = leader_block != bid;
                        let strict_ok = !cross_block
                            || (strict_reachable.contains(&leader_block)
                                && strict_reachable.contains(&bid));
                        if dominates(leader_block, bid, &idoms) && strict_ok {
                            let prior = value_number.insert(result, leader);
                            vn_undo.push((result, prior));
                            replacements.push((bid, i, leader));
                            continue;
                        }
                        // Leader fell out of scope or strict-CFG check
                        // failed — fall through and register `result` as a
                        // fresh leader for this scope.
                    }

                    // First time seeing this computation in this scope —
                    // become the leader.
                    let prior_key = key_to_leader.insert(key.clone(), result);
                    key_undo.push((key, prior_key));
                    let prior_vn = value_number.insert(result, result);
                    vn_undo.push((result, prior_vn));
                }

                // Schedule the exit frame BEFORE pushing children, so that
                // when all children (and their subtrees) are processed, this
                // block's scope contributions are undone exactly once.
                stack.push(Frame::Exit { key_undo, vn_undo });
                if let Some(kids) = dom_children.get(&bid) {
                    // Push children in reverse so that the first child is
                    // processed first (stack is LIFO).
                    for &child in kids.iter().rev() {
                        stack.push(Frame::Enter(child));
                    }
                }
            }
        }
    }

    // Apply replacements (replace redundant ops with Copy).
    for (bid, op_idx, leader) in &replacements {
        if let Some(block) = func.blocks.get_mut(bid)
            && *op_idx < block.ops.len() {
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

    // Operand renaming is deferred to copy_prop + DCE.  Direct operand
    // replacement requires per-use dominance checks; the Copy ops emitted
    // above are sufficient — copy_prop will resolve them, and DCE will
    // clean up the now-dead original ops.

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
    use crate::tir::values::TirValue;

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

    fn make_const_bool(value: bool, result: ValueId) -> TirOp {
        TirOp {
            dialect: Dialect::Molt,
            opcode: OpCode::ConstBool,
            operands: vec![],
            results: vec![result],
            attrs: {
                let mut m = AttrDict::new();
                m.insert("value".into(), AttrValue::Bool(value));
                m
            },
            source_span: None,
        }
    }

    fn make_const_bytes(bytes: &[u8], result: ValueId) -> TirOp {
        TirOp {
            dialect: Dialect::Molt,
            opcode: OpCode::ConstBytes,
            operands: vec![],
            results: vec![result],
            attrs: {
                let mut m = AttrDict::new();
                m.insert("bytes".into(), AttrValue::Bytes(bytes.to_vec()));
                m
            },
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
    fn different_const_bytes_not_folded() {
        let mut func = TirFunction::new("f".into(), vec![], TirType::Bytes);
        let c1 = func.fresh_value();
        let c2 = func.fresh_value();

        let entry = func.blocks.get_mut(&func.entry_block).unwrap();
        entry.ops.push(make_const_bytes(b"one,two", c1));
        entry.ops.push(make_const_bytes(b"two", c2));
        entry.terminator = Terminator::Return { values: vec![c2] };

        let _stats = run(&mut func);

        let ops = &func.blocks[&func.entry_block].ops;
        assert_eq!(ops[1].opcode, OpCode::ConstBytes);
        assert_eq!(
            ops[1].attrs.get("bytes"),
            Some(&AttrValue::Bytes(b"two".to_vec()))
        );
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

    // ── Cross-block dominator-scoped GVN tests ──────────────────────────

    /// entry: c1 = ConstInt 42; branch body
    /// body:  c2 = ConstInt 42; return c2
    /// → entry strictly dominates body, so c2 should become Copy(c1).
    #[test]
    fn cross_block_redundant_constant() {
        let mut func = TirFunction::new("f".into(), vec![], TirType::I64);
        let body = func.fresh_block();
        let c1 = func.fresh_value();
        let c2 = func.fresh_value();

        {
            let entry = func.blocks.get_mut(&func.entry_block).unwrap();
            entry.ops.push(make_const_int(42, c1));
            entry.terminator = Terminator::Branch {
                target: body,
                args: vec![],
            };
        }

        func.blocks.insert(
            body,
            TirBlock {
                id: body,
                args: vec![],
                ops: vec![make_const_int(42, c2)],
                terminator: Terminator::Return { values: vec![c2] },
            },
        );

        let stats = run(&mut func);
        assert!(stats.values_changed > 0);

        // c2 in `body` should now be a Copy of c1 from entry.
        let body_ops = &func.blocks[&body].ops;
        assert_eq!(body_ops[0].opcode, OpCode::Copy);
        assert_eq!(body_ops[0].operands[0], c1);
        assert_eq!(body_ops[0].results[0], c2);
    }

    /// entry: s1 = p0 + p1; branch body
    /// body:  s2 = p0 + p1; return s2
    /// → s2 should become Copy(s1).
    #[test]
    fn cross_block_redundant_arithmetic() {
        let mut func = TirFunction::new(
            "f".into(),
            vec![TirType::I64, TirType::I64],
            TirType::I64,
        );
        let p0 = ValueId(0);
        let p1 = ValueId(1);
        let body = func.fresh_block();
        let s1 = func.fresh_value();
        let s2 = func.fresh_value();

        {
            let entry = func.blocks.get_mut(&func.entry_block).unwrap();
            entry.ops.push(make_binop(OpCode::Add, p0, p1, s1));
            entry.terminator = Terminator::Branch {
                target: body,
                args: vec![],
            };
        }

        func.blocks.insert(
            body,
            TirBlock {
                id: body,
                args: vec![],
                ops: vec![make_binop(OpCode::Add, p0, p1, s2)],
                terminator: Terminator::Return { values: vec![s2] },
            },
        );

        let stats = run(&mut func);
        assert!(stats.values_changed > 0);

        let body_ops = &func.blocks[&body].ops;
        assert_eq!(body_ops[0].opcode, OpCode::Copy);
        assert_eq!(body_ops[0].operands[0], s1);
        assert_eq!(body_ops[0].results[0], s2);
    }

    /// Diamond:
    ///   entry: cond branch → then / else
    ///   then:  s1 = p0 + p1
    ///   else:  s2 = p0 + p1     ← NOT dominated by `then`, must NOT dedup
    ///   merge: return s_phi
    /// → s2 must remain a real Add; only entry-defined values may flow into
    ///   sibling blocks, and `then` does not dominate `else`.
    #[test]
    fn non_dominating_no_dedup() {
        let mut func = TirFunction::new(
            "f".into(),
            vec![TirType::I64, TirType::I64, TirType::Bool],
            TirType::I64,
        );
        let p0 = ValueId(0);
        let p1 = ValueId(1);
        let cond = ValueId(2);
        let then_b = func.fresh_block();
        let else_b = func.fresh_block();
        let merge_b = func.fresh_block();

        let s1 = func.fresh_value();
        let s2 = func.fresh_value();
        let merge_arg = func.fresh_value();

        {
            let entry = func.blocks.get_mut(&func.entry_block).unwrap();
            entry.terminator = Terminator::CondBranch {
                cond,
                then_block: then_b,
                then_args: vec![],
                else_block: else_b,
                else_args: vec![],
            };
        }

        func.blocks.insert(
            then_b,
            TirBlock {
                id: then_b,
                args: vec![],
                ops: vec![make_binop(OpCode::Add, p0, p1, s1)],
                terminator: Terminator::Branch {
                    target: merge_b,
                    args: vec![s1],
                },
            },
        );

        func.blocks.insert(
            else_b,
            TirBlock {
                id: else_b,
                args: vec![],
                ops: vec![make_binop(OpCode::Add, p0, p1, s2)],
                terminator: Terminator::Branch {
                    target: merge_b,
                    args: vec![s2],
                },
            },
        );

        func.blocks.insert(
            merge_b,
            TirBlock {
                id: merge_b,
                args: vec![TirValue {
                    id: merge_arg,
                    ty: TirType::I64,
                }],
                ops: vec![],
                terminator: Terminator::Return {
                    values: vec![merge_arg],
                },
            },
        );

        let _stats = run(&mut func);

        // Both sibling adds must remain real Add ops. `then` does not
        // dominate `else` (and vice versa), so neither may be replaced
        // with a Copy of the other.
        assert_eq!(
            func.blocks[&then_b].ops[0].opcode,
            OpCode::Add,
            "then-block add must not be deduped"
        );
        assert_eq!(
            func.blocks[&else_b].ops[0].opcode,
            OpCode::Add,
            "else-block add must not be deduped (then does not dominate else)"
        );
    }

    /// entry  → then → merge
    ///       → else → merge
    /// `entry` defines `e = p0 + p1`.  Both `then` and `else` recompute
    /// `p0 + p1`.  Both must dedup against `e` (entry dominates both).
    #[test]
    fn dominator_value_propagates_to_both_branches() {
        let mut func = TirFunction::new(
            "f".into(),
            vec![TirType::I64, TirType::I64, TirType::Bool],
            TirType::I64,
        );
        let p0 = ValueId(0);
        let p1 = ValueId(1);
        let cond = ValueId(2);
        let then_b = func.fresh_block();
        let else_b = func.fresh_block();
        let merge_b = func.fresh_block();
        let e = func.fresh_value();
        let s1 = func.fresh_value();
        let s2 = func.fresh_value();
        let merge_arg = func.fresh_value();

        {
            let entry = func.blocks.get_mut(&func.entry_block).unwrap();
            entry.ops.push(make_binop(OpCode::Add, p0, p1, e));
            entry.terminator = Terminator::CondBranch {
                cond,
                then_block: then_b,
                then_args: vec![],
                else_block: else_b,
                else_args: vec![],
            };
        }

        func.blocks.insert(
            then_b,
            TirBlock {
                id: then_b,
                args: vec![],
                ops: vec![make_binop(OpCode::Add, p0, p1, s1)],
                terminator: Terminator::Branch {
                    target: merge_b,
                    args: vec![s1],
                },
            },
        );

        func.blocks.insert(
            else_b,
            TirBlock {
                id: else_b,
                args: vec![],
                ops: vec![make_binop(OpCode::Add, p0, p1, s2)],
                terminator: Terminator::Branch {
                    target: merge_b,
                    args: vec![s2],
                },
            },
        );

        func.blocks.insert(
            merge_b,
            TirBlock {
                id: merge_b,
                args: vec![TirValue {
                    id: merge_arg,
                    ty: TirType::I64,
                }],
                ops: vec![],
                terminator: Terminator::Return {
                    values: vec![merge_arg],
                },
            },
        );

        let stats = run(&mut func);
        assert!(stats.values_changed >= 2);
        assert_eq!(func.blocks[&then_b].ops[0].opcode, OpCode::Copy);
        assert_eq!(func.blocks[&then_b].ops[0].operands[0], e);
        assert_eq!(func.blocks[&else_b].ops[0].opcode, OpCode::Copy);
        assert_eq!(func.blocks[&else_b].ops[0].operands[0], e);
    }

    /// Cross-block dedup must NOT escape the entry block when the
    /// "supposedly redundant" computation lives in a block that
    /// post-dominates entry but is itself a loop header — block args
    /// (phi values) coming in from the back edge are NOT visible in
    /// entry's value table, so dominator-scoped GVN naturally skips them.
    /// This regression guards against accidentally numbering loop-carried
    /// values as constants from the preheader.
    #[test]
    fn loop_header_back_edge_not_deduped() {
        let mut func = TirFunction::new("f".into(), vec![TirType::I64], TirType::I64);
        let p0 = ValueId(0);
        let header = func.fresh_block();
        let body = func.fresh_block();
        let exit = func.fresh_block();
        let header_arg = func.fresh_value();
        let bumped = func.fresh_value();
        let one = func.fresh_value();
        let one_in_body = func.fresh_value();

        {
            let entry = func.blocks.get_mut(&func.entry_block).unwrap();
            // Branch to header, threading p0 as the loop-carried value.
            entry.terminator = Terminator::Branch {
                target: header,
                args: vec![p0],
            };
        }

        func.blocks.insert(
            header,
            TirBlock {
                id: header,
                args: vec![TirValue {
                    id: header_arg,
                    ty: TirType::I64,
                }],
                ops: vec![make_const_int(1, one)],
                terminator: Terminator::CondBranch {
                    cond: one,
                    then_block: body,
                    then_args: vec![],
                    else_block: exit,
                    else_args: vec![],
                },
            },
        );

        func.blocks.insert(
            body,
            TirBlock {
                id: body,
                args: vec![],
                // body redefines `1` — same constant, but belongs to a different
                // dominator scope from entry's standpoint.  GVN should still
                // dedup against the header's `one` because header dominates body.
                ops: vec![
                    make_const_int(1, one_in_body),
                    make_binop(OpCode::Add, header_arg, one_in_body, bumped),
                ],
                terminator: Terminator::Branch {
                    target: header,
                    args: vec![bumped],
                },
            },
        );

        func.blocks.insert(
            exit,
            TirBlock {
                id: exit,
                args: vec![],
                ops: vec![],
                terminator: Terminator::Return {
                    values: vec![header_arg],
                },
            },
        );

        let _stats = run(&mut func);

        // body's `one_in_body` must be deduped against header's `one`
        // (header dominates body), but the loop-carried `bumped` must
        // remain a real Add (its operand `header_arg` is a phi).
        assert_eq!(func.blocks[&body].ops[0].opcode, OpCode::Copy);
        assert_eq!(func.blocks[&body].ops[0].operands[0], one);
        assert_eq!(
            func.blocks[&body].ops[1].opcode,
            OpCode::Add,
            "phi-fed Add must not be folded"
        );
    }

    /// Mirrors the `bench_struct` body-block pattern: the loop-carried
    /// induction variable `i: I64` participates in two structurally
    /// identical `i + 1` computations in the same block (one for `p.y =
    /// i + 1`, one for the `i += 1` increment). GVN must collapse the
    /// second into a Copy of the first — within the same block, two
    /// typed Adds with identical operands are equivalent regardless of
    /// whether the operand is a phi-fed loop-carried value.
    ///
    /// Locks in the contract that drove the dead-store-elim landing:
    /// `bench_struct` performance hinges on this dedup firing.
    ///
    /// The header's branch condition is a `ConstBool` instead of a
    /// `ConstInt(1)` to keep the dom-tree leader table from
    /// inadvertently aliasing the body's `1` literals against the
    /// branch cond — the assertion targets `i + 1` dedup, not constant
    /// folding across blocks.
    #[test]
    fn redundant_add_in_loop_body_dedups() {
        let mut func = TirFunction::new("f".into(), vec![TirType::I64], TirType::I64);
        let p0 = ValueId(0);
        let header = func.fresh_block();
        let body = func.fresh_block();
        let exit = func.fresh_block();
        let i = func.fresh_value();
        let cond = func.fresh_value();
        let one_a = func.fresh_value();
        let one_b = func.fresh_value();
        let plus_a = func.fresh_value();
        let plus_b = func.fresh_value();

        {
            let entry = func.blocks.get_mut(&func.entry_block).unwrap();
            entry.terminator = Terminator::Branch {
                target: header,
                args: vec![p0],
            };
        }

        func.blocks.insert(
            header,
            TirBlock {
                id: header,
                args: vec![TirValue {
                    id: i,
                    ty: TirType::I64,
                }],
                ops: vec![make_const_bool(true, cond)],
                terminator: Terminator::CondBranch {
                    cond,
                    then_block: body,
                    then_args: vec![],
                    else_block: exit,
                    else_args: vec![],
                },
            },
        );

        // Body computes `i + 1` twice. A real bench_struct lowering has
        // a fresh ConstInt SSA for each literal `1`. GVN's intra-block
        // leader table dedups the second ConstInt against the first,
        // then dedups the second `Add(i, 1)` against the first.
        func.blocks.insert(
            body,
            TirBlock {
                id: body,
                args: vec![],
                ops: vec![
                    make_const_int(1, one_a),
                    make_binop(OpCode::Add, i, one_a, plus_a),
                    make_const_int(1, one_b),
                    make_binop(OpCode::Add, i, one_b, plus_b),
                ],
                terminator: Terminator::Branch {
                    target: header,
                    args: vec![plus_b],
                },
            },
        );

        func.blocks.insert(
            exit,
            TirBlock {
                id: exit,
                args: vec![],
                ops: vec![],
                terminator: Terminator::Return { values: vec![i] },
            },
        );

        let _stats = run(&mut func);

        let body_ops = &func.blocks[&body].ops;
        assert_eq!(
            body_ops[0].opcode,
            OpCode::ConstInt,
            "first `1` literal stays as a const"
        );
        assert_eq!(
            body_ops[1].opcode,
            OpCode::Add,
            "first `i + 1` becomes the leader"
        );
        assert_eq!(
            body_ops[2].opcode,
            OpCode::Copy,
            "second ConstInt(1) collapses to the first"
        );
        assert_eq!(body_ops[2].operands[0], one_a);
        assert_eq!(
            body_ops[3].opcode,
            OpCode::Copy,
            "second `i + 1` collapses to the first Add"
        );
        assert_eq!(body_ops[3].operands[0], plus_a);
    }

    /// Sibling blocks that each define the same constant must NOT see each
    /// other's leaders.  After the dom-tree walk pops the first sibling, the
    /// second sibling enters with a clean (parent-scope) leader table.
    #[test]
    fn scope_pops_after_sibling() {
        let mut func = TirFunction::new(
            "f".into(),
            vec![TirType::Bool],
            TirType::I64,
        );
        let cond = ValueId(0);
        let then_b = func.fresh_block();
        let else_b = func.fresh_block();
        let merge_b = func.fresh_block();
        let c_then = func.fresh_value();
        let c_else = func.fresh_value();
        let merge_arg = func.fresh_value();

        {
            let entry = func.blocks.get_mut(&func.entry_block).unwrap();
            entry.terminator = Terminator::CondBranch {
                cond,
                then_block: then_b,
                then_args: vec![],
                else_block: else_b,
                else_args: vec![],
            };
        }

        func.blocks.insert(
            then_b,
            TirBlock {
                id: then_b,
                args: vec![],
                ops: vec![make_const_int(7, c_then)],
                terminator: Terminator::Branch {
                    target: merge_b,
                    args: vec![c_then],
                },
            },
        );

        func.blocks.insert(
            else_b,
            TirBlock {
                id: else_b,
                args: vec![],
                ops: vec![make_const_int(7, c_else)],
                terminator: Terminator::Branch {
                    target: merge_b,
                    args: vec![c_else],
                },
            },
        );

        func.blocks.insert(
            merge_b,
            TirBlock {
                id: merge_b,
                args: vec![TirValue {
                    id: merge_arg,
                    ty: TirType::I64,
                }],
                ops: vec![],
                terminator: Terminator::Return {
                    values: vec![merge_arg],
                },
            },
        );

        let _stats = run(&mut func);

        // Neither sibling block dominates the other, so each ConstInt 7
        // must remain a ConstInt (not a Copy of the other).
        assert_eq!(func.blocks[&then_b].ops[0].opcode, OpCode::ConstInt);
        assert_eq!(func.blocks[&else_b].ops[0].opcode, OpCode::ConstInt);
    }

    /// `make_const_bool` is exercised here to ensure that AttrValue::Bool
    /// (vs AttrValue::Int) discrimination is preserved across blocks.
    /// Regression guard for commit 8662b45f.
    #[test]
    fn cross_block_const_bool_dedup() {
        let mut func = TirFunction::new("f".into(), vec![], TirType::Bool);
        let body = func.fresh_block();
        let b1 = func.fresh_value();
        let b2 = func.fresh_value();

        {
            let entry = func.blocks.get_mut(&func.entry_block).unwrap();
            entry.ops.push(make_const_bool(false, b1));
            entry.terminator = Terminator::Branch {
                target: body,
                args: vec![],
            };
        }

        func.blocks.insert(
            body,
            TirBlock {
                id: body,
                args: vec![],
                ops: vec![make_const_bool(false, b2)],
                terminator: Terminator::Return { values: vec![b2] },
            },
        );

        let stats = run(&mut func);
        assert!(stats.values_changed > 0);
        assert_eq!(func.blocks[&body].ops[0].opcode, OpCode::Copy);
        assert_eq!(func.blocks[&body].ops[0].operands[0], b1);
    }
}
