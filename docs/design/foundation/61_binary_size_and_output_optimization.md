<!--
  Foundation blueprint — Arc 61: BINARY SIZE + OUTPUT OPTIMIZATION
  ("size is a tracked Performance-Constitution dimension, not just speed" — CLAUDE.md
   Performance Constitution: binary size, peak RSS, compile time, cold-vs-warm are
   tracked dimensions; cold-start is an artifact-footprint/page-in/codesign problem.)
  Author: portfolio-architect
  Date: 2026-06-24
  Status: DESIGN ONLY / EXECUTABLE PLAN (no code in this change; the lead integrates).
  Assigned number 61 belongs to the active 60-67 portfolio arc cluster. This DEEPENS
  doc 64_perf_scoreboards_and_harness (the measurement plane — Size board projection),
  doc 60 (tree-shaking / whole-program DCE), and doc 21e (runtime tiers).
-->

# 61 — The Size Plane: Artifact Footprint as a First-Class, Gated, Per-Backend/Profile Dimension

> **One-line thesis.** molt already ships *seven* hand-tuned Cargo profiles and a
> sophisticated per-crate opt-level policy, and it already has three size-measurement
> scripts — but **size is not gated, monomorphization bloat is completely uncontrolled
> (no `share-generics`, no polymorphization, the #1 Rust binary-size killer), the WASM
> split/component/streaming code is estimate-only stubs disconnected from product
> codegen, and the size budgets are scattered magic constants (35MB native / 20MB / 16MB
> WASM) rather than a per-`(backend × profile × tier)` scoreboard with history.** This arc
> retires the class **"silent size drift"** the way doc 64 retires "silent perf drift":
> by making artifact footprint a *projection of the same measurement plane*, gated per
> backend/profile/tier, attributed to the IR/codegen fact that caused the bloat, with the
> `release-output`/`release-size`/`wasm-release` profiles as the size/opt north star.

---

## 0. The end-state outcome (stated crisply)

**In five years, no molt commit can grow an artifact without the system saying so, and
the size of every shipped artifact is a derived fact attributed to a representation
cause — not a number a human eyeballs in a one-off `nm` dump.** Concretely:

1. **A SIXTH scoreboard projection exists — the Size board** — a sibling of doc 64's five
   (CPython/PyPy/Codon/Backend/Profile). It is keyed `artifact × backend × profile ×
   stdlib_tier`, with the **five Performance-Constitution dimensions per cell**: stripped
   bytes, **compressed bytes** (gzip + brotli — the real edge-deploy unit), section/segment
   breakdown, symbol-category census, and the *cause attribution* (which monomorphized
   generic family / which intrinsic-address-taken / which stdlib domain drove the bytes).
   It gates on **regression vs history** (CLAUDE.md triage #5) and on **absolute budgets**
   per `(backend, profile, tier)`. A `release-fast` daemon-size change *cannot* hide a
   `release-output` shipped-runtime regression, and a `stdlib_full` change *cannot* hide a
   `stdlib_micro` edge-tier regression — because they are **distinct cells with distinct
   budgets**, not numbers an eye skims.
2. **The `<2MB` hello-world contract (doc 51 §2/§6) is a gated fact, per tier.** Today it
   is prose in the roadmap. After this arc, `bench/scoreboard/size/` carries the budget,
   the gate enforces it on the `release-output`/`wasm-release` + `stdlib_micro` cell, and
   a regression past it fails to merge.
3. **Monomorphization bloat is a controlled, measured representation fact, not an
   accident.** `-Z share-generics` (where the toolchain allows it on the size profiles),
   a *generic-instantiation census* (`cargo llvm-lines` / `cargo bloat` *wired into the
   Size board as a gated input, not a doc suggestion*), and — for the runtime's own hot
   generic surfaces — **structural polymorphization** (replace monomorphized-per-T glue
   with `dyn`/erased-repr dispatch where the *type fact proves it is cold*, keeping opt-3
   monomorphic bodies only on the hot lanes doc 65_perf_compression_ladder Rung 4 names).
   The class retired: **"the generic-instantiation explosion nobody attributed."**
4. **WASM size has a real split/tree-shake pipeline, not an 8-bytes-per-op estimate.**
   `wasm_split.rs`/`wasm_component.rs`/`wasm_streaming.rs` either become product-wired
   (driven by the real emitted module + the `--gc-sections`/`wasm-opt -Oz --converge`
   contract of doc 0931) or are explicitly demoted to "research stub, not on the size
   path" so they stop masquerading as a shipped capability. The compressed-WASM cell is
   gated against the Cloudflare 3MB ceiling per tier.
5. **The dylib/static tradeoff is a measured, per-deployment decision.** The runtime is
   `staticlib`+`rlib`+`cdylib` today; the arc adds the *measured* "N independent molt
   binaries on one host" crossover where a shared dylib wins total footprint, recorded as
   a Size-board dimension (per-binary bytes vs amortized shared-runtime bytes), so the
   choice is evidence, not folklore.

**The class this arc retires:** **"size is invisible until a customer hits the 3MB
Workers ceiling / a 35MB native binary / a cold-start page-in cliff."** After this arc
that family is unexpressible on main: the Size board refuses the regression, names the
cause, and the budget gate blocks the merge.

---

## 1. What already exists (cite-and-compose; do NOT duplicate)

This arc is **~70% composition** of existing, high-quality substrate. Verified against
the tree at HEAD (2026-06-24):

| Asset | Path | What it already does | Gap this arc fills |
|---|---|---|---|
| **7 tuned Cargo profiles** | `Cargo.toml` lines 378-437 | `release-output` (opt-z, fat-LTO, cgu=1, panic=abort, strip), `release-size`, `wasm-release`, `wasm-release-fallback`; the *measured* hot-crate opt-level policy (lines 462-564: speed profiles opt-3 hot crates, size profiles re-assert opt-"s", with the 25.5% wasm measurement) | **No `-Z share-generics`/polymorphize**; profiles tuned but their *output is never gated* — no scoreboard consumes them |
| Native size analyser | `tools/binary_size_analysis.py` (831 ln) | nm symbol census, Mach-O segment/section breakdown, 5 symbol categories, `--compare` deltas, `--budget`, JSON | Budgets are **per-invocation magic constants** (35MB native / 20MB wasm, lines 107-108); **no history, no gate wiring, no per-tier budget, no compressed size, no cause attribution** |
| WASM size auditor | `tools/wasm_size_audit.py` | per-section LEB128 parse, code/data split, `--budget 16MB`/`--budget-code 10MB` gate (V8 OOM headroom) | Standalone; **not a board projection**; budgets are V8-OOM-driven not the 3MB Workers contract; no compressed size; no history |
| Output+startup+size matrix | `tools/output_startup_size_audit.py` (1239 ln) | builds hello-world across `native/wasm/luau/mlir × dev/release × auto/llvm × stdlib-{micro,full}`; records artifact bytes, cold-first-sighting + page-cache-cold + same-path startup, CPython + C baselines, `--max-artifact-mb`/`--max-fresh-start-ms` budget checks | The right *matrix shape* but **budgets are opt-in CLI flags (default None → never gates)**; writes a timestamped JSON, **no history index, no regression gate, not a doc-64 projection, no cause attribution, stripped-only (no gzip/brotli)** |
| WASM opt pipeline | `tools/wasm_optimize.py`, `tools/wasm_link.py`, `tools/wasm_pipeline.py` | `wasm-opt` invocation with the *load-bearing* feature flag set (`--disable-gc`, `--disable-custom-descriptors`, the rec-group flatten); export-contract preservation | `wasm-opt -Oz --converge` lane named as "high-value work" in doc 0931 but **not measured/gated**; the optimize step is not size-board-attributed |
| Runtime tiers | `runtime/molt-runtime/Cargo.toml` lines 16-120 | strict-superset feature chain `stdlib_micro ⊂ edge ⊂ standard ⊂ server ⊂ full`; `default=["stdlib_full"]`; per-domain `dep:`-backed features that drop crates when off | The tier *mechanism* exists; **no per-tier size budget, no measured per-tier footprint, no "which tier should this deployment use" evidence** (21e dedups satellites; THIS arc measures their size effect) |
| Symbol feature-gate map | `src/molt/_runtime_feature_gates.py` | the single source of truth for symbol-prefix→`stdlib_*` feature; `LINK_AFFECTING_FEATURES`; frontend compile-time refusal so excluded domains are dropped from the archive | This is the **tree-shaking substrate arc 60 builds on**; this arc *measures* what each gate saves and feeds it to the Size board |
| Linker contract | `docs/spec/areas/compiler/0931_LINKER_OPTIMIZATION_CONTRACT.md` | `--gc-sections`/`--export-if-defined`/no-ICF-while-fn-addrs-are-identities rules; "add size dashboards… raw/gzip/function-count/data-segment/export-count" as high-value work | Names the dashboard as TODO; **this arc IS that dashboard, as a board projection** |
| Size/cold-start spec | `docs/spec/areas/perf/0604_BINARY_SIZE_AND_COLD_START.md` | the metric definitions (stripped+unstripped, raw+gzip+brotli, `llvm-size`/`cargo bloat`/`cargo llvm-lines`/`twiggy`), 10% regression rule | **All of it is prose** — no tool implements `cargo bloat`/`llvm-lines`/`twiggy`/`brotli`; this arc operationalizes the spec |
| WASM split/component/streaming | `runtime/molt-tir/src/tir/wasm_split.rs`, `wasm_component.rs`, `wasm_streaming.rs` | **estimate-only stubs** (`ops*8` byte heuristic; WIT generation; hot/cold manifest) — name-prefix categorization, NOT wired to real emitted modules or product output | **Decision point (§3.5):** product-wire to the real module + linker, OR demote to research-stub so they stop implying a shipped split capability |
| The measurement plane | `docs/design/foundation/64_perf_scoreboards_and_harness.md` + `tools/perf_scoreboard.py` | `PerfCell` already carries `binary_size_kib`, `compile_time_s`, `molt_peak_rss_mib`, cold/warm split; `BoardProjection` abstraction; `bench/scoreboard/{...}.json` budget files incl. `cold_start_budget.json` | **The Size board is the missing sixth projection.** This arc adds it over the *same* cell stream — does not build a parallel loop |

> **Refusal recorded (deletes a bad plan).** The naive plan — "write a new size CI script
> with its own build loop and its own budget file" — is **REJECTED**. It would create a
> *second* build path, a *second* budget source of truth, and a *second* history index
> alongside doc 64's plane → the exact compound-interest-of-bugs trap CLAUDE.md forbids.
> The structurally correct design is: **the Size board is a `BoardProjection` over the
> SAME `list[PerfCell]` doc 64 already produces** (the cells already carry size; the build
> already happens once). Size is a *view*, not a new loop. This is the load-bearing
> architectural decision and the reason this arc is `DEEPENS 53`, not a sibling of it.

---

## 2. Time-traveler derivation (end-state → required structural facts)

Work backward from §0 to the mechanisms that make it inevitable.

- **END:** "no commit grows an artifact without the system saying so."
  → **requires** the Size board to be a *gated projection* on every PR + main, sharing
     doc 64's tiered authority (per-PR smoke gate + nightly full sweep).
  → **requires** size to be a **regression-vs-history** axis (CLAUDE.md triage #5), so a
     +3% creep gates even when still under absolute budget. **FACT NEEDED:** `SizeCell`
     fields on the existing `PerfCell` (most already present) + a `bench/scoreboard/size/`
     history keyed by the same `board_identity` doc 64 §3.4 defines.

- **END:** "size is per `(backend × profile × tier)`; a micro regression can't hide in a
  full win."
  → **requires** the build matrix to include the **stdlib tier** as a coordinate (today
     `output_startup_size_audit.py` has `stdlib-{micro,full}` but the perf plane's cells
     do not carry tier). **FACT NEEDED:** a `stdlib_tier` field on the cell + a
     `SizeBudget(backend, profile, tier) → {stripped_max, gzip_max, brotli_max}` table
     (the *typed* successor to the scattered 35MB/20MB/16MB constants).

- **END:** "monomorphization bloat is controlled, not accidental — the #1 Rust killer."
  → **requires** three independent mechanisms, because there is no single switch:
     (a) **`-Z share-generics`** on the size profiles where the toolchain supports it
         (stable on the standard profiles via `[profile].build-override`? — no; it is a
         `-Z` flag → requires nightly OR the `RUSTFLAGS=-Zshare-generics=y` opt-in path;
         the arc treats it as a *measured, gated-when-available* lever, never silently
         assumed — see §3.3 + Risk 3).
     (b) a **generic-instantiation census** that names *which* `T`-families explode:
         `cargo llvm-lines -p molt-runtime` + `cargo bloat --crates`/`--filter` wired as a
         **Size-board attribution input** (the spec 0604 §4.1 tools, finally executed).
     (c) **structural polymorphization of the runtime's own cold generic surfaces** — the
         Rustacean fix: where a generic helper is monomorphized per-`T` but the *type
         fact* (doc 65_perf_compression_ladder Rung 4 `Repr`/lane facts; doc 59 fact
         plane) proves the call site is **cold**, replace it with one erased-`Repr` body
         (`&dyn`/`ValueRef`) so N instantiations collapse to 1 — *keeping* opt-3
         monomorphic bodies only on the hot lanes. **FACT NEEDED:** a `hot/cold` partition
         of the runtime's generic surface (consumes the doc 65_perf_compression_ladder
         hot-symbol set + the cycle profile #76), so "monomorphize" vs "erase" is a
         *derived* decision, not a guess.

- **END:** "WASM size has a real split/tree-shake pipeline."
  → **requires** the stubs to either consume the *real* emitted `TirModule` byte sizes
     (post-`wasm-opt`, post-`--gc-sections`) or be demoted. **FACT NEEDED:** a
     decision (§3.5) + if product-wired, a `WasmSplitPlan` driven by real reachability
     (the existing `reachability.rs` pass), not name prefixes.

- **END:** "the dylib/static tradeoff is measured."
  → **requires** a `cdylib`-shared-runtime size lane alongside the `staticlib` lane, and
     the crossover-N computation. **FACT NEEDED:** a `linkage` cell coordinate
     (`static` | `shared`) + the per-binary-vs-amortized arithmetic.

- **END:** "cold-start is treated as artifact-footprint, not runtime-init" (CLAUDE.md:
  runtime init is 0.127ms; cold start is page-in/codesign).
  → already correct in `output_startup_size_audit.py` (`cold_first_sighting` =
     true-cold cdhash, `page_cache_cold` = copy-based). **FACT NEEDED:** fold its
     cold/page-cache fields into the Size board so size and cold-start co-report (the
     footprint *is* the cold-start driver — they are one dimension family, doc 64 Risk 8).

The dependency spine:

```
Phase 0  Pin the SizeBudget contract + fold size fields into the doc-64 cell schema
   │       (perf_schema.py: SizeCell view + SizeBudget table; one source of truth for budgets)
   │
Phase 1  Size board PROJECTION over the existing PerfCell stream (perf_board.py)
   │       + the tier/linkage coordinates on the measurement core (perf_measure.py)
   │       + compressed-size (gzip/brotli) capture. DEPENDS on doc 64 Phase 1 (the core).
   │
   ├── Phase 2  Size HISTORY + regression gate (reuse doc 64 Phase 4 history machinery)
   │             + per-(backend,profile,tier) absolute budgets. DEPENDS on Phase 1.
   │
   ├── Phase 3  Monomorphization control:
   │             3a share-generics measured lever (size profiles, gated-when-available)
   │             3b generic-instantiation census input (cargo llvm-lines/bloat) → Size board
   │             3c structural polymorphization of cold runtime generic surfaces
   │             DEPENDS on Phase 1 (to measure the delta) + doc 65_perf_compression_ladder
   │             (hot/cold partition) + doc 64 Phase 5 (cycle profile for cold-proof).
   │
   ├── Phase 4  WASM split/tree-shake decision + (if product-wire) real-reachability plan;
   │             wasm-opt -Oz --converge measured lane; compressed-WASM 3MB-tier gate.
   │             DEPENDS on Phase 1; composes with arc 60 tree-shaking + doc 0931.
   │
   ├── Phase 5  Tier-size evidence (per-tier footprint board) + dylib/static crossover lane.
   │             DEPENDS on Phase 1; composes with doc 21e + arc 60.
   │
   └── Phase 6  Size cause-attribution (which fact drove the bytes) — the Size analogue of
                 doc 64 Phase 5 perf_causality. DEPENDS on 3b + doc 64 Phase 5.

Phase 7  CI wiring: Size board into the doc-64 perf tier (one gate, two projections),
         per-PR smoke size gate + nightly full size sweep. DEPENDS on Phases 1-2.
```

Phases 2–6 parallelize once Phase 1 lands (non-overlapping files); Phase 1 + doc 64
Phase 1 are the serialization points. Only Phase 3c + Phase 4 (if product-wired) touch
Rust; the rest are host tooling.

---

## 3. The structural facts / mechanisms to build (each tied to the class it retires)

### 3.1 FACT: `SizeBudget` typed table — retires "scattered magic-constant budgets"

The four budget sources today (`binary_size_analysis.py` 35MB/20MB; `wasm_size_audit.py`
16MB/10MB/4MB; `output_startup_size_audit.py` opt-in `--max-artifact-mb`; the prose
`<2MB`/`<3MB` in doc 51) are **four sources of truth that already disagree**. Collapse to
ONE typed table in `tools/perf_schema.py` (doc 64's schema home), keyed by the matrix:

```
SizeBudget(backend, profile, stdlib_tier, linkage) -> {
    stripped_max_bytes, gzip_max_bytes, brotli_max_bytes,
    section_code_max_bytes|None,         # WASM code-section (V8 OOM headroom)
    note: str,                            # provenance of the number
}
# Seed values (hello-world, the canonical footprint probe), to be re-baselined
# from the FIRST authoritative nightly sweep, never hand-typed as aspiration:
#   (wasm, wasm-release, micro,    static) : gzip <= 3MB     [Cloudflare Workers ceiling]
#   (native, release-output, micro, static): stripped <= 2MB [doc 51 §2/§6 contract]
#   (native, release-output, full,  static): stripped <= <baseline+10%>  [regression rule]
```

**Validation contract (fail-closed):** a Size cell that claims GREEN without *both*
stripped and compressed (gzip) bytes is malformed (compressed is the real edge unit —
doc 0604 §1.1 mandates gzip/brotli; stripped-only is the current blind spot). **Class
retired:** "a size pass/profile that improved stripped but regressed compressed" — and
"four budget tables that disagree."

### 3.2 FACT: the Size board projection — retires "asymmetric size coverage"

A `BoardProjection` (doc 64 §3.2) over the SAME `list[PerfCell]`:

```
BoardProjection(
  name="size", kind="size_footprint_scoreboard",
  cell_filter = build_ok,                      # every successfully-built cell has a size
  group_by = ("backend", "profile", "stdlib_tier", "linkage"),
  gate_predicate = lambda c: FAIL iff
        c.stripped_bytes  > SizeBudget(...).stripped_max
     or c.gzip_bytes      > SizeBudget(...).gzip_max
     or regressed_vs_history(c, threshold=10%)   # CLAUDE.md 0604 §3 + triage #5
     or (c.backend=="wasm" and c.wasm_code_bytes > SizeBudget(...).section_code_max),
  required_lanes = (),                          # size needs only a successful build, not a run
)
```

Because it groups by `(backend, profile, tier, linkage)`, the doc-51 invariant "a native
win never excuses a WASM regression; release-output never hides a release-fast regression"
extends to size with **distinct exit codes per cell** — plus the two coordinates size
adds: **a `stdlib_full` win cannot hide a `stdlib_micro` edge regression**, and **a
`static` win cannot hide a `shared`-dylib regression**. **Class retired:** "asymmetric
size coverage" across the full footprint matrix.

> **Why size needs only build, not run.** Unlike the warm/cold perf boards, the Size board
> gates on a *built artifact's bytes* — no quiescence needed, no contamination, fully
> deterministic. This makes it the **cheapest, most trustable per-PR gate in the entire
> plane**: a size regression is a hard, reproducible fact on any runner (Risk-free under
> doc 64 §3.3's contamination concern). It can hard-gate per-PR where warm-perf can only
> regression-gate.

### 3.3 FACT: monomorphization control — retires "the generic-instantiation explosion"

Three mechanisms, because monomorphization bloat has no single switch (this is the arc's
deepest lever and the one with *zero* current coverage):

**3.3a `-Z share-generics` measured lever (size profiles).** Rust monomorphizes every
generic instantiation per crate; `share-generics` lets downstream crates reuse upstream
instantiations instead of re-emitting them. It is a `-Z` flag (nightly / `RUSTFLAGS`),
**so the arc treats it as gated-when-available, never silently assumed** (Risk 3): a
`tools/size_levers.py` probe detects whether the active toolchain honors
`-Zshare-generics=y`, builds the size-profile runtime both ways, and the Size board
records the **DIMENSIONAL delta** (CLAUDE.md "DIMENSIONAL_WIN reported honestly"). If it
wins and the toolchain supports it, it is wired into the size-profile `RUSTFLAGS`; if not,
the *measured-but-unavailable* fact is recorded so the lever lights up when the toolchain
does. This is the honest treatment: no fake assumption, a real measurement.

**3.3b generic-instantiation census as a Size-board input.** Spec 0604 §4.1 names
`cargo bloat -p molt-runtime --release` and `cargo llvm-lines -p molt-runtime` — **neither
is implemented anywhere** (verified: zero references outside docs). Build
`tools/mono_census.py`: run `cargo llvm-lines` (line-level monomorphization attribution)
and `cargo bloat --crates`/`--filter` against the size-profile runtime, parse the
top generic families by emitted-line/byte count, and emit them as the
`PerfCell.size_attribution` field. This *names the explosion* — e.g. "`HashMap<K,V>` ×27
key types = X KB", "`Vec<T>::extend` ×N = Y KB" — so 3c targets the real driver, not a
guess. **Class retired:** "we know the binary is big but not which generic family." The
census is the size analogue of doc 64's representation census (`call_fact_coverage.py`).

**3.3c structural polymorphization of cold generic surfaces (the Rustacean fix).** The
representation-correct lever, not a flag: where a runtime generic helper is monomorphized
per-`T` but the *type fact proves the call site is cold* (it is NOT on the
doc 65_perf_compression_ladder hot-lane set and NOT in the cycle-profile #76 hot symbols),
collapse the N monomorphizations to **one erased-`Repr`/`&dyn` body**. The hot lanes keep
their opt-3 monomorphic bodies (the perf contract is untouched — size is traded *only*
where it costs no measured warm cycles). The decision is **derived**: `mono_census.py`
(3b) names the big family → cross-reference the hot/cold partition → if cold, erase; if
hot, leave monomorphic and instead attack via Repr precision (the perf ladder's job). This
composes the two lenses: Pythonista semantics unchanged (the erased body is observably
identical), Rustacean representation fixed (one body, not N), and the perf floor protected
by the cold-proof. **Class retired:** "monomorphization bloat on code that is never hot" —
made *structurally unexpressible* once the cold-erasure pattern is applied to the census's
top cold families.

> **The compression-ladder rung this adds.** doc 65_perf_compression_ladder Rung 8
> ("artifact-footprint facts") names binary size as a rung but does not give the
> *monomorphization* mechanism. 3c is that mechanism: the missing IR/build fact is
> **"a generic instantiation's hot/cold status decides monomorphize-vs-erase,"** which
> makes the class "cold code paying monomorphization tax" unexpressible. This is the
> doc-51 method ("retire one CLASS, add the fact that makes it unexpressible") applied to
> size, and it is the load-bearing *deepening* this arc contributes beyond just measuring.

### 3.4 FACT: compressed + sectioned size, not stripped-only — retires "the gzip blind spot"

Every Size cell carries stripped bytes **and** gzip bytes **and** brotli bytes **and** the
section/segment breakdown (Mach-O segments via the existing `binary_size_analysis.py`
parser; WASM sections via the existing `wasm_size_audit.py` parser — *reused*, not
rewritten). The edge-deploy contract (Cloudflare 3MB) is a **compressed** ceiling; gating
on stripped-only (the current default) can pass a cell that fails in production. **Class
retired:** "an optimization that shrank uncompressed bytes but grew the compressed
artifact" (real — opt choices that add entropy can do this).

### 3.5 DECISION: WASM split/component/streaming — product-wire or demote (retires "stub masquerading as capability")

`wasm_split.rs`/`wasm_component.rs`/`wasm_streaming.rs` are estimate-only
(`ops.len() * 8` byte heuristic, name-prefix categorization, no connection to the real
emitted module or the linker). Leaving them as-is is the "sharp edge left for later" the
zero-workaround policy forbids — they *look* like a shipped split capability. **The arc
forces the decision (Phase 4):**
- **Product-wire (preferred IF the size win is real):** drive `WasmSplitPlan` from the
  real `reachability.rs` reachability set and the *actual* post-`wasm-opt` section bytes,
  emit a genuine multi-module split (core + on-demand stdlib) validated against the doc
  0931 export contract, and gate the core-module compressed size per tier. This is the
  structurally correct realization of the streaming/split idea.
- **Demote to research-stub (IF measurement shows split does not beat monolithic-`-Oz` +
  `--gc-sections` for the target deployments):** move them under a clearly-labeled
  `research/` path with a doc note "not on the size path; superseded by tree-shaking
  (arc 60) + wasm-opt -Oz", so they stop implying a shipped capability.
The decision is **evidence-driven** (measure split-core vs monolithic-shaken compressed
size on the real edge probe), never assumed. **Class retired:** "estimate-only code that
masquerades as a product size mechanism."

### 3.6 FACT: dylib/static crossover — retires "the linkage choice is folklore"

Add a `linkage ∈ {static, shared}` cell coordinate. The runtime already builds `cdylib`.
Measure: per-binary footprint with `staticlib` (today's default) vs `cdylib` shared
(amortized across M binaries). Compute the crossover M where shared wins total host
footprint. Record as a Size-board dimension so "ship static or shared" is a *measured*
deployment decision (single-binary CLI → static; many-binaries-one-host / edge-fleet →
shared crossover). **Class retired:** "linkage chosen by default, never measured."

---

## 4. Concrete phases (dependency order; each independently landable with green gates)

> Build/test discipline (CLAUDE.md): `export MOLT_SESSION_ID=size-<phase>` before any
> build; size builds use the **size profiles** (`release-output`/`release-size`/
> `wasm-release`) — these are slow (fat-LTO, cgu=1) so cache aggressively and never
> `cargo clean`; max 2 build-triggering agents; route any raw-binary run through
> `tools/safe_run.py`. Phases 0,1,2,5(measurement),6,7 are **host tooling** (Python, no
> Rust rebuild on the critical path); Phase 3c and Phase 4 (if product-wired) touch Rust.

### Phase 0 — Pin the `SizeBudget` contract + fold size fields into the doc-64 schema

**Deliverable:** in `tools/perf_schema.py` (doc 64 Phase 0's home): the `SizeBudget`
dataclass + `SIZE_BUDGETS` table (§3.1), the `stdlib_tier` + `linkage` cell coordinates,
the `gzip_bytes`/`brotli_bytes`/`wasm_code_bytes`/`size_attribution` fields on `PerfCell`,
and `validate_size_cell()` (fail-closed: GREEN requires stripped + gzip). Migrate the four
scattered budget constants (`binary_size_analysis.py:107-108`, `wasm_size_audit.py:51-53`)
to *import from* `SIZE_BUDGETS` (no behavior change — pure consolidation, the first slice
of dedup). **Gates:** `tests/tools/test_perf_schema.py` round-trips a real size cell;
asserts the four old constants now derive from the one table; `binary_size_analysis.py
--budget`/`wasm_size_audit.py --budget` still pass on a committed artifact (behavior
preserved). **Independently valuable:** one budget source of truth immediately.

### Phase 1 — The Size board projection + tier/linkage/compressed capture (keystone)

**Deliverable:** in `tools/perf_board.py` (doc 64 Phase 1's home): the `size`
`BoardProjection` (§3.2). In `tools/perf_measure.py`: capture `gzip_bytes`/`brotli_bytes`
(stdlib `gzip` + `brotli` if available, else record "brotli unavailable" — fail-closed,
never fake), the section breakdown (call the existing `binary_size_analysis.py`/
`wasm_size_audit.py` parsers as *library functions*), and thread the `stdlib_tier` +
`linkage` coordinates through the build spec. **Reuse, do not rewrite,** the two existing
section parsers — extract their parse functions into importable form if needed (pure
refactor). **Gates:** `--all-boards` emits a well-formed `size_*.json`;
`perf_schema.validate_board()` accepts it; a unit test feeds a hand-built cell list with a
WASM-micro size RED and a native-full green and asserts the Size board FAILs the micro
cell while the native cell PASSes (the asymmetry invariant); `structural_audit --check`
does not regress. **Independently valuable:** the first gated, historied size measurement.

### Phase 2 — Size history + regression gate + per-tier absolute budgets

**Deliverable:** reuse doc 64 Phase 4's `bench/scoreboard/history/` + `perf_history.py` +
`perf_regression.py` for the size board (size deltas are *deterministic* → simpler than
warm-perf: no statistical CI needed, a raw >10% delta is a hard fact per doc 0604 §3).
Seed the size history from the first authoritative nightly sweep. Fill `SIZE_BUDGETS` with
the *measured* baselines + the contract ceilings (3MB Workers, 2MB native hello). **Gates:**
record two synthetic size boards (baseline, then one cell +15%) → regression gate flags
exactly the regressed cell as `error`; a +3% cell is `warn` (creep), under-budget; a
deliberate over-3MB-gzip WASM-micro fixture FAILs the absolute-budget gate.

### Phase 3 — Monomorphization control (3a + 3b + 3c)

- **3a `tools/size_levers.py`:** probe `-Zshare-generics=y` availability; build the
  size-profile runtime with/without; record the DIMENSIONAL delta on the Size board; wire
  into size-profile `RUSTFLAGS` *only if* it wins and the toolchain supports it (else
  record measured-unavailable). **Gate:** the probe reports availability honestly; if
  applied, the runtime still passes the full lib suite + compliance (size lever must not
  change semantics) + the byte delta is recorded.
- **3b `tools/mono_census.py`:** run `cargo llvm-lines`/`cargo bloat` on the size-profile
  runtime, parse top generic families, emit `PerfCell.size_attribution`. **Gate:** on a
  committed runtime artifact, the census names the top-5 generic families with non-zero
  byte attribution; a test asserts the parse is stable.
- **3c structural polymorphization:** for the top *cold* families named by 3b
  (cross-referenced against the doc 65_perf_compression_ladder hot-lane set + cycle
  profile), replace per-`T` monomorphization with one erased-`Repr` body in the runtime.
  Each such change is its own commit. **Gate (the hard one):** the Size board shows the
  measured byte reduction; the **warm perf board shows ZERO regression** (the cold-proof
  must hold — if a "cold" family was actually hot, the perf board catches it and the
  change is reverted, never special-cased); full differential + lib suite green on
  native+wasm. This is where the two lenses are jointly enforced: size down, speed flat,
  semantics identical.

### Phase 4 — WASM split decision + wasm-opt converge lane + 3MB-tier gate

1. **Measure** split-core-vs-monolithic-shaken compressed size on the real edge probe
   (hello-world + a representative Workers handler) at `stdlib_micro`/`stdlib_edge`.
2. **Decide** per §3.5: product-wire (drive `WasmSplitPlan` from `reachability.rs` + real
   post-opt section bytes, validate against doc 0931 export contract) OR demote to
   `research/` with a doc note. **No third option** (leaving the stub as-is is forbidden).
3. **Add the measured `wasm-opt -Oz --converge` lane** (doc 0931 "high-value work #1") as
   a Size-board input with before/after export-contract verification (the contract's
   reproducible before/after check).
4. **Gate** the compressed-WASM cell against the per-tier 3MB ceiling.
**Gate:** the decision is recorded with the measurement that drove it; if product-wired,
the split output validates + passes the doc-0931 linked-Falcon/Tinygrad smoke; the
`--converge` lane records its byte delta + export-contract match.

### Phase 5 — Per-tier footprint evidence + dylib/static crossover lane

1. **Per-tier board rows:** sweep hello-world (and a small representative app) across
   `stdlib_{micro,edge,standard,server,full}` × size profiles, so each tier's footprint is
   a measured Size-board row — the evidence layer for "which tier should this deployment
   use" and the measurement that *quantifies* what 21e's satellite dedup + arc 60's
   tree-shaking save.
2. **Linkage lane:** add the `linkage ∈ {static, shared}` coordinate; measure
   per-binary-static vs amortized-shared-cdylib; compute + record the crossover-M.
**Gate:** the tier board shows monotonic-or-explained footprint up the chain (micro ≤ edge
≤ … ≤ full, since it is a strict-superset feature chain — a *non-monotonic* row is a bug
the gate flags); the linkage lane records both footprints + the crossover.

### Phase 6 — Size cause-attribution (the Size analogue of perf_causality)

Extend doc 64 Phase 5's `perf_causality.py` (or a sibling `size_causality.py`) to, for a
Size RED/regression, join: (a) the `mono_census.py` generic-family delta, (b) the section
breakdown delta, (c) the `_runtime_feature_gates.py` domain map (which stdlib domain's
symbols grew) → emit `size_cause ∈ {monomorphization/<family>, stdlib_domain/<feature>,
panic-tables, unwind-residue, debug-info, data-segment, intrinsic-address-taken}`. The
"intrinsic-address-taken" cause is doc 51 §8's named binary-size fact (an intrinsic whose
address is taken cannot be DCE'd) — this attribution surfaces it. **Gate (falsifiable):**
feed a known regression fixture (e.g. flip a `stdlib_*` feature on in a tier) and assert
`size_causality` attributes it to `stdlib_domain/<that feature>`; the
monomorphization-family attribution reproduces the `mono_census` top family.

### Phase 7 — CI wiring (Size board into the doc-64 perf tier)

Add the Size board to the doc-64 perf gate (`tools/ci_gate.py` perf tier +
`.github/workflows/perf-validation.yml`): a **per-PR smoke size gate** (build hello-world
at `release-output`+`micro` and `wasm-release`+`micro`, hard-gate absolute budget +
regression-vs-merge-base — size is deterministic so this hard-gates safely per §3.2's
note) and the **nightly full size sweep** (all tiers × profiles × backends, seeds the size
history baseline). Register in `pr_trust_gate.yml`. **Gate:** dry-run lists the size check;
a deliberate +20% fixture fails the gate locally; YAML lints.

---

## 5. Verification / gates per phase (measurement discipline)

- **Deterministic-size gate (every phase):** unlike warm perf, size is a reproducible
  byte count — the Size board hard-gates per-PR (no quiescence dependency). A size cell
  GREEN requires stripped **and** gzip (fail-closed via `validate_size_cell`).
- **Compressed-not-just-stripped gate (Phase 1+):** a cell missing gzip is malformed; the
  3MB/2MB contracts are checked on the *compressed/stripped* unit the contract names.
- **Cold-proof gate (Phase 3c — the load-bearing one):** every polymorphization commit
  must show **zero warm-perf regression** on the doc-64 CPython+Backend boards. If the
  perf board moves, the "cold" classification was wrong → revert, never special-case (the
  per-test-special-case rule). Size and speed are *jointly* gated, never traded silently.
- **Lever-honesty gate (Phase 3a):** `-Zshare-generics` is recorded as
  measured-applied / measured-unavailable, never silently assumed (Risk 3).
- **Export-contract gate (Phase 4):** any WASM split / wasm-opt step validates the doc
  0931 export contract + passes linked Falcon/Tinygrad smoke before/after (no size win at
  the cost of a missing required export — doc 0931 disallowed shortcut).
- **Monotonic-tier gate (Phase 5):** a strict-superset feature chain must produce
  monotonic-or-explained footprint; a non-monotonic tier row is a bug.
- **Falsifiable-attribution gate (Phase 6):** `size_causality` must reproduce a known
  injected cause (flip a feature → attributed to that domain).
- **Ratchet gate (all phases):** new tools are leaf modules; `structural_audit --check`
  must not regress `god_files`/`duplicate_authorities` (the budget table is the ONE size
  authority — `duplicate_authorities` stays 0).
- **No-fake-number gate:** brotli/`cargo llvm-lines`/`-Zshare-generics` unavailable →
  recorded as unavailable, never invented (mirrors doc 64's PyPy/Codon host-absent rule).

Every Size-plane PR runs (fast, mostly cargo-free): `--all-boards`, the schema tests, the
asymmetry test, `structural_audit --check`. Phase 3c/4 PRs additionally run the full
native+wasm differential + the cold-proof perf board.

---

## 6. How it composes with the decomposition (21a-e) and the 50-59 arcs

### Composition with the 50-59 portfolio (this arc DEEPENS three levers)

- **DEEPENS doc 64 (`64_perf_scoreboards_and_harness.md`) — the measurement plane.** The
  Size board is the **sixth `BoardProjection`** over doc 64's *existing* `PerfCell` stream.
  This arc adds **no new build loop** — it adds the size *view*, the size *budgets*, the
  size *history* (reusing doc 64's history machinery), and the size coordinates
  (`stdlib_tier`, `linkage`) to the cell. **Cross-arc dependency:** this arc is blocked-by
  doc 64 Phase 0 (schema home) + Phase 1 (measurement core) + Phase 4 (history) + Phase 5
  (causality), and it *extends* each. It must land its schema/board additions *into* doc
  53's files (`perf_schema.py`, `perf_board.py`, `perf_measure.py`) — coordinate ownership
  with the doc-64 implementing agents (additive fields/projections, no behavior change to
  the existing five boards).
- **DEEPENS arc 60 (tree-shaking / whole-program DCE).** Arc 60 builds the symbol/domain
  tree-shaker on the `_runtime_feature_gates.py` substrate; THIS arc **measures what it
  saves** (the per-tier footprint board, Phase 5) and **gates that the savings don't
  regress** (the Size board). Arc 60 *adds* the dead-code-elimination fact; arc 61
  *confirms the class is retired* by measuring the byte drop. **Seam:** arc 60 owns the
  tree-shaking mechanism (which symbols/domains to drop); arc 61 owns the size scoreboard
  that proves it worked. They co-evolve like doc 64 ↔ the fact plane.
- **DEEPENS doc 21e (runtime satellite dedup) + the runtime tiers.** 21e dedups the 24
  in-tree/satellite pairs and forces satellites on in reduced tiers; THIS arc measures the
  **per-tier footprint delta** of those dedups (a satellite dragging `rustls`/`mio`/
  `serde_json` into `micro` is a *size regression* arc 61's board catches — exactly 21e's
  "heavy-dep leak into micro" risk, made gateable). 21e's Option-A-vs-B decision rule
  ("light → delete, heavy → `#[path]`") becomes **evidence-driven via the Size board**.

### Composition with the decomposition program (21a-e)

- **No god-file growth.** All new tooling is leaf Python modules; the Rust changes (3c
  polymorphization) *reduce* code (N monomorphizations → 1 erased body) and touch
  `runtime/molt-runtime` along existing seams. The four scattered budget constants
  *collapse* to one table (a dedup, aligned with 21e's anti-duplication spirit).
- **21b crate-graph alignment:** the `mono_census` runs against the size-profile runtime
  staticlib; as 21b splits `molt-ir ← molt-passes ← molt-lower`, the census naturally
  attributes per-crate (it already uses `cargo bloat --crates`), so decomposition makes
  the attribution *sharper*, not harder.
- **Dependency direction:** the Size plane is strictly downstream of build (it weighs
  artifacts) + analysis (`mono_census`, the feature-gate map) — no cycle, same posture as
  doc 64's plane.

### Composition with the multi-agent / three-lane model

Squarely **Lane C** (CLAUDE.md: "infra/scoreboards/decomposition that makes A&B faster… C
is never decorative") — *except* Phase 3c, which is **Lane B** (a real representation fix
that retires a size-waste class). The plan is parallel-friendly: Phase 1 (into doc 64's
files) is the serialization point; after it, Phase 2 (history), Phase 3a/3b (levers/census,
pure tooling), Phase 5 (tier board), Phase 6 (attribution) fan out as pure-Python agents
(no cargo, no build-cap contention); Phase 3c + Phase 4-product-wire are the only
Rust-building agents (serialize through the daemon, max 2). **Every batch reports the
PERF/SPEED STATUS block AND a SIZE STATUS block** (over-budget cells + suspected cause;
size regressions vs history; the fastest next unlock = one budget / one census family / one
cold-erasure).

---

## 7. Parallel execution map (file ownership, no overlaps)

| Phase | Owner files (new unless noted) | Touches Rust? | Blocks / blocked-by |
|---|---|---|---|
| 0 | `tools/perf_schema.py` (+SizeBudget/fields, shared w/ doc 64 — additive); migrate consts in `binary_size_analysis.py`/`wasm_size_audit.py`; `tests/tools/test_perf_schema.py` | no | blocked-by doc64 P0; blocks all |
| 1 | `tools/perf_board.py` (+size projection), `tools/perf_measure.py` (+tier/linkage/compressed) — shared w/ doc 64 (additive) | no | blocked-by doc64 P1 + this P0; blocks 2-6 |
| 2 | `tools/perf_history.py`/`perf_regression.py` (reuse), `SIZE_BUDGETS` fill | no | blocked-by 1 |
| 3a | `tools/size_levers.py` | yes (size-profile rebuild, measure) | blocked-by 1 |
| 3b | `tools/mono_census.py` | yes (cargo bloat/llvm-lines run) | blocked-by 1 |
| 3c | `runtime/molt-runtime/src/**` (cold-family erasure) | **yes (representation fix)** | blocked-by 3b + doc65 ladder + doc64 P5; serialize build |
| 4 | `runtime/molt-tir/src/tir/wasm_split.rs`+`wasm_streaming.rs`+`wasm_component.rs` (wire or demote); `tools/wasm_optimize.py` (+converge lane measured) | **yes (if product-wire)** | blocked-by 1; serialize build |
| 5 | `tools/perf_measure.py` (tier/linkage sweep specs); per-tier board rows | no (measurement) | blocked-by 1 |
| 6 | `tools/size_causality.py` (or extend `perf_causality.py`) | no | blocked-by 3b + doc64 P5 |
| 7 | `tools/ci_gate.py`, `.github/workflows/perf-validation.yml`, `pr_trust_gate.yml` | no | blocked-by 1-2 |

Most phases are pure-Python (no build-cap contention); only 3c + 4(product) trigger Rust
builds → serialize those two through the daemon.

---

## 8. Risks + structural (not band-aid) treatment

### Risk 1: A second size measurement loop / second budget source of truth (the trap)
**Band-aid (rejected):** a standalone size-CI script with its own build + budgets.
**Structural fix:** the Size board is a `BoardProjection` over doc 64's *one* `PerfCell`
stream; the `SizeBudget` table is the *one* budget authority (the four scattered constants
collapse into it). `duplicate_authorities` stays 0.

### Risk 2: Optimizing stripped bytes while the compressed (shipped) artifact grows
**Band-aid (rejected):** gate on stripped size only (today's default).
**Structural fix:** §3.4 — every cell carries gzip + brotli; the contract ceilings gate on
the compressed unit; GREEN is malformed without compressed bytes.

### Risk 3: `-Zshare-generics` / `cargo llvm-lines` / brotli unavailable on the toolchain/host
**Band-aid (rejected):** assume the flag works / invent a number / silently skip.
**Structural fix:** §3.3a + the no-fake-number gate — availability is *probed* and recorded
(measured-applied / measured-unavailable), mirroring doc 64's PyPy/Codon host-absent
`ADVISORY` pattern. The lever lights up with zero code change when the toolchain supports it.

### Risk 4: A polymorphization (3c) silently slows a path that was actually hot
**Band-aid (rejected):** trust the cold classification; or special-case the failing bench.
**Structural fix:** the **cold-proof gate** — every 3c commit must show zero warm
regression on the doc-64 perf boards; the classification is *derived* from the ladder
hot-lane set + cycle profile #76, and if the perf board moves, the change is reverted (the
family was hot). Size is traded for speed *only* where the proof holds. Two lenses, jointly
gated.

### Risk 5: WASM split stubs stay half-wired ("sharp edge left for later")
**Band-aid (rejected):** leave `wasm_split.rs` as an estimate-only stub.
**Structural fix:** §3.5 forces a binary decision (product-wire from real reachability OR
demote to `research/`), evidence-driven; no third "leave it" option.

### Risk 6: The size win regresses correctness (drops a required export / changes semantics)
**Band-aid (rejected):** a test-specific export allowlist (doc 0931 disallowed).
**Structural fix:** the export-contract gate (doc 0931 reproducible before/after +
linked-Falcon/Tinygrad smoke) on every WASM size step; the full differential on every 3c
change. A size optimization that breaks a symbol is not a win, it is a regression.

### Risk 7: Size profiles are slow (fat-LTO, cgu=1) → the size gate is too slow per-PR
**Band-aid (rejected):** run the full size sweep per-PR and time out / flake.
**Structural fix:** the **tiered suite** (mirroring doc 64 §3.3) — per-PR gates only the
two contract cells (`release-output`+`micro`, `wasm-release`+`micro`) on a cached
incremental build; the full tier×profile×backend sweep is nightly and seeds the baseline.
Size is deterministic so the per-PR cells hard-gate safely (no quiescence cost).

### Risk 8: Cold-start conflated with runtime-init (CLAUDE.md: init is 0.127ms)
**Band-aid (rejected):** blame the runtime, optimize init.
**Structural fix:** §2 end + Phase 1 fold `output_startup_size_audit.py`'s
`cold_first_sighting` (cdhash/codesign) + `page_cache_cold` (page-in) into the Size board
as the footprint→cold-start dimension family; cold-start improvements are pursued as
*artifact footprint + codesign* facts (smaller artifact = fewer pages = faster cold), never
as a runtime-init red. This is the doc 64 Risk-8 discipline applied to the size lever.

---

## 9. The landing-report this arc makes automatic (the SIZE STATUS block)

When complete, every PR's gate emits — mechanically — the size half of the CLAUDE.md
landing report:

> **size matrix green** (Size board: 0 over-budget cells across backend×profile×tier×
> linkage; 0 previously-green size regressions vs history); **contract ceilings held**
> (wasm-release+micro hello ≤ 3MB gzip; native release-output+micro hello ≤ 2MB stripped);
> **compressed AND stripped reported**; **monomorphization attributed** (top-N generic
> families named, cold families erased, hot families left for the Repr ladder);
> **share-generics: measured-applied/unavailable**; **tiers monotonic**; **dylib/static
> crossover recorded**. Artifacts: `bench/scoreboard/size/<id>.json`.

That block is the product of this arc. Today the size of a molt artifact is a number a
human finds with `nm` after a customer complains. After this arc, the machine weighs every
artifact across the full footprint matrix, attributes every byte to a representation cause,
and the merge gate refuses silent growth — **artifact footprint becomes a release-gating
Performance-Constitution dimension in fact, not just in the constitution.**

---

## Appendix A — Exact file/line anchors (for the implementing agents)

- Cargo size profiles to gate the output of: `Cargo.toml` `release-output` (378-384),
  `release-size` (416-422), `wasm-release` (431-437), `wasm-release-fallback` (545-564);
  the measured hot-crate opt-level policy (462-532, the 25.5% wasm measurement is the
  template for "measure before/after, record the delta").
- `-Zshare-generics`/RUSTFLAGS seam (Phase 3a): `.cargo/config.toml` (the per-target
  `rustflags` arrays, lines 8-34 — size-profile flags would be added via env/`RUSTFLAGS`
  not hard-coded here, per the file's "keep the baseline portable" rule).
- Native size parser to reuse as a library (Phase 1): `tools/binary_size_analysis.py`
  `analyse_native`/`_parse_macho_segments`/`_categorise_symbol` (200-378); the scattered
  budgets to migrate to `SIZE_BUDGETS` (107-108).
- WASM size parser to reuse (Phase 1): `tools/wasm_size_audit.py` section parser + the
  16MB/10MB/4MB budgets (51-53, V8-OOM-driven — distinct from the 3MB Workers *contract*
  ceiling; both belong in `SIZE_BUDGETS` with provenance notes).
- The matrix shape to fold in (Phase 1/5): `tools/output_startup_size_audit.py`
  `MatrixCase` (50-67, already has `stdlib_profile`/`wasm_opt_level`/`linked`),
  `_measure_cold_first_sighting` (542-577, the true-cold cdhash discipline),
  `_budget_status` (778-810, the opt-in budget the arc makes gated/historied).
- Runtime tiers (Phase 5): `runtime/molt-runtime/Cargo.toml` `stdlib_micro`/`edge`/
  `standard`/`server`/`full` chain (41-75); `default=["stdlib_full"]` (16).
- Tree-shaking substrate (arc 60 seam, Phase 5/6): `src/molt/_runtime_feature_gates.py`
  `RUNTIME_FEATURE_GATES` (36-128) + `LINK_AFFECTING_FEATURES` (176-197).
- WASM split/component/streaming stubs to wire-or-demote (Phase 4):
  `runtime/molt-tir/src/tir/wasm_split.rs` (`plan_split` name-prefix heuristic 16-36,
  `estimate_sizes` `ops*8` 39-64), `wasm_streaming.rs`, `wasm_component.rs`; the real
  reachability source: `runtime/molt-passes/src/tir/passes/reachability.rs`.
- wasm-opt converge lane (Phase 4): `tools/wasm_optimize.py` `_DEFAULT_FEATURE_FLAGS`
  (45-56, the load-bearing `--disable-gc`/`--disable-custom-descriptors` set);
  `tools/wasm_link.py` (`--gc-sections`, rec-group flatten); doc 0931 "high-value work #1".
- Measurement-plane integration points (Phase 0/1/2/6): doc 64 §Appendix A — `perf_schema.py`,
  `perf_measure.py`, `perf_board.py`, `perf_history.py`, `perf_causality.py`; the
  `PerfCell` already carries `binary_size_kib`/`compile_time_s`/`molt_peak_rss_mib`.
- Budget files (Phase 2): `bench/scoreboard/cold_start_budget.json` (the existing budget
  pattern to mirror for `bench/scoreboard/size/`).
- Doctrine to update (Phase 7): `docs/spec/areas/perf/0604_BINARY_SIZE_AND_COLD_START.md`
  (its prose `cargo bloat`/`llvm-lines`/`twiggy`/`brotli` becomes implemented + gated);
  `docs/spec/areas/compiler/0931_LINKER_OPTIMIZATION_CONTRACT.md` (mark "size dashboards"
  high-value-work as delivered by this arc).

## Appendix B — Why this is a compression-ladder unit, not "a size dashboard"

A dashboard reports bytes. This plane makes a *class* of failure unexpressible:
- "a commit silently grew an artifact on some backend/profile/tier and merged" → the gate
  refuses it;
- "cold code paying monomorphization tax" → §3.3c makes it structurally unexpressible by
  erasing cold generic families (the new build fact: *hot/cold status decides
  monomorphize-vs-erase*);
- "an edge artifact that passes stripped but fails the 3MB compressed ceiling in
  production" → the compressed-gate refuses it;
- "a heavy dep leaking into the micro tier" → the per-tier monotonic gate catches it.
Each is a *class* retired by a first-class fact (a budget, a cold-erasure, a compressed
ceiling, a tier monotonicity invariant), per the doc-51 method. Once these facts exist and
gate, the entire family of silent-size-drift and cold-monomorphization-bloat bugs is gone
from main — and every downstream arc inherits a size plane sharp enough to name the
representation cause of the next byte it must remove.
