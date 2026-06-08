//! RC drop insertion (RC drop-insertion substrate, design 20, Phase 3).
//!
//! Inserts `DecRef` ops at every owned value's last use and `IncRef` ops before
//! suspension points for values that survive across a yield. This is the
//! compiler pass that closes molt's whole-program expression-temporary leak: the
//! runtime allocates every heap result with `ref_count = 1` and (before this
//! pass) never decremented it for expression temporaries.
//!
//! Runs `Mutates::Cfg`: it inserts `DecRef`/`IncRef` ops within blocks and MAY
//! SPLIT a critical edge (a fresh block carrying an edge-exact `IncRef`) for the
//! mixed-ownership-phi retain (§5 below). `DecRef`/`IncRef` carry no exception
//! edge, and the edge-split inserts only an unconditional `Branch` — but because
//! the block set/edges CAN change, the pass declares `Cfg` so the manager
//! recomputes CFG-sensitive analyses for the following `refcount_elim_post`.
//!
//! ## Ownership transfer at phi (block-arg) boundaries — the two-sided contract
//!
//! TIR uses MLIR block args as phis: a predecessor's terminator passes a value
//! that binds the successor's block arg on entry. A droppable (heap, function-
//! owned) block arg is treated as carrying ONE owned `+1`; the pass drops it
//! where it dies and TRANSFERS it (no drop) where it is forwarded. Soundness
//! requires BOTH halves of the transfer to be exact — the two over-release
//! classes the round-2/round-3 review exposed:
//!
//! * **Incoming side (§5, the `before_term_incref` / edge-split retain).** Every
//!   incoming edge of an owned phi must deliver an owned `+1`. An edge delivering
//!   a BORROWED value (a transparent alias of a `+0` parameter, or an owned value
//!   whose single `+1` is needed elsewhere too) is RETAINED on that edge. Without
//!   it, the phi's drop releases the caller's borrow → UAF (the loop-accumulator
//!   `x = base; while …: x = x + base` and the if-arm `x = a if c else …`).
//! * **Outgoing side (§3 `incoming_arg_roots` exclusion).** A value PASSED as a
//!   branch arg into a phi must NOT also be edge-dropped at the join: its
//!   ownership moved into the block arg, which is released by the phi's own
//!   last-use drop. Liveness reports the forwarded value dead-in to the join (its
//!   successor-side identity is the distinct block-arg SSA value), so the
//!   edge-dying rule would otherwise drop it there AND at the phi's last use →
//!   double-free (the inliner-produced multi-block `x = a + a; return x + a`
//!   chain — `invalid object header before dec_ref`).
//!
//! ## Ownership model (design 20 §1)
//!
//! Every op that returns a new heap reference returns it **owned** (`rc += 1`):
//! the current SSA holder is responsible for exactly one dec-ref before the value
//! goes out of scope. Operands are **borrowed** (the callee never decrefs its
//! args). So the drop rule is: at a value's last use, the holder releases its
//! ref — unless the last use itself transfers ownership (a Return value, a branch
//! arg passed to a successor block arg, or an operand the value-range / repr
//! filter proved carries no heap reference).
//!
//! ## What is dropped
//!
//! A value `v` is a drop candidate when ALL hold:
//! * `v` is heap-carrying (NOT a [`TirLivenessResult::is_raw_scalar`] — raw i64 /
//!   bool / float carriers hold no refcount; dropping them would pass a raw
//!   register to `molt_dec_ref_obj`).
//! * `v` is not produced by `StackAlloc` / `ObjectNewBoundStack` (stack lifetime,
//!   no RC — design R6).
//! * `v` is not a function parameter (parameters are borrowed from the caller per
//!   the ABI; the caller owns and drops them).
//!
//! ## Placement (design 20 §2.4–§2.7)
//!
//! * **Straight-line**: after the last op in a block that uses `v`, if `v` is not
//!   live-out of the block, insert `DecRef(v)` right after that op — UNLESS the
//!   last use is a borrow-into-call (see borrow inference below).
//! * **Edge-dying at successor entry** (§2.5, the OpsOnly form): if `v` is
//!   live-out of a predecessor but dead on entry to a particular successor (and
//!   not passed as that edge's block arg), insert `DecRef(v)` at the *start* of
//!   that successor. This avoids edge-splitting (a CFG mutation); the elim pass
//!   hoists the common case. Done by: for each block `B`, for each value live-in
//!   to `B`'s predecessors but dead in `B`, drop at `B`'s entry.
//! * **Loop-carried** (§2.7): a back-edge that passes a NEW value to a header
//!   block arg leaves the PREVIOUS iteration's value dead. The previous value is
//!   the header block arg itself (the phi); if it is not used after the point the
//!   new value is computed, drop it before the back-edge branch. This is the
//!   "consumer releases the slot" rule (CPython's `STORE_FAST`-on-overwrite).
//! * **Exception edges** (§2.6): `CheckException` successors are ordinary CFG
//!   successors here; a value live at the throw point but dead on a handler path
//!   is dropped at the handler's entry by the edge-dying rule.
//!
//! ## Suspension points (design 20 §2.9)
//!
//! For each `StateYield` / `ChanSendYield` / `ChanRecvYield` / `Yield` /
//! `YieldFrom`, every heap-carrying value live ACROSS the yield (live-out of the
//! block at the yield, used after a resume) is `IncRef`'d immediately before the
//! yield: the suspended coroutine frame now owns its own reference, which the
//! frame finalizer releases on teardown.
//!
//! ## Borrow inference (design 20 §3.2)
//!
//! If `v`'s last use is as an operand to a `Call` / `CallMethod` / `CallBuiltin`
//! and `v` is dead after the call, the callee borrows `v` for the call's
//! duration and the caller drops at last use — which is exactly the call site.
//! Inserting `DecRef(v)` right after the call is correct and is what the
//! straight-line rule does; there is no separate IncRef to elide here (molt's ABI
//! is borrow-args, so no IncRef was ever needed around the call). The borrow
//! inference therefore reduces to: drop after the call, never before — which the
//! last-use placement already does. We keep the call operands out of any
//! *pre-call* drop, which the last-use semantics guarantee.
//!
//! ## Soundness invariants (the over-release hazards this pass must avoid)
//!
//! All ownership reasoning is done over transparent-alias ROOTS (see
//! [`crate::tir::passes::alias_analysis`]). A `Copy` / `TypeGuard` identity move
//! produces a second SSA handle to the SAME owned reference (design §1.2), NOT a
//! new allocation; treating it as a consuming use would double-free. Five
//! soundness rails, each FAIL-CLOSED (keep the +1 / leak rather than risk a UAF):
//!
//! 1. **Alias-root ownership** — a whole alias group is ONE reference, dropped
//!    once at the group's last in-block *touch*, through a live alias of the
//!    root. The drop point dominates every in-block read of the group, so a
//!    later alias-move can never read a freed object. A `Copy` whose
//!    `_original_kind` is neither a proven fresh-owned producer nor an explicit
//!    no-heap move (`non_owning_copy_results`) is its OWN alias root in the
//!    union-find but is a no-incref bit-passthrough of operand 0, so it is
//!    excluded from droppability — releasing it would double-free operand 0.
//! 2. **TerminatorOnly dominance** — an edge-dying drop at a successor `B` is
//!    placed only when the value's def-block dominates `B` in the
//!    **terminator-only** CFG (the view codegen enforces). The *full*-CFG
//!    dominator would admit a value defined mid-block after a `CheckException`
//!    as "dominating" that op's handler, but the exception edge leaves before
//!    the def → use-before-def in codegen. (Observed otherwise as the LLVM
//!    verifier "Instruction does not dominate all uses!" abort.)
//! 3. **Conditionally-valid iterator results** — an `IterNextUnboxed` value
//!    result (`iter_cond_value_results`) is valid ONLY on the not-done branch;
//!    its slot is uninitialized garbage on the exhaustion edge. It is NEVER
//!    edge-dropped (and never IncRef'd onto a phi edge); the body straight-line
//!    rule releases it on the valid path.
//! 4. **State-machine gate** — the pass bails entirely on functions with
//!    generator/async `StateSwitch` / `StateTransition` / `StateYield` control
//!    flow (a `_poll` dispatcher re-enters `state_resume_*` blocks carrying none
//!    of the normal-flow values), in addition to `try`/`except` regions, and is
//!    idempotent (skips a function already carrying the `drop_inserted` attr —
//!    the native re-lift / module-slot re-run path). Re-enabling state-machine
//!    functions needs StateSwitch-aware liveness (design 20 follow-up).
//! 5. **Backend conditioning** — drop insertion is wired into the shared
//!    pipeline but only *runs* for backends that consume the ops by SSA-value
//!    identity with no competing automatic temp-RC (LLVM / WASM / Luau). The
//!    native Cranelift backend keeps its existing value-tracking RC substrate
//!    and is gated OFF until that substrate is retired (design §5 Phase 5; the
//!    `drop_inserted`-marker suppression that makes native's substrate inert for
//!    drop-inserted functions has landed, so native activation is now a pipeline
//!    flip pending the convergence-sweep clearance). See
//!    `pass_manager::target_uses_tir_drop_insertion`.
//!
//! ## Diagnostics
//!
//! `MOLT_DEBUG_DROP=<substr>` (or `=ALL`) writes a per-function dump of the
//! post-insertion block/op shape with per-operand repr tags to
//! `<artifact_root>/drop/<func>.txt`, including a `BAILED:` line for functions
//! the activation gate skipped. The instrument every optimization lands with.

use std::collections::{HashMap, HashSet};

use crate::tir::analysis::AnalysisManager;
use crate::tir::blocks::{BlockId, Terminator};
use crate::tir::function::TirFunction;
use crate::tir::ops::{AttrDict, AttrValue, Dialect, OpCode, TirOp};
use crate::tir::passes::liveness::{TirLiveness, TirLivenessResult};
use crate::tir::values::ValueId;

use super::PassStats;

/// The function-level attr the pass sets (round-tripped to the native backend as
/// a marker op) so the SimpleIR `loop_reassign_old_val` ad-hoc dec-ref path is
/// disabled for drop-inserted functions — preventing the R1 double-drop.
pub const DROP_INSERTED_ATTR: &str = "drop_inserted";

fn make_op(opcode: OpCode, operands: Vec<ValueId>) -> TirOp {
    TirOp {
        dialect: Dialect::Molt,
        opcode,
        operands,
        results: vec![],
        attrs: AttrDict::new(),
        source_span: None,
    }
}

/// True if `opcode` is a suspension point that escapes live values into a
/// coroutine frame (design §2.9).
fn is_suspension_point(opcode: OpCode) -> bool {
    matches!(
        opcode,
        OpCode::StateYield
            | OpCode::ChanSendYield
            | OpCode::ChanRecvYield
            | OpCode::Yield
            | OpCode::YieldFrom
    )
}

/// True if `opcode` produces a stack-allocated value with no RC (design R6).
fn produces_stack_value(opcode: OpCode) -> bool {
    matches!(opcode, OpCode::StackAlloc | OpCode::ObjectNewBoundStack)
}

/// The values `term` passes as block args to ANY successor (these transfer
/// ownership through the SSA phi — they are NOT dropped on that edge).
fn terminator_branch_args(term: &Terminator) -> HashSet<ValueId> {
    let mut out = HashSet::new();
    match term {
        Terminator::Branch { args, .. } => out.extend(args.iter().copied()),
        Terminator::CondBranch {
            then_args,
            else_args,
            ..
        } => {
            out.extend(then_args.iter().copied());
            out.extend(else_args.iter().copied());
        }
        Terminator::Switch {
            cases,
            default_args,
            ..
        } => {
            for (_, _, args) in cases {
                out.extend(args.iter().copied());
            }
            out.extend(default_args.iter().copied());
        }
        Terminator::Return { .. } | Terminator::Unreachable => {}
    }
    out
}

/// The successor blocks of `term` (de-dup not required; callers union into sets).
fn terminator_successor_blocks(term: &Terminator) -> Vec<BlockId> {
    match term {
        Terminator::Branch { target, .. } => vec![*target],
        Terminator::CondBranch {
            then_block,
            else_block,
            ..
        } => vec![*then_block, *else_block],
        Terminator::Switch {
            cases, default, ..
        } => {
            let mut out: Vec<BlockId> = cases.iter().map(|(_, b, _)| *b).collect();
            out.push(*default);
            out
        }
        Terminator::Return { .. } | Terminator::Unreachable => vec![],
    }
}

/// A stable identifier for ONE outgoing arc of a terminator, so the mixed-
/// ownership-phi retain can retarget exactly that arc when splitting a critical
/// edge (two arcs to the same block with different args must be distinguishable).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ArcDescriptor {
    /// The single arc of an unconditional `Branch`.
    Branch,
    /// The `then` arc of a `CondBranch`.
    CondThen,
    /// The `else` arc of a `CondBranch`.
    CondElse,
    /// The case arc at `cases[index]` of a `Switch`.
    SwitchCase(usize),
    /// The `default` arc of a `Switch`.
    SwitchDefault,
}

/// One outgoing arc of a block's terminator: which target it goes to, the args it
/// forwards, and a [`ArcDescriptor`] that pins it for retargeting.
struct Arc {
    descriptor: ArcDescriptor,
    target: BlockId,
    args: Vec<ValueId>,
}

impl Arc {
    /// A self-loop arc whose source block is also its target (the latch IS the
    /// header) — treated as ambiguous for IncRef placement, since a
    /// before-terminator IncRef on such an arc would sit on the in-block
    /// straight-line path that the body's drops also traverse. Splitting isolates
    /// the retain onto the edge. `pred` is the block the arc originates from.
    fn is_self_loop_into_own_phi(&self, pred: BlockId) -> bool {
        self.target == pred
    }
}

/// Enumerate every outgoing arc of `term` with its forwarding args and descriptor.
fn terminator_arcs(term: &Terminator) -> Vec<Arc> {
    match term {
        Terminator::Branch { target, args } => vec![Arc {
            descriptor: ArcDescriptor::Branch,
            target: *target,
            args: args.clone(),
        }],
        Terminator::CondBranch {
            then_block,
            then_args,
            else_block,
            else_args,
            ..
        } => vec![
            Arc {
                descriptor: ArcDescriptor::CondThen,
                target: *then_block,
                args: then_args.clone(),
            },
            Arc {
                descriptor: ArcDescriptor::CondElse,
                target: *else_block,
                args: else_args.clone(),
            },
        ],
        Terminator::Switch {
            cases,
            default,
            default_args,
            ..
        } => {
            let mut out: Vec<Arc> = cases
                .iter()
                .enumerate()
                .map(|(i, (_, b, args))| Arc {
                    descriptor: ArcDescriptor::SwitchCase(i),
                    target: *b,
                    args: args.clone(),
                })
                .collect();
            out.push(Arc {
                descriptor: ArcDescriptor::SwitchDefault,
                target: *default,
                args: default_args.clone(),
            });
            out
        }
        Terminator::Return { .. } | Terminator::Unreachable => vec![],
    }
}

/// Retarget exactly the arc named by `desc` to `new_target`, and CLEAR that arc's
/// forwarded args (the inserted edge-split block now supplies them via its own
/// `Branch`). Used to splice a critical-edge-split block onto one arc.
fn retarget_arc(term: &mut Terminator, desc: &ArcDescriptor, new_target: BlockId) {
    match (term, desc) {
        (Terminator::Branch { target, args }, ArcDescriptor::Branch) => {
            *target = new_target;
            args.clear();
        }
        (
            Terminator::CondBranch {
                then_block,
                then_args,
                ..
            },
            ArcDescriptor::CondThen,
        ) => {
            *then_block = new_target;
            then_args.clear();
        }
        (
            Terminator::CondBranch {
                else_block,
                else_args,
                ..
            },
            ArcDescriptor::CondElse,
        ) => {
            *else_block = new_target;
            else_args.clear();
        }
        (Terminator::Switch { cases, .. }, ArcDescriptor::SwitchCase(i)) => {
            if let Some((_, b, args)) = cases.get_mut(*i) {
                *b = new_target;
                args.clear();
            }
        }
        (
            Terminator::Switch {
                default,
                default_args,
                ..
            },
            ArcDescriptor::SwitchDefault,
        ) => {
            *default = new_target;
            default_args.clear();
        }
        // Descriptor/terminator mismatch is a logic error — the descriptor was
        // produced from THIS terminator by `terminator_arcs` and the terminator is
        // not mutated between enumeration and retarget. Leave unchanged (fail-
        // closed: a missed retarget keeps the original edge — the IncRef block is
        // then unreachable/dead, a leak at worst, never a UAF).
        _ => {}
    }
}

/// A critical-edge split to materialize: insert a fresh block holding `retains`
/// IncRefs + a `Branch(target, args)`, and retarget `pred`'s `arc` to it.
struct EdgeSplit {
    pred: BlockId,
    arc: ArcDescriptor,
    target: BlockId,
    args: Vec<ValueId>,
    retains: Vec<ValueId>,
}

/// Run drop insertion. See module docs for the algorithm.
pub fn run(func: &mut TirFunction, am: &mut AnalysisManager) -> PassStats {
    let mut stats = PassStats {
        name: "drop_insertion",
        ..Default::default()
    };

    // Conservative activation gate. Drop placement keys on single-entry
    // dominance (per-block last-use, edge-dying at successor entry), so it is
    // UNSOUND over any CFG that is not dominator-structured. Two such shapes are
    // bailed:
    //
    //  1. Real exception-HANDLER regions (`try`/`except` → `TryStart`/`TryEnd`,
    //     or a `StateBlockStart`/`StateBlockEnd`-delimited region) —
    //     `has_exception_handlers()`. (A bare universal `CheckException` is NOT a
    //     handler — it propagates to the function exception EXIT — and is fully
    //     handled as an ordinary CFG successor.)
    //
    //  2. A lowered coroutine `_poll` STATE MACHINE (`StateSwitch` dispatch +
    //     `StateTransition`/`StateYield`/`AllocTask`) — `has_state_machine()`.
    //     The state dispatch RE-ENTERS resume blocks, so a value defined in one
    //     state region reaches a resume block the dominator walk does NOT see as
    //     dominated; a drop placed there is a use-before-def (the LLVM verifier
    //     rejects it: `dec_ref %v` before `%v = ...`; on native it double-frees).
    //     Design §2.9's frame-finalizer model handles the high-level SUSPENSION,
    //     but NOT this post-lowering re-entrant CFG. A generator can be lowered to
    //     a `_poll` body carrying `StateSwitch` WITHOUT the `StateBlock*`
    //     delimiters, so predicate (1) alone misses it — hence the dedicated
    //     `has_state_machine()` check. Re-enabling drops for these is the
    //     follow-up that needs StateSwitch-aware (def-reaching) liveness.
    //
    // Idempotency: a function may be re-lifted (the native module path re-lifts
    // `ir.functions` → TIR for the inliner) and re-run through this pipeline (the
    // module-slot-promotion path re-runs `run_pipeline` on promoted functions).
    // The `lower_from_simple` round-trip preserves the `drop_inserted` attr, and
    // the DecRef/IncRef ops survive the re-lift as real ops — so re-running the
    // pass would DOUBLE-insert drops (a refcount underflow / use-after-free).
    // Skip a function whose RC is already TIR-managed; its drops are already in
    // place.
    let debug_this = std::env::var("MOLT_DEBUG_DROP")
        .map(|p| p == "ALL" || func.name.contains(&p))
        .unwrap_or(false);
    if matches!(
        func.attrs.get(DROP_INSERTED_ATTR),
        Some(AttrValue::Bool(true))
    ) {
        return stats;
    }
    if func.has_exception_handlers() || func.has_state_machine() {
        if debug_this {
            let _ = crate::debug_artifacts::write_debug_artifact(
                format!("drop/{}.txt", func.name),
                format!(
                    "[DROP] {} BAILED: exc_handlers={} state_machine={}\n",
                    func.name,
                    func.has_exception_handlers(),
                    func.has_state_machine()
                ),
            );
        }
        return stats;
    }

    let live: TirLivenessResult = am.get::<TirLiveness>(func).clone();

    // Parameters are borrowed from the caller (ABI): never dropped here.
    let param_ids: HashSet<ValueId> = {
        let mut s = HashSet::new();
        if let Some(entry) = func.blocks.get(&func.entry_block) {
            for arg in &entry.args {
                s.insert(arg.id);
            }
        }
        s
    };

    // Stack-allocated values: never dropped (design R6).
    let mut stack_values: HashSet<ValueId> = HashSet::new();
    // Conditionally-valid iterator value results (design §2.8). The VALUE result
    // of an `IterNextUnboxed` (results[0]) holds a valid owned reference ONLY on
    // the not-done branch: `molt_iter_next_unboxed` writes the value-out slot
    // exclusively when it returns `done=false`, and leaves that slot UNINITIALIZED
    // (stale stack garbage) on the `done=true` exhaustion path (verified in the
    // runtime — every `return done_true` skips `*value_out = …`). Such a value
    // must NEVER be dropped on a die-EDGE: the edge-dying rule (§3) would place a
    // `DecRef` of it at the exhaustion successor's entry, where it is a stale
    // pointer → use-after-free / segfault (the adversarial-review P0 #2(b): the
    // yielded element `9` double-/garbage-dropped on the loop-exit path of a
    // `list(gen)` / `"".join(gen)` / `for v in gen:` consumer). On the not-done
    // (body) path the value is valid and is released by the ordinary straight-line
    // last-use rule, which only ever runs in body blocks the done-edge can't reach.
    let mut iter_cond_value_results: HashSet<ValueId> = HashSet::new();
    // Non-owning, NON-aliased `Copy` results (the over-release-class keystone).
    // An `OpCode::Copy` result falls into exactly one of three drop classes:
    //   1. FreshValue (`alias_analysis::copy_kind_mints_fresh_owned_ref` — `slice`,
    //      `int_from_obj`, container constructors, `iter`, …): a brand-new `+1`.
    //      DROP it independently. (Stays droppable.)
    //   2. EXPLICIT transparent alias (`copy`/`copy_var`/`guard_tag`/…,
    //      `copy_kind_is_explicit_no_heap_move`): its result is operand 0's
    //      reference. The alias union-find folds it into operand 0's group (so its
    //      ROOT is operand 0, never itself) and the group is released ONCE through
    //      the root — `droppable`'s `r == v` rail already declines the non-root
    //      member, so it is naturally non-droppable here.
    //   3. EVERYTHING ELSE (an UNKNOWN / unmapped value-producing carrier with its
    //      OWN alias root — `copy_is_known_local_alias` is FALSE so the union-find
    //      does NOT fold it — or an inert marker): the backend lowers it as a
    //      no-incref bit-passthrough of operand 0 (returns operand 0's bits without
    //      a `+1`). Because the union-find leaves it as its own root, `droppable`'s
    //      `r == v` rail would (wrongly) admit it → dropping its result is a
    //      DOUBLE-FREE of operand 0's object. EXCLUDE it explicitly.
    // FAIL-CLOSED: class 3 is excluded from droppability (leak at worst — the
    // sanctioned direction, never a UAF). This is the precise drop-side half of the
    // lowering-truth alias contract that `alias_analysis::copy_is_known_local_alias`
    // mandates (its docstring: "the drop pass separately fails closed to 'do NOT
    // release'"); the MemGVN-precise union-find half lives in `alias_analysis`.
    // Class 3 is exactly "not FreshValue AND not an explicit alias move".
    let mut non_owning_copy_results: HashSet<ValueId> = HashSet::new();
    for block in func.blocks.values() {
        for op in &block.ops {
            if produces_stack_value(op.opcode) {
                for &r in &op.results {
                    stack_values.insert(r);
                }
            }
            if op.opcode == OpCode::IterNextUnboxed
                && let Some(&value_result) = op.results.first()
            {
                iter_cond_value_results.insert(value_result);
            }
            if op.opcode == OpCode::Copy {
                let kind = match op.attrs.get("_original_kind") {
                    Some(AttrValue::Str(s)) => Some(s.as_str()),
                    _ => None,
                };
                let mints_fresh = kind
                    .map(crate::tir::passes::alias_analysis::copy_kind_mints_fresh_owned_ref)
                    .unwrap_or(false);
                let explicit_alias =
                    crate::tir::passes::alias_analysis::copy_kind_is_explicit_no_heap_move(kind);
                if !mints_fresh && !explicit_alias {
                    for &r in &op.results {
                        non_owning_copy_results.insert(r);
                    }
                }
            }
        }
    }

    // Alias-root canonicalization (design 20 §1.2 — `Copy`/`TypeGuard` are
    // borrowed aliases, holding NO new reference). Ownership — and therefore the
    // drop obligation — is per alias ROOT, not per SSA value. The drop pass
    // operates entirely in root space: every value reference is canonicalized to
    // its root, and we drop each root EXACTLY ONCE (at the last use of any chain
    // member). Dropping each `Copy` independently is a refcount underflow /
    // use-after-free (the loop-carried accumulator loads its phi via
    // `load_var`→`Copy` every iteration; a per-copy drop double-frees the live
    // accumulator). This is the SAME union-find the liveness analysis used, so the
    // live sets (in root space) line up with these canonicalized placements.
    let aliases = crate::tir::passes::alias_analysis::build_alias_union_find(func);
    let canon = |v: ValueId| -> ValueId { aliases.root(v) };

    // Interior-borrow keepalive (design 20). A value produced by a borrowing read
    // (`LoadAttr`/`Index`) may borrow into / index its SOURCE object's backing
    // store; using such a result keeps the source object live. This is the SAME
    // relation the liveness analysis consumes (so cross-block keepalive is already
    // reflected in `live.is_live_out`), applied here ALSO to the within-block
    // straight-line `last_use` scan: a source object's last in-block "touch" must
    // extend through the last use of any borrow result derived from it, or the drop
    // would land before the consumer reads the borrow. (The round-6 BLOCKER-1 UAF:
    // `Counter._handle` is a raw-int registry handle whose owning wrapper's
    // finalizer destroys the registry entry — dropping the wrapper after the
    // `get_attr` but before `molt_counter_len(handle)` made `len(Counter(...))`
    // return 0.) FAIL-CLOSED: for an owned-result load this only defers the drop a
    // few ops (harmless); for the borrow/handle case it is required for soundness.
    let borrows = crate::tir::passes::alias_analysis::build_borrow_provenance(func, &aliases);

    // Root-space params / stack sets.
    let param_roots: HashSet<ValueId> = param_ids.iter().map(|&v| canon(v)).collect();
    let stack_roots: HashSet<ValueId> = stack_values.iter().map(|&v| canon(v)).collect();

    // A root is droppable iff it is heap-carrying, not a (root of a) param, not a
    // (root of a) stack value, AND it is its own alias root (a non-root alias is a
    // borrow — the root carries the single ownership obligation).
    // `is_raw_scalar` covers the repr filter — RawI64Safe/Bool/Float — and is
    // tested on the root carrier (a copy of a raw i64 is raw).
    // Class-3 (non-owning, unmapped) `Copy` results are their OWN alias root (the
    // union-find declines to fold them — see `non_owning_copy_results`), so the
    // `r == v` rail alone would admit them; exclude them explicitly. The set is
    // keyed by the SSA result id, which IS the root for a class-3 copy.
    let droppable = |v: ValueId| -> bool {
        let r = canon(v);
        r == v
            && !live.is_raw_scalar(r)
            && !param_roots.contains(&r)
            && !stack_roots.contains(&r)
            && !non_owning_copy_results.contains(&r)
    };

    // The plan: per block, a list of (insert_after_op_index OR at-entry, value)
    // DecRef placements, plus per-block at-entry edge-dying drops, plus
    // suspension IncRefs. We collect first (read-only over `func`), then apply.
    struct BlockPlan {
        /// DecRef(v) to insert immediately AFTER op at this index (straight-line
        /// last-use). Keyed by op index → values dropped after it.
        after_op: HashMap<usize, Vec<ValueId>>,
        /// DecRef(v) to insert at the START of the block (edge-dying values that
        /// arrive live from a predecessor but die on entry here).
        at_entry: Vec<ValueId>,
        /// DecRef(v) to insert just BEFORE the terminator (loop-carried phi whose
        /// last live use is the back-edge / values live-in but dead before exit).
        before_term: Vec<ValueId>,
        /// IncRef(v) to insert immediately BEFORE the op at this index (a
        /// suspension point). Keyed by op index → values inc-ref'd before it.
        before_op: HashMap<usize, Vec<ValueId>>,
        /// IncRef(v) to insert just BEFORE the terminator (the mixed-ownership-phi
        /// retain, design §ownership / §5): a BORROWED value `v` this block passes
        /// as a branch arg into a successor's OWNED block-arg (phi) must be retained
        /// on the edge so the phi is uniformly owned and the downstream drop
        /// releases a real `+1` rather than the caller's borrow. Placed before the
        /// terminator only when this block reaches the successor via a single,
        /// unambiguous arc (the common preheader / if-arm shape); the ambiguous
        /// multi-arc-same-target case is handled by an edge split instead.
        before_term_incref: Vec<ValueId>,
    }
    let mut plans: HashMap<BlockId, BlockPlan> = HashMap::new();

    // Predecessor map (terminator-only edges) for edge-dying placement.
    let pred_map = crate::tir::dominators::build_pred_map_with(
        func,
        crate::tir::dominators::CfgEdgePolicy::Full,
    );

    let block_ids: Vec<BlockId> = {
        let mut v: Vec<BlockId> = func.blocks.keys().copied().collect();
        v.sort_unstable_by_key(|b| b.0);
        v
    };
    let reachable = crate::tir::dominators::reachable_blocks_with(
        func,
        crate::tir::dominators::CfgEdgePolicy::Full,
    );

    for &bid in &block_ids {
        if !reachable.contains(&bid) {
            continue;
        }
        let block = &func.blocks[&bid];
        let mut plan = BlockPlan {
            after_op: HashMap::new(),
            at_entry: Vec::new(),
            before_term: Vec::new(),
            before_op: HashMap::new(),
            before_term_incref: Vec::new(),
        };

        // ── 1. Straight-line last-use drops (alias-root space) ───────────────
        // For every alias ROOT used by an op in this block, find the LAST op
        // index where any chain member is used as an operand. If the root is
        // droppable AND not live-out of this block AND not transferred by a
        // branch arg / terminator use (which pass ownership), drop the ROOT after
        // its last op-use. Canonicalizing collapses a `Copy`-chain into one
        // entity → one drop per owned object (no double-free across copies).
        //
        // Branch args / terminator direct uses are canonicalized to roots: a
        // copied value passed on an edge transfers the ROOT's ownership.
        let branch_arg_roots: HashSet<ValueId> = terminator_branch_args(&block.terminator)
            .into_iter()
            .map(canon)
            .collect();
        // Last op-use index per ROOT (max over all aliases). A use of operand `v`
        // at index `idx` is a last-use candidate for `canon(v)` AND for every
        // source-object root `v` borrows from (interior-borrow keepalive): the
        // source must stay live through the borrow result's last use.
        let mut last_use: HashMap<ValueId, usize> = HashMap::new();
        let record_use = |root: ValueId, idx: usize, lu: &mut HashMap<ValueId, usize>| {
            lu.entry(root)
                .and_modify(|e| {
                    if idx > *e {
                        *e = idx;
                    }
                })
                .or_insert(idx);
        };
        for (idx, op) in block.ops.iter().enumerate() {
            for &operand in &op.operands {
                record_use(canon(operand), idx, &mut last_use);
                if !borrows.is_empty() {
                    for src_root in borrows.keepalive_roots(operand, &canon) {
                        record_use(src_root, idx, &mut last_use);
                    }
                }
            }
        }
        for (&v, &idx) in &last_use {
            // `v` is already a root (last_use is keyed by canon'd operands).
            if !droppable(v) {
                continue;
            }
            // Transferred via branch arg (root space) → no drop (successor owns).
            if branch_arg_roots.contains(&v) {
                continue;
            }
            // Live-out of this block → dropped later; not here.
            if live.is_live_out(bid, v) {
                continue;
            }
            // Consumed by the terminator (Return value / cond) — canonicalize the
            // terminator's direct uses to roots and skip if `v` is among them.
            if terminator_uses_root(&block.terminator, v, &canon) {
                continue;
            }
            // Consumed AS AN OPERAND by its last-use op (design §1.2
            // takes-ownership): a CallArgs builder handed to `call_bind` /
            // `call_indirect` is freed inside the call (PtrDropGuard). Ownership
            // transferred to the op exactly like a Return value — no trailing
            // DecRef, or we double-free the `TYPE_ID_CALLARGS` object.
            if op_consumed_operand_root(&block.ops[idx], &canon) == Some(v) {
                continue;
            }
            // The owned object dies after op `idx` in this block: drop the root
            // after it.
            plan.after_op.entry(idx).or_default().push(v);
        }

        // ── 1b. Dead-result drops (defined-but-never-used owned values) ──────
        // The §1 scan keys drops on `last_use`, which is built EXCLUSIVELY from
        // values that appear as an OPERAND somewhere. An owned result that is
        // produced but NEVER consumed (zero uses — neither as an operand, nor a
        // branch arg, nor a terminator use) is therefore ABSENT from `last_use`
        // and would leak: its `+1` is never released, so for a `TYPE_ID_OBJECT`
        // with a `__del__` the finalizer NEVER runs (CPython runs it at the last
        // reference drop). The canonical example is a discarded constructor whose
        // local is dead or `del`'d: `def f(): x = Demo(); del x` lowers to a
        // `call_bind` whose owned result has no further use. The edge-dying rule
        // (§3) cannot catch it either — that rule requires the value to be
        // live-out of a predecessor, but a zero-use value is dead immediately.
        //
        // For a value with no uses, the LAST program point at which it is live is
        // immediately AFTER its defining op, so that is where its drop belongs.
        // We apply the SAME guards as the §1 last-use path (droppable / not
        // branch-transferred / not live-out / not terminator-consumed) plus the
        // conditionally-valid-iterator exclusion (§2.8): the value result of an
        // `IterNextUnboxed` is uninitialized stack garbage on the exhaustion path
        // and must never be dropped unconditionally. A result that IS used was
        // already handled by §1 (its root is in `last_use`); checking `last_use`
        // membership in ROOT space avoids any double-drop.
        for (idx, op) in block.ops.iter().enumerate() {
            for &result in &op.results {
                let r = canon(result);
                // Only the value's own root carries the ownership obligation; an
                // aliased result (`r != result`) is released through its root.
                if r != result {
                    continue;
                }
                // Already released by the §1 last-use path (some op used it).
                if last_use.contains_key(&r) {
                    continue;
                }
                if !droppable(r) {
                    continue;
                }
                // Conditionally-valid iterator value result: never drop it (it is
                // stale garbage on the iterator-exhaustion path).
                if iter_cond_value_results.contains(&result) {
                    continue;
                }
                // Transferred via branch arg (root space) → successor owns it.
                if branch_arg_roots.contains(&r) {
                    continue;
                }
                // Live-out of this block → dropped later, not here.
                if live.is_live_out(bid, r) {
                    continue;
                }
                // Consumed by the terminator (Return value / cond).
                if terminator_uses_root(&block.terminator, r, &canon) {
                    continue;
                }
                // The owned object is dead the instant it is produced: drop it
                // immediately after its defining op.
                plan.after_op.entry(idx).or_default().push(r);
            }
        }

        // ── 2. Suspension-point IncRef ───────────────────────────────────────
        // For each yield op at index `i`, every heap-carrying value that is
        // (a) DEFINED before the yield (an op result at index < i, or a block
        // arg), AND (b) live ACROSS the yield (live-out of the block — used after
        // a resume) gets an IncRef immediately before the yield so the suspended
        // frame owns its own reference.
        //
        // Requirement (a) is soundness-critical: a value defined AFTER the yield
        // is not yet in scope at the yield, so referencing it in an IncRef placed
        // before the yield would be a use-before-def (a TIR verify failure).
        // Build the set of values defined at or before each op position.
        if block.ops.iter().any(|o| is_suspension_point(o.opcode)) {
            // `live_out` is already in alias-root space (liveness canonicalized).
            let live_out_here: HashSet<ValueId> = live
                .live_out
                .get(&bid)
                .into_iter()
                .flatten()
                .copied()
                .collect();
            // Roots defined at-or-before each op (block args are roots).
            let mut defined: HashSet<ValueId> = block.args.iter().map(|a| canon(a.id)).collect();
            for (idx, op) in block.ops.iter().enumerate() {
                if is_suspension_point(op.opcode) {
                    let mut seen: HashSet<ValueId> = HashSet::new();
                    for &v in &live_out_here {
                        // `v` is a root; IncRef the root if it is droppable and
                        // already defined before the yield.
                        if droppable(v) && defined.contains(&v) && seen.insert(v) {
                            plan.before_op.entry(idx).or_default().push(v);
                        }
                    }
                }
                // The op's results become defined AFTER it executes (in root
                // space — a copy result canonicalizes to an already-defined root).
                for &r in &op.results {
                    defined.insert(canon(r));
                }
            }
        }

        plans.insert(bid, plan);
    }

    // ── 3. Edge-dying drops at successor entry (design §2.5 OpsOnly form) ─────
    // A value V is dropped at the START of block B when:
    //   * V is live-out of at least one predecessor P of B (i.e. P keeps it
    //     alive across the edge), AND
    //   * V is NOT live-in to B (B does not need it), AND
    //   * V is NOT a block arg of B (block args are re-supplied by the edge), AND
    //   * V's defining block DOMINATES B (V is provably available at B's entry —
    //     SSA-dominance soundness; see below), AND
    //   * V is droppable.
    // This releases the value on the path where it dies. Because every path into
    // B that delivered V must release it, and B is a join, dropping once at B's
    // entry is correct ONLY when V dies on ALL incoming paths. We therefore
    // require V to be dead-in to B and live-out of EVERY predecessor that can
    // reach B (so no path still needs it). The elim pass later hoists/dedups.
    //
    // DOMINANCE GUARD (soundness-critical, FAIL-CLOSED). The backward liveness
    // dataflow OVER-APPROXIMATES across the universal `CheckException` edges (C2
    // commit 430e09793): a value can be marked live-out of an exception-edge
    // predecessor whose def-block does NOT terminator-dominate the handler/join
    // block B. A `DecRef(V)` placed at B's entry where V's def does not dominate
    // B is a use-before-def → SSA dominance violation (observed as the LLVM
    // verifier "Instruction does not dominate all uses!" abort on
    // `molt_dec_ref_obj(%isinstance)`).
    //
    // We use the **TerminatorOnly** dominator tree, NOT the Full (analysis) one.
    // This is the SAME view the TIR verifier and the LLVM/native codegen use for
    // SSA dominance (dominators.rs CfgEdgePolicy doc): a handler block reached
    // only via a mid-block exception edge has NO terminator-predecessor, so a
    // value defined in the protected region does NOT terminator-dominate it.
    // The Full tree would (wrongly, for codegen purposes) say a value defined
    // mid-block AFTER a CheckException "dominates" that op's handler — but the
    // exception edge leaves from BEFORE the def, so at the instruction level the
    // def does not dominate the handler. TerminatorOnly dominance matches what
    // codegen enforces, so a guard built on it never admits an
    // exception-path use-before-def.
    //
    // FAIL-CLOSED: if V's def-block does not terminator-dominate B, we DO NOT
    // drop here (keep the +1 / accept a possible leak on that exception path) —
    // the under-release direction. Never over-release (UAF).
    let pred_map_term = crate::tir::dominators::build_pred_map_with(
        func,
        crate::tir::dominators::CfgEdgePolicy::TerminatorOnly,
    );
    let idoms = crate::tir::dominators::compute_idoms_with(
        func,
        &pred_map_term,
        crate::tir::dominators::CfgEdgePolicy::TerminatorOnly,
    );
    let def_block: HashMap<ValueId, BlockId> = {
        let mut m: HashMap<ValueId, BlockId> = HashMap::new();
        for (&bid, block) in &func.blocks {
            for arg in &block.args {
                m.insert(arg.id, bid);
            }
            for op in &block.ops {
                for &r in &op.results {
                    m.insert(r, bid);
                }
            }
        }
        m
    };
    for &bid in &block_ids {
        if !reachable.contains(&bid) {
            continue;
        }
        let preds = match pred_map.get(&bid) {
            Some(p) if !p.is_empty() => p,
            _ => continue,
        };
        let block_args: HashSet<ValueId> =
            func.blocks[&bid].args.iter().map(|a| a.id).collect();
        // Roots that some predecessor passes as a branch ARG into THIS block's
        // phi(s). Such a value transfers its ownership INTO the block arg on the
        // edge — it is NOT dying on entry, even though liveness reports it dead-in
        // to `B` (its successor-side identity is the block arg, a distinct SSA
        // value). Edge-dropping it here would double-free: the block arg (phi) is
        // the owner now and is released by ITS own last-use / loop / exit drop.
        // (This is the dual of the §5 mixed-ownership retain: §5 ensures the
        // transferred value is owned; this ensures the transfer itself is not also
        // released at the join. Without it, an owned value forwarded into a phi
        // through a multi-block chain — the shape the inliner produces for
        // `x = a + a; return x + a` — was dropped BOTH at the join entry AND at the
        // phi's last use → `invalid object header before dec_ref`.) The per-arc
        // `terminator_arcs` enumeration (filtered to `arc.target == bid`) is the
        // precise per-edge form — it is the single arc-enumeration helper §5 also
        // uses, so there is one source of truth for "args forwarded on this edge".
        let incoming_arg_roots: HashSet<ValueId> = {
            let mut s = HashSet::new();
            for p in preds {
                if let Some(pblock) = func.blocks.get(p) {
                    for arc in terminator_arcs(&pblock.terminator) {
                        if arc.target == bid {
                            for &v in &arc.args {
                                s.insert(canon(v));
                            }
                        }
                    }
                }
            }
            s
        };
        // FINDING 3 (round-4) — keying precision of `incoming_arg_roots`. This set
        // is keyed by alias ROOT over ALL predecessors, NOT per (root, edge). A
        // root forwarded into B's phi by SOME predecessor is excluded from B's
        // edge-dying drop on EVERY incoming path. The theoretically-imprecise case:
        // pred P1 forwards root R into B's phi (transfer), while a DIFFERENT pred P2
        // delivers R live-out and R dies on the P2→B edge without being forwarded.
        // The global exclusion would then skip R's legitimate drop on the P2 path.
        //
        // This is FAIL-CLOSED — leak-never-UAF — and the precise form is NOT a
        // localized change. (1) Fail-closed: on the P2 path R merely leaks (its +1
        // is never released); it is NEVER double-freed, because the phi that P1's
        // transfer fed is released exactly once by the phi's own last-use drop, and
        // the excluded R is never dropped at all. The over-release direction (the
        // only UAF risk) cannot occur. (2) Not localized: the edge-dying rule is
        // deliberately the OpsOnly form — it places ONE `DecRef` at B's *entry*,
        // which fires on EVERY incoming path, and relies on `all_preds_deliver` +
        // the elim pass hoisting the common case (see the §2.5 design note above).
        // Dropping R on the P2 edge but not the P1 edge would require SPLITTING the
        // P2 die-edge to host a per-edge drop — abandoning the at-entry form and
        // splitting a potentially large number of die-edges (a CFG explosion the
        // OpsOnly design exists to avoid). Refining the keying without that split is
        // impossible: a single at-entry drop cannot distinguish the path it runs on.
        //
        // Reachability: in molt's SSA construction a phi at B is fed by each
        // predecessor passing ITS version of the variable to the SAME arg position,
        // so R-from-P1 and R-from-P2 are normally DISTINCT SSA values (P2 would pass
        // its own W, not R) → R is not live-out of P2 and the case does not arise.
        // It can only appear if a value defined above both P1 and P2 is forwarded to
        // the phi on one edge AND separately live on the other — a shape the
        // frontend does not emit for plain joins, and which (per the fail-closed
        // analysis) costs at most a leak if a future frontend/inliner shape does.
        // Pinned by `forwarded_into_phi_other_pred_live_is_leak_not_uaf` below.
        let mut candidates: HashSet<ValueId> = HashSet::new();
        for p in preds {
            if let Some(set) = live.live_out.get(p) {
                candidates.extend(set.iter().copied());
            }
        }
        // Root-level live-in to B: any alias member of the root is live-in.
        let root_live_in = |root: ValueId| -> bool {
            live.live_in
                .get(&bid)
                .is_some_and(|set| set.iter().any(|&m| canon(m) == root))
        };
        // Roots already scheduled to drop at this block's entry (dedup by root,
        // not raw value — two aliases of the same group must drop once).
        let mut entry_root_seen: HashSet<ValueId> = HashSet::new();
        for v in candidates {
            if !droppable(v) {
                continue;
            }
            // Conditionally-valid iterator value result (§2.8): NEVER drop it on a
            // die-edge. On the exhaustion edge the value-out slot is uninitialized
            // garbage; a `DecRef` here is a UAF (review P0 #2(b)). On the not-done
            // edge it is consumed by the body's straight-line drop instead. We test
            // the alias ROOT so a transparent copy of the value result is covered
            // too. (An `IterNextUnboxed` value result is never itself a transparent
            // alias of another value — it is a fresh op result — so its root is
            // itself unless a later `Copy` of it widened the group; in that case
            // the whole group is conditionally-valid and equally unsafe to
            // edge-drop.)
            if iter_cond_value_results.contains(&v)
                || iter_cond_value_results
                    .iter()
                    .any(|&iv| canon(iv) == canon(v))
            {
                continue;
            }
            let root = canon(v);
            if block_args.contains(&v) || block_args.iter().any(|&a| canon(a) == root) {
                continue;
            }
            // Transferred-into-phi exclusion: `v` (or an alias) is passed as a
            // branch arg into THIS block's phi by some predecessor → its ownership
            // moves into the block arg, it does not die here. Dropping it would
            // double-free the phi's object.
            if incoming_arg_roots.contains(&root) {
                continue;
            }
            // Dead on entry to B (root-level — no alias member live-in).
            if root_live_in(root) {
                continue;
            }
            // Must die on ALL incoming paths: every predecessor delivers the root
            // group live-out (some alias member live-out of each predecessor), so
            // the single drop here releases it exactly once on every path. A
            // predecessor without the root live-out would mean that path never
            // owned it → a spurious drop on that path.
            let all_preds_deliver = preds.iter().all(|p| {
                live.live_out
                    .get(p)
                    .is_some_and(|s| s.iter().any(|&m| canon(m) == root))
            });
            if !all_preds_deliver {
                continue;
            }
            // DOMINANCE GUARD (fail-closed): V's def-block must dominate B under
            // the TerminatorOnly tree, else V is not provably defined at B's
            // entry and the DecRef would be a use-before-def. Skip (keep the +1).
            match def_block.get(&v) {
                Some(&dblk) if crate::tir::dominators::dominates(dblk, bid, &idoms) => {}
                _ => continue,
            }
            // One drop per root group at this entry.
            if !entry_root_seen.insert(root) {
                continue;
            }
            plans
                .entry(bid)
                .or_insert_with(|| BlockPlan {
                    after_op: HashMap::new(),
                    at_entry: Vec::new(),
                    before_term: Vec::new(),
                    before_op: HashMap::new(),
                    before_term_incref: Vec::new(),
                })
                .at_entry
                .push(v);
        }
    }

    // ── 4. Loop-carried phi drops before the back-edge (design §2.7) ─────────
    // A header block arg (phi) `p` whose back-edge passes a NEW value leaves the
    // previous iteration's `p` dead once the new value is computed. If `p` is
    // live-out of the loop body's latch block ONLY because of the phi-slot (i.e.
    // `p` is not used after the point the new value is produced) we would
    // double-count; the conservative correct rule the straight-line + edge-dying
    // rules already implement is: `p` is dropped at its last use. The loop EXIT
    // case (the final phi value, dead after the loop) is handled by edge-dying at
    // the exit block. No separate action needed here beyond what §1–§3 produce;
    // this block is retained as the documented anchor for the loop-carried case
    // and validated by the loop unit test.

    // ── 5. Mixed-ownership phi retain (design §ownership) ─────────────────────
    // A TIR block argument is the SSA phi: each predecessor edge passes a value
    // that binds the arg on entry. The straight-line / edge-dying / loop-carried
    // rules above treat a DROPPABLE (heap, function-owned) block arg as carrying
    // exactly ONE owned `+1` — they DROP it on the path where it dies and TRANSFER
    // it (no drop) where it is forwarded as a branch arg. That is sound ONLY when
    // EVERY incoming edge actually delivers an owned `+1` into the phi.
    //
    // It is NOT sound when an edge delivers a BORROWED value:
    //   * `x = base` then a loop `while …: x = x + base` — the loop-ENTRY edge
    //     binds the accumulator phi to `Copy(base)`, a transparent alias of the
    //     borrowed parameter `base` (the caller owns it; this function never does).
    //     The loop body then drops the phi every iteration, decrementing `base`'s
    //     refcount below the caller's borrow → premature free → UAF / SIGABRT /
    //     SIGSEGV (the round-2 over-release). The control `x = 0` is immune: the
    //     phi is then raw (inline), not droppable, so no drop is placed at all.
    //   * `x = a if c else fresh()` — the `then` arm binds the merge phi to the
    //     borrowed `a`; a later `x + …` drops the merge phi → the same UAF on the
    //     `c` path.
    //
    // THE FIX (uniform ownership at phi boundaries): when a DROPPABLE block arg
    // (an owned phi) has any incoming edge delivering a BORROWED value, RETAIN
    // (`IncRef`) that value on THAT edge. The phi then uniformly owns a `+1` on
    // every path, so the downstream drop releases a real reference and never the
    // caller's borrow. This composes with molt's `+0` borrowed-parameter ABI: the
    // parameter itself stays borrowed; the RETAINED copy is what flows into the
    // phi. It is also exactly correct for the degenerate shapes — `apply(base, 0)`
    // (loop body never runs) returns `x` which IS `base`, and the entry retain is
    // precisely the `+1` the return ABI must transfer to the caller.
    //
    // CLEAN-TRANSFER (no retain) vs BORROWED (retain). An edge value `v` binding
    // an owned phi delivers a clean owned `+1` iff ALL hold:
    //   (a) `v` is heap-carrying (a raw/inline `v` — e.g. `ConstInt 0` feeding a
    //       boxed phi — carries no refcount: `molt_*_ref_obj` is a runtime no-op on
    //       a non-pointer tag, so such an input is self-balancing and an `IncRef`
    //       on it would be a type error on a raw register; SKIP it, mirroring the
    //       repr filter the whole pass uses);
    //   (b) `v` is `droppable` (function-owned: heap, not a parameter, not stack,
    //       not a non-owning `Copy`) AND its alias `root` is not a parameter; and
    //   (c) THIS branch-arg is the sole downstream owner of `root(v)` — `root(v)`
    //       is not also forwarded to another phi (another arg position / edge) and
    //       is not live into a successor's body. If `root(v)` is consumed elsewhere
    //       too, the function's single `+1` stays with that other consumer and this
    //       edge transfers nothing → it must be retained (e.g. `t = f(); x = t;
    //       while …: x = x + 1; return t` — `t` is owned but BOTH seeds the phi and
    //       is returned, so the phi needs its own `+1`).
    // If (a)–(c) hold → clean transfer, NO retain. Otherwise → RETAIN on the edge.
    // FAIL-CLOSED: any doubt retains (an extra `IncRef` is at worst a leak the gates
    // catch — never a UAF). A blanket "never drop mixed phis" is rejected by spec:
    // it would leak the previous accumulator EVERY iteration (O(n) residual).
    //
    // PLACEMENT must be edge-exact. When this block (`P`) reaches the phi block via
    // a SINGLE arc carrying these args (an unconditional `Branch`, or a
    // `CondBranch`/`Switch` with exactly one arm to that target — the preheader and
    // if-arm shapes molt lowers), the `IncRef` goes just before `P`'s terminator
    // (`before_term_incref`). When `P` reaches the target on MULTIPLE arcs with
    // different args (a critical edge — e.g. a `Switch` routing two cases to one
    // block), a before-terminator `IncRef` would wrongly fire on the other arc; we
    // SPLIT that critical edge (a fresh block holding the `IncRef` + a `Branch`),
    // which is why this pass is `Mutates::Cfg`.
    //
    // Owned phis bail with the function (state-machine / exception-handler gate at
    // the top of `run`), so this never runs over `_poll` / handler CFGs.

    // Per-block: the values that block forwards as branch args, with multiplicity
    // by alias root, plus the set of roots live INTO any successor's body (so we
    // can test clean-transfer condition (c) without re-deriving liveness).
    //
    // A root is "live into a successor body" iff some live-in value of a successor
    // `S` aliases it AND that value is not one of `S`'s own block args (block args
    // are killed at `S`'s entry — they are the phi we may be feeding, not a body
    // use). This is the precise "consumed elsewhere than via a phi we feed" test.
    let succ_body_live_roots: HashMap<BlockId, HashSet<ValueId>> = {
        let mut m: HashMap<BlockId, HashSet<ValueId>> = HashMap::new();
        for &bid in &block_ids {
            if !reachable.contains(&bid) {
                continue;
            }
            let block = &func.blocks[&bid];
            let mut roots: HashSet<ValueId> = HashSet::new();
            for succ in terminator_successor_blocks(&block.terminator) {
                let succ_args: HashSet<ValueId> = func
                    .blocks
                    .get(&succ)
                    .map(|s| s.args.iter().map(|a| a.id).collect())
                    .unwrap_or_default();
                if let Some(set) = live.live_in.get(&succ) {
                    for &m in set {
                        if !succ_args.contains(&m) {
                            roots.insert(canon(m));
                        }
                    }
                }
            }
            m.insert(bid, roots);
        }
        m
    };

    // Critical-edge splits to materialize: (pred, arc_descriptor, retained_values,
    // forwarded_args_for_that_arc, target). Collected here, applied after the op
    // rebuild so block-id allocation doesn't disturb the in-place op insertion.
    let mut edge_splits: Vec<EdgeSplit> = Vec::new();

    for &bid in &block_ids {
        if !reachable.contains(&bid) {
            continue;
        }
        // Only successor blocks WITH owned block-arg phis matter.
        // Examine each outgoing arc of this block's terminator.
        let term = func.blocks[&bid].terminator.clone();
        let arcs = terminator_arcs(&term);
        // Count, per (target, arg-root), how many of THIS block's arcs forward an
        // alias of that root to that target — for the placement ambiguity test and
        // for clean-transfer condition (c) (forwarded to >1 phi).
        // Also count total forwards of each root across ALL arcs of this block.
        let mut root_forward_count: HashMap<ValueId, usize> = HashMap::new();
        for arc in &arcs {
            for &v in &arc.args {
                if !live.is_raw_scalar(v) {
                    *root_forward_count.entry(canon(v)).or_default() += 1;
                }
            }
        }
        for arc in &arcs {
            let Some(succ_block) = func.blocks.get(&arc.target) else {
                continue;
            };
            if succ_block.args.is_empty() {
                continue;
            }
            // How many arcs of THIS block target `arc.target` (placement ambiguity:
            // >1 ⇒ critical edge, must split to place an edge-exact IncRef).
            let arcs_to_target = arcs.iter().filter(|a| a.target == arc.target).count();
            // Compute the retains for THIS arc.
            let mut arc_retains: Vec<ValueId> = Vec::new();
            for (pos, &v) in arc.args.iter().enumerate() {
                let Some(phi) = succ_block.args.get(pos) else {
                    continue;
                };
                let phi_id = phi.id;
                // The phi must be an OWNED obj-lane phi (droppable) for the
                // transfer-ownership assumption to apply. A non-droppable phi
                // (raw/param/stack) is never dropped → no retain obligation.
                if !droppable(phi_id) {
                    continue;
                }
                // (a) raw/inline edge value → self-balancing, cannot RC. Skip.
                if live.is_raw_scalar(v) {
                    continue;
                }
                let root = canon(v);
                // (b) clean transfer requires the value be function-owned with a
                //     non-parameter root. A borrowed value (param-rooted, or a
                //     non-owning copy, or otherwise not droppable) is NOT a clean
                //     transfer → retain.
                //
                // Test droppability on the ROOT, not on `v` directly: in the
                // alias-root model `droppable(x)` is FALSE for any non-root alias
                // (`canon(x) != x`), but a forwarded value is very often an alias of
                // a fresh owned root (`s_next = Copy(s + "x")`, a bare-`Copy` SSA
                // move the union-find folds into `s + "x"`). Checking `droppable(v)`
                // would then misclassify that clean-owned forward as borrowed and
                // RETAIN it every iteration — a per-iteration leak of the
                // accumulator (the exact "fresh owned back-edge value must NOT be
                // retained" hazard). `droppable(root)` already excludes params /
                // stack / non-owning-copy roots, so it is the correct ownership
                // test for the value the edge actually delivers.
                let function_owned = droppable(root);
                // Conditionally-valid iterator value result feeding a phi: its
                // backing slot is only valid on the not-done path — never mint an
                // independent ref obligation for it on an edge. Treat as needing a
                // retain only if we cannot prove clean transfer; but since it is
                // never `droppable`-owned in the transfer sense here, fall through
                // to the borrowed branch is unsafe (it would IncRef a possibly
                // uninitialized slot). So SKIP iter-cond values entirely (they are
                // handled by the body straight-line rule on the valid path).
                if iter_cond_value_results.contains(&v)
                    || iter_cond_value_results
                        .iter()
                        .any(|&iv| canon(iv) == root)
                {
                    continue;
                }
                let clean_transfer = function_owned
                    // (c) sole downstream owner: this root is forwarded by exactly
                    //     one arc of this block AND is not live into any successor
                    //     body. Otherwise its single +1 is shared → retain.
                    && root_forward_count.get(&root).copied().unwrap_or(0) == 1
                    && !succ_body_live_roots
                        .get(&bid)
                        .is_some_and(|s| s.contains(&root));
                if clean_transfer {
                    continue;
                }
                // BORROWED edge into an owned phi → retain `v` on THIS arc.
                arc_retains.push(v);
            }
            if arc_retains.is_empty() {
                continue;
            }
            if arcs_to_target == 1 && !arc.is_self_loop_into_own_phi(bid) {
                // Single, unambiguous arc to the target: place the IncRef before
                // this block's terminator. (A self-loop where the block is its own
                // successor AND its terminator forwards into its own phi is treated
                // as ambiguous below — splitting keeps the IncRef off the in-block
                // straight-line path.)
                let p = plans.entry(bid).or_insert_with(|| BlockPlan {
                    after_op: HashMap::new(),
                    at_entry: Vec::new(),
                    before_term: Vec::new(),
                    before_op: HashMap::new(),
                    before_term_incref: Vec::new(),
                });
                for v in arc_retains {
                    p.before_term_incref.push(v);
                }
            } else {
                // Critical / ambiguous edge: split it. The new block carries the
                // IncRefs then an unconditional Branch to the target with the same
                // args this arc forwarded.
                edge_splits.push(EdgeSplit {
                    pred: bid,
                    arc: arc.descriptor,
                    target: arc.target,
                    args: arc.args.clone(),
                    retains: arc_retains,
                });
            }
        }
    }

    // ── Apply the plans ──────────────────────────────────────────────────────
    let mut inserted = 0usize;
    for (&bid, plan) in &plans {
        let Some(block) = func.blocks.get_mut(&bid) else {
            continue;
        };
        // Rebuild the op vector inserting before_op (IncRef) / after_op (DecRef).
        let mut new_ops: Vec<TirOp> = Vec::with_capacity(block.ops.len() + 8);
        // at_entry DecRefs first.
        let mut entry_seen: HashSet<ValueId> = HashSet::new();
        for &v in &plan.at_entry {
            if entry_seen.insert(v) {
                new_ops.push(make_op(OpCode::DecRef, vec![v]));
                inserted += 1;
            }
        }
        for (idx, op) in block.ops.iter().enumerate() {
            // before_op IncRefs (suspension).
            if let Some(vals) = plan.before_op.get(&idx) {
                let mut seen: HashSet<ValueId> = HashSet::new();
                for &v in vals {
                    if seen.insert(v) {
                        new_ops.push(make_op(OpCode::IncRef, vec![v]));
                        inserted += 1;
                    }
                }
            }
            new_ops.push(op.clone());
            // after_op DecRefs (straight-line last use).
            if let Some(vals) = plan.after_op.get(&idx) {
                let mut seen: HashSet<ValueId> = HashSet::new();
                for &v in vals {
                    if seen.insert(v) {
                        new_ops.push(make_op(OpCode::DecRef, vec![v]));
                        inserted += 1;
                    }
                }
            }
        }
        // before_term_incref IncRefs (the mixed-ownership-phi retain, §5): the
        // BORROWED value this block forwards into a successor's owned phi gets a
        // `+1` here, just before the terminator, on the unambiguous single arc.
        // Placed BEFORE the before_term DecRefs so a value both retained-for-a-phi
        // and dropped-on-another-arc is incref'd before the drop (net correct).
        let mut term_inc_seen: HashSet<ValueId> = HashSet::new();
        for &v in &plan.before_term_incref {
            if term_inc_seen.insert(v) {
                new_ops.push(make_op(OpCode::IncRef, vec![v]));
                inserted += 1;
            }
        }
        // before_term DecRefs (currently unused; kept for the documented
        // loop-carried anchor and future edge-split upgrade).
        let mut term_seen: HashSet<ValueId> = HashSet::new();
        for &v in &plan.before_term {
            if term_seen.insert(v) {
                new_ops.push(make_op(OpCode::DecRef, vec![v]));
                inserted += 1;
            }
        }
        block.ops = new_ops;
    }

    // ── Apply critical-edge splits (§5 ambiguous-arc retains) ─────────────────
    // Each split inserts a fresh block on ONE arc: it holds the retained-value
    // IncRefs then an unconditional Branch to the original target with the args
    // that arc forwarded. The predecessor's terminator is retargeted to the new
    // block (and that arc's args cleared — the new block now supplies them).
    for split in &edge_splits {
        let new_bid = func.fresh_block();
        let mut ops: Vec<TirOp> = Vec::with_capacity(split.retains.len());
        let mut seen: HashSet<ValueId> = HashSet::new();
        for &v in &split.retains {
            if seen.insert(v) {
                ops.push(make_op(OpCode::IncRef, vec![v]));
                inserted += 1;
            }
        }
        func.blocks.insert(
            new_bid,
            crate::tir::blocks::TirBlock {
                id: new_bid,
                args: vec![],
                ops,
                terminator: Terminator::Branch {
                    target: split.target,
                    args: split.args.clone(),
                },
            },
        );
        if let Some(pred) = func.blocks.get_mut(&split.pred) {
            retarget_arc(&mut pred.terminator, &split.arc, new_bid);
        }
    }

    if inserted > 0 {
        func.attrs
            .insert(DROP_INSERTED_ATTR.to_string(), AttrValue::Bool(true));
    }
    if debug_this {
        let mut out = format!("[DROP] {} inserted={} blocks:\n", func.name, inserted);
        let mut bids: Vec<_> = func.blocks.keys().copied().collect();
        bids.sort_by_key(|b| b.0);
        for bid in bids {
            let b = &func.blocks[&bid];
            let args: Vec<u32> = b.args.iter().map(|a| a.id.0).collect();
            out.push_str(&format!(
                "  bb{} args={:?} term={:?}\n",
                bid.0, args, b.terminator
            ));
            for op in &b.ops {
                let ops: Vec<u32> = op.operands.iter().map(|o| o.0).collect();
                let res: Vec<u32> = op.results.iter().map(|r| r.0).collect();
                let reprs: Vec<String> = op
                    .operands
                    .iter()
                    .map(|o| format!("{}:{}", o.0, if live.is_raw_scalar(*o) { "raw" } else { "heap" }))
                    .collect();
                // The `_original_kind` carried by a `Copy` is load-bearing for the
                // alias/ownership model (it decides whether the Copy is a no-incref
                // bit-passthrough alias of operand 0 or a fresh owned value). Surface
                // it in the dump so a re-reviewer can audit the alias-set membership
                // against the lowering truth at a glance.
                let kind = match op.attrs.get("_original_kind") {
                    Some(AttrValue::Str(s)) => format!(" kind={s}"),
                    _ => String::new(),
                };
                out.push_str(&format!(
                    "    {:?} ops={:?} -> {:?}  [{}]{}\n",
                    op.opcode, ops, res, reprs.join(","), kind
                ));
            }
        }
        let _ = crate::debug_artifacts::write_debug_artifact(
            format!("drop/{}.txt", func.name),
            out,
        );
    }
    stats.ops_added = inserted;
    stats
}

/// The alias roots of the operands an op TAKES OWNERSHIP OF (consumes), in
/// alias-root space (design §1.2 "Operands: takes-ownership").
///
/// Most molt ops borrow their operands (the callee never decrefs its args), so
/// the holder drops at last use. A handful of ops are the exception: they
/// **consume** an operand — the runtime entry frees it internally — so the
/// holder MUST NOT also drop it (that double-frees). The drop pass treats a
/// value whose last use is a consumed-operand position exactly like a value
/// consumed by a `Return` terminator: ownership transfers to the op, no
/// trailing `DecRef`.
///
/// The only such ops in molt's lowering are the two CallArgs-builder dispatch
/// forms. The un-fused `obj.method(args)` / indirect-call idiom lowers (in
/// `lower_to_simple`) to:
///
/// ```text
/// b   = callargs_new                         # allocates a CallArgs builder (rc=1)
/// ... = callargs_push_pos(b, a_i) ...
/// r   = call_bind(callee, b)    # molt_call_bind_ic FREES b internally (PtrDropGuard)
/// ```
///
/// `molt_call_bind_ic` / `molt_call_indirect_ic` (`call/bind.rs`) wrap the
/// builder in a `PtrDropGuard`, so the builder is dropped exactly once by the
/// call regardless of whether the call returns or raises. The TIR drop pass
/// would otherwise insert `DecRef(b)` after the call (the builder's last use is
/// the call) → a second free of a `TYPE_ID_CALLARGS` object → `invalid object
/// header before dec_ref`. The builder is operand index 1 (the LAST operand) of
/// `call_bind`/`call_indirect`: SimpleIR `call_bind args=[callee, builder]`,
/// matching the `molt_call_bind_ic(site, callee, builder)` ABI.
///
/// NOTE: `call_func`/`call_guarded`/`call`/`call_internal` do NOT take a
/// pre-built CallArgs operand — they marshal direct positional args and build
/// (and consume) their own builder internally — so they consume none of their
/// TIR operands. Only `call_bind`/`call_indirect` carry the builder as an
/// operand.
///
/// REGISTRY-DRIVEN (design 27 §2.3): the consume signature is no longer a
/// hardcoded `matches!` of the CallArgs-builder spellings here — it is a
/// `[[consuming_kind]]` row in `op_kinds.toml`, generated into
/// [`kind_consumed_operand_table`]. The single declarative authority means a
/// FUTURE consuming op (a streaming builder, a move-into-collection intrinsic)
/// gets correct drop treatment by adding ONE row, never by editing this pass —
/// retiring the per-pass operand-consume hand list (the C6 double-free class).
/// `kind_consumed_operand_table` resolves the `"last"` selector against the op's
/// operand count, exactly reproducing the prior `op.operands.last()` semantics
/// (`arity.checked_sub(1)` is `None` for a 0-operand op, matching `.last()`).
fn op_consumed_operand_root(
    op: &TirOp,
    canon: &dyn Fn(ValueId) -> ValueId,
) -> Option<ValueId> {
    // The consume fact is the UNION of the two generated authorities (the full
    // operand-ownership model, design 27 §2.1/§2.3), evaluated per operand
    // position:
    //   1. the per-OpCode floor `opcode_operand_ownership_table(opcode, idx)` —
    //      `Consumed` for an OpCode that consumes by construction (none today;
    //      `all_borrowed` across the enum — molt's callee-borrows-args ABI), and
    //   2. the per-SPELLING refinement `kind_consumed_operand_table(kind, arity)`
    //      keyed on the Copy-lifted `_original_kind` (finer than the OpCode:
    //      `call_bind`/`call_indirect` and the borrowing `call`/`call_func`/…
    //      spellings all share OpCode::Call; only the two CallArgs-builder forms
    //      consume their builder operand).
    // At most one operand is consumed in molt's lowering today, so the first
    // consumed position is returned (the CallArgs builder = the last operand).
    use crate::tir::op_kinds_generated::{
        kind_consumed_operand_table, opcode_operand_ownership_table, OperandOwnership,
    };
    let kind = match op.attrs.get("_original_kind") {
        Some(AttrValue::Str(k)) => Some(k.as_str()),
        _ => None,
    };
    let spelling_consumed = kind
        .and_then(|k| kind_consumed_operand_table(k, op.operands.len()));
    for idx in 0..op.operands.len() {
        let consumed = spelling_consumed == Some(idx)
            || opcode_operand_ownership_table(op.opcode, idx) == OperandOwnership::Consumed;
        if consumed {
            return op.operands.get(idx).copied().map(&canon);
        }
    }
    None
}

/// True if the alias root `v` is consumed directly by the terminator (a Return
/// value, a CondBranch/Switch condition), comparing in alias-root space. Branch
/// ARGS are handled separately (they transfer ownership to the successor's block
/// arg).
fn terminator_uses_root(term: &Terminator, v: ValueId, canon: &dyn Fn(ValueId) -> ValueId) -> bool {
    match term {
        Terminator::Return { values } => values.iter().any(|&x| canon(x) == v),
        Terminator::CondBranch { cond, .. } => canon(*cond) == v,
        Terminator::Switch { value, .. } => canon(*value) == v,
        Terminator::Branch { .. } | Terminator::Unreachable => false,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::tir::analysis::AnalysisManager;
    use crate::tir::blocks::{LoopRole, Terminator, TirBlock};
    use crate::tir::ops::{AttrDict, AttrValue, Dialect, OpCode, TirOp};
    use crate::tir::types::TirType;
    use crate::tir::values::TirValue;

    fn op(opcode: OpCode, operands: Vec<ValueId>, results: Vec<ValueId>) -> TirOp {
        TirOp {
            dialect: Dialect::Molt,
            opcode,
            operands,
            results,
            attrs: AttrDict::new(),
            source_span: None,
        }
    }

    fn const_str(result: ValueId) -> TirOp {
        let mut attrs = AttrDict::new();
        attrs.insert("s_value".into(), AttrValue::Str("x".into()));
        TirOp {
            dialect: Dialect::Molt,
            opcode: OpCode::ConstStr,
            operands: vec![],
            results: vec![result],
            attrs,
            source_span: None,
        }
    }

    fn count_decrefs(func: &TirFunction) -> usize {
        func.blocks
            .values()
            .flat_map(|b| b.ops.iter())
            .filter(|o| o.opcode == OpCode::DecRef)
            .count()
    }
    fn count_increfs(func: &TirFunction) -> usize {
        func.blocks
            .values()
            .flat_map(|b| b.ops.iter())
            .filter(|o| o.opcode == OpCode::IncRef)
            .count()
    }

    /// Regression (RC drop-insertion substrate, design 20): the real `accumulate`
    /// loop-slot shape from the frontend SimpleIR, run through the FULL pipeline.
    /// The loop loads its carried accumulator via `load_var`→`Copy` every
    /// iteration; a per-SSA-value drop pass double-frees the live accumulator
    /// (the activation blocker — `invalid object header before dec_ref` /
    /// use-after-free at n≥50k). The alias-root-aware drop pass must drop each
    /// underlying heap object EXACTLY ONCE per program point. This test asserts
    /// the no-double-drop invariant directly on the post-pipeline TIR: within any
    /// block, no two `DecRef`s name values that share an alias root.
    #[test]
    fn loop_slot_accumulator_no_double_drop() {
        use crate::ir::{FunctionIR, OpIR};
        use crate::tir::lower_from_simple::lower_to_tir;
        use crate::tir::passes::alias_analysis::build_alias_union_find;
        use crate::tir::passes::run_pipeline;
        use crate::tir::type_refine::refine_types;

        let mk = |kind: &str, out: Option<&str>, var: Option<&str>, args: Vec<&str>, val: Option<i64>, sval: Option<&str>| OpIR {
            kind: kind.into(),
            out: out.map(|s| s.to_string()),
            var: var.map(|s| s.to_string()),
            args: if args.is_empty() { None } else { Some(args.iter().map(|s| s.to_string()).collect()) },
            value: val,
            s_value: sval.map(|s| s.to_string()),
            ..OpIR::default()
        };
        // Shape from tmp/.../native/final_ir/bigint_accumulator__accumulate.txt:
        // total = 1<<60 ; i=0 ; while i<n: total=total+1; total=total-1; total=total+1; i=i+1 ; return total
        let func_ir = FunctionIR {
            name: "diag__accumulate".into(),
            params: vec!["n".into()],
            ops: vec![
                mk("const", Some("v106"), None, vec![], Some(1), None),
                mk("const", Some("v107"), None, vec![], Some(60), None),
                mk("lshift", Some("v108"), None, vec!["v106", "v107"], None, None),
                mk("const", Some("v109"), None, vec![], Some(0), None),
                mk("const", Some("v114"), None, vec![], Some(1), None),
                mk("const", Some("v117"), None, vec![], Some(1), None),
                mk("const", Some("v120"), None, vec![], Some(1), None),
                mk("const", Some("v123"), None, vec![], Some(1), None),
                mk("store_var", None, Some("_bb1_arg0"), vec!["v108"], None, None),
                mk("store_var", None, Some("_bb1_arg1"), vec!["v109"], None, None),
                mk("jump", None, None, vec![], Some(8), None),
                mk("label", None, None, vec![], Some(8), None),
                mk("loop_start", None, None, vec![], None, None),
                mk("load_var", Some("_v19"), Some("_bb1_arg0"), vec![], None, None),
                mk("load_var", Some("_v20"), Some("_bb1_arg1"), vec![], None, None),
                mk("lt", Some("v112"), None, vec!["_v20", "n"], None, None),
                mk("loop_break_if_false", None, None, vec!["v112"], None, None),
                mk("add", Some("v115"), None, vec!["_v19", "v114"], None, None),
                mk("sub", Some("v118"), None, vec!["v115", "v117"], None, None),
                mk("add", Some("v121"), None, vec!["v118", "v120"], None, None),
                mk("add", Some("v124"), None, vec!["_v20", "v123"], None, None),
                mk("store_var", None, Some("_bb1_arg0"), vec!["v121"], None, None),
                mk("store_var", None, Some("_bb1_arg1"), vec!["v124"], None, None),
                mk("loop_continue", None, None, vec![], None, None),
                mk("loop_end", None, None, vec![], None, None),
                mk("jump", None, None, vec![], Some(12), None),
                mk("label", None, None, vec![], Some(12), None),
                mk("ret", None, Some("_v19"), vec!["_v19"], None, None),
            ],
            param_types: Some(vec!["Any".into()]),
            source_file: None,
            is_extern: false,
        };

        let mut tir_func = lower_to_tir(&func_ir);
        refine_types(&mut tir_func);
        // Run the full optimization pipeline to reach the realistic lowered loop
        // shape (Copy-aliased loop-slot loads), THEN run drop insertion directly.
        // The pass is a complete primitive but intentionally NOT wired into
        // `build_default_pipeline` yet (Phase-5 native-RC retirement is the
        // remaining activation prerequisite — see the pass_manager activation
        // note), so we invoke it explicitly here to exercise the alias-root
        // placement on the production-shaped IR.
        run_pipeline(&mut tir_func, &crate::tir::target_info::TargetInfo::native_release_fast());
        {
            let mut am = AnalysisManager::new();
            run(&mut tir_func, &mut am);
        }

        // Invariant: within any block, no two DecRefs share an alias root — a
        // double-drop of one heap object is the activation-blocker use-after-free.
        let aliases = build_alias_union_find(&tir_func);
        for block in tir_func.blocks.values() {
            let mut dropped_roots: HashSet<ValueId> = HashSet::new();
            for op in &block.ops {
                if op.opcode == OpCode::DecRef {
                    let root = aliases.root(op.operands[0]);
                    assert!(
                        dropped_roots.insert(root),
                        "double-drop of alias root {root:?} in one block: {:?}",
                        block.ops
                    );
                }
            }
        }
        // The loop body must drop SOMETHING (the dead intermediates + the prev
        // accumulator) — a fully-inert pass would mean the leak is unclosed.
        let total_decrefs: usize = tir_func
            .blocks
            .values()
            .flat_map(|b| b.ops.iter())
            .filter(|o| o.opcode == OpCode::DecRef)
            .count();
        assert!(total_decrefs >= 2, "loop accumulator must insert drops, got {total_decrefs}");
    }

    /// Branch-arg transfer to a successor must NOT be edge-dropped (design §2.5).
    /// Regression for the `while True: break` shape: `v` is computed in `entry`,
    /// passed as a branch arg to `join`, and received as `join`'s block param `p`.
    /// `v`'s ownership transfers to `p` across the edge — the edge-dying rule must
    /// recognize the per-edge transfer (`incoming_arg_roots` via `terminator_arcs`)
    /// and NOT also drop `v` at `join`'s entry. Doing so double-frees the object the param now
    /// owns (the observed `invalid object header before dec_ref` UAF). `p` is then
    /// returned (transferred to the caller), so the function inserts ZERO drops.
    #[test]
    fn branch_arg_transfer_not_edge_dropped() {
        let mut func = TirFunction::new("xfer".into(), vec![], TirType::DynBox);
        let v = func.fresh_value();
        let p = func.fresh_value();
        func.value_types.insert(v, TirType::Str);
        func.value_types.insert(p, TirType::Str);
        let join = func.fresh_block();
        let entry = func.entry_block;
        {
            let b = func.blocks.get_mut(&entry).unwrap();
            b.ops.push(const_str(v));
            b.terminator = Terminator::Branch {
                target: join,
                args: vec![v],
            };
        }
        func.blocks.insert(join, TirBlock {
            id: join,
            args: vec![TirValue { id: p, ty: TirType::Str }],
            ops: vec![],
            terminator: Terminator::Return { values: vec![p] },
        });
        let mut am = AnalysisManager::new();
        run(&mut func, &mut am);
        // No DecRef of `v` (transferred to `p`), and none of `p` (returned).
        let dropped: Vec<ValueId> = func
            .blocks
            .values()
            .flat_map(|b| b.ops.iter())
            .filter(|o| o.opcode == OpCode::DecRef)
            .map(|o| o.operands[0])
            .collect();
        assert!(
            !dropped.contains(&v),
            "branch-arg `v` transferred to the successor param must NOT be edge-dropped (double-free); dropped={dropped:?}",
        );
        assert_eq!(
            count_decrefs(&func),
            0,
            "transfer-through-edge + return must insert zero drops; dropped={dropped:?}",
        );
    }

    /// Straight-line temp: v1 = Call(a); v2 = Call(v1); Return(v2).
    /// v1 dies after op 2 → exactly one DecRef(v1). v2 is returned (transferred)
    /// → not dropped.
    #[test]
    fn straight_line_temp_dropped_once() {
        let mut func = TirFunction::new("sl".into(), vec![], TirType::DynBox);
        let a = func.fresh_value();
        let v1 = func.fresh_value();
        let v2 = func.fresh_value();
        for v in [a, v1, v2] {
            func.value_types.insert(v, TirType::Str);
        }
        let entry = func.entry_block;
        {
            let b = func.blocks.get_mut(&entry).unwrap();
            b.ops.push(const_str(a));
            b.ops.push(op(OpCode::Call, vec![a], vec![v1]));
            b.ops.push(op(OpCode::Call, vec![v1], vec![v2]));
            b.terminator = Terminator::Return { values: vec![v2] };
        }
        let mut am = AnalysisManager::new();
        let stats = run(&mut func, &mut am);
        assert!(stats.ops_added >= 1);
        // a dies after op 1; v1 dies after op 2; v2 is returned. So DecRef(a) and
        // DecRef(v1), not DecRef(v2).
        let decrefs: Vec<ValueId> = func.blocks[&entry]
            .ops
            .iter()
            .filter(|o| o.opcode == OpCode::DecRef)
            .map(|o| o.operands[0])
            .collect();
        assert!(decrefs.contains(&a), "a must be dropped at last use");
        assert!(decrefs.contains(&v1), "v1 must be dropped at last use");
        assert!(!decrefs.contains(&v2), "returned value must not be dropped");
        assert!(func.attrs.contains_key(DROP_INSERTED_ATTR));
    }

    /// A CallArgs builder consumed by `call_bind` / `call_indirect` must NOT get
    /// a trailing DecRef: the runtime entry (`molt_call_bind_ic`, via
    /// `PtrDropGuard`) frees the builder internally, so an inserted DecRef would
    /// double-free the `TYPE_ID_CALLARGS` object (design-20 finding #3C: the
    /// method-call `'invalid object header before dec_ref'` abort). The callee
    /// (operand 0) and the call RESULT are still dropped normally.
    #[test]
    fn call_bind_callargs_operand_not_dropped() {
        let mut func = TirFunction::new("cb".into(), vec![], TirType::DynBox);
        let callee = func.fresh_value(); // the bound method (a fresh owned ref)
        let builder = func.fresh_value(); // the CallArgs builder
        let result = func.fresh_value(); // the call result
        for v in [callee, builder, result] {
            func.value_types.insert(v, TirType::DynBox);
        }
        let entry = func.entry_block;
        {
            let b = func.blocks.get_mut(&entry).unwrap();
            // callee = <fresh owned value> (model as a Call so it is owned).
            b.ops.push(op(OpCode::Call, vec![], vec![callee]));
            // builder = callargs_new (opaque Copy carrying _original_kind).
            let mut ca = AttrDict::new();
            ca.insert(
                "_original_kind".into(),
                AttrValue::Str("callargs_new".into()),
            );
            b.ops.push(TirOp {
                dialect: Dialect::Molt,
                opcode: OpCode::Copy,
                operands: vec![],
                results: vec![builder],
                attrs: ca,
                source_span: None,
            });
            // result = call_bind(callee, builder) — Call carrying _original_kind.
            let mut cb = AttrDict::new();
            cb.insert("_original_kind".into(), AttrValue::Str("call_bind".into()));
            b.ops.push(TirOp {
                dialect: Dialect::Molt,
                opcode: OpCode::Call,
                operands: vec![callee, builder],
                results: vec![result],
                attrs: cb,
                source_span: None,
            });
            b.terminator = Terminator::Return { values: vec![result] };
        }
        let mut am = AnalysisManager::new();
        run(&mut func, &mut am);
        let decrefs: Vec<ValueId> = func.blocks[&entry]
            .ops
            .iter()
            .filter(|o| o.opcode == OpCode::DecRef)
            .map(|o| o.operands[0])
            .collect();
        assert!(
            !decrefs.contains(&builder),
            "the CallArgs builder is consumed by call_bind; it must NOT be DecRef'd (double-free)"
        );
        assert!(
            decrefs.contains(&callee),
            "the callee (borrowed-then-dead) must be dropped at its last use (the call)"
        );
        // The result is returned → not dropped here.
        assert!(!decrefs.contains(&result));
    }

    /// Interior-borrow keepalive (round-6 BLOCKER-1). A heap object's LAST DIRECT
    /// operand use is a `LoadAttr` that extracts a value the object's backing store
    /// owns (the `Counter._handle` raw-int registry-handle shape: the wrapper's
    /// finalizer destroys the registry entry the handle indexes). The extracted
    /// value `h` is then consumed by a later `Call`. The source object `obj` MUST be
    /// dropped AFTER `h`'s last use (the Call), NEVER right after the `LoadAttr` —
    /// dropping it earlier runs the finalizer and invalidates `h` (the observed UAF:
    /// `len(Counter(...))` returned 0). Mirrors the de-sugared fast-path lowering
    /// `h = get_attr(counts, "_handle"); molt_counter_len(h)`.
    #[test]
    fn loadattr_source_kept_alive_through_borrow_result_use() {
        let mut func = TirFunction::new("borrow".into(), vec![], TirType::DynBox);
        let obj = func.fresh_value(); // the wrapper (fresh owned)
        let h = func.fresh_value(); // LoadAttr(obj) — borrows into obj's store
        let len_fn = func.fresh_value(); // the `molt_counter_len` builtin
        let res = func.fresh_value(); // Call(len_fn, h) result
        for v in [obj, h, len_fn, res] {
            func.value_types.insert(v, TirType::Str);
        }
        let entry = func.entry_block;
        {
            let b = func.blocks.get_mut(&entry).unwrap();
            b.ops.push(const_str(obj)); // op 0: obj = fresh owned
            b.ops.push(op(OpCode::LoadAttr, vec![obj], vec![h])); // op 1: h = obj._handle (last DIRECT use of obj)
            b.ops.push(const_str(len_fn)); // op 2: the builtin
            b.ops.push(op(OpCode::Call, vec![len_fn, h], vec![res])); // op 3: len(h) — needs obj alive
            b.terminator = Terminator::Return { values: vec![res] };
        }
        let mut am = AnalysisManager::new();
        run(&mut func, &mut am);
        let ops = &func.blocks[&entry].ops;
        // Find the Call (the consumer of the borrow result) and the DecRef(obj).
        let call_idx = ops
            .iter()
            .position(|o| o.opcode == OpCode::Call)
            .expect("call present");
        let decref_obj_idx = ops
            .iter()
            .position(|o| o.opcode == OpCode::DecRef && o.operands == vec![obj]);
        assert!(
            decref_obj_idx.is_some(),
            "source object must still be dropped (no leak); ops={ops:?}"
        );
        assert!(
            decref_obj_idx.unwrap() > call_idx,
            "source object must be dropped AFTER the borrow result's consuming Call \
             (interior-borrow keepalive), not at its last direct operand use; \
             decref@{:?} call@{call_idx} ops={ops:?}",
            decref_obj_idx.unwrap(),
        );
    }

    /// Interior-borrow keepalive across a transparent `Copy` of the source (the
    /// `load_var` shape): `obj` is loaded via a `Copy` (alias root = obj), the alias
    /// feeds a `LoadAttr`, and the LoadAttr result is consumed later. The drop of
    /// the underlying object (alias root) must still be deferred past the consumer.
    #[test]
    fn loadattr_keepalive_through_copy_aliased_source() {
        let mut func = TirFunction::new("borrow_alias".into(), vec![], TirType::DynBox);
        let obj = func.fresh_value();
        let obj_alias = func.fresh_value(); // Copy(obj) — load_var alias
        let h = func.fresh_value(); // LoadAttr(obj_alias)
        let consumer = func.fresh_value(); // Call(h) result
        for v in [obj, obj_alias, h, consumer] {
            func.value_types.insert(v, TirType::Str);
        }
        let entry = func.entry_block;
        {
            let b = func.blocks.get_mut(&entry).unwrap();
            b.ops.push(const_str(obj));
            b.ops.push({
                let mut o = op(OpCode::Copy, vec![obj], vec![obj_alias]);
                o.attrs
                    .insert("_original_kind".into(), AttrValue::Str("load_var".into()));
                o
            });
            b.ops.push(op(OpCode::LoadAttr, vec![obj_alias], vec![h]));
            b.ops.push(op(OpCode::Call, vec![h], vec![consumer]));
            b.terminator = Terminator::Return { values: vec![consumer] };
        }
        let mut am = AnalysisManager::new();
        run(&mut func, &mut am);
        let ops = &func.blocks[&entry].ops;
        let call_idx = ops
            .iter()
            .position(|o| o.opcode == OpCode::Call)
            .expect("call present");
        // The underlying object is released through some alias of its root, exactly
        // once, AFTER the consumer. Find any DecRef whose operand aliases obj's root.
        let aliases = crate::tir::passes::alias_analysis::build_alias_union_find(&func);
        let obj_root = aliases.root(obj);
        let decref_positions: Vec<usize> = ops
            .iter()
            .enumerate()
            .filter(|(_, o)| {
                o.opcode == OpCode::DecRef
                    && o.operands.first().is_some_and(|&v| aliases.root(v) == obj_root)
            })
            .map(|(i, _)| i)
            .collect();
        assert_eq!(
            decref_positions.len(),
            1,
            "the source object's group must be released exactly once; ops={ops:?}"
        );
        assert!(
            decref_positions[0] > call_idx,
            "source object drop must follow the borrow result's consumer; \
             decref@{} call@{call_idx} ops={ops:?}",
            decref_positions[0],
        );
    }

    /// Raw i64 values get ZERO drops (perf contract / design R3).
    #[test]
    fn raw_i64_gets_no_drops() {
        let mut func = TirFunction::new("raw".into(), vec![], TirType::I64);
        let c0 = func.fresh_value();
        let c1 = func.fresh_value();
        let s = func.fresh_value();
        for v in [c0, c1, s] {
            func.value_types.insert(v, TirType::I64);
        }
        let entry = func.entry_block;
        {
            let b = func.blocks.get_mut(&entry).unwrap();
            let mut a0 = AttrDict::new();
            a0.insert("value".into(), AttrValue::Int(3));
            b.ops.push(TirOp {
                dialect: Dialect::Molt,
                opcode: OpCode::ConstInt,
                operands: vec![],
                results: vec![c0],
                attrs: a0,
                source_span: None,
            });
            let mut a1 = AttrDict::new();
            a1.insert("value".into(), AttrValue::Int(4));
            b.ops.push(TirOp {
                dialect: Dialect::Molt,
                opcode: OpCode::ConstInt,
                operands: vec![],
                results: vec![c1],
                attrs: a1,
                source_span: None,
            });
            b.ops.push(op(OpCode::Add, vec![c0, c1], vec![s]));
            b.terminator = Terminator::Return { values: vec![s] };
        }
        let mut am = AnalysisManager::new();
        run(&mut func, &mut am);
        assert_eq!(count_decrefs(&func), 0, "raw i64 lane must get zero drops");
    }

    /// StackAlloc values get ZERO drops (design R6).
    #[test]
    fn stack_alloc_gets_no_drops() {
        let mut func = TirFunction::new("st".into(), vec![], TirType::DynBox);
        let s = func.fresh_value();
        let used = func.fresh_value();
        func.value_types.insert(s, TirType::DynBox);
        func.value_types.insert(used, TirType::DynBox);
        let entry = func.entry_block;
        {
            let b = func.blocks.get_mut(&entry).unwrap();
            b.ops.push(op(OpCode::StackAlloc, vec![], vec![s]));
            b.ops.push(op(OpCode::LoadAttr, vec![s], vec![used]));
            b.terminator = Terminator::Return { values: vec![used] };
        }
        let mut am = AnalysisManager::new();
        run(&mut func, &mut am);
        let decrefs: Vec<ValueId> = func.blocks[&entry]
            .ops
            .iter()
            .filter(|o| o.opcode == OpCode::DecRef)
            .map(|o| o.operands[0])
            .collect();
        assert!(!decrefs.contains(&s), "stack value must never be dropped");
    }

    /// A lowered coroutine `_poll` STATE MACHINE (a `StateSwitch` dispatch) must
    /// get ZERO drops — the pass bails (`has_state_machine`). Regression for the
    /// LLVM verifier failure where a drop placed in a state-resume block
    /// referenced a value defined only on the non-taken first-entry path
    /// (`dec_ref %v` before `%v = ...`; a use-before-def that also double-frees on
    /// native). A generator can carry `StateSwitch` WITHOUT `StateBlock*`
    /// delimiters, so the handler bail alone misses it.
    #[test]
    fn state_machine_function_gets_no_drops() {
        let mut func = TirFunction::new("poll".into(), vec![], TirType::DynBox);
        let st = func.fresh_value();
        let v = func.fresh_value();
        func.value_types.insert(st, TirType::I64);
        func.value_types.insert(v, TirType::Str);
        let entry = func.entry_block;
        {
            let b = func.blocks.get_mut(&entry).unwrap();
            // A state-machine dispatch op marks this as a lowered `_poll` body.
            b.ops.push(op(OpCode::StateSwitch, vec![st], vec![]));
            // A heap temp whose naive last-use drop would be unsound over the
            // re-entrant state CFG.
            b.ops.push(const_str(v));
            b.ops.push(op(OpCode::Call, vec![v], vec![]));
            b.terminator = Terminator::Return { values: vec![] };
        }
        assert!(
            func.has_state_machine(),
            "fixture must look like a lowered state machine",
        );
        let mut am = AnalysisManager::new();
        run(&mut func, &mut am);
        assert_eq!(
            count_decrefs(&func),
            0,
            "state-machine `_poll` body must get zero drops (pass bails)",
        );
        assert_eq!(count_increfs(&func), 0);
    }

    /// Loop-carried phi `s` used on BOTH the loop body (new value computed, old
    /// `s` dead on the back-edge path) AND the exit path (a non-alias consumer),
    /// in the real-phi (LLVM) shape. The header phi must be dropped on the path
    /// where it dies — the back-edge body block — exactly once. Regression for the
    /// LLVM string-concat leak: the drop pass inserted NO `DecRef(s_phi)` for this
    /// shape (the accumulator's old value leaked every iteration: `dealloc=5/n`).
    ///
    /// Shape (mirrors `string_concat__concat` after lowering):
    ///   entry: s0 = ConstStr; br header(s0)
    ///   header(s_phi): cond_br c, body, exit
    ///   body: s_new = Add(s_phi, "x"); br header(s_new)   // old s_phi dies here
    ///   exit: r = Len(s_phi); return r                    // s_phi consumed, dies
    #[test]
    fn loop_carried_phi_dropped_on_backedge() {
        let mut func = TirFunction::new("acc".into(), vec![], TirType::I64);
        let s0 = func.fresh_value();
        let s_phi = func.fresh_value();
        let s_alias = func.fresh_value();
        let lit = func.fresh_value();
        let cond = func.fresh_value();
        let s_new = func.fresh_value();
        let r = func.fresh_value();
        func.value_types.insert(s0, TirType::Str);
        func.value_types.insert(s_phi, TirType::Str);
        func.value_types.insert(s_alias, TirType::Str);
        func.value_types.insert(lit, TirType::Str);
        func.value_types.insert(cond, TirType::Bool);
        func.value_types.insert(s_new, TirType::Str);
        func.value_types.insert(r, TirType::I64);

        // Mirror the lowered `string_concat__concat` CFG precisely: the cond lives
        // in a SEPARATE block (`cond_blk`, real bb3) reached from the header, and a
        // transparent `Copy` of the phi (`s_alias`, real `%11 = copy %9`) is the
        // value actually consumed on BOTH the loop body and the exit paths. The
        // exit goes through an intermediate `pre_exit` block (real bb6). This is
        // the shape the simpler direct-header-cond fixture did NOT reproduce.
        let header = func.fresh_block();
        let cond_blk = func.fresh_block();
        let body = func.fresh_block();
        let pre_exit = func.fresh_block();
        let exit = func.fresh_block();
        let entry = func.entry_block;
        {
            let b = func.blocks.get_mut(&entry).unwrap();
            b.ops.push(const_str(s0));
            b.terminator = Terminator::Branch {
                target: header,
                args: vec![s0],
            };
        }
        func.blocks.insert(header, TirBlock {
            id: header,
            args: vec![TirValue { id: s_phi, ty: TirType::Str }],
            ops: vec![],
            terminator: Terminator::Branch {
                target: cond_blk,
                args: vec![],
            },
        });
        func.blocks.insert(cond_blk, TirBlock {
            id: cond_blk,
            args: vec![],
            // `s_alias = Copy(s_phi)` — a transparent alias (root = s_phi) used by
            // both successors; plus the loop condition.
            ops: vec![
                op(OpCode::Copy, vec![s_phi], vec![s_alias]),
                op(OpCode::ConstBool, vec![], vec![cond]),
            ],
            terminator: Terminator::CondBranch {
                cond,
                then_block: body,
                then_args: vec![],
                else_block: pre_exit,
                else_args: vec![],
            },
        });
        func.blocks.insert(body, TirBlock {
            id: body,
            args: vec![],
            ops: vec![const_str(lit), op(OpCode::Add, vec![s_alias, lit], vec![s_new])],
            terminator: Terminator::Branch {
                target: header,
                args: vec![s_new],
            },
        });
        func.blocks.insert(pre_exit, TirBlock {
            id: pre_exit,
            args: vec![],
            ops: vec![],
            terminator: Terminator::Branch {
                target: exit,
                args: vec![],
            },
        });
        // A fresh (non-alias) consumer of the aliased phi → it dies after it.
        // `Call` borrows its operand and returns a fresh owned value (the real IR
        // uses a `len`-carrying op here; the only property that matters for
        // liveness is that the result is NOT a transparent alias).
        func.blocks.insert(exit, TirBlock {
            id: exit,
            args: vec![],
            ops: vec![op(OpCode::Call, vec![s_alias], vec![r])],
            terminator: Terminator::Return { values: vec![r] },
        });
        func.loop_roles.insert(header, LoopRole::LoopHeader);

        let mut am = AnalysisManager::new();
        run(&mut func, &mut am);

        // The header phi `s_phi` (and the literal `lit`) are owned heap values that
        // die — `s_phi` on the back-edge body path and on the exit path, `lit`
        // after the Add. The pass MUST drop the accumulator; a fully-inert result
        // is the leak. Assert `s_phi` is dropped somewhere.
        let dropped: HashSet<ValueId> = func
            .blocks
            .values()
            .flat_map(|b| b.ops.iter())
            .filter(|o| o.opcode == OpCode::DecRef)
            .map(|o| o.operands[0])
            .collect();
        assert!(
            dropped.contains(&s_phi),
            "loop-carried phi accumulator must be dropped (else it leaks every \
             iteration); drops={dropped:?}",
        );
        // And no double-drop of any root within a single block.
        let aliases = crate::tir::passes::alias_analysis::build_alias_union_find(&func);
        for block in func.blocks.values() {
            let mut roots: HashSet<ValueId> = HashSet::new();
            for o in &block.ops {
                if o.opcode == OpCode::DecRef {
                    assert!(
                        roots.insert(aliases.root(o.operands[0])),
                        "double-drop in one block: {:?}",
                        block.ops,
                    );
                }
            }
        }
    }

    /// Parameters are borrowed — never dropped.
    #[test]
    fn params_not_dropped() {
        let mut func = TirFunction::new("p".into(), vec![TirType::Str], TirType::DynBox);
        let p0 = ValueId(0);
        let r = func.fresh_value();
        func.value_types.insert(r, TirType::Str);
        let entry = func.entry_block;
        {
            let b = func.blocks.get_mut(&entry).unwrap();
            b.ops.push(op(OpCode::Call, vec![p0], vec![r]));
            b.terminator = Terminator::Return { values: vec![r] };
        }
        let mut am = AnalysisManager::new();
        run(&mut func, &mut am);
        let decrefs: Vec<ValueId> = func.blocks[&entry]
            .ops
            .iter()
            .filter(|o| o.opcode == OpCode::DecRef)
            .map(|o| o.operands[0])
            .collect();
        assert!(!decrefs.contains(&p0), "parameter must not be dropped");
    }

    /// Borrow inference: a value whose only use is a call argument and is dead
    /// after the call is dropped AFTER the call (last-use), never before.
    #[test]
    fn borrow_into_call_dropped_after() {
        let mut func = TirFunction::new("bc".into(), vec![], TirType::DynBox);
        let x = func.fresh_value();
        let res = func.fresh_value();
        let out = func.fresh_value();
        for v in [x, res, out] {
            func.value_types.insert(v, TirType::Str);
        }
        let entry = func.entry_block;
        {
            let b = func.blocks.get_mut(&entry).unwrap();
            b.ops.push(const_str(x));
            b.ops.push(op(OpCode::Call, vec![x], vec![res]));
            b.ops.push(op(OpCode::Call, vec![res], vec![out]));
            b.terminator = Terminator::Return { values: vec![out] };
        }
        let mut am = AnalysisManager::new();
        run(&mut func, &mut am);
        // x's last use is op 1 (the call). DecRef(x) must come AFTER op 1, before
        // the next op. Find the index of DecRef(x) and assert it follows the call.
        let ops = &func.blocks[&entry].ops;
        let call_x_idx = ops
            .iter()
            .position(|o| o.opcode == OpCode::Call && o.operands == vec![x])
            .unwrap();
        let decref_x_idx = ops
            .iter()
            .position(|o| o.opcode == OpCode::DecRef && o.operands == vec![x]);
        assert!(decref_x_idx.is_some(), "x dropped at last use");
        assert!(decref_x_idx.unwrap() > call_x_idx, "drop AFTER the call");
    }

    /// Generator yield: a value live across the yield gets an IncRef before it.
    #[test]
    fn yield_increfs_live_across() {
        let mut func = TirFunction::new("g".into(), vec![], TirType::DynBox);
        let header = func.entry_block;
        let resume = func.fresh_block();
        let x = func.fresh_value();
        let yval = func.fresh_value();
        let used = func.fresh_value();
        for v in [x, yval, used] {
            func.value_types.insert(v, TirType::Str);
        }
        {
            let b = func.blocks.get_mut(&header).unwrap();
            b.ops.push(const_str(x));
            b.ops.push(const_str(yval));
            // Yield: x is live across (used in resume), yval is the yielded value.
            b.ops.push(op(OpCode::Yield, vec![yval], vec![]));
            b.terminator = Terminator::Branch { target: resume, args: vec![] };
        }
        func.blocks.insert(resume, TirBlock {
            id: resume,
            args: vec![],
            ops: vec![op(OpCode::Call, vec![x], vec![used])],
            terminator: Terminator::Return { values: vec![used] },
        });
        let mut am = AnalysisManager::new();
        run(&mut func, &mut am);
        // x must be IncRef'd before the Yield (it survives into the frame).
        let header_ops = &func.blocks[&header].ops;
        let yield_idx = header_ops
            .iter()
            .position(|o| o.opcode == OpCode::Yield)
            .unwrap();
        let incref_x_before = header_ops[..yield_idx]
            .iter()
            .any(|o| o.opcode == OpCode::IncRef && o.operands == vec![x]);
        assert!(incref_x_before, "live-across-yield value must be IncRef'd");
        assert!(count_increfs(&func) >= 1);
    }

    /// Loop accumulator: a heap accumulator threaded through a header block arg
    /// and updated on the back-edge gets a drop for the dead previous value, and
    /// the loop-exit value is dropped (dead after the loop).
    #[test]
    fn loop_accumulator_dropped() {
        let mut func = TirFunction::new("loop".into(), vec![], TirType::DynBox);
        let header = func.fresh_block();
        let body = func.fresh_block();
        let exit = func.fresh_block();
        let acc0 = func.fresh_value();
        let acc_phi = func.fresh_value();
        let cond = func.fresh_value();
        let acc_next = func.fresh_value();
        for v in [acc0, acc_phi, acc_next] {
            func.value_types.insert(v, TirType::Str);
        }
        func.value_types.insert(cond, TirType::Bool);
        let entry = func.entry_block;
        {
            let b = func.blocks.get_mut(&entry).unwrap();
            b.ops.push(const_str(acc0));
            b.terminator = Terminator::Branch { target: header, args: vec![acc0] };
        }
        func.blocks.insert(header, TirBlock {
            id: header,
            args: vec![TirValue { id: acc_phi, ty: TirType::Str }],
            ops: vec![op(OpCode::ConstBool, vec![], vec![cond])],
            terminator: Terminator::CondBranch {
                cond,
                then_block: body,
                then_args: vec![],
                else_block: exit,
                else_args: vec![],
            },
        });
        func.blocks.insert(body, TirBlock {
            id: body,
            args: vec![],
            // acc_next = Call(acc_phi): consumes the phi, produces a new owned acc.
            ops: vec![op(OpCode::Call, vec![acc_phi], vec![acc_next])],
            terminator: Terminator::Branch { target: header, args: vec![acc_next] },
        });
        func.blocks.insert(exit, TirBlock {
            id: exit,
            args: vec![],
            ops: vec![],
            // The final acc_phi is dead (not returned).
            terminator: Terminator::Return { values: vec![] },
        });
        func.loop_roles.insert(header, LoopRole::LoopHeader);
        let mut am = AnalysisManager::new();
        run(&mut func, &mut am);
        // The loop-exit value (acc_phi, live-out of header into exit but dead in
        // exit) must be dropped at the exit block entry (edge-dying rule).
        let exit_decrefs: Vec<ValueId> = func.blocks[&exit]
            .ops
            .iter()
            .filter(|o| o.opcode == OpCode::DecRef)
            .map(|o| o.operands[0])
            .collect();
        assert!(
            exit_decrefs.contains(&acc_phi),
            "loop-exit dead accumulator must be dropped at exit entry; got {exit_decrefs:?}"
        );
    }

    /// Mixed-ownership phi, INCOMING side (§5 retain). A loop accumulator phi is
    /// seeded on the loop-ENTRY edge with a transparent alias of a BORROWED
    /// parameter (`x = base`), and updated on the back-edge with a fresh owned
    /// value. Because the loop body drops the phi each iteration, the borrowed
    /// entry value must be RETAINED on the entry edge (before the preheader's
    /// terminator) so the phi uniformly owns a `+1`. The back-edge's fresh owned
    /// value must NOT be retained (that would leak the accumulator each iteration).
    #[test]
    fn mixed_phi_borrowed_param_retained_on_entry_edge() {
        // param `base` (id 0), preheader binds the accumulator phi to Copy(base).
        let mut func = TirFunction::new("apply".into(), vec![TirType::Str], TirType::DynBox);
        let base = ValueId(0);
        let pre = func.fresh_block(); // preheader
        let header = func.fresh_block();
        let body = func.fresh_block();
        let exit = func.fresh_block();
        let x0 = func.fresh_value(); // Copy(base) — borrowed alias seeding the phi
        let acc_phi = func.fresh_value();
        let load_x = func.fresh_value(); // Copy(acc_phi) in body
        let cond = func.fresh_value();
        let acc_next = func.fresh_value(); // fresh owned (Call result)
        for v in [x0, acc_phi, load_x, acc_next] {
            func.value_types.insert(v, TirType::Str);
        }
        func.value_types.insert(cond, TirType::Bool);
        let entry = func.entry_block;
        {
            let b = func.blocks.get_mut(&entry).unwrap();
            b.terminator = Terminator::Branch { target: pre, args: vec![] };
        }
        // preheader: x0 = copy_var(base) → transparent alias of the param.
        func.blocks.insert(pre, TirBlock {
            id: pre,
            args: vec![],
            ops: vec![{
                let mut o = op(OpCode::Copy, vec![base], vec![x0]);
                o.attrs.insert("_original_kind".into(), AttrValue::Str("copy_var".into()));
                o
            }],
            terminator: Terminator::Branch { target: header, args: vec![x0] },
        });
        func.blocks.insert(header, TirBlock {
            id: header,
            args: vec![TirValue { id: acc_phi, ty: TirType::Str }],
            ops: vec![op(OpCode::ConstBool, vec![], vec![cond])],
            terminator: Terminator::CondBranch {
                cond,
                then_block: body,
                then_args: vec![],
                else_block: exit,
                else_args: vec![],
            },
        });
        func.blocks.insert(body, TirBlock {
            id: body,
            args: vec![],
            ops: vec![
                {
                    let mut o = op(OpCode::Copy, vec![acc_phi], vec![load_x]);
                    o.attrs.insert("_original_kind".into(), AttrValue::Str("load_var".into()));
                    o
                },
                // acc_next = Call(load_x, base): fresh owned, reads base each iter.
                op(OpCode::Call, vec![load_x, base], vec![acc_next]),
            ],
            terminator: Terminator::Branch { target: header, args: vec![acc_next] },
        });
        func.blocks.insert(exit, TirBlock {
            id: exit,
            args: vec![],
            ops: vec![],
            terminator: Terminator::Return { values: vec![] },
        });
        func.loop_roles.insert(header, LoopRole::LoopHeader);
        let mut am = AnalysisManager::new();
        run(&mut func, &mut am);
        // The preheader must IncRef the borrowed `x0` (alias of the param) before
        // its terminator — the entry-edge retain.
        let pre_increfs: Vec<ValueId> = func.blocks[&pre]
            .ops
            .iter()
            .filter(|o| o.opcode == OpCode::IncRef)
            .flat_map(|o| o.operands.clone())
            .collect();
        assert!(
            pre_increfs.contains(&x0),
            "borrowed param alias seeding the loop phi must be retained on the entry edge; got {pre_increfs:?}"
        );
        // The back-edge (body) must NOT retain the fresh owned `acc_next` — that
        // would leak one accumulator per iteration.
        let body_increfs: Vec<ValueId> = func.blocks[&body]
            .ops
            .iter()
            .filter(|o| o.opcode == OpCode::IncRef)
            .flat_map(|o| o.operands.clone())
            .collect();
        assert!(
            !body_increfs.contains(&acc_next),
            "fresh owned back-edge value must NOT be retained (would leak); got {body_increfs:?}"
        );
        // The param itself is never dropped (borrowed ABI).
        let any_decref_base = func
            .blocks
            .values()
            .flat_map(|b| b.ops.iter())
            .any(|o| o.opcode == OpCode::DecRef && o.operands == vec![base]);
        assert!(!any_decref_base, "parameter must never be directly dropped");
    }

    /// Mixed-ownership phi, OUTGOING side (§3 incoming-arg exclusion). An owned
    /// value is FORWARDED as a branch arg into a join block's phi through a
    /// multi-block chain (the shape the inliner produces). The value's ownership
    /// transfers INTO the phi, so it must NOT be edge-dropped at the join entry —
    /// the phi is released by its own last-use drop. A spurious join-entry drop
    /// plus the phi's drop is a double-free.
    #[test]
    fn forwarded_owned_value_not_edge_dropped_at_join() {
        let mut func = TirFunction::new("fwd".into(), vec![], TirType::DynBox);
        let mid = func.fresh_block();
        let join = func.fresh_block();
        let owned = func.fresh_value(); // fresh owned (ConstStr)
        let fwd = func.fresh_value(); // Copy(owned) — alias forwarded to the phi
        let phi = func.fresh_value();
        let used = func.fresh_value();
        for v in [owned, fwd, phi, used] {
            func.value_types.insert(v, TirType::Str);
        }
        let entry = func.entry_block;
        {
            let b = func.blocks.get_mut(&entry).unwrap();
            b.terminator = Terminator::Branch { target: mid, args: vec![] };
        }
        func.blocks.insert(mid, TirBlock {
            id: mid,
            args: vec![],
            ops: vec![
                const_str(owned),
                {
                    let mut o = op(OpCode::Copy, vec![owned], vec![fwd]);
                    o.attrs.insert("_original_kind".into(), AttrValue::Str("copy_var".into()));
                    o
                },
            ],
            // Forward `fwd` (owned, via alias) into the join's phi.
            terminator: Terminator::Branch { target: join, args: vec![fwd] },
        });
        func.blocks.insert(join, TirBlock {
            id: join,
            args: vec![TirValue { id: phi, ty: TirType::Str }],
            ops: vec![op(OpCode::Call, vec![phi], vec![used])],
            terminator: Terminator::Return { values: vec![used] },
        });
        let mut am = AnalysisManager::new();
        run(&mut func, &mut am);
        // The forwarded owned value (`fwd`, alias root `owned`) must NOT be dropped
        // at the join entry — it transferred into the phi. The phi's own last-use
        // drop (after the Call) releases the object exactly once.
        let join_entry_decrefs: Vec<ValueId> = func.blocks[&join]
            .ops
            .iter()
            .take_while(|o| o.opcode == OpCode::DecRef)
            .flat_map(|o| o.operands.clone())
            .collect();
        assert!(
            !join_entry_decrefs.contains(&fwd) && !join_entry_decrefs.contains(&owned),
            "forwarded owned value must not be edge-dropped at the join; got {join_entry_decrefs:?}"
        );
        // Exactly one DecRef releases the group (the phi at its last use in join).
        let total_group_decrefs = func
            .blocks
            .values()
            .flat_map(|b| b.ops.iter())
            .filter(|o| {
                o.opcode == OpCode::DecRef
                    && o.operands
                        .first()
                        .is_some_and(|&v| v == fwd || v == owned || v == phi)
            })
            .count();
        assert_eq!(
            total_group_decrefs, 1,
            "the owned forwarded group must be released exactly once, not double-freed"
        );
    }

    /// Mixed-ownership phi, CRITICAL-EDGE SPLIT (§5 ambiguous-arc retain; round-4
    /// Finding 2). When a predecessor reaches an OWNED phi via MORE THAN ONE arc
    /// with DIFFERENT args, a before-terminator IncRef would wrongly fire on the
    /// other arc, so the pass SPLITS the critical edge: it inserts a fresh block
    /// holding the edge-exact `IncRef` + an unconditional `Branch` to the target,
    /// and retargets exactly that arc to the new block. This is the only path that
    /// allocates a block (it is why the pass is `Mutates::Cfg`), and it shipped
    /// with ZERO coverage before this test.
    ///
    /// Shape: `entry` ends in a `Switch` whose case-0 and DEFAULT arcs BOTH target
    /// `join` but forward DIFFERENT args into `join`'s single owned phi — case-0
    /// forwards a transparent alias of the borrowed param `base` (BORROWED → must
    /// retain), default forwards a freshly minted owned `ConstStr` (clean transfer
    /// → no retain). `join` consumes the phi (a `Call`) and returns nothing, so the
    /// phi is dropped and the borrowed case-0 edge needs its `+1`. Because case-0
    /// and default both go to `join`, the retain cannot be placed before `entry`'s
    /// terminator (it would also fire on the default arc); it must be split onto
    /// the case-0 arc.
    #[test]
    fn mixed_phi_critical_edge_split_inserts_fresh_incref_block() {
        // param `base` (id 0): borrowed heap Str.
        let mut func = TirFunction::new("split".into(), vec![TirType::Str], TirType::DynBox);
        let base = ValueId(0);
        let join = func.fresh_block();
        let case0_alias = func.fresh_value(); // Copy(base) — borrowed alias (case 0 arg)
        let sel = func.fresh_value(); // Switch selector (raw)
        let fresh_owned = func.fresh_value(); // ConstStr — fresh owned (default arg)
        let phi = func.fresh_value(); // join's owned obj-lane phi
        let used = func.fresh_value(); // Call(phi) result
        for v in [case0_alias, fresh_owned, phi, used] {
            func.value_types.insert(v, TirType::Str);
        }
        func.value_types.insert(sel, TirType::I64);
        let entry = func.entry_block;
        {
            let b = func.blocks.get_mut(&entry).unwrap();
            // case0_alias = copy_var(base): a transparent (borrowed) alias of the
            // param; fresh_owned = ConstStr: a freshly minted owned value; sel: the
            // raw Switch selector.
            b.ops.push({
                let mut o = op(OpCode::Copy, vec![base], vec![case0_alias]);
                o.attrs.insert("_original_kind".into(), AttrValue::Str("copy_var".into()));
                o
            });
            b.ops.push(const_str(fresh_owned));
            b.ops.push(op(OpCode::ConstInt, vec![], vec![sel]));
            // Switch: case 0 → join(case0_alias); default → join(fresh_owned).
            // TWO arcs to `join` with DIFFERENT args ⇒ a critical edge.
            b.terminator = Terminator::Switch {
                value: sel,
                cases: vec![(0, join, vec![case0_alias])],
                default: join,
                default_args: vec![fresh_owned],
            };
        }
        func.blocks.insert(join, TirBlock {
            id: join,
            args: vec![TirValue { id: phi, ty: TirType::Str }],
            // Consume the phi (drops it at its last use) and return nothing so the
            // phi dies in `join` — the case-0 borrowed edge therefore needs a +1.
            ops: vec![op(OpCode::Call, vec![phi], vec![used])],
            terminator: Terminator::Return { values: vec![] },
        });
        let n_blocks_before = func.blocks.len();
        let mut am = AnalysisManager::new();
        run(&mut func, &mut am);

        // A fresh block must have been inserted by the critical-edge split.
        assert!(
            func.blocks.len() > n_blocks_before,
            "the critical-edge split must allocate a fresh block; before={n_blocks_before} after={}",
            func.blocks.len()
        );

        // `entry`'s case-0 arc must now target a NEW block (not `join`): the retarget.
        // The default arc must still go to `join` (unsplit, clean transfer).
        let (case0_target, default_target) = match &func.blocks[&entry].terminator {
            Terminator::Switch {
                cases, default, ..
            } => (cases[0].1, *default),
            other => panic!("entry terminator must remain a Switch, got {other:?}"),
        };
        assert_ne!(
            case0_target, join,
            "the borrowed case-0 arc must be retargeted away from `join` to the split block"
        );
        assert_eq!(
            default_target, join,
            "the clean-transfer default arc must stay pointed at `join` (not split)"
        );

        // The split block must (a) hold an IncRef of the borrowed alias `case0_alias`
        // and (b) branch unconditionally to `join` forwarding that same arg.
        let split = &func.blocks[&case0_target];
        let split_increfs: Vec<ValueId> = split
            .ops
            .iter()
            .filter(|o| o.opcode == OpCode::IncRef)
            .flat_map(|o| o.operands.clone())
            .collect();
        assert!(
            split_increfs.contains(&case0_alias),
            "the split block must retain (IncRef) the borrowed case-0 value; got {split_increfs:?}"
        );
        match &split.terminator {
            Terminator::Branch { target, args } => {
                assert_eq!(*target, join, "split block must branch to the original target");
                assert_eq!(
                    args,
                    &vec![case0_alias],
                    "split block must forward the case-0 arg it took over"
                );
            }
            other => panic!("split block must end in an unconditional Branch, got {other:?}"),
        }

        // The default (clean-transfer, freshly owned) value must NOT be retained
        // anywhere — retaining it would leak.
        let any_incref_fresh = func
            .blocks
            .values()
            .flat_map(|b| b.ops.iter())
            .any(|o| o.opcode == OpCode::IncRef && o.operands.first() == Some(&fresh_owned));
        assert!(
            !any_incref_fresh,
            "the clean-transfer default value must not be retained (would leak)"
        );

        // The split result must be a valid CFG: re-run the analysis self-check over
        // the post-split function (mirrors MOLT_VERIFY_ANALYSIS=1) — a malformed
        // split (dangling edge / unreachable target) would diverge the recomputed
        // dominators from a fresh build.
        let mut verify_am = AnalysisManager::new();
        let preds = crate::tir::dominators::build_pred_map_with(
            &func,
            crate::tir::dominators::CfgEdgePolicy::Full,
        );
        let reachable = crate::tir::dominators::reachable_blocks_with(
            &func,
            crate::tir::dominators::CfgEdgePolicy::Full,
        );
        assert!(
            reachable.contains(&case0_target),
            "the split block must be reachable from entry"
        );
        assert!(
            preds.get(&join).is_some_and(|p| p.contains(&case0_target)),
            "the split block must be a predecessor of the original target"
        );
        // Liveness recomputes cleanly over the mutated CFG (would panic on a
        // use-before-def introduced by a bad split).
        let _ = verify_am.get::<TirLiveness>(&func).clone();
    }

    /// FINDING 3 (round-4) fail-closed pin. `incoming_arg_roots` keys on alias
    /// ROOT over ALL predecessors, so a root forwarded into a join's phi by ANY
    /// predecessor is excluded from that join's edge-dying drop on EVERY path.
    /// This test pins the load-bearing invariant the imprecision must preserve:
    /// the exclusion can only ever LEAK, NEVER double-free (over-release → UAF).
    ///
    /// Shape (a diamond where the SAME owned root reaches a join on BOTH edges):
    /// `entry` mints one owned value `r`, then branches to `p1` / `p2`. `p1`
    /// forwards `r` straight into the join's phi (a transfer). `p2` forwards `r`
    /// into the join's phi too (through a transparent alias `r_alias`, the
    /// load-var shape) — so `r`'s root is forwarded by MORE THAN ONE predecessor
    /// and is a member of `incoming_arg_roots`. The join consumes the phi (a
    /// `Call`) and returns nothing. There is exactly ONE underlying owned object;
    /// the assertion is that the pass emits AT MOST ONE `DecRef` naming any member
    /// of `r`'s group across the whole function — never two (the double-free the
    /// global keying must not introduce). A leak (zero drops) would be acceptable
    /// per the fail-closed contract; a double-free would be the UAF bug.
    #[test]
    fn forwarded_into_phi_other_pred_live_is_leak_not_uaf() {
        let mut func = TirFunction::new("diamond".into(), vec![], TirType::DynBox);
        let p1 = func.fresh_block();
        let p2 = func.fresh_block();
        let join = func.fresh_block();
        let r = func.fresh_value(); // fresh owned (ConstStr) defined in entry
        let cond = func.fresh_value();
        let r_alias = func.fresh_value(); // Copy(r) in p2 — transparent alias of r
        let phi = func.fresh_value(); // join's owned obj-lane phi
        let used = func.fresh_value(); // Call(phi) result
        for v in [r, r_alias, phi, used] {
            func.value_types.insert(v, TirType::Str);
        }
        func.value_types.insert(cond, TirType::Bool);
        let entry = func.entry_block;
        {
            let b = func.blocks.get_mut(&entry).unwrap();
            b.ops.push(const_str(r));
            b.ops.push(op(OpCode::ConstBool, vec![], vec![cond]));
            b.terminator = Terminator::CondBranch {
                cond,
                then_block: p1,
                then_args: vec![],
                else_block: p2,
                else_args: vec![],
            };
        }
        // p1: forward `r` straight into the join phi.
        func.blocks.insert(p1, TirBlock {
            id: p1,
            args: vec![],
            ops: vec![],
            terminator: Terminator::Branch { target: join, args: vec![r] },
        });
        // p2: r_alias = load_var(r) [transparent alias]; forward the alias into the
        // SAME phi position → `r`'s root is forwarded by a 2nd predecessor.
        func.blocks.insert(p2, TirBlock {
            id: p2,
            args: vec![],
            ops: vec![{
                let mut o = op(OpCode::Copy, vec![r], vec![r_alias]);
                o.attrs.insert("_original_kind".into(), AttrValue::Str("load_var".into()));
                o
            }],
            terminator: Terminator::Branch { target: join, args: vec![r_alias] },
        });
        func.blocks.insert(join, TirBlock {
            id: join,
            args: vec![TirValue { id: phi, ty: TirType::Str }],
            ops: vec![op(OpCode::Call, vec![phi], vec![used])],
            terminator: Terminator::Return { values: vec![] },
        });
        let mut am = AnalysisManager::new();
        run(&mut func, &mut am);
        // The single owned object (`r`'s alias group: r, r_alias, phi) must be
        // released AT MOST once — never twice. (Fail-closed: a leak is allowed; a
        // double-free is the UAF the global keying must never introduce.)
        let group_decrefs = func
            .blocks
            .values()
            .flat_map(|b| b.ops.iter())
            .filter(|o| {
                o.opcode == OpCode::DecRef
                    && o.operands.first().is_some_and(|&v| {
                        v == r || v == r_alias || v == phi
                    })
            })
            .count();
        assert!(
            group_decrefs <= 1,
            "incoming_arg_roots over-all-preds keying must never double-free a \
             forwarded root (fail-closed: leak ok, UAF never); got {group_decrefs} \
             DecRefs of the owned group"
        );
    }
}
