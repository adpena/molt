# Parallel-Build Architecture: maximizing dev velocity + incremental throughput

Status: design / proposal (2026-06-03). Author: autonomous session.

## TL;DR — the two structural bottlenecks (measured)

1. **`molt-runtime` is a 344K-line monolith** (2× the next crate `molt-backend` at
   162K; the long tail of 20+ runtime crates is ≤19K each). **26 crates depend on
   it**, so it sits on the critical path AND cannot parallelize internally beyond
   `codegen-units`. Editing ANY of its 116 `builtins/*.rs` or 38 `object/*.rs`
   files recompiles the whole 344K-line crate.
2. **`release-fast` (the build-iteration profile, used by the backend daemon) sets
   `lto = "fat"`.** Fat LTO re-merges every crate's bitcode into ONE module and
   re-optimizes/codegens the whole program in a **single-threaded serial phase** —
   it deletes the cross-crate parallelism `codegen-units` would give. The profile
   comment already concedes "cgu count has negligible effect on the final binary"
   under fat LTO; that's exactly the problem for an *iteration* profile.

Everything else (no default fast linker, no default `sccache`, per-worktree
`target/` in the multi-agent model) compounds these two.

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
- The crates share the MoltObject `u64` bit ABI + the intrinsic registry. The split
  MUST route all shared types through `molt-runtime-core` (no duplicated `MoltObject`
  definitions). Cyclic deps are illegal in cargo — design the layering as a DAG
  (core ← text/num/collections ← exceptions/iter ← facade).
- **`intrinsics/generated.rs` (24K lines) is a hub**: `resolve_core_symbol`
  address-takes every intrinsic, creating an artificial all-to-one dependency that
  also defeats `-dead_strip` (see the binary-size baton). Generate **per-crate
  intrinsic sub-registries** composed by a thin top-level resolver. This
  simultaneously: (i) breaks the build hub, (ii) advances the per-app intrinsic
  tree-shaking / <2MB binary-size goal. Two top priorities solved by one refactor.
- Do it as a real structural arc (one cohesive crate at a time, each landing
  green), not a half-split that leaves two sources of truth.

### 2. Stop fat-LTO'ing the build-ITERATION profile; reserve fat LTO for shipped artifacts
Distinguish two link products:
- **The backend daemon** (`molt-backend` + deps): a *compiler*; its hot path is
  Cranelift codegen, NOT whole-program-optimized runtime. It does not need fat LTO.
  Set `release-fast` → `lto = "thin"` (parallel, ~90% of fat's perf) or `"off"`.
- **The shipped user-binary runtime** (statically linked into the AOT output):
  fat LTO matters here for end-user runtime perf. Keep fat LTO on the *artifact*
  link step (`release`/published), not on the daemon's iteration builds.
These are different link steps — separating them removes the single-threaded LTO
tax from every dev rebuild while preserving the perf contract for shipped binaries.
Measure first: time the LTO phase of a `release-fast` daemon build to quantify.

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
With 26 dependents on `molt-runtime`, any feature-unification mismatch (a crate
built with different feature sets per consumer) forces duplicate compiles. Audit
`native-backend`, `stdlib_path`, wasm features for accidental thrash; prefer
additive features resolved once.

### 5. Multi-agent worktree throughput
The heavy worktree-per-agent model (currently many `worktree-agent-*`) maximally
benefits from #1 (agents editing different leaf crates don't serialize on the
monolith) + #3 (shared sccache). Consider a shared read-only `CARGO_HOME`/registry
cache + a shared sccache dir across worktrees (the session-scoped `target/` stays
per-agent for isolation; the *cache* is shared).

## Sequencing (each step lands green; no half-states)
1. **Now / low-risk, config-only:** default-on sccache + fast linker for dev
   profiles; split `release-fast` LTO (thin for daemon, fat for shipped artifact).
   Measure the wall-time delta on a touch-one-file incremental daemon build.
2. **Structural, highest-leverage:** carve `molt-runtime-core` out first (object
   model + GIL + alloc), make the monolith depend on it; verify all gates green.
   Then peel one builtins cluster at a time (text → num → exceptions → iter → …),
   each as a complete crate with its own tests, landing green before the next.
3. **Intrinsic registry:** per-crate intrinsic sub-registries + thin composing
   resolver (co-designed with the binary-size per-app resolver work).

## Cross-cutting wins
- Decomposition (#1) + per-crate intrinsic registries (#3-structural) ALSO advance
  the **<2MB binary-size** goal (precise per-app dead-strip) and the **typed-IR /
  backend-coherence** work (clearer crate contracts). One structural arc, three
  roadmap goals.
- Keep the perf contract intact: shipped artifacts retain fat LTO; only the dev
  iteration loop trades whole-program re-opt for parallelism + incrementality.
