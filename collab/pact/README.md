# collab/pact - pact <-> molt channel / START HERE

This folder is the correspondence and handoff index for the Pact browser
witness work. The live codebase and executable proofs are authoritative; these
notes route the next structural move.

## Current State (2026-07-01)

The requested end criterion is still the Kernel A oracle:

```powershell
python check_parity.py candidate_outputs.npz
```

`PASS` is the milestone, with the same generated reference keys as
`pact_witness_kernel/reference_outputs.npz`.

Heavy browser/WASM witness attempts are queue-native. Check queue state first,
then launch the owned Pact lane instead of running raw `molt build` or browser
proof commands:

```powershell
uv run --active --project . --python 3.12 python tools/proof_queue.py status
uv run --active --project . --python 3.12 python tools/proof_queue.py pact-witness-acceptance --detach
```

`pact-witness-acceptance` auto-admits conventional staged NumPy/SciPy
static-link artifact roots when they exist under `tmp/`, so the default lane is
the manifest-led package-native closure path. The selector is priority ordered:
the current canonical roots are `tmp/pact_numpy_multiarray_sealed_for_witness`
and `tmp/pact_scipy_ndimage_sealed_for_witness_next`, with older recovery roots
kept only as fallback evidence. Use `--print-spec` to inspect the selected roots,
or `--env MOLT_MODULE_ROOTS=... --env MOLT_EXTERNAL_STATIC_PACKAGES="numpy scipy"`
for an explicit power-user lane.

The smallest current parity proof for the witness oracle is also queued:

```powershell
uv run --active --project . --python 3.12 python tools/proof_queue.py pact-witness-oracle --detach
```

That oracle lane regenerates `lstar_sample.npz` and `reference_outputs.npz` in a
temporary directory, then runs `check_parity.py reference_outputs.npz`. The
browser/WASM acceptance lane remains the owner for producing a real
Molt-generated `candidate_outputs.npz` once package-native closure reaches that
point.

The named acceptance lane is owned by `tools/pact_witness_acceptance.py`: it
builds `field_solve.py`, executes the emitted WASM through the canonical runner
from an isolated fixture directory, renames the Molt-produced
`reference_outputs.npz` to `candidate_outputs.npz`, then runs
`check_parity.py candidate_outputs.npz reference_outputs.npz`. Queue row
`20260701T203840-pact-witness-acceptance-43e969d640e44709` proves the build and
link half of that lane, then fails during Node execution with
`RuntimeError: null function or function signature mismatch` before
`candidate_outputs.npz` exists. The next executable proof is therefore the
runtime call-table/signature closure behind that trap:

- The first native-callable lowering gap is retired: manifest-declared
  direct-symbol exports for the `scipy.ndimage` witness operation closure now
  lower to executable `invoke_ffi` ABI metadata without granting
  `known_modules` fake Python direct-call authority.
- The manifest-led package-native plan now closes for the first SciPy ndimage
  native sidecar layer: existing sealed roots select
  `numpy._core._multiarray_umath`, `scipy.ndimage._nd_image`, and
  `scipy.ndimage._ni_label`; publish exactly the five Kernel A ndimage callable
  exports; and stage provider support modules `_morphology`, `_filters`,
  `_measurements`, `_ni_support`, `scipy._lib._util`, and `numpy.exceptions`.
- Adding NumPy/SciPy source roots without package admission correctly fails
  closed.
- Adding package admission against the local Python 3.14 site-packages root still
  fails closed before graph expansion because the installed NumPy/SciPy roots
  contain native markers but do not publish wasm32 `static_link`
  `libmolt_source` artifact manifests with package symbol custody.
- Molt now has the producer-side command contract for package-native artifact
  shape: `molt extension build --target wasm` emits a wasm32 static-link
  `.molt.wasm` artifact and `extension_manifest.json` with direct-symbol,
  capsule, and object-closure custody. Sealed witness roots must publish
  precise `python_exports`, `callable_exports`, checksummed provider support
  files, and target-compatible capsule providers.
- An earlier package-admission probe timed out after 300s in the live WASM build
  path.
- A graph-only probe took 100.4s before backend work, found 186 modules, zero
  staged native artifacts, and pulled broad NumPy plus `scipy` and
  `scipy.ndimage` package initializer closure.

That makes the next structural unit the live Kernel A runtime/parity closure:
turn the first missing runtime call-table/signature, C/API, ndarray/storage, or
buffer primitive exposed by the Node trap into a shared Molt ABI surface, then
rerun the queued full acceptance lane. It is not a Molt-owned Python shim and
not a checked-in browser artifact bundle.

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
