<!-- Integrated parallel build program — master coordination artifact. Refreshed 2026-06-05 (wf_18b24759-006 lineage). Supersedes the 2026-06-04 synthesis: ledger landed, E1 activated on native+WASM, RC + LLVM-exception Tier-1 substrates added, L4 arc corrected. -->

# molt Compiler Foundation — Integrated Build Program

This is the master coordination artifact for the multi-tier foundation program.
It tracks what has landed, what is in flight, and the dependency edges that
order the remaining work. Design docs `01`–`20` in this directory carry the
full per-arc blueprints; this doc is the index and the schedule. Every claim
below is grounded in a commit hash or a numbered design doc.

## Status Legend

- **DONE** — landed on main, verified
- **IN FLIGHT** — active or partially landed; sub-phases noted
- **HELD** — implementation complete, driver wiring pending
- **OUTSTANDING** — designed, not yet started

---

## 1. Landed Ledger

The substrate and correctness foundation is largely built. Completed arcs:

### Tier-0 substrates (analysis/cost-model foundation)

| Arc | Commit | What landed |
|-----|--------|-------------|
| S1 AnalysisManager + PassManager | `ef284d182` | `TirPass::run(func, am)`, 7 analyses (PredMap/ImmediateDoms/DomChildren/ExecReachable/StrictReachable/LoopForest/DefMap); deleted 3 duplicate dominator impls; `MOLT_VERIFY_ANALYSIS=1` guard |
| S2 TargetInfo / cost model (TTI) | `9ff5d2e00` | `run(func, am, tti)`; deleted magic profitability constants |
| S3 effects oracle | `8b6b88286` | `effects.rs` single source of truth; licm/gvn deleted their dup lists; Div/Mod/Pow CSE-safe-but-not-movable asymmetry encoded |
| S4 call-graph + module phase | `7915b29a0` | `call_graph` + `ip_summary` + `module_phase`; replaced SimpleIR leaf-detection with TIR call-graph (byte-identical) |
| S6 SCEV + ValueRange | `cd66f365e` | SCEV + value-range as S1 analyses; rewrote BCE on range queries; deleted ~550 lines of ad-hoc RangeFact/GuardFact/KnownLength/prove_guard_bound; **fixed a latent silent-OOB** (old BCE elided length checks on non-negative const indices) |
| S5-ph1 alias analysis | `fb574b289` | First-class alias oracle; deleted the 4 ad-hoc barrier lists; conservative superset |
| S5-ph1 precision (CheckException) | `5d6274e04` | `CheckException` is not a memory clobber in the alias oracle |
| S5-ph1.5 TypedField regions | `d8275ed8a` | Class-aware TypedField alias regions from guarded-field ops' own runtime guards |
| S5-ph2a MemorySSA | `4e3c7ca7d`, `9f1097147` | Standalone analysis (types + `compute_standalone` + unit tests); registered with the AnalysisManager |
| S5-ph2b MemGVN | `081bda9e8` | Store-to-load forwarding + redundant-load elimination over MemorySSA |
| S5-ph2d SROA | `55b35d870` | Dead-object field promotion over MemorySSA |

### Tier-1 correctness

| Arc | Commit | What landed |
|-----|--------|-------------|
| C1 BCE natural loops | `850077e7f` | Dominator-based natural-loop collection (fixed unsound `collect_loop_body`) |
| C2 needs_exception_stack | `430e09793` | Polarity trap fixed — exception observation now universal; depth-bookkeeping gated on try/with; byte-identical CPython |
| C3 async _poll panic | `29cd7765b` | Loop-region external-reentry guard fixed the "invalid labels" panic |
| Import-error parity | `2cecc1415`, `f9afd99d3` | `ModuleNotFoundError` for missing modules; `ImportError` (not AttributeError) for `from M import missing` via dedicated `OpCode::ModuleImportFrom` (wired through every pass + all 5 backends) |

### Tier-2 / perf engine

| Arc | Commit | What landed |
|-----|--------|-------------|
| E1 inliner core (phases a/b) | `f14b196ce` (hardened `951938075`) | `tir/passes/inliner.rs`: clone/remap/splice/is_inlineable; refcount arg-IncRef guard; SSA verify; all-4-loop-metadata transfer |
| E1 phase-c (obs-only EH inlining) | `6d9962a98` | Inlines observation-only callees; `has_exception_handlers()` handler⟂observation split; fresh exception-label remap |
| **E1 ACTIVATION (native+WASM)** | `7512919fa` | **Routed native+WASM codegen through the `run_module_pipeline`-inlined `TirModule`; DELETED `passes::inline_functions` + the dead needs_inlining gate.** The inliner is no longer dormant on those two targets (see §2 for the LLVM/Luau gap) |
| module_slot_promotion | `b9188ab1c` | mem2reg of module-dict slots across loops — the bench_sum 16× fix (design 10) |
| dispatch-IC (fused method/super) | `798f9b136` | Allocation-free fused method/super dispatch — class_hierarchy 7× |
| CheckedAdd primitive (peel phase A) | `c2a373a3a` | `OpCode::CheckedAdd` exact signed-overflow add, all 4 backends |
| overflow_peel dual-loop peel (bug #15) | `e267a4f5a` | Dual-loop accumulator peel; bench 2.2× slower → **14× faster than CPython** |
| canonical counted-loop contract | `fae639e94` | Route B counted-loop contract; unlocks `loop_unroll` (L4 producer) |
| RawI64 carrier unification | `2639d490b` | Full-range value-keyed seeds, overflow-safe boxing, proof-gated triple, LIR naming unified |

### Backend correctness / parity / DX

| Arc | Commit | What landed |
|-----|--------|-------------|
| 3 LLVM-lane miscompile fixes | `0fd0e9794` | call-ABI boxing + first-class `ConstBigInt` + dead-edge phi dataflow; peel matrix 5/9 → 9/9 on LLVM |
| dir_fd `*at` intrinsics | `ca2c57ff1` | readlink/symlink/stat/lstat/rename/replace/link/utime dir_fd variants (design 19, 13 differential tests) |
| god-file split | `34e3bddbf` | `molt-backend/src/lib.rs` 6,928 → 264 lines (move-only, 0 behavior change); thin facade + focused submodules |
| LLVM toolchain unblock | `f91711944`, `02e5e9cc0` | host-triple staging + per-app intrinsic resolver; extern functions not lifted/inlined as shared-stdlib externals |
| WASM gc-proposal parse fix | `ab82ca479` | "rec group requires gc proposal" parse failure on linked modules |

The performance evidence (STATUS.md `bench-summary`, last full run 2026-05-23,
predates several of the above wins): top speedups class_hierarchy 6.94×,
bytes_find 6.27×, sum 5.30×; remaining regressions led by `bench_struct` 0.04×
(SROA + RC substrate target) and `bench_exception_heavy` 0.55×.

---

## 2. Tier-1 Correctness Substrates Still Open (highest priority)

Two load-bearing Tier-1 substrates were discovered on 2026-06-05. Both are
correctness blockers and outrank the remaining Tier-2/3 perf work.

### RC-1 — RC Ownership / Drop Insertion (design 20, `bc67f6406`) — **the #1 correctness blocker**

**The bug**: molt allocates every expression-result heap object with
`ref_count = 1` and **never decrements it**. The runtime `dec_ref` machinery,
the TIR `DecRef` opcode, and `molt_dec_ref_obj` all exist and are correct in
isolation — what is missing is the compiler pass that *inserts* `DecRef` ops for
expression temporaries. The result is a whole-program memory leak on every
refcounting backend (native, LLVM, WASM; Luau is GC-managed, no-op).

**Evidence** (design 20 §Executive Summary): a 1M-iteration BigInt accumulator
loop produces 3,000,635 allocations, **0 deallocations**, 297 MB RSS at exit;
a 30M-iteration string concat OOMs at the 512 MB cap. The native backend has
only a partial per-loop-variable dec-ref heuristic
(`function_compiler.rs:3566-3628`) that fires on loop-body reassignment of one
narrow shape — a symptom-suppressor, not an ownership model, and invisible to
the TIR pipeline.

**The fix** (design 20, 5 phases): a first-class TIR `DropInsertion` pass,
post-optimization / pre-lowering, that inserts `DecRef` at every value's last
use, with representation-aware filtering (raw scalar lanes carry no refcount —
the overflow-peel fast loop gets **zero** RC ops, preserving the perf contract),
exception-edge correctness (drop once on each of normal + handler paths),
loop-carried ownership (drop the prior phi before the back-edge), and suspension
survival (IncRef live-across-yield values into the coroutine frame). After
insertion, the existing `refcount_elim` pass elides redundant ops.

Phase map: **P1** runtime observability (`DEALLOC_COUNT`, `MOLT_ASSERT_NO_LEAK`,
`tests/differential/memory/`); **P2** `TirLiveness` analysis (`AnalysisId::Liveness`);
**P3** core `DropInsertion` pass + the `loop_reassign_old_val` double-drop guard;
**P4** WASM `DecRef`/`IncRef` wiring (`lower_to_wasm.rs`) + Luau no-op; **P5**
delete the legacy SimpleIR loop-reassign + `rc_coalescing` paths. Non-goals:
reference-cycle collection (future design 21), immortal-constant interning,
Perceus reuse-token emission (future design 22).

### RC-2 — LLVM exception-CFG arc — **in flight, no numbered design doc yet**

**The bug**: the LLVM backend's `compute_function_rpo`
(`llvm_backend/lowering.rs:645`) walks **terminator edges only**. The
`CheckException` op introduces a mid-block branch to a handler block that is not
visible in the TIR block terminator (see the comment at `lowering.rs:5435`:
"Mid-block branches from CheckException (not visible in TIR terminators)").
Handler blocks reached only by those CheckException edges are therefore never
included in the RPO walk → never lowered → emitted as `unreachable`
(`build_unreachable`, `lowering.rs:503,512,5405`) → control that does reach them
hits `llvm.assume`-style UB. The consequence is **0/25 exception differential
tests passing on the LLVM target.**

A second, related granularity class: the mid-block `CheckException` edge splits a
block's effective dominance below TIR block granularity, so phi/dominance
reasoning over those edges is coarse (the `check_exception_edge_feeds_handler_phi`
test at `lowering.rs:9096` exercises exactly this seam).

**The fix** (summary — to be promoted to a numbered design doc): make the LLVM
RPO/successor walk CheckException-aware so handler blocks are discovered and
lowered, and refine the dominance granularity at the mid-block exception edge.
This is the LLVM analogue of the C2/C3 exception-observation correctness arc on
the Cranelift/WASM paths.

**Dependency**: RC-1 Phase 3 on the LLVM target *requires this arc first* —
drop ops on exception paths need the handler blocks to actually be lowered, or
the inserted `DecRef`s land in `unreachable` blocks. Land RC-2 before RC-1's
LLVM coverage.

---

## 3. Dependency DAG (remaining work)

```
TIER 1 (correctness — open)
  RC-1 DropInsertion substrate     OUTSTANDING  (design 20; THE #1 blocker)
    └─ P3-on-LLVM requires: RC-2
  RC-2 LLVM exception-CFG arc      IN FLIGHT     (no numbered doc; §2)
    └─ requires: nothing (LLVM-local)

TIER 2 (engine)
  E1-e  inliner activation
    native + WASM                  DONE  7512919fa
    LLVM                           OUTSTANDING  (LLVM never calls run_module_pipeline)
    Luau                           OUTSTANDING  (doc 14 Gap 1: module phase entirely absent)
  E3  IP escape + purity summaries OUTSTANDING  (design 03/12; requires S4 DONE)
  E4  IPSCCP                       OUTSTANDING  (requires S4, E1-e LLVM/Luau for full IPO)
  E5  monomorphization (Julia axis) OUTSTANDING (design 03; requires S4, E1 clone infra)
  S5-ph2c cross-block DSE          OUTSTANDING  (design 02; MemorySSA DONE)
  S5-ph2e LICM-of-loads            OUTSTANDING  (design 02; MemorySSA DONE)

TIER 3 (consequences)
  D1  generator fusion / CoroElide OUTSTANDING  (design 07; requires E1 active, SROA DONE)
        → retires native os.walk + itertools intrinsics
  DX  build-speed (LTO/sccache/fc split)  OUTSTANDING (design 08; independent)

TIER 4 (loops / SIMD)
  L4  range_devirt ordering + IV strength reduction  OUTSTANDING (design 04)
        loop_unroll producer unlocked by fae639e94 (counted-loop contract)
  L2  real SIMD codegen            OUTSTANDING  (design 05; requires S2/S5-ph1/S6 DONE, L4)
  L1  IV canonical + FloorDiv/Mod SR  OUTSTANDING (requires L4)

TIER 5 (whole-program feedback / size)
  W1  PGO end-to-end              OUTSTANDING  (design 06; independent of engine)
  W3  per-attribute DCE           OUTSTANDING  (design 09/13; SimpleIR-only; <2MB lever)

TIER 6 (verification)
  V1  translation-validation      OUTSTANDING  (gates L2/E5/D1 risky lowering)

PARITY arcs (cross-cutting)
  Luau CheckedAdd lowering         OUTSTANDING  (design 15; portable helper, Luau f64 model)
  asyncio-wasm event loop          OUTSTANDING  (design 18; WASI poll_oneoff, 4 blockers)
  Luau module-phase parity         OUTSTANDING  (design 14 Gap 1)
  CPython surface / stdlib / GPU   ongoing      (design 16)
  ecosystem / third-party compat   ongoing      (design 17)
```

### Note on the corrected L4 framing

The previous version of this doc led with an "L4 Arc 1 gate-flip" — flipping
`has_exception_handling` → `has_exception_handlers()` in `loop_unroll`,
`block_versioning`, and `type_guard_hoist` — as the highest-leverage immediate
unlock. **That framing was stale and the gate-flip is inert in production.**
Verified (design 04 + prior session forensics): there is no TypeGuard *producer*
in the production pipeline, and range-loops carry no iterator ops, so the three
passes have nothing to fire on even with the exception gate opened. The real L4
arc is the producer chain: **TypeGuard generation → loop-shape canonicalization
→ the gate** — i.e. you must first emit the loop shapes and type guards those
passes consume. The counted-loop contract (`fae639e94`, "Route B") landed the
first half of this (a canonical counted-loop shape that unlocks `loop_unroll` as
a producer); `range_devirt` ordering and IV strength reduction (design 04)
remain. Do not schedule a bare gate-flip as if it were a perf unlock.

---

## 4. Critical Path and Scheduling

### 4.1 Correctness gate (precedes all perf work)

```
RC-2 (LLVM exception-CFG)  ──┐
                             ├─→ RC-1 P3+ (LLVM drop coverage) → RC-1 P4 (WASM) → RC-1 P5 (delete legacy)
RC-1 P1/P2/P3-native+WASM ──┘
```

RC-1's native+WASM legs (P1 observability, P2 liveness, P3 core pass) are
independent of RC-2 and can land first; they immediately stop the leak on the
two activated targets. RC-1's LLVM coverage waits on RC-2. This is the unmovable
front of the program: a whole-program memory leak outranks every perf arc, and
the `MOLT_ASSERT_NO_LEAK` + `safe_run.py --rss-mb` gates from design 20 §5 make
every subsequent arc continuously leak-checked.

### 4.2 Perf keystone

**E1-e LLVM + Luau activation is the pending perf keystone.** Native and WASM
already route codegen through the inlined `TirModule` (`7512919fa`); the inliner
fires in production on those targets and the SimpleIR dual path is deleted. Two
gaps remain (design 14):

- **LLVM (Gap 2)**: the LLVM branch consumes the inliner's changed bodies
  *transitively* (it re-lifts the already-mutated `ir.functions`), but never
  calls `run_module_pipeline` itself and re-runs the full per-function pipeline
  from post-inline SimpleIR — a compile-time asymmetry, not a miscompile. LLVM
  also gets no molt-level inlining decisions of its own (it relies on its `-O2`
  IPO).
- **Luau (Gap 1, blocking parity)**: `run_module_pipeline` is *never* called on
  the Luau path (`main.rs` bypasses `SimpleBackend::compile` entirely). The
  inliner and slot-promotion produce zero benefit for Luau. Insertion point:
  `main.rs` after the per-function loop / `eliminate_dead_ops`.

Activating those two closes the IPO context for E3/E4/E5 across all four targets.

### 4.3 The size/startup lever

The `<2 MB binary / <50 ms cold start` targets (ROADMAP "Active Blockers") are
driven by three converging arcs, none of which touch correctness:

- **W3 per-attribute DCE** (design 09/13): make `func_new` liveness precise —
  drop a module attr's body when it is provably never read in the static graph.
  SimpleIR-only, fail-closed. Expected 650 KB–1.1 MB reduction; directly attacks
  the 4.31 MB `empty.py` floor.
- **RuntimeSurfacePlan** (ROADMAP medium-term): one per-intrinsic/per-primitive
  reachability authority shared by native link-roots, the WASM import/export
  manifest, and the intrinsic resolver — so a tiny program stops linking async /
  GPU / networking / logging it cannot reach.
- **DX build-speed** (design 08): thin LTO, shared sccache, `function_compiler.rs`
  split — wall-clock, not size, but it is the multiplier on every other arc's
  iteration cost.

### 4.4 The generator-fusion strategic prize

**D1 generator fusion / CoroElide** (design 07) retires the native-iterator
treadmill. Every stdlib function that needs to be fast (os.walk, itertools.*)
currently demands a hand-written ~300–800-line Cranelift intrinsic bound to one
backend. Fusing `def`-with-`yield` generators to machine-code-equivalent loops
lets those live as pure Python and the intrinsics be deleted — including the
os.walk rewrite that closes the still-open os.walk OOM/SIGSEGV (the native
implementation is deleted from the tree at HEAD; the OOM/recursion bugs stay
open until fusion lands). Requires E1 active (DONE native/WASM) + SROA (DONE,
`55b35d870`) for frame-slot promotion.

### 4.5 Parity arcs

- **Luau CheckedAdd** (design 15): the `overflow_peel` arc landed `CheckedAdd` on
  4 backends, but Luau needs a portable helper — Luau is f64-only, has no i64
  overflow signal, so `CheckedAdd` lowers to `molt_checked_i64_add(a,b) → (a+b,
  false)` (overflow never fires; byte-identical to the un-peeled Luau path). No
  target-conditional pass logic.
- **asyncio-wasm** (design 18): 4 blockers (event-loop I/O has no WASM pathway —
  `add_reader`/`add_writer` are `#[cfg(not(wasm32))]` stubs; table-ref trap;
  zipimport; thread unavailability). The structural fix is a first-class WASM
  event loop over WASI `poll_oneoff`.
- **Luau lag** generally (design 14, ROADMAP item 8): Luau trails native/WASM —
  no module phase, a `< 4`-op skip heuristic with no parallel elsewhere, no TIR
  cache. Drive to checked parity per the support matrix.

---

## 5. Five-Year Mapping

The tiers above map onto the long-horizon outcomes. Perf contract throughout
(CLAUDE.md, ROADMAP): **molt must be faster than CPython on every benchmark,
across every target (native, WASM, LLVM, Luau) and every profile (release-fast,
dev-fast, debug-with-asserts).** Headline targets: sieve → 1000×, cold start
< 50 ms, binary < 2 MB.

| Horizon | Outcome | Carrying arcs |
|---------|---------|---------------|
| **Y1** | Foundations + CPython ≥3.12 parity | Tier-0 substrates (S1–S6, **DONE**), Tier-1 correctness (C1–C3 **DONE**; **RC-1 + RC-2 the open front**), import/surface parity (designs 14–19) |
| **Y1.5** | Ecosystem / dlopen | `libmolt` extension support (ROADMAP long-term), ecosystem compat (design 17), third-party import graph |
| **Y2** | Perf frontier | E1-e full activation, E3/E4/E5 IPO (design 03/12), S5-ph2c/2e memory opt (design 02), L4/L2/L1 loops + **real SIMD** (design 04/05), **W1 PGO** (design 06) |
| **Y2–3** | ML / AI | tinygrad fidelity + DFlash + GPU codegen (currently CPU-sim; ROADMAP molt-gpu Movement/Contiguous blocker), CLAUDE.md tinygrad/DFlash fidelity policy |
| **Y3** | Systems | D1 generator fusion / CoroElide (design 07) retiring native iterators; RuntimeSurfacePlan; the `< 2 MB / < 50 ms` size+startup arc (W3, design 09) |
| **Y4** | MLIR + formal verification | MLIR output surface (ROADMAP), V1 translation-validation (Tier 6) gating L2/E5/D1 |
| **Y5** | Leadership | "Mojo/Julia speed, Python semantics" delivered across all four targets; the full evidence matrix (native/WASM/Luau/MLIR) green on cold-start, size, and throughput simultaneously |

The single highest-leverage ordering: **finish the RC correctness front (RC-1 +
RC-2) → complete E1-e on LLVM/Luau → E3/E5 IPO → L2 SIMD / W1 PGO → D1 fusion +
W3 size.** Correctness gates perf; perf gates the five-year claim.

---

## 6. Cross-Cutting Risks

These survive from the prior synthesis and remain live.

### Risk 1: RC double-drop (design 20 R1)

The SimpleIR loop-reassign dec-ref (`function_compiler.rs:3566`) fires on the
same value as the new TIR `DropInsertion` pass → refcount underflow →
use-after-free. The `!drop_inserted` guard (RC-1 P3) must land *with* the pass,
not after, and the `drop_inserted` function attr must round-trip through
`lower_to_simple`. Triple-redundant stack-value defense already exists
(escape_analysis + refcount_elim Step 2a).

### Risk 2: RC repr-filter miss on the peel fast loop (design 20 R3)

If the `Repr` filter misclassifies an overflow-peel `CheckedAdd` result
(`RawI64Safe`), a raw i64 register is passed to `molt_dec_ref_obj` → type
confusion. The filter keys on `repr_by_value`/`Repr::default_for`; `CheckedAdd`
results are promoted to `RawI64Safe`. Guarded by the `bench_sum` zero-new-ops
smoke test.

### Risk 3: E5 monomorphization inherits the C2 exception flag

E5 clones functions and re-runs the pipeline on clones. The clone's
`is_specializable` predicate must use `has_exception_handlers()` (handler check),
not `has_exception_handling` (the universal-observation flag from C2) — else all
clones refuse specialization. Mirror the L4 fix.

### Risk 4: SROA + repr safety (design 02)

When SROA forwards a stored value to a load, the forwarded value inherits the
stored value's repr. `repr_by_value_for` must run fresh post-SROA so a
`MaybeBigInt` store does not silently become a trusted-unbox. The
`apply(f, 1<<60, 7)` bigint oracle covers this.

### Risk 5: W3 stdlib cache coherence (design 09/13)

The Python BFS (`cli.py:_reachable_function_names_for_stdlib_cache`) and the Rust
DFE BFS (`passes.rs:eliminate_dead_functions`) must produce identical live sets.
Update both atomically + bump `_SHARED_STDLIB_CACHE_SCHEMA_VERSION` or cached
builds silently over-link.

### Risk 6: L2 SIMD — atomic deletion of the 4× scalar unroll (design 05)

The only current "vectorization" is a manual 4× scalar unroll in
`function_compiler.rs`. Deleting it before the real SIMD emission is wired
regresses `bench_sum_list`. Deletion + new emission are ONE atomic arc. Always
query `tti.vector_width_*` (from `SimdCaps::detect_host()`), never hardcode AVX2.

### Risk 7: matches!-oracle silent miscompile on new opcodes

`effects.rs::opcode_may_throw` / `opcode_is_side_effecting` use `matches!` which
defaults to `false` for unlisted opcodes (the `ModuleImportFrom` lesson). On any
opcode add, audit both oracles. New *analyses* (e.g. `AnalysisId::Liveness` for
RC-1) must be added to `AnalysisId::ALL` and every exhaustive `match` — the
compiler enforces this for `match`, not for `matches!`.

### V1 translation-validation gate

`function_compiler.rs` (~38K lines) has no semantic-equivalence guarantee. The
riskiest lowering arcs — L2 (SIMD into Cranelift), E5 (clone + pipeline re-run),
D1 (generator-frame restructuring) — each add a new TIR→SimpleIR→native path with
no formal validation. Extend `tools/translation_validator.py` to validate
TIR→SimpleIR round-trips for transformed functions before landing L2/E5; document
"pre-V1, differential-matrix-substituted" otherwise.

---

## 7. Build Discipline

Per CLAUDE.md, unchanged:

- Max 2 concurrent build-triggering agents; production-codegen changes
  (RC pass, E1-e LLVM/Luau, backend paths) serialize through the daemon socket.
- Each agent exports `MOLT_SESSION_ID` before any build for its own
  `target-<id>/` directory.
- Never run a compiled molt binary raw — route smoke/bisect/profile through
  `python3 tools/safe_run.py --rss-mb <cap> --timeout <s> -- ./binary`. RC-1's
  leak regressions surface here as exit 137 (RSS cap) or `MOLT_ASSERT_NO_LEAK`
  abort.
- Structural change is the unit of work: an arc is not done until its last
  sub-phase lands. No localized hacks committed as placeholders; baton-pass
  notes for unfinished arcs.

---

## 8. Design-Doc Index

| Doc | Arc |
|-----|-----|
| `01`, `01b` | E1 inliner activation (native+WASM landed `7512919fa`; LLVM/Luau open) |
| `02` | S5 MemorySSA + SROA + MemGVN + cross-block DSE (ph2a/2b/2d landed; 2c/2e open) |
| `03`, `12` | E3 IP escape/purity summaries + E5 monomorphization |
| `04` | L4 loop transforms (corrected arc — producer chain, not gate-flip) |
| `05` | L2 real SIMD codegen |
| `06` | W1 PGO end-to-end |
| `07` | D1 generator fusion / CoroElide → os.walk-as-Python |
| `08` | DX build-speed |
| `09`, `13` | W3 per-attribute DCE (the `<2 MB` lever) |
| `10` | module-global loop promotion (bench_sum 16× — landed `b9188ab1c`) |
| `11` | bug-#15 dual-loop overflow peel (landed `e267a4f5a`, `c2a373a3a`) |
| `14` | target × profile parity audit (E1 LLVM/Luau gaps) |
| `15` | Luau CheckedAdd lowering plan |
| `16` | CPython surface / stdlib / GPU gap audit |
| `17` | ecosystem / third-party compat gap audit |
| `18` | asyncio-wasm event-loop fix plan |
| `19` | os dir_fd `*at` intrinsic design (landed `ca2c57ff1`) |
| `20` | **RC ownership & drop insertion** (the #1 correctness blocker) |
