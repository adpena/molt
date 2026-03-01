# 0513: GPU Parallelism and MLIR Integration

Status: **Backlog** (ROADMAP item 16, no milestone assigned)
Owner: runtime
Prerequisites: TC2 (type coverage), SL2 (stdlib coverage), TL2 (tooling)

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

## WASM Exclusion

GPU paths must be cleanly gated off for `wasm32` targets. GPU capability detection
raises `NotImplementedError` on WASM.
