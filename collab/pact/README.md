# collab/pact - pact <-> molt channel / START HERE

This folder is the correspondence and handoff index for the Pact browser
witness work. The live codebase and executable proofs are authoritative; these
notes route the next structural move.

## Current State (2026-06-29)

The requested end criterion is still:

```powershell
python check_parity.py candidate_outputs.npz
```

`PASS` is the milestone, with the same generated reference keys as
`pact_witness_kernel/reference_outputs.npz`.

The live recovery evidence says the current tree cannot honestly produce that
candidate yet:

- The first native-callable lowering gap is retired: manifest-declared
  direct-symbol exports for the `scipy.ndimage` witness operation closure now
  lower to executable `invoke_ffi` ABI metadata without granting
  `known_modules` fake Python direct-call authority.
- The remaining blocker is package-native execution custody: the live Pact
  build still needs reachable NumPy/SciPy native artifacts, ndarray/storage and
  buffer primitives, and C/API symbol closure before the emitted WASM can link
  and execute those upstream extension symbols.
- Adding NumPy/SciPy source roots without package admission correctly fails
  closed.
- Adding package admission against the local Python 3.14 site-packages root now
  fails closed before graph expansion because the installed NumPy/SciPy roots
  contain native markers but do not publish wasm32 `static_link`
  `libmolt_source` artifact manifests with package symbol custody.
- Molt now has the producer-side command contract for that missing artifact
  shape: `molt extension build --target wasm` emits a wasm32 static-link
  `.molt.wasm` artifact and `extension_manifest.json` with direct-symbol and
  object-closure custody. Source-recompiled NumPy/SciPy artifacts must also
  publish `python_exports`/`callable_exports`; package-root imports such as
  `numpy` need matching `python_exports` ownership rather than child artifact
  ancestry. The installed NumPy/SciPy roots still need reachable
  source-recompiled artifacts in that shape.
- An earlier package-admission probe timed out after 300s in the live WASM build
  path.
- A graph-only probe took 100.4s before backend work, found 186 modules, zero
  staged native artifacts, and pulled broad NumPy plus `scipy` and
  `scipy.ndimage` package initializer closure.

That makes the next structural unit package-native closure for Kernel A, not a
Molt-owned Python shim and not a checked-in browser artifact bundle.

Once package-native closure exists, replay Kernel A `field_solve(lstar)` first
because it is the SciPy ndimage stress test and interactive payload, then run
Kernel B `witness_forward.levelset_argmax` against
`witness_forward_reference.npz`.

## Correspondence

| file | what |
|---|---|
| `STATUS.md` | dogfooding status and current blockers |
| `001_witness_forward_to_wasm_use_case.md` | use case and original blockers |
| `002_numpy_scipy_wasm_coverage.md` | numpy/scipy-on-WASM compatibility questions |
| `003_browser_single_function_embed_api.md` | single-function browser embed API and recovery evidence |
| `004_molt_progress_ack_and_refined_asks.md` | ack of `molt-embed` and refined asks |
| `005_max_in_browser_witness_acceptance_kernel.md` | acceptance-kernel concept |
| `006_precise_contract_full_witness_pipeline.md` | exact kernels, gates, and vision |
| `007_molt_response_numpy_scipy_c_api_greenup_and_witness_kernel_plan.md` | ABI/package-native plan notes |
| `008_addendum_v2_witness_decoder_20260629.md` | decode-chain, contest-runtime contracts, and runtime-rs sister-backend notes |
| `pact_witness_kernel/` | runnable bundle, fixture oracle, and parity script |

## The Two Asks That Unblock The Rest

1. Browser-forward dogfood lane:
   `examples/browser_embed_forward/` contains source plus a plain JS runner for
   the existing split-runtime `wasm/browser_embed.js` authority. Generated WASM
   artifacts are outputs, not checked-in source and not a second embed lane.
   Package-native `molt.forward_f32_v1` now lowers to a typed
   `(input_ptr, byte_len, output_ptr) -> status` WASM import and is satisfied by
   browser `Float32Array` memory views, not a boxed native-call shim.
2. Kernel A first:
   `field_solve.py` reaches the `scipy.ndimage` closure
   `distance_transform_edt`, `gaussian_filter`, `maximum_filter`,
   `minimum_filter`, and `label`, plus NumPy ndarray operations including
   `sort`, `argmax`, `percentile`, `where`, `lexsort`, `gradient`, `clip`,
   `stack`, and `linalg.eigh`. NumPy/SciPy source admission must stage native
   artifacts, C/API symbols, ndarray/storage/buffer primitives, and a
   tree-shaken object closure before `candidate_outputs.npz` is a real target.

## About Pact

Pact is the lab's entry for the comma.ai video-compression challenge: the
shortest compliant `archive.zip` whose decoded witness lands in the same frozen
evaluator cells (SegNet argmax plus PoseNet) as the source clip. The capstone
vehicle is a non-RGB task-space witness: a coordinate-INR amortizing the SegNet
argmax partition as signed-distance fields. Canonical source pointers are in
`006_precise_contract_full_witness_pipeline.md` and `pact_witness_kernel/`.
