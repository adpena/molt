<!--
Foundation blueprint 65 — The Performance Compression Ladder.
Arc: the ordered roadmap of IR FACTS, each rung retiring one CLASS of slowness.
Author: portfolio-architect.
Date: 2026-06-24 (scoped 2026-06-23).
Status: DESIGN ONLY / EXECUTABLE PLAN. No implementation landed by this doc.

NUMBERING NOTE: the assigned path is `65_perf_compression_ladder.md` in the
active 54-67 portfolio route cluster.

All file:line anchors were verified read-only against the worktree snapshot
available on 2026-06-24. Code beats this doc when it drifts.
-->

# 65 — The Performance Compression Ladder: one IR fact per class of slowness

## 0. End-state outcome (the time-traveler's destination)

**In the end state, "this Python program is slow on molt" is not a bug you can
file — because the property that made it slow is a first-class, generated,
validated IR fact, and the absence of that fact makes the slow lowering
*unexpressible*.** A megamorphic call cannot stay generic once its `CallFacts.target`
is `Proven`; a boxed integer cannot survive a hot loop once its `Repr` is
`RawI64Safe`; a field load cannot re-dispatch once its `FieldOffset` shape fact is
attached; a per-iteration `inc_ref`/`dec_ref` cannot exist on a borrowed value once
the ownership lattice types it `Borrowed`. The optimizer stops *recovering* Python
semantics from low-level SSA events and instead *consumes* facts that were never
dissolved (doc 46 §0, the MLIR invariant).

Concretely, at the destination:
- **CPython floor permanently green** — every benchmark in `tools/bench_suites.py`
  `BENCHMARKS` (69 entries), every target (native/LLVM/WASM/Luau), every profile
  (dev-fast/release-fast/release-output/debug-with-asserts), `warm_speedup > 1.00`
  on `tools/perf_scoreboard.py` with zero `FAIL_ENGINE` cells.
- **PyPy matched/beaten on the dynamic subset** (`bench_fib`, `bench_class_hierarchy`,
  `bench_attr_access`, `bench_dict_ops`) — every place PyPy wins today is closed by a
  *named* missing molt fact, not by "JIT magic."
- **Codon approached/exceeded on the static subset** (`bench_sum*`, `bench_prod_list`,
  `bench_matrix_math`, `bench_struct`, `bench_csv_parse*`) on matched semantics.
- **Five-year cadence met:** ~one *class* of slowness retired per month (doc 51 §8),
  each landing as a new IR fact + its Alive2-style validator (#75) + a scoreboard
  row that goes and stays green.

This document is the **ordered ladder of rungs** that gets there. It is the
performance detail-out of the north star (doc 51 §5, §8): doc 51 names the classes;
this doc fixes the *order*, the *exact tir representation* of each fact, the
*benchmark class it makes unexpressible-as-slow*, the *PyPy/Codon gap it closes*,
and the *landable phases with green gates*.

### 0.1 What this doc is NOT (anti-duplication contract)

It does **not** re-specify the mechanisms that already have full blueprints. Each
rung *composes with and cites* the existing design:
- Rung 1 (ownership/RC) → docs 20 (DropInsertion, landed), 27 (Perceus borrow
  inference, design-ready), 49/50 (object-field / finalizer-lifetime ownership).
- Rung 2 (dispatch) → docs 47 (CallFacts, Phase 1a landed), 46 §4.4 (#71 typed
  CallableTarget), 06 (PGO, the profile evidence source).
- Rung 3 (boxing) → `representation_plan.rs` (the `Repr` lattice, landed), 03 (E5
  representation specialization / the Julia axis).
- Rung 4 (shapes) → docs 46 §4 / 47 §4 (ShapeFacts named, **0% built** — the single
  largest hole), 49 (object-field ownership).
- Rung 5 (loops) → docs 04 (L4 loop re-enable), 02 (MemorySSA/SROA, landed), 05 (SIMD).
- Rung 6 (generators) → docs 26 (real async generators), 07 (coroutine elision).
- Rung 7 (portable-IR / backend parity) → docs 06, 46 §4.7, 14 (target/profile parity).
- Rung 8 (cold-start/footprint) → doc 51 §2 dimensions, the scoreboard cold axis.

The ladder is the *spine* that orders these into a dependency-correct, one-class-per-
month cadence and names the **shared fact substrate** (the `FactValue` confidence
lattice from `call_facts.rs:117`) every rung reuses so we never grow a second
authority for "proven vs guarded vs unknown."

---

## 1. The method: a rung is a fact, not a pass

**Binding restatement of CLAUDE.md ("fix the REPRESENTATION, not the pass"):** a rung
of this ladder is complete only when (a) a new *fact family* exists as a typed,
cached, serializable record in `runtime/molt-tir/src/`, (b) every consumer reads the
fact instead of re-deriving it (no second authority — doc 49), (c) a validator turns
the fact into a checkable obligation (#75, Alive2 discipline), and (d) the scoreboard
row the fact targets is GREEN and *stays* green because the slow lowering is now
structurally absent. "We added a peephole that recovers 10%" is **not** a rung.

**The shared substrate every rung reuses (do not fork it):**
- The confidence lattice `FactValue { Proven | Guarded(GuardId) | Profiled(Confidence)
  | Unknown | False }` (`runtime/molt-tir/src/tir/call_facts.rs:117`). `Unknown` is the
  fail-closed default; a wrong `Proven` is a miscompile, a conservative `Unknown` is a
  missed opt. Every rung's facts are `FactValue`-typed so the same soundness reasoning
  applies uniformly.
- The cached-analysis machinery: `AnalysisId` + `AnalysisManager` (`tir/analysis/mod.rs:66`),
  fail-closed invalidation (`invalidate_cfg`), the `MOLT_VERIFY_ANALYSIS=1` self-check
  (`tir/analysis/mod.rs:33`). New facts are new `AnalysisId` variants with the same
  invalidation contract — never a side channel.
- The interprocedural seeding pattern: `CallFactsTable::build_module` in the module
  phase + `AnalysisManager::prepopulate` (`call_facts.rs:46-63`). Callee-side facts are
  built whole-program and *seeded*; the per-function `compute` is the fail-closed floor.
- The generated op-semantics registry: `op_kinds.toml` → `gen_op_kinds.py --check`
  (doc 25, doc 46 §1). Per-op facts (`may_throw`, `operand_ownership`, mints-fresh-ref)
  are generated and `--check`-gated; a rung that needs a per-op fact *adds a column to
  the registry*, never a hand-`match` in a pass.

**The measurement discipline every rung obeys (the gate, not a vibe):** every rung
reports `benchmark → target → backend → profile → CPython ratio → PyPy ratio → Codon
ratio → binary size → peak RSS → compile time → log artifact` via
`tools/perf_scoreboard.py` (the warm/cold split of `docs/perf/SCOREBOARD.md`), with
`>=5` samples, CV stability, cold AND warm, and provenance (anti-stale-lore). A rung
is not "healed" until measured GREEN against the full matrix it touches and the result
is classified GREEN / RED_STABLE / RED_NOISY / TIE / DIMENSIONAL_WIN (CLAUDE.md
tranche standard). A DIMENSIONAL_WIN (alloc/RSS/binary improved, warm gate did not
flip) is reported as dimensional, never as a speed heal.

### 1.1 Why this ordering (the dependency spine)

The rungs are ordered so that each rung's fact is *consumed* by later rungs, and no
rung is blocked on a fact a later rung produces. The load-bearing edges:

```
Rung 0  Scoreboards + perf-causality (the measurement path)         [C-lane; mostly built]
   │  (every later rung reports through it; no optimizing blind)
   ▼
Rung 1  Ownership lattice → Perceus borrow/reuse  (RC overhead)     [A-lane; doc 27]
   │  (unlocks: no_alloc/no_escape_args become Proven for Rung 2;
   │   RC-free borrowed args; the native value-tracking deletion)
   ▼
Rung 2  CallFacts complete + typed CallableTarget  (dynamic dispatch)[B-lane; doc 47/46]
   │  (consumes Rung 1 escape facts for no_alloc/no_escape_args;
   │   produces direct/devirt targets Rung 3/4 specialize on)
   ▼
Rung 3  Repr convergence + E5 representation specialization (boxing) [B-lane; doc 03]
   │  (consumes Rung 2 typed_return + direct targets to specialize
   │   monomorphic numeric bodies; produces raw lanes Rung 5 vectorizes)
   ▼
Rung 4  ShapeFacts: ClassShape/FieldOffset/ClassVersionGuard (shapes)[B-lane; NEW]
   │  (consumes Rung 2 Guarded(class_version); the ETL/ORM/attr cluster)
   ▼
Rung 5  Loop fact closure: induction/range/overflow/lane (slow loops)[B-lane; doc 04/05]
   │  (consumes Rung 3 raw lanes; SCEV/ValueRange already built)
   ▼
Rung 6  Resumable-frame ownership + generator fusion (slow generators)[A/B; doc 26]
   ▼
Rung 7  Portable-IR fact parity (WASM/LLVM losing a native opt)      [C/B; doc 46 §4.7]
   │  (a cross-cutting *closure*: every fact above must survive to every backend)
   ▼
Rung 8  Artifact-footprint facts (cold start / binary size / RSS)    [C-lane; doc 51 §2]
```

The vertical edges are real producer→consumer dependencies; the lane tags map to the
council three-lane model (§7). Rung 0 is a prerequisite for *all* (you cannot retire a
class you cannot measure). Rungs 1→5 are the main perf arc and are *strictly ordered*
by fact dependency. Rungs 6/7/8 are partially parallelizable once Rung 0 + the fact
substrate exist.

---

## 2. The current state (what the ladder builds on — verified against `main`)

The substrate is unusually mature; this ladder is *completion + ordering*, not a
greenfield build. Verified read-only at HEAD `1d92bc5cf`:

| Substrate | Where | State |
|---|---|---|
| `Repr` lattice (`Never/RawI64Safe/Bool/MaybeBigInt/FloatUnboxed/DynBox`) | `representation_plan.rs:838` | **landed**; the boxing-precision carrier |
| `TirType` lattice (incl. `UserClass(String)`) | `tir/types.rs:4`, `:46` | **landed**; `UserClass` documented as the devirt/shape hook but **codegen unwired** |
| `FactValue` confidence lattice | `tir/call_facts.rs:117` | **landed** (the shared substrate) |
| `CallFacts` record + `CallFactsTable` + `CallFactsAnalysis` | `tir/call_facts.rs` (49 KB) | **Phase 1a landed**: `target/typed_return/leaf/no_throw/inlinable` attached; `no_alloc`/`no_escape_args` are `Unknown` (Phase 2 = Rung 1/2) |
| `AnalysisManager` (S1) + 14 `AnalysisId`s | `tir/analysis/mod.rs:66` | **landed**; CallFacts + ExceptionRegions + ScalarEvolution + ValueRange + AliasAnalysis + MemorySSA + Liveness all registered |
| SCEV + ValueRange | `tir/passes/scev.rs`, `value_range.rs` (123 KB) | **landed** (S6); AddRec recurrences + interval lattice |
| Alias analysis + MemorySSA + MemGVN + SROA | `tir/passes/alias_analysis.rs`, `memory_ssa.rs`, `mem_gvn.rs`, `sroa.rs` | **landed** (S5 ph1/2a/2b/2d) |
| DropInsertion (RC, rung-1 of MM ladder) | `tir/passes/drop_insertion.rs` (342 KB) | **landed + active** native/LLVM/WASM/Luau (doc 27 §0; native RC flip DONE per memory) |
| `ownership_lattice_min.rs` | `tir/passes/ownership_lattice_min.rs` (64 KB) | **landed** (council #58 keystone slice: alias-root→ownership→boundary→ordered release) |
| refcount_elim / reuse_analysis / escape_analysis | `tir/passes/{refcount_elim,reuse_analysis,escape_analysis}.rs` | **landed**; the insert-then-remove model Rung 1 replaces |
| Loop passes (licm/loop_unroll/block_versioning/type_guard_hoist/counted_loop) | `tir/passes/*.rs` | **landed but gated**; doc 04 re-enable arc partially done (counted-loop contract `fae639e94`) |
| `vectorize.rs` (SIMD annotator) | `tir/passes/vectorize.rs` | **landed but DEAD** — backends read zero attrs (doc 05 §1) |
| PGO (`pgo.rs`, `PgoProfileIR`, `pgo_collect.py`) | `molt-backend/src/llvm_backend/pgo.rs`, `ir.rs`, `src/molt/pgo_collect.py` | **dead code / not wired** (doc 06 §1) |
| `perf_scoreboard.py` (warm/cold, provenance) + `bench.py` + suites | `tools/perf_scoreboard.py`, `tools/bench.py`, `tools/bench_suites.py` | **built + CI-gateable**; PyPy/Codon columns present-but-nullable (not installed on host) |
| **ShapeFacts / ClassShape / FieldOffset / ClassVersionGuard** | — | **DOES NOT EXIST** (grep confirms only `op_kinds`/`types.rs` comment refs). doc 46 §3 Q6: "0% — there is no shape system." **The largest single hole.** |
| perf-causality engine (`tools/perf_causality.py`) | — | **NAMED, not built** (doc 46 §4.2) — Rung 0 builds it |

**Live warm-reds the ladder must retire** (doc 51 §5, doc 47 §4, doc 00 §1 evidence):
`bench_struct` 0.04× · `bench_etl_orders` 0.60× · `bench_csv_parse_wide` 0.68× ·
`bench_exception_heavy` 0.55–0.68× · `bench_class_hierarchy` 0.01× (pre dispatch-IC;
re-measure) · LLVM lane `fib` 0.30× / `str_*` 0.65–0.68× · PyPy/Codon `fib` gap.

---

## 3. The rungs

Each rung states: **the class** (what becomes unexpressible), **the missing fact +
its tir representation**, **producers/consumers**, **the benchmark class it heals**,
**the PyPy/Codon gap closed**, and **landable phases with gates**.

### Rung 0 — The measurement path (prerequisite for every rung)

**Class retired:** "optimizing blind" — a perf claim from a noisy red, a warm claim
from alloc counters, a heal that cannot be attributed to a fact. This rung makes
*unmeasured perf work* unexpressible as a landing.

**The fact + representation.** Two artifacts, both *generated*, both `--check`-gated:
1. **The five+1 scoreboards as committed JSON** under `bench/scoreboard/`:
   CPython (`perf_scoreboard.py`, built), PyPy, Codon, Backend (native/LLVM/WASM/Luau),
   Profile (dev/release-fast/release-output). Each cell carries the warm/cold split +
   provenance (`docs/perf/SCOREBOARD.md`). PyPy/Codon columns are *present-but-nullable*
   now; this rung wires the toolchain so they populate (doc 51 §3, scoreboard footer).
2. **The perf-causality fact: `slow_benchmark → top_hot_helpers → missing_fact`**
   (`tools/perf_causality.py`, doc 46 §4.2). It *joins* the cycle profile (`#76`) +
   `tools/call_fact_coverage.py` census + (Rung 1+) the pass-delta ledger. Its output
   is a **typed `MissingFact` enum** (not prose): `{ ExceptionRegionExitOwnership |
   DataclassShape | SplitFieldView | FieldOffset | ClassVersionGuard | RawIntRecursion
   | NoEscapeArg | ... }` — so "this benchmark is slow" terminates as *which rung*.

**Why first:** every later rung's gate IS a scoreboard row; every later rung's
*selection* (which fact next) IS a `perf_causality` row. Without this, the ladder is
unfalsifiable.

**Phases / gates:**
- 0a — commit the scoreboard JSON skeletons + `bench/scoreboard/cold_start_budget.json`
  v0 baseline (`docs/perf/SCOREBOARD.md` already specifies the schema). Gate: scoreboard
  emits, CI runs it non-authoritative on PR, authoritative nightly.
- 0b — install + wire PyPy and Codon references (the columns exist, the host lacks the
  toolchains). Gate: PyPy/Codon cells populate for the matched-semantics subset; mark
  non-equivalent semantics as `non-equivalent`, never win/loss.
- 0c — build `tools/perf_causality.py` emitting the typed `MissingFact`. Gate:
  `perf_causality` self-validates against `call_fact_coverage.py` (a benchmark whose
  hot helper is `molt_inc_ref` must map to a Rung-1 `MissingFact`).

**Composition:** pure C-lane infra (doc 51 §9). Composes with the 21a–e decomposition
by living in `tools/` (not the god-files); no crate dependency. Unblocks all rungs.

---

### Rung 1 — Ownership lattice → Perceus borrow/reuse (RC overhead)

**Class retired:** **refcount churn on values that never needed it.** A `dec_ref`/
`inc_ref` on a *borrowed* value (owned by the caller / a longer-lived container / an
interior borrow) becomes **unexpressible**: the value is typed `Borrowed` and the
syntax-directed translation emits *no* `dup`/`drop` (doc 27 §0). This subsumes the
seven hand-maintained over-release defenses (doc 27 §0.2, C1–C7) into one type.

**The fact + representation.** The four-point ownership lattice **per (alias-root,
program-point)** (doc 27 §1.1), carried as a cached analysis:
```
Ownership ∈ { Owned(k) | Borrowed | Raw | MaybeUninit }   -- tir/passes/ownership_lattice_min.rs (extend)
```
- `Owned(k)` — `k≥1` net refs this function must release or transfer.
- `Borrowed` — a live ref owned by someone else → **no drop obligation** (kills C2/C4 UAF).
- `Raw` — no heap ref (the `Repr::RawI64Safe`/`Bool`/`FloatUnboxed` filter; bottom).
- `MaybeUninit` — no *valid* ref on this path (the `IterNextUnboxed` exhaustion edge; C7).

The lattice already has its keystone slice (`ownership_lattice_min.rs`, 64 KB,
council #58 — `alias-root → ownership state → Python lifetime boundary → ordered
release obligation`). Rung 1 **completes** it into the Perceus carrier and makes the
*borrow signature* of each op-kind a generated registry column (doc 27 §2):
`op_kinds.toml` gains `borrow_signature = { result: Owned|Borrowed, operands: [Owned-in|
Borrowed-in] }` so an unmapped/unknown kind defaults to `Borrowed result` fail-closed
(kills C5/C6). The `ReleaseOp = DecRefPython | FreeInternal | FreeUniqueNoPy(proof)`
typed contract (doc 46 §4.4, council "Free is demoted") is the lowering target.

**Producers:** `alias_analysis.rs` (alias roots, borrow provenance), the generated
`borrow_signature` column, `escape_analysis.rs` (the escape facts that promote
`Owned`→stack). **Consumers:** drop placement (replaces the insert-then-remove of
`drop_insertion.rs` + `refcount_elim.rs` Steps 5/6, doc 27 §0), **CallFacts Rung 2**
(`no_alloc`/`no_escape_args` become `Proven` from the same escape facts), reuse/FBIP
(`reuse_analysis.rs`), and the native value-tracking *deletion* (doc 51 §5, the dead
legacy lane).

**Benchmark class healed:** `bench_struct` 0.04× (per-iter alloc + RC on a non-escaping
`Point(i,i+1)` — `Owned`-proven-unique → stack + zero RC), `bench_gc_pressure`,
`bench_exception_heavy`'s ~22% `molt_inc_ref`/`molt_dec_ref` samples (doc 46 §3 Q3),
every allocation-in-loop benchmark. **PyPy/Codon gap closed:** Perceus garbage-free RC
is the mechanism PyPy gets from its GC + escape and Codon from value semantics; this is
the dynamic-RC reference closer (doc 51 §5 "borrow inference").

**Phases / gates** (doc 27 §7 has the full validator set):
- 1a — promote `ownership_lattice_min` to the full four-point per-(root,point) carrier;
  add the generated `borrow_signature` column to `op_kinds.toml` + `gen_op_kinds.py`.
  Gate: `gen_op_kinds.py --check` green; round-trip tests; **byte-identical** codegen
  (representation only, consumed by nothing yet).
- 1b — drop specialization: drive drop placement from the lattice; delete the C1–C7 ad-
  hoc defenses *as the lattice subsumes each* (asymmetry forbidden — migrate all). Gate:
  the leak gauge `MOLT_ASSERT_NO_LEAK` = actual destruction (council), the ownership
  validators (#75), the full finalizer/weakref/unwind test set (doc 27 §7), differential
  parity green on native+LLVM+WASM.
- 1c — Borrowed elision + reuse/FBIP: a `Borrowed` value receives no dup/drop; in-place
  reuse where the lattice proves uniqueness. Gate: `bench_struct` warm GREEN on the
  scoreboard (the headline); RC-sample count in `perf_causality` drops on
  `exception_heavy`; **no CPython-red regression** on any cell; classify result.
- 1d — `ReleaseOp` typed lowering + native value-tracking deletion: `FreeUniqueNoPy`
  only under `¬MayFinalize ∧ ¬HasWeakrefs ∧ ¬MayResurrect ∧ ¬InnerRefOrdering ∧
  ProvenUnique` (council); delete the dead legacy native value-tracking lane (doc 51 §5).
  Gate: full A-lane safety suite + scoreboard green; binary-size DIMENSIONAL check.

**Composition:** A-lane (P0 safety substrate) → unblocks B-lane Rung 2/3. The
ownership lattice is the council #58 keystone — this rung is the rung-1→rung-2 bridge
of the MM ladder (doc 27 §0.1). Composes with docs 48/49/50 (finalizer/field
ownership) which are *consumers* of the same lattice (no second authority).

---

### Rung 2 — CallFacts complete + typed CallableTarget (dynamic dispatch)

**Class retired:** **the generic call.** A call whose target is statically known (or
class-version-guarded) cannot stay routed through the megamorphic `molt_call`/IC helper
once `CallFacts.target` is `Proven(DirectCodePtr)` / `Guarded(class_version)`. The raw-
marker-decode SIGSEGV class (#59) becomes unexpressible because the target is a *typed
variant*, never a decoded `u64` (doc 47 §1, doc 46 §4.4).

**The fact + representation.** Complete the `CallFacts` record (doc 47 §1, the struct
exists at `call_facts.rs:50`):
- `target: CallTargetFact` → promote to the full **#71 typed `CallableTarget`**:
  `DirectCodePtr | RuntimeMarker | Closure | BoundMethod | MethodDescriptor | Deopt`
  (today `StaticDirect{name} | Opaque`, `call_facts.rs:28`). This is the **Typed
  Runtime Interface** member 1 (doc 46 §4.4) — a generated contract, not a convention.
- `no_alloc` / `no_escape_args` → fill from **Rung 1's escape facts** (today `Unknown`,
  `call_facts.rs:36`). This is the producer→consumer edge that makes Rung 1 pay off at
  call boundaries.
- `deopt_or_guard: Option<GuardFact>` + `FactValue::Guarded(GuardId)` → the
  class-version guard + its deopt edge (the PyPy IC-tiering analogue), fed by **Rung 4
  ClassVersionGuard** and **PGO type frequencies** (doc 06 §2, the speculative-devirt
  evidence). Phase-3 `Guarded` is reserved in the lattice today (`call_facts.rs:122`).

**Producers:** `call_graph.rs::classify_call_op` (target), `escape_analysis.rs` (Rung 1)
for `no_alloc`/`no_escape_args`, `effects.rs` + callee handlers for `no_throw`,
`inliner.rs::classify_inline_eligibility` (already wired). **Consumers (the "pop many
into place," doc 47 §3):** inliner (reads instead of recomputes), **all 4 backend call
lowerings** (one shared fact to lower from — the heterogeneous-backend divergence of
doc 46 §4.7 gets a single source), refcount/ownership (elide inc/dec across the call
from `no_alloc`+`no_escape_args`+`ownership_abi`), the exception normal-edge (`no_throw`
→ zero exception-stack churn, ties doc 45 ExceptionRegion), devirt/monomorphization
(`target`+`Guarded` → direct dispatch under guard).

**Benchmark class healed** (doc 47 §4 — these are the *same* missing-fact bug wearing
different names): `fib` (recursive self-call `DirectCodePtr` + `typed_return=I64` →
unboxed-int recursion), `bench_class_hierarchy` (`BoundMethod`+`Guarded(class_version)`
→ no per-call bound-method alloc), `bench_attr_access` / `bench_etl_orders` (direct
ctor + method devirt), `bench_exception_heavy` (`no_throw` normal edge). **PyPy/Codon
gap closed:** this is the **PyPy-parity lever** (doc 51 §5) — inline-cache tiering +
class-version guards are exactly what PyPy's JIT learns; molt learns it AOT via
CallFacts + PGO. Coverage moves from the measured 28.6% (doc 47 §0) toward 100%.

**Phases / gates** (doc 47 §5):
- 2a — typed `CallableTarget` (#71): replace `StaticDirect|Opaque` with the six-variant
  enum as a generated runtime contract; consume in the inliner (already reads `inlinable`)
  + the four backends' call lowering for the `DirectCodePtr` case. Gate:
  `runtime_contract_audit.py` (doc 46 §4.4) green; `call_fact_coverage.py --check`
  coverage UP; `fib` direct-self-call lands; differential parity all backends.
- 2b — `no_alloc`/`no_escape_args` from Rung 1 escape facts → elide cross-call RC +
  borrowed-arg passing. Gate: RC samples drop in `perf_causality`; coverage UP; no red.
- 2c — `no_throw` normal-edge → zero exception-stack churn (with doc 45 ExceptionRegion).
  Gate: `bench_exception_heavy` warm GREEN; the ~12% exception-bookkeeping samples gone.
- 2d — `Guarded(class_version)` devirt + deopt edge, fed by Rung 4 + PGO. Gate:
  `bench_class_hierarchy` warm GREEN and beats CPython; PyPy ratio recorded; deopt
  correctness differential (guard-fail path takes the slow path, no miscompile).

**Composition:** B-lane. **Depends on Rung 1** (escape facts) and Rung 0 (coverage
census). **Produces** the direct/guarded targets Rung 3 (specialization) and Rung 4
(shape devirt) consume. CallFacts Phase 1a already landed (`call_facts.rs`), so this
rung is *completion*. Composes with the 21a (function-compiler split) decomposition:
the per-backend call lowering is one of the function-compiler responsibilities being
split out (doc 21a) — the shared CallFacts is what makes the four splits implement one
contract.

---

### Rung 3 — Repr convergence + E5 representation specialization (boxing)

**Class retired:** **the boxed hot value.** A value the analyses prove `RawI64Safe`/
`FloatUnboxed` cannot survive boxed through a hot path, and a monomorphic numeric
function body cannot pay the `MaybeBigInt` dispatch tax once it is specialized per
`(Repr, Repr)` tuple. Boxing precision becomes a *proven fact* (`Repr`), not a
per-pass heuristic (doc 51 §5 "Repr/TirType convergence").

**The fact + representation.** The `Repr` lattice already exists
(`representation_plan.rs:838`: `Never/RawI64Safe/Bool/MaybeBigInt/FloatUnboxed/DynBox`),
and `MaybeBigInt` is *the* documented un-unboxable state (every Python `int` floors
there, raised to `RawI64Safe` only by an overflow/range proof — `representation_plan.rs:866`).
Two completions:
1. **Repr/TirType convergence** — make `Repr` the *single* carried representation fact
   on every value (not a parallel side-computation reconstructed at lower-to-LIR). The
   "token-typed unbox keystone" (doc 51 §5): a trusted unbox is legal *only* on a value
   whose `Repr` proves the tag — the validator (#75) makes a trusted-unbox-on-MaybeBigInt
   a *checkable* miscompile, not a latent one (the trusted-unbox class is already
   marked ✓ in doc 51 §8 via typed IR; this rung closes the convergence so no second
   repr authority exists).
2. **E5 representation specialization (the Julia axis, doc 03 §2.1 step 4)** — clone +
   compile a callee per `(Repr, Repr, ...)` argument-Repr tuple proven at the call sites
   (`run_specializer` in `module_phase.rs`, doc 03 §2.1). `add(a,b)` with both
   `RawI64Safe` at every site compiles a body with raw machine `add` and no BigInt
   dispatch. Consumes **Rung 2** `CallFacts.target` (you can only specialize a callee you
   can name) + `typed_return` (to propagate the specialized Repr back).

**Producers:** `overflow_peel.rs` / `value_range.rs` / `scev.rs` (the proofs that raise
`MaybeBigInt`→`RawI64Safe`), `type_refine.rs` (TirType→Repr floor), the E5 specializer
(per-tuple clones). **Consumers:** every arithmetic lowering (raw vs boxed lane),
**Rung 5** (only `RawI64Safe`/`FloatUnboxed` enter SIMD lanes — doc 05 §2 axis 1), the
RC calculus (`Raw` is the no-obligation bottom of Rung 1's lattice — doc 27 §1.2).

**Benchmark class healed:** `fib` (#67 PyPy 0.51×/Codon 0.26× — unboxed-int recursion
needs Rung 2 direct-self-call *and* Rung 3 specialization together), `bench_sum*`,
`bench_prod_list`, `bench_matrix_math`, the LLVM-lane `fib` 0.30× (the missing fact is
unboxed-int recursion *in portable IR* — doc 46 §3 Q7, closed here + Rung 7).
**PyPy/Codon gap closed:** this is **the Codon AOT reference closer** for the numeric
subset (doc 51 §5 "approach/exceed Codon on typed kernels") — Julia/Codon's entire
numeric advantage is per-type specialization; molt gets it from `Repr` + E5.

**Phases / gates** (doc 03 §2):
- 3a — Repr/TirType convergence: one carried `Repr` per value; the trusted-unbox
  validator (#75). Gate: representation_report coverage; `MOLT_VERIFY` repr self-check;
  byte-identical where no new unbox is proven.
- 3b — E3 interprocedural escape+purity summaries (doc 03 §2.2) if not already complete
  (`ip_summary.rs` exists; verify `does_not_capture_param`/`is_pure`). Gate: LICM/CSE
  hoist `y=f(x)` across pure-call boundaries; no red.
- 3c — E5 specializer (`run_specializer`): clone per Repr-tuple under a budget. Gate:
  `bench_sum`/`bench_prod_list` warm GREEN and approach Codon; `fib` (with Rung 2) beats
  CPython and closes the PyPy gap; compile-time DIMENSIONAL check (specialization grows
  code — budget-gated, report binary size).

**Composition:** B-lane. **Depends on Rung 2** (named/typed targets to specialize) and
the existing SCEV/ValueRange/overflow_peel (the Repr-raising proofs, landed). **Produces**
the raw lanes Rung 5 vectorizes. Composes with doc 03 (E3/E5 blueprint) directly — this
rung *is* the perf-ladder framing of doc 03.

---

### Rung 4 — ShapeFacts: ClassShape / FieldOffset / ClassVersionGuard (object/dict/attr shape)

**Class retired:** **the shape-blind field/attr/dict access.** `obj.field`, `d[k]` with
a stable-shape dict, and dataclass construction cannot re-dispatch through a runtime
layout lookup once the access carries a `FieldOffset` shape fact under a
`ClassVersionGuard`. **This is the single largest missing abstraction (doc 46 §3 Q6:
"0% — there is no shape system")** and the root of the ETL/dataclass/ORM benchmark
cluster.

**The fact + representation.** A **new** fact family in `runtime/molt-tir/src/tir/`
(new `shape_facts.rs`, registered as `AnalysisId::ShapeFacts`), built on the existing
`TirType::UserClass(String)` hook (`types.rs:46`, which *already documents* this exact
use: "prove static field offsets for direct load/store"). The records:
```
ClassShape  { class: ClassId, version: ClassVersion, fields: Vec<FieldSlot> }   // the layout
FieldSlot   { name, offset, repr: Repr, ownership: OwnershipKind }              // one slot
FieldOffset { value: ValueId, class: ClassId, slot: u32, guard: FactValue }     // an attached access fact
ClassVersionGuard(GuardId)                                                       // the deopt guard
DictShape   { keys: StableKeySet, value_repr: Repr }                            // value-slot flow for stable dicts
```
Every field is `FactValue`-typed (the shared substrate). A `FieldOffset` is `Proven`
when the receiver is a `UserClass` with no `__slots__`-defeating dynamism, else
`Guarded(class_version)` (direct access under a version guard with a deopt edge), else
`Unknown` (the current full runtime lookup — fail-closed).

**Producers:** the frontend class-layout knowledge (already deduplicated by
`UserClass` qualified names — `types.rs:46`) surfaced as `ClassShape`; `type_refine.rs`
(receiver class identity); the dataclass lowering. **Consumers:** field load/store
lowering (direct offset access, skip `class_layout_size` runtime lookup — `types.rs:34`),
**Rung 2** (`CallFacts.target = MethodDescriptor` + `Guarded(class_version)` for method
devirt; bound-method without per-call alloc), SROA (Rung 5/doc 02 — promote shaped
fields), dict value-slot flow (the `etl_orders` dict path).

**Benchmark class healed** (doc 47 §4, doc 51 §5 "ShapeFacts v0"): `bench_etl_orders`
0.60× (dataclass ctor + field loads — `FieldOffset` direct access + Rung 2 ctor devirt),
`bench_struct` (shaped field promotion compounds with Rung 1 stack-alloc),
`bench_attr_access`, `bench_descriptor_property`, `bench_csv_parse_wide` 0.68× (stable-
shape row dict), `bench_class_hierarchy` (`MethodDescriptor` devirt). **PyPy/Codon gap
closed:** PyPy's **maps/hidden classes** (the V8 "shapes" mechanism) is exactly this;
molt gets the same via `ClassShape`+`ClassVersionGuard`. Codon's struct layout is the
static analogue. This is the ORM/ETL/attr-cluster reference closer.

**Phases / gates:**
- 4a — `ClassShape`/`FieldSlot` for `@dataclass` and simple classes (no metaclass, no
  `__slots__` surprises): the layout fact, built from frontend class info, validated
  against the runtime `class_layout`. Gate: shape-coverage census (a new
  `tools/shape_coverage.py` ratchet, mirroring `call_fact_coverage.py`); byte-identical
  (representation only).
- 4b — `FieldOffset` `Proven` direct load/store for monomorphic `UserClass` receivers.
  Gate: `bench_attr_access` warm GREEN; `LoadAttr` becomes LICM-hoistable (ties doc 02
  MemorySSA — a `Proven` `FieldOffset` load is `ProvenPure`); differential parity.
- 4c — `Guarded(class_version)` + deopt for polymorphic-but-stable receivers; `DictShape`
  for stable-key dicts. Gate: `bench_etl_orders` + `bench_csv_parse_wide` warm GREEN;
  deopt correctness differential (mutating the class version takes the slow path).
- 4d — feed Rung 2's `MethodDescriptor`/`BoundMethod` devirt from `ClassShape`. Gate:
  `bench_class_hierarchy` warm GREEN beats CPython; PyPy ratio recorded.

**Composition:** B-lane. **Depends on Rung 2** (the `Guarded`/`MethodDescriptor`
machinery) and the landed `UserClass` type. **The biggest greenfield rung** (no code
exists). Composes with doc 49 (object-field ownership — `FieldSlot.ownership` is the
same fact doc 49 owns; do not duplicate, *reference*) and doc 02 (a `Proven` FieldOffset
load is the `ProvenPure` typed-slot load doc 02 §1 needs for LICM/MemGVN).

---

### Rung 5 — Loop fact closure: induction / range / overflow / lane stability (slow loops)

**Class retired:** **the un-specialized hot loop.** A counted loop cannot generate full
loop overhead + re-checked invariant type guards + re-multiplied induction expressions +
scalar arithmetic once the loop carries induction (SCEV `AddRec`), range (`ValueRange`),
overflow (`CheckedAdd`/peel), and lane-stability (`Repr`) facts. These facts mostly
*exist* (S6 SCEV/ValueRange landed); this rung **closes** the consumption: re-enable the
gated loop passes and wire the dead SIMD lowering.

**The fact + representation.** No new fact family — this rung is the *consumption
closure* of facts already built (the discipline: a fact that exists but is discarded is
a half-rung). The facts and their homes:
- **Induction:** `scev.rs` `AddRec` recurrences (landed, S6) → IV strength reduction
  (`strength_reduction.rs`, the `FloorDiv`/`Mod` deferral at doc 04 §1 "L1 gap"); the IV
  is a precondition for SIMD lane recognition (doc 04 §1).
- **Range:** `value_range.rs` interval lattice (landed, S6) → BCE (done) + unroll trip-
  count selection + the loop-bound that proves no-overflow.
- **Overflow/lane stability:** `Repr::RawI64Safe` (Rung 3) → only raw/float values enter
  SIMD lanes (doc 05 §2 axis 1); `MaybeBigInt` never does.
- **Re-enable the three exception-gated loop passes** (`loop_unroll.rs:250`,
  `block_versioning.rs:378`, `type_guard_hoist.rs:90`) by gating on
  `has_exception_handlers()` not `has_exception_handling` (doc 04 §2 — the `licm.rs`
  model, already correct), and switch back-edge detection to `TerminatorOnlyPredMap`
  (doc 04 §1, the phantom-back-edge hazard).
- **Wire the dead SIMD lowering:** `vectorize.rs` annotates but every backend reads zero
  attrs (doc 05 §1). Add the three `Dialect::Simd` ops (`VecLoad`/`VecAdd`/`VecReduce`,
  doc 05 §2) + a `vectorize_lower.rs` pass + per-backend SIMD ISA lowering
  (Cranelift `I64X2`/`F64X2`, WASM `simd128`, LLVM vectorize metadata — doc 05 §2).

**Producers:** SCEV/ValueRange (landed), Rung 3 `Repr`. **Consumers:** `loop_unroll`,
`block_versioning`, `type_guard_hoist`, `strength_reduction`, the new `vectorize_lower`.
**Benchmark class healed:** `bench_sum*`/`bench_prod_list`/`bench_matrix_math` (SIMD
reductions — 4–8× theoretical, doc 05 §1), `bench_deeply_nested_loop`, every tight
counted loop (unroll + hoisted guards). **PyPy/Codon gap closed:** trace-like loop
specialization (PyPy) + auto-vectorization (Codon/LLVM); doc 51 §5 "loop optimization
re-enable" + "induction/range/overflow/lane-stability."

**Phases / gates** (doc 04 + doc 05):
- 5a — re-enable the three loop passes (gate flip + `TerminatorOnlyPredMap`). Gate: the
  passes fire on real functions (they currently produce zero work, doc 04 §1); unroll/
  versioning differential parity; no red.
- 5b — IV strength reduction (the `strength_reduction.rs` `FloorDiv`/`Mod` Phase-3
  deferral). Gate: array-indexing loops stop re-multiplying; `bench_matrix_math` improves.
- 5c — SIMD: `Dialect::Simd` ops + `vectorize_lower` + per-backend ISA (conservative cut:
  `FlatListInt` sum/product reductions first, doc 05 §2). Gate: `bench_sum`/`bench_prod_list`
  warm GREEN approach Codon on native AND WASM (the simd128 lane — ties Rung 7);
  Repr-soundness validator (no `MaybeBigInt` in a lane).

**Composition:** B-lane. **Depends on Rung 3** (`Repr` lane stability) and the landed
SCEV/ValueRange/MemorySSA. Composes with doc 04 (loop re-enable) + doc 05 (SIMD) + doc 02
(MemorySSA-of-loads, the LICM-of-field-reads that Rung 4's `FieldOffset` makes `ProvenPure`).

---

### Rung 6 — Resumable-frame ownership + generator fusion (slow generators)

**Class retired:** **the per-iteration generator leak / un-fused generator pipeline.** A
generator frame cannot leak a reference per resume, and a `for x in gen_pipeline(...)`
cannot materialize intermediate sequences, once the resumable frame carries ownership
facts and fusion-eligibility (doc 51 §5 "resumable-frame ownership + fusion eligibility";
the per-iter leak class is already ✓ via doc 46/26, this rung closes fusion).

**The fact + representation.** Two facts on the generator/coroutine region (a
`GeneratorRegion` member of the Region calculus, doc 46 §4.3):
- **Resumable-frame ownership:** which captured values the frame *owns* vs *borrows*
  across a suspend point — the Rung-1 ownership lattice extended to the state-machine CFG
  (`has_state_machine()`, `function.rs:189`, which currently *excludes* generators from
  all structural passes — doc 30 §"Optimization pass coverage"). The fact makes a
  `DecRef` in a resume block referencing a value live only on the first-entry path
  *unexpressible* (the exact unsoundness `function.rs:176-186` documents).
- **Fusion eligibility:** a `FusionBarrier`/`no_heap_move` fact (the deforestation
  authority already reads generated op_kinds — per memory "fusion barrier + escape
  dedup" landed) extended to def-yield generators so producer/consumer loops fuse
  without an intermediate container (the `os.walk` OOM class, doc 51 §6).

**Producers:** Rung 1 lattice (extended to the state CFG), `generator_fusion.rs` (97 KB,
landed) + `deforestation.rs`. **Consumers:** the generator lowering (drop placement on
resume edges), the fusion pass. **Benchmark class healed:** `bench_generator_iter`,
`bench_async_await`, `bench_channel_throughput`, generator-pipeline programs. **PyPy/Codon
gap closed:** generator fusion + frame elision (doc 07 coroutine-elision; PyPy fuses via
trace inlining). **Composition:** A-lane (the resume-edge ownership is a *safety* fact —
a wrong drop double-frees) + B-lane (fusion). **Depends on Rung 1.** Composes with docs
26 (real async generators), 07 (coroutine elision), and the landed `generator_fusion.rs`.

---

### Rung 7 — Portable-IR fact parity (a native opt that doesn't survive to WASM/LLVM/Luau)

**Class retired:** **the backend-local optimization.** A fact that lives only in native
Cranelift codegen (so WASM/LLVM/Luau re-derive nothing and fall back) becomes
unexpressible: every fact on the ladder lives in *portable TIR*, and the backend
support matrix proves each backend lowers it (doc 46 §4.7). "A native win shadowed by a
WASM regression is a portable-IR fact gap, never a backend exception" (doc 51 §3, the
binding rule).

**The fact + representation.** This rung is a **cross-cutting closure**, not a single
new family: the generated **backend support matrix** (doc 46 §4.7) as a column set in
`op_kinds.toml` (`native_lowered`/`llvm_lowered`/`wasm_lowered`/`luau_lowered` ×
`rc_safe`/`exception_safe`/`repr_safe`), rendered by `gen_op_kinds.py`, with
`tools/backend_support_audit.py` checking each backend's *actual* dispatch against the
registry (drift, never inference — doc 46 §4.7 RECON FINDING: the four backends use four
incompatible dispatch paradigms, so a scrape was correctly *refused*). The
per-rung facts (CallFacts target lowering, Repr raw lanes, ShapeFacts offsets, SIMD ops)
each get a matrix row that must be GREEN on all four backends before the rung is "done."

**Benchmark class healed:** the LLVM lane (`fib` 0.30×, `str_*` 0.65–0.68×, doc 46 §3
Q7 — call-target devirt + unboxed-int recursion not present in *portable* IR), the WASM
run-blocked benchmarks, the Luau transform-pipeline gaps. **PyPy/Codon gap closed:**
none directly — this rung closes the *backend scoreboard* (doc 51 §3 table 4: "a native
win never excuses a WASM regression"). **Composition:** C-lane (the matrix) + B-lane
(the actual lowering). **Depends on** every rung above (it certifies their facts cross
to every backend). Composes with doc 06 (PGO block layout for native/WASM, doc 06 §1
items 5/6), doc 14 (target/profile parity audit), and the 21a function-compiler split
(the four backends' dispatch is what 21a decomposes — the matrix is the shared contract
that keeps the splits honest).

---

### Rung 8 — Artifact-footprint facts (cold start / binary size / RSS)

**Class retired:** **the silent footprint regression.** Binary growth, cold-start tax,
and RSS regression become gated facts, not "later" work. Per council, cold-start is an
*artifact-footprint/page-in/codesign* problem, NOT a runtime-init problem (runtime init
measured 0.127ms; doc 51 §9, the `WARN_COLD_FLOOR` verdict of `docs/perf/SCOREBOARD.md`).

**The fact + representation.** Whole-program reachability/DCE → per-attribute liveness
(doc 51 §6 "<2MB binary + per-attr liveness") + the **address-taken-intrinsics** fact
(doc 51 §8 "binary-size address-taken-intrinsics" — an intrinsic whose address is taken
cannot be tree-shaken; making address-taking a *fact* lets the linker drop the rest).
The cold-start budget (`bench/scoreboard/cold_start_budget.json`, v0 baseline → Y1
`startup_tax < 100ms` native release-output, doc 51 §6 / SCOREBOARD.md) is the gate.

**Producers:** whole-program reachability (`reachability.rs`, landed seed), the
address-taken analysis. **Consumers:** tree-shaking / DCE, the WASM hermetic build (doc
51 §6). **Benchmark class healed:** the 43 cold-red cells (doc 51 §5), `bench_startup`,
`bench_import_time`, binary size across all targets. **PyPy/Codon gap closed:** none —
this is the **fourth-dimension** scoreboard (doc 51 §2: warm + cold + RSS + binary
simultaneously world-class). **Composition:** C-lane. **Depends on** the fact substrate
(per-attr liveness needs Rung 4 shapes + Rung 2 call targets to know what is reachable).
Composes with doc 06 (function layout), the WASM split/streaming machinery
(`tir/wasm_split.rs`, `wasm_streaming.rs`), and doc 08 (build speed — compile-time is
the +1 dimension).

---

## 4. The cross-rung invariants (what keeps this a ladder, not eight projects)

1. **One confidence substrate.** Every fact on every rung is `FactValue`-typed
   (`call_facts.rs:117`). `Unknown` is always the fail-closed default. A wrong `Proven`
   is always a miscompile; a conservative `Unknown` is always a missed opt. No rung
   forks this.
2. **One cache contract.** Every fact is an `AnalysisId` with the `AnalysisManager`
   fail-closed invalidation + `MOLT_VERIFY_ANALYSIS=1` self-check (`analysis/mod.rs`).
   Interprocedural facts use the `build_module` + `prepopulate` seeding pattern
   (`call_facts.rs:46`), never a side channel.
3. **One op-semantics authority.** Per-op facts (borrow signature, may_throw,
   backend-lowered, mints-fresh-ref) are generated columns in `op_kinds.toml` checked by
   `gen_op_kinds.py --check`. A rung that needs a per-op fact adds a column; it never
   hand-`match`es opcodes in a pass (doc 46 §1, the discovery-vs-authority rule).
4. **One validator discipline (#75, Alive2).** Each fact ships a checkable obligation
   (the trusted-unbox validator, the ownership leak gauge, the deopt-guard correctness
   differential). A fact without a validator is a half-rung.
5. **One measurement contract.** Each rung's "done" is a GREEN scoreboard row
   (warm-split, provenance, classified GREEN/RED_STABLE/...) — not "tests pass." Each
   rung's *selection* is a `perf_causality` `MissingFact` row.
6. **No second authority for any fact** (doc 49). `FieldSlot.ownership` (Rung 4) IS doc
   49's object-field ownership; `no_throw` normal-edge (Rung 2) IS doc 45's
   ExceptionRegion exit; resume-edge ownership (Rung 6) IS Rung 1's lattice on the state
   CFG. Rungs *reference and consume*; they never re-derive.

---

## 5. Composition with the 21a–e decomposition

The decomposition program (doc 21, plans 21a–e) and this ladder are **orthogonal and
mutually enabling**, not competing:

- **21a (function-compiler split)** decomposes the per-backend `function_compiler.rs`
  monolith. Rung 2 (CallFacts) and Rung 7 (backend matrix) are *why* the split stays
  honest: the four backends' call lowering, once split, must each implement *one* shared
  CallFacts/CallableTarget contract (doc 47 §3, doc 46 §4.7). The ladder supplies the
  shared contract the split's pieces converge on; the split supplies the clean seams the
  ladder's per-backend lowering lands in.
- **21b (crate-graph blueprint)** — the new fact families (Rung 4 `shape_facts.rs`, Rung
  0 `perf_causality.py`) land as focused modules respecting the crate graph (precise
  visibility, test-util feature for cross-crate accessors — per memory "crate extraction
  precise visibility"). No fact family lands in a god-file.
- **21c (frontend mixin decomposition)** — Rung 4's `ClassShape` is *produced from*
  frontend class-layout knowledge; the mixin split (visitors/classes.py, already touched
  on `main`) is where shape production attaches cleanly.
- **21d/21e (CLI / remaining)** — Rung 0's scoreboard wiring lands in the CLI's
  bench/scoreboard surface without bloating the CLI god-file (the structural-debt ratchet
  is RED on main per memory — the ladder's new code must *lower* the ratchet, never raise
  it).

**Binding:** every rung's new code respects the `tools/structural_audit.py --check`
ratchet (debt only goes down) and the `tools/call_fact_coverage.py --check` ratchet
(coverage only goes up) — both already CI-gated (doc 46 §0). A rung that raises the god-
file ratchet is incomplete.

---

## 6. The parallel multi-agent execution model

This ladder runs on the council three-lane model (doc 51 §9, CLAUDE.md "Three-lane
model"), with non-overlapping files so agents do not collide:

| Lane | Owns | Rungs | Blocks |
|---|---|---|---|
| **A** (P0 safety) | ownership lattice, drop placement, resume-edge ownership, leak/finalizer/weakref/unwind tests | Rung 1, Rung 6 (safety half) | blocks B only when memory unsafety makes perf numbers untrustworthy |
| **B** (perf frontier) | CallFacts completion, Repr/E5, ShapeFacts, loop/SIMD, fusion | Rungs 2,3,4,5,6 (perf half) | blocks new features while any benchmark < CPython |
| **C** (infra/scoreboards) | scoreboards, perf-causality, backend matrix, footprint, decomposition support | Rung 0, Rung 7 (matrix), Rung 8 | never decorative; enables A&B to go faster |

**Concurrency rules (CLAUDE.md Build & Test):** max 2 build-triggering agents at once;
each agent exports `MOLT_SESSION_ID` before any build; raw `cargo` also exports
`CARGO_TARGET_DIR`. The ladder's *strict* dependency edges (Rung 1→2→3→4→5) mean those
rungs are **sequential within lane B**, but Rung 0 (lane C) runs *concurrently from day
one*, and Rung 1 (lane A) runs *concurrently with Rung 0*. Once Rung 1+2 land, Rung 3/4
can be two B-agents (3 consumes targets, 4 consumes guards — disjoint files:
`module_phase.rs`/specializer vs new `shape_facts.rs`). Rung 7's matrix (lane C) and Rung
8 (lane C) run concurrently with the late B rungs.

**Per-batch report (binding, CLAUDE.md):** every tranche reports the PERF/SPEED STATUS
block — CPython-red benchmarks + suspected missing fact, regressions, PyPy/Codon deltas
(semantically comparable only), and the single fastest next unlock (one fact / one
file-lane / one gate). If the block cannot be filled, the next task is to *create the
measurement path* (Rung 0), not optimize blind.

---

## 7. Cadence: one class per month (the five-year map)

Mapping the rungs to the doc 51 §8 "~one class/month" cadence (illustrative, gated by
build capacity and the council P0-first ranking — a memory-corruption bug always
outranks a perf rung, CLAUDE.md):

| Month(s) | Rung / sub-phase | Class made unexpressible |
|---|---|---|
| M0 (concurrent) | Rung 0a–0c | optimizing blind |
| M1–M2 | Rung 1a–1d | RC churn on borrowed/unique values |
| M2–M3 | Rung 2a–2d | the generic / megamorphic call |
| M3–M4 | Rung 3a–3c | the boxed hot value / per-type dispatch tax |
| M4–M6 | Rung 4a–4d | shape-blind field/attr/dict access |
| M6–M7 | Rung 5a–5c | the un-specialized / un-vectorized hot loop |
| M7–M8 | Rung 6 | the per-iter generator leak / un-fused pipeline |
| M8–M9 | Rung 7 | the backend-local (non-portable) optimization |
| M9–M10 | Rung 8 | the silent footprint regression |

(Rungs overlap at boundaries because a rung's late sub-phase often unblocks the next
rung's early sub-phase — e.g. Rung 2d `Guarded` devirt consumes Rung 4 `ClassVersionGuard`,
so 2d and 4c co-schedule.) Beyond M10 the ladder continues with the doc 51 §8 tail
(reference-cycle collection — the one thing Perceus does *not* give, doc 27 §0.1 rung 3;
metaclass/`__prepare__` dynamic-exec already ✓; the remaining stdlib-hot-helper classes
surfaced by `perf_causality`).

---

## 8. Risks and their structural (not band-aid) treatments

| Risk | Band-aid (forbidden) | Structural treatment |
|---|---|---|
| **A `Guarded` devirt/shape fact deopts incorrectly** (class version changes at runtime) | special-case the failing program; cap the guard | the deopt edge is a *first-class* `Deopt` `CallableTarget` variant (doc 46 §4.4) with a correctness differential per guard; the guard-fail path takes the proven-slow path, never a wrong fast path. Validator (#75): every `Guarded` fact's deopt edge is exercised by a mutation test. |
| **E5 specialization explodes code/compile time** | disable specialization; per-benchmark budget tweak | a *budget fact* on the specializer (doc 03) gated by the binary-size + compile-time scoreboard dimensions; profile-guided (Rung 0 PGO) selection of which tuples to specialize — specialize the *hot* tuples only, report the DIMENSIONAL cost. |
| **Rung 1 lattice change regresses RC correctness** (the seven UAF classes) | add an eighth ad-hoc defense | the four-point lattice *subsumes* all seven (doc 27 §0.2 mapping C1–C7 → lattice readings); the leak gauge + ownership validators (#75) are the safety net; migrate all seven sites (asymmetry forbidden, CLAUDE.md). |
| **ShapeFacts greenfield is large; risk of a partial shape system** | ship offset facts for dataclasses only, leave the rest "for later" | the `FactValue` lattice makes partiality *sound*: an un-shaped access is `Unknown` = the current full runtime lookup (fail-closed). Coverage grows monotonically via `shape_coverage.py --check`; no program is ever miscompiled by a missing shape, only un-optimized. |
| **A native opt fails to cross to WASM/LLVM/Luau** | a backend-specific exception in the scoreboard | Rung 7 backend matrix: the fact's matrix row is RED until all four backends lower it; a native win with a WASM red is a *portable-IR fact gap* (doc 46 §4.7), gated, not excepted. |
| **A perf claim from a noisy/stale measurement** | accept the green run | `perf_scoreboard.py` provenance (anti-stale-lore) + `>=5` samples + CV stability + GREEN/RED_STABLE/RED_NOISY/TIE/DIMENSIONAL_WIN classification (CLAUDE.md); no warm claim from alloc counters alone (alloc = a memory-dimension signal). |
| **A rung lands the fact but discards it** (the half-rung trap) | "representation only, consumer later" as a permanent state | a rung is *defined* as fact + consumer + validator + green row (§1). A representation-only sub-phase (e.g. 1a, 4a) is acceptable ONLY as an explicitly-noted intermediate within the same rung's arc, never as the rung's terminus (CLAUDE.md "structural change as the unit of work"). |
| **Build capacity / OOM with parallel agents** | exceed the agent cap to go faster | max 2 build-triggering agents, `MOLT_SESSION_ID` isolation, `molt clean --apply --kill-processes` between sessions (CLAUDE.md). Lane discipline keeps file sets disjoint so agents rarely both need a build of the same crate. |

---

## 9. Definition of done (the ladder, not a rung)

The ladder is complete when, on `tools/perf_scoreboard.py` run authoritatively against
`origin/main`, across all 69 `BENCHMARKS` × {native, LLVM, WASM, Luau} × {dev-fast,
release-fast, release-output}: **zero `FAIL_ENGINE` cells**; PyPy ratios recorded and
≥1.0 on the dynamic subset or the missing fact named; Codon ratios recorded and
approaching/≥1.0 on the static subset or marked non-equivalent; the cold/RSS/binary
dimensions within their v0→Y1 budgets; and every green cell *stays* green because the
slow lowering it once admitted is now structurally unexpressible — the property is a
generated, validated IR fact, not a heuristic the next refactor can lose.

At that point "make this Python program faster on molt" is answered the same way every
time: name the missing fact, add it to the ladder, validate it, watch the row go green.
