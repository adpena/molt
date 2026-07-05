use std::collections::{HashMap, HashSet};

use crate::tir::analysis::{AnalysisManager, DomChildren, ImmediateDoms, StrictReachable};
use crate::tir::blocks::BlockId;
use crate::tir::dominators::dominates;
use crate::tir::function::TirFunction;
use crate::tir::op_kinds_generated::{
    GvnNumberingRole, opcode_gvn_numbering_role_table, opcode_gvn_value_key_spec_table,
    opcode_operand_independent_result_tir_type,
};
use crate::tir::ops::{Dialect, OpCode, TirOp};
use crate::tir::values::ValueId;

use super::super::PassStats;
use super::keys::{ValueKey, gvn_value_key, gvn_value_key_from_spec, is_primitive_type};

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

                    let numbering_role = opcode_gvn_numbering_role_table(op.opcode);
                    let numberable = match numbering_role {
                        GvnNumberingRole::Always => true,
                        GvnNumberingRole::TypeGated => {
                            // Arithmetic/comparison/boolean ops are only numberable
                            // when ALL operands are proven primitive types. On DynBox
                            // operands, these ops may trigger dunder methods with side
                            // effects (__add__, __eq__, etc.).
                            op.operands
                                .iter()
                                .all(|v| value_type.get(v).is_some_and(is_primitive_type))
                        }
                        GvnNumberingRole::ValueKeyedConstant | GvnNumberingRole::Never => false,
                    };

                    if !numberable {
                        if numbering_role.is_value_keyed_constant()
                            && let Some(constant_key) = gvn_value_key(op)
                        {
                            let key = ValueKey {
                                opcode: op.opcode,
                                operands: Vec::new(),
                                attr_key: Some(constant_key),
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

                    let attr_key = match opcode_gvn_value_key_spec_table(op.opcode) {
                        Some(spec) => match gvn_value_key_from_spec(op, spec) {
                            Some(key) => Some(key),
                            None => {
                                let prior = value_number.insert(result, result);
                                vn_undo.push((result, prior));
                                continue;
                            }
                        },
                        None => None,
                    };

                    let key = ValueKey {
                        opcode: op.opcode,
                        operands: numbered_operands,
                        attr_key,
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
            let old = block.ops[*op_idx].clone();
            let result = old.results[0];
            let mut replacement = TirOp {
                dialect: Dialect::Molt,
                opcode: OpCode::Copy,
                operands: vec![*leader],
                results: vec![result],
                attrs: Default::default(),
                source_span: None,
            };
            replacement.inherit_source_from(&old);
            block.ops[*op_idx] = replacement;
            stats.values_changed += 1;
        }
    }

    // Operand renaming is deferred to copy_prop + DCE.  Direct operand
    // replacement requires per-use dominance checks; the Copy ops emitted
    // above are sufficient — copy_prop will resolve them, and DCE will
    // clean up the now-dead original ops.

    stats
}
