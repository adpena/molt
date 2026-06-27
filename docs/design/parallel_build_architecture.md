# Parallel-Build Architecture: maximizing dev velocity + incremental throughput

Status: live routing doc / partially landed (refreshed 2026-06-24).
The live codebase and executable Cargo metadata remain authoritative.

## Live State Snapshot (2026-06-24)

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
- `molt-runtime-stringprep`, the `html` / `unicodedata` portions of
  `molt-runtime-text`, `molt-runtime-zoneinfo`, the math-family modules owned by
  `molt-runtime-math`, XML owned by `molt-runtime-xml`, `difflib` owned by
  `molt-runtime-difflib`, and `ipaddress` owned by `molt-runtime-ipaddress` are
  completed leaf-ownership examples: their in-facade fallback modules are
  deleted, their generated resolver arms delegate into leaf-owned intrinsic
  sub-registries, their symbol prefixes are link-affecting feature gates, and
  feature-on/feature-off checks prove the facade no longer carries duplicate
  authorities for those domains.
- The lower stack is now split: `runtime/molt-ir/` owns canonical IR/TIR data,
  SimpleIR transport/schema, representation vocabulary, generated op-kind
  facts, debug/process diagnostics, and intrinsic-symbol utilities;
  `runtime/molt-passes/` owns TIR analyses, pass/fact orchestration,
  SimpleIR<->TIR transport, module/drop orchestration, target/profile
  descriptors, pass cache, and value-keyed representation facts; and
  `runtime/molt-tir/` owns backend projection, LIR/WASM/MLIR lowering, and
  SimpleIR-name representation projection. `runtime/molt-backend/Cargo.toml`
  depends on this stack and
  activates its feature gates through backend features. The old dx Phase-3
  baton predated this cut; use this document, not that standalone baton, for the
  remaining backend-native extraction route.
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
- Build-throughput measurement JSON now has one command-result authority:
  `tools/throughput_measurement.py` owns elapsed-time normalization, timeout
  return-code policy, bounded stdout/stderr tails, cwd capture, and optional
  output artifact size. `tools/throughput_matrix.py` and
  `tools/bench_backend_incremental.py` consume that schema instead of carrying
  sibling result dataclasses.
- The generated runtime intrinsic resolver is no longer one monolithic Rust
  source file. `runtime/molt-runtime/src/intrinsics/generated.rs` keeps the
  parser-facing `INTRINSICS` manifest table and re-exports a thin resolver, while
  `runtime/molt-runtime/src/intrinsics/generated_resolvers/` owns one generated
  resolver module per intrinsic category. This reduces resolver edit
  invalidation and makes the future per-leaf-crate registry cut mechanical
  instead of duplicating resolver authority.
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
   The current throughput bootstrap derives one canonical artifact root through
   `RunContext`/`tools/throughput_env.sh`, prefers a healthy external root when
   configured, and shares `CARGO_TARGET_DIR`, `MOLT_DIFF_CARGO_TARGET_DIR`,
   `MOLT_CACHE`, and `.sccache` under that root. Isolation comes from
   `MOLT_SESSION_ID`, daemon/socket identity, and lock custody rather than each
   agent inventing a private target tree.

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
- The generated resolver hub is split at source-file granularity:
  `generated.rs` remains the manifest table, and generated per-category resolver
  modules own the address-taking match arms. The generator emits rustfmt-stable
  resolver files and skips exact-content no-op writes before invoking rustfmt,
  lazy-loads formatting custody only for changed Rust files, and prevents
  repeated generation from dirtying mtimes or triggering needless Cargo
  rebuilds. `molt-runtime-stringprep`, `molt-runtime-math`,
  `molt-runtime-xml`, `molt-runtime-difflib`, and `molt-runtime-ipaddress` now
  own generated per-crate intrinsic sub-registries, with the `molt-runtime`
  category resolvers reduced to feature-gated facade delegates. `molt-runtime-path`
  now owns an event-specific audit bridge for `os_ext` and `pathlib`, replacing
  the old generic `path.has_capability` side effect and restoring the capability
  gates that were missing from fourteen leaf `os` operations. The remaining
  structural target is moving the other category resolvers into **per-crate
  intrinsic sub-registries** composed by a thin facade resolver. This
  simultaneously:
  (i) finishes breaking the build hub, (ii) advances the per-app intrinsic
  tree-shaking / <2MB binary-size goal. Two top priorities solved by one
  refactor.
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
  today any agent that misses the canonical throughput env can fall back to a
  private `target/` and recompile the whole world; `tools/new-agent-task.sh`
  writes `logs/agents/<task>/env.sh` so each lane can source the same
  shared-root policy before building.
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
monolith) + #3 (shared canonical artifact roots and sccache). Keep
`CARGO_TARGET_DIR`, `MOLT_DIFF_CARGO_TARGET_DIR`, `MOLT_CACHE`, `.sccache`, and
`tmp/` under the chosen artifact root; keep per-agent separation in
`MOLT_SESSION_ID`, daemon sockets, logs, and worktree ownership.

## Sequencing (each step lands green; no half-states)
1. **Structural runtime composition:** continue turning `molt-runtime` into a
   facade over the existing leaf crates. For each cluster, move the authority
   once, delete the old in-crate duplicate, and prove the feature gate builds
   both standalone and through the facade.
2. **Backend-native extraction:** create the `molt-backend-native` crate only
   when `native_backend/*` plus `llvm_backend/*` can move as one authority over
   native lowering. Keep TIR/passes/representation facts in backend core.
3. **Intrinsic registry:** the generated resolver source split has landed and
   the `stringprep` plus math-family resolvers are now leaf-owned; continue
   moving remaining categories to per-crate intrinsic sub-registries + thin
   composing facade resolvers, co-designed with the binary-size per-app resolver
   work.
4. **Frontend F2:** replace the F1 move-only mixin split with semantic authority
   surfaces so frontend changes stop serializing through one shared class/state
   owner.

## Cross-cutting wins
- Decomposition (#1) + split/per-crate intrinsic registries (#3-structural) ALSO advance
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

## Addendum (2026-06-24): dx Phase-3 Baton Folded Into Current Routing

The deleted `docs/design/foundation/dx_phase3_extraction_baton.md` was a
pre-`molt-tir` handoff anchored to base `9e93503bb`. Its durable content is the
backend-native extraction boundary, but its mechanics are stale now that
`molt-tir` exists and `molt-backend` depends on it directly.

Current extraction state:
- Already extracted: `runtime/molt-ir` owns the immutable IR/data layer,
  `runtime/molt-passes` owns TIR passes/facts/target descriptors plus the
  SimpleIR<->TIR round-trip, and `runtime/molt-tir` owns backend-neutral LIR,
  verification, representation planning, and representation-plan name
  projection. `runtime/molt-backend-wasm` now owns WASM instruction projection
  (`lower_to_wasm`) and the `wasm-encoder` dependency. None of the IR/pass/TIR
  crates depends on native/wasm/luau/llvm/rust backend implementation crates.
- Not yet extracted: there is no `runtime/molt-backend-native` workspace member.
  `runtime/molt-backend/Cargo.toml` still owns Cranelift and Inkwell
  dependencies, default `native-backend`, `llvm`, `wasm-backend`, `luau-backend`,
  and `rust-backend` feature composition.
- CLI/daemon orchestration is in `src/molt/cli/__init__.py`, with the backend
  binary still built from package `molt-backend` and named `molt-backend`.

Next structural cut:
- Create `molt-backend-native` only when `native_backend/*` and
  `llvm_backend/*` can move together as one native codegen authority.
- Keep `molt-ir` as the typed-IR authority, `molt-passes` as the midend
  pass/fact/round-trip authority, and `molt-tir` as the backend-neutral
  TIR/LIR/representation authority. Backend-specific instruction projection
  belongs in the backend crates; do not duplicate TIR facts or re-export
  compatibility shims from the native crate.
- Keep `molt-backend` as the composition crate until the daemon/bin move is
  atomic; when the bin moves, preserve the binary name `molt-backend` so CLI
  artifact discovery does not fork.
- Any new cross-crate `pub` surface must be a durable API needed by the native
  crate, not a temporary alias. Prefer the existing `molt-tir` exports first.

Landing gates for the native extraction:
- `cargo build -p molt-passes`, `cargo test -p molt-passes`, and
  `cargo build -p molt-tir` prove the core midend and TIR/LIR layers remain
  backend-free.
- `cargo build -p molt-backend --no-default-features` proves core composition
  no longer pulls Cranelift/Inkwell after the cut.
- `cargo build -p molt-backend-native --features native-backend` and
  `--features native-backend,llvm` prove the moved native/LLVM authority.
- `src/molt/cli/__init__.py` must still find a binary named `molt-backend`.
- BX-4 evidence must measure both directions: touch a TIR pass and confirm the
  native crate only relinks as required; touch native codegen and confirm
  `molt-tir` does not rebuild.
