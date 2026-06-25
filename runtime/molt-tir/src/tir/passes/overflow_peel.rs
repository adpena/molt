//! Overflow-peeled dual loop for unbounded integer accumulators (bug #15).
//!
//! A function-local `while` accumulator (`total = total + i`) is refused a
//! raw-i64 carrier by the value-range analysis — correctly, because its sum
//! grows without a provable bound and a bare `iadd` would silently wrap at
//! 2^63, after which boxing the wrapped value produces a WRONG BigInt (the
//! silent-integer-miscompile class). Today the accumulator is therefore
//! carried boxed (`MaybeBigInt`): one heap-BigInt `molt_add` per iteration
//! once the sum passes the 2^46 NaN-box inline window — the measured 2.2×-
//! slower-than-CPython cliff.
//!
//! This pass rewrites a qualifying loop into a **single structured fast loop
//! with hardware-exact overflow detection plus a boxed continuation loop**:
//!
//! ```text
//! preheader  → header(acc…, of=false, prev_acc…)
//! header     → guard
//! guard:       cond  = Lt(iv, stop)            (existing)
//!              brk   = And(cond, Not(of))      (NEW — single canonical break)
//!              CondBranch(brk, body, dispatch)
//! body:        (sum, f) = CheckedAdd(acc, step) (every qualifying phi update)
//!              of'      = Or(f…)                (flag fan-in)
//!              prev'    = Copy(acc)             (pre-iteration values)
//!              → header(sum…, of', prev'…)
//! dispatch:    CondBranch(of, slow_entry, exit(acc…))
//! slow_entry:  → slow_header(prev…)             (re-execute failed iteration)
//! slow loop:   verbatim clone of {header, guard, body} with plain `Add`s —
//!              the boxed `molt_add` path, BigInt-exact by construction.
//! exit(accₑ…): all post-loop uses of the phis rewired to the exit args.
//! ```
//!
//! Design invariants (each load-bearing — see the soundness notes inline):
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
//!   from the `prev_*` phis — the PRE-iteration values. The qualified body is
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
//!   rewritten to the preheader's init values — sound (the edge never
//!   executes) and metadata-preserving.
//! * **The slow loop carries no loop metadata.** It linearises through the
//!   generic label/jump path — correct on every backend, and it is the cold
//!   path, so structured-loop optimisations are irrelevant there.
//!
//! Engagement is staged (the two `RawI64Safe` contracts differ): the native
//! name-keyed carrier chain admits full-range i64 with escape-guarded boxing
//! (`ensure_boxed_overflow_safe`), so native gets the raw fast lane. The
//! value-keyed (WASM/LLVM) `Repr::RawI64Safe` is a 47-bit-window contract —
//! every inline-box site relies on it — so those backends keep the boxed
//! carrier until the planned `RawI64Full` lattice extension; `CheckedAdd` is
//! a total function (boxed `molt_add` + constant-false flag when operands are
//! unproven), so the transform is byte-identical-correct on every target
//! either way.
//!
//! Observability: `MOLT_OVERFLOW_PEEL_STATS=1` prints per-function peel/refusal
//! counts to stderr AND writes a `overflow_peel/<func>.txt` debug artifact
//! (backend stderr does not surface in build mode — the module_slot_promotion
//! lesson). Refusal is owned by the pass predicates below, not by ambient
//! process-global rollback state.

use std::collections::{HashMap, HashSet};

use crate::tir::blocks::{BlockId, Terminator, TirBlock};
use crate::tir::function::TirFunction;
use crate::tir::ops::{AttrDict, Dialect, OpCode, TirOp};
use crate::tir::types::TirType;
use crate::tir::values::{TirValue, ValueId};

use super::PassStats;

/// Why a loop was refused. Reported by the stats instrument so real-world
/// refusal layers are visible instead of silently inert (the L4 /
/// needs_inlining / promotion lesson, three times over).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Refusal {
    /// Function has try/except/generator-state handler structure.
    HasExceptionHandlers,
    /// Function contains generator/async state ops.
    Stateful,
    /// Header's guard chain is not the canonical header→guard→CondBranch.
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
    /// BigInt-seeded or parameter-seeded accumulator — must stay boxed).
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

/// One qualifying accumulator phi and its latch update. Plans are built in
/// header-arg order, so the vector index doubles as the arg index.
struct PhiPlan {
    /// The phi value itself.
    phi: ValueId,
    /// The init value passed by the preheader (chases to ConstInt).
    init_arg: ValueId,
    /// Index in the body ops of the pure arithmetic op (`Add` or `Mul`) that
    /// updates this phi.
    update_op_index: usize,
    /// The original update opcode (`Add` or `Mul`). The fast-loop swap maps it
    /// to the matching checked op (`CheckedAdd`/`CheckedMul`); the slow loop —
    /// cloned from the body BEFORE the swap — keeps this plain opcode, so its
    /// boxed `molt_add`/`molt_mul` path stays BigInt-exact on re-execution.
    update_opcode: OpCode,
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
        b.ops.iter().any(|op| {
            matches!(
                op.opcode,
                OpCode::StateSwitch
                    | OpCode::StateTransition
                    | OpCode::StateYield
                    | OpCode::Yield
                    | OpCode::YieldFrom
                    | OpCode::ChanSendYield
                    | OpCode::ChanRecvYield
                    | OpCode::AllocTask
            )
        })
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
        match try_peel_loop(func, header) {
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

/// Blocks reachable from the function entry via terminator edges.
fn reachable_blocks(func: &TirFunction) -> HashSet<BlockId> {
    let mut seen = HashSet::new();
    let mut work = vec![func.entry_block];
    while let Some(bid) = work.pop() {
        if !seen.insert(bid) {
            continue;
        }
        let Some(block) = func.blocks.get(&bid) else {
            continue;
        };
        match &block.terminator {
            Terminator::Branch { target, .. } => work.push(*target),
            Terminator::CondBranch {
                then_block,
                else_block,
                ..
            } => {
                work.push(*then_block);
                work.push(*else_block);
            }
            Terminator::Switch { cases, default, .. }
            | Terminator::StateDispatch { cases, default, .. } => {
                work.extend(cases.iter().map(|(_, b, _)| *b));
                work.push(*default);
            }
            Terminator::Return { .. } | Terminator::Unreachable => {}
        }
    }
    seen
}

/// Chase a value backward through `Copy` results to its origin within `ops`
/// (a map from result id to the defining op). Marker copies (`store_var`
/// round-trips) are 2-operand same-value Copies; both shapes chase through
/// `operands[0]`.
fn chase_copies(start: ValueId, def_by_result: &HashMap<ValueId, &TirOp>) -> ValueId {
    let mut cur = start;
    let mut fuel = 64; // structural bound; copy chains are short
    while fuel > 0 {
        fuel -= 1;
        match def_by_result.get(&cur) {
            Some(op) if op.opcode == OpCode::Copy && !op.operands.is_empty() => {
                cur = op.operands[0];
            }
            _ => break,
        }
    }
    cur
}

/// Attempt to peel the loop rooted at `header`. Returns the number of ops
/// added on success.
fn try_peel_loop(func: &mut TirFunction, header: BlockId) -> Result<usize, Refusal> {
    let reachable = reachable_blocks(func);

    // ── Shape: header(args) --Branch--> guard {…, cmp, CondBranch(body, exit)} ──
    let header_block = func.blocks.get(&header).ok_or(Refusal::NoCanonicalGuard)?;
    if header_block.args.is_empty() || !header_block.ops.iter().all(is_ignorable_marker) {
        return Err(Refusal::NoCanonicalGuard);
    }
    let guard = match &header_block.terminator {
        Terminator::Branch { target, args } if args.is_empty() => *target,
        _ => return Err(Refusal::NoCanonicalGuard),
    };
    let phis: Vec<TirValue> = header_block.args.clone();

    let guard_block = func.blocks.get(&guard).ok_or(Refusal::NoCanonicalGuard)?;
    if !guard_block.args.is_empty() {
        return Err(Refusal::NoCanonicalGuard);
    }
    let (cond, body, body_args, exit, exit_args) = match &guard_block.terminator {
        Terminator::CondBranch {
            cond,
            then_block,
            then_args,
            else_block,
            else_args,
        } => (
            *cond,
            *then_block,
            then_args.clone(),
            *else_block,
            else_args.clone(),
        ),
        _ => return Err(Refusal::NoCanonicalGuard),
    };
    if !body_args.is_empty() || !exit_args.is_empty() {
        return Err(Refusal::GuardExitArgs);
    }

    // Guard ops: ignorable markers + Copies + ONE compare producing `cond`.
    let mut guard_compare: Option<&TirOp> = None;
    for op in &guard_block.ops {
        match op.opcode {
            OpCode::Copy => {}
            OpCode::Lt | OpCode::Le | OpCode::Gt | OpCode::Ge | OpCode::Ne | OpCode::Eq
                if op.results.first() == Some(&cond) =>
            {
                if guard_compare.is_some() {
                    return Err(Refusal::ImpureBody);
                }
                guard_compare = Some(op);
            }
            _ => return Err(Refusal::ImpureBody),
        }
    }
    if guard_compare.is_none() {
        return Err(Refusal::NoCanonicalGuard);
    }

    // ── Body: a single linear block latching straight back to the header ──
    let body_block = func.blocks.get(&body).ok_or(Refusal::MultiBlockBody)?;
    if !body_block.args.is_empty() {
        return Err(Refusal::MultiBlockBody);
    }
    let latch_args = match &body_block.terminator {
        Terminator::Branch { target, args } if *target == header => args.clone(),
        _ => return Err(Refusal::MultiBlockBody),
    };
    if latch_args.len() != phis.len() {
        return Err(Refusal::MultiBlockBody);
    }
    // Body purity: Copies (incl. markers), `Add`/`Mul`s, and constant
    // materialisation only (the frontend leaves un-hoisted literal steps —
    // e.g. `total + -20000000` — as in-body ConstInts; they are pure).
    // Anything that can call, store, load, raise, or observe runtime state
    // disqualifies — re-execution of the failed iteration on the slow path
    // must be observationally identical. `Mul` is pure exactly like `Add`
    // (no side effect, deterministic), so a multiply accumulator
    // (`prod = prod * i`) re-executes BigInt-exact on the boxed slow loop.
    for op in &body_block.ops {
        match op.opcode {
            OpCode::Copy | OpCode::Add | OpCode::Mul | OpCode::ConstInt => {}
            _ => return Err(Refusal::ImpureBody),
        }
    }

    let loop_blocks: HashSet<BlockId> = [header, guard, body].into_iter().collect();

    // ── Header predecessors: one reachable preheader + the latch; any
    //    unreachable extras (the vestigial loop-else) get their edge args
    //    retargeted to the preheader's init args later. ──
    let mut preheader: Option<BlockId> = None;
    let mut stray_preds: Vec<BlockId> = Vec::new();
    for (bid, block) in &func.blocks {
        if *bid == body {
            continue;
        }
        let targets_header = match &block.terminator {
            Terminator::Branch { target, .. } => *target == header,
            Terminator::CondBranch {
                then_block,
                else_block,
                ..
            } => *then_block == header || *else_block == header,
            Terminator::Switch { cases, default, .. } => {
                cases.iter().any(|(_, b, _)| *b == header) || *default == header
            }
            _ => false,
        };
        if !targets_header {
            continue;
        }
        if reachable.contains(bid) {
            if preheader.is_some() {
                return Err(Refusal::MultiplePreheaders);
            }
            preheader = Some(*bid);
        } else {
            stray_preds.push(*bid);
        }
    }
    let preheader = preheader.ok_or(Refusal::MultiplePreheaders)?;
    let init_args = match &func.blocks[&preheader].terminator {
        Terminator::Branch { target, args } if *target == header => args.clone(),
        // v1 keeps the preheader shape strict: a guarded preheader would need
        // per-edge arg extension on the right arm only.
        _ => return Err(Refusal::NonBranchPreheader),
    };
    if init_args.len() != phis.len() {
        return Err(Refusal::NonBranchPreheader);
    }

    // ── Exit: single predecessor (the guard), so post-loop uses of the phis
    //    can be rerouted through fresh exit args fed by both loops. ──
    for (bid, block) in &func.blocks {
        if *bid == guard || !reachable.contains(bid) {
            continue;
        }
        let targets_exit = match &block.terminator {
            Terminator::Branch { target, .. } => *target == exit,
            Terminator::CondBranch {
                then_block,
                else_block,
                ..
            } => *then_block == exit || *else_block == exit,
            Terminator::Switch { cases, default, .. } => {
                cases.iter().any(|(_, b, _)| *b == exit) || *default == exit
            }
            _ => false,
        };
        if targets_exit {
            return Err(Refusal::ExitHasOtherPreds);
        }
    }

    // ── Qualify every phi: I64-typed, ConstInt init, latch update that is
    //    either a recognised Add/Mul accumulator or rejected. ALL phis must
    //    qualify (all-or-nothing: a single boxed phi would re-box the loop). ──
    let def_by_result: HashMap<ValueId, &TirOp> = func
        .blocks
        .values()
        .flat_map(|b| b.ops.iter())
        .flat_map(|op| op.results.iter().map(move |r| (*r, op)))
        .collect();
    let phi_ids: HashSet<ValueId> = phis.iter().map(|v| v.id).collect();

    let body_block = &func.blocks[&body];
    let body_defs: HashMap<ValueId, usize> = body_block
        .ops
        .iter()
        .enumerate()
        .flat_map(|(i, op)| op.results.iter().map(move |r| (*r, i)))
        .collect();

    let mut plans: Vec<PhiPlan> = Vec::new();
    for (arg_index, phi) in phis.iter().enumerate() {
        if !matches!(func.value_types.get(&phi.id), Some(TirType::I64)) {
            return Err(Refusal::NonIntPhi);
        }
        let init_arg = init_args[arg_index];
        if !matches!(
            def_by_result.get(&chase_copies(init_arg, &def_by_result)),
            Some(op) if op.opcode == OpCode::ConstInt
        ) {
            return Err(Refusal::NonConstInit);
        }
        let update = chase_copies(latch_args[arg_index], &def_by_result);
        let Some(&update_op_index) = body_defs.get(&update) else {
            return Err(Refusal::NonArithmeticUpdate);
        };
        let add_op = &body_block.ops[update_op_index];
        // The update must be a binary I64 `Add` or `Mul`. Both have a total
        // hardware-overflow-flagged checked form (`CheckedAdd`/`CheckedMul`)
        // and are pure, so the dual-loop transform is sound for either.
        if !matches!(add_op.opcode, OpCode::Add | OpCode::Mul)
            || add_op.operands.len() != 2
            || add_op.results.len() != 1
            || !matches!(func.value_types.get(&add_op.results[0]), Some(TirType::I64))
        {
            return Err(Refusal::NonArithmeticUpdate);
        }
        // Each operand must chase to a header phi, a loop-invariant
        // value (defined outside the loop blocks), or an in-body ConstInt
        // (a literal step the frontend left un-hoisted — constant, so
        // trivially invariant).
        for &operand in &add_op.operands {
            let origin = chase_copies(operand, &def_by_result);
            let in_body_const = body_defs
                .get(&origin)
                .is_some_and(|&i| body_block.ops[i].opcode == OpCode::ConstInt);
            let invariant = in_body_const
                || (!body_defs.contains_key(&origin)
                    && !phi_ids.contains(&origin)
                    && !func.blocks[&guard]
                        .ops
                        .iter()
                        .any(|op| op.results.contains(&origin)));
            if !phi_ids.contains(&origin) && !invariant {
                return Err(Refusal::NonArithmeticUpdate);
            }
        }
        plans.push(PhiPlan {
            phi: phi.id,
            init_arg,
            update_op_index,
            update_opcode: add_op.opcode,
        });
    }
    if plans.is_empty() {
        return Err(Refusal::NonArithmeticUpdate);
    }
    // Two phis updated by the same arithmetic op (aliased accumulators) would
    // make the checked-op swap ambiguous.
    {
        let mut seen = HashSet::new();
        for p in &plans {
            if !seen.insert(p.update_op_index) {
                return Err(Refusal::NonArithmeticUpdate);
            }
        }
    }

    // ── Live-out audit: nothing defined inside the loop may be used outside,
    //    except the header phis (which are rerouted through exit args). ──
    let mut loop_defined: HashSet<ValueId> = phi_ids.clone();
    for bid in &loop_blocks {
        for op in &func.blocks[bid].ops {
            loop_defined.extend(op.results.iter().copied());
        }
    }
    let mut exit_live_phis: Vec<ValueId> = Vec::new();
    for (bid, block) in &func.blocks {
        if loop_blocks.contains(bid) {
            continue;
        }
        let mut check_use = |v: ValueId| -> Result<(), Refusal> {
            if phi_ids.contains(&v) {
                if !exit_live_phis.contains(&v) {
                    exit_live_phis.push(v);
                }
                Ok(())
            } else if loop_defined.contains(&v) {
                Err(Refusal::InteriorLiveOut)
            } else {
                Ok(())
            }
        };
        for op in &block.ops {
            for &v in &op.operands {
                check_use(v)?;
            }
        }
        match &block.terminator {
            Terminator::Branch { args, .. } => {
                for &v in args {
                    check_use(v)?;
                }
            }
            Terminator::CondBranch {
                cond,
                then_args,
                else_args,
                ..
            } => {
                check_use(*cond)?;
                for &v in then_args.iter().chain(else_args.iter()) {
                    check_use(v)?;
                }
            }
            Terminator::Switch {
                value,
                cases,
                default_args,
                ..
            } => {
                check_use(*value)?;
                for (_, _, args) in cases {
                    for &v in args {
                        check_use(v)?;
                    }
                }
                for &v in default_args {
                    check_use(v)?;
                }
            }
            // `StateDispatch` has no condition value; only its per-edge args.
            Terminator::StateDispatch {
                cases,
                default_args,
                ..
            } => {
                for (_, _, args) in cases {
                    for &v in args {
                        check_use(v)?;
                    }
                }
                for &v in default_args {
                    check_use(v)?;
                }
            }
            Terminator::Return { values } => {
                for &v in values {
                    check_use(v)?;
                }
            }
            Terminator::Unreachable => {}
        }
    }

    // ════════════════════════ TRANSFORM (infallible from here) ═══════════════════════

    let mut ops_added = 0usize;

    // 1. Clone {header, guard, body} verbatim → the slow (boxed) loop. The
    //    clone happens FIRST, from the pristine blocks, so the slow loop
    //    keeps plain `Add`s (the boxed BigInt-exact path).
    let slow_header = func.fresh_block();
    let slow_guard = func.fresh_block();
    let slow_body = func.fresh_block();
    let block_map: HashMap<BlockId, BlockId> = [
        (header, slow_header),
        (guard, slow_guard),
        (body, slow_body),
    ]
    .into_iter()
    .collect();

    let mut value_map: HashMap<ValueId, ValueId> = HashMap::new();
    let mut new_value_types: Vec<(ValueId, TirType)> = Vec::new();
    {
        let mut remap = |old: ValueId,
                         func_next: &mut u32,
                         value_types: &HashMap<ValueId, TirType>|
         -> ValueId {
            let fresh = ValueId(*func_next);
            *func_next += 1;
            value_map.insert(old, fresh);
            if let Some(ty) = value_types.get(&old) {
                new_value_types.push((fresh, ty.clone()));
            }
            fresh
        };
        // Pre-allocate fresh ids for every value DEFINED inside the loop
        // (args + op results); operands defined outside remap to themselves.
        let mut next = func.next_value;
        for bid in [header, guard, body] {
            let block = &func.blocks[&bid];
            for arg in &block.args {
                remap(arg.id, &mut next, &func.value_types);
            }
            for op in &block.ops {
                for &r in &op.results {
                    remap(r, &mut next, &func.value_types);
                }
            }
        }
        func.next_value = next;
    }
    for (id, ty) in &new_value_types {
        func.value_types.insert(*id, ty.clone());
    }

    let map_value = |v: ValueId, value_map: &HashMap<ValueId, ValueId>| -> ValueId {
        value_map.get(&v).copied().unwrap_or(v)
    };
    let clone_block = |src: &TirBlock,
                       new_id: BlockId,
                       value_map: &HashMap<ValueId, ValueId>,
                       exit: BlockId,
                       exit_arg: Option<ValueId>|
     -> TirBlock {
        let args = src
            .args
            .iter()
            .map(|a| TirValue {
                id: map_value(a.id, value_map),
                ty: a.ty.clone(),
            })
            .collect();
        let ops = src
            .ops
            .iter()
            .map(|op| TirOp {
                dialect: op.dialect,
                opcode: op.opcode,
                operands: op
                    .operands
                    .iter()
                    .map(|&v| map_value(v, value_map))
                    .collect(),
                results: op
                    .results
                    .iter()
                    .map(|&v| map_value(v, value_map))
                    .collect(),
                attrs: op.attrs.clone(),
                source_span: op.source_span,
            })
            .collect();
        let terminator = match &src.terminator {
            Terminator::Branch { target, args } => Terminator::Branch {
                target: *block_map.get(target).unwrap_or(target),
                args: args.iter().map(|&v| map_value(v, value_map)).collect(),
            },
            Terminator::CondBranch {
                cond,
                then_block,
                then_args,
                else_block,
                else_args,
            } => {
                let mapped_else = *block_map.get(else_block).unwrap_or(else_block);
                let mut mapped_else_args: Vec<ValueId> =
                    else_args.iter().map(|&v| map_value(v, value_map)).collect();
                // The slow guard's exit edge feeds the (new) exit arg with the
                // slow accumulator phis.
                if mapped_else == exit
                    && let Some(arg) = exit_arg
                {
                    mapped_else_args.push(arg);
                }
                Terminator::CondBranch {
                    cond: map_value(*cond, value_map),
                    then_block: *block_map.get(then_block).unwrap_or(then_block),
                    then_args: then_args.iter().map(|&v| map_value(v, value_map)).collect(),
                    else_block: mapped_else,
                    else_args: mapped_else_args,
                }
            }
            other => other.clone(),
        };
        TirBlock {
            id: new_id,
            args,
            ops,
            terminator,
        }
    };

    // The exit args (one per exit-live phi, in `exit_live_phis` order) — the
    // slow guard passes its remapped phi values.
    let slow_exit_args: Vec<ValueId> = exit_live_phis
        .iter()
        .map(|&phi| map_value(phi, &value_map))
        .collect();

    let slow_header_block = {
        let src = &func.blocks[&header];
        clone_block(src, slow_header, &value_map, exit, None)
    };
    let slow_guard_block = {
        let src = &func.blocks[&guard];
        // Single exit-live phi is the common case; general case appends all.
        let mut blk = clone_block(src, slow_guard, &value_map, exit, None);
        if let Terminator::CondBranch {
            else_block,
            else_args,
            ..
        } = &mut blk.terminator
            && *else_block == exit
        {
            else_args.extend(slow_exit_args.iter().copied());
        }
        blk
    };
    let slow_body_block = {
        let src = &func.blocks[&body];
        clone_block(src, slow_body, &value_map, exit, None)
    };
    ops_added +=
        slow_header_block.ops.len() + slow_guard_block.ops.len() + slow_body_block.ops.len();
    func.blocks.insert(slow_header, slow_header_block);
    func.blocks.insert(slow_guard, slow_guard_block);
    func.blocks.insert(slow_body, slow_body_block);
    // Deliberately NO loop_roles/loop_pairs/loop_cond_blocks for the clones:
    // the cold loop linearises through the generic label/jump path.

    // 2. Extend the fast header with the new loop-carried phis:
    //    of (Bool), then prev_<phi> (I64) for every plan, in plan order.
    let of_phi = func.fresh_value();
    func.value_types.insert(of_phi, TirType::Bool);
    let prev_phis: Vec<ValueId> = plans
        .iter()
        .map(|_| {
            let v = func.fresh_value();
            func.value_types.insert(v, TirType::I64);
            v
        })
        .collect();
    {
        let header_block = func.blocks.get_mut(&header).expect("header exists");
        header_block.args.push(TirValue {
            id: of_phi,
            ty: TirType::Bool,
        });
        for &pv in &prev_phis {
            header_block.args.push(TirValue {
                id: pv,
                ty: TirType::I64,
            });
        }
    }

    // 3. Preheader: materialise `false` and extend the init edge args:
    //    [..init, false, init(plan_0), init(plan_1), …].
    let false_const = func.fresh_value();
    func.value_types.insert(false_const, TirType::Bool);
    {
        let pre = func.blocks.get_mut(&preheader).expect("preheader exists");
        let mut attrs = AttrDict::new();
        attrs.insert("value".into(), crate::tir::ops::AttrValue::Bool(false));
        pre.ops.push(TirOp {
            dialect: Dialect::Molt,
            opcode: OpCode::ConstBool,
            operands: vec![],
            results: vec![false_const],
            attrs,
            source_span: None,
        });
        ops_added += 1;
        if let Terminator::Branch { args, .. } = &mut pre.terminator {
            args.push(false_const);
            for plan in &plans {
                args.push(plan.init_arg);
            }
        }
    }

    // 4. Stray (unreachable) preds: retarget their header-edge args to the
    //    preheader's init shape so no `None` ever appears as a phi incoming.
    let stray_args: Vec<ValueId> = {
        let mut v: Vec<ValueId> = plans.iter().map(|p| p.init_arg).collect();
        // Original arity might exceed the planned phis only if plans != args;
        // plans cover every header arg by construction (all-or-nothing).
        v.push(false_const);
        for plan in &plans {
            v.push(plan.init_arg);
        }
        v
    };
    debug_assert_eq!(stray_args.len(), phis.len() + 1 + plans.len());
    for stray in &stray_preds {
        let block = func.blocks.get_mut(stray).expect("stray pred exists");
        let retarget = |args: &mut Vec<ValueId>| {
            args.clear();
            args.extend(stray_args.iter().copied());
        };
        match &mut block.terminator {
            Terminator::Branch { target, args } if *target == header => retarget(args),
            Terminator::CondBranch {
                then_block,
                then_args,
                else_block,
                else_args,
                ..
            } => {
                if *then_block == header {
                    retarget(then_args);
                }
                if *else_block == header {
                    retarget(else_args);
                }
            }
            _ => {}
        }
    }

    // 5. Guard: brk = And(cond, Not(of)); retarget the exit edge to the
    //    dispatch block.
    let dispatch = func.fresh_block();
    let not_of = func.fresh_value();
    let brk = func.fresh_value();
    func.value_types.insert(not_of, TirType::Bool);
    func.value_types.insert(brk, TirType::Bool);
    {
        let guard_block = func.blocks.get_mut(&guard).expect("guard exists");
        guard_block.ops.push(TirOp {
            dialect: Dialect::Molt,
            opcode: OpCode::Not,
            operands: vec![of_phi],
            results: vec![not_of],
            attrs: AttrDict::new(),
            source_span: None,
        });
        guard_block.ops.push(TirOp {
            dialect: Dialect::Molt,
            opcode: OpCode::And,
            operands: vec![cond, not_of],
            results: vec![brk],
            attrs: AttrDict::new(),
            source_span: None,
        });
        ops_added += 2;
        if let Terminator::CondBranch {
            cond: c,
            else_block,
            ..
        } = &mut guard_block.terminator
        {
            *c = brk;
            *else_block = dispatch;
        }
    }

    // 6. Body: swap each plan's Add → CheckedAdd / Mul → CheckedMul (keeping
    //    the original result ValueId so the latch args stay valid), fan the
    //    flags in with Or, snapshot the pre-iteration phi values, and extend
    //    the latch args. The matching checked op is chosen from the recorded
    //    update opcode; the slow loop was cloned BEFORE this swap, so it keeps
    //    the plain `Add`/`Mul` (BigInt-exact on re-execution).
    let mut flag_values: Vec<ValueId> = Vec::new();
    {
        let body_block = func.blocks.get_mut(&body).expect("body exists");
        for plan in &plans {
            let flag = ValueId(func.next_value);
            func.next_value += 1;
            func.value_types.insert(flag, TirType::Bool);
            let add_op = &mut body_block.ops[plan.update_op_index];
            add_op.opcode = match plan.update_opcode {
                OpCode::Add => OpCode::CheckedAdd,
                OpCode::Mul => OpCode::CheckedMul,
                // Unreachable: phi-qual admits only Add/Mul updates.
                other => unreachable!(
                    "overflow_peel: unexpected update opcode {other:?} (phi-qual \
                     admits only Add/Mul)"
                ),
            };
            add_op.results.push(flag);
            flag_values.push(flag);
        }
        // of' = Or(flag_0, flag_1, …) — left fold.
        let mut of_next = flag_values[0];
        for &f in &flag_values[1..] {
            let folded = ValueId(func.next_value);
            func.next_value += 1;
            func.value_types.insert(folded, TirType::Bool);
            body_block.ops.push(TirOp {
                dialect: Dialect::Molt,
                opcode: OpCode::Or,
                operands: vec![of_next, f],
                results: vec![folded],
                attrs: AttrDict::new(),
                source_span: None,
            });
            ops_added += 1;
            of_next = folded;
        }
        // prev_k = Copy(phi_k) — the pre-iteration snapshot that seeds the
        // slow loop's re-execution of the failed iteration.
        let mut prev_next: Vec<ValueId> = Vec::new();
        for plan in &plans {
            let snap = ValueId(func.next_value);
            func.next_value += 1;
            func.value_types.insert(snap, TirType::I64);
            body_block.ops.push(TirOp {
                dialect: Dialect::Molt,
                opcode: OpCode::Copy,
                operands: vec![plan.phi],
                results: vec![snap],
                attrs: AttrDict::new(),
                source_span: None,
            });
            ops_added += 1;
            prev_next.push(snap);
        }
        if let Terminator::Branch { args, .. } = &mut body_block.terminator {
            args.push(of_next);
            args.extend(prev_next.iter().copied());
        }
    }

    // 7. Exit args: one fresh arg per exit-live phi; rewrite every use of
    //    those phis OUTSIDE the loop to the corresponding exit arg.
    let exit_arg_ids: Vec<ValueId> = exit_live_phis
        .iter()
        .map(|&phi| {
            let v = func.fresh_value();
            if let Some(ty) = func.value_types.get(&phi).cloned() {
                func.value_types.insert(v, ty);
            }
            v
        })
        .collect();
    {
        let exit_block = func.blocks.get_mut(&exit).expect("exit exists");
        for (i, &arg) in exit_arg_ids.iter().enumerate() {
            exit_block.args.push(TirValue {
                id: arg,
                ty: func
                    .value_types
                    .get(&exit_live_phis[i])
                    .cloned()
                    .unwrap_or(TirType::I64),
            });
        }
    }
    let phi_to_exit_arg: HashMap<ValueId, ValueId> = exit_live_phis
        .iter()
        .copied()
        .zip(exit_arg_ids.iter().copied())
        .collect();
    let rewrite_blocks: Vec<BlockId> = func
        .blocks
        .keys()
        .filter(|b| {
            !loop_blocks.contains(b)
                && **b != slow_header
                && **b != slow_guard
                && **b != slow_body
                && **b != dispatch
        })
        .copied()
        .collect();
    for bid in rewrite_blocks {
        let block = func.blocks.get_mut(&bid).expect("block exists");
        let rw = |v: &mut ValueId| {
            if let Some(&replacement) = phi_to_exit_arg.get(v) {
                *v = replacement;
            }
        };
        for op in &mut block.ops {
            for v in &mut op.operands {
                rw(v);
            }
        }
        match &mut block.terminator {
            Terminator::Branch { args, .. } => args.iter_mut().for_each(rw),
            Terminator::CondBranch {
                cond,
                then_args,
                else_args,
                ..
            } => {
                rw(cond);
                then_args.iter_mut().for_each(rw);
                else_args.iter_mut().for_each(rw);
            }
            Terminator::Switch {
                value,
                cases,
                default_args,
                ..
            } => {
                rw(value);
                for (_, _, args) in cases {
                    args.iter_mut().for_each(rw);
                }
                default_args.iter_mut().for_each(rw);
            }
            // `StateDispatch` has no condition value; only its per-edge args.
            Terminator::StateDispatch {
                cases,
                default_args,
                ..
            } => {
                for (_, _, args) in cases {
                    args.iter_mut().for_each(rw);
                }
                default_args.iter_mut().for_each(rw);
            }
            Terminator::Return { values } => values.iter_mut().for_each(rw),
            Terminator::Unreachable => {}
        }
    }

    // 8. Dispatch: CondBranch(of, slow_entry, exit(fast phi values)). The
    //    fast path passes the (exact, non-overflowed) phis straight to the
    //    exit args; the boxing happens at the existing escape discipline
    //    (store into a non-raw slot / return) — no explicit BoxVal needed.
    let slow_entry = func.fresh_block();
    let fast_exit_args: Vec<ValueId> = exit_live_phis.clone();
    func.blocks.insert(
        dispatch,
        TirBlock {
            id: dispatch,
            args: vec![],
            ops: vec![],
            terminator: Terminator::CondBranch {
                cond: of_phi,
                then_block: slow_entry,
                then_args: vec![],
                else_block: exit,
                else_args: fast_exit_args,
            },
        },
    );
    // slow_entry: → slow_header(prev_0, prev_1, …) — the pre-iteration
    // snapshot seeds the boxed re-execution of the failed iteration.
    func.blocks.insert(
        slow_entry,
        TirBlock {
            id: slow_entry,
            args: vec![],
            ops: vec![],
            terminator: Terminator::Branch {
                target: slow_header,
                args: prev_phis.clone(),
            },
        },
    );

    Ok(ops_added)
}

/// Header blocks may carry zero-result marker Copies (line markers). Any op
/// with results disqualifies the canonical empty-header shape.
fn is_ignorable_marker(op: &TirOp) -> bool {
    op.opcode == OpCode::Copy && op.results.is_empty()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::tir::analysis::AnalysisManager;
    use crate::tir::blocks::LoopRole;
    use crate::tir::ops::AttrValue;
    use crate::tir::verify::verify_function;

    /// Build the live-shape fixture: the exact CFG `peel_sum.py`'s `compute`
    /// produces post-pipeline (entry consts → 2-phi header → guard(Lt) →
    /// linear body with two marker-wrapped Adds → exit Return), including the
    /// vestigial unreachable loop-else pred passing ConstNone.
    fn live_shape_function() -> TirFunction {
        let mut func = TirFunction::new(
            "peel_fixture".into(),
            vec![TirType::DynBox],
            TirType::DynBox,
        );
        let header = func.fresh_block();
        let guard = func.fresh_block();
        let body = func.fresh_block();
        let stray = func.fresh_block();
        let exit = func.fresh_block();

        let n = ValueId(0); // entry arg (param)
        let fresh = |func: &mut TirFunction, ty: TirType| {
            let v = func.fresh_value();
            func.value_types.insert(v, ty);
            v
        };
        let c_total = fresh(&mut func, TirType::I64);
        let c_i = fresh(&mut func, TirType::I64);
        let c_one = fresh(&mut func, TirType::I64);
        let none_v = fresh(&mut func, TirType::None);
        let t_phi = fresh(&mut func, TirType::I64);
        let i_phi = fresh(&mut func, TirType::I64);
        let i_copy = fresh(&mut func, TirType::I64);
        let cond = fresh(&mut func, TirType::Bool);
        let t_in = fresh(&mut func, TirType::I64);
        let i_in = fresh(&mut func, TirType::I64);
        let t_sum = fresh(&mut func, TirType::I64);
        let t_marker = fresh(&mut func, TirType::I64);
        let i_in2 = fresh(&mut func, TirType::I64);
        let i_sum = fresh(&mut func, TirType::I64);
        let i_marker = fresh(&mut func, TirType::I64);
        let ret_copy = fresh(&mut func, TirType::I64);

        let const_op = |opcode: OpCode, value: i64, result: ValueId| {
            let mut attrs = AttrDict::new();
            attrs.insert("value".into(), AttrValue::Int(value));
            TirOp {
                dialect: Dialect::Molt,
                opcode,
                operands: vec![],
                results: vec![result],
                attrs,
                source_span: None,
            }
        };
        let op = |opcode: OpCode, operands: Vec<ValueId>, results: Vec<ValueId>| TirOp {
            dialect: Dialect::Molt,
            opcode,
            operands,
            results,
            attrs: AttrDict::new(),
            source_span: None,
        };

        // entry: consts → header(t0, i0)
        {
            let entry = func.blocks.get_mut(&func.entry_block).unwrap();
            entry.ops.push(const_op(OpCode::ConstInt, 0, c_total));
            entry.ops.push(const_op(OpCode::ConstInt, 0, c_i));
            entry.ops.push(const_op(OpCode::ConstInt, 1, c_one));
            entry.terminator = Terminator::Branch {
                target: header,
                args: vec![c_total, c_i],
            };
        }
        // header(t, i) → guard
        func.blocks.insert(
            header,
            TirBlock {
                id: header,
                args: vec![
                    TirValue {
                        id: t_phi,
                        ty: TirType::I64,
                    },
                    TirValue {
                        id: i_phi,
                        ty: TirType::I64,
                    },
                ],
                ops: vec![],
                terminator: Terminator::Branch {
                    target: guard,
                    args: vec![],
                },
            },
        );
        // guard: i' = Copy(i); cond = Lt(i', n); CondBranch(cond, body, exit)
        func.blocks.insert(
            guard,
            TirBlock {
                id: guard,
                args: vec![],
                ops: vec![
                    op(OpCode::Copy, vec![i_phi], vec![i_copy]),
                    op(OpCode::Lt, vec![i_copy, n], vec![cond]),
                ],
                terminator: Terminator::CondBranch {
                    cond,
                    then_block: body,
                    then_args: vec![],
                    else_block: exit,
                    else_args: vec![],
                },
            },
        );
        // body: t+i and i+1, each through marker copies → header
        func.blocks.insert(
            body,
            TirBlock {
                id: body,
                args: vec![],
                ops: vec![
                    op(OpCode::Copy, vec![t_phi], vec![t_in]),
                    op(OpCode::Copy, vec![i_phi], vec![i_in]),
                    op(OpCode::Add, vec![t_in, i_in], vec![t_sum]),
                    op(OpCode::Copy, vec![t_sum, t_sum], vec![t_marker]),
                    op(OpCode::Copy, vec![i_phi], vec![i_in2]),
                    op(OpCode::Add, vec![i_in2, c_one], vec![i_sum]),
                    op(OpCode::Copy, vec![i_sum, i_sum], vec![i_marker]),
                ],
                terminator: Terminator::Branch {
                    target: header,
                    args: vec![t_marker, i_marker],
                },
            },
        );
        // stray (unreachable loop-else): ConstNone → header(None, None)
        func.blocks.insert(
            stray,
            TirBlock {
                id: stray,
                args: vec![],
                ops: vec![op(OpCode::ConstNone, vec![], vec![none_v])],
                terminator: Terminator::Branch {
                    target: header,
                    args: vec![none_v, none_v],
                },
            },
        );
        // exit: ret_copy = Copy(t); Return ret_copy
        func.blocks.insert(
            exit,
            TirBlock {
                id: exit,
                args: vec![],
                ops: vec![op(OpCode::Copy, vec![t_phi], vec![ret_copy])],
                terminator: Terminator::Return {
                    values: vec![ret_copy],
                },
            },
        );

        func.loop_roles.insert(header, LoopRole::LoopHeader);
        func.loop_roles.insert(stray, LoopRole::LoopEnd);
        func.loop_pairs.insert(header, stray);
        func.loop_cond_blocks.insert(header, guard);
        func
    }

    fn run_peel(func: &mut TirFunction) -> PassStats {
        let mut am = AnalysisManager::new();
        run(func, &mut am)
    }

    #[test]
    fn live_shape_peels_and_verifies() {
        let mut func = live_shape_function();
        let blocks_before = func.blocks.len();
        let stats = run_peel(&mut func);
        assert!(stats.ops_added > 0, "the live shape must peel");
        // Slow loop (3) + dispatch + slow_entry.
        assert_eq!(func.blocks.len(), blocks_before + 5);
        verify_function(&func).expect("peeled function must verify");

        // Both body Adds became CheckedAdds with 2 results.
        let body_checked: usize = func
            .blocks
            .values()
            .flat_map(|b| b.ops.iter())
            .filter(|op| op.opcode == OpCode::CheckedAdd)
            .count();
        assert_eq!(body_checked, 2, "both phi updates become CheckedAdd");

        // Exactly one slow loop with plain Adds survives.
        let plain_adds: usize = func
            .blocks
            .values()
            .flat_map(|b| b.ops.iter())
            .filter(|op| op.opcode == OpCode::Add)
            .count();
        assert_eq!(plain_adds, 2, "the slow clone keeps the plain Adds");

        // The fast header now carries 2 + 1 + 2 phis.
        let header = func
            .loop_roles
            .iter()
            .find(|(_, r)| **r == LoopRole::LoopHeader)
            .map(|(b, _)| *b)
            .unwrap();
        assert_eq!(func.blocks[&header].args.len(), 5);

        // The stray pred no longer passes ConstNone to the header.
        let stray_args = func
            .blocks
            .values()
            .filter_map(|b| match &b.terminator {
                Terminator::Branch { target, args }
                    if *target == header && b.ops.iter().any(|o| o.opcode == OpCode::ConstNone) =>
                {
                    Some(args.clone())
                }
                _ => None,
            })
            .next()
            .expect("stray pred still targets the header");
        assert_eq!(stray_args.len(), 5);
        let none_ids: HashSet<ValueId> = func
            .blocks
            .values()
            .flat_map(|b| b.ops.iter())
            .filter(|o| o.opcode == OpCode::ConstNone)
            .flat_map(|o| o.results.iter().copied())
            .collect();
        assert!(
            stray_args.iter().all(|a| !none_ids.contains(a)),
            "no ConstNone reaches the header phis"
        );

        // The exit block gained an arg and the post-loop use was rewired:
        // the block that Returns must no longer read the header phi
        // (ValueId(5) = t_phi in this fixture) — in-loop uses keep it.
        let exit_block = func
            .blocks
            .values()
            .find(|b| matches!(b.terminator, Terminator::Return { .. }) && !b.ops.is_empty())
            .expect("exit block exists");
        assert_eq!(exit_block.args.len(), 1, "exit gains one arg");
        assert!(
            !exit_block
                .ops
                .iter()
                .any(|op| op.operands.contains(&ValueId(5))),
            "post-loop Copy must read the exit arg, not the header phi"
        );
    }

    #[test]
    fn live_shape_mul_updates_become_checked_mul() {
        let mut func = live_shape_function();
        for block in func.blocks.values_mut() {
            for op in &mut block.ops {
                if op.opcode == OpCode::Add {
                    op.opcode = OpCode::Mul;
                }
            }
        }

        let stats = run_peel(&mut func);
        assert!(stats.ops_added > 0, "the multiply shape must peel");
        verify_function(&func).expect("peeled multiply function must verify");

        let checked_mul: usize = func
            .blocks
            .values()
            .flat_map(|b| b.ops.iter())
            .filter(|op| op.opcode == OpCode::CheckedMul)
            .count();
        assert_eq!(checked_mul, 2, "both phi updates become CheckedMul");

        let plain_mul: usize = func
            .blocks
            .values()
            .flat_map(|b| b.ops.iter())
            .filter(|op| op.opcode == OpCode::Mul)
            .count();
        assert_eq!(plain_mul, 2, "the slow clone keeps the plain Muls");
    }

    #[test]
    fn exception_handler_function_refuses() {
        let mut func = live_shape_function();
        // Inject a TryStart anywhere — has_exception_handlers() turns on.
        func.blocks
            .get_mut(&func.entry_block)
            .unwrap()
            .ops
            .push(TirOp {
                dialect: Dialect::Molt,
                opcode: OpCode::TryStart,
                operands: vec![],
                results: vec![],
                attrs: AttrDict::new(),
                source_span: None,
            });
        let blocks_before = func.blocks.len();
        let stats = run_peel(&mut func);
        assert_eq!(stats.ops_added, 0);
        assert_eq!(func.blocks.len(), blocks_before);
    }

    #[test]
    fn impure_body_refuses() {
        let mut func = live_shape_function();
        // A Call in the body breaks re-execution safety.
        let dead = func.fresh_value();
        for block in func.blocks.values_mut() {
            if matches!(&block.terminator, Terminator::Branch { target, .. }
                if func.loop_roles.get(target) == Some(&LoopRole::LoopHeader))
                && block.ops.iter().any(|o| o.opcode == OpCode::Add)
            {
                block.ops.push(TirOp {
                    dialect: Dialect::Molt,
                    opcode: OpCode::Call,
                    operands: vec![],
                    results: vec![dead],
                    attrs: AttrDict::new(),
                    source_span: None,
                });
            }
        }
        let stats = run_peel(&mut func);
        assert_eq!(stats.ops_added, 0, "a Call in the body must refuse");
    }

    #[test]
    fn non_const_init_refuses() {
        let mut func = live_shape_function();
        // Seed the accumulator from the function parameter (unproven).
        if let Some(entry) = func.blocks.get_mut(&func.entry_block)
            && let Terminator::Branch { args, .. } = &mut entry.terminator
        {
            args[0] = ValueId(0); // the DynBox param
        }
        func.value_types.insert(ValueId(0), TirType::I64);
        let stats = run_peel(&mut func);
        assert_eq!(stats.ops_added, 0, "param-seeded accumulator must refuse");
    }

    /// A product accumulator (`t = t * i`) peels exactly like an add
    /// accumulator: the fast body update swaps to `CheckedMul` (the
    /// hardware-overflow-flagged multiply) and the boxed slow clone keeps the
    /// plain `Mul` (BigInt-exact on re-execution). The co-resident IV update
    /// stays `Add`/`CheckedAdd`, proving the swap is keyed per-op on the
    /// recorded update opcode, not globally.
    #[test]
    fn mul_accumulator_peels_to_checked_mul() {
        let mut func = live_shape_function();
        // Convert the first accumulator's update `t = t + i` to `t = t * i`.
        // The fixture's body is a single linear block latching to the header;
        // its first `Add` (t_in + i_in -> t_sum) is the accumulator update.
        let header = func
            .loop_roles
            .iter()
            .find(|(_, r)| **r == LoopRole::LoopHeader)
            .map(|(b, _)| *b)
            .unwrap();
        let body = match &func.blocks[&func.loop_cond_blocks[&header]].terminator {
            Terminator::CondBranch { then_block, .. } => *then_block,
            _ => panic!("guard must end in a CondBranch"),
        };
        {
            let body_block = func.blocks.get_mut(&body).expect("body exists");
            let first_add = body_block
                .ops
                .iter_mut()
                .find(|o| o.opcode == OpCode::Add)
                .expect("the accumulator update is an Add");
            first_add.opcode = OpCode::Mul;
        }

        let blocks_before = func.blocks.len();
        let stats = run_peel(&mut func);
        assert!(stats.ops_added > 0, "the product accumulator must peel");
        assert_eq!(func.blocks.len(), blocks_before + 5);
        verify_function(&func).expect("peeled function must verify");

        let count = |opcode: OpCode| -> usize {
            func.blocks
                .values()
                .flat_map(|b| b.ops.iter())
                .filter(|op| op.opcode == opcode)
                .count()
        };
        // Fast loop: one CheckedMul (the product update) + one CheckedAdd (the
        // IV update). Each carries 2 results (wrapping value + overflow flag).
        assert_eq!(count(OpCode::CheckedMul), 1, "product update -> CheckedMul");
        assert_eq!(count(OpCode::CheckedAdd), 1, "IV update -> CheckedAdd");
        for op in func.blocks.values().flat_map(|b| b.ops.iter()) {
            if matches!(op.opcode, OpCode::CheckedMul | OpCode::CheckedAdd) {
                assert_eq!(op.results.len(), 2, "checked op must have 2 results");
            }
        }
        // Slow clone: the plain Mul + plain Add survive, BigInt-exact.
        assert_eq!(count(OpCode::Mul), 1, "slow clone keeps the plain Mul");
        assert_eq!(count(OpCode::Add), 1, "slow clone keeps the plain Add");
    }
}
