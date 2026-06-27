<!--
Design doc — MM-ladder rung 2 (Perceus-style borrow inference + drop/reuse
specialization). The required frontier follow-through to the DropInsertion arc
(design 20, rung 1). DESIGN ONLY; no implementation landed. All file:line anchors
verified against the origin/main worktree at e83f6b07f (2026-06-06). This doc is
numbered 27 because 26 was taken by `26_real-async-generators.md` after this
task was scoped; the canonical short name remains "Perceus borrow inference".
-->

# Perceus-Style Borrow Inference + Drop/Reuse Specialization (Design 27)

**Document status:** Implementation-ready design. **MM-ladder rung 2.**
**Scope:** All refcounting backends — native/Cranelift, LLVM, WASM. Luau is
GC-managed (dup/drop/reuse are no-ops). This is a complete structural arc that
*subsumes and deletes* rung 1's ad-hoc ownership machinery; it is not a patch on
top of it.

**Prerequisite (rung 1, design 20):** the `DropInsertion` pass
(`runtime/molt-passes/src/tir/passes/drop_insertion.rs`, 2715 lines) is landed
and **active on LLVM/WASM/Luau/NativeCranelift**
(`target_uses_tir_drop_insertion`, `pass_manager.rs:67`). Native activation now
uses the same shared terminal drop phase; the remaining native work is the
broader automatic temp-RC/value-tracking deletion proof, not a dormant target
gate. Rung 1 arrived at correctness by accreting **seven distinct
hand-maintained over-release defenses** (enumerated in §0.2). Rung 2 replaces
the *ad-hoc
collection of seven sets/helpers* with **one ownership-typed dataflow** from
which every one of those seven facts falls out as a derived consequence — making
each historical bug class **un-expressible**, not merely avoided.

---

## 0. The binding directive, restated as the engineering target

> *Structural fix = floor; RC elided-to-zero on non-escaping = the target.*

Rung 1 inserts a `DecRef` at every owned temporary's last use and then asks
`refcount_elim` to remove the redundant ones. That is **insert-then-remove**.
The two existing "elide to zero" steps — `refcount_elim` Step 5 (deferred-RC:
non-heap-exposed ⇒ remove IncRef/DecRef, `refcount_elim.rs:577`) and Step 6
(unique-ownership DecRef→Free, `refcount_elim.rs:628`) — are **disabled in the
post-drop mode** (`run_post_drop`, `refcount_elim.rs:154`; the `post_drop` early
return at `refcount_elim.rs:572`) precisely because, after insertion, they cannot
distinguish *the one balancing release* from *a redundant pair*. So today, on a
non-escaping owned temporary, rung 1 emits a real `molt_dec_ref_obj` call (the
runtime fast-paths the inline-tag case, but the boxed case is a true atomic
decrement + branch + maybe-free).

Perceus' contribution is to make the elision a property of the **inference**, not
a post-hoc removal. A value that is **borrowed** (never consumed by a transfer
site, never escapes) receives **no `dup` and no `drop`** in the first place.
"Elided to zero on non-escaping" becomes the *definition* of the Borrowed class,
not an optimization the cleanup pass might or might not prove. This is the
structural floor the directive demands, and it dissolves the Step-5/6 tension:
there is nothing to remove because nothing was inserted.

### 0.1 The MM ladder (where rung 2 sits)

| Rung | Substrate | Status | This doc |
|---|---|---|---|
| 0 | Runtime RC primitives (`dec_ref_ptr`, `molt_dec_ref_obj`, immortal/arena flags), DEALLOC counters, `MOLT_ASSERT_NO_LEAK` | landed (design 20 §5) | — |
| 1 | `DropInsertion` — insert `DecRef` at last use; `refcount_elim` elides redundant pairs | landed; active LLVM/WASM/Luau/Native | prerequisite |
| **2** | **Perceus borrow inference (Owned/Borrowed lattice) + drop specialization + reuse/FBIP** | **this design** | **here** |
| 3 (future) | Reference-cycle collection (the one thing Perceus does NOT give us — see §4.6) | deferred (design 20 §10.1) | non-goal |

### 0.2 The seven over-release classes rung 1 fought, and where each lives today

Each was a real UAF/leak caught in adversarial review rounds 1–7 (design 20
§4.1 Findings #1–#4). Rung 2's correctness bar: **make each un-expressible** in
the new model. The mapping (current code site → §7 gate that catches the
equivalent bug in rung 2):

| # | Class | Current rung-1 defense (file:line) | Rung-2 home |
|---|---|---|---|
| C1 | alias-set ⊉ no-incref lowerings (per-`Copy` double-drop of one object) | alias-root canonicalization via `build_alias_union_find` (`alias_analysis.rs:226`); drop pass operates in root space (`drop_insertion.rs:560`) | the lattice is **per alias-root**, not per SSA value (§1.3); a borrowed alias is `Borrowed` by construction |
| C2 | phi mixed-ownership (borrowed value flows into an owned phi → drop releases caller's borrow) | §5 mixed-ownership retain + `before_term_incref`/`EdgeSplit` (`drop_insertion.rs:1005-1260`) | **phi ownership join** in the lattice (§1.4): `Owned ⊔ Borrowed = Owned`, the retain is the materialized `dup` the join demands |
| C3 | forwarded-arg double-drop (value into phi AND dropped at join) | `incoming_arg_roots` exclusion (`drop_insertion.rs`); transfer roots sourced by `terminator_branch_args` (`ownership_lattice_min.rs`) | **transfer at branch-arg edges** is a lattice move (Owned consumed), §2.4 |
| C4 | interior-borrow / raw-handle lifetime (`Counter._handle`) | `BorrowProvenance` keepalive (`alias_analysis.rs:285`, `build_borrow_provenance:334`); consumed by both liveness and drop pass | **borrow-of edges** in the lattice (§1.5): a borrow result's source is held `Owned`-live through the result's last use |
| C5 | droppable-on-alias-vs-root weave (class-3 unmapped `Copy` is its own root yet not owned) | `OwnershipRootFacts::non_owning_copy_result_roots` (`ownership_lattice_min.rs`) sourced from `classify_copy_kind` (`alias_analysis.rs:515`) and consumed by `drop_insertion.rs` | the **borrow signature** of the op-kind (§2): an unmapped/unknown kind defaults to `Borrowed result`, fail-closed |
| C6 | CallArgs consumed-by-callee (builder freed inside `call_bind`) | generated operand-ownership facts through `op_consumed_operand_root` (`ownership_lattice_min.rs`), consumed by `DropInsertion` | the **consumed-operand column** of the borrow signature (§2.3): `call_bind`'s builder param is `Owned-in` (consumed) |
| C7 | use-scan completeness (IterNextUnboxed value-out uninitialized on exhaustion edge; SSA dominance on exception edges) | generated `[[result_validity]]` rows materialized by `OwnershipLattice` and consumed by `drop_insertion.rs`; TerminatorOnly dominance guard | **conditional-validity** is a lattice attribute (§1.6): the value is `Owned` only on the not-done edge; `MaybeUninit` elsewhere — never droppable on a die-edge |

The thesis of this document: items C1–C7 are seven *symptoms of one missing
abstraction* — a per-program-point ownership type. Build the type once; the seven
defenses become seven readings of it.

---

## 1. The ownership lattice

### 1.1 The carrier (Owned / Borrowed per (alias-root, program-point))

Perceus' linear resource calculus splits the typing environment into an **owned**
context Δ and a **borrowed** context Γ (Reinking–Xie–de Moura–Leijen, *Perceus:
Garbage Free Reference Counting with Reuse*, PLDI 2021, §3 "A Linear Resource
Calculus"; the `dup`/`drop` insertion is the syntax-directed translation `⌈·⌉`
that consults that split). molt's IR is post-SSA TIR (MLIR-style blocks with
arguments), not a lambda calculus, so the translation is reformulated as a
**dataflow over the alias-root graph** rather than a syntactic walk. The carrier:

```
Ownership ∈ { Owned, Borrowed, Raw, MaybeUninit }     -- a lattice value per (root, point)
```

| State | Meaning | Drop obligation |
|---|---|---|
| **Owned(k)** | this root holds exactly `k` net references the function is responsible for releasing (`k≥1`); minted by a fresh-value op, a `+1` runtime return, an explicit `dup`, or a phi ownership-join | release `k` (one `drop` each) before the root's lifetime ends, OR transfer to a consumer |
| **Borrowed** | the root's reference is owned by someone else (a parameter the caller owns; a longer-lived container; an interior borrow) | **none** — releasing it underflows the real owner (the C2/C4 UAF class) |
| **Raw** | the carrier holds no heap reference (inline int47, bool, unboxed f64, `Never`); the repr filter | none — `molt_*_ref_obj` is a runtime no-op on a non-pointer tag, but emitting one is a type error on a raw register |
| **MaybeUninit** | the carrier *may* hold stale/garbage bits on the current path (the `IterNextUnboxed` value-out on the exhaustion edge; C7) | **never droppable** on any path where it is `MaybeUninit` |

`Raw` is the bottom for drop purposes (no obligation). The lattice order used by
the phi join (§1.4) is on the *droppability* axis:

```
            Owned(k)                 -- a real release obligation (top of the drop axis)
              │
           Borrowed                  -- no obligation, but a LIVE reference exists
              │
             Raw                      -- no reference at all
              │
          MaybeUninit                 -- not even a valid reference on this path (bottom)
```

The join is **conservative-toward-Owned for soundness of *no-leak*** but
**conservative-toward-not-dropping for soundness of *no-UAF***; §1.4 makes the
direction precise (it is the crux of C2/C7 and the only subtle part of the
lattice).

### 1.2 Why a four-point lattice and not Perceus' two-point Owned/Borrowed

Perceus assumes a closed functional core where every value is either owned or
borrowed and every constructor field is statically typed. molt carries two facts
Perceus does not:

- **Raw** is forced by molt's NaN-boxed representation lattice (`Repr`,
  `representation_plan.rs`). An `int` that the `overflow_peel`/`ValueRange`
  analyses prove `RawI64Safe` is a bare `i64` register — it is *not* a reference
  the RC calculus may dup/drop. Perceus has no such case (boxed integers are
  heap objects in Koka). This is why the repr filter
  (`raw_i64_safe_values_for`, consumed by liveness at `liveness.rs:47`) is a
  *lattice state*, not a side-condition.
- **MaybeUninit** is forced by molt's `IterNextUnboxed` ABI: the runtime writes
  the value-out slot only on `done=false` and leaves it uninitialized on
  exhaustion (`drop_insertion.rs:483-492`). Perceus' constructors always produce
  a valid value. This conditional validity is a path-sensitive fact the
  two-point lattice cannot carry.

A two-point lattice would force these into "Borrowed" (no obligation) — but
`Borrowed` still asserts *a live reference exists*, which is false for `Raw` (no
reference) and dangerous for `MaybeUninit` (no valid reference). Conflating them
re-creates C7 (a `MaybeUninit` treated as `Borrowed`-live would be kept alive
across the join and a later transfer would read garbage). The four-point lattice
is the minimal carrier that keeps the seven classes un-expressible.

### 1.3 Per alias-root, not per SSA value (subsumes C1)

A molt `Copy`/`TypeGuard`/`guard_tag` produces a *bit-identical alias* of its
operand with **no incref** (`CopyLowering::TransparentAlias`,
`alias_analysis.rs:444`). The loop-carried accumulator is reloaded via
`load_var → Copy` every iteration, so one heap object has many SSA ids. Rung 1
discovered (the hard way, design 20 §4.1 Finding #1) that ownership is a property
of the **alias root**, and built `AliasUnionFind` (`alias_analysis.rs:357`) to
canonicalize. Rung 2 makes this foundational: **the lattice is indexed by alias
root**. `canon(v) = aliases.root(v)` (`drop_insertion.rs:561`). A transparent
alias is *definitionally* `Borrowed` (it shares its root's single obligation); the
root carries the `Owned(k)`. C1 — a second `DecRef` on a `Copy` of an owned
object — cannot be expressed because a non-root alias has no `Owned` state to
release.

The union-find is built by `record_transparent_aliases` (`alias_analysis.rs:381`)
over the `classify_copy_kind` contract (`alias_analysis.rs:515`): `FreshValue`
results get their own root (a real `+1`); `TransparentAlias` results union into
operand 0's root; `InertMarker` results carry no heap reference (`Raw`/no-state).
Current implementation status: conditionally-valid iterator results and
FinalizerSensitive roots are already materialized inside `OwnershipLattice` in
alias-root space. DropInsertion consumes those root facts for placement; it no
longer owns value-space scans or root-folding loops for those two lattice states.
Statement-boundary releases for finalizer-sensitive storage absorption are also
composed in root space by `StatementReleasePlan`, which combines
`OwnershipLattice` boundaries with `PythonLifetimeFacts` and `DropEligibility`;
DropInsertion only materializes the resulting DecRefs.

### 1.4 The phi ownership-join (subsumes C2)

A TIR block argument *is* the SSA phi. Each predecessor edge binds it to a value.
The join of the per-edge ownership states determines the phi's state:

```
join(states_over_incoming_edges) :
    if any edge is Owned                       → Owned   (the phi must own a +1 uniformly)
    elif all edges Borrowed                    → Borrowed
    elif all edges Raw                         → Raw
    -- mixed with MaybeUninit handled in §1.6
```

The critical line is `Owned ⊔ Borrowed = Owned`. Consider `x = base; while …: x =
x + base` (`drop_insertion.rs:1014-1023`): the loop-entry edge binds the
accumulator phi to `Copy(base)` — a `Borrowed` alias of the borrowed parameter
`base` — while the back-edge binds it to `Add(…)`, an `Owned` fresh BigInt. The
join is `Owned`. But an edge that *delivers* a `Borrowed` value into an `Owned`
phi has under-delivered by one reference. **The join obligation is a `dup`
(IncRef) on that edge** — exactly the §5 mixed-ownership retain
(`before_term_incref`, `drop_insertion.rs:1027`). In Perceus terms: the
translation inserts a `dup` wherever a borrowed binding is used in an owned
position (PLDI'21 §3, the `dup` insertion rule for a variable used where the
owned context requires it). Rung 1 hand-derived this as a special case; rung 2
*derives the retain as the materialization of the lattice join* — the IncRef is
not a patch, it is the `dup` the `Owned ⊔ Borrowed` join requires by definition.

The edge-exact placement (before-terminator on a single arc; critical-edge split
on a multi-arc-same-target) is unchanged from rung 1's `EdgeSplit`
(`drop_insertion.rs:394`, applied at `drop_insertion.rs:1064`); it is *where* the
`dup` lands, orthogonal to *whether* the lattice demands it.

### 1.5 Borrow-of edges (subsumes C4)

A borrowing read — `LoadAttr`/`Index` (`op_borrow_source`,
`alias_analysis.rs:272`) — produces a result that may be a borrow *into* its
source object's backing store, or an opaque raw-int handle indexing a registry
keyed off the source (the `Counter._handle` class). The result keeps its source
**`Owned`-live**: the source's `drop` must be deferred to after the result's last
use. This is the `BorrowProvenance` relation (`alias_analysis.rs:285`), a
*one-directional liveness coupling* (NOT a union — unioning would let MemGVN
forward a store on the source to a load of the result, a miscompile;
`alias_analysis.rs:247-250`). In the lattice: a **borrow-of edge** `result →
source_root` extends `source_root`'s `Owned` liveness to dominate `result`'s
last use. Rung 1 threads this through both `compute_liveness` (`liveness.rs:228`)
and the drop pass's in-block last-use scan (`drop_insertion.rs:688`). Rung 2
keeps the relation verbatim but reframes it as the third lattice edge type
(alias-union, phi-join, borrow-of). C4 — dropping the source before the handle
read — cannot be expressed because the borrow-of edge holds the source `Owned`.

### 1.6 Conditional validity (subsumes C7)

`MaybeUninit` is path-sensitive: an `IterNextUnboxed` value-out result is `Owned`
on the not-done (body) edge and `MaybeUninit` on the done (exhaustion) edge. The
lattice rule: **a root that is `MaybeUninit` on any incoming path is never
edge-droppable at that join**, and is droppable on a body path only by the
ordinary straight-line last-use rule (which the exhaustion edge cannot reach).
Today this fact is sourced from `[[result_validity]]` in `op_kinds.toml` and
rendered into `opcode_result_is_conditionally_valid_only_on_edge`;
`OwnershipLattice` materializes the conditionally-valid alias-root set and
DropInsertion consumes that root-space lattice fact for placement. DropInsertion
no longer owns either a duplicate scan, a root-folding loop, or an
`IterNextUnboxed` hand list. In the full lattice, C7 is un-expressible: a
`DecRef` of a `MaybeUninit`-on-this-edge value is rejected by the lattice state.

The exception-edge SSA-dominance hazard (the *other* half of C7,
`drop_insertion.rs:806`) is a **placement** constraint, not a lattice state: a
`DecRef` may only be placed where the value's def terminator-dominates the
insertion point. This stays a placement guard (the TerminatorOnly dominator
tree, `drop_insertion.rs:809-817`) because it is about *codegen SSA validity*,
not ownership. The lattice tells you *whether* to drop; dominance tells you
*where you may legally place* the drop; fail-closed if no legal point dominates
(keep the `+1`, accept a possible leak on that exception path — never a UAF).

### 1.7 The lattice in five lines (the summary the report wants)

```
Ownership(root, point) ∈ {Owned(k≥1), Borrowed, Raw, MaybeUninit}, indexed by ALIAS ROOT.
  Owned: a +k release obligation (fresh op / +1 return / dup / phi-join). Borrowed: a live ref someone else owns (param, container, interior borrow) — NO drop. Raw: no heap ref (repr filter). MaybeUninit: no valid ref on this path (IterNext exhaustion) — never droppable here.
  Three edge types drive the dataflow: alias-union (Copy⇒Borrowed of root), phi-join (Owned ⊔ Borrowed = Owned, materialized as a dup on the borrowed edge), borrow-of (LoadAttr/Index holds its source Owned-live through the result's last use).
  Drop placement = the Owned root's last-use point, MINUS transfers (Return / branch-arg / consumed-operand), GATED by terminator-dominance, FILTERED by Raw/MaybeUninit.
  Elision-to-zero is the DEFINITION of Borrowed (no dup, no drop inserted) — not a post-hoc removal; this is the binding-directive target made structural.
```

---

## 2. Borrow signatures at boundaries (registry integration)

Rung 1's ownership facts are scattered across three hand-maintained tables in
`alias_analysis.rs` — the `FreshValue` allow-list
(`copy_kind_mints_fresh_owned_ref`, `alias_analysis.rs:480`), the `InertMarker`
set (`copy_kind_is_inert_marker_table`), and the no-heap-move set
(`copy_kind_is_explicit_no_heap_move`, `alias_analysis.rs:487`) — plus the
hardcoded operand-borrow assumption (every op borrows its operands except the
consumed-operand query sourced through generated operand-ownership facts in
`ownership_lattice_min.rs`). Design 25 (the op-kind registry,
`docs/design/foundation/25_op_kind_registry.md`) already tables the *kind
vocabulary* and is the home for ownership signatures. Rung 2 adds **ownership
columns** to `op_kinds.toml` so the borrow signature of every op-kind is one
compiler-checked row, not N hand-lists.

### 2.1 The borrow-signature columns (additions to `op_kinds.toml`)

Per design 25 §5, the table row is
`(canonical_kind, aliases[], semantics_class, arity, mapper_opcode, classifier_class, effect, backends_required[], runtime_symbol?)`.
Rung 2 adds:

| New column | Domain | Meaning | Replaces (file:line) |
|---|---|---|---|
| `result_ownership` | `owned` \| `borrowed` \| `raw` \| `alias_of_operand(i)` \| `cond_owned(edge)` | the lattice state of the op's result at definition | `classify_copy_kind` buckets (`alias_analysis.rs:515`); the `FreshValue`/`TransparentAlias`/`InertMarker` enum |
| `operand_ownership[]` | per-operand: `borrowed` \| `consumed` | does the op release this operand internally? | the universal "operands borrowed" assumption + generated `op_consumed_operand_root` (`ownership_lattice_min.rs`) |
| `borrows_source_operand` | `none` \| `operand(i)` | does the result borrow into operand `i`'s backing store (interior borrow)? | `op_borrow_source` (`alias_analysis.rs:272`) |

`result_ownership = alias_of_operand(0)` is the `TransparentAlias` class (union
into operand 0's root). `cond_owned(not_done)` is the `IterNextUnboxed` class
(`Owned` on the not-done edge, `MaybeUninit` elsewhere). `raw` is the
`InertMarker`/repr class.

### 2.2 The +0 borrowed-parameter ABI (the cross-call contract)

molt's compiled-function call convention is **callee borrows all args, returns
owned** (design 20 §1.5; `pass_manager.rs` and the runtime call/bind path). This
is Perceus' *borrowed parameters* optimization (PLDI'21 §2.4 / the Koka
"borrowed parameters" — a parameter the function does not consume is typed
borrowed, eliminating the caller's dup-before-call / callee's drop-at-end pair).
In molt it is already the ABI floor: a parameter is `Borrowed` at function entry
(`param_ids`, `drop_insertion.rs:468`; never dropped). The borrow signature makes
the *call site* symmetric: passing a value as a borrowed arg is **not** a
transfer (no dup, the caller keeps its obligation and drops at the value's true
last use); passing it as a `consumed` operand **is** a transfer (no trailing
drop — C6).

Borrow inference for *result* ownership of a `Call` is the one place rung 2 can do
better than rung 1's "every Call returns Owned" (design 20 §1.2). With the E1
inliner's interprocedural summaries (`ip_summary.rs`, design 03/12) a callee that
provably returns a *borrowed* alias of one of its arguments (an accessor like
`def first(xs): return xs[0]` after inlining is the borrow case, but a
non-inlined `id`-shaped function that returns its argument) could be signed
`result_ownership = alias_of_operand(i)`. **This is deferred to rung 2 Phase 4+
and gated on the inliner's return-alias summary** (`compute_return_alias_summaries`,
deferred in design 20 §4.1 / S4); the default stays the fail-closed `owned` (a
mis-signed borrowed-return would be a *leak* if marked owned — the sanctioned
direction — never a UAF). The table documents the gap.

### 2.3 The consumed-operand class as a signature, not a special case (C6)

`call_bind` / `call_indirect` free their CallArgs builder internally via
`PtrDropGuard` (`call/bind.rs`; documented at `drop_insertion.rs:1395-1413`).
The old drop-pass special case has been collapsed into
`op_consumed_operand_root` (`ownership_lattice_min.rs`), which reads generated
`_original_kind` consume rows and generated first-class opcode operand
ownership. Rung 2 makes it a row: `call_bind: operand_ownership = [borrowed,
consumed]`. The drop pass reads the ownership-module query; the transfer at a
consumed operand is identical to a Return transfer (the op takes ownership; no
trailing drop). Any *future* consuming op (a streaming builder, a
move-into-collection intrinsic) gets correct treatment by adding a column value
— not by editing the drop pass. The sync test
(`tests/test_gen_op_kinds.py`, design 25 §5) makes a missing/wrong signature a
build error.

### 2.4 Transfer sites (the complete set, derived from signatures)

A `drop` is *omitted* at an `Owned` root's last use iff that last use is a
**transfer**. The complete transfer set, each now a signature reading:

- **Return** terminator value (`terminator_uses_root`, `ownership_lattice_min.rs`)
  — the return ABI transfers `+1` to the caller.
- **Branch-arg** into a successor's phi (`terminator_branch_args`,
  `ownership_lattice_min.rs`; the CFG placement exclusion remains
  `incoming_arg_roots` in `drop_insertion.rs`) — transfers into the block param
  (C3).
- **Consumed operand** (`operand_ownership[i] = consumed`, §2.3) — the op frees
  it (C6).
- **Store into a container/cell** is **NOT** a transfer: `StoreAttr`/`StoreIndex`/
  `ClosureStore`/`ModuleSetAttr` all **inc-ref the stored value** (the container
  takes its own ref; design 20 §4.1 Finding #3A audit, runtime evidence
  `object/ops.rs:9246,9273`, `builtins/functions.rs:5324`, `builtins/modules.rs:5126`).
  The caller keeps its obligation and drops at last use. (This is the audited
  resolution of the design 20 §4.1 Finding #2C/#3A scare — the global-store
  convention is borrowed, not transfer.)

---

## 3. Drop specialization + reuse analysis (the Perceus headline)

### 3.1 Drop specialization: `drop(x)` → uniqueness test → free vs decref

Perceus lowers `drop(x)` to (PLDI'21 §2.2, the `drop` definition):

```
drop(x) =  if is-unique(x) then { drop each child of x; free(x) }
           else decref(x)
```

molt's runtime already implements the *fused* form: `dec_ref_ptr`
(`object/mod.rs:1812`) does the `prev == 1` zero-transition test
(`object/mod.rs:1771`, the acquire-fence + type-dispatch + child-decref + free at
`object/mod.rs:2578`), and the Cranelift inline tag-check `emit_dec_ref_obj`
(`simple_backend.rs:1076-1103`) short-circuits the non-pointer case. So molt's
`DecRef` op *is* Perceus' specialized `drop` — the uniqueness test is inside the
runtime call, with the inline tag-check hoisted to codegen. **Rung 2 does not
need a separate specialization pass for the decref-vs-free split**; that is
`refcount_elim` Step 6 (`refcount_elim.rs:628`, the DecRef→Free promotion for
proven-unique values), which exists but is disabled post-drop.

The rung-2 structural fix: **Step 6 becomes sound post-drop once borrow
inference owns the obligation accounting.** Today Step 6 is disabled because it
keys on `build_heap_exposed_set` (`refcount_elim.rs:601`) which cannot see the
drop pass's lone releases. With the ownership lattice, a root that is `Owned(1)`,
non-escaping (the lattice's own escape fact, §3.4), and has its sole `drop` at a
point that dominates no other use **provably** hits the zero-transition — so the
`DecRef` can be a direct `Free` (skip the atomic decrement + branch). This is a
Phase-3 deliverable (§6 P3), gated on the lattice replacing
`build_heap_exposed_set` as the escape oracle.

### 3.2 Reuse / FBIP: `drop`-then-same-size-`alloc` → reuse token

The Perceus reuse rule (PLDI'21 §4 "Reuse Analysis", the `dropreuse`/`@reuse`
primitive): when a `drop(x)` of a constructor of size class `N` is immediately
followed (no aliasing barrier between) by an allocation of size class `N`, the
drop yields a **reuse token** (the data pointer, *if unique*) that the allocation
consumes in place — no free, no malloc. This is the FBIP ("functional but
in-place") pattern: a functional update that allocates a fresh structure and
discards the old one becomes an in-place mutation when the old one is unique.

**molt already has the analysis AND the runtime ABI** (design 20 §10.3 said the
ABI was missing — that is now stale):

- **Analysis:** `reuse_analysis::analyze` (`reuse_analysis.rs:160`) scans for
  `DecRef(x)`→`Alloc` pairs with `reuse_compatible` size classes
  (`reuse_analysis.rs:132`), barrier-bounded by `AliasAnalysisResult::is_barrier_for`
  (`alias_analysis.rs:932`), and annotates `reuse_token_id`/`reuse_from_token`
  attrs (`reuse_analysis.rs:265`).
- **Runtime ABI:** `molt_reuse_token(bits)` (`object/builders.rs:46`) returns the
  data pointer iff unique (`ref_count == 1`, `object/builders.rs:70`), excluding
  immortal (`object/builders.rs:59`) and arena (`object/builders.rs:64`) objects;
  `molt_reuse_alloc(token, size_bits)` (`object/builders.rs:93`) reuses if the
  existing allocation is large enough (`object/builders.rs:108`), else frees and
  mallocs. Both are GIL-Relaxed-safe (`object/builders.rs:68-69`).

**What is missing is the LOWERING and a pipeline-ordering fix:**

1. **Ordering defect (must fix first — perf-correctness gap).**
   `reuse_analysis` runs at pipeline slot ~11 (`pass_manager.rs:351`, `ReadOnly`)
   — **before** `drop_insertion` at slot 29 (`pass_manager.rs:455`). It scans for
   `DecRef`→`Alloc` pairs, but on the production path the DecRefs do not exist
   until `drop_insertion` runs. So `reuse_analysis` **currently finds almost
   nothing in the real pipeline** (only pre-existing round-trip DecRefs from
   `lower_to_simple`). The annotations it produces are vestigial. Rung 2 P3 moves
   reuse_analysis to run **after** `drop_insertion` (and after `refcount_elim_post`,
   so it sees the elided-down DecRef set). This is a pure ordering correction — no
   new analysis logic — and it is exactly the kind of "perf step skipped, come
   back later" gap the binding directive forbids leaving.
2. **Lowering (the headline new code):** a `DecRef` carrying `reuse_token_id=N`
   lowers to `tok_N = molt_reuse_token(x)` *instead of* `molt_dec_ref_obj(x)`;
   the paired `Alloc` carrying `reuse_from_token=N` lowers to
   `molt_reuse_alloc(tok_N, size)` *instead of* `molt_alloc(size)`. Per backend:
   native (`function_compiler.rs` / `simple_backend.rs`), LLVM
   (`llvm_backend/lowering.rs`), WASM (`tir/lower_to_wasm.rs`). The token is a raw
   `u64` flowing as an ordinary SSA value between the two ops (no RC on the token
   itself — it is `Raw`).

### 3.3 Which molt shapes qualify (the bench corpus, precisely)

The classic Perceus/FBIP win is functional `map`/`filter` reusing cons cells. molt
has no cons-cell lists; its allocators differ. The **honest** qualification, per
shape (anti-overclaiming, per the binding directive's "asymmetric coverage"
warning):

| Shape | Source | Per-iteration alloc | Reuse-eligible? | Win |
|---|---|---|---|---|
| BigInt accumulator `total = total + i` (large `n`) | `bench_fib` (recursive BigInt at large args), the design-20 1M-iter repro | one fresh BigInt per iter; old BigInt `drop`'d on back-edge | **YES** — same size class `TirType::BigInt` (`reuse_analysis.rs:89`); old and new BigInt are size-compatible; old is unique (sole owner is the loop) | **FBIP**: malloc+free per iter → in-place reuse; the headline 3418-alloc→9MiB-RSS class becomes O(1) allocations |
| String concat `s = s + x` | `bench_string` (via `",".join` the loop builds `parts`; the `s = s + x` shape in the design-20 30M repro) | one fresh `str` per iter (length grows!) | **PARTIAL** — same size *class* only while length stays within the class; growing strings cross size classes, so `molt_reuse_alloc` falls through to free+malloc (`object/builders.rs:120`) once the new string is larger. Reuse fires for the *steady-state-length* sub-case | bounded; the leak is already closed by rung 1's drop, reuse removes the malloc churn for same-length rebuilds |
| `lst.append(i)` loop | `bench_list` | **none** — the list *persists* across the loop; `append` mutates in place (amortized realloc), the element `int` is stored (borrowed-then-incref'd) | **NO reuse** (nothing is dropped per iter) | the win here is **elision-to-zero** of the element temp's RC (§0/P2), not reuse |
| `d[i] = i` loop | `bench_dict` | **none** — the dict persists; `i` is stored | **NO reuse** | elision-to-zero of the stored temp's RC |
| List comprehension `[x*2 for x in range(n)]` | design-20 corpus | the result list persists; per-element `x*2` is `Raw` (int) | reuse N/A; elements `Raw` | already zero (Raw filter) |

**Conclusion for §3:** the reuse/FBIP win is concentrated on the **per-iteration
freshen-and-discard** shapes (BigInt accumulators, same-length string rebuilds),
*not* the persistent-container append loops (where the win is RC elision, §0/P2).
The doc must not claim list-append FBIP — `lst.append` frees nothing per
iteration. This precision is the difference between an honest perf projection and
the overclaim the directive forbids.

### 3.4 The escape fact = the lattice, replacing `build_heap_exposed_set`

`refcount_elim` Steps 5/6 and reuse all need "does this value escape?".
Three oracles answer it today, inconsistently:

- `refcount_heap_exposure_opcodes` (`op_kinds.toml`, consumed by `refcount_elim.rs`) — opcode list.
- `build_heap_exposed_set` (`refcount_elim.rs:89`) — operand scan + Return.
- `escape_analysis.rs` (the SROA-enabling escape pass).

Rung 2 unifies the *RC-relevant* escape fact into the lattice: a root **escapes**
iff it is the operand of a non-borrowing store/build/call-capture/return/yield —
which is exactly "its obligation is transferred or shared". A `Borrowed` root
never escapes (someone else owns it); an `Owned` root escapes iff it is consumed
by a transfer or stored-with-incref into something that outlives it. This makes
the deferred-RC Step 5 *expressible post-drop* (P3): a non-escaping `Owned(1)`
root whose drop is its only release can have the drop elided to a `Free` (Step 6)
or, if reuse-paired, to a reuse token (§3.2). The directive's "elide to zero on
non-escaping" is this unification.

---

## 4. Uniqueness/refcount-1 reuse vs CPython parity

Reuse changes *when memory is reclaimed* and *which allocation backs an object* —
it can change object identity timelines. The verified-subset claim: **within
molt's verified subset, the reuse rewrite is unobservable.** Enumerate the
observable surface and the gating conditions (fail-closed where parity is at
risk).

### 4.1 `id()` and object identity

`id(x)` returns the object's address (NaN-box pointer). Reuse fires only when the
old object is **unique** (`ref_count == 1`, `object/builders.rs:70`) — i.e., no
other live reference exists, so no live `id()` of the old object can be compared.
The reused allocation gets a fresh logical object (header re-initialized,
`object/builders.rs:112-117`); its `id()` *may equal* the old object's `id()`
(same address). CPython makes the same guarantee in reverse: `id()` values may be
reused after an object is freed (CPython docs: "two objects with non-overlapping
lifetimes may have the same id()"). Since the old object is dead (unique +
dropped) before the new one is born, their lifetimes do not overlap — **identical
to CPython's `id`-reuse semantics.** No gating needed; this is *more* CPython-like
than a non-reusing allocator (which might hand out a different address).

### 4.2 `__del__` / finalizer ordering (the gating condition)

This is the parity-critical surface. CPython runs `__del__` at the
ref-count-zero transition, in a defined order. molt's `dec_ref_ptr` runs the
finalizer at `prev == 1` (`maybe_run_object_finalizer`, near `object/mod.rs:1883`).
The reuse rewrite replaces `drop(old)` + `alloc(new)` with `tok =
reuse_token(old)` + `reuse_alloc(tok, …)`. **Critical:** `molt_reuse_token`
returns the pointer *without running the finalizer* (`object/builders.rs:70-73`
returns the data pointer; it does **not** call `dec_ref_ptr`). If `old` has a
`__del__`, skipping the finalizer is an **observable parity break** — CPython
would run `__del__(old)` before the new object exists.

**Gating condition (fail-closed):** reuse is permitted **only for object types
that have no Python-level finalizer** — the runtime types `BigInt`, `str`,
`bytes`, `tuple`, `list`, `dict`, `set` (the `reuse_compatible` set,
`reuse_analysis.rs:132`) which have *no* `__del__` (their teardown is the runtime
child-decref + free, which `molt_reuse_alloc` must still honor for child refs —
see §4.3). A `UserClass` instance (`TirType::UserClass`, `reuse_analysis.rs:106`)
**may have `__del__`** and is therefore **excluded from reuse at the analysis
level** unless the class is statically proven to have no `__del__`/`__del__`-bearing
base (a class-hierarchy fact available from the frontend's `class_info`). **The
conservative rung-2 rule: reuse only the finalizer-free builtin container/number
types; exclude all `UserClass` reuse in Phase 3, gate `UserClass` reuse on a
proven-no-`__del__` summary in a later phase.** This is the single most important
parity gate in the design.

### 4.3 Child references on reuse (the deep-correctness condition)

A `list`/`dict`/`tuple`/`set` owns references to its elements. When the old
container is dropped, its children are decref'd (the type-dispatch teardown,
`object/mod.rs:2578` region). `molt_reuse_token` returns the pointer *without*
decref'ing children. So the reuse lowering MUST ensure the children are released
*before* the slab is reused — OR the reuse must only apply to containers whose
children were already accounted for. **The current `molt_reuse_alloc` zeroes the
payload (`object/builders.rs:117`) but does NOT decref the old children** — so
reusing a non-empty container slab would **leak the children**. Gating condition:
reuse of a container is sound only when the container is **empty or its children
are independently dropped** at the reuse point. For the BigInt/str/bytes case
(no child references — the payload is inline bytes/limbs) this is automatically
satisfied; **for list/dict/set/tuple, the reuse lowering must emit child-decrefs
or restrict to empty containers.** The honest rung-2 Phase 3 scope: **reuse the
no-child types (BigInt, str, bytes) first** (the accumulator/concat wins, which
are exactly the bench targets); list/dict/set/tuple reuse needs the child-decref
extension to `molt_reuse_token` (a `molt_reuse_token_drop_children` variant) and
lands in a later phase. Marking this clearly avoids the C4-adjacent leak.

### 4.4 `weakref` and callbacks

A `weakref` to `old` must be invalidated (and its callback run) when `old` dies.
CPython runs weakref callbacks at the zero transition. `molt_reuse_token` skips
the teardown, so a live weakref to a reused object would observe the *new* object
through the *old* weakref — a parity break. **Gating condition:** the unique
check (`ref_count == 1`) does **not** account for weakrefs (weakrefs are
non-owning and do not bump the strong count). Therefore reuse of any object that
**could have a weakref** is unsound unless the runtime clears weakrefs in the
reuse path. The finalizer-free builtin types in §4.2 — does molt permit weakrefs
to `list`/`dict`/`str`? In CPython, `str`/`int`/`tuple` do **not** support
weakrefs; `list`/`dict`/`set` **do**. So the §4.3 restriction (reuse only the
no-child types BigInt/str/bytes first) *also* resolves the weakref hazard for the
Phase-3 set: **BigInt/str/bytes are not weakref-able** (parity with CPython int/str),
so no weakref can observe their reuse. list/dict/set reuse (later phase) must
clear weakrefs in `molt_reuse_token` — folded into the same child-decref
extension as §4.3.

### 4.5 Finalizer resurrection

CPython allows `__del__` to resurrect an object (store `self` somewhere, bumping
the refcount back above zero). Reuse only fires for finalizer-free types (§4.2),
which cannot resurrect. The `UserClass`-with-`__del__` exclusion (§4.2) covers
the resurrection surface entirely. No additional gate.

### 4.6 The cycle-leak constraint (Perceus' one limitation, inherited)

Perceus is **garbage-free for acyclic data** but **leaks reference cycles**
(PLDI'21 §1 / the explicit limitation; molt inherits this, design 20 §10.1).
Reuse does not change this — a cyclic object never reaches `ref_count == 1`
(the cycle holds it ≥1), so `molt_reuse_token` returns 0 (no reuse) and the
object leaks exactly as it does without reuse. This is the *correct* fail-closed
behavior; reuse neither helps nor worsens cycles. The cycle collector is rung 3
(out of scope).

### 4.7 Parity verdict

| Surface | Observable under reuse? | Gate |
|---|---|---|
| `id()` | No (unique ⇒ non-overlapping lifetimes; matches CPython id-reuse) | none |
| `__del__` ordering | **Yes for `UserClass`** | exclude `UserClass` reuse (§4.2) |
| child refs | **Yes for containers** | reuse no-child types only (BigInt/str/bytes) in P3 (§4.3) |
| `weakref` | **Yes for list/dict/set** | no-child types are non-weakref-able ⇒ safe in P3; container reuse later (§4.4) |
| resurrection | covered by `UserClass` exclusion | none extra |
| cycles | No (cycle ⇒ refcount>1 ⇒ no reuse) | inherent fail-closed |

**Net:** Phase-3 reuse restricted to **BigInt/str/bytes** is provably unobservable
within the verified subset, and captures the headline accumulator/concat wins.
Container reuse (list/dict/set/tuple) is a clean later phase gated on the
child-decref + weakref-clear extension to `molt_reuse_token`.

---

## 5. Cross-backend story

The inference runs entirely on TIR (backend-agnostic), exactly like rung 1's
`DropInsertion`. The lattice, the `dup`/`drop` placement, and the reuse-token
annotation are computed once; each backend lowers the resulting ops.

### 5.1 What each backend already has (rung 1 inheritance)

| Op | Native/Cranelift | LLVM | WASM | Luau |
|---|---|---|---|---|
| `DecRef` | `emit_dec_ref_obj` inline tag-check (`simple_backend.rs:1076`); round-trips via `lower_to_simple.rs:1903` | `llvm_backend/lowering.rs:1275` | wired `tir/lower_to_wasm.rs` (design 20 §4.3) | no-op (GC) |
| `IncRef` | `emit_inc_ref_obj` | wired | wired | no-op |
| activation | **active** (legacy-RC deletion still gated by full ownership-surface proof) | **active** | **active** | **active** |

### 5.2 What rung 2 adds per backend (the reuse-token lowering)

The only *new* per-backend code is the reuse-token lowering (§3.2). A `DecRef`
with `reuse_token_id=N` and the paired `Alloc` with `reuse_from_token=N`:

- **Native/Cranelift** (`function_compiler.rs` / `simple_backend.rs`): emit
  `call molt_reuse_token(x) → tok` for the marked DecRef; thread `tok` as an
  i64 SSA value to the marked Alloc; emit `call molt_reuse_alloc(tok, size)`.
  The token value is `Raw` (no RC). New: two `match op.kind` arms keyed on the
  reuse attrs.
- **LLVM** (`llvm_backend/lowering.rs`): dedicated lowering arms for the
  reuse-attr'd DecRef/Alloc → `molt_reuse_token`/`molt_reuse_alloc` calls. The
  LLVM `Copy`-arm fail-loud gate (`lowering.rs:2348`) is unaffected (reuse ops
  are real `DecRef`/`Alloc`, not `Copy`).
- **WASM** (`tir/lower_to_wasm.rs`): emit `call $molt_reuse_token` /
  `call $molt_reuse_alloc` imports (add the two imports if absent).
- **Luau**: reuse is a **no-op** — emit the plain `Alloc` (Luau's GC handles
  reclamation; there is no manual free to fuse). The reuse annotations are
  ignored on the Luau target, mirroring how `DecRef`/`IncRef` are no-ops there.

### 5.3 The dormant/marker mechanism (mirroring rung 1's discipline)

Rung 1's staged rollout uses (a) the `drop_inserted` function attr
(`DROP_INSERTED_ATTR`, `drop_insertion.rs:167`) to make the pass idempotent and
to gate native's parallel value-tracking RC, and (b) the
`target_uses_tir_drop_insertion` per-target gate (`pass_manager.rs:67`). Rung 2
mirrors both:

- **Borrow inference (P1)** is a *refactor* of the existing active
  `DropInsertion` — it changes *how* the same ops are derived (from the lattice
  rather than the seven sets), with **byte-identical RC output** as the P1 gate.
  It therefore inherits rung 1's activation state exactly (active
  LLVM/WASM/Luau/Native) and adds no new target gate. The native
  ownership-surface sweep is *more likely to clear* under P1 because the unified
  lattice
  removes the cross-set inconsistencies that produced the batch-sensitive
  miscompile.
- **Reuse lowering (P3)** is gated by a new per-target predicate
  `target_uses_tir_reuse(target)` (default: the same set drop insertion is active
  on) and a `reuse_lowered` marker, so reuse can roll out per-backend
  independently of drop activation. Reuse stays **dormant** (annotations produced
  but not lowered) until the per-backend lowering is verified leak-clean.

---

## 6. Phasing as complete pieces (no-stopgap)

Each phase is a complete structural piece with its own gates; none leaves the
tree with two parallel sources of truth. The unit of work is the complete
structural change (CLAUDE.md). The verification protocol per phase follows the
rung-1 *activation protocol*: differential sweep (`tests/differential/basic`,
native AND LLVM) + RSS table (`safe_run.py --rss-mb`) + per-bench perf table +
`MOLT_ASSERT_NO_LEAK=1` gate + the honesty oracle (verdicts via `cmp -s` against
CPython, NEVER `rtk diff` which lies — design 20 workflow lessons).

| Phase | Scope | Deletes (file:line) | Gate (must all pass) |
|---|---|---|---|
| **P1 — Lattice replaces the seven sets** | Introduce `Ownership` lattice + the three edge types (alias-union/phi-join/borrow-of) as the single computation feeding `DropInsertion`. Re-derive every placement (straight-line drop, edge-dying, phi-retain, suspension-IncRef, transfer exclusions) from the lattice. | The *ad-hoc derivations* in `drop_insertion.rs`: remaining placement-local ownership sets become `lattice(root) == Owned` or explicit lattice states. Borrowed parameter roots, stack/no-RC roots, C5 non-owning `Copy` roots, and generated `[[result_validity]]` conditionally-valid result roots are already sourced by `OwnershipRootFacts`; `DropEligibility` now composes those roots with the liveness-owned raw-scalar roots, so DropInsertion no longer owns the scattered `droppable` predicate. Python-bound local, named-slot, local-store, explicit-release root facts, the composed boundary-release root set, statement-release eligibility, and return-boundary deferral classification are sourced by `PythonLifetimeFacts`, with local-store boundary releases routed through `PythonLifetimeFacts::boundary_release_roots`. FinalizerSensitive roots, generated result-absorption ownership, statement-release finalizer boundaries, generated terminator transfer roots, and generated consumed-operand roots are sourced by `OwnershipLattice`/the ownership module. Raw-scalar production still comes from `TirLivenessResult`/the representation lattice until the Raw state is folded deliberately. `BorrowProvenance`/`AliasUnionFind` plus generated terminator/operand transfer queries are **kept** (they are the edge sources) but their *consumers* unify. | **Byte-identical RC output** vs current `DropInsertion` on the full differential corpus (native AND LLVM): `cmp -s` the emitted TIR DecRef/IncRef set per function + binary output. Backend lib tests green. `MOLT_ASSERT_NO_LEAK=1` clean. 0 new warnings (`cargo test`, not just `build`). The seven historical repros (design 20 Findings #1–#4) stay green. |
| **P2 — Elision-to-zero on non-escaping** | The lattice's escape fact (§3.4) replaces `build_heap_exposed_set`. A `Borrowed` root gets no dup/drop (already true); make `Owned(1)` non-escaping roots whose drop is their sole release elide the `DecRef` entirely where the value is **dead with no observable release semantics** (no `__del__`-bearing type) — and promote the surviving sole-release `DecRef` to `Free` (re-enable Step 6 post-drop, soundly). | `refcount_elim` Step 5's `build_heap_exposed_set` consumer (:601) and the `post_drop` early-return that disables Step 6 (:572) — both replaced by the lattice escape fact. `is_heap_exposing` (:61) retires (folded into signatures). | Per-bench perf table showing the boxed-temp DecRef count drops to zero on non-escaping shapes (`bench_fib` intermediate temps; the design-20 accumulator). `bench_sum` (Raw lane) unchanged (zero ops, the contract). RSS bounded (`MOLT_ASSERT_NO_LEAK=1`). Native AND LLVM byte-identical to CPython. **Performance contract: faster-than-CPython on every bench, every target, every profile** (CLAUDE.md). |
| **P3 — Reuse/FBIP lowering** | (a) Move `reuse_analysis` to run **after** `drop_insertion`+`refcount_elim_post` (the ordering fix, §3.2). (b) Lower `reuse_token_id`/`reuse_from_token` to `molt_reuse_token`/`molt_reuse_alloc` per backend (§5.2). (c) Restrict reuse to **BigInt/str/bytes** (no-child, non-weakref-able, finalizer-free — §4.7); exclude `UserClass` and containers. | The vestigial early `reuse_analysis` slot (`pass_manager.rs:351`) moves; no deletion, a re-order. | **Alloc-count evidence**: `MOLT_PROFILE=1` alloc_count on the BigInt accumulator drops from O(n) to O(1) (the FBIP win). Same-length string-rebuild malloc churn eliminated. **No reuse fires for `UserClass`/containers** (asserted — the §4 parity gate). `__del__`/weakref differential tests byte-identical. `MOLT_ASSERT_NO_LEAK=1` clean (the §4.3 child-leak gate: BigInt/str/bytes have no children, so zero leak). RSS table per bench. |
| **P4 — Registry-column migration** | Move the borrow signatures (§2.1: `result_ownership`, `result_validity`, `operand_ownership[]`, `borrows_source_operand`) into `op_kinds.toml` (design 25) and generate the classifier. The lattice reads generated columns instead of hand lists. *(Optionally: the inliner-gated borrowed-return signatures, §2.2, if the return-alias summary has landed.)* | The hand-maintained `copy_kind_mints_fresh_owned_ref` table, `copy_kind_is_inert_marker_table`, `copy_kind_is_explicit_no_heap_move`, and remaining result-ownership lists are generated from the table. The `classifier_silent_fallthrough` hazard is closed: every kind gets an explicit `result_ownership`. The `IterNextUnboxed` value-out validity fact is already generated via `[[result_validity]]`. | The design-25 sync test (`tests/test_gen_op_kinds.py`) green: drift = build error. **Byte-identical codegen** vs P3 (the columns mirror current reality exactly). `audit_op_kinds.py --check` clean against the baseline. |

Current P1 shrinkage: `StatementReleasePlan` now owns the statement-release
map composition for finalizer-sensitive storage boundaries. `DropInsertion`
must not rebuild `statement_release_after_op`/`statement_released_roots`; it
only consumes `StatementReleasePlan::after_op` and
`StatementReleasePlan::contains_released_root` while inserting the DecRefs.

**Phase independence:** P1 ships the unified inference (byte-identical, the
correctness floor). P2 ships the perf win (elision-to-zero). P3 ships the FBIP
win (reuse). P4 ships the drift-proofing (registry). Each is independently
valuable and independently revertible. P1 must land before P2/P3 (they consume
the lattice's escape fact); P4 may land any time after P1 (it is a source-of-truth
move, byte-identical). **No phase may be split across sessions as "half now, half
later"** — if P1 cannot complete in a session, leave a baton-pass note and land
nothing (CLAUDE.md).

---

## 7. Risks + adversarial pre-mortem (the round-8+ lenses)

For each of the seven historical classes (§0.2), where the equivalent bug would
hide in *this* design and which gate catches it. Plus new risks rung 2 introduces.

### 7.1 The seven classes — where they hide now, and the catching gate

| # | Where the equivalent bug would hide in rung 2 | Gate that catches it |
|---|---|---|
| C1 (per-Copy double-drop) | If the lattice were indexed by SSA value instead of alias root, a `Copy` would get an independent `Owned` state. | The lattice is **defined** per `canon(v)` root (§1.3); a non-root has no `Owned` state. P1 gate: `loop_slot_accumulator_no_double_drop` (`drop_insertion.rs:1503`) — no two DecRefs share a root. |
| C2 (phi mixed-ownership) | If the join used `Owned ⊔ Borrowed = Borrowed` (no retain). | The join is `Owned ⊔ Borrowed = Owned` with a materialized `dup` (§1.4); P1 gate: the `apply(base, n)` / `x = a if c else fresh()` repros (design 20 §5). |
| C3 (forwarded-arg double-drop) | If a branch-arg transfer were not excluded from edge-dying. | Transfer is a lattice consume (§2.4); the `incoming_arg_roots` dual exclusion. P1 gate: the inliner `x = a+a; return x+a` repro (`drop_insertion.rs:851-853`). |
| C4 (interior-borrow lifetime) | If a borrow-of edge were dropped (source freed before handle read). | Borrow-of holds source `Owned`-live (§1.5); P1 gate: `len(Counter(...))` returns correct count (design 20 round-6 BLOCKER-1). |
| C5 (unmapped-Copy droppability) | If an unknown kind defaulted to `Owned result`. | `result_ownership` fail-closes to `borrowed`/`alias_of_operand(0)` for unknown kinds (§2.1, the `_ => TransparentAlias` rail, `alias_analysis.rs:544`); P4 makes it an explicit column. **Leak-not-UAF** is the only failure direction. |
| C6 (CallArgs consumed) | If `operand_ownership` missed a consuming op. | The `consumed` column (§2.3); P1 gate: `call_bind_callargs_operand_not_dropped` (`drop_insertion.rs` test). Adding a consuming op without the column → the op double-frees → caught by `MOLT_ASSERT_NO_LEAK` + the abort. P4 makes it compiler-checked. |
| C7 (use-scan completeness) | If `MaybeUninit` were conflated with `Borrowed`, or a drop placed where the def doesn't dominate. | `MaybeUninit` is a distinct lattice state, never droppable on its edge (§1.6); the TerminatorOnly dominance placement guard (§1.6, `drop_insertion.rs:806`). P1 gate: `list(gen)`/`"".join(gen)` exhaustion repros. |

### 7.2 New risks rung 2 introduces

- **R-reuse-1 (the `__del__` parity break, §4.2) — the highest-severity new
  risk.** A `UserClass` with `__del__` reused would skip the finalizer. *Gate:*
  reuse restricted to BigInt/str/bytes in P3 (no `UserClass`, no `__del__`);
  a differential test asserting `__del__` runs at the same point with/without
  reuse; an assertion in `reuse_analysis` that the size class is one of the
  finalizer-free types. **Round-8 lens:** grep every `reuse_compatible` true
  result and confirm it is in {BigInt, Str, Bytes} for P3; any `UserClass`/
  container match is a P3 bug.
- **R-reuse-2 (child-ref leak, §4.3).** Reusing a non-empty container slab leaks
  children. *Gate:* P3 excludes containers (no-child types only); `MOLT_ASSERT_NO_LEAK`
  catches any container reuse leak. **Round-8 lens:** confirm `molt_reuse_token`
  is never reached for a type with child references in P3.
- **R-reuse-3 (weakref staleness, §4.4).** *Gate:* the no-child P3 types are
  non-weakref-able (CPython parity); container reuse deferred. **Round-8 lens:**
  a `weakref.ref(x)` differential on a reused-type value.
- **R-order-1 (the reuse-before-drop ordering defect, §3.2).** Already present on
  main (reuse_analysis runs before drop_insertion → finds nothing). *Gate:* P3
  moves it after; a test asserting reuse candidates are non-empty on the BigInt
  accumulator post-reorder. **Round-8 lens:** confirm `reuse_analysis` slot is
  after `refcount_elim_post`.
- **R-elide-1 (P2 over-elision).** Re-enabling Step 6 (DecRef→Free) post-drop is
  the exact unsoundness `run_post_drop` was created to avoid (`refcount_elim.rs:572`).
  P2 must prove the lattice escape fact is *sound* where `build_heap_exposed_set`
  was *necessary*. *Gate:* byte-identical RC behavior on the full corpus + the
  design-20 leak repros under `MOLT_ASSERT_NO_LEAK`; a Free that should have been
  a DecRef (shared object) is an immediate UAF the corpus catches. **Round-8
  lens:** every DecRef→Free promotion must have a lattice proof of `Owned(1)` +
  non-escaping + sole-release; spot-check the promotions against `escape_analysis`.
- **R-native-1 (the legacy-RC deletion sweep, inherited).** Native is active on
  the shared terminal drop path, but P1's unification does not automatically
  prove that every broader automatic temp-RC/value-tracking lane is redundant.
  *Gate:* do not delete those native legacy lanes until `import typing`/
  `warnings`/`re`/`collections`/`asyncio` are clean wired under
  `MOLT_ASSERT_NO_LEAK` (native AND LLVM) across the full corpus and no new
  wired-fail/dormant-pass delta appears. **Round-8 lens:** P1's deliverable is
  unified inference; native legacy deletion is a consequence to verify, not a
  phase scope.
- **R-matches-1 (the design-25 `matches!`-default-false trap, instance #1).**
  Any new op-kind added without a `result_ownership`/`operand_ownership` signature
  defaults silently. *Gate:* P4's table has **no default** — every kind needs an
  explicit ownership class; the sync test makes omission a build error. Until P4,
  the fail-closed `_ => TransparentAlias`/`borrowed` rail holds (leak-not-UAF).

### 7.3 The review lenses for round-8+

1. **Lattice totality:** every TIR op-kind maps to exactly one
   `result_ownership` and a per-operand `operand_ownership`; the `_ =>` rail is
   `borrowed`/`alias_of_operand(0)` (fail-closed). No `Owned` default anywhere.
2. **Transfer completeness:** the transfer set (§2.4) is exactly
   {Return, branch-arg, consumed-operand}; stores are NOT transfers (they
   incref). Any new transfer site is a lattice consume, not a drop-pass special
   case.
3. **Reuse parity gate:** P3 reuse is BigInt/str/bytes only; assert no
   `UserClass`/container reuse; `__del__`/weakref differentials byte-identical.
4. **Elision soundness:** every DecRef→Free / elided-DecRef has a lattice proof
   (`Owned(1)` + non-escaping + sole-release + finalizer-free); `MOLT_ASSERT_NO_LEAK`
   green; faster-than-CPython on every bench/target/profile.
5. **Byte-identity discipline:** verdicts via `cmp -s`, never `rtk diff`;
   `cargo test` (not `build`) for warnings; kill the session daemon after each
   build; clear `stdlib_shared_*` cache before each build (design 20 workflow
   lessons).

---

## 8. File / modification map

| File | Action | Key change |
|---|---|---|
| `runtime/molt-passes/src/tir/passes/drop_insertion.rs` | Modify (P1) | Replace the seven ad-hoc sets/predicates with `Ownership` lattice reads; `droppable` → `lattice(root)==Owned`; keep alias-union/borrow-provenance/consumed-operand as the lattice edge sources |
| `runtime/molt-passes/src/tir/passes/alias_analysis.rs` | Modify (P1/P4) | `CopyLowering`/`copy_kind_*` become the `result_ownership` reading; P4 generates them from `op_kinds.toml` |
| `runtime/molt-passes/src/tir/passes/refcount_elim.rs` | Modify (P2) | Replace `build_heap_exposed_set` (:89) with the lattice escape fact; re-enable Step 6 post-drop soundly (:572 early-return removed under the lattice proof) |
| `runtime/molt-passes/src/tir/passes/reuse_analysis.rs` | Modify (P3) | Restrict `reuse_compatible` to {BigInt, Str, Bytes} for P3 (the parity gate, §4.7); exclude `UserClass`/containers |
| `runtime/molt-passes/src/tir/pass_manager.rs` | Modify (P3) | Move `reuse_analysis` (:351) to AFTER `refcount_elim_post` (:465); add `target_uses_tir_reuse` gate |
| `runtime/molt-backend/src/native_backend/{function_compiler,simple_backend}.rs` | Modify (P3) | Lower reuse-attr'd DecRef/Alloc → `molt_reuse_token`/`molt_reuse_alloc` |
| `runtime/molt-backend/src/llvm_backend/lowering.rs` | Modify (P3) | Reuse-token lowering arms |
| `runtime/molt-tir/src/tir/lower_to_wasm.rs` | Modify (P3) | `molt_reuse_token`/`molt_reuse_alloc` import + lowering |
| `runtime/molt-runtime/src/object/builders.rs` | Modify (P3-later) | `molt_reuse_token` child-decref + weakref-clear variant (for container reuse, post-P3) |
| `runtime/molt-ir/src/tir/op_kinds.toml` | Modify (P4) | Add `result_ownership` / `operand_ownership[]` / `borrows_source_operand` columns |
| `tools/gen_op_kinds.py`, `tests/test_gen_op_kinds.py` | Modify (P4) | Generate + sync-test the ownership columns |
| `tests/differential/memory/*.py` | Extend (P2/P3) | Alloc-count + `__del__`/weakref reuse-parity regressions |

---

## 9. Key file anchors (verified against origin/main e83f6b07f, 2026-06-06)

- DropInsertion pass + the seven defenses: `runtime/molt-passes/src/tir/passes/drop_insertion.rs` (run at :403; alias-root canon :560; borrowed parameter roots, stack/no-RC roots, C5 non-owning `Copy` roots, and generated `[[result_validity]]` conditionally-valid result roots sourced through `OwnershipRootFacts`; composed droppable/root/raw decision sourced through `DropEligibility`; Python local/slot/release roots, boundary-release root composition, statement-release eligibility, and return-boundary deferral classification sourced through `PythonLifetimeFacts`, including `PythonLifetimeFacts::boundary_release_roots`; result-absorption, generated consumed-operand roots, generated terminator transfer roots, and FinalizerSensitive roots sourced through `OwnershipLattice`/the ownership module; raw scalar production still sourced through `TirLivenessResult`; §5 retain :1005; transfer exclusion `incoming_arg_roots` remains CFG placement; dominance guard :806)
- Statement-release plan authority: `runtime/molt-passes/src/tir/passes/ownership_lattice_min.rs` (`StatementReleasePlan`) composes `OwnershipLattice::statement_release_finalizer_boundaries`, `PythonLifetimeFacts::is_statement_release_boundary_root`, and `DropEligibility`; `runtime/molt-passes/src/tir/passes/drop_insertion.rs` consumes the plan and owns only DecRef materialization.
- Alias/borrow machinery: `runtime/molt-passes/src/tir/passes/alias_analysis.rs` (`build_alias_union_find` :226; `BorrowProvenance` :285, `build_borrow_provenance` :334; `op_borrow_source` :272; `CopyLowering` :444; `copy_kind_mints_fresh_owned_ref` :480; `classify_copy_kind` :515; `is_rc_barrier` :917, `is_barrier_for` :932)
- Liveness (repr-filtered, root-space, borrow-keepalive): `runtime/molt-passes/src/tir/passes/liveness.rs` (`raw_i64_safe_values_for` import :47; `live_out_of` :197; `last_use_in_block` :98)
- refcount_elim (the elision-to-zero machinery): `runtime/molt-passes/src/tir/passes/refcount_elim.rs` (`is_heap_exposing` :61; `build_heap_exposed_set` :89; `run` :130; `run_post_drop` :154; `post_drop` early-return :572; Step 5 :577; Step 6 :628)
- reuse_analysis (Perceus reuse, annotation-only today): `runtime/molt-passes/src/tir/passes/reuse_analysis.rs` (`analyze` :160; `reuse_compatible` :132; `size_class` :77; `annotate` :265; `run` :293)
- Reuse runtime ABI (exists; design-20 §10.3 was stale): `runtime/molt-runtime/src/object/builders.rs` (`molt_reuse_token` :46, unique check :70, immortal :59, arena :64; `molt_reuse_alloc` :93, size check :108, payload zero :117)
- Size classes / allocator: `runtime/molt-runtime/src/object/mod.rs` (`size_class_for` :768; `total_size_from_header_fields` :731; `HEADER_FLAG_IMMORTAL` :444; alloc births :1155,:1228; dealloc zero-transition :1812, finalizer near :1883)
- Pipeline + gates: `runtime/molt-passes/src/tir/pass_manager.rs` (`target_uses_tir_drop_insertion` :67; `reuse_analysis` slot :351; `refcount_elim` :348; `drop_insertion` :455; `refcount_elim_post` :465; pinned pass-name list :682-701)
- Repr lattice (the Raw filter source): `runtime/molt-tir/src/representation_plan.rs`
- Registry (signature home): `docs/design/foundation/25_op_kind_registry.md`; `runtime/molt-ir/src/tir/op_kinds.toml`; `tools/audit_op_kinds.py`
- Rung 1 design: `docs/design/foundation/20_rc-ownership-drop-insertion.md` (the seven Findings #1–#4 in §4.1)

---

## 10. Non-goals (explicit)

- **Reference-cycle collection** — Perceus leaks cycles; molt inherits this
  (§4.6, design 20 §10.1). Rung 3.
- **Container (list/dict/set/tuple) reuse** — needs the `molt_reuse_token`
  child-decref + weakref-clear extension (§4.3/§4.4); a clean phase *after* P3.
- **Interprocedural borrowed-return signatures** — gated on the inliner's
  `compute_return_alias_summaries` (§2.2, deferred in design 20 §4.1 / S4); P4+.
- **Native legacy-RC deletion** — a *consequence* to verify (R-native-1), gated
  on the convergence sweep; not a rung-2 phase scope. Do not delete the broader
  native automatic temp-RC/value-tracking lanes as part of this arc.
