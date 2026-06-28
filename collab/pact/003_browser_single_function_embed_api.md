# 003 — DX: a minimal "call one compiled function from the browser" embed recipe

- **Priority:** medium
- **Kind:** developer experience / docs + (maybe) a small shim

## Ask
A documented, minimal path to: (a) ship a prebuilt `molt_runtime.wasm` + a user module compiled to
wasm, and (b) from plain JS, call `mod.forward(typedArray) -> typedArray` — without standing up the
full WASI process host.

## What we found
- `wasm/browser_host.js` (~130KB) + `wasm/run_wasm.js` (~188KB) are a complete WASI-style host
  (sockets, fds, errno, vfs). Great for "run a Python program in the browser", heavier than needed
  for "call one pure function from a viz".
- `wasm/*.wasm.sha256` are present but the actual `molt_runtime.wasm` / `_reloc.wasm` blobs were not
  in our shallow clone (LFS? build artifact?). A downstream consumer can't embed without the blob.

## Proposed
1. A `docs/.../browser-embed-minimal.md`: ~30 lines showing instantiate + call-one-export with a
   `Float32Array` in/out, no sockets/vfs.
2. Ship (or document how to fetch) a prebuilt `molt_runtime.wasm` so consumers can dogfood without a
   from-source Rust/wasm-tools build (important on shared/locked machines — see STATUS.md blocker 1).
3. Optional: a `browser_gpu_worker.js` example that runs a compiled compute kernel over a big array,
   so embarrassingly-parallel numeric kernels (like our per-pixel forward) get the WebGPU path.

## Impact
Removes the single biggest friction for viz/edge consumers: today the on-ramp is "build the whole
toolchain"; the ask is "fetch a wasm + 30 lines of JS".
