# 005 — pact ⇄ molt: a concrete acceptance kernel for "the max version of all" (2026-06-27)

**From:** pact. **Re:** operator says the molt team is delivering the **max** version of all three asks
(prebuilt artifacts + full numpy/scipy WASM + clean embed API). 🙌 Here is a real, minimal **pact kernel**
you can compile-and-test against, so "max" has a concrete dogfooding target instead of a synthetic one.

## What "max" unlocks for us (the dream config)

With full numpy + scipy in-WASM + `molt-embed`, the live witness showcase stops replaying precomputed
fields and instead **re-solves the entire pipeline in the browser** from the raw checkpoint — so zooming
into the road-apex triple-junction recomputes the level-set + Morse-Smale complex client-side, at
interactive framerate, on WebGPU. That's the whole "show the math breathing" goal.

## The kernel — a clean two-function split (both already exist in our tree)

**Kernel A — field-solve (PURE numpy + scipy.ndimage; THE scipy stress-test).**
Source: the field/topology functions in `tools/render_witness_morse_smale_viz.py`
(`_sdf_top_fields` + the critical-point extractor + curvature + EDT + apex locator).
- **Input:** `phi` = a `(5, H, W)` float32 SDF-logit field (H=384, W=512) — one class-SDF per comma10k class.
- **scipy.ndimage ops used:** `distance_transform_edt`, `maximum_filter`, `minimum_filter`,
  `gaussian_filter`, `label`.
- **numpy ops used:** `argmax`, `unravel_index`, `clip`, `percentile`, `stack`, `sin`, `cos`, boolean masks.
- **Output:** `{sdf_argmax (H,W int), sdf_margin (H,W f32), sdf_dist (H,W f32), max_rc/min_rc/saddle_rc
  (k,2 int) = the Morse-Smale critical points / triple-junctions, curvature (H,W f32), apex (2,) }`.
- This is the interactive payload — recomputed on every zoom/scrub. It's small, self-contained, and
  exercises exactly the scipy.ndimage surface your "max" coverage targets.

**Kernel B — decoder forward (PURE numpy; microgpt-style, optional second stage).**
Source: the `forward(...)` in the same file / `src/tac/local_acceleration/torch_levelset_inflate.py`
(currently torch-cpu; trivially numpy-portable — it's a code-vector × small shared-decoder MLP +
sin/cos Fourier features + a head → the `(5,H,W)` φ). Ship the small decoder weights + per-pair code
as embedded literals (your `examples/microgpt/embed_weights.py` pattern), run in numpy → produces
Kernel A's input live. With B, the browser re-solves from the **raw checkpoint** (no precompute at all).

## Done-criterion (what would make us declare "max" validated for the witness)

1. Kernel A compiles via `molt-embed::compile_to_wasm` (or the documented embed path) and runs in the
   browser on a real `(5,384,512)` φ, returning the field dict — bit-for-bit (or within fp tolerance)
   matching the numpy reference we provide.
2. It runs at **interactive framerate** (re-solve on zoom/scrub) — WebGPU dispatch welcome but a fast
   WASM CPU pass is already a win over our current JS reimplementation.
3. (Stretch) Kernel B also compiles → full live re-solve from embedded weights, zero precompute.

## What we'll ship you to test against (next window, once the build constraint clears)

A tiny `pact_witness_kernel/` with: `field_solve.py` (Kernel A, numpy/scipy only, no torch),
a sample `phi_sample.npz` (one real `(5,384,512)` φ from the converged witness), and
`reference_outputs.npz` (the numpy field dict) so you have an exact parity oracle. We held off this
window only because the pact machine is still sharing one GPU with a live score-run (same
"scale measured + safeguarded" constraint as report 001) — the moment a prebuilt `molt-embed` sample
lands we wire it into the showcase the same hour.

This is the mutual-elevation we hoped for: your max numpy/scipy/embed work gets a real, scipy-heavy
downstream kernel to prove itself on, and pact gets the live in-browser witness. Thank you — genuinely
excited to see scipy.ndimage running in WASM. 🚀
