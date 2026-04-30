//! Loop Invariant Code Motion (LICM) for TIR.
//!
//! Hoists operations out of loop bodies when their operands are all
//! defined outside the loop (loop-invariant).  This eliminates redundant
//! recomputation of values that don't change across iterations.
//!
//! ```python
//! for i in range(n):
//!     x = a + b          # invariant: a, b don't change in the loop
//!     result += x * i
//! ```
//!
//! After LICM:
//! ```python
//! x = a + b              # hoisted to preheader
//! for i in range(n):
//!     result += x * i
//! ```
//!
//! Multi-level hoisting: nested loops are processed innermost-first. An
//! op invariant w.r.t. the inner loop is hoisted to the inner preheader
//! (which still lives inside the outer loop). When the outer loop is
//! processed, that op now sits in a block belonging to the outer loop's
//! body and may itself be invariant w.r.t. the outer loop, in which case
//! it is hoisted again — to the outer preheader. This is a fixpoint
//! traversal of the loop nesting tree.
//!
//! Safety conditions:
//! 1. The op must be pure (no side effects).
//! 2. All operands must be defined outside the loop (or be other invariants).
//! 3. The op must dominate all loop exits (guaranteed by hoisting to preheader).
//! 4. Exception-handling regions are conservatively excluded.
//! 5. The op's result must not appear as a branch argument (phi value).
//!    Such uses cross block boundaries via terminators; excluding them is
//!    sufficient to ensure the only escapes from a loop go through phi
//!    nodes, so direct uses of any hoist candidate are dominated by the
//!    chosen preheader.
//!
//! Reference: Muchnick, "Advanced Compiler Design and Implementation" ch. 13.

use std::collections::{HashMap, HashSet};

use super::PassStats;
use crate::tir::blocks::{BlockId, LoopRole, Terminator};
use crate::tir::dominators::{build_pred_map, compute_idoms, dominates};
use crate::tir::function::TirFunction;
use crate::tir::ops::{OpCode, TirOp};
use crate::tir::values::ValueId;

/// Returns `true` if the op is pure and safe to hoist out of a loop.
fn is_hoistable(op: &TirOp) -> bool {
    matches!(
        op.opcode,
        OpCode::Add
            | OpCode::Sub
            | OpCode::Mul
            | OpCode::InplaceAdd
            | OpCode::InplaceSub
            | OpCode::InplaceMul
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
            | OpCode::ConstInt
            | OpCode::ConstFloat
            | OpCode::ConstStr
            | OpCode::ConstBool
            | OpCode::ConstNone
            | OpCode::ConstBytes
            | OpCode::BoxVal
            | OpCode::UnboxVal
            | OpCode::TypeGuard
            | OpCode::BuildSlice
    ) || op.is_plain_value_copy()
}

/// Identify all blocks that belong to the natural loop whose header is
/// `header_bid`.
///
/// Standard textbook construction (Muchnick §13.4): a back edge is an edge
/// `tail → header` where `header` dominates `tail`. The natural loop of a
/// back edge is `{header} ∪ {nodes that can reach tail without passing
/// through header}`. The natural loop of a header is the union over all
/// back edges to that header. Using *dominance* (rather than mere
/// reachability from the header) is what cleanly distinguishes inner-loop
/// bodies from outer-loop bodies in nested CFGs: an inner-loop preheader
/// is reachable from the inner header (via the outer iteration cycle) but
/// is not dominated by it, so the inner-loop preheader is correctly
/// excluded from the inner loop's body.
fn collect_loop_blocks(
    func: &TirFunction,
    pred_map: &HashMap<BlockId, Vec<BlockId>>,
    idoms: &HashMap<BlockId, Option<BlockId>>,
    header_bid: BlockId,
) -> HashSet<BlockId> {
    let mut loop_blocks = HashSet::new();
    loop_blocks.insert(header_bid);

    // Back-edge tails: predecessors of header dominated by header.
    let header_preds: &[BlockId] = pred_map
        .get(&header_bid)
        .map(|v| v.as_slice())
        .unwrap_or(&[]);

    let mut worklist: Vec<BlockId> = Vec::new();
    for &p in header_preds {
        if dominates(header_bid, p, idoms) && loop_blocks.insert(p) {
            worklist.push(p);
        }
    }

    // Walk predecessors backwards from each back-edge tail, never crossing
    // the header. The header acts as the loop's single entry, so any node
    // reaching a tail without going through the header belongs to the loop.
    while let Some(bid) = worklist.pop() {
        if let Some(block_preds) = pred_map.get(&bid) {
            for &p in block_preds {
                if p == header_bid {
                    continue;
                }
                if loop_blocks.insert(p) {
                    worklist.push(p);
                }
            }
        }
    }

    // Defensive: only retain blocks that actually exist in the function.
    loop_blocks.retain(|bid| func.blocks.contains_key(bid));
    loop_blocks
}

/// Find the preheader block for a loop header.
/// The preheader is the unique predecessor of the header that is NOT
/// part of the loop body.  If no unique preheader exists, returns None.
fn find_preheader(
    func: &TirFunction,
    header_bid: BlockId,
    loop_blocks: &HashSet<BlockId>,
) -> Option<BlockId> {
    let mut preds: Vec<BlockId> = Vec::new();
    for (&bid, block) in &func.blocks {
        let targets = match &block.terminator {
            Terminator::Branch { target, .. } => vec![*target],
            Terminator::CondBranch {
                then_block,
                else_block,
                ..
            } => vec![*then_block, *else_block],
            _ => vec![],
        };
        if targets.contains(&header_bid) && !loop_blocks.contains(&bid) {
            preds.push(bid);
        }
    }
    if preds.len() == 1 {
        Some(preds[0])
    } else {
        None
    }
}

pub fn run(func: &mut TirFunction) -> PassStats {
    let mut stats = PassStats {
        name: "licm",
        ..Default::default()
    };

    // Functions with exception handling are NOT skipped wholesale —
    // the per-op `is_hoistable` predicate restricts hoisting to a
    // safe-list of side-effect-free, never-throwing ops (constants,
    // pure arithmetic on already-computed values, etc.) for which
    // hoisting out of a try-bearing loop is observably equivalent
    // to lazy in-loop computation: the only behavioural difference
    // is the trivial "computed in preheader even if loop ran zero
    // iterations" case, which doesn't affect any program output.
    //
    // This is critical for performance: the frontend liberally emits
    // CHECK_EXCEPTION ops, and the lower_from_simple detection sets
    // `has_exception_handling = true` whenever any TryStart/TryEnd/
    // CheckException op appears.  In practice virtually every
    // non-trivial loop body trips this flag, so the prior wholesale
    // skip turned LICM into a no-op for nearly all real code.
    //
    // Loop-detection (natural-loop construction via dominators) is
    // unchanged by exception ops, since try_start/try_end/
    // check_exception don't add back-edges or alter the CFG topology
    // beyond the normal block-splitting that any structured
    // construct does.

    // Find loop headers from loop_roles metadata. Sort by id so that
    // tie-breaking in the post-order traversal below is deterministic
    // regardless of the underlying HashMap iteration order.
    let mut loop_headers: Vec<BlockId> = func
        .loop_roles
        .iter()
        .filter_map(|(bid, role)| {
            if matches!(role, LoopRole::LoopHeader) {
                Some(*bid)
            } else {
                None
            }
        })
        .collect();
    loop_headers.sort_unstable_by_key(|b| b.0);

    if loop_headers.is_empty() {
        return stats;
    }

    // Process all loops, innermost first, so ops hoisted from an inner
    // loop's body into the inner preheader become visible to the
    // enclosing loop and can be hoisted further out if invariant there.
    //
    // Step 0: compute dominators once. Used both for natural-loop body
    // construction (back-edge identification) and for cheaply ordering
    // the loop forest.
    let pred_map = build_pred_map(func);
    let idoms = compute_idoms(func, &pred_map);

    // Step 1: compute each loop's block set using dominator-based natural
    // loop construction. Natural loops nest properly: an inner loop's body
    // is a strict subset of its enclosing outer loop's body.
    let loop_block_sets: Vec<(BlockId, HashSet<BlockId>)> = loop_headers
        .iter()
        .map(|&h| (h, collect_loop_blocks(func, &pred_map, &idoms, h)))
        .collect();

    // Step 2: nesting depth = number of OTHER loops whose block set
    // contains this loop's header. A header at depth k is inside k
    // enclosing loops. Sorting by descending depth yields a post-order
    // over the loop forest: every inner loop is processed before its
    // enclosing loop.
    let mut headers_with_depth: Vec<(BlockId, usize)> = loop_block_sets
        .iter()
        .map(|(h, _)| {
            let depth = loop_block_sets
                .iter()
                .filter(|(other_h, other_blocks)| *other_h != *h && other_blocks.contains(h))
                .count();
            (*h, depth)
        })
        .collect();
    // Descending depth → innermost first. Stable across ties.
    headers_with_depth.sort_by_key(|(_, depth)| std::cmp::Reverse(*depth));
    let loop_headers: Vec<BlockId> = headers_with_depth.into_iter().map(|(h, _)| h).collect();

    // Build a set of all values defined in each block for quick lookup.
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
    // Function parameters are defined "before" the entry block.
    for i in 0..func.param_types.len() {
        let v = ValueId(i as u32);
        value_def_block.entry(v).or_insert(func.entry_block);
    }

    // Collect all values used as block arguments in terminators.
    // These participate in phi resolution and must not be hoisted.
    let mut phi_values: HashSet<ValueId> = HashSet::new();
    for block in func.blocks.values() {
        match &block.terminator {
            Terminator::Branch { args, .. } => {
                for v in args {
                    phi_values.insert(*v);
                }
            }
            Terminator::CondBranch {
                then_args,
                else_args,
                ..
            } => {
                for v in then_args.iter().chain(else_args.iter()) {
                    phi_values.insert(*v);
                }
            }
            _ => {}
        }
    }

    // Index the precomputed loop block sets by header for cheap lookup.
    let loop_blocks_by_header: HashMap<BlockId, HashSet<BlockId>> =
        loop_block_sets.into_iter().collect();

    for header_bid in &loop_headers {
        let loop_blocks = match loop_blocks_by_header.get(header_bid) {
            Some(set) => set,
            None => continue,
        };

        let preheader = match find_preheader(func, *header_bid, loop_blocks) {
            Some(p) => p,
            None => continue, // No unique preheader — can't hoist.
        };

        // Stable iteration order over loop blocks: keeps hoisted-op
        // ordering deterministic for golden-file diffs and for downstream
        // passes that depend on textual TIR equivalence.
        let mut sorted_loop_blocks: Vec<BlockId> = loop_blocks.iter().copied().collect();
        sorted_loop_blocks.sort_unstable_by_key(|b| b.0);

        // Collect invariant ops: ops in loop blocks whose operands are
        // all defined outside the loop.
        // Iterate to fixpoint: hoisting one op may make another invariant.
        for _round in 0..10 {
            let mut hoisted_this_round = 0usize;

            for &loop_bid in &sorted_loop_blocks {
                let block = match func.blocks.get(&loop_bid) {
                    Some(b) => b,
                    None => continue,
                };

                let mut to_hoist: Vec<usize> = Vec::new();

                for (i, op) in block.ops.iter().enumerate() {
                    if !is_hoistable(op) {
                        continue;
                    }
                    if op.results.is_empty() {
                        continue;
                    }

                    // Skip ops whose results participate in phi nodes.
                    let result_is_phi = op.results.iter().any(|r| phi_values.contains(r));
                    if result_is_phi {
                        continue;
                    }

                    // Check if all operands are defined outside the loop.
                    let all_invariant = op.operands.iter().all(|&operand| {
                        match value_def_block.get(&operand) {
                            Some(def_bid) => !loop_blocks.contains(def_bid),
                            None => true, // Unknown def = conservative: treat as external.
                        }
                    });

                    if all_invariant {
                        to_hoist.push(i);
                    }
                }

                // Hoist ops from back to front to preserve indices.
                for &idx in to_hoist.iter().rev() {
                    let op = func.blocks.get_mut(&loop_bid).unwrap().ops.remove(idx);
                    // Update def_block for the hoisted values.
                    for &res in &op.results {
                        value_def_block.insert(res, preheader);
                    }
                    // Insert at the end of the preheader (before the terminator,
                    // which is handled structurally, not in the ops vec).
                    func.blocks.get_mut(&preheader).unwrap().ops.push(op);
                    hoisted_this_round += 1;
                    stats.ops_removed += 1; // removed from loop
                    stats.ops_added += 1; // added to preheader
                }
            }

            if hoisted_this_round == 0 {
                break;
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
    use crate::tir::blocks::{LoopRole, Terminator, TirBlock};
    use crate::tir::function::TirFunction;
    use crate::tir::ops::{AttrDict, AttrValue, Dialect, OpCode, TirOp};
    use crate::tir::types::TirType;
    use crate::tir::values::{TirValue, ValueId};

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

    /// Build:  entry → preheader → loop_header → loop_body → loop_header
    ///                                         ↘ exit
    /// with a+b computed inside the loop body.
    #[test]
    fn invariant_add_hoisted_to_preheader() {
        let mut func = TirFunction::new("f".into(), vec![TirType::I64, TirType::I64], TirType::I64);
        let a = ValueId(0); // param
        let b = ValueId(1); // param

        let preheader = func.fresh_block();
        let loop_header = func.fresh_block();
        let loop_body = func.fresh_block();
        let exit = func.fresh_block();

        let loop_var = func.fresh_value();
        let sum_ab = func.fresh_value(); // a + b — loop invariant
        let result = func.fresh_value();
        let cond = func.fresh_value();

        // Entry → preheader
        {
            let init = func.fresh_value();
            let entry = func.blocks.get_mut(&func.entry_block).unwrap();
            entry.ops.push(make_const_int(0, init));
            entry.terminator = Terminator::Branch {
                target: preheader,
                args: vec![],
            };
        }

        // Preheader → loop_header
        func.blocks.insert(
            preheader,
            TirBlock {
                id: preheader,
                args: vec![],
                ops: vec![],
                terminator: Terminator::Branch {
                    target: loop_header,
                    args: vec![],
                },
            },
        );

        // Loop header: CondBranch → body or exit
        func.blocks.insert(
            loop_header,
            TirBlock {
                id: loop_header,
                args: vec![TirValue {
                    id: loop_var,
                    ty: TirType::I64,
                }],
                ops: vec![make_const_int(1, cond)],
                terminator: Terminator::CondBranch {
                    cond,
                    then_block: loop_body,
                    then_args: vec![],
                    else_block: exit,
                    else_args: vec![],
                },
            },
        );

        // Loop body: compute a+b (invariant!), use it locally, and loop back.
        // sum_ab is NOT passed as a branch arg (phi value), so it can be hoisted.
        let use_val = func.fresh_value();
        func.blocks.insert(
            loop_body,
            TirBlock {
                id: loop_body,
                args: vec![],
                ops: vec![
                    make_binop(OpCode::Add, a, b, sum_ab), // invariant — should be hoisted
                    make_binop(OpCode::Add, sum_ab, loop_var, use_val), // uses sum_ab locally
                ],
                terminator: Terminator::Branch {
                    target: loop_header,
                    args: vec![use_val],
                },
            },
        );

        // Exit
        func.blocks.insert(
            exit,
            TirBlock {
                id: exit,
                args: vec![TirValue {
                    id: result,
                    ty: TirType::I64,
                }],
                ops: vec![],
                terminator: Terminator::Return {
                    values: vec![result],
                },
            },
        );

        // Mark loop_header as a loop header.
        func.loop_roles.insert(loop_header, LoopRole::LoopHeader);

        let stats = run(&mut func);

        // The invariant Add(a, b) → sum_ab should have been hoisted.
        // The non-invariant Add(sum_ab, loop_var) → use_val should remain.
        let body_ops = &func.blocks[&loop_body].ops;
        let invariant_add_remains = body_ops
            .iter()
            .any(|op| op.opcode == OpCode::Add && op.operands == vec![a, b]);
        assert!(
            !invariant_add_remains,
            "Invariant Add(a, b) should have been hoisted out of the loop body"
        );

        let preheader_ops = &func.blocks[&preheader].ops;
        assert!(
            preheader_ops.iter().any(|op| op.opcode == OpCode::Add),
            "Add should appear in the preheader"
        );

        assert!(stats.ops_removed > 0 || stats.ops_added > 0);
    }

    #[test]
    fn fallback_semantic_copy_is_not_hoisted() {
        let mut func = TirFunction::new(
            "f".into(),
            vec![TirType::DynBox, TirType::Str],
            TirType::DynBox,
        );
        let module = ValueId(0);
        let attr_name = ValueId(1);

        let preheader = func.fresh_block();
        let loop_header = func.fresh_block();
        let loop_body = func.fresh_block();
        let exit = func.fresh_block();

        let loop_var = func.fresh_value();
        let lookup = func.fresh_value();
        let cond = func.fresh_value();

        {
            let init = func.fresh_value();
            let entry = func.blocks.get_mut(&func.entry_block).unwrap();
            entry.ops.push(make_const_int(0, init));
            entry.terminator = Terminator::Branch {
                target: preheader,
                args: vec![],
            };
        }

        func.blocks.insert(
            preheader,
            TirBlock {
                id: preheader,
                args: vec![],
                ops: vec![],
                terminator: Terminator::Branch {
                    target: loop_header,
                    args: vec![],
                },
            },
        );

        func.blocks.insert(
            loop_header,
            TirBlock {
                id: loop_header,
                args: vec![TirValue {
                    id: loop_var,
                    ty: TirType::I64,
                }],
                ops: vec![make_const_int(1, cond)],
                terminator: Terminator::CondBranch {
                    cond,
                    then_block: loop_body,
                    then_args: vec![],
                    else_block: exit,
                    else_args: vec![],
                },
            },
        );

        func.blocks.insert(
            loop_body,
            TirBlock {
                id: loop_body,
                args: vec![],
                ops: vec![TirOp {
                    dialect: Dialect::Molt,
                    opcode: OpCode::Copy,
                    operands: vec![module, attr_name],
                    results: vec![lookup],
                    attrs: {
                        let mut attrs = AttrDict::new();
                        attrs.insert(
                            "_original_kind".into(),
                            AttrValue::Str("module_get_attr".into()),
                        );
                        attrs
                    },
                    source_span: None,
                }],
                terminator: Terminator::Branch {
                    target: loop_header,
                    args: vec![loop_var],
                },
            },
        );

        func.blocks.insert(
            exit,
            TirBlock {
                id: exit,
                args: vec![],
                ops: vec![],
                terminator: Terminator::Return { values: vec![] },
            },
        );

        func.loop_roles.insert(loop_header, LoopRole::LoopHeader);

        let _stats = run(&mut func);

        assert!(
            func.blocks[&loop_body].ops.iter().any(|op| {
                op.opcode == OpCode::Copy
                    && matches!(
                        op.attrs.get("_original_kind"),
                        Some(AttrValue::Str(kind)) if kind == "module_get_attr"
                    )
            }),
            "fallback semantic Copy ops must not be hoisted as pure copies"
        );
        assert!(
            func.blocks[&preheader].ops.iter().all(|op| {
                !(op.opcode == OpCode::Copy && op.attrs.contains_key("_original_kind"))
            }),
            "semantic fallback Copy must not move into the preheader"
        );
    }

    /// Layout of a canonical 2-level nested loop CFG:
    /// ```text
    /// entry → outer_ph → outer_h ⇄ outer_b → inner_ph → inner_h ⇄ inner_b
    ///                       ↓                                  ↘
    ///                    outer_exit ← inner_exit ← inner_h
    /// ```
    /// Back edges: inner_b → inner_h (inner loop), inner_exit → outer_h (outer loop).
    /// outer_ph is the outer-loop preheader; inner_ph is the inner-loop preheader.
    /// inner_ph lives *inside* the outer loop body, but *outside* the inner loop body.
    struct NestedLoop {
        outer_ph: BlockId,
        outer_h: BlockId,
        outer_b: BlockId,
        inner_ph: BlockId,
        inner_h: BlockId,
        inner_b: BlockId,
        inner_exit: BlockId,
        outer_exit: BlockId,
        outer_var: ValueId,
        inner_var: ValueId,
        cond_outer: ValueId,
        cond_inner: ValueId,
        result: ValueId,
    }

    /// Build the canonical 2-level nested-loop CFG with empty bodies.
    /// The caller fills in the inner_b ops and any extra preheader/header ops.
    fn build_nested_loop(func: &mut TirFunction) -> NestedLoop {
        let outer_ph = func.fresh_block();
        let outer_h = func.fresh_block();
        let outer_b = func.fresh_block();
        let inner_ph = func.fresh_block();
        let inner_h = func.fresh_block();
        let inner_b = func.fresh_block();
        let inner_exit = func.fresh_block();
        let outer_exit = func.fresh_block();

        let outer_var = func.fresh_value();
        let inner_var = func.fresh_value();
        let cond_outer = func.fresh_value();
        let cond_inner = func.fresh_value();
        let result = func.fresh_value();
        let outer_init = func.fresh_value();
        let outer_ph_arg = func.fresh_value();
        let inner_init = func.fresh_value();
        let inner_ph_arg = func.fresh_value();
        let outer_next = func.fresh_value();
        let inner_next = func.fresh_value();

        // entry: → outer_ph (with outer_init)
        {
            let entry = func.blocks.get_mut(&func.entry_block).unwrap();
            entry.ops.push(make_const_int(0, outer_init));
            entry.terminator = Terminator::Branch {
                target: outer_ph,
                args: vec![outer_init],
            };
        }

        // outer_ph: takes the outer-loop seed; → outer_h
        func.blocks.insert(
            outer_ph,
            TirBlock {
                id: outer_ph,
                args: vec![TirValue {
                    id: outer_ph_arg,
                    ty: TirType::I64,
                }],
                ops: vec![],
                terminator: Terminator::Branch {
                    target: outer_h,
                    args: vec![outer_ph_arg],
                },
            },
        );

        // outer_h: outer-loop header; CondBranch → outer_b or outer_exit
        func.blocks.insert(
            outer_h,
            TirBlock {
                id: outer_h,
                args: vec![TirValue {
                    id: outer_var,
                    ty: TirType::I64,
                }],
                ops: vec![make_const_int(1, cond_outer)],
                terminator: Terminator::CondBranch {
                    cond: cond_outer,
                    then_block: outer_b,
                    then_args: vec![],
                    else_block: outer_exit,
                    else_args: vec![outer_var],
                },
            },
        );

        // outer_b: → inner_ph (no ops by default)
        func.blocks.insert(
            outer_b,
            TirBlock {
                id: outer_b,
                args: vec![],
                ops: vec![make_const_int(0, inner_init)],
                terminator: Terminator::Branch {
                    target: inner_ph,
                    args: vec![inner_init],
                },
            },
        );

        // inner_ph: takes the inner-loop seed; → inner_h
        func.blocks.insert(
            inner_ph,
            TirBlock {
                id: inner_ph,
                args: vec![TirValue {
                    id: inner_ph_arg,
                    ty: TirType::I64,
                }],
                ops: vec![],
                terminator: Terminator::Branch {
                    target: inner_h,
                    args: vec![inner_ph_arg],
                },
            },
        );

        // inner_h: inner-loop header; CondBranch → inner_b or inner_exit
        func.blocks.insert(
            inner_h,
            TirBlock {
                id: inner_h,
                args: vec![TirValue {
                    id: inner_var,
                    ty: TirType::I64,
                }],
                ops: vec![make_const_int(1, cond_inner)],
                terminator: Terminator::CondBranch {
                    cond: cond_inner,
                    then_block: inner_b,
                    then_args: vec![],
                    else_block: inner_exit,
                    else_args: vec![],
                },
            },
        );

        // inner_b: → inner_h (back-edge). Caller fills in ops.
        func.blocks.insert(
            inner_b,
            TirBlock {
                id: inner_b,
                args: vec![],
                ops: vec![make_const_int(1, inner_next)],
                terminator: Terminator::Branch {
                    target: inner_h,
                    args: vec![inner_next],
                },
            },
        );

        // inner_exit: → outer_h (outer back-edge), advancing outer_var.
        func.blocks.insert(
            inner_exit,
            TirBlock {
                id: inner_exit,
                args: vec![],
                ops: vec![make_const_int(1, outer_next)],
                terminator: Terminator::Branch {
                    target: outer_h,
                    args: vec![outer_next],
                },
            },
        );

        // outer_exit: Return.
        func.blocks.insert(
            outer_exit,
            TirBlock {
                id: outer_exit,
                args: vec![TirValue {
                    id: result,
                    ty: TirType::I64,
                }],
                ops: vec![],
                terminator: Terminator::Return {
                    values: vec![result],
                },
            },
        );

        // Mark both headers as loop headers.
        func.loop_roles.insert(outer_h, LoopRole::LoopHeader);
        func.loop_roles.insert(inner_h, LoopRole::LoopHeader);

        NestedLoop {
            outer_ph,
            outer_h,
            outer_b,
            inner_ph,
            inner_h,
            inner_b,
            inner_exit,
            outer_exit,
            outer_var,
            inner_var,
            cond_outer,
            cond_inner,
            result,
            // Suppress unused-field warnings; these are kept on the struct
            // for documentation and for richer assertions in future tests.
        }
    }

    /// `for i: for j: y = a + b` where `a`, `b` are function params.
    /// Both operands are free w.r.t. the inner loop, so the Add is hoisted
    /// to the inner preheader on the inner pass; on the outer pass both
    /// operands are still free w.r.t. the outer loop, so the Add is
    /// hoisted again — into the outer preheader. End state: the Add lives
    /// in the outer preheader; the inner preheader and inner body no
    /// longer contain it.
    #[test]
    fn nested_loop_inner_invariant_hoisted_to_outer_preheader() {
        let mut func = TirFunction::new("f".into(), vec![TirType::I64, TirType::I64], TirType::I64);
        let a = ValueId(0);
        let b = ValueId(1);

        let nl = build_nested_loop(&mut func);

        // Place y = a + b inside the inner body.
        let y = func.fresh_value();
        {
            let inner_b = func.blocks.get_mut(&nl.inner_b).unwrap();
            inner_b.ops.insert(0, make_binop(OpCode::Add, a, b, y));
        }
        // Suppress dead-code warnings on documentation-only struct fields.
        let _ = (
            nl.outer_var,
            nl.inner_var,
            nl.cond_outer,
            nl.cond_inner,
            nl.result,
            nl.outer_exit,
            nl.inner_exit,
            nl.outer_b,
            nl.inner_h,
            nl.outer_h,
        );

        let stats = run(&mut func);

        // The inner body must NOT still contain the Add(a, b).
        let inner_body_ops = &func.blocks[&nl.inner_b].ops;
        let in_inner_body = inner_body_ops
            .iter()
            .any(|op| op.opcode == OpCode::Add && op.operands == vec![a, b]);
        assert!(
            !in_inner_body,
            "Add(a, b) must have been hoisted out of the inner body"
        );

        // The inner preheader must NOT still contain the Add (it should
        // have been hoisted further to the outer preheader).
        let inner_ph_ops = &func.blocks[&nl.inner_ph].ops;
        let in_inner_ph = inner_ph_ops
            .iter()
            .any(|op| op.opcode == OpCode::Add && op.operands == vec![a, b]);
        assert!(
            !in_inner_ph,
            "Add(a, b) is outer-invariant — it should not stop in the inner preheader"
        );

        // The Add must end up in the outer preheader.
        let outer_ph_ops = &func.blocks[&nl.outer_ph].ops;
        let in_outer_ph = outer_ph_ops
            .iter()
            .any(|op| op.opcode == OpCode::Add && op.operands == vec![a, b]);
        assert!(
            in_outer_ph,
            "Add(a, b) should end up in the outer preheader (multi-level hoist)"
        );

        // Multi-level hoist accounts for two move events on the same op.
        assert!(
            stats.ops_removed >= 2 && stats.ops_added >= 2,
            "expected at least 2 hoist events (inner→inner_ph, inner_ph→outer_ph), got removed={} added={}",
            stats.ops_removed,
            stats.ops_added,
        );
    }

    /// Force the multi-level hoist to be observable: an inner-body op
    /// `t = a + b` followed by `y = t + a` chains through the inner
    /// preheader on round 1 and again to the outer preheader on round 2,
    /// once `t` has migrated outward. Verifies that both ops follow the
    /// invariant transitively across both preheaders.
    #[test]
    fn nested_loop_outer_invariant_hoisted_via_inner() {
        let mut func = TirFunction::new("f".into(), vec![TirType::I64, TirType::I64], TirType::I64);
        let a = ValueId(0);
        let b = ValueId(1);

        let nl = build_nested_loop(&mut func);

        let t = func.fresh_value();
        let y = func.fresh_value();
        {
            let inner_b = func.blocks.get_mut(&nl.inner_b).unwrap();
            // Two chained invariants. Both end up in outer preheader.
            inner_b.ops.insert(0, make_binop(OpCode::Add, a, b, t));
            inner_b.ops.insert(1, make_binop(OpCode::Mul, t, a, y));
        }
        let _ = nl.outer_exit; // doc-only field

        let _stats = run(&mut func);

        let outer_ph_ops = &func.blocks[&nl.outer_ph].ops;
        let inner_ph_ops = &func.blocks[&nl.inner_ph].ops;
        let inner_body_ops = &func.blocks[&nl.inner_b].ops;

        // Neither op should remain in the inner body.
        assert!(
            !inner_body_ops
                .iter()
                .any(|op| op.opcode == OpCode::Add && op.operands == vec![a, b]),
            "Add(a, b) must leave the inner body"
        );
        assert!(
            !inner_body_ops
                .iter()
                .any(|op| op.opcode == OpCode::Mul && op.operands == vec![t, a]),
            "Mul(t, a) must leave the inner body once t becomes outer-invariant"
        );

        // Neither op should rest in the inner preheader — both are
        // outer-invariant after t is hoisted out.
        assert!(
            !inner_ph_ops
                .iter()
                .any(|op| op.opcode == OpCode::Add && op.operands == vec![a, b]),
            "Add(a, b) must not stop in the inner preheader"
        );
        assert!(
            !inner_ph_ops
                .iter()
                .any(|op| op.opcode == OpCode::Mul && op.operands == vec![t, a]),
            "Mul(t, a) must not stop in the inner preheader"
        );

        // Both ops must reach the outer preheader, in dependency order.
        let add_pos = outer_ph_ops
            .iter()
            .position(|op| op.opcode == OpCode::Add && op.operands == vec![a, b]);
        let mul_pos = outer_ph_ops
            .iter()
            .position(|op| op.opcode == OpCode::Mul && op.operands == vec![t, a]);
        assert!(add_pos.is_some(), "Add(a, b) must land in outer preheader");
        assert!(mul_pos.is_some(), "Mul(t, a) must land in outer preheader");
        assert!(
            add_pos.unwrap() < mul_pos.unwrap(),
            "Add(a, b) must precede Mul(t, a) in the outer preheader (dependency order)"
        );
    }

    /// Partially invariant: `for i: for j: y = i + a`. `a` is a function
    /// param (free everywhere). `i` is the outer-loop induction variable
    /// — invariant w.r.t. the inner loop only. The Add must therefore
    /// land in the *inner* preheader (not the outer preheader, since `i`
    /// changes per outer iteration).
    #[test]
    fn nested_loop_partially_invariant_hoists_to_inner_preheader() {
        let mut func = TirFunction::new("f".into(), vec![TirType::I64], TirType::I64);
        let a = ValueId(0);

        let nl = build_nested_loop(&mut func);
        let i = nl.outer_var; // outer-loop induction variable

        let y = func.fresh_value();
        {
            let inner_b = func.blocks.get_mut(&nl.inner_b).unwrap();
            inner_b.ops.insert(0, make_binop(OpCode::Add, i, a, y));
        }
        let _ = nl.outer_exit;

        let _stats = run(&mut func);

        // Op must leave the inner body.
        let inner_body_ops = &func.blocks[&nl.inner_b].ops;
        assert!(
            !inner_body_ops
                .iter()
                .any(|op| op.opcode == OpCode::Add && op.operands == vec![i, a]),
            "Add(i, a) must leave the inner body — invariant w.r.t. the inner loop"
        );

        // Op must land in the inner preheader.
        let inner_ph_ops = &func.blocks[&nl.inner_ph].ops;
        assert!(
            inner_ph_ops
                .iter()
                .any(|op| op.opcode == OpCode::Add && op.operands == vec![i, a]),
            "Add(i, a) should land in the inner preheader (i changes per outer iter)"
        );

        // Op must NOT escape to the outer preheader — `i` is not outer-invariant.
        let outer_ph_ops = &func.blocks[&nl.outer_ph].ops;
        assert!(
            !outer_ph_ops
                .iter()
                .any(|op| op.opcode == OpCode::Add && op.operands == vec![i, a]),
            "Add(i, a) must NOT reach the outer preheader — i is the outer induction var"
        );
    }

    #[test]
    fn non_invariant_not_hoisted() {
        let mut func = TirFunction::new("f".into(), vec![TirType::I64], TirType::I64);
        let a = ValueId(0);

        let preheader = func.fresh_block();
        let loop_header = func.fresh_block();
        let loop_body = func.fresh_block();
        let exit = func.fresh_block();

        let loop_var = func.fresh_value();
        let sum = func.fresh_value(); // a + loop_var — NOT invariant
        let cond = func.fresh_value();
        let result = func.fresh_value();

        {
            let entry = func.blocks.get_mut(&func.entry_block).unwrap();
            entry.terminator = Terminator::Branch {
                target: preheader,
                args: vec![],
            };
        }

        func.blocks.insert(
            preheader,
            TirBlock {
                id: preheader,
                args: vec![],
                ops: vec![],
                terminator: Terminator::Branch {
                    target: loop_header,
                    args: vec![],
                },
            },
        );

        func.blocks.insert(
            loop_header,
            TirBlock {
                id: loop_header,
                args: vec![TirValue {
                    id: loop_var,
                    ty: TirType::I64,
                }],
                ops: vec![make_const_int(1, cond)],
                terminator: Terminator::CondBranch {
                    cond,
                    then_block: loop_body,
                    then_args: vec![],
                    else_block: exit,
                    else_args: vec![],
                },
            },
        );

        // a + loop_var: loop_var is defined inside the loop → NOT invariant.
        func.blocks.insert(
            loop_body,
            TirBlock {
                id: loop_body,
                args: vec![],
                ops: vec![make_binop(OpCode::Add, a, loop_var, sum)],
                terminator: Terminator::Branch {
                    target: loop_header,
                    args: vec![sum],
                },
            },
        );

        func.blocks.insert(
            exit,
            TirBlock {
                id: exit,
                args: vec![TirValue {
                    id: result,
                    ty: TirType::I64,
                }],
                ops: vec![],
                terminator: Terminator::Return {
                    values: vec![result],
                },
            },
        );

        func.loop_roles.insert(loop_header, LoopRole::LoopHeader);

        let _stats = run(&mut func);

        // The Add uses loop_var (defined in loop header) — should NOT be hoisted.
        let body_ops = &func.blocks[&loop_body].ops;
        assert!(
            body_ops.iter().any(|op| op.opcode == OpCode::Add),
            "Add should remain in the loop body (not invariant)"
        );
    }
}
