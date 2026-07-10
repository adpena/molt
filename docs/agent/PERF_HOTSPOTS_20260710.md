# Perf Hotspot Sweep — 2026-07-10

Standing P0 profiling/benchmarking sweep of the molt stack. Ranked by impact on
**(a)** the E1 witness iteration loop and **(b)** the 100-yr perf pillars
(M03 native-wasm/simd128; M46/M47 numeric loops; M09 build-time).

- Host: Windows 11, canonical volume `C:\Molt`, checkout `C:\Molt\molt-src` @
  `dd631d8f79` (= `origin/main`).
- Method: **measure-only** from the shared checkout + on-disk caches + the
  proof-queue run logs. No competing heavy build was launched — a live witness
  E2E (E1 lane, run `20260710T025647`) held the build slot for the whole sweep
  (rustc active, mid runtime-rebuild), so the native/wasm steady-state
  regression micro-benches were **deferred, not fabricated** (see §Deferred).
- Evidence sources: `logs/proof_queue/runs/*.log` (12 recent witness runs),
  `C:\Molt\.molt_cache\{runtime_wasm,module_lowering}`, `src/molt/cli/*` build
  pipeline, `bench/scoreboard/*`.

All numbers below are measured unless explicitly tagged *(estimate)* or
*(historical)*. A PASS is a hypothesis until reproduced (M05): the two big
levers are handoff **specs**, not applied fixes, because they sit on the hot
build path and could not be A/B-measured while the witness held the box.

---

## Witness run corpus (measured, 12 recent runs)

`logs/proof_queue/runs/*.log`, phase markers printed every 21 s:

| run (UTC prefix) | runtime rebuilds | max runtime-build elapsed | max backend elapsed | total wall | status |
|---|---|---|---|---|---|
| 20260709T131059 | **2** | 126 s | 63 s | **834.7 s** | failed@numpy-init |
| 20260709T111929 | **2** | 149 s | 85 s | **959.3 s** | failed@numpy-init |
| 20260709T090658 | 1 | 170 s | 84 s | 1164.3 s | failed |
| 20260709T064856 | 1 | 171 s | — | 1233.8 s | failed |
| 20260709T045445 | **2** | 279 s | 85 s | **1754.2 s** | failed |
| 20260709T001728 | **2** | 170 s | — | 779.8 s | failed |
| 20260709T080709 | 0 | — | — | 683.4 s | failed |
| 20260709T071828 | 0 | — | — | 453.3 s | failed |
| 20260708T222531 | 0 | — | 22 s | 455.2 s | failed |

Memory guard (bca45602 run): `max_rss=12.0 GB` (child cap hit), `max_total_rss=18.0 GB`.

**Signal:** the presence of a runtime rebuild is the single largest wall-time
discriminator. Runs with `rebuilds=2` cluster at **780–1754 s**; runs with
`rebuilds=0` (runtime hydrated from the shared cache) cluster at **453–683 s**.
A runtime rebuild roughly **doubles** the wall of an iteration.

### Phase decomposition of the representative 834.7 s run (`bca45602`)

Reconstructed from the ±21 s marker cadence:

| phase | wall | notes |
|---|---|---|
| uv resolve/install (`--with numpy scipy`) | ~sub-second visible | "Installed 1 package in 126 ms" |
| **Runtime wasm build #1** (one crate-type) | ~126–147 s | `release-output` opt, cold session target dir |
| **Frontend lowering** (field_solve + numpy/scipy closure) | **~150–300 s *(estimate)*** | **INVISIBLE — no progress markers**; M55 pegs cold numpy+scipy re-lower at ~180–250 s |
| Backend codegen (field_solve → wasm, Cranelift) | ~63–84 s | |
| **Runtime wasm build #2** (other crate-type) | ~105–126 s | `release-output`, same crate recompiled |
| wasm-ld link + split | seconds | app.wasm 29 212 KB + molt_runtime.wasm 9 358 KB |
| make_fixture + field_solve reference (native CPython) | <5 s | |
| node WASI instantiate app.wasm | fails @ `_multiarray_umath` init | E1 correctness frontier — expected |

- **Visible/attributed:** ~294–357 s.
- **Unaccounted (dominant hidden phase):** **~480–540 s**, chiefly the
  unmarked frontend-lowering pass + external-package staging (the full
  numpy/scipy trees are content-hash-copied into
  `.molt_build/.../external_static_packages/<sha>/` every run).

The build never enables `MOLT_BUILD_DIAGNOSTICS`, so the landed phase/cache
attestations (`phase_sec`, `frontend_lowering_cache.hit_rate`,
`runtime_wasm_cache`) are **not captured** on the witness path — that hidden
~480 s is currently unmeasurable per-run (see Hotspot 3).

---

## Ranked hotspots

### Hotspot 1 — Runtime wasm **dual-crate-type** rebuild (P0, iteration loop)

**Component:** `src/molt/cli/runtime_build.py` (wasm runtime build), shared cache
`src/molt/cli/runtime_wasm_cache.py`.

**Measured:**
- The runtime wasm is built **twice per invalidated build** as two *separate*
  `cargo rustc` invocations: `--crate-type=staticlib` (reloc, for
  `molt_runtime.wasm`) and `--crate-type=cdylib` (shared, for the linked
  `output.wasm`) — `runtime_build.py:1973–1976`. Same crate, two full wasm
  codegens: **~105–147 s each ≈ ~230 s combined**.
- The shared cross-session cache
  (`C:\Molt\.molt_cache\runtime_wasm`, **3.0 GB, 60 artifacts, ~72 MB each**)
  keys on the runtime fingerprint. It holds **both** kinds (33 reloc + 27
  shared today), so it works — `rebuilds=0` runs are pure cache hits.
- **Root of the invalidation churn:** `molt-runtime` depends on
  `molt-cpython-abi` (`runtime/molt-runtime/Cargo.toml:166`), and the runtime
  fingerprint source closure includes that crate's `src/api/*.rs` **and**
  `runtime/molt-cpython-abi/shims/` (`runtime_fingerprints.py:310`). **Every E1
  cpython-abi commit** — the entire content of the iteration loop — changes the
  fingerprint → both cache slots miss → 2× full ~115 s compiles. Verified: the
  last 5 cpython-abi commits each touch `molt-cpython-abi/src/api/*.rs` +
  `shims/*.c`. The cache dir shows **33 distinct runtime fingerprints in one
  day** = 33 such invalidations.
- Each rebuild runs in a **cold per-session cargo target dir**
  (`target/sessions/<MOLT_SESSION_ID>/`, fresh id per proof-queue run), so
  cargo incremental never engages — the whole runtime crate + deps recompile
  from scratch even for a one-line cpython-abi change.

**Big-O:** per invalidated iteration, runtime rebuild cost ≈ `2 × C_full(molt-runtime→wasm)`
where `C_full ≈ 115 s`; independent of user-program size.

**Proposed fixes (handoff specs — not applied; hot build path, need A/B):**

- **Lever A — dedupe crate-types.** Emit both in one rustc pass
  (`cargo rustc … -- --crate-type=staticlib,cdylib`) so the crate is
  type-checked/MIR-lowered/codegen'd once and only the final emit differs; **or**
  confirm split-runtime needs only the reloc artifact and drop the cdylib build
  entirely. *Payoff (estimate): ~115 s/iteration.* Verify rustc actually shares
  codegen across crate-types for `wasm32-wasip1` before committing (measure
  single-invocation dual-emit vs two invocations).
- **Lever B — stable/incremental target dir** (the M09 "stable target dir"
  lever, explicitly pending). Reuse a target dir keyed on the runtime
  *fingerprint-family* (not the ephemeral session id) so a one-file cpython-abi
  change compiles **incrementally** (~15–30 s) instead of full (~115 s). This is
  the **largest single lever** for the E1 loop. *Payoff (estimate): ~90–200 s/iteration.*
  Guard concurrency (the per-session dir exists for agent isolation — a
  fingerprint-family lock or copy-on-write reuse preserves correctness).

**Triage: P0.** Dominant avoidable cost of the E1 iteration loop; ~230 s of every
cpython-abi-commit iteration.

---

### Hotspot 2 — Runtime built at **release-output** (heaviest opt) during correctness iteration (P0, iteration loop)

**Component:** `src/molt/cli/cargo_profiles.py:50–54` — `--profile browser`
(non-dev) resolves the runtime staticlib to **`release-output`**, the heaviest
optimization profile.

**Measured/derived:** the ~105–147 s per runtime-wasm compile is dominated by
full-opt codegen of the whole runtime crate to wasm. The E1 loop is debugging
**import correctness** (numpy `_multiarray_umath` init), not perf — the heavy-opt
runtime is pure iteration tax; a `dev-fast`/low-opt runtime would reproduce the
same deterministic import failure in a fraction of the compile time (opt level
does not change the correctness outcome here).

**Proposed fix (spec/knob):** an iteration-scoped runtime profile
(e.g. `MOLT_WITNESS_ITERATION_RUNTIME_PROFILE=dev-fast`) for the debug loop;
reserve `release-output` for the final acceptance/parity run. Stacks with
Hotspot 1 Lever B (dev-fast + incremental target dir → runtime compile in
low tens of seconds).

**Caveat:** dev/release behavior can diverge in edge cases (wasm stack usage,
UB). Sound for a *deterministic correctness frontier*; the final green must
still be produced with the shipped `release-output` runtime.

**Triage: P0** for the iteration loop (cheap, high-leverage), gated by the
"final green uses release-output" rule.

---

### Hotspot 3 — Hidden frontend-lowering phase + **unverified** cross-session lowering-cache reuse (P0/P1, iteration loop)

**Component:** frontend lowering + `src/molt/cli/module_frontend_cache.py`
(persistent lowering cache), `src/molt/cli/build_diagnostics.py` (attestation).

**Measured:**
- The frontend-lowering pass over the field_solve + numpy/scipy closure is
  **~480–540 s of unattributed wall** in the 834.7 s run (§Phase decomposition)
  — it prints no progress markers and the witness build does not enable
  `MOLT_BUILD_DIAGNOSTICS`, so `frontend_lowering_cache.hit_rate` is never
  captured on the witness path.
- On-disk `module_lowering` cache: **4.2 GB, 5449 entries**. `field_solve`
  alone has 30 entries (one content-hash × 14 config-digests). The lowering key
  is `{stem}.{content_key}.{context_digest}` (`module_frontend_cache.py:116–139`).
- **Context:** `CODEX-B-CACHE-KEY` **already landed** (`b9d6963fb`, 2026-07-08):
  the lowering cache now keys on a *semantic* tooling fingerprint
  (`_frontend_semantic_tooling_fingerprint`, 33 post-lowering cli files
  excluded) so unrelated cli/runtime/link edits no longer cold-start it. The
  disk multiplicity therefore mostly reflects a day of many-lane commits and
  the pre-fix churn, **not** proven per-run thrash. But the *persistence across
  fresh-session witness runs* is still tagged **unverified** (commits
  `960671a224`/`dd08e8a015`), and M55 flags a session-scoped
  effect-attestation gap.

**Open measurement (the one that matters for the loop):** does the persistent
lowering cache reuse across two consecutive **same-HEAD** fresh-session witness
builds? Command:

```
MOLT_BUILD_DIAGNOSTICS=1 MOLT_BUILD_DIAGNOSTICS_FILE=<run>/bd.json \
  .venv/Scripts/python.exe -m molt build collab/pact/pact_witness_kernel/field_solve.py \
  --target wasm --profile browser --wasm-profile auto --split-runtime --out-dir <run>/build
# run twice at the same HEAD; compare bd.json .frontend_lowering_cache.hit_rate
# ≈1.0 → persistence works; ≈0 → key still unstable across sessions → CODEX-B follow-up
```

**Proposed fix (instrumentation — HANDOFF to the E1 lane owner):**
`tools/pact_witness_acceptance.py` is inside the **E1-WITNESS-TO-GREEN** solo
lane (CLAIMS.md), so I did **not** edit it. Spec for the owner: in `_build_wasm`,
set on the build subprocess env:
`MOLT_BUILD_DIAGNOSTICS=1`,
`MOLT_BUILD_DIAGNOSTICS_FILE=<build_dir>/build_diagnostics.json`,
`MOLT_BUILD_DIAGNOSTICS_VERBOSITY=summary`. The JSON captures **full** payload
regardless of stderr verbosity (`build_diagnostics.py:98–108`): `phase_sec`,
`frontend_lowering_cache.{hit_rate,reused_s,relowered_s}`, `runtime_wasm_cache`
hydrate/publish, `frontend_parallel`, `midend`. Do **not** enable
`MOLT_BUILD_ALLOCATIONS` (tracemalloc — expensive). This is diagnostics-only
(no build-output change) and turns the hidden ~480 s into gated, per-run,
machine-checkable data (M10).

**Triage: P0 to instrument, P1 pending the hit-rate measurement.**

---

### Hotspot 4 — WASM binary size (P1, 100-yr M03/#62)

**Measured (current, witness run):** `app.wasm` **29 212 KB** + `molt_runtime.wasm`
**9 358 KB** = ~38 MB split. *(Historical, `wasm_hotspot_baseline.json`, Mar 2026,
darwin: even `hello` was ~7 MB, code section 6.3 MB of 7 MB; one function
`func_1074` was 198 KB.)* Large binary → node WASI instantiation + browser
download/compile cost, and blows past the 3 MB Cloudflare-free ceiling the
runtime build already trims unicode-names for (`runtime_build.py:514–517`).

**Fix:** existing tree-shaking / binary-size arc (foundation #62/#65). Data point
only; owned by that lane.

**Triage: P1**, tracked elsewhere.

---

### Hotspot 5 — Shared-cache disk growth, no eviction (P2, hygiene / M30)

**Measured:** `runtime_wasm` **3.0 GB** (60 × ~72 MB, 33 fingerprints/day) +
`module_lowering` **4.2 GB** (5449 entries) = **~7.2 GB** in `C:\Molt\.molt_cache`,
growing ~unbounded (33 new runtime fingerprints/day). C: is near-full (M30).

**Fix (spec):** age/LRU eviction keeping the N newest artifacts per key-family
for both shared caches. Small and safe **as a tool**, but must not run
concurrently with a live witness (could evict an in-flight artifact) — spec, not
run tonight.

**Triage: P2.**

---

## Deferred (honest — not measured tonight)

- **Native steady-state regression check** for tonight's hot-path changes
  (kw-call trampoline `58928854b0`; container-anchor INCREFs / PyObject_Call
  authority `6013b845be`/`dd631d8f79`). These touch hot call paths; a kw-call-heavy
  loop + a dict-store loop should be A/B'd. Requires a **quiescent native
  release build** (`molt build --release`, M28) which the live witness lane's
  cargo/rustc held all sweep. Not fabricated. Reproduce with the perf authority:
  `tools/perf_scoreboard.py --set core --backend native --backend llvm
  --profile release-fast --samples 5 --warmup 2 --repeat 5 --classify
  --require-quiescent`. M46/M47 remain the landed baseline; no regression was
  observed in static review, but this pass did not re-measure them.
- **WASM steady-state under node** — the witness app.wasm fails at numpy import
  (E1 frontier), so no clean steady-state kernel ran; a standalone small wasm
  kernel bench (`wasm/run_wasm.js`) is the follow-up once a green app.wasm exists.

## Top-5 summary

1. **Runtime dual-crate-type rebuild** — ~230 s/iteration, every cpython-abi
   commit; fix = single-pass dual-emit **and** stable/incremental target dir
   (M09). *P0.*
2. **Runtime at release-output during correctness iteration** — iteration-scoped
   `dev-fast` runtime profile. *P0.*
3. **Hidden ~480 s frontend-lowering + unverified cross-session cache reuse** —
   wire `MOLT_BUILD_DIAGNOSTICS` into the (E1-owned) witness build; then measure
   `hit_rate`. *P0 instrument / P1 verify.*
4. **~38 MB split wasm binary** — tree-shaking arc. *P1.*
5. **~7.2 GB unbounded shared caches** — LRU eviction. *P2.*
