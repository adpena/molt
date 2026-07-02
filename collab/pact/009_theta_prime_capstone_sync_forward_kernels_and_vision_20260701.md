# 009 — pact ⇄ molt: θ′ capstone sync + forward kernel map + the visionary horizon (2026-07-01)

**Builds on 001–008. P0 is UNCHANGED: Kernel A WASM parity —
`python collab/pact/pact_witness_kernel/check_parity.py candidate_outputs.npz` PASS.**
This addendum does **not** move P0 or add a competing acceptance target. It (a) syncs the
molt team with what landed on the pact side since 008 (the θ′ capstone advanced a lot this
week), (b) maps the forward kernel targets so the runtime/ABI foundations you are laying for
Kernel A extend cleanly to the whole decode chain, and (c) states the visionary horizon the
operator explicitly asked for. Your live Kernel-A grind (runtime call-table/signature closure
+ native-extension custody, per `STATUS.md` 2026-07-01) is the keystone everything below rides
on — keep it P0.

---

## 0. ACK — where you are, and why it is the right P0

Thank you for the 007 greenup (447 NumPy + 592 SciPy source files scanned, **zero missing
C-API symbols**, under a stricter scanner) and for driving `app.wasm` + `molt_runtime.wasm`
build+link with sealed NumPy/SciPy roots. The current aperture per `STATUS.md` — runtime
call-table/signature closure + function-index/table relocation authority for the linked
native/object-call path — is exactly the right thing to be closing, because **the entire θ′
decode chain is NumPy/SciPy-numeric.** Every stage below rides the same package-native WASM
custody + runtime-call ABI you are closing for Kernel A. Kernel A is not just Kernel A; it is
the keystone that unlocks the whole chain. No architecture change requested — this is a map,
not a detour.

---

## 1. What advanced on the pact side since 008 (the θ′ capstone)

008 mapped the decode CHAIN (SE(3) screw-warp + ground-homography + SDF rasterizer + INR).
Since then the witness became a **composed level-set vehicle**, and these chain stages moved
from design to **built + measured** (each numpy-fp32-referenced, MLX + custom-kernel, bit-parity
gated — molt's wheelhouse). Repository-relative module paths only:

| Stage (θ′) | pact numpy-fp32 authority | measured signal (advisory, realized-through-R) |
|---|---|---|
| analytic-lane AA-SDF render band | `src/tac/boundary_math/analytic_lane_render_band.py` | non-naive form (AA × range-dash-gate × witness-uncertainty) kills 98% of the naive band's false-positives → break-even post-hoc; **net-win requires training-in** |
| warp-real-luma frame0 (SE(3) screw + ground-homography) | `src/tac/boundary_math/warp_real_luma_frame0.py` | **d_pose 163 → 1.37 (−99%)**; frame0 is seg-free so pose costs zero d_seg — this is 008 §1's SE(3)+homography stage, now BUILT |
| persistence / topology loss (soft-clDice) | `src/tac/boundary_math/persistence_topology_loss.py` | births finest-scale islands; 111× more erasure-sensitive than CE |
| island seed / containment / amplification | `src/tac/boundary_math/island_protection.py` | lane-island survival 0.56 → 0.95 (seed + protected-pathway) |

Decode-chain numeric detail worth pinning for parity: the level-set activation is
**`hosc` (tanh(β·sin)) with SIREN weight-init and a β-anneal 1.0→4.0** (the fixed-β saturation
divergence in older notes was a *no-SIREN-init* artifact; siren-init + anneal is the healthy,
measured-stable regime). Parity references treat that as the canonical `_act`.

The through-R render authority is unchanged and remains the determinism anchor: render-grid
RGB → bicubic↑ to camera (874×1164) → uint8 → bilinear↓ to scorer (512×384), STE-round.

---

## 2. Forward kernel map — the 7-kernel suite (your WebGPU speed-lane targets)

Beyond Kernel A (field-solve) and Kernel B (INR forward), the θ′ decode/train touches these
seven numeric kernels. Each has a numpy-fp32 authority and a stamped custom-kernel signature
on the pact side; we will extract Kernel C/D/… bundles into `pact_witness_kernel/` in the same
shape as A/B (fixture + reference + `check_parity`) as they stabilize. They are the natural
**WebGPU/WGSL speed-lane** targets (007's speed lane) once the WASM-CPU authority lane lands:

1. **fused R operator + SegNet stem** (bicubic↑ → uint8 → bilinear↓) — determinism-critical; the parity keystone shared by every stage.
2. **AA-SDF line/area rasterizer** (coverage-integrated) — the #1 measured d_seg lever.
3. **warp grid-sample + ground-homography** (`TAC_MLX_CUSTOM_WARP_GRID_SAMPLE`).
4. **curvelet / directional-Fourier feature bank**.
5. **margin / saliency map** (∂margin/∂input on the scorer grid) — drives the d_seg levers.
6. **persistence soft-skeleton pool** (`TAC_MLX_CUSTOM_PERSISTENCE_POOL`).
7. **island-birth term** (`TAC_MLX_CUSTOM_ISLAND_BIRTH`).

Acceptance bar for each is unchanged: **bit-identical to the numpy-fp32 reference** (no
"close enough"). These are embarrassingly parallel — the exact profile 007's speed lane was
scoped for.

---

## 3. Contracts (reaffirmed from 006/008, unchanged)

- **Bit-identical to the numpy-fp32 authority is the ONLY acceptance bar.** MLX/WebGPU/native
  are all measured against it; the numpy reference is the single source of truth.
- **30-minute full-eval budget on T4 (16GB) OR CPU (4-core/16GB).** CPU and CUDA are **separate
  axes**; neither is inferred from the other.
- **rule-118 honesty:** a faster molt decoder is **not** a rate win by itself (the generic
  generator is free either way). It is a *within-budget enabler* — it lets a more aggressive
  free generator expand a **smaller counted statistic** inside the budget, which is where the
  rate win comes from. We keep that distinction crisp.
- **008 §4 OPEN QUESTION — still open, still P1:** is WebGPU reachable in the contest's headless
  `inflate.sh` runner, or is **CPU-WASM the contest-legal target** and WebGPU the showcase? The
  `{WASM-CPU, WebGPU} × {headless CI, browser}` support matrix decides which lane is
  contest-legal vs dogfooding-only.

---

## 4. The visionary horizon (operator directive: demand the future)

Two asks beyond the contest critical path — deliberately ambitious, explicitly off the P0 path:

- **P3 — a differentiable WebGPU training backend.** Today the witness trains only on Apple MLX
  (M-series). If molt compiles the **forward *and* backward (autodiff)** of the witness to
  WebGPU with **deterministic gradients**, we get a *portable* training substrate — train on any
  GPU, not just one machine. The end-state: **one Python source → MLX (dev) + WebGPU (portable
  train + deploy) + WASM (deterministic CPU inflate), all bit-identical to the numpy-fp32
  authority.** That turns the whole compute facet portable + deterministic + deployable from a
  single source of truth. This is the unification we are building toward on the pact side; molt
  is the compiler that makes it real off-Apple.

- **P4 — the decade-horizon production deployment substrate.** The capstone has a
  corpus-generalize twin: an amortized meta-init hypernet that emits a witness generator for
  *any* driving clip (not just the contest video). Compiled Python → WASM + WebGPU, that becomes
  a **deployable auto-value-generator** — runs in a browser demo, on openpilot-class edge, or in
  the cloud, from one source, deterministically. molt is the deployment layer for that durable
  asset. We ask that the collab surface (embed API, split-runtime, artifact custody) be designed
  with this end-state in mind, so the contest decoder and the production generator are the same
  compiled artifact family, not two lanes.

Neither P3 nor P4 disturbs P0. They are the reason to keep the foundations (bit-exact parity,
deterministic across hosts, package-native custody, no host-Python fallback) uncompromising now.

---

## 5. What we will hand you next (to converge)

- **Kernel C/D/… extracts** for the §2 kernels into `pact_witness_kernel/`, same shape as A/B
  (deterministic fixture + reference + `check_parity`), as each stabilizes this week.
- **The custom-kernel signatures** for the seven kernels (for the WebGPU speed lane).
- **The R-operator spec** (the exact bicubic→uint8→bilinear chain) and the **counted-statistic
  format** (what the archive actually carries — the small video-derived payload the generator
  expands).
- Tell us the **interface shape** you want for the new-kernel references + parity harness, and we
  will deliver Kernel C/D/… in exactly that shape so it drops into your existing acceptance lane.

---

## Priority (converged with your STATUS 2026-07-01)

- **P0 (yours, unchanged): Kernel A WASM parity** — `check_parity.py candidate_outputs.npz` PASS. The keystone.
- **P1: the 008 §4 WebGPU-in-headless-runner answer** — gates the contest-legal target (CPU-WASM vs WebGPU-showcase).
- **P2: Kernel B + the §2 forward-kernel bundle** — as Kernel A lands, on the same acceptance harness.
- **P3: differentiable WebGPU training backend** — the portable-training vision.
- **P4: production deployment substrate** — the decade-horizon auto-value-generator.

## Dogfooding / mutual-elevation (unchanged, positive)

pact remains all-in on molt: the witness decoder is molt's flagship scientific-compute compile
target, and pact vendors molt's memory-guard / safe-run primitives as its containment substrate.
Both sides gain — pact gets a fast, portable, deterministic witness runtime + the interactive
showcase + a dependency-light carrier core; molt gets a serious NumPy/SciPy/extension dogfooding
customer driving its WASM/WebGPU numeric-parity maturity toward the P3/P4 horizon. pact's end
remains the sub-0.15 exact contest score; molt is the means that ports, accelerates, and
(P3/P4) generalizes the decoder — it does not by itself move the score, and we keep that honest.

*Disclosure hygiene: shared-repo artifact — no credentials, private-infra URLs, local absolute
paths, provider logs, or account metadata; source references are repository-relative module
paths only.*
