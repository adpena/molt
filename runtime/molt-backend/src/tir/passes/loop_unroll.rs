//! Loop Unrolling Pass for TIR.
//!
//! Unrolls loops with known small trip counts (compile-time constant
//! bounds) by duplicating the loop body. The unrolled body enables
//! SCCP to fold constants per-iteration and DCE to eliminate dead
//! branches, producing straight-line code for tight numeric loops.
//!
//! Only unrolls loops that meet ALL criteria:
//! 1. Trip count is a compile-time constant, recovered structurally
//!    from the SSA shape that `range_devirt` emits for `for i in range(...)`.
//! 2. Trip count <= MAX_UNROLL_TRIP_COUNT (default 8).
//! 3. Loop body has <= MAX_UNROLL_OPS ops (prevents code bloat).
//! 4. No exception handling in the function.
//! 5. No nested loops (only innermost loops).
//!
//! The unrolled code replaces the entire loop with straight-line ops,
//! one copy of the body per iteration with the induction variable
//! replaced by the constant iteration value.
//!
//! ## Structural detection
//!
//! After `range_devirt`, a `for i in range(start, stop, step):` lowers to:
//!
//! ```text
//! preheader (any block with Branch -> header(start_val)):
//!   ... ConstInt(start), ConstInt(stop), ConstInt(step) ...
//!   Branch -> header(start_val)
//!
//! header(ind_var):  // exactly one block arg
//!   cond = Lt(ind_var, stop_val)   // Gt for negative step
//!   CondBranch(cond, body, exit)
//!
//! body:
//!   ... user ops ...
//!   next_ind = Add(ind_var, step_val)
//!   Branch -> header(next_ind)
//!
//! exit:
//!   ...
//! ```
//!
//! The detector recovers (start, stop, step) by:
//!   1. Confirming the header has exactly one block arg (the induction var).
//!   2. Reading the comparison op in the header (`Lt`/`Le`/`Gt`/`Ge`) and
//!      resolving the right-hand side to a `ConstInt` definition.
//!   3. Reading the back-edge `Add(ind_var, step_const)` in the body and
//!      resolving the step operand to a `ConstInt`.
//!   4. Tracing the start value passed to the header on the non-back-edge
//!      predecessor (the preheader) and resolving it to a `ConstInt`.
//!
//! Reference: Muchnick ch. 17, LLVM LoopUnrollPass.

use std::collections::HashMap;

use super::PassStats;
use crate::tir::blocks::{BlockId, LoopRole, Terminator, TirBlock};
use crate::tir::function::TirFunction;
use crate::tir::ops::{AttrDict, AttrValue, Dialect, OpCode, TirOp};
use crate::tir::values::ValueId;

/// Maximum trip count for unrolling.
const MAX_UNROLL_TRIP_COUNT: i64 = 8;

/// Maximum ops in loop body for unrolling (prevents code bloat).
const MAX_UNROLL_OPS: usize = 20;

/// Metadata about a loop eligible for unrolling.
struct UnrollCandidate {
    /// The loop header block.
    header: BlockId,
    /// The loop body block (single block only).
    body: BlockId,
    /// The exit block (after the loop).
    exit: BlockId,
    /// The induction variable (block arg of the header).
    induction_var: ValueId,
    /// Start value of the induction variable.
    start: i64,
    /// Trip count (number of iterations).
    trip_count: i64,
    /// Step value per iteration.
    step: i64,
}

/// Whole-function map of `ValueId -> i64` for every `ConstInt` definition.
fn build_const_int_map(func: &TirFunction) -> HashMap<ValueId, i64> {
    let mut map = HashMap::new();
    for block in func.blocks.values() {
        for op in &block.ops {
            if op.opcode == OpCode::ConstInt && op.results.len() == 1 {
                if let Some(AttrValue::Int(v)) = op.attrs.get("value") {
                    map.insert(op.results[0], *v);
                }
            }
        }
    }
    map
}

/// True if `target_block` is `header` reached unconditionally from `pred`.
fn pred_branches_to(term: &Terminator, header: BlockId) -> bool {
    match term {
        Terminator::Branch { target, .. } => *target == header,
        Terminator::CondBranch {
            then_block,
            else_block,
            ..
        } => *then_block == header || *else_block == header,
        Terminator::Switch { cases, default, .. } => {
            *default == header || cases.iter().any(|(_, b, _)| *b == header)
        }
        _ => false,
    }
}

/// Extract the args passed to `header` from `pred`'s terminator.
fn header_args_from(term: &Terminator, header: BlockId) -> Option<&[ValueId]> {
    match term {
        Terminator::Branch { target, args } if *target == header => Some(args.as_slice()),
        Terminator::CondBranch {
            then_block,
            then_args,
            else_block,
            else_args,
            ..
        } => {
            if *then_block == header {
                Some(then_args.as_slice())
            } else if *else_block == header {
                Some(else_args.as_slice())
            } else {
                None
            }
        }
        Terminator::Switch {
            cases,
            default,
            default_args,
            ..
        } => {
            if *default == header {
                Some(default_args.as_slice())
            } else {
                cases.iter().find_map(|(_, b, args)| {
                    if *b == header {
                        Some(args.as_slice())
                    } else {
                        None
                    }
                })
            }
        }
        _ => None,
    }
}

/// Reject if any operand of any op outside the loop (header/body) references
/// a value defined inside the body — proves there are no escaping uses we
/// would have to repair after unrolling. Header block args (the induction
/// variable) are local to the loop and never escape after the rewrite.
fn body_value_escapes(
    func: &TirFunction,
    header: BlockId,
    body: BlockId,
    exit: BlockId,
) -> bool {
    // Collect all values defined inside the body.
    let body_block = match func.blocks.get(&body) {
        Some(b) => b,
        None => return true,
    };
    let mut body_defs: std::collections::HashSet<ValueId> = std::collections::HashSet::new();
    for op in &body_block.ops {
        for r in &op.results {
            body_defs.insert(*r);
        }
    }

    // Any op or terminator outside header/body that uses a body def → escape.
    for (&bid, block) in &func.blocks {
        if bid == header || bid == body {
            continue;
        }
        for op in &block.ops {
            for v in &op.operands {
                if body_defs.contains(v) {
                    return true;
                }
            }
        }
        match &block.terminator {
            Terminator::Branch { args, .. } => {
                for v in args {
                    if body_defs.contains(v) {
                        return true;
                    }
                }
            }
            Terminator::CondBranch {
                cond,
                then_args,
                else_args,
                ..
            } => {
                if body_defs.contains(cond) {
                    return true;
                }
                for v in then_args.iter().chain(else_args.iter()) {
                    if body_defs.contains(v) {
                        return true;
                    }
                }
            }
            Terminator::Switch {
                value,
                cases,
                default_args,
                ..
            } => {
                if body_defs.contains(value) {
                    return true;
                }
                for (_, _, args) in cases {
                    for v in args {
                        if body_defs.contains(v) {
                            return true;
                        }
                    }
                }
                for v in default_args {
                    if body_defs.contains(v) {
                        return true;
                    }
                }
            }
            Terminator::Return { values } => {
                for v in values {
                    if body_defs.contains(v) {
                        return true;
                    }
                }
            }
            _ => {}
        }
    }

    // Exit must not consume body defs as block args either (already covered by
    // the predecessor terminator scan, but be explicit about the contract).
    let _ = exit;
    false
}

/// Detect loops eligible for unrolling using the structural CFG shape that
/// `range_devirt` emits. Does NOT use any `range_role`-style metadata.
fn find_unroll_candidates(func: &TirFunction) -> Vec<UnrollCandidate> {
    // Bail out early on functions with exception handling — none of the loops
    // in such a function are safe to unroll under the current contract.
    if func.has_exception_handling {
        return Vec::new();
    }

    let const_map = build_const_int_map(func);
    let mut candidates = Vec::new();

    // Iterate deterministically by BlockId so the pass is reproducible.
    let mut header_ids: Vec<BlockId> = func
        .loop_roles
        .iter()
        .filter_map(|(bid, role)| (*role == LoopRole::LoopHeader).then_some(*bid))
        .collect();
    header_ids.sort_by_key(|b| b.0);

    for header_id in header_ids {
        let Some(header_block) = func.blocks.get(&header_id) else {
            continue;
        };

        // Header must have exactly one block arg → the induction variable.
        if header_block.args.len() != 1 {
            continue;
        }
        let induction_var = header_block.args[0].id;

        // Header terminator must be CondBranch picking between body and exit.
        let (body_id, exit_id, _cmp_cond) = match &header_block.terminator {
            Terminator::CondBranch {
                cond,
                then_block,
                else_block,
                then_args,
                else_args,
                ..
            } => {
                // Both successor edges must be argument-free in the
                // range_devirt shape: the induction variable is a header
                // block arg, not a successor argument.
                if !then_args.is_empty() || !else_args.is_empty() {
                    continue;
                }
                // Pick the successor that branches back to header as the body,
                // the other as the exit.
                let then_loops_back = func.blocks.get(then_block).is_some_and(|b| {
                    matches!(&b.terminator, Terminator::Branch { target, .. } if *target == header_id)
                });
                let else_loops_back = func.blocks.get(else_block).is_some_and(|b| {
                    matches!(&b.terminator, Terminator::Branch { target, .. } if *target == header_id)
                });
                match (then_loops_back, else_loops_back) {
                    (true, false) => (*then_block, *else_block, *cond),
                    (false, true) => (*else_block, *then_block, *cond),
                    _ => continue, // ambiguous or no back-edge
                }
            }
            _ => continue,
        };

        // No nested loops — the body block itself must not be a header.
        if matches!(
            func.loop_roles.get(&body_id),
            Some(&LoopRole::LoopHeader)
        ) {
            continue;
        }

        // Find the loop-exit comparison in the header.
        // Pattern: cond = Lt|Le|Gt|Ge(induction_var, stop_const).
        let cmp_op = header_block
            .ops
            .iter()
            .find(|op| op.results.first() == Some(&_cmp_cond));
        let Some(cmp_op) = cmp_op else { continue };
        let cmp_kind = match cmp_op.opcode {
            OpCode::Lt | OpCode::Le | OpCode::Gt | OpCode::Ge => cmp_op.opcode,
            _ => continue,
        };
        if cmp_op.operands.len() != 2 {
            continue;
        }
        // LHS must be the induction variable, RHS must be a known ConstInt.
        if cmp_op.operands[0] != induction_var {
            continue;
        }
        let stop = match const_map.get(&cmp_op.operands[1]) {
            Some(s) => *s,
            None => continue,
        };

        // Body must end with `Branch -> header(next_ind)` carrying exactly one
        // arg — the next induction value.
        let body_block = match func.blocks.get(&body_id) {
            Some(b) => b,
            None => continue,
        };
        let back_arg = match &body_block.terminator {
            Terminator::Branch { target, args } if *target == header_id && args.len() == 1 => {
                args[0]
            }
            _ => continue,
        };
        // The back-edge value must be defined in the body as `Add(ind_var, step_const)`.
        let increment_op = body_block
            .ops
            .iter()
            .find(|op| op.results.first() == Some(&back_arg));
        let Some(increment_op) = increment_op else {
            continue;
        };
        if increment_op.opcode != OpCode::Add || increment_op.operands.len() != 2 {
            continue;
        }
        if increment_op.operands[0] != induction_var {
            continue;
        }
        let step = match const_map.get(&increment_op.operands[1]) {
            Some(s) => *s,
            None => continue,
        };
        if step == 0 {
            continue;
        }

        // Comparison polarity must match the step sign (range_devirt invariant
        // — see range_devirt::apply_transform). We refuse to invent a trip
        // count if the polarity disagrees.
        let polarity_ok = match cmp_kind {
            OpCode::Lt | OpCode::Le => step > 0,
            OpCode::Gt | OpCode::Ge => step < 0,
            _ => false,
        };
        if !polarity_ok {
            continue;
        }

        // Body size cap.
        if body_block.ops.len() > MAX_UNROLL_OPS {
            continue;
        }

        // Walk header predecessors. There must be exactly one non-back-edge
        // predecessor (the preheader) and it must pass a ConstInt as the only
        // header argument.
        let mut preheader_args: Option<Vec<ValueId>> = None;
        let mut preheader_count = 0usize;
        let mut backedge_count = 0usize;
        for (&pred_id, pred_block) in &func.blocks {
            if !pred_branches_to(&pred_block.terminator, header_id) {
                continue;
            }
            if pred_id == body_id {
                backedge_count += 1;
                continue;
            }
            preheader_count += 1;
            preheader_args = header_args_from(&pred_block.terminator, header_id)
                .map(|s| s.to_vec());
        }
        if preheader_count != 1 || backedge_count != 1 {
            // We require exactly one preheader (so the start is unambiguous)
            // and exactly one back-edge (so the increment chain is unambiguous).
            continue;
        }
        let preheader_args = match preheader_args {
            Some(a) if a.len() == 1 => a,
            _ => continue,
        };
        let start = match const_map.get(&preheader_args[0]) {
            Some(s) => *s,
            None => continue,
        };

        // Compute trip count using the same formula range_devirt would use to
        // evaluate the comparison. We support `<`, `<=`, `>`, `>=`.
        // Loop iterates while comparing to `stop` is true. With step `s`
        // starting at `start`, the values produced are `start, start+s, ...`.
        let trip_count = match cmp_kind {
            OpCode::Lt => {
                // i < stop, step > 0 → trip = ceil((stop - start) / step)
                if start >= stop {
                    0
                } else {
                    let diff = stop - start;
                    (diff + step - 1) / step
                }
            }
            OpCode::Le => {
                if start > stop {
                    0
                } else {
                    let diff = stop - start + 1;
                    (diff + step - 1) / step
                }
            }
            OpCode::Gt => {
                // i > stop, step < 0 → trip = ceil((start - stop) / -step)
                if start <= stop {
                    0
                } else {
                    let diff = start - stop;
                    let neg = -step;
                    (diff + neg - 1) / neg
                }
            }
            OpCode::Ge => {
                if start < stop {
                    0
                } else {
                    let diff = start - stop + 1;
                    let neg = -step;
                    (diff + neg - 1) / neg
                }
            }
            _ => continue,
        };

        if trip_count <= 0 || trip_count > MAX_UNROLL_TRIP_COUNT {
            continue;
        }

        // Refuse to unroll if any body-defined value is consumed outside the
        // loop. Without this, exit-arg rewrites would be unsound for shapes
        // we have not tested. The legitimate "loop-carried reduction" case
        // is already plumbed through the header block-arg list, which is
        // empty here by construction (we required header.args.len() == 1).
        if body_value_escapes(func, header_id, body_id, exit_id) {
            continue;
        }

        candidates.push(UnrollCandidate {
            header: header_id,
            body: body_id,
            exit: exit_id,
            induction_var,
            start,
            trip_count,
            step,
        });
    }

    candidates
}

pub fn run(func: &mut TirFunction) -> PassStats {
    let mut stats = PassStats {
        name: "loop_unroll",
        ..Default::default()
    };

    let candidates = find_unroll_candidates(func);
    if candidates.is_empty() {
        return stats;
    }

    for candidate in candidates {
        unroll_candidate(func, &candidate, &mut stats);
    }

    stats
}

/// Unroll a single loop candidate: replace header + body with a straight-line
/// "landing" block that contains `trip_count` copies of the body, each with
/// the induction variable substituted by its iteration constant. Exit-edge
/// arguments that referenced the induction variable are rewritten to the
/// final post-loop value (start + trip_count * step), and arguments that
/// referenced the body's loop-back values are rewritten to the corresponding
/// last-iteration result.
fn unroll_candidate(func: &mut TirFunction, c: &UnrollCandidate, stats: &mut PassStats) {
    // Snapshot what we need from header/body before mutating.
    let body_ops = match func.blocks.get(&c.body) {
        Some(b) => b.ops.clone(),
        None => return,
    };
    let body_back_args: Vec<ValueId> = match func.blocks.get(&c.body) {
        Some(b) => match &b.terminator {
            Terminator::Branch { target, args } if *target == c.header => args.clone(),
            _ => Vec::new(),
        },
        None => Vec::new(),
    };
    let header_block = match func.blocks.get(&c.header) {
        Some(b) => b.clone(),
        None => return,
    };
    let orig_exit_args: Vec<ValueId> = match &header_block.terminator {
        Terminator::CondBranch {
            then_block,
            then_args,
            else_block,
            else_args,
            ..
        } => {
            if *else_block == c.exit {
                else_args.clone()
            } else if *then_block == c.exit {
                then_args.clone()
            } else {
                return;
            }
        }
        _ => return,
    };

    // Map each header block-arg index to which body back-edge ValueId fills it.
    // body_back_args[i] is the value the body branch passes for header.args[i].
    let header_arg_ids: Vec<ValueId> = header_block.args.iter().map(|a| a.id).collect();

    let header_ops_count = header_block.ops.len();
    let body_ops_count = body_ops.len();

    // Build the landing block ops.
    let mut landing_ops: Vec<TirOp> = Vec::new();
    // Track the last iteration's remap so we can substitute exit-arg references
    // to body-loop-back values with the last clone's outputs.
    let mut last_iter_remap: HashMap<ValueId, ValueId> = HashMap::new();

    for i in 0..c.trip_count {
        let iter_value = c.start + i * c.step;
        let iter_const_id = func.fresh_value();

        let mut const_attrs = AttrDict::new();
        const_attrs.insert("value".into(), AttrValue::Int(iter_value));
        landing_ops.push(TirOp {
            dialect: Dialect::Molt,
            opcode: OpCode::ConstInt,
            operands: vec![],
            results: vec![iter_const_id],
            attrs: const_attrs,
            source_span: None,
        });

        // Per-iteration value remap: induction var → this iteration's constant,
        // then each cloned op's result gets a fresh ValueId added to the map.
        let mut remap: HashMap<ValueId, ValueId> = HashMap::new();
        remap.insert(c.induction_var, iter_const_id);

        for op in &body_ops {
            let new_results: Vec<ValueId> = op
                .results
                .iter()
                .map(|&result| {
                    let new_value = func.fresh_value();
                    remap.insert(result, new_value);
                    new_value
                })
                .collect();

            let new_operands: Vec<ValueId> = op
                .operands
                .iter()
                .map(|v| remap.get(v).copied().unwrap_or(*v))
                .collect();

            landing_ops.push(TirOp {
                dialect: op.dialect,
                opcode: op.opcode,
                operands: new_operands,
                results: new_results.clone(),
                attrs: op.attrs.clone(),
                source_span: op.source_span,
            });

            stats.ops_added += 1;
            stats.values_changed += new_results.len();
        }

        last_iter_remap = remap;
    }

    // If any exit arg references the induction variable, materialise the
    // post-loop final value (start + trip_count * step) as a ConstInt in the
    // landing block and use it as the substitution target.
    let final_value_id: Option<ValueId> = if orig_exit_args.iter().any(|v| *v == c.induction_var) {
        let final_value = c.start + c.trip_count * c.step;
        let final_id = func.fresh_value();
        let mut final_attrs = AttrDict::new();
        final_attrs.insert("value".into(), AttrValue::Int(final_value));
        landing_ops.push(TirOp {
            dialect: Dialect::Molt,
            opcode: OpCode::ConstInt,
            operands: vec![],
            results: vec![final_id],
            attrs: final_attrs,
            source_span: None,
        });
        Some(final_id)
    } else {
        None
    };

    // Substitute exit args:
    //   - References to the induction variable → final post-loop value.
    //   - References to a header block-arg whose body back-edge value is in
    //     last_iter_remap → that remapped result. (Handles cases where the
    //     exit-edge passes a body-defined value that loops back through
    //     header.args[k], i.e. the loop-carried reduction pattern.)
    let new_exit_args: Vec<ValueId> = orig_exit_args
        .iter()
        .map(|&v| {
            if v == c.induction_var {
                return final_value_id.unwrap_or(v);
            }
            // Loop-carried block arg: header.args[k] is forwarded by body's
            // back-edge as body_back_args[k]. The exit edge gets the same
            // value as the LAST iteration's body produced, which is
            // last_iter_remap[body_back_args[k]].
            if let Some(arg_idx) = header_arg_ids.iter().position(|&id| id == v) {
                if let Some(&body_value) = body_back_args.get(arg_idx) {
                    if let Some(&remapped) = last_iter_remap.get(&body_value) {
                        return remapped;
                    }
                }
            }
            v
        })
        .collect();

    // Allocate the landing block.
    let landing = func.fresh_block();
    let landing_block = TirBlock {
        id: landing,
        args: Vec::new(),
        ops: landing_ops,
        terminator: Terminator::Branch {
            target: c.exit,
            args: new_exit_args,
        },
    };
    func.blocks.insert(landing, landing_block);

    // Redirect every predecessor of the header to the landing block.
    let preds: Vec<BlockId> = func
        .blocks
        .iter()
        .filter_map(|(&bid, b)| {
            if bid == c.header || bid == c.body {
                return None;
            }
            if branches_to(&b.terminator, c.header) {
                Some(bid)
            } else {
                None
            }
        })
        .collect();
    for pred in preds {
        if let Some(b) = func.blocks.get_mut(&pred) {
            redirect_terminator(&mut b.terminator, c.header, landing);
        }
    }
    // Also remap the function entry if it pointed at the header.
    if func.entry_block == c.header {
        func.entry_block = landing;
    }

    // Drop the original header and body blocks plus their loop metadata.
    func.blocks.remove(&c.header);
    func.blocks.remove(&c.body);
    func.loop_roles.remove(&c.header);
    func.loop_pairs.remove(&c.header);
    func.loop_break_kinds.remove(&c.header);
    func.loop_cond_blocks.remove(&c.header);

    stats.ops_removed += header_ops_count + body_ops_count;
}

/// Returns `true` if the terminator references `target` as any successor.
fn branches_to(term: &Terminator, target: BlockId) -> bool {
    match term {
        Terminator::Branch { target: t, .. } => *t == target,
        Terminator::CondBranch {
            then_block,
            else_block,
            ..
        } => *then_block == target || *else_block == target,
        Terminator::Switch { cases, default, .. } => {
            *default == target || cases.iter().any(|(_, b, _)| *b == target)
        }
        _ => false,
    }
}

/// Replace every successor reference to `from` with `to` in `term`. The
/// landing block has zero block arguments by construction, so we MUST also
/// drop any argument list that was being forwarded to `from` to keep TIR
/// verification (block-arg arity match) sound.
fn redirect_terminator(term: &mut Terminator, from: BlockId, to: BlockId) {
    match term {
        Terminator::Branch { target, args } => {
            if *target == from {
                *target = to;
                args.clear();
            }
        }
        Terminator::CondBranch {
            then_block,
            then_args,
            else_block,
            else_args,
            ..
        } => {
            if *then_block == from {
                *then_block = to;
                then_args.clear();
            }
            if *else_block == from {
                *else_block = to;
                else_args.clear();
            }
        }
        Terminator::Switch {
            cases,
            default,
            default_args,
            ..
        } => {
            for (_, b, args) in cases.iter_mut() {
                if *b == from {
                    *b = to;
                    args.clear();
                }
            }
            if *default == from {
                *default = to;
                default_args.clear();
            }
        }
        _ => {}
    }
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

    fn const_int_op(result: ValueId, value: i64) -> TirOp {
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

    fn add_op(lhs: ValueId, rhs: ValueId, result: ValueId) -> TirOp {
        TirOp {
            dialect: Dialect::Molt,
            opcode: OpCode::Add,
            operands: vec![lhs, rhs],
            results: vec![result],
            attrs: AttrDict::new(),
            source_span: None,
        }
    }

    fn cmp_op(opcode: OpCode, lhs: ValueId, rhs: ValueId, result: ValueId) -> TirOp {
        TirOp {
            dialect: Dialect::Molt,
            opcode,
            operands: vec![lhs, rhs],
            results: vec![result],
            attrs: AttrDict::new(),
            source_span: None,
        }
    }

    struct TestLoop {
        func: TirFunction,
        header: BlockId,
        body: BlockId,
        exit: BlockId,
        /// Result of the user's body Add op (defined inside body, never escapes).
        body_op_result: ValueId,
    }

    /// Build a TIR function that mirrors the post-`range_devirt` SSA shape for
    /// `for i in range(start, stop, step): body_op(i)`.
    ///
    /// CFG:
    /// ```text
    /// entry: ConstInt(start), ConstInt(stop), ConstInt(step)
    ///        Branch -> header(start_val)
    /// header(ind_var):
    ///   cond = Lt|Gt(ind_var, stop_val)
    ///   CondBranch(cond, body, exit)
    /// body:
    ///   ... body_op_count user Add ops ...
    ///   next_ind = Add(ind_var, step_val)
    ///   Branch -> header(next_ind)
    /// exit:
    ///   Return
    /// ```
    fn build_test_loop(start: i64, stop: i64, step: i64, body_op_count: usize) -> TestLoop {
        assert!(step != 0, "step must be non-zero in test fixture");
        assert!(body_op_count >= 1, "tests rely on at least one user body op");

        let mut func = TirFunction::new("f".into(), vec![], TirType::None);

        let header = func.fresh_block();
        let body = func.fresh_block();
        let exit = func.fresh_block();
        let ind_var = func.fresh_value();
        let cond = func.fresh_value();
        let start_val = func.fresh_value();
        let stop_val = func.fresh_value();
        let step_val = func.fresh_value();
        let external = func.fresh_value();
        let body_op_result = func.fresh_value();
        let next_ind = func.fresh_value();

        // Entry/preheader → header(start_val)
        {
            let entry = func.blocks.get_mut(&func.entry_block).unwrap();
            entry.ops.push(const_int_op(external, 10));
            entry.ops.push(const_int_op(start_val, start));
            entry.ops.push(const_int_op(stop_val, stop));
            entry.ops.push(const_int_op(step_val, step));
            entry.terminator = Terminator::Branch {
                target: header,
                args: vec![start_val],
            };
        }

        // Header: cmp + CondBranch (body-or-exit, no successor args).
        let cmp = if step > 0 { OpCode::Lt } else { OpCode::Gt };
        func.blocks.insert(
            header,
            TirBlock {
                id: header,
                args: vec![TirValue {
                    id: ind_var,
                    ty: TirType::I64,
                }],
                ops: vec![cmp_op(cmp, ind_var, stop_val, cond)],
                terminator: Terminator::CondBranch {
                    cond,
                    then_block: body,
                    then_args: vec![],
                    else_block: exit,
                    else_args: vec![],
                },
            },
        );

        // Body: user op(s) + increment Add(ind_var, step_val) -> next_ind
        let mut body_ops = vec![add_op(ind_var, external, body_op_result)];
        for _ in 1..body_op_count {
            let extra_result = func.fresh_value();
            body_ops.push(add_op(ind_var, external, extra_result));
        }
        body_ops.push(add_op(ind_var, step_val, next_ind));

        func.blocks.insert(
            body,
            TirBlock {
                id: body,
                args: vec![],
                ops: body_ops,
                terminator: Terminator::Branch {
                    target: header,
                    args: vec![next_ind],
                },
            },
        );

        // Exit
        func.blocks.insert(
            exit,
            TirBlock {
                id: exit,
                args: vec![],
                ops: vec![],
                terminator: Terminator::Return { values: vec![] },
            },
        );

        func.loop_roles.insert(header, LoopRole::LoopHeader);

        TestLoop {
            func,
            header,
            body,
            exit,
            body_op_result,
        }
    }

    fn attr_int(op: &TirOp, name: &str) -> Option<i64> {
        match op.attrs.get(name) {
            Some(AttrValue::Int(value)) => Some(*value),
            _ => None,
        }
    }

    #[test]
    fn unrolls_small_range_loop_into_four_body_copies() {
        // for i in range(0, 4, 1): body_op(i)
        let TestLoop {
            mut func,
            header,
            body,
            ..
        } = build_test_loop(0, 4, 1, 1);

        let stats = run(&mut func);

        // 4 trips × (1 user Add + 1 increment Add) = 8 Adds plus 4 iter constants.
        // Stats add: 4 iter ConstInts are produced via fresh_value but accounted
        // for separately from the 4 trip × 2 body ops. Our unroller bumps
        // ops_added/values_changed once per cloned body op.
        assert!(stats.ops_added > 0, "loop_unroll should fire");
        assert!(stats.values_changed > 0);
        assert!(!func.blocks.contains_key(&header));
        assert!(!func.blocks.contains_key(&body));
        assert!(!func.loop_roles.contains_key(&header));

        let landing = match &func.blocks[&func.entry_block].terminator {
            Terminator::Branch { target, args } => {
                assert!(args.is_empty(), "entry must drop header arg after redirect");
                *target
            }
            _ => panic!("entry should branch to unrolled landing block"),
        };
        assert_ne!(landing, header);
        assert_ne!(landing, body);

        let landing_block = &func.blocks[&landing];
        let add_count = landing_block
            .ops
            .iter()
            .filter(|op| op.opcode == OpCode::Add)
            .count();
        // Body had 2 Adds (user op + increment); unrolled 4 times → 8 Adds.
        assert_eq!(add_count, 8, "body Adds should be cloned once per trip");

        let iteration_constants: Vec<_> = landing_block
            .ops
            .iter()
            .filter(|op| op.opcode == OpCode::ConstInt)
            .filter_map(|op| attr_int(op, "value"))
            .collect();
        // The first 4 ConstInts are the per-iteration induction values.
        assert!(
            iteration_constants.starts_with(&[0, 1, 2, 3]),
            "expected leading iteration constants 0..4, got {iteration_constants:?}"
        );

        crate::tir::verify::verify_function(&func)
            .expect("unrolled function should pass TIR verification");
    }

    #[test]
    fn unrolls_loop_with_explicit_start_and_step() {
        // for i in range(2, 10, 2): body_op(i) -> trip count = 4 (2,4,6,8)
        let TestLoop { mut func, .. } = build_test_loop(2, 10, 2, 1);
        let stats = run(&mut func);
        assert!(stats.ops_added > 0);

        let landing = match &func.blocks[&func.entry_block].terminator {
            Terminator::Branch { target, .. } => *target,
            _ => panic!("entry should branch to landing"),
        };
        let iteration_constants: Vec<_> = func.blocks[&landing]
            .ops
            .iter()
            .filter(|op| op.opcode == OpCode::ConstInt)
            .filter_map(|op| attr_int(op, "value"))
            .collect();
        assert!(
            iteration_constants.starts_with(&[2, 4, 6, 8]),
            "expected iteration constants 2,4,6,8, got {iteration_constants:?}"
        );

        crate::tir::verify::verify_function(&func)
            .expect("unrolled function should pass TIR verification");
    }

    #[test]
    fn unrolls_loop_with_negative_step() {
        // for i in range(5, 0, -1): body_op(i) -> trip count = 5 (5,4,3,2,1)
        let TestLoop { mut func, .. } = build_test_loop(5, 0, -1, 1);
        let stats = run(&mut func);
        assert!(stats.ops_added > 0, "negative-step loop should still unroll");

        let landing = match &func.blocks[&func.entry_block].terminator {
            Terminator::Branch { target, .. } => *target,
            _ => panic!("entry should branch to landing"),
        };
        let iteration_constants: Vec<_> = func.blocks[&landing]
            .ops
            .iter()
            .filter(|op| op.opcode == OpCode::ConstInt)
            .filter_map(|op| attr_int(op, "value"))
            .collect();
        assert!(
            iteration_constants.starts_with(&[5, 4, 3, 2, 1]),
            "expected iteration constants 5..1, got {iteration_constants:?}"
        );

        crate::tir::verify::verify_function(&func)
            .expect("unrolled function should pass TIR verification");
    }

    #[test]
    fn does_not_unroll_body_larger_than_max_unroll_ops() {
        let TestLoop {
            mut func,
            header,
            body,
            body_op_result,
            ..
        } = build_test_loop(0, 4, 1, MAX_UNROLL_OPS + 1);
        let entry_target_before = match &func.blocks[&func.entry_block].terminator {
            Terminator::Branch { target, .. } => *target,
            _ => panic!("entry should branch to header"),
        };

        let stats = run(&mut func);

        assert_eq!(stats.values_changed, 0);
        assert_eq!(stats.ops_added, 0);
        assert_eq!(stats.ops_removed, 0);
        assert_eq!(entry_target_before, header);
        assert!(func.blocks.contains_key(&header));
        assert!(func.blocks.contains_key(&body));
        assert!(func.loop_roles.contains_key(&header));
        assert_eq!(
            func.blocks[&body].ops[0].results,
            vec![body_op_result],
            "oversized body should remain intact"
        );
    }

    #[test]
    fn does_not_unroll_when_trip_count_exceeds_limit() {
        // for i in range(0, 100): trip count = 100 > MAX_UNROLL_TRIP_COUNT (8)
        let TestLoop {
            mut func, header, ..
        } = build_test_loop(0, 100, 1, 1);
        let stats = run(&mut func);
        assert_eq!(stats.ops_added, 0, "loop with 100 trips should not unroll");
        assert!(func.blocks.contains_key(&header));
    }

    #[test]
    fn does_not_unroll_when_step_is_zero_step_means_no_loop() {
        // We can't construct a step=0 loop via build_test_loop's assert,
        // so simulate it directly: cmp Lt with step=0 in increment.
        let mut func = TirFunction::new("zero_step".into(), vec![], TirType::None);
        let header = func.fresh_block();
        let body = func.fresh_block();
        let exit = func.fresh_block();
        let ind_var = func.fresh_value();
        let cond = func.fresh_value();
        let start_val = func.fresh_value();
        let stop_val = func.fresh_value();
        let step_val = func.fresh_value();
        let next_ind = func.fresh_value();

        {
            let entry = func.blocks.get_mut(&func.entry_block).unwrap();
            entry.ops.push(const_int_op(start_val, 0));
            entry.ops.push(const_int_op(stop_val, 4));
            entry.ops.push(const_int_op(step_val, 0));
            entry.terminator = Terminator::Branch {
                target: header,
                args: vec![start_val],
            };
        }
        func.blocks.insert(
            header,
            TirBlock {
                id: header,
                args: vec![TirValue {
                    id: ind_var,
                    ty: TirType::I64,
                }],
                ops: vec![cmp_op(OpCode::Lt, ind_var, stop_val, cond)],
                terminator: Terminator::CondBranch {
                    cond,
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
                ops: vec![add_op(ind_var, step_val, next_ind)],
                terminator: Terminator::Branch {
                    target: header,
                    args: vec![next_ind],
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
        func.loop_roles.insert(header, LoopRole::LoopHeader);

        let stats = run(&mut func);
        assert_eq!(stats.ops_added, 0, "step=0 loop must never unroll");
        assert!(func.blocks.contains_key(&header));
    }

    #[test]
    fn does_not_unroll_when_polarity_disagrees_with_step_sign() {
        // Build a loop where the comparison is Lt but step is negative —
        // this is a non-terminating loop that range_devirt would never emit,
        // and the unroller must reject it rather than synthesize bogus trips.
        let TestLoop { mut func, header, .. } = build_test_loop(5, 0, -1, 1);
        // Mutate the comparison from Gt (as built for step=-1) to Lt to break
        // the polarity contract.
        let header_block = func.blocks.get_mut(&header).unwrap();
        for op in header_block.ops.iter_mut() {
            if op.opcode == OpCode::Gt {
                op.opcode = OpCode::Lt;
            }
        }
        let stats = run(&mut func);
        assert_eq!(
            stats.ops_added, 0,
            "polarity/step mismatch must reject the loop"
        );
        assert!(func.blocks.contains_key(&header));
    }

    #[test]
    fn does_not_unroll_when_function_has_exception_handling() {
        let TestLoop { mut func, header, .. } = build_test_loop(0, 4, 1, 1);
        func.has_exception_handling = true;
        let stats = run(&mut func);
        assert_eq!(stats.ops_added, 0);
        assert!(func.blocks.contains_key(&header));
    }

    #[test]
    fn does_not_unroll_nested_loop_inner_when_body_is_a_header() {
        // Mark the body block itself as a LoopHeader to simulate nesting.
        let TestLoop {
            mut func,
            header,
            body,
            ..
        } = build_test_loop(0, 4, 1, 1);
        func.loop_roles.insert(body, LoopRole::LoopHeader);
        let stats = run(&mut func);
        assert_eq!(stats.ops_added, 0, "nested loop must be rejected");
        assert!(func.blocks.contains_key(&header));
    }

    #[test]
    fn rejects_loop_when_body_value_escapes_to_exit() {
        // Add a use of body_op_result in the exit block — the structural
        // detector must refuse to unroll because rewriting that use after the
        // loop disappears would be unsound under the current contract.
        let TestLoop {
            mut func,
            header,
            exit,
            body_op_result,
            ..
        } = build_test_loop(0, 4, 1, 1);
        let dummy = func.fresh_value();
        let exit_block = func.blocks.get_mut(&exit).unwrap();
        exit_block.ops.push(add_op(body_op_result, body_op_result, dummy));

        let stats = run(&mut func);
        assert_eq!(stats.ops_added, 0, "escaping body value must block unroll");
        assert!(func.blocks.contains_key(&header));
    }

    /// Regression test for the structural detector: loop is built WITHOUT any
    /// `range_role` metadata (only real ConstInt + Add + Lt ops, exactly what
    /// `range_devirt` produces for `for i in range(8): ...`). Proves the
    /// metadata-keyed dead path is gone and the structural recognizer fires.
    #[test]
    fn structural_detector_unrolls_real_range_devirt_shape() {
        // for i in range(0, 8, 1): body_op(i)
        let TestLoop {
            mut func,
            header,
            body,
            ..
        } = build_test_loop(0, 8, 1, 1);

        // Sanity: no op in the function carries a `range_role` attribute.
        for block in func.blocks.values() {
            for op in &block.ops {
                assert!(
                    !op.attrs.contains_key("range_role"),
                    "fixture must not emit range_role metadata"
                );
            }
        }

        let stats = run(&mut func);
        assert!(
            stats.ops_added > 0 && stats.values_changed > 0,
            "structural detector must fire on real range_devirt CFG shape"
        );
        assert!(!func.blocks.contains_key(&header), "header must be removed");
        assert!(!func.blocks.contains_key(&body), "body must be removed");

        let landing = match &func.blocks[&func.entry_block].terminator {
            Terminator::Branch { target, .. } => *target,
            _ => panic!("entry should branch to landing"),
        };
        let landing_block = &func.blocks[&landing];

        // Eight per-iteration induction-value constants must have been
        // emitted, in the order 0,1,…,7.
        let iter_consts: Vec<i64> = landing_block
            .ops
            .iter()
            .filter(|op| op.opcode == OpCode::ConstInt)
            .filter_map(|op| attr_int(op, "value"))
            .collect();
        assert!(
            iter_consts.starts_with(&[0, 1, 2, 3, 4, 5, 6, 7]),
            "iter constants must be 0..8, got {iter_consts:?}"
        );

        // Body had 2 Adds; unrolled 8× → 16 Adds total.
        let add_count = landing_block
            .ops
            .iter()
            .filter(|op| op.opcode == OpCode::Add)
            .count();
        assert_eq!(add_count, 16);

        crate::tir::verify::verify_function(&func)
            .expect("post-unroll function must satisfy TIR verifier");
    }
}
