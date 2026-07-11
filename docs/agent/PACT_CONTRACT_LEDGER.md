# PACT CONTRACT LEDGER — molt ⇄ pact exit criteria (authoritative tracker)

The orchestrator owns **all** exit criteria across pact-collab correspondence
`collab/pact/001…010` and beyond. This file is the tracking instrument: every
concrete ask, exit criterion, contract clause, and deliverable directed at molt
(and molt→pact commitments) has exactly **one row** here, with an evidence-backed
status. Code beats docs; a status is a hypothesis until it cites a landed commit,
a queue RUN_ID, a CLAIMS row, or a file on disk.

- **Contract root:** `docs/agent/ORCHESTRATOR_GOAL.md` (done-criterion = Kernel A
  parity). Source correspondence: `collab/pact/001…010_*.md` + `STATUS.md` +
  `README.md`. Live lane ledger: `docs/agent/CLAIMS.md`.
- **P0 is unchanged and every doc agrees:** Kernel A WASM parity —
  `python collab/pact/pact_witness_kernel/check_parity.py candidate_outputs.npz`
  → PASS. Everything else is downstream of that keystone.
- **The ONLY acceptance bar (006/009/010):** bit-identical to the numpy-fp32
  reference; `atol=1e-3` on float fields, **exact** on integer coords. Never widen
  the atol — surface a divergence instead.

## Refresh protocol (binding — update on EVERY new correspondence 011+)

1. Read the new `collab/pact/0NN_*.md` end to end. Extract each new ask / exit
   criterion / contract clause / deliverable and the molt→pact commitments.
2. Add **one row per new obligation** with a stable ID (topic-based, never a
   priority number — priorities move, IDs must not). If a new doc restates an
   existing obligation, **do not add a row** — append the new `doc§` to the
   existing row's Source cell and update its Status/Evidence.
3. Re-verify EVERY existing row against the live tree (`git log`, `CLAIMS.md`,
   queue RUN_IDs, files on disk). Downgrade any status that no longer holds. No
   optimistic rounding — "landed but proof uncaptured" is **in-flight**, not done.
4. Reconcile the priority ordering (below) with the new doc's own ranking. P0 =
   Kernel A until a doc explicitly moves it (none has through 010).
5. Update the status counts + "Surprising / untracked" section, then land on
   `origin/main` (rebased, SSH, drift gate PASS) with a CLAIMS `PACT-CONTRACT-LEDGER`
   row.

_Last refreshed: **2026-07-10**, against `origin/main` @ `4df4fdbad9`. Latest
correspondence: `010` (landed `7c82badf37`, 2026-07-09). Molt-authored reply
`011` (progress sync + parity-harness interface + 008–010 ack) authored this
refresh, landing with the `MATRIX` doc and the `STATUS.md` rewrite in one push._

## Priority ordering (converged with 010 §4 / 009 §Priority)

| tier | obligation | id |
|---|---|---|
| **P0** | Kernel A WASM parity — the keystone | `KA` |
| **W3 (day-1)** | witness trunk through the EXISTING ONNX→WASM path (immediate integration while intrinsics grow) | `W3-ONNX` |
| **W1 (flagship)** | deterministic decode → WASM-CPU bit-exact **+** the #212 kernels as molt's next intrinsics | `KA` / `KERN7` |
| **P1** | the `{WASM-CPU,WebGPU} × {headless CI, browser}` support matrix (decides the contest-legal target; = W1(a) second half) | `MATRIX` |
| **W2** | FLOW realtime in-browser (WebGPU + WebCodecs) — the showcase | `W2-FLOW` |
| **W4** | co-design the verified numeric-array intrinsic subset — the durable investment | `W4-ARRAY` |
| **W5** | dashboard / product surface — optional | `W5-DASH` |
| **P3 (horizon)** | differentiable WebGPU training backend | `P3-TRAIN` |
| **P4 (horizon)** | production auto-value-generator deployment substrate | `P4-DEPLOY` |

Ordering rationale is 010's own: P0 stays Kernel A; W3 is the ranked #1 post-A
item because the ONNX→WASM substrate already ships (it is integration, not new
compiler work); W1 is the marquee; the P1 matrix is a low-cost doc that unblocks
the contest-legal-target decision; W2/W4/W5 follow; P3/P4 are the decade horizon
that W4's verified array subset is the foundation for.

## Obligation ledger

Status legend: **done** (landed + proven) · **in-flight** (active lane / partial) ·
**standing** (a binding constraint continuously honored, not a one-shot deliverable) ·
**queued** (planned, blocked on P0 or an upstream input) · **not-started** (no lane,
no plan, effectively unowned).

### P0 — Kernel A keystone

| id | obligation | source doc§ | status | evidence | owner / next action |
|---|---|---|---|---|---|
| <a id="KA"></a>`KA` | **Kernel A WASM parity.** Compile `field_solve(lstar)` through molt's package-native WASM path, write all 11 output keys to `candidate_outputs.npz`, pass `check_parity.py`. | 001 (use-case); 005 §done-crit 1; 006 §done; 007 §milestone; 008 P0; 009 P0; 010 P0/W1(a) | **in-flight** | Lane `E1-WITNESS-TO-GREEN` (SOLO). CLAIMS row 186 (`opus-e1-foreign-custody-20260710`): foreign-object custody landed `39a4f737ee`, `DType.__name__` resolves, `_multiarray_umath` runs past `_add_dtype_helper` into DType registration. E2E RUN_ID `20260710T033748-pact-witness-acceptance-ae136709e9574896` rc=1 — **no `candidate_outputs.npz` yet**. Frontier = split-runtime **call-indirect** `null function or function signature mismatch` during `_multiarray_umath` init (branch `e1-callindirect-20260710`). | E1 owner: name the trapping funcref (call-indirect diagnostic), fix the split-runtime app↔runtime call-table/signature relocation, rerun `pact-witness-acceptance`. |
| <a id="KA-GATES"></a>`KA-GATES` | **Kernel A scipy/numpy parity gates.** `distance_transform_edt` exact Euclidean (Maurer/FH, sampling=1); `gaussian_filter` reflect/truncate=4 separable; `maximum_filter`(15)/`minimum_filter`(11) square footprint reflect, **bit-exact** at extremum; `label` 4-connectivity; `percentile` linear; `eigh` eigenvalues ascending. | 006 §parity table; 001 blocker 3 (EDT) | **in-flight** | Native custody BUILT+SEALED for the scipy.ndimage closure: `_nd_image`+`_ni_label` (CLAIMS 161), `_ni_support`/`_ni_docstrings` (119), `_rank_filter_1d` (lane 111), `_ccallback_c` (176), numpy `_umath_linalg`/LAPACK `dsyevd_` for `eigh` (167). Runtime parity of the ops not yet reached (blocked behind `KA`). | Subsumed by `KA`; once WASM runs, diff each field at `atol=1e-3` / exact-int. |

### Standing constraints (binding, continuously honored)

| id | obligation | source doc§ | status | evidence | owner / next action |
|---|---|---|---|---|---|
| <a id="FP32-BAR"></a>`FP32-BAR` | **numpy-fp32 is the sole determinism authority.** `atol=1e-3` float / exact int; do not widen — surface divergence. Kernel-B argmax may switch to an argmax-margin tolerance gate if exact-uint8 is too strict on real φ. | 006 §gates; 009 §3; 010 | **standing** | `check_parity.py` is the pact-owned oracle in-tree; unchanged. | Enforce as-is; flag near-ties to pact rather than loosening. |
| <a id="PKG-CUSTODY"></a>`PKG-CUSTODY` | **Package-source custody rule.** Compile only the package code the program needs; admit **only** source-recompiled native artifacts with explicit custody sidecars; keep tree-shaking/deforestation; **no** host-CPython/Pyodide fallback, patched sources, or compat crutches. | 007 §package-source rule + §impl direction; 003 §Proposed 3 | **standing / honored** | The whole E1 seal/custody architecture respects it — sealed roots `tmp/pact_numpy_multiarray_sealed_for_witness` + `tmp/pact_scipy_ndimage_sealed_for_witness_next`; `fail_closed_gate` keeps `ecosystem_baked=0`. | Keep fail-closed; never bake a Molt-owned numpy/scipy shim. |
| <a id="TWO-LANE"></a>`TWO-LANE` | **Two lanes stay separate.** WASM-CPU determinism authority first; WebGPU/WGSL + SIMD speed lane only after the authority lane is green (and never at the authority's expense). | 007 §impl direction; 006 §compile targets | **standing / honored** | Authority (WASM-CPU) lane is the active P0; no speed-lane work started, correctly. | Hold until `KA` green, then open the speed lane. |
| <a id="RULE118"></a>`RULE118` | **rule-118 honesty.** molt is a *within-budget enabler*, NOT a rate win by itself (the generic generator is free either way); a faster decoder lets a bigger free generator expand a smaller counted statistic inside budget. Keep this distinction crisp. | 008 §2; 009 §3; 010 §5 | **standing** | Recorded here; no molt claim to the contrary exists. | Never frame a molt speedup as a direct score/rate win. |

### Browser embed + distribution

| id | obligation | source doc§ | status | evidence | owner / next action |
|---|---|---|---|---|---|
| <a id="EMBED-API"></a>`EMBED-API` | **Minimal single-function browser embed.** Call `mod.forward(typedArray) → typedArray` without standing up the full WASI process host; `molt-embed::compile_to_wasm` as the downstream entry point. | 003 ask; 004 ask#3 | **in-flight (landed, proof uncaptured)** | `runtime/molt-embed/` crate present; `wasm/browser_embed.js` + `wasm/loader_bridge.js` authorities; typed `molt.forward_f32_v1` import `(input_ptr,byte_len,output_ptr)->i32`. **BUT** the pinned proof `tests/test_wasm_browser_embed.py::test_browser_embed_forward_roundtrips_float32_typed_arrays` is listed **Unknown** in `STATUS.md` — "do not treat as green until rerun on a quiet machine." | Rerun the pinned test on a quiet box; capture green before calling this done. |
| <a id="EMBED-SAMPLE"></a>`EMBED-SAMPLE` | **Confirm the entry point + ship a ~10-line compile-forward-and-call-from-JS sample.** | 004 ask#3 | **done** | `examples/browser_embed_forward/` = `forward.py` + `run_browser_embed_forward.mjs` (plain JS, no `browser_host.js`) + `README.md`. `003` confirms `molt-embed` is the intended embed entry. | — (kept current on embed-ABI changes). |
| <a id="RELEASE-WASM"></a>`RELEASE-WASM` | **Release-managed `molt_runtime.wasm` artifacts** with integrity metadata, so downstream can dogfood without a from-source Rust/wasm build. | 003 §Proposed 2; 004 ask#1 (build-cost relief) | **not-started** | molt **declined** the pact ask for checked-in `.wasm` blobs (003 §Proposed 1, `STATUS.md`): prebuilt distribution belongs in a release/artifact pipeline, not in-repo. `wasm/*.wasm.sha256` are integrity **pins**, not shipped payloads. No release pipeline exists yet. | Build the release-artifact + integrity pipeline (out of the acceptance critical path). |
| <a id="GPU-WORKER"></a>`GPU-WORKER` | **`browser_gpu_worker.js` example** running a compiled compute kernel over a big array (the WebGPU path for embarrassingly-parallel numeric kernels). | 001 bonus; 003 §Proposed 4 | **queued** | Speed lane; blocked by `TWO-LANE` until `KA` is green. WebGPU substrate exists (`tests/e2e/bench_webgpu_conv2d.js`). | Open after `KA`. |

### numpy/scipy coverage

| id | obligation | source doc§ | status | evidence | owner / next action |
|---|---|---|---|---|---|
| <a id="NUMPY-MATRIX"></a>`NUMPY-MATRIX` | **Publish a numpy-on-WASM op support matrix** — `docs/.../numpy_wasm_support.md`, op × {native, wasm, wasm+simd128} × {green/partial/none}, covering matmul, elementwise (sin/cos/tanh/clip/exp), reductions (argmax/max), broadcasting, concatenate/reshape/transpose, and `distance_transform_edt`. | 002 ask + §Proposed | **not-started** | Doc absent (`find docs -iname '*numpy*wasm*'` → none). `docs/CAPABILITIES.md` has **0** numpy/scipy mentions. 010 §0 independently confirms no evidence of a general numpy-array runtime today. Superseded-in-spirit by `W4-ARRAY`. | Publish the matrix as the front end of the `W4-ARRAY` co-design. |
| <a id="NUMPY-SMOKE"></a>`NUMPY-SMOKE` | **One-line WASM smoke:** `phi = feats @ W.T + b; phi.argmax(-1)` compiled to wasm. | 002 §Proposed | **not-started** | No such smoke in-tree. Subsumed by `W3-ONNX` / `W4-ARRAY`. | Fold into the W3 trunk export (matmul+argmax subgraph). |
| <a id="CAPI-GREENUP"></a>`CAPI-GREENUP` | **NumPy/SciPy C-API scan + missing-symbol closure green** under a stricter scanner (447 NumPy + 592 SciPy source files, zero missing symbols). | 007 §delivered | **done** | Landed in the 007 greenup; verified probes in 007 §Verified proof. **Honest scope (010 §0):** this is a *symbol-surface declaration* (the C-API a compiled numpy *would* reach is present), a prerequisite — **not** proof that compiled numpy/scipy *runs*. | — (superseded by the live `KA` runtime closure). |

### Kernel B + forward-kernel suite

| id | obligation | source doc§ | status | evidence | owner / next action |
|---|---|---|---|---|---|
| <a id="KB"></a>`KB` | **Kernel B parity.** WASM `levelset_argmax` == `witness_forward_reference.npz["lstar"]`, exact uint8 (argmax-margin tolerance fallback allowed on real φ per 006 caveat). | 005 stretch; 006 §Kernel B; 007; 009 P2 | **queued** | Explicitly sequenced **after** Kernel A. Pact bundle present; extract re-verified bit-identical to live pact production 2026-07-09 (010 §1 `verify_against_tac` ALL-MATCH). | Run after `KA` on the same acceptance harness. |
| <a id="KERN7"></a>`KERN7` | **The 7-kernel forward suite as WebGPU speed-lane / next intrinsics.** fused R+SegNet stem (determinism keystone), AA-SDF rasterizer (#1 d_seg lever), warp grid-sample+ground-homography, curvelet/directional-Fourier bank, margin/saliency map, persistence soft-skeleton pool, island-birth. 010 W1(b) names the first 4 as molt's next intrinsics, each w/ Metal ref + Rust ref (runtime-rs #282/#283) + golden vectors. Acceptance: WASM-CPU bit-exact vs numpy-fp32; WebGPU **deterministic-per-device** (do NOT promise cross-vendor bit-exactness) + documented vendor variance. | 009 §2; 010 W1(b) | **queued** | Post-A. Pact to deliver Kernel C/D… extracts (`KERN-CD`). Prior-art fix pattern from pact memory L70: fixed-order VJP ("fused-R") kernel for the R-operator dup-index atomic scatter. | Open after `KA`; start with the fused-R determinism keystone. |
| <a id="FRAMERATE"></a>`FRAMERATE` | **Interactive-framerate re-solve** of Kernel A on zoom/scrub (WebGPU dispatch welcome; a fast WASM-CPU pass is already a win). | 005 §done-crit 2; 006 Phase 3 | **queued** | Perf goal, post-parity. | Measure after `KA`; profile the hot path before optimizing (M10). |

### Contest-runtime contracts + cross-backend

| id | obligation | source doc§ | status | evidence | owner / next action |
|---|---|---|---|---|---|
| <a id="MATRIX"></a>`MATRIX` | **The `{WASM-CPU, WebGPU} × {headless CI, browser}` support matrix** → supported / needs-port / blocked. Decides the contest-legal target (CPU-WASM vs WebGPU-showcase). This is W1(a)'s second half and answers 008 §4 / 009 P1. | 008 §4 (P1); 009 P1; 010 W1(a) | **done** | **Delivered** `docs/PACT_SUPPORT_MATRIX.md` (2026-07-10, evidence-based per cell). Verdicts: WASM-CPU×headless = supported (witness parity in-flight); WASM-CPU×browser = needs-port (node-proven, browser E2E uncaptured); WebGPU×headless = **blocked** (no node WebGPU binding — grep zero for `@webgpu/dawn`/`wgpu-native`; JS-mock dispatcher only); WebGPU×browser = needs-port (WGSL shaders shipped, on-GPU parity uncaptured). **Contest-legal target = WASM-CPU/native; WebGPU is showcase-only.** | — (kept current as browser/WebGPU proofs land). |
| <a id="CONTEST-RT"></a>`CONTEST-RT` | **Contest-runtime contracts attached to the authority lane.** 30-min full-eval budget on T4 (16GB) **or** CPU (4-core/16GB); CPU and CUDA are **separate axes**, neither inferred from the other. | 008 §3 (P2); 009 §3 | **queued** | Binding; becomes actionable once `KA` produces a runnable WASM decode to time. | Attach a budget/throughput measurement to the authority lane as it lands. |
| <a id="RUNTIME-RS"></a>`RUNTIME-RS` | **runtime-rs sister-backend parity.** molt (Python→WASM/WebGPU) and pact's `runtime-rs` (Rust→native) both pass the **same** numpy-fp32 parity vectors; the numpy reference is the single source of truth; promote either backend only after bit-exact parity. | 008 §5 | **queued** | molt's obligation here is `KA` parity itself. runtime-rs is pact-owned; increments #282/#283 referenced in 010 W1(b). | Deliver molt's WASM-CPU parity vectors (= `KA`); coordinate golden vectors with `KERN7`. |

### Vision / work-package (010 §3)

| id | obligation | source doc§ | status | evidence | owner / next action |
|---|---|---|---|---|---|
| <a id="W3-ONNX"></a>`W3-ONNX` | **DAY-1 QUICK WIN — export the witness TRUNK through the EXISTING ONNX→WASM path.** The coord-INR trunk (Fourier/curvelet features → FiLM modulation → small MLP → 5-class head) is a standard matmul+activation stack; export it to ONNX and run through the shipped PaddleOCR ONNX→WASM harness + `matmul_f32_tiled`. Acceptance: trunk output parity vs numpy-fp32 for the matmul/activation subgraph (non-argmax, non-scipy). | 010 §3 W3, §4 | **not-started** | **Ranked #1 post-A by 010 §4, yet unowned.** Substrate EXISTS: `demos/tinygrad/onnx_interpreter.py`, `tests/e2e/test_onnx_interpreter_correctness.py`, `matmul_f32_tiled` in `deploy/browser/simd-ops-rs/src/lib.rs`, PaddleOCR ONNX models in `models/paddleocr/`. No lane has exported the witness trunk. | Spin up a `W3-ONNX-TRUNK` lane: export the trunk subgraph, run it through the ONNX→WASM path, diff vs the numpy-fp32 reference. Parallelizable with `KA` (does not need the numpy-array runtime). |
| <a id="W4-ARRAY"></a>`W4-ARRAY` | **Co-design the verified numeric-array intrinsic subset.** The minimal verified array subset the witness needs: elementwise (sin/cos/tanh/exp), matmul (have it), `argmax`, `scipy.ndimage` label/distance, grid-sample — with a per-op parity harness (pact supplies the numpy-fp32 authority per op). | 010 §3 W4, §4; ties 009 P3/P4 | **not-started** | Durable investment; the foundation both P3 and P4 rest on. Absorbs `NUMPY-MATRIX`/`NUMPY-SMOKE`. No scoped intrinsic list exists yet. | Draft the scoped intrinsic list + per-op parity-harness interface; this is also the reply owed to pact (`RESPOND`). |
| <a id="W2-FLOW"></a>`W2-FLOW` | **FLOW realtime in-browser.** The level-set FLOW shader client-side on molt's WebGPU engine, frames over WebCodecs — the interactive witness re-solve in a browser. Acceptance: deterministic-per-device WebGPU parity vs the numpy/MLX forward; showcase lane (off the contest-legal critical path if WebGPU is browser-only). | 010 §3 W2 (operator-authorized #264/#371) | **not-started** | Showcase; downstream of `MATRIX` (is WebGPU reachable?) and the speed lane. | Open after the authority lane + `MATRIX`. |
| <a id="W5-DASH"></a>`W5-DASH` | **Dashboard / product surface (optional).** Consolidate pact's observability dashboard (#236) onto a molt WASM/worker deploy. | 010 §3 W5 | **not-started** | Explicitly optional, off the numeric-critical path. | Only if team appetite; no commitment. |
| <a id="P3-TRAIN"></a>`P3-TRAIN` | **Differentiable WebGPU training backend.** Compile the witness forward **and** backward (autodiff) to WebGPU with deterministic gradients → a portable training substrate (train on any GPU, not just Apple MLX). End-state: one Python source → MLX (dev) + WebGPU (portable train+deploy) + WASM (deterministic CPU inflate), all bit-identical to numpy-fp32. | 009 §4 P3 | **not-started** | Decade horizon; `W4-ARRAY`'s verified subset is its foundation. | Horizon — keep foundations (bit-exact, cross-host determinism, package-native custody) uncompromising now. |
| <a id="P4-DEPLOY"></a>`P4-DEPLOY` | **Production deployment substrate.** Design the collab surface (embed API, split-runtime, artifact custody) so the contest decoder and the decade-horizon amortized auto-value-generator are the **same compiled artifact family**, not two lanes. | 009 §4 P4; 010 §3 | **not-started (design directive)** | Shapes `EMBED-API` / split-runtime / custody design decisions **now**, even though the substrate itself is horizon-scoped. | Carry the "one artifact family" constraint into every embed/custody design call. |

### molt → pact commitments (open actions)

#### Canonical scientific stack

Kernel-A candidate and oracle authority is aligned on **NumPy 2.5.1 / SciPy
1.18.0**. Future multi-version support is a standing follow-on: parameterize
NumPy/SciPy selection through a single fail-closed version gate, analogous to
`TargetPythonVersion`, rather than adding per-version witness lanes or fallback
behavior. This latest-version alignment does not implement that parameterization.

| id | obligation | source doc§ | status | evidence | owner / next action |
|---|---|---|---|---|---|
| <a id="RESPOND"></a>`RESPOND` | **Propose the parity-harness interface shape back to pact.** Tell pact the interface it should deliver new-kernel references + parity harness in, so it ships Kernel C/D… in exactly that shape; and (soft) close the loop on 008/009/010 with a molt-authored reply — `007` is the last molt-side doc. | 009 §5; 010 §Open invitation | **done** | **Authored** `collab/pact/011_molt_reply_progress_sync_and_harness_proposal_20260710.md`: (a) honest Kernel-A progress sync (the numpy/scipy frontier chain landed this session, **not green** — halts at the split-runtime call-indirect trap, RUN_ID `20260710T033748...ae136709`); (b) the parity-harness interface (per-kernel file-set + declarative `<k>_gates.json` mirroring Kernel A's exact/exact_set/atol/order-robust gates, drop-in for B..7); (c) ack of the 010 work package with the converged W3→W1→P1 ranking; (d) `EMBED-API` flagged done-but-unproven. | — (open the `KERN-CD` ingest once pact ships extracts in this shape). |

### pact → molt inputs (their side — tracked for completeness)

| id | obligation | source doc§ | status | evidence | owner / next action |
|---|---|---|---|---|---|
| <a id="PACT-BUNDLE"></a>`PACT-BUNDLE` | pact ships the runnable kernel bundle (`field_solve.py`, `witness_forward.py`, fixtures, `check_parity.py`, `verify_against_tac.py`). | 005; 006 | **done** | `collab/pact/pact_witness_kernel/` present with all 7 files. | — |
| <a id="PACT-RETEST"></a>`PACT-RETEST` | pact owes a $0 re-test of the bundle on its stack (numpy 1.26.4 / scipy 1.17.1). | 010 §1 | **done** | 010 §1: reference reproduce + parity oracle **PASS** (bit-exact, all 11 fields); `verify_against_tac.py` **ALL-MATCH** vs live production 2026-07-09. | — |
| <a id="KERN-CD"></a>`KERN-CD` | pact hands Kernel C/D… extracts (fixture + reference + `check_parity`) in molt's chosen harness shape, as each of the 7 kernels stabilizes. | 009 §5; 010 §Open invitation | **queued (pact side, now unblocked)** | Harness interface delivered (`RESPOND` → `011 §2`): per-kernel `<k>.py` + `make_<k>_fixture.py` + `<k>_reference.npz` + declarative `<k>_gates.json` + `verify_against_tac.py`, shared `check_parity.py`. | pact side: ship extracts in the `011 §2` shape; molt ingests them into `pact-witness-acceptance` behind `KA`. |

## Post-A pivot plan (sequencing the work package)

Kernel A (`KA`) is the single keystone in flight; the moment
`check_parity.py candidate_outputs.npz` → PASS, pivot in the order the docs
themselves rank (010 §4), parallelizing the items that do **not** depend on the
numpy-array runtime:

1. **Parallel-with-A (does not wait for green): `W3-ONNX`.** The witness trunk is
   pure matmul+activation and the ONNX→WASM substrate already ships. This is the
   ranked #1 post-A item and can start **today** on a separate lane — it needs no
   numpy-array runtime, only the existing ONNX interpreter + `matmul_f32_tiled`.
   Also knocks out `NUMPY-SMOKE` (the `feats@W.T+b; argmax` smoke).
2. **Immediately on A-green: `MATRIX` (P1).** A doc, not compiler work; it answers
   the thrice-asked 008 §4/009 P1/010 W1(a) question and decides whether the
   contest-legal target is CPU-WASM or WebGPU-showcase — which gates the order of
   everything after. Cheap, unblocking, overdue.
3. **`KB` then `KERN7` / W1(b) (flagship intrinsics).** Kernel B first (same
   harness, exact uint8), then stand up the #212 kernels as molt's next intrinsics
   — start with the **fused-R + SegNet stem** (the determinism keystone shared by
   every stage), then the AA-SDF rasterizer (#1 d_seg lever), curvelet bank, and
   margin/saliency. WASM-CPU bit-exact; WebGPU deterministic-per-device only.
   Requires `RESPOND` (harness interface) so pact can deliver `KERN-CD` extracts.
4. **`CONTEST-RT` + `RUNTIME-RS`.** Attach the 30-min / per-axis budget contracts
   to the now-runnable authority lane; hand runtime-rs the shared numpy-fp32
   golden vectors.
5. **`W2-FLOW` (showcase)** — only if `MATRIX` says WebGPU is reachable in the
   target environment.
6. **`W4-ARRAY` (durable)** — co-design the verified numeric-array intrinsic
   subset + per-op parity harness; publish `NUMPY-MATRIX` as its front end. This is
   the foundation for the P3/P4 horizon and the substance of the `RESPOND` reply.
7. **Horizon: `P3-TRAIN`, `P4-DEPLOY`; optional `W5-DASH`.** Keep the "one
   compiled artifact family" (`P4-DEPLOY`) and bit-exact/cross-host-determinism
   constraints binding on every embed/custody design decision **now**, so the
   contest decoder and the production generator never fork into two lanes.

Cross-cutting, always-on: `FP32-BAR`, `PKG-CUSTODY`, `TWO-LANE`, `RULE118` gate
every item above; `RELEASE-WASM` (prebuilt-artifact pipeline) and `GPU-WORKER`
ride alongside the speed lane.

## Status roll-up (2026-07-10)

| status | count | ids |
|---|---:|---|
| done | 6 | `EMBED-SAMPLE`, `CAPI-GREENUP`, `PACT-BUNDLE`, `PACT-RETEST`, `MATRIX`, `RESPOND` |
| in-flight | 3 | `KA`, `KA-GATES`, `EMBED-API` (landed, proof uncaptured) |
| standing (binding, honored) | 4 | `FP32-BAR`, `PKG-CUSTODY`, `TWO-LANE`, `RULE118` |
| queued | 6 | `GPU-WORKER`, `KB`, `KERN7`, `FRAMERATE`, `CONTEST-RT`, `RUNTIME-RS`, `KERN-CD` |
| not-started | 10 | `RELEASE-WASM`, `NUMPY-MATRIX`, `NUMPY-SMOKE`, `W3-ONNX`, `W4-ARRAY`, `W2-FLOW`, `W5-DASH`, `P3-TRAIN`, `P4-DEPLOY` |

_(Counts: 6 done, 7 in-flight incl. 4 standing constraints, 7 queued incl. the
pact-side `KERN-CD`, 10 not-started. 30 obligations total. This refresh moved
`MATRIX` + `RESPOND` from not-started → done via the `011` reply + support matrix.)_

## Surprising / under-tracked (read before assuming coverage)

- **STATUS.md refreshed to 2026-07-10 (was 9 days stale).** `collab/pact/STATUS.md`
  previously described the frontier as a call-arity-mismatch / function-index
  problem; it is now rewritten to the live reality (CLAIMS row 186): the
  numpy/scipy frontier chain landed (`sys.flags` `f3b97fa194`, GOT retarget
  `596d8baa8e`, foreign-object custody `39a4f737ee`, …) and the live blocker is
  the split-runtime **call-indirect** signature-mismatch trap. Historical
  2026-07-01 notes preserved in-file. Still trust `CLAIMS.md` + `git log` for the
  daily-moving frontier.
- **The `{WASM-CPU,WebGPU}×{headless,browser}` support matrix is DELIVERED**
  (`docs/PACT_SUPPORT_MATRIX.md`, 2026-07-10) after being asked three times (008
  §4, 009 P1, 010 W1(a)). Evidence-based verdict: **WebGPU is blocked in the
  headless lane** (no node WebGPU binding; JS-mock dispatcher only), so the
  **contest-legal target is WASM-CPU/native** and WebGPU is showcase-only.
- **W3 (the ranked #1 post-A item) is completely unowned.** 010 §4 ranks the ONNX
  trunk export directly under P0 precisely because the substrate already ships —
  yet there is no CLAIMS lane, no queued proof, no export attempt. The entire
  orchestration is P0-Kernel-A-monofocused; **every** non-P0 pact obligation is
  effectively unowned right now.
- **The `EMBED-API` "ask #3 DONE" is done-but-unproven.** 003/004 present the
  browser embed as delivered, but the pinned proof
  `test_browser_embed_forward_roundtrips_float32_typed_arrays` is flagged
  **Unknown** in STATUS.md ("do not treat as green until rerun on a quiet
  machine"). The implementation landed; the acceptance was never captured green.
- **Molt-authored reply 011 delivered** (`collab/pact/011_molt_reply_progress_sync_and_harness_proposal_20260710.md`),
  closing the "no reply since 007" gap: honest Kernel-A progress sync (not green —
  call-indirect frontier), the parity-harness interface for Kernel C/D (per-kernel
  file-set + declarative `<k>_gates.json`), 008–010 ack with the converged
  W3→W1→P1 ranking, and the `EMBED-API` done-but-unproven flag.
- **`docs/CAPABILITIES.md` has zero numpy/scipy content**, corroborating 010 §0's
  honest framing that no general numpy-array runtime is documented today — the 007
  greenup is a symbol-surface declaration, not a runtime proof. The `NUMPY-MATRIX`
  deliverable (002) was never published.
- **No hard deadline surfaced** in 001–010. The only time constraint is the
  contest-runtime **30-min full-eval budget** (`CONTEST-RT`), which is a per-run
  budget, not a calendar deadline. The contest itself is noted **closed** (010 §5),
  so the methods are open-source-destined — no external clock is ticking on the
  collaboration.
