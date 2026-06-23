# 51 — Molt 10-year roadmap: the compression ladder to Python-at-C-speed

Status: NORTH STAR (2026-06-09). The single strategic map. Everything below is in
service of one contract and one method.

## 0. The contract (north star)
- **Drop-in CPython ≥3.12 → 3.14 deterministic parity** — every feature, edge, and
  corner case, except `exec`/`eval`/`compile`, runtime monkeypatching, unrestricted
  reflection.
- **Faster than CPython on EVERY benchmark, EVERY target, EVERY backend, EVERY
  profile.** This is the FLOOR, release-blocking, not an aspiration. Any benchmark
  <1.00× vs CPython is RED.
- **Approach / match / beat PyPy** (dynamic-JIT reference) on pure-Python dynamic
  workloads and **Codon** (AOT-static reference) on the statically-compilable numeric/
  loop/data subset.
- **Run everywhere tinygrad runs and more** — GPU/ML (tinygrad + DFlash) is a
  first-class, exact-fidelity product, not a bolt-on.
- **All four footprint dimensions world-class simultaneously:** warm perf, cold start,
  peak RSS, binary size (<2MB) — plus fast compile.

## 1. The method (why molt keeps winning classes, not instances)
**The recurring root cause of every molt bug/slowness:** the compiler reconstructs
Python semantics — lifetimes, types, shapes, call targets, ownership, effects — from
LOW-LEVEL events AFTER the high-level meaning was already lost (the MLIR lesson: keep
the structure that matters until the facts are extracted).

**The cure: a SEMANTIC FACT PLANE.** First-class, cached, *generated*, *checkable*
facts attached to the IR — each one makes a whole CLASS of wrong/slow programs
**unexpressible**. The fact families:
op_kinds registry · operand-ownership table · FinalizerSensitive · CallFacts ·
Typed CallableTarget · ShapeFacts · ownership lattice · ExceptionRegion · Repr/TirType
lattice · class identity/version.

**The cadence: retire one CLASS per month** (the compression ladder), never one
benchmark or one bug. The unit of work is the structural change / the missing fact.
Ten years × ~one class/month ≈ 120 classes of slowness and wrong-answers made
*unexpressible*.

**The discipline (binding):** prove the mechanism (measure, don't reason); keep the
four+1 scoreboards green; every fact gets an Alive2-style validator (a checkable
obligation, not a convention — #75); no second authority for any fact (doc 49); no
rushed memory surgery; verified refusals leave durable artifacts.

## 2. The product surface (the full matrix)
### Targets / backends
| target | role | reference ceiling |
|---|---|---|
| Native (Cranelift) | default; fast compile, JIT-class codegen | Codon (AOT) |
| LLVM | max-opt AOT; release-output codegen; warm-perf ceiling | Codon / C |
| WASM | browser/edge; portable. A native opt that doesn't survive to WASM is an **IR-fact gap**, not a WASM gap | — |
| Luau | embedding / sandbox | — |

### Profiles (separate products; none hides another's regressions)
| profile | optimizes | invariant |
|---|---|---|
| dev-fast | compile latency, debuggability | NO silent runtime regression |
| release-fast | compile + shipped-perf (daily driver) | held to shipped-perf standards |
| release-output | max opt, smallest/fastest artifact (LLVM -O3, wasm-opt -Oz) | the ceiling |
| debug-with-asserts | every invariant checked (leak gauge, RC tripwires, ownership validators) | corruption is impossible to ship silently |

### Dimensions (each its own CI-gated scoreboard)
warm perf · cold start (#62) · peak RSS · binary size (<2MB) · compile time.

### References
CPython 3.12/3.14 = the FLOOR + parity oracle. PyPy = dynamic JIT reference (name the
missing molt mechanism where it wins). Codon = AOT static reference (mark
non-equivalent semantic models honestly, never as win/loss).

## 3. The four+1 scoreboards (kept green, CI-gated)
1. **CPython** — every benchmark × backend/profile; any <1.00× RED.
2. **PyPy** — pure-Python dynamic; names the missing mechanism (IC tiering,
   class-version guard, trace-like loop spec, generator fusion).
3. **Codon** — static/AOT subset on matched semantics.
4. **Backend** — native/LLVM/WASM/Luau each its own table (a native win never excuses a
   WASM regression).
5. **Profile** — dev / release-fast / release-output separate.

## 4. NOW → weeks: the correctness verticals that UNLOCK the optimizer
The optimizer cannot be turned up because of **trust**: the native RC "flip"
(value-tracking → drop-insertion) and aggressive escape/RC-elim/stack-alloc are gated
on memory-model trust. So correctness verticals come first *because they unlock perf*.

- **A. Finalizer Lifetime Closure** (in progress, doc 50): placement (#87/#63) →
  ordering (#58) → execution (#65 ✓) → field-release (#86 ✓) → child-finalization +
  consolidated matrix + pointer-field heap-free microbench.
- **B. Ownership lattice** (the #58 keystone, council-binding): `alias-root → ownership
  state → Python lifetime boundary → ordered release`. This substrate ALSO unlocks
  Free-eligibility, refcount-elim soundness, stack-alloc, and the RC flip.
- **C. Async/coroutine correctness** (#39 bug-3 multi-suspend propagation, #38 generator
  reraise, #40 call_soon drain, #24 StateDispatch terminator).
- **D. Structural correctness** (#87 dataclass, #64 weakref, #30 regex ReDoS, #48
  div-by-zero version gate, #53 caret coverage).

## 5. Months: the fact plane build-out + the perf frontier
**Fact plane (each piece retires a class):**
- op_kinds registry (✓) + operand-ownership table (#70 ✓, #74 remaining) + **ownership
  validators (#75 — Alive2 discipline)**.
- **CallFacts** (✓ Phase 1) → the call op carries target/arity/shape facts.
- **#71 Typed CallableTarget** (`DirectCodePtr | RuntimeMarker | Closure | BoundMethod |
  MethodDescriptor | Deopt`) — deletes the raw-marker-decode bug class (#59 root) AND
  unlocks direct-call / devirt / leaf-call optimization.
- **ShapeFacts v0** (after hot profile) — dataclass/class/dict layout facts → retires
  #68 (etl_orders 0.60×) and dict value-slot flow.
- **ExceptionRegion** (Phase 1 ✓ #80) → lightweight handled-exception path → retires #77
  (exception_heavy 0.68×).
- **Class identity / version guard / IC tiering** → devirt + deopt guards = the
  PyPy-parity lever.
- **Repr/TirType convergence** (typed IR Phase 2 — token-typed unbox keystone) → boxing
  precision is a proven fact, not a heuristic.
- **The native RC flip (✓ LANDED)** — `target_uses_tir_drop_insertion` is true for
  NativeCranelift; native value-tracking → drop-insertion is the live RC authority on every
  target, with the leak gauge + ownership validators as the safety net. The single biggest
  dynamic-perf unlock; the remaining work is deleting the dead legacy value-tracking lane.

**Warm-reds to retire (CPython-floor scoreboard):** #68 etl_orders 0.60×, csv_parse_wide
0.68×, #77 exception_heavy 0.68×; #66 LLVM lane (fib 0.30×, str_* 0.65-0.68×,
bytes/bytearray RUN_ERRORs); #67 PyPy/Codon fib gap (devirt + unboxed-int recursion);
#62 cold-start (43 cold-red — artifact footprint/page-in/codesign, not runtime init).

## 6. 1-3 years: optimizer maturity + full stack
**Optimizer classes:** devirtualization + monomorphization (E5; class-version-guarded
direct dispatch + specialized clones) · escape → region/stack allocation → RC
elimination (on the ownership lattice) · loop optimization re-enable (L4 #6:
TypeGuard-gen → loop-canon → gate; induction/range/overflow/lane-stability) ·
generator/async fusion (def-yield in-compiler, resumable-frame ownership, os.walk OOM) ·
inliner maturity (cross-module, observation/handler split) · whole-program reachability/
DCE → <2MB binary + per-attr liveness.

**Full stdlib + surface parity:** every builtin/dunder/protocol with edge+corner cases,
3.12→3.14; Rust-accelerated where hot, Python where semantic; target × profile parity
for every feature.

**GPU/ML (turn-blocking fidelity):** 26-primitive tinygrad-conformant stack; MLX/Metal +
CUDA/ROCm lanes; TurboQuant; **DFlash + DDTree exact algorithmic fidelity**
(target-conditioned draft, verifier/drafter separation, hidden-feature conditioning, KV
injection, trained drafter). `molt.gpu` / `molt.gpu.dflash` never diverge from the
tinygrad/DFlash source of truth.

**Ecosystem:** numpy-like typed-kernel subset (Codon territory); deploy lanes (Modal,
Cloudflare Workers/WASM, R2, edge); hermetic/tree-shaken WASM.

## 7. 3-10 years: the semantic nervous system
The fact plane becomes a **complete, generated, verified** semantic model: every
Python-visible property (lifetime, type, shape, identity, version, ownership, effect,
exception region) is a first-class cached fact, consumed uniformly by every pass on
every backend, with validators turning each into a checkable obligation. No pass-local
reasoning; no reconstruct-from-low-level-events. On that substrate:
- whole-program optimization (cross-module monomorphization, devirt, region memory,
  dead-attr elimination, profile-guided specialization);
- the references retired class by class — CPython floor permanently green; PyPy
  matched/beaten on dynamic; Codon matched/exceeded on the static subset; every
  target/profile/backend at parity;
- a shipped, world-class tinygrad-everywhere + DFlash ML stack;
- all four footprint dimensions world-class at once (binary <2MB, cold-start sub-ms,
  minimal RSS, fast compile);
- bootstrap maturity (single runtime import-boundary authority).

## 8. The compression ladder (illustrative classes — retire ~one/month)
trusted-unbox miscompile (✓ typed IR) · refcount underflow/over-release · finalizer
dispatch (✓ #65) → ordering (#58) → placement (#87/#63) · raw-marker decode (#71) ·
inline-field ownership (✓ #86) · boxing imprecision (Repr) · dynamic dispatch (devirt) ·
dict/attr shape (ShapeFacts) · exception-region churn (#77) · generator per-iter leak
(✓ #46) · loop induction/overflow/lane · cold-start footprint (#62) · binary-size
address-taken-intrinsics · WASM portable-IR opt loss · async multi-suspend propagation
(#39) · class-body/metaclass dynamic exec (✓ #50) · …

## 9. Operating doctrine (how the work is run)
Three non-overlapping lanes — **A** = P0 semantic safety (corruption/finalizer/
ownership), **B** = perf frontier (CPython-reds, regressions, PyPy/Codon gaps), **C** =
infra/scoreboards/decomposition. A blocks B only on memory unsafety; B blocks new
features while any benchmark < CPython; C is never decorative. Macro-tranches (close a
whole layer) with small atomic commits. Every batch reports the PERF/SPEED STATUS block.
The deliverable of perf work is a NEW IR FACT that makes a class of slow programs
unexpressible — not "faster code."
