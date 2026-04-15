# Tinygrad GPU Primitives — Plan 2: WebGPU + CUDA + HIP Backends

**Goal:** Extend `molt-gpu` with three additional backends: WebGPU (WGSL), CUDA, and HIP. Each backend gets a renderer (shader codegen) and device implementation (allocator, compiler, executor). All backends must produce results that match CpuDevice bit-for-bit for deterministic ops.

**Depends on:** Plan 1 (complete)

---

## File Map

| Path | Responsibility |
| --- | --- |
| `runtime/molt-gpu/src/render/wgsl.rs` | WgslRenderer — WGSL shader codegen |
| `runtime/molt-gpu/src/render/cuda.rs` | CudaRenderer — CUDA C codegen |
| `runtime/molt-gpu/src/render/hip.rs` | HipRenderer — HIP C codegen |
| `runtime/molt-gpu/src/device/webgpu.rs` | WebGpuDevice — wgpu-based backend |
| `runtime/molt-gpu/src/device/cuda.rs` | CudaDevice — CUDA runtime backend |
| `runtime/molt-gpu/src/device/hip.rs` | HipDevice — HIP/ROCm backend |
| `runtime/molt-gpu/tests/test_render_wgsl.rs` | WGSL render output tests |
| `runtime/molt-gpu/tests/test_render_cuda.rs` | CUDA render output tests |
| `runtime/molt-gpu/tests/test_webgpu_ops.rs` | WebGPU per-op CPU reference comparison |
| `runtime/molt-gpu/tests/test_cuda_ops.rs` | CUDA per-op CPU reference comparison |

## Tasks

### Task 1: WgslRenderer
- Implement WGSL codegen for all 26 ops
- DType narrowing: f64->f32, i64->i32, u64->u32 via DType::narrow_webgpu()
- Thread index: `@builtin(global_invocation_id) gid: vec3<u32>`
- Workgroup size annotation: `@workgroup_size(256, 1, 1)`
- WGSL-specific: no ternary operator (`select(false_val, true_val, cond)`)
- WGSL-specific: bitcast syntax (`bitcast<f32>(x)`)
- Tests: same structure as test_render_msl.rs

### Task 2: CudaRenderer
- CUDA C codegen for all 26 ops
- Full i64/f64 support (no narrowing)
- Thread index: `blockIdx.x * blockDim.x + threadIdx.x`
- Includes: `<cuda_runtime.h>`, `<math.h>`
- CUDA-specific: `__global__` function qualifier
- Tests: output validation

### Task 3: HipRenderer
- HIP C codegen (nearly identical to CUDA — HIP is source-compatible)
- Thread index: `hipBlockIdx_x * hipBlockDim_x + hipThreadIdx_x`
- Includes: `<hip/hip_runtime.h>`
- Tests: output validation

### Task 4: WebGpuDevice
- Use `wgpu` crate for native + browser WebGPU
- Implement Allocator (wgpu::Buffer with MAP_READ|MAP_WRITE|STORAGE)
- Implement Compiler (wgpu::ShaderModule from WGSL source, ComputePipeline)
- Implement Executor (CommandEncoder, dispatch_workgroups, queue.submit)
- Buffer mapping for copy_in/copy_out (wgpu buffer mapping API)
- Tests: per-op comparison vs CpuDevice

### Task 5: CudaDevice (optional — requires NVIDIA GPU)
- Use `cuda-driver-sys` or `cudarc` crate
- Implement Allocator (cuMemAlloc/cuMemFree)
- Implement Compiler (nvrtcCompileProgram, cuModuleLoadData)
- Implement Executor (cuLaunchKernel)
- Gate behind `cuda-backend` feature flag
- Tests: per-op comparison vs CpuDevice

### Task 6: HipDevice (optional — requires AMD GPU)
- Use `hip-sys` or FFI bindings
- Implement Allocator (hipMalloc/hipFree)
- Implement Compiler (hiprtcCompileProgram)
- Implement Executor (hipLaunchKernel)
- Gate behind `hip-backend` feature flag
- Tests: per-op comparison vs CpuDevice

### Task 7: Workgroup-Level Reduce Optimization
- Replace sequential reduce loop with shared memory + workgroup reduction
- Metal: threadgroup memory + simdgroup_add/max
- WebGPU: workgroup variable + workgroupBarrier()
- CUDA: __shared__ memory + __syncthreads()
- Benchmark: reduce of 1M elements should be >100x faster than sequential

### Task 8: Multi-Backend Test Harness
- Parametric test macro that runs every op test across all available backends
- Runtime detection: Metal (macOS), WebGPU (always via wgpu), CUDA (nvidia-smi), HIP (rocm-smi)
- Cross-backend result comparison: all backends must match CpuDevice

### Task 9: Integration + Cleanup
- Update lib.rs with new module declarations
- Update Cargo.toml with wgpu, cuda, hip dependencies behind feature flags
- Run full test suite across all backends
- Clippy clean
- Git add + commit

---

## What Plan 2 Delivers

1. WGSL, CUDA, HIP renderers with full 26-op coverage
2. WebGPU device backend (native + browser-ready)
3. CUDA and HIP device backends (feature-gated)
4. Workgroup-level reduce optimization for all GPU backends
5. Cross-backend correctness test harness
