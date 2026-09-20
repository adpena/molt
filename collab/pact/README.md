# Pact acceptance contract

This directory contains the stable public interface between Pact and Molt. It
defines what Molt must execute, how outputs are accepted, and which inputs are
authoritative. Dated progress, owners, priorities, and live blockers do not
belong here.

## Authority

- This README owns the stable Pact acceptance and intake contract.
- [`parity/check_parity.py`](parity/check_parity.py), together with a kernel's
  gate manifest, is the executable output-acceptance authority.
- [`docs/PACT_SUPPORT_MATRIX.md`](../../docs/PACT_SUPPORT_MATRIX.md) records
  stable runtime capabilities and the evidence required for a Pact claim.
- `tools/proof_queue.py` and its evidence own live execution state. A build,
  package seal, source scan, or forward-only smoke is never a parity verdict.

## Kernel A acceptance and determinism

Kernel A is
[`pact_witness_kernel/field_solve.py`](pact_witness_kernel/field_solve.py).
Molt must compile and execute that real package-native NumPy/SciPy program
through the WASM-CPU lane, without host-CPython, Pyodide, host-library, or
semantic-clone fallback. The run must write `candidate_outputs.npz` containing
all 11 arrays declared by
[`field_solve_gates.json`](pact_witness_kernel/field_solve_gates.json), with no
missing or extra output, and pass:

```powershell
uv run --active --python 3.12 python collab/pact/parity/check_parity.py `
  candidate_outputs.npz `
  collab/pact/pact_witness_kernel/reference_outputs.npz `
  collab/pact/pact_witness_kernel/field_solve_gates.json
```

Acceptance is fail loud. The engine rejects missing or extra arrays, dtype or
shape drift, NaN/Inf-mask drift, unknown gates, and any declared tolerance
above `1e-3`. Integer and label outputs use exact gates, critical-point rows use
order-independent exact-set gates, and floating fields use their declared
absolute tolerances. A gate pass is required; build, link, import, or runtime
startup success is not a substitute.

The kernel is deterministic by construction. Sort tie order, connected-component
enumeration, and eigenvector sign are canonicalized in the kernel. The detailed
operation classification and the parity microscope live beside the executable
kernel in [`pact_witness_kernel/`](pact_witness_kernel/); those aids diagnose
drift but do not weaken the gate manifest.

## Kernel A operator constraints

The accepted output gates depend on these package-operation semantics:

- `distance_transform_edt` is exact Euclidean distance with unit sampling.
- `gaussian_filter` uses reflect boundaries, `truncate=4`, separable passes, and
  SciPy's serial per-pixel accumulation order. The `sigma=2.0` result contains a
  630-way tie at the critical-point cutoff; a one-ULP change to `m_smooth` can
  change the exact critical-point set, so accelerated or fused implementations
  must preserve that rounding.
- `maximum_filter(15)` and `minimum_filter(11)` use square footprints and
  reflect boundaries; extrema that select critical points are exact.
- `label` uses four-connectivity, `percentile` uses linear interpolation, and
  `eigh` returns ascending eigenvalues before the kernel's sign canonicalization.

Approximate distance transforms or numerically similar filter rewrites are not
authority-lane substitutes. Any optimization must still pass the existing
11-output manifest without widening a gate.

## Scientific stack and source custody

[`config/scientific_stack_versions.toml`](../../config/scientific_stack_versions.toml)
is the sole version-selection authority for CPython, NumPy, and SciPy. Pact
execution must use the selected upstream package sources and their build
systems through Molt's checksummed source, extension, ABI, and artifact custody.

Molt must not copy NumPy/SciPy behavior into package-specific overlays, replace
it with Molt-owned semantic clones, or fall back to host Python or host package
artifacts. A missing source, symbol, extension, import, or runtime capability
must fail closed.

## Runtime axes and contest contract

WASM-CPU is the portable deterministic authority lane. Native CPU is a sister
deployment lane behind the same oracle. WebGPU is a separately labelled
per-device speed and browser-showcase lane: a CPU result never proves a GPU
cell, and a GPU result never proves a CPU cell. Cross-device WebGPU bit identity
is not assumed.

The full decoder must fit the contest's 30-minute evaluation budget on the
chosen contest target: either CPU (4 cores, 16 GiB) or T4 (16 GiB VRAM). The
contest runner is headless and executes `inflate.sh`; browser-only WebGPU is not
a contest-legal dependency. Only `archive.zip` bytes contribute to rate;
generic decoder code is free, while video-derived learned artifacts remain
counted. A faster decoder enables a more capable generator within the budget;
speed alone is not a score improvement.

See the [runtime support matrix](../../docs/PACT_SUPPORT_MATRIX.md) before
claiming a target or environment.

## Per-kernel file-set contract

Every newly supplied kernel `<k>` is one reviewable file set:

- `<k>.py` — the real deterministic kernel source.
- `make_<k>_fixture.py` — the deterministic fixture and reference generator.
- `<k>_reference.npz` — regenerable reference output; do not track it.
- `<k>_gates.json` — the declarative output schema and acceptance gates.

The shared engine is
[`collab/pact/parity/check_parity.py`](parity/check_parity.py). Create an intake
set with
[`collab/pact/parity/make_kernel_scaffold.py`](parity/make_kernel_scaffold.py).
The generated gate manifest contains
`"status": "AWAITING_PACT_KERNEL_SOURCE"`; the engine rejects that status
structurally with exit code 2 before comparing arrays. Generated kernel and
fixture entry points also raise `NotImplementedError` until replaced.

The scaffolder refuses to overwrite a non-scaffold real file unless explicitly
forced. A ready manifest removes the scaffold status and declares only the
supported gate classes: `exact`, `bitwise`, `exact_set`, `atol`, and
`order_robust_atol`. Missing or extra arrays, dtype or shape drift, NaN/Inf-mask
drift, unknown gates, and tolerances above `1e-3` fail acceptance.

## Kernel B and future kernels

Kernel B is the `levelset_argmax` forward contract. Its manifest must require
the `partition` output as exact `uint8`. Optional `phi` and trunk outputs may use
declared tolerances no wider than `1e-3`, unless Pact explicitly publishes a
stronger CPU bitwise gate. If real-`phi` evidence falsifies exact partition
parity at near ties, any argmax-margin policy is a reviewed manifest change with
an explicit pixel budget; implementations may not silently relax exactness.

Each later kernel enters through the same [per-kernel file-set
contract](#per-kernel-file-set-contract). WASM-CPU output compared with the
independent NumPy reference remains the portable authority. WebGPU evidence is
device-labelled and cannot change the CPU manifest.

## Browser embed and artifact distribution

The maintained
[`0970 Browser Numeric Kernel Embed`](../../docs/spec/areas/wasm/0970_BROWSER_NUMERIC_KERNEL_EMBED.md)
owns the minimal `forward(typedArray) -> typedArray` split-runtime interface,
the `molt.forward_f32_v1` ABI, the WebGPU worker route, and artifact integrity
requirements. The runnable small integration is
[`examples/browser_embed_forward/`](../../examples/browser_embed_forward/README.md).

Downstream use must not require every consumer to rebuild Rust/WASM from source.
A release-managed runtime generation must publish the complete shared and
relocatable WASM pair, the role-checked browser loader closure, manifests, and
SHA-256 identities atomically. Integrity pins without payloads are not a
release. The contest decoder, browser embed, and future production generator
must remain one compiled artifact family behind the same ABI, custody, and
integrity authorities rather than forking into compatibility implementations.

## Expansion contracts

These requirements remain behind the same oracle and source-custody rules; they
do not create alternate acceptance engines or package semantics:

- **Native sister backend.** Pact's Rust-native runtime and Molt targets must
  consume the same independent NumPy reference vectors. Neither backend is
  promotable from the other's result.
- **ONNX trunk.** The coordinate-INR trunk (feature bank, FiLM, MLP, and
  five-class head) may use Molt's ONNX-to-WASM graph lane, but its declared
  matmul/activation outputs must pass the shared parity engine.
- **Verified numeric-array surface.** Capability claims must publish an
  operation matrix across native, WASM, and deterministic `simd128`, covering
  the witness's elementwise operations (`sin`, `cos`, `tanh`, `exp`, `clip`),
  matmul, reductions and `argmax`, broadcasting, reshape/transpose/concatenate,
  `scipy.ndimage` label and distance operations, and grid sampling. Each green
  cell requires an independent reference and fail-closed per-operation proof.
- **Seven-kernel suite.** The forward expansion comprises fused R plus SegNet
  stem, AA-SDF rasterization, warp/grid-sample plus ground homography,
  curvelet/directional-Fourier features, margin/saliency, persistence
  soft-skeleton pooling, and island-birth. Each kernel uses the file-set
  contract; WebGPU claims are deterministic per named device.
- **Interactive browser lane.** FLOW and Kernel A re-solve may target real-time
  WebGPU with WebCodecs transport, but require real-browser execution,
  device-labelled parity, and measured framerate. Mock dispatch is not proof.
- **Portable training horizon.** A differentiable WebGPU lane must eventually
  cover forward and backward with deterministic-gradient evidence while
  retaining WASM-CPU as the deterministic deployment oracle. The maintained
  acceleration architecture is
  [`71 WASM/WebGPU numeric acceleration`](../../docs/design/foundation/71_wasm_webgpu_numeric_acceleration.md).

An observability dashboard is an optional product surface, not an acceptance
obligation. Historical C-API scans, correspondence replies, and upstream
reference reruns are evidence or prerequisites only; current claims still
require the real Molt-produced candidate in the claimed matrix cell.

## About Pact

Pact is the lab's entry for the comma.ai video-compression challenge. Its
decoder reconstructs a task-space witness judged by frozen SegNet-argmax and
PoseNet evaluator cells. The Molt collaboration uses that decoder as a real
scientific-computing acceptance workload across NumPy, SciPy, native, WASM,
browser, and GPU boundaries.
