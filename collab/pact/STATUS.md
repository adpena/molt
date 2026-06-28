# STATUS — pact dogfooding of molt (2026-06-27)

> **UPDATE (2026-06-27, cycle 2):** Re-read `origin/main` — the team shipped
> `runtime/molt-embed/` (the `compile_to_wasm` embed SDK = **ask #3**) and
> `examples/microgpt/embed_weights.py` (pure-Python, zero-numpy, "runs on Cloudflare
> Workers via molt" = the **ask #1 pattern**). This **demotes ask #2** (numpy/scipy-in-WASM):
> the witness forward can be numpy-free per microgpt, and the SDF (`distance_transform_edt`)
> is a compress-time HOST step that never runs in the browser. Refined asks (now 2) in
> **004_molt_progress_ack_and_refined_asks.md**. Strong progress — thank you.

Positive signal first, then the honest gaps. This is a dogfooding report, not a complaint.

## What worked / what's promising
- **Clone + repo shape**: `git clone` clean (10.4k files). The repo is clearly real and ambitious:
  native AOT + WASM as first-class targets, a WebGPU worker (`wasm/browser_gpu_worker.js`), GPU
  examples (`examples/gpu_*.py`), a full WASI-style browser host (`wasm/browser_host.js`, 130KB),
  and a pyodide A/B harness (`wasm/bench_pyodide.html`). The "smaller/faster than pyodide + WebGPU"
  pitch is visibly backed by code, not just docs.
- **Docs quality**: `README.md` is honest about the contract ("verified subset", "WASM cross-target
  parity still incomplete and actively tracked", "no unrestricted exec/eval/monkeypatch"). The
  `docs/design/foundation/*compat_gap*` audits are exactly the kind of surface a downstream user
  needs to predict whether their kernel will compile. Thank you for keeping these explicit.
- **CLI ergonomics** read well: `molt run app.py` / `molt build app.py --release` / `molt compare`
  mirror cargo conventions — easy to reason about.

## What blocked the pact use-case THIS window (details in numbered reports)
1. **Toolchain build cost vs a shared machine.** `molt` installs via `uv sync --group dev` + a Rust
   build; WASM linked builds additionally need `wasm-ld` + `wasm-tools`. The pact machine is shared
   with a live GPU score-run under a strict "scale measured + safeguarded, never destabilize" rule,
   so we did **not** kick off a multi-minute Rust/wasm toolchain build this window. A **prebuilt
   `molt` wheel / a prebuilt `molt_runtime.wasm` + a 10-line "call one compiled function from JS"
   sample** would let downstream consumers dogfood without a from-source build. (`wasm/*.wasm.sha256`
   exist but the `.wasm` blobs themselves weren't in the shallow clone — see report 003.)
2. **numpy/scipy coverage on WASM** is the gating compat question for our kernel (report 002): the
   witness forward is numpy matmul + sin/cos + argmax (likely partial) and the SDF builder is
   `scipy.ndimage.distance_transform_edt` (almost certainly unsupported). We could not confirm
   coverage without building.
3. **A clean single-function browser embed API** (report 003): `browser_host.js` is a full
   process/WASI host; for a viz we want "load module, call `forward(Float32Array)->Float32Array`",
   ideally dispatched to the WebGPU worker. A minimal documented embed recipe would close this.

## Net
molt is the right long-term runtime for the live in-browser witness (WASM+WebGPU, ours, fast). For
this window we shipped the showcase on vanilla-JS + three.js (which already gives full-framerate live
re-solve from the exported real fields). The reports here are the concrete asks that would let the
**next** iteration move the heavy per-pixel SDF/argmax/Morse-Smale compute onto molt-WASM/WebGPU.
