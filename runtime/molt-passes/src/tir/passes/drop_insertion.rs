//! RC drop insertion (RC drop-insertion substrate, design 20, Phase 3).
//!
//! Inserts `DecRef` ops at every owned value's lifetime end and `IncRef` ops
//! at borrowed publication boundaries, including suspension returns. This is the
//! compiler pass that closes molt's whole-program expression-value leak: the
//! runtime allocates every heap result with `ref_count = 1` and (before this
//! pass) never decremented it for expression temporaries.
//!
//! Runs `Mutates::Cfg`: it inserts `DecRef`/`IncRef` ops within blocks, MAY
//! SPLIT an edge (a fresh block carrying edge-exact retains and releases) for
//! an arc whose ownership differs from its source's other arcs, and gives a
//! check a landing block for its exceptional retains and releases.
//! `DecRef`/`IncRef` carry no exception edge, and a split inserts only an
//! unconditional `Branch` — but because the block set/edges CAN change, the pass
//! declares `Cfg` so the manager recomputes CFG-sensitive analyses for the
//! following `refcount_elim_post`.
//!
//! ## Ownership transfer at phi (block-arg) boundaries — the two-sided contract
//!
//! TIR uses MLIR block args as phis. A predecessor's terminator passes a value
//! that binds the successor's block arg on entry, and a raising
//! `CheckException` passes its operands to its handler's block args. A
//! `TryStart` registers its region: it keeps the handler reachable, but control
//! never leaves through it, so it binds nothing. A droppable (heap,
//! function-owned) block arg carries ONE owned `+1`; the pass drops it where it
//! dies and TRANSFERS it (no drop) where it is forwarded. `availability.rs`
//! keeps the custody of every such arc in one place, and both halves of each
//! transfer read it:
//!
//! * **Incoming side (§5 and §2b retains).** Every incoming arc of an owned phi
//!   must deliver an owned `+1`. An arc delivering a BORROWED value (a
//!   transparent alias of a `+0` parameter, or an owned value whose single `+1`
//!   is needed elsewhere too) is RETAINED on that arc: before the terminator or
//!   on a split edge for a branch, and in the landing block for a check.
//!   Without it, the phi's drop releases the caller's borrow → UAF (the
//!   loop-accumulator `x = base; while …: x = x + base` and the if-arm
//!   `x = a if c else …`).
//! * **Outgoing side (moved roots).** A value MOVED into a phi must NOT also be
//!   released at the join, at the handler, or in a descendant block: the block
//!   arg owns it now and releases it at its own last use, on entry when nothing
//!   reads it, or at its lexical boundary. Liveness reports the forwarded value
//!   dead-in to the target (its successor-side identity is the distinct
//!   block-arg SSA value) while its definition still reaches there, so a
//!   release placed on definition availability alone drops it there AND where
//!   the phi is released → double-free. Custody reports a moved root unowned
//!   until its definition runs again.
//!
//! ## Ownership model (design 20 §1)
//!
//! Every op that returns a new heap reference returns it **owned** (`rc += 1`):
//! the current SSA holder is responsible for exactly one dec-ref before the value
//! goes out of scope. Operands are **borrowed** unless the op takes them: a frame
//! home store or a call that frees its CallArgs builder consumes one, and a
//! source Python call instruction adopts its arguments (`transfers.rs`). So the
//! drop rule is: at a value's last use, the holder releases its ref — unless the
//! last use itself transfers ownership (a Return value, a branch arg passed to a
//! successor block arg, a taking op that takes the root's own +1, or an operand
//! the value-range / repr filter proved carries no heap reference). A taking
//! position that cannot take the root's own +1 is retained right before the op.
//! A frame binding view (a home store's or load's result, or a block argument
//! carrying only views) holds no reference, so it is never released.
//!
//! ## What is dropped
//!
//! `DropEligibility` owns the composed predicate for whether a value root is a
//! drop candidate. A value `v` is eligible when ALL hold:
//! * `v` is heap-carrying (NOT a [`TirLivenessResult::is_raw_scalar`] — raw i64 /
//!   bool / float carriers hold no refcount; dropping them would pass a raw
//!   register to `molt_dec_ref_obj`).
//! * `v` is not a borrowed parameter (the caller owns and drops it). A
//!   parameter whose declared custody is `Transferred` is function-owned and a
//!   Python binding: lexical custody releases it at its frame boundary.
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
//! * **Dead block args** (§1c): an owned block arg that nothing in its block
//!   reads is released on entry, once on every path: joins, handlers and loop
//!   headers alike. A loop header phi is an ordinary block arg; its previous
//!   value dies at its last use, on entry when unread, or on the arc into the
//!   body when only the exit reads it (§2.7).
//! * **Lexical custody**: a Python-bound local and an explicitly released root
//!   keep their objects to a Python boundary rather than their last use, and a
//!   block arg that takes one of them inherits that boundary. The root is
//!   released before a Return it reaches owned on every entry, or on the split
//!   arc into a join it does not reach owned the same way. That includes the
//!   back edge that rebinds a lexical loop phi (CPython's `STORE_FAST` release
//!   on overwrite).
//! * **Exception edges** (§2.6): liveness enters each `CheckException` at the
//!   observation, not at the block terminator. After ordinary placement,
//!   `exception_edges.rs` gives a landing block to each observation whose
//!   normal continuation still needs an owner that the handler path does not
//!   name, or whose payload needs a retained handler arg. The landing retains
//!   and releases on that edge only, then branches to the original handler.
//!   `availability.rs` decides where any RC operation may name a root.
//!
//! ## Suspension points (design 20 §2.9)
//!
//! Terminal lowering exposes `StateYield` and `StateTransition` as explicit
//! state writes, polls, branches, wait registration and ordinary Return exits.
//! Persistence is owned by frame ClosureStore/ClosureLoad, not by a second
//! backend retain/release lane. Each poll invocation releases local owners and
//! transfers its result using the same rules as any other function activation.
//!
//! ## Borrow inference (design 20 §3.2)
//!
//! If `v`'s last use is as a borrowed operand to a `Call` / `CallMethod` /
//! `CallBuiltin` and `v` is dead after the call, the callee borrows `v` for the
//! call's duration and the caller drops at last use — which is exactly the call
//! site. Inserting `DecRef(v)` right after the call is correct and is what the
//! straight-line rule does; a borrowing call needs no IncRef around it. An
//! adopted operand instead moves or is retained (`transfers.rs`). The borrow
//! inference therefore reduces to: drop after the call, never before — which the
//! last-use placement already does. Positive Python named-owner provenance
//! (`bound_local`) or an explicit delete boundary overrides last-use placement
//! even without known finalizer metadata. Mutable classes and opaque values do
//! not prove destruction unobservable. Unmarked expression temporaries still
//! die at their last use; `store_var` alone is not new lexical-owner evidence.
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
//! 2. **Program-point availability** — an RC operation names a root only where
//!    the root's definition reaches on every path and the root's name still owns
//!    its object there. The first includes an exception edge that leaves a block
//!    before the definition (`availability.rs`, over `ProgramPointDominance`).
//!    Block dominance errs both ways. The *full*-CFG tree lets a definition below
//!    a `CheckException` "dominate" its handler. The terminator-only tree ignores
//!    the exception entries of a mixed block, such as the exit that `raise; jump
//!    exit` shares with every check. The second excludes every point that some
//!    path reaches after the root moved into a block argument, such as the
//!    handler argument a check's payload binds, or after an explicit release or
//!    consuming use of it. A release boundary that one entry reaches without the
//!    root moves to the normal arcs that still own it, and landings release it
//!    on exceptional entries. (Otherwise a use-before-def, observed as the LLVM
//!    verifier "Instruction does not dominate all uses!" abort, or a double
//!    release, observed as `invalid object header before dec_ref`.)
//! 3. **Python lifetime release boundaries** — a root released by `DelBoundary` /
//!    `DeleteVar` / pre-existing `DecRef`, statement finalizer release, or
//!    `store_var` scope cleanup is path-authoritative and is never edge-dropped
//!    at a join, nor is a block arg that took such a root's object. The OpsOnly
//!    edge-dying form has one block-entry drop for all incoming paths; adding it
//!    beside a path-conditioned Python rebind/delete or later scope-exit
//!    boundary can release the same local owner twice.
//! 4. **Conditionally-valid iterator results** — an `IterNextUnboxed` value
//!    result (from the generated result-validity table) is initialized ONLY
//!    below its not-done edge; on the exhaustion edge its slot holds stale
//!    bits. Validity is a program-point fact. `OwnershipRootFacts` records the
//!    initialized region: the not-done target, when that edge is its sole
//!    entry. Point-sensitive consumers release or retain the value only inside
//!    that region, never on the exhaustion edge: entry and arc releases, phi
//!    retains and exceptional landings. An arc that does not initialize it
//!    binds no obligation. Consumers that would make it a Python lifetime
//!    authority still exclude it wholesale: `DelBoundary` normalization and
//!    Python-bound local stores.
//! 5. **Activation ownership** — suspension becomes an ordinary Return before
//!    analysis. Resume dispatch does not carry SSA owners from earlier calls;
//!    explicit closure storage owns persistence. Program-point availability
//!    includes dispatch and exceptional entries. High-level yields surviving
//!    frame lowering fail at this boundary instead of inventing frame retains.
//! 6. **Backend conditioning** — drop insertion runs for every target plan
//!    that claims deterministic Python lifetimes (LLVM / WASM / native
//!    Cranelift). Native suppresses its legacy value-tracking RC substrate
//!    on `drop_inserted` functions, so TIR drops are the single RC authority
//!    for activated functions. GC-managed Luau and the Rust and MLIR source
//!    targets claim no such lifetimes and do not activate this pass. See
//!    `pass_manager::target_uses_tir_drop_insertion`.
//!
//! ## Diagnostics
//!
//! `MOLT_DEBUG_DROP=<substr>` (or `=ALL`) writes a per-function dump of the
//! post-insertion block/op shape with per-operand repr tags to
//! `<artifact_root>/drop/<func>.txt`. Activation and handler paths use the same
//! dump and ownership authority as ordinary functions.

mod activation;
mod arcs;
mod audit;
mod availability;
mod exception_edges;
mod exception_region;
mod remap;
mod runner;
mod transfers;
mod util;

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

pub(crate) use self::runner::frame_clear;
pub use self::runner::run;
