<!-- Foundation blueprint 75. Arc: DEVELOPER EXPERIENCE — mine the world's best OSS
compiler/build-system practice to kill the remaining wall-clock in Molt's
iteration loop, ranked and actionable against OUR attested numbers AND OUR
already-landed state. Author: research scout. Date: 2026-07-10. RESEARCH-ONLY
landing (doc-only; no builds, no source edits). Composes with 74 (build-work-dedup
doctrine — the four laws this mining serves), 73 (8GB builds / toolchain
provisioning / binary CDN), 56 + 08 + dx_baseline (DX build-speed arc, the daemon
design, "module≠crate / function-is-the-codegen-unit"). Every external claim
carries a PRIMARY-source citation (§8). Every "already landed" claim is verified
against the tree (§7). Windows-primary dev box (14 cores); wasm32-wasip1 target. -->

# 75 — Mining OSS Build-Speed Practice for Molt's Iteration Loop

> Historical research record. The V2/V3 config-lattice and environment-knob
> descriptions below are retained with the 2026-07-10 evidence they explain;
> immutable atomic runtime generations and exact pair identities now supersede
> that runtime-WASM cache authority. Current shipping policy lives in
> `Cargo.toml` and `docs/agent/RUNTIME_ARTIFACT_CODEGEN_AUTHORITY_20260727.md`.
> Native and WASM dev requests already select `dev-fast`; the historical L1
> recommendation and July timings below are not current profile or latency facts.
> Current WASM iteration intent, effective incrementality and target/publication
> custody are separate typed decisions in `runtime_wasm_build_policy.py` and
> `RuntimeCargoPlan`; [operations](../../OPERATIONS.md#build-time) owns their controls.
> Default-on incrementality was not adopted: ordinary dev and release runtime
> builds retain the nonincremental default. Explicit iteration controls remain
> available; this document makes no current default speedup claim.

The workflow comparison also covers [tstack](https://github.com/t3dotgg/tstack)
and [pstack](https://github.com/cursor/plugins/tree/main/pstack): their useful
patterns are review through finding closure, bounded parallel ownership, measured
performance experiments and follow-through to integration. Molt keeps those
responsibilities in its existing review and managed-landing authorities rather
than introducing another gate runner or proof registry.

**Status:** research directive + ranked plan, 2026-07-10. Owner: DX arc.
Doc-only landing. Implements the operator mandate: *a world-class compiler mines
all the best from OSS — no waste, no duplicated work, EVER* (74 four laws;
M05/M07/M09/M12).

## 0. What this doc is (and the anti-duplication guard)

This is the **evidence + ranking layer** under doctrine 74. Doctrine 74 states
the *laws* (content-addressed, failure-resilient, config-lattice, attested). This
doc mines the OSS state of the art and maps each lever onto **Molt's attested
per-iteration costs (§1)** AND **Molt's already-landed build machinery (§7)**,
so no lever below re-invents something the tree already has.

**Before scoring any lever, this session read the tree (§7).** The result
reshaped the ranking: Molt has ALREADY landed the fast-iteration Cargo profiles
(`dev-fast`: `lto="off"`, cgu=256), the per-package opt-level layering, the
config-lattice reuse + stable dep-cache (V1/V2/V3, opt-in), a resident BACKEND
compiler daemon (`backend_process` + `function_cache_key` + `daemon_cache`), the
content-addressed `runtime.wasm` cache (73 R73.1), and the per-module frontend
lowering cache. The remaining wall-clock is therefore NOT "we lack fast profiles
or a daemon." It is two precise gaps:

1. **Configured ≠ effective (historical; superseded by DO-1 below).** The July
   snapshot charged a ~150–230s `wasm-release` compile to iteration. Dev requests
   now select `dev-fast`; exact runtime generations replaced config-lattice
   reuse. Those historical costs do not justify enabling Cargo incrementality
   by default.
2. **The daemon is warm on the BACKEND, cold on the FRONTEND.** `module_graph`
   (~99s) and much of `backend_prepare` (~125s) are Python-side phases that still
   cold-start every invocation (Molt spawns a fresh Python per build). The peers
   that solved this (mypy `dmypy`, rust-analyzer/salsa, esbuild, Zig) hold the
   *frontend* module graph + analysis resident with fine-grained invalidation.

Nothing here was built or measured this session. Every "expected win" is a
**hypothesis to gate** (M05/74 law 4): attach a before/after wall attestation
before claiming any of it landed. A cache/profile that "exists but cold-starts"
is a FAILING one (the M55 trap).

## 1. The attested baseline (what we are attacking)

Per-iteration wall costs already attested in the tree (74 preamble; 73 R73.1;
E2-BUILD-WALLCLOCK CLAIMS rows). Every lever is scored against THESE, not
generic "Rust is slow" folklore.

| Phase | Attested cost | Trigger | Cores used today |
|---|---|---|---|
| `module_graph` (frontend) | ~99s | every build (cold per process) | ~1 (serial-ish) |
| `backend_prepare` | ~125s | every build | partial |
| `backend_subprocess_compile` | ~95s | every build (Molt's OWN wasm codegen of app+stdlib) | partial |
| runtime cargo compile | historical ~150–230s | **only when a runtime `.rs` changes** | **shipping policy is now ThinLTO/cgu=16/debug=0; see runtime codegen authority evidence** |
| wasm-opt + wasm-ld | phase-attested and content-addressed | first artifact/policy identity | dev: no optimizer (2026-10-23) |
| uv provisioning | ~minutes | cold only | n/a |
| proof-queue | overhead | per proof | n/a |

## 2. Ranked lever table (scored on §1 numbers AND §7 landed state)

Effort S/M/L. Risk = correctness/regression risk. "Status" flags whether the
*mechanism* already exists (so the lever is finish/default/attest, not build-new).

| # | Lever | Expected win on OUR numbers | Effort | Risk | Status vs tree (§7) | Evidence (primary) | Windows caveat |
|---|---|---|---|---|---|---|---|
| **L1 — superseded (see DO-1)** | Dev requests already select `dev-fast`; exact runtime generations replace config-lattice reuse | No current default-on incrementality speed gain established | — | Preserve exact artifact admission and proof custody | Ordinary WASM runtime builds retain their nonincremental default; explicit iteration controls remain available | `runtime_wasm_build_policy.py`, `RuntimeCargoPlan`, [operations](../../OPERATIONS.md#build-time) | Performance remains workload- and host-specific |
| **L2** | **Extend resident-daemon warm state to the FRONTEND module-graph + analysis** (backend daemon already warm); fine-grained per-top-level-def invalidation, NOT whole-module | `module_graph` **~99s → sub-5s** warm; large slice of `backend_prepare` warmed. Biggest structural win across ALL iterations | **L** | **Med** — stale warm cache = silent-wrong-answer class (M34) | **Partial:** backend daemon exists; FRONTEND phases still cold per-process | mypy `dmypy` fine-grained deps + `astdiff`/`aststrip` [3]; salsa red/green early-cutoff (rust-analyzer, Ruff Red-Knot) [4][5]; esbuild immutable-shared warm AST cache [6]; Zig in-process incremental ~125× / <300ms [7] | file-watch = `ReadDirectoryChangesW`, mtime/size is NTFS-lossy (M35); bind to `MOLT_SESSION_ID` + collision oracle (56 DX-2/3) |
| **L3** | **Tighten + ATTEST the existing `function_cache_key`** to mypy per-def grain + full-input key + hit-rate gate | `backend_subprocess_compile` **~95s → proportional-to-changed-fns** | **M** | **Med** — key must include ALL true inputs or silent stale codegen (M34/M36) | **Mechanism LANDED** (`function_cache_key` in `backend_process`); gap = grain + attestation | mypy grain = "module top level or top-level fn/method" [3]; salsa early-cutoff [4]; Turborepo/Buck2 action key = hash(cmd+inputs+upstream) [8][9] | None specific |
| **L4 — LANDED, superseded 2026-10-23 (dev runs no optimizer; see DO-4)** | **Default DEV/iteration wasm-opt to `-O1`; keep explicit bounded ship policies** | removes fixed-point `Oz` from the dev critical path and content-addresses the exact pipeline | **S** | **Low** — config-lattice plus fail-closed publication | `molt.wasm_optimization` is the single level/feature/pass authority | Binaryen: `-O1` = "quick & useful opts, useful for iteration builds"; `-O3/-O4` "spends potentially a lot of time" [10] | None |
| **L5** | **`cargo-hakari` workspace-hack (feature unification across the ~30 crates)** | fresh-session/first-build dep tail shrinks; up to ~1.7× cumulative on the cargo side; kills "same dep built N ways" | **S/M** | **Low** — generated crate + CI check | **ABSENT** (verified: no hakari/workspace-hack in tree) — genuinely new | cargo-hakari (guppy feature-union, "up to 1.7×") [11] | None. Pure-Cargo, cross-platform |
| L6 | **Parallel rustc front-end `-Zthreads=8`** for the runtime crate compile | clean/full runtime compile **~-23–30%** at our core count | S | Med | nightly toolchain pin needed | Rust project-goals + blog: 8 threads ≈ -23–30% clean-build wall [12][13] | nightly only; distributed on x86_64 Windows nightly |
| L7 | **`ccache` as `CC` launcher for the clang→wasm32-wasi C-ext (numpy/scipy) recompiles** | cold C-ext recompiles served from cache across sessions/machines | S | Low | not wired (sccache is off, M09) | ccache supports Clang on Windows; used as compiler-launcher for wasi-sdk [14] | Works on Windows (unlike sccache, §4). Direct-mode preprocessed-source hashing |
| L8 | **Pyodide-style prebuilt sealed-wheel artifact + CDN for numpy/scipy** (fetch, don't rebuild) — the 73 R73.3 CDN, grounded in Pyodide's recipe/wheel model | uv/C-ext cold minutes → seconds (fetch) | M | Low | **73 R73.3 pending** (M09/M15) | Pyodide: build once → platform-tagged wheel, skip deps already in dist, ABI-tag includes toolchain version [15][16] | None (fetch path) |
| L9 | **Recompile-blast-radius crate split** (leaf crates so a runtime edit rebuilds <1 crate cone) — the 56 DX-1 ratchet | shrinks *which* code L1 even has to recompile | L | Med | 21b crate-graph program in flight | Bevy dynamic_linking + leaf-crate/linker guidance [17]; "only a CRATE split buys build-cache isolation; a function is rustc's atomic codegen unit" (dx_baseline, 56) | None |
| L10 | **rustc Cranelift codegen backend for DEV-only builds** (`-Zcodegen-backend=cranelift`) | ~20% codegen-time cut on parts still LLVM-bound | M | **Med-High** | not used | ~20% codegen / ~5% clean-total; ~10× faster codegen than LLVM [18] | **Windows NOT production-ready** (Linux/macOS focus 2025–26); on x86_64 Windows nightly but MUST gate parity before trusting output |

## 3. Top-5 do-next (concrete first steps)

Ordered by leverage. Each is a hypothesis with an attestation gate (74 law 4).

### DO-1 — superseded profile and reuse recommendation

Native and WASM dev requests already select `dev-fast`. The old config-lattice
knob is retired; exact runtime generations own reuse. Ordinary source-checkout
WASM dev and release requests retain the nonincremental default unless explicit
iteration or Cargo controls select otherwise. A valid `MOLT_RUNTIME_BUILD_PROFILE`
or enabled `MOLT_RUNTIME_WASM_INCREMENTAL` opts into stable family placement,
subject to exact Cargo target, pinned-session and proof/reproducibility custody.
Generated session labels do not request isolation.

The captured Cargo plan determines effective incrementality after sccache
normalization. Incremental emissions are excluded from shared publication;
bound nonincremental emissions remain eligible regardless of target placement.
Keep these decisions separate, preserve exact artifact admission, and measure
the intended workload before proposing another default change. The July cost
figures above do not establish current latency or a default-on speed gain.

### DO-2 — extend the resident daemon to the FRONTEND (module_graph + analysis)
The backend compiler daemon (`backend_process`, `function_cache_key`,
`daemon_cache`) is warm, but `module_graph` (99s) + much of `backend_prepare`
(125s) are Python-side and cold every invocation. Lift the proven invalidation
model so a warm frontend can't serve stale state:
- **Granularity = top-level def, not module.** mypy's smallest reprocessing unit
  is "a module top level or a top-level function/method" [3]; match that bar (56
  already commits to "function is the codegen unit"). Whole-*file* `.tsbuildinfo`
  caching has a documented stale-cache class we must NOT copy (§4, [19]).
- **Early-cutoff.** salsa's red/green rule: when a recomputed derived result
  equals the prior value, stop propagating [4][5] — this is what makes a rename /
  added import touch only the affected scope.
- **Diff by symbol table, not mtime.** mypy compares symbol tables (`astdiff`)
  and strips the AST fresh (`aststrip`) [3]. On Windows the daemon must
  content-hash and watch via `ReadDirectoryChangesW` (mtime/size listing is
  NTFS-lossy, M35).
- **Immutable warm state** so concurrent agents share safely (esbuild's rule
  [6]); bind to `MOLT_SESSION_ID` + the collision oracle (56 DX-2). Ship behind a
  flag with a **hit-rate attestation from day one** — a frontend daemon that
  cold-starts every session is the exact M55 failing-cache trap.

### DO-3 — tighten + attest the existing `function_cache_key`
`function_cache_key` already exists in `backend_process`. Two concrete moves:
(a) make the grain a single top-level fn/method (mypy's proven unit [3]) so an
edit re-codegens only changed functions of the ~95s `backend_subprocess_compile`;
(b) audit that the key contains the *complete* input set — source + resolved
facts + config-lattice node + tooling fingerprint — exactly as Turborepo/Buck2
hash `cmd + all inputs + upstream hashes` [8][9] (a missing input = silent stale
codegen, the M34/M36 class). Add `{key, hit|miss, wall}` lines and a hit-rate
gate (74 law 4).

### DO-4 (landed, then superseded) — dev wasm-opt at `-O1`
The build-intent resolver selected non-converging `O1` for dev and the
deployment profile's shipping policy for release. Since 2026-10-23 dev builds
run no optimizer: for a 91-line asyncio program, O1 cost 267 CPU seconds of a
388 CPU-second build (`docs/spec/areas/wasm/WASM_OPTIMIZATION_PLAN.md` §4.4). App and monolithic optimized
publication fail closed. Shared-runtime body optimization belongs to its Cargo
generation; the app linker now performs only content-addressed structural
cleanup and never repeats an invalid whole-runtime Binaryen pass.

### DO-5 — add `cargo-hakari` workspace-hack (genuinely absent)
Verified: no hakari/workspace-hack crate in the tree. Across ~30 crates,
dependencies built with differing feature sets get rebuilt multiple ways; hakari
unions them (guppy build simulation; "up to 1.7× cumulative" [11]). Steps:
`cargo install cargo-hakari`; `cargo hakari init`; `cargo hakari generate`; add
`cargo hakari manage-deps` + a `cargo hakari verify` CI check (mirror the
god-file ratchet discipline, M43/M44 — a new crate born UNGATED gets a gate).
Measure with `cargo build --timings` (the HTML unit-graph shows exactly which
crate serializes the critical path [20]) before/after. The per-package opt-level
layering that usually pairs with this is **already landed** (dev + dev-fast
`[profile.*.package.*]` overrides) — do NOT redo it.

## 4. DON'T list (measured/known harmful, or already landed — do not re-litigate)

- **DON'T re-invent what §7 shows is landed.** The `dev-fast`/`release-fast`
  fast-iteration profiles, the per-package opt-level layering, the config-lattice
  reuse + stable dep-cache (V1/V2/V3), the backend compiler daemon, the
  content-addressed `runtime.wasm` cache, and the frontend per-module lowering
  cache existed in the July snapshot; exact runtime generations now replace
  config-lattice reuse. Recommending "create a dev-fast profile" or "add opt
  layering" would be exactly the duplicated work the operator forbids.
- **DON'T use sccache on Windows.** Measured HARMFUL here (M09). That is why L7
  specifies `ccache` (which supports Clang on Windows [14]) for the C-ext path.
- **DON'T reach for `wild` or `mold` on Windows.** Both are Linux-only ELF
  linkers, no Windows support, no LTO [21][22].
- **DON'T swap the Windows *host* linker to `lld-link`/`rust-lld` without a parity
  gate.** rust-lld is still NOT default on x64-msvc-windows (open since 2020), and
  `lld-link` has reported miscompilations fixed only by reverting to MSVC
  `link.exe` [23]. (The wasm32 target ALREADY uses rust-lld by default — no action
  there; this is about the native host link only.)
- **DON'T trust the Cranelift backend for the SHIPPED or acceptance build on
  Windows.** Windows is explicitly not the production-ready target for
  rustc_codegen_cranelift in 2025–26 [18]. L10 is DEV-only, gated by a parity
  check vs the LLVM build — never on the correctness lane (M05).
- **DON'T copy TypeScript's whole-file `.tsbuildinfo` model for the frontend
  daemon.** It has a long-standing stale-cache-invalidation class (dependency/type
  changes not invalidating; manual delete required) [19]. Match mypy's fine-grained
  per-definition bar (DO-2), not whole-file.
- **DON'T confuse the dev profile with Cargo incrementality.** Dev requests
  already select `dev-fast`; ordinary WASM runtime builds remain nonincremental.
  Measure any proposed profile or incrementality change on its intended workload.
- **DON'T ship dynamic-linked / side-module runtime to "avoid relink" without
  proving it beats DO-1 + a warm daemon.** Emscripten/WASI dynamic linking adds
  runtime overhead and is discouraged for best performance/size [24], and it
  touches the size-constrained SHIP path (3MB ceiling). Investigate only if a
  measured relink bottleneck survives DO-1 + DO-2.

## 5. Historical ranking is superseded

The July ranking placed a default `dev-fast` profile and config-lattice reuse
first. Dev requests already select `dev-fast`, and exact runtime generations own
reuse. This is no longer an actionable default flip. Cargo incrementality is a
separate decision: ordinary WASM dev and release runtime builds retain the
nonincremental default. Rank further work from current phase measurements;
neither the July figures nor this ranking establish a current default speed gain.

## 6. Surprising / liftable-wholesale findings

- **The historical ship-profile finding is resolved for dev requests.** They
  already select `dev-fast`. The July 150–230s compile does not measure current
  behavior or justify enabling incrementality by default; see DO-1.
- **The daemon warmth is asymmetric.** Molt already runs a resident BACKEND
  compiler daemon (`backend_process`, `function_cache_key`, `daemon_cache`) — but
  the FRONTEND `module_graph`/analysis (the 99s phase) is still cold per Python
  process. The high-value structural work is extending existing daemon discipline
  to the frontend (DO-2), not standing up a daemon from scratch.
- **Pyodide is a wholesale lift for the C-ext path.** Pyodide already cross-builds
  numpy/scipy to wasm, emits *standard platform-tagged wheels*, **skips building
  deps already present in the distribution**, and bakes the toolchain version into
  the ABI tag so a built wheel is safely reusable out-of-tree [15][16]. That is
  precisely the 73 R73.3 "build once, publish, fetch" CDN with a proven recipe
  format — Molt's numpy seal should become a fetched, ABI-tagged artifact (L8).
- **`-Zshare-generics` is default-on for our dev builds but OFF under the runtime's
  release-LTO (`wasm-release`).** It de-dupes cross-crate monomorphizations [25] —
  a historical profile distinction, not evidence for a current default change.
- **Zig is the existence proof for the daemon ceiling.** In-process incremental
  compilation took Zig ~36s cold → 228–288ms warm (~125×) [7]. That is the target
  class for DO-2 — a two-orders-of-magnitude structural change to
  module_graph/analysis, not a 2× tweak.

## 7. Landed-state verification (read this session; do not duplicate)

Verified against the worktree tree at `origin/main` HEAD `d4888f7103`:

- **Fast-iteration Cargo profiles — LANDED.** `Cargo.toml` `[profile.dev-fast]`
  `inherits="dev"`, `codegen-units=256`, `lto="off"`; `[profile.release-fast]`
  (backend-daemon iteration). Ship profiles `[profile.release-output]` and
  `[profile.wasm-release]` now uses measured ThinLTO, 16 codegen units, debug=0,
  and `opt-level="z"`.
- **Per-package opt-level layering — LANDED.** The historical snapshot had
  mirrored `dev`/`dev-fast` package tables. Current root profiles own dependency
  symbols once through `[profile.dev.package."*"] debug=0`; `dev-fast` inherits
  that policy and explicit workspace/hot-crate optimization settings. See
  [Cargo workspace truth custody](../../spec/areas/tooling/0001-toolchains.md#cargo-workspace-truth-custody)
  for the current authority and the separate shipped-runtime policies.
- **Config-lattice reuse + stable dep-cache — LANDED (opt-in).** V1
  (c1ff76bd7d/f21164c7e6 single combined compile, now the sole split-runtime
  `both` producer with no dual-compile kill switch/retry), V2 (c13e2e216f stable dep-cache
  default-on for iteration profiles), V3 (0ac15702df `MOLT_BUILD_REUSE_COMPATIBLE`
  config-lattice reuse) per CLAIMS BUILD-DEDUP-B rows.
- **Runtime wasm profile resolver — LANDED.** `src/molt/cli/runtime_build.py`
  `_resolve_wasm_cargo_profile`: precedence = explicit `MOLT_WASM_CARGO_PROFILE`
  → `MOLT_RUNTIME_BUILD_PROFILE` (iteration knob, e.g. `dev-fast`) → default
  (`release`→`wasm-release`, `dev`→`dev-fast`). L1's historical dev-profile gap
  is superseded; profile selection does not establish effective incrementality.
- **Backend compiler daemon + function cache — LANDED.** `runtime/molt-backend/
  src/backend_process/{job.rs,protocol.rs}` carry `function_cache_key`;
  `src/main_tests/{daemon_cache.rs,daemon_env.rs}` exercise the daemon.
- **wasm-opt level — parameterized, dev default none (was `O1`).** The canonical authority
  supports every Binaryen level, owns cross-engine feature flags and explicit
  ship pass sets, and keys derived artifacts by the complete pipeline.
- **cargo-hakari / workspace-hack — ABSENT.** No hakari config or workspace-hack
  crate in the tree (grep, this session). L5 is genuinely new.
- **Content-addressed `runtime.wasm` cache + frontend per-module lowering cache —
  LANDED** (73 R73.1; `runtime_wasm_cache.py`; frontend cache per 74 preamble).

## 8. Sources (primary, grouped)

Rust/Cargo compile-time:
- [1] Cargo Book — Profiles (custom `inherits`, `[profile.*.package]`, build-override, `--profile`): https://doc.rust-lang.org/cargo/reference/profiles.html
- [2] Rust Performance Book — Build Configuration (LTO fat/thin/off, codegen-units): https://nnethercote.github.io/perf-book/build-configuration.html
- [11] cargo-hakari / workspace-hack (guppy feature-union, "up to 1.7× cumulative"): https://docs.rs/cargo-hakari/latest/cargo_hakari/about/index.html
- [12] Rust Project Goals — Promoting the parallel front-end (2026): https://rust-lang.github.io/rust-project-goals/2026/parallel-front-end.html
- [13] Rust Blog — Faster compilation with the parallel front-end (-Zthreads): https://blog.rust-lang.org/2023/11/09/parallel-rustc/
- [18] Rust Project Goals — Production-ready Cranelift + rustc_codegen_cranelift README (Windows status; ~20%/~5%): https://rust-lang.github.io/rust-project-goals/2025h2/production-ready-cranelift.html , https://github.com/rust-lang/rustc_codegen_cranelift
- [20] Cargo Book — Reporting build timings (`--timings`): https://doc.rust-lang.org/cargo/reference/timings.html
- [23] rust-lang/rust #71520 — Use lld by default on x64 msvc windows (open; lld-link miscompiles): https://github.com/rust-lang/rust/issues/71520
- [25] rustc PR #123244 — `-Zshare-generics` (default-on for non-opt builds): https://github.com/rust-lang/rust/pull/123244

WASM toolchain:
- [10] Binaryen `wasm-opt` man page (opt levels; -O1 "useful for iteration"): https://manpages.debian.org/testing/binaryen/wasm-opt.1.en.html
- [21] Phoronix — Wild linker (Linux-only, no LTO): https://www.phoronix.com/news/Wild-Linker
- [22] wild-linker/wild README (Linux x86_64/ARM64/RISC-V only): https://github.com/wild-linker/wild
- [24] Emscripten — Dynamic Linking (overhead; prefer static) + WebAssembly/tool-conventions DynamicLinking.md: https://emscripten.org/docs/compiling/Dynamic-Linking.html , https://github.com/WebAssembly/tool-conventions/blob/main/DynamicLinking.md

C-ext cross-compile:
- [14] ccache platform/compiler support (Clang on Windows) + wasi-sdk `CMAKE_CXX_COMPILER_LAUNCHER=ccache`: https://ccache.dev/platform-compiler-language-support.html , https://github.com/WebAssembly/wasi-sdk
- [15] Pyodide — Building Python Packages from Source (cross-build env, skip already-built deps, ABI tag): https://pyodide.org/en/stable/development/building-packages-from-source.html
- [16] pyodide-build docs: https://pyodide-build.readthedocs.io/en/stable/index.html

Peer pipeline caching / daemons / incremental analysis:
- [3] mypy — Mypy Daemon wiki (fine-grained deps; unit = top-level fn/method; astdiff/aststrip): https://github.com/python/mypy/wiki/Mypy-Daemon , https://mypy.readthedocs.io/en/stable/mypy_daemon.html
- [4] Salsa — rustc-dev-guide (red/green, early-cutoff): https://rustc-dev-guide.rust-lang.org/queries/salsa.html
- [5] rust-analyzer architecture (salsa query store) + Ruff Red-Knot (salsa incremental): https://rust-analyzer.github.io/book/contributing/architecture.html
- [6] esbuild architecture/API (immutable shared warm AST/file cache, precise watch): https://esbuild.github.io/api/
- [7] Zig incremental compilation (in-process, ~125× / <300ms): https://ziglang.org/devlog/2026/ , https://deepwiki.com/ziglang/zig/3.3-incremental-compilation
- [8] Turborepo caching (content-addressed task hash = inputs+env+deps+upstream): https://turbo.build/
- [9] Buck2 — content-addressed action cache + persistent workers: https://buck2.build/docs/rule_authors/persistent_workers/ , https://buck2.build/docs/users/remote_execution/
- [17] Bevy — dynamic_linking + leaf-crate/linker guidance: https://bevy.org/learn/quick-start/getting-started/setup/ , https://docs.rs/bevy_dylib
- [19] TypeScript — `.tsbuildinfo`/incremental stale-cache class (issues #54501, #38648): https://www.typescriptlang.org/tsconfig/tsBuildInfoFile.html

## 9. tsrs: a compiler built for speed, low memory and an agent dev loop (2026-10-22)

Read 2026-10-22 from the tsrs repository at commit 89ea430 [T1]. tsrs ports the TypeScript 7
compiler (tsgo, itself a Go port of tsc) to Rust. Its published benchmarks [T2] are the 2026-10-16
snapshot at tsrs d57703ab5f07. On a 16-vCPU host it type-checks mui-docs in 0.75 s against tsgo's
12.66 s, in 1.60 GiB against 7.39 GiB. On an 8-vCPU host, after a comment edit, its watch mode
re-checks vscode in 33 ms against 1.76 s. Molt has two resource regimes: building and verifying
Molt uses the whole machine for the least wall clock; compiling a user's program is low-memory and
fast. tsrs informs both.

### 9.1 The development loop (building and verifying Molt)

| tsrs practice | tsrs number | Molt today | Owner |
|---|---|---|---|
| Edit-loop profile: `release` settings, no LTO, 256 codegen units, incremental | a checker edit rebuilds in 4.9 s against 176 s [T3] | dev builds default to `dev-fast` (no LTO, 256 codegen units) since 130b439e1 | — |
| The pinned Go compiler is the specification; any diagnostic, order, text or exit-code difference is a tsrs defect | all 13,462 error baselines pass in the harness's `--baselines js` mode, measured at tsrs f250e637 on 2026-10-08 [T4] | CPython is the oracle; the differential suite has no enforced passing set | HF-106 |
| The passing set never shrinks: `comm -23 before after` prints nothing before a merge | — [T5] | not enforced | HF-106 |
| The 15,197-case conformance suite runs from baselines [T4] | 15 s wall, 51 s CPU [T3] | `molt_diff.py` builds and runs each program | — |
| Performance lands on evidence: 1% of single-threaded instructions (deterministic), 2% of wall time across two publishes, or 5% of peak memory at the default checker count | — [T6] | the scoreboard records wall time, peak RSS, binary size and compile time of compiled programs, and flags a 5% RSS or size gain; no lane records instructions retired or the compiler's own peak RSS; the perf gate does not complete | HF-185, HF-459, V1-07 |
| Search before an experiment: a "measured and rejected" list, read first; an idea returns only with new evidence | global identifier interning: parse +9% [T7] | §4 here, HF-263, scattered rows | HF-460 |
| Short agent rules that point to the authority | `AGENTS.md` [T6] | `AGENTS.md` | — |

### 9.2 Under the hood (compiling a user's program)

| tsrs technique | Measured effect [T7] | Molt counterpart |
|---|---|---|
| Native core; work-stealing parallel check; by default all cores up to 16, then half the cores, at most 32, fewer on small projects | 4.2–16.9x faster than tsgo, each at its default thread count [T2] | Molt's frontend (module graph, analysis, lowering) runs in Python: HF-254, DO-2 |
| 32-bit arena handles; `Option<P<T>>` is 4 bytes; `const` size pins (`Type` is 24 bytes) | peak memory −14 to −15%, instructions +6.5% | HF-202 (typed compact ops): add size pins with it |
| `FxHashMap` everywhere, std `HashMap` banned, sorted output | Fx costs 0.7–1.4% of instructions | audit the backend's maps |
| Per-thread arenas; per-file regions retired once their diagnostics are read | vscode peak RSS −13 to −16% | the frontend keeps whole-program state in Python dicts |
| Lazy members; cold fields split out of hot structs | — | TIR facts (HF-201, HF-202) |
| `#[cold]` splits of rare branches | parse 16.66 G → 15.07 G instructions | TIR facts (HF-201, HF-202) |
| Huge pages for arena chunks only (`MADV_HUGEPAGE`) | −2 to −6% wall, 21–48x fewer faults | after HF-202 |
| Heavy files split by statement range across checkers; the owner merges in order | check 0.40 s → 0.12 s at 32 checkers, byte-identical | the backend's per-function parallelism |
| mimalloc v3 as the compiler's allocator | −3 to −14% wall against v2, jemalloc and glibc | a jemalloc switch for the backend daemon was deleted unmeasured (797be4558, HF-281): HF-458 |
| Ship build: fat LTO, PGO and BOLT (frame pointers only in the profiling build) | PGO −13.5% instructions; BOLT −1.0 to −3.2% wall on a 38k-file codebase, −3.4 to −4.0% on vscode | `Makefile.pgo` (`pgo-full`, `bolt-backend`, `tools/bolt_optimize.sh`) builds a PGO and BOLT `molt-backend`; on 2026-04-23 (0c587cd8e) PGO gave about 0% because the backend was 1.5% of compile wall clock and the Python frontend dominated: HF-458 |

### 9.3 What Molt adopts

1. Selection and overlap (landed 40438ab3d): a landing selects only the plan entries an edit
   reaches (HF-457) and overlaps its Cargo and pytest gates (HF-334).
2. HF-458: first re-measure the backend's share of compile time, which was 1.5% when April's PGO
   run found about 0% gain. Since then a resident daemon serves the backend, record cutoffs shorten
   the frontend on rebuilds, and HF-254 moves frontend work into Rust. Re-run PGO, BOLT and
   mimalloc v3 through `Makefile.pgo` only where that share makes the bar in 9.1 reachable.
3. HF-459: instructions retired and the compiler's own peak RSS become scoreboard axes, reported
   before and after on every performance change.
4. HF-460: one evidence registry for measured optimizations, with each verdict tied to the typed
   context that produced it (`docs/design/optimization_evidence_registry.md`). Once its first phase
   lands, it will replace the static §4 list as the place agents search before they design.

Sources (tsrs at commit 89ea430, https://github.com/oxc-project/tsrs):
- [T1] `README.md`
- [T2] `bench/RESULTS.md`
- [T3] `notes/dev-loop-build.md`
- [T4] `notes/upstream-gaps-2026-10-08.md`
- [T5] `CONTRIBUTING.md`
- [T6] `AGENTS.md`
- [T7] `docs/RUST.md`, `notes/perf-round2-followups.md`, `crates/tsrs_checker/src/types.rs`
