<!-- Foundation blueprint 21. Architect: agent-arch, 2026-06-05. Arc: codebase
decomposition program — kill the god-file problem, make concurrent dev + incremental
builds fast. Companion to 08_DX-buildspeed.md (build-speed sub-arc); this doc is the
superset program (crate graph + frontend Python package + runtime satellite dedup +
concurrency/ownership model). EVERY factual claim below is verified against the tree at
base 9e93503bb; verification commands are inlined. This doc is a PLAN ONLY — no code was
refactored in the session that produced it. -->

# 21 — Codebase Decomposition Program

**God-file inventory · crate graph · phased move-only extraction plan**

Status: **IN EXECUTION.** T1 (`molt-tir` crate) LANDED (`cd8a62a30`); M1
(`function_compiler` function-split) in progress. Base commit: `9e93503bb`.
Companion: `08_DX-buildspeed.md` (the build-speed sub-arc, partially landed — see §0.3).

> **CORRECTIONS — read these for the authoritative plan.** This doc's original move #1
> and crate granularity were written before the dependency DAG was measured and are
> SUPERSEDED:
> - **Move #1** → [`21a_function_compiler_function_split_PLAN.md`](21a_function_compiler_function_split_PLAN.md):
>   the *function*-split of `compile_func_inner`, not a file-split (a file-split buys ~0
>   build win — a function is rustc's atomic codegen unit; see `dx_baseline.md` §8).
> - **Crate graph** → [`21b_crate_graph_blueprint.md`](21b_crate_graph_blueprint.md):
>   split `molt-tir` into `molt-ir` ← `molt-passes` ← `molt-lower`; one crate *per*
>   backend (native→llvm is a real one-way edge — NOT grouped); add `molt-codegen-abi`
>   for the shared NaN-box ABI. 21b §"Flags" lists every granularity correction to this
>   doc and to 08.
> - **Crate graph execution** → [`21f_crate_graph_smove_execution_specs.md`](21f_crate_graph_smove_execution_specs.md):
>   the live-state, per-S-move execution spec for 21b's S1-S8 crate moves, including
>   file partitions, feature wiring, visibility widening, and per-commit gates.
> - **Frontend move #2** → [`21c_frontend_mixin_decomposition_PLAN.md`](21c_frontend_mixin_decomposition_PLAN.md).
Methodology precedent: `34e3bddbf` (the `lib.rs` god-file split: 6,928→264 lines,
move-only, 0-warning build + byte-identical diagnostics + lib tests + symbol identity +
e2e). This program continues that arc.

---

## 0. Executive summary

molt has three distinct decomposition problems, each with a different correct fix:

1. **Rust backend monolith** (`molt-backend`, 185,928 lines, ONE crate, all 5 backends +
   all TIR/SimpleIR passes in one compilation unit). Fix = **crate extraction** (build-cache
   win). The `tir/` subtree has ZERO back-edges to any backend (verified), so the layering
   is already clean — the crate boundary just isn't drawn yet.
2. **Frontend Python mega-class** (`src/molt/frontend/__init__.py`, 44,620 lines, of which
   ~43,260 are a *single* `SimpleTIRGenerator(ast.NodeVisitor)` class, lines 1343→44,603,
   with 261 `visit_`/emit methods). Fix = **Python package decomposition into visitor
   mixins** (edit-locality / reviewability / parallel-ownership win — NOT compile time;
   Python has no compile step).
3. **Runtime satellite duplication + DRIFT** (`molt-runtime`, 346,220 lines). The satellite
   pattern (`molt-runtime-http` etc.) *works for build caching* but was applied as a
   `#[cfg(not(feature))]` dual-path that left the in-tree copy physically duplicated — e.g.
   `functions_http.rs` exists in two crates. **CORRECTION (see §1.4): the two copies are NOT
   content-identical — all 28 pairs bidirectionally DRIFTED, and the in-tree copy is the live
   source for the reduced tiers, not a dead fallback.** Fix = a three-phase arc: (R.1, LANDED)
   a fail-closed parity guard + per-pair drift reconciliation; (R.2) unify the access layer
   (direct-call vs FFI-bridge) so one source compiles in both contexts; (R.3) only then make
   the satellite the single source of truth and delete the in-tree copy.

The unifying lesson, stated plainly: **a module split buys edit-locality and review
ergonomics; only a crate split buys build-cache isolation.** Do not conflate them. The
frontend wants module/package splits (Python). The backend wants crate splits (Rust build
cache). The runtime wants the *completion* of crate splits it half-did.

### 0.1 The five highest-leverage first moves (ranked — full detail in §6)

| # | Move | Friction relief | Build win | Risk | Score |
|---|------|-----------------|-----------|------|-------|
| 1 | Split `function_compiler.rs` (39,043 LOC) into opcode-family submodules, within-crate, move-only | Highest churn god-file; #1 ownership-collision source after frontend | Recompile blast radius 39K→~4-6K per family | Low (module split, `use super::*` preserved) | **A+** |
| 2 | Decompose `frontend/__init__.py` `SimpleTIRGenerator` into a `frontend/` package of visitor mixins | #1 ownership-collision source (3 contention events this window) | None (Python) — but unblocks parallel agents | Medium (mixin MRO, no static typing of `self`) | **A** |
| 3 | Extract `molt-tir` crate (tir/ + ir/ + Repr), the clean lower layer | TIR-pass authors stop recompiling all 5 backends | Editing a TIR pass no longer rebuilds Cranelift/WASM/LLVM codegen | Medium (pub-surface contract, the `Repr` cycle cut) | **A** |
| 4 | Finish the runtime satellite arc: parity guard + reconcile drift (R.1, LANDED), then access-layer unification (R.2) + dedup (R.3). NOT a naive "delete the 28 copies" — they drifted, serve disjoint tiers, and are two access models (§1.4/§2.4) | Stops new drift now (guard); eliminates dual-maintenance after R.3 | Removes duplicate CUs at R.3 | R.1 Low; R.2/R.3 Med (access-layer unification, feature-unification audit) | **A−** |
| 5 | Extract `molt-backend-native` (native_backend/ + llvm_backend/) onto `molt-tir` | Cranelift/LLVM authors isolated from WASM/Luau/TIR authors | Editing codegen ⟂ editing passes; parallel codegen | Medium (the `use super::*` glob → explicit `use molt_tir::*`) | **B+** |

### 0.2 What contradicts the supervisor's stated assumptions

- **Churn data is low-signal at this base.** The repo has exactly **50 commits total** (verified:
  `git log --oneline | wc -l` = 50 in both the worktree and main tree — history was recently
  reset/squashed). 14-day, 30-day, and 60-day windows return identical results. Frequency-count
  churn is therefore not statistically meaningful here. I rank god-files by the *documented*
  contention (MEMORY.md: frontend contention "happened repeatedly this week") + size + the
  structural fact that the largest files are the ones every agent must touch. The churn *table*
  in §1 uses lines-changed over the 50-commit window as the best available proxy and flags the
  limitation. **The frontend/function_compiler hypotheses are confirmed by structure, not by a
  rich churn signal that does not exist in this tree.**
- **The DX doc (08) is partially STALE but its core claims hold.** It cites `release-fast` `lto =
  "fat"` at `Cargo.toml:295` — VERIFIED still correct (line 295 *is* `lto = "fat"` inside
  `[profile.release-fast]`). But its Phase-1 `release-output` profile **already landed**
  (`Cargo.toml:366`). Its `function_compiler.rs` count (38,510) drifted to 39,043. Treat 08 as
  a true-but-aging companion: its Phase 1 (config) is mostly done, its Phase 2 (fc split) and
  Phase 3 (native extraction) are the same moves this program sequences (moves #1 and #5).
- **`functions_http` is duplicated, and it is systemic, not a one-off.** There are **28**
  `cfg(not(feature = "stdlib_*"))` dual-path gates in `builtins/mod.rs` (verified). The
  satellite pattern's *failure mode* is that the in-tree copy was never deleted.

### 0.3 Relationship to in-flight work (do not redo)

- **DX agent lane** (doc 08): owns Cargo profiles, sccache default-on, the `function_compiler.rs`
  split (move #1), and possibly `molt-backend-native` extraction (move #5). **This program adopts
  08's Phase-1 config wins as already-in-progress and sequences moves #1/#5 as shared deliverables
  — whichever lane lands them first, the other consumes.** Where 08 and this doc both specify the
  fc split, **08's submodule boundary list is authoritative**; this doc's move #1 defers to it and
  only adds the gate checklist + line budgets if 08's are absent.
- **Partner LLVM lane**: actively edits `llvm_backend/lowering.rs`. The `molt-backend-native`
  extraction (move #5) bundles `llvm_backend/` — **sequence move #5 AFTER the LLVM partner's
  current arc lands**, or coordinate a freeze window. Do not extract a crate out from under an
  active editor.
- **Baseline build numbers**: 08 will produce measured cold/incremental timings. This doc leaves
  **keyed placeholders `{DX-BASELINE:<key>}`** for per-phase build-win estimates; fill them once
  08's measurements exist. Do not invent numbers.

---

## 1. Evidence: inventory + churn + dependency findings

### 1.1 God-file inventory (verified `wc -l`, base 9e93503bb)

**Python (`src/molt/`):**

| File | Lines | Shape | Decomposition kind |
|------|-------|-------|--------------------|
| `frontend/__init__.py` | 44,620 | ONE class `SimpleTIRGenerator` spans L1343→44,603 (261 `visit_`/emit methods); only 29 top-level defs/classes total, all pre-L1343 are small dataclasses | **package → visitor mixins** |
| `cli.py` | 39,238 | 896 top-level defs/classes — flat kitchen-sink of subcommand handlers + helpers | **package → per-subcommand modules** |
| `frontend/cfg_analysis.py` | 416 | leaf helper | already fine |
| `capability_manifest.py` | 1,217 | cohesive | fine |

Sibling frontend files are tiny (`cfg_analysis.py` 416, `tv_hooks.py` 260) — the package is
*de facto* one file.

**Rust backend (`runtime/molt-backend/src/`, 185,928 LOC, ONE crate):**

| File / subtree | Lines | Crate-cut target |
|----------------|-------|-------------------|
| `native_backend/function_compiler.rs` | 39,043 | `molt-backend-native` (split into families first, move #1) |
| `runtime/molt-backend-wasm/src/wasm.rs` | 4,418 | extracted WASM facade in `molt-backend-wasm` |
| `luau.rs` (+luau_ir 1,038 +luau_lower) | 12,278 (14,272 incl. ir/lower) | `molt-backend-luau` |
| `llvm_backend/lowering.rs` | 10,656 | `molt-backend-native` (bundled w/ Cranelift) |
| `tir/lower_to_simple.rs` | 7,274 | `molt-tir` |
| `native_backend/simple_backend.rs` | 6,268 | `molt-backend-native` |
| `passes.rs` (SimpleIR passes) | 5,837 | `molt-tir` (or `molt-backend` core) |
| `rust.rs` (transpiler) | 4,854 | `molt-backend-rust` |
| `representation_plan.rs` + `repr.rs` | 4,631 | split: repr vocabulary -> `molt-tir`, plan logic -> backend/lower core |
| **subtree: `tir/`** | **72,041** | **`molt-tir`** (clean lower layer) |
| **subtree: `native_backend/`** | **45,429** | **`molt-backend-native`** |
| **subtree: `llvm_backend/`** | **12,821** | **`molt-backend-native`** |
| **subtree: `runtime/molt-backend-wasm/src/`** | **extracted** | **WASM codegen, ABI manifest/generated registry, import planning, binary patching** |

**Rust runtime (`runtime/molt-runtime/src/`, 346,220 LOC):**

| File | Lines | Note |
|------|-------|------|
| `intrinsics/generated.rs` | 24,502 | `@generated by tools/gen_intrinsics.py` — GENERATED, `pub(crate)` |
| `object/ops.rs` | 11,863 | already in a split `object/` dir (9 files, 67,634 total) |
| `builtins/gpu.rs` | 11,816 | `molt-gpu` candidate |
| `builtins/platform_importlib_ffi.rs` | 7,658 | |
| `builtins/platform.rs` | 7,173 | |
| `builtins/functions_http.rs` | 7,144 | **DUPLICATE of `molt-runtime-http/src/functions_http.rs` (7,338)** |
| `builtins/exceptions.rs` | 7,114 | |
| `builtins/io.rs` | 6,848 | |
| `builtins/types.rs` | 6,472 | |

**Workspace already has ~37 crates** (verified `find runtime -maxdepth 2 -name Cargo.toml`):
core (`molt-runtime`, `molt-backend`, `molt-runtime-core`) + 18 runtime satellites
(`-http`, `-net`, `-asyncio`, `-math`, `-path`, `-collections`, `-regex`, `-text`,
`-itertools`, `-serial`, `-difflib`, `-logging`, `-crypto`, `-compression`, `-stringprep`,
`-xml`, `-ipaddress`, `-zoneinfo`) + capability crates (`molt-gpu`, `molt-ffi`, `molt-db`,
`molt-embed`, `molt-python`, `molt-snapshot`, `molt-tier`, `molt-harness`, `molt-worker`,
`molt-wasm-host`, `molt-obj-model`, `molt-cpython-abi`, `molt-backend-mlir`). **The satellite
pattern is proven; the backend is the conspicuous monolith.**

### 1.2 Churn ranking (50-commit window — best-available proxy, see §0.2 caveat)

By total lines changed over all 50 commits (`git log --numstat`):

| Lines Δ | Touches | File |
|---------|---------|------|
| 801 | 1 | `tir/passes/drop_insertion.rs` (recent RC sprint) |
| 710 | 1 | `docs/design/foundation/00_integrated_parallel_program.md` |
| **622** | **3** | **`src/molt/frontend/__init__.py`** ← only file touched 3× |
| 617 | 1 | `tir/passes/liveness.rs` |
| 246 | 1 | `wasm.rs` |
| 156 | 2 | `tir/lower_to_simple.rs` |
| 56 | 2 | `representation_plan.rs` |

`frontend/__init__.py` is the only file with >2 touches in the window — consistent with the
documented "frontend contention happened repeatedly this week." The window is too short for a
rich frequency signal; **structure + documented contention are the authoritative ranking
inputs**, and both point at `frontend/__init__.py` (#1) and `function_compiler.rs` (#2, the
largest single file, which every backend-correctness change must touch).

### 1.3 Dependency reality (the crate-cut feasibility findings)

All verified by `grep -rE 'crate::<mod>' <subtree>`:

- **`tir/` → backends: ZERO edges.** `grep -rcE 'crate::(wasm|luau|rust|llvm_backend|native_backend)' tir/`
  returns nothing. **`molt-tir` can be extracted with no circular dependency.** This is the
  single most important finding: the layering is already correct; only the crate boundary is
  missing.
- **`passes.rs` → backends: ZERO edges.** SimpleIR passes are backend-agnostic.
- **`tir/` → `ir`: 31 edges** (TIR consumes the SimpleIR transport type). `ir.rs` → only
  `json_boundary`, `ir_schema`. So `ir` is a leaf that `molt-tir` depends on → `ir` joins
  `molt-tir`.
- **The `repr` vocabulary cut (surgical):** `repr.rs` owns the carrier lattice and lane
  vocabulary (`Repr`, `ScalarKind`, `ContainerKind`, and container-storage facts). TIR analyses,
  lowerers, and backend crates import that vocabulary through `crate::repr`; the richer
  `representation_plan` *logic* (LlvmReprFacts, ScalarRepresentationPlan, value_range_for) consumes
  it instead of owning it. This keeps the physical-carrier facts backend-neutral and leaves the
  planner as planner logic only.
- **Backend → shared deps (verified `grep crate::`):**
  - `function_compiler.rs`: `debug_artifacts`, `passes::ReturnAliasSummary`, `representation_plan`,
    `switch_to_block_tracking`, `block_has_terminator`, `unbox_int` (NaN-box helpers in `lib.rs`).
  - `wasm.rs`: heavy `tir::*` (lower_to_wasm, lower_to_simple, type_refine, serialize, cache,
    target_info), `passes::*`, `wasm_imports`, `representation_plan`.
  - `llvm_backend/lowering.rs`: `tir::ops/values/types/function/blocks`, `repr::{Repr, ContainerKind}`,
    `representation_plan::LlvmReprFacts`, `pending_bits`/`stable_ic_site_id` (lib.rs NaN-box).
  - `luau.rs`, `rust.rs`: only `representation_plan` (minimal coupling — easiest to extract).
- **`native_backend/` privacy mechanism:** uses `use super::*` glob (module-ancestry privacy;
  verified `native_backend/mod.rs:1`, `simple_backend.rs` 6 `super::` refs). The `lib.rs` split
  (34e3bddbf) preserved this by widening private→`pub(crate)` and moving shared Cranelift imports
  into `native_backend/mod.rs`. **A crate split must replace `use super::*` with explicit
  `use molt_tir::{...}` / `use molt_backend_core::{...}`** — this is the main mechanical cost of
  move #5 (NOT a blocker, but the reason move #5 is riskier than the within-crate move #1).
- **`molt_backend` public API surface** (what `main.rs` and `wasm.rs` consume): `tir::*`,
  `eliminate_dead_*`, `inject_runtime_exit`, `compute_intrinsic_manifest_checked`,
  `fold_constants`, the backend entrypoints (`wasm`, `rust`, `luau`, `llvm_backend`,
  `SimpleBackend`). These re-exports in `lib.rs:44-65` define the contract a thin orchestrator
  must preserve.

### 1.4 The runtime satellite finding (drift hazard — CORRECTED 2026-06-05)

> **CORRECTION.** An earlier draft of this section claimed the 28 in-tree
> `cfg(not(feature))` copies are "content-identical" to their satellites
> (`functions_http` sorted-diff = 0). **That claim was false.** Verification at a
> later base (`d48ac22df`+) found **every one of the 28 pairs had bidirectionally
> drifted** — `functions_http`'s raw sorted-diff was 820 lines, not 0. The drift
> is the silent-miscompile bug-class this program targets, already materialized:
> a behavioral fix landed in only ONE copy makes **shipped behavior differ by
> build tier** *today* (see below). The full inventory + access-model analysis is
> in `memory/recovery/baton_move_R_satellite_drift.md`; the reconciliation +
> fail-closed guard landed under Move R (this section now reflects that reality).

Three load-bearing facts (all verified):

1. **The in-tree copies are NOT dead fallbacks.** They are the SOLE compiled
   source for the reduced build tiers: `--stdlib-profile micro`, `stdlib_edge`,
   and the WASM feature set all build with `--no-default-features` and leave most
   satellites OFF (`src/molt/cli.py:25340-25368`), so the in-tree copy is what
   compiles there. The default native build (`stdlib_full`) pulls the satellites.
   **Both copies are live, in disjoint configs.** Deleting the in-tree copy would
   break micro/edge/WASM unless the satellites are forced into those tiers — which
   defeats the binary-size lever those tiers exist for.
2. **The two copies are NOT one logic in two namespaces.** The in-tree copy calls
   molt-runtime internals DIRECTLY (`use crate::{...}`, the `PyToken` GIL token,
   `crate::with_gil_entry_nopanic!`). The satellite reaches the same internals
   through an `extern "C"` FFI BRIDGE (`use crate::bridge::*` +
   `molt_runtime_core::prelude::*`, the `CoreGilToken` token, `with_core_gil!`;
   the serial satellite dispatches through a single `RuntimeVtable`). A
   `#[path]`/symlink include cannot unify them — the access layer must be unified
   first.
3. **Most residual drift, once the access layer is normalized away, is
   access-layer shape — but not all of it.** Move R's reconciliation normalizes
   the by-design access differences (imports, doc comments, the GIL macro/token,
   bridge path prefixes, single-line `unsafe {}` wrappers, trailing comments) and
   compares the residual line-multiset. After normalization, 12 of the 28 pairs
   have ZERO residual (provably the same behavior in two access models). The
   rest carry genuine semantic divergence ranging from 2 lines (decimal — a lone
   lifetime annotation; the earlier "2439-line, architecturally different" figure
   compared the satellite against the 13-line in-tree dispatcher stub instead of
   the real `decimal_without_mpdec.rs` implementation) up to ~570 lines
   (itertools, whose in-tree copy adopted RuntimeState-scoped slots that the
   satellite has not). **At least one residual was a real shipped bug**: the
   satellite `csv` copy hardcoded a list/dict/set unhashable-key check that
   missed `bytearray` (and every other unhashable type) and was not
   version-gated, so `csv.get_dialect(bytearray(...))` returned the wrong result
   on the default native build while the in-tree copy was already correct. Move R
   ported the in-tree `ensure_hashable`/`HashContext::DictKey` path into the
   satellite (via a new `RuntimeVtable::ensure_hashable` entry) with a
   two-tier differential regression.

There is **no sync script** anywhere under `tools/` — which is exactly why the
drift accumulated silently. Move R replaces the absent sync with a CONTRACT:
`tools/check_satellite_parity.py` (run by the
`runtime/molt-runtime/tests/satellite_parity.rs` integration test) is a
fail-closed, ratcheting parity guard that makes any NEW drift a test failure.
**The eventual fix remains to make the satellite the single source of truth and
delete the in-tree fallback — but only AFTER the access layer is unified (the
real Phase 2) and the per-pair drift is reconciled; see §3 Phase R.**

---

## 2. Target architecture

### 2.1 Backend crate graph (the build-cache win)

```
                          ┌─────────────────────────────────────┐
                          │  molt-tir   (lower layer, no backends)│
                          │  = tir/ + ir/ + ir_schema + json_     │
                          │    boundary + passes.rs (SimpleIR     │
                          │    passes) + repr.rs + ops/values/    │
                          │    types/function/blocks/cfg/dom/     │
                          │    pass_manager/analysis/* + all       │
                          │    tir/passes/* optimizer passes       │
                          │  ~78K LOC. ZERO backend back-edges.    │
                          └───────────────┬──────────────────────┘
                                          │ (depended on by everything below)
        ┌──────────────┬──────────────────┼───────────────────┬───────────────┐
        ▼              ▼                  ▼                   ▼               ▼
 molt-backend-    molt-backend-     molt-backend-       molt-backend-   molt-backend-
   native            wasm              luau                rust            (none more)
 (Cranelift +     (wasm.rs +        (luau.rs +          (rust.rs          [feature crates
  LLVM):           wasm_imports +    luau_ir +           transpiler)       above are leaves]
  native_backend/  lower_to_wasm)    luau_lower)         ~5K LOC
  + llvm_backend/  ~21K LOC          ~14K LOC
  ~58K LOC
        └──────────────┴──────────────────┴───────────────────┴───────────────┘
                                          │ (all re-exported through)
                          ┌───────────────▼──────────────────────┐
                          │  molt-backend  (thin orchestrator)     │
                          │  = lib.rs facade + representation_plan  │
                          │    logic (non-Repr) + ir_rewrites +     │
                          │    intrinsic_symbols + debug_artifacts +│
                          │    NaN-box consts/helpers + main.rs bin │
                          │  Re-exports the public API (§1.3). The  │
                          │  ONLY crate that knows all backends.    │
                          └────────────────────────────────────────┘
```

**Feature-flag wiring** (mirrors the existing flags exactly — verified `molt-backend/Cargo.toml:40`):
the current per-backend features (`native-backend`, `llvm`, `wasm-backend`, `luau-backend`,
`rust-backend`) become **dependency activations** on the orchestrator:

```toml
# molt-backend/Cargo.toml (orchestrator, after extraction)
[features]
default       = ["native-backend"]
native-backend = ["dep:molt-backend-native"]
llvm          = ["molt-backend-native/llvm"]   # llvm lives inside -native
wasm-backend  = ["dep:molt-backend-wasm"]
luau-backend  = ["dep:molt-backend-luau"]
rust-backend  = ["dep:molt-backend-rust"]
egraphs       = ["molt-tir/egraphs"]
```

Each backend crate depends only on `molt-tir`. **Build-cache effect:** editing `wasm.rs` rebuilds
`molt-backend-wasm` + `molt-backend` (relink) only — NOT `molt-tir`, NOT native, NOT llvm, NOT luau.
Editing a TIR pass rebuilds `molt-tir` + all dependents (unavoidable — it IS the shared layer), but
the dependents compile **in parallel** instead of as one 185K-line unit.

**Generated-intrinsics crate decision:** `generated.rs` (24,502 lines) is GENERATED and `pub(crate)`
to `molt-runtime`. Per the supervisor's question — **yes, generated code should live in a cache-stable
position.** Two options:
- (A) **`molt-runtime-intrinsics-gen` crate** holding `generated.rs` + the `gen_intrinsics.py`
  output. Cache-stable unless `gen_intrinsics.py` re-runs. Recommended IF the `IntrinsicSpec` blocks
  reference only stable types (verify the `pub(crate)` items it touches can be made `pub`).
- (B) **Split `generated.rs` into per-feature-gate sub-files** (`generated_core.rs`,
  `generated_http.rs`, …) within `molt-runtime`, per doc 08's intrinsic-registry split. This is the
  *lower-risk* first step (no new crate, no privacy widening) and is the recommended **Phase G**.
  The full crate extraction (A) is a follow-on once the per-file split proves the boundary.

### 2.2 Frontend Python package layout (the edit-locality win — NO compile-time effect)

`SimpleTIRGenerator` is ONE class (L1343→44,603). The win is **splitting it into visitor mixins**
so that agents own disjoint files. Python mixins compose via MRO: `class SimpleTIRGenerator(
ExprVisitorMixin, StmtVisitorMixin, FunctionLoweringMixin, ClassLoweringMixin,
ComprehensionMixin, PatternMatchMixin, AsyncGenMixin, EmitMixin, AnalysisMixin, ast.NodeVisitor)`.

Domain grouping is derived from the verified method-prefix census
(`_emit_` ×178, `_collect_` ×42, `_match_` ×20, predicates `_is_/_can_/_should_`, `_static_/_midend_`):

```
src/molt/frontend/
  __init__.py            # PUBLIC API ONLY: re-exports SimpleTIRGenerator + compile_to_tir
                         #   (L44604) + the small dataclasses (MoltValue, MoltOp, SCCPResult,
                         #   LoopBoundFact, ClassInfo, FuncInfo, …). Stays the import anchor
                         #   for cli.py / debug/ir.py / tv_hooks.py (verified importers).
                         #   Target: <800 lines.
  generator.py           # class SimpleTIRGenerator(<mixins>, ast.NodeVisitor): the assembly
                         #   point + shared __init__/state + dispatch. <1500 lines.
  visitors/
    expressions.py       # ExprVisitorMixin: visit_Name/Call/Attribute/Subscript/BinOp/
                         #   UnaryOp/Compare/BoolOp/Slice/Tuple/Set/Starred/NamedExpr
    statements.py        # StmtVisitorMixin: visit_Assign/AnnAssign/AugAssign/Delete/If/
                         #   While/For/With/Try/TryStar/Import/ImportFrom/Global/Nonlocal
    functions.py         # FunctionLoweringMixin: visit_FunctionDef/AsyncFunctionDef/Lambda/
                         #   Return/Yield/YieldFrom/Await + closure/trampoline emit
    classes.py           # ClassLoweringMixin: visit_ClassDef + MRO/metaclass/descriptor emit
    comprehensions.py    # ComprehensionMixin: ListComp/SetComp/DictComp/GeneratorExp
    pattern_match.py     # PatternMatchMixin: visit_Match + match_* helpers (×20 _match_)
    async_gen.py         # AsyncGenMixin: AsyncFor/AsyncWith + generator/coroutine state machine
  lowering/
    emit.py              # EmitMixin: the 178 _emit_* primitives (op construction)
    analysis.py          # AnalysisMixin: _collect_* (×42), _static_*, _midend_* (SCCP,
                         #   binding collection, loop-bound facts, tier classification)
    intrinsics.py        # _intrinsic_* arity/symbol caches + IntrinsicHandle* specs
  cfg_analysis.py        # (exists, 416 LOC — unchanged)
  tv_hooks.py            # (exists, 260 LOC — unchanged)
```

**Import-cycle strategy** (verified safe): the frontend's only inbound molt imports are
`molt.compat`, `molt.frontend.cfg_analysis`, `molt.type_facts` (all small leaves) — **no cycle
risk**. The mixins import shared dataclasses FROM `__init__.py`? **No** — that would create
`__init__ → mixin → __init__` cycles. Instead: move the small dataclasses into
`frontend/_types.py` (a true leaf), and have BOTH `__init__.py` and the mixins import from
`_types`. `__init__.py` re-exports them for backward-compat (`from molt.frontend._types import
MoltValue, MoltOp, ...`). External importers (`cli.py` does `from molt.frontend import ...`) see
no change.

**Mixin hazard (risk register §4):** Python mixins lose static `self`-type checking; a method in
`expressions.py` calling `self._emit_load(...)` (defined in `emit.py`) is unverifiable until
runtime. Mitigation: define a `_GeneratorProtocol(Protocol)` in `_types.py` enumerating the
cross-mixin method surface, and annotate each mixin `class ExprVisitorMixin: ...` with a
`if TYPE_CHECKING: self: _GeneratorProtocol` shim, so `mypy`/`pyright` (if run) catches cross-mixin
calls. This is the structurally-correct way to retain the type safety the single-class form had
implicitly.

### 2.3 cli.py package layout (the same edit-locality win)

`cli.py` is 896 flat top-level defs — a kitchen-sink, NOT one class. Split by subcommand domain:

```
src/molt/cli/
  __init__.py            # arg parser assembly + dispatch + main(). Re-exports the public
                         #   entrypoints anything imports today. <1000 lines.
  build.py               # _run_wrapper_build, _wrapper_build_cache_*, build subcommand
  run.py, test.py        # run/test subcommand handlers
  clean.py               # molt clean (process guard, artifact deletion)
  pgo.py                 # pgo_collect / pgo feedback handlers
  diff.py                # molt_diff harness wiring
  daemon.py              # backend daemon lifecycle (_BackendDaemonCompileResult etc.)
  _shared.py             # _emit_json/_fail/_run_command/_base_env/TargetPythonVersion (leaf)
```

cli.py is **lower priority** than the frontend (it's flat, so collisions are rarer — agents edit
different functions), but it is the #2 Python god-file and should follow the frontend package
precedent once that lands.

### 2.4 Runtime decomposition (continue + COMPLETE the satellite pattern)

- **Complete the 28 dual-path satellites** — but NOT by a naive "verify identical,
  delete the in-tree copy" move. As §1.4 corrects, the copies have **drifted**,
  they live in **disjoint live tiers** (in-tree = micro/edge/WASM; satellite =
  default native), and they are two **access models** (direct `crate::` calls vs
  an `extern "C"` FFI bridge / `RuntimeVtable`), not one logic in two namespaces.
  The structurally-correct arc is three phases, in order:
  - **Phase R.1 (LANDED, Move R) — fail-closed parity GUARD + drift
    reconciliation.** `tools/check_satellite_parity.py` +
    `runtime/molt-runtime/tests/satellite_parity.rs` normalize the access layer
    and ratchet the per-pair residual toward zero, failing CI on any new drift; a
    committed `tools/satellite_parity_baseline.json` allowlists the
    not-yet-reconciled residual (one-way ratchet: it may only shrink). This stops
    NEW drift immediately and is independently valuable. Reconciliation ports
    one-sided behavioral fixes into BOTH copies with two-tier differential
    regressions (the csv `ensure_hashable` fix is the first).
  - **Phase R.2 (the real "phase 2", PENDING) — bridge-facade access-layer
    unification.** Make ONE source file compile in BOTH the direct-call (in-tree)
    and FFI-bridge (satellite) contexts: a shared `bridge` facade +
    `molt_runtime_core::prelude` that resolves to direct `crate::` re-exports
    inside molt-runtime and to the `extern "C"`/`RuntimeVtable` bridge as a
    standalone crate, with a unified GIL token/macro. Only then is a `#[path]`
    include (feature flag controls LINKAGE, not DUPLICATION) sound.
  - **Phase R.3 (PENDING) — per-satellite dedup.** After R.2, with a reconciled
    pair and the guard at zero residual, convert the in-tree `mod` to a `#[path]`
    include of the satellite and delete the in-tree copy. Smallest-drift-first
    (stringprep / zipfile / xml_sax already at zero residual); decimal last (it
    must preserve the in-tree mpdec / without-mpdec split, or have the satellite
    absorb it). The default feature set must include the (now-mandatory) satellite
    features so a default build is behaviorally unchanged.
  See §3 Phase R and `memory/recovery/baton_move_R_satellite_drift.md`.
- **`molt-gpu` already exists as a crate** — `builtins/gpu.rs` (11,816) should migrate into it (or
  the existing `molt-gpu` should absorb it), gated by `molt_gpu_primitives` (verified the gate
  exists, `builtins/mod.rs:73`).
- **`object/ops.rs` (11,863)** is already in a split `object/` dir (9 files) — the residual large
  file is a *module-split* candidate (split by op family: the `object/` dir already demonstrates
  the pattern with `ops_string/ops_bytes/ops_arith/ops_iter/...`). Low priority (not a crate-cache
  win; it's runtime-core which everything links).

---

## 3. Migration plan (phased; each phase = one independently-complete move-only commit)

**Universal gate methodology (the 34e3bddbf contract), applied to EVERY phase:**

```bash
export MOLT_SESSION_ID="<unique>" && export CARGO_TARGET_DIR="$PWD/target-<id>"
# G1. Zero-warning build (CI-exact — NOTE: no --lib, so tests compile too; this is the
#     "cargo BUILD ≠ cargo TEST for warnings" lesson, MEMORY.md):
cargo clippy -p molt-backend --features native-backend -- -D warnings
cargo clippy -p molt-backend --features "native-backend llvm" --lib -- -D warnings   # llvm gate
# G2. Full lib suites (CI-exact, molt-backend/Cargo.toml gates):
cargo test -p molt-backend --features native-backend --lib
cargo test -p molt-backend --features "native-backend llvm" --lib llvm_backend::lowering
# G3. Byte-identical artifact/diagnostic check (move-only ⇒ output must not change):
#     build a fixed corpus before & after, diff the emitted .o/.wasm/.ll + stderr diagnostics.
python3 -m molt build --target native --output /tmp/before_<phase> <corpus.py> --rebuild
#     (repeat post-move; `diff` the artifacts — move-only MUST be byte-identical)
# G4. e2e smoke (guarded — never raw binary, CLAUDE.md):
python3 -m molt test  # or a representative differential subset
# G5. Symbol identity (crate moves): nm the rlib; the C-ABI surface (molt-runtime) must be
#     unchanged; no new no_mangle leaks from the move.
```

**A phase is NOT done until G1–G5 pass.** Move-only means **byte-identical artifacts** where the
phase claims to be move-only (G3). A phase that changes behavior is NOT a move-only phase and is
out of scope for this program.

### Phase ordering (dependency-correct)

```
M1  function_compiler.rs → opcode-family submodules     [within-crate, move-only]  ← move #1
F1  frontend/__init__.py → frontend/ package (mixins)    [Python, move-only]        ← move #2
G   generated.rs → per-feature-gate sub-files            [within-crate, move-only]  ← Phase G
T1  Extract molt-tir crate (tir/+ir/+passes.rs+Repr)     [crate, move-only]         ← move #3
R   Complete 28 satellite dedups (delete in-tree copies) [crate dep, behavior-pres] ← move #4
N1  Extract molt-backend-native (native+llvm onto -tir)  [crate]                    ← move #5
W1  Extract molt-backend-wasm                            [crate]
L1  Extract molt-backend-luau + molt-backend-rust        [crate]
C1  cli.py → cli/ package                                [Python, move-only]
O   object/ops.rs module split; gpu.rs → molt-gpu        [within-crate / crate]
```

**Why this order:** M1/F1/G are within-crate or Python — **zero blast radius on other agents'
crates**, do them first to relieve the hottest god-files immediately. T1 (extract `molt-tir`)
must precede N1/W1/L1 (the backend crates depend on it). R is independent (runtime side) and can
interleave anytime. N1 must wait for the LLVM partner's arc (§0.3). C1/O are cleanup, last.

### Per-phase specs

**M1 — `function_compiler.rs` split (defer boundary list to doc 08 if present; else use this):**
- Move-only split of the 39,043-line file into `native_backend/fc/{arith,control_flow,
  collections,exceptions,closures,trampolines,loops,async_gen,intrinsic_calls}.rs` + a
  `function_compiler.rs` that becomes a thin `mod fc; use fc::*;` re-export shell.
- Each submodule keeps `use super::*` (module-ancestry privacy preserved — NO crate boundary, so
  this is safe and unchanged from 34e3bddbf methodology).
- Line budget: no submodule >6,000 lines; target ~4,000 avg.
- Blast-radius win: `{DX-BASELINE:fc-incremental}` — editing one op family recompiles ~4-6K lines
  instead of 39K. (08 will measure; placeholder until then.)
- Rollback: `git revert` the single move-only commit; no API/behavior change to undo.

**F1 — frontend package (move #2):**
- Create `frontend/_types.py` (the small dataclasses), `frontend/visitors/*`, `frontend/lowering/*`,
  `frontend/generator.py` per §2.2. Methods move verbatim (move-only — no logic change).
- `__init__.py` shrinks to <800 lines (public re-exports + `compile_to_tir`).
- Add the `_GeneratorProtocol` typing shim (§2.2) — this is NOT gold-plating; it restores the type
  safety the single-class form had implicitly, and is required by the zero-workarounds policy.
- Gate: the frontend has no Rust gate; gate is `python3 -m molt build` byte-identical TIR output on
  a corpus + the full differential suite (`tests/differential/`) green. Python has no compile step,
  so G1/G3 reduce to "TIR output byte-identical" + "import surface unchanged."
- Rollback: revert the commit; `__init__.py` is restored wholesale.

**G — generated.rs per-feature split (move #Phase G):**
- Split `intrinsics/generated.rs` into `generated_core.rs` + `generated_<feature>.rs` per the
  existing `resolve_<X>_symbol` cfg-gated functions (doc 08 §"intrinsic registry split"). `generated.rs`
  becomes a thin routing file. **Regenerate via `tools/gen_intrinsics.py` — do not hand-edit the
  `@generated` file; update the generator to emit the split.** This is the structurally-correct path
  (the file says "DO NOT EDIT").
- This is the precursor to a future `molt-runtime-intrinsics-gen` crate (§2.1 option A), deferred.
- Gate: byte-identical resolver behavior (the manifest must resolve the same symbols).

**T1 — extract `molt-tir`:**
- New crate `runtime/molt-tir` = `tir/` + `ir.rs` + `ir_schema.rs` + `json_boundary.rs` +
  `passes.rs` + `repr.rs` (the representation vocabulary: `Repr`, scalar lanes, container
  lanes, and container-storage facts).
- `molt-backend` adds `molt-tir = { path = "../molt-tir" }`; `lib.rs` re-exports `pub use
  molt_tir::{...}` to preserve the public API surface (§1.3).
- The `repr` cycle cut: TIR, lower-to-LIR/WASM, call facts, liveness, and pass-delta import
  `crate::repr::Repr`; backend crates import `crate::repr::{...}` through the `molt_tir::repr`
  re-export. The `representation_plan` *logic* consumes that vocabulary instead of owning it.
- pub-surface contract: enumerate exactly which `tir::*` items `wasm.rs`/`main.rs`/`function_compiler.rs`
  consume (the §1.3 list) and make precisely those `pub` in `molt-tir` — no more (pub-creep risk §4).
- **matches!-oracle audit (CRITICAL, §4):** when ops/opcodes move crates, audit every
  `matches!(op.kind, ...)` and `matches!(opcode, ...)` oracle (e.g. `effects.rs`,
  `opcode_may_throw`, `is_side_effecting`) — these DEFAULT-FALSE on a missed arm and silently
  miscompile. Prefer exhaustive `match` (compiler-enforced) over `matches!` for anything that moves.
  This is the MEMORY.md lesson made a phase gate.
- Build win: `{DX-BASELINE:tir-extract}` — editing a TIR pass rebuilds `molt-tir` + parallel
  dependents instead of one 185K unit.
- Rollback: revert; merge `molt-tir`'s files back under `molt-backend/src/`, restore `Repr` to
  `representation_plan.rs`, drop the dep. (Larger revert than M1/F1 — hence T1 is move #3 not #1.)

**R — reconcile the 28 satellite pairs, then dedup (move #4) — CORRECTED:**

> The naive "verify `diff <(sort) <(sort)` = 0, delete the in-tree copy" recipe in
> the original draft is UNSOUND here: the pairs have drifted, the copies serve
> disjoint live tiers, and they are two access models (see §1.4/§2.4). The
> corrected, structurally-ordered plan:

- **R.1 (LANDED, Move R) — fail-closed parity guard + drift reconciliation.**
  - `tools/check_satellite_parity.py` normalizes the by-design access-layer
    differences and compares the residual line-multiset per pair; the
    `runtime/molt-runtime/tests/satellite_parity.rs` integration test runs it in
    the lib suite. `tools/satellite_parity_baseline.json` records the per-pair
    allowed residual count + a SHA-256 of the residual content, plus a one-way
    `ratchet_ceiling` (total residual; may only DECREASE). The guard FAILS on: a
    pair's residual exceeding baseline, the residual content changing at the same
    count, a missing pair, or the total exceeding the ceiling. This is a CONTRACT,
    not a sync script — it makes new drift a CI failure and ratchets toward zero.
  - Reconciliation ports each genuine one-sided behavioral fix into BOTH copies
    with a differential regression run vs CPython under BOTH a default build
    (satellite copy) and a `MOLT_DIFF_STDLIB_PROFILE=micro` build (in-tree copy).
    First landed fix: satellite `csv` adopted the in-tree general
    `ensure_hashable`/`HashContext::DictKey` unhashable-key check (new
    `RuntimeVtable::ensure_hashable` entry), pinned by
    `tests/differential/stdlib/csv_get_dialect_unhashable.py` (byte-identical on
    both tiers, incl. the previously-broken `bytearray` case).
- **R.2 (PENDING) — bridge-facade access-layer unification** (the real "phase 2";
  see §2.4). REQUIRED before any deletion: one source file must compile in both
  access contexts.
- **R.3 (PENDING) — per-satellite dedup.** Only after R.2 and a zero-residual,
  reconciled pair: convert the in-tree `mod` to a `#[path]` include of the
  satellite and delete the in-tree copy. Smallest-residual-first (12 pairs are
  already at zero residual: difflib, ipaddress, cmath, fractions,
  functions_zipfile, stringprep, html, unicodedata, xml_etree, xml_sax, zoneinfo,
  + the access-layer-only http drop). decimal LAST and special (preserve the
  in-tree mpdec / without-mpdec split). itertools needs its satellite to adopt
  RuntimeState-scoped slots first (it still uses process-global `AtomicU64`s).
- Per-pair gate: the parity guard at the new (lower) baseline + the full
  differential suite for that stdlib module green on BOTH tiers; binary-size delta
  measured at the eventual R.3 dedup (should be ≤0 — removing a duplicate CU).
- **Feature-unification trap (§4):** at R.3, if two satellites share a transitive
  dep with conflicting feature requirements, unifying them in the default set can
  silently enable a feature elsewhere. Audit `cargo tree -f '{p} {f}'`
  before/after each satellite's default-on migration.
- Rollback: R.1 is two independently-revertable commits (guard; reconciliation).
  R.3 is per-satellite (each satellite IS a complete structural piece per the
  CLAUDE.md "intermediate commits acceptable when each is itself complete" rule).

**N1 — extract `molt-backend-native` (move #5):**
- New crate = `native_backend/` + `llvm_backend/` onto `molt-tir`. The `use super::*` glob becomes
  explicit `use molt_tir::{...}` / `use molt_backend_core::{NaN-box helpers}`. The NaN-box helpers
  + `stable_ic_site_id`/`pending_bits` currently in `lib.rs` must move to a shared spot both
  `molt-backend` and `molt-backend-native` can import — put them in `molt-tir` (they are
  representation-level) or a tiny `molt-backend-abi` leaf.
- **Sequence AFTER the LLVM partner's current arc** (§0.3) — coordinate a freeze.
- Gate: symbol identity (G5) is load-bearing here — the C-ABI export surface must be byte-identical;
  `nm` the artifacts before/after.
- Rollback: largest revert; merge files back, restore `use super::*`. This is why N1 is last among
  the backend extractions and scored B+ not A.

**W1 / L1** — same pattern for wasm / luau+rust, each onto `molt-tir`. Lower coupling (luau/rust
only touch `representation_plan`), so easier than N1. wasm pulls heavy `tir::*` (already public after
T1).

**C1 / O** — cli.py package + object/ops.rs module split + gpu.rs→molt-gpu. Cleanup, lowest priority.

---

## 4. Risk register

| Risk | Where it bites | Mitigation |
|------|----------------|------------|
| **module split ≠ crate split** (the headline lesson) | Conflating M1 (module, no cache win) with a crate extraction; expecting build-cache wins from F1/M1 | State it in every phase header: M1/F1/G/C1/O are *edit-locality* moves (no/Python compile); T1/N1/W1/L1/R are *build-cache* moves. Only crate boundaries isolate the build. |
| **pub-boundary creep** | T1/N1/W1: making items `pub` to satisfy a cross-crate call, then more, until the boundary is meaningless | Enumerate the EXACT consumed surface (§1.3 lists) before extraction; make precisely those `pub`, no more. CI: a `pub`-surface snapshot test (count `pub` items in the crate's lib.rs; fail on unexplained growth). |
| **matches!-oracle silent miscompile** | T1/N1 when ops/opcodes move crates | Audit every `matches!(op.kind/opcode, ...)` (default-false on miss). Convert side-effect/throw/movability oracles to exhaustive `match`. This is a hard phase gate, not advice (MEMORY.md: caught a real silent-miscompile). |
| **feature-unification trap** | R (satellite default-on), N1 (llvm inside -native) | `cargo tree -f '{p} {f}'` diff before/after every default-feature change; verify no transitive feature silently flips. |
| **circular-dep landmine** | T1 (`representation_plan`/`tir` vocabulary ownership); F1 (`__init__`/mixin) | `repr.rs` is the molt-tir vocabulary authority; `representation_plan` consumes it. Frontend: dataclasses -> `_types.py` leaf; mixins import `_types`, never `__init__`. |
| **`use super::*` glob breakage** | N1/W1/L1 (native_backend relies on module-ancestry privacy) | Replace globs with explicit `use molt_tir::{...}`. This is mechanical but touch-heavy; budget it as the main cost of N1. Do NOT widen everything to `pub` to dodge it (pub-creep). |
| **byte-identical regression** | Any phase claiming move-only | G3 artifact diff is mandatory. If artifacts differ, the move changed behavior → it is not move-only → reject and find what leaked (usually an inlining/ordering change from a crate boundary; acceptable ONLY if proven semantically identical and documented). |
| **extracting a crate from under an active editor** | N1 vs LLVM partner | Sequence N1 after the partner's arc; or freeze-window coordinate. Never `git mv` files another agent is mid-edit on. |
| **mixin loses static self-typing** | F1 | `_GeneratorProtocol` + `if TYPE_CHECKING: self: _GeneratorProtocol` per mixin. |
| **satellite drift already happened** (CONFIRMED, partly mitigated) | R | All 28 in-tree copies DID drift (confirmed, not "likely") — `functions_http` raw sorted-diff was 820, not 0. Move R landed the fail-closed parity guard (`tools/check_satellite_parity.py` + `satellite_parity.rs`) that normalizes the access layer, ratchets the residual toward zero, and fails CI on new drift; reconciliation ports one-sided fixes into both copies with two-tier differential tests. A raw `diff <(sort) <(sort)`=0 check is the WRONG gate (it flags by-design access-layer differences); use the guard's normalized residual. Deletion (R.3) is blocked on the R.2 access-layer unification. |
| **churn-data illusion** | Prioritization | The 50-commit window is too short for frequency churn. Do NOT re-derive priorities from `git log` frequency; use the structural ranking (§1.2 caveat). |

---

## 5. Per-phase build-time win (keyed placeholders — fill from doc 08's baseline)

These are estimates keyed to the DX agent's measurements (do not invent numbers — §0.2):

| Phase | Win | Key |
|-------|-----|-----|
| M1 | fc incremental: 39K → ~4-6K recompile per op family | `{DX-BASELINE:fc-incremental}` |
| G | one fewer 24.5K-line CU in the runtime hot path | `{DX-BASELINE:generated-split}` |
| T1 | TIR-pass edits compile dependents in parallel, not as one 185K unit | `{DX-BASELINE:tir-extract}` |
| R | removes 28 duplicate CUs from the monolith | `{DX-BASELINE:satellite-dedup}` |
| N1 | codegen edits ⟂ pass edits; parallel native/wasm/llvm compile | `{DX-BASELINE:native-extract}` |
| F1/C1/O | no build-time win (Python / module-only) — friction-relief only | n/a |

Config wins (LTO thin for daemon, sccache default-on, mold/lld) are doc 08's Phase 1 — mostly
landed (`release-output` exists, `[profile.release]` is thin). The remaining `release-fast` `lto =
"fat"` (Cargo.toml:295) is 08's call, not this program's.

---

## 6. The five highest-leverage first moves (ranked detail)

Ranked by `(friction-relief × build-win) / risk`. Rationale per move:

1. **M1 — split `function_compiler.rs`.** Highest score: it is the largest single file (39,043),
   the #2 contention source, and the split is the *safest* kind (within-crate, `use super::*`
   preserved, byte-identical, proven by 34e3bddbf). Delivers a real incremental-build win with
   minimal risk. Likely owned by the DX agent — coordinate, don't duplicate.
2. **F1 — frontend package (mixins).** #1 contention source (the only 3-touch file; documented
   repeated contention). No build win (Python) but the *highest friction relief*: it converts the
   single most-collided file into ~10 independently-ownable files, directly enabling more parallel
   agents. Medium risk (mixin MRO + lost static self-typing, mitigated by `_GeneratorProtocol`).
3. **T1 — extract `molt-tir`.** The keystone build-cache move. Enabled by the verified zero
   back-edge finding — the layering is *already* correct, only the crate line is missing. Unblocks
   N1/W1/L1. Medium risk (pub-surface contract + the surgical Repr cut + the matches!-oracle audit).
4. **R — reconcile + guard the satellites (R.1 LANDED), then unify + dedup (R.2/R.3).** Eliminates a
   systemic 28× dual-maintenance / silent-drift hazard that was already materialized (shipped
   behavior differed by build tier). R.1 (parity guard + reconciliation) is low risk, independently
   revertable, and stops new drift NOW; R.2 (access-layer unification) and R.3 (per-satellite dedup,
   independently revertable) are medium risk and follow. High friction relief (stdlib-module authors
   stop editing two diverging copies). Independent of the backend arc — can run anytime.
5. **N1 — extract `molt-backend-native`.** The biggest single build-cache win (isolates the 58K-line
   Cranelift+LLVM codegen from the rest) but the riskiest extraction (the `use super::*` → explicit
   imports rewrite, symbol-identity gate, and the LLVM-partner sequencing constraint). Last of the
   five because its risk is real and it must wait on T1 + the partner.

---

## 7. Verification appendix (every §-claim's command)

```bash
# §1.1 sizes
wc -l src/molt/frontend/__init__.py src/molt/cli.py
find runtime/molt-backend/src -name '*.rs' | xargs wc -l | sort -rn | head
grep -nE '^(class |def )' src/molt/frontend/__init__.py   # → SimpleTIRGenerator L1343, compile_to_tir L44604
# §1.2 churn
git log --oneline | wc -l                                  # → 50 (full history)
git log --numstat --pretty=format: | awk '...'             # → frontend/__init__.py only 3-touch file
# §1.3 dependency direction
grep -rcE 'crate::(wasm|luau|rust|llvm_backend|native_backend)' runtime/molt-tir/src/tir/  # → (none)
grep -rnE 'crate::representation_plan' runtime/molt-tir/src/tir/  # → only lower_to_wasm:1530, lower_to_lir:13
grep -ohE 'crate::[a-z_]+' runtime/molt-tir/src/representation_plan.rs | sort|uniq -c  # → tir×22, ir×2
# §1.4 satellite drift (CORRECTED — the copies are NOT identical):
diff <(sort runtime/molt-runtime/src/builtins/functions_http.rs) \
     <(sort runtime/molt-runtime-http/src/functions_http.rs) | grep -c '^[<>]'   # → 820 (NOT 0)
grep -c 'cfg(not(feature = "stdlib_' runtime/molt-runtime/src/builtins/mod.rs    # → 28
# The correct drift metric (access layer normalized away) + the fail-closed guard:
python3 tools/check_satellite_parity.py --verbose        # per-pair normalized residual vs baseline
python3 tools/check_satellite_parity.py --show functions_http   # the residual diff for one pair
# §0.2 DX-doc staleness
grep -nE 'lto|\[profile' Cargo.toml | head      # release thin@40; release-fast fat@295; release-output@366
# §3 gates
sed -n '/cargo clippy -p molt-backend/p' .github/workflows/ci.yml   # the CI-exact gate (no --lib)
```

---

*Co-Authored-By: Claude Opus 4.8 (1M context) <noreply@anthropic.com>*
