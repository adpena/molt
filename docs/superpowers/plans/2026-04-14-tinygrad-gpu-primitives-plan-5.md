# Tinygrad GPU Primitives — Plan 5: Legacy GPU Code Deletion

**Goal:** Remove the entire legacy GPU subsystem now that the tinygrad-conformant stack is proven. Delete ~13,000 LOC of ad-hoc GPU code and update all references.

**Depends on:** Plans 1-4 (complete, all tests passing, Falcon-OCR migrated)

---

## Pre-Conditions (Must Be True Before Starting)

1. All Plan 1-4 tests pass on all target backends
2. Falcon-OCR produces identical results on new stack
3. No remaining code references legacy GPU types
4. Performance benchmarks show new stack meets or exceeds legacy

## File Map — Files to DELETE

| Path | LOC (approx) | Responsibility |
| --- | --- | --- |
| `runtime/molt-backend/src/tir/gpu_pipeline.rs` | ~1500 | Legacy GPU kernel pipeline |
| `runtime/molt-backend/src/tir/gpu_types.rs` | ~400 | Legacy GpuKernel, GpuOp types |
| `runtime/molt-backend/src/gpu/` | ~3000 | Legacy GPU device, shader gen |
| `runtime/molt-backend/src/gpu/metal_device.rs` | ~800 | Legacy Metal device |
| `runtime/molt-backend/src/gpu/webgpu_device.rs` | ~600 | Legacy WebGPU device |
| `runtime/molt-backend/src/gpu/shader_gen.rs` | ~2000 | Legacy per-kernel MSL/WGSL gen |
| `runtime/molt-backend/src/gpu/kernel_cache.rs` | ~400 | Legacy kernel cache |
| `runtime/molt-backend/src/gpu/device_pool.rs` | ~300 | Legacy device pool |
| `runtime/molt-runtime/src/builtins/gpu_ops.rs` | ~2000 | Legacy tensor_linear, etc. |
| `stdlib/gpu_ops.py` | ~800 | Legacy Python GPU API |
| `tests/gpu/test_legacy_*.py` | ~1200 | Legacy GPU tests |

**Total: ~13,000 LOC deleted**

## Tasks

### Task 1: Audit All References
- Search codebase for all imports/references to legacy GPU types
- Map each reference to its replacement in the new stack
- Verify no production code depends on legacy GPU types

### Task 2: Update Backend Integration Points
- Replace any backend GPU pipeline references with molt-gpu FFI
- Update TIR lowering to route GPU ops through molt-gpu
- Remove GpuKernel/GpuOp from TIR type system

### Task 3: Delete Legacy GPU Device Code
- Delete all files in `runtime/molt-backend/src/gpu/`
- Remove corresponding module declarations
- Update Cargo.toml to remove legacy GPU dependencies

### Task 4: Delete Legacy Runtime GPU Builtins
- Delete `gpu_ops.rs` and `tensor_linear`, `tensor_softmax_last_axis`, etc.
- Remove from builtins module index
- Update any stdlib imports

### Task 5: Delete Legacy Python GPU API
- Delete `stdlib/gpu_ops.py`
- Remove from stdlib module index
- Verify Falcon-OCR uses only new Tensor API

### Task 6: Delete Legacy Tests
- Delete `tests/gpu/test_legacy_*.py`
- Verify new test suite has full coverage of all deleted functionality

### Task 7: Clean Up Build System
- Remove legacy GPU feature flags from Cargo.toml
- Update CI configuration for new test paths
- Remove legacy GPU from documentation

### Task 8: Final Verification
- Full workspace build (`cargo check --workspace`)
- Full test suite (all backends)
- Falcon-OCR end-to-end test
- Binary size comparison (should be smaller)
- Performance comparison (should be equal or better)

---

## What Plan 5 Delivers

1. ~13,000 LOC of legacy GPU code deleted
2. Single GPU subsystem: molt-gpu with tinygrad-conformant primitives
3. Cleaner codebase: no duplicate GPU abstractions
4. Smaller binary size
5. Single source of truth for GPU compute
