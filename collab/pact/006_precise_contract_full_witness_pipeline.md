# 006 — pact ⇄ molt: the EXACT Python to compile + determinism gates + the vision (2026-06-27)

Operator: *"give it the exact python you want to compile, be bold and demanding and visionary —
the molt team is world class and hungry."* So here it is — the real generator, runnable, with a
machine-checkable fidelity proof. Everything in `pact_witness_kernel/` is the **actual** pact
compute (not a toy), proven bit-identical to our production tac source.

## The vision — a live, in-browser, self-re-solving witness

pact's "witness" is a non-RGB task-space codec: a tiny coordinate-INR that **generates** the
SegNet class partition of a driving scene. The showcase should let anyone open a browser and
watch the math *work* — scrub the drive, zoom into the road-apex triple-junction, and see the
level-set partition + Morse-Smale complex (saddles = lane/road/sky triple-junctions, curvature
spiking at the vanishing point) **recompute live from the raw checkpoint**, client-side, on molt
WASM+WebGPU. No precompute, no server. That is the destination.

Three phases (we want to walk all three with you):
- **Phase 1 (NOW — this bundle):** compile the two pure-numpy/scipy kernels below → the browser
  re-solves the partition + topology live from embedded weights.
- **Phase 2:** the frozen SegNet (EfficientNet-B2, `smp.Unet`) in WASM/WebGPU → the **realized
  d_seg loop** runs in-browser (paint RGB → SegNet argmax → disagreement), the actual contest
  authority, live. Bold, but you're the team that can.
- **Phase 3:** WebGPU compute shaders for the per-pixel EDT/filters → full framerate at 384×512.

## Compile targets — WASM-CPU **and** WebGPU/WGSL (use both)

Both backends are in scope, and the two kernels map onto them differently — please leverage
whichever fits each op:
- **WASM-CPU** = the **correctness baseline + determinism authority.** It must match
  `reference_outputs.npz` under the gates below. This is what `check_parity.py` validates first.
- **WebGPU / WGSL** = the **speed path** for the embarrassingly-parallel pieces:
  - *Kernel B forward* is **per-coordinate independent** — `sin/cos` curvelet features + the
    FiLM-MLP are one WGSL workgroup-per-pixel matmul/trig kernel. Ideal GPU fit; ~P=H·W lanes.
  - *Kernel A field-solve* is **stencil/filter** work — `gaussian_filter`/`maximum_filter`/
    `minimum_filter` are separable convolutions (great WGSL); `distance_transform_edt` is the
    exact Euclidean EDT (scipy = Maurer/Felzenszwalb-Huttenlocher lower-envelope — **exact**).
    A separable parabola lower-envelope parallelizes well and stays exact; **jump-flood (JFA) is a
    *different, approximate* GPU EDT** — great for an interactive preview, but it can exceed the
    parity atol, so the **authority** pass must use an exact EDT or fall back to WASM-CPU.
- **Hybrid is welcome and the implementation is YOURS.** CPU/GPU split, WGSL compute, storage
  buffers, jump-flood EDT, SIMD-WASM (`deploy/browser/simd-ops.wasm` looks perfect for the MLP),
  whatever mix you choose — you're the team that knows molt best, so pick the implementation; we
  won't prescribe it. The only thing we hold fixed is the **contract** (the determinism gates +
  `check_parity.py` PASS): one path must reproduce the numpy-fp32 reference within atol to serve
  as our authority; everything else (a faster preview path, a hybrid dispatch) is your call.

## EXACTLY what to compile (two kernels, both real, both proven)

Bundle: `pact_witness_kernel/` (runnable; `python field_solve.py` / `witness_forward.py` reproduce
the references; `verify_against_tac.py` proves fidelity; `check_parity.py` is the oracle).

**Kernel B — `witness_forward.py` : `field_solve` input generator (the INR).**
`levelset_argmax(params, cfg, coords, pair_idx, H, W) -> lstar (H,W) uint8`.
Pipeline: `coords (P,2)` → `curvelet_feats` (`[sin,cos]` of `2π·coords@B`, B = deterministic
curvelet bank) → FiLM-modulated MLP (4 hidden, HOSC `tanh(β·sin)` activation) → linear 5-class
SDF head → `argmax`. Fixture: `witness_weights_sample.npz` (synthetic seeded weights, real
shapes). Reference: `witness_forward_reference.npz`.
ops: `matmul, sin, cos, tanh, exp, maximum, argmax, reshape, stack`.

**Kernel A — `field_solve.py` : the Morse-Smale field-solve (the interactive payload).**
`field_solve(lstar (H,W) uint8) -> dict` (11 arrays): per-class signed-distance φ (EDT) → argmax
/ margin / triple-junction gap → boundary → Morse-Smale critical points (maxima/minima/saddles) →
separatrix eigvectors → curvature → distance field. Fixture: `lstar_sample.npz`. Reference:
`reference_outputs.npz`.
ops: `distance_transform_edt, gaussian_filter, maximum_filter, minimum_filter, label` +
`argmax, sort, gradient, percentile, argsort, where, bincount, clip, linalg.eigh`.

**Done = `python check_parity.py candidate_outputs.npz` → PASS**, where `candidate_outputs.npz`
is your WASM run of `field_solve(lstar_sample)`. (And for Kernel B: WASM `levelset_argmax` ==
`witness_forward_reference.npz["lstar"]`, exact uint8.)

> **Kernel B argmax caveat (transcendental precision):** the forward is `sin/cos/tanh/exp` in
> float64, and IEEE-754 does NOT mandate correctly-rounded transcendentals, so a WASM libm can
> differ from numpy — most at LARGE `sin/cos` arguments (`2π·coords@B` reaches ~140 rad here, the
> worst case for argument reduction). On the synthetic fixture the min argmax margin is ~7e-5
> (a few pixels < 1e-3), so exact-uint8 holds; but the **real** witness boundary is where margins
> are smallest, so expect near-ties there. If exact-uint8 is too strict on real φ, we'll switch the
> Kernel-B gate to an **argmax-margin tolerance** (disagreements allowed only where top1−top2 < ε,
> with a small declared pixel budget). Flag it if you see it — accurate-or-caveated, never silently off.

## Determinism gates (precise — this is the part that bites)

Our whole program runs on **numpy-fp32 as the bit-identical verdict authority**; a WASM op that
silently diverges breaks reproducibility. The gates, by status:

**KERNEL-OWNED (already canonicalized in our code — WASM need NOT match a fragile convention):**
| gate | risk | our fix (in `field_solve.py`) |
|---|---|---|
| `np.linalg.eigh` eigenvector **sign** | LAPACK returns `v` or `−v` arbitrarily | canonicalized to first-nonzero-component-positive |
| **critical-point keep-cut selection** (top-40 max / top-120 min) | the value at the cut is **massively tied** (e.g. 630/672 tied on the fixture), so which points survive depended on `where()`/`nonzero()` enumeration order — a GPU/unordered-compaction impl would pick a *different set* | selection is now by canonical `(value, row, col)` `lexsort` → **data-determined, enumeration-independent** (your `where`/`label` may return any order) |
| **critical-point + saddle output order** | `label`/`where` enumeration order leaked into `crit_*_rc` / `saddle_eigvec` row order | all crit arrays are emitted in canonical `(row, col)` order; AND `check_parity` compares `crit_*_rc` as sets and `crit_saddle_eigvec` order-robustly (lexsort by its self-coords) → your output order does not matter |

**WASM↔scipy PARITY-REQUIRED (you must match scipy semantics; these are the real asks):**
| gate | exact requirement |
|---|---|
| `distance_transform_edt` | **exact Euclidean** — scipy uses the exact Maurer/Felzenszwalb-Huttenlocher lower-envelope algorithm (FH **is exact**, not an approximation), `sampling=1`, same tie handling. This drives `dist`, `sdf_margin_m12`, `sdf_gap13` — the dominant float fields. NOTE: jump-flood (JFA) is a *different, approximate* GPU EDT — fine for a preview pass, but it can exceed `atol` on adversarial layouts, so the **authority** path must use an exact EDT (separable lower-envelope) or fall back to WASM-CPU. |
| `gaussian_filter` | `mode="reflect"` (scipy default), `truncate=4.0`, separable; accumulation dtype = input dtype (float64 in curvature, float32 in `m_smooth`). |
| `maximum_filter` / `minimum_filter` | square footprint `size=15` / `size=11`, `mode="reflect"`. **Exact-equality gate:** `m == maximum_filter(m)` selects critical pixels — the compiled filter must return the *bit-exact* same float at the extremum or the critical-point set changes. |
| `label` (connected components) | default **4-connectivity** structure; component enumeration drives triple-junction centroids. |
| `np.percentile` | `interpolation="linear"` (numpy default). |
| `eigh` eigen**values** ordering | ascending (we take `v[:,0]`); only ordering matters now that sign is canonical. |

**Float tolerance:** `check_parity.py` uses `atol=1e-3` on float fields and **exact** on
`sdf_argmax`/`boundary`/critical-point integer coords. EDT/gaussian on a fixed grid should agree
to ~fp32 rounding; a larger drift means the op diverged — please surface it, don't widen the atol.

## Pointers (canonical source — fidelity oracles, rule-118-FREE generic algorithm)
- Kernel B forward: `src/tac/boundary_math/lever_b_levelset_generator.py`
  (`curvelet_directional_B`, `curvelet_feats`, `numpy_levelset_forward`, `levelset_argmax`).
- Kernel A field-solve: `tools/render_witness_morse_smale_viz.py`
  (`_sdf_top_fields`, `_critical_points`, `_boundary_curvature`, `_signed_dist_to_boundary`).
- Repos: `pact`/`tac` (the witness + viz) and `comma_lab` (research-state custody). **Kernel B** is
  proven `==` its tac source via `verify_against_tac.py` (ALL-MATCH). **Kernel A** is a faithful
  extract of the viz field-fns with two intentional determinism canonicalizations (tie-robust
  keep-cut + eigvec sign) → it is NOT bit-identical to the viz; its authority is `reference_outputs.npz`.
- comma10k class order (for the viz palette): 0=Road 1=Lane 2=Undrivable 3=Movable 4=MyCar.

## The two asks that unblock the rest
1. A prebuilt **`molt-embed` example .wasm + 5-line JS loader** we can dogfood without a
   from-source Rust/WASM build (the pact machine is still sharing one GPU with a live score-run).
2. Compile **Kernel A first** (it's the scipy.ndimage stress-test + the interactive payload);
   then Kernel B (pure numpy). PASS on `check_parity.py` is the milestone.

Provenance: generated under numpy 1.26.4 / scipy 1.17.1. `lstar_sample.npz` is a deterministic
*synthetic* road-scene partition (sufficient for numerical parity); a real witness-φ bundle
follows once our GPU-run constraint clears — it changes input values, not the kernels or gates.

**Optimize for whatever is most performant — and bring the magic.** WASM-CPU, WebGPU/WGSL, SIMD,
any hybrid, and any inventive trick you can dream up — the single invariant is that ONE path
reproduces the numpy-fp32 reference within the parity gate to serve as our determinism authority;
past that, go as fast and as clever as molt can. Surprise us. You know the runtime best; we'll
match your ambition. 🚀
