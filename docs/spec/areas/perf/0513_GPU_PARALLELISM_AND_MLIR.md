# 0513: GPU Parallelism and MLIR Integration

Status: **Backlog** (ROADMAP item 16, no milestone assigned)
Owner: runtime
Prerequisites: TC2 (type coverage), SL2 (stdlib coverage), TL2 (tooling)

## Implementation: `molt-gpu` Crate

The tinygrad-conformant GPU primitive stack is implemented in `runtime/molt-gpu/`.
This crate provides:

- **26 primitive ops** (`runtime/molt-gpu/src/ops.rs`): The complete tinygrad op set
  (Add, Sub, Mul, Idiv, Mod, Neg, Cmplt, Cmpeq, Cmpne, And, Or, Xor, Shl, Shr,
  Exp2, Log2, Sin, Sqrt, Reciprocal, Trunc, Max, Where, Cast, Bitcast, ReduceSum,
  ReduceMax).
- **ShapeTracker** (`runtime/molt-gpu/src/shapetracker.rs`): Zero-copy view system
  with O(1) reshape, permute, expand, pad, shrink, and flip.
- **LazyOp DAG** (`runtime/molt-gpu/src/lazy.rs`): Deferred computation graph.
- **Kernel fusion** (`runtime/molt-gpu/src/fuse.rs`): Elementwise-reduce-elementwise
  chain fusion.
- **4 renderers**: MSL (`render/msl.rs`), WGSL (`render/wgsl.rs`), CUDA (`render/cuda.rs`),
  HIP (`render/hip.rs`) -- all implementing the full 26-op set.
- **3 device backends**: CPU interpreter (`device/cpu.rs`), Metal (`device/metal.rs`),
  WebGPU (`device/webgpu.rs`).
- **MLIR serialization** (`runtime/molt-gpu/src/mlir.rs`): Textual MLIR IR generation
  from FusedKernel (string output only, no C++ dependencies).

The Python Tensor API is at `src/molt/stdlib/tinygrad/`. It includes the Tensor class,
LazyBuffer, dtypes, TurboQuant (Remez-optimal quantization), DDTree (decision tree
routing with additive log-probability scoring), and DFlash (flash attention).

## Two-Lane Architecture

### Lane 1: Arrow-first + libcudf Routing (90% Win)

Route high-level tabular ops to GPU-native kernel library (libcudf).
Not compiling arbitrary Python to GPU; routing ops to existing GPU libraries.

**Design**:
- `MoltArray<T>` / `MoltTable` backed by Arrow-compatible buffers
- `MemoryLocation` tag: `Host` | `CudaDevice` (later: `Metal`, `RocmDevice`)
- Explicit capability-gated movement: `to_device()`, `to_host()` -- no silent fallbacks
- CPU backend: Molt SIMD kernels + Arrow compute
- GPU backend: libcudf for supported ops, custom kernels for gaps
- Arrow C Device Interface / `ArrowDeviceArray` for zero-copy interop

### Lane 2: TIR Kernel Subset to GPU (Power-User / UDF Lane)

Compile restricted Python subset into GPU kernels via MLIR.

**Kernel Subset Whitelist (initially)**:
- Primitive arithmetic + comparisons on Int/Float/Bool
- Loads/stores from/to typed buffers (`Buffer<T>`)
- Simple predicated control flow
- Pure intrinsics: min/max/abs, math subset

**Disallowed (must stay CPU)**:
- Any Python object allocation (lists/dicts/strings)
- Dict/list mutation
- Calls that allocate/raise/touch global state
- Python object model operations (attribute lookup, dynamic dispatch)
- Unbounded exception behavior

## Loop Classification in TIR

Preserve loop intent via explicit TIR ops during HIR→TIR lowering:
- `ForRange(i: i64, start, stop, step)`
- `ForEach(elem, buffer)`
- `ZipForEach(a, b, ...)`

### Dependence Pattern Classification

**A) Map** (embarrassingly parallel): `out[i] = f(in1[i], in2[i], constants)`
**B) Reduction** (associative op): `acc = acc + g(in[i])` -- loop-carried phi
**C) Scan/Prefix** (harder, deferred): loop-carried state producing per-iteration output
**D) Scatter/Histogram** (needs atomics): `out[idx(i)]` writes with potential collisions

### Backend Dispatch

- **GPU**: large iteration spaces, simple per-element compute, data on device, minimal branching
- **CPU-parallel** (threads/SIMD): smaller sizes, low launch overhead, data on host
- **Scalar CPU**: tiny loops, failed kernel subset

## GIL Contract with Async GPU Kernels

1. **With GIL held**: validate, allocate/lock buffers, materialize kernel args, pin references
2. **Launch** async kernel on CUDA stream
3. **Release GIL** immediately
4. Return `GpuFuture` that integrates with Molt async (poll checks completion)
5. **On completion**: reacquire GIL, publish results, release pins

```rust
enum GpuFutureState {
    NotLaunched,
    Launched { event: CudaEvent, pins: Vec<MoltHandle> },
    Done,
    Error(GpuError),
}
```

Key rule: `poll()` must never block.

## Determinism Policy

- **Deterministic tier** (default): elementwise maps, stable integer reductions with fixed tree structure
- **Nondeterministic tier** (capability-gated): float reductions, atomics, groupby-style ops

## Backend Strategy

- **Cranelift**: stays default CPU backend (fast compilation, Rust-native, lean)
- **LLVM**: only for GPU targets (NVPTX for NVIDIA, AMDGPU for AMD) via MLIR
- MLIR pathway: TIR → Molt MLIR dialect → linalg/affine → gpu dialect → nvvm/rocdl → PTX/GCN

## Staged Execution Plan

1. **CPU kernelization**: TIR loop classifier (Map/Reduce) + KernelIR → scalar/SIMD/threaded CPU
2. **Columnar runtime** (DF1/DF2): MoltTable/MoltColumn with Arrow-compatible buffers
3. **libcudf backend**: Route DataFrame ops to libcudf via ArrowDeviceArray interop
4. **GPU kernel backend**: Elementwise maps first, then reductions with determinism policy
5. **Tight async integration**: GPU ops as first-class Molt futures

## Cancellation

- Before launch: honor cancellation immediately
- After launch: ignore results and drop (stream-level cancel not guaranteed deterministic)

## Dependency Investigation

### Rust Crates for GPU Pipeline

| Crate | Version | Purpose | Evaluation Status |
|-------|---------|---------|-------------------|
| `egg` | 0.9 | E-graph equality saturation for kernel expression optimization | Prototype exists (`molt-backend --features egraphs`) |
| `mlir-sys` | 0.3+ | Raw MLIR C-API bindings for GPU dialect lowering | Not evaluated — requires LLVM/MLIR build |
| `inkwell` | 0.5+ | Safe LLVM IR builder (alternative to mlir-sys for NVPTX) | Not evaluated |
| `cudarc` | 0.12+ | Safe CUDA runtime/driver API bindings | Not evaluated |
| `vulkano` | 0.34+ | Vulkan compute (cross-vendor GPU alternative) | Not evaluated |
| `wgpu` | 24+ | WebGPU abstraction (portable, lower performance ceiling) | Not evaluated |

### Evaluation Criteria

1. **Build complexity**: Does it require a full LLVM/MLIR toolchain?
2. **Determinism**: Can we produce deterministic GPU kernels?
3. **Platform support**: NVIDIA + AMD + Apple Silicon (Metal)?
4. **Maintenance burden**: Active upstream, stable API?
5. **Integration cost**: How much glue code to connect to Molt's TIR?

### Recommended Path

- **Short-term (M-GPU-1, M-GPU-2)**: No GPU dependencies needed — pure CPU work.
- **Medium-term (M-GPU-3)**: `cudarc` + Arrow C Device Interface for libcudf.
- **Long-term (M-GPU-4)**: `mlir-sys` or `inkwell` for custom kernel compilation.

## External Research Directions

### TurboQuant / PolarQuant / QJL

Recent vector-compression work is relevant to Molt's future tensor and
model-serving path:

- **TurboQuant**: randomized rotation plus residual sketching for very-low-bit
  inner-product preservation
- **PolarQuant**: rotation-based low-bit compression aimed at KV-cache storage
- **QJL**: sketch-based residual correction for approximate dot products

These are not substitutes for the current compiler/runtime burndown. They are
future format/runtime work for packed numeric tensors, model weights, and
attention-state storage.

### Applicability to Molt

- **Native runtime**: strong future fit for packed tensor storage and KV-cache
  compression once tensor layouts become stable runtime ABI surfaces
- **Browser/WebGPU/WASM**: strong fit for reducing shipped model artifacts and
  peak memory, but only after low-bit formats are part of Molt's runtime and
  host-interface contract
- **MLIR lane**: good long-term fit for representing
  `rotate -> quantize -> residual sketch` as explicit lowering stages, but
  premature before the current MLIR bridge grows into a real optimization path
- **Falcon-OCR-style workloads**: relevant as an external model-serving
  strategy, not as a direct fix for current compiler/link/runtime blockers

### Molt Constraints

- Determinism is mandatory: rotations/codebooks/seeds must be explicit,
  versioned, and hashed into artifacts.
- No silent host fallback: encode/dequant paths must lower into Rust/runtime
  kernels and wasm ops, not Python helper layers.
- WIT-facing browser/edge deployments must expose quantized tensor layout and
  capability metadata explicitly. If Molt adopts low-bit runtime tensors, the
  WIT surface cannot assume raw byte compatibility without versioning.

### Near-Term Experiments

1. Compare current per-channel INT4 against rotation-based low-bit packing on
   Molt tensor buffers for bytes, reconstruction error, and linear-kernel drift.
2. Add a KV-cache benchmark lane before implementing full low-bit runtime
   formats.
3. Prototype packed low-bit browser model delivery on a small WASM demo before
   touching larger models.

### Primary References

- Google Research blog: TurboQuant
- arXiv `2504.19874`: TurboQuant
- arXiv `2502.02617`: PolarQuant
- arXiv `2406.03482`: QJL

## Kernel Subset Whitelist (Formal)

The following TIR operations are eligible for GPU kernel extraction. Operations
not on this list MUST remain on CPU.

### Allowed in GPU Kernels

| Category | Operations |
|----------|-----------|
| **Arithmetic** | `add`, `sub`, `mul`, `div`, `floordiv`, `mod`, `pow` (int/float only) |
| **Comparison** | `eq`, `ne`, `lt`, `le`, `gt`, `ge` |
| **Bitwise** | `bit_and`, `bit_or`, `bit_xor`, `bit_not`, `lshift`, `rshift` |
| **Unary** | `neg`, `abs`, `invert` |
| **Math** | `sqrt`, `exp`, `log`, `sin`, `cos`, `tan`, `floor`, `ceil`, `round` |
| **Buffer access** | `load(buffer, index)`, `store(buffer, index, value)` |
| **Control flow** | `if/else` (predicated), `for_range` (parallel map) |
| **Constants** | Integer literals, float literals, bool literals |
| **Variables** | SSA temporaries (scalar, not object) |

### Forbidden in GPU Kernels (Must Stay CPU)

| Category | Reason |
|----------|--------|
| Python object allocation | No GC/refcount on GPU |
| String/bytes operations | Variable-length, heap-allocated |
| Dict/list/set mutation | Requires GIL-protected runtime |
| Function calls (non-intrinsic) | No call stack on GPU kernels |
| Exception raising | No unwinding on GPU |
| I/O operations | No filesystem/network on GPU |
| Global state access | No shared mutable state across warps |

### Kernel Classification

A TIR loop is kernel-eligible if:
1. All operations in the loop body are in the "Allowed" table.
2. All variables are scalar (int/float/bool) or buffer references.
3. Loop iteration count is statically known or bounded.
4. No inter-iteration dependencies (map pattern) OR dependencies are
   associative/commutative (reduction pattern).

## WASM Browser GPU Contract

`wasm32` is no longer a blanket GPU exclusion zone.

Current contract:
- compiled `@gpu.kernel` on wasm must preserve synchronous kernel semantics
  from the language/runtime point of view
- browser-hosted WebGPU execution is allowed through a host-dispatch boundary
  rather than native-in-runtime GPU code
- the host boundary must be explicit and capability-visible; no silent CPU
  fallback when `MOLT_GPU_BACKEND=webgpu` is requested

Operational implications:
- browser/main-thread execution is not a viable implementation target for the
  current synchronous kernel contract because WebGPU readback is async
- worker-backed browser hosts are the correct deployment shape for real WebGPU
  kernel execution on wasm
- Node/CLI wasm runners may continue to provide `ENOSYS` stubs for browser-only
  WebGPU host imports unless they implement an equivalent host dispatcher
