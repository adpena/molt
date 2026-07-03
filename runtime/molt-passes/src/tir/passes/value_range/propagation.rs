use std::collections::{HashMap, HashSet};

use crate::tir::blocks::{BlockId, Terminator};
use crate::tir::dominators;
use crate::tir::function::TirFunction;
use crate::tir::numeric_facts::IntRange;
use crate::tir::values::ValueId;

use super::super::value_identity::copy_value_source;
use super::ValueRangeResult;
use super::transfer::transfer_op_range;
/// Forward transfer-function sweep: compute a sound loop-invariant range for
/// every op-defined integer value from its operands' ranges, to a fixpoint.
///
/// ## Why this is sound and terminating
///
/// The sweep is strictly *monotone-additive*: it only ever ASSIGNS a range to a
/// value that currently has **none** (`global_range` miss ⇒ implicitly
/// `FULL_I64`). Once a value gains a range it is never revisited. Each iteration
/// therefore strictly shrinks the set of un-ranged op results, so the fixpoint
/// is reached in at most `#values` iterations. Crucially, it **never re-derives
/// a phi / block-argument's range** (phis are not ops) and **never widens** an
/// existing fact, so:
///
///   * The IV's SCEV-proven recurrence range (seeded above) is authoritative and
///     untouched — the sweep cannot loosen it.
///   * An unbounded accumulator (`total = total + i`, a header phi with no proven
///     AddRec) keeps its `FULL_I64` (absent) range: its `Add` needs the phi's
///     range, which is FULL, so the transfer yields FULL ⇒ no fact assigned. The
///     accumulator stays un-proven and correctly falls to the boxed BigInt
///     carrier. This is the mandatory `bigint_accumulator` soundness gate: a
///     value that can exceed the inline window must never be proven inline.
///
/// Every transfer is computed in i128 and saturates to the i64 domain — a result
/// that would overflow yields a wider (sound) range, never a wrapped one.
pub(super) fn propagate_op_ranges(func: &TirFunction, result: &mut ValueRangeResult) {
    // Seed `bool`-typed values as `[0, 1]` (a bool is an integer 0/1). Trivially
    // sound and lets `bool`-derived arithmetic (`a + (x < y)`) participate.
    for (&v, ty) in &func.value_types {
        if matches!(ty, crate::tir::types::TirType::Bool) {
            let canon = result.resolve(v);
            result
                .global_range
                .entry(canon)
                .or_insert(IntRange::new(0, 1));
        }
    }

    // Iterate to a fixpoint, assigning a range only to results that have none.
    // Bound the iteration count defensively by the op count (the additive
    // monotonicity already guarantees termination; this is a hard ceiling).
    let max_iters = func.blocks.values().map(|b| b.ops.len()).sum::<usize>() + 1;
    for _ in 0..max_iters {
        let mut changed = false;
        for block in func.blocks.values() {
            for op in &block.ops {
                // Single-result integer ops only. (Value-identity copies — plain
                // or tagged — are already threaded by `resolve` through
                // `copy_src`; skip them here so the fact lands on the canonical
                // source rather than a copy alias.)
                if op.results.len() != 1 || copy_value_source(op).is_some() {
                    continue;
                }
                let res = result.resolve(op.results[0]);
                if result.global_range.contains_key(&res) {
                    continue; // already ranged (constant / IV / earlier sweep).
                }
                let Some(range) = transfer_op_range(op, result) else {
                    continue;
                };
                if range.is_full() {
                    continue; // no information — leave un-ranged.
                }
                result.global_range.insert(res, range);
                changed = true;
            }
        }
        if !changed {
            break;
        }
    }
}

/// True if `r` is a phi-INDEPENDENT, genuinely BOUNDED range — the licensing
/// condition for using an incoming as a phi-narrowing JOIN contributor (see
/// [`narrow_loop_header_phis`]).
///
/// "Non-FULL" alone is INSUFFICIENT. The transfer functions ([`IntRange::add`],
/// `sub`, `mul`, `neg`, `shl_const`, …) *saturate* to the i64 endpoints rather
/// than collapsing to the exact `FULL_I64` sentinel, so a phi-DEPENDENT
/// computation under the all-phis-FULL sweep can yield a range like
/// `add(FULL, [1,1]) = [i64::MIN + 1, i64::MAX]` — not `is_full()`, yet
/// effectively unbounded and entirely a function of the FULL phi's magnitude. An
/// unbounded counter `i = i + 1` with an opaque loop bound is exactly this
/// shape, and treating its near-full back-edge range as a "bound" would wrongly
/// narrow the IV (the `e2e_nonconst_bound_no_nsw_not_proven` soundness gate).
///
/// A genuine phi-independent re-bound (`x & MASK` ⇒ `[0, MASK]`, `x % c` ⇒
/// `[0, c-1]`) has bounds that are *constants drawn from the program*, strictly
/// INTERIOR to the i64 domain — never touching either extreme. Requiring the
/// range to be strictly interior (`lo > i64::MIN && hi < i64::MAX`) therefore
/// rejects every saturated/unbounded transfer result while accepting every real
/// masked/modular bound. This is sound and loses no raw-i64 promotion: a bound
/// that reaches an i64 extreme cannot fit the `2**46` inline window anyway, so it
/// could never license a raw carrier even if narrowed.
#[inline]
fn is_phi_independent_bound(r: IntRange) -> bool {
    r.lo > i64::MIN && r.hi < i64::MAX
}

/// Narrow loop-header phis (block arguments of `LoopHeader` blocks) to the JOIN
/// of their incoming-edge ranges — the targeted producer that restores the
/// raw-i64 lane for a **masked back-edge accumulator** (`s = (s << 1) & MASK`),
/// whose carried value is re-bounded to `[0, MASK]` independently of the phi.
/// Returns `true` if any phi was narrowed (so the caller re-runs the forward
/// sweep to range values derived from the narrowed phi).
///
/// ## The soundness boundary (why a masked back edge licenses narrowing and an
/// unbounded accumulator does not)
///
/// A header phi's range is the JOIN (union) of the ranges of the values flowing
/// into it on every reachable incoming edge — *provided each of those ranges is
/// independent of the phi itself*. The danger is circularity: an accumulator
/// `total = total + i` carries `total + i` on the back edge, whose range depends
/// on `total`'s range (the phi). "Narrowing" it from a range that already
/// assumed a bound on the phi would be unsound — the accumulator can exceed the
/// inline window and even i64, requiring a heap BigInt; a false inline proof is
/// a silent truncation miscompile (the worst bug class).
///
/// This pass sidesteps the circularity WITHOUT a bespoke dependency analysis, by
/// exploiting an invariant the forward sweep ([`propagate_op_ranges`]) already
/// guarantees: **it never assigns a range to a phi**, so every op-result range
/// in `global_range` at this point was computed treating *all* phis as FULL
/// (unknown). Consequently, any incoming value whose current range is a genuine
/// **bounded interior** range ([`is_phi_independent_bound`]) is phi-INDEPENDENT
/// by construction — its bound was derived without assuming anything about any
/// phi. That is exactly the licensing condition:
///
///   * Masked back edge `s_next = (s << 1) & MASK`: under the FULL-phi sweep,
///     `s << 1` is FULL (operand `s` is FULL) but the `& MASK` re-bounds it to
///     `[0, MASK]` regardless — a *constant* range derived purely from the mask.
///     `s_next` is a bounded interior range ⇒ phi-independent ⇒ a valid JOIN
///     incoming.
///   * Unbounded accumulator back edge `total + i` / `acc << 1`: under the
///     FULL-phi sweep these are FULL or saturated-to-the-i64-extreme (a FULL
///     operand poisons the transfer). Not a bounded interior range ⇒ the
///     all-incomings test fails ⇒ NOT narrowed. The phi keeps its FULL range and
///     correctly falls to the boxed BigInt carrier.
///
/// We narrow ONLY when EVERY incoming is a bounded interior range (so the JOIN is
/// itself bounded and every contributor is phi-independent); a single
/// FULL/saturated incoming makes the phi unprovable, so we refuse — fail-closed.
/// The narrowed range is the JOIN of the incomings; it is a sound
/// over-approximation of every value the phi can hold (each iteration's value
/// flows in on some edge), and it holds for the phi everywhere it is live
/// (header, body, and the loop-exit use), so it is placed as a global fact,
/// mirroring the IV-range placement.
///
/// ## Why a single narrowing round + one re-sweep is sound (no fixpoint)
///
/// Narrowing reads ONLY the FULL-phi sweep results. A *second* narrowing round
/// would read ranges the re-sweep computed AFTER the first round narrowed some
/// phi — those are no longer guaranteed phi-independent (they may incorporate
/// the just-narrowed phi's bound), so iterating narrow→sweep→narrow could feed a
/// phi-dependent range back into a phi and lose soundness. We therefore narrow
/// exactly once, from the FULL-phi baseline, then re-sweep once to propagate the
/// narrowed phi ranges to derived values. This is complete for any set of
/// *independent* masked accumulators (each proven under the same FULL-phi
/// baseline); a phi whose mask depends on another narrowed phi is a conservative
/// miss (sound), never a miscompile.
pub(super) fn narrow_loop_header_phis(
    func: &TirFunction,
    loop_bodies: &HashMap<BlockId, HashSet<BlockId>>,
    result: &mut ValueRangeResult,
) -> bool {
    // Only header phis are candidates: a loop-header block argument is the
    // canonical loop-carried (phi) value. Non-header block args (e.g. a plain
    // join point) are out of scope — the masked-accumulator shape this targets
    // is loop-carried. `loop_bodies`' keys are exactly the recognized headers.
    if loop_bodies.is_empty() {
        return false;
    }

    // Collect, per header block argument, the values flowing in on every
    // reachable incoming edge (preheader entry + back edges). Dead-edge
    // insensitive (the standard SCCP phi semantics): an unreachable source block
    // delivers no value, so its fabricated args (e.g. the vestigial
    // `loop_end → header` `ConstNone`s the SSA lift keeps as loop metadata) must
    // not contribute — counting them would inject a spurious FULL incoming and
    // defeat every narrow. Shares the `executable_reachable_blocks` oracle with
    // the raw-i64-safe phi propagation (`propagate_raw_i64_safe_values`).
    let reachable = dominators::executable_reachable_blocks(func);
    let mut incomings: HashMap<(BlockId, usize), Vec<ValueId>> = HashMap::new();
    for block in func.blocks.values() {
        if !reachable.contains(&block.id) {
            continue;
        }
        let mut add = |target: BlockId, args: &[ValueId]| {
            for (index, &arg) in args.iter().enumerate() {
                incomings.entry((target, index)).or_default().push(arg);
            }
        };
        match &block.terminator {
            Terminator::Branch { target, args } => add(*target, args),
            Terminator::CondBranch {
                then_block,
                then_args,
                else_block,
                else_args,
                ..
            } => {
                add(*then_block, then_args);
                add(*else_block, else_args);
            }
            Terminator::Switch {
                cases,
                default,
                default_args,
                ..
            } => {
                for (_, target, args) in cases {
                    add(*target, args);
                }
                add(*default, default_args);
            }
            Terminator::StateDispatch {
                cases,
                default,
                default_args,
            } => {
                for (_, target, args) in cases {
                    add(*target, args);
                }
                add(*default, default_args);
            }
            Terminator::Return { .. } | Terminator::Unreachable => {}
        }
    }

    // Decide narrowings against the FROZEN FULL-phi sweep state (read-only over
    // `result`), then apply them in one batch. Computing every narrowing from
    // the same pre-narrow snapshot is what makes the rule a single round (no
    // phi's narrowed range can leak into another phi's decision this round).
    let mut narrowings: Vec<(ValueId, IntRange)> = Vec::new();
    let mut headers: Vec<BlockId> = loop_bodies.keys().copied().collect();
    headers.sort_unstable_by_key(|b| b.0); // deterministic order.
    for header in headers {
        let Some(header_block) = func.blocks.get(&header) else {
            continue;
        };
        for (index, arg) in header_block.args.iter().enumerate() {
            let phi = arg.id;
            // If the phi already has a proven range (an AddRec IV ranged above),
            // that fact is authoritative — never widen/disturb it.
            if result.global_range.contains_key(&result.resolve(phi)) {
                continue;
            }
            let Some(srcs) = incomings.get(&(header, index)) else {
                continue; // no reachable incoming edges → cannot narrow.
            };
            if srcs.is_empty() {
                continue;
            }
            // JOIN the incoming ranges. Bail to "no narrow" the instant any
            // incoming is FULL (phi-dependent or simply unproven) — fail-closed.
            // A self-referential incoming (the phi feeding itself directly, with
            // no re-bounding op) resolves to the phi, whose range is absent here
            // (we skipped already-ranged phis) ⇒ FULL ⇒ bails. So a bare
            // `x = x` / rotate-without-mask phi never narrows.
            let mut joined: Option<IntRange> = None;
            let mut all_independent = true;
            for &src in srcs {
                let r = result.range_of(src); // resolves copies; FULL if unknown.
                if !is_phi_independent_bound(r) {
                    all_independent = false;
                    break;
                }
                joined = Some(match joined {
                    None => r,
                    Some(acc) => acc.join(r),
                });
            }
            if !all_independent {
                continue;
            }
            // Every incoming is a phi-independent bounded fact ⇒ the JOIN is a
            // sound, bounded bound on the phi.
            if let Some(range) = joined {
                debug_assert!(
                    is_phi_independent_bound(range),
                    "JOIN of interior bounds must itself be an interior bound"
                );
                narrowings.push((result.resolve(phi), range));
            }
        }
    }

    if narrowings.is_empty() {
        return false;
    }
    for (phi, range) in narrowings {
        // The phi had no prior range (checked above); insert the JOIN as a weak
        // global fact. Meet with any concurrently-inserted fact for the same
        // canonical value (two header args resolving to one source — rare) so we
        // never widen.
        let existing = result
            .global_range
            .get(&phi)
            .copied()
            .unwrap_or(IntRange::FULL_I64);
        result.global_range.insert(phi, existing.meet(range));
    }
    true
}
