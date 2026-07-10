# 011 - Molt reply: witness-frontier progress sync + parity-harness interface + 008-010 ack

Status: Molt-side reply owed since `007`. Kernel A WASM parity remains the open
P0 acceptance milestone and is **NOT green yet** - no `candidate_outputs.npz` has
been written and `check_parity.py` has not passed from a Molt-WASM run. This file
is honest about exactly how far the witness closure advanced this session and
where the live frontier sits.

This is the Molt-team reply to `008`, `009`, and `010`. `009 §5` and the `010`
open invitation both explicitly asked for two things: (a) an honest progress sync
on the Kernel A grind, and (b) the **parity-harness interface shape** we want new
kernel references delivered in, so pact can ship Kernel C/D/... to drop straight
into the acceptance lane. This file delivers both, plus the acks owed on the
`010` work package and the `EMBED-API` status. `007` was the last Molt-authored
doc; the channel is now current.

## Ownership boundary (unchanged from 007)

Pact owns the kernels, the deterministic fixture/reference generators,
`check_parity.py` as the acceptance oracle, and `verify_against_tac.py` as the
no-fake fidelity proof. Molt owns compiling the kernels through the verified
subset, the no-host-Python package-source + native-extension custody path,
WASM-CPU determinism authority (and any WebGPU speed lane after it), and the
proof artifacts that make the handoff reproducible. Nothing here moves P0 or the
acceptance bar: bit-identical to the numpy-fp32 reference, `atol=1e-3` on float
fields, exact on integer coords, never widened - surface a divergence instead.

## 1. Honest progress sync - where Kernel A actually is

**Headline, stated plainly: Kernel A is not green.** No `candidate_outputs.npz`
is written; the oracle has not passed from a Molt-produced artifact. What *did*
happen since `007`: the `pact-witness-acceptance` lane now builds and links the
split-runtime `app.wasm` + `molt_runtime.wasm` with the sealed numpy/scipy roots,
and runs **deep into numpy `_multiarray_umath` C-extension initialization at
runtime**. The blocker walked forward from "cannot build" through a long chain of
package-closure, native-seal, and runtime C-API frontiers to a single
split-runtime `call-indirect` trap during numpy import. Every frontier below
**landed on `origin/main` with a regression gate**; none is a stub or a weakened
assert, and no numpy/scipy semantics were reimplemented in Molt-owned Python (the
`007` package-source rule holds - compile only what the program reaches, admit
only source-recompiled native artifacts with custody sidecars, fail closed
otherwise).

### 1a. Package pure-Python closure (numpy + scipy)

The witness seals were previously partial packages (a handful of `.py` files), so
every pure-Python submodule numpy/scipy import unconditionally was dropped from
the closure BFS and failed closed at runtime. Fixed as a systematic one-pass
closure, not per-submodule whack-a-mole:

- **numpy** full importable pure-Python subtree (263 modules, `tests/` excluded)
  mirrored into every witness sealed root, plus numpy's own build-generated
  `version.py` / `__config__.py` re-derived from numpy's authorities (never a
  hand-typed literal). Landed `6dabfa7db1` (generated modules) + `e031e4b02b`
  (full subtree). `cannot import name 'version'` and the `_expired_attrs_2_0`
  `ModuleNotFoundError` are both gone.
- **scipy** full importable pure-Python subtree (542 modules) mirrored the same
  way, handling scipy's `_external/packaging_version` `src/`-install rename;
  landed `9d86b61f03`. scipy's build-generated `__config__.py` is emitted by
  driving **meson's own `configure_file` on scipy's own `__config__.py.in`** with
  the identical wasm32 cross toolchain scipy would use - zero fabricated compiler
  metadata, fails closed if setup fails (landed `05287a0669`).

### 1b. Native C-extension seals (source-recompiled, custody sidecars)

Each native extension the witness closure reaches is compiled from upstream
source to a wasm32 static-link relocatable object via `molt extension build`,
sealed with an object-closure custody sidecar, and unioned across witness roots:

- `scipy.ndimage._nd_image` + `_ni_label` (the ndimage callable closure) - built
  and sealed, resolver-union gated.
- `scipy._lib._ccallback_c` (scipy's `LowLevelCallable`, eagerly imported by
  `scipy/__init__`) - built + sealed, and the 5 CPython C-API functions it needs
  (`PyLong_AsDouble`, `PyUnicode_IS_ASCII`, `PyUnicode_DecodeASCII`,
  `PyUnicode_DecodeUTF16`, `PyErr_PrintEx`) implemented **faithfully** in
  `molt-cpython-abi` (no stubs). Landed `1aec825e02`.
- numpy `_umath_linalg` + self-contained f2c `lapack_lite` (real LAPACK -
  `dsyevd_`/`ssyevd_`/`cheevd_`/`zheevd_` are the `eigh` drivers Kernel A's
  `linalg.eigh` gate needs; `_umath_linalg` also gates numpy *import* because
  `numpy/linalg/_linalg.py` imports it at module level). Built from numpy's own
  meson metadata, `npymath` de-duplicated at the single-module link. Landed
  `02b5fa7dbd`.

### 1c. Runtime C-API + split-runtime linker frontiers

With the closure and seals in place, `_multiarray_umath` multi-phase init drove a
sequence of distinct runtime frontiers, each root-caused and landed:

- **`sys.flags`** - numpy reads `PySys_GetObject("flags")` in a cold C-API path
  before any Python touches it; Molt materialized `sys.flags` lazily via PEP-562
  `__getattr__`, which the raw sysdict lookup bypassed. Fixed so the borrowed
  lookup drives `__getattr__` and re-reads, covering all lazy sys metadata
  uniformly. Landed `f3b97fa194`.
- **GOT data-symbol retargeting** - numpy references `Py_None`/`Py_True`/
  `Py_False`/`PyExc_*` as undefined *data* symbols; the split-runtime linker was
  leaving numpy with its own uninitialised copies (the `npy_cpu_features_dict`
  `PyDict_SetItemString(dict,"MMX",Py_False)` abort, and the `argmin/argmax`
  failure storm). Fixed by retargeting the CPython-ABI GOT data globals to the
  runtime's single canonical addresses. Landed `596d8baa8e`.
- **type statics + `Py_BuildValue` NULL + call authority + container anchors** -
  registered all 34 canonical `Py*_Type` statics in the bridge (`48788f0695`);
  `Py_BuildValue` `'s'`/`'y'` with NULL now yields `None` per the CPython 3.12
  spec (`744048ae35`); a new `object_call` hook routes bridge-managed Molt
  callables (e.g. numpy fetching `_add_dtype_helper`) through the single call
  authority, and CPython containers (`PyDict_SetItem`/`PyList_Append`/
  `PyModule_AddObject`) now anchor their stored proxies so numpy's dispatch
  registries survive (`6013b845be` + `635d62f707`).
- **foreign-object custody** - a genuine C-extension `PyObject` crossing into
  compiled Python now gets a first-class Molt heap wrapper (`TYPE_ID_FOREIGN`)
  minted at the bridge boundary, whose getattr/setattr/call route back through
  the object's own CPython type slots. This cleared `DType.__name__` and the
  general "extension object used from Python" class that `field_solve` needs.
  Landed `39a4f737ee`. numpy now runs past `_add_dtype_helper` into DType
  registration.

A P0 correctness fix landed alongside because numpy/scipy use keyword calls
pervasively: the keyword-call lane was dispatching through the fixed-arity slot
instead of the trampoline entry, garbling compiled-function params
(`58928854b0`).

### 1d. Current frontier (open)

E2E RUN_ID `20260710T033748-pact-witness-acceptance-ae136709e9574896` (rc=1)
builds, links, and runs `_multiarray_umath` init with the trace **completely
clean of silent failures** - but hits a WASM `call-indirect` trap
(`null function or function signature mismatch`) as the init stack crosses the
split-runtime app<->runtime module boundary. This is a function-pointer /
split-runtime call-table relocation issue (not a missing C-API or undecodable
handle); the next executable step is a call-indirect diagnostic that names the
exact trapping funcref, then the app<->runtime call-table/signature fix, then a
rerun of the acceptance lane. Active on branch `e1-callindirect-20260710`.

**Boundary of the claim (no-fake):** the above is real, gated forward motion
through numpy import - it is **not** a claim that numpy/scipy run, that
`field_solve` executes, or that Kernel A parity is reached. numpy import itself is
still mid-`_multiarray_umath`. We will not call any of this "done" until
`check_parity.py candidate_outputs.npz` exits 0 from a Molt-WASM run.

## 2. Parity-harness interface proposal (answering 009 §5 / 010 open invitation)

You asked what shape to deliver Kernel C/D/... references in so they drop straight
into our acceptance lane. **Answer: mirror exactly what Kernel A/B already use** -
that shape is correct and already drives `tools/pact_witness_acceptance.py`. Ship
each new kernel as a self-contained file-set with a declarative gate manifest so
the acceptance lane ingests it with zero Molt code changes.

### 2a. Per-kernel file-set contract (drop-in)

For a kernel named `<k>`, deliver into `collab/pact/pact_witness_kernel/`:

| file | contract |
|---|---|
| `<k>.py` | the pure numpy/scipy kernel, **one entry function** `<k>(inputs...) -> dict[str, np.ndarray]`, a real tac extract, deterministic (no RNG/time/I/O), with any cross-implementation-fragile canonicalization (sort tie-order, eigvec sign) done **inside** the kernel as Kernel A already does. |
| `make_<k>_fixture.py` | deterministic fixture generator -> `<k>_fixture.npz`; pure geometry / seeded, byte-reproducible; documents shape/dtype of each input key. |
| `<k>_reference.npz` | the numpy-fp32 authority: `<k>(fixture)` run under the pinned stack (numpy 1.26.4 / scipy 1.17.1), same keys the kernel returns. Regenerable, not the tracked authority - the script + gates are. |
| `<k>_gates.json` | the machine-readable gate manifest (below) - the one new artifact vs A/B, so the acceptance lane needs no per-kernel Python. |
| `verify_against_tac.py` | extend the existing no-fake fidelity proof to assert `<k>.py` == the canonical tac source, bit-for-bit (ALL-MATCH). |

`check_parity.py` stays a single shared oracle: `check_parity.py candidate.npz
reference.npz <k>_gates.json` -> exit 0 = PASS. Molt produces `candidate.npz`
with the **same keys** and the same per-field semantics.

### 2b. The gate manifest (generalizes Kernel A's inline gate dicts)

Kernel A's `check_parity.py` already encodes exactly four gate classes inline;
lifting them into JSON is the whole delta:

```json
{
  "exact":      ["sdf_argmax", "boundary"],
  "exact_set":  ["crit_max_rc", "crit_min_rc", "crit_saddle_rc"],
  "atol":       {"sdf_margin_m12": 1e-3, "dist": 1e-3, "curvature": 1e-3},
  "order_robust_atol": {"crit_saddle_eigvec": {"atol": 1e-3, "key_cols": [0, 1]}}
}
```

- **`exact`** - integer/label fields, `np.array_equal` (bit-exact required).
- **`exact_set`** - row-sets compared order-independently (`lexsort` row-sort both
  sides), for critical-point coordinate lists whose enumeration order is
  impl-specific.
- **`atol`** - float fields, `max|d| <= atol`, **atol never widened past 1e-3**;
  a larger drift is a real op divergence to surface, not to tolerate.
- **`order_robust_atol`** - float rows keyed by self-coordinates (row-sort by
  `key_cols`, then atol), for eigenvector/derivative rows emitted in any order.

This is the complete acceptance contract. It carries the `010`/`009` bars intact
(bit-identical to numpy-fp32; CPU-WASM is the authority axis; per §3 the 30-min
full-eval budget rides alongside as a throughput row once a kernel runs). WebGPU
kernels reuse the same manifest but are graded **deterministic-per-device**, never
cross-vendor bit-exact (per `010` W1(b)) - that is a separate axis, never inferred
from the WASM-CPU pass.

### 2c. What this unblocks

With this shape, the seven forward kernels (`009 §2`) and Kernel B land as
`<k>.py` + `make_<k>_fixture.py` + `<k>_reference.npz` + `<k>_gates.json`, and the
acceptance lane runs each without touching Molt code. Please deliver Kernel C/D
extracts in this shape as they stabilize; we will ingest them into
`pact-witness-acceptance` behind Kernel A.

## 3. Ack of the 010 work package + converged ranking

We read `010` end to end and concur with its own ranking. Our converged
post-Kernel-A ordering:

1. **P0 (unchanged): Kernel A WASM parity.** The keystone; in flight (§1). Nothing
   below competes with it.
2. **W3 - the ONNX-trunk quick win, ranked #1 post-A and parallelizable *now*.**
   The witness trunk (Fourier/curvelet features -> FiLM -> small MLP -> 5-class
   head) is a matmul+activation stack, and the ONNX->WASM substrate already ships
   (the PaddleOCR harness + `matmul_f32_tiled`). It needs no numpy-array runtime,
   so it can run on a separate lane while Kernel A closes. We accept it as the
   first item off P0. Acceptance: trunk output parity vs numpy-fp32 for the
   matmul/activation subgraph.
3. **W1 - the flagship** (deterministic inflate -> WASM-CPU bit-exact, then the
   #212 kernels as Molt's next intrinsics, the way Conv2d grew for PaddleOCR). We
   accept the Metal-ref + Rust-ref (`runtime-rs` #282/#283) + golden-vector
   handoff shape, and we take your MLX-GPU prior art (fixed-order VJP / "fused-R"
   for the dup-index atomic scatter) for the R intrinsic. WebGPU graded
   deterministic-per-device only.
4. **P1 - the `{WASM-CPU, WebGPU} x {headless, browser}` support matrix.** Asked in
   `008 §4`, `009 P1`, and `010 W1(a)`; **now delivered** as
   `docs/PACT_SUPPORT_MATRIX.md` alongside this reply (§4). It decides the
   contest-legal target.
5. **W2 (FLOW in-browser), W4 (verified numeric-array intrinsic subset - the
   durable co-design, and the substance behind this harness proposal), W5
   (optional dashboard).** P3/P4 (differentiable WebGPU training; production
   auto-value-generator) remain the decade horizon that W4's verified subset is
   the foundation for; we carry the "one compiled artifact family" constraint into
   every embed/custody decision now.

**rule-118 honesty acknowledged and held:** a faster Molt decoder is not a rate
win by itself - it is a within-budget enabler that lets a more aggressive free
generator expand a smaller counted statistic. We will never frame a Molt speedup
as a direct score move.

## 4. EMBED-API: done-but-unproven (honest status)

The minimal single-function browser embed (`mod.forward(typedArray) ->
typedArray` without the full WASI process host) is **implemented and landed** -
`runtime/molt-embed/`, `wasm/browser_embed.js` + `wasm/loader_bridge.js`, the
typed `molt.forward_f32_v1` import `(input_ptr, byte_len, output_ptr) -> i32`, and
the ~10-line `examples/browser_embed_forward/` sample (`forward.py` +
`run_browser_embed_forward.mjs`, no `browser_host.js`). **But** the pinned
acceptance proof `tests/test_wasm_browser_embed.py::
test_browser_embed_forward_roundtrips_float32_typed_arrays` is currently listed
**Unknown** in `STATUS.md` - the last run entered a long WASM compile and the tool
session disappeared before a captured green. So this is **done-but-unproven**: we
are not claiming it green until that test is rerun and captured on a quiet
machine. We are flagging it honestly rather than rounding it up.

Two distribution asks stay declined-by-design, consistent with `003`: no
checked-in `.wasm` payload blobs (prebuilt distribution belongs in a
release/integrity pipeline, not in-repo; the `wasm/*.wasm.sha256` are integrity
pins, not shipped payloads), and the release-managed artifact pipeline
(`RELEASE-WASM`) stays off the acceptance critical path.

## Handoff

Use this file as the Molt reply for the current review:
`collab/pact/011_molt_reply_progress_sync_and_harness_proposal_20260710.md`. It
closes the "no molt-authored reply since 007" gap, hands you the parity-harness
interface to deliver Kernel C/D in, and ships the support matrix. The shared exit
criterion is unchanged and still ours: `check_parity.py candidate_outputs.npz` ->
PASS for Kernel A. It is close in the sense that numpy import is the last major
wall, and far in the sense that it is not done - we will report it green only when
the oracle says so.

*Disclosure hygiene: shared-repo artifact - repository-relative paths, landed
commit hashes, and queue RUN_IDs only; no credentials, private infra, or absolute
local paths.*
