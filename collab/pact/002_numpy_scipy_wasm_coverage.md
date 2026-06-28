# 002 — compat: numpy/scipy coverage on the WASM target (for dense numeric kernels)

- **Priority:** high
- **Kind:** compatibility gap / coverage question

## Ask
A crisp, downstream-readable matrix of what `numpy` (and key `scipy.ndimage`) ops are supported on
the **WASM** target, since that determines whether a kernel compiles before we invest in a build.

The pact witness forward needs (in rough priority):
1. `A @ B` (2D float32 matmul) — the dominant cost
2. `np.sin`, `np.cos`, `np.tanh`, `np.clip`, `np.exp` (elementwise)
3. `x.argmax(axis=-1)`, `x.max(axis=-1, keepdims=True)` (reductions)
4. broadcasting `(HW,hidden) * (hidden,)` and `(HW,K) - (HW,1)`
5. `np.concatenate`, `reshape`, `transpose`
6. (SDF builder only) `scipy.ndimage.distance_transform_edt` — Euclidean distance transform

## Why this matters
Items 1–5 are the whole forward; if those are green on WASM, the live witness re-solve compiles. Item
6 is the only heavy outlier — a WASM EDT intrinsic (or a documented "unsupported, precompute it"
note) would let us decide cleanly whether SDF-from-labels can run client-side too.

## Observed
`docs/design/foundation/*ecosystem_compat_gap*` and `*domain-critical-portfolio*` reference numpy in
the gap-audit surface, which we read as "partial / in-progress" — but we couldn't find a single
"numpy-on-wasm op support" table. A consolidated table (op × target × status) would be high-leverage
for every numeric downstream (pact, and presumably the `gpu_*` examples).

## Proposed
- Publish `docs/.../numpy_wasm_support.md` (op × {native, wasm, wasm+simd128} × {green/partial/none}).
- If matmul/elementwise/reductions are already green on wasm, a one-line example doing
  `phi = feats @ W.T + b; phi.argmax(-1)` compiled to wasm would be a perfect smoke for us.
