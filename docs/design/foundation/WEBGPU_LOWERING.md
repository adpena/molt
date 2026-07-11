# Real WebGPU Lowering Doctrine

Status: binding architecture and acceptance doctrine for M03/M74. Scope:
browser WASM compute acceleration, with pact `field_solve.py` as the first
concrete workload. This document does not declare the witness GPU-ready.

## Existing surface: real versus aspirational

Molt already has multiple GPU surfaces. They compose, but are not equivalent.

| Surface | Real today | Not proven by its existence |
|---|---|---|
| `src/molt/gpu/` | Public tensor/kernel API, typed buffers, explicit backend selection, tensor operations, and compiled dispatch entry points. | Arbitrary Python, NumPy, or SciPy calls do not thereby lower to GPU. |
| `src/tinygrad/` | Compatibility namespace routed onto the Molt-owned `molt.gpu` tensor/runtime authority. | It is not a separately vendored upstream tinygrad compiler and must not become a second execution authority. |
| `runtime/molt-gpu/` | Kernel algebra, scheduling/fusion, WGSL renderer, and real `wgpu` device code for shader compilation, buffers, command submission, and readback. | Native `wgpu` execution alone does not connect general Python TIR or the pact witness to browser WebGPU. |
| `wasm/browser_gpu_dispatch.js` and `wasm/browser_gpu_worker.js` | Real browser adapter/device acquisition, WGSL pipeline creation, GPU buffers, dispatch, and copy-back through a generated WASM host import. | The bridge accepts explicitly admitted kernel requests; it is not a general fallback. |
| `wasm-browser-webgpu` profile | Generated, non-default profile with separately gated WebGPU capabilities and declared fail-closed or CPU-WASM fallbacks. | Capability truth is not a witness support claim. |
| WebGPU tests | Renderer/device tests, host-contract tests, required-GPU fail-fast tests, and injected-dispatch tests for admitted tensor linear kernels. | Source-presence or injected-dispatch evidence is not real-adapter parity. |

[`71_wasm_webgpu_numeric_acceleration.md`](71_wasm_webgpu_numeric_acceleration.md)
remains the broad roadmap. This document is the stricter lowering contract.

## Determinism comes before offload

WGSL floating point is not a cross-device bit-deterministic contract. The W3C
specification leaves intermediate rounding direction unspecified, permits
selected subnormal inputs or outputs to flush to zero, permits reassociation and
fusion under stated conditions, and gives operation-specific accuracy bounds.
Even `fma` may be implemented as separate multiply and add operations. See
[differences from IEEE-754](https://www.w3.org/TR/WGSL/#floating-point-differences),
[floating-point accuracy](https://www.w3.org/TR/WGSL/#floating-point-accuracy), and
[reassociation and fusion](https://www.w3.org/TR/WGSL/#floating-point-reassociation).

Therefore:

1. Mathematical equivalence never admits a float kernel.
2. One GPU/browser/driver result never establishes support.
3. A local `atol` gate is insufficient when exact downstream decisions consume
   the float result.
4. A bitwise pact output stays CPU-only unless pact changes the gate or the
   shader proves bit identity across the required adapter matrix.
5. `f16`, relaxed SIMD, fast-math, transcendental approximations, and implicit
   fusion require a distinct non-bit-exact profile.

### Witness gate reality and operation classification

The authority is `field_solve_gates.json`. It currently declares no bitwise
output: `m_smooth` is `atol=1e-3`, while critical-point coordinates are exact row
sets. A Gaussian result can pass its own tolerance but still perturb a comparison,
percentile, local extremum, or label decision and fail an exact downstream gate.

| Class | Witness examples | Eligibility |
|---|---|---|
| Exact, race-free pointwise integer/boolean | boundary-mask comparisons, `where` predicates, proven-range index arithmetic, label equality | **Candidate.** Preserve integer conversion/overflow, bounds, shape, and exact bytes. |
| Float producer with only tolerant observables | isolated `f32` maps contributing to margin, gap, curvature, distance, or eigenvector values | **Candidate with per-output and end-to-end proof.** No tolerance widening. |
| Float producer feeding exact topology | Gaussian/gradient/Hessian values feeding maxima, minima, saddle, mask, or row identity | **CPU-only by default.** Admit only the whole producer-to-exact-decision subgraph after all exact gates pass across the matrix. |
| Order-sensitive selection/reduction | `sort`, `argsort`, `percentile`, reductions, top-k and ties | **CPU-only** until stable total order and tie behavior are represented and proven. |
| Irregular global algorithm | exact EDT, connected-component `label`, general filter border modes | **Not Gear 1.** Requires a real GPU algorithm and adversarial parity fixtures. |
| Small dense linear algebra | symmetric `2x2` `eigh` | **Later candidate.** Witness sign canonicalization does not settle ordering, degeneracy, NaN, or downstream-key behavior. |

The proposed “`m_smooth` bit identity” is stricter than the checked-in manifest.
Molt must not silently impose or relax pact semantics. Pact owns gate types,
tolerances, fixtures, and the release adapter matrix. Molt owns proof against the
unchanged contract.

### Required float acceptance evidence

Every float gear records browser/version, adapter/driver or OS, WebGPU backend,
WGSL hash, kernel-contract version, workgroup size, enabled features, buffer
layout, CPU-WASM and fixture hashes, every output gate, and compile/transfer/warm
dispatch timings. Release support requires representative adapters from every
supported browser backend family; a single machine is exploration evidence only.

## Lowering architecture

### One kernel algebra

Molt reuses `runtime/molt-gpu` scheduling, fusion, dtype/layout facts, kernel
algebra, and WGSL renderer. General Python TIR does not emit ad hoc WGSL and the
browser host does not infer kernels from Python names.

```text
Python / NumPy / SciPy
  -> frontend + typed TIR
  -> explicit GPU eligibility for a closed pure subgraph
  -> molt-gpu kernel algebra and schedule
  -> WGSL renderer
  -> generated host-import ABI
  -> browser GPU worker / WebGPU queue
  -> explicit publication back to WASM
```

The eligibility fact includes dtype, shape/stride/layout, alias and mutation
effects, deterministic acceptance class, required WebGPU features, and CPU
continuation boundary. Unknown facts refuse lowering.

### Dispatch boundary

Offload is a closed array subgraph, not a Python bytecode or arbitrary library
call. A legal region has explicit buffers/scalars; closed dtype, rank, shape,
stride, bounds, alias, and mutation facts; no Python identity, callbacks, dynamic
attribute access, exceptions, or hidden host allocation; and an executable
CPU-WASM continuation. The host executes the exact request or returns a typed
failure; it never substitutes another operation.

### Buffer and lease custody

WASM linear memory and `GPUBuffer` are distinct storage domains. The browser host
owns GPU handles, compiled Python owns Python-visible lifetime, and `Py_buffer`
authority owns exported-view leases.

1. Input leases pin allocation and record shape, strides, format, readonly state,
   and byte extent before upload.
2. Upload uses the logical strided view or a proven contiguous span.
3. GPU outputs remain unpublished while commands are in flight.
4. Readback completes before a CPU/Python consumer observes data unless a future
   device-resident storage fact proves continued GPU custody.
5. Resize, free, storage swap, and conflicting mutation cannot race dispatch.
6. Device loss, validation failure, timeout, or mapping failure releases handles
   and leaves Python-visible state unchanged or explicitly failed.

Gear 1 copies at the boundary. Zero-copy/device residency needs a first-class
storage-location fact, not a JavaScript side cache.

### Loud fallback attribution

Every candidate region records one of `gpu_admitted`, `cpu_wasm_selected`,
`gpu_required_but_unavailable`, or `gpu_validation_failed`. CPU execution is
allowed only when declared by the profile/kernel contract. Diagnostics and proof
artifacts expose the selection. Required-GPU execution fails before output if the
admitted kernel cannot execute; CPU may never masquerade as WebGPU success.

## Staged gears

Each gear is a complete supported island with real-adapter evidence.

### Gear 0: authority and refusal boundary

The generated profile stays non-default; WebGPU capabilities remain separately
gated; existing admitted tensor kernels use the real worker; unavailable WebGPU
fails loudly; and the witness stays CPU-WASM. Acceptance is generator drift,
host-contract, and fail-fast proof. This gear makes no witness acceleration claim.

### Gear 1: exact pointwise witness region

Select one closed race-free integer/boolean boundary-mask region—not Gaussian,
EDT, labeling, sorting, percentile, or `eigh`. Required evidence: typed-TIR
eligibility; one kernel-algebra representation; WGSL only through
`runtime/molt-gpu`; real browser dispatch/copy-back; exact region bytes and
unchanged full witness gates; explicit attribution/fail-fast; and a transfer-aware
timing win.

### Gear 2: tolerant pointwise float

Admit one pure `f32` map whose downstream observables are all tolerant. Prove the
adapter matrix, NaN/Inf/subnormal fixtures, fusion behavior, and unchanged
`1e-3` ceilings.

### Gear 3: deterministic stencil

Admit one fixed-border, fixed-radius stencil with specified accumulation order.
Gaussian is eligible only if its complete downstream topology passes exact gates.

### Gear 4: stable selection and reductions

Represent total order and tie behavior, then prove sort/argsort, percentile, and
reductions. Equal-key ordering cannot be implementation-defined.

### Gear 5: irregular algorithms

Implement EDT and connected components as real GPU algorithms with bounded
memory, synchronization, deterministic labels/canonicalization, and adversarial
fixtures. A CPU call wrapped in a GPU API is forbidden.

### Gear 6: whole-witness scheduling

Keep admitted intermediates device-resident under explicit storage/lease facts.
Acceptance is the real browser `field_solve.py` producing
`candidate_outputs.npz`, passing the unchanged checker, and beating CPU-WASM for
zoom/scrub after compile and transfer accounting.

## Adopt versus greenfield

Upstream tinygrad has a real IR/compiler and WebGPU accelerator; see its
[accelerator documentation](https://github.com/tinygrad/tinygrad#accelerators).
`wgpu` consumes WGSL and supports browser WebGPU on WASM; see official
[`wgpu` documentation](https://docs.rs/wgpu/latest/wgpu/).

- Adopt upstream concepts/fixes, not a parallel runtime. Molt already owns the
  tensor, device, cache, scheduler, and kernel authority.
- Keep `wgpu` for native validation/shared shader semantics; keep the browser
  worker as the product host boundary.
- Do not greenfield another shader language or general shader compiler.
- Do not copy upstream package semantics. Reuse components or general techniques
  through normal dependency/contribution custody.

## Forbidden partials

None of these is WebGPU lowering: a target flag without an admitted kernel;
source-presence tests without real dispatch; injected dispatch presented as GPU
execution; CPU hidden behind a WebGPU label; JavaScript NumPy/SciPy semantics; a
one-off witness shader bypassing `runtime/molt-gpu`; widened gates or changed
fixtures; device-specific goldens as a portable contract; or unprofiled `f16`,
relaxed SIMD, fast-math, reassociation, or subgroup behavior.

## Operator and pact decisions

Molt recommends but does not decide the release adapter matrix, gate changes,
GPU-required fallback policy, zoom/scrub fixture families, or whether non-bit-
exact acceleration is a separate user-visible mode. Until those are recorded,
Molt preserves current gates, keeps exact-downstream float chains CPU-only, and
treats Gear 1 as the first valid witness aperture.

