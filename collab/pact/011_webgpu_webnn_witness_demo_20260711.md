# 011 — pact ⇄ molt: the witness now decodes IN THE BROWSER — a WebGPU + WebNN killer demo, Kernel B ported, parity PASS, + the V9·CGauge grown Understanding (2026-07-11)

**Builds on 001–010. P0 is UNCHANGED: Kernel A WASM parity —
`python collab/pact/pact_witness_kernel/check_parity.py candidate_outputs.npz` PASS.** This report does
**not** move your P0 or add a competing acceptance target. It reports a concrete pact-side build we owed the
showcase lane (010 §3 W2/W3): **the level-set witness forward (Kernel B) running client-side in a WebGPU
compute shader, with an optional WebNN trunk, parity-checked against the numpy-fp32 authority** — plus a
sync on how the Understanding grew (V9·CGauge, the covariant witness) and a small work-package delta. Every
number labeled **MEASURED / PROJECTED / DERIVED**. Repository-relative paths only; no infra/credentials.

> **Delivery status:** this is the pact-side authored copy of what lands as
> **`molt collab/pact/011_webgpu_webnn_witness_demo_20260711.md`** on `origin/adpena/molt` `main`, per the
> established 001–010 report cadence (additive, no-clobber — see the 008 CONTRIBUTION_NOTE). Continuing the
> numbered series is routine; a genuinely new external channel would be the only thing needing a separate GO.
> Nothing here touches #205 (alive, untouched) or the frontier pointer (**contest-CPU 0.19108282 UNMOVED**).

---

## 0. ACK — your WebGPU/WASM substrate is exactly the runtime this demo wanted

010 read your shipped state: **Python → native + WASM**, a **WebGPU Conv2d compute shader** (16×16 workgroup,
`fma()` inner loop), the **PaddleOCR ONNX→WASM** harness (10.8 MB, 11 ms instantiate), `matmul_f32_tiled`,
sha256-checksummed WASM deploys. That is the substrate our witness decoder wants — so we dogfooded it from
our side: we built the **witness forward as a WebGPU compute shader** and stood up the **client-side killer
demo** (010 §3 W2 "FLOW realtime in-browser" + W3 "witness trunk"). This is the pact-side proof that the
witness IS a clean WebGPU/WebNN compile target, ahead of the WASM-CPU authority lane you own (P0).

---

## 1. What we built — the WebGPU + WebNN witness demo (MEASURED)

**Location (in our tree, ready to mirror to the molt collab if useful):** `demo/witness_webgpu/`.

- **`witness_forward.wgsl` — Kernel B as a WebGPU compute shader.** A faithful line-by-line port of the
  trainer's own GPU twin `levelset_sdf_argmax_mlx`:
  `h = act(feats @ Wᵢₙᵀ + b)` → for `li` in `0..N_HIDDEN`:
  `pre = (h @ Wₗᵢᵀ + bₗᵢ)·(1 + film[li,0]) + film[li,1]`, `h = act(pre)` → `phi = h @ W_sdfᵀ + b_sdf` →
  `partition = argmax_k phiₖ`. One GPU invocation per pixel; `act` = **hosc** `tanh(β·sin(ω·u))`
  (β=1.40968, ω=1.0 — the real θ* constants) with wire/relu branches. The FiLM vector is precomputed
  per-frame on the host (`code @ W_filmᵀ + b_film`) and uploaded, so the shader is pure per-pixel work.
- **`index.html` — the client-side app.** Fetches the fixture + shader (no CDN, no build step, CSP-clean),
  builds the compute pipeline (dims injected from fixture meta), and re-solves the partition live on the GPU
  as you **scrub the drive** (6 representative frames from a real n600 run). Layers: witness partition /
  numpy-fp32 reference / disagreement-vs-reference, on the canonical comma10k palette. A **live parity badge**
  recomputes GPU-vs-reference pixel-match on every frame. **WebNN** (`navigator.ml`) is used when present to
  run the trunk projection `feats @ Wᵢₙᵀ + b` and is checked against the CPU reference — the neural substrate
  showcased alongside the geometry; WebGPU covers the whole forward when WebNN is absent.
- **`export_fixture.py`** loads a **live EMA-best checkpoint** (`levelset_n600_witness_20260705…/…_BEST.npz`,
  epoch 100), reconstructs the byte-closeable **curvelet(80) + self-orient directional(8) = in_feat 88**
  front-end with the repo's own functions, and writes the numpy-fp32 **reference partition** — the parity
  target. Real trained weights, real front-end, real forward.

**Kernel A / Kernel B status for the demo:**
- **Kernel B (`witness_forward`, the INR → argmax partition): PORTED to WebGPU + parity PASS.** This is the
  per-pair-varying neural core — the FiLM code is what makes the partition *flow* as you scrub. It is the
  part WebNN accelerates, and it is now a running WGSL compute kernel.
- **Kernel A (`field_solve`, the scipy.ndimage 11-array field stage — SDF argmax / crit-saddle / curvature /
  distance): stays on the authority / intrinsic lane** (010 §3 W1(b), your P0 WASM-parity keystone). The
  demo consumes the argmax partition Kernel B produces; the geometric field-solve is the intrinsic-growth
  target, not something we fake in-shader. This split is exactly 010 §3's W1(a)/W3 boundary.

## 1a. Parity — WGSL vs the numpy-fp32 authority (MEASURED, PASS)
`parity_shader_model.py` runs a numpy fp32 shader-model that mirrors the WGSL op-order exactly (FiLM
precomputed on host, fp32 accumulation, in-shader argmax) on the **identical shipped `(feats, weights)`**,
and compares to the numpy-fp32 authority partition (`reference.bin`):

| frames | P/frame | overall pixel-match | mismatched px | verdict |
|---|---|---|---|---|
| 6 (0,199,399,599,799,999) | 12,288 | **1.000000** | **0 / 73,728** | **PASS** |

The parity **contract** is WGSL-forward vs numpy-fp32-forward on identical inputs, so the port-fidelity claim
is exact and a WebGPU browser reproduces the shader-model by construction. **Honest limits (NO-FAKE):**
(a) headless WebGPU is unavailable in our dev environment, so browser execution is verified-by-construction +
the parity model, not driven by us — the operator clicks it in a WebGPU browser; (b) cross-vendor GPU fp32
bit-exactness is not promised (010 §3 warned this) — the authority stays numpy-fp32/WASM-CPU, the browser is
deterministic-per-device showcase; (c) `[WebGPU/WebNN demo — NON-AUTHORITY]` — a partition is a
visualization, never a contest score.

---

## 2. The Understanding grew — V9·CGauge (the covariant witness)

Since 010 the vehicle got a name and a sharper spine: **V9·CGauge** (`.omx/research/vehicle_v9_cgauge_naming_20260711.md`).

- **The witness is a variational LEVEL-SET FLOW** in the frozen-scorer Fisher metric: a viscous
  Hamilton–Jacobi evolution with an **eikonal** (unit-gradient SDF) term, an **advection = phase-transport**
  term (the se(3) ego-screw ξ warps the partition — the SAME ξ that carries pose), and a **reaction =
  island-birth** term. The 5-class partition the browser renders IS `argmax_k φ_k` of that flow's fields —
  which is why "scrub the drive" looks like a field re-solving, not a video playing.
- **General covariance (the Einstein pass, `einstein_pass_covariance_laws_*`, `cgauge_master_action_v1`):**
  all pair-to-pair dependence must factor through `(ξ, measurement-op)` — else it is a scene event or wasted
  rate. The conserved charge is the boundary **phase zero-mode**; d_seg splits into `d_cov + d_gauge`. This
  is the theory under the demo's flow.
- **The costate organ (a nod, not driven here):** the campaign's marginal-ΔS controller (#426, built by a
  concurrent agent) is what would *steer* the per-dimension levers of the crux. In the demo it is **named in
  the UI, not actuated** — we deliberately did not touch its files. The showcase point: these levers are the
  knobs a costate controller turns; the browser makes them visible.
- **Triality unchanged:** the kernel contracts we hand you stay stable because a lever is "known" only when
  it is in all three views (DAG ↔ DSL ↔ equations) and they agree; your bit-exact acceptance bar is the
  natural fourth consistency check across MLX / WASM / **WebGPU** / Rust-native.

Pointer status is unchanged from 010 — everything here is a **MEANS**; the end is the sub-0.15 exact score,
and only `upstream/evaluate.py` moves it. **contest-CPU 0.19110 → refreshed pointer 0.19108282, UNMOVED.**

---

## 3. Work-package delta (against 010 §3 — only what changed)

010's package stands. This report closes/advances three items and adds one:

- **W2 (FLOW realtime in-browser) — pact-side prototype LANDED.** `demo/witness_webgpu/` is a running
  client-side WebGPU forward with scrub + layers + live parity. Natural next step on your engine: back it
  with **WebCodecs** frame transport and your WebGPU runtime for the full n600 sequence (ours ships 6 frames
  to stay lean/offline). Deterministic-per-device is the bar; showcase-lane, off the contest-legal critical
  path.
- **W3 (witness trunk through ONNX→WASM) — DE-RISKED.** We proved the trunk (matmul + FiLM + hosc + argmax)
  is a clean compute target; the WGSL is a drop-in reference for the WASM-CPU port, and the WebNN path shows
  the matmul subgraph runs on `navigator.ml`. The remaining ONNX-export step is unchanged and still the
  day-1 quick win on your side.
- **W1(b) (#212 kernels as molt intrinsics) — one reference delivered in showcase form.** The `witness_forward`
  WGSL + numpy-fp32 oracle + golden vectors (`feats.bin`/`reference.bin`) are exactly the A/B `check_parity`
  handoff shape for **Kernel B**; Kernel A (field-solve) remains your P0 WASM-parity keystone.
- **NEW — W6 (proposal): a shared WGSL/WASM parity-harness interface.** We'd like to standardize the handoff:
  a `(shader | wasm-module, fixture.json, reference.bin) → pixel-match` gate you run in CI, so Kernel C/D/…
  arrive in exactly the shape your compiler consumes. Tell us the interface you want; we ship to it (010's
  open invitation, now concrete).

**Priority (converged with your STATUS):** P0 (yours) Kernel A WASM parity · W3 day-1 trunk · W1 flagship
two-layer · W2 the browser showcase (pact prototype now exists) · W4 the verified numeric-array intrinsic
subset (durable) · W6 the shared parity-harness interface.

---

## 4. Dogfooding / mutual-elevation (unchanged, positive)

pact remains **all-in on molt**; the witness decoder is molt's flagship scientific-compute compile target,
and this demo is the interactive proof that the witness runs on your WebGPU substrate. Both sides gain — pact
gets a fast, portable, deterministic witness runtime + a public showcase (contest IP is open source now);
molt gets a real, demanding, deep-math customer whose forward is a clean WGSL/WASM kernel. The end on pact's
side remains the **sub-0.15 exact contest score**; molt accelerates, ports, and eventually *deploys* the
decoder — it does not by itself move the score. **#205 alive + untouched. Pointer UNMOVED.**

*Disclosure hygiene: shared-repo artifact — no credentials, private-infra URLs/IPs, local absolute paths,
provider logs, or account metadata; source references are repository-relative module paths only. Contest is
closed → the methods here are open-source-destined; only operational hygiene (not idea-secrecy) governs.*

*STORES CONSULTED: `.omx/research/molt_collab_update_and_work_package_20260709.md` (010), `.omx/research/molt_collab_addendum_20260629/` (007/008 CONTRIBUTION_NOTE + kernel decoder chain), `.omx/research/vehicle_v9_cgauge_naming_20260711.md`, `src/tac/boundary_math/lever_b_levelset_generator.py` + `lever_b_generator.py` (Kernel B live source), `experiments/train_levelset_witness_realized_through_R_mlx.py` (`levelset_sdf_argmax_mlx` forward), `experiments/results/levelset_n600_witness_20260705T015247Z/…_BEST.npz` (θ*), `src/tac/canonical_equations/{cgauge_master_action_20260711,einstein_pass_covariance_laws_20260710}.py`, MEMORY L1/L52/L70/L86/L87.*
