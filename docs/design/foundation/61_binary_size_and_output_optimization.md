> 2026-11-21 status: the Luau and Rust-source targets and the direct WASM backend named in this
> design are deleted. Shipped backends are Cranelift and LLVM; WASM builds through LLVM wasm32.
> Current authorities: `docs/design/foundation/call_anatomy.md` and `78_whole_program_type_flow.md`.

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

> **Status 2026-11-10 (HF-222).** The doc-64 homes this plan extends for its
> board and history phases are retired: `tools/perf_board.py`,
> `tools/perf_history.py`, `tools/perf_regression.py` and
> `bench/scoreboard/history/`. Phases 1 and 2 need a new home before they start;
> size deltas are deterministic, so the instruction signal's model (a farm store
> of write-once records compared with the nearest measured ancestor,
> `tools/perf_instructions.py`) is the candidate. Phase text below names the
> retired files as design history.

> **One-line thesis.** molt already ships *seven* hand-tuned Cargo profiles and a
> sophisticated per-crate opt-level policy, and it already has three size-measurement
> scripts — but **size is not gated, monomorphization bloat is completely uncontrolled
> (no `share-generics`, no polymorphization, the #1 Rust binary-size killer), the
> now-retired WASM split/component/streaming TIR stubs exposed how easily estimate-only
> code can masquerade as product authority, and the size budgets are scattered magic
> constants (35MB native / 20MB / 16MB
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
   The estimate-only TIR split/component/streaming stubs are retired from the compiled
   product surface. Future split/component/streaming work must be product-wired from
   emitted-WASM artifact facts: real reachability, import/export/type sections,
   post-link/post-`wasm-opt` section bytes, compressed size-board rows, and the
   `--gc-sections`/`wasm-opt -Oz --converge` contract of doc 0931. The
   compressed-WASM cell is gated against the Cloudflare 3MB ceiling per tier.
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
| **Tuned Cargo profiles** | `Cargo.toml` shipping profiles | `release-output` (opt-3) and `release-size` (opt-z) use fat LTO in one codegen unit (measured 2026-11-15 in `Cargo.toml`); `wasm-release` follows the [measured WASM policy](../../spec/areas/wasm/WASM_OPTIMIZATION_PLAN.md). All use debug=0, panic=abort, strip; package overrides own only hot-crate opt levels. `dev-release` is the sole symbol-bearing release profile. | **No `-Z share-generics`/polymorphize**; output still needs the Size-board projection below |
| Native size analyser | `tools/binary_size_analysis.py` | Compiler inspector section/symbol facts, disjoint file-byte attribution, explicit unknown naming, `--compare` deltas, `--budget`, JSON | Budgets are **per-invocation magic constants** (35MB native / 20MB wasm, lines 107-108); **no history, no gate wiring, no per-tier budget, no compressed size, no cause attribution** |
| WASM size auditor | `tools/wasm_size_audit.py` | per-section LEB128 parse, code/data split, `--budget 16MB`/`--budget-code 10MB` gate (V8 OOM headroom) | Standalone; **not a board projection**; budgets are V8-OOM-driven not the 3MB Workers contract; no compressed size; no history |
| Output+startup+size matrix | `tools/output_startup_size_audit.py` (1239 ln) | builds hello-world across `native/wasm/luau/mlir × dev/release × auto/llvm × stdlib-{micro,full}`; records artifact bytes, cold-first-sighting + page-cache-cold + same-path startup, CPython + C baselines, `--max-artifact-mb`/`--max-fresh-start-ms` budget checks | The right *matrix shape* but **budgets are opt-in CLI flags (default None → never gates)**; writes a timestamped JSON, **no history index, no regression gate, not a doc-64 projection, no cause attribution, stripped-only (no gzip/brotli)** |
| WASM opt pipeline | `tools/wasm_optimize.py`, `tools/wasm_link.py`, `tools/wasm_pipeline.py` | `wasm-opt` invocation with the *load-bearing* feature flag set (`--disable-gc`, `--disable-custom-descriptors`, the rec-group flatten); export-contract preservation | `wasm-opt -Oz --converge` lane named as "high-value work" in doc 0931 but **not measured/gated**; the optimize step is not size-board-attributed |
| Runtime tiers | `runtime/molt-runtime/Cargo.toml` lines 16-120 | strict-superset feature chain `stdlib_micro ⊂ edge ⊂ standard ⊂ server ⊂ full`; `default=["stdlib_full"]`; per-domain `dep:`-backed features that drop crates when off | The tier *mechanism* exists; **no per-tier size budget, no measured per-tier footprint, no "which tier should this deployment use" evidence** (21e dedups satellites; THIS arc measures their size effect) |
| Symbol feature-gate map | Each intrinsic's `# cfg:` line in `runtime/molt-runtime/src/intrinsics/manifest.pyi` + generated `src/molt/_runtime_feature_gates.py` | One exact availability fact per executable (no name prefixes); generated resolver absence probes prove each definition's cfg equals it (HF-487); the generated Python module feeds resolver generation, WASM exports and compile-time refusal | This is the **tree-shaking substrate arc 60 builds on**; this arc *measures* what each gate saves and feeds it to the Size board |
| Linker contract | `docs/spec/areas/compiler/0931_LINKER_OPTIMIZATION_CONTRACT.md` | `--gc-sections`/`--export-if-defined`/no-ICF-while-fn-addrs-are-identities rules; "add size dashboards… raw/gzip/function-count/data-segment/export-count" as high-value work | Names the dashboard as TODO; **this arc IS that dashboard, as a board projection** |
| Size/cold-start spec | `docs/spec/areas/perf/0604_BINARY_SIZE_AND_COLD_START.md` | the metric definitions (stripped+unstripped, raw+gzip+brotli, `llvm-size`/`cargo bloat`/`cargo llvm-lines`/`twiggy`), 10% regression rule | **All of it is prose** — no tool implements `cargo bloat`/`llvm-lines`/`twiggy`/`brotli`; this arc operationalizes the spec |
| WASM split/component/streaming | Retired from `runtime/molt-tir/src/tir/` on 2026-06-27 | The former code was **estimate-only stubs** (`ops*8` byte heuristic; WIT generation; hot/cold manifest) — name-prefix categorization, NOT wired to real emitted modules or product output | **Decision landed (§3.5):** no TIR pseudo-authority remains. Future product work must derive split/component/streaming facts from real backend-emitted WASM artifacts and linker/optimizer output |
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
  → **requires** no TIR pseudo-authority. The former stubs are retired; any product split
     must consume real emitted WASM artifact facts (post-`wasm-opt`,
     post-`--gc-sections`, section/import/export/type metadata, compressed bytes).
     **FACT NEEDED:** if product-wired, a split plan driven by real reachability (the
     existing `reachability.rs` pass) and emitted artifact measurements, not name prefixes.

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
section breakdown (native physical ranges via the compiler-owned facts consumed
by `binary_size_analysis.py`; WASM sections via the existing `wasm_size_audit.py`
owner). The edge-deploy contract (Cloudflare 3MB) is a **compressed** ceiling; gating
on stripped-only (the current default) can pass a cell that fails in production. **Class
retired:** "an optimization that shrank uncompressed bytes but grew the compressed
artifact" (real — opt choices that add entropy can do this).

### 3.5 DECISION: WASM split/component/streaming stubs retired (retires "stub masquerading as capability")

`wasm_split.rs`/`wasm_component.rs`/`wasm_streaming.rs` were removed from
`runtime/molt-tir/src/tir/` on 2026-06-27. They were estimate-only
(`ops.len() * 8` byte heuristic, name-prefix categorization, no connection to the real
emitted module or the linker), so leaving them in the compiled crate made the size path
look more product-wired than it was. **The decision has landed: no TIR pseudo-authority.**

Future split/component/streaming work has one valid path: product-wire from the real
backend artifact stream. That means reachability from `reachability.rs`, actual
post-link/post-`wasm-opt` section bytes, import/export/type metadata, compressed size
rows, and the doc 0931 export contract. The evidence question remains valid — measure
split-core vs monolithic-shaken compressed size on the real edge probe — but the
measurement must start from emitted artifacts, never from TIR function names or op-count
estimates. **Class retired:** "estimate-only code that masquerades as a product size
mechanism."

#### Freestanding control flow enters the existing optimizer (HF-639)

Commit `f6973ba2c5` introduced post-link WASI import stubbing so freestanding
deployment can reuse the `wasm32-wasip1` dependency tree. This does not imply
identical runtime bytes: the freestanding runtime selects `wasm_freestanding`
and the unsupported getrandom backend. The application module can be identical
while the runtime link input differs. Runtime generation receipts, not a common
target directory or filename, identify those inputs.

The SIMD restriction added in `31a183a4a8` was a workaround for the stub
rewriter's then-unsupported instructions. Commit `76aebc3a` added SIMD/atomics
rewriting and explicitly removed that limitation, but the CLI continued to force
scalar codegen. That stale consumer made freestanding compile a different,
larger instruction path than hosted WASM. Both deployment modes now use the
existing SIMD128 baseline through
`molt.cli.runtime_wasm_build_policy.runtime_wasm_simd_policy`; source builders,
release-cell selectors and distributed-runtime producers consume this authority.
Freestanding host-service features remain independent of that instruction-set
choice. Explicit scalar runtime coordinates remain distinct in build identity;
the CLI does not select them as an implicit consequence of removing WASI.

WASI stubbing is a control-flow transform. A call that could return now traps;
its arguments and preceding side effects still execute, but its continuation
cannot. Stubbing therefore belongs **before the existing post-link Binaryen
optimizer**, after structural export cleanup and before resource normalization
and final publication. Binaryen's
[inliner propagates unreachability and refinalizes callers](https://github.com/WebAssembly/binaryen/blob/version_133/src/passes/Inlining.cpp),
allowing the same size pipeline to remove those continuations and their unused
dependencies. Optimizing before stubbing leaves that information unavailable.
Adding a second whole-module optimizer would duplicate work and optimize the
wrong input first.

New traps must propagate through multiple Rust/libc wrapper layers. When
stubbing actually replaces imports, the same optimizer invocation uses the
existing level's fixed-point policy (`wasm_opt_converges`); `O1` retains its
single-pass budget. Ordinary WASI links and modules with no WASI imports keep
their single link pass. This pays the pruning cost only for the control-flow
change that needs it. Reordering alone, or retaining the stale scalar guard
while iterating cleanup, does not satisfy the hello-world size oracle.

`tools/wasm_link_pipeline.py::_post_link_transform_stage` owns this ordering.
The selected optimization level, export roots, optimizer identity and failure
handling remain the existing authorities; the optimizer attestation now binds
the stubbed input. Unoptimized development builds still perform stubbing without
launching Binaryen. Final structural, export, callable-table and zero-WASI-import
validation remain mandatory. The independent host-call/trap execution fixture
in `tests/test_wasm_optimization_imported_memory.py` and the unchanged
`tests/test_wasm_size_tracking.py` oracle cover behavior and linked size.

The unchanged hello-world oracle measured the following linked outputs on
`fd80f0423d` and the HF-639 correction with the same slot-06 fixture path:

| Deployment | Before bytes | After bytes | Before functions | After functions |
| --- | ---: | ---: | ---: | ---: |
| WASI | 7,592,970 | 7,592,970 | 7,742 | 7,742 |
| Freestanding | 7,612,185 | 7,592,535 | 7,789 | 7,654 |

The WASI output is byte-identical. Freestanding loses 19,559 code-section bytes
while data grows 206 bytes; it validates with zero WASI imports. These are
hello-world artifact measurements, not a whole-program performance claim.

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
> `wasm-release`) — these use the measured target policies in `Cargo.toml`, so cache aggressively and never
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
2. **Product-wire only from real artifacts** per §3.5: the TIR stubs are retired, so the
   split decision must drive from `reachability.rs` + real post-opt section bytes and
   validate against doc 0931 export contract. **No third option** (reintroducing a TIR
   name-prefix or op-count estimate is forbidden).
3. **Add the measured `wasm-opt -Oz --converge` lane** (doc 0931 "high-value work #1") as
   a Size-board input with before/after export-contract verification (the contract's
   reproducible before/after check).
4. **Gate** the compressed-WASM cell against the per-tier 3MB ceiling.
**Gate:** the retired-stub guard stays green; if product-wired, the split output
validates + passes the doc-0931 linked-Falcon/Tinygrad smoke; the `--converge` lane
records its byte delta + export-contract match.

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
| 4 | Retired TIR stubs stay absent; product split work must live in backend emitted-WASM artifact facts + `tools/wasm_optimize.py` (+converge lane measured) | **yes (if product-wire)** | blocked-by 1; serialize build |
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
**Structural fix:** §3.5 retired the TIR stubs entirely. Product split work may return
only as backend-emitted artifact facts driven by real reachability, sections, imports,
exports, compressed bytes, and the export contract; no third "leave it" or estimate path.

### Risk 6: The size win regresses correctness (drops a required export / changes semantics)
**Band-aid (rejected):** a test-specific export allowlist (doc 0931 disallowed).
**Structural fix:** the export-contract gate (doc 0931 reproducible before/after +
linked-Falcon/Tinygrad smoke) on every WASM size step; the full differential on every 3c
change. A size optimization that breaks a symbol is not a win, it is a regression.

### Risk 7: Size profiles remain expensive → the size gate is too slow per-PR
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

- Cargo size profiles to gate the output of: `Cargo.toml` `release-output`,
  `release-size`, and `wasm-release`; the measured hot-crate opt-level policy (the 25.5% wasm measurement is the
  template for "measure before/after, record the delta").
- `-Zshare-generics`/RUSTFLAGS seam (Phase 3a): `.cargo/config.toml` (the per-target
  `rustflags` arrays, lines 8-34 — size-profile flags would be added via env/`RUSTFLAGS`
  not hard-coded here, per the file's "keep the baseline portable" rule).
- Native size parser to reuse as a library (Phase 1): `tools/binary_size_analysis.py`
  `analyse_native`/`_native_file_accounting`/`_categorise_symbol`; the scattered
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
- Tree-shaking substrate (arc 60 seam, Phase 5/6):
  per-intrinsic `# cfg:` declarations in
  `runtime/molt-runtime/src/intrinsics/manifest.pyi` + generated
  `src/molt/_runtime_feature_gates.py` consumer data.
- WASM split/component/streaming stubs retired (Phase 4):
  `runtime/molt-tir/src/tir/wasm_split.rs`, `wasm_streaming.rs`, and
  `wasm_component.rs` are deleted and guarded against reintroduction. The real
  reachability source for any future product split is
  `runtime/molt-passes/src/tir/passes/reachability.rs`; emitted artifact facts belong in
  the WASM link and optimizer path, not in TIR name heuristics.
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

### Native analyzer attribution contract

`tools/binary_size_analysis.py` consumes the existing `molt-backend` read-only
`--scan-native-artifact-facts PATH` command. The maintained `object` reader owns
ELF/Mach-O decoding and `rustc-demangle` owns Rust name decoding. Human-readable
LLVM/nm output is not a data protocol. Raw names use tagged UTF-8 text with JSON
escaping, or byte arrays only when the name is not UTF-8. Mach-O decoration is
removed only for demangling and classification; the original name is retained.
An optional demangled field is emitted only when it differs. Known workspace
crate roots come from Cargo manifests and exact runtime ABI names from the
intrinsics manifest. Classification describes names, not proven source
ownership, language, reachability or callability. Unmatched names, including
unproven user code and unowned crate roots, remain `unknown`.

Native attribution schema 1 uses disjoint file intervals. Section offsets and
lengths are bounded by the artifact or validated universal Mach-O slice.
The section's declared size, compression format, physical range and uncompressed
length remain separate; the ELF compressed section sh_size is not called a
virtual extent.
Compressed sections never borrow symbol virtual lengths as physical extents.
Zero-fill sections have no file bytes. ELF declared function/object extents may
attribute backed, uncompressed section bytes; aliases count once, conflicting
names and overlapping sections remain unknown. Other bytes are explicitly
`outside_sections`, with no invented claim that they are all debug or padding.
Mach-O nlist has no symbol lengths: its names and counts are available, while
symbol extents are unavailable. No next-address estimate enters file accounting.
Native symbol tables may overlap and their declared sizes must not be summed.
The endpoint sweep takes O(n log n) time and O(n) auxiliary space in the number
of sections and symbols; it never allocates one entry per artifact byte.

The inspector digest and size describe the exact parsed byte buffer. Input and
complete JSON response are separately bounded by the existing backend request
byte limit (`MOLT_BACKEND_STDIN_REQUEST_LIMIT_BYTES`, default 512 MiB). It holds
one bounded input, one bounded output, reader metadata and one temporary demangle;
it does not construct a second owned tree of every name. The Python receiver
holds the JSON and decoded tables plus sweep events under the existing process
memory guard. Limits are ceilings, not predicted peak RSS; allocator capacity,
JSON object expansion and large-artifact latency must be measured independently.
The inspector adds compiler executable code, with no guest runtime hook.

The analyzer selects an already admitted installed compiler or published
feature-specific development compiler; `--native-facts-scanner` explicitly
selects an existing inspector. Analysis never runs Cargo or auto-builds. Receipt
byte identity is joined to stable executable capture before execution, and input
and executable mutation fences close before publication. An old compiler that
lacks this command fails; it does not fall back to symbol-text parsing.

ELF and thin Mach-O are the actual retained format fixtures. Canonical big-endian
fat32/fat64 headers are handled through maintained object readers and the existing
native header authority; universal output remains an independent qualification
cell. Swapped fat headers are explicitly unsupported by the maintained reader.
COFF and archives were outside this analyzer's detected formats and remain outside
this aperture. Unsupported/malformed facts and nonempty scanner stderr fail;
missing tools do not produce empty success. WASM continues to use
`molt.wasm_artifact`. JSON, comparisons and analysis capsules retain the native
denominator, method, overlap counts and explicit unknowns. Native JSON from the
retired symbol-sum schema must be regenerated.

Native size inspection preserves ELF raw symbol values and projects the Arm
Thumb function-state tag from the maintained reader's exact target and symbol
type. Only that typed tag is removed when mapping function bytes; ordinary odd
data addresses and other architectures remain unchanged. Mach-O dylib, dyld and
bundle images share the existing native header's shared-image kind; symbol
extents remain unavailable for all of them.

Native comparisons retain each input digest, ordered slice context, inspector
identity and naming-authority identities. Whole-file deltas remain available
across formats. Category deltas are unavailable when symbol-extent capability,
attribution method, inspector identity or naming authorities differ. This avoids
presenting a change in measurement capability as an optimization. JSON CLI
stdout contains one complete object; budget diagnostics use stderr in JSON mode
and preserve the existing budget exit status.

Native and WASM file inspectors share regular-artifact descriptor admission in
the existing backend input owner. Unix uses nonblocking open before descriptor
regular-file validation, so an unopened FIFO does not wait for a writer. Windows
checks disk-handle type before regular-file metadata; unsupported host families
refuse this file-inspection operation. This does not change stdin/stream inputs,
WASM mmap storage, resource limits or guest execution. Windows and actual native
format/large-artifact measurements remain explicit qualification coordinates.

## Appendix C — Closed-world release binary size

### Problem and acceptance

The dispatch records a 10.6 MB macOS arm64 release executable for the 25-line `bench_class_hierarchy.py`, including 7.3 MB of `__text` and 2.6 MB of `__const`. Its first-launch scan contributes to edit-to-run latency. The V1 ledger records a 1,616,065-byte LLVM wasm32 split application against a 1 MiB budget (HF-859) and a 3.28 MiB gzip -9 shared runtime against a 3.25 MiB budget (HF-869). Those are historical observations. They establish the workloads and failure thresholds. Fresh measurements must identify the objects and roots responsible for retained bytes before a change claims savings.

Success means that the release executable and the LLVM wasm32 application carry less code and data when their reachable program is smaller. Measure the fixed startup footprint with hello and the additional bytes associated with features independently exercised by the controls. A linked WASM runtime may follow the app's closure. A reusable split runtime carries the full generated ABI for its target and capability tier, so its budget is a separate obligation even when the app shrinks. Preserve the verified subset, runtime and host ABI, C-extension link contract, launch time, and executed instruction count. Use the existing budget tests unchanged. [V1 findings ledger](../../agent/V1_HANDOFF_FINDINGS.md)

### Existing authorities and prior art

- Native executable links already enable section collection through `native_link_policy`, with a narrow dynamic export surface on Mach-O and ELF. The separately linked runtime image has a distinct policy. LLVM wasm32 links already request `--gc-sections`, and split app links use `--no-export-dynamic`. Compare the selected commands and real relocations before assigning the size to an archive or adding flags. [Native link command](../../../src/molt/cli/native_link_command.py) · [WASM object linker](../../../tools/wasm_object_link.py)
- Molt's source function closure and final typed native closure already share `reachable_with_callees`. `AppCallableManifest` captures address-taking for dynamic builtins, intrinsic acquisition and GPU descriptor targets. The module registry emits init relocations. The generated entry enum owns external compiled entry names. LLVM emits these metadata references before optimization. Its optimizer currently gives external function definitions temporary `DLLExport` retention, which makes external visibility an optimization decision too. [Function closure](../../../runtime/molt-tir/src/passes/dead_functions.rs) · [Resolver manifest](../../../runtime/molt-tir/src/passes/app_callable_manifest.rs) · [LLVM optimizer](../../../runtime/molt-backend-native/src/llvm_backend/mod.rs)
- `reached_intrinsic_symbols` and generated availability gates predict runtime features from prepared IR. The emitted app import ABI decides the final WASM runtime requirements, including extension or lowering-introduced imports. The Cargo feature graph owns tier selection. The split export helper intentionally includes all generated runtime imports, runtime-owned dynamic intrinsics, host exports and CPython ABI exports and data, plus build-specific obligations. [Runtime feature contract](feature_reachability_tree_shaking.md) · [WASM export planner](../../../src/molt/_wasm_runtime_exports.py) · [Runtime WASM build spec](../../../src/molt/cli/runtime_wasm_build_spec.py)
- Go's linker follows real relocations from roots and uses additional compiler-marked edges for interface and reflective calls. The transferable idea is to represent edges the linker cannot infer explicitly. LLVM internal linkage provides a local visibility boundary. `llvm.used` adds a preservation root, and wasm-ld section collection starts at the entry and exported or otherwise retained symbols. Use these mechanisms with Molt's own typed identities and ABI. [Go linker dead-code pass](https://go.dev/src/cmd/link/internal/ld/deadcode.go) · [LLVM linkage and preservation](https://llvm.org/docs/LangRef.html#linkage-types) · [wasm-ld](https://lld.llvm.org/WebAssembly.html#garbage-collection)

The lane has no committed or stashed implementation of the reported internal-linkage chip in the checked refs. The live implementation already internalizes the WASM entry wrapper's renamed body. Use that convention as a local precedent. The native final-body ownership changes visible on `origin/main` should be taken through the normal lane update before the source slice. The checked versions leave the proposed LLVM clone decision at the same boundary.

### Architecture and implementation sequence

#### 1. Attribute the bytes on the existing build paths

Use one freshness-checked release build of each input per cell for structural measurement. Use `print("hello")`, `bench_class_hierarchy`, `bench_json_roundtrip`, `bench_etl_orders` and `bench_matrix_math` as the common workload set. The linked LLVM wasm32 and split modes run the same supported sources where their declared profile supports them. Also measure the existing HF-859 split application and its shared runtime. Use identical flags and inputs for baseline and candidate. Preserve the compiler, runtime, target, profile, link arguments, and artifact identity with the receipt. Label any witness the target rejects.

Capture three views from the existing boundaries:

1. Prepared IR and `reached_intrinsic_symbols` show the program's early runtime requirements. The callable manifest dump shows address-taken runtime and compiled entries. Inventory the lowering-introduced references separately from these prepared inputs. LLVM IR before and after optimization identifies emitted definitions, declarations, and relocation-producing data, including the resolver and module registry.
2. Native app and runtime archive symbols, the executable link map, and sampled `-why_live` queries identify the real linker roots for high-byte members. For LLVM wasm32 use the published app's imports and exports and map types with the generated ABI, plus the linker map or relocatable symbols for linked mode. Record launch glue, registry and callable-table references, direct app references, and the runtime's transitive references. An undefined reference in an unlinked app object can originate in a body the final linker removes. A linked runtime symbol without a direct app edge can be needed transitively. Record unattributed bytes as such.
3. `tools/binary_size_analysis.py` supplies native section and symbol attribution and WASM code metrics. Use `tools/wasm_size_audit.py` for the existing WASM section view and the split suite's content-addressed gzip -9 evidence for HF-869. Report native executable bytes and named segments/sections, WASM code/data/custom and total bytes, and the app and shared split members independently. Record the largest reachable runtime owners by evidence, including data and padding where the tools leave a residual.

Run the diagnostics in lane scratch. Use the existing diagnostic settings and inspection tools. An initial symbol join can be scratch work with `RuntimeFn`, `FunctionId`, the generated ABI, and the existing object readers as authorities. Artifact symbol strings are open to the toolchain and are joined at the file-format boundary. An absent name in a stripped binary carries no reachability claim. Where link maps or named symbols are unavailable, record section totals and the attribution gap.

For the macOS arm64 LLVM release baseline at `f92e7ba20b71`, class hierarchy
links to 10,659,032 bytes. The link map reports 7,312,440 bytes of `__text`,
including 6,501,332 from the selected micro runtime, 711,100 from the compiled
stdlib archive, and 64,260 from the app archive. The prepared IR has 99 reached
functions, seven initializer roots, and no optional link-feature requirement.
The app callable manifest has 109 entries. The export list contains `_main`.

The table sums disjoint atoms among the 154 large symbols matched by the
targeted Mach-O linker trace. Each amount counts one observed path per matched
atom. Shared destinations can have several incoming roots, so these amounts
give no estimate of bytes saved by deleting an individual edge.

| Sampled family | Attributed bytes | Example linker retention path, from root to atom |
| --- | ---: | --- |
| RustPython parser | 1,155,276 | `_main` → runtime exception reporting → `traceback_caret_anchor` → `parse_filtered_tokens` → `___parse__Top::__reduce` |
| Unicode name tables | 865,258 | app callable table → runtime import and iterator support → `glob_join_path` → `root_path_parse` → `encode_string_with_errors` → `unicode_name_escape` → `unicode_names2::Name::next` → 263,962-byte constant |
| Core GIL vtable | 697,876 | runtime initialization → `CORE_GIL_VT` → execution guard release → runtime teardown and CPython ABI hook paths |
| Generated name classifier | 91,676 | app callable table → `abs` → dynamic special-method lookup → `NameId::from_bits` → 78,268-byte `NameId::from_bytes` |

The groups leave most linked text outside the sampled trace. HF-902 owns the
CPython traceback migration, including deletion of the Rust renderer. The
filesystem decoder and encoder must retain Unicode semantics where called by
reached generic codecs. Pruning the parser's renderer dependency, or reducing
the C-API startup table according to the admitted application ABI, requires a
separate A/B at the producer.

The same baseline builds and runs the HF-859 Cloudflare split fixture, which
prints `42`. The side module is 1,219,099 bytes, including 1,097,410 bytes of
code. The shared member is 10,755,038 bytes and 3,517,242 bytes with `gzip -n -9`.
Its code and data sections are 7,611,676 and 2,948,853 bytes respectively. Both
budget findings remain open. The standalone linked module is 8,511,376 bytes.

The next split release link compresses relocated LEB operands and removes the
debug name section. On macOS arm64 the Cloudflare fixture dropped from 1,220,151
to 1,073,310 bytes in a same-object link replay. Both versions print `42` in Node
and normalize to the same module with Binaryen's read/write path and no code
passes. The app remains 24,734 bytes over its budget. The full Binaryen size
pipeline saved 228,148 bytes in a separate matched replay and increased retired
instructions by about 1% on `bench_class_hierarchy`, so it was not adopted.

#### 2. Prune the dominant runtime and app roots

The native decision is driven by the root attribution. Start with the existing Cargo-derived tier and archive/link plan. For a root that should be absent in a smaller program, remove the dependency at its producer, preserving demanded relocation paths. If an app-level resolver row demonstrably originates only in a dead publisher, fix the existing `AppCallableManifest` preparation and its partition consumers together. If fixed runtime support retains optional code via legitimate dynamic behavior, use the typed program fact at that support producer and retain the generic path in cells that can use it. A feature/profile change must keep publisher, fingerprint, installed-runtime and link-validation consumers aligned and justify its cold-build and artifact-cache costs. Plan for these decisions from the measured owners before choosing paths or writing a new record.

For linked WASM, use the app's emitted imports with the existing generated import-to-export and feature mapping. For a shared split runtime, preserve the full ABI and compare the retained HF-535 fixed generation with an HF-869 breaching generation if both artifacts can be recovered. Fix the largest attributable producers beneath those roots, then exercise consumers whose ABI bodies/data moved. If the retained artifacts are missing or incompatible, report the comparison as unavailable and bisect a reproducible source baseline if the current budget still fails. Do not select a per-app export subset for a shared member.

For remaining HF-859 app code, rank the live app bytes by function and shared lowering family. The ledger has already measured `default<Oz>` and cold code. If reached exception, literal or init operations dominate, use existing shared lowering and the owning typed facts to shrink that path. A reserved producer requires the integrator's resolution. Report independently whether the measured candidate closes HF-859, HF-869 and native size proportionality.

#### 3. Test object-local LLVM linkage if clone bytes survive

If the link map shows material retained clone bytes, an isolated candidate can address functions introduced by the owned module phase. Its `owned.clones` records provide the final row identities, before declaration and before the LLVM optimization pass. A specialization clone is created inside the object after partitioning and cannot be a target in a sibling object's input. Existing function addresses inside this LLVM module are real IR references. Use `FunctionId` to select each clone's LLVM definition and mark it `Internal` before optimization. Keep the decision at `native_backend/simple_backend/compile_driver.rs`, where it already walks the typed rows to declare their functions. Keep `lowering::declare_tir_function` as the single declaration authority, applied once to each row. Plain bodies and externs at the partition boundary retain their existing linkage.

The clone A/B must isolate this choice. Attribute the change in both LLVM output bytes and final release bytes. The physical linker may already collect some of the same bodies. If the candidate has no shipped size benefit and no material build benefit, drop it from the landing and use the attributed roots to select the next change. A synthetic dead clone establishes the mechanism, while the five workloads establish its user impact. Existing rooted function retention in `module_metadata.rs` remains the authority if a later measured slice needs a terminal TIR walk shared by Cranelift and LLVM.

### Alternatives considered and risks

| Option | Consequence |
| --- | --- |
| Enable more linker collection and change LLVM's optimization level globally | Existing native and WASM links already collect dead sections. `default<Oz>` barely moved HF-859 in the retained measurement. Keep link-policy changes for measured roots whose behavior varies by linker or profile. |
| Internalize every compiled body outside the launcher entry list | Cached stdlib members, batch objects and externally callable entries can bind across objects. A name-only list misses those edges. The first slice changes compiler-created clones with object-local provenance. Broader internalization needs an authoritative cross-object consumer inventory. |
| Build a custom runtime archive per app | This could prune roots, but adds another Cargo/tool identity for each reached set and can raise cold-build latency and cache storage. First use existing tier/feature plans and linker collection. Compare cold and unchanged rebuilds before proposing a finer archive identity. |
| Remove unused exports from the shared runtime | It would make separately built apps and native extensions dependent on which app generated the shared member. Keep the generated public ABI as the shared contract and shrink its implementations and transitive dependencies. |

The correctness risk is a dropped call, relocation or data definition that is exercised through a registry, indirect dispatch, extension ABI or sibling object. Negative size evidence alone cannot detect that loss. Require independent execution on those paths, unchanged approved public exports, and explicit section counts. A first-launch wall time comparison can be confounded by signing, linker variant or page-cache state. Use fresh distinct paths for first launch, interleave the measurements, record host and signing mode, and also measure steady execution instructions. A new proof record format or instrumentation is unnecessary for the conditional clone slice.

### Global invariants

1. A clone's locality is decided once from the final module pipeline's typed `FunctionId` rows. No symbol pattern, suffix rule or backend-independent copy classifies a clone. The compiler emits LLVM's `Internal` only for proven local definitions in this slice. LLVM still follows actual references inside the module.
2. Any root crossing an object or host boundary remains represented by its existing ABI entry, explicit public export, partition declaration, or real relocation. Direct links and resolver/registry metadata keep their current ABI owners. ABI catalogs and existing generator checks own their projections.
3. Source requirements, linker roots, and final bytes have distinct evidence. Each comparison records its selected source/tool/profile and artifact identity. A smaller archive or a lower root count is insufficient for a release-size claim.
4. The shared split runtime's public surface remains derived from the generated ABI and declared capability tier independent of app closure. The existing budget and ABI tests apply to the published bytes.

### Implementation skeleton

Conditional clone slice in `native_backend/simple_backend/compile_driver.rs`. This consumes the existing `TirOwnedModule` clone record and `declare_tir_function` return value inside `SimpleBackend::compile`. The helper expresses the visibility decision and can be inlined into the existing declaration loop if review prefers.

```rust
/// Apply visibility to the LLVM definition declared by the current final row.
/// Requires: `id` is the document's final FunctionId, `function` is that row's
/// owned LLVM definition, and `local_clones` is projected from `owned.clones`
/// after object partitioning. Call before LLVM optimization (invariants 1–2).
/// Ensures: identical declaration and ABI, with Internal linkage for a proven
/// clone. Raises: the existing typed-row and LLVM verification boundaries
/// reject inconsistent final rows.
fn apply_specialized_clone_linkage(
    id: molt_ir::FunctionId,
    function: inkwell::values::FunctionValue<'_>,
    local_clones: &molt_ir::function_table::FunctionSet,
) {
    todo!()
}
```

LLVM declarations cross the existing name boundary through `declare_tir_function`, with typed `FunctionId` used for locality. Extern declarations follow `declare_extern_tir_function`. Structural coverage should check that both a live local clone and an externally consumed original use the same definition path and retain their respective linkage. The existing typed producer, Rust type checking, and generated ABI checks cover the relevant authorities. The proposed code introduces no loose strings or dictionaries and uses the existing environment, transport and registry boundaries.

### Verification and landing

- Attribute final size before editing runtime or codegen. Cut the dominant producer edges only when the closed-world program or target proves them unnecessary. Include a real linker retention chain for each materially retained family. Where retained runtime code dominates, inspect at least one shortest evidenced root path for each materially changed family.
- Extend `llvm_generated_object_linkage.rs` for the local/live clone and externally used body case if the current generated-object cases do not exercise it. Use the existing object inspector and link/run the witness. Exercise whole app, batched app and shared-stdlib object paths in the declared LLVM cell. Run the affected native and LLVM differentials for direct calls, address-taken callable lookup, registry/import, generators/tasks and dynamic method dispatch. Use the existing WASM linked and split host tests for startup and imports, including the C ABI consumer when a runtime link boundary changes. Compare output and exit to the uv-managed CPython oracle in isolated mode. A missing profile, toolchain or host cell remains explicitly unverified.
- Run the five before/after size and launch measurements with the existing instructions authority and separate first-launch and steady execution runs. `tests/findings/test_wasm_app_size.py` and `tests/test_wasm_split_runtime.py` own the two split budgets. Use the existing source-tree fingerprints and feature-on Rust build, then run the changed crate lib/Clippy cells through the existing consumer gate and landing precheck. Reuse exact receipts. [Instructions methodology](../../../tools/PERF_AUTHORITY.md)
- Record each landed slice and its impact in the V1 ledger. Drop a candidate whose evidence fails the changed obligation or revert a landed regression through normal integration. Keep the lane's measurement outputs under its scratch root and record the receipt locations and durable numbers in the lane report.

### GoldfishContext

| Boundary | Review inputs and consumers |
| --- | --- |
| Function producer, typed roots and local LLVM linkage | `runtime/molt-backend-native/src/native_backend/simple_backend/{compile_driver,program_pipeline,module_metadata}.rs`, `runtime/molt-backend-native/src/llvm_backend/{mod,lowering,app_resolver,module_registry,program_slots,wasm_entry}.rs`, `runtime/molt-tir/src/tir/pipeline_cache.rs`, `runtime/molt-passes/src/tir/module_phase.rs` (reserved, read-only), `runtime/molt-ir/src/function_table.rs`, `runtime/molt-tir/src/passes/{dead_functions,runtime_roots,app_callable_manifest}.rs`. Clone changes are limited to `compile_driver.rs` and the existing linkage test if needed. |
| Cross-object producers, launchers and ABI | `runtime/molt-backend/src/backend_process/native_batch/{application/single,application/batched/jobs,worker/compile}.rs`, `runtime/molt-backend/src/backend_process/shared_stdlib_cache/{prepare/mod,compile/batched}.rs`, `src/molt/cli/native_main_stub.py`, `src/molt/cli/native_link_command.py`, `tools/wasm_object_link.py`, `runtime/runtime_abi.toml`, `runtime/molt-ir/src/runtime_callable_abi_generated.rs`, `src/molt/_wasm_abi_generated.py`. No shape change for the native worker job, partition archive or launch ABI in the clone slice. |
| Requirements, existing diagnostics and binary readers | `src/molt/cli/{backend_ir,required_features,function_references,runtime_features,native_symbol_inspection,backend_diagnostics}.py`, `runtime/molt-backend-native/src/app_resolver_abi.rs`, `src/molt/environment_registry.toml`, `src/molt/backend_environment.json`, `runtime/molt-runtime/src/intrinsics/{manifest.pyi,categories.toml}`, `src/molt/_runtime_feature_gates.py`, `tools/{gen_intrinsics,gen_wasm_abi,binary_size_analysis,wasm_size_audit}.py`, `src/molt/wasm_artifact.py`, `tests/tools/test_binary_size_analysis.py`. Existing dump settings feed inspection only. |
| Runtime selection and product consumers for a later measured slice | `src/molt/_wasm_runtime_exports.py`, `src/molt/cli/{runtime_wasm_build_spec,runtime_wasm_build_support,runtime_wasm_validation,runtime_wasm_generation,runtime_fingerprints,runtime_build_identity,runtime_identity_schema,runtime_cargo_plan,installed_runtime,link_fingerprints,cache_fingerprints}.py`, `runtime/molt-runtime/Cargo.toml`, `tools/release/runtime_cells.py`, `runtime/molt-runtime/tests/wasm_cdylib_exports.rs`. Reconcile their capture, publication and validation lists if the measured runtime slice changes an input or an ABI row. |
| Selection and independent witnesses | LLVM existing `runtime/molt-backend/tests/llvm_generated_object_linkage.rs`, native control `runtime/molt-backend/tests/native_extern_linkage.rs`, LLVM sibling `runtime/molt-backend-native/src/native_backend/simple_backend/tests/llvm_backend.rs`, callable manifest fixtures `runtime/molt-tir/src/passes/tests/manifest.rs`, `tests/molt_diff.py`, `tests/findings/test_wasm_app_size.py`, `tests/test_wasm_split_runtime.py`, `tests/wasm_linked_runner.py`, `tools/proof_plan.toml`, `.github/proof-plan.generated.json`, `tools/{consumer_gate,check_changed_crate_tests,molt_dev,perf_instructions}.py`. Existing LLVM proof inputs cover the source and witness, so no new proof-plan or fingerprint role is needed for the clone slice. |
| Durable design and status | `docs/design/foundation/{60_tree_shaking_whole_program_dce,61_binary_size_and_output_optimization,feature_reachability_tree_shaking}.md` own the contract and mechanism. `docs/agent/V1_HANDOFF_FINDINGS.md` owns the measured release status. This review artifact proposes code and proof only. |
