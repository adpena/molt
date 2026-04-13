# 0513: GPU Parallelism and MLIR Integration

Status: **Backlog** (ROADMAP item 16, no milestone assigned)
Owner: runtime
Prerequisites: TC2 (type coverage), SL2 (stdlib coverage), TL2 (tooling)

## Public API Contract

The public ML/tensor surface should converge on **tinygrad-compatible API
shape**, not a parallel Molt-specific tensor dialect.

Rules:
- user-facing imports should prefer `tinygrad`-compatible names and behavior
- lower-level `molt_gpu_*` and `tensor__*` wrapper intrinsics are internal
  lowering machinery, not the compatibility contract
- composed tensor formulations are the canonical correctness path
- fused wrapper intrinsics are optional acceleration lanes and must never be
  the only semantically correct implementation path

Implication:
- when a high-level wrapper intrinsic diverges from the lower canonical
  primitive stack, prefer the canonical path and treat the wrapper as an
  optimization problem, not a semantic dependency

## Compression Package Boundaries

Quantization and speculative-decode infrastructure belong under `molt.gpu`
as first-class packages with narrow responsibilities:

- `molt.gpu.dflash`: speculative decode contracts, adapter selection, and
  runtime orchestration
- `molt.gpu.turboquant`: KV-cache/vector compression contracts, codebooks,
  structured-rotation reference codecs, and cache helpers

Rules:
- model-specific logic stays outside these core packages
- core packages define reusable contracts and reference semantics first
- fused backend kernels (`cuda`, `metal`, `webgpu`, `rocm`) are acceleration
  lanes layered under these package boundaries, not alternate public APIs
- no silent fallback between compression schemes; enablement must be explicit
  and capability-aware

Current reference cache split:
- `molt.gpu.kv_cache`: projected-attention cache backends (`DenseKVCache`,
  TurboQuant-backed cache) and cache append/attend contracts
- `molt.gpu.transformer`: optional cache plumbing for transformer attention and
  decoder blocks

Current cache semantics:
- cache backends must preserve tensor-SDPA-visible semantics for masks and
  causal decode
- grouped-query attention is supported when query heads are an integer multiple
  of KV heads
- dense cache append should avoid per-token concat churn and materialize lazily
  for attention reads

Near-term direction:
- keep cache semantics generic at the tensor/attention layer
- move toward tensor-level cache-backed attention dispatch, so tinygrad-shaped
  model code can benefit without model-specific API forks

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

## WASM Exclusion

GPU paths must be cleanly gated off for `wasm32` targets. GPU capability detection
raises `NotImplementedError` on WASM.
