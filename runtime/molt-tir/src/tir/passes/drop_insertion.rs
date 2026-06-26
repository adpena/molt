//! RC drop insertion (RC drop-insertion substrate, design 20, Phase 3).
//!
//! Inserts `DecRef` ops at every owned value's last use and `IncRef` ops before
//! suspension points for values that survive across a yield. This is the
//! compiler pass that closes molt's whole-program expression-value leak: the
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
//! * **Outgoing side (§3 transfer exclusion).** A value PASSED as a branch arg
//!   into a phi must NOT also be edge-dropped at the join OR in a descendant
//!   block while the phi remains live: its ownership moved into the block arg,
//!   which is released by the phi's own last-use drop. Liveness reports the
//!   forwarded value dead-in to the join (its successor-side identity is the
//!   distinct block-arg SSA value), so the edge-dying rule would otherwise drop it
//!   there, or later when the old source root appears dead, AND at the phi's last
//!   use → double-free.
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
//! `DropEligibility` owns the composed predicate for whether a value root is a
//! drop candidate. A value `v` is eligible when ALL hold:
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
//! last-use placement already does. Finalizer-sensitive values only override
//! that placement when they are Python-bound roots (`store_var` / explicit
//! delete boundary); unbound expression temporaries still die at their last use.
//! We keep the call operands out of any *pre-call* drop, which the last-use
//! semantics guarantee.
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
//!    later alias-move can never read a freed object. A `Copy` result root that
//!    the ownership lattice classifies as non-owning is a no-incref
//!    bit-passthrough or no-heap marker, so it is excluded from droppability:
//!    releasing it would double-free operand 0 or drop a non-ref carrier.
//! 2. **TerminatorOnly dominance** — an edge-dying drop at a successor `B` is
//!    placed only when the value's def-block dominates `B` in the
//!    **terminator-only** CFG (the view codegen enforces). The *full*-CFG
//!    dominator would admit a value defined mid-block after a `CheckException`
//!    as "dominating" that op's handler, but the exception edge leaves before
//!    the def → use-before-def in codegen. (Observed otherwise as the LLVM
//!    verifier "Instruction does not dominate all uses!" abort.)
//! 3. **Python lifetime release boundaries** — a root released by `DelBoundary` /
//!    `DeleteVar` / pre-existing `DecRef`, statement finalizer release, or
//!    `store_var` scope cleanup is path-authoritative and is never edge-dropped
//!    at a join. The OpsOnly edge-dying form has one block-entry drop for all
//!    incoming paths; adding it beside a path-conditioned Python rebind/delete or
//!    later scope-exit boundary can release the same local owner twice.
//! 4. **Conditionally-valid iterator results** — an `IterNextUnboxed` value
//!    result (from the generated result-validity table) is valid ONLY on the
//!    not-done branch; its slot carries a non-owned `None` sentinel on the
//!    exhaustion edge. It is NEVER
//!    edge-dropped (and never IncRef'd onto a phi edge); the body straight-line
//!    rule releases it on the valid path.
//! 5. **State-machine gate** — the pass bails on full-function RC insertion for
//!    functions with generator/async `StateSwitch` / `StateTransition` /
//!    `StateYield` control flow (a `_poll` dispatcher re-enters
//!    `state_resume_*` blocks carrying none of the normal-flow values), in
//!    addition to `try`/`except` regions. Exception transport drops have their
//!    own idempotency marker so the handler-safe CreationRef/MatchRef releases
//!    can still be inserted before the full-function bail without pretending
//!    native's whole value-tracking RC substrate has been retired.
//! 6. **Backend conditioning** — drop insertion is wired into the shared
//!    pipeline for LLVM / WASM / native Cranelift / Luau. Native suppresses its
//!    legacy value-tracking RC substrate on `drop_inserted` functions, so TIR
//!    drops are the single RC authority for activated functions; Luau consumes
//!    the same shared facts as checked GC no-ops. See
//!    `pass_manager::target_uses_tir_drop_insertion`.
//!
//! ## Diagnostics
//!
//! `MOLT_DEBUG_DROP=<substr>` (or `=ALL`) writes a per-function dump of the
//! post-insertion block/op shape with per-operand repr tags to
//! `<artifact_root>/drop/<func>.txt`, including a `BAILED:` line for functions
//! the activation gate skipped. The instrument every optimization lands with.

mod arcs;
mod audit;
mod exception_region;
mod remap;
mod runner;
mod util;

#[cfg(test)]
mod tests;

/// The function-level attr the pass sets (round-tripped to the native backend as
/// a marker op) so the SimpleIR `loop_reassign_old_val` ad-hoc dec-ref path is
/// disabled for drop-inserted functions — preventing the R1 double-drop.
pub const DROP_INSERTED_ATTR: &str = "drop_inserted";

/// Function-level attr for the exception-region-only pre-bail slice. It protects
/// CreationRef/MatchRef `DecRef`s across TIR<->SimpleIR round-trips and
/// `refcount_elim` re-runs, but native MUST NOT interpret it as full-function RC
/// ownership: handlers/state machines still need the legacy native value tracker
/// until shared DropInsertion covers their complete lifetime graph.
pub const EXCEPTION_REGION_DROPS_INSERTED_ATTR: &str = "exception_region_drops_inserted";

pub use self::runner::run;
pub(crate) use self::util::attr_is_true;
