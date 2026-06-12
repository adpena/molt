# Parallel-Build Architecture: maximizing dev velocity + incremental throughput

Status: live routing doc / partially landed (refreshed 2026-06-12).
The live codebase and executable Cargo metadata remain authoritative.

## Live State Snapshot (2026-06-12)

- The build-iteration profile fix from this document has already landed in the
  root `Cargo.toml`: `release-fast` uses thin LTO with high codegen-unit
  parallelism, while shipped output profiles retain fat LTO where binary
  size/runtime performance need whole-program optimization.
- Runtime leaf crates exist and are wired as path dependencies from
  `runtime/molt-runtime/Cargo.toml`: `molt-runtime-core`, `-math`, `-text`,
  `-collections`, `-serial`, `-crypto`, `-compression`, `-net`, `-asyncio`,
  `-regex`, `-path`, `-itertools`, `-difflib`, `-logging`, `-http`, `-xml`,
  `-ipaddress`, `-zoneinfo`, `-stringprep`, and `-tk`. Guarded
  `cargo metadata --no-deps` reports these as workspace packages.
- `molt-runtime-stringprep` is now a completed leaf-ownership example:
  the in-facade fallback module was deleted, `molt_stringprep_*` is a
  `stdlib_stringprep` link-affecting gate, and checks pass with the feature
  both enabled and disabled.
- The extraction is not complete. `molt-runtime` is still the facade plus a
  large implementation owner, `runtime/molt-backend/src/native_backend/function_compiler.rs`
  remains a ~28K-line codegen lock, and `src/molt/frontend/__init__.py` remains
  a ~27K-line frontend lock. Native Cranelift is nevertheless decomposing by
  complete op-family handlers under `native_backend/function_compiler/fc/`;
  indexing plus scalar builtins (`id`, `ord`, fused `ord_at`, `chr`) now live
  outside `compile_func_inner` while `len` stays inline because it owns
  representation-plan specialization.
- Native backend TIR optimization no longer constructs one whole-program
  uncached result wave for large user closures. Uncached user functions are
  partitioned by function count and op budget, optimized in one bounded
  parallel batch at a time, then applied and cache-written before the next batch
  is materialized. This is the current backend compile-memory response for the
  enabled off-the-shelf tinygrad runner.
- The next throughput work is therefore extraction/composition, not another
  profile-only LTO fix.

## TL;DR — current structural bottlenecks

1. **`molt-runtime` is still a ~352K-line crate** (the next crate,
   `molt-backend`, is ~208K; the long tail of 20+ runtime leaves is ≤20K each).
   **Multiple product crates depend on
   it**, so it sits on the critical path AND cannot parallelize internally beyond
   `codegen-units`. Editing ANY of its 116 `builtins/*.rs` or 38 `object/*.rs`
   files recompiles the whole ~352K-line crate.
2. **The backend-native and frontend god-file locks still serialize multi-agent
   development.** The backend module split landed, but native codegen is still
   centered on `function_compiler.rs`; frontend F1 split files, but F2 semantic
   authority split is still active work.
3. **Shared-cache policy is still more important than raw local target size.**
   Per-worktree `target/` roots isolate agents correctly, but the system still
   needs more shared, deterministic cache surfaces so the Nth agent does not
   rebuild what the first agent already proved.

## Prioritized levers (highest leverage first)

### 1. Decompose `molt-runtime` into a core + feature-cohesive leaf crates (THE lever)
Turn the monolith into a thin facade (`molt-runtime`) that re-exports cohesive
sub-crates. Suggested cut (along the existing module seams):
- `molt-runtime-core` — object model (`object/`: MoltObject u64 ABI, headers,
  refcount, GIL/PyToken, alloc, layout). Everything depends on this; keep it small
  and stable so its metadata is ready early for cargo pipelining. (`molt-obj-model`
  already exists — fold the truly-shared ABI there or into core, and make the big
  runtime *depend* on it instead of inlining.)
- `molt-runtime-builtins-text` (str/bytes/codecs — `ops_string.rs`, `ops_bytes.rs`,
  `builtins/codecs.rs`), `-num` (int/float/bigint), `-collections` (list/dict/set/
  tuple — note `molt-runtime-collections` already exists as a separate crate),
  `-exceptions`, `-iter`, `-io`, `-os`, etc. Each maps to a builtins/object cluster.
- `molt-runtime` becomes a façade crate: `pub use molt_runtime_text::*; …` + the
  intrinsic resolver glue.

Benefits: (a) cargo compiles leaf crates **in parallel across all cores**; (b)
touching one builtin recompiles ONE small crate, not 344K lines — the daemon
incremental loop drops from "recompile the monolith" to "recompile one leaf +
relink"; (c) cargo **pipelining** starts dependents as soon as a crate's *metadata*
(not full codegen) is ready — currently blocked because the monolith is one giant
unit.

Hard constraints / watch-items:
- The crates share the MoltObject `u64` bit ABI + the intrinsic registry. The
  target split MUST route all shared types through `molt-runtime-core`, but live
  `molt-runtime-core` still has copied type IDs and a `PyToken` stub while the
  real object-model/GIL authority remains in `molt-runtime`; deleting that
  duplicate authority is part of the extraction work. Cyclic deps are illegal in
  cargo — design the layering as a DAG (core ← text/num/collections ←
  exceptions/iter ← facade).
- **`intrinsics/generated.rs` (24K lines) is a hub**: `resolve_core_symbol`
  address-takes every intrinsic, creating an artificial all-to-one dependency that
  also defeats `-dead_strip` (see the binary-size baton). Generate **per-crate
  intrinsic sub-registries** composed by a thin top-level resolver. This
  simultaneously: (i) breaks the build hub, (ii) advances the per-app intrinsic
  tree-shaking / <2MB binary-size goal. Two top priorities solved by one refactor.
- Do it as a real structural arc (one cohesive crate at a time, each landing
  green), not a half-split that leaves two sources of truth.

### 2. Keep build-iteration LTO split from shipped-artifact LTO (LANDED; preserve)
Distinguish two link products:
- **The backend daemon** (`molt-backend` + deps): a *compiler*; its hot path is
  Cranelift codegen, NOT whole-program-optimized runtime. It does not need fat LTO.
  `release-fast` is already thin-LTO in the root `Cargo.toml`; keep it that way
  unless new measurements prove another profile is better.
- **The shipped user-binary runtime** (statically linked into the AOT output):
  fat LTO matters here for end-user runtime perf. Keep fat LTO on the *artifact*
  link step (`release`/published), not on the daemon's iteration builds.
These are different link steps — separating them removes the single-threaded LTO
tax from every dev rebuild while preserving the perf contract for shipped binaries.
Root `Cargo.toml` records the measured fat→thin `release-fast` delta; future work
should extend the measurement to crate extraction and cache-hit rebuild cases.

### 3. Default-on `sccache` + a fast linker (lld/mac, mold/Linux)
- **`sccache`**: caches compiled rlibs across sessions AND worktrees. The repo
  already has `MOLT_USE_SCCACHE` + `_run_cargo_with_sccache_retry`; make it
  default-on for dev. This is enormous for the **multi-agent worktree model** —
  today each `.claude/worktrees/agent-*` has its own `target/` and recompiles the
  whole world; sccache lets the Nth agent reuse the 1st's artifacts.
- **Fast linker**: `release-fast`/fat-LTO link of a 344K-line crate is link-bound.
  `-C link-arg=-fuse-ld=lld` (mac) / `mold` (Linux). Currently opt-in only in
  `.cargo/config.toml`; flip on for dev profiles (keep the portable baseline for CI).

### 4. Feature-graph hygiene (avoid rebuild thrash)
Guarded live metadata currently shows three direct workspace reverse-dependencies
on `molt-runtime`: `molt-wasm-host`, `molt-embed`, and `molt-ffi`. Treat that as
the live critical path, not the older over-broad dependent count. Any
feature-unification mismatch across those consumers, backend features, or WASM
profiles can still force duplicate compiles; audit `native-backend`,
`stdlib_path`, wasm features, and leaf stdlib gates for accidental thrash, and
prefer additive features resolved once.

### 5. Multi-agent worktree throughput
The heavy worktree-per-agent model (currently many `worktree-agent-*`) maximally
benefits from #1 (agents editing different leaf crates don't serialize on the
monolith) + #3 (shared sccache). Consider a shared read-only `CARGO_HOME`/registry
cache + a shared sccache dir across worktrees (the session-scoped `target/` stays
per-agent for isolation; the *cache* is shared).

## Sequencing (each step lands green; no half-states)
1. **Structural runtime composition:** continue turning `molt-runtime` into a
   facade over the existing leaf crates. For each cluster, move the authority
   once, delete the old in-crate duplicate, and prove the feature gate builds
   both standalone and through the facade.
2. **Backend-native extraction:** create the `molt-backend-native` crate only
   when `native_backend/*` plus `llvm_backend/*` can move as one authority over
   native lowering. Keep TIR/passes/representation facts in backend core.
3. **Intrinsic registry:** per-crate intrinsic sub-registries + thin composing
   resolver (co-designed with the binary-size per-app resolver work).
4. **Frontend F2:** replace the F1 move-only mixin split with semantic authority
   surfaces so frontend changes stop serializing through one shared class/state
   owner.

## Cross-cutting wins
- Decomposition (#1) + per-crate intrinsic registries (#3-structural) ALSO advance
  the **<2MB binary-size** goal (precise per-app dead-strip) and the **typed-IR /
  backend-coherence** work (clearer crate contracts). One structural arc, three
  roadmap goals.
- Keep the perf contract intact: shipped artifacts retain fat LTO; only the dev
  iteration loop trades whole-program re-opt for parallelism + incrementality.
- Keep docs honest: when a profile or crate boundary lands, update this file
  from `Cargo.toml`, `cargo metadata`, and targeted build timings before using
  old design text as a work plan.

## Addendum (2026-06-03): backend god-file split landed; crate-extraction boundary scoped

Step 1 (module split) LANDED `34e3bddbf`: `runtime/molt-backend/src/lib.rs` 6,928→264 lines.
`SimpleBackend` + native codegen now live in `native_backend/simple_backend.rs`.

Step 2 (extract `molt-backend-native`) — measured boundary from `simple_backend.rs`:
- Its cross-module edges: `crate::tir` ×29 (type_refine, lower_to_simple, lower_from_simple,
  lower_to_lir, serialize, cache, verify_lir(_repr), passes, printer), `crate::passes` ×7
  (compute_return_alias_summaries, compute_intrinsic_manifest), `crate::debug_artifacts` ×5,
  `representation_plan`, `ir_rewrites`, `ir`, and a `cfg(llvm)` edge to `llvm_backend`.
- No back-edge: only `main.rs` (the binary) + sibling `function_compiler.rs` reference
  `SimpleBackend` — so `native_backend/*` is extractable.
- CORRECTION (per the project taxonomy): **native = Cranelift + LLVM** (one codegen
  family — `SimpleBackend` drives `llvm_backend::lowering`/`runtime_imports` under
  `cfg(llvm)`); **wasm is its own backend**; **luau + rust are transpilers**. So the cut is:
  **`molt-backend` (core)** keeps ir/tir/passes/representation_plan/ir_rewrites/
  intrinsic_symbols/debug_artifacts/json_boundary; **`molt-backend-native`** = `native_backend/*`
  (SimpleBackend, function_compiler, consts, vec_layout) **+ `llvm_backend`** (LLVM is part of
  native, not core), depending on core; **wasm**, and the **luau/rust transpilers**, are their
  own extractions (or stay in core until measured). This keeps the `SimpleBackend → llvm_backend`
  edge intra-crate (no cross-crate cfg dance) and matches the native=Cranelift+LLVM model.
- Honest caveat: native's heavy `tir` dependency means a `tir` edit still recompiles native
  (unavoidable — native depends on tir). The incremental win is the reverse: editing native
  codegen no longer recompiles tir/passes/the non-native backends. Pick this boundary (one
  cut, low back-edge) over splitting `tir` further until measurement shows tir churn dominates.
