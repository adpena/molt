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

1. **Configured ≠ effective (74 law 4 / M34 / M55).** The fast profile + lattice
   reuse EXIST but are OPT-IN. The attested ~150–230s "fat LTO + cgu=1" runtime
   compile (§1) is `wasm-release` — the SHIP profile — still landing on the
   iteration path by default, because `dev-fast` reuse is gated behind
   `MOLT_RUNTIME_BUILD_PROFILE`/`MOLT_BUILD_REUSE_COMPATIBLE=1`. The lever is
   **prove-safe-per-edge → default-on + attest**, not a new profile.
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
| runtime cargo compile | ~150–230s | **only when a runtime `.rs` changes** | **~1 of 14 (`wasm-release`: fat LTO + cgu=1)** |
| wasm-opt + wasm-ld | untimed-but-real | every app change | 1 (wasm-opt default single-thread; dev default `Oz`) |
| uv provisioning | ~minutes | cold only | n/a |
| proof-queue | overhead | per proof | n/a |

## 2. Ranked lever table (scored on §1 numbers AND §7 landed state)

Effort S/M/L. Risk = correctness/regression risk. "Status" flags whether the
*mechanism* already exists (so the lever is finish/default/attest, not build-new).

| # | Lever | Expected win on OUR numbers | Effort | Risk | Status vs tree (§7) | Evidence (primary) | Windows caveat |
|---|---|---|---|---|---|---|---|
| **L1** | **Default-on the already-built `dev-fast` runtime profile + config-lattice reuse for the ITERATION runtime.wasm; keep `wasm-release`/`release-output` (fat LTO/cgu=1) only on ship/acceptance** | runtime-touch path **~150–230s → ~40–90s** (reclaims 13 idle cores via cgu=256; drops whole-program LTO). Pure 74-law-3 reuse: dev wasm is semantically identical, only fatter/slower | **S** | **Low** — LTO/cgu never change semantics; ship/acceptance still pins exact identity (74 non-negotiable) | **Mechanism LANDED** (`dev-fast`, V2/V3); gap = it's OPT-IN. Default resolves to `wasm-release` | our Cargo.toml (`[profile.dev-fast] lto="off" cgu=256`) + `runtime_build.py` precedence; 74 law 3 "opt-in until proven safe per edge, then default-on"; perf-book LTO/cgu [2] | None. cgu parallelism *helps* on 14 cores |
| **L2** | **Extend resident-daemon warm state to the FRONTEND module-graph + analysis** (backend daemon already warm); fine-grained per-top-level-def invalidation, NOT whole-module | `module_graph` **~99s → sub-5s** warm; large slice of `backend_prepare` warmed. Biggest structural win across ALL iterations | **L** | **Med** — stale warm cache = silent-wrong-answer class (M34) | **Partial:** backend daemon exists; FRONTEND phases still cold per-process | mypy `dmypy` fine-grained deps + `astdiff`/`aststrip` [3]; salsa red/green early-cutoff (rust-analyzer, Ruff Red-Knot) [4][5]; esbuild immutable-shared warm AST cache [6]; Zig in-process incremental ~125× / <300ms [7] | file-watch = `ReadDirectoryChangesW`, mtime/size is NTFS-lossy (M35); bind to `MOLT_SESSION_ID` + collision oracle (56 DX-2/3) |
| **L3** | **Tighten + ATTEST the existing `function_cache_key`** to mypy per-def grain + full-input key + hit-rate gate | `backend_subprocess_compile` **~95s → proportional-to-changed-fns** | **M** | **Med** — key must include ALL true inputs or silent stale codegen (M34/M36) | **Mechanism LANDED** (`function_cache_key` in `backend_process`); gap = grain + attestation | mypy grain = "module top level or top-level fn/method" [3]; salsa early-cutoff [4]; Turborepo/Buck2 action key = hash(cmd+inputs+upstream) [8][9] | None specific |
| **L4** | **Default the DEV/iteration wasm-opt to `-O1`; keep `Oz`/`O2`/`O3` for ship** | the untimed wasm-opt step drops to "quick & useful" dev level; ship size/perf unchanged | **S** | **Low** — pure config-lattice; dev wasm not shipped | **Knob exists** (`wasm_opt_level`, defaults `Oz` even for dev) — gap = dev default | Binaryen: `-O1` = "quick & useful opts, useful for iteration builds"; `-O3/-O4` "spends potentially a lot of time" [10] | None |
| **L5** | **`cargo-hakari` workspace-hack (feature unification across the ~30 crates)** | fresh-session/first-build dep tail shrinks; up to ~1.7× cumulative on the cargo side; kills "same dep built N ways" | **S/M** | **Low** — generated crate + CI check | **ABSENT** (verified: no hakari/workspace-hack in tree) — genuinely new | cargo-hakari (guppy feature-union, "up to 1.7×") [11] | None. Pure-Cargo, cross-platform |
| L6 | **Parallel rustc front-end `-Zthreads=8`** for the runtime crate compile | clean/full runtime compile **~-23–30%** at our core count | S | Med | nightly toolchain pin needed | Rust project-goals + blog: 8 threads ≈ -23–30% clean-build wall [12][13] | nightly only; distributed on x86_64 Windows nightly |
| L7 | **`ccache` as `CC` launcher for the clang→wasm32-wasi C-ext (numpy/scipy) recompiles** | cold C-ext recompiles served from cache across sessions/machines | S | Low | not wired (sccache is off, M09) | ccache supports Clang on Windows; used as compiler-launcher for wasi-sdk [14] | Works on Windows (unlike sccache, §4). Direct-mode preprocessed-source hashing |
| L8 | **Pyodide-style prebuilt sealed-wheel artifact + CDN for numpy/scipy** (fetch, don't rebuild) — the 73 R73.3 CDN, grounded in Pyodide's recipe/wheel model | uv/C-ext cold minutes → seconds (fetch) | M | Low | **73 R73.3 pending** (M09/M15) | Pyodide: build once → platform-tagged wheel, skip deps already in dist, ABI-tag includes toolchain version [15][16] | None (fetch path) |
| L9 | **Recompile-blast-radius crate split** (leaf crates so a runtime edit rebuilds <1 crate cone) — the 56 DX-1 ratchet | shrinks *which* code L1 even has to recompile | L | Med | 21b crate-graph program in flight | Bevy dynamic_linking + leaf-crate/linker guidance [17]; "only a CRATE split buys build-cache isolation; a function is rustc's atomic codegen unit" (dx_baseline, 56) | None |
| L10 | **rustc Cranelift codegen backend for DEV-only builds** (`-Zcodegen-backend=cranelift`) | ~20% codegen-time cut on parts still LLVM-bound | M | **Med-High** | not used | ~20% codegen / ~5% clean-total; ~10× faster codegen than LLVM [18] | **Windows NOT production-ready** (Linux/macOS focus 2025–26); on x86_64 Windows nightly but MUST gate parity before trusting output |

## 3. Top-5 do-next (concrete first steps)

Ordered by leverage. Each is a hypothesis with an attestation gate (74 law 4).

### DO-1 (highest leverage) — cash the already-built `dev-fast` runtime profile into the DEFAULT iteration path
The fast profile is DONE (`[profile.dev-fast] lto="off" codegen-units=256`, plus
per-package opt layering); the config-lattice reuse (V3) and stable dep-cache
(V2) are DONE but OPT-IN. The attested ~150–230s (fat LTO + cgu=1) proves the
SHIP profile `wasm-release` is still what lands on the runtime-touch iteration
path by default — a textbook "configured ≠ effective" miss (74 law 4). This is
**not** new mechanism; it is flipping proven capability to default-on:
- In `runtime_build.py`'s profile resolver, make an **iteration** build (dev
  cargo profile, no acceptance/ship flag) resolve the wasm runtime to `dev-fast`
  by default, not `wasm-release`. Ship/acceptance/CDN paths continue to pin
  `wasm-release`/`release-output` exactly (74 non-negotiable; M05).
- Escalate the V3 lattice reuse from `MOLT_BUILD_REUSE_COMPATIBLE=1` opt-in to
  **default-on for the iteration consumer** — 74 law 3 explicitly authorizes this
  "once proven safe per edge." The edge (release-output→dev-fast served) already
  has 8 edge tests (V3 CLAIMS row); add the one that the *default* iteration
  request now hits it.
- **Attest** `{profile=dev-fast, target_dir=stable, wall}` and gate the
  runtime-touch wall before/after on the 14-core box; gate that the shipped
  artifact byte-identity is unchanged (proof lane unaffected). Kill-switch:
  the existing `MOLT_RUNTIME_BUILD_PROFILE`/`MOLT_WASM_CARGO_PROFILE` still win.

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

### DO-4 (quick win) — dev wasm-opt at `-O1`
`wasm_opt_level` is already a knob but defaults to `Oz` (size, slow) even for
dev. Make the iteration/dev layout resolve `-O1` ("quick & useful opts, useful
for iteration builds" [10]); keep `Oz`/`O2`/`O3` for the shipped 3MB artifact.
Thread it through the same DX resolver that picks the runtime profile (DO-1) so
"dev" is one coherent choice. Low risk, no semantics change, no Windows caveat.

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
  cache ALL EXIST. The gap is **default-on + attestation + FRONTEND residency**,
  not new mechanisms. Recommending "create a dev-fast profile" or "add opt
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
- **DON'T run `wasm-release`/fat-LTO+cgu=1 on the iteration path.** That is a
  ship-size setting; on iteration it serializes 13 of 14 cores for zero iteration
  value (this is exactly what DO-1 removes).
- **DON'T ship dynamic-linked / side-module runtime to "avoid relink" without
  proving it beats DO-1 + a warm daemon.** Emscripten/WASI dynamic linking adds
  runtime overhead and is discouraged for best performance/size [24], and it
  touches the size-constrained SHIP path (3MB ceiling). Investigate only if a
  measured relink bottleneck survives DO-1 + DO-2.

## 5. The single highest-leverage lever

**DO-1 — default-on the already-built `dev-fast` iteration runtime profile + V3
lattice reuse.** It attacks the biggest single number (150–230s), reclaims 13
idle cores, is pure config-lattice reuse (74 law 3, already blessed and already
BUILT as V2/V3), its risk is Low (LTO/cgu never change semantics; ship/acceptance
still pin exact identity), and it is **S** effort with **no Windows caveat** — a
resolver default flip + an attestation gate, not new machinery. It is the literal
form of M07 ("cash instrumental tooling into outcomes") and 74 law 4 ("configured
≠ effective"). L2 (frontend daemon residency) is the larger *absolute* structural
ceiling, but L-effort/Med-risk; DO-1 is the best win-per-effort-per-risk and it
converts sunk, already-landed capability into a default-path win.

## 6. Surprising / liftable-wholesale findings

- **We are paying a SHIP setting on the DEV path — and the fix is already built.**
  The 150–230s runtime compile is `wasm-release` (fat LTO + cgu=1, opt-z, a 3MB-
  ceiling concern) charged to iteration, which neither ships nor cares about size.
  The `dev-fast` profile that fixes it (lto=off, cgu=256) is ALREADY in Cargo.toml
  and the reuse plumbing already landed (V2/V3) — it is merely opt-in. The single
  cheapest big win on the board is flipping it default-on (DO-1), not building
  anything.
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
  another reason the iteration runtime should NOT run under the ship profile
  (DO-1 gets share-generics back for free).
- **Zig is the existence proof for the daemon ceiling.** In-process incremental
  compilation took Zig ~36s cold → 228–288ms warm (~125×) [7]. That is the target
  class for DO-2 — a two-orders-of-magnitude structural change to
  module_graph/analysis, not a 2× tweak.

## 7. Landed-state verification (read this session; do not duplicate)

Verified against the worktree tree at `origin/main` HEAD `18ed35b063`:

- **Fast-iteration Cargo profiles — LANDED.** `Cargo.toml` `[profile.dev-fast]`
  `inherits="dev"`, `codegen-units=256`, `lto="off"`; `[profile.release-fast]`
  (backend-daemon iteration). Ship profiles `[profile.release-output]` and
  `[profile.wasm-release]` are `lto="fat"`, `codegen-units=1`, `opt-level="z"`.
- **Per-package opt-level layering — LANDED.** Extensive `[profile.dev.package.*]`
  and `[profile.dev-fast.package.*]` overrides (molt-backend=1, molt-runtime=2,
  cranelift-codegen=1, …) + hot-crate opt policy for the shipped runtime.
- **Config-lattice reuse + stable dep-cache — LANDED (opt-in).** V1
  (018d83e104/8bc067ee27 single combined compile), V2 (7e248d384b stable dep-cache
  default-on for iteration profiles), V3 (4644a2c4d1 `MOLT_BUILD_REUSE_COMPATIBLE`
  config-lattice reuse) per CLAIMS BUILD-DEDUP-B rows.
- **Runtime wasm profile resolver — LANDED.** `src/molt/cli/runtime_build.py`
  `_resolve_wasm_cargo_profile`: precedence = explicit `MOLT_WASM_CARGO_PROFILE`
  → `MOLT_RUNTIME_BUILD_PROFILE` (iteration knob, e.g. `dev-fast`) → default
  (`release`→`wasm-release`). The default resolves to the SHIP profile = the L1
  configured≠effective gap.
- **Backend compiler daemon + function cache — LANDED.** `runtime/molt-backend/
  src/backend_process/{job.rs,protocol.rs}` carry `function_cache_key`;
  `src/main_tests/{daemon_cache.rs,daemon_env.rs}` exercise the daemon.
- **wasm-opt level — parameterized, dev default `Oz`.** `wasm_opt_level` in
  `src/molt/cli/{backend_pipeline,backend_output_pipeline,build_output_layout,
  build_pipeline}.py` defaults `Oz`; `build_output_layout` carries per-layout
  levels (`Oz`/`O3`). The L4 gap = no `-O1` dev/iteration default.
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
