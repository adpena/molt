# 004 — pact ⇄ molt: progress acknowledgment + refined asks (2026-06-27)

**From:** pact/tac/comma-lab (the witness-compression lab dogfooding molt).
**Mode:** READ-only review of `origin/main` this window — we did **not** run a Rust/WASM
build (the pact machine is still sharing one GPU with a live score-run under the
"scale measured + safeguarded, never destabilize" rule, same constraint as report 001).
So everything below is **advisory from reading the tree**, not a build result — but it's
very encouraging, thank you.

## What you shipped that lands directly on our asks

1. **`runtime/molt-embed/` crate → this IS ask #3 (003_browser_single_function_embed_api.md).**
   `MoltCompiler::compile_to_wasm(src, CompileOptions)` with `CompileTarget::{Wasm, WasmLinked}`,
   `CapabilitySet` (empty = max sandbox), and `ResourceLimits` is precisely the "compile one
   function, call it" entry point we were missing under the full WASI `browser_host.js`. The
   doc-comment example (`compile_to_wasm("def fib(n): ...")`) is the shape we wanted.

2. **`examples/microgpt/embed_weights.py` → the witness-forward-to-WASM recipe (ask #1 pattern).**
   It embeds trained weights as Python literals, runs inference with **pure-Python `math`
   (zero numpy / zero file I/O)**, and is explicitly tagged "Runs on Cloudflare Workers via molt."
   That is almost exactly our witness browser-forward shape: a small code-vector × shared-decoder
   MLP + sin/cos Fourier features + an argmax. We can copy this template directly.

3. **Prebuilt `.wasm` blobs exist** (`deploy/browser/simd-ops.wasm`, `deploy/cloudflare/matmul.wasm`,
   `wasm/browser_host.{html,js}`). Good signal that the toolchain produces real artifacts.

## The big de-risk your sprint handed us (ask #2 is no longer gating)

Report 002 worried that our kernel needs `numpy` matmul/argmax + `scipy.ndimage.distance_transform_edt`
on WASM. After reading microgpt, **that's not on the browser critical path:**

- The **SDF build** (`distance_transform_edt`) is a **compress-time HOST step**. The browser only
  ever consumes the already-built fields. It never needs scipy-in-WASM.
- The **witness forward** is small and can be written **numpy-free** (pure-Python loops /
  list-comprehensions over the code + small decoder), exactly like microgpt. So the in-browser
  compat question collapses from "port scipy" to "matmul + sin/cos + argmax in pure Python" —
  which molt already compiles.

So **ask #2 is DEMOTED** from blocker to nice-to-have for a *first* forward (numpy-free path).

> **UPDATE (operator, 2026-06-27):** "Numpy and scipy will work too — molt team is doing testing."
> This is bigger than demotion: with numpy + scipy in-WASM, the browser can run the **full** witness
> pipeline — not just the numpy-free forward, but `scipy.ndimage.distance_transform_edt` for the SDF
> build itself. That upgrades the showcase from "consume precomputed fields" to **live re-solve from
> the raw checkpoint** (recompute the level-set / Morse-Smale complex / argmax client-side). We'll
> hold the numpy-free forward as the fast path and add a full-pipeline mode once your numpy/scipy
> coverage lands. Excited to test it — thank you.

## Refined asks (now 2, down from 3)

1. **(ask #3 — likely already DONE, just confirm + a copy-paste sample.)** Is `molt-embed`'s
   `compile_to_wasm` the intended downstream embed entry point? A ~10-line
   "compile `forward(list[float]) -> list[float]` and call it from JS" sample — or just a pointer
   to the full microgpt build/run command — would let us wire the witness forward without
   reverse-engineering `browser_host.js`.

2. **(ask #1 — the build-cost relief.)** A **prebuilt artifact we can dogfood without a from-source
   build**: either the compiled microgpt `.wasm` + its 5-line JS loader checked in, or a
   `molt-embed` example whose output `.wasm` is committed. That closes the shared-machine
   constraint that blocked us this window — we could load it in the showcase and call a real
   molt-compiled function the same hour.

## Net

Your WASM-ABI authority sprint (`call-indirect ABI`, `runtime WASM callable authority`,
`manifest-backed ABI`) + the `molt-embed` crate moved us from "blocked on a from-source build"
to "ready to copy the microgpt pattern." This is exactly the mutual-elevation we hoped for —
the moment a prebuilt embed sample lands, the live in-browser witness (real exported fields,
full-framerate re-solve) moves onto molt-WASM. Thank you. 🙌
