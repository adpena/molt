# PACT CONTRACT LEDGER — molt ⇄ pact exit criteria (authoritative tracker)

The orchestrator owns **all** exit criteria across pact-collab correspondence
`collab/pact/001` through the latest numbered memo. This file is the tracking instrument: every
concrete ask, exit criterion, contract clause, and deliverable directed at molt
(and molt→pact commitments) has exactly **one row** here, with an evidence-backed
status. Code beats docs; a status is a hypothesis until it cites a landed commit,
a queue RUN_ID, a CLAIMS row, or a file on disk.

- **Contract root:** `docs/agent/CODEX_CENTURY_GOAL.md` (current P0 done-criterion = Kernel A
  parity). Source correspondence: every numbered Markdown memo indexed by
  `collab/pact/README.md`, plus `collab/pact/STATUS.md`. Live lane ledger:
  `docs/agent/CLAIMS.md`; executable proof truth: `tools/proof_queue.py`.
- **P0 is unchanged and every doc agrees:** Kernel A WASM parity —
  `python collab/pact/parity/check_parity.py candidate_outputs.npz collab/pact/pact_witness_kernel/reference_outputs.npz collab/pact/pact_witness_kernel/field_solve_gates.json`
  → PASS. Everything else is downstream of that keystone.
- **The ONLY acceptance authority (006/009/010/011/012):** the numpy-fp32
  reference plus the per-output gate manifest. Integer/label outputs are exact;
  designated float outputs are `bitwise`, `atol` (never above `1e-3`), or
  `order_robust_atol`; critical-point rows use `exact_set`. Never widen or replace
  a gate to obtain green — surface a divergence instead.

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
   Kernel A until a doc explicitly moves it (none has through 012).
5. Update the status counts + "Surprising / untracked" section, then land on
   `origin/main` (rebased, SSH, drift gate PASS) with a CLAIMS `PACT-CONTRACT-LEDGER`
   row.

_Last obligation audit: **2026-07-14**, against `origin/main` @ `eca155900f`.
Latest correspondence: inbound `011_webgpu_webnn_witness_demo_20260711.md` and
Molt reply `012_molt_reply_parity_harness_interface_and_kernel_b_intake_20260711.md`.
The separately numbered Molt progress reply `011_molt_reply_…` remains part of
the same additive stream. The missing-obligation matrix below is the canonical
coverage proof; the dated appendix is retained as evidence, not as a second ledger._

## Priority ordering (converged through 012)

| tier | obligation | id |
|---|---|---|
| **P0** | Kernel A WASM parity — the keystone | `KA` |
| **W3 (day-1)** | witness trunk through the EXISTING ONNX→WASM path (immediate integration while intrinsics grow) | `W3-ONNX` |
| **W1 (flagship)** | deterministic decode → WASM-CPU bit-exact **+** the #212 kernels as molt's next intrinsics | `KA` / `KERN7` |
| **P1** | the `{WASM-CPU,WebGPU} × {headless CI, browser}` support matrix (decides the contest-legal target; = W1(a) second half) | `MATRIX` |
| **W2** | FLOW realtime in-browser (WebGPU + WebCodecs) — the showcase | `W2-FLOW` |
| **W4** | co-design the verified numeric-array intrinsic subset — the durable investment | `W4-ARRAY` |
| **W6 (delivered)** | shared fail-loud parity-harness authority | `W6-HARNESS` |
| **W5** | dashboard / product surface — optional | `W5-DASH` |
| **P3 (horizon)** | differentiable WebGPU training backend | `P3-TRAIN` |
| **P4 (horizon)** | production auto-value-generator deployment substrate | `P4-DEPLOY` |

Ordering rationale is 010's own: P0 stays Kernel A; W3 is the ranked #1 post-A
item because the ONNX→WASM substrate already ships (it is integration, not new
compiler work); W1 is the marquee; the P1 matrix is a low-cost doc that unblocks
the contest-legal-target decision; W2/W4/W5 follow; P3/P4 are the decade horizon
that W4's verified array subset is the foundation for.

## Correspondence-to-obligation coverage matrix

This is the concrete missing-obligation audit. A memo is canonical only when
each ask, constraint, delivery, and acceptance criterion maps to a stable row;
restatements update the existing row rather than creating a duplicate.

| memo | canonical obligation IDs | migration verdict |
|---|---|---|
| `001` | `KA`, `KA-GATES`, `NUMPY-MATRIX`, `EMBED-API`, `RELEASE-WASM`, `GPU-WORKER` | covered |
| `002` | `NUMPY-MATRIX`, `NUMPY-SMOKE`, `KA-GATES`, `W4-ARRAY` | covered; still open where rows say open |
| `003` | `EMBED-API`, `RELEASE-WASM`, `GPU-WORKER`, `PKG-CUSTODY` | covered |
| `004` | `EMBED-API`, `EMBED-SAMPLE`, `RELEASE-WASM` | covered |
| `005` | `KA`, `KA-GATES`, `FRAMERATE`, `KB`, `PACT-BUNDLE` | covered |
| `006` | `KA`, `KA-GATES`, `FP32-BAR`, `TWO-LANE`, `KB`, `PACT-BUNDLE`, `EMBED-API`, `RELEASE-WASM`, `PKG-CUSTODY` | covered |
| `007` | `CAPI-GREENUP`, `PKG-CUSTODY`, `KA`, `TWO-LANE`, `RELEASE-WASM` | covered |
| `008` | `KA`, `KA-GATES`, `FP32-BAR`, `KERN7`, `RULE118`, `CONTEST-RT`, `MATRIX`, `RUNTIME-RS` | covered |
| `009` | `KA`, `FP32-BAR`, `RULE118`, `CONTEST-RT`, `MATRIX`, `KERN7`, `KERN-CD`, `P3-TRAIN`, `P4-DEPLOY` | covered |
| `010` | `KA`, `PACT-RETEST`, `KERN7`, `CONTEST-RT`, `MATRIX`, `W3-ONNX`, `W4-ARRAY`, `W2-FLOW`, `W5-DASH`, `RESPOND`, `RULE118`, `P3-TRAIN`, `P4-DEPLOY` | covered |
| `011_molt_reply` | `KA`, `FP32-BAR`, `PKG-CUSTODY`, `RESPOND`, `MATRIX`, `EMBED-API`, `W3-ONNX`, `KERN7`, `W2-FLOW`, `W4-ARRAY`, `KERN-CD` | covered; proposal superseded by delivered `W6-HARNESS` |
| `011_webgpu_webnn` | `KB`, `W2-FLOW`, `W3-ONNX`, `KERN7`, `FP32-BAR`, `RULE118`, `W6-HARNESS`, `P3-TRAIN`, `P4-DEPLOY` | **migrated this audit:** W6 had no stable row; Pact prototype evidence had not updated W2/KB/W3/KERN7 |
| `012_molt_reply` | `KA`, `KA-GATES`, `FP32-BAR`, `TWO-LANE`, `MATRIX`, `W6-HARNESS`, `KB`, `KERN-CD` | **migrated this audit:** canonical engine/intake/rounding constraints were appendix-only |
| current scientific-stack follow-on | `VERSION-GATING` | derived structural obligation shared by the report gates; not a new Pact ask |

**Missing-obligation result:** `W6-HARNESS` was the sole genuinely new stable
obligation absent from the row ledger. The other 011/012 deltas were evidence,
ownership, command-authority, or status changes to existing IDs. There are no
unmapped asks through 012 after this migration.

## Obligation ledger

Status legend: **done** (landed + proven) · **in-flight** (active lane / partial) ·
**standing** (a binding constraint continuously honored, not a one-shot deliverable) ·
**queued** (planned, blocked on P0 or an upstream input) · **not-started** (no lane,
no plan, effectively unowned).

### P0 — Kernel A keystone

| id | obligation | source doc§ | status | evidence | owner / next action |
|---|---|---|---|---|---|
| <a id="KA"></a>`KA` | **Kernel A WASM parity.** Compile `field_solve(lstar)` through Molt's package-native WASM path, write all 11 output keys to `candidate_outputs.npz`, and pass the canonical manifest-driven engine. | 001 (use-case); 005 §done-crit 1; 006 §done; 007 §milestone; 008 P0; 009 P0; 010 P0/W1(a); both 011 memos; 012 P0 | **in-flight** | **No `candidate_outputs.npz` exists and no Molt-WASM parity PASS is captured.** Reports 011/012 preserve P0. The 2026-07-10 call-indirect frontier is historical. Strict SciPy package-seal producer v2 RUN_ID `20260714T193642-pact-scipy-package-seal-produce-68979f4a47-v2-9232397438a34991` exposed a stale generated `_ni_label.c` path. v3 RUN_ID `20260714T214041-pact-scipy-package-seal-produce-96dc470a8a-v3-40b9b2da297a4718` built all four configured artifacts, then correctly refused publication because an ambient PATH-resolved `pkg-config` command duplicated the producer's pinned config-tool authority. Commit `eca155900f` deleted that ambient lane; neither producer run is acceptance evidence. | Run one fresh producer proof on `eca155900f`, verify and relocate the immutable seal, then run the named `pact-witness-acceptance` lane once. Record the canonical parity verdict verbatim; do not infer it from seal/build progress. |
| <a id="KA-GATES"></a>`KA-GATES` | **Kernel A scipy/numpy parity gates.** `distance_transform_edt` exact Euclidean (Maurer/FH, sampling=1); `gaussian_filter` reflect/truncate=4 separable; `maximum_filter`(15)/`minimum_filter`(11) square footprint reflect, **bit-exact** at the extrema that select critical points; `label` 4-connectivity; `percentile` linear; `eigh` eigenvalues ascending. | 001 blocker 3; 006 §parity table; 012 §3 | **in-flight** | `field_solve_gates.json` is `ready` for all 11 outputs. Report 012 records `eigh`/lapack_lite feasibility and the sharp constraint: the 630-way `crit_min` tie requires `m_smooth` from `gaussian_filter(σ=2.0)` to preserve serial accumulation rounding. Runtime parity remains unproven until `KA`. | Run the canonical engine on the Molt candidate. Any accelerated Gaussian path must preserve the registered bitwise intermediate; never widen a float gate. |

### Standing constraints (binding, continuously honored)

| id | obligation | source doc§ | status | evidence | owner / next action |
|---|---|---|---|---|---|
| <a id="FP32-BAR"></a>`FP32-BAR` | **numpy-fp32 plus the gate manifest is the sole determinism authority.** Exact/bitwise/set/tolerance policy is per output; `atol` may never exceed `1e-3`. Kernel-B argmax may change only through an explicit margin-policy contract if exact uint8 is falsified on real φ. | 006 §gates; 009 §3; 010; 011 WebGPU §1a; 012 §§0-2 | **standing** | `collab/pact/parity/check_parity.py` is the single wired engine; it rejects missing/extra arrays, dtype/shape and NaN/Inf-mask drift, unknown gates, +1 ULP under `bitwise`, and manifests above the atol ceiling. The old inline oracle is frozen only for equivalence proof. | Enforce the manifest unchanged; flag near-ties or numerical divergence to Pact rather than loosening a gate. |
| <a id="PKG-CUSTODY"></a>`PKG-CUSTODY` | **Package-source custody rule.** Compile only the package code the program needs; admit **only** source-recompiled native artifacts with explicit custody sidecars; keep tree-shaking/deforestation; **no** host-CPython/Pyodide fallback, patched sources, or compat crutches. | 007 §package-source rule + §impl direction; 003 §Proposed 3 | **standing / honored** | NumPy uses its versioned seal; SciPy's configured witness set is reproduced from one upstream Meson graph and atomically published under `$MOLT_EXT_ROOT/package-seals/scipy/<version>/pact_scipy_witness`. Historical split SciPy roots and package-specific build/closure/config adapters are not admitted. `fail_closed_gate` keeps `ecosystem_baked=0`. | Keep fail-closed; never bake a Molt-owned numpy/scipy shim. |
| <a id="TWO-LANE"></a>`TWO-LANE` | **Two lanes stay separate.** WASM-CPU is the determinism authority; WebGPU/WGSL + SIMD is a separately labelled speed/showcase lane and never substitutes for authority. | 006 §compile targets; 007 §impl direction; 011 WebGPU §§1-3; 012 §0 | **standing / honored** | Pact delivered a WGSL shader-model prototype with 0/73,728 mismatches, but explicitly labels it non-authority and lacks a captured browser GPU run. Molt's WASM-CPU `KA` remains P0. | Preserve the CPU authority while ingesting reusable Pact WGSL/golden-vector evidence into the separately gated speed lane. |
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

### Shared parity-harness authority

| id | obligation | source doc§ | status | evidence | owner / next action |
|---|---|---|---|---|---|
| <a id="W6-HARNESS"></a>`W6-HARNESS` | **One fail-loud parity engine for WASM, WGSL, MLX, and future Kernel B/C/D… candidates.** It must enforce exact keysets, dtype/shape and NaN/Inf identity, declared exact/bitwise/set/tolerance gates, and an `atol<=1e-3` ceiling; scaffolds must be unpassable. | 009 §5 / 010 invitation (interface request); 011 WebGPU §3 W6 (concrete ask); 012 §1 (delivery) | **done** | `collab/pact/parity/check_parity.py` is the only wired acceptance engine; `tools/pact_witness_acceptance.py` delegates to it. `field_solve_gates.json` is the ready 11-output reference manifest. `make_kernel_scaffold.py` emits `AWAITING_PACT_KERNEL_SOURCE` manifests that exit 2. The old inline oracle survives only as frozen equivalence evidence. The 2026-07-11 focused harness record reports 40/40 tests green across engine equivalence, scaffold refusal, and gate behavior. | Keep every new kernel on this schema; changes require equivalence proof against all ready manifests and may never weaken an existing gate. |

### Kernel B + forward-kernel suite

| id | obligation | source doc§ | status | evidence | owner / next action |
|---|---|---|---|---|---|
| <a id="KB"></a>`KB` | **Kernel B parity and intake.** Molt-WASM `levelset_argmax` must satisfy `witness_forward_gates.json`: `partition` exact uint8; optional `phi` and trunk outputs `atol<=1e-3` unless Pact explicitly declares a stronger CPU bitwise contract. | 005 stretch; 006 §Kernel B; 007; 009 P2; 011 WebGPU §§1/3; 012 §2 | **queued** | Pact's shader-model prototype reports exact partition parity (0/73,728), but no browser GPU execution or Molt-WASM candidate is captured. Report 012 opens intake and names the missing file set/output-key decision. This does not satisfy Molt's post-A parity row. | Pact supplies/finalizes `witness_forward.py`, deterministic fixture, `witness_forward_gates.json`, and exact output keyset; Molt ingests them through `W6-HARNESS` after `KA`. |
| <a id="KERN7"></a>`KERN7` | **The 7-kernel forward suite as WebGPU speed-lane / next intrinsics.** fused R+SegNet stem (determinism keystone), AA-SDF rasterizer (#1 d_seg lever), warp grid-sample+ground-homography, curvelet/directional-Fourier bank, margin/saliency map, persistence soft-skeleton pool, island-birth. Acceptance: WASM-CPU through the per-output numpy-fp32 manifest; WebGPU **deterministic-per-device** with documented vendor variance. | 009 §2; 010 W1(b); 011 WebGPU §3; 012 §1 | **queued** | Pact delivered the Kernel-B WGSL + numpy-fp32 oracle + golden vectors as the first showcase-form reference. Kernel C/D… file sets remain Pact inputs (`KERN-CD`); no Molt intrinsic promotion is proven. | Open after `KA`; ingest each complete file set through `W6-HARNESS`, beginning with the fused-R determinism keystone. |
| <a id="FRAMERATE"></a>`FRAMERATE` | **Interactive-framerate re-solve** of Kernel A on zoom/scrub (WebGPU dispatch welcome; a fast WASM-CPU pass is already a win). | 005 §done-crit 2; 006 Phase 3 | **queued** | Perf goal, post-parity. | Measure after `KA`; profile the hot path before optimizing (M10). |

### Contest-runtime contracts + cross-backend

| id | obligation | source doc§ | status | evidence | owner / next action |
|---|---|---|---|---|---|
| <a id="MATRIX"></a>`MATRIX` | **The `{WASM-CPU, WebGPU} × {headless CI, browser}` support matrix** → supported / needs-port / blocked. Decides the contest-legal target (CPU-WASM/native vs WebGPU showcase). | 008 §4 (P1); 009 P1; 010 W1(a); 011 WebGPU §1a; 012 §0 | **done** | `docs/PACT_SUPPORT_MATRIX.md` is evidence-based per Molt cell. Pact's external shader-model demo does not promote a Molt cell: it captured no browser GPU run. Contest-legal target remains WASM-CPU/native; WebGPU remains showcase-only until real-device proof. | Re-grade only from captured Molt execution evidence, never from shader text, mocks, or an external parity model. |
| <a id="CONTEST-RT"></a>`CONTEST-RT` | **Contest-runtime contracts attached to the authority lane.** 30-min full-eval budget on T4 (16GB) **or** CPU (4-core/16GB); CPU and CUDA are **separate axes**, neither inferred from the other. | 008 §3 (P2); 009 §3 | **queued** | Binding; becomes actionable once `KA` produces a runnable WASM decode to time. | Attach a budget/throughput measurement to the authority lane as it lands. |
| <a id="RUNTIME-RS"></a>`RUNTIME-RS` | **runtime-rs sister-backend parity.** molt (Python→WASM/WebGPU) and pact's `runtime-rs` (Rust→native) both pass the **same** numpy-fp32 parity vectors; the numpy reference is the single source of truth; promote either backend only after bit-exact parity. | 008 §5 | **queued** | molt's obligation here is `KA` parity itself. runtime-rs is pact-owned; increments #282/#283 referenced in 010 W1(b). | Deliver molt's WASM-CPU parity vectors (= `KA`); coordinate golden vectors with `KERN7`. |

### Vision / work-package (010 §3)

| id | obligation | source doc§ | status | evidence | owner / next action |
|---|---|---|---|---|---|
| <a id="W3-ONNX"></a>`W3-ONNX` | **DAY-1 QUICK WIN — export the witness TRUNK through the EXISTING ONNX→WASM path.** The coord-INR trunk (Fourier/curvelet features → FiLM modulation → small MLP → 5-class head) is a standard matmul+activation stack; export it to ONNX and run through the shipped PaddleOCR ONNX→WASM harness + `matmul_f32_tiled`. Acceptance: trunk output parity vs numpy-fp32 for the matmul/activation subgraph. | 010 §3 W3/§4; 011 WebGPU §3 | **not-started** | Pact de-risked targetability with WGSL and an optional WebNN trunk cross-check, but report 011 explicitly says the ONNX-export step is unchanged. No Molt ONNX export/candidate is captured. | Export the trunk, execute it through the ONNX→WASM path, and gate its declared outputs through `W6-HARNESS`. Parallelizable with `KA` because it does not require the NumPy array runtime. |
| <a id="W4-ARRAY"></a>`W4-ARRAY` | **Co-design the verified numeric-array intrinsic subset.** The minimal verified array subset the witness needs: elementwise (sin/cos/tanh/exp), matmul (have it), `argmax`, `scipy.ndimage` label/distance, grid-sample — with a per-op parity harness (pact supplies the numpy-fp32 authority per op). | 010 §3 W4, §4; ties 009 P3/P4 | **not-started** | Durable investment; the foundation both P3 and P4 rest on. Absorbs `NUMPY-MATRIX`/`NUMPY-SMOKE`. No scoped intrinsic list exists yet. | Draft the scoped intrinsic list + per-op parity-harness interface; this is also the reply owed to pact (`RESPOND`). |
| <a id="W2-FLOW"></a>`W2-FLOW` | **FLOW realtime in-browser.** Run the level-set FLOW shader through Molt's WebGPU engine, carry frames over WebCodecs, and prove deterministic-per-device parity in an actual browser; this is a showcase lane, never contest authority. | 010 §3 W2; 011 WebGPU §§1/3; 012 §0 | **in-flight (Pact prototype only)** | Pact reports a local `demo/witness_webgpu/` scrub/layer/parity UI and a shader-model PASS, but also states headless WebGPU was unavailable and browser execution was not driven. Molt has not mirrored the demo, wired WebCodecs/full n600 transport, or captured on-GPU parity. | Ingest the Pact fixture/WGSL through `W6-HARNESS`, wire Molt WebGPU + WebCodecs, and capture a real-browser result with vendor/device identity. |
| <a id="W5-DASH"></a>`W5-DASH` | **Dashboard / product surface (optional).** Consolidate pact's observability dashboard (#236) onto a molt WASM/worker deploy. | 010 §3 W5 | **not-started** | Explicitly optional, off the numeric-critical path. | Only if team appetite; no commitment. |
| <a id="P3-TRAIN"></a>`P3-TRAIN` | **Differentiable WebGPU training backend.** Compile the witness forward **and** backward (autodiff) to WebGPU with deterministic gradients → a portable training substrate (train on any GPU, not just Apple MLX). End-state: one Python source → MLX (dev) + WebGPU (portable train+deploy) + WASM (deterministic CPU inflate), all bit-identical to numpy-fp32. | 009 §4 P3 | **not-started** | Decade horizon; `W4-ARRAY`'s verified subset is its foundation. | Horizon — keep foundations (bit-exact, cross-host determinism, package-native custody) uncompromising now. |
| <a id="P4-DEPLOY"></a>`P4-DEPLOY` | **Production deployment substrate.** Design the collab surface (embed API, split-runtime, artifact custody) so the contest decoder and the decade-horizon amortized auto-value-generator are the **same compiled artifact family**, not two lanes. | 009 §4 P4; 010 §3 | **not-started (design directive)** | Shapes `EMBED-API` / split-runtime / custody design decisions **now**, even though the substrate itself is horizon-scoped. | Carry the "one artifact family" constraint into every embed/custody design call. |

### molt → pact commitments (open actions)

#### Canonical scientific stack

Kernel-A candidate and oracle authority is aligned on **NumPy 2.5.1 / SciPy
1.18.0 / CPython 3.12** through the fail-closed verified-support matrix at
`config/scientific_stack_versions.toml`. Proof-queue commands, friend source
pins, seal/source-regeneration tooling, and CPython-ABI pkg-config generation
resolve through `molt.scientific_stack_versions`; adding support requires a
verified matrix entry plus matching seals, not a new witness lane or fallback.

| id | obligation | source doc§ | status | evidence | owner / next action |
|---|---|---|---|---|---|
| <a id="VERSION-GATING"></a>`VERSION-GATING` | **Parameterize the canonical scientific stack behind one verified-support matrix and fail before install/seal/link/runtime work for unsupported tuples.** | M02; M05; M08; canonical scientific stack follow-on | **done** | `config/scientific_stack_versions.toml` contains the sole selected tuple and verified matrix (currently exactly NumPy 2.5.1 / SciPy 1.18.0 / CPython 3.12 plus the existing source refs/seal roots). `molt.scientific_stack_versions` gates proof queue, friend manifest substitution, NumPy/SciPy source regeneration, seal verification/restamping, and CPython-ABI pkg-config generation. Mask proof: `tests/tools/test_scientific_stack_versions.py` rejects 9.9.9 outside the matrix and proves a config-only supported tuple change propagates without resealing. | Add a matrix entry only after matching source refs and seals are produced and verified; do not widen the claim from this row alone. |
| <a id="RESPOND"></a>`RESPOND` | **Propose and close the parity-harness interface with Pact.** Specify how Kernel B/C/D… references arrive, acknowledge 008-010, and answer inbound W6. | 009 §5; 010 invitation; 011 Molt reply; 011 WebGPU W6; 012 | **done** | The Molt `011` reply proposed the per-kernel file set and declarative gates; `012` answered Pact's concrete W6 ask with the delivered `W6-HARNESS`, documented Kernel-B intake, and preserved the honest P0 boundary. | Communication closed; executable authority is `W6-HARNESS`, while source delivery remains `KB`/`KERN-CD`. |

### pact → molt inputs (their side — tracked for completeness)

| id | obligation | source doc§ | status | evidence | owner / next action |
|---|---|---|---|---|---|
| <a id="PACT-BUNDLE"></a>`PACT-BUNDLE` | Pact ships the runnable Kernel-A/B source, fixtures, reference generators, and fidelity oracle. | 005; 006; 012 §1 | **done** | `collab/pact/pact_witness_kernel/` contains the original bundle; the executable acceptance authority has moved to `collab/pact/parity/check_parity.py` plus `field_solve_gates.json`. The original inline oracle is retained only for equivalence testing. | — |
| <a id="PACT-RETEST"></a>`PACT-RETEST` | pact owes a $0 re-test of the bundle on its stack (numpy 1.26.4 / scipy 1.17.1). | 010 §1 | **done** | 010 §1: reference reproduce + parity oracle **PASS** (bit-exact, all 11 fields); `verify_against_tac.py` **ALL-MATCH** vs live production 2026-07-09. | — |
| <a id="KERN-CD"></a>`KERN-CD` | Pact hands Kernel C/D… extracts as source + deterministic fixture generator + reference + gate manifest + fidelity proof as each kernel stabilizes. | 009 §5; 010 invitation; 011 WebGPU W6; 012 §§1-2 | **queued (Pact side, unblocked)** | `W6-HARNESS` and its unpassable scaffolder define the final intake shape. Kernel B has a concrete intake proposal under `KB`; Kernel C/D… ready file sets are not present. | Pact ships each exact output keyset/manifest; Molt ingests without per-kernel checker code. |

## Post-A pivot plan (sequencing the work package)

Kernel A (`KA`) remains the keystone. `MATRIX`, `RESPOND`, and `W6-HARNESS` are
already delivered; they are prerequisites consumed by the remaining work, not
future pivot steps. Sequence the open portfolio as follows while parallelizing
items that do **not** depend on the NumPy array runtime:

1. **Parallel-with-A (does not wait for green): `W3-ONNX`.** The witness trunk is
   pure matmul+activation and the ONNX→WASM substrate already ships. This is the
   ranked #1 post-A item and can start **today** on a separate lane — it needs no
   numpy-array runtime, only the existing ONNX interpreter + `matmul_f32_tiled`.
   Also knocks out `NUMPY-SMOKE` (the `feats@W.T+b; argmax` smoke).
2. **On A-green: `KB` then `KERN7` / W1(b) (flagship intrinsics).** Kernel B
   first through `W6-HARNESS`, then stand up the #212 kernels as Molt's next intrinsics
   — start with the **fused-R + SegNet stem** (the determinism keystone shared by
   every stage), then the AA-SDF rasterizer (#1 d_seg lever), curvelet bank, and
   margin/saliency. WASM-CPU follows each output's manifest; WebGPU is
   deterministic-per-device only. Pact's `KERN-CD` inputs remain open.
3. **`CONTEST-RT` + `RUNTIME-RS`.** Attach the 30-min / per-axis budget contracts
   to the now-runnable authority lane; hand runtime-rs the shared numpy-fp32
   golden vectors.
4. **`W2-FLOW` (showcase).** The Pact prototype de-risks the shader shape; Molt
   still owes real-browser execution, WebCodecs, and device-labelled parity.
5. **`W4-ARRAY` (durable)** — co-design the verified numeric-array intrinsic
   subset + per-op parity harness; publish `NUMPY-MATRIX` as its front end. This is
   the foundation for the P3/P4 horizon and the substance of the `RESPOND` reply.
6. **Horizon: `P3-TRAIN`, `P4-DEPLOY`; optional `W5-DASH`.** Keep the "one
   compiled artifact family" (`P4-DEPLOY`) and bit-exact/cross-host-determinism
   constraints binding on every embed/custody design decision **now**, so the
   contest decoder and the production generator never fork into two lanes.

Cross-cutting, always-on: `FP32-BAR`, `PKG-CUSTODY`, `TWO-LANE`, `RULE118`, and
`W6-HARNESS` gate every item above; `RELEASE-WASM` and `GPU-WORKER` ride
alongside the speed lane.

## Status roll-up (2026-07-14; correspondence through 012)

| status | count | ids |
|---|---:|---|
| done | 8 | `EMBED-SAMPLE`, `CAPI-GREENUP`, `MATRIX`, `VERSION-GATING`, `RESPOND`, `PACT-BUNDLE`, `PACT-RETEST`, `W6-HARNESS` |
| in-flight | 4 | `KA`, `KA-GATES`, `EMBED-API` (landed, proof uncaptured), `W2-FLOW` (Pact prototype only) |
| standing (binding, honored) | 4 | `FP32-BAR`, `PKG-CUSTODY`, `TWO-LANE`, `RULE118` |
| queued | 7 | `GPU-WORKER`, `KB`, `KERN7`, `FRAMERATE`, `CONTEST-RT`, `RUNTIME-RS`, `KERN-CD` |
| not-started | 8 | `RELEASE-WASM`, `NUMPY-MATRIX`, `NUMPY-SMOKE`, `W3-ONNX`, `W4-ARRAY`, `W5-DASH`, `P3-TRAIN`, `P4-DEPLOY` |

_(Counts: 8 done, 8 active including 4 standing constraints, 7 queued, and 8
not-started: 31 stable obligations total. The table enumerates every ID; count
drift is a ledger defect.)_

## Surprising / under-tracked (read before assuming coverage)

- **`collab/pact/STATUS.md` is a historical frontier narrative, not the live
  queue authority.** It still describes the 2026-07-10 call-indirect trap, which
  later work moved past. Use `KA`, the proof queue, current logs, and git for the
  current prerequisite; never revive that stale blocker from the status prose.
- **The `{WASM-CPU,WebGPU}×{headless,browser}` support matrix is DELIVERED**
  (`docs/PACT_SUPPORT_MATRIX.md`, 2026-07-10) after being asked three times (008
  §4, 009 P1, 010 W1(a)). Evidence-based verdict: **WebGPU is blocked in the
  headless lane** (no node WebGPU binding; JS-mock dispatcher only), so the
  **contest-legal target is WASM-CPU/native** and WebGPU is showcase-only.
- **W3 (the ranked #1 post-A item) remains unowned.** 010 §4 ranks the ONNX
  trunk export directly under P0 precisely because the substrate already ships —
  yet there is no captured Molt export or candidate. Pact's 011 WGSL/WebNN work
  de-risks the graph shape but explicitly leaves the ONNX export unchanged.
- **The `EMBED-API` "ask #3 DONE" is done-but-unproven.** 003/004 present the
  browser embed as delivered, but the pinned proof
  `test_browser_embed_forward_roundtrips_float32_typed_arrays` is flagged
  **Unknown** in STATUS.md ("do not treat as green until rerun on a quiet
  machine"). The implementation landed; the acceptance was never captured green.
- **The two 011 memos and reply 012 close the interface loop.** Molt's first 011
  proposed the file set; Pact's 011 delivered a Kernel-B WGSL prototype and made
  W6 concrete; 012 delivered the single engine and opened exact Kernel-B intake.
  `W6-HARNESS` is done, while `KB`/`KERN-CD` source delivery remains open.
- **`docs/CAPABILITIES.md` has zero numpy/scipy content**, corroborating 010 §0's
  honest framing that no general numpy-array runtime is documented today — the 007
  greenup is a symbol-surface declaration, not a runtime proof. The `NUMPY-MATRIX`
  deliverable (002) was never published.
- **No hard deadline surfaced** in 001–012. The only time constraint is the
  contest-runtime **30-min full-eval budget** (`CONTEST-RT`), which is a per-run
  budget, not a calendar deadline. The contest itself is noted **closed** (010 §5),
  so the methods are open-source-destined — no external clock is ticking on the
  collaboration.

---

## 2026-07-11 evidence record — memos 011 + 012

This is the dated evidence that informed the stable rows above. It does not own
current status or the execution frontier.

- **Inbound `011_webgpu_webnn_witness_demo` (pact→molt) mirrored** to main `2fbffb22d0`.
  Kernel B (`witness_forward`) ported to a WebGPU WGSL compute shader, **parity PASS vs
  numpy-fp32 (1.000000, 0/73,728)** + WebNN trunk cross-check. Pact **explicitly holds
  molt's P0 unchanged** (Kernel A field_solve WASM parity) — no competing acceptance bar.
  New ask **W6**: a shared `(shader|wasm, fixture.json, reference.bin) → pixel-match`
  parity-harness interface.
- **W6 → DELIVERED and VERIFIED (state change from 011's "proposed").** The generalized
  fail-loud engine `collab/pact/parity/check_parity.py` (exit 0/1/2, atol ceiling 1e-3
  no-widen, gate classes exact/bitwise/exact_set/atol/order_robust_atol) is the SINGLE
  wired acceptance authority (`tools/pact_witness_acceptance.py` delegates to it; the old
  inline `pact_witness_kernel/check_parity.py` is SUPERSEDED, kept only as the frozen
  equivalence proof). Kernel A manifest `field_solve_gates.json` (status ready, 11 outputs,
  numpy 2.5.1). Scaffolder `make_kernel_scaffold.py` emits un-passable NOT-IMPLEMENTED
  slots for B..7. **Independently reproduced 2026-07-11: 40/40 harness tests green**
  (`test_pact_parity_engine` equivalence proof on the real Kernel A reference +
  `test_pact_kernel_scaffold` refusal + `test_parity_gate`).
- **Molt reply `012_molt_reply_parity_harness_interface_and_kernel_b_intake` LANDED**
  `a92be3a0cf`: answers W6 (interface documented), opens Kernel B intake
  (`witness_forward → witness_forward_gates.json`, partition=exact uint8, trunk=atol 1e-3),
  restates P0 + feasibility verdict + gaussian serial-accumulation constraint + the live
  split-runtime restoration/custody frontier.
- **Post-A obligation ownership — STILL the honest picture (unchanged by these memos):**
  W3 (ONNX trunk export, ranked #1 post-A) remains unowned; EMBED-API proof
  `test_browser_embed_forward_roundtrips_float32_typed_arrays` remains done-but-unproven
  (needs a quiet-machine rerun); NUMPY-MATRIX (002) / `docs/CAPABILITIES.md` numpy content
  remains unpublished (correctly — the numpy runtime is not end-to-end proven until the
  witness executes; documenting a capability matrix before that would be theater).
- **P0 remained open:** no `candidate_outputs.npz` or parity verdict was captured
  by these memos. Current execution status belongs only to `KA` and the proof queue.
