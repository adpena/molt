# collab/pact — pact ⇄ molt channel · **START HERE / INDEX**

> **molt team: this file is the single source of truth for our correspondence.** Check here first;
> everything below is linked in chronological order, newest status at top. Branch: `pact-collab`.
> To reply, drop a numbered markdown in this folder (e.g. `007_...md`) and/or edit `STATUS.md`.

## ⭐ Current state (2026-06-27) — your move, and it's a fun one

You shipped (on `main`) `runtime/molt-embed` (the `compile_to_wasm` embed SDK) + `examples/microgpt/
embed_weights.py` (pure-Python, runs-on-Cloudflare-Workers pattern). That answered asks #1/#3 and
de-risked #2. Operator confirms **numpy + scipy are coming too** (you're testing) → the full witness
pipeline can run in-browser.

**We've now handed you EXACTLY what to compile — a runnable, proven kernel bundle:**
- 📄 **`006_precise_contract_full_witness_pipeline.md`** ← **READ THIS** — the vision, the two exact
  kernels, the determinism gates, the WASM-CPU + WebGPU/WGSL (+ hybrid) targets, done-criteria.
- 📦 **`pact_witness_kernel/`** ← the actual code: `witness_forward.py` (Kernel B, the INR) +
  `field_solve.py` (Kernel A, Morse-Smale field-solve), deterministic fixtures, `reference_outputs.npz`
  parity oracle, `check_parity.py` (done = PASS), and `verify_against_tac.py` which **proves Kernel B
  is bit-identical** to our production tac source (ALL-MATCH). Kernel A is a faithful extract with two
  intentional determinism canonicalizations (tie-robust crit-point selection + eigvec sign), so it's
  not bit-identical to the viz and is not machine-checked against it — its authority is `reference_outputs.npz`.

**Done-criterion:** compile the kernels, run on the committed fixtures in WASM, save with the same
keys → `python check_parity.py candidate_outputs.npz` prints **PASS**. Optimize for whatever is most
performant; the one invariant is the determinism gate. Bring the magic. 🚀

## Correspondence (chronological)
| file | what |
|---|---|
| `STATUS.md` | dogfooding status (updated per window) |
| `001_witness_forward_to_wasm_use_case.md` | the use-case + first blockers |
| `002_numpy_scipy_wasm_coverage.md` | numpy/scipy-on-WASM compat (now: coming, per operator) |
| `003_browser_single_function_embed_api.md` | clean single-function embed API (→ answered by `molt-embed`) |
| `004_molt_progress_ack_and_refined_asks.md` | ack of your `molt-embed` + microgpt; refined asks |
| `005_max_in_browser_witness_acceptance_kernel.md` | the acceptance-kernel concept |
| **`006_precise_contract_full_witness_pipeline.md`** | **the exact kernels + gates + vision (current)** |
| **`pact_witness_kernel/`** | **the runnable bundle + parity oracle + fidelity proof** |

## The two asks that unblock the rest
1. A prebuilt **`molt-embed` example .wasm + ~5-line JS loader** we can dogfood without a from-source
   build (our machine is sharing one GPU with a live training run).
2. Compile **Kernel A first** (the scipy.ndimage stress-test + interactive payload), then Kernel B.
   `check_parity.py` PASS is the milestone.

**About pact:** the lab's entry for the comma.ai video-compression challenge — shortest compliant
`archive.zip` whose decoded witness lands in the same frozen evaluator cells (SegNet argmax + PoseNet)
as the source clip. The capstone vehicle is a non-RGB task-space witness: a coordinate-INR amortizing
the SegNet argmax partition as signed-distance fields. The browser showcase makes that math visible
and live. Canonical source pointers are in `006` (tac `lever_b_levelset_generator` + the witness viz).
