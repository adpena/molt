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

use std::collections::{HashMap, HashSet};

use super::PassStats;
use crate::tir::analysis::{AnalysisManager, DomChildren, ImmediateDoms, StrictReachable};
use crate::tir::blocks::BlockId;
use crate::tir::dominators::dominates;
use crate::tir::function::TirFunction;
use crate::tir::op_kinds_generated::{
    opcode_is_gvn_value_keyed_constant_table, opcode_operand_independent_result_tir_type,
};
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
    matches!(opcode, OpCode::BoxVal | OpCode::UnboxVal)
}

/// Returns `true` if the opcode is numberable when operands are proven typed.
///
/// The opcode-level purity decision is delegated to the single source of truth
/// in `effects::opcode_is_type_gated_numberable`: a deterministic,
/// side-effect-free computation whose purity is conditional on its operands
/// being primitive types. The *type* precondition itself (operands proven
/// primitive) is enforced separately at the call site via `is_primitive_type`,
/// because it depends on the per-value type map rather than the opcode.
///
/// Note this family includes `Div`/`FloorDiv`/`Mod`/`Pow`, which may raise
/// `ZeroDivisionError`. CSE is still sound for them: GVN only replaces a
/// duplicate that is *dominated* by its leader, so if the leader raises, the
/// replaced op is never reached — the throw is preserved. (This is why GVN can
/// number a may-throw op that LICM must not hoist.)
fn is_typed_numberable(opcode: OpCode) -> bool {
    super::effects::opcode_is_type_gated_numberable(opcode)
}

/// A type is "primitive" when arithmetic on it is provably side-effect-free.
fn is_primitive_type(ty: &crate::tir::types::TirType) -> bool {
    use crate::tir::types::TirType;
    matches!(
        ty,
        TirType::I64 | TirType::F64 | TirType::Bool | TirType::None
    )
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

pub fn run(func: &mut TirFunction, am: &mut AnalysisManager) -> PassStats {
    let mut stats = PassStats {
        name: "gvn",
        ..Default::default()
    };

    if func.blocks.len() <= 1 && func.blocks.values().all(|b| b.ops.is_empty()) {
        return stats;
    }

    // Dominator tree (exception-edge-aware) + dom-children, from the analysis
    // manager. The idom tree and its children map share a single dominator
    // computation across GVN/LICM/BCE/refcount-elim.
    let idoms = am.get::<ImmediateDoms>(func).clone();
    let dom_children = am.get::<DomChildren>(func).clone();

    // Strict-CFG reachability (terminator-only). Cross-block replacements are
    // only safe when the use site is reachable via terminators — that is the
    // reachability `verify_lir` uses; emitting `Copy(leader)` into a block
    // reachable only through exception edges would make the verifier reject the
    // new operand. Blocks reachable only via exception edges still get
    // intra-block GVN (their leaders never escape their own scope).
    let strict_reachable = am.get::<StrictReachable>(func).clone();

    // Build a value→type map from STRUCTURALLY GUARANTEED sources only:
    // block args (set by type_refine), operand-independent result-type opcodes,
    // and function params.
    // NO speculative type inference — if a value's type isn't provably
    // known, it's treated as DynBox (not primitive, not safe to number).
    let mut value_type: HashMap<ValueId, crate::tir::types::TirType> = HashMap::new();
    {
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
        // Generated opcode facts own operand-independent result types.
        for block in func.blocks.values() {
            for op in &block.ops {
                if let Some(t) = opcode_operand_independent_result_tir_type(op.opcode) {
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
                // Constants are intentionally not replaced by Copy ops:
                // backend-native constant materialization is representation
                // sensitive, and cross-block constant copies can violate the
                // stricter post-lowering dominance verifier for exception-only
                // handler blocks.  We still give same-block duplicate constants
                // a block-local value number so expressions like two adjacent
                // `i + 1` computations CSE without emitting a constant Copy or
                // leaking the constant leader into dominated child blocks.
                let mut local_const_key_to_leader: HashMap<ValueKey, ValueId> = HashMap::new();
                let mut local_value_number: HashMap<ValueId, ValueId> = HashMap::new();

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
                        if opcode_is_gvn_value_keyed_constant_table(op.opcode) {
                            let (const_int_key, const_str_key, const_bytes_key) = const_keys(op);
                            let key = ValueKey {
                                opcode: op.opcode,
                                operands: Vec::new(),
                                const_int_key,
                                const_str_key,
                                const_bytes_key,
                            };
                            let leader = local_const_key_to_leader
                                .get(&key)
                                .copied()
                                .unwrap_or(result);
                            local_const_key_to_leader.entry(key).or_insert(result);
                            local_value_number.insert(result, leader);
                        }
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
                        .map(|v| {
                            local_value_number
                                .get(v)
                                .copied()
                                .or_else(|| value_number.get(v).copied())
                                .unwrap_or(*v)
                        })
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
            && *op_idx < block.ops.len()
        {
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
        entry.terminator = Terminator::Return { values: vec![sum2] };

        let stats = run(&mut func, &mut crate::tir::analysis::AnalysisManager::new());
        assert!(stats.values_changed > 0);

        // sum2's definition should now be a Copy from sum1.
        let ops = &func.blocks[&func.entry_block].ops;
        assert_eq!(ops[1].opcode, OpCode::Copy);
        assert_eq!(ops[1].operands[0], sum1);
    }

    #[test]
    fn duplicate_constants_not_folded_by_gvn() {
        let mut func = TirFunction::new("f".into(), vec![], TirType::I64);
        let c1 = func.fresh_value();
        let c2 = func.fresh_value(); // same constant as c1

        let entry = func.blocks.get_mut(&func.entry_block).unwrap();
        entry.ops.push(make_const_int(42, c1));
        entry.ops.push(make_const_int(42, c2));
        entry.terminator = Terminator::Return { values: vec![c2] };

        let stats = run(&mut func, &mut crate::tir::analysis::AnalysisManager::new());

        // Constants are intentionally left as constants. Backends handle
        // safe constant pooling in backend-native form; GVN must not create
        // cross-control-flow Copy dependencies for constants.
        let ops = &func.blocks[&func.entry_block].ops;
        assert_eq!(stats.values_changed, 0);
        assert_eq!(ops[1].opcode, OpCode::ConstInt);
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

        let stats = run(&mut func, &mut crate::tir::analysis::AnalysisManager::new());
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

        let _stats = run(&mut func, &mut crate::tir::analysis::AnalysisManager::new());

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

        let _stats = run(&mut func, &mut crate::tir::analysis::AnalysisManager::new());

        // Both calls must remain — not folded.
        let ops = &func.blocks[&func.entry_block].ops;
        assert_eq!(ops[0].opcode, OpCode::Call);
        assert_eq!(ops[1].opcode, OpCode::Call);
    }

    // ── Cross-block dominator-scoped GVN tests ──────────────────────────

    /// entry: c1 = ConstInt 42; branch body
    /// body:  c2 = ConstInt 42; return c2
    /// → constants stay backend-native constants. GVN must not replace c2
    ///   with Copy(c1), even though entry strictly dominates body.
    #[test]
    fn cross_block_redundant_constant_not_folded() {
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

        let stats = run(&mut func, &mut crate::tir::analysis::AnalysisManager::new());
        assert_eq!(stats.values_changed, 0);

        // c2 in `body` remains a backend-native constant.
        let body_ops = &func.blocks[&body].ops;
        assert_eq!(body_ops[0].opcode, OpCode::ConstInt);
        assert_eq!(body_ops[0].results[0], c2);
        let _ = c1;
    }

    /// entry: s1 = p0 + p1; branch body
    /// body:  s2 = p0 + p1; return s2
    /// → s2 should become Copy(s1).
    #[test]
    fn cross_block_redundant_arithmetic() {
        let mut func = TirFunction::new("f".into(), vec![TirType::I64, TirType::I64], TirType::I64);
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

        let stats = run(&mut func, &mut crate::tir::analysis::AnalysisManager::new());
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

        let _stats = run(&mut func, &mut crate::tir::analysis::AnalysisManager::new());

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

        let stats = run(&mut func, &mut crate::tir::analysis::AnalysisManager::new());
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

        let _stats = run(&mut func, &mut crate::tir::analysis::AnalysisManager::new());

        // body constants are not replaced with cross-block copies. The
        // loop-carried `bumped` must remain a real Add because its operand
        // `header_arg` is a phi.
        assert_eq!(func.blocks[&body].ops[0].opcode, OpCode::ConstInt);
        assert_eq!(
            func.blocks[&body].ops[1].opcode,
            OpCode::Add,
            "phi-fed Add must not be folded"
        );
        let _ = one;
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
        // a fresh ConstInt SSA for each literal `1`. GVN must not replace
        // the second ConstInt with a Copy, but its block-local constant
        // value number lets the second `Add(i, 1)` dedup against the first.
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

        let _stats = run(&mut func, &mut crate::tir::analysis::AnalysisManager::new());

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
            OpCode::ConstInt,
            "second ConstInt(1) stays backend-native"
        );
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
        let mut func = TirFunction::new("f".into(), vec![TirType::Bool], TirType::I64);
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

        let _stats = run(&mut func, &mut crate::tir::analysis::AnalysisManager::new());

        // Neither sibling block dominates the other, so each ConstInt 7
        // must remain a ConstInt (not a Copy of the other).
        assert_eq!(func.blocks[&then_b].ops[0].opcode, OpCode::ConstInt);
        assert_eq!(func.blocks[&else_b].ops[0].opcode, OpCode::ConstInt);
    }

    /// `make_const_bool` is exercised here to ensure that constants are not
    /// replaced across blocks. Bool-vs-int discrimination is still represented
    /// by the constant opcode and attributes, not by a cross-block Copy.
    #[test]
    fn cross_block_const_bool_not_folded() {
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

        let stats = run(&mut func, &mut crate::tir::analysis::AnalysisManager::new());
        assert_eq!(stats.values_changed, 0);
        assert_eq!(func.blocks[&body].ops[0].opcode, OpCode::ConstBool);
        let _ = b1;
    }
}
