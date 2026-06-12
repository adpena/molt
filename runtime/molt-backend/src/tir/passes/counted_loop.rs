//! Counted-loop recognition — the canonical counted-loop *contract* (L4).
//!
//! ## Why this module exists
//!
//! The frontend lowers `for i in range(start, stop, step):` to a *counted*
//! arithmetic loop with **no** iterator protocol op (`range_devirt` never fires
//! on it — there is no `CallBuiltin("range")`/`GetIter`/`IterNextUnboxed` to
//! match). The real SSA shape it produces is:
//!
//! ```text
//! preheader:                      // unique reachable non-back-edge pred of H
//!   ... Branch -> H(start, acc0, ...)
//!
//! H (header):  args = [iv, acc, ...]   // MULTI-arg: the IV *and* every
//!   Branch -> C                        // loop-carried value (accumulators)
//!
//! C (cond block, == loop_cond_blocks[H]):
//!   iv_view = Copy(iv)                 // frontend stack-machine copy chains
//!   cond    = Lt|Le|Gt|Ge(iv_view, stop_const)
//!   CondBranch(cond, Body, Exit)
//!
//! Body:
//!   ... user ops (may read iv_view defined in C) ...
//!   iv_next  = Add(iv_view, step_const)
//!   acc_next = <loop-carried update>
//!   Branch -> H(iv_next, acc_next, ...)   // back-edge
//! ```
//!
//! This is the OPPOSITE of the textbook "1-arg header with the comparison in
//! the header" shape the historical `loop_unroll` detector required, which is
//! why `loop_unroll` was inert on every real counted loop (verified 2026-06-04
//! in `docs/design/foundation/04_L4-loops.md` §"CORRECTION"). A literal
//! canonicalization *to* a 1-arg header is impossible: a loop-carried
//! accumulator is intrinsically a header block-argument in SSA. The structurally
//! correct fix (Route B) is to recognize the real multi-arg shape natively and
//! expose it as ONE canonical descriptor that loop transforms consume.
//!
//! ## What this module provides
//!
//! [`recognize_counted_loop`] returns a [`CountedLoop`] for a loop header whose
//! trip count is a compile-time constant, or `None` (a principled refusal — the
//! caller leaves the loop untouched; it is never miscompiled). The descriptor
//! carries the induction variable, the loop-carried block-arg set, the resolved
//! `(start, stop, step, trip_count)`, and the region blocks. It is the single
//! source of truth a downstream transform (`loop_unroll` today; range/SIMD/IV
//! strength-reduction later) reads — no transform re-derives the trip count or
//! re-classifies the IV.
//!
//! ## Soundness boundaries (what is deliberately refused)
//!
//! * **Header → cond-block must be a single unconditional edge.** Loops with
//!   guard blocks interposed between the header and the comparison block are
//!   refused (the chain walk is not modelled here). The frontend counted loop
//!   has a direct `H -> C` edge.
//! * **Exactly one reachable preheader and one back-edge.** Dead structural
//!   blocks (e.g. the `LoopEnd` marker, which has no terminator predecessor yet
//!   still branches to the header) are excluded via terminator-only
//!   reachability — they must NOT be miscounted as a second preheader.
//! * **The IV arg must increment by a non-zero compile-time constant** and the
//!   comparison polarity must agree with the step sign. A degree-≥2 or
//!   non-constant recurrence is refused.
//! * **No nested loop** inside the region (`Body`/`C` must not themselves be
//!   loop headers).
//!
//! Representation note (bug #15): this module derives only the *structural*
//! trip count. It does NOT change how any loop-carried value is represented; a
//! consumer that materialises iteration values must keep each value's `Repr`
//! classification (an unbounded accumulator stays `MaybeBigInt`). The transform
//! in `loop_unroll` only fires for trip counts within the cost-model cap, so the
//! per-iteration induction constants it emits are themselves small.

use std::collections::{HashMap, HashSet};

use crate::tir::blocks::{BlockId, LoopRole, Terminator};
use crate::tir::dominators::{self, CfgEdgePolicy};
use crate::tir::function::TirFunction;
use crate::tir::ops::{AttrValue, OpCode};
use crate::tir::values::ValueId;

/// A recognized counted loop with a compile-time-constant trip count.
///
/// The induction variable and every loop-carried value are header block-args;
/// `iv_arg_index` selects the IV among them. A transform threads the carried
/// values (all indices `!= iv_arg_index`) through each iteration while the IV
/// takes the constant value `start + k*step` on iteration `k`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CountedLoop {
    /// The loop header block (a pure phi block: `Branch -> cond_block`).
    pub header: BlockId,
    /// The block holding the loop-exit comparison + `CondBranch(body, exit)`.
    /// Equal to `header` only in the legacy 1-arg synthesized shape where the
    /// comparison lives in the header itself.
    pub cond_block: BlockId,
    /// The loop body block (the `CondBranch` successor that loops back).
    pub body: BlockId,
    /// The exit block (the `CondBranch` successor that does not loop back).
    pub exit: BlockId,
    /// The unique reachable preheader (non-back-edge predecessor of `header`).
    pub preheader: BlockId,
    /// Index into `header.args` of the induction variable.
    pub iv_arg_index: usize,
    /// The induction-variable `ValueId` (`header.args[iv_arg_index].id`).
    pub induction_var: ValueId,
    /// Start value of the induction variable (preheader-provided constant).
    pub start: i64,
    /// Step per iteration (non-zero compile-time constant).
    pub step: i64,
    /// Trip count (number of iterations; always `> 0`).
    pub trip_count: i64,
    /// The exit-edge argument list on the cond block's `CondBranch` (the values
    /// forwarded to `exit`). May reference header args (loop-carried values) or
    /// the IV — a transform substitutes those with their final-iteration values.
    pub exit_args: Vec<ValueId>,
    /// The back-edge argument list on the body's `Branch -> header` (the values
    /// forwarded to the header for the next iteration). `back_args[k]` fills
    /// `header.args[k]`.
    pub back_args: Vec<ValueId>,
    /// The structural `LoopEnd` marker block paired with this header
    /// (`loop_pairs[header]`), if any. The frontend leaves this as an
    /// unreachable dead block; a transform that unrolls the loop away must drop
    /// its now-orphaned `LoopEnd` role so the TIR→SimpleIR back-conversion does
    /// not see a `LoopEnd` without a matching `LoopHeader`.
    pub loop_pairs_end: Option<BlockId>,
}

/// Whole-function map of `ValueId -> i64` for every `ConstInt` definition.
fn build_const_int_map(func: &TirFunction) -> HashMap<ValueId, i64> {
    let mut map = HashMap::new();
    for block in func.blocks.values() {
        for op in &block.ops {
            if op.opcode == OpCode::ConstInt
                && op.results.len() == 1
                && let Some(AttrValue::Int(v)) = op.attrs.get("value")
            {
                map.insert(op.results[0], *v);
            }
        }
    }
    map
}

/// Attr keys whose presence on a `Copy` op means it is NOT a transparent value
/// copy: `_original_kind`/`fused` mark a semantically-special op that merely
/// shares the `Copy` opcode (the same keys `copy_prop` refuses to propagate),
/// and `_simple_result_1` marks a multi-result op (e.g. an `iter_next` lowering)
/// whose single TIR result does not equal `operands[0]`. Every other attr the
/// frontend attaches to a real value copy (`_simple_out`, `_type_hint`, `var`,
/// `container_type`, the round-trip transport hints) is benign for value
/// forwarding.
const NON_COPY_ATTR_KEYS: [&str; 3] = ["_original_kind", "fused", "_simple_result_1"];

/// The source value a single-result `Copy` op forwards, or `None` if the op is
/// not a transparent value copy. The frontend produces two value-copy shapes
/// (both `OpCode::Copy`, both single-result):
///
/// * **Plain copy** `Copy(src) -> dst` — one operand (a `copy`/`load_var`
///   lowering, or a stack-machine dup). `dst ≡ src`.
/// * **Store-var copy** `Copy(src, src) -> dst` — the SSA renamer lowers
///   `store_var var=V args=[src]` by pushing `src` once for the value iteration
///   and once for the store-source iteration (`ssa.rs` lines ~1026/1086), so the
///   two operands are *identical*. `dst ≡ src`.
///
/// Both shapes carry frontend transport attrs (`_simple_out`, `_type_hint`,
/// `var`, …) which do NOT change the value-forwarding semantics, so — unlike the
/// stricter [`crate::tir::ops::TirOp::is_plain_value_copy`], which runs before
/// `copy_prop` and demands attr-emptiness — we ignore them. We refuse only when a
/// genuinely semantic attr ([`NON_COPY_ATTR_KEYS`]) is present, or the operand
/// pattern is not a copy (two *different* operands carry a slot hint with
/// non-trivial meaning).
fn copy_source(op: &crate::tir::ops::TirOp) -> Option<ValueId> {
    if op.opcode != OpCode::Copy || op.results.len() != 1 {
        return None;
    }
    if NON_COPY_ATTR_KEYS.iter().any(|k| op.attrs.contains_key(*k)) {
        return None;
    }
    match op.operands.as_slice() {
        [src] => Some(*src),
        [a, b] if a == b => Some(*a),
        _ => None,
    }
}

/// Transitive copy-resolution map over recognizable value copies (see
/// [`copy_source`]). The frontend emits long `a = Copy(b)` chains and
/// `store_var`-lowered `Copy(c, c)` ops; the IV in the cond block, the increment
/// operands, and the back-edge values are copies of the header args through
/// these. Resolving them is required to recognize the loop. Shared with the
/// `loop_unroll` transform's exit-arg substitution so both use one copy model.
pub fn build_copy_map(func: &TirFunction) -> HashMap<ValueId, ValueId> {
    let mut copy_of: HashMap<ValueId, ValueId> = HashMap::new();
    for block in func.blocks.values() {
        for op in &block.ops {
            if let Some(src) = copy_source(op) {
                copy_of.insert(op.results[0], src);
            }
        }
    }
    // Flatten transitive chains to the root source.
    for _ in 0..64 {
        let mut changed = false;
        let keys: Vec<ValueId> = copy_of.keys().copied().collect();
        for k in keys {
            let v = copy_of[&k];
            if let Some(&deeper) = copy_of.get(&v)
                && deeper != v
            {
                copy_of.insert(k, deeper);
                changed = true;
            }
        }
        if !changed {
            break;
        }
    }
    copy_of
}

/// Resolve `v` through the copy map to its root source value.
pub fn resolve(copy_of: &HashMap<ValueId, ValueId>, v: ValueId) -> ValueId {
    copy_of.get(&v).copied().unwrap_or(v)
}

/// Locate the op defining `value` anywhere in the function (block, op).
fn find_def(func: &TirFunction, value: ValueId) -> Option<(BlockId, &crate::tir::ops::TirOp)> {
    for (&bid, block) in &func.blocks {
        for op in &block.ops {
            if op.results.first() == Some(&value) {
                return Some((bid, op));
            }
        }
    }
    None
}

/// Recognize a counted loop rooted at `header`, or refuse with `None`.
///
/// `header` must be a `LoopRole::LoopHeader`. The caller is responsible for
/// iterating headers in a deterministic order.
pub fn recognize_counted_loop(func: &TirFunction, header: BlockId) -> Option<CountedLoop> {
    macro_rules! trace {
        ($($a:tt)*) => {
            if std::env::var("MOLT_DEBUG_COUNTED_LOOP").is_ok() {
                let _ = crate::debug_artifacts::append_debug_artifact(
                    "counted_loop_trace.txt",
                    format!("[counted_loop {:?} fn={}] {}\n", header, func.name, format!($($a)*)),
                );
            }
        };
    }
    if func.loop_roles.get(&header) != Some(&LoopRole::LoopHeader) {
        return None;
    }
    trace!("BEGIN recognition");
    let header_block = func.blocks.get(&header)?;

    // The header must be a pure phi block whose sole successor is the cond
    // block: `Branch -> cond_block`. (In the legacy synthesized shape the
    // header IS the cond block and ends in the CondBranch directly — handled
    // by allowing cond_block == header below.)
    let (cond_block_id, cond_block) = match &header_block.terminator {
        Terminator::Branch { target, args } if args.is_empty() => {
            let cb = func.blocks.get(target)?;
            (*target, cb)
        }
        Terminator::CondBranch { .. } => (header, header_block),
        _ => {
            trace!("header terminator not Branch/CondBranch");
            return None;
        }
    };
    trace!("cond_block = {:?}", cond_block_id);

    // When the cond block is a separate block, it must not be a loop header
    // itself (which would mean we walked into a nested loop).
    if cond_block_id != header
        && matches!(
            func.loop_roles.get(&cond_block_id),
            Some(&LoopRole::LoopHeader)
        )
    {
        return None;
    }
    // Cross-check against the frontend-recorded cond block when present: if the
    // metadata names a different block, our structural pick is suspect — refuse
    // rather than risk picking the wrong comparison.
    if let Some(&meta_cond) = func.loop_cond_blocks.get(&header)
        && meta_cond != cond_block_id
    {
        trace!(
            "meta cond {:?} != structural cond {:?}",
            meta_cond, cond_block_id
        );
        return None;
    }

    // The cond block must end in `CondBranch(cond, body, exit)` (no successor
    // args on the body edge — the IV/carried values flow as header args, not
    // body-edge args). The exit edge MAY carry args (loop-carried values that
    // escape the loop).
    let (cmp_cond, body_id, exit_id, exit_args) = match &cond_block.terminator {
        Terminator::CondBranch {
            cond,
            then_block,
            then_args,
            else_block,
            else_args,
        } => {
            // Identify which successor loops back to the header (the body).
            let then_loops = block_loops_back_to(func, *then_block, header);
            let else_loops = block_loops_back_to(func, *else_block, header);
            match (then_loops, else_loops) {
                (true, false) => {
                    if !then_args.is_empty() {
                        trace!("body (then) edge carries args");
                        return None;
                    }
                    (*cond, *then_block, *else_block, else_args.clone())
                }
                (false, true) => {
                    if !else_args.is_empty() {
                        trace!("body (else) edge carries args");
                        return None;
                    }
                    (*cond, *else_block, *then_block, then_args.clone())
                }
                _ => {
                    trace!(
                        "ambiguous back-edge: then_loops={} else_loops={}",
                        then_loops, else_loops
                    );
                    return None;
                }
            }
        }
        _ => {
            trace!("cond_block terminator not CondBranch");
            return None;
        }
    };

    // No nested loop: the body must not itself be a loop header.
    if matches!(func.loop_roles.get(&body_id), Some(&LoopRole::LoopHeader)) {
        trace!("body {:?} is a nested loop header", body_id);
        return None;
    }

    let const_map = build_const_int_map(func);
    let copy_of = build_copy_map(func);

    // The comparison defines `cmp_cond`. It must be Lt/Le/Gt/Ge(iv_view, stop).
    let Some(cmp_op) = cond_block
        .ops
        .iter()
        .find(|op| op.results.first() == Some(&cmp_cond))
    else {
        trace!("no op defines the cond {:?}", cmp_cond);
        return None;
    };
    let cmp_kind = match cmp_op.opcode {
        OpCode::Lt | OpCode::Le | OpCode::Gt | OpCode::Ge => cmp_op.opcode,
        _ => {
            trace!("cond op is {:?}, not a comparison", cmp_op.opcode);
            return None;
        }
    };
    if cmp_op.operands.len() != 2 {
        return None;
    }
    // LHS resolves (through copies) to a header arg → the IV. RHS resolves to a
    // ConstInt → the stop bound.
    let cmp_lhs_root = resolve(&copy_of, cmp_op.operands[0]);
    let Some(iv_arg_index) = header_block.args.iter().position(|a| a.id == cmp_lhs_root) else {
        trace!(
            "cmp lhs {:?} (root {:?}) is not a header arg",
            cmp_op.operands[0], cmp_lhs_root
        );
        return None;
    };
    let induction_var = header_block.args[iv_arg_index].id;
    let Some(&stop) = const_map.get(&resolve(&copy_of, cmp_op.operands[1])) else {
        trace!(
            "cmp rhs {:?} is not a ConstInt (runtime bound)",
            cmp_op.operands[1]
        );
        return None;
    };

    // The body must end with the back-edge `Branch -> header(back_args)` with
    // one arg per header block-arg.
    let body_block = func.blocks.get(&body_id)?;
    let back_args = match &body_block.terminator {
        Terminator::Branch { target, args }
            if *target == header && args.len() == header_block.args.len() =>
        {
            args.clone()
        }
        _ => {
            trace!("body terminator is not the expected back-edge Branch");
            return None;
        }
    };

    // The back-edge value for the IV slot must be `Add(iv_view, step_const)`
    // (resolving copies on the back-edge value and on the Add's IV operand).
    let iv_next_root = resolve(&copy_of, back_args[iv_arg_index]);
    let Some((_def_block, inc_op)) = find_def(func, iv_next_root) else {
        trace!("no def for IV-next {:?}", iv_next_root);
        return None;
    };
    if inc_op.opcode != OpCode::Add || inc_op.operands.len() != 2 {
        trace!("IV-next def is {:?}, not a binary Add", inc_op.opcode);
        return None;
    }
    if resolve(&copy_of, inc_op.operands[0]) != induction_var {
        trace!("IV-next Add lhs does not resolve to the IV");
        return None;
    }
    let Some(&step) = const_map.get(&resolve(&copy_of, inc_op.operands[1])) else {
        trace!("IV step is not a ConstInt");
        return None;
    };
    if step == 0 {
        return None;
    }

    // Comparison polarity must match the step sign (a non-terminating or
    // backward-counting mismatch is refused rather than assigned a bogus trip).
    let polarity_ok = match cmp_kind {
        OpCode::Lt | OpCode::Le => step > 0,
        OpCode::Gt | OpCode::Ge => step < 0,
        _ => false,
    };
    if !polarity_ok {
        trace!("polarity mismatch: cmp {:?} step {}", cmp_kind, step);
        return None;
    }

    // Exactly one reachable preheader and one back-edge. Dead structural blocks
    // (e.g. the LoopEnd marker) still branch to the header but are unreachable;
    // they must be excluded via terminator-only reachability so they are not
    // miscounted as a second preheader.
    let reachable = dominators::reachable_blocks_with(func, CfgEdgePolicy::TerminatorOnly);
    let mut preheader: Option<BlockId> = None;
    let mut preheader_count = 0usize;
    let mut backedge_count = 0usize;
    let mut start: Option<i64> = None;
    for (&pred_id, pred_block) in &func.blocks {
        if !reachable.contains(&pred_id) {
            continue;
        }
        let Some(pred_args) = branch_args_to(&pred_block.terminator, header) else {
            continue;
        };
        if pred_id == body_id {
            backedge_count += 1;
            continue;
        }
        preheader_count += 1;
        preheader = Some(pred_id);
        // The preheader must supply a constant start for the IV slot.
        if pred_args.len() == header_block.args.len() {
            start = const_map
                .get(&resolve(&copy_of, pred_args[iv_arg_index]))
                .copied();
        }
    }
    if preheader_count != 1 || backedge_count != 1 {
        trace!(
            "preheader_count={} backedge_count={} (need 1/1)",
            preheader_count, backedge_count
        );
        return None;
    }
    let preheader = preheader?;
    let Some(start) = start else {
        trace!("preheader IV-slot arg is not a ConstInt start");
        return None;
    };

    let trip_count = compute_trip_count(cmp_kind, start, stop, step)?;
    trace!(
        "RECOGNIZED: iv_idx={} start={} stop={} step={} trip={}",
        iv_arg_index, start, stop, step, trip_count
    );
    if trip_count <= 0 {
        return None;
    }

    Some(CountedLoop {
        header,
        cond_block: cond_block_id,
        body: body_id,
        exit: exit_id,
        preheader,
        iv_arg_index,
        induction_var,
        start,
        step,
        trip_count,
        exit_args,
        back_args,
        loop_pairs_end: func.loop_pairs.get(&header).copied(),
    })
}

/// True if `block` unconditionally branches back to `header` (the back-edge).
fn block_loops_back_to(func: &TirFunction, block: BlockId, header: BlockId) -> bool {
    func.blocks.get(&block).is_some_and(
        |b| matches!(&b.terminator, Terminator::Branch { target, .. } if *target == header),
    )
}

/// The argument list `term` passes to `target` along whichever edge reaches it,
/// or `None` if `term` does not branch to `target`.
fn branch_args_to(term: &Terminator, target: BlockId) -> Option<&[ValueId]> {
    match term {
        Terminator::Branch { target: t, args } if *t == target => Some(args.as_slice()),
        Terminator::CondBranch {
            then_block,
            then_args,
            else_block,
            else_args,
            ..
        } => {
            if *then_block == target {
                Some(then_args.as_slice())
            } else if *else_block == target {
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
            if *default == target {
                Some(default_args.as_slice())
            } else {
                cases
                    .iter()
                    .find_map(|(_, b, args)| (*b == target).then_some(args.as_slice()))
            }
        }
        _ => None,
    }
}

/// Compute the static trip count for `start (cmp) stop` stepping by `step`.
/// Returns `None` only on the unreachable `step == 0` / non-comparison case
/// (already filtered by the caller); a zero-iteration loop returns `Some(0)`.
fn compute_trip_count(cmp_kind: OpCode, start: i64, stop: i64, step: i64) -> Option<i64> {
    let trip = match cmp_kind {
        OpCode::Lt => {
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
        _ => return None,
    };
    Some(trip)
}

/// The set of blocks that make up the loop region between `header` and the
/// back-edge: `{header, cond_block, body}`. Used by a transform to decide which
/// blocks to retire when fully unrolling. (Header → cond is a single edge by
/// construction; there are no interposed guard blocks in a recognized loop.)
pub fn region_blocks(loop_info: &CountedLoop) -> HashSet<BlockId> {
    let mut set = HashSet::new();
    set.insert(loop_info.header);
    set.insert(loop_info.cond_block);
    set.insert(loop_info.body);
    set
}
