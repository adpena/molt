# 010 — pact ⇄ molt: the stack got a lot more beautiful — v7.5/v8 sync, the 7-dim crux, geometry-native rate collapse, + a work package for the grown team (2026-07-09)

**Builds on 001–009. P0 is UNCHANGED: Kernel A WASM parity —
`python collab/pact/pact_witness_kernel/check_parity.py candidate_outputs.npz` PASS.** This report
does **not** move your P0 or add a competing acceptance target. It (a) syncs you with how the pact
witness stack evolved since 009 — it became *much* more optimal and, honestly, more beautiful; (b)
reports a fresh **$0 local re-test of your kernel bundle** (owed on our side); and (c) hands the grown
molt team a **ranked, contract-carrying work package** now that you have world-class engineers hungry
for hard, real targets. Every number below is labeled **MEASURED / PROJECTED / DERIVED**. Repository-
relative module paths only; no infra/credentials/paths.

> **Delivery status (for the pact operator):** this file is the pact-side authored copy of what should
> land as **`molt collab/pact/010_pact_update_and_work_package_20260709.md`** on `origin/adpena/molt`
> `main`. Per collab convention (additive, no-clobber — see the 008 CONTRIBUTION_NOTE) it is **NOT**
> auto-pushed. Awaiting operator **GO** to push to the molt channel. Nothing here touches #205 or the
> frontier pointer.

---

## 0. ACK — your shipped state is genuinely impressive, and you're at the right P0

We read your real shipped state (CHANGELOG + README + git log + docs), not just the collab channel, and
it's excellent: **Python → standalone native binaries + WASM**, Rust-owned runtime, CPython ≥3.12 parity
on a **verified subset** (no `exec`/`eval`/monkeypatch/unrestricted reflection), parity/perf/security as
**measurable gates**. Concretely shipped: a **WebGPU Conv2d compute shader** (16×16 workgroup,
`fma()`-optimized inner loop, zero-pad) — because Conv2d is ~60% of PaddleOCR compute; a **Node.js WASM
inference harness running PaddleOCR end-to-end** (10.8 MB binary, **11 ms** instantiation, Chinese-OCR
round-trip parity verified); **ONNX graph optimization** (62-node conv+activation fusion);
**sha256-checksummed WASM deploys** + fail-closed WASM dispatch; a **generic speculative GPU runtime**
being extracted; `matmul_f32_tiled` (K-loop unrolled). That is a real, running, deterministic numeric
WASM/WebGPU stack — exactly the substrate our witness decoder wants.

Thank you also for the 007 C-API greenup (447 NumPy + 592 SciPy source files, **zero missing symbols**
under the stricter scanner). **Honest framing (ours):** we read that as a **symbol-surface declaration**
(the C-API a compiled numpy *would* reach is present), a prerequisite — **not** a proof that compiled
numpy/scipy *runs* yet. We did not find evidence in `docs/CAPABILITIES.md` / `molt.capabilities.toml` that
a general numpy-array runtime exists today; your demonstrated numeric path is the **ONNX-op intrinsic
subset** (Conv2d, tiled matmul) you built for OCR. That distinction *shapes* the work package below (§3):
the near-term real bridge is **ONNX-trunk-now + grow numeric intrinsics**, not "assume numpy compiles."

Per your `STATUS.md` (2026-07-01), the live aperture is the shared runtime-call binder — the
`call arity mismatch (expected 3, got 1) for _LazyIntrinsic.__call__` on the `pact-witness-acceptance`
row, now with green evidence for the fixed-arity→varargs binder primitive. That is exactly the right
keystone to close. No architecture change requested — this is a map + a menu, not a detour.

---

## 1. Fresh $0 re-test on the pact side (owed action — honest result)

We re-ran your kernel bundle end-to-end on our stack (CPython, **numpy 1.26.4 / scipy 1.17.1** — the exact
pins). Two results, both honest:

- **Reference reproduce + parity oracle: PASS (bit-exact, exit 0).** `make_weights_fixture` →
  `witness_forward` (Kernel B, lstar 96×128 u8) → `make_fixture` → `field_solve` (Kernel A, 11 arrays,
  self-check `argmax(phi)==lstar` OK) → `check_parity reference_outputs.npz` → **all 11 fields PASS**
  (`sdf_argmax` exact 0px mismatch; `crit_saddle_eigvec` order-robust max|d|=0; `curvature`/`dist`
  max|d|=0). The bundle is intact and deterministic on our numpy/scipy.
- **NO-FAKE fidelity proof: ALL-MATCH.** `verify_against_tac.py` (with our `src/` on the path) confirms
  your Kernel B extract is **still bit-identical to our LIVE production source** as of today (07-09):
  `curvelet_directional_B`, `curvelet_feats`, `numpy_levelset_forward`, `levelset_argmax` all **MATCH**
  against `tac.boundary_math.lever_b_levelset_generator`. This matters: the θ′ capstone advanced a lot
  this week (§2), and your kernel contract has **not drifted** from what we ship. The extract you are
  compiling is the real thing.

**Honest scope of this test (per apples-to-apples + CPU/CUDA-separate-axes):** this validates the
**pact-side authority lane only** — the numpy-fp32 reference and the extract's fidelity to production. It
does **not** test any molt output (we have no molt WASM build of `field_solve` here), and — per §0 — it
does **not** establish that molt can *run* these numpy/scipy kernels yet; that is the open engineering the
work package scopes. WASM-CPU parity remains **your** live P0. Our side of the contract is green and
current; the compile+run keystone is yours and in-flight. No axis inferred from another.

**The beautiful convergence (why WASM is the *right* target):** WASM-CPU is a **deterministic** compile
target, so a molt-compiled decode is **host-bit-identical by construction** — which is *exactly* our
deterministic-decode non-negotiable, and a **stronger** guarantee than our numpy-portability story. When
molt can run the decode in WASM, host-portability stops being something we verify per-host and becomes a
property of the artifact.

---

## 2. What advanced on the pact side since 009 (the stack got more optimal + beautiful)

009 synced you through the θ′ capstone (SE(3) warp, warp-real-luma frame0 pose −99%, persistence loss,
island protection). Since then the vehicle became a **composed level-set line — v7.5 → v8 — and the
geometry got much sharper.** All numbers `[macOS-MLX / advisory · realized-through-R · NON-PROMOTABLE]`
unless tagged otherwise; the **contest-CPU pointer is UNMOVED at 0.19110** — everything below is a MEANS
until a byte-closed exact-eval row moves it.

### 2a. The vehicle line: v7.5 (optimal single trunk) → v8 (per-class edge-centric)
- **v7.5** = the optimal single-trunk witness (one coord-INR, sealed constants, the composed lever stack).
  **This is what #205 is running RIGHT NOW** (a live 3000-epoch n600 run, the full composed config:
  hosc+SIREN-init β-anneal, self-orient directional basis, chroma, lane-render band, persistence/island
  terms, Muon finish, pose-carrier, ladder island-homotopy). It is the trunk your Kernel A/B live inside.
- **v8** = the edge-centric per-class decomposition: instead of one trunk carrying all 5 classes, **de-share
  the boundary into per-class edge carriers** (Road is the hub — MEASURED ~70% of flip mass flips only at
  its Road separatrix). Increment-1 (DESIGNED, `$0`, not launched) de-shares the Road↔Undrivable
  bulk-boundary into ONE field. This is the natural next compile-target family — see §3.

### 2b. THE crux, stated cleanly: the boundary-band flip over 7 dimensions
The entire residual d_seg is **one object** — the flip of a codim-1 boundary-band pixel — living in a
**product space of 7 dimensions, each with BUILT machinery** (MEASURED, not aspirational):

| dim | built machinery (repo-relative) |
|---|---|
| **scale** | curvelet multi-scale basis (Candès-Donoho) + coarse→fine curriculum = persistence order = anneal |
| **res** | camera-res sub-pixel placement (flip at 874×1164 *before* the downsample D averages it away) |
| **time** | ξ / se(3) Lie engine (`src/tac/se3.py`) + keyframe+warp (~0.33px drift) + horizon-ξ ego-rigid |
| **direction** | all-class directional (anisotropic/curvelet) basis = the #1 lever (**−48% d_seg**, MEASURED) |
| **chroma** | chroma DOF + `chroma_boundary` loss — SegNet reads RGB, so chroma flips the argmax |
| **luma** | luma carrier + seg/luma coupling |
| **place** | margin field = Fisher surrogate (Pearson **0.978**, MEASURED) + annulus (~97% of d_seg in a 4.7%-area band) + sub-pixel localizer |

The **costate controller** is the joint optimizer over the per-dim levers; curvelet self-similarity across
scale makes {direction × place} **scale-recursive (fractal)**. Why this matters to molt: **every one of
these dims is a pure-numeric kernel** with a numpy-fp32 authority — the 7-kernel suite you already scoped
in 009 §2 is the actuator layer for this crux.

### 2c. The SegNet argmax IS a power diagram → store GENERATORS, not boundaries (deep frame)
The scorer partition `P(x) = argmax_c (φ_c(x) + b_c)` is an additively-weighted Voronoi = **Laguerre /
power diagram**. Laguerre = tropical = curvelet = se3 are duals of ONE parsimonious structure. **The lever
is parsimony**: store a few generators (centerline polynomials / Laguerre sites), not a dense boundary.
In-tree substrate already exists: `laguerre_logit_offset` + `power_diagram_argmax`. This re-shapes the v8
carriers onto **parametric generators** — new, clean, low-dimensional compile targets.

### 2d. The rate half collapsed — MEASURED, with real coders (not a proxy)
This is the most exciting recent result. The current frontier archive spends **0.118 S on the rate term**.
The geometry-native representation crushes it:
- **Road↔Undrivable horizon = a degree-3 polynomial**: fits the dominant horizon arc at **1.46 px** median
  residual over 425/512 columns → **4 coefficients**, not 428 chain symbols. The cubic/quadratic coeffs are
  **frozen frame-to-frame** (|Δ|≈1e-7); only the intercept moves ~1.2 px/frame = **ego pitch** = the ξ we
  already store for pose. So 599/600 frames are a near-free ego-warp of frame-0.
- **MEASURED store (real coder — zlib on delta-coded fp16 coeffs): 4.7 KB @ n600 = 0.0032 S** — **8× below**
  a generic arithmetic chain-coder (0.026 S) and **88× below** the naive boundary bitmap (0.282 S).
- **Whole-scene projection (real-coder, n=200/600, de-shared): ~0.02–0.05 S** vs the current 0.118 — 2 edges
  MEASURED-geometric, 3 owed but each with a named primitive (lane poly, sparse object contours).
- **HONEST caveat (NO-FAKE):** 0.0032 S is the DOMINANT-arc term; a small residual sidecar (secondary arcs /
  fit residual) is owed for a complete number. And **rule-118 honesty still governs:** a faster molt decoder
  is not a rate win *by itself* — it lets a **more aggressive free generator** expand a **smaller counted
  statistic** inside the 30-min budget. That is precisely where molt is the enabler.

### 2e. Pose is banked (through byte-close)
The pose half is **banked as an artifact** at n600: the R1 joint-descent ξ ("dxi") ships through the actual
byte-close decode at **d_pose 0.001610 → contribution 0.127, with ξ_eff ≈ 7.2 KB** — 20× better than
no-dxi, 16000× better than a naive bolt-on. `[macOS-CPU advisory · NON-PROMOTABLE]`. The frontier gap to
sub-0.19 is now **entirely d_seg** (the boundary-band-flip crux above). Pose being solved means the whole
compute budget — and molt's speedups — point at the d_seg boundary object.

### 2f. A top-AIML re-open vehicle: cells2pixels (NCA + LPPN)
SIGGRAPH'26 "Neural CA: From Cells to Pixels" (Mordvintsev et al.) — a coarse-lattice NCA (~49K params) +
a per-cell **SIREN** LPPN decoder (~11K) = **~60K counted params**, decoding at **arbitrary resolution**
retrain-free. We measured the architecture from source. It maps onto our AMBER d_seg-core (our strongest
d_seg-core: boundary_band_flip **0.079 = half the polynomial wall**, MEASURED) and its arbitrary-res decode
IS the camera-res sub-pixel placement DOF. It is **staged, not launched** (our blocker is a diagnosed
training-collapse bug, being fixed as P0; #205 owns the GPU). Flagging it because a compiled NCA+LPPN
forward is a beautiful browser/WASM+WebGPU target once it stabilizes.

### 2g. The triality apparatus (how we keep this coherent)
The campaign is held as **ONE object seen through three cyclically-consistent views** — **DAG** (trajectory
/history) ↔ **DSL** (the typed program that compiles to the trainer argv, flag-validated) ↔ **equations**
(the confirmed laws). A finding is "known" only when it is in all three and they agree. This is why the
kernel contracts we hand you are stable: a lever is not "built" until it is a DSL `Lever` factory with a
registered equation and a DAG row. Your bit-exact acceptance bar is the natural fourth consistency check —
the numpy-fp32 reference is the shared source of truth across MLX, WASM, WebGPU, and Rust-native.

---

## 3. The work package — ranked around YOUR shipped state (for the grown team)

We re-ranked this around what molt actually ships (ONNX→WASM running, WebGPU Conv2d, tiled matmul; numpy
runtime not yet established — §0). Two lanes: an **immediate integration** (ONNX-trunk today) and an
**intrinsic-growth** lane (the #212 kernels as molt's next intrinsics, the same way Conv2d grew for
PaddleOCR). Every item ships with the **acceptance discipline you already own**: a numpy-fp32 reference
oracle + deterministic fixture + `check_parity`-shape gate + payload-cleanliness (generic algorithm/code =
FREE and compiled; learned/video-derived artifacts = COUNTED, stay in `archive.zip`). **Bit-identical to
the numpy-fp32 authority is the CPU bar.** P0 = yours, unchanged (Kernel A WASM parity via `molt-embed`).

### W1 — FLAGSHIP: the witness decoder as molt's flagship numeric showcase (two layers)
This is the headline: **the witness decoding live, deterministically, in molt's WASM/WebGPU runtime**, as
molt's marquee scientific-compute compile target (the way PaddleOCR was the OCR marquee).
- **(a) Compile our deterministic inflate/decode to WASM-CPU.** The beautiful convergence (§1): WASM-CPU is
  deterministic, so a molt-compiled decode is **host-bit-identical by construction** — this IS our
  deterministic-decode non-negotiable, and stronger than our numpy-portability story. **Acceptance:**
  `check_parity.py candidate_outputs.npz` PASS from the WASM-produced output; then the
  `{WASM-CPU, WebGPU} × {headless, browser}` support matrix (also answers 008 §4 / 009 P1 — the
  contest-legal target). Reference: `collab/pact/pact_witness_kernel/` (already in your tree, and re-tested
  green on our side today, §1).
- **(b) The #212 kernel suite as molt's NEXT INTRINSICS.** You built a Conv2d intrinsic because PaddleOCR
  needed it. We hand you the perfect next batch: **fused diff-R + SegNet stem** (determinism keystone),
  **AA-SDF line/area rasterizer** (the #1 measured d_seg lever), **curvelet / directional-Fourier feature
  bank**, **margin/saliency map** — each with a **Metal reference + a Rust reference** (our `runtime-rs`
  increments #282/#283) + **golden vectors**. That is a clean intrinsic-development handoff: numpy-fp32
  oracle + two prior implementations + fixtures, in exactly the A/B `check_parity` shape.
  **Honesty on WebGPU (important):** cross-vendor GPU fp32 **bit-exactness is hard** — do NOT promise it.
  Aim for **deterministic-per-device** on WebGPU and reserve bit-exact for the WASM-CPU authority lane.
  **Prior art you can reuse:** we fought exactly this on MLX-GPU (our memory L70) — a dup-index atomic
  scatter in the R-operator's VJP was the *only* non-bit-identical op across processes; a **fixed-order VJP
  ("fused-R") kernel** made the full witness reproducible cross-process. Same fix pattern will help your R
  intrinsic. **Acceptance:** WASM-CPU bit-exact vs numpy-fp32; WebGPU deterministic-per-device + documented
  vendor variance.

### W2 — FLOW realtime in-browser (operator-authorized #264/#371)
**What:** the level-set FLOW shader running client-side on your WebGPU engine, frames over **WebCodecs**
transport — the interactive re-solve of the witness in a browser. **Why:** a natural next step after
OCR-in-browser; highest-observability dogfooding + a public showcase (contest IP is open source now).
**Acceptance:** deterministic-per-device WebGPU parity vs the numpy/MLX forward; the interactive path is
showcase-lane (off the contest-legal critical path if WebGPU is browser-only).

### W3 — DAY-1 QUICK WIN: export the witness TRUNK through your EXISTING ONNX→WASM path
**What:** the witness trunk is a standard **matmul + activation** stack (coord-INR: Fourier/curvelet
features → FiLM modulation → small MLP → 5-class head). Export **that** part to ONNX and run it through the
**path you already ship** (the PaddleOCR ONNX→WASM harness + `matmul_f32_tiled`) — **now**, while the W1
intrinsics develop. **Why it matters:** immediate integration + immediate demo without waiting on the
numpy-array runtime; it also exercises your ONNX lane on a fresh, real graph. **Acceptance:** trunk output
parity vs the numpy-fp32 reference for the matmul/activation subgraph (the non-argmax, non-scipy part).
The geometric/scipy stages (field-solve, warp, argmax) stay on the W1 intrinsic lane.

### W4 — ROADMAP-SHAPING (for the hungry new engineers): co-design a verified numeric-array intrinsic subset
**What:** the future-building ask — **"what would it take for molt to compile our numpy-fp32 reference?"**
Not "assume numpy works" (we found no evidence it runs today, §0), but co-design the **minimal verified
numeric-array intrinsic subset** the witness needs: elementwise (sin/cos/tanh/exp), matmul (have it),
`argmax`, `scipy.ndimage` label/distance ops (Kernel A), grid-sample. This is the durable investment: a
verified array subset makes molt a first-class scientific-compute compiler, not just an OCR runtime.
**Acceptance:** a scoped intrinsic list + per-op parity harness; we supply the numpy-fp32 authority for each.

### W5 — Dashboard / product surface (optional)
If the team wants a product surface, the pact observability dashboard (#236) could consolidate onto a molt
WASM/worker deploy. Optional, off the numeric-critical path — flagged only if it suits the team's appetite.

### Compatibility note (your subset boundary is a feature, not a blocker)
Your supported subset **excludes** `exec`/`eval`/monkeypatch/unrestricted reflection. **Our inflate/decode
path is already compatible in spirit** — it is deterministic, does no reflection, no runtime codegen, no
monkeypatching (it's pure-numeric forward passes + scipy.ndimage). The `runtime-rs` Rust increments are the
bridge on the native side, molt is the bridge on the WASM/WebGPU side, and both sit behind the one
numpy-fp32 oracle. Nothing in the decoder needs the capabilities your subset forbids.

### Open invitation — propose back to us
Your compiler roadmap almost certainly implies targets and interface shapes we haven't scoped. Tell us the
**parity-harness interface** you want new kernel references delivered in, and we ship Kernel C/D/… in
exactly that shape. We designed the collab so the contest decoder and the decade-horizon production
generator (009 §4 P4) are the **same compiled artifact family**. Ideas flowing both ways.

---

## 4. Priority (converged with your STATUS 2026-07-01)

- **P0 (yours, unchanged):** Kernel A WASM parity — `check_parity.py candidate_outputs.npz` PASS. Keystone.
- **W3 (day-1):** witness trunk through the existing ONNX→WASM path — immediate integration while intrinsics grow.
- **W1:** flagship two-layer — deterministic inflate → WASM-CPU (bit-exact) + the #212 kernels as next
  intrinsics (Metal + Rust references + golden vectors). This is the marquee.
- **P1 answer:** the W1(a) support matrix decides the contest-legal target (WASM-CPU vs WebGPU-showcase).
- **W2:** FLOW realtime in-browser (WebGPU + WebCodecs) — the showcase.
- **W4:** co-design the verified numeric-array intrinsic subset — the durable, future-building investment.
- **W5:** dashboard/product surface — optional.

The differentiable-WebGPU-training vision (009 P3) and the production auto-value-generator (009 P4) remain
the decade horizon — W4's verified array subset is the foundation both rest on.

---

## 5. Dogfooding / mutual-elevation (unchanged, positive)

pact remains **all-in on molt** and vendors molt's memory-guard / safe-run primitives as the containment
substrate for our measured-and-bounded compute runs — they work well and they are load-bearing. The witness
decoder is molt's flagship scientific-compute compile target: a serious numpy/scipy/extension dogfooding
customer driving your WASM/WebGPU numeric-parity maturity. Both sides gain — pact gets a fast, portable,
deterministic witness runtime + the interactive showcase + a dependency-light carrier core; molt gets a
real, demanding, deep-math customer. The end on pact's side remains the **sub-0.15 exact contest score**;
molt is a means that accelerates, ports, and eventually *deploys* the decoder — it does not by itself move
the score. **Pointer contest-CPU 0.19110 UNMOVED.** #205 alive + untouched.

*Disclosure hygiene: shared-repo artifact — no credentials, private-infra URLs/IPs, local absolute paths,
provider logs, or account metadata; source references are repository-relative module paths only. Contest is
closed → the methods here are open-source-destined; only operational hygiene (not idea-secrecy) governs.*
