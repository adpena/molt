//! Overflow-peeled dual loop for unbounded integer accumulators (bug #15).
//!
//! A function-local `while` accumulator (`total = total + i`) is refused a
//! raw-i64 carrier by the value-range analysis - correctly, because its sum
//! grows without a provable bound and a bare `iadd` would silently wrap at
//! 2^63, after which boxing the wrapped value produces a WRONG BigInt (the
//! silent-integer-miscompile class). Today the accumulator is therefore
//! carried boxed (`MaybeBigInt`): one heap-BigInt `molt_add` per iteration
//! once the sum passes the 2^46 NaN-box inline window - the measured 2.2x-
//! slower-than-CPython cliff.
//!
//! This pass rewrites a qualifying loop into a **single structured fast loop
//! with hardware-exact overflow detection plus a boxed continuation loop**:
//!
//! ```text
//! preheader  -> header(acc..., of=false, prev_acc...)
//! header     -> guard
//! guard:       cond  = Lt(iv, stop)            (existing)
//!              brk   = And(cond, Not(of))      (NEW - single canonical break)
//!              CondBranch(brk, body, dispatch)
//! body:        (sum, f) = CheckedAdd(acc, step) (every qualifying phi update)
//!              of'      = Or(f...)                (flag fan-in)
//!              prev'    = Copy(acc)             (pre-iteration values)
//!              -> header(sum..., of', prev'...)
//! dispatch:    CondBranch(of, slow_entry, exit(acc...))
//! slow_entry:  -> slow_header(prev...)             (re-execute failed iteration)
//! slow loop:   verbatim clone of {header, guard, body} with plain `Add`s -
//!              the boxed `molt_add` path, BigInt-exact by construction.
//! exit(acc_e...): all post-loop uses of the phis rewired to the exit args.
//! ```
//!
//! Design invariants (each load-bearing - see the soundness notes inline):
//!
//! * **No mid-body branch.** The overflow flag is a loop-carried Bool phi and
//!   the loop keeps its ONE loop-controlling CondBranch, so the structured
//!   loop-region reconstruction in `lower_to_simple` (which the native
//!   backend's loop optimisations key on) still recognises the fast loop. A
//!   second mid-body CondBranch is exactly the ambiguity that detector
//!   documents as corrupting.
//! * **The bridge re-executes the failed iteration.** When a `CheckedAdd`
//!   overflows, the wrapped sums are carried to the header but the loop
//!   breaks on the very next guard evaluation, and the slow loop is seeded
//!   from the `prev_*` phis - the PRE-iteration values. The qualified body is
//!   pure (Copies + Add/Mul + ConstInt only), so re-running the iteration on the boxed path
//!   is observationally identical, and no bridge arithmetic exists that could
//!   itself wrap. Wrapped values are never observed as Python ints: the only
//!   op that can read them on the overflow pass is the guard compare, whose
//!   result is then discarded by `And(_, Not(of=true)) = false`.
//! * **Unreachable header predecessors are retargeted, not deleted.** The
//!   frontend leaves a vestigial unreachable loop-else block (`LoopEnd` role)
//!   branching into the header with `ConstNone` args. It is loop METADATA
//!   (`loop_pairs` points at it) so it cannot be removed, but its `None` args
//!   would poison every raw-carrier admission chain. Its edge args are
//!   rewritten to the preheader's init values - sound (the edge never
//!   executes) and metadata-preserving.
//! * **The slow loop carries no loop metadata.** It linearises through the
//!   generic label/jump path - correct on every backend, and it is the cold
//!   path, so structured-loop optimisations are irrelevant there.
//!
//! Engagement is staged (the two `RawI64Safe` contracts differ): the native
//! name-keyed carrier chain admits full-range i64 with escape-guarded boxing
//! (`ensure_boxed_overflow_safe`), so native gets the raw fast lane. The
//! value-keyed (WASM/LLVM) `Repr::RawI64Safe` is a 47-bit-window contract -
//! every inline-box site relies on it - so those backends keep the boxed
//! carrier until the planned `RawI64Full` lattice extension; `CheckedAdd` is
//! a total function (boxed `molt_add` + constant-false flag when operands are
//! unproven), so the transform is byte-identical-correct on every target
//! either way.
//!
//! Observability: `MOLT_OVERFLOW_PEEL_STATS=1` prints per-function peel/refusal
//! counts to stderr AND writes a `overflow_peel/<func>.txt` debug artifact
//! (backend stderr does not surface in build mode - the module_slot_promotion
//! lesson). Refusal is owned by the pass predicates below, not by ambient
//! process-global rollback state.

use crate::tir::blocks::BlockId;
use crate::tir::function::TirFunction;
use crate::tir::op_kinds_generated::opcode_is_state_machine_table;

use super::PassStats;

mod rewrite;
#[cfg(test)]
mod tests;

/// Why a loop was refused. Reported by the stats instrument so real-world
/// refusal layers are visible instead of silently inert (the L4 /
/// needs_inlining / promotion lesson, three times over).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Refusal {
    /// Function has try/except/generator-state handler structure.
    HasExceptionHandlers,
    /// Function contains generator/async state ops.
    Stateful,
    /// Header's guard chain is not the canonical header->guard->CondBranch.
    NoCanonicalGuard,
    /// Loop body is not a single linear block latching back to the header.
    MultiBlockBody,
    /// The header has a reachable predecessor besides preheader + latch.
    MultiplePreheaders,
    /// The preheader edge into the header is not a plain Branch.
    NonBranchPreheader,
    /// A body/guard op is outside the pure {Copy, Add, guard-compare} set.
    ImpureBody,
    /// A header phi's init value does not chase to a ConstInt (e.g. a
    /// BigInt-seeded or parameter-seeded accumulator - must stay boxed).
    NonConstInit,
    /// A header phi is not I64-typed.
    NonIntPhi,
    /// A header phi's latch update is not a recognised arithmetic accumulator.
    NonArithmeticUpdate,
    /// A value defined inside the loop (other than the phis) is used outside.
    InteriorLiveOut,
    /// The exit block has predecessors other than the loop guard.
    ExitHasOtherPreds,
    /// Guard exit edge already carries args (unsupported v1 shape).
    GuardExitArgs,
}

pub fn run(func: &mut TirFunction, _am: &mut crate::tir::analysis::AnalysisManager) -> PassStats {
    let mut stats = PassStats {
        name: "overflow_peel",
        ..PassStats::default()
    };
    let debug = std::env::var("MOLT_OVERFLOW_PEEL_STATS").as_deref() == Ok("1");
    let mut refusals: Vec<(BlockId, Refusal)> = Vec::new();
    let mut peeled: Vec<BlockId> = Vec::new();

    // Function-level disqualifiers: exception handler structure means the
    // body's observable order matters beyond pure dataflow; generator/async
    // state machines re-enter blocks externally.
    let function_refusal = if func.has_exception_handlers() {
        Some(Refusal::HasExceptionHandlers)
    } else if func.blocks.values().any(|b| {
        b.ops
            .iter()
            .any(|op| opcode_is_state_machine_table(op.opcode))
    }) {
        Some(Refusal::Stateful)
    } else {
        None
    };

    let headers: Vec<BlockId> = func
        .loop_roles
        .iter()
        .filter(|(_, role)| **role == crate::tir::blocks::LoopRole::LoopHeader)
        .map(|(bid, _)| *bid)
        .collect();

    for header in headers {
        if let Some(r) = function_refusal {
            refusals.push((header, r));
            continue;
        }
        match rewrite::try_peel_loop(func, header) {
            Ok(added) => {
                stats.ops_added += added;
                peeled.push(header);
            }
            Err(r) => refusals.push((header, r)),
        }
    }

    if debug {
        let mut report = format!(
            "[overflow_peel] func '{}': {} peeled, {} refused\n",
            func.name,
            peeled.len(),
            refusals.len()
        );
        for bid in &peeled {
            report.push_str(&format!("  peeled loop @ block {}\n", bid.0));
        }
        for (bid, r) in &refusals {
            report.push_str(&format!("  refused loop @ block {}: {:?}\n", bid.0, r));
        }
        eprint!("{report}");
        let sanitized: String = func
            .name
            .chars()
            .map(|c| {
                if c.is_alphanumeric() || c == '_' {
                    c
                } else {
                    '_'
                }
            })
            .collect();
        let _ = crate::debug_artifacts::write_debug_artifact(
            format!("overflow_peel/{sanitized}.txt"),
            report,
        );
    }

    stats
}
