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
use super::value_range::{ValueRange, ValueRangeResult};
use crate::tir::analysis::{AnalysisManager, DefMap, LoopForest};
use crate::tir::blocks::{BlockId, Terminator};
use crate::tir::function::TirFunction;
use crate::tir::op_kinds_generated::{
    opcode_requires_i64_shift_count_guard_table, opcode_requires_i64_zero_divisor_guard_table,
};
use crate::tir::ops::{OpCode, TirOp};
use crate::tir::values::ValueId;

/// Returns `true` if the op is pure and safe to hoist out of a loop.
///
/// The opcode-level purity decision is delegated to the single source of truth
/// in `effects::opcode_is_pure_movable` (deterministic + side-effect-free +
/// never-throwing). LICM additionally permits a structural SSA value copy,
/// which is a property of the op *instance* (its operand/result arity and empty
/// attrs), not of the opcode, so that check stays here.
///
/// Hoisting requires the FULL pure-movable property (including `nothrow`):
/// moving an op above the loop guard changes whether/when it would raise, so a
/// may-throw op (e.g. `Div`) must not be hoisted even though it is CSE-safe —
/// UNLESS its specific throw condition is *disproven* at the hoist site, which
/// [`throw_condition_disproven`] decides per-instance from the value-range proof
/// (a shift whose count is in `[0, 63]`, a divide whose divisor is non-zero).
fn is_hoistable(op: &TirOp, vr: &ValueRangeResult) -> bool {
    super::effects::opcode_is_pure_movable(op.opcode)
        || op.is_plain_value_copy()
        || (super::effects::opcode_is_pure_may_throw(op.opcode)
            && throw_condition_disproven(op, vr))
}

/// True when a `pure_may_throw` op (`{Div, FloorDiv, Mod, Pow, Shl, Shr}`) is
/// PROVEN not to raise on its operands — so hoisting it above the loop guard
/// cannot move an observable raise earlier (it would never have raised). This is
/// the honest generalization of the hoist gate: "throw-condition disproven",
/// parameterized per opcode, each arm reusing the SINGLE value-range proof the
/// raw-i64 lane already uses (no duplicated proof logic).
///
///   * **`Shl` / `Shr`**: a negative shift count raises `ValueError`, and a
///     count `>= 64` is a wrong-value machine shift on the raw lane. The op is
///     nothrow-and-well-defined iff the count operand is range-proven in
///     `[0, 63]` — the exact gate the raw-i64 shift seed
///     (`representation_plan::raw_i64_safe_value_seed`) applies. We DO NOT
///     additionally require the result to fit the inline window: hoisting is a
///     *position* change, not a representation change — a hoisted `x << k` whose
///     result is a heap BigInt is still computed (boxed) in the preheader,
///     correctly, exactly once. The only property hoisting needs is that the
///     shift does not *raise* where the loop guard used to protect it, i.e. a
///     non-negative, in-machine-range count.
///   * **`Div` / `FloorDiv` / `Mod`**: a zero divisor raises
///     `ZeroDivisionError`. The op is nothrow iff the divisor operand
///     `proves_nonzero()` — the same predicate the WASM raw `sdiv`/`srem` lane
///     uses (#42). (Integer `i64::MIN / -1` overflow is a separate concern that
///     does not raise in Python — it produces a bigint — and is handled by the
///     boxed lowering, not a raise, so it does not block the hoist.)
///   * **`Pow`**: REFUSED. `x ** y` raises `ZeroDivisionError` for `0 ** -1` and
///     returns a float for a negative integer exponent, so the nothrow
///     condition couples base AND exponent ranges (and the int/float result
///     repr); it is not trivially range-provable. We never hoist `Pow` here —
///     documenting the refusal rather than shipping an unsound or fragile gate.
///     CSE of `Pow` (under dominance) is unaffected; only the hoist is withheld.
fn throw_condition_disproven(op: &TirOp, vr: &ValueRangeResult) -> bool {
    if opcode_requires_i64_shift_count_guard_table(op.opcode) {
        // Count operand proven in the valid machine-shift range [0, 63].
        return op.operands.get(1).is_some_and(|&count| {
            let r = vr.range_of(count);
            r.lo >= 0 && r.hi <= 63
        });
    }
    if opcode_requires_i64_zero_divisor_guard_table(op.opcode) {
        // Divisor proven to exclude zero.
        return op
            .operands
            .get(1)
            .is_some_and(|&divisor| vr.range_of(divisor).proves_nonzero());
    }
    // Pow's throw condition is not a single-operand range fact — refuse.
    false
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

pub fn run(func: &mut TirFunction, am: &mut AnalysisManager) -> PassStats {
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

    // The loop forest (headers from `loop_roles`, sorted by id for
    // deterministic tie-breaking; bodies via dominator-based natural-loop
    // construction) is shared with BCE through the analysis manager.
    let forest = am.get::<LoopForest>(func).clone();
    if forest.headers.is_empty() {
        return stats;
    }

    // Value-range proof, shared with BCE/SROA via the analysis manager. Used to
    // DISPROVE the throw condition of a `pure_may_throw` op (a shift whose count
    // is in `[0, 63]`, a divide whose divisor is non-zero), which makes that op
    // provably nothrow at the hoist site and therefore LICM-hoistable. Cloned
    // because we take `&mut func.blocks` below; the analysis is a pure function
    // of the function and LICM only moves invariant ops (never changes any value
    // range), so the snapshot stays valid across the hoists. (A hoisted op keeps
    // its operands and result `ValueId`s, so the value-keyed ranges still line
    // up after the move.)
    let vr = am.get::<ValueRange>(func).clone();

    // Process all loops, innermost first, so ops hoisted from an inner
    // loop's body into the inner preheader become visible to the
    // enclosing loop and can be hoisted further out if invariant there.
    //
    // Step 1: index each loop's natural-loop block set by header (headers are
    // already in ascending-id order from the forest). Natural loops nest
    // properly: an inner loop's body is a strict subset of its enclosing
    // outer loop's body.
    let loop_block_sets: Vec<(BlockId, HashSet<BlockId>)> = forest
        .headers
        .iter()
        .map(|&h| (h, forest.bodies[&h].clone()))
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

    // Value → defining-block map (block args, op results, and params at entry),
    // shared with GVN through the analysis manager. Cloned because LICM mutates
    // its local copy as it hoists ops (recording their new preheader def block);
    // the cached analysis is not mutated.
    let mut value_def_block = am.get::<DefMap>(func).clone();

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
                    if !is_hoistable(op, &vr) {
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

        let stats = run(&mut func, &mut crate::tir::analysis::AnalysisManager::new());

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

        let _stats = run(&mut func, &mut crate::tir::analysis::AnalysisManager::new());

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

        let stats = run(&mut func, &mut crate::tir::analysis::AnalysisManager::new());

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

        let _stats = run(&mut func, &mut crate::tir::analysis::AnalysisManager::new());

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

        let _stats = run(&mut func, &mut crate::tir::analysis::AnalysisManager::new());

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

        let _stats = run(&mut func, &mut crate::tir::analysis::AnalysisManager::new());

        // The Add uses loop_var (defined in loop header) — should NOT be hoisted.
        let body_ops = &func.blocks[&loop_body].ops;
        assert!(
            body_ops.iter().any(|op| op.opcode == OpCode::Add),
            "Add should remain in the loop body (not invariant)"
        );
    }

    // ── #49: proven-safe `pure_may_throw` hoist gate ────────────────────────

    /// Direct unit coverage of the throw-disproof predicate against a
    /// hand-built `ValueRangeResult`, exercising every per-opcode arm and the
    /// `Pow` refusal — independent of the full pipeline.
    #[test]
    fn throw_condition_disproven_per_opcode() {
        let mut vr = ValueRangeResult::default();
        let x = ValueId(1);
        let count_ok = ValueId(2); // [0, 63]
        let count_neg = ValueId(3); // [-4, 10] (straddles negative)
        let count_big = ValueId(4); // [0, 200] (exceeds 63)
        let count_unknown = ValueId(5); // FULL
        let divisor_nz = ValueId(6); // [1, 9] (non-zero)
        let divisor_zero = ValueId(7); // [-2, 5] (straddles zero)
        let _ = x;
        vr.set_global_range_for_test(count_ok, 0, 63);
        vr.set_global_range_for_test(count_neg, -4, 10);
        vr.set_global_range_for_test(count_big, 0, 200);
        vr.set_global_range_for_test(divisor_nz, 1, 9);
        vr.set_global_range_for_test(divisor_zero, -2, 5);
        // count_unknown deliberately left absent (FULL).

        // Shl / Shr: disproven iff count in [0, 63].
        assert!(throw_condition_disproven(
            &make_binop(OpCode::Shl, x, count_ok, ValueId(20)),
            &vr
        ));
        assert!(throw_condition_disproven(
            &make_binop(OpCode::Shr, x, count_ok, ValueId(21)),
            &vr
        ));
        assert!(
            !throw_condition_disproven(&make_binop(OpCode::Shl, x, count_neg, ValueId(22)), &vr),
            "a possibly-negative count can raise ValueError — must NOT be disproven"
        );
        assert!(
            !throw_condition_disproven(&make_binop(OpCode::Shl, x, count_big, ValueId(23)), &vr),
            "a count > 63 is a wrong-value machine shift — must NOT be disproven"
        );
        assert!(
            !throw_condition_disproven(
                &make_binop(OpCode::Shl, x, count_unknown, ValueId(24)),
                &vr
            ),
            "an unknown count must NOT be disproven (fail-closed)"
        );

        // Div / FloorDiv / Mod: disproven iff divisor proven non-zero.
        for opcode in [OpCode::Div, OpCode::FloorDiv, OpCode::Mod] {
            assert!(
                throw_condition_disproven(&make_binop(opcode, x, divisor_nz, ValueId(30)), &vr),
                "{opcode:?} with a non-zero divisor must be disproven"
            );
            assert!(
                !throw_condition_disproven(&make_binop(opcode, x, divisor_zero, ValueId(31)), &vr),
                "{opcode:?} with a possibly-zero divisor must NOT be disproven"
            );
        }

        // Pow: REFUSED unconditionally (gnarly base/exponent coupling).
        assert!(
            !throw_condition_disproven(&make_binop(OpCode::Pow, x, divisor_nz, ValueId(40)), &vr),
            "Pow's throw condition is not a single-operand range fact — always refused"
        );
    }

    /// Build a loop whose body contains a loop-invariant `y = x << k` (both `x`
    /// and `k` defined in the preheader), with `k` a `ConstInt` whose value
    /// determines whether the shift's `ValueError` throw is range-disproven.
    /// Returns the function plus the loop-body block id and the shift result id.
    fn invariant_shift_loop(shift_count: i64) -> (TirFunction, BlockId, ValueId) {
        let mut func = TirFunction::new("sh".into(), vec![TirType::I64], TirType::I64);
        let x = ValueId(0); // param — loop-invariant
        let preheader = func.fresh_block();
        let loop_header = func.fresh_block();
        let loop_body = func.fresh_block();
        let exit = func.fresh_block();

        let k = func.fresh_value(); // ConstInt shift count (preheader → invariant)
        let loop_var = func.fresh_value();
        let y = func.fresh_value(); // x << k — the hoist candidate
        let use_val = func.fresh_value();
        let cond = func.fresh_value();
        let result = func.fresh_value();

        {
            let entry = func.blocks.get_mut(&func.entry_block).unwrap();
            entry.terminator = Terminator::Branch {
                target: preheader,
                args: vec![],
            };
        }
        // Preheader defines the invariant shift count `k`.
        func.blocks.insert(
            preheader,
            TirBlock {
                id: preheader,
                args: vec![],
                ops: vec![make_const_int(shift_count, k)],
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
        // Body: y = x << k (invariant!), then use it locally and loop back.
        func.blocks.insert(
            loop_body,
            TirBlock {
                id: loop_body,
                args: vec![],
                ops: vec![
                    make_binop(OpCode::Shl, x, k, y),
                    make_binop(OpCode::Add, y, loop_var, use_val),
                ],
                terminator: Terminator::Branch {
                    target: loop_header,
                    args: vec![use_val],
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
        (func, loop_body, y)
    }

    #[test]
    fn invariant_shift_with_proven_count_is_hoisted() {
        // y = x << 12: count 12 is range-proven in [0, 63] -> ValueError throw
        // disproven -> the shift is provably nothrow at the hoist site -> hoisted.
        let (mut func, loop_body, _y) = invariant_shift_loop(12);
        run(&mut func, &mut crate::tir::analysis::AnalysisManager::new());
        let body_has_shl = func.blocks[&loop_body]
            .ops
            .iter()
            .any(|op| op.opcode == OpCode::Shl);
        assert!(
            !body_has_shl,
            "loop-invariant `x << 12` (count proven [0,63]) must be hoisted out of the loop"
        );
    }

    #[test]
    fn invariant_shift_with_unprovable_count_is_not_hoisted() {
        // y = x << 80: count 80 is OUTSIDE [0, 63] -> the raw machine shift is a
        // wrong value (and CPython produces a bigint, but the point is the throw/
        // well-definedness is NOT disproven) -> must NOT be hoisted above the
        // guard. It stays in the loop on the boxed lane (BigInt-correct).
        let (mut func, loop_body, _y) = invariant_shift_loop(80);
        run(&mut func, &mut crate::tir::analysis::AnalysisManager::new());
        let body_has_shl = func.blocks[&loop_body]
            .ops
            .iter()
            .any(|op| op.opcode == OpCode::Shl);
        assert!(
            body_has_shl,
            "`x << 80` (count outside [0,63]) must NOT be hoisted — throw/well-definedness not disproven"
        );
    }

    #[test]
    fn invariant_negative_shift_count_is_not_hoisted() {
        // y = x << -1: a negative count raises ValueError every iteration the
        // loop runs. Hoisting it would move that raise to the preheader, where it
        // fires even if the loop body would never execute (zero-trip). The throw
        // is NOT disproven (count range [-1,-1] has lo < 0) -> must NOT hoist.
        let (mut func, loop_body, _y) = invariant_shift_loop(-1);
        run(&mut func, &mut crate::tir::analysis::AnalysisManager::new());
        let body_has_shl = func.blocks[&loop_body]
            .ops
            .iter()
            .any(|op| op.opcode == OpCode::Shl);
        assert!(
            body_has_shl,
            "`x << -1` raises ValueError — hoisting would move the raise above the guard"
        );
    }
}
