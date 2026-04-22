# GPU Primitive Stack Architecture

## Overview

Molt's GPU compute subsystem implements all of deep learning with 26 compute primitives, a zero-copy ShapeTracker view system, lazy evaluation DAG, kernel fusion, and multi-backend rendering. The design is tinygrad-conformant: the same 3 OpTypes and 26 ops that tinygrad uses to express all ML operations.

```
                     Python Tensor API
                     (src/molt/stdlib/tinygrad/)
                            |
                            v
                    +-----------------+
                    |   LazyOp DAG    |  Deferred computation graph
                    | (runtime/molt-gpu/src/lazy.rs)
                    +-----------------+
                            |
                     schedule()
                            |
                            v
                    +-----------------+
                    |   Scheduler     |  DAG -> topological kernel list
                    | (schedule.rs)   |
                    +-----------------+
                            |
                       fuse()
                            |
                            v
                    +-----------------+
                    |  Fusion Engine  |  Merge kernels: elem->reduce->elem
                    | (fuse.rs)       |
                    +-----------------+
                            |
                     render()
                            |
                            v
            +------+------+------+------+------+--------+------+
            | MSL  | WGSL | GLSL | CUDA | HIP  | OpenCL | MLIR |
            +------+------+------+------+------+--------+------+
                            |
                     compile() + exec()
                            |
                            v
            +------+------+------+------+
            | Metal| WebGPU|WebGL2| CPU  |
            +------+------+------+------+
```

## Location

- **Rust crate**: `runtime/molt-gpu/` (48 files, 15,748 LOC — 25 source, 21 test, 2 bench)
- **Python API**: `src/molt/stdlib/tinygrad/` (21 files, 7,291 LOC)
- **Tests**: `runtime/molt-gpu/tests/` (21 test files, 323 tests)
- **Benchmarks**: `runtime/molt-gpu/benches/` (2 files)

## The 3 OpTypes

All computation reduces to three categories:

### ElementwiseOps

Operate on 1-3 tensors, run elementwise, fuse freely with each other.

- **UnaryOps** (9): `NEG`, `RECIPROCAL`, `SQRT`, `EXP2`, `LOG2`, `SIN`, `TRUNC`, `CAST`, `BITCAST`
- **BinaryOps** (14): `ADD`, `SUB`, `MUL`, `IDIV`, `MOD`, `MAX`, `CMPLT`, `CMPEQ`, `CMPNE`, `AND`, `OR`, `XOR`, `SHL`, `SHR`
- **TernaryOps** (1): `WHERE`

### ReduceOps

Operate on one tensor, return a smaller tensor. Fusion boundary: reduce-to-reduce requires materialization.

- `REDUCE_SUM`, `REDUCE_MAX`

### MovementOps

Virtual ops, zero-cost, no kernel generated. Implemented entirely through ShapeTracker view modifications:

- `RESHAPE`, `PERMUTE`, `EXPAND`, `PAD`, `SHRINK`, `FLIP`

## The 26 Primitive Ops

| # | Op | Type | Pattern | Notes |
|---|-----|------|---------|-------|
| 1 | ADD | Binary | `a + b` | |
| 2 | SUB | Binary | `a - b` | Distinct from Add(a, Neg(b)) for -0.0 |
| 3 | MUL | Binary | `a * b` | |
| 4 | IDIV | Binary | `a / b` | Integer, truncates toward zero (C semantics) |
| 5 | MOD | Binary | `a % b` | Sign of dividend (C semantics) |
| 6 | NEG | Unary | `-a` | Distinct from a * -1 for -0.0, NaN sign bit |
| 7 | CMPLT | Binary | `a < b` | Output always Bool. NaN < x = false |
| 8 | CMPEQ | Binary | `a == b` | NaN == NaN = false |
| 9 | CMPNE | Binary | `a != b` | NaN != NaN = true |
| 10 | AND | Binary | `a & b` | |
| 11 | OR | Binary | `a \| b` | |
| 12 | XOR | Binary | `a ^ b` | |
| 13 | SHL | Binary | `a << b` | Logical left shift |
| 14 | SHR | Binary | `a >> b` | Arithmetic for signed, logical for unsigned |
| 15 | EXP2 | Unary | `exp2(a)` | |
| 16 | LOG2 | Unary | `log2(a)` | |
| 17 | SIN | Unary | `sin(a)` | |
| 18 | SQRT | Unary | `sqrt(a)` | |
| 19 | RECIPROCAL | Unary | `1/a` | Float-only. 1/0 = +inf per IEEE 754 |
| 20 | TRUNC | Unary | `trunc(a)` | Needed for floor/ceil/round compositions |
| 21 | MAX | Binary | `max(a,b)` | NaN-propagating for floats |
| 22 | WHERE | Ternary | `c ? a : b` | Ternary select |
| 23 | CAST | Unary | `(T)a` | Type conversion |
| 24 | BITCAST | Unary | reinterpret | Reinterpret bits, no conversion |
| 25 | REDUCE_SUM | Reduce | `sum(a[i])` | Over axis |
| 26 | REDUCE_MAX | Reduce | `max(a[i])` | NaN-propagating for floats |

## ShapeTracker

The ShapeTracker (`shapetracker.rs`) is a stack of Views that tracks how a contiguous buffer is accessed. All movement ops (reshape, permute, expand, pad, shrink, flip) are O(1) modifications to the view -- no GPU kernel, no memory copy.

A View contains:
- `shape`: logical shape of the view
- `strides`: stride per dimension (0 = broadcast, negative = flipped)
- `offset`: offset into the underlying buffer
- `mask`: optional validity mask for padding regions

## Lazy Evaluation

Every Tensor operation returns a new `LazyOp` node without executing anything. The full computation graph is visible to the scheduler and fusion engine before any GPU kernel runs.

The `LazyOp` variants:
- `Buffer`: leaf node (realized data)
- `Unary`: elementwise unary op
- `Binary`: elementwise binary op
- `Ternary`: ternary select (WHERE)
- `Reduce`: reduction over an axis
- `Movement`: view modification (free)
- `Contiguous`: force materialization

## Kernel Fusion

The fusion engine (`fuse.rs`) merges chains of single-op kernels into fused multi-op kernels:

1. Consecutive elementwise ops merge into a single kernel
2. An elementwise chain followed by a reduce merges into one kernel
3. A reduce followed by elementwise ops merges into one kernel (post-reduce)
4. Reduce-to-reduce is a fusion boundary (must materialize between)

The fused kernel chain structure: `[elementwise prefix] -> [optional reduce] -> [elementwise suffix]`

## Backend Matrix

| Backend | Language | DType Support | Platform |
|---------|----------|---------------|----------|
| Metal | MSL | f32, f16, bf16, i32, i64 (no f64) | macOS, iOS |
| WebGPU | WGSL | f32, f16, i32, u32 (no f64, i64, u64) | Browser, WASM |
| WebGL2 | GLSL ES 3.0 | f32, i32, u32 (no f64, i64, u64, f16, bf16, i8, u8, i16, u16) | Browser fallback, WASM |
| CUDA | CUDA C | Full (f64, i64, bf16 via nv_bfloat16) | NVIDIA GPUs |
| HIP | HIP C | Full (f64, i64, bf16 via hip_bfloat16) | AMD GPUs |
| OpenCL | OpenCL C | f64 via cl_khr_fp64, i64 native, no bf16 | Cross-vendor GPUs, FPGAs, DSPs |
| CPU | Rust | Full | All platforms (reference backend) |
| MLIR | MLIR text | Full | Cross-compilation target |

### DType Narrowing

Backends that lack certain types automatically narrow:
- **Metal**: f64 -> f32
- **WebGPU**: f64 -> f32, i64 -> i32, u64 -> u32, i8 -> i32, u8 -> u32, i16 -> i32, u16 -> u32, bf16 -> f32
- **WebGL2**: f64 -> f32, i64 -> i32, u64 -> u32, f16 -> f32, bf16 -> f32, i8 -> i32, u8 -> u32, i16 -> i32, u16 -> u32
- **OpenCL**: f64 -> f32 (when `cl_khr_fp64` absent), bf16 -> f32 (always). i64 supported natively.

Narrowing is applied at render time and is transparent to the user.

## Integration Points

### Python Tensor API

`src/molt/stdlib/tinygrad/tensor.py` provides the user-facing Tensor class with ~80 methods that compose the 26 primitives:

- `exp()`, `log()`, `sin()`, `sqrt()` -- direct unary ops
- `matmul()` -- RESHAPE + EXPAND + MUL + REDUCE_SUM
- `softmax()` -- REDUCE_MAX + SUB + EXP2 + REDUCE_SUM + RECIPROCAL + MUL
- `layernorm()`, `rmsnorm()` -- compositions of reduce + elementwise

### Falcon-OCR

Full VLM inference pipeline using the primitive stack:
- Patch embedding via convolution (matmul compositions)
- Multi-head attention via DFlash
- RMSNorm via reduce + elementwise
- Rotary position embedding (RoPE) via sin/cos compositions
- TurboQuant 4-bit dequantization

### TurboQuant

4-bit weight quantization/dequantization expressed as primitive compositions:
- Dequant: AND + SHR + CAST + MUL + ADD
- Pack: CAST + SHL + OR

### DFlash

Flash-attention-style fused attention using DDTree scoring:
- Block-diagonal attention with H2O importance scoring
- Tiered KV cache with eviction policies
- All expressed as compositions of the 26 primitives

## Performance Characteristics

- **Movement ops**: O(1), zero memory traffic
- **Elementwise fusion**: N fused elementwise ops = 1 kernel launch, 1 memory read, 1 memory write
- **Kernel cache**: compiled shaders are cached by source hash. Repeated operations with the same shapes skip compilation
- **Device persistence**: Metal/WebGPU device objects persist across the session. No per-kernel device creation/destruction

## WebGL2 Fragment-Shader-as-Compute

The WebGL2 backend (`render/glsl.rs`, `device/webgl2.rs`) provides GPU compute for browsers that lack WebGPU support (~25% of users, especially iOS 15-25). WebGL2 has no compute shaders, so all computation is performed via render-to-texture with fragment shaders.

### How it works

1. **Input data as textures**: Linear buffer data is packed into RGBA32F/RGBA32I/RGBA32UI 2D textures. Each RGBA texel holds 4 consecutive elements. A uniform `u_tex_width` tracks the packing layout.

2. **Fragment shader codegen**: `GlslRenderer` generates GLSL ES 3.0 fragment shaders that read inputs via `texelFetch()` on `sampler2D` uniforms and write results to a framebuffer-attached output texture.

3. **Index mapping**: `gl_FragCoord.xy` replaces `global_invocation_id`. Each fragment computes 4 output values (one RGBA texel). The linear index is recovered as `(floor(gl_FragCoord.x) + floor(gl_FragCoord.y) * tex_width) * 4 + component`.

4. **Dispatch as draw calls**: A full-screen triangle (3 vertices, no vertex buffer) is drawn. The viewport dimensions match the output texture size. The fragment shader runs once per output texel.

5. **Reduce ops**: Reductions use a loop inside the fragment shader, iterating over the reduction dimension. For large reductions, the device orchestrates multi-pass ping-pong rendering where each pass halves the data until a single value remains.

### Texture-as-buffer mapping

| Buffer concept | WebGL2 equivalent |
|---------------|-------------------|
| Storage buffer | RGBA32F / RGBA32I / RGBA32UI texture |
| Buffer read | `texelFetch(sampler2D, ivec2, 0)` |
| Buffer write | Framebuffer color attachment |
| Element index | `(texel_row * tex_width + texel_col) * 4 + rgba_component` |
| Random access read | `texelFetch` with computed ivec2 |
| Random access write | Not possible; output fixed to fragment location |

### Performance characteristics

WebGL2 render-to-texture compute is 3-5x slower than native WebGPU due to:
- Fragment shader overhead (rasterization pipeline vs compute pipeline)
- Texture read/write latency vs storage buffer random access
- No shared memory / workgroup synchronization
- 4-component RGBA packing/unpacking overhead
- `readPixels` synchronization stalls (no async readback without PBO extensions)

This is acceptable as a compatibility fallback; performance-sensitive users are directed to WebGPU-capable browsers.

## OpenCL Backend

The OpenCL renderer (`render/opencl.rs`) and device types (`device/opencl.rs`) provide GPU compute for cross-vendor hardware (NVIDIA, AMD, Intel, ARM GPUs, FPGAs, DSPs) via the OpenCL standard.

### Key Design Decisions

- **fp64 extension gating**: OpenCL does not guarantee double precision. The renderer checks `has_fp64` at construction and emits `#pragma OPENCL EXTENSION cl_khr_fp64 : enable` only when the device advertises `cl_khr_fp64`. Without it, Float64 is narrowed to Float32 via `DType::narrow_opencl`.
- **BFloat16 always narrowed**: No OpenCL implementation supports BFloat16 natively. Always narrowed to Float32.
- **i64 native**: Unlike WebGPU/WebGL2, OpenCL supports 64-bit integers natively across all conformant implementations.
- **Bitcast via `as_type()`**: OpenCL uses `as_int()`, `as_float()`, etc. for reinterpret casts, unlike CUDA's `reinterpret_cast`.
- **Workgroup reduction**: Reduce ops use `__local` shared memory with `barrier(CLK_LOCAL_MEM_FENCE)` for efficient parallel reduction within workgroups, matching the OpenCL memory model.
- **Buffer qualifiers**: All buffers use `__global T* restrict` for maximum optimization opportunities.

### Device Types (feature-gated)

The device module (`device/opencl.rs`) is feature-gated behind `opencl-backend`. It provides type definitions (`OpenClBuffer`, `OpenClProgram`, `OpenClDeviceLimits`) that a future FFI layer will use to interface with the OpenCL runtime. The actual `clCreateContext`/`clEnqueueNDRangeKernel` calls are not yet implemented.

## OCR Engine Strategy

The primitive stack powers two complementary OCR engines:

### PaddleOCR (via molt/tinygrad) — Fast Workhorse

- **99.6% accuracy** on printed invoices/documents (PP-OCRv4 benchmark)
- **~16 MB total**: detector (4.7 MB) + classifier (0.6 MB) + recognizer (10.8 MB)
- Standard CNN/transformer ops: Conv2d, BatchNorm, ReLU, MatMul, Softmax, CTC decode
- All ops decompose to the 26 primitives — no new Rust code needed
- ONNX weights loaded via minimal protobuf parser (zero external deps)
- Runs on: **browser (WebGPU/WASM)**, **Workers edge**, **native**
- Implementation: `src/molt/stdlib/tinygrad/paddleocr.py`
- Models: PP-OCRv4 mobile ONNX from HuggingFace (OleehyO/paddleocrv4.onnx)

ONNX op decomposition to tinygrad primitives:

| ONNX Op | Count (det/rec/cls) | Tinygrad Decomposition |
|---------|---------------------|----------------------|
| Conv | 62/38/53 | `conv2d` (im2col + REDUCE_SUM + MUL) |
| BatchNorm | 3/0/35 | SUB + MUL + SQRT + RECIPROCAL + ADD |
| Relu | 12/0/15 | MAX(x, 0) |
| Sigmoid | 1/7/0 | RECIPROCAL(1 + EXP2(-x * LOG2_E)) |
| HardSigmoid | 10/0/9 | clip(ax+b) via MAX compositions |
| MatMul | 0/13/0 | RESHAPE + EXPAND + MUL + REDUCE_SUM |
| Softmax | 0/1/0 | REDUCE_MAX + SUB + EXP2 + REDUCE_SUM + MUL |
| GlobalAvgPool | 10/0/10 | REDUCE_SUM / spatial_size |
| Resize (2x) | 6/0/0 | RESHAPE + EXPAND (nearest neighbor) |
| ReduceMean | 0/10/0 | REDUCE_SUM / axis_size |

### Falcon-OCR — Heavy Duty VLM

- **300M+ params**, vision-language model (full multimodal understanding)
- Best for: complex/multi-page/creative layouts, handwriting, mixed content
- TurboQuant INT4 quantization for edge deployment
- Runs on: **browser (WebGPU)**, **GPU server (Modal)**, **Workers AI**
- Higher quality on difficult inputs but 10-100x slower than PaddleOCR
- Implementation: `src/molt/stdlib/tinygrad/eagle.py`

### Routing Strategy

```
Input image
    |
    v
[PaddleOCR first-pass] -- 50ms, 99.6% on clean docs
    |
    +-- confidence > 0.9 --> return result (fast path)
    |
    +-- confidence < 0.9 --> [Falcon-OCR fallback] -- 2-5s, handles edge cases
```

## MLIR Dual-Path Rendering

The MLIR serializer (`mlir.rs`) generates MLIR textual IR from FusedKernel. This enables:
- Integration with MLIR-based optimization passes
- Cross-compilation to targets beyond direct GPU backends
- Analysis and verification of kernel structure

The MLIR output maps 1:1 to the 26 primitives using standard MLIR dialects:
- `arith` dialect for arithmetic, comparison, bitwise ops
- `math` dialect for exp2, log2, sin, sqrt, trunc
- Reduce ops map to `linalg.reduce` patterns
