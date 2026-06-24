<!-- Foundation blueprint 55. Arc: P0 MEMORY SAFETY via the OWNERSHIP LATTICE
(resurrection / finalizer / weakref corruption made structurally impossible).
Author: portfolio-architect. Date: 2026-06-24. Status: DESIGN ONLY / EXECUTABLE PLAN.
This doc ADVANCES and COMPOSES with docs 20, 27, 45, 48, 49, 50 and the live
ownership_lattice_min.rs / drop_insertion.rs / refcount_elim.rs / escape_analysis.rs;
it never duplicates or contradicts them. Council Operating Doctrine (CLAUDE.md,
2026-06-08, binding) is the governing authority; this is its full executable form. -->

# 55 — Memory Safety via the Ownership Lattice (the P0 keystone)

## 0. END-STATE (stated crisply)

**A finalizer / resurrection / weakref interaction can no longer produce memory
corruption, a use-after-free, a double-free, or a phantom-no-leak — because the
ownership lattice makes the *placement and lowering decisions that would cause
them unrepresentable in the IR*, not merely unreached at runtime.**

Concretely, in the 5-to-100-year end-state:

1. Every heap object with a non-trivial lifetime boundary (`MayFinalize`,
   `HasWeakrefs`, `MayResurrect`, `InnerRefOrdering`) is released through a
   **finalizer-aware `DecRef`** whose *placement* is the object's
   **Python-visible lifetime boundary**, never its SSA last-read. The compiler
   cannot emit an early release for such an object because the lattice gates the
   placement on a generated fact, and the gate fails closed.
2. `Free` (direct dealloc, bypassing `maybe_run_object_finalizer` /
   `weakref_clear_for_ptr` / inner-ref ordering) is **only** emittable for a
   proven-unique `DecRef` on an object that is provably **none** of the four
   boundary classes. It is a *backend lowering* of a `DecRef`, demoted from an
   independent opcode, and it is unrepresentable for any Python heap object that
   could finalize, resurrect, expose a weakref, or own ordered inner refs.
3. Runtime-internal, finalizer-free frees (arena slabs, transient scratch Vecs,
   ABI shuttle buffers) use a **separate opcode** (`FreeRaw`) that shares no code
   path, no eligibility predicate, and no audit counter with "free a Python
   object." The two can never be confused by a future refactor.
4. `MOLT_ASSERT_NO_LEAK` asserts **actual destruction** (DEALLOC_COUNT reaching
   the allocation high-water minus survivors), not a zero net RC transition, so
   resurrection cannot launder a leak into a green gate.
5. The four boundary facts plus FinalizerSensitive are **one** ClassInfo / MRO /
   class-version-derived cached fact-plane, consumed identically by escape
   analysis, refcount-elim, stack-allocation, Free-eligibility, and
   ownership-lowering. There is no second finalizer-reasoning authority anywhere
   in the pass pipeline or any backend.

The CLASS this retires (the compression-ladder unit, per CLAUDE.md): **"a
lifetime-significant object released at the wrong place, in the wrong way, or
counted as destroyed when it was not."** Not the one SIGSEGV — the entire family
of resurrection/finalizer/weakref corruption bugs, across all four backends.

This arc **outranks all performance and feature work** (CLAUDE.md P0 ranking: "a
resurrection/finalizer/weakref MEMORY-CORRUPTION bug … OUTRANKS the native RC
flip and all performance/feature work — it invalidates trust in the memory
model"). Lane A (P0 semantic safety) of the three-lane model.

---

## 1. Time-traveler derivation (end-state → required facts)

Working backward from the END-STATE to the structural facts that make it
inevitable. Each fact is tied to the bug-class it makes unexpressible.

### 1.1 Why a *lattice*, not more DropInsertion special-cases

The live `drop_insertion.rs` is 8,413 lines (STRUCTURAL_AUDIT_BOARD.md:36, a
`medium` god-file over the 4,000 ceiling). It already carries **seven**
date-stamped "Current invariant (2026-06-xx)" patches for finalizer/transfer
edge cases (doc 20 §2.5, lines 181-234): `_load_collections_abc`,
`collections.namedtuple`, `store_var_rebind_epoch`, `origin_carrier_liveness`,
etc. Each was a real corruption (`invalid object header before dec_ref`) fixed by
a *placement special-case*. That is the exact "compound interest of bugs" the
constitution forbids: every new finalizer composition adds another special-case,
and the *next* composition exposes the gap between them.

The council's binding resolution (CLAUDE.md): build the **minimal OWNERSHIP
LATTICE** — `alias-root → ownership state → Python lifetime boundary → ordered
release obligation` — and ship ordering/safety *on it*. The lattice already
exists in embryonic form (`ownership_lattice_min.rs`, 1,618 lines) and already
backs #58 finalizer *ordering*. This arc **completes** that lattice into the full
memory-safety substrate: it adds the three missing boundary facts (weakref,
resurrection, inner-ref ordering) to the one already present (finalizer), unifies
them into a single cached fact-plane, and rewires Free-eligibility and the
resurrection runtime contract to consume them.

The structural test for "is this a lattice rung or a DropInsertion hack?": **a
rung is a monotone fact over alias-roots computed once and consumed by ≥2
passes; a hack is a placement decision made inside `drop_insertion::run` keyed on
an op-shape.** Every mechanism below is a rung.

### 1.2 The five-rung lattice (target shape)

```
rung 0  alias-root            canonical owning value (AliasUnionFind::root)
                              [EXISTS: alias_analysis.rs build_alias_union_find]
rung 1  ownership state       Owned | Borrowed | StackOnly | NonOwningAlias
                              [EXISTS in pieces: OwnershipRootFacts in
                               ownership_lattice_min.rs — borrowed_parameter_roots,
                               stack_value_roots, non_owning_copy_result_roots]
rung 2  LIFETIME CLASS        the boundary lattice (THE NEW WORK):
   ¬Trivial = MayFinalize ∨ HasWeakrefs ∨ MayResurrect ∨ InnerRefOrdering
                              [EXISTS: MayFinalize only, as
                               finalizer_sensitive_roots; ADD the other three]
rung 3  release boundary      where the ordered DecRef must land:
   SSA-last-use (Trivial) | Python-lifetime-boundary (¬Trivial)
                              [EXISTS: PythonLifetimeFacts::boundary_release_roots,
                               gated on is_finalizer_sensitive_root — GENERALIZE
                               the gate to ¬Trivial]
rung 4  lowering obligation   finalizer-aware DecRef (¬Trivial)
                              | Free-eligible DecRef (Trivial ∧ ProvenUnique)
                              [Free demotion + FreeRaw split: THE NEW WORK]
```

The crucial observation: **rungs 0, 1, 3 already exist and are correct; rung 2
exists only for `MayFinalize`; rung 4 is a defense-in-depth guard, not yet the
authority.** This arc is therefore *narrow and structural*: generalize the rung-2
fact from one boundary class to four, and make rung 4 the sole authority. It is
emphatically NOT a rewrite of drop_insertion.

### 1.3 Fact → class-retired table

| Structural fact (rung 2/4) | Where it lives | Class it makes unexpressible |
|---|---|---|
| `MayFinalize(class)` — class/MRO/version cached `__del__` reachability | already `HEADER_FLAG_CLASS_HAS_FINALIZER`; lift to a TIR `LifetimeClassFacts` row | `__del__` skipped or run at wrong time (doc 48/50 #58) |
| `HasWeakrefs(class)` — class supports weakref OR any live `weakref.ref`/`proxy`/`WeakValueDictionary` targets an instance | NEW `HEADER_FLAG_CLASS_SUPPORTS_WEAKREF` + TIR fact | weakref left dangling past free; `Free` skipping `weakref_clear_for_ptr` (object/mod.rs:2173) → UAF on later weakref deref |
| `MayResurrect(class)` — `MayFinalize` ∧ `__del__` can re-root self (conservatively = `MayFinalize`) | derived from `MayFinalize` | dealloc counted before resurrection check → phantom-no-leak; Free bypassing the rc 0→1 abort (object/mod.rs:1949-1953) → free of a resurrected object |
| `InnerRefOrdering(class)` — object owns inner refs whose release order is observable (`__del__` reads a field, or finalizer order across a container) | derived: `MayFinalize` ∧ `HEADER_FLAG_HAS_PTRS` | field freed before `__del__` reads it; out-of-order container teardown |
| `Free = lowering of DecRef under ¬¬Trivial ∧ ProvenUnique` | refcount_elim.rs Step 6 → consult `LifetimeClassFacts` | direct dealloc that skips finalizer/weakref/resurrection/ordering for a Python object |
| `FreeRaw` separate opcode | NEW OpCode + lower_to_* arms | runtime-internal free sharing a path with Python-object free |
| `MOLT_ASSERT_NO_LEAK = DEALLOC_COUNT reaches target` | object/mod.rs profile counters | resurrection laundering a leak into a green gate |

### 1.4 Why these four boundary classes are *complete* (no fifth)

CPython's `Py_DECREF`→0 path does exactly four lifetime-significant things beyond
the byte free: (a) `tp_finalize`/`tp_del` (`MayFinalize`/`MayResurrect`); (b)
`PyObject_ClearWeakRefs` (`HasWeakrefs`); (c) `tp_clear`/`tp_traverse` ordered
subobject release (`InnerRefOrdering`); (d) `tp_dealloc` itself. molt's
`dec_ref_ptr` (object/mod.rs:1980-2180) mirrors this: rooted-exception
resurrection (2052-2062), `maybe_run_object_finalizer` (2161, → resurrection
abort), `weakref_clear_for_ptr` (2173), then the per-`type_id` inner-ref
release. The four boundary facts are exactly the *non-(d)* obligations. There is
no fifth because (d) is unconditional and carries no ordering/resurrection
hazard. This completeness is what lets us prove `Free` (which does only (d)) is
sound iff all four facts are false.

---

## 2. The structural mechanisms (each tied to its class)

### 2.1 Mechanism M1 — `LifetimeClassFacts`: the one cached fact-plane (rung 2)

**The single authority** for all four boundary classes, derived from
ClassInfo/MRO/class-version, cached, and consumed by every pass and backend that
makes a lifetime/placement/Free decision. This is the council's
"`FinalizerSensitive` = one ClassInfo/MRO/version-derived cached fact" mandate,
**generalized from one class to four** so the same single-authority discipline
covers weakref/resurrection/ordering.

Two layers, one source of truth:

- **Runtime/class layer** (`runtime/molt-runtime/src/object/mod.rs`): the
  existing per-class header flags are the cached, version-refreshed source.
  - `HEADER_FLAG_CLASS_HAS_FINALIZER` (mod.rs:472 region) — EXISTS.
  - `HEADER_FLAG_CLASS_SUPPORTS_WEAKREF` — NEW, set when a class's `__slots__`
    omits `__weakref__` suppression / the type allows weakrefs (CPython
    `tp_weaklistoffset != 0`). Refreshed on the same MRO/version-change hook that
    refreshes the finalizer flag (mod.rs:1443 region: `object_set_class_bits`).
  - Resurrection and inner-ref-ordering are **derived**, not stored:
    `MayResurrect ⊇ MayFinalize` (any `__del__` can `self`-root);
    `InnerRefOrdering = MayFinalize ∧ HEADER_FLAG_HAS_PTRS` (doc 49: ptr fields +
    a `__del__` that can observe them).

- **TIR fact layer** (`runtime/molt-tir/src/tir/passes/ownership_lattice_min.rs`):
  a new `LifetimeClassFacts` struct, computed once per function from the frontend
  `defines_del` result attr (already preserved through the SimpleIR round-trip —
  doc 48 status; escape_analysis.rs `op_result_defines_del`) **plus** two new
  result attrs the frontend emits on allocation ops:
  `class_supports_weakref: bool` and `class_has_ptr_fields: bool`. These mirror
  the runtime flags at compile time so the placement decision is made from a
  fact, not re-derived per-pass.

```rust
// ownership_lattice_min.rs — generalizes finalizer_alloc_roots into the
// full boundary lattice. ONE fact, four predicates.
#[derive(Clone, Debug, Default)]
pub(crate) struct LifetimeClassFacts {
    may_finalize_roots: HashSet<ValueId>,   // = today's finalizer_alloc_roots ∪ container closure
    has_weakref_roots: HashSet<ValueId>,    // NEW
    // MayResurrect and InnerRefOrdering are computed predicates over the above:
    has_ptr_field_roots: HashSet<ValueId>,  // NEW (for InnerRefOrdering)
}
impl LifetimeClassFacts {
    pub(crate) fn is_trivial_lifetime_root(&self, root: ValueId) -> bool {
        !self.may_finalize_roots.contains(&root)
            && !self.has_weakref_roots.contains(&root)
        // MayResurrect ⊆ MayFinalize; InnerRefOrdering ⊆ MayFinalize → already covered.
    }
    pub(crate) fn may_finalize_root(&self, r: ValueId) -> bool { self.may_finalize_roots.contains(&r) }
    pub(crate) fn has_weakref_root(&self, r: ValueId) -> bool { self.has_weakref_roots.contains(&r) }
    pub(crate) fn may_resurrect_root(&self, r: ValueId) -> bool { self.may_finalize_root(r) }
    pub(crate) fn inner_ref_ordering_root(&self, r: ValueId) -> bool {
        self.may_finalize_root(r) && self.has_ptr_field_roots.contains(&r)
    }
}
```

The existing `OwnershipLattice::finalizer_sensitive_roots` (the container-absorption
fixpoint, ownership_lattice_min.rs:564-683) is **reused verbatim** as the
`may_finalize` seed-and-closure; `has_weakref` and `has_ptr_field` flow through
the *same* container-absorption fixpoint (a list holding a weakref-able object is
itself weakref-significant for release-ordering purposes). This is one fixpoint,
four bitsets — not four analyses.

**Class retired:** "a pass re-derives finalizer/weakref reasoning and gets it
subtly different from another pass." The fact is computed once; every consumer
reads `LifetimeClassFacts`.

**Fail-closed rule (binding):** an allocation op whose class facts are *unknown*
(no `defines_del`/`class_supports_weakref` attr, e.g. a dynamically-typed
`Call` result) is treated as `¬Trivial` (all four facts conservatively TRUE). The
generated `op_kinds.toml` classifier already does this for `non_owning_copy`
(ownership_lattice_min.rs:282-307, "fail closed"); extend the same posture to
lifetime class. Over-conservatism costs a finalizer-aware DecRef (a tag-check
branch); under-conservatism costs a UAF. The constitution's choice is forced.

### 2.2 Mechanism M2 — rung-3 boundary generalization (placement)

Today `PythonLifetimeFacts::boundary_release_roots` (ownership_lattice_min.rs:495-510)
gates Python-lifetime-boundary placement on `is_finalizer_sensitive_root`.
**Generalize the gate to `¬is_trivial_lifetime_root`** so weakref-significant and
inner-ref-ordered objects also defer to the Python boundary, not SSA-last-use.

The change is exactly one predicate swap plus threading `LifetimeClassFacts` in
place of the narrower `OwnershipLattice` finalizer query. The seven dated
"Current invariant" transfer/epoch special-cases in drop_insertion.rs (§1.1)
**remain valid and unchanged** — they govern *how* a boundary release is placed
across block-arg transfers; M2 only widens *which* roots are boundary-deferred.
This is the structural-vs-hack line: M2 adds a fact-plane input; it does not add
a placement special-case.

> **VERIFIED OPEN SUB-CASE (2026-06-24, measured).** Beyond *which* roots defer
> (the M2 gate widening), there is a placement *gap*: the boundary release is
> missing on the **exception-transfer edge** even for an already-finalizer-
> sensitive root. `tests/differential/memory/resurrect_during_exception_unwind.py`
> fails today — molt prints `box_len 0` vs CPython `1`. This is **fail-closed
> (a dropped finalizer = LEAK), NOT corruption**: measured under
> `MOLT_ASSERT_NO_LEAK` with 0 SIGSEGV / 0 UAF / 0 double-free (the memory-
> corruption axis is GREEN on all 6 resurrection/finalizer repros; this is the
> lone parity+leak failure). Root cause: a frame-local `obj = R()`
> (`MayFinalize ∧ MayResurrect`) that dies on a `raise` with no local handler gets
> its finalizer-aware `DecRef` placed on the *normal-flow* last-use
> (`drop_insertion.rs:1130-1135`); the exception-transfer edge leaves the block
> first, and `exception_arcs_for_block` (`:1099-1106`, `:1468-1499`) inserts
> retain/release only for *phi-edge payloads into handler blocks* — never the
> dying frame-local's finalizer release on the exception-transfer-to-**EXIT** arc.
> **Fix:** rung-3 placement must emit `boundary_release_roots` on EVERY exit edge,
> *including the exception-transfer-to-exit arc*, not only normal-flow + phi-handler
> edges. This is the "not released at all on the exception edge" instance of §0's
> "lifetime-significant object released at the wrong place." The repro is a live
> (un-suppressed) differential gate — it flips green when this lands; do NOT
> suppress it.
>
> **Re-verified GREEN post-decomposition (2026-06-24, `ac391f8e6`).** After the
> molt-tir crate extraction moved `drop_insertion.rs` + `ownership_lattice_min.rs`
> into `runtime/molt-tir/src/tir/passes/`, the corruption suite was re-run
> dual-path — canonical `molt diff` (dev / debug-with-asserts profile) plus direct
> `safe_run` with `MOLT_ASSERT_NO_LEAK=1` (bypassing the xfail overlay to observe
> raw process exit) — across the full 15-repro suite (`tests/differential/memory/
> resurrect_*` + `finalizer_*`, `basic/finalizer_*`; the suite grew from the 6
> above): **0 SIGSEGV / 0 UAF / 0 double-free / 0 leak-abort / 0 OOM / 0 hang**,
> including the at-scale N1000 / 2000-iter loop-stress / 500K-iter exception-loop
> cases that historically exposed the IC SIGSEGV. The crate-boundary move preserved
> the memory model; `resurrect_during_exception_unwind` remains the sole
> FAIL_CLOSED (this sub-case), unchanged. (`resurrect_with_weakref` +
> `resurrect_with_exception_in_del` are known non-corruption xfails — clean exit 0.)

**Class retired:** "a weakref-significant or field-ordered object released early
(at SSA-last-read) so a weakref deref or a `__del__` field read dangles."

### 2.3 Mechanism M3 — `Free` demotion (rung 4, the council mandate)

Council-binding (CLAUDE.md): "`Free` is demoted. For Python heap objects it is a
backend/runtime LOWERING of a proven-unique DecRef only under `¬MayFinalize ∧
¬HasWeakrefs ∧ ¬MayResurrect ∧ ¬InnerRefOrdering ∧ ProvenUnique`; otherwise the
only legal op is finalizer-aware DecRef."

Today refcount_elim.rs Step 6 (lines 621-718) promotes `DecRef → Free` under
`ProvenUnique` (alloc-rooted, balance 0, `¬heap_exposed`) with a **finalizer-only**
guard (`!finalizer_roots.contains(&val)`, line 704; the guard rationale at
626-659 explicitly notes it is "defense-in-depth … `alloc_vals` keys on
`Alloc`/`StackAlloc` … finalizer roots are `ObjectNewBound` … disjoint by
construction"). That disjointness is *fragile* (the comment itself warns "keeps
it true if a finalizer alloc ever reaches this set"), and it covers only one of
the four classes.

**M3 replaces the single-class guard with the full boundary gate:**

```rust
// refcount_elim.rs Step 6, the promotion predicate (line ~701):
let lifetime = LifetimeClassFacts::compute(func, &aliases);   // the ONE fact
// ...
if *balance == 0
    && alloc_vals.contains(&val)
    && !heap_exposed.contains(&val)
    && lifetime.is_trivial_lifetime_root(aliases.root(val))   // ALL FOUR classes false
{
    op.opcode = OpCode::Free;   // now provably safe: skips only (d)
}
```

`Free` semantics are simultaneously *narrowed in the type system*: it is no
longer a peer of `DecRef` in the ownership table (doc 20 §1.2, line 47, currently
"`Free` | Takes-ownership (frees unconditionally)"). It is re-documented as **"a
backend lowering of a proven-`Trivial`-unique `DecRef`"** and the backends lower
it to the *finalizer-free tail* of `dec_ref_ptr` (object/mod.rs after line 2173:
`weakref_clear_for_ptr` + per-`type_id` inner-ref release + byte free), which for
a `Trivial` object is provably equivalent to the full path minus the
no-op finalizer/weakref/resurrection checks.

**Class retired:** "a direct dealloc that skips `maybe_run_object_finalizer` /
`weakref_clear_for_ptr` / the resurrection abort / ordered inner-ref release for
an object that needed one of them" — the exact resurrection-at-scale SIGSEGV
class.

### 2.4 Mechanism M4 — `FreeRaw`: the separate runtime-internal opcode

Council-binding: "Runtime-internal finalizer-free frees get a SEPARATE opcode
(`FreeInternal`/`FreeRaw`) — never share with 'free Python object.'"

Today there is exactly one `Free` opcode (ops.rs:100) used for both intents. M4
adds `OpCode::FreeRaw` (the council's `FreeRaw` spelling) for runtime-internal,
never-a-Python-object frees: arena slabs, transient `Vec<u64>` scratch (e.g.
`release_dealloc_tracked_bits_vec`, object/mod.rs:1957), ABI shuttle buffers,
the `lower_to_simple` round-trip's own scratch. `FreeRaw`:

- has its **own** `op_kinds.toml` row (effects: side-effecting, never-throws,
  no lifetime-class consultation);
- lowers to `std::alloc::dealloc` / `Box::from_raw` drop **directly**, with no
  `dec_ref_ptr` entry, no finalizer dispatch, no weakref clear, no audit counter
  shared with object dealloc;
- is **never** produced by refcount_elim Step 6 (which only ever emits the
  Python-object `Free`);
- is verified by a `verify.rs` rule: a `FreeRaw` operand must NOT be a value with
  any `LifetimeClassFacts` membership (fail-closed: a `FreeRaw` of a possibly-Python
  value is a verifier error, not a silent miscompile).

**Class retired:** "a future refactor routes a runtime-internal free through the
Python-object free path (or vice-versa) because they shared an opcode" — the
confusion is made unrepresentable by the type system.

### 2.5 Mechanism M5 — `MOLT_ASSERT_NO_LEAK` = actual destruction

Council-binding: "`MOLT_ASSERT_NO_LEAK` = actual destruction (not zero-transition)."

The runtime already separates the two correctly: `DEALLOC_COUNT` is bumped only
*after* the resurrection check passes (object/mod.rs:2164-2172, with the explicit
rationale at 2069-2080: "Counting the dealloc here would over-count destructions
and make `live = alloc - dealloc` UNDER-count live objects — an unsound leak
gauge under resurrection"). M5 makes the **assertion** consume this correctly:
`MOLT_ASSERT_NO_LEAK` asserts `DEALLOC_COUNT == ALLOC_COUNT − <statically-known
survivors>` (true destruction), NOT `sum(rc deltas) == 0` (which a resurrection
+ re-drop satisfies while the object is still alive at the assertion point).

This is largely a *verification-harness* change (the runtime counters are already
correct), but it is load-bearing: without it, every gate below could pass on a
resurrection-laundered leak. M5 lands FIRST (Phase 0) so every later phase's
gate is trustworthy.

**Class retired:** "a leak gauge that reports green because resurrection
re-incremented the count, hiding a real leak."

---

## 3. Phases (dependency order; each independently landable with green gates)

Lane assignment (CLAUDE.md three-lane model): **all of this is Lane A** (P0
semantic safety). It blocks Lane B (perf) only where memory unsafety makes perf
numbers untrustworthy — i.e., Phase 3 (Free demotion) gates any Free-related perf
claim. Phases compose with the decomposition program (§5).

### Phase 0 — `MOLT_ASSERT_NO_LEAK` = actual destruction (M5). [Lane A, ~1 day]

The trust foundation. Make the gate honest before relying on it.

- **Files:** the leak-assertion path that reads `DEALLOC_COUNT`/`ALLOC_COUNT`
  (object/mod.rs profile counters; the `MOLT_ASSERT_NO_LEAK` reader). Compute
  the static survivor set (module-level immortals, interned constants) so the
  target is exact.
- **Gate (G0):**
  - `tests/differential/memory/finalizer_resurrection_leak_gauge.py` flips from
    fail-closed xfail (doc 48 status) to **must-pass under destruction
    semantics**: a program that resurrects N times then drops must report
    `live==0` only when truly destroyed.
  - `MOLT_ASSERT_NO_LEAK=1 python3 tools/safe_run.py --rss-mb 1024 --timeout 180
    -- python3 -m molt run tests/differential/memory/resurrect_once_N1000.py
    --target native --release --rebuild` — asserts destruction, not transition.
  - Negative test: a deliberately-leaking microbench must now FAIL the assertion
    (proving the gauge detects real leaks).
- **Why first:** every subsequent phase's acceptance uses this gauge.

### Phase 1 — `LifetimeClassFacts` fact-plane (M1). [Lane A, ~3-4 days]

The one cached authority. No placement/lowering behavior changes yet — this phase
only *computes and exposes* the four facts and proves them against CPython class
semantics.

- **Files:**
  - `runtime/molt-runtime/src/object/mod.rs`: add `HEADER_FLAG_CLASS_SUPPORTS_WEAKREF`,
    refresh it on the MRO/version hook beside the finalizer flag (mod.rs:1443
    region), copy to instance flag on `object_set_class_bits`.
  - `src/molt/frontend/`: emit `class_supports_weakref` and `class_has_ptr_fields`
    result attrs on allocation ops, alongside the existing `defines_del` attr
    (frontend visitors/classes.py — already a target of the decomposition F1).
  - `runtime/molt-tir/src/tir/passes/ownership_lattice_min.rs`: add
    `LifetimeClassFacts` (§2.1); thread the two new seed sets through the existing
    container-absorption fixpoint (lines 564-683); keep `finalizer_sensitive_roots`
    as the `may_finalize` view (no behavior change to #58).
  - `runtime/molt-tir/src/tir/op_kinds.toml` + `tools/gen_op_kinds.py`: a
    `lifetime_class` classifier set so the facts are generated, not hand-matched
    (STRUCTURAL_AUDIT_BOARD deletion-candidate discipline; avoids a new
    `matches!` semantic-fallthrough).
- **Gate (G1):**
  - `cargo test -p molt-backend --lib --features native-backend
    ownership_lattice_min` — extend the existing 20+ unit tests
    (ownership_lattice_min.rs:835-1617) with weakref/ptr-field/resurrection
    fixtures mirroring the finalizer fixtures (`container_absorbing_*`,
    `nested_container_propagates`).
  - `tools/gen_op_kinds.py --check` green (the fact is generated).
  - A parity probe: `LifetimeClassFacts.has_weakref_root` must agree with CPython
    `cls.__weakref__`-availability for the matrix classes in
    `tests/differential/basic/dict_subclass_slots_weakref*.py`.
  - **No placement change:** byte-identical TIR/artifacts on the full differential
    corpus (this phase is fact-only). `structural_audit.py --check` does not
    regress (no new hand-classified match).

### Phase 2 — rung-3 boundary generalization (M2). [Lane A, ~2-3 days]

Widen Python-lifetime-boundary placement from `MayFinalize` to `¬Trivial`.

- **Files:** `ownership_lattice_min.rs` (`boundary_release_roots` gate →
  `¬is_trivial_lifetime_root`); `drop_insertion.rs` (thread `LifetimeClassFacts`
  where it currently queries `OwnershipLattice::is_finalizer_sensitive_root`,
  lines 1398-1411, 2495, 2578, 2912). The seven dated transfer/epoch invariants
  are unchanged.
- **Gate (G2):**
  - `tests/differential/memory/resurrect_with_weakref.py` and
    `resurrect_with_field_refs.py` flip to must-pass (weakref/field objects now
    deferred to the Python boundary).
  - `tests/differential/basic/finalizer_object_attr_release.py`,
    `finalizer_rebind_alias_lifetime.py`, `object_finalizer_dict_class_lifetime.py`
    stay green (no #58 regression).
  - `MOLT_ASSERT_NO_LEAK=1` (Phase-0 honest gauge) on the weakref/field repros:
    true destruction at the Python boundary.
  - Native + LLVM agree (doc 45/48 backend-parity discipline); WASM/Luau checked
    lowering green or documented gap.

### Phase 3 — `Free` demotion (M3). [Lane A; gates Lane B Free-perf claims; ~2-3 days]

The keystone safety lowering. `Free` becomes a `Trivial`-unique `DecRef` lowering.

- **Files:** `refcount_elim.rs` Step 6 (lines 621-718): replace the
  finalizer-only guard with `is_trivial_lifetime_root` (§2.3); update the Step-6
  FinalizerSensitive guard tests (lines 1566-1700) to cover all four classes.
  Doc 20 §1.2 line 47 ownership-table row re-documented. Backend `Free` lowerings
  (native simple_backend, llvm_backend/lowering, lower_to_wasm, luau) audited to
  confirm they lower to the finalizer-free *tail*, not a path that could re-enter
  finalizer dispatch.
- **Gate (G3):**
  - `cargo test -p molt-backend --lib --features "native-backend llvm
    luau-backend wasm-backend" refcount_elim` — extend the Step-6 guard tests:
    "a weakref/resurrect/inner-ref-ordered DecRef must NEVER be promoted to Free"
    (mirroring the existing finalizer guard test at line 1624-1666).
  - `tests/differential/memory/resurrect_in_loop_stress.py`,
    `resurrect_during_exception_unwind.py`, `resurrect_subclass_inherits_del.py`,
    `resurrect_then_final_drop.py` must-pass under all four backends.
  - **The SIGSEGV repro** (resurrection-at-scale): build under
    `tools/safe_run.py --rss-mb 2048 --timeout 30` and confirm no SIGSEGV, exit
    0, `live==0`, across native/LLVM (WASM/Luau as available).
  - **Perf (Lane B unblock):** confirm `Free` still fires on `Trivial`-unique
    hot temporaries (bench_struct, the pointer-field heap-free microbench doc 50
    §acceptance requires) — Free-elimination perf is *preserved* for the common
    `Trivial` case; report the CPython/PyPy/Codon matrix per the Performance
    Constitution. A regression here means the `Trivial` predicate is too
    conservative (a missing fact), to be fixed structurally, not by widening
    Free.

### Phase 4 — `FreeRaw` separation (M4). [Lane A, ~2 days]

Make the runtime-internal-vs-Python-object free distinction unrepresentable-to-confuse.

- **Files:** `runtime/molt-tir/src/tir/ops.rs` (add `OpCode::FreeRaw` after
  `Free`, line 100); `op_kinds.toml` (its own row); the exhaustive `match`
  consumers flagged in STRUCTURAL_AUDIT_BOARD (lower_to_wasm:551,
  lower_to_simple:1651, verify.rs:235, type_refine.rs — these are exhaustive
  `match`, so the compiler *forces* a `FreeRaw` arm: no silent default, by design
  — this is why exhaustive match is mandated over `matches!` for moved/added
  opcodes, doc 21 §4 matches!-oracle risk). Migrate the handful of true
  runtime-internal frees (`release_dealloc_tracked_bits_vec` and peers) to
  `FreeRaw`. Add the `verify.rs` rule: `FreeRaw` operand ∉ any `LifetimeClassFacts`.
- **Gate (G4):**
  - `tools/gen_op_kinds.py --check` + `cargo test … verify` green.
  - A verifier negative test: a synthetic `FreeRaw` of a `defines_del` value is
    REJECTED (fail-closed proof).
  - Byte-identical runtime behavior on the full differential suite (the migrated
    sites were always finalizer-free; this only renames their opcode).
  - `structural_audit.py --check`: `duplicate_authorities` stays 0; no new
    semantic_fallthrough (the new arms are exhaustive-match-forced).

### Phase 5 — consolidated memory-safety matrix + decomposition extraction. [Lane A + Lane C, ~2-3 days]

Lock the class closed and pay down the drop_insertion god-file debt this arc
touched.

- **Consolidated matrix** (Lane A): one
  `tests/differential/memory/memory_safety_matrix.py` (or a driver over the
  existing `resurrect_*` + `finalizer_*` files) asserting, across
  native/LLVM/WASM/Luau, under `MOLT_ASSERT_NO_LEAK`: finalize-once, resurrect-once,
  resurrect-then-final-drop, resurrect-in-loop-stress, resurrect-with-weakref,
  resurrect-with-field-refs, resurrect-during-exception-unwind,
  subclass-inherits-del, weakref-cleared-on-free, inner-field-ordering,
  container-clear, plus the primitive fast-path zero-cost check (doc 50
  §acceptance).
- **Decomposition (Lane C):** extract the lattice-consumption logic from
  `drop_insertion.rs` (8,413 lines, god-file) into a cohesive
  `tir/passes/ownership_boundaries.rs` (the council named this file explicitly:
  "`ownership_lattice_min.rs`/`ownership_boundaries.rs`"). Move-only, byte-identical
  (doc 21 §3 universal gate methodology G1-G5). This is the rung-3 placement code;
  `ownership_lattice_min.rs` keeps rung 0-2 (the facts). Drives `max_god_file_lines`
  / `god_files` ratchet DOWN (STRUCTURAL_AUDIT_BOARD ratchet rule).
- **Gate (G5):** the full matrix green on all four backends; `structural_audit.py
  --check` shows drop_insertion below the 4,000 ceiling OR a measured reduction
  with a baton note; the Performance Constitution landing-report block (perf
  matrix green, zero CPython-reds, regressions zero-or-tracked).

---

## 4. Verification & gates (per-phase discipline)

Per CLAUDE.md "Tranche & evidence standard" and the Performance Constitution
methodology. Every phase reports the **PERF/SPEED STATUS block** even though this
is Lane A (a safety fix must not silently regress speed):

- **Parity oracle:** CPython ≥3.12 differential for every memory test, stdout
  byte-identical where applicable, unraisable-to-stderr matched, exit code
  matched. The existing `tests/differential/memory/` + `tests/differential/basic/`
  finalizer corpus is the oracle; new fixtures mirror existing shapes.
- **Memory-safety gate:** `MOLT_ASSERT_NO_LEAK=1` (Phase-0 honest semantics) under
  `tools/safe_run.py --rss-mb <small> --timeout <short>` (CLAUDE.md Safe
  Execution — never raw binary; bisecting a suspected resurrection SIGSEGV uses
  `--rss-mb 512 --timeout 8` so a runaway dies in <1s).
- **Backend parity:** native + LLVM must agree (load-bearing per doc 45/48);
  WASM (host-EH + native-EH + LIR `dec_ref` lane) and Luau (GC no-op of shared
  drop artifacts) checked or documented-gap. A native win never excuses a WASM
  regression (Performance Constitution scoreboard #4).
- **Fact-plane gate:** `tools/gen_op_kinds.py --check` (the facts are generated,
  not hand-matched); `structural_audit.py --check` (duplicate_authorities stays
  0; no new semantic_fallthrough; god-file ratchet not regressed).
- **Verifier gate:** `cargo test … verify`/`verify_lir` — `FreeRaw`/`Free`
  operand lifetime-class rules fail closed.
- **Perf gate (Phase 3 especially):** `benchmark → target → backend → profile →
  CPython ratio → PyPy/Codon ratio → binary size → peak RSS → compile time →
  log artifact`, quiescent + repeated + cycle-attributed, classified GREEN /
  RED_STABLE / RED_NOISY / TIE / DIMENSIONAL_WIN. The pointer-field heap-free
  microbench (doc 50 §acceptance) is the Free-perf witness.

**No phase is "done" on tests-green alone:** the landing report is "tests green;
perf matrix green; no CPython-red benchmarks; resurrection/finalizer/weakref
matrix green under destruction-semantics MOLT_ASSERT_NO_LEAK on all four
backends; regressions zero or explicitly tracked."

---

## 5. Composition with the decomposition (21a-e) and multi-agent execution

### 5.1 With the decomposition program (doc 21, 21a-e)

- **drop_insertion.rs is a `medium` god-file** (8,413 lines, board:36). Phase 5
  extracts `ownership_boundaries.rs` from it — this **advances** doc 21's
  god-file ratchet (it does not conflict; doc 21 does not specifically sequence
  drop_insertion). Coordinate with any active drop_insertion editor (the file had
  the single largest churn in the 50-commit window, doc 21 §1.2).
- **Frontend attrs (Phase 1)** touch `visitors/classes.py` — a **doc 21c (F1)
  frontend-mixin** target. Sequence Phase 1's frontend change to land *before* or
  *coordinate-window with* F1's class-visitor extraction; the new attr emission is
  small and move-stable.
- **`molt-tir` crate (doc 21 T1)**: `ownership_lattice_min.rs`,
  `ownership_boundaries.rs`, `refcount_elim.rs`, `escape_analysis.rs`,
  `op_kinds.toml` all live under `tir/` and move into `molt-tir` as a unit when
  T1 lands. This arc adds `OpCode::FreeRaw` to `ops.rs` (a T1-moved file) — the
  **matches!-oracle audit** doc 21 §4 mandates for moved opcodes is satisfied
  here by using exhaustive `match` (Phase 4), so T1's audit finds no new
  default-false hazard from this arc.
- **No contradiction:** this arc adds facts and one opcode; it does not change the
  crate graph, satellite, or frontend-package boundaries doc 21 defines.

### 5.2 Cross-arc dependencies

- **Exception-region ownership (doc 45)** consumes the *same* lattice
  (`ownership_lattice_min::exception_creation_ref_values`; doc 45 §7 "exception
  ownership becomes a region fact on the #58 ownership-boundary lattice"). M1's
  `LifetimeClassFacts` is the substrate doc 45's wider `HandlerState` boundary
  will read for handler-owned exceptions that themselves `MayFinalize`/`HasWeakrefs`.
  **This arc is upstream of doc 45's completion** — do not duplicate the lattice
  in exception_regions.rs.
- **Finalizer lifecycle (docs 48/49/50)**: doc 50 §"Open slice A — #58" names
  exactly this lattice as the keystone; M1-M2 *are* the generalized form of that
  slice. Doc 49's inline-field single-authority invariant is the `InnerRefOrdering`
  fact's runtime counterpart (do not add a second field-release authority — doc 49
  rule 1). Doc 48's `maybe_run_object_finalizer` is the SOLE finalizer authority
  M3's `Free` demotion protects.
- **Perceus borrow inference (doc 27)** and **escape analysis**: both consume
  `LifetimeClassFacts` for stack-allocation / RC-strip eligibility (the council's
  "consumed by escape + refcount-elim + stack-alloc + Free-eligibility +
  ownership-lowering"). Escape analysis already declines stack-promotion for
  finalizer roots (escape_analysis.rs finalizer guard, doc 48 status); generalize
  to `¬Trivial`. **Perf depends on this fact-plane** (the Performance
  Constitution "fix the REPRESENTATION": dynamic dispatch/RC overhead → ownership
  facts) — this arc supplies the fact perf later optimizes against.
- **Bootstrap (CLAUDE.md Bootstrap Authority)**: `classmethod`/`staticmethod`/
  `property` type objects come from runtime bootstrap intrinsics; M1's
  class-flag refresh must run *after* those types exist (use the existing
  `object_set_class_bits` hook, not a new bootstrap probe).

### 5.3 Multi-agent execution model

- **Lane A owner** drives Phases 0-4 sequentially (each independently landable;
  Phase 0 first for gate honesty). A second Lane A agent can develop the Phase 1
  weakref/ptr-field fixtures and the Phase 5 matrix in parallel (test-only, no
  pass-file contention).
- **Lane C agent** does the Phase 5 `ownership_boundaries.rs` extraction
  (move-only) *after* Phases 2-3 land (so the moved code is final).
- **Max 2 build-triggering agents** (CLAUDE.md); each `export MOLT_SESSION_ID`
  before any build. `molt clean --apply --kill-processes` between sessions.
- **File ownership:** ownership_lattice_min.rs + refcount_elim.rs + op_kinds.toml
  (Lane A); drop_insertion.rs (Lane A Phase 2, then Lane C Phase 5 — serialize,
  do not co-edit); object/mod.rs flags (Lane A Phase 1) — coordinate with any
  runtime-RC editor.

---

## 6. Risks + structural (not band-aid) treatment

| Risk | Structural treatment (no band-aid) |
|---|---|
| **Conservatism kills Free-perf** (every dynamically-typed alloc is `¬Trivial` → loses Free) | The fix is *more facts*, not *wider Free*: when a `Call` result's class is provably `Trivial` (monomorphic call-site, class-version guard — the IC/shape facts the Performance Constitution names), the fact-plane learns it and Free re-fires. Never relax the `Trivial` gate to recover perf; that re-opens the corruption class. Report any Free-loss as a missing-fact in the PERF/SPEED STATUS block. |
| **`HasWeakrefs` is over-broad** (most classes support weakrefs in CPython) | `HasWeakrefs` for *release* purposes is only load-bearing when a weakref to the instance can actually exist. Refine the fact: `class_supports_weakref ∧ (a weakref/proxy was created in-program OR escape-analysis says the instance escapes to where one could be)`. Frame as a fact refinement (rung 2), measured against the weakref differential corpus — not a special-case in Free. |
| **Adding a fifth boundary class later** | §1.4 proves the four are complete vs CPython's `Py_DECREF`→0 obligations. A genuine fifth (e.g. a GC-tracked cycle participant) is added as a fifth bitset in `LifetimeClassFacts` consumed by the same `is_trivial_lifetime_root`, never as a new guard in a consumer pass. The lattice is the extension point. |
| **`FreeRaw` arm missed in a backend** | Exhaustive `match` (not `matches!`) for every `FreeRaw` consumer (Phase 4) makes a missing arm a *compile error*, per doc 21 §4. The verifier rule (operand ∉ LifetimeClassFacts) is the second line. This is the MEMORY.md silent-miscompile lesson made a hard gate. |
| **drop_insertion's seven dated invariants interact with the widened gate** | Phase 2 widens *which* roots defer (rung 2 input); it does NOT touch *how* they transfer (the dated rung-3 invariants). Each dated invariant has a pinned regression test (`_load_collections_abc`, `namedtuple`, `store_var_rebind_epoch`, …) that must stay green — they are the proof the widening did not perturb placement. If one breaks, the widening exposed a real gap in that invariant, to be fixed in the invariant, not worked around. |
| **Resurrection SIGSEGV reproduces under a backend not yet covered** | The repro runs under `safe_run.py` with a small RSS/timeout (never raw binary — CLAUDE.md). A surviving SIGSEGV is root-caused structurally (which of the four facts was wrong at the free site), never capped/xfail'd (CLAUDE.md P0: "never cap the repro or mark it expected"). |
| **Perf regression masked because gate was zero-transition** | Phase 0 (M5) lands first precisely so no later phase can pass on a resurrection-laundered leak. Destruction-semantics `MOLT_ASSERT_NO_LEAK` is the trust root for the entire arc. |
| **God-file extraction (Phase 5) perturbs behavior** | Move-only with doc 21's G1-G5 (byte-identical artifacts G3, symbol identity G5). If artifacts differ, it is not move-only → reject and find the leak. |

---

## 7. Why this is the structurally-correct fix (the constitution check)

- **Retires a class, not an instance** (compression ladder): not "the SIGSEGV" but
  "lifetime-significant object released wrong/wrong-place/wrong-count," across all
  four backends, made unrepresentable by the four-fact lattice + Free demotion.
- **Fixes the REPRESENTATION** (Performance Constitution posture): the missing
  fact was "this object's lifetime class"; we add it once and every consumer
  reads it. We do not add a guard per finalizer composition (the rejected
  DropInsertion-special-case path).
- **One authority** (council mandate): `LifetimeClassFacts` is the single
  finalizer/weakref/resurrection/ordering fact; no pass re-derives it.
- **No workaround / no partial** (Zero-Workarounds): all four boundary classes,
  all four backends, the Free demotion AND the FreeRaw split AND the honest leak
  gauge — the complete structural arc. Asymmetric coverage (e.g. finalizer-only
  Free guard, the current state) is exactly the partial-implementation the
  constitution forbids; M3 closes it symmetrically.
- **P0 ranking honored**: this outranks the perf frontier; Lane A; Phase 0 first.

---

*Design only / executable plan. The lead integrates; no code changed in this
session. Co-Authored-By: Claude Opus 4.8 <noreply@anthropic.com>*
