<!-- Foundation design 46. Supervisor-authored from the structural-sweep tooling
(tools/structural_audit.py, tools/call_fact_coverage.py) + council directives
2026-06-08 ("structural sweep" → "alien compiler laboratory"). HEAD-anchored at
origin/main caa4d6e62. Tools are the living artifacts; this note is the map. -->

# The Molt Semantic Control Plane — structural sweep + the instruments it seeds

## 0. The question this answers

Not "what is the next bug?" but: **which missing abstraction explains this bug,
the next five bugs, and the next ten slow benchmarks?** The post-Lattner form:
*what semantic facts would make the next five years collapse into table rows,
verifier obligations, and predictable lowering — instead of heroic debugging?*

The MLIR invariant we extract (not copy): **semantic structure must survive
until every optimization that needs it has consumed it.** molt's recurring bug
and slowness classes are the same shape — a Python-visible fact (exception
lifetime, finalizer order, call target, value Repr, object shape) is *dissolved
into low-level SSA early*, then *reconstructed by fragile per-pass analysis*, and
*discarded before the backend or the next pass can use it*. The Control Plane is
the machine that keeps facts alive, measures their coverage, and ratchets the
hand-maintained drift to zero.

## 1. Two binding rules (enforced, not aspirational)

1. **Discovery-vs-authority.** Discovery tools may use heuristic parsing (regex,
   line scans). **Any tool output that GATES behavior must consume generated
   facts or typed AST.** `structural_audit.py` *ranks candidates* (heuristic, no
   semantic claim); the authoritative op-semantics gate stays
   `tools/gen_op_kinds.py --check` (consumes the generated registry). This rule
   is the lesson of the `_count_enum_variants` parser bug, made permanent.
2. **Every failure becomes an artifact.** Each bug/slowness must terminate as a
   generated fact, a region, a table row, a verifier obligation, a fuzz
   generator, a coverage metric, or a deleted fallback — never a one-off patch.

## 2. The instruments (built / building / named)

| instrument | tool | gate | status |
| --- | --- | --- | --- |
| structural drift / deletion board | `tools/structural_audit.py` | `--check` ratchet (debt only goes down) + `tests/test_structural_audit.py` | **BUILT** |
| call-site fact coverage census | `tools/call_fact_coverage.py` | `--check` ratchet (attached facts only go up) + evidence self-validation | **BUILT** |
| Repr coverage | `tools/representation_report.py` (existing) + `--corpus` join | typed_repr_report backend binary | EXISTS — wire into census |
| FactGraph (per-value provenance) | `molt factgraph` + `tools/fact_graph_dump.py` + `runtime/.../fact_graph.rs` | `cargo test -p molt-backend fact_graph` + `cargo test -p molt-tir fact_graph` + `tests/tools/test_fact_graph_dump.py` | BUILT compiler-emitted artifact route (§4.1); CLI/backend/tooling share one JSON contract |
| perf causality (slow→missing fact) | `tools/perf_causality.py` | joins #76 profiles + census | NAMED (§4.2) |
| Region calculus | `runtime/.../lifetime_regions.rs` + `tools/region_coverage.py` | validator obligations | NAMED (§4.3); ExceptionRegion (doc 45) is member 1 |
| Typed runtime interface | `runtime/.../runtime_contracts.rs` | `tools/runtime_contract_audit.py` | NAMED (§4.4); #71 CallableTarget is member 1 |
| Semantic fuzz forge | `tools/molt_fuzz.py` | differential + backend matrix | NAMED (§4.5) |
| Ecosystem compat graph | `docs/spec/conformance_manifest.toml` + scanners | manifest-backed claims | NAMED (§4.6) |
| Backend support matrix | `tools/backend_support_audit.py` | generated from op registry | NAMED (§4.7) |
| Pass-delta ledger | `tools/pass_delta_dashboard.py` | per-pass fact-loss attribution | NAMED (§4.8) |

The first two BUILT instruments are **complementary ratchets**: structural_audit
drives hand-maintained debt *down*; call_fact_coverage drives recorded-fact
coverage *up*. FactGraph is now the live provenance substrate those ratchets can
join against. The ratchets fail CI on regression — drift is a red build, not a
reviewer's vigilance.

## 3. The sweep answers the council's 10 questions (with tool data)

Measured against origin/main `caa4d6e62` by the two built tools.

1. **Top duplicate semantic authorities.** `purity` decided in 3 files;
   `side_effecting` in 3 files (`effects.rs` ×2, `escape_analysis.rs`,
   `deforestation.rs`) — yet op_kinds.toml's effect oracle already owns both.
   Plus 57 hand-classified opcode `match`es + 47 `matches!` opcode hand-sets.
   Worst offenders: `alias_analysis.rs` (21+21+16-opcode matches, a 45-opcode
   `matches!`), `type_refine.rs` (56), `verify.rs` (74). → **replace with the
   generated predicate; gate `gen_op_kinds.py --check`.**
2. **Top backend-local semantic guesses.** Near-exhaustive silent-default
   classifications in backend lowering: `lower_to_wasm.rs:549` and
   `lower_to_simple.rs:1507` (106-opcode `match` → silent default), plus
   `llvm_backend/lowering.rs` hand-lists. A 106-of-111-opcode hand-list with a
   `_ =>` default should either drop the wildcard (rustc-gate it) or read a
   generated fact.
3. **Top hot runtime helpers that should vanish** (from #76/#77 cycle
   attribution, not the static tools): `molt_inc_ref`/`molt_dec_ref` (~22% of
   `exception_heavy`), exception-stack push/pop (~12%), GIL (~11%);
   `etl_orders`: str split/decode + dataclass construction + attr/field loads
   (#68). Each names a **missing fact** (ExceptionRegion exit ownership; dataclass
   shape; split-field view; field offset / class-version guard).
4. **% calls direct / leaf / no-throw / no-alloc / inlinable.** *This is the
   finding:* call-site representation coverage is **2/7 (28.6%)**. Only
   `direct_target` (s_value attr) and `typed_return` (result Repr) are recorded
   on the call op. `leaf`, `no_throw`, `no_alloc`, `inlinable`, `noescape` are
   **computed-and-discarded** inside passes — so no backend can consume them and
   no tool can measure their site-level percentage. The missing IR primitive is a
   **`CallFacts` record on the call op** (§4.1). (Direct + typed-return
   percentages are measurable now via `call_fact_coverage.py --corpus` over a
   quiescent build's typed_repr_report dumps.)
5. **% hot values raw vs boxed.** Measurable via `representation_report.py` /
   `typed_repr_report` per benchmark; to be joined into the census `--corpus`
   path (pending a quiescent build — not run here to avoid non-authoritative
   timing under the active #73 build).
6. **% attr/dict/field ops with shape facts.** **0% — there is no shape system.**
   This is the largest single missing abstraction for the ETL/dataclass/ORM
   benchmark cluster (doc 47, `ShapeRegion` + `ClassShape`/`FieldOffset`/
   `ClassVersionGuard`).
7. **Backend divergences that are really IR fact losses.** #66 (LLVM `fib` 0.30×,
   `str_*` 0.65–0.68×) = call-target devirt + unboxed-int recursion not present
   in *portable* IR, so native's fast lane has nothing to lower. Quantified by the
   (named) backend support matrix §4.7 — a native win that a WASM/LLVM regression
   shadows is a portable-IR fact gap, never a backend exception.
8. **Compatibility claims lacking a manifest.** **All of them** — there is no
   `conformance_manifest.toml` yet, so every ecosystem-compat claim is prose
   (§4.6). First skeleton turns dynamism features into manifest nodes.
9. **The tool that replaces the next 2 hours of manual debugging.** `fact_graph_dump`
   ("why is this value boxed / this call generic / this object heap-allocated?")
   and `perf_causality` ("which missing fact forces which hot helper for which
   %?"). Both named §4.
10. **The abstraction that makes the next five bugs impossible.** The `CallFacts`
    record (kills the generic-call + IC-marker classes — §4.1/§4.4), the Region
    calculus (kills the exception/finalizer/with lifetime classes — §4.3), and
    the Typed Runtime Interface (kills the raw-bits-across-a-semantic-boundary
    class — §4.4).

## 4. The named primitives (extract the invariant, build molt-native)

### 4.1 FactGraph + CallFacts
A compiler fact carries: `subject` (value/op/fn/class/region/backend), `kind`
(Repr/Ownership/Shape/CallTarget/Effect/ExceptionRegion/FinalizerSensitive/Leaf/
NoThrow/NoAlloc), `producer pass`, `confidence` (proven/guarded/assumed/profiled/
imported), `guards`, `consumers`, `invalidators`, `backend lowering status`, `test
coverage`, `perf relevance`. Start narrow: **attach a `CallFacts` record to the
call op** (direct target, leaf, no-throw, no-alloc, inline-eligibility + why-not,
arg-noescape mask). That single primitive moves call_fact_coverage from 28.6% to
measurable, lets backends specialize calls, and is the substrate for #67/#68/#71.
`runtime/molt-tir/src/tir/fact_graph.rs` now emits deterministic JSON from live
`TirFunction` / `CallFactsTable` state, `molt factgraph <entry> <function>
--output <path>` routes the normal frontend/backend pipeline into that
compiler-emitted artifact, and `tools/fact_graph_dump.py` validates and renders
the graph (including `--why-boxed`). The CLI, backend flag, and dump tool share
the same JSON contract; none is a second fact authority. **Full implementation spec:
`docs/design/foundation/47_call_facts_leaf_coverage.md`** (the `CallFacts` struct,
`FactValue` confidence lattice, per-field producers, the "pop many reds into
place" benchmark map, and the 4-phase plan).

### 4.2 Optimization Causality Engine
`tools/perf_causality.py` joins #76 hot profiles + the census + the
pass-delta ledger → `benchmark → top hot helpers → missing facts → likely fix →
expected metric movement`. Turns "exception_heavy is slow" into "missing
ExceptionRegion-exit ownership forces inc/dec_ref churn = 22% of samples; fix =
ExceptionRegion Phase 1; expect leak gone + RC samples down."

### 4.3 Region / Lifetime calculus
One `LifetimeRegion` trait (`entry / normal_exit / exceptional_exit / transfer /
restore / discard / validator_obligations`) with instances ExceptionRegion (doc
45, **member 1**), FinalizerRegion (#58), WithRegion, GeneratorRegion,
AsyncCancelRegion, ImportRegion, OwnershipBoundary. This is the structural answer
to "are we lowering Python too early?" — Python-visible lifetime constructs get
region semantics instead of being dissolved into SSA and rediscovered.

### 4.4 Typed Runtime Interface
Typed contracts at the compiler/runtime boundary so raw bits never cross with
"convention": `CallableTarget = DirectCodePtr | RuntimeMarker | Closure |
BoundMethod | MethodDescriptor | Deopt` (#71 — the structural successor to the
#59 IC-marker SIGSEGV); `ReleaseOp = DecRefPython | FreeInternal |
FreeUniqueNoPy(proof)`; `ExceptionHandle = Pending | Matched | BoundLocal |
Reraising | SavedForFinally`; `BorrowHandle = InteriorBorrow{source_root, handle}`.
Generated from the registry; consumed by every backend (no backend invents
runtime semantics). `tools/runtime_contract_audit.py` gates it.

### 4.5 Semantic Fuzz Forge
`tools/molt_fuzz.py` generates programs from **our own bug history as a grammar**
(exception/finally/reraise, finalizer/resurrection/weakref, ownership/alias/copy,
iter-next conditional-validity, callargs/callable-target, dataclass/shape
mutation, repr/bigint boundary, import/module cache). Each case: seed + feature
labels + CPython output + backend matrix + minimizer command → `tests/generated_bugclass/`.

### 4.6 Ecosystem Compatibility Graph
`docs/spec/conformance_manifest.toml`: `library → Python features → C-ABI
features → runtime semantics → compiler facts → backend support → perf status`,
each feature node `supported | guarded | typed-shim | C-bridge | incompatible-by-
design | not-yet`. Makes "full ecosystem, dynamism-bounded" an engineering graph,
not a slogan. (HPy studied as the handle-based C-extension boundary pattern.)

### 4.7 Backend support matrix
`tools/backend_support_audit.py` → `docs/backend/OP_SUPPORT_MATRIX.md`, generated
from the op registry: rows = OpCode/Terminator/OwnershipEvent/LifetimeRegion/
CallableTarget/RuntimeHelper; columns = native/LLVM/WASM/Luau × lowered? / RC-safe?
/ exception-safe? / repr-safe? / known-degradation? / test-coverage?.

**RECON FINDING (2026-06-08) — why this MUST be registry-generated, not scraped.**
A direct scrape of backend support was attempted and **refused** (verified
refusal): the four backends dispatch through **four incompatible paradigms**, so
there is *no shared lowering contract* to scrape uniformly —
- **LLVM** (`llvm_backend/lowering.rs::lower_op`) — clean `match op.opcode` over
  the `OpCode` enum.
- **native** (`native_backend/function_compiler.rs`) — `match op.kind.as_str()`
  over SimpleIR **kind strings**, a *different vocabulary* (mapped to `OpCode`
  only via the op_kinds `[[kind]]` mapper).
- **WASM** (`wasm.rs`) — nested opcode matches, no single dispatch site.
- **Luau** (`luau.rs`) — a **transform pipeline** (`lower_try_to_pcall`,
  `lower_iter_to_for`, …) with ~49 fail-loud arms, no opcode match at all.

That heterogeneity *is* the Q7/Q10 answer: backend-parity bugs recur because each
backend reinvents dispatch instead of implementing one portable lowering
contract. A heuristic 4-way scrape would be exactly the brittle, drift-prone
tool the discovery-vs-authority rule forbids. **Authoritative design:** add
per-backend support as GENERATED columns to `op_kinds.toml` (like `may_throw`);
`gen_op_kinds.py` renders the matrix; the tool then checks each backend's actual
dispatch *against* the registry (drift), never *infers* support from it. This is
a build-requiring registry arc (touches the generator + a careful first
population) — sequenced for build capacity (after #73) and coordinated with the
lowering work, NOT rushed as a scrape.

### 4.8 Pass-delta ledger + profile-tiered compilation
`tools/pass_delta_dashboard.py`: per pass, the Δ in op-count / boxed values /
generic calls / RC events / alloc sites / backend-unsupported ops / compile time —
to attribute fact loss to a pass. Profile tiers (dev / release-fast /
release-output) may deserve *distinct lowering strategies* (copy-and-patch-style
stencil baseline for dev; thin-LTO + hot facts for release-fast; fat-LTO + post-
link layout for release-output) — `docs/design/foundation/profile_tiering...md`.

## 5. Sequencing (no new abstraction blocks the P0 lane)

- The op-semantics ladder (#70→#74) keeps absorbing the §3.1 hand-classifications
  into the generated registry — each migration *deletes a deletion-candidate* and
  drops the structural_audit ratchet. #73 (interior-borrow `matches!` in
  alias_analysis — the 45-opcode set this board ranks #1) is in flight.
- **ExceptionRegion Phase 1** (doc 45) is Region-calculus member 1 and the first
  consumer of the #58 ownership lattice the ladder seeds. It is the single-lane
  priority.
- `CallFacts` (§4.1) is the highest-leverage new primitive: it converts the
  28.6% call-site coverage finding into a measurable, lowerable, specializable
  substrate, and is the shared root of #67/#68/#71.
- The remaining instruments land incrementally, each as its own tranche, each
  with a `--check` gate, each deleting a fallback when it lands.

## 6. Status

Built + gated + pushed: `structural_audit.py` (+ baseline + board + pytest),
`call_fact_coverage.py` (+ baseline + pytest), this note, CI wiring. The board is
`docs/design/foundation/STRUCTURAL_AUDIT_BOARD.md` (regenerated by `--write-board`).
Related: doc 45 (ExceptionRegion), #58 (ownership lattice), #71 (CallableTarget),
#70–#74 (op-semantics ladder), #76/#77 (cycle attribution feeding §4.2).
