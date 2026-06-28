# 001 — use-case: compile the witness forward to WASM(+WebGPU) for a live browser viz

- **Priority:** high (it's the operator's stated punchline for the pact showcase)
- **Kind:** use-case + blockers

## What we want
Compile a small pure-numeric Python core to WASM (and ideally a WebGPU compute path) and call it from
the browser so UI sliders re-solve it live. The pact core is the **level-set witness forward**:

```
feats   = curvelet_features(coords, B)          # (HW, F)  sin/cos of coords @ B
h0      = act(feats @ in_proj.T + b0)           # (HW, hidden)
for L:  h = act((h @ Wl.T + bl)*(1+film0) + film1)   # FiLM-modulated hidden stack
phi     = h @ out_sdf.T + b_sdf                 # (HW, K)  the 5 SDF logits
argmax/softmax/margin/separatrix/morse-smale over phi   # the live re-solve
```
All of it is dense float matmul + elementwise `sin/cos/tanh/clamp` + an argmax reduction. No I/O, no
exec/eval, no reflection — exactly the "verified subset" molt targets. This is an ideal molt kernel.

## Blockers this window
1. **Build cost on a shared machine.** Needs `uv sync --group dev` + Rust build + (`wasm-ld`,
   `wasm-tools`) for the linked WASM target. The pact host shares a live GPU score-run under a
   no-destabilize rule, so we didn't run a from-source toolchain build. → see report 003 for the
   "prebuilt artifact + minimal embed" ask that removes this.
2. **numpy coverage uncertainty** for `@` (matmul), `np.sin/np.cos`, `argmax(axis=-1)`, broadcasting.
   → report 002.
3. **The self-orient step uses `scipy.ndimage.distance_transform_edt`** (tangent field from the
   argmax boundary). That's the one non-trivial dependency. For a viz we can pre-bake the directional
   feats (we already do) so the *browser* kernel is the numpy-only forward above — but a WASM EDT
   intrinsic would let the whole pipeline (including SDF construction from labels) run client-side.

## Proposed shape of a fix
- A documented "numeric kernel → WASM export" recipe: a Python module exposing
  `def forward(code: list[float], ...) -> list[float]` that `molt build --target wasm` turns into a
  callable, plus the JS glue to invoke it with a typed array.
- Bonus: route the per-pixel matmul to the WebGPU worker (`browser_gpu_worker.js`) via a documented
  compute-shader entry, since the forward is embarrassingly parallel over HW pixels.

## Impact
Unlocks the "drag tau/frame/class → the full-resolution partition re-solves live on the browser GPU"
experience (the operator's explicit ask) and makes molt the runtime for all future pact viz.
