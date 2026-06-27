<!-- Spine-4 Outcome 1, Phase 0 trust root. Build-free design artifact.
Author: Lane-A prep agent. Date: 2026-06-25. Status: DESIGN ONLY / EXECUTABLE PLAN.
PARENT BLUEPRINT: docs/design/foundation/55_memory_safety_ownership_lattice.md
(§2.5 Mechanism M5; Phase 0, doc 55:379-397). GOVERNING AUTHORITY: Council
Operating Doctrine (CLAUDE.md, 2026-06-08) — "MOLT_ASSERT_NO_LEAK = actual
destruction (not zero-transition)". This doc specifies Phase 0 + the
LifetimeClassFacts 4-bitset (M1) it gates. It NEVER duplicates or contradicts
doc 55; it pins doc 55's plan to the verified runtime line numbers. -->

# Ownership Lattice — Phase 0 Trust Root + LifetimeClassFacts 4-Bitset Spec

## 0. Why this doc exists (the trust-root ordering)

Doc 55 (§2.5, Phase 0) makes `MOLT_ASSERT_NO_LEAK` = **actual destruction** the
FIRST thing that lands, because every later phase's acceptance gate consumes it.
If the gate can pass on a resurrection-laundered leak, every "green" below is a
lie. This doc specifies that trust root precisely against the **verified current
runtime**, then specifies the `LifetimeClassFacts` 4-bitset (doc 55 M1) it
gates — the single cached fact-plane the whole lattice reads.

This is **Spine-4 Outcome 1**, the P0 FLOOR of the 100-year portfolio (CLAUDE.md
P0 ranking: a resurrection/finalizer/weakref MEMORY-CORRUPTION bug OUTRANKS the
native RC flip and all perf/feature work). It gates Outcomes 2/3/4/5/7.

All work here is **build-free design**. No cargo/molt build was run; the runtime
line numbers below were read from the live tree (2026-06-25) and cited so the
implementing agent edits the right authority.

---

## 1. Verified current state (read from the tree, 2026-06-25)

The runtime is ALREADY destruction-correct at the **counter** layer. The bug is
at the **assertion-target** layer. Both facts are load-bearing for Phase 0.

### 1.1 The counter is correct (do NOT touch it)

`runtime/molt-runtime/src/object/mod.rs` `dec_ref_ptr` (mod.rs:1994) at the
rc→0 transition:

- **mod.rs:2175** — `if maybe_run_object_finalizer(py, ptr) { return; }`. When
  `__del__` resurrects (re-increments rc), `maybe_run_object_finalizer` returns
  `true` and the free is **aborted before any dealloc counter moves**.
- **mod.rs:2184** — `profile_hit(py, &DEALLOC_COUNT);` is committed ONLY past the
  resurrection check. So `DEALLOC_COUNT` already means "objects truly destroyed",
  and `live = ALLOC_COUNT − DEALLOC_COUNT` already excludes resurrected-alive
  objects. The inline rationale (mod.rs:2178-2183) states this exactly.
- **mod.rs:2187** — `weakref_clear_for_ptr(py, ptr);` runs only on the true-death
  path, after the resurrection abort. (This is the runtime fact the two Phase-0
  weakref differentials gate; see §4.)

**Conclusion:** doc 55:357 is correct that M5 is "largely a verification-harness
change." The zero-transition-vs-destruction distinction is already right in the
counter. Phase 0 must NOT re-plumb `dec_ref_ptr`.

### 1.2 The assertion target is a fixed ceiling — the actual Phase-0 gap

`runtime/molt-runtime/src/object/ops.rs` `assert_no_leak_at_exit` (ops.rs:762):

```rust
let live = allocs.saturating_sub(deallocs);
if live <= crate::EXPECTED_LIVE_OBJECTS { return; }   // ops.rs:768-770
```

and `runtime/molt-runtime/src/state/metrics.rs`:

```rust
pub(crate) const EXPECTED_LIVE_OBJECTS: u64 = 200_000;  // metrics.rs:28
```

with the comment (metrics.rs:22-27): *"This is an UPPER-BOUND ceiling, not an
exact equality target … sized so a non-leaking program passes and a
per-iteration leak (which grows `live` without bound) fails decisively."*

**This is the Phase-0 gap.** A 200,000-object ceiling catches only *unbounded*
leaks. A **bounded** resurrection-laundered leak — e.g. a program that leaks
exactly 50 finalizer objects, or resurrects N=1000 then fails to destroy them —
passes silently (`50 ≤ 200_000`, `1000 ≤ 200_000`). That is precisely the
"phantom-no-leak" class doc 55:128 and §0 name. The gauge as shipped is a
coarse net, not the destruction oracle doc 55 G0 requires:

> doc 55:389-391 — *"a program that resurrects N times then drops must report
> `live==0` only when truly destroyed"* and *"a deliberately-leaking microbench
> must now FAIL the assertion."*

`live==0`-class precision is **unreachable** against a 200K ceiling. Phase 0 must
add an **exact-target mode**.

---

## 2. Phase-0 contract (M5): actual destruction, exactly

### 2.1 Definitions (binding)

- **Actual destruction of object O** = `tp_dealloc`-equivalent ran for O: control
  reached past mod.rs:2175 (resurrection abort NOT taken) into the byte-free
  tail, so `DEALLOC_COUNT` was bumped for O at mod.rs:2184. NOT "O's refcount
  touched 0" (a resurrecting `__del__` makes rc touch 0 and then rebound — the
  zero-transition — without destruction).
- **Survivor set S** = the objects that legitimately reach process exit alive:
  module-level immortals, interned constants/singletons, builtin type objects —
  the immortal bootstrap floor. `|S|` is small and program-dependent (varies with
  which stdlib modules are imported; metrics.rs:23-27).
- **Honest leak** at exit = `live − |S| > 0`, i.e. some non-immortal heap object
  was allocated and never actually destroyed.

### 2.2 The POSITIVE contract (actual-destruction must pass)

A program that allocates T transient objects and **truly destroys all of them**
(including objects that resurrect K times and are then finally destroyed) MUST
report `live == |S|` at exit and PASS the assertion. Resurrection that ends in a
real final drop is NOT a leak; the count returns to the survivor floor exactly
because `DEALLOC_COUNT` is committed once per real destruction (mod.rs:2184),
once per object even across K resurrections (FINALIZER_RAN-once semantics,
mod.rs:2171).

Witness program (already in tree, validated CPython): the canonical
`tests/differential/memory/finalizer_resurrection_leak_gauge.py` drives one
object through resurrect→final-drop; under the Phase-0 exact gauge it must report
`live == |S|` (the object IS destroyed). The N-scaled
`tests/differential/memory/resurrect_once_N1000.py` is the at-scale positive
witness (doc 55:392-394): 1000 resurrect-then-destroy cycles, exit 0, exact
survivor floor.

### 2.3 The NEGATIVE contract (a deliberate leak MUST fire)

The trust root is only trustworthy if it FAILS on a real leak. Phase 0 ships a
**deliberate-leak negative test** whose sole purpose is to make the assertion
fire:

- A program that stashes a **bounded** number L of finalizer objects in a
  module-global container that is NEVER cleared (so they survive to exit
  un-destroyed), with L small (e.g. L = 64) — far below the 200K ceiling.
- Under the Phase-0 **exact gauge**, `live − |S| == L > 0` ⇒ the assertion MUST
  print `[MOLT_ASSERT_NO_LEAK] FAIL: …` and exit non-zero (ops.rs:760 mirrors the
  `safe_run.py` RSS-cap convention with `process::exit(137)`).
- This is the proof the gauge detects bounded leaks, not just runaway ones. Per
  CLAUDE.md world-class-rigor: *prove the gate fails on a synthetic violation.*
  A gauge never observed to fail is an untested gauge.

The negative test is the falsification artifact (CLAUDE.md evidence standard):
it must be a committed file that, when run WITHOUT the Phase-0 fix, PASSES
(silently laundering the 64-object leak under the 200K ceiling) and, WITH the
fix, FAILS. That delta is the Phase-0 acceptance.

### 2.4 The mechanism: exact survivor target (no band-aid)

The fix is to give the assertion an **exact** target instead of a coarse ceiling,
structurally — not by lowering the 200K magic number (which would just move the
silent-pass threshold). Two composable parts:

1. **Measured survivor floor.** At a stable point BEFORE user code runs its first
   allocation (end of bootstrap), snapshot `live_floor = ALLOC_COUNT −
   DEALLOC_COUNT`. This is `|S|` for *this* program's import set, measured, not
   guessed. Store it in a runtime cell read by `assert_no_leak_at_exit`.
2. **Exact mode + tolerance override.** `assert_no_leak_at_exit` gates on
   `live ≤ live_floor + MOLT_LEAK_TOLERANCE` where `MOLT_LEAK_TOLERANCE` defaults
   to a small constant (e.g. 0 for the destruction-oracle tests; a tiny slack for
   programs whose exit-time immortal set is not perfectly snapshot-aligned). The
   memory-safety differentials run with `MOLT_LEAK_TOLERANCE=0` (or an env that
   selects exact mode) so `live == live_floor` is the pass condition — the
   `live==0`-class precision doc 55 G0 demands (here `0` is relative to the
   measured floor, the structurally-correct generalization of "zero transient
   leak").

   The existing 200K ceiling is **retained as the fail-decisively upper bound for
   the default (non-exact) profile** so ordinary `molt run` keeps its cheap
   runaway-leak guard; exact mode is opt-in for the safety gate. This is additive,
   not a regression of the existing behavior.

**Rejected band-aids** (per CLAUDE.md zero-workarounds):
- Lowering `EXPECTED_LIVE_OBJECTS` to a smaller fixed number — still a ceiling,
  still silently passes any bounded leak below it. NO.
- Special-casing the resurrection tests to assert their own counts inline — that
  is a per-test guard, not a gauge fix. NO. The gauge itself must become exact.
- Asserting `sum(rc deltas) == 0` — the EXACT thing doc 55 forbids (a resurrect +
  re-drop satisfies it while the object is alive at the assertion point,
  doc 55:359-360). NO.

### 2.4.1 MEASURED CORRECTION (2026-06-25): the gauge must run POST-teardown

The §2.4 mechanism as first specified (snapshot floor at end-of-bootstrap, gate
`live ≤ floor + tol` *inside `assert_no_leak_at_exit`*) was **implemented and then
falsified by measurement** before commit. It is structurally wrong on two counts;
both were proven against a real native binary, not argued.

**Falsification run** (cycle/global-stash leak binary, `MOLT_ASSERT_NO_LEAK=1
MOLT_LEAK_TOLERANCE=8`): the gauge fired with `live_objects=828`, `floor=18`,
**`dealloc_object=0`**. The `dealloc_object=0` is the load-bearing fact: at the
`assert_no_leak_at_exit` call site, molt has freed **zero** of the program's heap
objects. The 828 ≫ 18 is not a leak — it is the program's entire still-reachable
working set, resident because **molt reclaims reachable graphs at TEARDOWN, not at
the assertion point**.

**Root cause — the assertion site is pre-teardown.**
`runtime/molt-runtime/src/state/runtime_state.rs` `molt_runtime_exit` calls
`assert_no_leak_at_exit` (the §2.4 site) *before*
`runtime_teardown_for_process_exit`. Teardown (`lifecycle.rs:107`) is what runs
`modules_clear_runtime_state` (`builtins/modules.rs:313-317`), which clears the
module registry slots and so DecRefs every user `__main__` global. Until that
runs, every reachable object — module globals, their transitive closure, interned
working strings/dicts/tuples — is still live. So `live == |S|` (§2.2) is
**unreachable** at the §2.4 site for *any* non-trivial program; the exact gauge
there false-positives on the working set of every clean program. §2.1's "S = …
the immortal bootstrap floor" is correct as a *post-teardown* survivor set, not a
*pre-teardown* one. §2.2/§2.4 silently assumed the two coincide. They do not.

**Root cause — the §2.3 negative test is not a leak.** A bounded count of
finalizer objects stashed in a never-cleared module global is **reachable** and is
freed by `modules_clear_runtime_state` at teardown — exactly as CPython clears
module dicts at interpreter shutdown (and runs those `__del__`s). It is not a
leak; it is ordinary shutdown reclamation. Pre-teardown it is indistinguishable
from the working set; post-teardown it is gone. So the §2.3 test can never be the
falsification artifact for a *true*-leak gauge.

**What a TRUE leak is in molt.** molt is reference-counted with **no cycle
collector**: `formal/quint/molt_gc_safety.qnt:3-5` proves absence of leaks for
**acyclic** object graphs only; `include/molt/Python.h` carries `tp_traverse`
slots purely as C-ABI stubs (`Python.h:3210` discards them — nothing consumes them
for collection). Therefore the canonical — and essentially *only* — leak class is
an **unreachable reference cycle**: RC pins each node at refcount ≥ 1 (its peer
holds the reference), teardown's module-clear cannot reach it, and nothing
reclaims it. CPython's cyclic gc would collect it; molt leaks it. (This is itself a
known, deliberate parity gap, scoped by the formal model — the gauge's job is to
make it *visible and gated*, not to hide it.)

**Corrected mechanism — two gates for two distinct properties:**

1. **Pre-teardown RUNAWAY guard** (`assert_no_leak_at_exit`, unchanged behavior):
   the existing `EXPECTED_LIVE_OBJECTS` ceiling, measuring peak reachable
   high-water-mark. A coarse OOM/runaway canary. NOT a leak gauge (it sees the
   working set). Retained verbatim for the default `molt run` profile.
2. **Post-teardown TRUE-LEAK gauge** (`assert_no_true_leak_post_teardown`, NEW;
   called in `molt_runtime_exit` *after* `runtime_teardown_for_process_exit`):
   teardown has now reclaimed every reachable acyclic graph, so survivors =
   immortal floor + genuine leaks. Gate `live ≤ floor + MOLT_LEAK_TOLERANCE` in
   exact mode (tolerance set); the `floor` snapshot from §2.4 part 1 is still the
   right reference because the immortal set is stable across teardown. No-op in the
   default profile. Additive; never changes default behavior. (One shared
   `emit_leak_breakdown` authority backs both gates so their reporting cannot
   drift.)

**Corrected positive contract (supersedes §2.2's assertion site).** A program that
truly destroys all transients reports `live == floor` **post-teardown** (not at the
pre-teardown site). Witness control: `cycle_leak_clean_control.py` (acyclic churn,
all dropped) — exact mode exit 0.

**Corrected negative contract (supersedes §2.3).** The falsification artifact is
`tests/differential/memory/cycle_leak_negative.py`: 64 unreachable 2-cycles. Exact
mode → exit 137 (post-teardown survivors = floor + 128 cycle nodes); default
ceiling → exit 0 (laundered). That delta is the Phase-0 acceptance. The
global-stash test (`bounded_leak_negative.py`) is **deleted** as semantically
incorrect.

### 2.5 Files (Phase 0) — as corrected by §2.4.1 (two-gate design)

- `runtime/molt-runtime/src/object/ops.rs`:
  - `assert_no_leak_at_exit` (ops.rs:762) — kept as the **pre-teardown runaway
    ceiling** (`EXPECTED_LIVE_OBJECTS`), behavior unchanged; refactored to emit via
    the shared `emit_leak_breakdown`.
  - `assert_no_true_leak_post_teardown` (NEW) — the **post-teardown exact gauge**:
    gate `live ≤ floor + MOLT_LEAK_TOLERANCE`, exact-mode-only (no-op without
    tolerance), emits via the same shared `emit_leak_breakdown`.
  - `emit_leak_breakdown` (NEW, private) — the single per-type diagnostic authority
    backing both gates so reporting cannot drift.
- `runtime/molt-runtime/src/state/runtime_state.rs` — `molt_runtime_exit`: call
  `assert_no_true_leak_post_teardown(&py)` **after** `runtime_teardown_for_process_exit`
  (the pre-teardown `assert_no_leak_at_exit` call stays where it is). GIL still held;
  reads crate-static counters only.
- `runtime/molt-runtime/src/state/metrics.rs` — the `live_floor` cell + its
  snapshot setter (`snapshot_live_floor`, called at end of bootstrap) + the
  `leak_exact_tolerance` reader beside `leak_assertion_enabled`. Keep
  `EXPECTED_LIVE_OBJECTS` as the default-profile ceiling.
- The bootstrap site that calls the snapshot setter — routed through the existing
  end-of-bootstrap boundary in `molt_runtime_init` (CLAUDE.md Bootstrap Authority).

### 2.6 Gate (G0) — as corrected by §2.4.1

- `tests/differential/memory/cycle_leak_clean_control.py` (acyclic churn) PASSES
  under exact mode (`live ≤ floor + tol`, post-teardown): positive contract.
- `tests/differential/memory/cycle_leak_negative.py` (64 unreachable 2-cycles)
  FAILS under exact mode (exit 137) and PASSES (wrongly, exit 0) under the default
  ceiling: negative contract + the falsification delta.
- The leak is exit-code-only; stdout is byte-identical to CPython, so the gauge
  composes with the standard differential stdout check.
- No behavior change for the default profile (200K ceiling path) on the full
  differential corpus — the post-teardown gauge is a no-op without
  `MOLT_LEAK_TOLERANCE`.

---

## 3. LifetimeClassFacts — the 4-bitset spec (M1, gated by Phase 0)

Doc 55 §2.1 defines `LifetimeClassFacts` as the ONE ClassInfo/MRO/version-derived
cached fact-plane. This section pins each bit's derivation and its consumers, so
the implementing agent emits ONE fact, four predicates — never four analyses,
never a pass-local re-derivation. Phase 0 gates this because every G1 fact-parity
assertion below is checked under the honest destruction gauge.

### 3.1 The four bits and their derivation

| Bit | Derivation (source of truth) | Verified current state |
|---|---|---|
| **MayFinalize(class)** | class/MRO has `__del__`. Runtime: `HEADER_FLAG_CLASS_HAS_FINALIZER` set by `class_refresh_finalizer_flag` (mod.rs:1493-1501) reading `__del__` via `class_lookup_raw_mro_dict_attr`, sealed by `class_finish_definition` (mod.rs:1512-1516). | EXISTS. Reused verbatim as the `may_finalize` seed. |
| **HasWeakrefs(class)** | class allows weakrefs (CPython `tp_weaklistoffset != 0`): `__slots__` does not suppress `__weakref__`. Runtime: NEW `HEADER_FLAG_CLASS_SUPPORTS_WEAKREF`, refreshed on the SAME MRO/version hook beside the finalizer flag (mod.rs:1493 region), copied to the instance on `object_set_class_bits` (mod.rs:1417). | DOES NOT EXIST yet (grep: no `SUPPORTS_WEAKREF`/`weaklistoffset` in runtime/). NEW WORK. |
| **MayResurrect(class)** | conservatively `= MayFinalize` (any `__del__` can re-root `self`). DERIVED, not stored. | Derived from bit 1. |
| **InnerRefOrdering(class)** | `MayFinalize ∧ HEADER_FLAG_HAS_PTRS` (mod.rs:446; the object owns ref-counted fields a `__del__` can observe, doc 49). DERIVED, not stored. | `HEADER_FLAG_HAS_PTRS` EXISTS (mod.rs:446, set by `object_mark_has_ptrs` mod.rs:1518). |

**Why exactly four (no fifth):** doc 55 §1.4 proves these are exactly the
non-byte-free obligations of CPython's `Py_DECREF`→0 path, mirrored by molt's
`dec_ref_ptr` (resurrection abort mod.rs:2175, `weakref_clear_for_ptr`
mod.rs:2187, per-`type_id` inner-ref release mod.rs:2188+). A genuine fifth class
(e.g. a GC-tracked cycle participant) is added as a fifth bitset consumed by the
same `is_trivial_lifetime_root`, never as a new guard in a consumer pass.

### 3.2 Compile-time layer + the one fixpoint

`runtime/molt-passes/src/tir/passes/ownership_lattice_min.rs` gains
`LifetimeClassFacts` (doc 55:183-202): three seed sets (`may_finalize_roots` =
today's `finalizer_alloc_roots`; NEW `has_weakref_roots`; NEW
`has_ptr_field_roots`) flowed through the SAME container-absorption fixpoint that
already produces `finalizer_sensitive_roots` (ownership_lattice_min.rs:564-683).
One fixpoint, three stored bitsets, two derived predicates:

```
is_trivial_lifetime_root(r) = ¬may_finalize(r) ∧ ¬has_weakref(r)
   (MayResurrect ⊆ MayFinalize; InnerRefOrdering ⊆ MayFinalize — both covered)
may_resurrect_root(r)       = may_finalize_root(r)
inner_ref_ordering_root(r)  = may_finalize_root(r) ∧ has_ptr_field_root(r)
```

The frontend (`src/molt/frontend/visitors/classes.py`) emits two NEW allocation-op
result attrs beside the existing `defines_del`: `class_supports_weakref: bool`
and `class_has_ptr_fields: bool` — the compile-time mirror of the runtime flags,
so placement reads a fact instead of re-deriving it. The classifier is GENERATED
via `op_kinds.toml` + `tools/gen_op_kinds.py` (a `lifetime_class` set), never a
hand-written `matches!` (avoids a new semantic-fallthrough; STRUCTURAL_AUDIT
discipline).

### 3.3 The single-authority consumer list (binding)

The council mandate (CLAUDE.md): `FinalizerSensitive` is ONE fact consumed by
**escape + refcount-elim + stack-alloc + Free-eligibility + ownership-lowering**.
Generalized to the 4-bitset, every consumer reads `LifetimeClassFacts`, none
re-derives:

| Consumer | Reads | Replaces (today's narrower query) |
|---|---|---|
| escape analysis (`escape_analysis.rs`) | `¬is_trivial_lifetime_root` declines stack-promotion | finalizer-only guard (doc 48 status) |
| refcount-elim Step 6 (`refcount_elim.rs:621-718`) | `is_trivial_lifetime_root` gates `DecRef→Free` (M3) | `!finalizer_roots.contains` single-class guard (refcount_elim.rs:704) |
| stack-allocation | `¬is_trivial_lifetime_root` blocks stack alloc | finalizer-only |
| Free-eligibility (M3) | all four false ⇒ Free-eligible | finalizer-only |
| ownership-lowering / rung-3 placement (`ownership_lattice_min.rs:495-510`, `drop_insertion.rs`) | `¬is_trivial_lifetime_root` ⇒ Python-lifetime-boundary release (M2) | `is_finalizer_sensitive_root` |

### 3.4 Fail-closed rule (binding)

An allocation whose class facts are UNKNOWN (a dynamically-typed `Call` result
with no `defines_del`/`class_supports_weakref` attr) is `¬Trivial`: all four facts
conservatively TRUE (doc 55:216-222). Over-conservatism costs a finalizer-aware
DecRef branch; under-conservatism costs a UAF. The constitution's choice is
forced. The generated `op_kinds.toml` classifier already fails closed for
`non_owning_copy` (ownership_lattice_min.rs:282-307); extend the same posture.

### 3.5 Gate (G1) — fact-only, no placement change

- `cargo test -p molt-backend --lib --features native-backend
  ownership_lattice_min` — extend the existing 20+ fixtures
  (ownership_lattice_min.rs:835-1617) with weakref/ptr-field/resurrection
  fixtures mirroring the finalizer ones (`container_absorbing_*`,
  `nested_container_propagates`).
- `tools/gen_op_kinds.py --check` green (fact is generated).
- Parity probe: `has_weakref_root` agrees with CPython `cls.__weakref__`
  availability for `tests/differential/basic/dict_subclass_slots_weakref*.py`.
- **Byte-identical TIR/artifacts** on the full differential corpus (this phase is
  fact-only; placement changes land in M2/Phase 2). `structural_audit.py --check`
  does not regress.

---

## 4. The two Phase-0/trust-root differentials (authored + validated this session)

These are the gates that would CATCH the resurrection-at-scale SIGSEGV class.
Authored in `tests/differential/memory_safety/`, validated as plain CPython
3.12.13 (the oracle) with exact recorded traces. They are **strictly additive**
to the canonical `tests/differential/memory/resurrect_*` corpus (each header
cross-references its canonical sibling; no duplicate authority).

### 4.1 `resurrect_with_weakref.py` — weakref-callback ordering across resurrection

Exercises the load-bearing PEP 442 fact (empirically verified): a resurrecting
`__del__` runs FIRST; the object is then live again, so CPython does NOT clear the
weakref and does NOT fire the weakref callback — the weakref stays live and
resolves to the resurrected object. The callback fires exactly once, only on the
later real death. A `Free` skipping `weakref_clear_for_ptr` (mod.rs:2187) on this
object = UAF on a live weakref; the lattice (HasWeakrefs ∧ MayResurrect) makes
that `Free` unrepresentable (doc 55 §2.3). Exact CPython trace (recorded in the
file header):

```
alive_before True
trace_after_resurrect ['del']
box_len 1
weakref_live_after_resurrect True
weakref_same_object True
trace_after_final ['del', 'cb']
weakref_dead_after_final True
done
```

### 4.2 `resurrect_during_exception_unwind.py` — exception-unwind release placement

The council "Finding 9" known-fix-never-measured leak (doc 55:250-258). A
resurrecting frame-local with an ORDERED INNER REF (a `Child` field) dies as the
stack unwinds through `try/finally`/`except`. Verified CPython ordering: the
traceback holds the local alive, so `__del__` runs AFTER the `finally` block and
AFTER the handler body; the inner `Child` survives the unwind (no premature free)
and is released only on the parent's final death, in cascade order. The
finalizer-aware DecRef must land on the exception-transfer-to-EXIT arc, not the
normal-flow last-use (drop_insertion.rs:1130-1135) — else a fail-closed LEAK
(not corruption, doc 55:250-258). Exact CPython trace (recorded in the header):

```
trace ['R_init', 'pre_raise', 'finally_ran', 'handler:unwind-boom', 'R_del']
caught True
box_len 1
child_alive_via_resurrected k
trace_final ['R_init', 'pre_raise', 'finally_ran', 'handler:unwind-boom', 'R_del', 'child_del:k']
after_final box_len 0
done
```

Both traces are deterministic (verified bit-identical across repeated CPython
runs). The molt-vs-CPython differential run is post-build (the build slot is owned
by the float-merge); these files are correct CPython-matching Python NOW and flip
to live differential gates the moment a build is available — do NOT suppress them.

---

## 5. Composition + discipline

- **Lane A** (P0 semantic safety). Phase 0 lands FIRST for gate honesty, then M1
  (LifetimeClassFacts) §3, then M2/M3/M4 per doc 55 §3.
- **No duplicate authority:** the survivor floor is computed once
  (`state/metrics.rs`), read by `assert_no_leak_at_exit`; `LifetimeClassFacts` is
  computed once, read by all five consumers (§3.3). No pass-local finalizer/
  weakref reasoning anywhere.
- **Build-free:** this doc and the two differentials are design + validated-CPython
  artifacts only. No cargo/molt build was run. Runtime line numbers are cited from
  the live tree (2026-06-25) for the implementing agent.
- **Safe execution:** every runtime gate runs under `tools/safe_run.py` with a
  small RSS/timeout, never a raw binary (CLAUDE.md).

*Design only / executable plan. No code changed in this session beyond the two
validated differentials. Co-Authored-By: Claude Opus 4.8 <noreply@anthropic.com>*
