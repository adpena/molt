<!--
  Foundation blueprint — Arc: STARTUP / COLD-START
  ("cold-start is an ARTIFACT-FOOTPRINT / PAGE-IN / CODESIGN problem, NOT a
   runtime-init problem (runtime init measured 0.127ms)" — CLAUDE.md Performance
   Constitution; doc 51 §2 the "< 2 MB / < 50 ms" size+startup arc; W3/design 09).
  Author: portfolio-architect
  Date: 2026-06-24
  Status: DESIGN ONLY / EXECUTABLE PLAN (no code in this change; the lead integrates).
  Assigned number 62 (docs/design/foundation/62_startup_cold_start.md) was free.
-->

# 62 — Instant Cold Start: Make the Startup Tax a Function of Bytes-Paged, Not a Phantom

> **One-line thesis.** molt's cold start is already at the OS floor for a trivial
> program (≈18 ms same-path, *identical to a no-op C binary* — see
> `docs/perf/COLD_START.md`), and runtime-init is a measured 0.127 ms — so the
> startup arc's job is **not** to "make startup fast" (it is) but to make the
> *worst-case first launch* and the *page-cold first launch* **inevitably bounded
> and provably below CPython**, across every backend and profile, *as the stdlib
> and artifact grow*. Today two molt-controllable taxes leak through unmanaged:
> (1) **the ~70 ms macOS first-launch code-signature tax is paid by every freshly
> built user binary because molt never ad-hoc-signs the user artifact at build
> time** (it signs only the daemon binary + BOLT output); and (2) **page-in of the
> linked image is unordered** — `bolt.rs::generate_order_file` is a *stub*, so the
> ld64 `-order_file` / section-ordering lever that turns "fault N scattered pages"
> into "fault 1 hot run of pages" is unbuilt. This arc retires the **CLASS** of
> "cold-start surprises that scale with artifact growth" by (a) **paying the
> one-time costs at build/install time, never at first launch**, (b) **ordering the
> artifact so the cold path touches a contiguous hot prefix**, and (c) making both
> a **gated, measured fact** in the 64 cold-start board — so a cold regression is
> *unmergeable*, not discovered by a user.

---

## 0. The end-state outcome (stated crisply)

**In five years, molt's first launch of a freshly produced artifact beats
CPython's interpreter startup on every target, and the only remaining cold cost
is a tiny, bounded, *measured* page-in of a hot prefix — with zero one-time taxes
leaking onto the user's first run.** Concretely, the steady state is:

1. **No first-launch codesign surprise (Darwin).** Every native artifact molt
   emits is **ad-hoc code-signed at build time** (the same `codesign -f -s -` molt
   already runs for the daemon binary). The user's first launch of a just-built /
   just-downloaded molt binary pays **0 ms** of Gatekeeper/code-signature
   validation that a warm launch wouldn't — the ~70 ms `macos-codesign-first-launch`
   component (isolated in `cold_start_decomposition.json`) is **moved to build
   time** where it belongs, and is *reported as a build-step cost*, not a runtime
   tax. (When a real Developer-ID identity is configured via the existing
   `_codesign_sign` path, that is used instead; ad-hoc is the floor.)

2. **Page-in is a contiguous hot prefix, not a scatter.** The shipped artifact's
   hot startup path (entry → `molt_runtime_init`'s 12 phases → the entry module's
   top-level code) is laid out **first and contiguous** in `__TEXT`, via a
   **derived `-order_file`** (Darwin) / **section-ordering** (`--gc-sections` +
   ordered input on ELF). A genuinely page-cold first launch faults a *short run*
   of adjacent pages, so the molt-controllable `binary-page-in` component stays
   **sub-millisecond and bounded as the binary grows** instead of scaling with
   total linked size.

3. **The cold path is a *budget*, and the budget *ratchets down*.**
   `cold_start_budget.json` (today: `native/release-fast` 380 ms first-run ceiling;
   `native/release-output` budget unseeded; Y1 target `startup_tax < 100 ms`)
   becomes a **seeded, gated, monotonically-decreasing** ceiling for *every*
   (backend, profile) — including `release-output` (the shipped artifact) and a new
   **`first-launch` budget** distinct from the `same-path` budget, so the codesign
   fix is *measured as a win* and protected from regression.

4. **WASM/edge cold start is streaming + cached, not eager-from-buffer.** The WASM
   launch path uses **`WebAssembly.compileStreaming` / `instantiateStreaming`** and
   a **compiled-`Module` cache** (the Cloudflare/`run_wasm.js` paths today use eager
   `new WebAssembly.Module(buffer)` / `WebAssembly.instantiate(buffer)` — they
   block on full download+compile). The `< 2 MB` artifact target (doc 51 §2)
   composes here: a smaller module compiles faster and streams sooner.

5. **The dead lever stays dead, on the record.** Runtime-init (0.127 ms) and
   per-module eager init (≈0 ms) are **three orders of magnitude** below the OS
   floor; this arc **does not** add a startup heap-snapshot / AOT-init-snapshot /
   module-init deferral mechanism (that would be a workaround chasing a non-problem
   — `molt-snapshot` is for *WASM execution pause/resume across machines*, not
   startup). The arc *guards* runtime-init against regression with a micro-budget,
   and otherwise leaves it alone.

**The class this arc retires:** **"artifact-growth cold-start surprise"** — the
family of "the binary/stdlib got bigger / the artifact was freshly produced, and
the first launch silently got slower (codesign, scattered page faults, eager WASM
compile) and nobody noticed until a user / an edge cold-invoke did." After this
arc, that family is **unexpressible on a shipped artifact**: the one-time taxes are
pre-paid at build, the page-in is ordered, and the residual is a gated budget that
ratchets down with the binary-size arc.

---

## 1. What already exists (cite-and-compose; do NOT duplicate)

This arc is **~70% composition** of existing, high-quality measurement + build
machinery. The investigation (verified against the tree, 2026-06-24) found the
*diagnosis* is done and excellent; the gap is **paying the costs at the right time,
ordering the artifact, and gating the result** — not re-measuring.

| Asset | Path | What it already does | Gap this arc fills |
|---|---|---|---|
| **Cold-start decomposition** | `tools/cold_start_decompose.py` (788 ln) + `docs/perf/COLD_START.md` | Decomposes the same-path tax into `process-launch/dyld` (18 ms OS floor), `binary-page-in` (~0 same-path, size-driven), `molt-runtime-init` (0.127 ms), `module-init` (≈0); isolates `macos-codesign-first-launch` (~70 ms) on the no-op C as **one-time**, never summed; FRESH vs SAME path modes; `MOLT_TRACE_RUNTIME_INIT` ladder; `DYLD_PRINT_STATISTICS` cross-check | Measures but does **not** *fix*: no build-time codesign, no order-file, no first-launch budget. The "highest-leverage = binary-page-in (size-driven)" pick names the lever; this arc *builds* it |
| **Cold-start budget board** | `bench/scoreboard/cold_start_budget.json` | Per-(backend,profile) `startup_tax_ms` CEILING; `native/release-fast` 380 ms, `llvm/release-fast` 340 ms (v0 = measured baseline); notes release-output unseeded + Y1 `< 100 ms` | No `first-launch` budget distinct from `same-path`; release-output unseeded; not ratcheted; not split codesign-out |
| **Decomposition data** | `bench/scoreboard/cold_start_decomposition.json` | The measured per-component table (codesign 70 ms one-time, dyld 18 ms, page-in ~0, init 0.127 ms) seeding the budget | The before/after fixture for the codesign + order-file wins |
| **Native ad-hoc codesign** | `src/molt/cli/native_toolchain.py` `_codesign_binary` (`codesign -f -s -`) | Ad-hoc signs a Mach-O; called for the **daemon binary** (`__init__.py:27484`) and **BOLT output** (`_atomic_copy_file(codesign=True)`) | **NOT called for the user binary** after `_post_link_strip(output_binary)` (`__init__.py:20842`) — the load-bearing gap (§3.1) |
| **Real-identity codesign** | `src/molt/cli/__init__.py` `_codesign_sign` / `_codesign_identity_info` (lines ~3750–3805) | `codesign -s <identity>` + display/verify for a configured Developer-ID | The "preferred over ad-hoc when configured" branch the build-time signer dispatches to (§3.1) |
| **Native link driver** | `src/molt/cli/__init__.py` `_build_native_link_driver_command` / `_finalize_native_link` (lines ~20156–20285) | ld64 `-dead_strip` + `-exported_symbols_list` (Darwin); `--gc-sections` + `--version-script` (ELF); `-x -S` + `strip -x` post-link; `-Wl,-O2`; `/OPT:REF` (Windows) | No `-order_file` (Darwin) / ordered-section (ELF) for page-in locality; no `__TEXT`/`__DATA` hot-prefix grouping (§3.2) |
| **Order-file generator (STUB)** | `runtime/molt-tir/src/tir/bolt.rs` `generate_order_file` (lines 124–136) | Writes a *placeholder* order file ("# Add function symbols in hot-to-cold order"); BOLT/`perf2bolt` (Linux) + Instruments (macOS) scaffolding exists | **The stub never emits real symbols** — this arc derives the startup-hot symbol order and feeds it to the linker (§3.2) |
| **BOLT post-link** | `tools/bolt_optimize.sh` + `native_toolchain.py::_run_bolt_post_link` (`--bolt`) | Optional BOLT reordering of an existing binary (re-codesigns via `_atomic_copy_file(codesign=True)`) | The *opt-in heavyweight* path; this arc adds the *always-on lightweight* static startup order (§3.2) and reuses BOLT's reorder as the heavy tier |
| **Binary-size audit** | `tools/binary_size_analysis.py`, `tools/output_startup_size_audit.py` (fresh-path aware), `tools/wasm_size_audit.py` | Section/symbol size attribution; fresh-path startup shape; WASM raw/gzip/brotli | The size arc's instruments — this arc *consumes* their output (smaller image ⇒ less page-in) and feeds the convergence (§6) |
| **Runtime-init trace** | `runtime/molt-runtime/src/state/runtime_state.rs` `molt_runtime_init` + `trace_runtime_init` (lines 668–818) | The 12-phase `MOLT_TRACE_RUNTIME_INIT` ladder (0.127 ms total); eager capability load (security-required, not deferrable) | No micro-budget guard so a future phase can't silently regress init (§3.4); confirms NO snapshot is warranted |
| **WASM launch (host/JS)** | `wasm/run_wasm.js` (`new WebAssembly.Module(buffer)` ~4552, `WebAssembly.instantiate(runtimeBuffer/wasmBuffer)` ~5584/5612); `deploy/cloudflare/worker.js` (serves 13.4 MB `falcon-ocr.wasm`) | Eager compile + instantiate from a fully-downloaded buffer | No `compileStreaming`/`instantiateStreaming`; no compiled-`Module` cache across cold invokes (§3.3) |
| **Cargo ship profiles** | `Cargo.toml` `[profile.release-output]` (opt-`z`, `lto="fat"`, `codegen-units=1`, `panic="abort"`, `strip=true`) | Already size-optimal for the shipped runtime | The page-in lever is *post-compile layout*, not a profile change — release-output is already correct (§3.2) |
| **Cold-start measurement core (53)** | `tools/perf_scoreboard.py` `startup_tax_ms` / `cold_*` fields, `FAIL_COLD_BUDGET`/`WARN_COLD_FLOOR` verdicts | The board that *records* cold + warm per cell (schema v3) | Arc 64 gates it in CI; **this arc supplies the new `first-launch` dimension + the codesign-isolated cell + the order-file before/after** that 53's board projects |

**North-star alignment.** Doc 51 §2 names the matrix dimension ("4 profiles × 5
dimensions: warm, **cold #62**, RSS, size <2 MB, compile") and §3 the Y3 deliverable
("the `< 2 MB / < 50 ms` size+startup arc (W3, design 09)"). `docs/perf/COLD_START.md`
*explicitly redirects #62* to "the binary-size / tree-shaking / artifact-layout arc,
NOT a `molt_runtime_init` deferral." **This arc is the executable realization of that
redirect**: it builds the two artifact-layer levers (build-time codesign + page-in
ordering) the decomposition named, and the budget/board that makes them stick.

> **Refusal recorded (deletes a bad plan).** The naive plan — "molt cold start is
> slow, add a startup snapshot / lazy-init the runtime / defer module init" — is
> **REJECTED with measured evidence**. `cold_start_decomposition.json`:
> `molt-runtime-init` = **0.127 ms** (full 12-phase ladder), `module-init` ≈ **0 ms**,
> `binary-page-in` ≈ **0 ms same-path**. A snapshot/lazy-init mechanism would add a
> serialization format, an invalidation problem, and a security surface (the eager
> capability load at `runtime_state.rs:808–813` is a *deliberate* anti-privilege-
> escalation measure — deferring it is a security regression) to "optimize" a
> 0.127 ms cost three orders of magnitude below the 18 ms OS floor. That is the
> compound-interest-of-bugs trap CLAUDE.md forbids. The structurally correct levers
> are **artifact-layer** (pre-pay codesign, order page-in, shrink the image), not
> **runtime-layer**. This is the load-bearing architectural decision.

---

## 2. Time-traveler derivation (end-state → required structural facts)

Work backward from the §0 end-state to the mechanisms that make it inevitable.

- **END:** "no first-launch codesign surprise on Darwin."
  → **requires** the user artifact to carry a **valid code signature before it
     leaves the build** (ad-hoc at minimum; real identity when configured).
  → **requires** the build pipeline to *invoke the signer on the final user binary*
     at the one place it is finalized (`_finalize_native_link`, after
     `_post_link_strip`), not only on the daemon/BOLT artifacts.
     **FACT NEEDED:** a single **`sign_native_artifact(binary, profile, target)`**
     authority that (a) is the *only* place native signing happens, (b) chooses
     real-identity (`_codesign_sign`) over ad-hoc (`_codesign_binary`) when an
     identity is configured, (c) is a *build step with its own timed cost*, and
     (d) is a no-op off-Darwin. The existing two signers become its *backends*, not
     two independent call sites (retires "signing scattered across call sites").

- **END:** "page-in is a contiguous hot prefix, not a scatter."
  → **requires** the linker to place the **startup-hot symbols first and adjacent**.
  → **requires** a *derived* hot-symbol order (entry, `molt_runtime_init` + its
     callee chain, the entry module's top-level function), not the stub's
     placeholder. **FACT NEEDED:** a **`StartupOrder`** fact — a deterministic,
     statically-derivable ordered list of the symbols on the cold path (the *static*
     tier, always on, zero profiling needed) — emitted by the backend and consumed
     by the linker via `-order_file` (Darwin) / ordered `.text.*` sections + a
     linker order map (ELF). A *profile-refined* tier (Instruments/`perf2bolt`,
     reusing `bolt.rs`) is the heavy opt-in that *sharpens* the same fact.

- **END:** "the cold path is a budget that ratchets down, including release-output."
  → **requires** the budget board to distinguish **first-launch** (page-cold +
     codesign-if-unsigned) from **same-path** (warm-signature) — today
     `cold_start_budget.json` has only the first-run number, conflating them.
     **FACT NEEDED:** a **two-axis cold budget** `{first_launch_ms, same_path_ms}`
     per (backend, profile), with `release-output` seeded, and a *ratchet rule* (the
     budget may only decrease) wired into the 64 gate.

- **END:** "WASM/edge cold start is streaming + cached."
  → **requires** the JS/host launch path to **overlap download with compile**
     (`compileStreaming`) and **cache the compiled `Module`** across cold invokes.
     **FACT NEEDED:** a **`WasmColdLaunch`** contract in the host glue
     (`run_wasm.js` + the deploy templates) — streaming-first with a
     buffer-instantiate fallback (Node/embedded where streaming is absent), and a
     module cache keyed by artifact hash.

- **END:** "the dead lever stays dead, on the record."
  → **requires** a **runtime-init micro-budget** so a future init phase that
     regresses the 0.127 ms floor is *caught*, *without* inviting a snapshot.
     **FACT NEEDED:** an `init_budget_us` cell in the board (e.g. 500 µs ceiling,
     ~4× headroom) fed by the `MOLT_TRACE_RUNTIME_INIT` ladder.

The dependency spine (what must exist before what):

```
Phase 0  pin the cold-start contract: the two-axis budget schema + the StartupOrder
   │      fact shape + this doc (no behavior change)
   │
Phase 1  BUILD-TIME CODESIGN of the user artifact (the single highest-leverage,
   │      lowest-risk win — moves ~70 ms off first launch). Independently landable.
   │
   ├── Phase 2  STATIC StartupOrder + linker order-file wiring (page-in locality,
   │             always-on, zero-profiling tier). DEPENDS on nothing in 1.
   │
   ├── Phase 3  TWO-AXIS cold budget board + 64 gate wiring (first-launch vs
   │             same-path; release-output seeded; ratchet). DEPENDS on 1 (so the
   │             codesign win is measured) — composes with arc 64.
   │
   ├── Phase 4  WASM/edge streaming + module cache (compileStreaming + cache).
   │             Independent of 1/2 (different artifact + host).
   │
   ├── Phase 5  PROFILE-REFINED StartupOrder (Instruments/perf2bolt via bolt.rs) —
   │             the heavy opt-in tier that sharpens Phase 2's static order.
   │             DEPENDS on 2 (the order-file plumbing) + existing BOLT.
   │
   └── Phase 6  runtime-init micro-budget guard (anti-regression for the dead
                 lever). Independent; tiny.
```

Phases 1, 2, 4, 6 are **parallelizable** (non-overlapping files; see §7). Phase 3
depends on 1; Phase 5 depends on 2. Phase 1 is the fastest, highest-leverage start.

---

## 3. The structural facts / mechanisms to build (each tied to the class it retires)

### 3.1 FACT: `sign_native_artifact` — one signing authority, paid at build time — retires "first-launch codesign tax"

**The waste class:** every freshly built/copied/downloaded molt native binary on
macOS pays ~70 ms of Gatekeeper/code-signature validation on its *first* launch
because it ships **unsigned** (the build strips it and emits it without signing).
The decomposition isolated this as `macos-codesign-first-launch` and correctly
labeled it "one-time/install" — but molt currently makes the *user's first run* the
install event instead of the *build*.

**The mechanism.** A single authority, the *only* place native signing happens:

```
sign_native_artifact(binary: Path, *, target_triple, profile,
                     identity: str | None) -> SignResult
  # Darwin only (no-op elsewhere, returns SignResult(skipped, reason="not-darwin")).
  # 1. identity resolved from (explicit arg) > MOLT_CODESIGN_IDENTITY env >
  #    project config > None.
  # 2. if identity: _codesign_sign(binary, identity)   # real Developer-ID
  #    else:         _codesign_binary(binary)           # ad-hoc '-' (the floor)
  # 3. returns {signed: bool, identity: str|"adhoc"|None, elapsed_ms, verified: bool}
  #    verified via _codesign_identity_info (the existing display/verify probe).
```

- **Call site (the load-bearing one):** in `_finalize_native_link`, immediately
  after `_post_link_strip(output_binary, target_triple)` (`__init__.py:20842`) and
  *before* the success JSON is emitted, for `emit_mode == "bin"` on Darwin. The
  signing cost is captured into the build-success payload (`build_native_link_success_data`)
  as a `codesign` sub-object (`{signed, identity, elapsed_ms}`), so it is reported
  as a **build step**, never a runtime number.
- **The existing two signers become backends, not call sites.** `_codesign_binary`
  (ad-hoc) and `_codesign_sign` (identity) are reachable *only* through
  `sign_native_artifact`. The daemon-binary site (`__init__.py:27484`) and the BOLT
  site (`_atomic_copy_file(codesign=True)`) route through it too (asymmetric-coverage
  rule: one authority, all artifacts). `_atomic_copy_file(codesign=True)` calls
  `sign_native_artifact` with the destination's target inferred (BOLT re-sign after
  reorder is preserved).
- **Determinism / reproducibility:** ad-hoc signing is deterministic for a fixed
  binary (the signature is content-derived); the build-success payload records it so
  the `link_fingerprint` accounts for the signed bytes (a signed and unsigned binary
  differ — the fingerprint must cover the *signed* artifact, computed after signing).
- **Cross-target:** when cross-building `*-apple-darwin` from a non-Darwin host,
  `codesign` is unavailable; `sign_native_artifact` returns `skipped` with
  `reason="cross-host-no-codesign"` and the build emits a **warning** (the user must
  sign on a Mac or the first launch will pay the tax) — surfaced, never silent.

**Why this is structural, not a band-aid.** The wrong fix is "sign in three places
with copy-pasted `codesign` calls." The right fix is *one authority* that owns the
signing policy (ad-hoc vs identity), is exhaustively the only signer, and reports
the cost where it is incurred (build). Adding a new artifact kind (e.g. a future
universal binary) means routing it through the one authority, not adding a fourth
call site. **Pythonista-Rustacean:** the Pythonista sees a binary that "just runs
fast the first time" (no Gatekeeper spinner); the Rustacean sees one signing
authority with a typed `SignResult`, no scattered side-effecting calls.

**Backends/profiles:** Darwin native, all profiles (dev-fast/release-fast/
release-output) — the user binary is signed regardless of profile. ELF/Windows:
no-op (no equivalent first-launch validation tax; Windows SmartScreen is a
*download*-reputation gate, not a per-binary validation molt can pre-pay — noted as
out of scope, recorded). WASM/Luau: N/A (not Mach-O).

### 3.2 FACT: `StartupOrder` + linker ordering — retires "scattered page-in scaling with binary size"

**The waste class:** the molt-controllable cold component is `binary-page-in` — the
pages the OS must fault in before the first user instruction. Today the cold path's
symbols (entry, the `molt_runtime_init` callee chain, the entry module's top-level
code) are scattered across `__TEXT` wherever the linker placed them, so a page-cold
launch faults *many non-adjacent pages*. This cost **scales with total linked size**
(more scatter as the stdlib grows) instead of with the *hot path's* size.

**The mechanism.** A two-tier ordered placement of the startup-hot symbols:

```
StartupOrder (a derived backend fact, emitted alongside the artifact):
  ordered list of symbol names on the cold path, hot-first:
    [ _main / entry,
      molt_runtime_init  (+ its static callee chain: runtime_reset_for_init,
                           register_intrinsics_module, init_vtable×N,
                           molt_runtime_init_{resources,audit,io_mode},
                           is_trusted, has_capability),
      <entry-module top-level function symbol(s)>,
      molt_runtime_exit (teardown is on the cold path too, at the tail) ]
```

- **Static tier (always on, zero profiling — Phase 2).** The backend already knows
  the entry symbol and the runtime-init entry; the callee chain is a *statically
  fixed* set of `#[no_mangle]` runtime symbols (enumerable from `runtime_state.rs`).
  The entry-module top-level symbol is known from codegen. The backend writes a
  `<artifact>.startup_order` file; the CLI link step consumes it:
  - **Darwin (ld64/lld):** `-Wl,-order_file,<path>` placed alongside `-dead_strip`
    + `-exported_symbols_list` in `_build_native_link_driver_command`. ld64 honors
    the order file to lay the named symbols first in `__TEXT,__text`. (Note: this
    requires the named local symbols to survive long enough for the linker to order
    them; the order file is applied at link, *before* the `-x -S` strip — strip
    removes the *names* afterward, the *layout* persists.)
  - **ELF (GNU ld/lld):** the backend already emits `per_function_section(true)`
    (`simple_backend.rs:2606`) → one `.text.<sym>` per function. Add a generated
    **linker section-ordering** (a `--section-ordering-file` for lld, or a
    `SECTIONS`/`INSERT` fragment listing the hot `.text.*` first) so `--gc-sections`
    keeps them adjacent at the image head.
  - **Windows:** `/ORDER:@<path>` (MSVC link) — the COFF analogue; lower priority
    (Windows cold-start is download-gated, §3.1).
- **Profile-refined tier (opt-in, Phase 5).** Reuse `bolt.rs::generate_order_file`
  (today a stub): an Instruments (`xctrace`)/`perf2bolt` startup profile of the
  *actual* cold path refines the static order (catches the real callee order through
  intrinsic dispatch). This is the `--bolt`-class heavyweight; the static tier is the
  always-on floor it sharpens. **The stub is completed here**, not left as debt.
- **`__DATA`/`__const` cold prefix.** The same order applies to the data the cold
  path touches first (the intrinsic function-pointer table `resolve_symbol`, the
  capability/audit statics) — grouped so the cold path's data reads hit a contiguous
  run. (Lower leverage than text; included for completeness, gated by measurement.)

**Why this is structural, not a band-aid.** The wrong fix is "hope the linker
happens to place hot code together" or "turn on BOLT and call it done." The right
fix is a *derived `StartupOrder` fact* — the compiler *knows* the cold path
statically, so it *states* it, and the linker *honors* it on every backend. As the
stdlib grows, the hot prefix stays the same size; only the cold tail grows, and the
cold tail is never faulted at startup. This is the compression-ladder move: **the
fact (StartupOrder) makes "page-in scaling with total size" unexpressible** — page-in
now scales with the *hot path*, which is bounded. **Pythonista-Rustacean:** the
Pythonista gets a binary whose first run touches a tight working set (fast cold
start that doesn't degrade as they `import` more stdlib); the Rustacean gets a
generated, one-authority ordering fact, exhaustive over the cold-path symbol set,
honored identically across linkers.

**Backends/profiles:** native (ld64 + ELF + COFF), all profiles. The order is a
*layout* fact — orthogonal to opt-level/LTO, so `release-output` (fat-LTO, cgu=1)
benefits without a profile change. WASM: N/A in the same form (WASM has no demand
paging of an image; its analogue is §3.3 streaming). Luau: N/A (source, no image).

### 3.3 FACT: `WasmColdLaunch` — streaming compile + module cache — retires "block-on-download-then-compile" cold start

**The waste class:** the WASM launch paths (`run_wasm.js`, the Cloudflare worker
templates) **download the full `.wasm` into a buffer, then compile, then
instantiate** — three serial phases. For a 13.4 MB module (the OCR worker) this is a
multi-hundred-ms cold invoke where download and compile could overlap, and where
every cold invoke recompiles from scratch.

**The mechanism.** A `WasmColdLaunch` contract in the host glue:

- **Streaming-first:** `WebAssembly.compileStreaming(fetch(url))` /
  `instantiateStreaming(fetch(url), imports)` — overlaps network download with
  compilation. Fallback to `compile(buffer)` only where streaming is unavailable
  (Node without the fetch shim, embedded hosts) — detected, not assumed.
- **Compiled-`Module` cache:** keep the compiled `WebAssembly.Module` (not just the
  bytes) across cold invokes, keyed by artifact hash. On Cloudflare Workers the
  module is already compiled once per isolate; the contract makes the *runtime* +
  *output* modules (today instantiated separately at `run_wasm.js:5584`/`5612`)
  cache their compiled form so a warm isolate skips recompile. In the browser, an
  `IndexedDB`-backed `Module` cache (where `structuredClone` of a `Module` is
  supported) skips recompile across page loads.
- **Size convergence:** the `< 2 MB` target (doc 51 §2; the binary-size arc) is the
  multiplier here — streaming a 2 MB module beats streaming a 13 MB one on the same
  link. `wasm-opt -Oz` + `wasm-tools strip` (already in the size spec) feed this.

**Why this is structural, not a band-aid.** The wrong fix is "call instantiate
faster." The right fix is *overlap the phases and cache the expensive one* — a
launch *contract* the deploy templates and the local runner share, so a new
deploy target inherits streaming+cache instead of re-implementing eager-buffer
launch. **Pythonista-Rustacean:** the Pythonista deploys to the edge and the first
request is fast without tuning; the Rustacean sees a single launch contract,
streaming-by-default with an explicit, detected fallback.

**Backends/profiles:** WASM target, all WASM hosts (browser, Cloudflare, Node).
Native/Luau: N/A.

### 3.4 FACT: `init_budget_us` — runtime-init micro-budget — retires "silent init regression invites a snapshot"

**The waste class:** the *risk* that a future init phase (a new vtable, a new eager
load) silently pushes `molt_runtime_init` from 0.127 ms toward the OS floor — and
that the regression, undetected, *motivates* someone to add a startup snapshot
(the rejected lever). A guard makes the dead lever *stay* dead.

**The mechanism.** A `init_budget_us` cell (e.g. **500 µs**, ~4× the 0.127 ms
measured baseline) measured from the existing `MOLT_TRACE_RUNTIME_INIT` ladder
(parsed by `cold_start_decompose.py`'s `_measure_runtime_init_phases`) and gated in
the cold-start board (§3.5 / arc 64). A breach is a `WARN` (init is not the cold
bottleneck) routed to the runtime lane — *not* a license to snapshot.

**Why this is structural.** It encodes the COLD_START.md adjudication as an
*enforced* invariant: "runtime-init is solved; keep it solved." The guard is the
authority that says "do not add a snapshot" with a number.

### 3.5 FACT: two-axis cold budget — first-launch vs same-path — retires "the codesign/page-in win is invisible"

**The waste class:** today `cold_start_budget.json` has a *single* first-run number
per (backend, profile) that **conflates** the one-time codesign tax with the
page-cold page-in. So the Phase-1 codesign fix (moving ~70 ms off first launch) and
the Phase-2 page-in fix (ordering) *cannot be distinguished or protected* — a future
regression in one could hide under the other's headroom.

**The mechanism.** Split the budget into two gated axes per (backend, profile):

```
cold_start_budget.json (schema_version 2):
  budgets[<backend>/<profile>] = {
    first_launch_ms: <ceiling for a freshly-produced, page-cold launch>,
    same_path_ms:    <ceiling for the realistic repeated cold (signature cached)>,
    codesign_at_build: bool,   # Phase 1: true ⇒ first_launch excludes codesign
    ratchet: "monotone-decreasing",
  }
```

- **Seed `release-output`** (today unseeded) from a release-output board run.
- **`first_launch_ms` after Phase 1** is measured on a *build-time-signed* artifact
  — so it *excludes* codesign by construction (codesign moved to build). The win is
  the drop from the old first-run number (~207–341 ms p50–max) toward the dyld+page-in
  floor.
- **Ratchet:** the gate (arc 64) rejects any increase; the budget only ratchets down
  as the binary-size arc shrinks page-in.

**Why this is structural.** It makes the two distinct cold taxes *separately
measurable and separately gated*, so each lever's win is real and protected. This is
the §0(3) "budget that ratchets down" made concrete, and the hand-off surface to
arc 64.

---

## 4. Concrete phases (dependency order; each independently landable with green gates)

> Build/test discipline for every phase (CLAUDE.md): `export MOLT_SESSION_ID=cold-<phase>`
> and `CARGO_TARGET_DIR="$PWD/target/sessions/$MOLT_SESSION_ID"` before any build;
> route every raw-binary launch through `tools/safe_run.py --rss-mb <cap> --timeout <s>`
> (cold-start measurement *launches binaries* — this is mandatory); never `cargo clean`;
> max 2 build-triggering agents. Cold-start numbers are **macOS-primary** (codesign is a
> Darwin tax); the order-file/page-in lever is measured on **both** Darwin and Linux.

### Phase 0 — Pin the cold-start contract (this doc + schema)

**Deliverable:** bump `bench/scoreboard/cold_start_budget.json` to `schema_version 2`
with the two-axis shape (§3.5), seeded from the existing
`cold_start_decomposition.json` measured components (the same-path and first-run
numbers already present); add a `StartupOrder` fact description to
`docs/perf/COLD_START.md` (the symbol-order shape, §3.2). **No behavior change** —
schema + doc only.

**Gates:** `python3 -c "import json; json.load(open('bench/scoreboard/cold_start_budget.json'))"`;
a `tests/tools/test_cold_start_budget_schema.py` round-trips the v2 budget and
rejects a single-axis (v1) mutant; `tools/cold_start_decompose.py` still runs
unchanged (it reads the decomposition, not the budget).

**Independently valuable:** yes — the two-axis budget is the measurement frame the
other phases land against.

### Phase 1 — Build-time codesign of the user artifact (highest leverage, lowest risk)

**Deliverable:** `sign_native_artifact` (§3.1) in `src/molt/cli/native_toolchain.py`;
call it in `_finalize_native_link` after `_post_link_strip` for `emit_mode == "bin"`
on Darwin; route the daemon-binary + BOLT signing through it; record the
`{signed, identity, elapsed_ms}` in the build-success JSON; compute `link_fingerprint`
over the *signed* artifact. Cross-host-no-codesign emits a warning.

**Gates:**
- **Functional:** build a `print()`-only probe on macOS; assert
  `codesign --verify <binary>` passes (the artifact is signed at build); assert the
  build JSON carries `codesign.signed == true`.
- **The win (falsifiable):** run `tools/cold_start_decompose.py --profile release-output`
  on a *build-time-signed* artifact; assert the realistic first-launch *fresh-path*
  minimal total drops materially toward the no-op C fresh-path baseline (the ~70 ms
  codesign component is no longer paid at launch — it moved to build). Record the new
  number into `cold_start_budget.json` `first_launch_ms` for `native/release-output`.
- **No-regression:** warm `bench_sum`/`bench_fib` unchanged (signing is build-time,
  zero runtime cost); `_assert_native_binary_valid` still passes (signing preserves
  the Mach-O); off-Darwin builds unchanged (no-op).
- **Discipline:** every probe launch via `safe_run.py`.

**Independently valuable:** yes — this alone moves ~70 ms off every macOS user's
first run. **Lowest risk** (build-time, additive, Darwin-gated).

### Phase 2 — Static `StartupOrder` + linker order-file wiring (page-in locality)

**Deliverable:** the backend emits `<artifact>.startup_order` (the static hot-symbol
list, §3.2) — a small additive emit in the native backend
(`simple_backend.rs` / `function_compiler`), gated so it is always produced for
`emit_mode == "bin"`; the CLI link step (`_build_native_link_driver_command`)
consumes it: `-Wl,-order_file,<path>` (Darwin), `--section-ordering-file`/ordered
`.text.*` (ELF lld), `/ORDER:@<path>` (Windows). The order file is applied *before*
the existing `-x -S` strip (layout persists; names stripped after).

**Gates:**
- **Functional:** the order file lists `_main` + the runtime-init chain + the entry
  module symbol; `nm`/`otool -l` on the *unstripped* (`MOLT_KEEP_SYMBOLS=1`) binary
  confirms the hot symbols are at the head of `__TEXT,__text` (Darwin) / image head
  (ELF). Build validity (`_assert_native_binary_valid`) unchanged.
- **The win (page-cold, falsifiable):** measure a *genuinely page-cold* first launch
  (the fresh-path mode, which defeats the page cache) before vs after ordering, on a
  binary large enough to span multiple `__TEXT` pages (a real benchmark, not the
  4.26 MiB minimal — e.g. `bench_etl_orders`); assert the page-in residual drops.
  Cross-check with `DYLD_PRINT_STATISTICS`. On a tiny binary the win is ~0 (already
  one page) — that is expected and recorded, not a failure.
- **No-regression:** warm perf unchanged (layout is cold-path only); CPython floor
  unaffected; the order file is additive (a build without it links as today).
- **Both backends:** measured on Darwin (ld64) *and* Linux (lld) — a native win must
  not leave ELF unordered (Performance Constitution: backend parity).

**Independently valuable:** yes — bounds page-in as the binary grows, even before
profiling refines it.

### Phase 3 — Two-axis cold budget board + 64 gate wiring (compose with arc 64)

**Deliverable:** seed `first_launch_ms` (post-Phase-1, signed) and `same_path_ms`
for **every** (backend, profile) including `release-output`; add the `init_budget_us`
cell (§3.4); wire the two-axis budget + ratchet rule into the **64 cold-start
projection** (`perf_scoreboard.py`'s `FAIL_COLD_BUDGET`/`WARN_COLD_FLOOR` already
consume `startup_tax_ms` — extend to consume `first_launch_ms` vs `same_path_ms`
distinctly, and the ratchet). This is the **hand-off to arc 64**: arc 62 supplies
the dimension + seeded budgets; arc 64 gates them in CI.

**Gates:** the 64 self-test accepts the v2 budget; a synthetic cell exceeding
`first_launch_ms` is `FAIL_COLD_BUDGET` while a same-path-only regression is routed
to `same_path_ms`; the ratchet rejects a budget *increase* in a test fixture; the
`init_budget_us` breach is `WARN` (not `FAIL`). **Coordinate with arc 64** (it owns
the gate wiring; this phase supplies the budget semantics) — file overlap is the
`cold_start_budget.json` data + the budget-consumption logic in `perf_scoreboard.py`
(serialize through arc 64's owner).

**Independently valuable:** yes — makes the Phase-1/2 wins gated and ratcheting.

### Phase 4 — WASM/edge streaming compile + module cache

**Deliverable:** `WasmColdLaunch` (§3.3) in `wasm/run_wasm.js` (streaming-first with
buffer fallback for the runtime + output modules at ~5584/5612, replacing eager
`new WebAssembly.Module(buffer)`/`instantiate(buffer)`); a compiled-`Module` cache
keyed by artifact hash; mirror in the `deploy/cloudflare/*` + `deploy/browser/*`
templates (streaming `fetch` + per-isolate/IndexedDB module cache).

**Gates:** a Node/`run_wasm.js` smoke runs a `print()` WASM probe via streaming
(asserts `compileStreaming` taken when available, fallback when not, via a logged
mode marker); a cold-vs-warm timing on the OCR-class module shows the warm
(cached-`Module`) invoke skips recompile. WASM output parity unchanged (same module,
different load path). Size cross-check: `wasm_size_audit.py` numbers unchanged
(launch path doesn't change bytes).

**Independently valuable:** yes — edge/browser cold invoke is a distinct, important
cold-start surface (the deploy story).

### Phase 5 — Profile-refined `StartupOrder` (complete the `bolt.rs` stub)

**Deliverable:** complete `runtime/molt-tir/src/tir/bolt.rs::generate_order_file`
(today a stub) to emit a *real* startup-hot order from an Instruments
(`xctrace record --template 'Time Profiler'`)/`perf2bolt` cold-path profile,
refining Phase 2's static order; wire it behind the existing `--bolt`-class opt-in
so the heavy tier sharpens the always-on static tier.

**Gates:** with profiling tools absent, `generate_order_file` returns a clear
error / falls back to the static order (no crash — mirrors the existing
`test_generate_order_file`); with a profile present, the refined order is a superset
ordering of the static hot symbols (the static floor is never *worse*). The
profile-refined page-cold first launch is ≤ the static-order first launch on the
large benchmark.

**Independently valuable:** yes — the opt-in sharpening for ship artifacts; **closes
the stub** (no debt left).

### Phase 6 — runtime-init micro-budget guard (anti-regression for the dead lever)

**Deliverable:** the `init_budget_us` board cell (§3.4) wired to the
`MOLT_TRACE_RUNTIME_INIT` ladder parse (reuse `cold_start_decompose.py`'s parser); a
test that fails if total init exceeds 500 µs. Document in COLD_START.md: "init is
guarded; do not add a startup snapshot — the lever is dead, here is the number."

**Gates:** the guard passes at the 0.127 ms baseline; a fixture that injects a
synthetic 1 ms init phase trips the `WARN`. No product behavior change.

**Independently valuable:** yes — institutionalizes the "no snapshot" decision.

---

## 5. Verification / gates per phase (measurement discipline, parity oracle)

The arc's measurement obeys the Performance Constitution: **cold AND warm**,
**first-launch AND same-path**, per (backend, profile), with the codesign tax
*separated* from execution.

- **Cold/warm separation (always):** every cold number is reported beside its warm
  counterpart; a cold win that costs warm time is a *failed landing* (none here —
  all levers are cold-path-only or build-time).
- **First-launch / same-path separation (Phase 1/2/3):** the two cold axes are
  measured distinctly (`cold_start_decompose.py` already has FRESH vs SAME path
  modes — reuse, do not re-measure). The codesign component is reported as
  *build-time* post-Phase-1, never folded into the runtime tax.
- **Parity oracle (always):** every probe's stdout is parity-checked vs CPython
  (a fast-but-wrong cold start is invalid). Signing/ordering/streaming must not
  change program output — asserted.
- **Page-cold realism (Phase 2/5):** the page-in win is measured in *fresh-path*
  mode (defeats the page cache) on a *multi-page* binary; a same-path-only
  measurement would read ~0 and hide the win (the COLD_START.md path-mode lesson).
- **Backend parity (Phase 2):** ld64 *and* lld measured; a Darwin ordering win must
  not leave ELF unordered.
- **Falsifiable codesign gate (Phase 1):** `codesign --verify` passes on the build
  artifact *and* the fresh-path first-launch total drops toward the no-op C
  baseline — the win is reproduced, not asserted.
- **Ratchet gate (Phase 3):** the budget may only decrease; a fixture increase is
  rejected.
- **No-snapshot guard (Phase 6):** init stays ≤ 500 µs; the dead lever is enforced
  dead.
- **Safe execution (always):** every binary launch via `tools/safe_run.py` with an
  RSS cap + timeout (a cold-start measurement that hangs/OOMs must die small).

Every PR touching this arc runs: the budget-schema test, the codesign-verify gate
(Darwin), the order-file head-of-`__TEXT` check, and (Phase 4) the WASM
streaming-mode smoke. The Rust-touching phases (2 backend emit, 5 bolt.rs) serialize
through the daemon socket (max 2 build agents).

---

## 6. How it composes with the decomposition (21a–e) and the 50–59 / 60–61 arcs

### Composition with the binary-size + tree-shaking arcs (the load-bearing convergence)

`docs/perf/COLD_START.md` is explicit: **#62's molt-controllable lever (binary-page-in)
is size-driven and converges with the binary-size / tree-shaking arc.** This arc and
the size arc are *the same fix viewed from two ends*:

- **The size arc (referenced in the prompt as 61 footprint / 60 tree-shaking;
  `RuntimeSurfacePlan` in `00_integrated_parallel_program.md:276`, ROADMAP
  medium-term) makes the image smaller** (per-attr DCE, `RuntimeSurfacePlan`, stdlib
  slicing) → fewer total pages → less page-in.
- **This arc (62) makes the page-in that remains *ordered*** (StartupOrder, §3.2) →
  the cold path faults a contiguous hot prefix regardless of the cold tail's size.

Together they retire the class completely: size shrinks the tail, ordering bounds
the head, so cold-start page-in is *both* small *and* bounded-as-it-grows. **Cross-arc
dependency:** 62 Phase 2's `StartupOrder` consumes the size arc's section-attribution
(`binary_size_analysis.py`) to know which symbols are on the cold path vs the cold
tail; the size arc consumes 62's first-launch budget to know its page-in win is real.
Neither blocks the other (Phase 1 codesign is independent of size; Phase 2 ordering
benefits from but doesn't require the size arc).

### Composition with arc 64 (perf scoreboards + harness — cold+warm measurement)

Arc 53 (`64_perf_scoreboards_and_harness.md`) **already owns** the `startup_tax_ms` /
`cold_*` schema-v3 fields, `FAIL_COLD_BUDGET`/`WARN_COLD_FLOOR` verdicts, and
explicitly carves out cold as a distinct gated dimension (its §8 Risk 8: "cold-start
/ size / RSS conflated with warm speed — #62 lesson… each dimension is a distinct
gated field"). **This arc supplies the *content* 64 gates:** the two-axis budget
(§3.5), the build-time-signed first-launch number, and the order-file before/after.
The division: **53 owns the gate machinery + CI wiring; 62 owns the cold-start
*levers* + the budget *semantics*.** Phase 3 is the explicit hand-off (it edits the
budget data + the budget-consumption logic 53 projects — serialize through 53's
owner). **Cross-arc dependency (the primary one):** 62 Phase 3 depends on arc 64's
cold-start projection existing (or co-lands the two-axis consumption with it).

### Composition with the 21x decomposition program

- **Touches a god-file responsibly.** Phase 1/2 add to `src/molt/cli/__init__.py`
  (the largest god-file on the audit board) — but **additively via the
  `native_toolchain.py` satellite** (the signing authority lives there, not in the
  god-file; the god-file only *calls* it). This *reduces* concern-mixing (signing
  policy leaves `__init__.py` for the toolchain module) — aligned with 21d (CLI
  package decomposition). The backend emit (Phase 2) is a small additive method in
  `simple_backend.rs` (already a flagged god-file, ceiling 4000, 7860 ln) — it must
  land as a *cohesive* `startup_order` emit (a few methods), and the arc *notes* it
  should land in a `startup_order.rs` submodule if `simple_backend.rs` is being
  decomposed concurrently (coordinate; do not grow the god-file's line count
  net-negative-ly). **Killer touched:** the ownership-collision killer (signing
  policy in one place, not scattered).
- **No new cycle.** The arc is strictly downstream of build + codegen; it consumes
  the artifact and the size attribution. `StartupOrder` is a backend-emitted fact
  consumed by the CLI linker step — same direction as the existing
  `-exported_symbols_list` / `.molt_version.ver` generated link inputs.

### Composition with the multi-agent / three-lane model

Squarely **Lane C** (infra that makes A&B faster) with a Lane-B (perf frontier)
deliverable (cold-start dominance). Parallel-friendly: Phase 1 (CLI/toolchain),
Phase 2 (backend emit + CLI link), Phase 4 (JS host glue), Phase 6 (board) touch
disjoint files and can run as four agents; only Phase 2 + Phase 5 trigger a Rust
build (serialize). Phase 3 coordinates with arc 64's owner.

---

## 7. Parallel execution map (file ownership, no overlaps)

| Phase | Owner files (new unless noted) | Touches Rust? | Blocks / blocked-by |
|---|---|---|---|
| 0 | `bench/scoreboard/cold_start_budget.json` (v2); `docs/perf/COLD_START.md`; `tests/tools/test_cold_start_budget_schema.py` | no | blocks 3 |
| 1 | `src/molt/cli/native_toolchain.py` (`sign_native_artifact`); `src/molt/cli/__init__.py` (call site at `_finalize_native_link`, route daemon/BOLT signing) | no | independent; feeds 3 |
| 2 | `runtime/molt-backend/src/native_backend/simple_backend.rs` (additive `startup_order` emit); `src/molt/cli/__init__.py` (`_build_native_link_driver_command` order-file flags) | **yes (additive emit)** | independent; feeds 5; serialize Rust build |
| 3 | `bench/scoreboard/cold_start_budget.json` (seed); `tools/perf_scoreboard.py` (two-axis budget consumption) | no | blocked-by 0,1; **coordinate arc 64** |
| 4 | `wasm/run_wasm.js`; `deploy/cloudflare/*.js`; `deploy/browser/*.js` | no | independent |
| 5 | `runtime/molt-tir/src/tir/bolt.rs` (complete `generate_order_file`); `src/molt/cli/native_toolchain.py` (`_run_bolt_post_link` order-file hook) | **yes** | blocked-by 2; serialize Rust build |
| 6 | `tools/perf_scoreboard.py` (`init_budget_us` cell) or `bench/scoreboard/cold_start_budget.json`; a guard test | no | independent |

Five of seven phases (0,1,3,4,6) never trigger a Rust build → maximal parallelism.
Phases 1, 4, 6 can run simultaneously the moment the arc starts.

---

## 8. Risks + structural (not band-aid) treatment

### Risk 1: build-time codesign breaks a cross-host or unsigned-required workflow
**Band-aid (rejected):** always sign, no escape.
**Structural fix:** `sign_native_artifact` is Darwin-gated, identity-aware
(real > ad-hoc > skip), and emits a *warning* (not a failure) on cross-host
(`codesign` absent) — the artifact still builds; the user is told to sign on a Mac
or pay the first-launch tax. An env opt-out (`MOLT_CODESIGN=0`) exists for the rare
"I sign downstream" workflow, recorded in the build JSON.

### Risk 2: the order-file's named symbols are stripped before the linker can order them
**Band-aid (rejected):** stop stripping (re-inflates size).
**Structural fix:** the order file is applied at **link** (`-order_file`), which runs
*before* `-x -S` and the post-link `strip -x`. Layout is decided at link; the strip
removes *names*, not *placement*. Verified by the head-of-`__TEXT` check on a
`MOLT_KEEP_SYMBOLS=1` build (names retained for the assertion only).

### Risk 3: the page-in win is ~0 on small binaries → looks like a no-op
**Band-aid (rejected):** claim a win anyway / measure on the minimal probe.
**Structural fix:** measure on a *multi-page* benchmark in *fresh-path* mode (defeats
the page cache); honestly report ~0 on tiny binaries as **expected** (already one
page) — a `DIMENSIONAL_WIN`-style honest classification, never inflated. The win is
*architectural* (bounds growth), proven on the large case.

### Risk 4: signing changes the binary bytes → the link fingerprint / cache breaks
**Band-aid (rejected):** fingerprint the unsigned bytes (cache serves an unsigned
artifact).
**Structural fix:** compute `link_fingerprint` over the **signed** artifact (sign,
then fingerprint), so the cache key reflects what ships. The signature is
content-deterministic for ad-hoc, so the fingerprint is stable.

### Risk 5: WASM streaming unavailable on a host → the launch path breaks
**Band-aid (rejected):** assume streaming everywhere.
**Structural fix:** `WasmColdLaunch` *detects* `compileStreaming` availability and
falls back to `compile(buffer)` with a logged mode marker — streaming-first, never
streaming-only. Tested in both modes.

### Risk 6: the profile-refined order (BOLT/Instruments) drifts from the static order
**Band-aid (rejected):** trust the profile blindly.
**Structural fix:** the refined order must be a *superset ordering* of the static hot
symbols (the static floor is the invariant; profiling only *sharpens within* it) —
gated so the static floor is never made worse. The static tier is always-on; the
profile tier is opt-in sharpening.

### Risk 7: someone "optimizes" cold start with a startup snapshot anyway
**Band-aid (rejected):** rely on the doc saying don't.
**Structural fix:** the `init_budget_us` guard (§3.4) encodes "init is 0.127 ms,
solved" as a *number*; COLD_START.md's adjudication (snapshot REJECTED with measured
evidence) is the durable refusal artifact. The arc's whole framing is "artifact-layer,
not runtime-layer" — the snapshot lever is named dead and guarded dead.

### Risk 8: cold-start regresses silently as the stdlib/artifact grows
**Band-aid (rejected):** one cold number, eyeballed.
**Structural fix:** the two-axis ratcheting budget (§3.5) gated by arc 64 — first-launch
and same-path are *separately* gated and may only *decrease*. A growth-driven page-in
regression trips `FAIL_COLD_BUDGET` and is unmergeable. This is the §0 end-state made
enforceable.

---

## 9. The landing-report this arc makes automatic

When this arc is complete, every native build's cold-start posture is a *measured,
gated fact*:

> **first-launch (signed-at-build, page-cold) cold ≤ budget on every (backend,
> profile)**; codesign moved to build (≈70 ms off the user's first run); page-in
> bounded by the StartupOrder hot prefix (not total size); WASM/edge cold invoke
> streaming + module-cached; runtime-init guarded ≤ 500 µs (no snapshot warranted);
> same-path cold ≤ budget; both budgets ratcheting down with the binary-size arc.
> Artifacts: `bench/scoreboard/cold_start_budget.json` (v2),
> `cold_start_decomposition.json`, the 64 cold-start projection.

That is the §0 outcome: molt's *first* launch of a fresh artifact beats CPython's
startup on every target, the one-time taxes are pre-paid at build, the page-in is a
bounded hot prefix, and a cold regression is *unmergeable* — **cold-start dominance
made a release-gating correctness property, not an aspiration.**

---

## Appendix A — Exact file/line anchors (for the implementing agents)

- **Codesign authority + call site (Phase 1):** `src/molt/cli/native_toolchain.py`
  `_codesign_binary` (lines 40–54) + `__init__.py` `_codesign_sign`/`_codesign_identity_info`
  (~3750–3805); the **missing** user-binary call site is in `_finalize_native_link`
  after `_post_link_strip(output_binary, target_triple)` (`__init__.py:20842`); the
  daemon-binary site to route through it: `__init__.py:27484`; BOLT site:
  `_atomic_copy_file(..., codesign=True)` (`__init__.py:14126–14134`, called at
  `27482` / `native_toolchain.py:177`).
- **Native link command to add `-order_file` (Phase 2):** `__init__.py`
  `_build_native_link_driver_command` Darwin branch (lines ~20201–20218, beside
  `-Wl,-dead_strip` / `-exported_symbols_list`) and ELF branch (~20219–20239, beside
  `--gc-sections` / `--version-script`); Windows `/OPT:REF` (~20241).
- **Per-function-section emit (Phase 2, ELF ordering relies on it):**
  `simple_backend.rs:2604–2606` (`per_function_section(true)`).
- **Order-file generator to complete (Phase 5):** `runtime/molt-tir/src/tir/bolt.rs`
  `generate_order_file` (lines 124–136, **stub**); BOLT scaffolding
  (`optimize_with_bolt`, `collect_perf_profile`) in the same file; CLI hook
  `native_toolchain.py::_run_bolt_post_link` (lines 57–192).
- **Runtime-init ladder (Phase 6, dead-lever evidence):**
  `runtime/molt-runtime/src/state/runtime_state.rs` `molt_runtime_init` (726–818),
  `trace_runtime_init` (668–681), eager capability load (808–813, **do not defer —
  security**).
- **Cold-start decomposition tool (reuse, all phases):** `tools/cold_start_decompose.py`
  (FRESH/SAME path modes, `_measure_runtime_init_phases` parser, `_attribute_components`
  at 583–637, `_highest_leverage` at 650–676).
- **Budget board to bump to v2 (Phase 0/3):** `bench/scoreboard/cold_start_budget.json`
  (current v1, single-axis; `native/release-output` budget `null`); decomposition
  data: `bench/scoreboard/cold_start_decomposition.json` (the seed numbers).
- **64 cold-start projection to wire (Phase 3):** `tools/perf_scoreboard.py`
  `startup_tax_ms` / `cold_*` fields + `FAIL_COLD_BUDGET`/`WARN_COLD_FLOOR` verdicts
  (coordinate with `64_perf_scoreboards_and_harness.md`).
- **WASM launch path (Phase 4):** `wasm/run_wasm.js` `new WebAssembly.Module(buffer)`
  (~4552), `WebAssembly.instantiate(runtimeBuffer)` (~5584), `instantiate(wasmBuffer)`
  (~5612); deploy templates `deploy/cloudflare/worker.js` (serves 13.4 MB
  `falcon-ocr.wasm`, ~1025), `deploy/browser/*.js`.
- **Size convergence (Phase 2/6):** `tools/binary_size_analysis.py`,
  `tools/output_startup_size_audit.py` (fresh-path aware), `tools/wasm_size_audit.py`;
  ship profile `Cargo.toml [profile.release-output]` (opt-`z`, fat-LTO, cgu=1,
  panic=abort, strip — already size-optimal, **no change**).
- **NOT a startup mechanism (scope guard):** `runtime/molt-snapshot/` (WASM
  execution pause/resume across machines — *not* startup; do not repurpose).

## Appendix B — Why this is the compression-ladder unit, not "make startup faster"

A "make startup faster" task tunes a number. This arc makes a *class* of failure
unexpressible: after it lands, "a freshly produced artifact's first launch silently
got slower (unsigned ⇒ codesign tax, scattered ⇒ page-in scaling with size, eager ⇒
block-on-compile) and shipped anyway" cannot happen — the one-time taxes are pre-paid
at build, the page-in is a bounded ordered prefix, the WASM load streams+caches, and
the residual is a two-axis ratcheting budget the 64 gate refuses to let increase. The
missing *fact* here is **"the cold path is a statically-known, ordered, pre-paid,
budgeted property of the artifact"** — `StartupOrder` (the symbols), build-time
signing (the one-time tax), and the two-axis budget (the gate). Once those exist,
the entire family of artifact-growth cold-start surprises is gone, and the arc
converges with the binary-size ladder: shrink the tail, bound the head, gate the
residual — cold-start dominance that *stays* dominant as molt grows.
